//! The meter on the bar: how much of some allowance is used, as a command reports it. The command
//! (`[meter]` in the settings) runs every so often off the main thread and prints a JSON report of
//! sections, each with a few meters. The bar shows each section's first two as a small gauge, and
//! quick settings shows every one with the time left until it starts over.
//!
//! A command that fails, hangs or prints something unreadable leaves the last good report on show,
//! dimmed, so a network blip doesn't make the meter blink out.

use std::{
    io::Read,
    os::fd::AsRawFd,
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    thread::Thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

/// A command still running after this long is stopped.
const RUN_LIMIT: Duration = Duration::from_secs(30);
/// The most a report is read to.
const OUTPUT_LIMIT: usize = 64 * 1024;
/// The most sections the bar makes room for; quick settings shows them all.
pub const BAR_SECTIONS: usize = 3;

/// What the meter shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub detail: Option<String>,
    pub gauges: Vec<Gauge>,
    /// Why the numbers are what they are, e.g. that they couldn't be refreshed.
    pub note: Option<String>,
    /// The numbers are old: the report says so, or the command has stopped answering.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    pub label: String,
    pub percent: u8,
    /// How long until it starts over, e.g. `2 h 5 min`.
    pub resets_in: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Report {
    #[serde(default)]
    sections: Vec<ReportSection>,
}

#[derive(Debug, Deserialize)]
struct ReportSection {
    name: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    meters: Vec<ReportMeter>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    stale: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ReportMeter {
    label: String,
    percent: f64,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    resets: Option<f64>,
}

const CALM: u32 = 0xcfd6e2ff;
const AMBER: u32 = 0xffb547ff;
/// Nearly used up. Orange rather than red, which the bar keeps for sharing.
const HOT: u32 = 0xff7a45ff;

/// A gauge's colour at `percent` used.
pub fn ink(percent: u8) -> u32 {
    match percent {
        90.. => HOT,
        75.. => AMBER,
        _ => CALM,
    }
}

/// `rgba` faded, for numbers that are old.
pub fn dim(rgba: u32) -> u32 {
    let alpha = (rgba & 0xff) * 45 / 100;
    (rgba & !0xff) | alpha
}

/// The command and how often it runs, as the settings last said.
static WANTED: Mutex<(String, Duration)> = Mutex::new((String::new(), Duration::from_secs(60)));
static READER: OnceLock<Thread> = OnceLock::new();

pub fn start() -> Arc<Mutex<Option<Reading>>> {
    let shared = Arc::new(Mutex::new(None));
    let writer = shared.clone();
    let reader = std::thread::spawn(move || {
        let mut running = (String::new(), Duration::ZERO);
        let mut report: Option<Report> = None;
        let mut failing = false;
        let mut next = Instant::now();
        loop {
            let wanted = WANTED.lock().unwrap().clone();
            if wanted != running {
                if wanted.0 != running.0 {
                    report = None;
                    failing = false;
                }
                running = wanted;
                next = Instant::now();
            }
            if running.0.is_empty() {
                *writer.lock().unwrap() = None;
                std::thread::park();
                continue;
            }
            if Instant::now() >= next {
                match run(&running.0).and_then(|text| parse(&text)) {
                    Some(fresh) => {
                        report = Some(fresh);
                        failing = false;
                    }
                    None => {
                        if !failing {
                            tracing::warn!(
                                command = running.0,
                                "the meter's command gave no report"
                            );
                        }
                        failing = true;
                    }
                }
                next = Instant::now() + running.1;
            }
            let shown = report
                .as_ref()
                .map(|report| reading(report, failing, unix_now()));
            {
                let mut current = writer.lock().unwrap();
                if *current != shown {
                    *current = shown;
                }
            }
            // Wake for the next run, or sooner to count the time left down.
            let wait = next
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(15));
            std::thread::park_timeout(wait);
        }
    });
    let _ = READER.set(reader.thread().clone());
    shared
}

/// Follows the settings: a new command runs straight away, and an empty one takes the meter off.
pub fn configure(meter: &slipstream_config::Meter) {
    let wanted = (meter.command.trim().to_string(), meter.every());
    let mut current = WANTED.lock().unwrap();
    if *current != wanted {
        *current = wanted;
        drop(current);
        if let Some(reader) = READER.get() {
            reader.unpark();
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

fn parse(text: &str) -> Option<Report> {
    match serde_json::from_str(text.trim()) {
        Ok(report) => Some(report),
        Err(err) => {
            tracing::debug!("the meter's report isn't readable: {err}");
            None
        }
    }
}

/// What a report shows at `now`, in seconds since the Unix epoch. `failing` marks every section
/// old.
fn reading(report: &Report, failing: bool, now: i64) -> Reading {
    Reading {
        sections: report
            .sections
            .iter()
            .map(|section| Section {
                name: section.name.clone(),
                detail: section.detail.clone().filter(|detail| !detail.is_empty()),
                gauges: section
                    .meters
                    .iter()
                    .map(|meter| Gauge {
                        label: meter.label.clone(),
                        percent: if meter.percent.is_finite() {
                            meter.percent.round().clamp(0.0, 100.0) as u8
                        } else {
                            0
                        },
                        resets_in: meter
                            .resets
                            .filter(|at| at.is_finite())
                            .map(|at| at as i64 - now)
                            .filter(|left| *left > 0)
                            .map(time_left),
                    })
                    .collect(),
                note: section.note.clone().filter(|note| !note.is_empty()),
                stale: failing || section.stale.unwrap_or(false),
            })
            .collect(),
    }
}

/// `5 d 23 h`, `2 h 5 min`, `12 min`, rounded up so it never says a minute less than is left.
fn time_left(secs: i64) -> String {
    if secs < 60 {
        return "under a minute".into();
    }
    if secs >= 86_400 {
        let (days, hours) = (secs / 86_400, secs % 86_400 / 3600);
        return if hours == 0 {
            format!("{days} d")
        } else {
            format!("{days} d {hours} h")
        };
    }
    let minutes = (secs + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Runs `command` with `sh -c` and returns what it printed, if it succeeded in time.
fn run(command: &str) -> Option<String> {
    let mut child = crate::launch::command("sh")
        .args(["-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| tracing::warn!(command, "couldn't run the meter's command: {err}"))
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read without blocking: something the command left running may hold the pipe open after the
    // command itself is done.
    // SAFETY: fcntl on a descriptor this function owns.
    unsafe {
        let fd = stdout.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let started = Instant::now();
    let mut output = Vec::new();
    let status = loop {
        read_available(&mut stdout, &mut output);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= RUN_LIMIT => {
                tracing::warn!(command, "the meter's command took too long; stopped it");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    };
    read_available(&mut stdout, &mut output);
    status
        .success()
        .then(|| String::from_utf8_lossy(&output).into_owned())
}

fn read_available(out: &mut impl Read, output: &mut Vec<u8>) {
    let mut chunk = [0u8; 4096];
    while output.len() < OUTPUT_LIMIT {
        match out.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => output.extend_from_slice(&chunk[..n]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"{"sections": [
        {"name": "Storage", "detail": "Pro", "note": null, "stale": false, "updated": 1,
         "meters": [{"label": "Daily", "percent": 42.4, "resets": 10000},
                    {"label": "Weekly", "percent": 180, "resets": 1000.5}]},
        {"name": "Mail", "meters": [], "note": "Couldn't reach the server", "stale": true}
    ]}"#;

    #[test]
    fn a_report_reads_with_its_optional_fields_left_out() {
        let report = parse(REPORT).unwrap();
        let shown = reading(&report, false, 2800);
        let storage = &shown.sections[0];
        assert_eq!(storage.detail.as_deref(), Some("Pro"));
        assert!(!storage.stale);
        assert_eq!(
            storage.gauges[0],
            Gauge {
                label: "Daily".into(),
                percent: 42,
                resets_in: Some("2 h".into()),
            }
        );
        assert_eq!(storage.gauges[1].percent, 100, "clamped");
        assert_eq!(storage.gauges[1].resets_in, None, "already past");
        let mail = &shown.sections[1];
        assert!(mail.stale && mail.gauges.is_empty());
        assert_eq!(mail.detail, None);
        assert_eq!(mail.note.as_deref(), Some("Couldn't reach the server"));

        assert!(
            reading(&report, true, 0)
                .sections
                .iter()
                .all(|section| section.stale),
            "a failing command dims everything"
        );
        assert!(parse("not json").is_none());
        assert!(
            parse(r#"{"sections": [{"meters": []}]}"#).is_none(),
            "a name is needed"
        );
        assert_eq!(parse("{}").unwrap().sections.len(), 0);
    }

    #[test]
    fn time_left_reads_naturally_and_never_short() {
        assert_eq!(time_left(30), "under a minute");
        assert_eq!(time_left(61), "2 min");
        assert_eq!(time_left(3600), "1 h");
        assert_eq!(time_left(2 * 3600 + 4 * 60 + 1), "2 h 5 min");
        assert_eq!(time_left(86_400), "1 d");
        assert_eq!(time_left(5 * 86_400 + 23 * 3600 + 59), "5 d 23 h");
    }

    #[test]
    fn colours_warm_as_the_allowance_runs_out() {
        assert_eq!(ink(74), CALM);
        assert_eq!(ink(75), AMBER);
        assert_eq!(ink(90), HOT);
        assert_eq!(dim(0xffb547ff), 0xffb54772);
    }

    #[test]
    fn a_command_is_read_and_a_failing_one_gives_nothing() {
        assert_eq!(run("printf '{}'").as_deref(), Some("{}"));
        assert_eq!(run("printf '{}'; exit 3"), None);
        // Something left running that holds the output open doesn't hold up the reading.
        let started = Instant::now();
        assert_eq!(run("printf ok; sleep 5 & exit 0").as_deref(), Some("ok"));
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}

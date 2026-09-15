//! The bar's meter: the JSON report its command prints, what that report shows, and running the
//! command. The compositor runs it every so often; Settings runs it once to try it out.
//!
//! ```json
//! {"sections": [{"name": "Storage", "detail": "Pro", "note": null, "stale": false,
//!                "meters": [{"label": "Daily", "percent": 42, "resets": 1790000000}]}]}
//! ```

use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

/// A command still running after this long is stopped.
pub const RUN_LIMIT: Duration = Duration::from_secs(30);
/// The most of a command's output that's kept.
const OUTPUT_LIMIT: usize = 64 * 1024;

/// What a report shows.
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

/// A report as the command printed it.
#[derive(Debug, Deserialize)]
pub struct Report {
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

impl Report {
    /// The report in `text`, or why it isn't one.
    pub fn parse(text: &str) -> Result<Report, String> {
        serde_json::from_str(text.trim()).map_err(|err| err.to_string())
    }

    /// What it shows at `now`, in seconds since the Unix epoch. `failing` marks every section old.
    pub fn reading(&self, failing: bool, now: i64) -> Reading {
        Reading {
            sections: self
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
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// `5 d 23 h`, `2 h 5 min`, `12 min`, rounded up so it never says a minute less than is left.
pub fn time_left(secs: i64) -> String {
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

/// How running the command went.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// It succeeded, and printed this.
    Printed(String),
    /// It exited unsuccessfully, with this code (none when a signal ended it) and the last line
    /// it wrote to standard error.
    Failed { code: Option<i32>, error: String },
    /// It was still running at the limit, and was stopped.
    TimedOut,
    /// It couldn't be started at all.
    CouldntStart(String),
}

/// Runs `command`, whose standard input, output and error this sets, stopping it after `limit`.
pub fn run(mut command: Command, limit: Duration) -> Run {
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return Run::CouldntStart(err.to_string()),
    };
    let stdout = collect(child.stdout.take());
    let stderr = collect(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Run::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => return Run::CouldntStart(err.to_string()),
        }
    };
    // Something the command left running can hold its output open after it's gone, so what has
    // arrived shortly after it exits is taken as everything.
    let deadline = Instant::now() + Duration::from_millis(300);
    let output = gather(&stdout, deadline);
    if status.success() {
        return Run::Printed(String::from_utf8_lossy(&output).into_owned());
    }
    let error = gather(&stderr, deadline);
    Run::Failed {
        code: status.code(),
        error: String::from_utf8_lossy(&error)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string(),
    }
}

/// Reads a pipe on a thread of its own, a chunk at a time, up to the output limit.
fn collect(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            let mut kept = 0;
            while kept < OUTPUT_LIMIT {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        kept += n;
                        if sender.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    receiver
}

/// Everything a pipe's thread sends until it finishes or `deadline` passes.
fn gather(chunks: &mpsc::Receiver<Vec<u8>>, deadline: Instant) -> Vec<u8> {
    let mut output = Vec::new();
    while let Ok(chunk) = chunks.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        output.extend_from_slice(&chunk);
    }
    output.truncate(OUTPUT_LIMIT);
    output
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
        let report = Report::parse(REPORT).unwrap();
        let shown = report.reading(false, 2800);
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
            report
                .reading(true, 0)
                .sections
                .iter()
                .all(|section| section.stale),
            "a failing command dims everything"
        );
        assert!(Report::parse("not json").is_err());
        assert!(
            Report::parse(r#"{"sections": [{"meters": []}]}"#)
                .unwrap_err()
                .contains("name"),
            "a name is needed, and the error says so"
        );
        assert_eq!(Report::parse("{}").unwrap().sections.len(), 0);
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

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_run_says_what_was_printed_or_why_it_failed() {
        assert_eq!(run(sh("printf '{}'"), RUN_LIMIT), Run::Printed("{}".into()));
        assert_eq!(
            run(
                sh("echo starting >&2; echo 'no login' >&2; exit 3"),
                RUN_LIMIT
            ),
            Run::Failed {
                code: Some(3),
                error: "no login".into()
            }
        );
        assert_eq!(
            run(sh("sleep 5"), Duration::from_millis(200)),
            Run::TimedOut
        );
        assert!(matches!(
            run(Command::new("/nonexistent/command"), RUN_LIMIT),
            Run::CouldntStart(_)
        ));
        // Something left running that holds the output open doesn't hold up the answer.
        let started = Instant::now();
        assert_eq!(
            run(sh("printf ok; sleep 5 & exit 0"), RUN_LIMIT),
            Run::Printed("ok".into())
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}

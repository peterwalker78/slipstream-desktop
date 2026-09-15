//! The meter on the bar: how much of some allowance is used, as a command reports it. The command
//! (`[meter]` in the settings) runs every so often off the main thread and prints a JSON report of
//! sections, each with a few meters. The bar shows each section's first two as a small gauge, and
//! quick settings shows every one with the time left until it starts over.
//!
//! A command that fails, hangs or prints something unreadable leaves the last good report on show,
//! dimmed, so a network blip doesn't make the meter blink out.

use std::{
    sync::{Arc, Mutex, OnceLock},
    thread::Thread,
    time::{Duration, Instant},
};

use slipstream_config::meter::{RUN_LIMIT, Report, Run, unix_now};
pub use slipstream_config::meter::{Reading, Section};

/// The most sections the bar makes room for; quick settings shows them all.
pub const BAR_SECTIONS: usize = 3;

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
                match read(&running.0) {
                    Ok(fresh) => {
                        report = Some(fresh);
                        failing = false;
                    }
                    Err(why) => {
                        if !failing {
                            tracing::warn!(command = running.0, "the meter has no report: {why}");
                        }
                        failing = true;
                    }
                }
                next = Instant::now() + running.1;
            }
            let shown = report
                .as_ref()
                .map(|report| report.reading(failing, unix_now()));
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

/// Runs the command and reads its report, or says why there isn't one.
fn read(command: &str) -> Result<Report, String> {
    let mut sh = crate::launch::command("sh");
    sh.args(["-c", command]);
    match slipstream_config::meter::run(sh, RUN_LIMIT) {
        Run::Printed(text) => Report::parse(&text).map_err(|err| format!("unreadable: {err}")),
        Run::Failed { code, error } => Err(format!("exit {code:?}: {error}")),
        Run::TimedOut => Err(format!("still running after {} s", RUN_LIMIT.as_secs())),
        Run::CouldntStart(err) => Err(format!("couldn't start: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_warm_as_the_allowance_runs_out() {
        assert_eq!(ink(74), CALM);
        assert_eq!(ink(75), AMBER);
        assert_eq!(ink(90), HOT);
        assert_eq!(dim(0xffb547ff), 0xffb54772);
    }

    #[test]
    fn a_report_is_read_and_a_failure_says_why() {
        assert_eq!(
            read("printf '{}'").unwrap().reading(false, 0),
            Reading::default()
        );
        assert!(
            read("printf nonsense")
                .unwrap_err()
                .starts_with("unreadable")
        );
        assert_eq!(
            read("echo gone >&2; exit 2").unwrap_err(),
            "exit Some(2): gone"
        );
    }
}

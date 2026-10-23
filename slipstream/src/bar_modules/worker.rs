//! One module's thread: it runs the module's command as its `interval` says, reads what the
//! command prints, and publishes what the bar should show. Nothing here runs on the event loop,
//! and nothing waits on it: stopping is a flag the thread looks at every so often, and the thread
//! ends the command's whole process group on its way out.

use std::{
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

use slipstream_config::{
    bar::{self, Interval, Module, Output, ReturnType, Shown},
    command::{self, Lines, Run, Said},
    meter::RUN_LIMIT,
};

/// The least time between two updates the bar is shown, however fast a command prints.
pub const UPDATE_GAP: Duration = Duration::from_millis(250);
/// A continuous command that exits sooner than this after starting is waited on longer each time.
const STEADY: Duration = Duration::from_secs(10);
/// The longest wait before a command that keeps exiting is started again.
const LONGEST_BACKOFF: Duration = Duration::from_secs(300);

/// How a worker is told to stop or to run again.
#[derive(Debug, Default)]
struct Control {
    stop: AtomicBool,
    again: AtomicBool,
}

/// A running worker, and what it last published. Dropping it stops the worker.
pub struct Worker {
    control: Arc<Control>,
    shown: Arc<Mutex<Option<Shown>>>,
    thread: Thread,
}

impl Worker {
    /// Starts `module`'s worker.
    pub fn start(module: Module) -> Worker {
        let control = Arc::new(Control::default());
        let shown = Arc::new(Mutex::new(None));
        let job = Job {
            control: control.clone(),
            shown: shown.clone(),
            label: module.label(),
            module,
            failures: Throttle::default(),
        };
        let thread = std::thread::Builder::new()
            .name("bar module".into())
            .spawn(move || job.go())
            .expect("a thread for a bar module");
        Worker {
            control,
            shown,
            thread: thread.thread().clone(),
        }
    }

    /// What the module shows now, if anything.
    pub fn shown(&self) -> Option<Shown> {
        self.shown.lock().unwrap().clone()
    }

    /// Runs the command again now, for a module that runs every so often or once.
    pub fn again(&self) {
        self.control.again.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }

    fn stop(&self) {
        self.control.stop.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker thread's own state.
struct Job {
    module: Module,
    label: String,
    control: Arc<Control>,
    shown: Arc<Mutex<Option<Shown>>>,
    failures: Throttle,
}

impl Job {
    fn go(mut self) {
        if self.module.exec.trim().is_empty() {
            // A button with no command: its format is all there is to show.
            self.publish(bar::shown(&self.module, &Output::default()));
            return;
        }
        match self.module.interval {
            Interval::Continuous => self.continuous(),
            Interval::Once => self.every(None),
            Interval::Every(secs) => self.every(Some(Duration::from_secs(secs))),
        }
    }

    fn stopped(&self) -> bool {
        self.control.stop.load(Ordering::Relaxed)
    }

    fn publish(&self, shown: Option<Shown>) {
        *self.shown.lock().unwrap() = shown;
    }

    /// Waits up to `wait` (for ever with none), or until told to stop or to run again. True
    /// while the worker should carry on.
    fn rest(&self, wait: Option<Duration>) -> bool {
        let until = wait.map(|wait| Instant::now() + wait);
        while !self.stopped() && !self.control.again.swap(false, Ordering::Relaxed) {
            match until {
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    std::thread::park_timeout(left);
                }
                None => std::thread::park(),
            }
        }
        !self.stopped()
    }

    /// Logs why the module has nothing to show, a few times a minute at most.
    fn failed(&mut self, why: &str) {
        if let Some(dropped) = self.failures.admit(Instant::now()) {
            let module = &self.label;
            if dropped > 0 {
                tracing::warn!("bar module {module}: {why} ({dropped} more not logged)");
            } else {
                tracing::warn!("bar module {module}: {why}");
            }
        }
    }

    /// Whether `exec-if` lets the module show, when it has one.
    fn allowed(&self) -> Option<bool> {
        let check = self.module.exec_if.trim();
        if check.is_empty() {
            return Some(true);
        }
        let ran = command::run_tidily(sh(check), RUN_LIMIT, &|| self.stopped())?;
        Some(matches!(ran, Run::Printed(_)))
    }

    /// A command that runs every `every`, or once with none, and again after a click or scroll.
    fn every(&mut self, every: Option<Duration>) {
        let json = self.module.return_type == ReturnType::Json;
        loop {
            let Some(allowed) = self.allowed() else {
                return;
            };
            if !allowed {
                self.publish(None);
            } else {
                let Some(ran) =
                    command::run_tidily(sh(&self.module.exec), RUN_LIMIT, &|| self.stopped())
                else {
                    return;
                };
                match ran {
                    Run::Printed(printed) => match Output::of_run(&printed, json) {
                        Ok(output) => self.publish(bar::shown(&self.module, &output)),
                        // The last good reading stays up.
                        Err(err) => self.failed(&format!("unreadable output: {err}")),
                    },
                    failed => {
                        self.publish(None);
                        self.failed(&why(&failed));
                    }
                }
            }
            if !self.rest(every) {
                return;
            }
        }
    }

    /// A command that keeps running and prints an update a line, started again whenever it
    /// exits, after a wait that grows while it keeps exiting soon after starting.
    fn continuous(&mut self) {
        let json = self.module.return_type == ReturnType::Json;
        let usual = self.module.restart_after();
        let mut backoff = usual;
        loop {
            let Some(allowed) = self.allowed() else {
                return;
            };
            let started = Instant::now();
            if allowed {
                // Stopping ends the command too, which isn't the command exiting by itself.
                if !self.follow(json) || self.stopped() {
                    return;
                }
            } else {
                self.publish(None);
            }
            backoff = next_backoff(backoff, usual, started.elapsed());
            if allowed {
                self.failed(&format!(
                    "its command exited; starting it again in {:.1} s",
                    backoff.as_secs_f64()
                ));
            }
            if !self.rest(Some(backoff)) {
                return;
            }
        }
    }

    /// Follows one run of a continuous command until it exits. False when told to stop, which
    /// ends the command.
    fn follow(&mut self, json: bool) -> bool {
        let errors = Arc::new(Mutex::new(Throttle::default()));
        let label = self.label.clone();
        let lines = match Lines::start(sh(&self.module.exec), move |line| {
            if let Some(dropped) = errors.lock().unwrap().admit(Instant::now()) {
                let more = if dropped > 0 {
                    format!(" ({dropped} more lines not logged)")
                } else {
                    String::new()
                };
                tracing::info!("bar module {label} says: {line}{more}");
            }
        }) {
            Ok(lines) => lines,
            Err(err) => {
                self.failed(&format!("couldn't start: {err}"));
                return !self.stopped();
            }
        };
        let mut waiting: Option<Option<Shown>> = None;
        let mut last = Instant::now() - UPDATE_GAP;
        loop {
            if self.stopped() {
                return false;
            }
            let wait = if waiting.is_some() {
                (last + UPDATE_GAP).saturating_duration_since(Instant::now())
            } else {
                UPDATE_GAP
            };
            let ended = match lines.next(wait.max(Duration::from_millis(1))) {
                Said::Line(line) => {
                    let output = if json {
                        Output::json(&line)
                    } else {
                        Ok(Output::plain(&line))
                    };
                    match output {
                        Ok(output) => waiting = Some(bar::shown(&self.module, &output)),
                        Err(err) if !line.trim().is_empty() => {
                            self.failed(&format!("unreadable line: {err}"));
                        }
                        Err(_) => {}
                    }
                    false
                }
                Said::Nothing => false,
                Said::Ended => true,
            };
            // Only the newest update counts, and the bar is shown one a quarter of a second at
            // most.
            if let Some(shown) = waiting.take_if(|_| ended || last.elapsed() >= UPDATE_GAP) {
                self.publish(shown);
                last = Instant::now();
            }
            if ended {
                return true;
            }
        }
    }
}

/// The wait before starting a continuous command again: `usual`, doubled each time it ran for
/// less than `STEADY`, up to `LONGEST_BACKOFF` (or `usual`, if that is longer).
fn next_backoff(backoff: Duration, usual: Duration, ran: Duration) -> Duration {
    if ran >= STEADY {
        usual
    } else {
        (backoff * 2).min(LONGEST_BACKOFF.max(usual))
    }
}

fn sh(line: &str) -> Command {
    let mut sh = crate::launch::command("sh");
    sh.args(["-c", line]);
    sh
}

/// Why a run has nothing to show.
fn why(run: &Run) -> String {
    match run {
        Run::Printed(_) => "it ran".into(),
        Run::Failed { code, error } if error.is_empty() => format!("exit {code:?}"),
        Run::Failed { code, error } => format!("exit {code:?}: {error}"),
        Run::TimedOut => format!(
            "still running after {} s, so it was stopped",
            RUN_LIMIT.as_secs()
        ),
        Run::CouldntStart(err) => format!("couldn't start: {err}"),
    }
}

/// Lets a few log lines through a minute, and counts the rest.
#[derive(Debug, Default)]
struct Throttle {
    since: Option<Instant>,
    let_through: u32,
    dropped: u32,
}

const LINES_A_MINUTE: u32 = 5;

impl Throttle {
    /// Whether a line at `now` is logged: with how many were held back since the last one that
    /// was, or `None` to drop it.
    fn admit(&mut self, now: Instant) -> Option<u32> {
        let fresh = self
            .since
            .is_none_or(|since| now.saturating_duration_since(since) >= Duration::from_secs(60));
        if fresh {
            self.since = Some(now);
            self.let_through = 0;
        }
        if self.let_through >= LINES_A_MINUTE {
            self.dropped += 1;
            return None;
        }
        self.let_through += 1;
        Some(std::mem::take(&mut self.dropped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(text: &str) -> Module {
        let table: toml::Table = toml::from_str(text).unwrap();
        Module::from_table(table)
    }

    /// What `worker` shows once it shows `wanted`, or after a few seconds whatever it shows.
    fn shows(worker: &Worker, wanted: Option<&str>) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = worker.shown().map(|shown| shown.text);
            if text.as_deref() == wanted || Instant::now() >= deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn an_interval_module_runs_its_command_and_again_when_asked() {
        let dir = scratch("again");
        let count = dir.join("count");
        let worker = Worker::start(module(&format!(
            "exec = \"echo x >> '{0}'; wc -l < '{0}'\"\ninterval = \"once\"",
            count.display()
        )));
        assert_eq!(shows(&worker, Some("1")).as_deref(), Some("1"));
        worker.again();
        assert_eq!(shows(&worker, Some("2")).as_deref(), Some("2"));
    }

    #[test]
    fn exec_if_hides_the_module_while_it_fails() {
        let worker = Worker::start(module(
            "exec = \"echo on\"\nexec-if = \"false\"\ninterval = 1",
        ));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(worker.shown(), None);
        let shown = Worker::start(module(
            "exec = \"echo on\"\nexec-if = \"true\"\ninterval = 1",
        ));
        assert_eq!(shows(&shown, Some("on")).as_deref(), Some("on"));
    }

    #[test]
    fn a_continuous_module_shows_each_line_and_is_started_again_when_it_exits() {
        let dir = scratch("continuous");
        let runs = dir.join("runs");
        let worker = Worker::start(module(&format!(
            "exec = \"echo run >> '{0}'; echo first; sleep 0.3; echo second\"\n\
             restart-interval = 0.2",
            runs.display()
        )));
        assert_eq!(shows(&worker, Some("second")).as_deref(), Some("second"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&runs)
            .unwrap_or_default()
            .lines()
            .count()
            < 2
        {
            assert!(Instant::now() < deadline, "started again");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_command_that_keeps_exiting_waits_longer_each_time() {
        let usual = Duration::from_secs(1);
        let quick = Duration::from_millis(100);
        let mut backoff = usual;
        let mut waits = Vec::new();
        for _ in 0..12 {
            backoff = next_backoff(backoff, usual, quick);
            waits.push(backoff.as_secs());
        }
        assert_eq!(&waits[..4], [2, 4, 8, 16]);
        assert_eq!(
            *waits.last().unwrap(),
            300,
            "never longer than five minutes"
        );
        assert_eq!(
            next_backoff(backoff, usual, STEADY),
            usual,
            "a steady run starts afresh"
        );
        let long = Duration::from_secs(600);
        assert_eq!(next_backoff(long, long, quick), long);
    }

    #[test]
    fn a_fast_printer_is_shown_a_few_times_a_second_at_most() {
        let worker = Worker::start(module(
            "exec = \"i=0; while [ $i -lt 100000 ]; do i=$((i+1)); echo $i; done; sleep 30\"",
        ));
        let mut seen = std::collections::BTreeSet::new();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(1) {
            if let Some(shown) = worker.shown() {
                seen.insert(shown.text);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!seen.is_empty() && seen.len() <= 5, "{seen:?}");
    }

    #[test]
    fn stopping_a_worker_ends_its_command_and_everything_it_started() {
        let dir = scratch("stop");
        let pid = dir.join("pid");
        let worker = Worker::start(module(&format!(
            "exec = \"sleep 60 & echo $! > '{}'; echo up; wait\"",
            pid.display()
        )));
        assert_eq!(shows(&worker, Some("up")).as_deref(), Some("up"));
        let child: i32 = std::fs::read_to_string(&pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(child) {
            assert!(Instant::now() < deadline, "the sleep was left running");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn logging_is_held_to_a_few_lines_a_minute() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        let admitted: Vec<_> = (0..8).map(|_| throttle.admit(start)).collect();
        assert_eq!(&admitted[..5], [Some(0); 5]);
        assert_eq!(&admitted[5..], [None; 3]);
        assert_eq!(
            throttle.admit(start + Duration::from_secs(61)),
            Some(3),
            "with a count of what was held back"
        );
    }

    fn alive(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(')')?.1.trim_start().chars().next())
            .is_some_and(|state| state != 'Z' && state != 'X')
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "slipstream-bar-modules-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

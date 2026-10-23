//! One plugin's thread: it runs the plugin's program as its manifest says, reads the cards it
//! prints, and publishes what the panel should show. Nothing here runs on the event loop, and
//! nothing waits on it: stopping is a flag the thread looks at every so often, and the thread ends
//! the program's whole process group on its way out.

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
    command::{self, Lines, Run, Said},
    meter::{RUN_LIMIT, unix_now},
    plugins::{self, Card, Plugin},
};

/// The least time between two cards published, however fast a program prints.
pub const UPDATE_GAP: Duration = Duration::from_millis(250);
/// How often a card's times left are counted down between runs.
const RECOUNT: Duration = Duration::from_secs(30);
/// A program that keeps running and exits sooner than this after starting is waited on longer
/// each time.
const STEADY: Duration = Duration::from_secs(10);
/// The wait before a program that kept running is started again after it exits, at first.
const RESTART: Duration = Duration::from_secs(1);
/// The longest wait before a program that keeps exiting is started again.
const LONGEST_BACKOFF: Duration = Duration::from_secs(300);

/// What a plugin shows: its last card, and why it has none or why that card is old.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Published {
    pub card: Option<Card>,
    pub problem: Option<String>,
}

/// Why a rest ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Woken {
    Stop,
    Again,
    Waited,
}

/// How a worker is told to stop or to run again.
#[derive(Debug, Default)]
struct Control {
    stop: AtomicBool,
    again: AtomicBool,
}

/// A running worker, and what it last published. Dropping it stops the worker.
pub struct Worker {
    control: Arc<Control>,
    published: Arc<Mutex<Published>>,
    thread: Thread,
}

impl Worker {
    /// Starts `plugin`'s worker; `hidden` names sections of its cards not to show.
    pub fn start(plugin: Plugin, hidden: Vec<String>) -> Worker {
        let control = Arc::new(Control::default());
        let published = Arc::new(Mutex::new(Published::default()));
        let job = Job {
            control: control.clone(),
            published: published.clone(),
            plugin,
            hidden,
            last: None,
            failures: Throttle::default(),
        };
        let thread = std::thread::Builder::new()
            .name("plugin".into())
            .spawn(move || job.go())
            .expect("a thread for a plugin");
        Worker {
            control,
            published,
            thread: thread.thread().clone(),
        }
    }

    pub fn published(&self) -> Published {
        self.published.lock().unwrap().clone()
    }

    /// What runs the program again now, for a plugin that runs every so often, from any thread.
    pub fn again_handle(&self) -> impl Fn() + Send + 'static {
        let (control, thread) = (self.control.clone(), self.thread.clone());
        move || {
            control.again.store(true, Ordering::Relaxed);
            thread.unpark();
        }
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
    plugin: Plugin,
    hidden: Vec<String>,
    control: Arc<Control>,
    published: Arc<Mutex<Published>>,
    /// The last line that read as a card, read again to count its times left down.
    last: Option<String>,
    failures: Throttle,
}

impl Job {
    fn go(mut self) {
        match self.plugin.manifest.every() {
            Some(every) => self.every(every),
            None => self.continuous(),
        }
    }

    fn stopped(&self) -> bool {
        self.control.stop.load(Ordering::Relaxed)
    }

    /// Publishes the last card read, `stale` when its program has since failed, with `problem`.
    fn publish(&self, stale: bool, problem: Option<String>) {
        let card = self.last.as_ref().and_then(|line| {
            Card::parse(line, stale, unix_now())
                .ok()
                .map(|card| card.without(&self.hidden))
        });
        *self.published.lock().unwrap() = Published { card, problem };
    }

    /// Takes `line` as the new card if it reads as one; says why not otherwise.
    fn read(&mut self, line: &str) -> Result<(), String> {
        Card::parse(line, false, unix_now()).map_err(|err| format!("unreadable card: {err}"))?;
        self.last = Some(line.to_string());
        Ok(())
    }

    /// Waits up to `wait`, or until told to stop or to run again, and says which.
    fn rest(&self, wait: Duration) -> Woken {
        let until = Instant::now() + wait;
        loop {
            if self.stopped() {
                return Woken::Stop;
            }
            if self.control.again.swap(false, Ordering::Relaxed) {
                return Woken::Again;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Woken::Waited;
            }
            std::thread::park_timeout(left);
        }
    }

    /// Logs why the plugin has nothing new to show, a few times a minute at most.
    fn failed(&mut self, why: &str) {
        if let Some(dropped) = self.failures.admit(Instant::now()) {
            let id = &self.plugin.id;
            if dropped > 0 {
                tracing::warn!("plugin {id}: {why} ({dropped} more not logged)");
            } else {
                tracing::warn!("plugin {id}: {why}");
            }
        }
    }

    /// A program that runs every `every`, and again after one of its buttons is pressed. Between
    /// runs its card is read again now and then, so the times left on it count down.
    fn every(&mut self, every: Duration) {
        loop {
            let Some(ran) =
                command::run_tidily(program(&self.plugin, &[]), RUN_LIMIT, &|| self.stopped())
            else {
                return;
            };
            let problem = match ran {
                Run::Printed(printed) => self.read(&printed).err(),
                failed => Some(why(&failed)),
            };
            if let Some(problem) = &problem {
                self.failed(problem);
            }
            let stale = problem.is_some();
            self.publish(stale, problem.clone());
            let next = Instant::now() + every;
            loop {
                let left = next.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match self.rest(left.min(RECOUNT)) {
                    Woken::Stop => return,
                    Woken::Again => break,
                    // Only time to count the card's times left down.
                    Woken::Waited => self.publish(stale, problem.clone()),
                }
            }
        }
    }

    /// A program that keeps running and prints a card a line, started again whenever it exits,
    /// after a wait that grows while it keeps exiting soon after starting.
    fn continuous(&mut self) {
        let mut backoff = RESTART;
        loop {
            let started = Instant::now();
            // Stopping ends the program too, which isn't it exiting by itself.
            if !self.follow() || self.stopped() {
                return;
            }
            backoff = next_backoff(backoff, started.elapsed());
            let problem = format!(
                "its program exited; starting it again in {:.0} s",
                backoff.as_secs_f64().ceil()
            );
            self.failed(&problem);
            self.publish(true, Some(problem));
            if self.rest(backoff) == Woken::Stop {
                return;
            }
        }
    }

    /// Follows one run of a program that keeps running until it exits. False when told to stop,
    /// which ends the program.
    fn follow(&mut self) -> bool {
        let errors = Arc::new(Mutex::new(Throttle::default()));
        let id = self.plugin.id.clone();
        let lines = match Lines::start(program(&self.plugin, &[]), move |line| {
            if let Some(dropped) = errors.lock().unwrap().admit(Instant::now()) {
                let more = if dropped > 0 {
                    format!(" ({dropped} more lines not logged)")
                } else {
                    String::new()
                };
                tracing::info!("plugin {id} says: {line}{more}");
            }
        }) {
            Ok(lines) => lines,
            Err(err) => {
                let problem = format!("couldn't start: {err}");
                self.failed(&problem);
                self.publish(true, Some(problem));
                return !self.stopped();
            }
        };
        let mut waiting = false;
        let mut last = Instant::now() - UPDATE_GAP;
        loop {
            if self.stopped() {
                return false;
            }
            let wait = if waiting {
                (last + UPDATE_GAP).saturating_duration_since(Instant::now())
            } else {
                RECOUNT
            };
            let ended = match lines.next(wait.max(Duration::from_millis(1))) {
                Said::Line(line) if !line.trim().is_empty() => {
                    match self.read(&line) {
                        Ok(()) => waiting = true,
                        Err(why) => self.failed(&why),
                    }
                    false
                }
                Said::Line(_) => false,
                Said::Nothing => {
                    // Quiet a while: the times left on the card still count down.
                    if !waiting && last.elapsed() >= RECOUNT {
                        waiting = true;
                    }
                    false
                }
                Said::Ended => true,
            };
            // Only the newest card counts, and one is shown a quarter of a second at most.
            if waiting && (ended || last.elapsed() >= UPDATE_GAP) {
                self.publish(false, None);
                waiting = false;
                last = Instant::now();
            }
            if ended {
                return true;
            }
        }
    }
}

/// The plugin's program, with `args` after its own words. A plugin's program is run as its
/// manifest writes it, in its folder, from a cleared environment (`plugins::ENVIRONMENT`). The
/// settings file's own command is run with `sh -c` and the session's environment, as it always
/// was, `args` after it as `"$@"`: never pasted into the line, so the shell reads nothing in them.
pub fn program(plugin: &Plugin, args: &[&str]) -> Command {
    let exec = &plugin.manifest.exec;
    if plugin.settings_command {
        let mut sh = crate::launch::command("sh");
        sh.arg("-c")
            .arg(format!("{} \"$@\"", exec.join(" ")))
            .arg(&plugin.id)
            .args(args);
        return sh;
    }
    let program = &exec[0];
    let mut command = if program.contains('/') {
        Command::new(plugin.dir.join(program))
    } else {
        Command::new(program)
    };
    command
        .args(&exec[1..])
        .args(args)
        .current_dir(&plugin.dir)
        .env_clear();
    for name in plugins::ENVIRONMENT {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let state = plugins::state_dir(&plugin.id);
    if let Err(err) = std::fs::create_dir_all(&state) {
        tracing::warn!("plugin {}: no folder for its state: {err}", plugin.id);
    }
    command.env("SLIPSTREAM_PLUGIN_STATE", state);
    for (n, folder) in plugins::read_folders(&plugin.manifest).iter().enumerate() {
        command.env(format!("SLIPSTREAM_PLUGIN_READ_{n}"), folder);
    }
    command
}

/// The wait before starting a program that keeps running again: `RESTART`, doubled each time it
/// ran for less than `STEADY`, up to `LONGEST_BACKOFF`.
fn next_backoff(backoff: Duration, ran: Duration) -> Duration {
    if ran >= STEADY {
        RESTART
    } else {
        (backoff * 2).min(LONGEST_BACKOFF)
    }
}

/// Why a run has nothing new to show.
pub fn why(run: &Run) -> String {
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
            let dropped = std::mem::take(&mut self.dropped);
            self.let_through += 1;
            return Some(dropped);
        }
        if self.let_through < LINES_A_MINUTE {
            self.let_through += 1;
            Some(0)
        } else {
            self.dropped += 1;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_keeps_exiting_waits_longer_each_time() {
        let quick = Duration::from_secs(1);
        assert_eq!(next_backoff(RESTART, quick), RESTART * 2);
        assert_eq!(next_backoff(LONGEST_BACKOFF, quick), LONGEST_BACKOFF);
        assert_eq!(next_backoff(LONGEST_BACKOFF, STEADY), RESTART);
    }

    #[test]
    fn the_log_lets_a_few_lines_a_minute_through() {
        let mut throttle = Throttle::default();
        let now = Instant::now();
        for _ in 0..LINES_A_MINUTE {
            assert!(throttle.admit(now).is_some());
        }
        assert_eq!(throttle.admit(now), None);
        assert_eq!(throttle.admit(now), None);
        assert_eq!(throttle.admit(now + Duration::from_secs(61)), Some(2));
    }
}

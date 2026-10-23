//! Running a command of the person's own, such as the meter's or a bar module's, without letting
//! it hang anything or outlive its welcome. Each starts in a process group of its own, so whatever
//! it starts can be stopped along with it; its output is read on threads of its own, a capped
//! amount of it; and a run has a time limit.
//!
//! Every group started here is remembered until it has been ended or its leader has exited, so
//! the compositor can stop them all on its way out ([`end_all`]).

use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

/// The most of a command's output that's kept from one run.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// The longest line of a command that prints line after line; the rest of a longer one is
/// dropped.
pub const LINE_LIMIT: usize = 4096;

/// How long a group asked to stop has before it is made to.
const GRACE: Duration = Duration::from_millis(500);

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

/// The groups started here that may still have something running in them, by their id.
static OPEN: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// The open groups. A panic elsewhere while they were held leaves them as good as they were, and
/// the way out still needs them, so a poisoned lock is no reason to give up.
fn open_groups() -> std::sync::MutexGuard<'static, Vec<i32>> {
    OPEN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The process group a command was started as the leader of. Its id is the leader's pid.
#[derive(Debug)]
pub struct Group(i32);

impl Group {
    /// Starts `command` as the leader of a new process group.
    fn spawn(command: &mut Command) -> io::Result<(Child, Group)> {
        let child = command.process_group(0).spawn()?;
        let id = child.id() as i32;
        open_groups().push(id);
        Ok((child, Group(id)))
    }

    pub fn id(&self) -> i32 {
        self.0
    }

    /// Sends `signal` to every process in the group. False once there are none.
    fn signal(&self, signal: i32) -> bool {
        // SAFETY: kill takes no pointers; a negative pid names the group.
        unsafe { libc::kill(-self.0, signal) == 0 }
    }

    /// Stops everything in the group: SIGTERM first, then SIGKILL for whatever is still there
    /// after `grace`. The leader is reaped either way.
    fn end(&self, leader: &mut Child, grace: Duration) {
        let deadline = Instant::now() + grace;
        let mut reaped = !self.signal(libc::SIGTERM);
        loop {
            if !reaped {
                reaped = !matches!(leader.try_wait(), Ok(None));
            }
            // A group outlives its leader while anything it started is still running.
            if reaped && !self.signal(0) {
                break;
            }
            if Instant::now() >= deadline {
                self.signal(libc::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = leader.wait();
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        open_groups().retain(|id| *id != self.0);
    }
}

/// Stops every group started here that hasn't been ended yet, for the compositor's way out:
/// nothing a command started is left running without it.
pub fn end_all() {
    let open = std::mem::take(&mut *open_groups());
    for id in &open {
        // SAFETY: as in `Group::signal`.
        unsafe { libc::kill(-id, libc::SIGTERM) };
    }
    if !open.is_empty() {
        std::thread::sleep(Duration::from_millis(200));
    }
    for id in &open {
        // SAFETY: as in `Group::signal`.
        unsafe { libc::kill(-id, libc::SIGKILL) };
    }
}

/// Runs `command`, whose standard input, output and error this sets, stopping it after `limit`.
/// Whatever it leaves running when it exits is left alone.
pub fn run(command: Command, limit: Duration) -> Run {
    run_with(command, limit, &|| false, false).unwrap_or(Run::TimedOut)
}

/// `run`, but `stop` is asked every so often, and once it says so the run is ended at once and
/// there is no answer. Whatever the command leaves running when it exits is ended too.
pub fn run_tidily(command: Command, limit: Duration, stop: &dyn Fn() -> bool) -> Option<Run> {
    run_with(command, limit, stop, true)
}

fn run_with(
    mut command: Command,
    limit: Duration,
    stop: &dyn Fn() -> bool,
    tidy: bool,
) -> Option<Run> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, group) = match Group::spawn(&mut command) {
        Ok(started) => started,
        Err(err) => return Some(Run::CouldntStart(err.to_string())),
    };
    let stdout = collect(child.stdout.take());
    let stderr = collect(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if stop() => {
                group.end(&mut child, Duration::ZERO);
                return None;
            }
            Ok(None) if started.elapsed() >= limit => {
                group.end(&mut child, Duration::ZERO);
                return Some(Run::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                group.end(&mut child, Duration::ZERO);
                return Some(Run::CouldntStart(err.to_string()));
            }
        }
    };
    // Something the command left running can hold its output open after it's gone, so what has
    // arrived shortly after it exits is taken as everything.
    let deadline = Instant::now() + Duration::from_millis(300);
    let output = gather(&stdout, deadline);
    let error = (!status.success()).then(|| gather(&stderr, deadline));
    if tidy {
        group.end(&mut child, GRACE);
    }
    Some(match error {
        None => Run::Printed(String::from_utf8_lossy(&output).into_owned()),
        Some(error) => Run::Failed {
            code: status.code(),
            error: String::from_utf8_lossy(&error)
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_string(),
        },
    })
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

/// A command that keeps running and prints a line whenever it has something new to say, as a
/// bar module's script without an interval does. Dropping it ends its whole group.
pub struct Lines {
    child: Child,
    group: Group,
    lines: mpsc::Receiver<String>,
}

/// What a running command has said since it was last asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    Line(String),
    /// Nothing within the wait.
    Nothing,
    /// Its output has closed: it has exited, or as good as.
    Ended,
}

/// How many lines may wait to be taken before the command is held up writing more.
const WAITING_LINES: usize = 64;

impl Lines {
    /// Starts `command`. Each line it writes to standard error goes to `errors`, on a thread of
    /// its own.
    pub fn start(
        mut command: Command,
        mut errors: impl FnMut(String) + Send + 'static,
    ) -> io::Result<Lines> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, group) = Group::spawn(&mut command)?;
        // Bounded, so a command that prints faster than it is read waits on its pipe rather than
        // filling memory.
        let (sender, lines) = mpsc::sync_channel(WAITING_LINES);
        if let Some(stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                read_lines(stdout, |line| sender.send(line).is_ok());
            });
        }
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                read_lines(stderr, |line| {
                    errors(line);
                    true
                });
            });
        }
        Ok(Lines {
            child,
            group,
            lines,
        })
    }

    pub fn group(&self) -> &Group {
        &self.group
    }

    /// The next line, waiting up to `wait` for it.
    pub fn next(&self, wait: Duration) -> Said {
        match self.lines.recv_timeout(wait) {
            Ok(line) => Said::Line(line),
            Err(mpsc::RecvTimeoutError::Timeout) => Said::Nothing,
            Err(mpsc::RecvTimeoutError::Disconnected) => Said::Ended,
        }
    }
}

impl Drop for Lines {
    fn drop(&mut self) {
        self.group.end(&mut self.child, GRACE);
    }
}

/// Reads `pipe` line by line until it closes or `deliver` says to stop. A line is cut at
/// `LINE_LIMIT` bytes, and the rest of it is dropped.
fn read_lines(mut pipe: impl Read, mut deliver: impl FnMut(String) -> bool) {
    let mut splitter = Splitter::default();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for line in splitter.feed(&chunk[..n]) {
            if !deliver(line) {
                return;
            }
        }
    }
    if let Some(line) = splitter.finish() {
        deliver(line);
    }
}

/// Bytes into lines, each at most `LINE_LIMIT` bytes long.
#[derive(Default)]
struct Splitter {
    line: Vec<u8>,
    /// The line has passed the limit: the rest of it is skipped.
    overlong: bool,
}

impl Splitter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for piece in bytes.split_inclusive(|byte| *byte == b'\n') {
            let (body, ends) = match piece.strip_suffix(b"\n") {
                Some(body) => (body, true),
                None => (piece, false),
            };
            if !self.overlong {
                let room = LINE_LIMIT - self.line.len();
                self.line.extend_from_slice(&body[..body.len().min(room)]);
                self.overlong = body.len() > room;
            }
            if ends {
                lines.push(self.take());
            }
        }
        lines
    }

    /// What's left once the pipe has closed, if anything.
    fn finish(&mut self) -> Option<String> {
        (!self.line.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> String {
        self.overlong = false;
        let line = std::mem::take(&mut self.line);
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        String::from_utf8_lossy(line).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::atomic::AtomicBool};

    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    /// Whether a process with this pid is running (a zombie counts as gone).
    fn alive(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let state = stat.rsplit_once(')')?.1.trim_start().chars().next()?;
                Some(state != 'Z' && state != 'X')
            })
            .unwrap_or(false)
    }

    /// The pid a script wrote to `file`, once it has.
    fn pid_in(file: &Path) -> i32 {
        for _ in 0..200 {
            if let Some(pid) = std::fs::read_to_string(file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no pid in {}", file.display());
    }

    fn gone_soon(pid: i32) -> bool {
        (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            !alive(pid)
        })
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("slipstream-command-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(name);
        let _ = std::fs::remove_file(&file);
        file
    }

    #[test]
    fn a_run_says_what_was_printed_or_why_it_failed() {
        let limit = Duration::from_secs(30);
        assert_eq!(run(sh("printf '{}'"), limit), Run::Printed("{}".into()));
        assert_eq!(
            run(sh("echo starting >&2; echo 'no login' >&2; exit 3"), limit),
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
            run(Command::new("/nonexistent/command"), limit),
            Run::CouldntStart(_)
        ));
        // Something left running that holds the output open doesn't hold up the answer.
        let started = Instant::now();
        assert_eq!(
            run(sh("printf ok; sleep 5 & exit 0"), limit),
            Run::Printed("ok".into())
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_timed_out_run_takes_everything_it_started_with_it() {
        let file = scratch("timed-out");
        let script = format!("sleep 30 & echo $! > '{}'; wait", file.display());
        assert_eq!(run(sh(&script), Duration::from_millis(300)), Run::TimedOut);
        assert!(gone_soon(pid_in(&file)), "the sleep it started was ended");
    }

    #[test]
    fn a_stopped_run_ends_at_once_and_a_tidy_one_leaves_nothing_behind() {
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            });
            let ran = run_tidily(sh("sleep 20"), Duration::from_secs(30), &|| {
                stop.load(std::sync::atomic::Ordering::Relaxed)
            });
            assert_eq!(ran, None);
        });
        assert!(started.elapsed() < Duration::from_secs(5));

        let file = scratch("left-behind");
        let script = format!(
            "sleep 30 >/dev/null & echo $! > '{}'; echo done",
            file.display()
        );
        assert_eq!(
            run_tidily(sh(&script), Duration::from_secs(30), &|| false),
            Some(Run::Printed("done\n".into()))
        );
        assert!(gone_soon(pid_in(&file)), "what it left running was ended");
    }

    #[test]
    fn lines_arrive_one_by_one_and_ending_them_ends_the_whole_group() {
        let file = scratch("lines");
        let script = format!(
            "sleep 30 & echo $! > '{}'; echo one; echo two; while :; do sleep 1; done",
            file.display()
        );
        let lines = Lines::start(sh(&script), |_| {}).unwrap();
        let wait = Duration::from_secs(5);
        assert_eq!(lines.next(wait), Said::Line("one".into()));
        assert_eq!(lines.next(wait), Said::Line("two".into()));
        assert_eq!(lines.next(Duration::from_millis(50)), Said::Nothing);
        let (leader, child) = (lines.group().id(), pid_in(&file));
        drop(lines);
        assert!(!alive(leader) && gone_soon(child), "nothing is orphaned");
    }

    #[test]
    fn a_command_that_ignores_sigterm_is_made_to_stop() {
        let lines = Lines::start(
            sh("trap '' TERM; echo up; while :; do sleep 1; done"),
            |_| {},
        )
        .unwrap();
        assert_eq!(lines.next(Duration::from_secs(5)), Said::Line("up".into()));
        let leader = lines.group().id();
        let started = Instant::now();
        drop(lines);
        assert!(!alive(leader));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn errors_are_handed_on_and_an_exit_ends_the_lines() {
        let (sender, errors) = mpsc::channel();
        let lines = Lines::start(sh("echo oops >&2; echo last"), move |line| {
            let _ = sender.send(line);
        })
        .unwrap();
        let wait = Duration::from_secs(5);
        assert_eq!(lines.next(wait), Said::Line("last".into()));
        assert_eq!(lines.next(wait), Said::Ended);
        assert_eq!(errors.recv_timeout(wait).unwrap(), "oops");
    }

    #[test]
    fn a_line_is_cut_at_the_limit_and_the_rest_of_it_dropped() {
        let mut splitter = Splitter::default();
        let long = vec![b'x'; LINE_LIMIT + 10];
        assert!(splitter.feed(&long).is_empty());
        let lines = splitter.feed(b"more\nnext\r\npart");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), LINE_LIMIT);
        assert_eq!(lines[1], "next");
        assert_eq!(splitter.feed(b"ial\n"), ["partial"]);
        assert_eq!(splitter.feed(b"\xff\xfeend"), Vec::<String>::new());
        assert_eq!(splitter.finish().as_deref(), Some("\u{fffd}\u{fffd}end"));
        assert_eq!(splitter.finish(), None);
    }
}

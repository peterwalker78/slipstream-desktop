//! Bringing an AI agent's session back with the layout.
//!
//! Slipstream has no AI in it, and this does nothing on its own. It is here for people who run
//! an AI coding agent in a terminal, whichever agent that is: the agent says how its session is
//! reopened, and the layout record puts that session back in the window it was in. With no
//! agent installed, or one that leaves no word, a terminal comes back as it always has.
//!
//! The agent says so with a note: a file named for its process id in
//! `$XDG_RUNTIME_DIR/slipstream/resume/`, holding one line, the command that reopens the session
//! it has. Most agents can run a command when a session starts, which is where the note is
//! written from. Slipstream never works a command out from what is running, since a command run
//! again unasked can do anything; it runs the note or nothing.
//!
//! Only a terminal with one window and one shell is read. A process with several windows or
//! several tabs gives no way to say which shell belongs to which window, so those are left as
//! they were.

use std::{
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// A note longer than this isn't a command line.
const LONGEST_NOTE: u64 = 1024;

/// How many processes under one shell are looked at before giving up.
const MOST_PROCESSES: usize = 512;

/// An agent's session in one terminal window, as it asked to be reopened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The command its note gave.
    pub command: String,
    /// The folder the terminal's shell was in, to run it from.
    pub directory: Option<String>,
    /// That shell, if it was an interactive one: the session is reopened in the same, so the
    /// agent has the search path and settings its startup files gave it the first time.
    pub shell: Option<String>,
    /// The agent's search path, so it finds the same tools: a terminal's shell is often given
    /// folders at login that nothing later adds again. Only this of its environment is kept,
    /// which holds no secrets and doesn't go stale from one login to the next.
    pub search_path: Option<String>,
}

/// A search path longer than this is left out rather than written down.
const LONGEST_PATH: usize = 4096;

/// Shells that take `-i -c`, by the name of their program.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "mksh"];

/// Where programs leave their notes, if there is a runtime folder to keep them in.
pub fn notes_dir() -> Option<PathBuf> {
    notes_about("resume")
}

/// The folder for one kind of note an agent leaves, each kind in a folder of its own.
pub fn notes_about(kind: &str) -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime
        .is_absolute()
        .then(|| runtime.join("slipstream").join(kind))
}

/// Makes the notes folders, so an agent has somewhere to write without making them itself. They
/// are this user's alone: a note is a command that will be run.
pub fn make_notes_dir() {
    for dir in [notes_dir(), crate::working::notes_dir()]
        .into_iter()
        .flatten()
    {
        if let Err(err) = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
        {
            tracing::debug!(path = %dir.display(), "couldn't make a folder for agents' notes: {err}");
        }
    }
}

/// Why a window's terminal has no agent's session to bring back, for the log: a window that ought
/// to have come back attached and didn't can then say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// Its process has no shell on a terminal under it: it isn't a terminal.
    NoShell,
    /// Its process has this many windows, and which shell is whose can't be told.
    Windows(usize),
    /// Its process has this many shells, one for each tab, and which is the window's can't be
    /// told.
    Shells(usize),
    /// Nothing under its shell has left a note: a terminal with no agent in it.
    NoNote,
    /// This many notes were left under its shell as near to it as each other, so none is chosen.
    Notes(usize),
    /// A process under its shell has a note that isn't taken: written before the process started
    /// (by whatever had its number before), or not one plain line in a file of this user's.
    Refused(u32),
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Miss::NoShell => write!(f, "no shell runs in it"),
            Miss::Windows(n) => write!(f, "its process has {n} windows"),
            Miss::Shells(n) => write!(f, "its process has {n} shells"),
            Miss::NoNote => write!(f, "nothing under its shell left a note"),
            Miss::Notes(n) => write!(f, "{n} notes under its shell, none nearer it than the rest"),
            Miss::Refused(pid) => write!(f, "the note of process {pid} is stale or malformed"),
        }
    }
}

/// The session to reopen in the terminal whose process is `pid`, if that process is a terminal
/// with exactly one shell and something under the shell has left a note; or why not.
pub fn in_terminal(pid: u32) -> Result<Session, Miss> {
    let Some(notes) = notes_dir() else {
        return Err(Miss::NoNote);
    };
    let (uid, ticks) = kernel_values();
    look(Path::new("/proc"), &notes, pid, uid, ticks)
}

/// This user's id and the kernel's clock ticks a second.
fn kernel_values() -> (u32, u64) {
    // SAFETY: both only read values the kernel holds for this process.
    let (uid, ticks) = unsafe { (libc::getuid(), libc::sysconf(libc::_SC_CLK_TCK)) };
    (uid, ticks.max(1) as u64)
}

/// `in_terminal` over any process table and notes folder, so it can be tested on made-up ones.
fn look(proc: &Path, notes: &Path, pid: u32, uid: u32, ticks: u64) -> Result<Session, Miss> {
    let shells: Vec<u32> = children(proc, pid)
        .into_iter()
        .filter(|&child| leads_a_terminal(proc, child))
        .collect();
    let shell = match shells[..] {
        [shell] => shell,
        [] => return Err(Miss::NoShell),
        _ => return Err(Miss::Shells(shells.len())),
    };
    let mut refused = None;
    let mut left = Vec::new();
    for (depth, process) in tree(proc, shell) {
        match note(proc, notes, process, uid, ticks) {
            Some(command) => left.push((depth, process, command)),
            None if refused.is_none() && notes.join(process.to_string()).exists() => {
                refused = Some(process);
            }
            None => {}
        }
    }
    // The note nearest the shell is the window's: one further down was left by an agent that one
    // started for itself, which isn't what the window shows. Two as near as each other can't both
    // have the window, so neither is guessed at.
    let Some(nearest) = left.iter().map(|(depth, ..)| *depth).min() else {
        return Err(refused.map_or(Miss::NoNote, Miss::Refused));
    };
    left.retain(|(depth, ..)| *depth == nearest);
    let [(_, agent, command)] = &left[..] else {
        return Err(Miss::Notes(left.len()));
    };
    let directory = fs::read_link(proc.join(shell.to_string()).join("cwd"))
        .ok()
        .and_then(|dir| dir.to_str().map(String::from))
        // A folder deleted from under the shell reads back with a suffix in place of a path.
        .filter(|dir| dir.starts_with('/') && !dir.ends_with(" (deleted)"));
    Ok(Session {
        command: command.clone(),
        directory,
        shell: interactive_shell(proc, shell),
        search_path: search_path(proc, *agent),
    })
}

/// Removes the notes in `notes` whose process has gone, or that were written before their
/// process started and so were left by whatever had its number before. The runtime folder
/// outlives a login whenever something of the person's keeps running, so without this the notes
/// of every earlier session gather there.
pub fn sweep(proc: &Path, notes: &Path, uid: u32, ticks: u64) {
    let Ok(dir) = fs::read_dir(notes) else {
        return;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_file() || meta.uid() != uid {
            continue;
        }
        let written = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs());
        let current = started(proc, pid, ticks)
            .is_some_and(|start| written.is_some_and(|written| written + 2 >= start));
        if !current {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Tidies the resume notes, as `sweep` says.
pub fn sweep_notes() {
    if let Some(notes) = notes_dir() {
        let (uid, ticks) = kernel_values();
        sweep(Path::new("/proc"), &notes, uid, ticks);
    }
}

/// `PATH` as `pid` was started with it, and nothing else from its environment.
fn search_path(proc: &Path, pid: u32) -> Option<String> {
    let environment = fs::read(proc.join(pid.to_string()).join("environ")).ok()?;
    let path = environment
        .split(|&byte| byte == 0)
        .find_map(|entry| entry.strip_prefix(b"PATH="))?;
    let path = std::str::from_utf8(path).ok()?;
    (!path.is_empty() && path.len() <= LONGEST_PATH && !path.chars().any(char::is_control))
        .then(|| path.to_string())
}

/// The program of `pid`, if it is a shell at a prompt rather than one running a command for
/// something else. A shell started with `-c` and no `-i` never read its interactive startup
/// file, and starting one that does in its place could behave quite differently.
fn interactive_shell(proc: &Path, pid: u32) -> Option<String> {
    let dir = proc.join(pid.to_string());
    let program = fs::read_link(dir.join("exe")).ok()?;
    let name = program.file_name()?.to_str()?;
    if !SHELLS.contains(&name) {
        return None;
    }
    let line = fs::read_to_string(dir.join("cmdline")).ok()?;
    let flags = |letter: char| {
        line.split('\0')
            .skip(1)
            .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains(letter))
    };
    (!flags('c') || flags('i'))
        .then(|| program.to_str().map(String::from))
        .flatten()
}

/// The fields of `/proc/<pid>/stat` after the command name, which can itself hold spaces and
/// brackets: state, parent, process group, session, terminal and so on.
fn stat(proc: &Path, pid: u32) -> Option<Vec<String>> {
    let text = fs::read_to_string(proc.join(pid.to_string()).join("stat")).ok()?;
    Some(
        text[text.rfind(')')? + 1..]
            .split_whitespace()
            .map(String::from)
            .collect(),
    )
}

/// Whether `pid` is a shell in the sense that matters here: the leader of its own session, with
/// a terminal. That is what a terminal emulator starts for each window or tab, whatever the
/// program is called.
fn leads_a_terminal(proc: &Path, pid: u32) -> bool {
    let Some(fields) = stat(proc, pid) else {
        return false;
    };
    let number = |at: usize| fields.get(at).and_then(|field| field.parse::<i64>().ok());
    number(3) == Some(i64::from(pid)) && number(4).is_some_and(|terminal| terminal != 0)
}

fn children(proc: &Path, pid: u32) -> Vec<u32> {
    let tasks = fs::read_dir(proc.join(pid.to_string()).join("task"));
    tasks
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|task| fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// `pid` and everything started beneath it, each with how far below `pid` it is, nearest first.
fn tree(proc: &Path, pid: u32) -> Vec<(usize, u32)> {
    let mut tree = vec![(0, pid)];
    let mut at = 0;
    while at < tree.len() && tree.len() < MOST_PROCESSES {
        let (depth, process) = tree[at];
        let below = children(proc, process);
        tree.extend(below.into_iter().map(|child| (depth + 1, child)));
        at += 1;
    }
    tree
}

/// The note `pid` left, if it is this user's own file, one sane line, and written since `pid`
/// started. Process ids come round again, and a note outliving its writer mustn't be taken for
/// the word of whatever has the number now.
fn note(proc: &Path, notes: &Path, pid: u32, uid: u32, ticks: u64) -> Option<String> {
    // Never through a link and never waiting on a FIFO, and the checks are on the file opened, so
    // nothing swapped in after them is read.
    let file = fs::File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(notes.join(pid.to_string()))
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.uid() != uid || meta.len() > LONGEST_NOTE {
        return None;
    }
    let written = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    // The start is known to the clock tick and the boot time to the second, so allow for both.
    if written.as_secs() + 2 < started(proc, pid, ticks)? {
        return None;
    }
    // One byte past the limit, to tell a note that grew since it was measured.
    let mut text = String::new();
    file.take(LONGEST_NOTE + 1).read_to_string(&mut text).ok()?;
    let line = text.trim();
    (text.len() as u64 <= LONGEST_NOTE && !line.is_empty() && !line.chars().any(char::is_control))
        .then(|| line.to_string())
}

/// When `pid` started, in seconds since the epoch: the boot time plus how long after it.
pub fn started(proc: &Path, pid: u32, ticks: u64) -> Option<u64> {
    let boot: u64 = fs::read_to_string(proc.join("stat"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let after: u64 = stat(proc, pid)?.get(19)?.parse().ok()?;
    Some(boot + after / ticks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Made-up boot time; processes below start a hundred seconds after it.
    const BOOT: u64 = 1_000_000;
    const TICKS: u64 = 100;

    /// A process table and a notes folder in a scratch folder.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(name: &str) -> Self {
            let root = crate::files::test_scratch(name);
            fs::create_dir_all(root.join("proc")).unwrap();
            fs::create_dir_all(root.join("notes")).unwrap();
            fs::write(root.join("proc/stat"), format!("cpu 1 2 3\nbtime {BOOT}\n")).unwrap();
            Self { root }
        }

        /// A process under `parent`. A `shell` leads its own session on a terminal.
        fn process(&self, pid: u32, parent: u32, shell: bool, cwd: &str) {
            self.process_at(pid, parent, shell, cwd, 100);
        }

        /// `process`, started `after` seconds after boot.
        fn process_at(&self, pid: u32, parent: u32, shell: bool, cwd: &str, after: u64) {
            let dir = self.root.join(format!("proc/{pid}"));
            fs::create_dir_all(dir.join(format!("task/{pid}"))).unwrap();
            fs::write(dir.join(format!("task/{pid}/children")), "").unwrap();
            let (session, terminal) = if shell { (pid, 34816) } else { (parent, 0) };
            let fields = format!(
                "S {parent} {pid} {session} {terminal} -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {} 0 0",
                after * TICKS
            );
            fs::write(dir.join("stat"), format!("{pid} (a (odd) name) {fields}\n")).unwrap();
            symlink(cwd, dir.join("cwd")).unwrap();
            let listed = self
                .root
                .join(format!("proc/{parent}/task/{parent}/children"));
            if let Ok(text) = fs::read_to_string(&listed) {
                fs::write(listed, format!("{text}{pid} ")).unwrap();
            }
        }

        /// What `pid` is running, and how it was started.
        fn program(&self, pid: u32, exe: &str, args: &[&str]) {
            let dir = self.root.join(format!("proc/{pid}"));
            let _ = fs::remove_file(dir.join("exe"));
            symlink(exe, dir.join("exe")).unwrap();
            fs::write(dir.join("cmdline"), args.join("\0") + "\0").unwrap();
        }

        /// The environment `pid` was started with.
        fn environment(&self, pid: u32, entries: &[&str]) {
            let path = self.root.join(format!("proc/{pid}/environ"));
            fs::write(path, entries.join("\0") + "\0").unwrap();
        }

        /// `pid` leaves `text`, `after` seconds after boot.
        fn note(&self, pid: u32, text: &str, after: u64) {
            let path = self.root.join(format!("notes/{pid}"));
            fs::write(&path, text).unwrap();
            let when = UNIX_EPOCH + std::time::Duration::from_secs(BOOT + after);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }

        fn look(&self, pid: u32) -> Option<Session> {
            self.why(pid).ok()
        }

        fn why(&self, pid: u32) -> Result<Session, Miss> {
            // SAFETY: reads this process's own user id.
            let uid = unsafe { libc::getuid() };
            look(
                &self.root.join("proc"),
                &self.root.join("notes"),
                pid,
                uid,
                TICKS,
            )
        }

        /// Every process ends, as at a logout; the notes folder stays, as the runtime folder
        /// does while anything of the person's keeps running.
        fn log_out(&self) {
            for entry in fs::read_dir(self.root.join("proc")).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    fs::remove_dir_all(entry.path()).unwrap();
                }
            }
        }

        fn sweep(&self) {
            // SAFETY: reads this process's own user id.
            let uid = unsafe { libc::getuid() };
            sweep(
                &self.root.join("proc"),
                &self.root.join("notes"),
                uid,
                TICKS,
            );
        }

        fn notes(&self) -> Vec<String> {
            let mut notes: Vec<String> = fs::read_dir(self.root.join("notes"))
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            notes.sort();
            notes
        }
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A terminal (10) with one shell (11) in `/work/site`, running an agent (12).
    fn one_terminal(name: &str) -> Machine {
        let machine = Machine::new(name);
        machine.process(10, 1, false, "/");
        machine.process(11, 10, true, "/work/site");
        machine.program(11, "/usr/bin/zsh", &["zsh"]);
        machine.process(12, 11, false, "/work/site");
        machine
    }

    fn command(machine: &Machine) -> Option<String> {
        machine.look(10).map(|session| session.command)
    }

    #[test]
    fn with_no_agent_leaving_a_note_nothing_is_resumed() {
        let machine = one_terminal("resume-nothing");
        assert_eq!(
            machine.look(10),
            None,
            "nothing is guessed from what is running"
        );
    }

    #[test]
    fn a_session_comes_back_by_its_note_in_its_shells_folder() {
        let machine = one_terminal("resume-note");
        machine.note(12, "agent --resume 7\n", 200);
        assert_eq!(
            machine.look(10),
            Some(Session {
                command: "agent --resume 7".into(),
                directory: Some("/work/site".into()),
                shell: Some("/usr/bin/zsh".into()),
                search_path: None,
            })
        );
    }

    #[test]
    fn only_the_search_path_is_kept_of_an_agents_environment() {
        let machine = one_terminal("resume-path");
        machine.note(12, "agent --resume 7", 200);
        machine.environment(
            12,
            &[
                "HOME=/home/sam",
                "API_TOKEN=secret",
                "PATH=/opt/tools/bin:/usr/bin",
                "LANG=C",
            ],
        );
        let session = machine.look(10).unwrap();
        assert_eq!(
            session.search_path.as_deref(),
            Some("/opt/tools/bin:/usr/bin")
        );
        assert!(!format!("{session:?}").contains("secret"));
        // The shell's own path is not the agent's.
        machine.environment(11, &["PATH=/usr/bin"]);
        machine.environment(12, &["HOME=/home/sam"]);
        assert_eq!(machine.look(10).unwrap().search_path, None);
    }

    #[test]
    fn the_session_is_reopened_in_the_shell_it_was_typed_into() {
        let machine = one_terminal("resume-shell");
        machine.note(12, "agent --resume 7", 200);
        let shell = |machine: &Machine| machine.look(10).and_then(|session| session.shell);
        assert_eq!(shell(&machine).as_deref(), Some("/usr/bin/zsh"));
        // Started to run one command, it never was the shell anyone typed into.
        machine.program(
            11,
            "/usr/bin/bash",
            &["/bin/bash", "-c", "agent --resume 7"],
        );
        assert_eq!(shell(&machine), None);
        // Unless it was started as an interactive one all the same.
        machine.program(11, "/usr/bin/zsh", &["zsh", "-i", "-c", "agent --resume 7"]);
        assert_eq!(shell(&machine).as_deref(), Some("/usr/bin/zsh"));
        machine.program(11, "/usr/bin/zsh", &["zsh", "-ic", "agent --resume 7"]);
        assert_eq!(shell(&machine).as_deref(), Some("/usr/bin/zsh"));
        // A program that isn't a shell is never started as though it were one.
        machine.program(11, "/usr/bin/python3", &["python3"]);
        assert_eq!(shell(&machine), None);
        assert_eq!(
            machine.look(10).map(|session| session.command).as_deref(),
            Some("agent --resume 7")
        );
    }

    #[test]
    fn a_note_older_than_its_process_is_someone_elses() {
        let machine = one_terminal("resume-stale");
        // Written fifty seconds after boot, by whatever had the number before this process
        // started at a hundred.
        machine.note(12, "agent --resume 7", 50);
        assert_eq!(machine.look(10), None);
    }

    #[test]
    fn a_note_is_one_plain_line_in_a_plain_file() {
        let machine = one_terminal("resume-plain");
        machine.note(12, "agent\u{1b}[2J --resume 7", 200);
        assert_eq!(command(&machine), None, "control characters");
        machine.note(12, "first\nsecond", 200);
        assert_eq!(command(&machine), None, "two lines");
        machine.note(12, &"x".repeat(LONGEST_NOTE as usize + 1), 200);
        assert_eq!(command(&machine), None, "too long");
        machine.note(12, "   \n", 200);
        assert_eq!(command(&machine), None, "empty");
        fs::remove_file(machine.root.join("notes/12")).unwrap();
        machine.note(99, "agent --resume 7", 200);
        symlink(machine.root.join("notes/99"), machine.root.join("notes/12")).unwrap();
        assert_eq!(command(&machine), None, "a link to another file");
    }

    #[test]
    fn two_shells_in_one_terminal_are_left_alone() {
        let machine = one_terminal("resume-tabs");
        machine.note(12, "agent --resume 7", 200);
        machine.process(13, 10, true, "/work/other");
        assert_eq!(machine.look(10), None);
    }

    #[test]
    fn two_notes_under_one_shell_resume_neither() {
        let machine = one_terminal("resume-two");
        machine.process(14, 11, false, "/work/site");
        machine.note(12, "agent --resume 7", 200);
        machine.note(14, "agent --resume 8", 200);
        assert_eq!(machine.why(10), Err(Miss::Notes(2)));
    }

    #[test]
    fn the_note_nearest_the_shell_is_the_windows() {
        let machine = one_terminal("resume-nested");
        machine.note(12, "agent attach 7", 200);
        // An agent the window's agent started for itself, further down.
        machine.process(13, 12, false, "/work/site");
        machine.note(13, "agent --resume 99", 200);
        assert_eq!(command(&machine).as_deref(), Some("agent attach 7"));
    }

    #[test]
    fn a_terminal_that_isnt_brought_back_says_why() {
        let machine = one_terminal("resume-why");
        assert_eq!(machine.why(10), Err(Miss::NoNote));
        machine.note(12, "agent --resume 7", 50);
        assert_eq!(
            machine.why(10),
            Err(Miss::Refused(12)),
            "older than its process"
        );
        machine.process(13, 10, true, "/work/other");
        assert_eq!(machine.why(10), Err(Miss::Shells(2)));
        assert_eq!(machine.why(4242), Err(Miss::NoShell));
    }

    #[test]
    fn notes_whose_process_has_gone_or_came_before_it_are_swept() {
        let machine = one_terminal("resume-sweep");
        machine.note(12, "agent --resume 7", 200);
        machine.note(11, "agent --resume 6", 50);
        machine.note(77, "agent --resume 5", 200);
        fs::write(machine.root.join("notes/not-a-process"), "x").unwrap();
        machine.sweep();
        assert_eq!(machine.notes(), ["12", "not-a-process"]);
    }

    #[test]
    fn a_terminal_comes_back_with_its_session_after_any_number_of_logouts() {
        let machine = Machine::new("resume-cycles");
        for login in 0..12u32 {
            let start = 1000 * u64::from(login) + 1000;
            // Process numbers come round again: every other login, each terminal's agent has the
            // number the other's had, and the plain terminal's shell has one that has a note
            // left from before it started.
            let (a, b) = if login % 2 == 0 { (12, 22) } else { (22, 12) };
            let plain_shell = if login % 2 == 0 { 32 } else { 132 };
            machine.process_at(10, 1, false, "/", start);
            machine.process_at(11, 10, true, "/work/a", start);
            machine.process_at(a, 11, false, "/work/a", start);
            machine.process_at(20, 1, false, "/", start);
            machine.process_at(21, 20, true, "/work/b", start);
            machine.process_at(b, 21, false, "/work/b", start);
            machine.process_at(30, 1, false, "/", start);
            machine.process_at(plain_shell, 30, true, "/work/c", start);
            machine.note(a, "agent attach a", start + 1);
            machine.note(b, "agent attach b", start + 1);
            machine.note(132, "agent attach gone", start - 500);
            if login % 3 == 0 {
                // The window's agent started one of its own further down.
                machine.process_at(40, a, false, "/work/a", start + 5);
                machine.note(40, "agent --resume inner", start + 6);
            }
            assert_eq!(
                command(&machine).as_deref(),
                Some("agent attach a"),
                "login {login}"
            );
            let b_came_back = machine.look(20).map(|session| session.command);
            assert_eq!(
                b_came_back.as_deref(),
                Some("agent attach b"),
                "login {login}"
            );
            assert!(machine.look(30).is_none(), "login {login}");
            machine.sweep();
            assert!(
                machine.notes().len() <= 3,
                "login {login}: {:?}",
                machine.notes()
            );
            machine.log_out();
        }
    }

    #[test]
    fn an_app_with_no_shell_has_nothing_to_resume() {
        let machine = Machine::new("resume-none");
        machine.process(10, 1, false, "/");
        machine.process(11, 10, false, "/work/site");
        machine.note(11, "agent --resume 7", 200);
        assert_eq!(machine.look(10), None);
        assert_eq!(machine.look(4242), None, "no such process");
    }

    #[test]
    fn a_deleted_folder_is_no_folder() {
        let machine = Machine::new("resume-deleted");
        machine.process(10, 1, false, "/");
        machine.process(11, 10, true, "/work/gone (deleted)");
        machine.note(11, "agent --resume 7", 200);
        assert_eq!(machine.look(10).and_then(|session| session.directory), None);
    }
}

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
    os::unix::fs::MetadataExt,
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
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime
        .is_absolute()
        .then(|| runtime.join("slipstream").join("resume"))
}

/// Makes the notes folder, so an agent has somewhere to write without making it itself.
pub fn make_notes_dir() {
    let Some(dir) = notes_dir() else {
        return;
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        tracing::debug!(path = %dir.display(), "couldn't make the folder for resume notes: {err}");
    }
}

/// The session to reopen in the terminal whose process is `pid`. Nothing, unless that process
/// is a terminal with exactly one shell and something under the shell has left a note.
pub fn in_terminal(pid: u32) -> Option<Session> {
    let notes = notes_dir()?;
    // SAFETY: both only read values the kernel holds for this process.
    let (uid, ticks) = unsafe { (libc::getuid(), libc::sysconf(libc::_SC_CLK_TCK)) };
    look(Path::new("/proc"), &notes, pid, uid, ticks.max(1) as u64)
}

/// `in_terminal` over any process table and notes folder, so it can be tested on made-up ones.
fn look(proc: &Path, notes: &Path, pid: u32, uid: u32, ticks: u64) -> Option<Session> {
    let mut shells = children(proc, pid)
        .into_iter()
        .filter(|&child| leads_a_terminal(proc, child));
    let (Some(shell), None) = (shells.next(), shells.next()) else {
        return None;
    };
    let mut left = tree(proc, shell).into_iter().filter_map(|process| {
        note(proc, notes, process, uid, ticks).map(|command| (process, command))
    });
    // Two sessions under one shell can't both have the window, so neither is guessed at.
    let (Some((agent, command)), None) = (left.next(), left.next()) else {
        return None;
    };
    let directory = fs::read_link(proc.join(shell.to_string()).join("cwd"))
        .ok()
        .and_then(|dir| dir.to_str().map(String::from))
        // A folder deleted from under the shell reads back with a suffix in place of a path.
        .filter(|dir| dir.starts_with('/') && !dir.ends_with(" (deleted)"));
    Some(Session {
        command,
        directory,
        shell: interactive_shell(proc, shell),
        search_path: search_path(proc, agent),
    })
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

/// `pid` and everything started beneath it.
fn tree(proc: &Path, pid: u32) -> Vec<u32> {
    let mut tree = vec![pid];
    let mut at = 0;
    while at < tree.len() && tree.len() < MOST_PROCESSES {
        let below = children(proc, tree[at]);
        tree.extend(below);
        at += 1;
    }
    tree
}

/// The note `pid` left, if it is this user's own file, one sane line, and written since `pid`
/// started. Process ids come round again, and a note outliving its writer mustn't be taken for
/// the word of whatever has the number now.
fn note(proc: &Path, notes: &Path, pid: u32, uid: u32, ticks: u64) -> Option<String> {
    let path = notes.join(pid.to_string());
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.uid() != uid || meta.len() > LONGEST_NOTE {
        return None;
    }
    let written = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    // The start is known to the clock tick and the boot time to the second, so allow for both.
    if written.as_secs() + 2 < started(proc, pid, ticks)? {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    let line = text.trim();
    (!line.is_empty() && !line.chars().any(char::is_control)).then(|| line.to_string())
}

/// When `pid` started, in seconds since the epoch: the boot time plus how long after it.
fn started(proc: &Path, pid: u32, ticks: u64) -> Option<u64> {
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
            let dir = self.root.join(format!("proc/{pid}"));
            fs::create_dir_all(dir.join(format!("task/{pid}"))).unwrap();
            fs::write(dir.join(format!("task/{pid}/children")), "").unwrap();
            let (session, terminal) = if shell { (pid, 34816) } else { (parent, 0) };
            let fields = format!(
                "S {parent} {pid} {session} {terminal} -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {} 0 0",
                100 * TICKS
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
        assert_eq!(machine.look(10), None);
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

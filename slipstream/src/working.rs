//! Which AI agents are running, for the wallpaper to show once the desktop has faded.
//!
//! Slipstream has no AI in it, and this does nothing on its own. It is here for people who run
//! an AI coding agent, whichever agent that is, and step away: the faded desktop shows a readout
//! for each open agent session (`scope.rs`). With no agent installed, or one that leaves no word,
//! the wallpaper is as it always was.
//!
//! The agent says so with a note: a file named for its process id in
//! `$XDG_RUNTIME_DIR/slipstream/working/`, kept there while the session is open. The first line
//! is what to call the agent on screen: its own name and what it is working on, say. An empty
//! first line falls back to the folder the agent is working in. The second line says what the
//! agent is doing: `idle` means it is between tasks, and anything else, or no second line, that
//! it is working. Each line after that is a helper the agent has working for it, by its task.
//! Only the agent itself leaves a note, so its helpers are listed on its readout rather than each
//! getting one of their own.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::resume;

/// More notes than this are more than the screen has room to show.
const MOST_NOTES: usize = 64;

/// A note longer than this isn't a name, and a name is cut to this many characters.
const LONGEST_NOTE: u64 = 1024;
const LONGEST_NAME: usize = 80;
/// More helpers than this are more than one readout has room to list.
const MOST_HELPERS: usize = 8;

/// What a note's second line says the agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Working, itself or through its helpers: any second line but `idle`.
    Active,
    /// Open between tasks: second line `idle`.
    Idle,
}

/// An agent with an open session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pid: u32,
    /// What to call it: what its note says, or the folder it is working in.
    pub name: String,
    pub status: AgentStatus,
    /// What each helper it has working is doing.
    pub helpers: Vec<String>,
}

/// Where agents leave these notes, if there is a runtime folder to keep them in.
pub fn notes_dir() -> Option<PathBuf> {
    resume::notes_about("working")
}

/// Removes notes that belong to processes that are no longer running, to keep the folder tidy
/// across a long session.
fn clean_stale(notes: &Path, uid: u32, ticks: u64) {
    let Ok(dir) = fs::read_dir(notes) else {
        return;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
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
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let started = resume::started(Path::new("/proc"), pid, ticks);
        let alive = started.is_some_and(|s| written.is_some_and(|w| w + 2 >= s));
        if !alive {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Every agent with an open session now.
pub fn at_work() -> Vec<Agent> {
    let Some(notes) = notes_dir() else {
        return Vec::new();
    };
    // SAFETY: both only read values the kernel holds for this process.
    let (uid, ticks) = unsafe { (libc::getuid(), libc::sysconf(libc::_SC_CLK_TCK)) };
    let agents = read(Path::new("/proc"), &notes, uid, ticks.max(1) as u64);
    clean_stale(&notes, uid, ticks.max(1) as u64);
    agents
}

/// `at_work` over any process table and notes folder, so it can be tested on made-up ones.
fn read(proc: &Path, notes: &Path, uid: u32, ticks: u64) -> Vec<Agent> {
    let mut agents: Vec<Agent> = fs::read_dir(notes)
        .into_iter()
        .flatten()
        .flatten()
        .take(MOST_NOTES)
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let meta = fs::symlink_metadata(entry.path()).ok()?;
            if !meta.is_file() || meta.uid() != uid {
                return None;
            }
            // Process ids come round again, and a note outliving its writer mustn't be taken for
            // the word of whatever has the number now. A process that has gone has no start.
            let written = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            if written.as_secs() + 2 < resume::started(proc, pid, ticks)? {
                return None;
            }
            let (note_name, status, helpers) = parse_note(&entry.path(), meta.len());
            let name = note_name.unwrap_or_else(|| folder(proc, pid));
            Some(Agent {
                pid,
                name,
                status,
                helpers,
            })
        })
        .collect();
    agents.sort_by_key(|agent| agent.pid);
    agents
}

/// What the note at `path`, `len` bytes long, says about its agent: what to call it (its first
/// line, if that is plain text), what the agent is doing (from the second line) and what its
/// helpers are doing (the plain lines after).
fn parse_note(path: &Path, len: u64) -> (Option<String>, AgentStatus, Vec<String>) {
    if len == 0 || len > LONGEST_NOTE {
        return (None, AgentStatus::Active, Vec::new());
    }
    // Read without blocking and only if still a small regular file: the note can be swapped for
    // a FIFO between the look and the read, and this runs on the event loop.
    let Some(text) = crate::files::read_small_text(path, LONGEST_NOTE) else {
        return (None, AgentStatus::Active, Vec::new());
    };
    let plain = |line: &str| {
        let line = line.trim();
        (!line.is_empty() && !line.chars().any(char::is_control))
            .then(|| line.chars().take(LONGEST_NAME).collect::<String>())
    };
    let mut lines = text.lines();
    let name = lines.next().and_then(plain);
    let status = match lines.next().map(|l| l.trim()) {
        Some("idle") => AgentStatus::Idle,
        _ => AgentStatus::Active,
    };
    let helpers = lines.filter_map(plain).take(MOST_HELPERS).collect();
    (name, status, helpers)
}

/// The name of the folder `pid` is working in, or nothing where it can't be read.
fn folder(proc: &Path, pid: u32) -> String {
    fs::read_link(proc.join(pid.to_string()).join("cwd"))
        .ok()
        .and_then(|dir| {
            let name = dir.file_name()?.to_str()?;
            // A folder deleted from under it reads back with a suffix in place of a name.
            Some(name.strip_suffix(" (deleted)").unwrap_or(name).to_string())
        })
        .filter(|name| !name.chars().any(char::is_control))
        .unwrap_or_default()
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

        fn process(&self, pid: u32, cwd: &str) {
            let dir = self.root.join(format!("proc/{pid}"));
            fs::create_dir_all(&dir).unwrap();
            let fields = format!(
                "S 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {} 0 0",
                100 * TICKS
            );
            fs::write(dir.join("stat"), format!("{pid} (agent) {fields}\n")).unwrap();
            symlink(cwd, dir.join("cwd")).unwrap();
        }

        /// `pid` leaves an empty note, `after` seconds after boot.
        fn note(&self, pid: u32, after: u64) {
            self.note_saying(pid, "", after);
        }

        /// `pid` leaves a note saying `text`, `after` seconds after boot.
        fn note_saying(&self, pid: u32, text: &str, after: u64) {
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

        fn read(&self) -> Vec<Agent> {
            // SAFETY: reads this process's own user id.
            let uid = unsafe { libc::getuid() };
            read(
                &self.root.join("proc"),
                &self.root.join("notes"),
                uid,
                TICKS,
            )
        }
    }

    #[test]
    fn with_no_agent_leaving_a_note_nothing_is_at_work() {
        let machine = Machine::new("working-none");
        machine.process(300, "/home/sam/notes");
        assert_eq!(machine.read(), Vec::new());
    }

    #[test]
    fn an_agent_is_at_work_while_its_note_is_there_and_named_for_its_folder() {
        let machine = Machine::new("working-note");
        machine.process(300, "/home/sam/projects/kiln");
        machine.process(301, "/home/sam/projects/ledger");
        machine.note(300, 150);
        machine.note(301, 160);
        let names: Vec<String> = machine.read().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["kiln", "ledger"]);
        fs::remove_file(machine.root.join("notes/300")).unwrap();
        let names: Vec<String> = machine.read().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["ledger"]);
    }

    #[test]
    fn second_line_sets_the_agent_status() {
        let machine = Machine::new("working-status");
        machine.process(300, "/home/sam/projects/kiln");
        machine.process(301, "/home/sam/projects/ledger");
        machine.process(302, "/home/sam/projects/orchard");
        machine.note_saying(300, "robin: fixing the kiln\nwaiting\n", 150);
        machine.note_saying(301, "robin: counting the ledger\nidle\n", 160);
        machine.note_saying(302, "robin: growing the orchard\n", 170);
        let agents = machine.read();
        assert_eq!(agents[0].name, "robin: fixing the kiln");
        assert_eq!(
            agents[0].status,
            AgentStatus::Active,
            "any second line but 'idle'"
        );
        assert_eq!(agents[1].name, "robin: counting the ledger");
        assert_eq!(agents[1].status, AgentStatus::Idle, "second line 'idle'");
        assert_eq!(agents[2].name, "robin: growing the orchard");
        assert_eq!(agents[2].status, AgentStatus::Active, "no second line");
    }

    #[test]
    fn the_lines_after_the_second_are_the_agent_s_helpers() {
        let machine = Machine::new("working-helpers");
        machine.process(300, "/home/sam/projects/kiln");
        machine.process(301, "/home/sam/projects/ledger");
        machine.note_saying(
            300,
            "robin: firing the kiln\n\nglazing the pots\n\n  stacking\u{7} wood\nsweeping up\n",
            150,
        );
        let many: String = (0..12).map(|i| format!("helper {i}\n")).collect();
        machine.note_saying(301, &format!("robin: the ledger\nidle\n{many}"), 160);
        let agents = machine.read();
        assert_eq!(
            agents[0].status,
            AgentStatus::Active,
            "an empty second line"
        );
        assert_eq!(
            agents[0].helpers,
            ["glazing the pots", "sweeping up"],
            "plain lines only"
        );
        assert_eq!(agents[1].status, AgentStatus::Idle);
        assert_eq!(agents[1].helpers.len(), MOST_HELPERS);
    }

    #[test]
    fn idle_agent_is_shown_like_any_other() {
        let machine = Machine::new("working-idle");
        machine.process(300, "/home/sam/projects/kiln");
        machine.note_saying(300, "robin: done\nidle\n", 150);
        let agents = machine.read();
        assert_eq!(agents.len(), 1, "idle note is still counted");
        assert_eq!(agents[0].status, AgentStatus::Idle);
    }

    #[test]
    fn an_agent_is_called_what_its_note_says() {
        let machine = Machine::new("working-named");
        machine.process(300, "/home/sam");
        machine.process(301, "/home/sam/projects/ledger");
        machine.process(302, "/home/sam/projects/orchard");
        machine.note_saying(300, "robin: mending the kiln door\nand more\n", 150);
        machine.note_saying(301, "bell\u{7}s", 150);
        machine.note_saying(302, &"x".repeat(2000), 150);
        let names: Vec<String> = machine.read().into_iter().map(|a| a.name).collect();
        assert_eq!(
            names,
            ["robin: mending the kiln door", "ledger", "orchard"],
            "one plain line, or the folder"
        );
        // status words only count when they are the second line.
        let statuses: Vec<AgentStatus> = machine.read().into_iter().map(|a| a.status).collect();
        assert_eq!(
            statuses,
            [
                AgentStatus::Active,
                AgentStatus::Active,
                AgentStatus::Active
            ]
        );
    }

    #[test]
    fn a_note_whose_agent_has_gone_is_nobody_at_work() {
        let machine = Machine::new("working-gone");
        machine.note(300, 150);
        assert_eq!(machine.read(), Vec::new());
    }

    #[test]
    fn a_note_older_than_its_process_is_someone_elses() {
        let machine = Machine::new("working-stale");
        machine.process(300, "/home/sam/projects/kiln");
        machine.note(300, 50);
        assert_eq!(machine.read(), Vec::new());
    }

    #[test]
    fn only_a_plain_file_named_for_a_process_counts() {
        let machine = Machine::new("working-plain");
        machine.process(300, "/home/sam/projects/kiln");
        fs::write(machine.root.join("notes/kiln"), "").unwrap();
        fs::create_dir(machine.root.join("notes/300")).unwrap();
        assert_eq!(machine.read(), Vec::new());
    }
}

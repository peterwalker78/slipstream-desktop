//! Which AI agents are at work, for the wallpaper to show once the desktop has faded.
//!
//! Slipstream has no AI in it, and this does nothing on its own. It is here for people who run
//! an AI coding agent, whichever agent that is, and step away while it works: the agent says
//! when it is working, and the faded desktop shows a trace for it (`scope.rs`). With no agent
//! installed, or one that leaves no word, the wallpaper is as it always was.
//!
//! The agent says so with a note: a file named for its process id in
//! `$XDG_RUNTIME_DIR/slipstream/working/`, there while it is working and gone when it stops.
//! Nothing is read from the note; that it exists is the whole message. Most agents can run a
//! command when a task starts and when it ends, which is where the note is made and removed.
//! Only the agent itself leaves one, so the helpers it starts for a task don't each get a trace.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::resume;

/// More notes than this are more than the screen has room to show.
const MOST_NOTES: usize = 64;

/// An agent that says it is working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pid: u32,
    /// The folder it is working in, which is what tells one agent from another.
    pub name: String,
}

/// Where agents leave these notes, if there is a runtime folder to keep them in.
pub fn notes_dir() -> Option<PathBuf> {
    resume::notes_about("working")
}

/// Every agent at work now.
pub fn at_work() -> Vec<Agent> {
    let Some(notes) = notes_dir() else {
        return Vec::new();
    };
    // SAFETY: both only read values the kernel holds for this process.
    let (uid, ticks) = unsafe { (libc::getuid(), libc::sysconf(libc::_SC_CLK_TCK)) };
    read(Path::new("/proc"), &notes, uid, ticks.max(1) as u64)
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
            Some(Agent {
                pid,
                name: folder(proc, pid),
            })
        })
        .collect();
    agents.sort_by_key(|agent| agent.pid);
    agents
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

        /// `pid` leaves its note, `after` seconds after boot.
        fn note(&self, pid: u32, after: u64) {
            let path = self.root.join(format!("notes/{pid}"));
            fs::write(&path, "").unwrap();
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

//! The process tree, read from a proc folder (`/proc`, or a made-up one in tests).

use std::fs;
use std::path::PathBuf;

pub struct Procs {
    root: PathBuf,
}

impl Procs {
    pub fn system() -> Procs {
        Procs::at("/proc")
    }

    pub fn at(root: impl Into<PathBuf>) -> Procs {
        Procs { root: root.into() }
    }

    fn file(&self, pid: u32, name: &str) -> PathBuf {
        self.root.join(pid.to_string()).join(name)
    }

    pub fn exists(&self, pid: u32) -> bool {
        self.root.join(pid.to_string()).is_dir()
    }

    /// The process's name, as the kernel keeps it (cut to 15 bytes).
    pub fn comm(&self, pid: u32) -> Option<String> {
        let comm = fs::read_to_string(self.file(pid, "comm")).ok()?;
        Some(comm.trim_end_matches('\n').to_owned())
    }

    pub fn parent(&self, pid: u32) -> Option<u32> {
        let stat = fs::read_to_string(self.file(pid, "stat")).ok()?;
        // The command name comes first, in parentheses, and can hold spaces and parentheses of
        // its own: the parent is the second field after the last ") ".
        let (_, rest) = stat.rsplit_once(") ")?;
        rest.split(' ').nth(1)?.parse().ok()
    }

    /// The process's start, in clock ticks after boot.
    pub fn start_ticks(&self, pid: u32) -> Option<u64> {
        let stat = fs::read_to_string(self.file(pid, "stat")).ok()?;
        let (_, rest) = stat.rsplit_once(") ")?;
        // Field 22 of stat, counting the state after the name as field 3.
        rest.split(' ').nth(19)?.parse().ok()
    }

    /// The first argument after the program: the script an interpreter runs.
    fn script(&self, pid: u32) -> Option<String> {
        let cmdline = fs::read(self.file(pid, "cmdline")).ok()?;
        let arg = cmdline.split(|&byte| byte == 0).nth(1)?;
        Some(String::from_utf8_lossy(arg).into_owned())
    }

    fn link(&self, pid: u32, name: &str) -> Option<PathBuf> {
        fs::read_link(self.file(pid, name)).ok()
    }

    fn reads_a_terminal(&self, pid: u32) -> bool {
        self.link(pid, "fd/0")
            .is_some_and(|stdin| stdin.starts_with("/dev/pts/"))
    }

    /// Whether process `pid` is `agent`: called that, an interpreter (node, python) running a
    /// script called that, as Gemini CLI is, or a versioned copy of it running in a terminal.
    /// Claude Code runs a background session straight from its versions folder, so the process
    /// is named after the version. The host above it, which keeps the session's terminal, has no
    /// terminal of its own and isn't counted, or the session would look like a subagent. Nor is a
    /// client such as `claude attach`: the one that started the background service has every
    /// background session beneath it, and none of them is its own.
    pub fn is_agent(&self, agent: &str, pid: u32) -> bool {
        if self
            .script(pid)
            .is_some_and(|first| crate::agents::subcommands(agent).contains(&first.as_str()))
        {
            return false;
        }
        if self.comm(pid).as_deref() == Some(agent) {
            return true;
        }
        if self
            .script(pid)
            .is_some_and(|script| crate::text::last_part(&script) == agent)
        {
            return true;
        }
        let versions = format!("/{agent}/versions/");
        self.link(pid, "exe")
            .is_some_and(|exe| exe.to_string_lossy().contains(&versions))
            && self.reads_a_terminal(pid)
    }

    /// The agent's own process: the nearest one at or above `from` that is called what the agent
    /// is. None when this is a subagent: a second ancestor with that name further up means a
    /// session started this one. One straight above another is the same session restarting
    /// itself, as Gemini CLI does, and counts as the lower.
    pub fn agent_above(&self, agent: &str, from: u32) -> Option<u32> {
        let mut pid = from;
        let mut found = None;
        let mut below = None;
        while pid > 1 {
            if self.is_agent(agent, pid) {
                if found.is_none() {
                    found = Some(pid);
                } else if below.is_none() {
                    return None;
                }
                below = Some(pid);
            } else {
                below = None;
            }
            pid = self.parent(pid).unwrap_or(0);
        }
        found
    }

    /// The terminal a process is running in, by name and PID: the nearest process above it that
    /// isn't itself reading from one.
    pub fn terminal_of(&self, pid: u32) -> Option<(String, u32)> {
        let mut pid = self.parent(pid).unwrap_or(0);
        while pid > 1 {
            if !self.reads_a_terminal(pid) {
                return Some((self.comm(pid).unwrap_or_default(), pid));
            }
            pid = self.parent(pid).unwrap_or(0);
        }
        None
    }

    /// The owner of the process.
    pub fn owner(&self, pid: u32) -> Option<u32> {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(self.root.join(pid.to_string()))
            .ok()
            .map(|meta| meta.uid())
    }

    /// The folder the process is working in.
    pub fn cwd(&self, pid: u32) -> Option<PathBuf> {
        self.link(pid, "cwd")
    }
}

#[cfg(test)]
pub mod fake {
    //! A made-up process table, for tests.

    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    pub struct FakeProcs {
        pub root: PathBuf,
    }

    impl FakeProcs {
        pub fn new(name: &str) -> FakeProcs {
            let root = std::env::temp_dir()
                .join(format!("agent-notify-procs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            FakeProcs { root }
        }

        /// A process called `comm`, with `parent` above it, running `args`.
        pub fn process(&self, pid: u32, parent: u32, comm: &str, args: &[&str]) -> PathBuf {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            let stat = format!(
                "{pid} ({comm}) S {parent} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 500 0 0\n"
            );
            std::fs::write(dir.join("stat"), stat).unwrap();
            let mut cmdline = Vec::new();
            for arg in args {
                cmdline.extend_from_slice(arg.as_bytes());
                cmdline.push(0);
            }
            std::fs::write(dir.join("cmdline"), cmdline).unwrap();
            dir
        }

        pub fn link(&self, pid: u32, name: &str, to: &str) {
            symlink(to, self.root.join(pid.to_string()).join(name)).unwrap();
        }

        pub fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for FakeProcs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeProcs;
    use super::*;

    #[test]
    fn the_agent_is_the_nearest_process_called_what_it_is() {
        let table = FakeProcs::new("nearest");
        table.process(100, 1, "konsole", &["konsole"]);
        table.process(200, 100, "zsh", &["zsh"]);
        table.process(300, 200, "claude", &["claude"]);
        table.process(
            400,
            300,
            "sh",
            &["sh", "-c", "agent-notify hook claude working"],
        );
        let procs = Procs::at(table.root());
        assert_eq!(procs.agent_above("claude", 400), Some(300));
        assert_eq!(procs.agent_above("gemini", 400), None);
    }

    #[test]
    fn an_interpreter_running_the_agent_s_script_is_the_agent() {
        let table = FakeProcs::new("script");
        table.process(
            300,
            1,
            "node",
            &["node", "/opt/gemini-cli/bin/gemini", "--yolo"],
        );
        table.process(310, 300, "node", &["node", "/opt/gemini-cli/bin/gemini"]);
        table.process(400, 310, "sh", &["sh"]);
        let procs = Procs::at(table.root());
        assert_eq!(
            procs.agent_above("gemini", 400),
            Some(310),
            "one restarting itself is one"
        );
    }

    #[test]
    fn a_second_agent_further_up_makes_this_a_subagent() {
        let table = FakeProcs::new("subagent");
        table.process(300, 1, "codex", &["codex"]);
        table.process(310, 300, "bash", &["bash"]);
        table.process(320, 310, "codex", &["codex", "exec"]);
        table.process(400, 320, "sh", &["sh"]);
        assert_eq!(Procs::at(table.root()).agent_above("codex", 400), None);
    }

    #[test]
    fn a_versioned_copy_counts_only_while_it_reads_a_terminal() {
        let table = FakeProcs::new("versions");
        table.process(
            300,
            1,
            "2.1.0",
            &["/home/sam/.local/share/claude/versions/2.1.0"],
        );
        table.link(300, "exe", "/home/sam/.local/share/claude/versions/2.1.0");
        table.link(300, "fd/0", "/dev/null");
        table.process(
            310,
            300,
            "2.1.0",
            &["/home/sam/.local/share/claude/versions/2.1.0"],
        );
        table.link(310, "exe", "/home/sam/.local/share/claude/versions/2.1.0");
        table.link(310, "fd/0", "/dev/pts/4");
        table.process(400, 310, "sh", &["sh"]);
        assert_eq!(
            Procs::at(table.root()).agent_above("claude", 400),
            Some(310)
        );
    }

    #[test]
    fn a_client_above_the_background_service_is_not_the_agent() {
        let table = FakeProcs::new("client");
        let versions = "/home/sam/.local/share/claude/versions/2.1.0";
        table.process(200, 1, "zsh", &["zsh"]);
        table.link(200, "fd/0", "/dev/pts/3");
        table.process(300, 200, "claude", &["claude", "attach", "a1b2c3d4"]);
        table.link(300, "exe", versions);
        table.link(300, "fd/0", "/dev/pts/3");
        table.process(310, 300, "2.1.0", &[versions, "daemon", "run"]);
        table.link(310, "exe", versions);
        table.link(310, "fd/0", "/dev/null");
        table.process(320, 310, "2.1.0", &[versions, "--session-id", "e5f6"]);
        table.link(320, "exe", versions);
        table.link(320, "fd/0", "/dev/pts/7");
        table.process(400, 320, "sh", &["sh"]);
        let procs = Procs::at(table.root());
        assert_eq!(
            procs.agent_above("claude", 400),
            Some(320),
            "not a subagent"
        );
        assert!(!procs.is_agent("claude", 300));
    }

    #[test]
    fn a_name_with_spaces_and_parentheses_still_gives_the_parent() {
        let table = FakeProcs::new("parens");
        let dir = table.process(500, 1, "x", &["x"]);
        std::fs::write(dir.join("stat"), "500 (odd) name) S 42 500 500 0\n").unwrap();
        assert_eq!(Procs::at(table.root()).parent(500), Some(42));
    }

    #[test]
    fn the_terminal_is_the_first_process_above_not_reading_one() {
        let table = FakeProcs::new("terminal");
        table.process(100, 1, "foot", &["foot"]);
        table.link(100, "fd/0", "/dev/null");
        table.process(200, 100, "zsh", &["zsh"]);
        table.link(200, "fd/0", "/dev/pts/3");
        table.process(300, 200, "agent-notify", &["agent-notify"]);
        table.link(300, "fd/0", "/dev/pts/3");
        assert_eq!(
            Procs::at(table.root()).terminal_of(300),
            Some(("foot".to_owned(), 100))
        );
    }
}

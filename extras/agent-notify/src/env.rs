//! Where everything lives, read from the environment once.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Folders Homebrew installs programs into on Linux. A desktop session's PATH is set before any
/// shell profile runs, so agents installed there aren't on it.
const HOMEBREW_BIN: &str = "/home/linuxbrew/.linuxbrew/bin";

pub struct Env {
    pub home: PathBuf,
    config_home: PathBuf,
    cache_home: PathBuf,
    state_home: PathBuf,
    runtime_dir: Option<PathBuf>,
    codex_home: PathBuf,
    /// PATH, with Homebrew's folders after it when they exist.
    path: OsString,
}

/// A variable that is set and not empty, the way `${NAME:-default}` reads one.
pub fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

pub fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

impl Env {
    pub fn from_process() -> Env {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let under_home = |name: &str, default: &str| {
            var(name)
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(default))
        };
        let path = with_homebrew(var("PATH").unwrap_or_default(), &home);
        Env {
            config_home: under_home("XDG_CONFIG_HOME", ".config"),
            cache_home: under_home("XDG_CACHE_HOME", ".cache"),
            state_home: under_home("XDG_STATE_HOME", ".local/state"),
            codex_home: under_home("CODEX_HOME", ".codex"),
            runtime_dir: var("XDG_RUNTIME_DIR").map(PathBuf::from),
            path,
            home,
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_home.join("agent-notify/config")
    }

    /// This tool's own state: notification ids, task start times, kept sessions' windows.
    pub fn state_dir(&self) -> PathBuf {
        match &self.runtime_dir {
            Some(dir) => dir.join("agent-notify"),
            None => Path::new(&format!("/tmp/agent-notify-{}", uid())).join("agent-notify"),
        }
    }

    pub fn runtime_dir(&self) -> Option<&Path> {
        self.runtime_dir.as_deref()
    }

    /// Where a desktop that draws agents at work looks for their notes, if there is a runtime
    /// folder to look in.
    pub fn notes_dir(&self) -> Option<PathBuf> {
        self.runtime_dir
            .as_ref()
            .map(|dir| dir.join("slipstream/working"))
    }

    /// Where a desktop that restores sessions at login looks for the command that reopens each.
    pub fn resume_dir(&self) -> Option<PathBuf> {
        self.runtime_dir
            .as_ref()
            .map(|dir| dir.join("slipstream/resume"))
    }

    pub fn usage_dir(&self) -> PathBuf {
        self.cache_home.join("agent-notify/usage")
    }

    pub fn log_file(&self) -> PathBuf {
        self.state_home.join("agent-notify/agent-notify.log")
    }

    pub fn codex_home(&self) -> &Path {
        &self.codex_home
    }

    /// The first executable called `name` on PATH, as `command -v` finds it.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(&self.path).find_map(|dir| {
            let dir = if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir
            };
            let candidate = dir.join(name);
            let meta = candidate.metadata().ok()?;
            (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(candidate)
        })
    }

    pub fn have(&self, name: &str) -> bool {
        self.which(name).is_some()
    }

    /// A command that finds its program, and passes PATH on, with Homebrew's folders included.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.env("PATH", &self.path);
        command
    }

    /// The path hooks call this program by: the copy on PATH if there is one, so the folder it
    /// was first run from can move, or else the one running now.
    pub fn self_path(&self) -> PathBuf {
        if let Some(found) = self.which("agent-notify").filter(|path| path.is_absolute()) {
            return found;
        }
        let invoked = std::env::args_os().next().map(PathBuf::from);
        match invoked {
            Some(path) if path.is_absolute() => path,
            Some(path) if path.components().count() > 1 => std::env::current_dir()
                .map(|dir| dir.join(&path))
                .unwrap_or(path),
            _ => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent-notify")),
        }
    }
}

fn with_homebrew(path: OsString, home: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let had = dirs.clone();
    for brew in [PathBuf::from(HOMEBREW_BIN), home.join(".linuxbrew/bin")] {
        if !had.contains(&brew) && brew.is_dir() {
            dirs.push(brew);
        }
    }
    if dirs.len() == had.len() {
        return path;
    }
    std::env::join_paths(dirs).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_s_folders_go_after_the_path_only_when_they_exist() {
        let home = std::env::temp_dir().join(format!("agent-notify-env-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".linuxbrew/bin")).unwrap();
        let path = with_homebrew(OsString::from("/usr/bin:/bin"), &home);
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs[..2],
            [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
        assert_eq!(dirs.last(), Some(&home.join(".linuxbrew/bin")));
        let again = with_homebrew(path.clone(), &home);
        assert_eq!(
            again, path,
            "a folder already on the path isn't added twice"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}

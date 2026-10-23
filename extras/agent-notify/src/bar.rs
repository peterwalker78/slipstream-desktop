//! `agent-notify bar`: a module for a bar that takes Waybar's custom-module format. It runs until
//! the bar stops it and prints one line of JSON each time the agents at work change: the text
//! says how many are working (and how many wait on you), the tooltip lists every open session by
//! its trace's name with its subagents under it, and the class is `warning` while one is waiting
//! on you. With no agent at work the text is empty, which hides the module.
//!
//! It reads the same working notes the hooks write, and watches their folder with inotify, so it
//! wakes only when a note changes, and every few seconds to notice a session whose process has
//! gone.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;

use crate::env::{Env, uid};
use crate::hook::waiting_marker;
use crate::note::Note;
use crate::procs::Procs;
use crate::text::{escape_markup, is_number, last_part};

/// How often the processes behind the notes are checked, since a process ending changes no note.
const RECHECK: Duration = Duration::from_secs(5);
/// How long to let a burst of note changes settle before reading them.
const SETTLE: Duration = Duration::from_millis(100);

pub fn run(env: &Env) -> ExitCode {
    let procs = Procs::system();
    let clock = Clock::read();
    let mut shown = None;
    let mut watch = None;
    let mut stdout = io::stdout().lock();
    loop {
        let notes = env.notes_dir();
        if watch.is_none() {
            watch = notes.as_deref().and_then(Watch::new);
        }
        let line = line(&sessions(env, &procs, &clock)).to_string();
        if shown.as_ref() != Some(&line) {
            // The bar has gone: so has the reason to run.
            if writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return ExitCode::SUCCESS;
            }
            shown = Some(line);
        }
        match &watch {
            Some(current) => {
                if !current.wait(RECHECK) {
                    watch = None;
                }
            }
            None => std::thread::sleep(RECHECK),
        }
    }
}

/// An open session, as its note and the hooks' state describe it.
pub struct Session {
    pub pid: u32,
    pub note: Note,
    pub waiting: bool,
}

/// When processes started, for telling a note from a process that has gone from one left by a
/// new process with the same number.
struct Clock {
    boot: u64,
    ticks: u64,
}

impl Clock {
    fn read() -> Clock {
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        let boot = stat
            .lines()
            .find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok());
        // SAFETY: sysconf only reads a system value.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        Clock {
            boot: boot.unwrap_or(0),
            ticks: u64::try_from(ticks).unwrap_or(100).max(1),
        }
    }

    fn started(&self, procs: &Procs, pid: u32) -> Option<u64> {
        Some(self.boot + procs.start_ticks(pid)? / self.ticks)
    }
}

/// The open sessions, by process ID.
fn sessions(env: &Env, procs: &Procs, clock: &Clock) -> Vec<Session> {
    let Some(dir) = env.notes_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let state = env.state_dir();
    let mut sessions: Vec<Session> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .into_string()
                .ok()
                .filter(|name| is_number(name))?;
            let pid: u32 = name.parse().ok()?;
            if procs.owner(pid) != Some(uid()) {
                return None;
            }
            // A note older than its process belongs to one that had the number before.
            let written = entry.metadata().ok()?.modified().ok()?;
            let written = written.duration_since(UNIX_EPOCH).ok()?.as_secs();
            if written + 2 < clock.started(procs, pid)? {
                return None;
            }
            let mut note = Note::parse(&fs::read_to_string(entry.path()).ok()?);
            if note.name.is_empty() {
                note.name = procs
                    .cwd(pid)
                    .map(|cwd| last_part(&cwd.to_string_lossy()).to_owned())
                    .unwrap_or_default();
            }
            let waiting = note.idle && waiting_marker(&state, pid).exists();
            Some(Session { pid, note, waiting })
        })
        .collect();
    sessions.sort_by_key(|session| session.pid);
    sessions
}

/// The module's line for these sessions.
fn line(sessions: &[Session]) -> serde_json::Value {
    let working = sessions.iter().filter(|session| !session.note.idle).count();
    let waiting = sessions.iter().filter(|session| session.waiting).count();
    if working == 0 && waiting == 0 {
        return json!({"text": ""});
    }
    let mut text = Vec::new();
    if working > 0 {
        text.push(format!("{working} working"));
    }
    if waiting > 0 {
        text.push(format!("{waiting} waiting"));
    }
    let mut tooltip = Vec::new();
    for session in sessions {
        let status = match (session.waiting, session.note.idle) {
            (true, _) => " (waiting on you)",
            (false, true) => " (idle)",
            (false, false) => "",
        };
        tooltip.push(format!("{}{status}", session.note.name));
        tooltip.extend(
            session
                .note
                .helpers
                .iter()
                .map(|helper| format!("  {helper}")),
        );
    }
    json!({
        "text": escape_markup(&text.join(", ")),
        "tooltip": escape_markup(&tooltip.join("\n")),
        "class": if waiting > 0 { "warning" } else { "working" },
    })
}

/// An inotify watch on the notes folder.
struct Watch {
    fd: OwnedFd,
}

impl Watch {
    fn new(dir: &Path) -> Option<Watch> {
        let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: inotify_init1 returns a new descriptor or -1, and OwnedFd takes sole charge of
        // it; inotify_add_watch reads the path for the length of the call.
        unsafe {
            let fd = libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK);
            if fd < 0 {
                return None;
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let events = libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_MOVED_TO
                | libc::IN_MOVED_FROM
                | libc::IN_CLOSE_WRITE
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF
                | libc::IN_ONLYDIR;
            if libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), events) < 0 {
                return None;
            }
            Some(Watch { fd })
        }
    }

    /// Waits up to `timeout` for a change. False once the folder itself has gone.
    fn wait(&self, timeout: Duration) -> bool {
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll waits on the one descriptor in a struct owned here.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) };
        if ready <= 0 {
            return true;
        }
        std::thread::sleep(SETTLE);
        self.drain()
    }

    /// Reads every waiting event. False if one says the folder has gone.
    fn drain(&self) -> bool {
        let mut buffer = [0u8; 4096];
        let mut alive = true;
        loop {
            // SAFETY: read fills at most the buffer's length.
            let read = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            let Ok(read) = usize::try_from(read) else {
                return alive;
            };
            if read == 0 {
                return alive;
            }
            let mut at = 0;
            let header = std::mem::size_of::<libc::inotify_event>();
            while at + header <= read {
                // SAFETY: the kernel writes whole events, each a header and then `len` bytes of
                // name; read_unaligned copes with the buffer's alignment.
                let event: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buffer[at..].as_ptr().cast()) };
                if event.mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                {
                    alive = false;
                }
                at += header + event.len as usize;
            }
        }
    }
}

/// Seconds since the epoch, for tests that make notes.
#[cfg(test)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pid: u32, name: &str, idle: bool, helpers: &[&str], waiting: bool) -> Session {
        let note = Note {
            name: name.into(),
            idle,
            helpers: helpers.iter().map(|h| h.to_string()).collect(),
        };
        Session { pid, note, waiting }
    }

    #[test]
    fn with_no_agent_at_work_the_module_is_empty() {
        assert_eq!(line(&[]), json!({"text": ""}));
        assert_eq!(
            line(&[session(10, "claude: kiln", true, &[], false)]),
            json!({"text": ""})
        );
    }

    #[test]
    fn the_line_counts_who_is_working_and_lists_every_session() {
        let sessions = [
            session(
                10,
                "claude: fix <the> kiln",
                false,
                &["reading the tests"],
                false,
            ),
            session(11, "gemini: ledger", true, &[], false),
            session(12, "codex: orchard", false, &[], false),
        ];
        assert_eq!(
            line(&sessions),
            json!({
                "text": "2 working",
                "tooltip": "claude: fix &lt;the&gt; kiln\n  reading the tests\ngemini: ledger (idle)\ncodex: orchard",
                "class": "working",
            })
        );
    }

    #[test]
    fn one_waiting_on_you_is_a_warning() {
        let sessions = [
            session(10, "claude: kiln", true, &[], true),
            session(11, "codex: x", false, &[], false),
        ];
        let line = line(&sessions);
        assert_eq!(line["text"], "1 working, 1 waiting");
        assert_eq!(line["class"], "warning");
        assert_eq!(line["tooltip"], "claude: kiln (waiting on you)\ncodex: x");
    }

    #[test]
    fn the_clock_dates_this_process_after_boot() {
        let clock = Clock::read();
        let started = clock.started(&Procs::system(), std::process::id()).unwrap();
        assert!(started <= now() + 1 && started + 86_400 * 365 > now());
    }
}

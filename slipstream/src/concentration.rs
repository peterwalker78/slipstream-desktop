//! Whether you're in the middle of typing, from key timing alone: which keys were pressed is never
//! looked at or kept, only when a key reached an app and whether a shortcut modifier was held.
//!
//! While you type, nothing but the app you're typing in (or a program it started) may take the
//! keyboard. The typing ends at a pause, at a shortcut (Ctrl+O asks for the window that follows),
//! at a click or a key the desktop keeps, or when the keyboard moves somewhere else.

use std::{
    fs,
    time::{Duration, Instant},
};

/// How long after the last key you still count as typing. A pause this long is a break.
pub const PAUSE: Duration = Duration::from_secs(4);

#[derive(Debug, Default)]
pub struct Concentration {
    /// When a key last reached an app, while the keyboard stayed where it was.
    last_typed: Option<Instant>,
    /// When you last clicked or pressed a key, on anything.
    last_input: Option<Instant>,
    /// When you last asked for something to start: a launch from the desktop, or an app handed
    /// out an activation token for a click or key press.
    last_asked: Option<Instant>,
}

/// How soon after a click or key press a window counts as what it opened, such as a confirmation
/// from a helper process or a portal's file chooser.
pub const INPUT_OPENS: Duration = Duration::from_secs(5);

/// How long an app started from the desktop may take to show its window.
pub const LAUNCH_OPENS: Duration = Duration::from_secs(30);

impl Concentration {
    /// A key pressed and passed on to the focused app. `shortcut` is a press with Ctrl, Alt or
    /// Super held: a command rather than typing, so it ends the typing instead.
    pub fn key(&mut self, now: Instant, shortcut: bool) {
        self.last_typed = (!shortcut).then_some(now);
        self.last_input = Some(now);
    }

    /// A click or a key press, wherever it went.
    pub fn input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    /// Something was started for you.
    pub fn asked(&mut self, now: Instant) {
        self.last_asked = Some(now);
    }

    /// Whether a window appearing now is plausibly what you just asked for.
    pub fn recently_asked(&self, now: Instant) -> bool {
        let within = |at: Option<Instant>, limit| {
            at.is_some_and(|at| now.saturating_duration_since(at) < limit)
        };
        within(self.last_input, INPUT_OPENS) || within(self.last_asked, LAUNCH_OPENS)
    }

    /// The keyboard went somewhere else, or you clicked or used a desktop key: whatever you were
    /// typing is over, and what comes next was asked for.
    pub fn end(&mut self) {
        self.last_typed = None;
    }

    pub fn typing(&self, now: Instant) -> bool {
        self.last_typed
            .is_some_and(|typed| now.saturating_duration_since(typed) < PAUSE)
    }
}

/// Whether `pid` is `ancestor` or was started by it, however far down: a program run from a
/// terminal, or a helper an app spawned. Stops at init, or after a depth no real tree reaches.
pub fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match fs::read_to_string(format!("/proc/{current}/stat"))
            .ok()
            .and_then(|stat| parent_in_stat(&stat))
        {
            Some(parent) if parent > 1 => current = parent,
            _ => return false,
        }
    }
    false
}

/// The parent pid from a `/proc/<pid>/stat` line. The command name comes in brackets and may
/// hold spaces and brackets itself, so the fields are counted from the last `)`.
fn parent_in_stat(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_lasts_until_a_pause() {
        let start = Instant::now();
        let mut c = Concentration::default();
        assert!(!c.typing(start));
        c.key(start, false);
        assert!(c.typing(start + Duration::from_secs(3)));
        assert!(!c.typing(start + PAUSE));
    }

    #[test]
    fn shortcuts_and_the_end_of_typing_stop_it() {
        let start = Instant::now();
        let mut c = Concentration::default();
        c.key(start, false);
        c.key(start + Duration::from_millis(200), true);
        assert!(!c.typing(start + Duration::from_millis(300)));
        c.key(start, false);
        c.end();
        assert!(!c.typing(start));
    }

    #[test]
    fn parent_is_read_past_awkward_command_names() {
        assert_eq!(parent_in_stat("412 (bash) S 400 412 412 0"), Some(400));
        assert_eq!(
            parent_in_stat("9001 (Web Content (x) 2) S 8810 1 1 0"),
            Some(8810)
        );
        assert_eq!(parent_in_stat("garbage"), None);
    }

    #[test]
    fn a_process_descends_from_itself_and_its_parents() {
        let me = std::process::id();
        assert!(descends_from(me, me));
        let parent = std::os::unix::process::parent_id();
        assert!(descends_from(me, parent));
        assert!(!descends_from(parent, me));
    }
}

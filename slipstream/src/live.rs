//! Keeping a second Slipstream away from the session that's already running.
//!
//! The login session puts its socket's name in `SLIPSTREAM_LIVE_SOCKET`, which everything started
//! in it inherits. A Slipstream run from one of its terminals would otherwise open its nested
//! window on that session, take the keyboard, and act on the same desktop from inside it; so a
//! nested compositor whose display is that socket refuses to start. Checks nest on a headless
//! Weston instead, whose socket is another name.
//!
//! Whatever runs, one compositor at a time owns the state folder: whichever holds an exclusive
//! lock on `owner.lock` there. Only the owner offers or reopens the recorded layout, writes it
//! down, or writes the settings file, so a second compositor sharing the folders can't overwrite
//! the session's record or settings. The login session starts first, so it is the owner; the lock
//! goes with the process however it ends, so it can't go stale.

use std::{ffi::OsStr, fs::File, io, os::fd::AsRawFd, path::Path};

/// Where the login session puts its socket's name.
pub const LIVE_SOCKET: &str = "SLIPSTREAM_LIVE_SOCKET";

/// The lock file's name inside the state folder.
const OWNER_LOCK: &str = "owner.lock";

/// Whether a nested compositor would open its window on the Slipstream session it was started
/// in: `wayland_display` is that session's socket (by name, or by path), or there is no Wayland
/// display at all, when the window would go to `DISPLAY`, the session's own XWayland.
/// `nest_on_live` is `SLIPSTREAM_NEST_ON_LIVE`, which allows it on purpose when it's `1`.
pub fn nests_on_live(
    wayland_display: Option<&OsStr>,
    live_socket: Option<&OsStr>,
    nest_on_live: Option<&OsStr>,
) -> bool {
    if nest_on_live == Some(OsStr::new("1")) {
        return false;
    }
    let Some(live) = live_socket.filter(|live| !live.is_empty()) else {
        return false;
    };
    match wayland_display.filter(|display| !display.is_empty()) {
        Some(display) => Path::new(display).file_name() == Some(live),
        None => true,
    }
}

/// The state folder, held for as long as this value lives.
#[derive(Debug)]
pub struct OwnerLock {
    _file: File,
}

impl OwnerLock {
    /// Takes the lock in `dir` without waiting. `Ok(None)` when another process holds it.
    pub fn take(dir: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(dir)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(OWNER_LOCK))?;
        // SAFETY: `flock` on a descriptor `file` owns for the whole call. The lock belongs to this
        // open file, so it is let go when `file` closes, or when the process ends however it ends.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EWOULDBLOCK) => Ok(None),
            _ => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(text: &str) -> Option<&OsStr> {
        Some(OsStr::new(text))
    }

    #[test]
    fn a_nested_window_on_the_live_session_is_refused() {
        assert!(nests_on_live(os("wayland-1"), os("wayland-1"), None));
        assert!(nests_on_live(
            os("/run/user/1000/wayland-1"),
            os("wayland-1"),
            None
        ));
        assert!(
            nests_on_live(None, os("wayland-1"), None),
            "no Wayland display means the session's XWayland"
        );
        assert!(nests_on_live(os("wayland-1"), os("wayland-1"), os("0")));
    }

    #[test]
    fn everywhere_else_a_nested_window_opens() {
        assert!(
            !nests_on_live(os("slipstream-dev"), os("wayland-1"), None),
            "a headless Weston inside the session"
        );
        assert!(
            !nests_on_live(os("wayland-0"), None, None),
            "another desktop, with no Slipstream session"
        );
        assert!(!nests_on_live(None, None, None));
        assert!(!nests_on_live(os("wayland-1"), os(""), None));
        assert!(
            !nests_on_live(os("wayland-1"), os("wayland-1"), os("1")),
            "asked for on purpose"
        );
    }

    #[test]
    fn one_owner_at_a_time() {
        let dir = crate::files::test_scratch("owner");
        let first = OwnerLock::take(&dir).unwrap().expect("the first takes it");
        // A second open of the same file, as a second compositor would make.
        assert!(
            OwnerLock::take(&dir).unwrap().is_none(),
            "the second is refused"
        );
        drop(first);
        let again = OwnerLock::take(&dir).unwrap();
        assert!(again.is_some(), "free once the first is gone");
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

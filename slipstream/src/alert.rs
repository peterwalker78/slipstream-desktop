//! Notification sounds: the Desktop Notifications spec's `sound-file`, `sound-name` and
//! `suppress-sound` hints.
//!
//! A name is looked up as the XDG Sound Theme spec describes: in the chosen theme, then the themes
//! it inherits from, then `freedesktop`; in each, the name and then its shorter forms
//! (`message-new-instant`, `message-new`, `message`), so a theme without the exact sound still has
//! something close. A `.disabled` file silences a sound on purpose, and the lookup stops there.
//!
//! Playing goes through `pw-play` with the Notification media role, so the session manager's
//! policies for notification streams (their own volume, ducking) apply. Everything happens on one
//! thread of its own, so a slow disk or a stuck player never holds up a frame, and only one sound
//! plays at a time: a burst of notifications is one sound, not a pile of them.

use std::{
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

/// What a notification asked to sound like.
#[derive(Debug, Clone, PartialEq)]
pub enum Sound {
    /// `sound-file`: a file, by path or `file://` URI.
    File(PathBuf),
    /// `sound-name`: a name from the sound theme.
    Name(String),
}

/// No notification sound goes on longer than this. They are meant to be a moment long; a file that
/// isn't is cut off rather than left playing over everything.
const LONGEST: Duration = Duration::from_secs(8);
/// Files bigger than this aren't a notification sound, whatever they are.
const LARGEST: u64 = 8 << 20;

/// Whether a notification's sound should play. Not while you're looking at the window that sent
/// it (`watching`), which is already in front of you, and under do not disturb only for a
/// critical one.
pub fn wanted(
    enabled: bool,
    suppressed: bool,
    watching: bool,
    critical: bool,
    do_not_disturb: bool,
) -> bool {
    enabled && !suppressed && !watching && (critical || !do_not_disturb)
}

/// Plays `sound`, looking names up from `theme`. Returns at once.
pub fn play(sound: Sound, theme: &str) {
    // A nested run's sounds would come out of the speakers of the desktop around it.
    if crate::launch::machine_commands_held() {
        return;
    }
    let asks = ASKS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<(Sound, String)>();
        thread::spawn(move || player(receiver));
        Mutex::new(sender)
    });
    let _ = asks.lock().unwrap().send((sound, theme.to_string()));
}

static ASKS: OnceLock<Mutex<mpsc::Sender<(Sound, String)>>> = OnceLock::new();

/// The player thread: one sound at a time, each cut off at `LONGEST`, and asks that arrive while
/// one is playing dropped.
fn player(asks: mpsc::Receiver<(Sound, String)>) {
    let mut playing: Option<(Child, Instant)> = None;
    loop {
        let ask = match &playing {
            Some(_) => asks.recv_timeout(Duration::from_millis(100)),
            None => asks
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        if let Some((child, started)) = &mut playing {
            let finished = !matches!(child.try_wait(), Ok(None));
            if !finished && started.elapsed() >= LONGEST {
                let _ = child.kill();
                let _ = child.wait();
            }
            if finished || started.elapsed() >= LONGEST {
                playing = None;
            }
        }
        let (sound, theme) = match ask {
            Ok(ask) => ask,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if playing.is_some() {
            tracing::debug!(?sound, "a notification sound is already playing");
            continue;
        }
        let Some(path) = resolve(&sound, &theme, &bases()) else {
            tracing::debug!(?sound, theme, "no notification sound to play");
            continue;
        };
        let child = crate::launch::command("pw-play")
            .args(["--media-role", "Notification", "-P"])
            .arg(r#"{ application.name = "Slipstream" media.name = "Notification" }"#)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => {
                tracing::debug!(path = %path.display(), "notification sound");
                playing = Some((child, Instant::now()));
            }
            Err(err) => tracing::warn!("couldn't play a notification sound: {err}"),
        }
    }
}

/// The file to play for `sound`: a file that's there to play, or the theme's sound for a name.
fn resolve(sound: &Sound, theme: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    let path = match sound {
        Sound::File(path) => path.clone(),
        Sound::Name(name) => slipstream_config::sounds::find(name, theme, bases, &read_index)?,
    };
    playable(&path).then_some(path)
}

/// Whether `path` could be a sound to play: an absolute path to a regular file of a sensible size.
/// Anyone on the session bus can name a file, and a FIFO or a device would leave the player
/// waiting on it for good.
fn playable(path: &Path) -> bool {
    path.is_absolute()
        && std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() <= LARGEST)
}

/// The `sound-file` hint as a path: a plain absolute path, or a local `file://` URI with its
/// escapes decoded.
pub fn file_hint(text: &str) -> Option<PathBuf> {
    let path = match text.strip_prefix("file://") {
        Some(rest) => {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            decode_uri_path(rest)?
        }
        None => text.to_string(),
    };
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

/// A URI's path with its `%XX` escapes decoded. Nothing for a malformed escape, or bytes that
/// aren't UTF-8.
fn decode_uri_path(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Where sound themes live.
fn bases() -> Vec<PathBuf> {
    slipstream_config::sounds::theme_bases()
}

/// `path` read as a theme's index, if it's a small regular file.
fn read_index(path: &Path) -> Option<String> {
    crate::files::read_small_text(path, crate::files::DESKTOP_ENTRY_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("slipstream-alert-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sound_files_by_path_or_uri() {
        assert_eq!(
            file_hint("/usr/share/sounds/done.oga"),
            Some(PathBuf::from("/usr/share/sounds/done.oga"))
        );
        assert_eq!(
            file_hint("file:///home/sam/My%20Sounds/ping.wav"),
            Some(PathBuf::from("/home/sam/My Sounds/ping.wav"))
        );
        assert_eq!(
            file_hint("file://localhost/srv/a.ogg"),
            Some(PathBuf::from("/srv/a.ogg"))
        );
        assert_eq!(file_hint("relative/ping.wav"), None);
        assert_eq!(file_hint("file:///bad%zzescape"), None);
        assert_eq!(file_hint("file:///cut%2"), None);
    }

    #[test]
    fn only_regular_files_of_a_sensible_size_play() {
        let root = scratch("playable");
        let file = root.join("ping.wav");
        std::fs::write(&file, b"RIFF").unwrap();
        assert!(playable(&file));
        assert!(!playable(&root), "a folder");
        assert!(!playable(Path::new("/dev/zero")), "a device");
        assert!(!playable(Path::new("ping.wav")), "relative");
        assert!(!playable(&root.join("missing.wav")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn who_hears_what() {
        // enabled, suppressed, watching, critical, do not disturb
        assert!(wanted(true, false, false, false, false));
        assert!(!wanted(false, false, false, true, false), "switched off");
        assert!(
            !wanted(true, true, false, true, false),
            "the app asked for quiet"
        );
        assert!(
            !wanted(true, false, true, true, false),
            "already looking at the window it came from"
        );
        assert!(!wanted(true, false, false, false, true), "do not disturb");
        assert!(
            wanted(true, false, false, true, true),
            "critical gets through"
        );
    }
}

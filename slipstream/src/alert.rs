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
    ffi::OsStr,
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
/// How far a theme's inheritance is followed, against a loop or a runaway chain.
const THEMES: usize = 16;
/// The file types a sound theme holds, in the spec's order.
const EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];

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
        Sound::Name(name) => find(name, theme, bases)?,
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

/// Where sound themes live: `$XDG_DATA_HOME/sounds`, then `sounds` under each of
/// `$XDG_DATA_DIRS`, with the spec's defaults for either when unset.
fn bases() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(
            dirs.split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("sounds"))
        .collect()
}

/// A name or theme that is one plain path component, so nothing sent over the bus can reach
/// outside the sound folders.
fn plain_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

/// A theme's `index.theme`, as much of it as the lookup needs.
#[derive(Debug, Default, PartialEq)]
struct Index {
    inherits: Vec<String>,
    /// Its sound folders, stereo ones first.
    directories: Vec<String>,
}

/// The first `index.theme` for `theme` under `bases`, read. Nothing when no base has the theme.
fn index(theme: &str, bases: &[PathBuf]) -> Option<Index> {
    bases.iter().find_map(|base| {
        let text = std::fs::read_to_string(base.join(theme).join("index.theme")).ok()?;
        Some(parse_index(&text))
    })
}

/// `index.theme`'s `Inherits` and `Directories` from its `[Sound Theme]` group, with the folders
/// whose `OutputProfile` is `stereo` put first, since that's what the speakers are.
fn parse_index(text: &str) -> Index {
    let list = |value: &str| -> Vec<String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| plain_component(item))
            .map(str::to_string)
            .collect()
    };
    let mut index = Index::default();
    let mut stereo: Vec<String> = Vec::new();
    let mut group = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (group.as_str(), key.trim()) {
            ("Sound Theme", "Inherits") => index.inherits = list(value),
            ("Sound Theme", "Directories") => index.directories = list(value),
            (_, "OutputProfile") if value.trim() == "stereo" => stereo.push(group.clone()),
            _ => {}
        }
    }
    index.directories.sort_by_key(|dir| !stereo.contains(dir));
    index
}

/// The sound for `name` in `theme` or what it inherits from, ending with `freedesktop`, trying
/// the name and then its shorter forms in each theme before moving on. A `.disabled` file ends
/// the search with nothing.
fn find(name: &str, theme: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    if !plain_component(name) {
        return None;
    }
    let mut chain: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![theme.to_string()];
    while let Some(next) = (!queue.is_empty()).then(|| queue.remove(0)) {
        if chain.len() >= THEMES || chain.contains(&next) || !plain_component(&next) {
            continue;
        }
        if let Some(index) = index(&next, bases) {
            queue.extend(index.inherits.iter().cloned());
            chain.push(next);
        }
    }
    if !chain.iter().any(|theme| theme == "freedesktop") {
        chain.push("freedesktop".to_string());
    }
    for theme in &chain {
        let Some(index) = index(theme, bases) else {
            continue;
        };
        for variant in shorter_forms(name) {
            for dir in &index.directories {
                for base in bases {
                    let stem = base.join(theme).join(dir).join(variant);
                    if stem.with_extension("disabled").exists() {
                        return None;
                    }
                    for extension in EXTENSIONS {
                        let path = stem.with_extension(extension);
                        if path.is_file() {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

/// `name`, then each shorter form made by dropping its last `-` part.
fn shorter_forms(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(name), |name| {
        name.rfind('-')
            .map(|at| &name[..at])
            .filter(|n| !n.is_empty())
    })
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

    fn theme(base: &Path, name: &str, index: &str, sounds: &[&str]) {
        let dir = base.join(name);
        std::fs::create_dir_all(dir.join("stereo")).unwrap();
        std::fs::write(dir.join("index.theme"), index).unwrap();
        for sound in sounds {
            std::fs::write(dir.join("stereo").join(sound), b"sound").unwrap();
        }
    }

    const STEREO: &str =
        "[Sound Theme]\nName=T\nDirectories=stereo\n\n[stereo]\nOutputProfile=stereo\n";

    #[test]
    fn a_name_falls_back_to_shorter_forms_then_inherited_themes_then_freedesktop() {
        let root = scratch("lookup");
        let (user, system) = (root.join("user"), root.join("system"));
        theme(
            &system,
            "harbour",
            &STEREO.replace("Name=T\n", "Name=T\nInherits=reef\n"),
            &["message-new-instant.oga"],
        );
        theme(&system, "reef", STEREO, &["bell.wav"]);
        theme(
            &system,
            "freedesktop",
            STEREO,
            &["complete.oga", "bell.oga"],
        );
        // Sounds in the user's own folder for a theme are searched too, ahead of the system's; the
        // theme's index is the system's, the first one found.
        std::fs::create_dir_all(user.join("harbour/stereo")).unwrap();
        std::fs::write(user.join("harbour/stereo/dialog-warning.ogg"), b"sound").unwrap();
        let bases = [user.clone(), system.clone()];

        let found = |name: &str| find(name, "harbour", &bases);
        assert_eq!(
            found("message-new-instant"),
            Some(system.join("harbour/stereo/message-new-instant.oga"))
        );
        assert_eq!(
            found("dialog-warning"),
            Some(user.join("harbour/stereo/dialog-warning.ogg"))
        );
        assert_eq!(
            found("bell-terminal"),
            Some(system.join("reef/stereo/bell.wav")),
            "a shorter form, from the inherited theme"
        );
        assert_eq!(
            found("complete-download"),
            Some(system.join("freedesktop/stereo/complete.oga")),
            "freedesktop last of all"
        );
        assert_eq!(found("phone-incoming-call"), None);
        assert_eq!(
            find("complete", "no-such-theme", &bases),
            Some(system.join("freedesktop/stereo/complete.oga")),
            "a missing theme still reaches freedesktop"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_disabled_sound_stays_silent_and_names_cant_leave_the_theme() {
        let root = scratch("disabled");
        theme(&root, "freedesktop", STEREO, &["bell.oga", "complete.oga"]);
        theme(&root, "quiet", STEREO, &["bell.disabled"]);
        let bases = [root.clone()];
        assert_eq!(find("bell", "quiet", &bases), None, "silenced on purpose");
        assert!(find("complete", "quiet", &bases).is_some());
        assert_eq!(find("../freedesktop/stereo/bell", "quiet", &bases), None);
        assert_eq!(
            find("bell", "../freedesktop", &bases).map(|_| ()),
            Some(()),
            "a bad theme name is skipped, not followed"
        );
        assert_eq!(find("", "quiet", &bases), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_inheritance_loop_ends() {
        let root = scratch("loop");
        let inherits =
            |parent: &str| STEREO.replace("Name=T\n", &format!("Name=T\nInherits={parent}\n"));
        theme(&root, "a", &inherits("b"), &[]);
        theme(&root, "b", &inherits("a"), &["found.oga"]);
        assert_eq!(
            find("found", "a", std::slice::from_ref(&root)),
            Some(root.join("b/stereo/found.oga"))
        );
        assert_eq!(find("missing", "a", std::slice::from_ref(&root)), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stereo_folders_come_first() {
        let index = parse_index(
            "[Sound Theme]\nName=X\nInherits=freedesktop, ../etc\nDirectories=5.1,stereo\n\n\
             [5.1]\nOutputProfile=5.1\n\n[stereo]\nOutputProfile=stereo\n",
        );
        assert_eq!(index.directories, ["stereo", "5.1"]);
        assert_eq!(index.inherits, ["freedesktop"], "no path in a theme name");
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

    #[test]
    fn shorter_forms_drop_a_part_at_a_time() {
        assert_eq!(
            shorter_forms("message-new-instant").collect::<Vec<_>>(),
            ["message-new-instant", "message-new", "message"]
        );
        assert_eq!(shorter_forms("bell").collect::<Vec<_>>(), ["bell"]);
        assert_eq!(shorter_forms("-x").collect::<Vec<_>>(), ["-x"]);
    }
}

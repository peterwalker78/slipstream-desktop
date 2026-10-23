//! Slipstream's own sounds: the click the volume makes when it changes, and the sound a
//! notification makes. Each is one of a few presets or a file of the user's own.
//!
//! The presets are made here, note by note, rather than shipped as samples, so there's nothing to
//! licence and nothing to find at runtime. Each is written out as a WAV under the runtime
//! directory the first time it's wanted, and played with PipeWire's `pw-play`.
//!
//! The sound theme is here too: a name is looked up as the XDG Sound Theme spec describes, in the
//! chosen theme, then the themes it inherits from, then `freedesktop`; in each, the name and then
//! its shorter forms (`message-new-instant`, `message-new`, `message`), so a theme without the
//! exact sound still has something close. A `.disabled` file silences a sound on purpose, and the
//! lookup stops there.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// A sound made of notes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    /// What the settings file calls it.
    pub id: &'static str,
    /// What Settings calls it.
    pub name: &'static str,
    notes: &'static [Note],
    /// The loudest it gets, from 0 to 1.
    peak: f64,
}

/// One note: a pitch that may slide, its overtones, and how quickly it dies away.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Note {
    /// When it starts, in seconds from the start of the sound.
    at: f64,
    /// How long it lasts. The note fades to silence by then.
    secs: f64,
    /// Its pitch in hertz as it starts, and where it slides to over `slide` seconds.
    hz: f64,
    to_hz: f64,
    slide: f64,
    /// How loud it is beside the sound's other notes.
    gain: f64,
    /// How quickly it dies away: the time it takes to fall to about a third.
    decay: f64,
    /// Its overtones, as multiples of its pitch and how loud each is. The first is the note.
    partials: &'static [(f64, f64)],
}

const PURE: &[(f64, f64)] = &[(1.0, 1.0)];
/// A bell: overtones at whole multiples, quieter as they go up.
const BELL: &[(f64, f64)] = &[(1.0, 1.0), (2.0, 0.32), (3.0, 0.1)];
/// Struck glass: overtones that don't line up, which is what makes it ring rather than hum.
const GLASS: &[(f64, f64)] = &[(1.0, 1.0), (2.76, 0.38), (5.4, 0.16), (8.93, 0.06)];
/// A bar of wood: one strong overtone far above the note.
const WOOD: &[(f64, f64)] = &[(1.0, 1.0), (3.93, 0.32), (9.2, 0.06)];

const fn note(
    at: f64,
    secs: f64,
    hz: f64,
    gain: f64,
    decay: f64,
    partials: &'static [(f64, f64)],
) -> Note {
    Note {
        at,
        secs,
        hz,
        to_hz: hz,
        slide: 0.0,
        gain,
        decay,
        partials,
    }
}

const fn sliding(
    at: f64,
    secs: f64,
    hz: f64,
    to_hz: f64,
    slide: f64,
    gain: f64,
    decay: f64,
) -> Note {
    Note {
        at,
        secs,
        hz,
        to_hz,
        slide,
        gain,
        decay,
        partials: PURE,
    }
}

/// The volume's clicks: each a moment long, since one plays for every step.
pub const CLICKS: &[Preset] = &[
    Preset {
        id: "tap",
        name: "Tap",
        notes: &[note(0.0, 0.040, 1000.0, 1.0, 0.040 / 3.0, PURE)],
        peak: 0.35,
    },
    Preset {
        id: "tick",
        name: "Tick",
        notes: &[note(0.0, 0.018, 2400.0, 1.0, 0.005, PURE)],
        peak: 0.3,
    },
    Preset {
        id: "wood",
        name: "Wood",
        notes: &[note(0.0, 0.050, 640.0, 1.0, 0.012, WOOD)],
        peak: 0.4,
    },
    Preset {
        id: "bubble",
        name: "Bubble",
        notes: &[sliding(0.0, 0.050, 520.0, 1250.0, 0.035, 1.0, 0.016)],
        peak: 0.35,
    },
    Preset {
        id: "pop",
        name: "Pop",
        notes: &[sliding(0.0, 0.045, 330.0, 140.0, 0.030, 1.0, 0.012)],
        peak: 0.5,
    },
];

/// Notification sounds: a moment, then a ring that fades.
pub const DINGS: &[Preset] = &[
    Preset {
        id: "chime",
        name: "Chime",
        notes: &[
            note(0.0, 0.9, 1318.5, 0.8, 0.22, BELL),
            note(0.11, 1.0, 1975.5, 1.0, 0.26, BELL),
        ],
        peak: 0.45,
    },
    Preset {
        id: "glass",
        name: "Glass",
        notes: &[note(0.0, 1.2, 1760.0, 1.0, 0.3, GLASS)],
        peak: 0.4,
    },
    Preset {
        id: "droplets",
        name: "Droplets",
        notes: &[
            sliding(0.0, 0.16, 620.0, 1400.0, 0.06, 1.0, 0.05),
            sliding(0.13, 0.18, 820.0, 1850.0, 0.06, 0.7, 0.05),
        ],
        peak: 0.5,
    },
    Preset {
        id: "marimba",
        name: "Marimba",
        notes: &[
            note(0.0, 0.35, 1046.5, 0.9, 0.08, WOOD),
            note(0.08, 0.35, 1318.5, 0.9, 0.08, WOOD),
            note(0.16, 0.5, 1568.0, 1.0, 0.11, WOOD),
        ],
        peak: 0.45,
    },
    Preset {
        id: "signal",
        name: "Signal",
        notes: &[
            note(0.0, 0.09, 1174.7, 1.0, 0.06, BELL),
            note(0.13, 0.16, 1174.7, 1.0, 0.08, BELL),
        ],
        peak: 0.4,
    },
];

/// The notification sound that means each app's own, as it asks for one.
pub const APPS_OWN: &str = "app";

/// Notification sounds from the sound theme, by the names apps ask for, to make for every
/// notification: what the settings file calls each, what Settings calls it, and its name.
pub const THEMED: &[(&str, &str, &str)] = &[("ding", "Ding", "complete"), ("bell", "Bell", "bell")];

/// The sound theme's name for `choice`, if it's one of `THEMED`.
pub fn themed(choice: &str) -> Option<&'static str> {
    THEMED
        .iter()
        .find(|(id, ..)| *id == choice)
        .map(|(.., name)| *name)
}

/// How far a theme's inheritance is followed, against a loop or a runaway chain.
const THEMES: usize = 16;
/// The file types a sound theme holds, in the spec's order.
const EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];

/// Where sound themes live: `$XDG_DATA_HOME/sounds`, then `sounds` under each of
/// `$XDG_DATA_DIRS`, with the spec's defaults for either when unset.
pub fn theme_bases() -> Vec<PathBuf> {
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    std::iter::once(crate::xdg_home("XDG_DATA_HOME", ".local/share"))
        .chain(
            system
                .split(':')
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

/// The first `index.theme` for `theme` under `bases`, read with `read`. Nothing when no base has
/// the theme.
fn index(theme: &str, bases: &[PathBuf], read: &dyn Fn(&Path) -> Option<String>) -> Option<Index> {
    bases.iter().find_map(|base| {
        let text = read(&base.join(theme).join("index.theme"))?;
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
/// the search with nothing. Each theme's `index.theme` is read with `read`, which may refuse a file
/// that isn't small and regular.
pub fn find(
    name: &str,
    theme: &str,
    bases: &[PathBuf],
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<PathBuf> {
    if !plain_component(name) {
        return None;
    }
    let mut chain: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![theme.to_string()];
    while let Some(next) = (!queue.is_empty()).then(|| queue.remove(0)) {
        if chain.len() >= THEMES || chain.contains(&next) || !plain_component(&next) {
            continue;
        }
        if let Some(index) = index(&next, bases, read) {
            queue.extend(index.inherits.iter().cloned());
            chain.push(next);
        }
    }
    if !chain.iter().any(|theme| theme == "freedesktop") {
        chain.push("freedesktop".to_string());
    }
    for theme in &chain {
        let Some(index) = index(theme, bases, read) else {
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

/// Files bigger than this aren't a sound to play, whatever they are.
const LARGEST: u64 = 8 << 20;
const RATE: u32 = 48_000;

/// No more than one click in this long, so a key held down doesn't queue up dozens of them.
const CLICK_EVERY: Duration = Duration::from_millis(120);
static LAST_CLICK: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether a click may play now: true at most once in `CLICK_EVERY`.
pub fn due() -> bool {
    let mut last = LAST_CLICK.lock().unwrap();
    let now = Instant::now();
    if last.is_some_and(|then| now.duration_since(then) < CLICK_EVERY) {
        return false;
    }
    *last = Some(now);
    true
}

/// Whether `choice` is a file of the user's own rather than one of the presets.
pub fn is_own_file(choice: &str) -> bool {
    Path::new(choice).is_absolute()
}

/// The file to play for `choice`: one of `presets` by its id, or a file of the user's own by its
/// path. `None` for a preset that doesn't exist or a file that isn't there to play.
pub fn file(choice: &str, presets: &[Preset]) -> Option<PathBuf> {
    if is_own_file(choice) {
        let path = PathBuf::from(choice);
        // A FIFO or a device would leave the player waiting on it for good.
        let playable =
            std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() <= LARGEST);
        return playable.then_some(path);
    }
    let preset = presets.iter().find(|preset| preset.id == choice)?;
    written(preset)
}

/// `preset` as a file under the runtime directory, written the first time it's wanted by this
/// process, so a file from an older build that made it differently is replaced.
fn written(preset: &Preset) -> Option<PathBuf> {
    static WRITTEN: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?)
        .join("slipstream")
        .join("sounds");
    let path = dir.join(format!("{}.wav", preset.id));
    let mut written = WRITTEN.get_or_init(Mutex::default).lock().unwrap();
    if !written.contains(&preset.id) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, wav(&render(preset), RATE, 1)).ok()?;
        written.push(preset.id);
    }
    Some(path)
}

/// `preset` as 16-bit samples: its notes added together, each rising in 2 ms so it doesn't click
/// in its own right and fading to silence by its end, then scaled to the preset's peak.
fn render(preset: &Preset) -> Vec<i16> {
    let end = preset
        .notes
        .iter()
        .map(|note| note.at + note.secs)
        .fold(0.0, f64::max);
    let mut mix = vec![0.0f64; (end * RATE as f64).ceil() as usize];
    for note in preset.notes {
        let first = (note.at * RATE as f64) as usize;
        let frames = (note.secs * RATE as f64) as usize;
        let floor = (-note.secs / note.decay).exp();
        let mut phase = 0.0f64;
        for frame in 0..frames.min(mix.len() - first) {
            let t = frame as f64 / RATE as f64;
            let hz = if note.slide > 0.0 {
                note.hz + (note.to_hz - note.hz) * (t / note.slide).min(1.0)
            } else {
                note.hz
            };
            phase += std::f64::consts::TAU * hz / RATE as f64;
            let attack = (t / 0.002).min(1.0);
            // Falls away from 1 and reaches exactly 0 at the note's end.
            let decay = (((-t / note.decay).exp() - floor) / (1.0 - floor)).max(0.0);
            let tone: f64 = note
                .partials
                .iter()
                .map(|(ratio, amp)| {
                    // Higher overtones die away sooner, as they do in anything struck.
                    let fade = if *ratio > 1.0 {
                        (-t * ratio.sqrt() / note.decay).exp() / (-t / note.decay).exp()
                    } else {
                        1.0
                    };
                    amp * fade * (phase * ratio).sin()
                })
                .sum();
            mix[first + frame] += note.gain * attack * decay * tone;
        }
    }
    let loudest = mix.iter().fold(0.0f64, |most, s| most.max(s.abs()));
    let scale = if loudest > 0.0 {
        preset.peak / loudest
    } else {
        0.0
    };
    mix.iter()
        .map(|s| (s * scale * i16::MAX as f64) as i16)
        .collect()
}

/// `samples` wrapped in a canonical 44-byte WAV header: 16-bit PCM, `channels` at `rate`.
fn wav(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let bytes_per_frame = u32::from(channels) * 2;
    let data = (samples.len() * 2) as u32;
    let mut file = Vec::with_capacity(44 + data as usize);
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + data).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes()); // PCM
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * bytes_per_frame).to_le_bytes());
    file.extend_from_slice(&(bytes_per_frame as u16).to_le_bytes());
    file.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    file.extend_from_slice(b"data");
    file.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        file.extend_from_slice(&sample.to_le_bytes());
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("slipstream-sounds-{name}-{}", std::process::id()));
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

        let found = |name: &str| find(name, "harbour", &bases, &read);
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
            find("complete", "no-such-theme", &bases, &read),
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
        assert_eq!(
            find("bell", "quiet", &bases, &read),
            None,
            "silenced on purpose"
        );
        assert!(find("complete", "quiet", &bases, &read).is_some());
        assert_eq!(
            find("../freedesktop/stereo/bell", "quiet", &bases, &read),
            None
        );
        assert_eq!(
            find("bell", "../freedesktop", &bases, &read).map(|_| ()),
            Some(()),
            "a bad theme name is skipped, not followed"
        );
        assert_eq!(find("", "quiet", &bases, &read), None);
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
            find("found", "a", std::slice::from_ref(&root), &read),
            Some(root.join("b/stereo/found.oga"))
        );
        assert_eq!(
            find("missing", "a", std::slice::from_ref(&root), &read),
            None
        );
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
    fn shorter_forms_drop_a_part_at_a_time() {
        assert_eq!(
            shorter_forms("message-new-instant").collect::<Vec<_>>(),
            ["message-new-instant", "message-new", "message"]
        );
        assert_eq!(shorter_forms("bell").collect::<Vec<_>>(), ["bell"]);
        assert_eq!(shorter_forms("-x").collect::<Vec<_>>(), ["-x"]);
    }

    #[test]
    fn every_preset_is_a_playable_wav_that_starts_and_ends_at_silence() {
        for preset in CLICKS.iter().chain(DINGS) {
            let samples = render(preset);
            let file = wav(&samples, RATE, 1);
            assert_eq!(&file[..4], b"RIFF");
            assert_eq!(&file[8..16], b"WAVEfmt ");
            assert_eq!(&file[36..40], b"data");
            let data = u32::from_le_bytes(file[40..44].try_into().unwrap()) as usize;
            assert_eq!(data, file.len() - 44);
            assert_eq!(
                u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize,
                file.len() - 8
            );
            // Starting or ending part way up a wave is a click of its own, on top of this one.
            assert_eq!(samples[0], 0, "{}", preset.id);
            assert!(
                samples.last().unwrap().abs() < 64,
                "{} ends on {:?}",
                preset.id,
                samples.last()
            );
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            let wanted = (preset.peak * i16::MAX as f64) as u16;
            assert!(
                peak <= wanted && peak > wanted - 64,
                "{}: {peak}",
                preset.id
            );
        }
    }

    #[test]
    fn clicks_are_a_moment_and_ids_are_their_own() {
        for preset in CLICKS {
            let longest = preset
                .notes
                .iter()
                .map(|n| n.at + n.secs)
                .fold(0.0, f64::max);
            assert!(longest <= 0.06, "{} lasts {longest}s", preset.id);
        }
        let mut ids: Vec<&str> = CLICKS.iter().chain(DINGS).map(|p| p.id).collect();
        ids.push(APPS_OWN);
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn a_choice_is_a_preset_or_a_file_of_ones_own() {
        assert!(!is_own_file("chime"));
        assert!(is_own_file("/home/someone/sounds/ding.ogg"));
        assert_eq!(file("no-such-preset", DINGS), None);
        assert_eq!(file("/no/such/file.ogg", DINGS), None);
    }
}

//! Slipstream's settings file, `~/.config/slipstream/settings.toml`. The compositor reads it at
//! startup and again whenever it changes; `slipstream-settings` and quick settings write it. Every
//! setting's name, default and meaning lives here, so they never disagree.
//!
//! ```toml
//! [wallpaper]
//! fade-after-secs = 120
//! variations = ["slipstream"]
//! change-every-mins = 10
//!
//! [motion]
//! reduced = false
//!
//! [display]
//! night-light = false
//! night-light-schedule = "off"
//! night-light-from = "21:00"
//! night-light-to = "07:00"
//!
//! [notifications]
//! do-not-disturb = false
//! wait-while-typing = true
//! longest-wait-mins = 15
//! sounds = true
//! sound-theme = "ocean"
//!
//! [sound]
//! volume-blip = true
//!
//! [clipboard]
//! history = true
//!
//! [session]
//! remember = false
//! reopen-without-asking = false
//!
//! [borders]
//! selected-tile = "#42d3ff"
//! bullet-time = "#ffb547"
//!
//! [[workspaces.list]]
//! id = 1
//! name = "Mail"
//!
//! [[workspaces.list]]
//! id = 2
//! name = ""
//! ```
//!
//! Missing keys take their defaults and unknown ones are ignored, so an older compositor still
//! reads a newer file.

pub mod battery;
pub mod sun;

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// The file's name inside Slipstream's config folder.
pub const FILE_NAME: &str = "settings.toml";

/// Written at the top of the file.
const HEADER: &str = "\
# Slipstream's settings. The Settings app (Super+I) and quick settings (Super+A) write this file,
# and Slipstream applies each change as soon as it's saved. Hand edits work too, but the apps
# don't keep comments.

";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    pub wallpaper: Wallpaper,
    pub motion: Motion,
    pub display: Display,
    pub notifications: Notifications,
    pub borders: Borders,
    pub session: Session,
    pub lock: Lock,
    pub sound: Sound,
    pub clipboard: Clipboard,
    pub workspaces: Workspaces,
}

/// The living wallpaper, and when the UI steps aside for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Wallpaper {
    /// Seconds without keyboard or pointer input before the UI fades out; 0 never fades.
    pub fade_after_secs: u64,
    /// Which variations to show, by id; they take turns in a shuffled order. Empty means all of
    /// them. Unknown ids are ignored, and if that leaves none, the default is used.
    pub variations: Vec<String>,
    /// Minutes one variation holds before the next takes over. Ignored when only one is chosen;
    /// 0 never changes.
    pub change_every_mins: u64,
}

impl Default for Wallpaper {
    fn default() -> Self {
        Self {
            fade_after_secs: 120,
            variations: vec![VARIATIONS[0].id.to_string()],
            change_every_mins: 10,
        }
    }
}

impl Wallpaper {
    /// The variations to cycle through: the ones named in the file, in the file's order, or all
    /// of them if it names none. Never empty — a file that names only ids nobody knows falls
    /// back to the default.
    pub fn chosen(&self) -> Vec<&'static Variation> {
        if self.variations.is_empty() {
            return VARIATIONS.iter().collect();
        }
        let mut chosen: Vec<&'static Variation> = Vec::new();
        for id in &self.variations {
            let id = current_id(id);
            if let Some(variation) = VARIATIONS.iter().find(|v| v.id == id) {
                // A file naming a variation and the one it replaced shows it once.
                if !chosen.contains(&variation) {
                    chosen.push(variation);
                }
            }
        }
        if chosen.is_empty() {
            vec![&VARIATIONS[0]]
        } else {
            chosen
        }
    }
}

/// Variations that were replaced, and the id of what replaced each, so a settings file written
/// before the change keeps showing something in the same place in its list.
const REPLACED: &[(&str, &str)] = &[
    ("decode", "vortex"),
    ("rain", "departures"),
    ("wind-tunnel", "coral"),
    ("flow-field", "attractor"),
    ("shear", "murmuration"),
    ("turntable", "murmuration"),
    ("mach", "physarum"),
    ("canyon", "physarum"),
    ("console", "maze"),
];

/// The id a settings file's variation goes by now: itself, or what replaced it.
pub fn current_id(id: &str) -> &str {
    REPLACED
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id, |(_, new)| new)
}

/// One living-wallpaper variation, as the settings file and the Settings app know it. The
/// compositor builds one for each id; `saver::make` and this table must agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variation {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
}

/// Every variation, in the order the Settings app lists them.
/// The compositor's `saver::make` builds one for each id, and a test there checks the two agree,
/// so a variation joins this list in the same commit as its code.
pub const VARIATIONS: &[Variation] = &[
    Variation {
        id: "slipstream",
        title: "Slipstream",
        blurb: "The logo arrives letter by letter, holds, and peels away downwind in smoke.",
    },
    Variation {
        id: "vortex",
        title: "Vortex",
        blurb: "The logo sits in the eye of a turning tunnel of light, and rings on the minute.",
    },
    Variation {
        id: "departures",
        title: "Departures",
        blurb: "A split-flap board with the time, date, workspace and battery, turning as they \
                change.",
    },
    Variation {
        id: "prompt",
        title: "Prompt",
        blurb: "A command is typed at a terminal, and prints the logo as a banner.",
    },
    Variation {
        id: "circuit",
        title: "Circuit",
        blurb: "Tracks are routed in from the edges, and pulses run along them to the logo.",
    },
    Variation {
        id: "life",
        title: "Life",
        blurb: "Conway's Game of Life, seeded with the logo, with a spark dropped in when it \
                settles.",
    },
    Variation {
        id: "sonar",
        title: "Sonar",
        blurb: "A beam sweeps round, and everything it touches answers and fades behind it.",
    },
    Variation {
        id: "tide",
        title: "Tide",
        blurb: "Swell crosses the screen in shaded bands, and the logo surfaces out of it.",
    },
    Variation {
        id: "warp",
        title: "Warp",
        blurb: "A steering tunnel of streaks, and the logo comes up out of its vanishing point.",
    },
    Variation {
        id: "glitch",
        title: "Glitch",
        blurb: "The picture smears like a broken video stream, then swirls away.",
    },
    Variation {
        id: "contours",
        title: "Contours",
        blurb: "A pressure map of the air round the logo, with a weather system drifting across.",
    },
    Variation {
        id: "contrails",
        title: "Contrails",
        blurb: "Aircraft leave trails that light up the logo, and now and then skywrite the time.",
    },
    Variation {
        id: "coral",
        title: "Coral",
        blurb: "The letters seed a chemical reaction that grows over the screen in stripes and \
                spots, then dies back.",
    },
    Variation {
        id: "attractor",
        title: "Attractor",
        blurb: "The logo unravels into a strange attractor that slowly changes shape, then winds \
                back into letters.",
    },
    Variation {
        id: "murmuration",
        title: "Murmuration",
        blurb: "The logo takes off as a flock of starlings that wheels round the screen, scatters \
                from a falcon, and lands back in the letters.",
    },
    Variation {
        id: "maze",
        title: "Maze",
        blurb: "A walk through a maze, seen first hand, to the logo on the wall at its end.",
    },
    Variation {
        id: "physarum",
        title: "Physarum",
        blurb: "A slime mould creeps out of the letters, spreads over the screen as a network of \
                veins, then draws back into the logo.",
    },
    Variation {
        id: "galaxies",
        title: "Galaxies",
        blurb: "The two halves of the logo wind up into spiral galaxies that collide and merge, \
                then the collision runs backwards into letters.",
    },
    Variation {
        id: "chladni",
        title: "Chladni",
        blurb: "The logo is sand on a ringing plate: each note shakes it into a new figure, and \
                then it walks home.",
    },
    Variation {
        id: "frost",
        title: "Frost",
        blurb: "Frost grows out of the letters in branching ferns, glitters, and melts back the \
                way it came.",
    },
];

/// The most workspaces there can be. The bar has to find room for every one of them.
pub const MAX_WORKSPACES: usize = 20;

/// The workspaces, in order, each with a name of its own or none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Workspaces {
    pub list: Vec<WorkspaceEntry>,
}

/// One workspace. The id is what the compositor follows when the list is edited, so renaming,
/// reordering or deleting its neighbours never moves a window off it; the name is what shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct WorkspaceEntry {
    pub id: u32,
    /// Empty means the workspace goes by its number.
    pub name: String,
}

impl Default for Workspaces {
    /// Five, going by their numbers: the desktop as it was before workspaces had names.
    fn default() -> Self {
        Self {
            list: (1..=5)
                .map(|id| WorkspaceEntry {
                    id,
                    name: String::new(),
                })
                .collect(),
        }
    }
}

impl Workspaces {
    /// The list as the compositor uses it: never empty, never over `MAX_WORKSPACES`, each id once
    /// (a repeated or missing id gets a fresh one rather than two workspaces merging), and names
    /// trimmed.
    pub fn entries(&self) -> Vec<WorkspaceEntry> {
        let mut out: Vec<WorkspaceEntry> = Vec::new();
        for entry in self.list.iter().take(MAX_WORKSPACES) {
            let id = if entry.id == 0 || out.iter().any(|seen| seen.id == entry.id) {
                let taken: Vec<u32> = self
                    .list
                    .iter()
                    .chain(out.iter())
                    .map(|entry| entry.id)
                    .collect();
                unused_id(&taken)
            } else {
                entry.id
            };
            out.push(WorkspaceEntry {
                id,
                name: entry.name.trim().to_string(),
            });
        }
        if out.is_empty() {
            out.push(WorkspaceEntry {
                id: 1,
                name: String::new(),
            });
        }
        out
    }

    /// An id no workspace in the list has, for one being added.
    pub fn fresh_id(&self) -> u32 {
        let taken: Vec<u32> = self.list.iter().map(|entry| entry.id).collect();
        unused_id(&taken)
    }
}

/// An id none of `taken` is: one past the highest, or, when the highest is already `u32::MAX` (a
/// hand edit can put it there), the lowest free one from 1. There are never more than a few dozen
/// ids, so the search always ends.
pub fn unused_id(taken: &[u32]) -> u32 {
    match taken.iter().max() {
        None => 1,
        Some(highest) => highest
            .checked_add(1)
            .unwrap_or_else(|| (1..u32::MAX).find(|id| !taken.contains(id)).unwrap_or(1)),
    }
}

/// What a workspace is called on screen: its name, or its number counting from 1.
pub fn workspace_label(name: &str, index: usize) -> String {
    let name = name.trim();
    if name.is_empty() {
        (index + 1).to_string()
    } else {
        name.to_string()
    }
}

/// How windows and effects move.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Motion {
    /// Reduced motion: moves jump, and every effect becomes a short fade.
    pub reduced: bool,
}

/// The screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Display {
    /// Night light: warmer colours on every screen. On a schedule, Slipstream turns it on and off
    /// itself; turning it by hand in between lasts until the schedule's next change.
    pub night_light: bool,
    /// When night light comes on by itself.
    pub night_light_schedule: NightSchedule,
    /// A custom schedule's start and end, `HH:MM` local time; night runs past midnight if the end
    /// is earlier than the start.
    pub night_light_from: String,
    pub night_light_to: String,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            night_light: false,
            night_light_schedule: NightSchedule::Off,
            night_light_from: "21:00".into(),
            night_light_to: "07:00".into(),
        }
    }
}

/// Night light's schedule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NightSchedule {
    /// Only by hand.
    #[default]
    Off,
    /// From sunset to sunrise where the time zone's city is.
    Sunset,
    /// Between `night-light-from` and `night-light-to`.
    Custom,
}

/// How long night light takes to warm up or cool down when its schedule turns it, in minutes.
pub const NIGHT_FADE_MINS: f64 = 30.0;

impl Display {
    /// Tonight's night, in local minutes after midnight: when it starts and ends for `date`, at a
    /// clock `offset_mins` ahead of UTC. `None` without a schedule, and for sunset where the sun
    /// doesn't set or the time zone's place is unknown, in which case the custom hours stand in.
    pub fn night_window(
        &self,
        date: (i64, u32, u32),
        offset_mins: f64,
        location: Option<(f64, f64)>,
    ) -> Option<(f64, f64)> {
        let custom = || {
            Some((
                sun::parse_time(&self.night_light_from).unwrap_or(21.0 * 60.0),
                sun::parse_time(&self.night_light_to).unwrap_or(7.0 * 60.0),
            ))
        };
        match self.night_light_schedule {
            NightSchedule::Off => None,
            NightSchedule::Custom => custom(),
            NightSchedule::Sunset => location
                .and_then(|(lat, lon)| sun::sun_times(lat, lon, date, offset_mins))
                .map(|(sunrise, sunset)| (sunset, sunrise))
                .or_else(custom),
        }
    }
}

/// Notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Notifications {
    /// Do not disturb: notifications still reach the notification centre, but don't pop up.
    pub do_not_disturb: bool,
    /// Pop-ups wait while you're typing and show at the next pause, together. Critical ones
    /// don't wait.
    pub wait_while_typing: bool,
    /// The longest a pop-up waits for a pause, in minutes. Urgent messages tend to reach a phone
    /// first, so a quarter of an hour of unbroken typing is left alone. At least 1.
    pub longest_wait_mins: u64,
    /// Plays the sound a notification asks for, by file or by name from the sound theme. Not
    /// while you're looking at the window it came from, and under do not disturb only for
    /// critical ones.
    pub sounds: bool,
    /// The freedesktop sound theme sound names are looked up in, falling back through the
    /// themes it inherits from to `freedesktop`.
    pub sound_theme: String,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            do_not_disturb: false,
            wait_while_typing: true,
            longest_wait_mins: 15,
            sounds: true,
            sound_theme: "ocean".into(),
        }
    }
}

impl Notifications {
    /// `longest_wait_mins` in seconds, never under a minute.
    pub fn longest_wait_secs(&self) -> f64 {
        self.longest_wait_mins.max(1) as f64 * 60.0
    }
}

/// The sounds Slipstream makes itself, as against the ones apps make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Sound {
    /// A short click at the new level whenever the volume keys change it, as macOS and Windows
    /// do, so the setting can be heard as well as seen. Never on mute, never on brightness.
    pub volume_blip: bool,
}

impl Default for Sound {
    fn default() -> Self {
        Self { volume_blip: true }
    }
}

/// What's open when the session ends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Session {
    /// Record which apps were open, on which workspace and in which tiling place, so the layout
    /// can be put back. Off by default: with it off nothing is written to disk at all. The way
    /// out's card carries the same switch, and turning it on there turns it on here.
    pub remember: bool,
    /// Put the layout back at the next login without offering first. Only means anything with
    /// `remember` on, since with it off there is nothing recorded to put back.
    pub reopen_without_asking: bool,
}

/// The clipboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Clipboard {
    /// Keep the last things copied, text and pictures, for Super+V to paste again. In memory
    /// only, forgotten at logout, and never what a password manager marks as secret. Turning it
    /// off forgets everything kept.
    pub history: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self { history: true }
    }
}

/// The lock screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Lock {
    /// Lock after this many minutes with no keyboard or mouse input; 0 never does. Off by
    /// default: only Super+L, quick settings, `loginctl lock-session` and sleep lock.
    pub after_idle_mins: u64,
    /// Lock just before the machine sleeps, so it wakes up locked.
    pub before_sleep: bool,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            after_idle_mins: 0,
            before_sleep: true,
        }
    }
}

/// The rings that mark what's selected. Each is `#rrggbb`; a value that isn't falls back to the
/// default, so a mistyped hand edit leaves the desktop usable rather than colourless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Borders {
    /// The focused window's ring, and the keyboard's ring in quick settings and the notification
    /// centre: one colour for "this is what the keys are aimed at".
    pub selected_tile: String,
    /// Bullet time's rings around the chosen window and workspace, and its workspace frames.
    pub bullet_time: String,
}

/// Ice cyan. It sits 73° of hue from the code rain, so the ring reads against it rather than
/// dissolving into it, and it is the mockup's colour for what's active.
pub const SELECTED_TILE: &str = "#42d3ff";
/// The mockup's amber, near-opposite the ring's cyan: cold means the live desktop, warm the slow
/// world.
pub const BULLET_TIME: &str = "#ffb547";

impl Default for Borders {
    fn default() -> Self {
        Self {
            selected_tile: SELECTED_TILE.to_string(),
            bullet_time: BULLET_TIME.to_string(),
        }
    }
}

impl Borders {
    pub fn selected_tile_rgb(&self) -> [f32; 3] {
        rgb(&self.selected_tile).unwrap_or_else(|| rgb(SELECTED_TILE).expect("a valid default"))
    }

    pub fn bullet_time_rgb(&self) -> [f32; 3] {
        rgb(&self.bullet_time).unwrap_or_else(|| rgb(BULLET_TIME).expect("a valid default"))
    }
}

/// `#rrggbb` or `rrggbb`, either case, as three channels from 0 to 1. Anything else is `None`.
pub fn rgb(text: &str) -> Option<[f32; 3]> {
    let digits = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| {
        u8::from_str_radix(&digits[at..at + 2], 16)
            .ok()
            .map(|value| value as f32 / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// `$XDG_CONFIG_HOME/slipstream/settings.toml`, else `~/.config/slipstream/settings.toml`.
pub fn path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    config.join("slipstream").join(FILE_NAME)
}

/// Reads the settings at `path`. A missing file means every default.
pub fn read(path: &Path) -> io::Result<Settings> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(err) => Err(err),
    }
}

pub fn parse(text: &str) -> io::Result<Settings> {
    toml::from_str(text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Writes `settings` to `path`, creating its folder, as `replace` does.
pub fn write(path: &Path, settings: &Settings) -> io::Result<()> {
    let text =
        toml::to_string(settings).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    replace(path, format!("{HEADER}{text}").as_bytes())
}

/// Partial files left this long ago were left by a writer that died part-way.
const ABANDONED: Duration = Duration::from_secs(60);

/// Tells apart two partial files one process starts in the same nanosecond.
static PARTIALS: AtomicU64 = AtomicU64::new(0);

/// Replaces the file at `path` with `bytes`, creating its folder, so that nothing ever reads half
/// a file and a power cut leaves either the old file or the new one. The bytes go to a partial
/// file of this writer's own (`<name>.<pid>.<nanos>.<n>.partial`: the compositor and the Settings
/// app can save at the same moment), which is synced, renamed over `path`, and the folder synced
/// so the rename lasts too. Partial files of `path`'s that a dead writer left behind go.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    fs::create_dir_all(dir)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut partial_name = name.to_owned();
    partial_name.push(format!(
        ".{}.{nanos}.{}.partial",
        std::process::id(),
        PARTIALS.fetch_add(1, Ordering::Relaxed)
    ));
    let partial = dir.join(&partial_name);
    let written = File::options()
        .write(true)
        .create_new(true)
        .open(&partial)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&partial, path));
    if let Err(err) = written {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    File::open(dir)?.sync_all()?;
    remove_abandoned_partials(dir, name);
    Ok(())
}

/// Removes `name`'s partial files in `dir` that are older than `ABANDONED`. A writer still
/// working on its own is younger than that, so it's left alone.
fn remove_abandoned_partials(dir: &Path, name: &std::ffi::OsStr) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut prefix = name.to_owned();
    prefix.push(".");
    let prefix = prefix.as_encoded_bytes();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let bytes = file_name.as_encoded_bytes();
        if !bytes.starts_with(prefix) || !bytes.ends_with(b".partial") {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ABANDONED);
        if abandoned {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// For a settings file that can't be read: renames it to `settings.toml.broken-YYYYmmdd-HHMMSS`
/// (UTC) beside itself, so none of it is lost, and writes the defaults in its place. Returns
/// where the old file went.
pub fn move_aside(path: &Path) -> io::Result<PathBuf> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    let stamp = format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    let mut aside: OsString = path.as_os_str().to_owned();
    aside.push(format!(".broken-{stamp}"));
    // A second move within the same second keeps the first one's file too.
    let mut target = PathBuf::from(&aside);
    let mut n = 1;
    while target.exists() {
        n += 1;
        let mut numbered = aside.clone();
        numbered.push(format!("-{n}"));
        target = PathBuf::from(numbered);
    }
    fs::rename(path, &target)?;
    write(path, &Settings::default())?;
    Ok(target)
}

/// The civil date of a day counted from 1970-01-01. Howard Hinnant's days-to-civil algorithm,
/// which is public domain.
pub fn civil(days: i64) -> (i64, u32, u32) {
    // Shift to an era starting on 1 March 0000, so a leap day falls at the end of a year.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_means_every_default() {
        let settings = parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.wallpaper.fade_after_secs, 120);
        assert_eq!(settings.wallpaper.variations, ["slipstream"]);
        assert_eq!(settings.wallpaper.change_every_mins, 10);
        assert!(!settings.motion.reduced);
        assert!(!settings.display.night_light);
        assert!(!settings.notifications.do_not_disturb);
        assert!(settings.notifications.wait_while_typing);
        assert_eq!(settings.notifications.longest_wait_mins, 15);
        assert!(settings.notifications.sounds);
        assert_eq!(settings.notifications.sound_theme, "ocean");
        assert!(!settings.session.remember);
        assert!(!settings.session.reopen_without_asking);
        assert_eq!(
            settings.borders.selected_tile_rgb().map(round3),
            [0.259, 0.827, 1.0]
        );
        assert_eq!(
            settings.borders.bullet_time_rgb().map(round3),
            [1.0, 0.71, 0.278]
        );
    }

    #[test]
    fn lock_defaults() {
        let settings = parse("").unwrap();
        assert_eq!(settings.lock.after_idle_mins, 0, "nothing locks by itself");
        assert!(settings.lock.before_sleep);
        let set = parse("[lock]\nafter-idle-mins = 5\nbefore-sleep = false\n").unwrap();
        assert_eq!(
            set.lock,
            Lock {
                after_idle_mins: 5,
                before_sleep: false
            }
        );
    }

    fn round3(channel: f32) -> f32 {
        (channel * 1000.0).round() / 1000.0
    }

    #[test]
    fn a_colour_is_read_with_or_without_its_hash_in_either_case() {
        assert_eq!(rgb("#000000"), Some([0.0, 0.0, 0.0]));
        assert_eq!(rgb("FFFFFF"), Some([1.0, 1.0, 1.0]));
        assert_eq!(rgb(" #ffb547 "), rgb("#FFB547"));
    }

    #[test]
    fn a_colour_that_is_not_a_colour_falls_back_to_the_default() {
        for text in ["", "#fff", "mint", "#12345g", "#1234567", "rgb(1,2,3)"] {
            assert_eq!(rgb(text), None, "{text}");
        }
        let borders = Borders {
            selected_tile: "mint".into(),
            bullet_time: "#12345g".into(),
        };
        assert_eq!(borders.selected_tile_rgb(), rgb(SELECTED_TILE).unwrap());
        assert_eq!(borders.bullet_time_rgb(), rgb(BULLET_TIME).unwrap());
    }

    #[test]
    fn missing_keys_keep_their_defaults_and_unknown_ones_are_ignored() {
        let settings =
            parse("[motion]\nreduced = true\nfuture-setting = 3\n\n[portal]\nzones = 4\n").unwrap();
        assert!(settings.motion.reduced);
        assert_eq!(settings.wallpaper, Wallpaper::default());
    }

    #[test]
    fn the_chosen_variations_are_the_file_s_own_order_and_never_empty() {
        let all = Wallpaper {
            variations: Vec::new(),
            ..Default::default()
        };
        assert_eq!(all.chosen().len(), VARIATIONS.len(), "none named means all");
        let some = Wallpaper {
            variations: vec!["vortex".into(), "nothing".into(), "slipstream".into()],
            ..Default::default()
        };
        let ids: Vec<&str> = some.chosen().iter().map(|v| v.id).collect();
        assert_eq!(ids, ["vortex", "slipstream"]);
        let nonsense = Wallpaper {
            variations: vec!["nothing".into()],
            ..Default::default()
        };
        assert_eq!(nonsense.chosen(), vec![&VARIATIONS[0]]);
    }

    #[test]
    fn a_replaced_variation_s_id_still_chooses_its_replacement() {
        let old = Wallpaper {
            variations: vec!["decode".into(), "slipstream".into(), "vortex".into()],
            ..Default::default()
        };
        let ids: Vec<&str> = old.chosen().iter().map(|v| v.id).collect();
        assert_eq!(ids, ["vortex", "slipstream"], "in its place, and once");
        for (old, new) in REPLACED {
            assert!(
                VARIATIONS.iter().all(|v| v.id != *old),
                "{old} is replaced, so it isn't in the table"
            );
            assert_eq!(current_id(old), *new);
        }
        assert_eq!(current_id("tide"), "tide");
    }

    #[test]
    fn every_variation_is_named_once_with_something_to_read() {
        for (i, variation) in VARIATIONS.iter().enumerate() {
            assert!(
                VARIATIONS[..i].iter().all(|other| other.id != variation.id),
                "{} appears twice",
                variation.id
            );
            assert!(!variation.title.is_empty() && variation.blurb.len() > 20);
            assert!(
                variation
                    .id
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '-'),
                "{} should be a plain id",
                variation.id
            );
        }
    }

    #[test]
    fn workspaces_default_to_five_numbers_and_are_never_empty_or_doubled() {
        let settings = parse("").unwrap();
        let entries = settings.workspaces.entries();
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|entry| entry.name.is_empty()));
        assert_eq!(workspace_label("", 2), "3");
        assert_eq!(workspace_label("  Mail ", 0), "Mail");

        let none = Workspaces { list: Vec::new() };
        assert_eq!(none.entries().len(), 1, "never no workspaces at all");

        let doubled = parse(
            "[[workspaces.list]]\nid = 4\nname = \"Mail\"\n[[workspaces.list]]\nid = 4\nname = \"Web\"\n",
        )
        .unwrap();
        let entries = doubled.workspaces.entries();
        assert_eq!(entries.len(), 2);
        assert_ne!(
            entries[0].id, entries[1].id,
            "a repeated id doesn't merge two workspaces"
        );

        let many = Workspaces {
            list: (1..=40)
                .map(|id| WorkspaceEntry {
                    id,
                    name: String::new(),
                })
                .collect(),
        };
        assert_eq!(many.entries().len(), MAX_WORKSPACES);
        assert_eq!(many.fresh_id(), 41);
    }

    #[test]
    fn an_id_at_the_top_of_its_range_gets_no_neighbour_past_it() {
        let file = parse(
            "[[workspaces.list]]\nid = 4294967295\nname = \"Mail\"\n\
             [[workspaces.list]]\nid = 4294967295\nname = \"Web\"\n\
             [[workspaces.list]]\nid = 0\nname = \"\"\n",
        )
        .unwrap();
        let entries = file.workspaces.entries();
        let ids: Vec<u32> = entries.iter().map(|entry| entry.id).collect();
        assert_eq!(
            ids,
            [u32::MAX, 1, 2],
            "the doubled id and the missing one get free ones"
        );
        assert_eq!(file.workspaces.fresh_id(), 1);
        assert_eq!(unused_id(&[]), 1);
        assert_eq!(unused_id(&[3, 7]), 8);
    }

    #[test]
    fn a_broken_file_is_an_error_not_defaults() {
        let err = parse("[wallpaper]\nfade-after-secs = \"soon\"\n").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// A folder of its own for one test, removed when it's dropped: under the workspace's
    /// `target/test-scratch`, not the system's temporary folder, which is memory on some machines.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            let dir = manifest
                .parent()
                .unwrap_or(manifest)
                .join("target/test-scratch")
                .join(format!("config-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn two_writers_never_share_a_temp_name() {
        let scratch = Scratch::new("writers");
        let path = scratch.0.join(FILE_NAME);
        write(&path, &Settings::default()).unwrap();
        let writer = |fade_after_secs: u64| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut settings = Settings::default();
                settings.wallpaper.fade_after_secs = fade_after_secs;
                for _ in 0..200 {
                    write(&path, &settings).unwrap();
                }
            })
        };
        let (a, b) = (writer(30), writer(600));
        let mut reads = 0;
        while !(a.is_finished() && b.is_finished()) {
            let settings = read(&path).expect("every read in between parses");
            assert!([120, 30, 600].contains(&settings.wallpaper.fade_after_secs));
            reads += 1;
        }
        a.join().unwrap();
        b.join().unwrap();
        assert!(reads > 0);
        assert_eq!(scratch.names(), [FILE_NAME]);
    }

    #[test]
    fn a_write_leaves_no_partial_behind() {
        let scratch = Scratch::new("partial");
        let path = scratch.0.join(FILE_NAME);
        // One a writer that died left a while ago, one another writer is working on right now,
        // and another program's file that merely looks like one.
        let abandoned = scratch.0.join("settings.toml.4242.1.0.partial");
        let working = scratch.0.join("settings.toml.4343.2.0.partial");
        let unrelated = scratch.0.join("notes.txt.partial");
        for file in [&abandoned, &working, &unrelated] {
            fs::write(file, "half").unwrap();
        }
        let old = SystemTime::now() - Duration::from_secs(600);
        for file in [&abandoned, &unrelated] {
            File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        write(&path, &Settings::default()).unwrap();
        assert_eq!(
            scratch.names(),
            [
                "notes.txt.partial",
                FILE_NAME,
                "settings.toml.4343.2.0.partial"
            ]
        );
        assert_eq!(read(&path).unwrap(), Settings::default());
    }

    #[test]
    fn move_aside_keeps_the_bytes() {
        let scratch = Scratch::new("aside");
        let path = scratch.0.join(FILE_NAME);
        let broken = "[wallpaper]\nfade-after-secs = \"soon\"\n[[workspaces.list]]\nid = 1\nname = \"Mail\"\n";
        fs::write(&path, broken).unwrap();
        assert!(read(&path).is_err());
        let aside = move_aside(&path).unwrap();
        let aside_name = aside.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            aside_name.starts_with("settings.toml.broken-") && aside_name.len() == 36,
            "{aside_name}"
        );
        assert_eq!(fs::read_to_string(&aside).unwrap(), broken);
        assert_eq!(read(&path).unwrap(), Settings::default());
        // Twice in a second loses neither.
        fs::write(&path, "second = [").unwrap();
        let again = move_aside(&path).unwrap();
        assert_ne!(again, aside);
        assert_eq!(fs::read_to_string(&again).unwrap(), "second = [");
        assert_eq!(fs::read_to_string(&aside).unwrap(), broken);
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(20_708), (2026, 9, 12));
    }

    #[test]
    fn what_is_written_reads_back() {
        let settings = Settings {
            session: Session {
                remember: true,
                reopen_without_asking: true,
            },
            lock: Lock {
                after_idle_mins: 10,
                before_sleep: false,
            },
            wallpaper: Wallpaper {
                fade_after_secs: 300,
                variations: vec!["vortex".into(), "slipstream".into()],
                change_every_mins: 5,
            },
            motion: Motion { reduced: true },
            display: Display {
                night_light: true,
                night_light_schedule: NightSchedule::Custom,
                night_light_from: "22:15".into(),
                night_light_to: "06:45".into(),
            },
            notifications: Notifications {
                do_not_disturb: true,
                wait_while_typing: false,
                longest_wait_mins: 30,
                sounds: false,
                sound_theme: "freedesktop".into(),
            },
            borders: Borders {
                selected_tile: "#b794ff".into(),
                bullet_time: "#ffcf5c".into(),
            },
            sound: Sound { volume_blip: false },
            clipboard: Clipboard { history: false },
            workspaces: Workspaces {
                list: vec![
                    WorkspaceEntry {
                        id: 3,
                        name: "Mail".into(),
                    },
                    WorkspaceEntry {
                        id: 1,
                        name: String::new(),
                    },
                ],
            },
        };
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("fade-after-secs = 300"), "{text}");
        assert!(
            text.contains(r#"variations = ["vortex", "slipstream"]"#),
            "{text}"
        );
        assert!(text.contains("change-every-mins = 5"), "{text}");
        assert!(text.contains("night-light = true"), "{text}");
        assert!(
            text.contains(r#"night-light-schedule = "custom""#),
            "{text}"
        );
        assert!(text.contains("do-not-disturb = true"), "{text}");
        assert!(text.contains("history = false"), "{text}");
        assert!(text.contains(r##"selected-tile = "#b794ff""##), "{text}");
        assert_eq!(parse(&format!("{HEADER}{text}")).unwrap(), settings);
    }
}

//! The screens Slipstream has seen before: what each one comes back to, and how big it is.
//!
//! Shared by the compositor, which writes this, and the Settings app, which reads it to know what
//! is plugged in — a Wayland client can enumerate outputs itself, but only the compositor knows
//! the serial a screen is told apart by, so the one list is the one both agree on.
//!
//! `screen.rs` remembers a screen that goes out and comes back within a session. This is the same
//! memory across sessions, in `~/.local/state/slipstream/screens.toml`: a screen plugged into the
//! same machine next week shows what it showed last week, and a screen that was given a workspace
//! of its own keeps getting one rather than taking one of the user's.
//!
//! A screen is known by what its EDID says it is — make, model and serial — so a different monitor
//! on the same port is a different screen, and the same monitor on another port is not. Only
//! screens that say nothing useful fall back to the connector's name.
//!
//! Nothing here is a setting: the file is state, written as screens come and go, and deleting it
//! only means being asked again.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "screens.toml";

/// Bumped when the format changes in a way an older Slipstream would read wrongly.
pub const VERSION: u32 = 1;

const HEADER: &str = "\
# The screens Slipstream has seen, and the workspace each one comes back to. Written as screens
# are plugged in and out. Delete this file to be asked about every screen again.

";

/// Every screen this machine has shown a desktop on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Remembered {
    pub version: u32,
    #[serde(rename = "screen")]
    pub screens: Vec<Known>,
}

/// A mode a screen can run in: its size in its own pixels, and how many times a second it draws,
/// in millihertz as DRM gives it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ScreenMode {
    pub width: i32,
    pub height: i32,
    /// Millihertz: 59.94 Hz is 59940. Zero where the screen doesn't say.
    pub refresh: i32,
    /// The one the screen itself asks for.
    pub preferred: bool,
}

impl ScreenMode {
    /// How it is written down and read back: `1920x1080@60` or `1920x1080@59.94`. Refresh is
    /// given to two decimal places, and trailing zeroes are dropped, so the common rates read
    /// the way people say them.
    pub fn label(&self) -> String {
        let hz = self.refresh as f64 / 1000.0;
        let rounded = (hz * 100.0).round() / 100.0;
        let mut hz = format!("{rounded:.2}");
        while hz.ends_with('0') {
            hz.pop();
        }
        if hz.ends_with('.') {
            hz.pop();
        }
        format!("{}x{}@{hz}", self.width, self.height)
    }

    /// The size and refresh a label names, or `None` for one that isn't a mode at all. A refresh
    /// that is left off matches any rate at that size.
    pub fn from_label(label: &str) -> Option<(i32, i32, Option<i32>)> {
        let (size, rate) = match label.split_once('@') {
            Some((size, rate)) => (size, Some(rate)),
            None => (label, None),
        };
        let (w, h) = size.trim().split_once('x')?;
        let refresh = match rate {
            None => None,
            Some(rate) => Some((rate.trim().parse::<f64>().ok()? * 1000.0).round() as i32),
        };
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?, refresh))
    }

    /// Whether this mode is the one a label names. A refresh within half a hertz counts, so a
    /// label written by hand as `@60` still finds a 59.94 Hz mode.
    pub fn matches(&self, label: &str) -> bool {
        let Some((w, h, refresh)) = Self::from_label(label) else {
            return false;
        };
        self.width == w
            && self.height == h
            && refresh.is_none_or(|refresh| (self.refresh - refresh).abs() <= 500)
    }
}

/// One screen, and what it shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Known {
    /// What it is known by: `make model serial` from its EDID, else its connector's name.
    pub monitor: String,
    /// The connector it was last plugged into. Not what it's found by — it's here to read.
    pub connector: String,
    /// It gets a workspace of its own, made fresh, rather than one from the settings' list.
    pub own_workspace: bool,
    /// The settings id of the workspace it was last showing, when that wasn't one of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<u32>,
    /// A short name to show: the model its EDID gives, else the connector. `monitor` has to be
    /// unique, so it carries the maker and serial too and is a mouthful to read.
    pub label: String,
    /// How big it was when last lit, in logical pixels: what the Settings app shows beside its
    /// name, and what an arrangement is worked out from.
    pub width: i32,
    pub height: i32,
    /// Plugged in and lit right now. Kept up to date as screens come and go, so the Settings app
    /// can tell what is on this desk from what was on another one.
    pub lit: bool,
    /// Every mode it offers, as it last said. The Settings app can't ask DRM itself, so this is
    /// how it knows what there is to choose from.
    #[serde(rename = "mode", skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<ScreenMode>,
    /// The mode it is running now, as `ScreenMode::label` writes it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub running: String,
    /// The scale it is running now.
    #[serde(skip_serializing_if = "is_zero")]
    pub scale: f64,
}

fn is_zero(scale: &f64) -> bool {
    *scale == 0.0
}

impl Remembered {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            screens: Vec::new(),
        }
    }

    /// A file from a newer Slipstream is left alone rather than half-understood.
    pub fn is_too_new(&self) -> bool {
        self.version > VERSION
    }

    pub fn get(&self, monitor: &str) -> Option<&Known> {
        self.screens.iter().find(|known| known.monitor == monitor)
    }

    /// Records that `monitor` on `connector` shows a workspace of its own, or the one with
    /// `workspace_id`. Keeps whatever else is known about the screen.
    pub fn set(&mut self, monitor: &str, connector: &str, shows: Option<u32>) {
        let slot = self.entry(monitor);
        slot.connector = connector.to_string();
        slot.own_workspace = shows.is_none();
        slot.workspace_id = shows;
    }

    /// Records how big `monitor` is and that it is lit, for the Settings app to offer.
    pub fn set_lit(
        &mut self,
        monitor: &str,
        connector: &str,
        label: &str,
        width: i32,
        height: i32,
    ) {
        let slot = self.entry(monitor);
        slot.connector = connector.to_string();
        slot.label = label.to_string();
        slot.width = width;
        slot.height = height;
        slot.lit = true;
    }

    /// Records what `monitor` can do and what it is doing, for the Settings app to offer. The
    /// modes come from the connector, which only the compositor can read.
    pub fn set_modes(&mut self, monitor: &str, modes: Vec<ScreenMode>, running: &str, scale: f64) {
        let slot = self.entry(monitor);
        slot.modes = modes;
        slot.running = running.to_string();
        slot.scale = scale;
    }

    pub fn set_dark(&mut self, monitor: &str) {
        if let Some(slot) = self
            .screens
            .iter_mut()
            .find(|screen| screen.monitor == monitor)
        {
            slot.lit = false;
        }
    }

    /// Every screen plugged in right now, in the order they were first seen.
    pub fn lit(&self) -> impl Iterator<Item = &Known> {
        self.screens.iter().filter(|screen| screen.lit)
    }

    fn entry(&mut self, monitor: &str) -> &mut Known {
        if !self.screens.iter().any(|screen| screen.monitor == monitor) {
            self.screens.push(Known {
                monitor: monitor.to_string(),
                ..Known::default()
            });
        }
        self.screens
            .iter_mut()
            .find(|screen| screen.monitor == monitor)
            .expect("just added")
    }

    pub fn forget_monitor(&mut self, monitor: &str) {
        self.screens.retain(|known| known.monitor != monitor);
    }
}

/// `$XDG_STATE_HOME/slipstream`, else `~/.local/state/slipstream`.
pub fn state_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local")
                .join("state")
        });
    state.join("slipstream")
}

pub fn path() -> PathBuf {
    state_dir().join(FILE_NAME)
}

/// Reads the file. A missing one, an unreadable one, or one from a newer Slipstream all mean
/// "nothing remembered": this is state, and being asked again is a much better failure than
/// refusing to start.
pub fn read(path: &Path) -> Remembered {
    let Ok(text) = fs::read_to_string(path) else {
        return Remembered::new();
    };
    match toml::from_str::<Remembered>(&text) {
        Ok(remembered) if !remembered.is_too_new() => remembered,
        // A file from a newer Slipstream, or one that can't be read at all: being asked about
        // the screens again is a far better failure than refusing to start.
        _ => Remembered::new(),
    }
}

/// Writes `remembered` over the file, whole or not at all.
pub fn write(path: &Path, remembered: &Remembered) -> io::Result<()> {
    let text = toml::to_string(remembered)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::replace(path, format!("{HEADER}{text}").as_bytes())
}

/// What a screen is known by: what its EDID says, else the connector's name. Makes and models
/// come back as "Unknown" from `libdisplay-info` when the screen doesn't say, so those are
/// dropped rather than written into an identity three screens could share.
pub fn identity(make: &str, model: &str, serial: &str, connector: &str) -> String {
    let told = |field: &str| {
        let field = field.trim();
        (!field.is_empty() && !field.eq_ignore_ascii_case("unknown")).then(|| field.to_string())
    };
    let parts: Vec<String> = [make, model, serial]
        .iter()
        .filter_map(|f| told(f))
        .collect();
    if parts.is_empty() {
        connector.to_string()
    } else {
        parts.join(" ")
    }
}

impl Known {
    /// What to call it on screen: its model, or failing that whatever it is known by.
    pub fn name(&self) -> &str {
        match self.label.is_empty() {
            true => &self.monitor,
            false => &self.label,
        }
    }
}

/// The short name for a screen: the model its EDID gives, else the connector. `libdisplay-info`
/// says "Unknown" when a screen doesn't tell us, which is no use as a name.
pub fn short_name(model: &str, connector: &str) -> String {
    let model = model.trim();
    match !model.is_empty() && !model.eq_ignore_ascii_case("unknown") {
        true => model.to_string(),
        false => connector.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_prefers_what_the_screen_says() {
        assert_eq!(
            identity("Dell Inc.", "DELL U2720Q", "8J3M2P3", "DP-1"),
            "Dell Inc. DELL U2720Q 8J3M2P3"
        );
    }

    #[test]
    fn identity_falls_back_to_the_connector() {
        assert_eq!(identity("Unknown", "Unknown", "Unknown", "DP-1"), "DP-1");
        assert_eq!(identity("", "", "", "eDP-1"), "eDP-1");
    }

    #[test]
    fn identity_keeps_the_fields_a_screen_does_give() {
        assert_eq!(identity("BOE", "Unknown", "", "eDP-1"), "BOE");
    }

    #[test]
    fn a_screen_shows_its_model_where_it_gives_one() {
        assert_eq!(short_name("DELL U2720Q", "DP-1"), "DELL U2720Q");
        assert_eq!(short_name("Unknown", "DP-1"), "DP-1");
        assert_eq!(short_name("", "HDMI-A-1"), "HDMI-A-1");
    }

    #[test]
    fn a_screen_is_remembered_and_replaced() {
        let mut remembered = Remembered::new();
        remembered.set("Dell U2720Q", "DP-1", Some(3));
        assert_eq!(remembered.get("Dell U2720Q").unwrap().workspace_id, Some(3));
        assert!(!remembered.get("Dell U2720Q").unwrap().own_workspace);
        remembered.set("Dell U2720Q", "DP-2", None);
        assert_eq!(remembered.screens.len(), 1, "the same screen, not a second");
        assert!(remembered.get("Dell U2720Q").unwrap().own_workspace);
        assert_eq!(remembered.get("Dell U2720Q").unwrap().connector, "DP-2");
    }

    #[test]
    fn a_file_from_a_newer_slipstream_is_ignored() {
        let text = format!("version = {}\n", VERSION + 1);
        let remembered: Remembered = toml::from_str(&text).unwrap();
        assert!(remembered.is_too_new());
    }

    #[test]
    fn what_is_written_reads_back() {
        let mut remembered = Remembered::new();
        remembered.set("Dell U2720Q", "DP-1", Some(3));
        remembered.set("BOE panel", "eDP-1", None);
        let text = toml::to_string(&remembered).unwrap();
        let back: Remembered = toml::from_str(&text).unwrap();
        assert_eq!(back, remembered);
    }

    #[test]
    fn a_mode_reads_the_way_people_say_it() {
        let mode = |w, h, refresh| ScreenMode {
            width: w,
            height: h,
            refresh,
            preferred: false,
        };
        assert_eq!(mode(1920, 1080, 60_000).label(), "1920x1080@60");
        assert_eq!(mode(1920, 1080, 59_940).label(), "1920x1080@59.94");
        assert_eq!(mode(2880, 1800, 120_000).label(), "2880x1800@120");
        // A screen that doesn't say.
        assert_eq!(mode(1024, 768, 0).label(), "1024x768@0");
    }

    #[test]
    fn a_label_finds_its_mode_again() {
        let sixty = ScreenMode {
            width: 1920,
            height: 1080,
            refresh: 59_940,
            preferred: true,
        };
        assert!(sixty.matches(&sixty.label()), "its own label finds it");
        // Written by hand as a round number, which is how people say 59.94.
        assert!(sixty.matches("1920x1080@60"));
        // Size alone takes any rate at that size.
        assert!(sixty.matches("1920x1080"));
        assert!(!sixty.matches("1920x1080@120"));
        assert!(!sixty.matches("1280x720@60"));
        assert!(!sixty.matches("nonsense"));
    }

    #[test]
    fn modes_and_the_running_one_are_remembered_for_the_settings_app() {
        let mut remembered = Remembered::new();
        let modes = vec![ScreenMode {
            width: 2880,
            height: 1800,
            refresh: 120_000,
            preferred: true,
        }];
        remembered.set_modes("Made Up MU27 0001", modes.clone(), "2880x1800@120", 1.5);
        let known = remembered.get("Made Up MU27 0001").unwrap();
        assert_eq!(known.modes, modes);
        assert_eq!(known.running, "2880x1800@120");
        assert_eq!(known.scale, 1.5);
    }
}

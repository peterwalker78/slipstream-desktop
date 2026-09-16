//! The screens Slipstream has seen before, and what each one comes back to.
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
    /// `workspace_id`. Replaces what was there for that screen.
    pub fn set(&mut self, monitor: &str, connector: &str, shows: Option<u32>) {
        let known = Known {
            monitor: monitor.to_string(),
            connector: connector.to_string(),
            own_workspace: shows.is_none(),
            workspace_id: shows,
        };
        match self
            .screens
            .iter_mut()
            .find(|screen| screen.monitor == monitor)
        {
            Some(slot) => *slot = known,
            None => self.screens.push(known),
        }
    }

    pub fn forget_monitor(&mut self, monitor: &str) {
        self.screens.retain(|known| known.monitor != monitor);
    }
}

pub fn path() -> PathBuf {
    crate::session::state_dir().join(FILE_NAME)
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
        Ok(_) => {
            tracing::info!("the remembered screens are from a newer Slipstream; ignoring them");
            Remembered::new()
        }
        Err(err) => {
            tracing::warn!("the remembered screens can't be read: {err}");
            Remembered::new()
        }
    }
}

/// Writes `remembered` over the file, whole or not at all.
pub fn write(path: &Path, remembered: &Remembered) -> io::Result<()> {
    let text = toml::to_string(remembered)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    slipstream_config::replace(path, format!("{HEADER}{text}").as_bytes())
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
}

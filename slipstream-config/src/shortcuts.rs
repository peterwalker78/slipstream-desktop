//! Shortcuts apps were allowed to keep: the keys that work for an app while another one has the
//! keyboard, such as a voice chat's push to talk.
//!
//! Apps ask through the desktop portal, the compositor asks the person, and the answer is written
//! here, in `~/.local/state/slipstream/shortcuts.toml`, so the app isn't asked again at the next
//! login. The Settings app reads the same file to list them and takes one away by writing it back;
//! the compositor follows the file as it changes.
//!
//! Triggers are written the way the XDG shortcuts specification writes them (`CTRL+ALT+m`), and
//! beside each one the keys as a person reads them (`Ctrl+Alt+M`), so the Settings app needs no
//! keyboard tables of its own to show them.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "shortcuts.toml";

/// Bumped when the format changes in a way an older Slipstream would read wrongly.
pub const VERSION: u32 = 1;

const HEADER: &str = "\
# Shortcuts apps were allowed to keep, which work while another app has the keyboard. Written when
# an app's request is answered. Take one away in Settings, or delete this file to be asked again.

";

/// Every app that was allowed shortcuts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Granted {
    pub version: u32,
    #[serde(rename = "app")]
    pub apps: Vec<AppShortcuts>,
}

/// One app's shortcuts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AppShortcuts {
    /// Its app ID.
    pub id: String,
    /// Its name as a person reads it, for the Settings app.
    pub name: String,
    #[serde(rename = "shortcut")]
    pub shortcuts: Vec<Shortcut>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Shortcut {
    /// The app's own name for it, which is what it is told was pressed.
    pub id: String,
    /// What the app says it does.
    pub description: String,
    /// The keys, as the XDG shortcuts specification writes them: `CTRL+ALT+m`. Empty for one
    /// the person left out, so the app isn't asked about it again.
    pub trigger: String,
    /// The same keys as a person reads them: `Ctrl+Alt+M`.
    pub keys: String,
}

impl Granted {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            apps: Vec::new(),
        }
    }

    /// A file from a newer Slipstream is left alone rather than half-understood.
    pub fn is_too_new(&self) -> bool {
        self.version > VERSION
    }

    pub fn app(&self, id: &str) -> Option<&AppShortcuts> {
        self.apps.iter().find(|app| app.id == id)
    }

    /// The app's shortcut with this id.
    pub fn shortcut(&self, app: &str, id: &str) -> Option<&Shortcut> {
        self.app(app)?.shortcuts.iter().find(|s| s.id == id)
    }

    /// Writes down an answer: each of `shortcuts` replaces the app's shortcut of the same id, and
    /// the app's other shortcuts are left as they were.
    pub fn set(&mut self, app: &str, name: &str, shortcuts: Vec<Shortcut>) {
        let index = match self.apps.iter().position(|a| a.id == app) {
            Some(index) => index,
            None => {
                self.apps.push(AppShortcuts {
                    id: app.to_string(),
                    ..AppShortcuts::default()
                });
                self.apps.len() - 1
            }
        };
        let entry = &mut self.apps[index];
        entry.name = name.to_string();
        entry
            .shortcuts
            .retain(|kept| !shortcuts.iter().any(|s| s.id == kept.id));
        entry.shortcuts.extend(shortcuts);
    }

    /// Takes one shortcut away. An app left with none is forgotten altogether. Returns whether
    /// there was anything to take.
    pub fn forget(&mut self, app: &str, id: &str) -> bool {
        let Some(index) = self.apps.iter().position(|a| a.id == app) else {
            return false;
        };
        let entry = &mut self.apps[index];
        let before = entry.shortcuts.len();
        entry.shortcuts.retain(|s| s.id != id);
        let took = entry.shortcuts.len() != before;
        if entry.shortcuts.is_empty() {
            self.apps.remove(index);
        }
        took
    }

    /// Which app, other than `except`, already has `trigger`, by its name.
    pub fn held_by_another(&self, trigger: &str, except: &str) -> Option<&str> {
        self.apps
            .iter()
            .filter(|app| app.id != except)
            .find(|app| {
                app.shortcuts
                    .iter()
                    .any(|s| !s.trigger.is_empty() && s.trigger == trigger)
            })
            .map(|app| app.name.as_str())
    }
}

pub fn path() -> PathBuf {
    crate::screens::state_dir().join(FILE_NAME)
}

/// Reads the file. A missing one, an unreadable one, or one from a newer Slipstream all mean
/// "nothing allowed": apps are asked again, which is the safe way to be wrong.
pub fn read(path: &Path) -> Granted {
    let Ok(text) = fs::read_to_string(path) else {
        return Granted::new();
    };
    match toml::from_str::<Granted>(&text) {
        Ok(granted) if !granted.is_too_new() => granted,
        _ => Granted::new(),
    }
}

/// Writes `granted` over the file, whole or not at all.
pub fn write(path: &Path, granted: &Granted) -> io::Result<()> {
    let text =
        toml::to_string(granted).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::replace(path, format!("{HEADER}{text}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(id: &str, trigger: &str, keys: &str) -> Shortcut {
        Shortcut {
            id: id.to_string(),
            description: format!("{id} described"),
            trigger: trigger.to_string(),
            keys: keys.to_string(),
        }
    }

    fn talk() -> Granted {
        let mut granted = Granted::new();
        granted.set(
            "org.example.Talk",
            "Talk",
            vec![
                shortcut("push-to-talk", "CTRL+ALT+t", "Ctrl+Alt+T"),
                shortcut("mute", "CTRL+ALT+m", "Ctrl+Alt+M"),
            ],
        );
        granted
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let dir = std::env::temp_dir().join(format!("slipstream-shortcuts-{}", std::process::id()));
        let file = dir.join(FILE_NAME);
        write(&file, &talk()).unwrap();
        assert_eq!(read(&file), talk());
        assert!(
            fs::read_to_string(&file)
                .unwrap()
                .starts_with("# Shortcuts apps")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_newer_file_allows_nothing() {
        assert_eq!(
            read(Path::new("/nonexistent/shortcuts.toml")),
            Granted::new()
        );
        let newer: Granted = toml::from_str("version = 99").unwrap();
        assert!(newer.is_too_new());
    }

    #[test]
    fn an_answer_replaces_the_same_shortcut_and_keeps_the_others() {
        let mut granted = talk();
        granted.set(
            "org.example.Talk",
            "Talk",
            vec![shortcut("push-to-talk", "LOGO+t", "Super+T")],
        );
        let app = granted.app("org.example.Talk").unwrap();
        assert_eq!(app.shortcuts.len(), 2);
        assert_eq!(
            granted
                .shortcut("org.example.Talk", "push-to-talk")
                .unwrap()
                .trigger,
            "LOGO+t"
        );
        assert!(granted.shortcut("org.example.Talk", "mute").is_some());
    }

    #[test]
    fn a_shortcut_left_out_is_remembered_without_keys() {
        let mut granted = talk();
        granted.set("org.example.Talk", "Talk", vec![shortcut("mute", "", "")]);
        assert_eq!(
            granted
                .shortcut("org.example.Talk", "mute")
                .unwrap()
                .trigger,
            ""
        );
        assert_eq!(granted.held_by_another("", "org.example.Recorder"), None);
    }

    #[test]
    fn taking_the_last_shortcut_away_forgets_the_app() {
        let mut granted = talk();
        assert!(granted.forget("org.example.Talk", "mute"));
        assert!(!granted.forget("org.example.Talk", "mute"));
        assert!(granted.forget("org.example.Talk", "push-to-talk"));
        assert!(granted.apps.is_empty());
    }

    #[test]
    fn a_trigger_another_app_has_is_named_by_that_app() {
        let granted = talk();
        assert_eq!(
            granted.held_by_another("CTRL+ALT+m", "org.example.Recorder"),
            Some("Talk")
        );
        assert_eq!(
            granted.held_by_another("CTRL+ALT+m", "org.example.Talk"),
            None
        );
        assert_eq!(
            granted.held_by_another("CTRL+ALT+x", "org.example.Recorder"),
            None
        );
    }
}

//! Plugins: little programs of the person's own that each add a card to the plugin panel, the one
//! that drops from the chip beside the clock.
//!
//! A plugin is a folder holding a `plugin.toml`:
//!
//! ```toml
//! name = "Disk space"
//! description = "Free space on each drive"
//! exec = ["./disk.sh", "card"]   # the program and its words, run in the plugin's folder
//! every-secs = 60                # leave it out for a program that keeps running
//! ```
//!
//! The program is run as it is written, with no shell, so what Settings shows before a plugin is
//! turned on is exactly what runs. Its folder is its code, never written to: whatever it keeps
//! goes in a folder of its own under `$XDG_STATE_HOME/slipstream/plugins/`, which it is given as
//! `SLIPSTREAM_PLUGIN_STATE`. Folders it reads from outside are named in its manifest
//! (`read = ["$XDG_CACHE_HOME/weather"]`) and given as `SLIPSTREAM_PLUGIN_READ_0`, `_1` and on. It starts from a cleared environment: only the variables that
//! say who and where the person is, and their language, are passed on (`ENVIRONMENT`), so a
//! plugin can't use the session's keys or sockets it wasn't given.
//!
//! The program prints a card as one line of JSON, every time it runs or, when it keeps running,
//! whenever the card changes:
//!
//! ```json
//! {"lines": ["212 GB free"], "glance": "212 GB",
//!  "sections": [{"name": "Home", "meters": [{"label": "Used", "percent": 55}]}],
//!  "buttons": [{"id": "refresh", "label": "Refresh"}], "attention": false}
//! ```
//!
//! Every field is optional. `sections` are a meter's report (`meter`): named groups of
//! percentages, each with when it starts over. A pressed button runs the plugin's own program
//! again with `press <id>` after it, and that is the only thing anything on a card can do: a card
//! is shown, as text and gauges in Slipstream's own style, and nothing in it is ever run.
//!
//! Plugins live in `$XDG_DATA_HOME/slipstream/plugins/<id>/` (the person's own) and in
//! `slipstream/plugins/` under each of `$XDG_DATA_DIRS` (installed for everyone). A plugin is off
//! until it is turned on (`[plugins] on`), and turning it on records a fingerprint of its folder
//! (`fingerprint`). A plugin whose folder has changed since is held, and doesn't run, until it is
//! turned on again: an update can't slip in unseen. That guards against a plugin changing
//! underneath its approval, not against something already running as the person, which could
//! change the settings file as easily.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::meter::{Report, Section};

/// The file in a plugin's folder that says what it is.
pub const MANIFEST: &str = "plugin.toml";
/// The `[meter]` setting, shown as a plugin of its own while it has a command.
pub const METER_ID: &str = "meter";
/// The most lines, sections and buttons a card shows.
pub const MAX_LINES: usize = 6;
pub const MAX_SECTIONS: usize = 4;
pub const MAX_BUTTONS: usize = 3;
/// The longest a line or a glance is shown, in characters.
const LINE_CHARS: usize = 80;
const GLANCE_CHARS: usize = 12;
const LABEL_CHARS: usize = 20;
/// A gauge this full or fuller asks for attention, as the meter's amber does.
pub const ATTENTION_PERCENT: u8 = 75;
/// The most a plugin's folder may hold, all its files together, in bytes.
const MOST_BYTES: u64 = 64 << 20;
/// The environment a plugin's program is given: who and where the person is, their language,
/// and the way to the network through a proxy. Everything else, the session bus, the display and
/// the SSH and GPG agents among it, is left out.
pub const ENVIRONMENT: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// `[plugins]` in the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Plugins {
    /// The plugins turned on, by id, in the order their cards are shown.
    pub on: Vec<String>,
    /// The plugin whose headline shows on the chip, if any.
    pub glance: Option<String>,
    /// Each plugin's fingerprint when it was turned on, by id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fingerprints: BTreeMap<String, String>,
}

impl Plugins {
    pub fn is_on(&self, id: &str) -> bool {
        self.on.iter().any(|on| on == id)
    }

    /// Turns `id` on, with its folder's fingerprint as it is now, or off.
    pub fn set_on(&mut self, id: &str, on: Option<String>) {
        self.on.retain(|other| other != id);
        match on {
            Some(fingerprint) => {
                self.on.push(id.to_string());
                self.fingerprints.insert(id.to_string(), fingerprint);
            }
            None => {
                self.fingerprints.remove(id);
                if self.glance.as_deref() == Some(id) {
                    self.glance = None;
                }
            }
        }
    }

    /// Whether `plugin` may run: it's on, and its folder is as it was when it was turned on.
    pub fn approved(&self, plugin: &Plugin) -> bool {
        self.is_on(&plugin.id)
            && self.fingerprints.get(&plugin.id) == Some(&plugin.fingerprint)
    }
}

/// `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The program and the words it is run with. A program with a `/` in it is the plugin's own,
    /// found from its folder; one without is looked for on `PATH`.
    pub exec: Vec<String>,
    /// How often the program runs; none for one that keeps running and prints a line per change.
    #[serde(default)]
    pub every_secs: Option<u64>,
    /// Folders outside its own it reads, each an absolute path or one starting with one of the
    /// `$XDG_…_HOME` folders, `$XDG_RUNTIME_DIR` or `$HOME`.
    #[serde(default)]
    pub read: Vec<String>,
}

/// The folders a manifest's `read` names, as paths; one that isn't absolute once its folder is
/// put in is left out.
pub fn read_folders(manifest: &Manifest) -> Vec<PathBuf> {
    let home = || crate::xdg_home("HOME", "");
    let runtime = || {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
    };
    manifest
        .read
        .iter()
        .filter_map(|folder| {
            let (name, rest) = match folder.strip_prefix('$') {
                Some(after) => {
                    let after = after.strip_prefix('{').map_or(after, |a| a);
                    let end = after
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(after.len());
                    let rest = after[end..].strip_prefix('}').unwrap_or(&after[end..]);
                    (Some(&after[..end]), rest)
                }
                None => (None, folder.as_str()),
            };
            let base = match name {
                None => PathBuf::from("/"),
                Some("HOME") => home(),
                Some("XDG_RUNTIME_DIR") => runtime()?,
                Some("XDG_CONFIG_HOME") => crate::xdg_home("XDG_CONFIG_HOME", ".config"),
                Some("XDG_DATA_HOME") => crate::xdg_home("XDG_DATA_HOME", ".local/share"),
                Some("XDG_CACHE_HOME") => crate::xdg_home("XDG_CACHE_HOME", ".cache"),
                Some("XDG_STATE_HOME") => crate::xdg_home("XDG_STATE_HOME", ".local/state"),
                Some(_) => return None,
            };
            let path = base.join(rest.trim_start_matches('/'));
            (path.is_absolute() && !rest.split('/').any(|part| part == ".."))
                .then_some(path)
        })
        .collect()
}

/// Where plugin `id` keeps what it keeps.
pub fn state_dir(id: &str) -> PathBuf {
    crate::xdg_home("XDG_STATE_HOME", ".local/state")
        .join("slipstream")
        .join("plugins")
        .join(id)
}

impl Manifest {
    /// `every_secs`, kept between a second and a day.
    pub fn every(&self) -> Option<std::time::Duration> {
        self.every_secs
            .map(|secs| std::time::Duration::from_secs(secs.clamp(1, 86_400)))
    }
}

/// A plugin found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    /// Its folder's name.
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// In the person's own folder, so it can be removed from Settings.
    pub own: bool,
    /// What its folder holds now (`fingerprint`).
    pub fingerprint: String,
    /// The settings file's own command (`meter_plugin`): run with `sh -c` and the session's
    /// environment, as it always was, rather than as a plugin's program is.
    pub settings_command: bool,
}

/// Whether `id` can name a plugin: a folder name of letters, digits, `.`, `-` and `_`, starting
/// with a letter or digit.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The person's own plugins folder.
pub fn own_dir() -> PathBuf {
    crate::xdg_home("XDG_DATA_HOME", ".local/share")
        .join("slipstream")
        .join("plugins")
}

/// Every folder plugins are looked for in, the person's own first.
pub fn dirs() -> Vec<PathBuf> {
    let shared = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(own_dir())
        .chain(
            shared
                .split(':')
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("slipstream").join("plugins")),
        )
        .collect()
}

/// Reads the plugin in `dir`, or says why it isn't one.
pub fn read(dir: &Path) -> Result<Plugin, String> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| valid_id(id))
        .ok_or("its folder's name can only have letters, digits, '.', '-' and '_'")?
        .to_string();
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|err| format!("no {MANIFEST}: {err}"))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|err| format!("{MANIFEST}: {err}"))?;
    if manifest.name.trim().is_empty() {
        return Err(format!("{MANIFEST} gives it no name"));
    }
    if manifest.exec.first().is_none_or(|program| program.trim().is_empty()) {
        return Err(format!("{MANIFEST} gives it no program to run"));
    }
    let fingerprint = fingerprint(dir)?;
    Ok(Plugin {
        id,
        dir: dir.to_path_buf(),
        manifest,
        own: dir.starts_with(own_dir()),
        fingerprint,
        settings_command: false,
    })
}

/// A fingerprint of everything in `dir`: each file's path and contents, SHA-256 over them all in
/// order. A folder with a link in it, or too much in it, isn't a plugin.
pub fn fingerprint(dir: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut files = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let entries = std::fs::read_dir(&folder).map_err(|err| err.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = entry.file_type().map_err(|err| err.to_string())?;
            if kind.is_symlink() {
                return Err(format!(
                    "it holds a link, {}, and plugins can't",
                    path.strip_prefix(dir).unwrap_or(&path).display()
                ));
            }
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    let mut total = 0u64;
    for file in files {
        let bytes = std::fs::read(&file).map_err(|err| err.to_string())?;
        total += bytes.len() as u64;
        if total > MOST_BYTES {
            return Err(format!("it holds more than {} MB", MOST_BYTES >> 20));
        }
        let name = file.strip_prefix(dir).unwrap_or(&file).to_string_lossy();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Every plugin in `dirs`, by name, with why the folders that aren't plugins were left out. Where
/// two folders share an id, the first in `dirs` wins.
pub fn find_in(dirs: &[PathBuf]) -> (Vec<Plugin>, Vec<String>) {
    let mut found: Vec<Plugin> = Vec::new();
    let mut problems = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            match read(&folder) {
                Ok(plugin) if plugin.id == METER_ID => {
                    problems.push(format!("{}: the id \"{METER_ID}\" is kept", folder.display()));
                }
                Ok(plugin) if found.iter().any(|other| other.id == plugin.id) => {}
                Ok(plugin) => found.push(plugin),
                Err(why) => problems.push(format!("{}: {why}", folder.display())),
            }
        }
    }
    found.sort_by_key(|plugin| plugin.manifest.name.to_lowercase());
    (found, problems)
}

/// Every plugin there is.
pub fn find() -> (Vec<Plugin>, Vec<String>) {
    find_in(&dirs())
}

/// The `[meter]` setting as a plugin, while it has a command: on, since it was set up to show.
pub fn meter_plugin(meter: &crate::Meter) -> Option<Plugin> {
    let command = meter.command.trim();
    (!command.is_empty()).then(|| Plugin {
        id: METER_ID.into(),
        dir: PathBuf::new(),
        manifest: Manifest {
            name: "Meter".into(),
            description: "The command in the settings file's [meter]".into(),
            exec: vec![command.into()],
            every_secs: Some(meter.every().as_secs()),
            read: Vec::new(),
        },
        own: false,
        fingerprint: String::new(),
        settings_command: true,
    })
}

/// Whether `id` is on: as `[plugins] on` says, and the meter whenever it has a command.
pub fn is_on(settings: &crate::Settings, id: &str) -> bool {
    if id == METER_ID {
        return !settings.meter.command.trim().is_empty();
    }
    settings.plugins.is_on(id)
}

/// The plugin whose headline is on the chip: the one chosen, while it is on, else the meter, as it
/// was on the bar before plugins.
pub fn glance(settings: &crate::Settings) -> Option<String> {
    match &settings.plugins.glance {
        Some(id) => is_on(settings, id).then(|| id.clone()),
        None => is_on(settings, METER_ID).then(|| METER_ID.to_string()),
    }
}

/// A button on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub id: String,
    pub label: String,
}

/// What a plugin shows, as its program last printed it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Card {
    pub lines: Vec<String>,
    pub sections: Vec<Section>,
    pub buttons: Vec<Button>,
    /// What the chip shows for it, when it is the one chosen.
    pub glance: Glance,
    /// Something wants looking at: the chip turns amber.
    pub attention: bool,
}

/// A plugin's headline on the chip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Glance {
    pub text: String,
    /// Up to two gauges drawn before the text, in percent.
    pub gauges: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct Printed {
    #[serde(flatten)]
    report: Report,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    buttons: Vec<PrintedButton>,
    #[serde(default)]
    glance: Option<String>,
    #[serde(default)]
    attention: bool,
}

#[derive(Debug, Deserialize)]
struct PrintedButton {
    id: String,
    label: String,
}

/// `text` cut to `chars` characters, on one line.
fn clip(text: &str, chars: usize) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    match line.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line.to_string(),
    }
}

impl Card {
    /// The card in a line a plugin printed, at `now` in seconds since the Unix epoch. `stale`
    /// marks its numbers old.
    pub fn parse(text: &str, stale: bool, now: i64) -> Result<Card, String> {
        let printed: Printed =
            serde_json::from_str(text.trim()).map_err(|err| err.to_string())?;
        let mut sections = printed.report.reading(stale, now).sections;
        sections.truncate(MAX_SECTIONS);
        let buttons = printed
            .buttons
            .into_iter()
            .filter(|button| valid_id(&button.id) && !button.label.trim().is_empty())
            .take(MAX_BUTTONS)
            .map(|button| Button {
                id: button.id,
                label: clip(&button.label, LABEL_CHARS),
            })
            .collect();
        let gauges: Vec<u8> = sections
            .iter()
            .find(|section| !section.gauges.is_empty())
            .map(|section| section.gauges.iter().take(2).map(|g| g.percent).collect())
            .unwrap_or_default();
        let glance = match printed.glance.filter(|glance| !glance.trim().is_empty()) {
            Some(text) => Glance {
                text: clip(&text, GLANCE_CHARS),
                gauges: Vec::new(),
            },
            None => Glance {
                text: gauges
                    .iter()
                    .max()
                    .map(|percent| format!("{percent}%"))
                    .unwrap_or_default(),
                gauges,
            },
        };
        let full = sections
            .iter()
            .flat_map(|section| &section.gauges)
            .any(|gauge| gauge.percent >= ATTENTION_PERCENT);
        Ok(Card {
            lines: printed
                .lines
                .iter()
                .map(|line| clip(line, LINE_CHARS))
                .filter(|line| !line.is_empty())
                .take(MAX_LINES)
                .collect(),
            sections,
            buttons,
            glance,
            attention: printed.attention || full,
        })
    }

    /// The card with its sections called one of `hidden` left out.
    pub fn without(mut self, hidden: &[String]) -> Card {
        self.sections.retain(|section| !hidden.contains(&section.name));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("slipstream-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plugin(root: &Path, id: &str, manifest: &str) {
        std::fs::create_dir_all(root.join(id)).unwrap();
        std::fs::write(root.join(id).join(MANIFEST), manifest).unwrap();
    }

    #[test]
    fn plugins_are_found_by_name_and_the_person_s_own_win() {
        let own = scratch("own");
        let shared = scratch("shared");
        plugin(&own, "timer", "name = \"Timer\"\nexec = [\"./timer\"]\n");
        plugin(&shared, "timer", "name = \"Shared timer\"\nexec = [\"timer\"]\n");
        plugin(&shared, "disk", "name = \"Disk space\"\nexec = [\"df\"]\nevery-secs = 60\n");
        plugin(&shared, "broken", "name = \"\"\nexec = [\"x\"]\n");
        plugin(&shared, "meter", "name = \"Mine\"\nexec = [\"x\"]\n");
        std::fs::create_dir_all(shared.join("bad name")).unwrap();
        let (found, problems) = find_in(&[own.clone(), shared.clone()]);
        let names: Vec<_> = found.iter().map(|p| p.manifest.name.as_str()).collect();
        assert_eq!(names, ["Disk space", "Timer"]);
        assert_eq!(found[0].manifest.every(), Some(std::time::Duration::from_secs(60)));
        assert_eq!(found[1].manifest.every(), None);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn the_folders_a_plugin_reads_are_put_in_and_nothing_else_is() {
        let manifest = Manifest {
            name: "x".into(),
            description: String::new(),
            exec: vec!["x".into()],
            every_secs: None,
            read: vec![
                "$HOME/notes".into(),
                "${XDG_CACHE_HOME}/weather".into(),
                "/srv/shared".into(),
                "relative".into(),
                "$SSH_AUTH_SOCK".into(),
                "$HOME/../root".into(),
            ],
        };
        let folders = read_folders(&manifest);
        assert_eq!(folders.len(), 4, "{folders:?}");
        assert!(folders[0].ends_with("notes") && folders[0].is_absolute());
        assert!(folders[1].ends_with("weather"));
        assert_eq!(folders[2], PathBuf::from("/srv/shared"));
        assert_eq!(folders[3], PathBuf::from("/relative"));
    }

    #[test]
    fn a_card_reads_with_everything_left_out() {
        assert_eq!(Card::parse("{}", false, 0).unwrap(), Card::default());
        assert!(Card::parse("not json", false, 0).is_err());
    }

    #[test]
    fn a_card_s_glance_comes_from_its_gauges_unless_it_gives_one() {
        let card = Card::parse(
            r#"{"sections": [{"name": "Claude", "meters": [
                {"label": "Session", "percent": 62}, {"label": "Week", "percent": 81}]}]}"#,
            false,
            0,
        )
        .unwrap();
        assert_eq!(card.glance.gauges, [62, 81]);
        assert_eq!(card.glance.text, "81%");
        assert!(card.attention, "a gauge past three quarters wants looking at");
        let card = Card::parse(r#"{"glance": "18:42 left on the clock"}"#, false, 0).unwrap();
        assert_eq!(card.glance.text, "18:42 left o…");
        assert!(card.glance.gauges.is_empty());
        assert!(!card.attention);
    }

    /// The promise in the guide: a card is only ever shown. Its buttons carry nothing but an id,
    /// which is handed back to the plugin's own program, and an id that could be read as anything
    /// else is dropped.
    #[test]
    fn a_card_carries_no_command_only_ids_for_its_own_program() {
        let card = Card::parse(
            r#"{"buttons": [{"id": "pause", "label": "Pause"},
                            {"id": "$(rm -rf ~)", "label": "Oops"},
                            {"id": "a b", "label": "Space"},
                            {"id": "skip", "label": "Skip", "exec": "rm -rf ~"}],
                "lines": ["one\ntwo", "", "three"], "exec": "rm -rf ~"}"#,
            false,
            0,
        )
        .unwrap();
        let ids: Vec<_> = card.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["pause", "skip"]);
        assert_eq!(card.lines, ["one two", "three"]);
    }

    #[test]
    fn turning_a_plugin_off_takes_its_glance_too() {
        let mut plugins = Plugins::default();
        plugins.set_on("timer", Some("a".into()));
        plugins.set_on("disk", Some("b".into()));
        plugins.set_on("timer", Some("c".into()));
        assert_eq!(plugins.on, ["disk", "timer"]);
        assert_eq!(plugins.fingerprints["timer"], "c");
        plugins.glance = Some("timer".into());
        plugins.set_on("timer", None);
        assert_eq!(plugins.on, ["disk"]);
        assert_eq!(plugins.glance, None);
        assert!(!plugins.fingerprints.contains_key("timer"));
    }

    /// The promise in the guide: a plugin that changes after it was turned on doesn't run until
    /// it is turned on again.
    #[test]
    fn a_plugin_changed_since_it_was_turned_on_is_held() {
        let root = scratch("changed");
        plugin(&root, "timer", "name = \"Timer\"\nexec = [\"./timer\"]\n");
        std::fs::write(root.join("timer").join("timer"), "#!/bin/sh\necho {}\n").unwrap();
        let first = read(&root.join("timer")).unwrap();
        let mut plugins = Plugins::default();
        plugins.set_on("timer", Some(first.fingerprint.clone()));
        assert!(plugins.approved(&first));
        std::fs::write(root.join("timer").join("timer"), "#!/bin/sh\necho changed\n").unwrap();
        let changed = read(&root.join("timer")).unwrap();
        assert_ne!(changed.fingerprint, first.fingerprint);
        assert!(!plugins.approved(&changed));
        std::fs::create_dir_all(root.join("timer").join("lib")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("timer").join("lib").join("x")).unwrap();
        assert!(read(&root.join("timer")).unwrap_err().contains("link"));
    }

    #[test]
    fn the_meter_setting_is_a_plugin_while_it_has_a_command() {
        let mut settings = crate::Settings::default();
        assert_eq!(meter_plugin(&settings.meter), None);
        assert_eq!(glance(&settings), None);
        settings.meter.command = "usage".into();
        let meter = meter_plugin(&settings.meter).unwrap();
        assert_eq!(meter.id, METER_ID);
        assert!(is_on(&settings, METER_ID));
        assert_eq!(glance(&settings).as_deref(), Some(METER_ID));
        settings.plugins.set_on("disk", Some("a".into()));
        settings.plugins.glance = Some("disk".into());
        assert_eq!(glance(&settings).as_deref(), Some("disk"));
    }
}

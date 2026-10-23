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
//! turned on is exactly what runs. It runs from a read-only copy of its folder (`store`), so
//! whatever it keeps goes in a folder of its own beside that copy, which it is given as
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
//! Every field is optional. `sections` are named groups of gauges (`card`), each with when it
//! starts over. A pressed button runs the plugin's own program
//! again with `press <id>` after it, and that is the only thing anything on a card can do: a card
//! is shown, as text and gauges in Slipstream's own style, and nothing in it is ever run.
//!
//! Plugins live in `$XDG_DATA_HOME/slipstream/plugins/<id>/` (the person's own) and in
//! `slipstream/plugins/` under each of `$XDG_DATA_DIRS` (installed for everyone). A plugin is off
//! until it is turned on (`[plugins] on`), and turning it on records the fingerprint of its folder
//! that Settings showed (`contents`), and copies the folder as it was then. A plugin whose folder
//! has changed since is held, and doesn't run, until it is turned on again: an update can't slip
//! in unseen. What it asked for is kept beside that
//! (`approval`), so a held plugin can say whether its update is like for like or asks for more.
//! That guards against a plugin changing underneath its approval, not against something already
//! running as the person, which could change the settings file as easily.

pub mod approval;
pub mod contents;
pub mod store;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::card::{Report, Section};

/// The file in a plugin's folder that says what it is.
pub const MANIFEST: &str = "plugin.toml";
/// The most lines, sections and buttons a card shows.
pub const MAX_LINES: usize = 6;
pub const MAX_SECTIONS: usize = 4;
pub const MAX_BUTTONS: usize = 3;
/// The longest a line or a glance is shown, in characters.
const LINE_CHARS: usize = 80;
const GLANCE_CHARS: usize = 12;
const LABEL_CHARS: usize = 20;
/// A gauge this full or fuller asks for attention.
pub const ATTENTION_PERCENT: u8 = 75;
/// The largest `plugin.toml` read.
const MOST_MANIFEST: u64 = 64 << 10;
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
    /// The sections of each plugin's card not to show, by plugin id and section name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub hidden: BTreeMap<String, Vec<String>>,
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

    /// The plugin whose headline is on the chip, while it is on.
    pub fn glance(&self) -> Option<&str> {
        self.glance.as_deref().filter(|id| self.is_on(id))
    }

    /// The sections of plugin `id`'s card not to show.
    pub fn hidden(&self, id: &str) -> &[String] {
        self.hidden.get(id).map_or(&[], Vec::as_slice)
    }

    /// Shows plugin `id`'s section `name` if it was hidden, or hides it.
    pub fn toggle_hidden(&mut self, id: &str, name: &str) {
        let list = self.hidden.entry(id.to_string()).or_default();
        if list.iter().any(|hidden| hidden == name) {
            list.retain(|hidden| hidden != name);
        } else {
            list.push(name.to_string());
        }
        self.hidden.retain(|_, list| !list.is_empty());
    }

    /// Whether `plugin` may run: it's on, and its folder is as it was when it was turned on.
    pub fn approved(&self, plugin: &Plugin) -> bool {
        plugin.too_big.is_none()
            && self.is_on(&plugin.id)
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
            (path.is_absolute() && !rest.split('/').any(|part| part == "..")).then_some(path)
        })
        .collect()
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
    /// What its folder holds now (`contents`); empty when it is too big to check.
    pub fingerprint: String,
    /// Each file's own fingerprint, by its path in the folder.
    pub files: BTreeMap<String, String>,
    /// Why its folder is too big to check, if it is: it can't be turned on.
    pub too_big: Option<String>,
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

/// Reads the plugin in `dir`, or says why it isn't one. Its folder's limits are checked from
/// metadata first: one past them is read no further than its manifest, and can't be turned on.
pub fn read(dir: &Path) -> Result<Plugin, String> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| valid_id(id))
        .ok_or("its folder's name can only have letters, digits, '.', '-' and '_'")?
        .to_string();
    let listed = match contents::list(dir) {
        Ok(_) => None,
        Err(contents::Unchecked::TooBig(why)) => Some(why),
        Err(contents::Unchecked::NotAPlugin(why)) => return Err(why),
    };
    let manifest = read_manifest(&dir.join(MANIFEST))?;
    let (fingerprint, files, too_big) = match listed {
        Some(why) => (String::new(), BTreeMap::new(), Some(why)),
        None => match contents::contents(dir) {
            Ok(found) => (found.fingerprint, found.files, None),
            Err(contents::Unchecked::TooBig(why)) => (String::new(), BTreeMap::new(), Some(why)),
            Err(contents::Unchecked::NotAPlugin(why)) => return Err(why),
        },
    };
    Ok(Plugin {
        id,
        dir: dir.to_path_buf(),
        manifest,
        own: dir.starts_with(own_dir()),
        fingerprint,
        files,
        too_big,
    })
}

/// The manifest at `path`: a file of its own, not a link, and small.
fn read_manifest(path: &Path) -> Result<Manifest, String> {
    use std::io::Read;
    let (file, _) = contents::open_regular(path).map_err(|err| format!("no {MANIFEST}: {err}"))?;
    let mut text = String::new();
    file.take(MOST_MANIFEST)
        .read_to_string(&mut text)
        .map_err(|err| format!("{MANIFEST}: {err}"))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|err| format!("{MANIFEST}: {err}"))?;
    if manifest.name.trim().is_empty() {
        return Err(format!("{MANIFEST} gives it no name"));
    }
    if manifest
        .exec
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        return Err(format!("{MANIFEST} gives it no program to run"));
    }
    Ok(manifest)
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

/// Moves a `[meter] command` from a settings file from before plugins into a plugin of its own,
/// once: the first start that finds one writes it out as `meter/` in the person's plugins folder,
/// where it waits to be turned on like any other plugin. Says where it went, or why it couldn't
/// go, when there was one to move.
pub fn move_meter_once() -> Option<Result<PathBuf, String>> {
    let record = crate::xdg_home("XDG_STATE_HOME", ".local/state")
        .join("slipstream")
        .join("meter-moved");
    move_meter(&crate::path(), &own_dir(), &record)
}

/// `move_meter_once`, reading `settings`, writing into `plugins`, and keeping `record` to say it
/// has. The command runs as `sh -c <command> meter`, so a button's words reach it as `$1` and `$2`
/// and nothing is added to the line it was written as.
pub fn move_meter(
    settings: &Path,
    plugins: &Path,
    record: &Path,
) -> Option<Result<PathBuf, String>> {
    if record.exists() {
        return None;
    }
    let text = std::fs::read_to_string(settings).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let meter = table.get("meter")?.as_table()?;
    let command = meter.get("command")?.as_str()?.trim();
    if command.is_empty() {
        return None;
    }
    let every = meter
        .get("every-secs")
        .and_then(toml::Value::as_integer)
        .unwrap_or(60)
        .clamp(10, 86_400);
    let dir = plugins.join("meter");
    let written = if dir.exists() {
        Err(format!("{} is there already", dir.display()))
    } else {
        let mut manifest = toml::Table::new();
        manifest.insert("name".into(), "Meter".into());
        manifest.insert(
            "description".into(),
            "The command from the settings file's [meter]".into(),
        );
        manifest.insert(
            "exec".into(),
            toml::Value::Array(vec![
                "sh".into(),
                "-c".into(),
                command.into(),
                "meter".into(),
            ]),
        );
        manifest.insert("every-secs".into(), every.into());
        let text = toml::to_string(&manifest).map_err(|err| err.to_string());
        let wrote = text.and_then(|text| {
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(dir.join(MANIFEST), text))
                .map_err(|err| err.to_string())
        });
        // Tried again at the next start, when it couldn't be written.
        if let Err(why) = wrote {
            return Some(Err(why));
        }
        Ok(dir)
    };
    if let Some(parent) = record.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(record, "");
    Some(written)
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
    /// The headline the plugin gave for the chip, if it gave one (`glance`).
    pub headline: Option<String>,
    /// The plugin says something wants looking at (`attention`).
    pub flagged: bool,
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
        let printed: Printed = serde_json::from_str(text.trim()).map_err(|err| err.to_string())?;
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
            headline: printed
                .glance
                .filter(|glance| !glance.trim().is_empty())
                .map(|text| clip(&text, GLANCE_CHARS)),
            flagged: printed.attention,
        })
    }

    /// The card with its sections called one of `hidden` left out.
    pub fn without(mut self, hidden: &[String]) -> Card {
        self.sections
            .retain(|section| !hidden.contains(&section.name));
        self
    }

    /// What the chip shows for it, when it is the one chosen: the headline it gave, else its
    /// first section's first two gauges and the larger of their percentages.
    pub fn glance(&self) -> Glance {
        if let Some(text) = &self.headline {
            return Glance {
                text: text.clone(),
                gauges: Vec::new(),
            };
        }
        let gauges: Vec<u8> = self
            .sections
            .iter()
            .find(|section| !section.gauges.is_empty())
            .map(|section| section.gauges.iter().take(2).map(|g| g.percent).collect())
            .unwrap_or_default();
        Glance {
            text: gauges
                .iter()
                .max()
                .map(|percent| format!("{percent}%"))
                .unwrap_or_default(),
            gauges,
        }
    }

    /// Whether something on it wants looking at: the plugin says so, or a gauge is past three
    /// quarters.
    pub fn attention(&self) -> bool {
        self.flagged
            || self
                .sections
                .iter()
                .flat_map(|section| &section.gauges)
                .any(|gauge| gauge.percent >= ATTENTION_PERCENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("slipstream-plugins-{name}-{}", std::process::id()));
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
        plugin(
            &shared,
            "timer",
            "name = \"Shared timer\"\nexec = [\"timer\"]\n",
        );
        plugin(
            &shared,
            "disk",
            "name = \"Disk space\"\nexec = [\"df\"]\nevery-secs = 60\n",
        );
        plugin(&shared, "broken", "name = \"\"\nexec = [\"x\"]\n");
        std::fs::create_dir_all(shared.join("bad name")).unwrap();
        let (found, problems) = find_in(&[own.clone(), shared.clone()]);
        let names: Vec<_> = found.iter().map(|p| p.manifest.name.as_str()).collect();
        assert_eq!(names, ["Disk space", "Timer"]);
        assert_eq!(
            found[0].manifest.every(),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(found[1].manifest.every(), None);
        assert_eq!(problems.len(), 2, "{problems:?}");
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
        assert_eq!(card.glance().gauges, [62, 81]);
        assert_eq!(card.glance().text, "81%");
        assert!(
            card.attention(),
            "a gauge past three quarters wants looking at"
        );
        let hidden = card.clone().without(&["Claude".to_string()]);
        assert_eq!(
            hidden.glance().text,
            "",
            "a hidden section isn't on the chip"
        );
        assert!(!hidden.attention(), "nor does it want looking at");
        let card = Card::parse(r#"{"glance": "18:42 left on the clock"}"#, false, 0).unwrap();
        assert_eq!(card.glance().text, "18:42 left o…");
        assert!(card.glance().gauges.is_empty());
        assert!(!card.attention());
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
    fn hiding_a_section_is_kept_for_each_plugin() {
        let mut plugins = Plugins::default();
        plugins.toggle_hidden("tides", "Harbour");
        plugins.toggle_hidden("kettle", "Harbour");
        assert_eq!(plugins.hidden("tides"), ["Harbour"]);
        plugins.toggle_hidden("tides", "Harbour");
        assert!(plugins.hidden("tides").is_empty());
        assert_eq!(plugins.hidden("kettle"), ["Harbour"]);
        plugins.toggle_hidden("kettle", "Harbour");
        assert!(plugins.hidden.is_empty(), "no empty lists left in the file");
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
        std::fs::write(
            root.join("timer").join("timer"),
            "#!/bin/sh\necho changed\n",
        )
        .unwrap();
        let changed = read(&root.join("timer")).unwrap();
        assert_ne!(changed.fingerprint, first.fingerprint);
        assert!(!plugins.approved(&changed));
        std::fs::write(root.join("timer").join("timer"), "#!/bin/sh\necho {}\n").unwrap();
        assert!(plugins.approved(&read(&root.join("timer")).unwrap()));
        {
            use std::os::unix::fs::PermissionsExt;
            let program = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(root.join("timer").join("timer"), program).unwrap();
        }
        assert!(
            !plugins.approved(&read(&root.join("timer")).unwrap()),
            "made a program since: held too"
        );
        std::fs::create_dir_all(root.join("timer").join("lib")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("timer").join("lib").join("x"))
            .unwrap();
        assert!(read(&root.join("timer")).unwrap_err().contains("link"));
    }

    /// The promise in the guide: a plugin too big to check is listed, and can't be turned on.
    #[test]
    fn a_plugin_too_big_to_check_can_t_be_turned_on() {
        let root = scratch("too-big");
        plugin(&root, "atlas", "name = \"Atlas\"\nexec = [\"./atlas\"]\n");
        std::fs::File::create(root.join("atlas").join("maps"))
            .unwrap()
            .set_len(100 << 30)
            .unwrap();
        let (found, problems) = find_in(std::slice::from_ref(&root));
        assert!(problems.is_empty(), "{problems:?}");
        let atlas = &found[0];
        assert_eq!(atlas.manifest.name, "Atlas");
        assert!(atlas.too_big.as_deref().unwrap().contains("MB"));
        let mut plugins = Plugins::default();
        plugins.set_on("atlas", Some(atlas.fingerprint.clone()));
        assert!(!plugins.approved(atlas));
    }

    /// The promise in the guide: a `[meter]` from before plugins becomes a plugin of its own,
    /// once, and waits to be turned on.
    #[test]
    fn a_meter_command_becomes_a_plugin_folder_once() {
        let root = scratch("meter");
        let settings = root.join("settings.toml");
        let record = root.join("state").join("meter-moved");
        let own = root.join("plugins");
        std::fs::write(
            &settings,
            "[meter]\ncommand = \"if true; then printf '{}'; fi # done\"\nevery-secs = 120\n",
        )
        .unwrap();
        let moved = move_meter(&settings, &own, &record).unwrap().unwrap();
        let meter = read(&moved).unwrap();
        assert_eq!(meter.id, "meter");
        assert_eq!(
            meter.manifest.exec,
            ["sh", "-c", "if true; then printf '{}'; fi # done", "meter"]
        );
        assert_eq!(meter.manifest.every_secs, Some(120));
        let parsed = crate::read(&settings).unwrap();
        assert!(
            !parsed.plugins.is_on("meter"),
            "it waits to be turned on, like any plugin"
        );
        std::fs::remove_dir_all(&moved).unwrap();
        assert_eq!(move_meter(&settings, &own, &record), None);
        assert!(!moved.exists(), "a second start leaves it removed");
    }

    #[test]
    fn a_settings_file_from_before_plugins_still_loads() {
        let settings: crate::Settings = toml::from_str(
            "[meter]\ncommand = \"quota\"\nhidden = [\"Mail\"]\n\n[plugins]\nglance = \"\"\n",
        )
        .unwrap();
        assert_eq!(
            settings.plugins.glance(),
            None,
            "an empty glance is nothing on the chip"
        );
    }

    #[test]
    fn an_empty_meter_command_moves_nothing() {
        let root = scratch("no-meter");
        let settings = root.join("settings.toml");
        std::fs::write(&settings, "[meter]\ncommand = \"\"\n").unwrap();
        let record = root.join("meter-moved");
        assert_eq!(move_meter(&settings, &root.join("plugins"), &record), None);
        assert!(!root.join("plugins").exists());
    }
}

//! What a plugin asked for when it was turned on, kept so that when it has changed the person can
//! be told what the change means for them: a like-for-like update of its code, or one that asks
//! for more than they agreed to.
//!
//! The record is `$XDG_STATE_HOME/slipstream/plugin-approvals/<id>.toml`: the fingerprint it was
//! turned on with, its program and words, how often it runs, the folders it says it reads, and
//! the SHA-256 of each of its files. It is Slipstream's, not the plugin's, which keeps its own
//! things in a folder of its own. A plugin turned on before records were kept gets one the next
//! time it is seen unchanged, since then it is exactly what was approved.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use super::Plugin;

/// What a plugin asked for, and what its folder held, when it was turned on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Approval {
    pub fingerprint: String,
    pub exec: Vec<String>,
    pub every_secs: Option<u64>,
    pub read: Vec<String>,
    /// The SHA-256 of each file, by its path in the plugin's folder.
    pub files: BTreeMap<String, String>,
}

impl Approval {
    /// What `plugin` asks for, and held when it was read.
    pub fn of(plugin: &Plugin) -> Approval {
        Approval {
            fingerprint: plugin.fingerprint.clone(),
            exec: plugin.manifest.exec.clone(),
            every_secs: plugin.manifest.every_secs,
            read: plugin.manifest.read.clone(),
            files: plugin.files.clone(),
        }
    }
}

/// Where approvals are kept.
pub fn folder() -> PathBuf {
    crate::xdg_home("XDG_STATE_HOME", ".local/state")
        .join("slipstream")
        .join("plugin-approvals")
}

/// The approval kept for plugin `id`, if one was.
pub fn load(id: &str) -> Option<Approval> {
    load_from(&folder(), id)
}

fn load_from(folder: &Path, id: &str) -> Option<Approval> {
    let text = std::fs::read_to_string(folder.join(format!("{id}.toml"))).ok()?;
    toml::from_str(&text).ok()
}

/// Keeps what `plugin` asks for now as what was approved: called as it is turned on.
pub fn save(plugin: &Plugin) -> Result<(), String> {
    save_in(&folder(), plugin)
}

fn save_in(folder: &Path, plugin: &Plugin) -> Result<(), String> {
    let approval = Approval::of(plugin);
    let text = toml::to_string(&approval).map_err(|err| err.to_string())?;
    std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
    crate::replace(&folder.join(format!("{}.toml", plugin.id)), text.as_bytes())
        .map_err(|err| err.to_string())
}

/// Forgets what plugin `id` was approved with: called as it is turned off.
pub fn forget(id: &str) {
    let _ = std::fs::remove_file(folder().join(format!("{id}.toml")));
}

/// Keeps a record in `folder` for a plugin that is running as it was approved, if none is kept for
/// that approval yet: one turned on before records were kept, or by hand in the settings file.
pub fn remember_if_missing_in(folder: &Path, plugin: &Plugin, approved: &str) {
    if plugin.fingerprint != approved {
        return;
    }
    let kept = load_from(folder, &plugin.id);
    if kept.is_none_or(|kept| kept.fingerprint != approved) {
        // Nothing is lost if it can't be kept: an update is then shown as one it can't compare.
        let _ = save_in(folder, plugin);
    }
}

/// How a plugin now differs from what was approved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// The program and words it runs, before and now, if they changed.
    pub program: Option<(Vec<String>, Vec<String>)>,
    /// How often it runs, before and now (none: it keeps running), if that changed.
    pub schedule: Option<(Option<u64>, Option<u64>)>,
    pub reads_added: Vec<String>,
    pub reads_removed: Vec<String>,
    pub files_changed: Vec<String>,
    pub files_added: Vec<String>,
    pub files_removed: Vec<String>,
}

impl Change {
    /// Whether it asks for more than was agreed to: another program, or folders it didn't read.
    pub fn asks_for_more(&self) -> bool {
        self.program.is_some() || !self.reads_added.is_empty()
    }

    /// Whether what it asks for is as it was, whatever happened to its files.
    pub fn like_for_like(&self) -> bool {
        self.program.is_none()
            && self.schedule.is_none()
            && self.reads_added.is_empty()
            && self.reads_removed.is_empty()
    }
}

/// How `now` differs from `before`.
pub fn compare(before: &Approval, now: &Approval) -> Change {
    let not_in = |list: &[String], other: &[String]| -> Vec<String> {
        list.iter()
            .filter(|item| !other.contains(item))
            .cloned()
            .collect()
    };
    let changed = |a: &BTreeMap<String, String>, b: &BTreeMap<String, String>| -> Vec<String> {
        a.iter()
            .filter(|(name, hash)| b.get(*name).is_some_and(|other| other != *hash))
            .map(|(name, _)| name.clone())
            .collect()
    };
    let only_in = |a: &BTreeMap<String, String>, b: &BTreeMap<String, String>| -> Vec<String> {
        a.keys()
            .filter(|name| !b.contains_key(*name))
            .cloned()
            .collect()
    };
    Change {
        program: (before.exec != now.exec).then(|| (before.exec.clone(), now.exec.clone())),
        schedule: (before.every_secs != now.every_secs)
            .then_some((before.every_secs, now.every_secs)),
        reads_added: not_in(&now.read, &before.read),
        reads_removed: not_in(&before.read, &now.read),
        files_changed: changed(&before.files, &now.files),
        files_added: only_in(&now.files, &before.files),
        files_removed: only_in(&before.files, &now.files),
    }
}

/// Where the plugin's present files came from, as far as Slipstream's own installer can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Downloaded from its published release, and the download matched the release's checksum.
    Release,
    /// Built from a checkout of its source on this computer, at this commit.
    Checkout(String),
    /// Changed after the installer put it there.
    ChangedSince,
    /// Not put there by the installer.
    Unknown,
}

/// Where plugin `plugin`'s files came from: the installer's record of putting it there
/// (`$XDG_STATE_HOME/slipstream/extras/<id>`), so long as nothing in the folder is newer than
/// the record.
pub fn source(plugin: &Plugin) -> Source {
    let record = crate::xdg_home("XDG_STATE_HOME", ".local/state")
        .join("slipstream")
        .join("extras")
        .join(&plugin.id);
    source_from(&record, &plugin.dir)
}

fn source_from(record: &Path, dir: &Path) -> Source {
    let Ok(text) = std::fs::read_to_string(record) else {
        return Source::Unknown;
    };
    let value = |key: &str| {
        text.lines()
            .filter_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .next_back()
            .map(str::to_string)
    };
    let from = match value("from").as_deref() {
        Some("release") => Source::Release,
        Some("checkout") => Source::Checkout(value("built").unwrap_or_default()),
        Some(_) => return Source::Unknown,
        // Recorded before the installer said which: only one of them can tell.
        None => match (value("bundle"), value("built")) {
            (Some(_), None) => Source::Release,
            (None, Some(commit)) => Source::Checkout(commit),
            _ => return Source::Unknown,
        },
    };
    let written = std::fs::metadata(record).and_then(|meta| meta.modified());
    match (written, newest(dir)) {
        // The installer copies the files in, then writes its record.
        (Ok(written), Some(newest)) if newest > written + Duration::from_secs(2) => {
            Source::ChangedSince
        }
        (Ok(_), _) => from,
        _ => Source::Unknown,
    }
}

/// When the most recently changed thing in `dir` was changed.
fn newest(dir: &Path) -> Option<SystemTime> {
    let mut newest = None;
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).ok()?.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                folders.push(entry.path());
            }
            if let Ok(modified) = meta.modified() {
                newest = newest.max(Some(modified));
            }
        }
    }
    newest
}

/// What a held plugin's update means, as short lines for Settings to show under the plugin, below
/// its own line saying it was updated. `change` is none when nothing was kept of what it was
/// approved with.
pub fn explain(plugin: &Plugin, change: Option<&Change>, source: &Source) -> Vec<String> {
    let mut lines = Vec::new();
    match change {
        None => lines.push(format!(
            "Slipstream can't compare it with the version you turned on. It runs {}{}; read its \
             plugin.toml first.",
            plugin.manifest.exec.join(" "),
            reads(&plugin.manifest.read),
        )),
        Some(change) if change.like_for_like() => lines.push(
            "It does what it did before: the same program, as often, reading the same folders."
                .to_string(),
        ),
        Some(change) => {
            let mut asks = Vec::new();
            if let Some((before, now)) = &change.program {
                asks.push(format!(
                    "it now runs {} (it ran {})",
                    now.join(" "),
                    before.join(" ")
                ));
            }
            for folder in &change.reads_added {
                asks.push(format!("it now says it reads {folder}"));
            }
            for folder in &change.reads_removed {
                asks.push(format!("it no longer reads {folder}"));
            }
            if let Some((before, now)) = change.schedule {
                asks.push(format!(
                    "it now {} (it {})",
                    schedule(now),
                    schedule(before)
                ));
            }
            let head = if change.asks_for_more() {
                "It asks for more than before"
            } else {
                "What it asks for has changed"
            };
            lines.push(format!("{head}: {}.", asks.join("; ")));
        }
    }
    if let Some(change) = change {
        let mut files = Vec::new();
        files.extend(change.files_changed.iter().map(|name| name.to_string()));
        files.extend(
            change
                .files_added
                .iter()
                .map(|name| format!("{name} (new)")),
        );
        files.extend(
            change
                .files_removed
                .iter()
                .map(|name| format!("{name} (gone)")),
        );
        if !files.is_empty() {
            let shown = files.len().min(4);
            let more = files.len() - shown;
            let mut list = files[..shown].join(", ");
            if more > 0 {
                list.push_str(&format!(" and {more} more"));
            }
            lines.push(format!("Changed files: {list}."));
        }
    }
    lines.push(match source {
        Source::Release => {
            "It came from its official release, and the download matched its published checksum."
                .to_string()
        }
        Source::Checkout(commit) if !commit.is_empty() => format!(
            "It was built on this computer from your copy of its source (commit {}).",
            commit.chars().take(12).collect::<String>()
        ),
        Source::Checkout(_) => {
            "It was built on this computer from your copy of its source.".to_string()
        }
        Source::ChangedSince => {
            "Something changed its files after Slipstream installed it.".to_string()
        }
        Source::Unknown => {
            "Slipstream didn't install this update, so it can't say where it came from.".to_string()
        }
    });
    lines
        .push("Its code runs as you, so switch it on only if you trust where it came from.".into());
    lines
}

fn reads(folders: &[String]) -> String {
    if folders.is_empty() {
        String::new()
    } else {
        format!(" and says it reads {}", folders.join(", "))
    }
}

fn schedule(every: Option<u64>) -> String {
    match every {
        Some(secs) => format!("runs every {secs} s"),
        None => "keeps running".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for one test, under the workspace's `target/test-scratch`.
    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("approval-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn timer(root: &Path, manifest: &str, program: &str) -> Plugin {
        let dir = root.join("timer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(super::super::MANIFEST), manifest).unwrap();
        std::fs::write(dir.join("timer"), program).unwrap();
        super::super::read(&dir).unwrap()
    }

    const MANIFEST: &str =
        "name = \"Timer\"\nexec = [\"./timer\", \"card\"]\nread = [\"$XDG_CACHE_HOME/timer\"]\n";

    #[test]
    fn an_update_of_its_code_alone_is_like_for_like() {
        let root = scratch("like");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let now = Approval::of(&timer(&root, MANIFEST, "echo two"));
        let change = compare(&before, &now);
        assert!(change.like_for_like());
        assert!(!change.asks_for_more());
        assert_eq!(change.files_changed, ["timer"]);
        let plugin = timer(&root, MANIFEST, "echo two");
        let said = explain(&plugin, Some(&change), &Source::Release).join("\n");
        assert!(said.contains("does what it did before"), "{said}");
        assert!(said.contains("Changed files: timer."), "{said}");
        assert!(said.contains("matched its published checksum"), "{said}");
    }

    /// The promise in the guide: an update that asks for more than was turned on says so.
    #[test]
    fn an_update_that_asks_for_more_says_what() {
        let root = scratch("more");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let more = "name = \"Timer\"\nexec = [\"./timer\", \"card\", \"--upload\"]\n\
                    read = [\"$XDG_CACHE_HOME/timer\", \"$HOME/.ssh\"]\nevery-secs = 5\n";
        let plugin = timer(&root, more, "echo one");
        let change = compare(&before, &Approval::of(&plugin));
        assert!(change.asks_for_more());
        assert_eq!(change.reads_added, ["$HOME/.ssh"]);
        assert_eq!(change.files_changed, ["plugin.toml"]);
        let said = explain(&plugin, Some(&change), &Source::Unknown).join("\n");
        assert!(said.contains("It asks for more than before:"), "{said}");
        assert!(said.contains("it now says it reads $HOME/.ssh"), "{said}");
        assert!(said.contains("it now runs ./timer card --upload"), "{said}");
        assert!(
            said.contains("it now runs every 5 s (it keeps running)"),
            "{said}"
        );
        assert!(said.contains("can't say where it came from"), "{said}");
    }

    #[test]
    fn a_plugin_approved_before_records_were_kept_gets_one_while_unchanged() {
        let root = scratch("remember");
        let records = root.join("records");
        let plugin = timer(&root, MANIFEST, "echo one");
        remember_if_missing_in(&records, &plugin, "not this one");
        assert_eq!(
            load_from(&records, "timer"),
            None,
            "only what was approved is kept"
        );
        remember_if_missing_in(&records, &plugin, &plugin.fingerprint);
        let kept = load_from(&records, "timer").unwrap();
        assert_eq!(kept, Approval::of(&plugin));
        let said = explain(&plugin, None, &Source::Unknown).join("\n");
        assert!(said.contains("can't compare it"), "{said}");
    }

    #[test]
    fn the_installer_s_record_says_where_the_files_came_from() {
        let root = scratch("source");
        let plugin = timer(&root, MANIFEST, "echo one");
        let record = root.join("record");
        std::fs::write(&record, "bundle=ab12\nfrom=release\n").unwrap();
        assert_eq!(source_from(&record, &plugin.dir), Source::Release);
        std::fs::write(&record, "bundle=ab12\nbuilt=cd34\nfrom=checkout\n").unwrap();
        assert_eq!(
            source_from(&record, &plugin.dir),
            Source::Checkout("cd34".into())
        );
        std::fs::write(&record, "built=cd34\n").unwrap();
        assert_eq!(
            source_from(&record, &plugin.dir),
            Source::Checkout("cd34".into())
        );
        std::fs::write(&record, "bundle=ab12\nbuilt=cd34\n").unwrap();
        assert_eq!(
            source_from(&record, &plugin.dir),
            Source::Unknown,
            "which was last is unknown"
        );
        // A file changed after the record was written wasn't the installer's doing.
        std::fs::write(&record, "from=release\n").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(plugin.dir.join("timer"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(source_from(&record, &plugin.dir), Source::ChangedSince);
        assert_eq!(
            source_from(&root.join("none"), &plugin.dir),
            Source::Unknown
        );
    }
}

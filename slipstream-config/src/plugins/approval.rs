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

use super::{Need, Plugin};

/// What a plugin asked for, and what its folder held, when it was turned on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Approval {
    pub fingerprint: String,
    pub exec: Vec<String>,
    pub every_secs: Option<u64>,
    pub read: Vec<String>,
    /// The folders it changes. A record from before plugins could has none.
    pub write: Vec<String>,
    /// What it needed beyond its folders. A record from before plugins said has none, which is
    /// right: they couldn't.
    pub needs: Vec<Need>,
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
            write: plugin.manifest.write.clone(),
            needs: plugin.manifest.needs.clone(),
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

/// How a plugin now differs from what was approved, in what it asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// It runs another program, or the same with other words.
    pub program: bool,
    pub reads_added: Vec<String>,
    pub writes_added: Vec<String>,
    pub needs_added: Vec<Need>,
}

impl Change {
    /// Whether it asks for more than was agreed to: another program, folders it didn't read, or
    /// something it didn't need. Asking for less, or running more or less often, isn't more.
    pub fn asks_for_more(&self) -> bool {
        self.program
            || !self.reads_added.is_empty()
            || !self.writes_added.is_empty()
            || !self.needs_added.is_empty()
    }
}

/// How `now` differs from `before`.
pub fn compare(before: &Approval, now: &Approval) -> Change {
    Change {
        program: before.exec != now.exec,
        reads_added: now
            .read
            .iter()
            .filter(|folder| !before.read.contains(folder))
            .cloned()
            .collect(),
        writes_added: now
            .write
            .iter()
            .filter(|folder| !before.write.contains(folder))
            .cloned()
            .collect(),
        needs_added: now
            .needs
            .iter()
            .filter(|need| !before.needs.contains(need))
            .copied()
            .collect(),
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

/// What a held plugin's update means, in a sentence or two for Settings to show under the
/// plugin, below its own line saying it was updated: what it wants now that it didn't, and where
/// it came from. `change` is none when nothing was kept of what it was approved with.
pub fn explain(change: Option<&Change>, source: &Source) -> Vec<String> {
    let mut lines = Vec::new();
    match change {
        None => lines.push(
            "Slipstream can't tell what changed, so have a look at it before switching it on."
                .to_string(),
        ),
        Some(change) if change.needs_added.contains(&Need::Everything) => {
            lines.push("It now wants full access to your computer.".to_string());
        }
        Some(change) => {
            // The folders it now changes say more than that it needs `write`.
            let mut wants: Vec<String> = change
                .needs_added
                .iter()
                .filter(|need| **need != Need::Write || change.writes_added.is_empty())
                .map(|need| need.says().to_string())
                .collect();
            if !change.reads_added.is_empty() {
                let folders: Vec<String> = change
                    .reads_added
                    .iter()
                    .map(|f| super::shown_folder(f))
                    .collect();
                wants.push(format!("look in {}", super::listed(&folders)));
            }
            if !change.writes_added.is_empty() {
                let folders: Vec<String> = change
                    .writes_added
                    .iter()
                    .map(|f| super::shown_folder(f))
                    .collect();
                wants.push(format!("change the files in {}", super::listed(&folders)));
            }
            lines.push(match (wants.is_empty(), change.program) {
                (true, false) => "It doesn't ask for anything new.".to_string(),
                (true, true) => "It doesn't ask for anything new, but it now starts a different \
                                 program."
                    .to_string(),
                (false, program) => format!(
                    "It now also wants to {}.{}",
                    super::listed(&wants),
                    if program {
                        " It starts a different program too."
                    } else {
                        ""
                    }
                ),
            });
        }
    }
    lines.push(
        match source {
            Source::Release => {
                "It's from the plugin's official release, and passed Slipstream's \
                                checks."
            }
            Source::Checkout(_) => "You built it on this computer, from its source.",
            Source::ChangedSince => "Its files were changed after Slipstream installed it.",
            Source::Unknown => "Slipstream didn't install it, so it can't say where it came from.",
        }
        .to_string(),
    );
    lines
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
    fn an_update_of_its_code_alone_asks_for_nothing_new() {
        let root = scratch("like");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let now = Approval::of(&timer(&root, MANIFEST, "echo two"));
        let change = compare(&before, &now);
        assert_eq!(change, Change::default());
        let said = explain(Some(&change), &Source::Release).join("\n");
        assert_eq!(
            said,
            "It doesn't ask for anything new.\nIt's from the plugin's official release, and \
             passed Slipstream's checks."
        );
    }

    /// The promise in the guide: an update that asks for more than was turned on says so, in
    /// everyday words.
    #[test]
    fn an_update_that_asks_for_more_says_what() {
        let root = scratch("more");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let more = "name = \"Timer\"\nexec = [\"./timer\", \"card\", \"--upload\"]\n\
                    read = [\"$XDG_CACHE_HOME/timer\", \"$HOME/Documents\"]\nevery-secs = 5\n\
                    needs = [\"network\"]\n";
        let plugin = timer(&root, more, "echo one");
        let change = compare(&before, &Approval::of(&plugin));
        assert!(change.asks_for_more());
        let said = explain(Some(&change), &Source::Unknown).join("\n");
        assert_eq!(
            said,
            "It now also wants to go online and look in ~/Documents. It starts a different program \
             too.\nSlipstream didn't install it, so it can't say where it came from."
        );
        // Running more often, or needing less, asks for nothing more.
        let now = Approval::of(&plugin);
        let quieter = Approval {
            needs: Vec::new(),
            every_secs: Some(600),
            ..now.clone()
        };
        assert!(!compare(&now, &quieter).asks_for_more());
    }

    /// Each of the newer needs is asking for more, and a folder to change is said as the folder.
    #[test]
    fn an_update_that_changes_files_or_sees_more_says_what() {
        let root = scratch("write");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let more = format!(
            "{MANIFEST}needs = [\"write\", \"clipboard\", \"open-windows\"]\n\
             write = [\"$HOME/Notes\"]\n"
        );
        let change = compare(&before, &Approval::of(&timer(&root, &more, "echo one")));
        assert!(change.asks_for_more());
        assert_eq!(change.writes_added, ["$HOME/Notes"]);
        let said = explain(Some(&change), &Source::Unknown);
        assert_eq!(
            said[0],
            "It now also wants to see what you copy, see which windows you have open and their \
             titles and change the files in ~/Notes."
        );
        for need in Need::ALL {
            let name = toml::Value::try_from(need).unwrap();
            let asks = format!("{MANIFEST}needs = [{name}]\n");
            let now = Approval::of(&timer(&root, &asks, "echo one"));
            assert!(compare(&before, &now).asks_for_more(), "{need:?}");
        }
    }

    #[test]
    fn an_update_that_wants_full_access_says_so_first() {
        let root = scratch("needs");
        let before = Approval::of(&timer(&root, MANIFEST, "echo one"));
        let out = format!("{MANIFEST}needs = [\"notify\", \"everything\"]\n");
        let plugin = timer(&root, &out, "echo one");
        let change = compare(&before, &Approval::of(&plugin));
        assert!(change.asks_for_more());
        let said = explain(Some(&change), &Source::Checkout("ab12".into())).join("\n");
        assert_eq!(
            said,
            "It now wants full access to your computer.\nYou built it on this computer, from \
             its source."
        );
    }

    /// A record kept before plugins said what they need still loads, as needing nothing.
    #[test]
    fn a_record_from_before_needs_still_loads() {
        let root = scratch("old-record");
        std::fs::write(
            root.join("timer.toml"),
            "fingerprint = \"ab\"\nexec = [\"./timer\"]\nread = []\n[files]\ntimer = \"cd\"\n",
        )
        .unwrap();
        let kept = load_from(&root, "timer").unwrap();
        assert!(kept.needs.is_empty());
        assert_eq!(kept.fingerprint, "ab");
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
        let said = explain(None, &Source::Unknown).join("\n");
        assert!(said.contains("can't tell what changed"), "{said}");
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

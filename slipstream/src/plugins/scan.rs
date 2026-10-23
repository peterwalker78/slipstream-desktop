//! Looking at the plugin folders, on a thread of its own: finding each plugin and its
//! fingerprint, keeping a read-only copy of each one that's on and approved as it is, moving its
//! `current` link to that copy, and clearing away copies nothing is approved as or runs from.
//! Asks that arrive while a look is under way are folded into one more look after it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc,
};

use slipstream_config::plugins::{self, Plugin, approval, store};

use super::worker::{self, Approved};

/// Where plugins are looked for and kept.
pub struct Places {
    /// The plugins folders, the person's own first.
    pub dirs: Vec<PathBuf>,
    /// Where each plugin's store goes (`store::root`).
    pub stores: PathBuf,
    /// Where what each plugin asked for when it was approved is kept (`approval`).
    pub approvals: PathBuf,
}

impl Places {
    pub fn real() -> Places {
        Places {
            dirs: plugins::dirs(),
            stores: store::root(),
            approvals: approval::folder(),
        }
    }
}

/// A plugin that's on but doesn't run.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub id: String,
    pub name: String,
    /// Why, when it isn't that it changed since it was turned on.
    pub why: Option<String>,
}

/// What one look found.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub installed: usize,
    pub problems: Vec<String>,
    /// The plugins to run, in the order they were turned on, each from its copy.
    pub run: Vec<Approved>,
    pub held: Vec<Held>,
}

/// What a look is asked to follow: the plugins on, and what each was approved as.
#[derive(Debug, Clone)]
struct Ask {
    on: Vec<String>,
    fingerprints: BTreeMap<String, String>,
}

/// The thread that looks. Dropping it lets the thread finish.
pub struct Scanner {
    ask: mpsc::Sender<Ask>,
}

impl Scanner {
    pub fn start(places: Places, found: Box<dyn Fn(Scan) + Send>) -> Scanner {
        let (ask, asked) = mpsc::channel::<Ask>();
        std::thread::Builder::new()
            .name("plugin scan".into())
            .spawn(move || {
                while let Ok(mut latest) = asked.recv() {
                    while let Ok(newer) = asked.try_recv() {
                        latest = newer;
                    }
                    found(scan(&places, &latest));
                }
            })
            .expect("a thread to look at plugins");
        Scanner { ask }
    }

    /// Asks for a look that follows `wanted`.
    pub fn ask(&self, wanted: &plugins::Plugins) {
        let _ = self.ask.send(Ask {
            on: wanted.on.clone(),
            fingerprints: wanted.fingerprints.clone(),
        });
    }
}

/// One look, as `ask` says.
fn scan(places: &Places, ask: &Ask) -> Scan {
    let (found, problems) = plugins::find_in(&places.dirs);
    let mut run = Vec::new();
    let mut held = Vec::new();
    // Nothing a worker runs from is cleared away while this looks.
    let in_use = worker::copies_in_use();
    for id in &ask.on {
        let Some(plugin) = found.iter().find(|plugin| plugin.id == *id) else {
            continue;
        };
        let hold = |why: Option<String>| Held {
            id: id.clone(),
            name: plugin.manifest.name.clone(),
            why,
        };
        if let Some(why) = &plugin.too_big {
            held.push(hold(Some(format!("Too big to check: {why}"))));
            continue;
        }
        if ask.fingerprints.get(id) != Some(&plugin.fingerprint) {
            held.push(hold(None));
            continue;
        }
        let kept = places.stores.join(id);
        match store::snapshot(plugin, &kept) {
            Ok(copy) => {
                if let Err(err) = store::point_current(&kept, &plugin.fingerprint) {
                    tracing::warn!("plugin {id}: its current link couldn't be moved: {err}");
                }
                // So a later update can be compared with it, where it was turned on before what
                // a plugin asks for was kept.
                approval::remember_if_missing_in(&places.approvals, plugin, &plugin.fingerprint);
                run.push(Approved {
                    plugin: Plugin {
                        dir: copy,
                        ..plugin.clone()
                    },
                    state: kept.join(store::STATE),
                    store: Some(kept),
                });
            }
            Err(why) => held.push(hold(Some(format!("Couldn't keep a copy to run: {why}")))),
        }
    }
    tidy(&places.stores, ask, &|path| {
        in_use.iter().any(|used| used == path)
    });
    Scan {
        installed: found.len(),
        problems,
        run,
        held,
    }
}

/// Clears away each store's copies that nothing is approved as or runs from, and the `current`
/// link of every plugin that's off.
fn tidy(stores: &Path, ask: &Ask, in_use: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(stores) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let kept = entry.path();
        let approved = ask
            .on
            .contains(&id)
            .then(|| ask.fingerprints.get(&id))
            .flatten();
        if approved.is_none() {
            store::clear_current(&kept);
        }
        store::prune(&kept, approved.map(String::as_str), in_use);
    }
}

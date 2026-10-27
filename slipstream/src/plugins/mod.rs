//! Plugins: little programs of the person's own, each adding a card to the panel that drops from
//! the chip beside the clock (`slipstream_config::plugins` says what a plugin and a card are).
//! Each plugin's program runs out of process, driven from a thread of its own (`worker.rs`);
//! nothing of a plugin's is ever loaded into the compositor. What a program prints is only ever
//! shown, and a button on its card only runs that same program again, with the button's id.
//!
//! Plugin folders are looked at on a thread of their own too (`scan.rs`), since reading and
//! fingerprinting them can take a while: at the start, whenever the settings change, when a
//! plugins folder changes and when the panel opens. A plugin that's on runs from the read-only
//! copy kept when it was approved (`slipstream_config::plugins::store`), never from its folder,
//! and in its sandbox unless it needs `everything`.
//!
//! What a plugin sees of the desktop beyond its sandbox, Slipstream hands it, as lines on its
//! standard input: the notifications that arrive, what's playing, the windows open and what's
//! copied, each only to a plugin that needs it. What it can have done beyond its own program, a
//! button's press does by Slipstream's hand (`Brokered`, `brokered.rs`).

mod brokered;
pub mod card;
mod scan;
pub mod surfaces;
mod worker;

pub use scan::Scan;
pub use slipstream_config::plugins::Glance;
pub use worker::Sent;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use slipstream_config::{
    Settings,
    plugins::{
        self, Card, Need, OpenWindow,
        surfaces::{Action, Hit, SEARCH_CHARS, Tile},
    },
};

use worker::{Approved, Ear, Sink, Worker};

/// What a button pressed on a card asks Slipstream to do, for a plugin that needs it, in place of
/// running its program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Brokered {
    Media(crate::media::Transport),
    /// Open this web address in the browser.
    Open(String),
    /// Put this text on the clipboard.
    Copy(String),
}

/// The least time between two looks at the open windows for the plugins that need them.
const WINDOWS_GAP: Duration = Duration::from_secs(1);

/// What's playing, watched while a plugin that needs `media` runs, and the plugins told of it.
struct MediaWatch {
    ears: Arc<Mutex<Vec<Ear>>>,
    /// Which plugins those are.
    ids: Vec<String>,
    /// Dropped last, which stops it.
    _watch: crate::media::Watch,
}

/// One plugin that's on, as it runs.
struct Running {
    approved: Approved,
    hidden: Vec<String>,
    worker: Worker,
    /// Its searches, for one with a keyword.
    searcher: Option<surfaces::Searcher>,
}

/// A plugin's tile in quick settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTile {
    pub plugin: String,
    /// The plugin's name, under the tile's own.
    pub name: String,
    pub tile: Tile,
}

/// What a plugin has to show for a search that starts with its keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    pub plugin: String,
    pub name: String,
    /// What was typed after the keyword.
    pub words: String,
    pub hits: Vec<Hit>,
    /// The hits answer these words, rather than ones typed before.
    pub fresh: bool,
    /// Why there are none, when the search for these words failed.
    pub problem: Option<String>,
}

/// The actions a plugin that's on offers keys for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub plugin: String,
    pub name: String,
    pub actions: Vec<Action>,
}

/// A plugin's card as the panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub id: String,
    pub name: String,
    /// Its card as its program printed it, every section included.
    pub card: Option<Card>,
    /// The card's sections not to show, by name.
    pub hidden: Vec<String>,
    /// Why there's no card, or why the one shown is old.
    pub problem: Option<String>,
    /// It is on but doesn't run: it has changed since it was turned on, or can't be checked.
    pub held: bool,
}

impl Shown {
    /// Its card with only the sections that show.
    pub fn visible(&self) -> Option<Card> {
        self.card.clone().map(|card| card.without(&self.hidden))
    }
}

/// What the chip on the bar shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chip {
    /// How many plugins there are, and how many are on.
    pub installed: usize,
    pub on: usize,
    /// The chosen plugin's headline.
    pub glance: Option<plugins::Glance>,
    /// A plugin that's on wants looking at.
    pub attention: bool,
}

/// Every plugin there is, and the ones on, running.
pub struct Plugins {
    installed: usize,
    running: Vec<Running>,
    /// Plugins that are on but don't run, and why when it isn't that they changed.
    held: Vec<scan::Held>,
    /// What the settings last said: which plugins are on, as approved, and what their cards show.
    wanted: plugins::Plugins,
    /// What was last logged as wrong, so a settings change elsewhere doesn't log it again.
    reported: Vec<String>,
    scanner: scan::Scanner,
    sink: Sink,
    media: Option<MediaWatch>,
    /// When the open windows were last looked at for the plugins that need them, and the line
    /// they were last told.
    windows_looked: Option<Instant>,
    windows_told: Option<String>,
}

impl Plugins {
    /// Plugins looked for where they are installed, each look's result handed to `found`, from
    /// the thread that looks. The notifications their cards send go to `sent`, from theirs.
    pub fn new(
        found: impl Fn(Scan) + Send + 'static,
        sent: impl Fn(Sent) + Send + Sync + 'static,
    ) -> Plugins {
        Plugins::looking_in(scan::Places::real(), found, sent)
    }

    fn looking_in(
        places: scan::Places,
        found: impl Fn(Scan) + Send + 'static,
        sent: impl Fn(Sent) + Send + Sync + 'static,
    ) -> Plugins {
        Plugins {
            installed: 0,
            running: Vec::new(),
            held: Vec::new(),
            wanted: plugins::Plugins::default(),
            reported: Vec::new(),
            scanner: scan::Scanner::start(places, Box::new(found)),
            sink: Arc::new(sent),
            media: None,
            windows_looked: None,
            windows_told: None,
        }
    }

    /// Follows the settings. A plugin turned off stops at once, with everything its program
    /// started; one turned on starts once its folder has been looked at.
    pub fn configure(&mut self, settings: &Settings) {
        self.wanted = settings.plugins.clone();
        let wanted = &self.wanted;
        // Whatever is dropped here is stopped.
        self.running
            .retain(|running| still_wanted(wanted, &running.approved));
        for running in &mut self.running {
            // Which sections show is only the panel's business: the program carries on.
            running.hidden = wanted.hidden(&running.approved.plugin.id).to_vec();
        }
        self.watch_media();
        self.rescan();
    }

    /// Looks at the plugin folders again, off the event loop.
    pub fn rescan(&self) {
        self.scanner.ask(&self.wanted);
    }

    /// What a look at the plugin folders found: a plugin that's on and unchanged keeps running,
    /// one newly approved starts, and one gone or held is stopped.
    pub fn scanned(&mut self, scan: Scan) {
        if scan.problems != self.reported {
            for why in &scan.problems {
                tracing::warn!("plugins: {why}");
            }
            self.reported = scan.problems;
        }
        self.installed = scan.installed;
        let wanted = &self.wanted;
        // A look asked for before the settings last changed can't start what is now off.
        self.held = scan
            .held
            .into_iter()
            .filter(|held| wanted.is_on(&held.id))
            .collect();
        let mut old = std::mem::take(&mut self.running);
        for approved in scan.run {
            if !still_wanted(&self.wanted, &approved) {
                continue;
            }
            let hidden = self.wanted.hidden(&approved.plugin.id).to_vec();
            let kept = old
                .iter()
                .position(|running| running.approved == approved)
                .map(|at| old.remove(at));
            if kept.is_none() && approved.plugin.manifest.needs(Need::OpenWindows) {
                // A newcomer is told of the windows at the next look, changed or not.
                self.windows_told = None;
            }
            let running = kept.unwrap_or_else(|| {
                tracing::debug!("starting plugin {}", approved.plugin.id);
                Running {
                    worker: Worker::start(approved.clone(), self.sink.clone()),
                    searcher: approved
                        .plugin
                        .manifest
                        .keyword
                        .is_some()
                        .then(|| surfaces::Searcher::start(approved.clone())),
                    approved,
                    hidden: Vec::new(),
                }
            });
            self.running.push(Running { hidden, ..running });
        }
        // Whatever is left in `old` is dropped here, which stops it.
        drop(old);
        self.watch_media();
    }

    /// Whether a plugin that runs needs `need`.
    pub fn wants(&self, need: Need) -> bool {
        self.running
            .iter()
            .any(|running| running.approved.plugin.manifest.needs(need))
    }

    /// Watches what's playing while a plugin that needs `media` runs, and stops when none does.
    fn watch_media(&mut self) {
        let listening: Vec<&Running> = self
            .running
            .iter()
            .filter(|running| running.approved.plugin.manifest.needs(Need::Media))
            .collect();
        if listening.is_empty() {
            self.media = None;
            return;
        }
        let ids: Vec<String> = listening
            .iter()
            .map(|running| running.approved.plugin.id.clone())
            .collect();
        let ears: Vec<Ear> = listening
            .iter()
            .map(|running| running.worker.ear())
            .collect();
        if let Some(media) = &mut self.media {
            *media.ears.lock().unwrap() = ears;
            if media.ids != ids {
                media.ids = ids;
                media._watch.again();
            }
            return;
        }
        let shared = Arc::new(Mutex::new(ears));
        let told = shared.clone();
        let watch = crate::media::Watch::start(move |now| {
            let line = plugins::media_line(
                now.as_ref()
                    .map(|now| (&*now.player, &*now.title, &*now.artist, now.playing)),
            );
            for ear in told.lock().unwrap().iter() {
                ear.hear_latest(&line, "media");
            }
        });
        self.media = watch.map(|watch| MediaWatch {
            ears: shared,
            ids,
            _watch: watch,
        });
    }

    /// Whether to look at the open windows now, for the plugins that need them: there is one,
    /// and the last look was a while ago.
    pub fn windows_due(&mut self, now: Instant) -> bool {
        if !self.wants(Need::OpenWindows) {
            self.windows_told = None;
            return false;
        }
        if self
            .windows_looked
            .is_some_and(|looked| now.saturating_duration_since(looked) < WINDOWS_GAP)
        {
            return false;
        }
        self.windows_looked = Some(now);
        true
    }

    /// The windows open now: the plugins that need them are told, if they changed.
    pub fn windows(&mut self, windows: &[OpenWindow]) {
        let line = plugins::windows_line(windows);
        if self.windows_told.as_ref() == Some(&line) {
            return;
        }
        for running in &self.running {
            if running.approved.plugin.manifest.needs(Need::OpenWindows) {
                running.worker.hear_latest(&line, "windows");
            }
        }
        self.windows_told = Some(line);
    }

    /// Text was copied: the plugins that need to see it are told.
    pub fn copied(&self, text: &str) {
        let mut line = None;
        for running in &self.running {
            if running.approved.plugin.manifest.needs(Need::Clipboard) {
                let line = line.get_or_insert_with(|| plugins::clipboard_line(text));
                running.worker.hear(line);
            }
        }
    }

    /// The cards of the plugins that are on, in order.
    pub fn shown(&self) -> Vec<Shown> {
        self.running
            .iter()
            .map(|running| {
                let published = running.worker.published();
                Shown {
                    id: running.approved.plugin.id.clone(),
                    name: running.approved.plugin.manifest.name.clone(),
                    card: published.card,
                    hidden: running.hidden.clone(),
                    problem: published.problem,
                    held: false,
                }
            })
            .chain(self.held.iter().map(|held| Shown {
                id: held.id.clone(),
                name: held.name.clone(),
                card: None,
                hidden: Vec::new(),
                problem: held.why.clone(),
                held: true,
            }))
            .collect()
    }

    /// What the chip shows, from `shown`. None when there are no plugins at all.
    pub fn chip(&self, shown: &[Shown]) -> Option<Chip> {
        (self.installed > 0).then(|| Chip {
            installed: self.installed,
            on: self.running.len() + self.held.len(),
            glance: self.wanted.glance().and_then(|id| {
                let glance = shown
                    .iter()
                    .find(|shown| shown.id == id)?
                    .visible()?
                    .glance();
                (!glance.text.is_empty()).then_some(glance)
            }),
            attention: shown
                .iter()
                .any(|shown| shown.visible().is_some_and(|card| card.attention())),
        })
    }

    /// Button `button` on plugin `id`'s card was pressed: its program runs with `press <button>`
    /// after it, on the plugin's own thread and in its turn, and then runs as usual again to
    /// show what changed. A button that opens a web address, copies or controls what's playing
    /// is what's returned instead, for Slipstream to do, if the plugin needs it.
    pub fn press(&self, id: &str, button: &str) -> Option<Brokered> {
        let running = self
            .running
            .iter()
            .find(|running| running.approved.plugin.id == id)?;
        let needs = |need| running.approved.plugin.manifest.needs(need);
        let pressed = running
            .worker
            .published()
            .card
            .and_then(|card| card.buttons.into_iter().find(|shown| shown.id == button));
        if let Some(pressed) = pressed {
            if let Some(address) = pressed.open {
                return needs(Need::Open).then_some(Brokered::Open(address));
            }
            if let Some(text) = pressed.copy {
                return needs(Need::Clipboard).then_some(Brokered::Copy(text));
            }
        }
        if let Some(transport) = media_control(button) {
            return needs(Need::Media).then_some(Brokered::Media(transport));
        }
        if plugins::valid_id(button) {
            running.worker.press(button);
        }
        None
    }

    /// The tiles of the plugins that are on and say they have one, in order.
    pub fn tiles(&self) -> Vec<PluginTile> {
        self.running
            .iter()
            .filter(|running| running.approved.plugin.manifest.tile)
            .filter_map(|running| {
                let tile = running.worker.published().card?.tile?;
                Some(PluginTile {
                    plugin: running.approved.plugin.id.clone(),
                    name: running.approved.plugin.manifest.name.clone(),
                    tile,
                })
            })
            .collect()
    }

    /// `query` was typed in the explorer: each plugin whose keyword it starts with is asked the
    /// words after it, off the event loop, and says what it has so far. Any other plugin is
    /// neither asked nor told.
    pub fn search(&self, query: &str) -> Vec<Answers> {
        self.running
            .iter()
            .filter_map(|running| {
                let searcher = running.searcher.as_ref()?;
                let manifest = &running.approved.plugin.manifest;
                let words: String = manifest
                    .searched(query)?
                    .chars()
                    .take(SEARCH_CHARS)
                    .collect();
                if !words.is_empty() {
                    searcher.ask(&words);
                }
                let answer = searcher.answer().filter(|_| !words.is_empty());
                let fresh = answer.as_ref().is_some_and(|answer| answer.words == words);
                let (hits, problem) = match answer.map(|answer| answer.found) {
                    Some(Ok(hits)) => (hits, None),
                    Some(Err(why)) => (Vec::new(), fresh.then_some(why)),
                    None => (Vec::new(), None),
                };
                Some(Answers {
                    plugin: running.approved.plugin.id.clone(),
                    name: manifest.name.clone(),
                    words,
                    hits,
                    fresh,
                    problem,
                })
            })
            .collect()
    }

    /// The actions the plugins that are on offer keys for.
    pub fn offered(&self) -> Vec<Offered> {
        self.running
            .iter()
            .filter(|running| !running.approved.plugin.manifest.actions.is_empty())
            .map(|running| Offered {
                plugin: running.approved.plugin.id.clone(),
                name: running.approved.plugin.manifest.name.clone(),
                actions: running.approved.plugin.manifest.actions.clone(),
            })
            .collect()
    }

    /// The keys for plugin `id`'s action `action` were pressed: it runs as a press of a button
    /// with that id does, if the plugin is on and lists the action.
    pub fn act(&self, id: &str, action: &str) {
        let listed = self.running.iter().any(|running| {
            running.approved.plugin.id == id
                && running.approved.plugin.manifest.action(action).is_some()
        });
        if listed {
            self.press(id, action);
        }
    }

    /// A notification arrived: the plugins that need to see them are told, except the one that
    /// sent it, if a plugin did.
    pub fn notification(&self, app: &str, title: &str, body: &str, from: Option<&str>) {
        let mut line = None;
        for running in &self.running {
            let plugin = &running.approved.plugin;
            if plugin.manifest.needs(Need::Notifications) && from != Some(plugin.id.as_str()) {
                let line = line.get_or_insert_with(|| {
                    plugins::notification_line(
                        app,
                        title,
                        body,
                        slipstream_config::card::unix_now(),
                    )
                });
                running.worker.hear(line);
            }
        }
    }

    /// Stops every plugin, now: the compositor is on its way out.
    pub fn stop_all(&mut self) {
        self.media = None;
        self.running.clear();
        slipstream_config::command::end_all();
    }
}

/// The control a media button's id names.
/// Whether the settings still have `approved` switched on as it was when it started.
fn still_wanted(wanted: &plugins::Plugins, approved: &Approved) -> bool {
    wanted.is_on(&approved.plugin.id)
        && wanted.fingerprints.get(&approved.plugin.id) == Some(&approved.granted)
}

fn media_control(id: &str) -> Option<crate::media::Transport> {
    use crate::media::Transport;
    match id {
        "media:play-pause" => Some(Transport::PlayPause),
        "media:next" => Some(Transport::Next),
        "media:previous" => Some(Transport::Previous),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::mpsc,
        time::{Duration, Instant},
    };

    use slipstream_config::plugins::{Plugin, store};

    use super::*;

    /// A scratch folder for one test, on disk under the workspace's `target/test-scratch`.
    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("plugins-{name}-{}", std::process::id()));
        let _ = store::remove_tree(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Plugins looked for in `root/plugins`, kept in `root/stores`, with each look's result
    /// handed back here.
    fn looking(root: &Path) -> (Plugins, mpsc::Receiver<Scan>) {
        let (found, scans) = mpsc::channel();
        let places = scan::Places {
            dirs: vec![root.join("plugins"), root.join("bundled")],
            bundled: Some(root.join("bundled")),
            stores: root.join("stores"),
            approvals: root.join("approvals"),
        };
        let plugins = Plugins::looking_in(
            places,
            move |scan| {
                let _ = found.send(scan);
            },
            |_| {},
        );
        (plugins, scans)
    }

    /// Follows `settings`, as the event loop would once the look it asks for comes back.
    fn follow(plugins: &mut Plugins, scans: &mpsc::Receiver<Scan>, settings: &Settings) {
        plugins.configure(settings);
        looked(plugins, scans);
    }

    /// Takes the next look's result, as the event loop would.
    fn looked(plugins: &mut Plugins, scans: &mpsc::Receiver<Scan>) {
        let scan = scans.recv_timeout(Duration::from_secs(20)).unwrap();
        plugins.scanned(scan);
    }

    fn settle(plugins: &Plugins, until: impl Fn(&[Shown]) -> bool) -> Vec<Shown> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let shown = plugins.shown();
            if until(&shown) || Instant::now() > deadline {
                return shown;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn gone(path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        !path.exists()
    }

    /// Writes plugin `id` into `root/plugins`, its manifest running `sh -c script` with
    /// everything the test has: its own files, and process ids that mean the same outside.
    fn install(root: &Path, id: &str, script: &str, every: Option<u64>) -> Plugin {
        install_needing(root, id, script, every, &["everything"])
    }

    /// `install`, but needing only `needs`, in its sandbox otherwise.
    fn install_needing(
        root: &Path,
        id: &str,
        script: &str,
        every: Option<u64>,
        needs: &[&str],
    ) -> Plugin {
        let dir = root.join("plugins").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut manifest = toml::Table::new();
        manifest.insert("name".into(), id.into());
        manifest.insert(
            "exec".into(),
            toml::Value::Array(vec!["sh".into(), "-c".into(), script.into(), id.into()]),
        );
        if let Some(every) = every {
            manifest.insert("every-secs".into(), (every as i64).into());
        }
        let needs = needs.iter().map(|need| toml::Value::from(*need)).collect();
        manifest.insert("needs".into(), toml::Value::Array(needs));
        std::fs::write(
            dir.join(plugins::MANIFEST),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        plugins::read(&dir).unwrap()
    }

    /// Settings with `plugin` turned on as it is now.
    fn turned_on(plugin: &Plugin) -> Settings {
        let mut settings = Settings::default();
        settings
            .plugins
            .set_on(&plugin.id, Some(plugin.fingerprint.clone()));
        settings
    }

    /// A card whose only line is the name of the folder its program runs in.
    const SAYS_WHERE: &str = r#"printf '{"lines":["%s"]}' "$(basename "$(pwd)")""#;

    fn first_line(shown: &[Shown]) -> Option<String> {
        shown.first()?.card.as_ref()?.lines.first().cloned()
    }

    #[test]
    fn a_plugin_runs_and_its_glance_is_on_the_chip() {
        let root = scratch("glance");
        let plugin = install(
            &root,
            "allowance",
            r#"printf '{"sections":[{"name":"Disk","meters":[{"label":"Home","percent":81}]},{"name":"Mail","meters":[]}]}'"#,
            Some(60),
        );
        let mut settings = turned_on(&plugin);
        settings.plugins.glance = Some("allowance".into());
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &settings);
        let shown = settle(&plugins, |shown| shown[0].card.is_some());
        let card = shown[0].card.as_ref().unwrap();
        assert_eq!(card.sections.len(), 2);
        let chip = plugins.chip(&shown).unwrap();
        assert_eq!(chip.glance.unwrap().text, "81%");
        assert!(chip.attention);
        assert_eq!((chip.installed, chip.on), (1, 1));
        plugins.configure(&Settings::default());
        assert!(plugins.shown().is_empty(), "off at once, before any look");
    }

    /// The promise in the guide: a plugin runs from a copy of what was approved, and a change to
    /// its folder doesn't run until it is turned on again.
    #[test]
    fn a_plugin_runs_from_its_approved_copy_and_a_change_is_held() {
        let root = scratch("held");
        let plugin = install(&root, "tides", SAYS_WHERE, Some(60));
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&plugin));
        let shown = settle(&plugins, |shown| first_line(shown).is_some());
        assert_eq!(first_line(&shown), Some(plugin.fingerprint.clone()));

        std::fs::write(plugin.dir.join("extra.sh"), "echo more\n").unwrap();
        plugins.rescan();
        looked(&mut plugins, &scans);
        let shown = plugins.shown();
        assert!(shown[0].held && shown[0].card.is_none(), "{shown:?}");
        assert_eq!(
            store::current(&root.join("stores/tides")).as_deref(),
            Some(plugin.fingerprint.as_str()),
            "what was approved stays approved"
        );
    }

    /// The promise in the guide: a plugin that comes with Slipstream runs its update straight
    /// away when it asks for nothing more, without the settings changing, and is held when it
    /// asks for more.
    #[test]
    fn a_bundled_plugin_runs_a_like_for_like_update_and_holds_one_asking_for_more() {
        let root = scratch("bundled");
        let plugin = install(&root, "clock", SAYS_WHERE, Some(60));
        let dir = root.join("bundled/clock");
        std::fs::create_dir_all(root.join("bundled")).unwrap();
        std::fs::rename(&plugin.dir, &dir).unwrap();
        let (mut plugins, scans) = looking(&root);
        let settings = turned_on(&plugin);
        follow(&mut plugins, &scans, &settings);
        let shown = settle(&plugins, |shown| first_line(shown).is_some());
        assert_eq!(first_line(&shown), Some(plugin.fingerprint.clone()));

        std::fs::write(dir.join("extra.sh"), "echo more\n").unwrap();
        let updated = plugins::read(&dir).unwrap();
        plugins.rescan();
        looked(&mut plugins, &scans);
        let shown = settle(&plugins, |shown| {
            first_line(shown).as_deref() == Some(updated.fingerprint.as_str())
        });
        assert!(!shown[0].held, "{shown:?}");
        assert_eq!(first_line(&shown), Some(updated.fingerprint.clone()));
        let kept = root.join("stores/clock");
        assert_eq!(
            store::current(&kept).as_deref(),
            Some(updated.fingerprint.as_str())
        );
        assert!(gone(&kept.join(&plugin.fingerprint)), "the old copy goes");
        // A settings change elsewhere keeps it running as it is.
        plugins.configure(&settings);
        assert_eq!(plugins.shown().len(), 1);
        looked(&mut plugins, &scans);

        let manifest = std::fs::read_to_string(dir.join(plugins::MANIFEST)).unwrap();
        std::fs::write(
            dir.join(plugins::MANIFEST),
            format!("{manifest}tile = true\n"),
        )
        .unwrap();
        plugins.rescan();
        looked(&mut plugins, &scans);
        let shown = plugins.shown();
        assert!(shown[0].held && shown[0].card.is_none(), "{shown:?}");
    }

    #[test]
    fn making_a_file_a_program_since_it_was_approved_holds_the_plugin() {
        let root = scratch("mode");
        let plugin = install(&root, "kettle", SAYS_WHERE, Some(60));
        std::fs::write(plugin.dir.join("helper"), "echo hi\n").unwrap();
        let plugin = plugins::read(&plugin.dir).unwrap();
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&plugin));
        assert!(!plugins.shown()[0].held);
        let program = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(plugin.dir.join("helper"), program).unwrap();
        plugins.rescan();
        looked(&mut plugins, &scans);
        assert!(plugins.shown()[0].held);
    }

    /// Approving sends the fingerprint the person was shown: a folder that has changed since
    /// isn't copied or run.
    #[test]
    fn approving_with_a_fingerprint_the_folder_no_longer_has_runs_nothing() {
        let root = scratch("stale");
        let shown_then = install(&root, "ferry", SAYS_WHERE, Some(60));
        std::fs::write(shown_then.dir.join("later.sh"), "echo later\n").unwrap();
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&shown_then));
        let shown = plugins.shown();
        assert!(shown[0].held, "{shown:?}");
        let kept = root.join("stores/ferry");
        assert!(!kept.join(&shown_then.fingerprint).exists());
        assert_eq!(store::current(&kept), None);
    }

    /// The promise in the guide: commands a plugin offers follow its approval, through the
    /// `current` link, and an old copy goes once nothing runs from it.
    #[test]
    fn current_moves_to_the_copy_approved_and_the_old_one_goes() {
        let root = scratch("current");
        let first = install(&root, "lantern", SAYS_WHERE, Some(60));
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&first));
        let kept = root.join("stores/lantern");
        assert_eq!(
            store::current(&kept).as_deref(),
            Some(first.fingerprint.as_str())
        );
        settle(&plugins, |shown| first_line(shown).is_some());

        std::fs::write(first.dir.join("glow.sh"), "echo glow\n").unwrap();
        let second = plugins::read(&first.dir).unwrap();
        follow(&mut plugins, &scans, &turned_on(&second));
        assert_eq!(
            store::current(&kept).as_deref(),
            Some(second.fingerprint.as_str())
        );
        assert!(kept.join(store::CURRENT).join("glow.sh").exists());
        assert!(
            gone(&kept.join(&first.fingerprint)),
            "the old copy goes once its worker has"
        );

        settle(&plugins, |shown| first_line(shown).is_some());
        follow(&mut plugins, &scans, &Settings::default());
        assert_eq!(store::current(&kept), None, "off: nothing is approved");
        assert!(gone(&kept.join(&second.fingerprint)));
    }

    /// The copy is checked again on the worker's thread before each start of its program.
    #[test]
    fn a_copy_changed_after_it_was_approved_doesn_t_run() {
        let root = scratch("altered");
        let said = root.join("ran");
        let plugin = install(
            &root,
            "harbour",
            &format!("touch '{}'; printf '{{}}'", said.display()),
            Some(60),
        );
        let kept = root.join("stores").join("harbour");
        let copied = store::snapshot(&plugin, &kept).unwrap();
        // Read-only, but its owner can still open it up and add to it.
        std::fs::set_permissions(&copied.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(copied.dir.join("extra"), "x").unwrap();
        let worker = Worker::start(
            Approved {
                granted: copied.fingerprint.clone(),
                plugin: copied,
                state: kept.join(store::STATE),
                store: Some(kept),
            },
            Arc::new(|_| {}),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while worker.published().problem.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let problem = worker.published().problem.unwrap();
        assert!(problem.contains("changed"), "{problem}");
        assert!(!said.exists(), "nothing of it ran");
    }

    /// Looking at a hostile folder of a hundred thousand files never holds up the event loop,
    /// and the look itself stops at the limit.
    #[test]
    fn a_hostile_folder_is_looked_at_without_holding_up_the_loop() {
        let root = scratch("hostile");
        let plugin = install(&root, "flood", SAYS_WHERE, Some(60));
        for n in 0..100_000 {
            std::fs::File::create(plugin.dir.join(n.to_string())).unwrap();
        }
        let mut settings = Settings::default();
        settings.plugins.set_on("flood", Some("0".repeat(64)));
        let (mut plugins, scans) = looking(&root);
        let started = Instant::now();
        plugins.configure(&settings);
        assert!(started.elapsed() < Duration::from_millis(50));
        let scan = scans.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        plugins.scanned(scan);
        let shown = plugins.shown();
        assert!(shown[0].held);
        let problem = shown[0].problem.as_deref().unwrap();
        assert!(problem.contains("Too big to check"), "{problem}");
        let _ = store::remove_tree(&root);
    }

    /// The promises in the guide: only a search that starts with its keyword reaches a plugin, as
    /// `search` and the words after it, and a plugin without one is never asked. Its tile shows
    /// once its manifest says it has one, and only the actions it lists can be pressed.
    #[test]
    fn a_plugin_answers_only_searches_after_its_keyword() {
        let root = scratch("surfaces");
        let asked = root.join("asked");
        let script = format!(
            r#"case "$1" in
search) printf 'search %s\n' "$2" >> '{asked}'; printf '{{"results":[{{"id":"r1","label":"%s!"}}]}}' "$2";;
press) printf 'press %s\n' "$2" >> '{asked}'; printf '{{}}';;
*) printf '{{"tile":{{"id":"flip","label":"Flip","on":true}}}}';;
esac"#,
            asked = asked.display()
        );
        let sums = install(&root, "sums", &script, Some(60));
        let plain = install(&root, "plain", &script, Some(60));
        let manifest = sums.dir.join(plugins::MANIFEST);
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            format!(
                "tile = true\nkeyword = \"=\"\n{text}[[actions]]\nid = \"go\"\nlabel = \"Go\"\n"
            ),
        )
        .unwrap();
        let sums = plugins::read(&sums.dir).unwrap();
        let mut settings = turned_on(&sums);
        settings
            .plugins
            .set_on(&plain.id, Some(plain.fingerprint.clone()));
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &settings);
        settle(&plugins, |shown| {
            shown.len() == 2 && shown.iter().all(|s| s.card.is_some())
        });
        let tiles = plugins.tiles();
        assert_eq!(tiles.len(), 1, "only the plugin that says it has a tile");
        assert_eq!(
            (tiles[0].plugin.as_str(), tiles[0].tile.label.as_str()),
            ("sums", "Flip")
        );

        assert!(plugins.search("hello").is_empty());
        let deadline = Instant::now() + Duration::from_secs(5);
        let answers = loop {
            let answers = plugins.search(" = 2+2");
            if answers.iter().all(|a| a.fresh) || Instant::now() > deadline {
                break answers;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].words, "2+2");
        assert_eq!(answers[0].hits[0].label, "2+2!");

        plugins.act("sums", "format-disk");
        plugins.act("plain", "go");
        plugins.act("sums", "go");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&asked).is_ok_and(|said| said.contains("press"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&asked).unwrap(),
            "search 2+2\npress go\n"
        );
    }

    /// The promise in the guide: a button only runs its plugin's own program again, with the
    /// button's id as a word of its own, which the shell never reads.
    #[test]
    fn a_press_runs_the_plugin_s_own_program_with_the_button_s_id() {
        let root = scratch("press");
        let said = root.join("said");
        let plugin = install(
            &root,
            "kettle",
            &format!(
                r#"[ "$1" = press ] && printf '%s' "$2" > '{}'; printf '{{}}'"#,
                said.display()
            ),
            Some(60),
        );
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&plugin));
        settle(&plugins, |shown| shown[0].card.is_some());
        plugins.press("kettle", "$(touch nope)");
        plugins.press("kettle", "go");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !said.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "go");
        assert!(!Path::new("nope").exists());
    }

    /// Starts a worker for `plugin` straight from its folder.
    fn worker(root: &Path, plugin: Plugin) -> Worker {
        Worker::start(
            Approved {
                granted: plugin.fingerprint.clone(),
                plugin,
                state: root.join("state"),
                store: None,
            },
            Arc::new(|_| {}),
        )
    }

    /// Whether anything is still running in process group `group`. One that has ended stays in
    /// the group until it is reaped, which in a container with no init to adopt it is never.
    fn alive(group: i32) -> bool {
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return false;
        };
        processes.flatten().any(|process| {
            let stat = std::fs::read_to_string(process.path().join("stat")).unwrap_or_default();
            // After the name in brackets: the state, the parent and the group.
            let mut fields = stat
                .rsplit_once(')')
                .unwrap_or_default()
                .1
                .split_whitespace();
            let state = fields.next();
            state.is_some_and(|state| state != "Z")
                && fields.nth(1).and_then(|pgrp| pgrp.parse().ok()) == Some(group)
        })
    }

    /// Waits up to five seconds for `file` to say something, and returns it.
    fn said_in(file: &Path, until: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let said = std::fs::read_to_string(file).unwrap_or_default();
            if until(&said) || Instant::now() > deadline {
                return said;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Turning off a plugin whose program keeps running ends it at once, not at its next line.
    #[test]
    fn a_plugin_that_keeps_running_stops_at_once_when_turned_off() {
        let root = scratch("off");
        let pid = root.join("pid");
        let plugin = install(
            &root,
            "beacon",
            &format!(
                "echo $$ > '{}'; printf '{{}}\\n'; while :; do sleep 1; done",
                pid.display()
            ),
            None,
        );
        let worker = worker(&root, plugin);
        let deadline = Instant::now() + Duration::from_secs(5);
        while worker.published().card.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Well into its wait for the next line.
        std::thread::sleep(Duration::from_millis(300));
        let leader: i32 = said_in(&pid, |said| said.ends_with('\n'))
            .trim()
            .parse()
            .unwrap();
        assert!(alive(leader));
        let started = Instant::now();
        drop(worker);
        while alive(leader) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "{:?}",
            started.elapsed()
        );
    }

    /// A press while the program runs waits for that run to finish.
    #[test]
    fn a_press_during_a_run_waits_its_turn() {
        let root = scratch("turn");
        let log = root.join("log");
        let plugin = install(
            &root,
            "mill",
            &format!(
                r#"if [ "$1" = press ]; then echo press >> '{0}'; else echo start >> '{0}'; sleep 0.5; echo end >> '{0}'; fi; printf '{{}}'"#,
                log.display()
            ),
            Some(60),
        );
        let worker = worker(&root, plugin);
        said_in(&log, |said| said.contains("start"));
        worker.press("grind");
        let said = said_in(&log, |said| said.matches("start").count() == 2);
        let order: Vec<&str> = said.lines().collect();
        assert_eq!(order, ["start", "end", "press", "start"], "{said}");
    }

    /// A press that leaves something running leaves nothing once its plugin is stopped.
    #[test]
    fn a_press_that_leaves_a_program_running_leaves_nothing_once_stopped() {
        let root = scratch("left");
        let pid = root.join("pid");
        let plugin = install(
            &root,
            "sluice",
            &format!(
                r#"if [ "$1" = press ]; then echo $$ > '{}'; sleep 30 & wait; fi; printf '{{}}'"#,
                pid.display()
            ),
            Some(60),
        );
        let worker = worker(&root, plugin);
        worker.press("open");
        let leader: i32 = said_in(&pid, |said| said.ends_with('\n'))
            .trim()
            .parse()
            .unwrap();
        assert!(alive(leader));
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive(leader) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!alive(leader), "the press's whole group was ended");
    }

    /// A `[meter]` moved into a plugin runs its command as it was written: a line ending in a
    /// keyword or a comment still works, since nothing is added to it.
    #[test]
    fn a_moved_meter_runs_its_command_as_written() {
        let root = scratch("moved-meter");
        let settings_file = root.join("settings.toml");
        std::fs::write(
            &settings_file,
            r#"[meter]
command = '''if true; then printf '{"lines":["%s"]}' "${1:-ran}"; fi # last word'''
"#,
        )
        .unwrap();
        let moved = plugins::move_meter(&settings_file, &root, &root.join("moved"))
            .unwrap()
            .unwrap();
        let meter = plugins::read(&moved).unwrap();
        let state = root.join("state");
        assert!(meter.manifest.unconfined(), "it ran as the person before");
        let out = worker::program(&meter, &state, &[])
            .unwrap()
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), r#"{"lines":["ran"]}"#);
        let out = worker::program(&meter, &state, &["press", "go"])
            .unwrap()
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            r#"{"lines":["press"]}"#,
            "a button's words reach it as $1 and $2"
        );
    }

    /// The promise in the guide: a sandboxed plugin has a login keyring of its own, so it can't
    /// see the keys kept in yours.
    #[test]
    fn a_sandboxed_plugin_has_a_keyring_of_its_own() {
        // A secret in a session keyring of the test's own, as a login might keep one.
        // SAFETY: plain system calls with valid C strings, changing only this process's keyrings.
        let key = unsafe {
            libc::syscall(
                libc::SYS_keyctl,
                libc::KEYCTL_JOIN_SESSION_KEYRING,
                std::ptr::null::<libc::c_char>(),
            );
            libc::syscall(
                libc::SYS_add_key,
                c"user".as_ptr(),
                c"slipstream-test".as_ptr(),
                c"hunter2".as_ptr(),
                7,
                libc::KEY_SPEC_SESSION_KEYRING,
            )
        };
        assert!(key > 0, "{}", std::io::Error::last_os_error());
        let root = scratch("keyring");
        let script = format!("keyctl print {key} 2>&1; echo done");
        let plugin = install_needing(&root, "keys", &script, None, &[]);
        let out = worker::program(&plugin, &root.join("state"), &[])
            .unwrap()
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.ends_with("done\n"), "{said}");
        assert!(!said.contains("hunter2"), "{said}");
        // Run as the person, it could, so the test does test something.
        let everything = install(&root, "keys-too", &script, None);
        let out = worker::program(&everything, &root.join("state"), &[])
            .unwrap()
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("hunter2"));
    }

    /// The promise in the guide: a plugin's program is given only the environment that says who
    /// and where the person is, never the session's sockets or keys.
    #[test]
    fn a_plugin_s_program_runs_without_the_session_s_secrets() {
        let root = scratch("env");
        let plugin = install_needing(&root, "env", "env; pwd", None, &[]);
        // The test runs with all of cargo's environment, and the session's: none of it gets
        // through but what the list names, and what `sh` sets for itself.
        let state = root.join("state");
        let out = worker::program(&plugin, &state, &[])
            .unwrap()
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        let (env, folder) = said.trim_end().rsplit_once('\n').unwrap();
        for line in env.lines() {
            let name = line.split('=').next().unwrap();
            assert!(
                plugins::ENVIRONMENT.contains(&name)
                    || ["PWD", "SHLVL", "_", "SLIPSTREAM_PLUGIN_STATE"].contains(&name),
                "{name} got through"
            );
        }
        assert!(env.contains("HOME="), "{said}");
        assert!(
            env.contains(&format!("SLIPSTREAM_PLUGIN_STATE={}", state.display())),
            "{said}"
        );
        assert_eq!(
            folder,
            plugin.dir.display().to_string(),
            "run in its folder"
        );
    }

    /// The promise in the guide: in its sandbox a plugin reaches its own folders and what it says
    /// it reads, and nothing else of yours: not your files, not your session bus, not the network.
    #[test]
    fn a_sandboxed_plugin_reaches_only_what_it_says() {
        let root = scratch("sandbox");
        let secret = root.join("secret");
        std::fs::write(&secret, "x").unwrap();
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("note"), "hello").unwrap();
        let bus = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("bus"))
            .unwrap_or_default();
        let script = format!(
            "test -e '{secret}' && echo saw-secret; test -e '{bus}' && echo saw-bus; \
             cat \"$SLIPSTREAM_PLUGIN_READ_0/note\"; echo; \
             echo kept > \"$SLIPSTREAM_PLUGIN_STATE/kept\" && echo wrote-state; \
             touch ./mine 2>/dev/null && echo wrote-own; \
             unshare -Urm true 2>/dev/null && echo nested-namespace; \
             grep -c : /proc/net/dev; grep -c '^Seccomp:.2' /proc/self/status",
            secret = secret.display(),
            bus = bus.display(),
        );
        let dir = root.join("plugins/sandboxed");
        let plugin = install_needing(&root, "sandboxed", &script, None, &[]);
        let mut manifest = std::fs::read_to_string(dir.join(plugins::MANIFEST)).unwrap();
        manifest.push_str(&format!("read = [\"{}\"]\n", shared.display()));
        std::fs::write(dir.join(plugins::MANIFEST), manifest).unwrap();
        let plugin = plugins::Plugin {
            manifest: plugins::read(&dir).unwrap().manifest,
            ..plugin
        };
        let state = root.join("state");
        let out = worker::program(&plugin, &state, &[])
            .unwrap()
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        let err = String::from_utf8_lossy(&out.stderr);
        // The last line: the filter of system calls is in force.
        assert_eq!(said, "hello\nwrote-state\n1\n1\n", "{err}");
        assert_eq!(
            std::fs::read_to_string(state.join("kept")).unwrap(),
            "kept\n"
        );
    }

    /// The promise in the guide: a plugin that needs `notifications` is told of each one that
    /// arrives, and one that needs `notify` sends them from its card. One that doesn't need them
    /// is told nothing, and its card sends nothing.
    #[test]
    fn a_plugin_hears_notifications_and_sends_them_only_if_it_needs_to() {
        let root = scratch("notifications");
        let (found, scans) = mpsc::channel();
        let (sink, sent) = mpsc::channel();
        let sink = std::sync::Mutex::new(sink);
        let places = scan::Places {
            dirs: vec![root.join("plugins")],
            bundled: None,
            stores: root.join("stores"),
            approvals: root.join("approvals"),
        };
        let mut plugins = Plugins::looking_in(
            places,
            move |scan| {
                let _ = found.send(scan);
            },
            move |notice| {
                let _ = sink.lock().unwrap().send(notice);
            },
        );
        let echo = r#"while read -r line; do
            title=$(printf '%s' "$line" | sed 's/.*"title":"\([^"]*\)".*/\1/')
            printf '{"lines":["%s"],"notify":{"title":"heard %s"}}\n' "$title" "$title"
        done"#;
        let listener = install_needing(&root, "log", echo, None, &["notify", "notifications"]);
        let deaf = install_needing(&root, "deaf", echo, None, &[]);
        let mut settings = turned_on(&listener);
        settings
            .plugins
            .set_on(&deaf.id, Some(deaf.fingerprint.clone()));
        follow(&mut plugins, &scans, &settings);
        // Wait for both to be running, then a notification arrives.
        let deadline = Instant::now() + Duration::from_secs(5);
        while plugins.running.len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        plugins.notification("Mail", "Ferry delayed", "The 9:40 is running late", None);
        let shown = settle(&plugins, |shown| {
            shown.iter().any(|shown| shown.card.is_some())
        });
        let log = shown.iter().find(|shown| shown.id == "log").unwrap();
        assert_eq!(
            log.card.as_ref().map(|card| card.lines.clone()),
            Some(vec!["Ferry delayed".to_string()]),
            "{shown:?}"
        );
        let notice = sent.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(notice.title, "heard Ferry delayed");
        assert_eq!((notice.id.as_str(), notice.name.as_str()), ("log", "log"));
        // Its own, coming back, isn't told to it.
        plugins.notification("log", &notice.title, "", Some("log"));
        std::thread::sleep(Duration::from_millis(500));
        let deaf = plugins.shown().into_iter().find(|shown| shown.id == "deaf");
        assert!(
            deaf.unwrap().card.is_none(),
            "one that doesn't need it hears nothing"
        );
        assert!(sent.try_recv().is_err());
    }

    /// `install_needing`, with `more` added to its manifest.
    fn install_with(root: &Path, id: &str, script: &str, needs: &[&str], more: &str) -> Plugin {
        let plugin = install_needing(root, id, script, None, needs);
        let dir = root.join("plugins").join(id);
        let mut manifest = std::fs::read_to_string(dir.join(plugins::MANIFEST)).unwrap();
        manifest.push_str(more);
        std::fs::write(dir.join(plugins::MANIFEST), manifest).unwrap();
        plugins::Plugin {
            manifest: plugins::read(&dir).unwrap().manifest,
            ..plugin
        }
    }

    /// The promise in the guide: in its sandbox a plugin that needs `write` changes the folders
    /// it names and nothing else of yours, those it reads staying read-only, and only one that
    /// needs `system` sees `/sys`, and only read-only.
    #[test]
    fn write_and_system_give_exactly_what_they_say() {
        let root = scratch("write");
        let notes = root.join("notes");
        let music = root.join("music");
        let other = root.join("other");
        for folder in [&notes, &music, &other] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(notes.join("old"), "x").unwrap();
        let script = format!(
            "echo new > \"$SLIPSTREAM_PLUGIN_WRITE_0/new\" && echo wrote-notes; \
             rm \"$SLIPSTREAM_PLUGIN_WRITE_0/old\" && echo removed; \
             touch \"$SLIPSTREAM_PLUGIN_READ_0/x\" 2>/dev/null && echo wrote-music; \
             touch '{other}/x' 2>/dev/null && echo wrote-other; \
             test -d /sys/class && echo saw-sys; \
             awk '$5 == \"/sys\" {{ print substr($6, 1, 2) }}' /proc/self/mountinfo",
            other = other.display(),
        );
        let more = format!(
            "write = [\"{}\"]\nread = [\"{}\"]\n",
            notes.display(),
            music.display()
        );
        let plugin = install_with(&root, "notes", &script, &["write", "system"], &more);
        let state = root.join("state");
        let out = worker::program(&plugin, &state, &[])
            .unwrap()
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "wrote-notes\nremoved\nsaw-sys\nro\n",
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(notes.join("new")).unwrap(), "new\n");
        assert!(!notes.join("old").exists());
        assert!(!music.join("x").exists() && !other.join("x").exists());

        // Without them, neither the folder nor `/sys` is there.
        let script = format!(
            "touch '{notes}/y' 2>/dev/null && echo wrote-notes; \
             test -e /sys/class && echo saw-sys; \
             awk '$5 == \"/sys\" {{ print $5 }}' /proc/self/mountinfo",
            notes = notes.display(),
        );
        let bare = install_with(&root, "bare", &script, &[], "");
        let out = worker::program(&bare, &state, &[])
            .unwrap()
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
        assert!(!notes.join("y").exists());
    }

    /// The promise in the guide: a plugin that needs `open-windows` or `clipboard` is told which
    /// windows are open, or what's copied, as lines on its input, and one that doesn't is told
    /// nothing.
    #[test]
    fn a_plugin_sees_windows_and_copies_only_if_it_needs_to() {
        let root = scratch("seeing");
        let (mut plugins, scans) = looking(&root);
        let echo = r#"while read -r line; do
            said=$(printf '%s' "$line" | sed -n 's/.*"title":"\([^"]*\)".*/\1/p; s/.*"text":"\([^"]*\)".*/\1/p')
            printf '{"lines":["%s"]}\n' "$said"
        done"#;
        let seeing = install_needing(&root, "seeing", echo, None, &["open-windows", "clipboard"]);
        let blind = install_needing(&root, "blind", echo, None, &[]);
        let mut settings = turned_on(&seeing);
        settings
            .plugins
            .set_on(&blind.id, Some(blind.fingerprint.clone()));
        follow(&mut plugins, &scans, &settings);
        let deadline = Instant::now() + Duration::from_secs(5);
        while plugins.running.len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(plugins.wants(Need::OpenWindows) && plugins.wants(Need::Clipboard));
        assert!(plugins.windows_due(Instant::now()));
        plugins.windows(&[OpenWindow {
            app: "org.example.mail".into(),
            title: "Re: ferry times".into(),
            focused: true,
        }]);
        let line = |shown: &[Shown]| {
            let seeing = shown.iter().find(|shown| shown.id == "seeing")?;
            seeing.card.as_ref()?.lines.first().cloned()
        };
        let shown = settle(&plugins, |shown| {
            line(shown).as_deref() == Some("Re: ferry times")
        });
        assert_eq!(
            line(&shown).as_deref(),
            Some("Re: ferry times"),
            "{shown:?}"
        );
        // A second look so soon waits.
        assert!(!plugins.windows_due(Instant::now()));
        plugins.copied("the 9:40 ferry");
        let shown = settle(&plugins, |shown| {
            line(shown).as_deref() == Some("the 9:40 ferry")
        });
        assert_eq!(line(&shown).as_deref(), Some("the 9:40 ferry"), "{shown:?}");
        let blind = plugins
            .shown()
            .into_iter()
            .find(|shown| shown.id == "blind");
        assert!(
            blind.unwrap().card.is_none(),
            "one that doesn't need it hears nothing"
        );
    }

    /// The promise in the guide: a button that opens a web address, copies or controls what's
    /// playing is done by Slipstream, and only for a plugin that needs it; for any other it
    /// isn't even on the card.
    #[test]
    fn a_button_opens_copies_or_controls_media_only_for_a_plugin_that_needs_it() {
        let root = scratch("brokered");
        let card = r#"printf '{"buttons":[{"id":"site","label":"Site","open":"https://example.org"},{"id":"copy","label":"Copy","copy":"ferry times"},{"id":"media:next","label":"Next"}]}'"#;
        let able = install_needing(
            &root,
            "able",
            card,
            Some(60),
            &["open", "clipboard", "media"],
        );
        let unable = install_needing(&root, "unable", card, Some(60), &[]);
        let mut settings = turned_on(&able);
        settings
            .plugins
            .set_on(&unable.id, Some(unable.fingerprint.clone()));
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &settings);
        let shown = settle(&plugins, |shown| {
            shown.len() == 2 && shown.iter().all(|shown| shown.card.is_some())
        });
        let buttons = |id: &str| {
            let shown = shown.iter().find(|shown| shown.id == id).unwrap();
            shown.card.as_ref().unwrap().buttons.len()
        };
        assert_eq!((buttons("able"), buttons("unable")), (3, 0), "{shown:?}");
        assert_eq!(
            plugins.press("able", "site"),
            Some(Brokered::Open("https://example.org".into()))
        );
        assert_eq!(
            plugins.press("able", "copy"),
            Some(Brokered::Copy("ferry times".into()))
        );
        assert_eq!(
            plugins.press("able", "media:next"),
            Some(Brokered::Media(crate::media::Transport::Next))
        );
        for button in ["site", "copy", "media:next"] {
            assert_eq!(plugins.press("unable", button), None, "{button}");
        }
    }

    #[test]
    fn a_program_that_fails_says_why() {
        let root = scratch("fails");
        let plugin = install(&root, "ferry", "echo gone >&2; exit 3", Some(60));
        let (mut plugins, scans) = looking(&root);
        follow(&mut plugins, &scans, &turned_on(&plugin));
        let shown = settle(&plugins, |shown| shown[0].problem.is_some());
        assert_eq!(
            shown[0].problem.as_deref(),
            Some("Stopped with error 3: gone")
        );
        assert!(shown[0].card.is_none());
    }
}

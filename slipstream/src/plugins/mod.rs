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

pub mod card;
mod scan;
mod worker;

pub use scan::Scan;
pub use slipstream_config::plugins::Glance;
pub use worker::Sent;

use std::sync::Arc;

use slipstream_config::{
    Settings,
    plugins::{self, Card, Need},
};

use worker::{Approved, Sink, Worker};

/// One plugin that's on, as it runs.
struct Running {
    approved: Approved,
    hidden: Vec<String>,
    worker: Worker,
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
        }
    }

    /// Follows the settings. A plugin turned off stops at once, with everything its program
    /// started; one turned on starts once its folder has been looked at.
    pub fn configure(&mut self, settings: &Settings) {
        self.wanted = settings.plugins.clone();
        let wanted = &self.wanted;
        // Whatever is dropped here is stopped.
        self.running
            .retain(|running| wanted.approved(&running.approved.plugin));
        for running in &mut self.running {
            // Which sections show is only the panel's business: the program carries on.
            running.hidden = wanted.hidden(&running.approved.plugin.id).to_vec();
        }
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
            if !self.wanted.approved(&approved.plugin) {
                continue;
            }
            let hidden = self.wanted.hidden(&approved.plugin.id).to_vec();
            let kept = old
                .iter()
                .position(|running| running.approved == approved)
                .map(|at| old.remove(at));
            let running = kept.unwrap_or_else(|| {
                tracing::debug!("starting plugin {}", approved.plugin.id);
                Running {
                    worker: Worker::start(approved.clone(), self.sink.clone()),
                    approved,
                    hidden: Vec::new(),
                }
            });
            self.running.push(Running { hidden, ..running });
        }
        // Whatever is left in `old` is dropped here, which stops it.
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
    /// show what changed.
    pub fn press(&self, id: &str, button: &str) {
        if !plugins::valid_id(button) {
            return;
        }
        if let Some(running) = self
            .running
            .iter()
            .find(|running| running.approved.plugin.id == id)
        {
            running.worker.press(button);
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
        self.running.clear();
        slipstream_config::command::end_all();
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
            dirs: vec![root.join("plugins")],
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
             grep -c : /proc/net/dev",
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
        assert_eq!(said, "hello\nwrote-state\n1\n", "{err}");
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

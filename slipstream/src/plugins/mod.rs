//! Plugins: little programs of the person's own, each adding a card to the panel that drops from
//! the chip beside the clock (`slipstream_config::plugins` says what a plugin and a card are).
//! Each plugin's program runs out of process, driven from a thread of its own (`worker.rs`);
//! nothing of a plugin's is ever loaded into the compositor. What a program prints is only ever
//! shown, and a button on its card only runs that same program again, with the button's id.

pub mod card;
mod worker;

pub use slipstream_config::plugins::Glance;

use slipstream_config::{
    Settings,
    card::RUN_LIMIT,
    command::Run,
    plugins::{self, Card, Plugin},
};

use worker::Worker;

/// One plugin that's on, as it runs.
struct Running {
    plugin: Plugin,
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
    /// It has changed since it was turned on, so it doesn't run until it is turned on again.
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
#[derive(Default)]
pub struct Plugins {
    installed: usize,
    running: Vec<Running>,
    /// Plugins that are on but have changed since, and wait to be turned on again.
    held: Vec<Plugin>,
    glance: Option<String>,
    /// What was last logged as wrong, so a settings change elsewhere doesn't log it again.
    reported: Vec<String>,
}

impl Plugins {
    /// Looks for plugins again and follows the settings. A plugin that's on and unchanged keeps
    /// running; a new or changed one starts; one turned off or gone is stopped, with everything
    /// its program started.
    pub fn configure(&mut self, settings: &Settings) {
        self.configure_from(settings, &plugins::dirs());
    }

    /// `configure`, looking for plugins in `dirs`.
    fn configure_from(&mut self, settings: &Settings, dirs: &[std::path::PathBuf]) {
        let (found, problems) = plugins::find_in(dirs);
        if problems != self.reported {
            for why in &problems {
                tracing::warn!("plugins: {why}");
            }
            self.reported = problems;
        }
        self.installed = found.len();
        // In the order they were turned on.
        let mut wanted: Vec<Plugin> = Vec::new();
        self.held.clear();
        for id in &settings.plugins.on {
            if let Some(plugin) = found.iter().find(|plugin| plugin.id == *id) {
                if settings.plugins.approved(plugin) {
                    // So a later update can be compared with it, where it was turned on before
                    // what a plugin asks for was kept.
                    plugins::approval::remember_if_missing(plugin, &plugin.fingerprint);
                    wanted.push(plugin.clone());
                } else {
                    tracing::warn!("plugin {id} has changed since it was turned on, so it waits");
                    self.held.push(plugin.clone());
                }
            }
        }
        let mut old = std::mem::take(&mut self.running);
        for plugin in wanted {
            // Which sections show is only the panel's business: the program carries on.
            let hidden = settings.plugins.hidden(&plugin.id).to_vec();
            let kept = old
                .iter()
                .position(|running| running.plugin == plugin)
                .map(|at| old.remove(at));
            let mut running = kept.unwrap_or_else(|| {
                tracing::debug!("starting plugin {}", plugin.id);
                Running {
                    worker: Worker::start(plugin.clone()),
                    plugin,
                    hidden: Vec::new(),
                }
            });
            running.hidden = hidden;
            self.running.push(running);
        }
        // Whatever is left in `old` is dropped here, which stops it.
        self.glance = settings.plugins.glance().map(str::to_string);
    }

    /// The cards of the plugins that are on, in order.
    pub fn shown(&self) -> Vec<Shown> {
        self.running
            .iter()
            .map(|running| {
                let published = running.worker.published();
                Shown {
                    id: running.plugin.id.clone(),
                    name: running.plugin.manifest.name.clone(),
                    card: published.card,
                    hidden: running.hidden.clone(),
                    problem: published.problem,
                    held: false,
                }
            })
            .chain(self.held.iter().map(|plugin| Shown {
                id: plugin.id.clone(),
                name: plugin.manifest.name.clone(),
                card: None,
                hidden: Vec::new(),
                problem: None,
                held: true,
            }))
            .collect()
    }

    /// What the chip shows, from `shown`. None when there are no plugins at all.
    pub fn chip(&self, shown: &[Shown]) -> Option<Chip> {
        (self.installed > 0).then(|| Chip {
            installed: self.installed,
            on: self.running.len() + self.held.len(),
            glance: self.glance.as_ref().and_then(|id| {
                let glance = shown
                    .iter()
                    .find(|shown| shown.id == *id)?
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
    /// after it, off the event loop, and then runs as usual again to show what changed.
    pub fn press(&self, id: &str, button: &str) {
        let Some(running) = self.running.iter().find(|running| running.plugin.id == id) else {
            return;
        };
        if !plugins::valid_id(button) {
            return;
        }
        let command = worker::program(&running.plugin, &["press", button]);
        let plugin = id.to_string();
        let again = running.worker.again_handle();
        std::thread::spawn(move || {
            let ran = slipstream_config::command::run(command, RUN_LIMIT);
            if !matches!(ran, Run::Printed(_)) {
                tracing::warn!("plugin {plugin}: a press: {}", worker::why(&ran));
            }
            again();
        });
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
        path::PathBuf,
        time::{Duration, Instant},
    };

    use super::*;

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

    /// A scratch folder of plugins holding `id`, whose manifest runs `sh -c script`, with the
    /// settings that turn it on.
    fn turned_on(name: &str, id: &str, script: &str, every: Option<u64>) -> (PathBuf, Settings) {
        let root =
            std::env::temp_dir().join(format!("slipstream-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(id)).unwrap();
        let mut manifest = toml::Table::new();
        manifest.insert("name".into(), id.into());
        manifest.insert(
            "exec".into(),
            toml::Value::Array(vec!["sh".into(), "-c".into(), script.into(), id.into()]),
        );
        if let Some(every) = every {
            manifest.insert("every-secs".into(), (every as i64).into());
        }
        std::fs::write(
            root.join(id).join(plugins::MANIFEST),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        let plugin = plugins::read(&root.join(id)).unwrap();
        let mut settings = Settings::default();
        settings.plugins.set_on(id, Some(plugin.fingerprint));
        (root, settings)
    }

    #[test]
    fn a_plugin_runs_and_its_glance_is_on_the_chip() {
        let (root, mut settings) = turned_on(
            "glance",
            "allowance",
            r#"printf '{"sections":[{"name":"Disk","meters":[{"label":"Home","percent":81}]},{"name":"Mail","meters":[]}]}'"#,
            Some(60),
        );
        settings.plugins.glance = Some("allowance".into());
        let mut plugins = Plugins::default();
        plugins.configure_from(&settings, std::slice::from_ref(&root));
        let shown = settle(&plugins, |shown| shown[0].card.is_some());
        let card = shown[0].card.as_ref().unwrap();
        assert_eq!(card.sections.len(), 2);
        let chip = plugins.chip(&shown).unwrap();
        assert_eq!(chip.glance.unwrap().text, "81%");
        assert!(chip.attention);
        assert_eq!((chip.installed, chip.on), (1, 1));
        plugins.configure_from(&Settings::default(), &[root]);
        assert!(plugins.shown().is_empty());
    }

    /// The promise in the guide: a button only runs its plugin's own program again, with the
    /// button's id as a word of its own, which the shell never reads.
    #[test]
    fn a_press_runs_the_plugin_s_own_program_with_the_button_s_id() {
        let dir = std::env::temp_dir().join(format!("slipstream-press-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let said = dir.join("said");
        let (root, settings) = turned_on(
            "press",
            "kettle",
            &format!(
                r#"[ "$1" = press ] && printf '%s' "$2" > '{}'; printf '{{}}'"#,
                said.display()
            ),
            Some(60),
        );
        let mut plugins = Plugins::default();
        plugins.configure_from(&settings, &[root]);
        settle(&plugins, |shown| shown[0].card.is_some());
        plugins.press("kettle", "$(touch nope)");
        plugins.press("kettle", "go");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !said.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "go");
        assert!(!std::path::Path::new("nope").exists());
    }

    /// A `[meter]` moved into a plugin runs its command as it was written: a line ending in a
    /// keyword or a comment still works, since nothing is added to it.
    #[test]
    fn a_moved_meter_runs_its_command_as_written() {
        let root =
            std::env::temp_dir().join(format!("slipstream-moved-meter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
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
        let out = worker::program(&meter, &[]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), r#"{"lines":["ran"]}"#);
        let out = worker::program(&meter, &["press", "go"]).output().unwrap();
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
        let dir = std::env::temp_dir().join(format!("slipstream-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(plugins::MANIFEST),
            "name = \"Env\"\nexec = [\"sh\", \"-c\", \"env; pwd\"]\n",
        )
        .unwrap();
        let plugin = plugins::read(&dir).unwrap();
        // The test runs with all of cargo's environment, and the session's: none of it gets
        // through but what the list names, and what `sh` sets for itself.
        let out = worker::program(&plugin, &[]).output().unwrap();
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
        assert_eq!(folder, dir.display().to_string(), "run in its folder");
    }

    #[test]
    fn a_program_that_fails_says_why() {
        let (root, settings) = turned_on("fails", "ferry", "echo gone >&2; exit 3", Some(60));
        let mut plugins = Plugins::default();
        plugins.configure_from(&settings, &[root]);
        let shown = settle(&plugins, |shown| shown[0].problem.is_some());
        assert_eq!(shown[0].problem.as_deref(), Some("exit Some(3): gone"));
        assert!(shown[0].card.is_none());
    }
}

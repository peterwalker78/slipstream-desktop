//! The bar's modules (`[[bar.modules]]`), items of the person's own beside the clock in the format
//! of Waybar's custom modules. Each module's command runs out of process, driven from a thread of
//! its own (`worker.rs`); nothing of a module's is ever loaded into the compositor. What a command
//! prints is only ever shown, as plain text: a click or a scroll runs the command the settings
//! give for it, and a module's output has no way to name one.

mod worker;

use slipstream_config::bar::{Bar, Event, Interval, Module, Shown};

use worker::Worker;

/// One module as it runs: its settings, and its worker.
struct Running {
    /// Its own, for as long as it runs unchanged: the bar keeps its place and its width by it.
    id: u64,
    module: Module,
    worker: Worker,
}

/// Every module the settings name, running.
#[derive(Default)]
pub struct Modules {
    running: Vec<Running>,
    next_id: u64,
    /// What was last logged as ignored, so a settings change elsewhere doesn't log it again.
    reported: Vec<String>,
}

impl Modules {
    /// Follows the settings. A module whose settings are unchanged keeps running, and keeps its
    /// id; a new or changed one starts; one that has gone is stopped, with everything its
    /// command started.
    pub fn configure(&mut self, bar: &Bar) {
        let ignored = bar.ignored();
        if ignored != self.reported {
            for why in &ignored {
                tracing::warn!("settings: {why}");
            }
            self.reported = ignored;
        }
        let mut old = std::mem::take(&mut self.running);
        for module in bar.running() {
            let kept = old
                .iter()
                .position(|running| running.module == *module)
                .map(|at| old.remove(at));
            let running = kept.unwrap_or_else(|| {
                self.next_id += 1;
                tracing::debug!("starting bar module {}", module.label());
                Running {
                    id: self.next_id,
                    module: module.clone(),
                    worker: Worker::start(module.clone()),
                }
            });
            self.running.push(running);
        }
        // Whatever is left in `old` is dropped here, which stops it.
    }

    /// What the modules show, left to right, by id. One with nothing to show is left out.
    pub fn shown(&self) -> Vec<(u64, Shown)> {
        self.running
            .iter()
            .filter_map(|running| Some((running.id, running.worker.shown()?)))
            .collect()
    }

    /// Someone clicked or scrolled on module `id`: runs the command its settings give for that,
    /// if any, and then the module's own command again, so it shows what changed.
    pub fn event(&self, id: u64, event: Event) {
        let Some(running) = self.running.iter().find(|running| running.id == id) else {
            return;
        };
        let Some(command) = running.module.on(event) else {
            return;
        };
        crate::launch::shell(command);
        if running.module.exec_on_event && running.module.interval != Interval::Continuous {
            running.worker.again();
        }
    }

    /// Stops every module, now: the compositor is on its way out.
    pub fn stop_all(&mut self) {
        self.running.clear();
        slipstream_config::command::end_all();
    }
}

/// The event a pointer button is, on a module.
pub fn button_event(button: u32) -> Option<Event> {
    match button {
        crate::input::BTN_LEFT => Some(Event::Click),
        0x111 => Some(Event::RightClick),
        crate::input::BTN_MIDDLE => Some(Event::MiddleClick),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use slipstream_config::bar::{Output, ReturnType};

    use super::*;

    fn bar(text: &str) -> Bar {
        let settings: slipstream_config::Settings = toml::from_str(text).unwrap();
        settings.bar
    }

    fn texts(modules: &Modules) -> Vec<String> {
        modules
            .shown()
            .into_iter()
            .map(|(_, shown)| shown.text)
            .collect()
    }

    fn settle(modules: &Modules, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while modules.shown().len() < count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn unchanged_modules_keep_running_through_a_settings_change() {
        let mut modules = Modules::default();
        let first = "[[bar.modules]]\nexec = \"echo a\"\ninterval = 60\n";
        let second = "[[bar.modules]]\nexec = \"echo b\"\ninterval = 60\n";
        modules.configure(&bar(first));
        settle(&modules, 1);
        let id = modules.shown()[0].0;
        modules.configure(&bar(&format!("{second}{first}")));
        settle(&modules, 2);
        assert_eq!(texts(&modules), ["b", "a"]);
        assert_eq!(
            modules.shown()[1].0,
            id,
            "the same module, not started again"
        );
        modules.configure(&bar(second));
        assert_eq!(texts(&modules), ["b"]);
        modules.configure(&Bar::default());
        assert!(modules.shown().is_empty());
    }

    #[test]
    fn a_button_names_its_event() {
        assert_eq!(button_event(0x110), Some(Event::Click));
        assert_eq!(button_event(0x111), Some(Event::RightClick));
        assert_eq!(button_event(0x112), Some(Event::MiddleClick));
        assert_eq!(button_event(0x113), None);
    }

    /// The promise in the guide: what a module's command prints is only ever shown, and never
    /// causes a command to run. Output that looks like settings, shell or markup is shown as text,
    /// and the clicks and scrolls run only what the settings give.
    #[test]
    fn a_module_s_output_never_causes_a_command_to_run() {
        let dir = std::env::temp_dir().join(format!("slipstream-promise-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let flag = dir.join("ran");
        let touch = format!("touch '{}'", flag.display());
        let printed = format!(
            concat!(
                r#"{{"text": "$({touch}) `{touch}` <span onclick=\"{touch}\">x</span>", "#,
                r#""tooltip": "; {touch}", "alt": "{touch}", "class": "{touch}", "#,
                r#""on-click": "{touch}", "on-click-right": "{touch}", "#,
                r#""on-scroll-up": "{touch}", "exec": "{touch}"}}"#,
            ),
            touch = touch,
        );
        let script = dir.join("say.json");
        std::fs::write(&script, format!("{printed}\n")).unwrap();
        let mut modules = Modules::default();
        modules.configure(&bar(&format!(
            "[[bar.modules]]\nexec = \"cat '{}'\"\nreturn-type = \"json\"\ninterval = \"once\"\n\
             format = \"{{}} {{alt}} {{icon}}\"\n",
            script.display()
        )));
        settle(&modules, 1);
        let (id, shown) = modules.shown().remove(0);
        assert!(
            shown.text.starts_with("$(touch"),
            "shown as text: {}",
            shown.text
        );
        for event in [
            Event::Click,
            Event::RightClick,
            Event::MiddleClick,
            Event::ScrollUp,
            Event::ScrollDown,
        ] {
            assert_eq!(modules.running[0].module.on(event), None);
            modules.event(id, event);
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(!flag.exists(), "nothing the module printed was run");

        // The same output read every way it can be, which is all a module's output ever goes
        // through, has nowhere to put a command.
        let output = Output::json(&printed).unwrap();
        let module = Module {
            exec: "true".into(),
            return_type: ReturnType::Json,
            ..Module::default()
        };
        let shown = slipstream_config::bar::shown(&module, &output).unwrap();
        assert!(!shown.text.is_empty());
        assert_eq!(module.on(Event::Click), None);
    }
}

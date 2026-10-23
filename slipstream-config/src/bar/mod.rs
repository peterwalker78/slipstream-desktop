//! The bar's modules: items of the person's own to the right of the clock, each fed by a command
//! of theirs, in the format of Waybar's custom modules so that a Waybar script works unchanged.
//!
//! ```toml
//! [[bar.modules]]
//! name = "weather"
//! exec = "weather-report --short"
//! interval = 600
//! on-click = "xdg-open https://example.org/forecast"
//! ```
//!
//! [`Module`] lists the keys that are read. Any other key, a Waybar key with no meaning here or a
//! slip of the keyboard, is kept as it was written and reported by [`Module::ignored`], as is a key
//! whose value is the wrong kind of thing; the module does without it. Neither stops the rest of
//! the settings from loading.
//!
//! A module's command prints what it shows (`output`), which is formatted and stripped down to
//! plain text (`format`). What it prints is only ever shown: the commands a click or the wheel
//! runs come from here, from the settings, and nowhere else.

mod format;
mod output;

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml::{Table, Value};

pub use format::{DEFAULT_MAX_LENGTH, Shown, TOOLTIP_LINES, format, shown};
pub use output::{Output, Tint, plain};

/// The most modules the bar runs. Each is a command running beside the compositor.
pub const MAX_MODULES: usize = 12;

/// The bar's own settings, `[bar]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bar {
    /// `[[bar.modules]]`, left to right.
    pub modules: Vec<Module>,
    /// What was wrong with the table itself, such as a module that isn't a table.
    pub problems: Vec<String>,
}

impl Bar {
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// The modules that run: the first `MAX_MODULES`.
    pub fn running(&self) -> &[Module] {
        &self.modules[..self.modules.len().min(MAX_MODULES)]
    }

    /// Everything about the table and its modules that is ignored, and why, to be logged.
    pub fn ignored(&self) -> Vec<String> {
        let mut ignored = self.problems.clone();
        if self.modules.len() > MAX_MODULES {
            ignored.push(format!(
                "only the first {MAX_MODULES} bar modules run; the other {} don't",
                self.modules.len() - MAX_MODULES
            ));
        }
        for module in self.running() {
            ignored.extend(
                module
                    .ignored()
                    .into_iter()
                    .map(|why| format!("bar module {}: {why}", module.label())),
            );
        }
        ignored
    }

    fn from_value(value: Value) -> Bar {
        let mut bar = Bar::default();
        let Value::Table(mut table) = value else {
            bar.problems
                .push("[bar] isn't a table, so it's ignored".into());
            return bar;
        };
        match table.remove("modules") {
            None => {}
            Some(Value::Array(modules)) => {
                for (index, module) in modules.into_iter().enumerate() {
                    match module {
                        Value::Table(table) => bar.modules.push(Module::from_table(table)),
                        _ => bar.problems.push(format!(
                            "bar module {} isn't a table, so it's ignored",
                            index + 1
                        )),
                    }
                }
            }
            Some(_) => bar
                .problems
                .push("bar.modules isn't a list of tables, so it's ignored".into()),
        }
        bar
    }
}

impl<'de> Deserialize<'de> for Bar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Bar::from_value(Value::deserialize(deserializer)?))
    }
}

impl Serialize for Bar {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut table = Table::new();
        if !self.modules.is_empty() {
            let modules = self
                .modules
                .iter()
                .map(|module| Value::Table(module.to_table()))
                .collect();
            table.insert("modules".into(), Value::Array(modules));
        }
        table.serialize(serializer)
    }
}

/// How the command's output reads (`return-type`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReturnType {
    /// The text on its first line, the tooltip on its second and the class on its third.
    #[default]
    Plain,
    /// A JSON object a line: `text`, `alt`, `tooltip`, `class` and `percentage`.
    Json,
}

/// When the command runs (`interval`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Interval {
    /// It keeps running, and each line it prints is an update. No `interval`, or 0.
    #[default]
    Continuous,
    /// Once, and again only after a click or scroll (`"once"`).
    Once,
    /// Every so many seconds.
    Every(u64),
}

/// The longest `interval`: a day.
const LONGEST_INTERVAL: u64 = 86_400;

/// The icons `{icon}` picks from (`format-icons`).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Icons {
    #[default]
    None,
    One(String),
    /// Picked by `percentage`, the lowest first.
    Ramp(Vec<String>),
    /// Picked by `alt`, with `default` for any other; each is one icon or a ramp.
    ByAlt(BTreeMap<String, Icons>),
}

/// What a person does to a module that can run one of its commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Click,
    RightClick,
    MiddleClick,
    ScrollUp,
    ScrollDown,
}

/// One module, `[[bar.modules]]`, with Waybar's names for its keys.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// What Settings and the log call it. Not one of Waybar's keys: Waybar knows a module by its
    /// place in its own config.
    pub name: String,
    /// `exec`: the command that says what to show, run with `sh -c`. With none, the module shows
    /// its `format` as it is, as a button for its click commands.
    pub exec: String,
    /// `exec-if`: run first, and the module shows only while it succeeds.
    pub exec_if: String,
    pub return_type: ReturnType,
    pub interval: Interval,
    /// `restart-interval`: seconds to wait before a continuous command that exited is started
    /// again. It waits longer each time a command keeps exiting straight away.
    pub restart_interval: Option<f64>,
    /// `exec-on-event`: runs `exec` again after a click or scroll command, so the module shows
    /// what changed. On unless set off.
    pub exec_on_event: bool,
    /// `format`: `{}` (or `{text}`), `{alt}`, `{icon}` and `{percentage}` stand for what the
    /// command said. `{}` when not set.
    pub format: Option<String>,
    pub format_icons: Icons,
    /// `max-length`: the most characters shown before an ellipsis.
    pub max_length: Option<usize>,
    /// `tooltip`: whether the pointer resting on it shows its tooltip. On unless set off.
    pub tooltip: bool,
    pub on_click: String,
    pub on_click_right: String,
    pub on_click_middle: String,
    pub on_scroll_up: String,
    pub on_scroll_down: String,
    /// The keys that aren't read here, as they were written, so a save keeps them.
    pub other: Table,
    /// The keys whose values couldn't be used, and why. Their values are in `other`.
    pub problems: Vec<String>,
}

impl Default for Module {
    fn default() -> Self {
        Self {
            name: String::new(),
            exec: String::new(),
            exec_if: String::new(),
            return_type: ReturnType::Plain,
            interval: Interval::Continuous,
            restart_interval: None,
            exec_on_event: true,
            format: None,
            format_icons: Icons::None,
            max_length: None,
            tooltip: true,
            on_click: String::new(),
            on_click_right: String::new(),
            on_click_middle: String::new(),
            on_scroll_up: String::new(),
            on_scroll_down: String::new(),
            other: Table::new(),
            problems: Vec::new(),
        }
    }
}

impl Module {
    /// Its name, or its command when it has none, for the log.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if !self.exec.trim().is_empty() {
            format!("`{}`", self.exec.trim())
        } else {
            "with no command".into()
        }
    }

    /// The command `event` runs, if the settings give it one. Nothing a module's command prints
    /// can add or change one.
    pub fn on(&self, event: Event) -> Option<&str> {
        let command = match event {
            Event::Click => &self.on_click,
            Event::RightClick => &self.on_click_right,
            Event::MiddleClick => &self.on_click_middle,
            Event::ScrollUp => &self.on_scroll_up,
            Event::ScrollDown => &self.on_scroll_down,
        };
        Some(command.trim()).filter(|command| !command.is_empty())
    }

    /// `restart-interval`, or a second.
    pub fn restart_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.restart_interval.unwrap_or(1.0).clamp(0.1, 3600.0))
    }

    /// What in its table is ignored, and why.
    pub fn ignored(&self) -> Vec<String> {
        let mut ignored = self.problems.clone();
        let unknown: Vec<&str> = self
            .other
            .keys()
            .map(String::as_str)
            .filter(|key| {
                !self
                    .problems
                    .iter()
                    .any(|why| why.starts_with(&format!("`{key}`")))
            })
            .collect();
        if !unknown.is_empty() {
            ignored.push(format!(
                "{} {} not used here, so {} ignored",
                unknown
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if unknown.len() == 1 { "is" } else { "are" },
                if unknown.len() == 1 {
                    "it's"
                } else {
                    "they're"
                },
            ));
        }
        ignored
    }

    /// A module from its table, taking what it can use and keeping the rest.
    pub fn from_table(table: Table) -> Module {
        let mut module = Module::default();
        for (key, value) in table {
            if let Err(why) = module.take(&key, &value) {
                module
                    .problems
                    .push(format!("`{key}` {why}, so it's ignored"));
                module.other.insert(key, value);
            }
        }
        module
    }

    /// Takes one key, or says what's wrong with its value. A key it doesn't read goes to
    /// `other`.
    fn take(&mut self, key: &str, value: &Value) -> Result<(), &'static str> {
        let text = || value.as_str().map(str::to_string).ok_or("should be text");
        let flag = || value.as_bool().ok_or("should be true or false");
        match key {
            "name" => self.name = text()?,
            "exec" => self.exec = text()?,
            "exec-if" => self.exec_if = text()?,
            "return-type" => {
                self.return_type = match value.as_str() {
                    Some("json") => ReturnType::Json,
                    // Waybar reads anything else as plain lines.
                    Some(_) => ReturnType::Plain,
                    None => return Err("should be \"json\" or left out"),
                }
            }
            "interval" => self.interval = interval(value)?,
            "restart-interval" => {
                self.restart_interval = Some(seconds(value).ok_or("should be a number of seconds")?)
            }
            "exec-on-event" => self.exec_on_event = flag()?,
            "format" => self.format = Some(text()?),
            "format-icons" => self.format_icons = icons(value, true)?,
            "max-length" => {
                self.max_length = Some(
                    value
                        .as_integer()
                        .filter(|length| *length > 0)
                        .ok_or("should be a whole number above 0")? as usize,
                )
            }
            "tooltip" => self.tooltip = flag()?,
            "on-click" => self.on_click = text()?,
            "on-click-right" => self.on_click_right = text()?,
            "on-click-middle" => self.on_click_middle = text()?,
            "on-scroll-up" => self.on_scroll_up = text()?,
            "on-scroll-down" => self.on_scroll_down = text()?,
            _ => {
                self.other.insert(key.to_string(), value.clone());
            }
        }
        Ok(())
    }

    /// Its table, with only the keys that differ from their defaults, and the ones it keeps
    /// unread.
    pub fn to_table(&self) -> Table {
        let mut table = Table::new();
        let mut text = |key: &str, value: &str| {
            if !value.is_empty() {
                table.insert(key.into(), Value::String(value.into()));
            }
        };
        text("name", &self.name);
        text("exec", &self.exec);
        text("exec-if", &self.exec_if);
        if self.return_type == ReturnType::Json {
            table.insert("return-type".into(), "json".into());
        }
        match self.interval {
            Interval::Continuous => {}
            Interval::Once => {
                table.insert("interval".into(), "once".into());
            }
            Interval::Every(secs) => {
                table.insert("interval".into(), Value::Integer(secs as i64));
            }
        }
        if let Some(secs) = self.restart_interval {
            table.insert("restart-interval".into(), number(secs));
        }
        if !self.exec_on_event {
            table.insert("exec-on-event".into(), false.into());
        }
        if let Some(format) = &self.format {
            table.insert("format".into(), format.as_str().into());
        }
        if self.format_icons != Icons::None {
            table.insert("format-icons".into(), icons_value(&self.format_icons));
        }
        if let Some(length) = self.max_length {
            table.insert("max-length".into(), Value::Integer(length as i64));
        }
        if !self.tooltip {
            table.insert("tooltip".into(), false.into());
        }
        let mut text = |key: &str, value: &str| {
            if !value.is_empty() {
                table.insert(key.into(), Value::String(value.into()));
            }
        };
        text("on-click", &self.on_click);
        text("on-click-right", &self.on_click_right);
        text("on-click-middle", &self.on_click_middle);
        text("on-scroll-up", &self.on_scroll_up);
        text("on-scroll-down", &self.on_scroll_down);
        for (key, value) in &self.other {
            table.entry(key.clone()).or_insert_with(|| value.clone());
        }
        table
    }
}

/// A number of seconds, whole or not, from zero up.
fn seconds(value: &Value) -> Option<f64> {
    let secs = match value {
        Value::Integer(secs) => *secs as f64,
        Value::Float(secs) => *secs,
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

fn number(secs: f64) -> Value {
    if secs.fract() == 0.0 && secs < i64::MAX as f64 {
        Value::Integer(secs as i64)
    } else {
        Value::Float(secs)
    }
}

/// `interval`: seconds, rounded up to whole ones and at most a day; 0 for a command that keeps
/// running; or `"once"`.
fn interval(value: &Value) -> Result<Interval, &'static str> {
    if value.as_str() == Some("once") {
        return Ok(Interval::Once);
    }
    let secs = seconds(value).ok_or("should be a number of seconds or \"once\"")?;
    Ok(if secs == 0.0 {
        Interval::Continuous
    } else {
        Interval::Every((secs.ceil() as u64).min(LONGEST_INTERVAL))
    })
}

/// `format-icons`: one icon, a ramp of them, or (at the top level only) a table of either by
/// `alt`.
fn icons(value: &Value, top: bool) -> Result<Icons, &'static str> {
    const WRONG: &str = "should be an icon, a list of icons, or a table of them";
    match value {
        Value::String(icon) => Ok(Icons::One(icon.clone())),
        Value::Array(ramp) => ramp
            .iter()
            .map(|icon| icon.as_str().map(str::to_string).ok_or(WRONG))
            .collect::<Result<_, _>>()
            .map(Icons::Ramp),
        Value::Table(by_alt) if top => by_alt
            .iter()
            .map(|(alt, icon)| Ok((alt.clone(), icons(icon, false)?)))
            .collect::<Result<_, _>>()
            .map(Icons::ByAlt),
        _ => Err(WRONG),
    }
}

fn icons_value(icons: &Icons) -> Value {
    match icons {
        Icons::None => Value::Array(Vec::new()),
        Icons::One(icon) => icon.as_str().into(),
        Icons::Ramp(ramp) => Value::Array(ramp.iter().map(|icon| icon.as_str().into()).collect()),
        Icons::ByAlt(by_alt) => Value::Table(
            by_alt
                .iter()
                .map(|(alt, icon)| (alt.clone(), icons_value(icon)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Settings, file::parse};

    fn bar(text: &str) -> Bar {
        parse(text).unwrap().bar
    }

    #[test]
    fn a_module_reads_with_waybar_s_names_for_its_keys() {
        let bar = bar(r#"
            [[bar.modules]]
            name = "Mail"
            exec = "mail-count --json"
            exec-if = "command -v mail-count"
            return-type = "json"
            interval = 30
            format = "{icon} {}"
            format-icons = { unread = "M", default = ["a", "b"] }
            max-length = 12
            tooltip = false
            exec-on-event = false
            on-click = "mail-app"
            on-click-right = "mail-count --mark-read"
            on-click-middle = "true"
            on-scroll-up = "up"
            on-scroll-down = "down"

            [[bar.modules]]
            exec = "date +%S"
            interval = "once"

            [[bar.modules]]
            exec = "watch-something"
            restart-interval = 2.5
        "#);
        let mail = &bar.modules[0];
        assert_eq!(mail.name, "Mail");
        assert_eq!(mail.exec_if, "command -v mail-count");
        assert_eq!(mail.return_type, ReturnType::Json);
        assert_eq!(mail.interval, Interval::Every(30));
        assert_eq!(mail.format.as_deref(), Some("{icon} {}"));
        let Icons::ByAlt(by_alt) = &mail.format_icons else {
            panic!("{:?}", mail.format_icons)
        };
        assert_eq!(by_alt["unread"], Icons::One("M".into()));
        assert_eq!(by_alt["default"], Icons::Ramp(vec!["a".into(), "b".into()]));
        assert_eq!(mail.max_length, Some(12));
        assert!(!mail.tooltip && !mail.exec_on_event);
        assert_eq!(mail.on(Event::Click), Some("mail-app"));
        assert_eq!(mail.on(Event::RightClick), Some("mail-count --mark-read"));
        assert_eq!(mail.on(Event::ScrollDown), Some("down"));
        assert!(mail.ignored().is_empty(), "{:?}", mail.ignored());

        assert_eq!(bar.modules[1].interval, Interval::Once);
        assert_eq!(bar.modules[1].return_type, ReturnType::Plain);
        assert!(bar.modules[1].tooltip && bar.modules[1].exec_on_event);
        assert_eq!(bar.modules[1].on(Event::Click), None);
        assert_eq!(bar.modules[2].interval, Interval::Continuous);
        assert_eq!(bar.modules[2].restart_after().as_millis(), 2500);
        assert_eq!(Module::default().restart_after().as_secs(), 1);
    }

    #[test]
    fn intervals_are_whole_seconds_up_to_a_day_and_zero_keeps_running() {
        let read = |value: Value| interval(&value);
        assert_eq!(read(0.into()), Ok(Interval::Continuous));
        assert_eq!(read(0.2.into()), Ok(Interval::Every(1)));
        assert_eq!(read(5.into()), Ok(Interval::Every(5)));
        assert_eq!(read(10_000_000.into()), Ok(Interval::Every(86_400)));
        assert!(read((-1).into()).is_err());
        assert!(read("soon".into()).is_err());
    }

    #[test]
    fn a_waybar_key_with_no_meaning_here_is_kept_and_reported_but_breaks_nothing() {
        let settings = parse(
            r#"
            [motion]
            reduced = true

            [[bar.modules]]
            exec = "spotify-status"
            signal = 8
            escape = true
            interval = "sometimes"
            max-length = -3
            "#,
        )
        .unwrap();
        assert!(settings.motion.reduced, "the other settings still load");
        let module = &settings.bar.modules[0];
        assert_eq!(module.exec, "spotify-status");
        assert_eq!(module.interval, Interval::Continuous, "the default stands");
        let ignored = module.ignored();
        assert_eq!(ignored.len(), 3, "{ignored:?}");
        assert!(ignored[0].starts_with("`interval` should be"));
        assert!(ignored[1].starts_with("`max-length` should be"));
        assert_eq!(
            ignored[2],
            "`escape`, `signal` are not used here, so they're ignored"
        );
        // And all of it is written back as it was.
        let table = module.to_table();
        assert_eq!(table["signal"], Value::Integer(8));
        assert_eq!(table["interval"], Value::String("sometimes".into()));
        assert_eq!(table["max-length"], Value::Integer(-3));
    }

    #[test]
    fn a_bar_table_of_the_wrong_shape_is_reported_and_the_rest_still_loads() {
        for text in [
            "bar = 3\n[motion]\nreduced = true\n",
            "[bar]\nmodules = 4\n[motion]\nreduced = true\n",
            "[bar]\nmodules = [1, { exec = \"date\" }]\n[motion]\nreduced = true\n",
        ] {
            let settings = parse(text).unwrap();
            assert!(settings.motion.reduced, "{text}");
            assert_eq!(settings.bar.problems.len(), 1, "{text}");
        }
        assert_eq!(
            parse("[bar]\nmodules = [1, { exec = \"date\" }]\n")
                .unwrap()
                .bar
                .modules[0]
                .exec,
            "date"
        );
    }

    #[test]
    fn only_so_many_modules_run() {
        let text = "[[bar.modules]]\nexec = \"date\"\n".repeat(MAX_MODULES + 2);
        let bar = bar(&text);
        assert_eq!(bar.running().len(), MAX_MODULES);
        assert_eq!(bar.ignored().len(), 1);
    }

    #[test]
    fn modules_are_written_back_with_only_what_was_set() {
        let text = r#"
            [[bar.modules]]
            exec = "date +%H"
            interval = "once"
            restart-interval = 0.5
            format-icons = ["x", "y"]
            hide-empty-text = true
        "#;
        let settings = parse(text).unwrap();
        let written = toml::to_string(&settings).unwrap();
        assert!(written.contains("[[bar.modules]]"), "{written}");
        assert!(
            !written.contains("tooltip"),
            "defaults aren't written: {written}"
        );
        assert_eq!(parse(&written).unwrap().bar, settings.bar);
        // No modules, no table.
        let none = toml::to_string(&Settings::default()).unwrap();
        assert!(!none.contains("bar"), "{none}");
    }
}

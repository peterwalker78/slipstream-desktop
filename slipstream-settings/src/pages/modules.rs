//! The bar's modules: items of the person's own beside the clock, each fed by a command, in the
//! format of Waybar's custom modules. This page sets the keys most modules need; any other Waybar
//! key set in the file is kept as it is.

use std::process::Command;

use slipstream_config::{
    bar::{self, Interval, MAX_MODULES, Module, Output, ReturnType, Shown, Tint},
    command::{self, Lines, Run, Said},
    meter::RUN_LIMIT,
};

use super::*;

/// How often a module's command can be set to run from here, in seconds.
const EVERY_CHOICES: [u64; 8] = [1, 5, 10, 30, 60, 300, 600, 3600];

/// A command or name being typed is saved once typing pauses for this long, or straight away on
/// Enter: each save starts the module again.
const TYPING_PAUSE: Duration = Duration::from_millis(1500);

/// How long trying a module that keeps running waits for its first line.
const FIRST_LINE: Duration = Duration::from_secs(5);

/// The keys this page sets. Any other is listed as kept.
const ON_THIS_PAGE: [&str; 5] = ["name", "exec", "interval", "return-type", "on-click"];

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Bar modules",
        "Your own items on the bar, right of the clock, each showing what a command prints. \
         Scripts written for Waybar's custom modules work as they are.",
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.append(&list);
    fill_modules(&list, store);
    let safety = group(&page, "What a module can do");
    row(
        &safety,
        "Show text, and nothing else",
        Some(
            "Each command runs on its own, outside the desktop. What it prints is only ever shown, \
             never run, and a click runs only the command set here.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
}

/// A group for each module, then a row to add one, built again whenever the list changes.
fn fill_modules(list: &gtk::Box, store: &Store) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let modules = store.get().bar.modules;
    let count = modules.len();
    let refill = refiller(list, store, fill_modules);
    for (index, module) in modules.iter().enumerate() {
        let title = if module.name.trim().is_empty() {
            format!("Module {}", index + 1)
        } else {
            module.name.trim().to_string()
        };
        let card = group(list, &title);
        module_rows(&card, store, index, module, refill.clone());
    }
    let card = group(list, "Add one");
    let add = gtk::Button::with_label("Add a module");
    add.set_sensitive(count < MAX_MODULES);
    let add_store = store.clone();
    add.connect_clicked(move |_| {
        add_store.change(|settings| {
            if settings.bar.modules.len() < MAX_MODULES {
                settings.bar.modules.push(Module {
                    interval: Interval::Every(60),
                    ..Module::default()
                });
            }
        });
        refill();
    });
    let plural = if count == 1 { "module" } else { "modules" };
    row(
        &card,
        &format!("{count} {plural}"),
        Some(&format!(
            "A new one goes at the end of the bar, up to {MAX_MODULES}."
        )),
        &add,
    );
}

/// The rows for module `index`.
fn module_rows(
    card: &gtk::Box,
    store: &Store,
    index: usize,
    module: &Module,
    refill: impl Fn() + Clone + 'static,
) {
    let name = entry(&module.name, &format!("Module {}", index + 1));
    saved_after_typing(&name, store, index, |module, text| module.name = text);
    row(card, "Name", Some("What this page calls it."), &name);

    let exec = entry(&module.exec, "weather-report --short");
    saved_after_typing(&exec, store, index, |module, text| module.exec = text);
    row(
        card,
        "Command",
        Some(
            "Run with sh -c. It prints the text to show, or a line of Waybar's JSON with a \
             tooltip and a class.",
        ),
        &exec,
    );

    let runs = runs_choices(module.interval);
    let names: Vec<String> = runs.iter().map(|interval| runs_name(*interval)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let runs_menu = gtk::DropDown::from_strings(&names);
    runs_menu.set_selected(
        runs.iter()
            .position(|interval| *interval == module.interval)
            .unwrap_or(0) as u32,
    );
    let runs_store = store.clone();
    runs_menu.connect_selected_notify(move |menu| {
        if let Some(&interval) = runs.get(menu.selected() as usize) {
            change(&runs_store, index, |module| module.interval = interval);
        }
    });
    row(
        card,
        "Runs",
        Some("A command that keeps running prints a line whenever it has something new."),
        &runs_menu,
    );

    let output = gtk::DropDown::from_strings(&["Lines of text", "JSON"]);
    output.set_selected(u32::from(module.return_type == ReturnType::Json));
    let output_store = store.clone();
    output.connect_selected_notify(move |menu| {
        let json = menu.selected() == 1;
        change(&output_store, index, |module| {
            module.return_type = if json {
                ReturnType::Json
            } else {
                ReturnType::Plain
            }
        });
    });
    row(
        card,
        "Output",
        Some("Lines of text: the text, then its tooltip, then its class."),
        &output,
    );

    let click = entry(&module.on_click, "xdg-open https://example.org");
    saved_after_typing(&click, store, index, |module, text| module.on_click = text);
    row(
        card,
        "On a click",
        Some("Run with sh -c when the module is clicked."),
        &click,
    );

    let kept = kept_keys(module);
    if !kept.is_empty() {
        row(
            card,
            "Also set in the file",
            Some(&format!("{}: kept as they are.", kept.join(", "))),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
    }

    let run = gtk::Button::with_label("Run it now");
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.set_tooltip_text(Some("Remove"));
    remove.add_css_class("flat");
    let controls = gtk::Box::builder().spacing(4).build();
    controls.append(&run);
    controls.append(&remove);
    let result = row(
        card,
        "Try it",
        Some("See what the bar would show, or why it would show nothing."),
        &controls,
    )
    .expect("the row has a line of explanation");
    let remove_store = store.clone();
    remove.connect_clicked(move |_| {
        remove_store.change(|settings| {
            if index < settings.bar.modules.len() {
                settings.bar.modules.remove(index);
            }
        });
        refill();
    });
    let try_store = store.clone();
    run.connect_clicked(move |button| {
        let Some(module) = try_store.get().bar.modules.get(index).cloned() else {
            return;
        };
        button.set_sensitive(false);
        result.set_label("Running…");
        let (button, result) = (button.clone(), result.clone());
        gtk::glib::spawn_future_local(async move {
            let tried = gtk::gio::spawn_blocking(move || tried(&module)).await;
            button.set_sensitive(true);
            result.set_label(&tried.unwrap_or_else(|_| "It couldn't be run.".into()));
        });
    });
}

fn entry(text: &str, placeholder: &str) -> gtk::Entry {
    gtk::Entry::builder()
        .text(text)
        .placeholder_text(placeholder)
        .width_chars(26)
        .build()
}

/// Changes module `index` in the settings, if it is still there.
fn change(store: &Store, index: usize, edit: impl FnOnce(&mut Module)) {
    store.change(|settings| {
        if let Some(module) = settings.bar.modules.get_mut(index) {
            edit(module);
        }
    });
}

/// Saves what is typed into `entry` with `apply` once typing pauses, or on Enter.
fn saved_after_typing(
    entry: &gtk::Entry,
    store: &Store,
    index: usize,
    apply: fn(&mut Module, String),
) {
    let save = {
        let store = store.clone();
        move |entry: &gtk::Entry| {
            let text = entry.text().trim().to_string();
            change(&store, index, |module| apply(module, text));
        }
    };
    entry.connect_activate(save.clone());
    let pending: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::default();
    entry.connect_changed(move |entry| {
        if let Some(waiting) = pending.borrow_mut().take() {
            waiting.remove();
        }
        let entry = entry.clone();
        let fired = pending.clone();
        let save = save.clone();
        let waiting = gtk::glib::timeout_add_local_once(TYPING_PAUSE, move || {
            fired.borrow_mut().take();
            save(&entry);
        });
        *pending.borrow_mut() = Some(waiting);
    });
}

/// The choices for how often a module runs, with `current` among them when it's a hand edit.
fn runs_choices(current: Interval) -> Vec<Interval> {
    let mut choices = vec![Interval::Continuous, Interval::Once];
    let current_secs = match current {
        Interval::Every(secs) => secs,
        _ => EVERY_CHOICES[0],
    };
    choices.extend(
        super::choices(&EVERY_CHOICES, current_secs)
            .into_iter()
            .map(Interval::Every),
    );
    choices
}

fn runs_name(interval: Interval) -> String {
    match interval {
        Interval::Continuous => "All the time".into(),
        Interval::Once => "Once, and after a click".into(),
        Interval::Every(secs) => format!("Every {}", super::describe(secs)),
    }
}

/// The keys set in the file that this page doesn't show.
fn kept_keys(module: &Module) -> Vec<String> {
    module
        .to_table()
        .keys()
        .filter(|key| !ON_THIS_PAGE.contains(&key.as_str()))
        .cloned()
        .collect()
}

fn sh(line: &str) -> Command {
    let mut sh = Command::new("sh");
    sh.args(["-c", line]);
    sh
}

/// Runs `module` the way the bar would, once, and says what the bar would show.
fn tried(module: &Module) -> String {
    if module.exec.trim().is_empty() {
        return match bar::shown(module, &Output::default()) {
            Some(shown) => said(&shown),
            None => "There's no command to run.".into(),
        };
    }
    if !module.exec_if.trim().is_empty()
        && !matches!(
            command::run(sh(&module.exec_if), RUN_LIMIT),
            Run::Printed(_)
        )
    {
        return "Its exec-if check fails, so the bar hides it.".into();
    }
    let json = module.return_type == ReturnType::Json;
    let output = if module.interval == Interval::Continuous {
        match Lines::start(sh(&module.exec), |_| {}) {
            Err(err) => return format!("It couldn't be started: {err}"),
            Ok(lines) => match lines.next(FIRST_LINE) {
                Said::Line(line) if json => Output::json(&line),
                Said::Line(line) => Ok(Output::plain(&line)),
                Said::Nothing => {
                    return format!(
                        "It printed nothing in its first {} seconds.",
                        FIRST_LINE.as_secs()
                    );
                }
                Said::Ended => return "It exited without printing anything.".into(),
            },
        }
    } else {
        match command::run(sh(&module.exec), RUN_LIMIT) {
            Run::Printed(printed) => Output::of_run(&printed, json),
            Run::Failed { code, error } => {
                let how = match code {
                    Some(code) => format!("It failed with exit code {code}"),
                    None => "It was stopped by a signal".to_string(),
                };
                return if error.is_empty() {
                    format!("{how}.")
                } else {
                    format!("{how}: {error}")
                };
            }
            Run::TimedOut => {
                return format!(
                    "It was still running after {} seconds, so it was stopped.",
                    RUN_LIMIT.as_secs()
                );
            }
            Run::CouldntStart(err) => return format!("It couldn't be started: {err}"),
        }
    };
    match output {
        Err(err) => format!("It ran, but what it printed isn't Waybar's JSON: {err}"),
        Ok(output) => match bar::shown(module, &output) {
            Some(shown) => said(&shown),
            None => "It ran, but printed no text, so the bar hides it.".into(),
        },
    }
}

/// What the bar shows, in a sentence.
fn said(shown: &Shown) -> String {
    let tint = match shown.tint {
        Tint::Plain => "",
        Tint::Warning => ", in amber",
        Tint::Critical => ", in orange",
    };
    let tooltip = shown
        .tooltip
        .as_deref()
        .and_then(|tooltip| tooltip.lines().next())
        .filter(|line| *line != shown.text)
        .map(|line| format!(" Its tooltip starts “{line}”."))
        .unwrap_or_default();
    format!("The bar shows “{}”{tint}.{tooltip}", shown.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(exec: &str) -> Module {
        Module {
            exec: exec.into(),
            interval: Interval::Once,
            ..Module::default()
        }
    }

    #[test]
    fn trying_a_module_says_what_the_bar_would_show() {
        assert_eq!(tried(&module("echo 21°C")), "The bar shows “21°C”.");
        let json = Module {
            return_type: ReturnType::Json,
            ..module(r#"echo '{"text": "3 new", "tooltip": "From Ada", "class": "warning"}'"#)
        };
        assert_eq!(
            tried(&json),
            "The bar shows “3 new”, in amber. Its tooltip starts “From Ada”."
        );
        assert_eq!(
            tried(&module("true")),
            "It ran, but printed no text, so the bar hides it."
        );
        assert_eq!(
            tried(&module("echo nope >&2; exit 4")),
            "It failed with exit code 4: nope"
        );
        let continuous = Module {
            interval: Interval::Continuous,
            ..module("echo first; sleep 30")
        };
        assert_eq!(tried(&continuous), "The bar shows “first”.");
        let checked = Module {
            exec_if: "false".into(),
            ..module("echo x")
        };
        assert_eq!(
            tried(&checked),
            "Its exec-if check fails, so the bar hides it."
        );
        assert_eq!(tried(&Module::default()), "There's no command to run.");
    }

    #[test]
    fn a_hand_edited_interval_joins_the_choices() {
        let usual = runs_choices(Interval::Continuous);
        assert_eq!(usual[..2], [Interval::Continuous, Interval::Once]);
        assert_eq!(usual.len(), 2 + EVERY_CHOICES.len());
        assert!(runs_choices(Interval::Every(45)).contains(&Interval::Every(45)));
        assert_eq!(runs_name(Interval::Every(300)), "Every 5 minutes");
    }

    #[test]
    fn keys_set_only_in_the_file_are_listed_as_kept() {
        let module = Module {
            format: Some("{icon}".into()),
            max_length: Some(9),
            on_click: "made-up-app".into(),
            ..module("x")
        };
        assert_eq!(kept_keys(&module), ["format", "max-length"]);
    }
}

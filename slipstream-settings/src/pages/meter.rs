//! The bar's meter: its command, how often it runs, and trying it out.

use slipstream_config::{
    command::Run,
    meter::{self as reports, RUN_LIMIT, Reading, Report, Section},
};

use super::*;

/// How often the meter's command can be set to run, in seconds.
const METER_EVERY_CHOICES: [u64; 6] = [10, 30, 60, 120, 300, 600];

/// A command being typed is saved once typing pauses for this long, or straight away on Enter.
const COMMAND_PAUSE: Duration = Duration::from_millis(1500);

/// The bar's meter: the command that feeds it, how often it runs, and a way to try it out.
pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Meter",
        "A gauge on the bar, beside quick settings, for anything a command can report: a quota, \
         a plan's limits, a disk.",
    );
    let feed = group(&page, "Command");
    let settings = store.get().meter;

    let command = gtk::Entry::builder()
        .text(&settings.command)
        .placeholder_text("quota-report --json")
        .width_chars(30)
        .build();
    row(
        &feed,
        "Command",
        Some(
            "Run with sh -c. It prints a JSON report of how much is used, in sections of meters, \
             as Slipstream's README describes. Leave it empty for no meter.",
        ),
        &command,
    );
    let save = {
        let store = store.clone();
        move |entry: &gtk::Entry| {
            let text = entry.text().trim().to_string();
            store.change(move |settings| settings.meter.command = text);
        }
    };
    command.connect_activate(save.clone());
    // Saved once typing pauses, so Slipstream doesn't run every half-typed command.
    let pending: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::default();
    command.connect_changed(move |entry| {
        if let Some(waiting) = pending.borrow_mut().take() {
            waiting.remove();
        }
        let entry = entry.clone();
        let fired = pending.clone();
        let save = save.clone();
        let waiting = gtk::glib::timeout_add_local_once(COMMAND_PAUSE, move || {
            fired.borrow_mut().take();
            save(&entry);
        });
        *pending.borrow_mut() = Some(waiting);
    });

    let current = settings.every().as_secs();
    let everies = choices(&METER_EVERY_CHOICES, current);
    let names: Vec<String> = everies.iter().map(|&secs| describe(secs)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let every = gtk::DropDown::from_strings(&names);
    every.set_selected(
        everies
            .iter()
            .position(|&secs| secs == current)
            .unwrap_or(0) as u32,
    );
    let every_store = store.clone();
    every.connect_selected_notify(move |every| {
        if let Some(&secs) = everies.get(every.selected() as usize) {
            every_store.change(|settings| settings.meter.every_secs = secs);
        }
    });
    row(
        &feed,
        "Run every",
        Some(
            "Some sources limit how often they can be asked, so ask no more often than they allow.",
        ),
        &every,
    );

    let trial = group(&page, "Try it");
    let run = gtk::Button::with_label("Run it now");
    let result = row(
        &trial,
        "Run the command once",
        Some("See what the bar would show, or why it would show nothing."),
        &run,
    )
    .expect("the row has a line of explanation");
    let shown: Rc<RefCell<Vec<gtk::Box>>> = Rc::default();
    run.connect_clicked(move |button| {
        for old in shown.borrow_mut().drain(..) {
            trial.remove(&old);
        }
        let text = command.text().trim().to_string();
        if text.is_empty() {
            result.set_label("There's no command to run.");
            return;
        }
        button.set_sensitive(false);
        result.set_label("Running…");
        let (button, result, trial, shown) =
            (button.clone(), result.clone(), trial.clone(), shown.clone());
        gtk::glib::spawn_future_local(async move {
            let ran = gtk::gio::spawn_blocking(move || {
                let mut sh = std::process::Command::new("sh");
                sh.args(["-c", &text]);
                slipstream_config::command::run(sh, RUN_LIMIT)
            })
            .await;
            button.set_sensitive(true);
            let (headline, reading) = match ran {
                Ok(ran) => tried(&ran, reports::unix_now()),
                Err(_) => ("The command couldn't be run.".to_string(), None),
            };
            result.set_label(&headline);
            for section in reading.iter().flat_map(|reading| &reading.sections) {
                for preview in preview_rows(section) {
                    trial.append(&preview);
                    shown.borrow_mut().push(preview);
                }
            }
        });
    });
    page.upcast()
}

/// What trying the meter's command found: a line saying how it went, and the report it read.
fn tried(ran: &Run, now: i64) -> (String, Option<Reading>) {
    match ran {
        Run::Printed(text) if text.trim().is_empty() => {
            ("It ran, but printed nothing.".to_string(), None)
        }
        Run::Printed(text) => match Report::parse(text) {
            Ok(report) => {
                let reading = report.reading(false, now);
                let headline = if reading
                    .sections
                    .iter()
                    .all(|section| section.gauges.is_empty())
                {
                    "It works, but its report has no meters yet, so the bar shows nothing."
                } else {
                    "It works. The bar and quick settings show:"
                };
                (headline.to_string(), Some(reading))
            }
            Err(err) => (
                format!("It ran, but what it printed isn't a report: {err}"),
                None,
            ),
        },
        Run::Failed { code, error } => {
            let how = match code {
                Some(code) => format!("It failed with exit code {code}"),
                None => "It was stopped by a signal".to_string(),
            };
            if error.is_empty() {
                (format!("{how}."), None)
            } else {
                (format!("{how}: {error}"), None)
            }
        }
        Run::TimedOut => (
            format!(
                "It was still running after {} seconds, so it was stopped. The bar shows nothing \
                 from a command that slow.",
                RUN_LIMIT.as_secs()
            ),
            None,
        ),
        Run::CouldntStart(err) => (format!("It couldn't be started: {err}"), None),
    }
}

/// A section of a tried report as rows: its name, then each meter with a bar coloured as the
/// bar on the desktop colours it.
fn preview_rows(section: &Section) -> Vec<gtk::Box> {
    let title = match &section.detail {
        Some(detail) => format!("{} · {detail}", section.name),
        None => section.name.clone(),
    };
    let sub = match (&section.note, section.stale) {
        (Some(note), _) => Some(note.clone()),
        (None, true) => Some("Marked as not up to date, so it's dimmed".to_string()),
        (None, false) => None,
    };
    let (heading, _) = row_box(
        &title,
        sub.as_deref(),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    let mut rows = vec![heading];
    for gauge in &section.gauges {
        let bar = gtk::ProgressBar::builder()
            .fraction(gauge.percent as f64 / 100.0)
            .width_request(160)
            .valign(gtk::Align::Center)
            .css_classes(["gauge"])
            .build();
        match gauge.percent {
            90.. => bar.add_css_class("hot"),
            75.. => bar.add_css_class("amber"),
            _ => {}
        }
        let percent = label(&format!("{}%", gauge.percent), "kv");
        percent.set_wrap(false);
        percent.set_width_chars(4);
        percent.set_xalign(1.0);
        let control = gtk::Box::builder().spacing(10).build();
        control.append(&bar);
        control.append(&percent);
        let resets = gauge
            .resets_in
            .as_ref()
            .map(|left| format!("Starts over in {left}"));
        let (row, _) = row_box(&gauge.label, resets.as_deref(), &control);
        row.add_css_class("gauge-row");
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trying_the_meter_says_what_happened() {
        let (headline, reading) = tried(
            &Run::Printed(r#"{"sections": [{"name": "Storage", "meters": [{"label": "Daily", "percent": 42, "resets": 3600}]}]}"#.into()),
            0,
        );
        assert_eq!(headline, "It works. The bar and quick settings show:");
        let reading = reading.unwrap();
        assert_eq!(
            reading.sections[0].gauges[0].resets_in.as_deref(),
            Some("1 h")
        );
        let (headline, reading) = tried(&Run::Printed(r#"{"sections": []}"#.into()), 0);
        assert!(headline.contains("no meters") && reading.is_some());
        assert_eq!(
            tried(&Run::Printed(" \n".into()), 0).0,
            "It ran, but printed nothing."
        );
        assert!(
            tried(&Run::Printed("oops".into()), 0)
                .0
                .starts_with("It ran, but what it printed isn't a report: ")
        );
        assert_eq!(
            tried(
                &Run::Failed {
                    code: Some(1),
                    error: "no login".into()
                },
                0
            )
            .0,
            "It failed with exit code 1: no login"
        );
        assert_eq!(
            tried(
                &Run::Failed {
                    code: None,
                    error: String::new()
                },
                0
            )
            .0,
            "It was stopped by a signal."
        );
        assert!(tried(&Run::TimedOut, 0).0.contains("30 seconds"));
    }
}

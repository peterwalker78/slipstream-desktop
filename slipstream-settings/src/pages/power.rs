//! Power & lock: the battery and the charger, locking, and the screen turning off.

use super::*;
use crate::power as supplies;

/// How often the Power page reads the battery again.
const POWER_EVERY: Duration = Duration::from_secs(2);

/// The battery's charge and what it's doing, then the details: power flowing, the charger, and
/// how the battery has worn. Read again every couple of seconds while the window is open.
pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Power & lock",
        "Battery and charging, locking, and when the screen goes off",
    );

    let card = gtk::Box::builder()
        .spacing(18)
        .css_classes(["card", "batt"])
        .build();
    let percent = label("", "batt-percent");
    percent.set_wrap(false);
    let bar = gtk::ProgressBar::builder()
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let summary = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    let state = label("", "batt-state");
    let time = label("", "row-sub");
    summary.append(&state);
    summary.append(&time);
    card.append(&percent);
    card.append(&bar);
    card.append(&summary);
    page.append(&card);

    let details = group(&page, "Details");
    let value = || {
        let value = label("", "kv");
        value.set_xalign(1.0);
        value
    };
    let add = |title: &str, sub: Option<&str>| {
        let value = value();
        let (row, sub) = row_box(title, sub, &value);
        details.append(&row);
        (row, value, sub)
    };
    let (_, flow, flow_sub) = add("Power", Some(""));
    let (_, charger, _) = add("Charger", None);
    let (limit_row, limit, _) = add(
        "Charge limit",
        Some("Set in the firmware, to spare the battery"),
    );
    let (health_row, health, health_sub) = add("Health", Some(""));
    let (cycles_row, cycles, _) = add("Charge cycles", None);

    let mut estimate = slipstream_config::battery::Estimate::default();
    let opened = std::time::Instant::now();
    let mut update = move || {
        let reading = supplies::read(&mut estimate, opened.elapsed().as_secs_f64());
        let Some(battery) = &reading.battery else {
            percent.set_label("—");
            bar.set_fraction(0.0);
            bar.remove_css_class("charging");
            state.set_label(if reading.plugged {
                "No battery"
            } else {
                "No battery found"
            });
            time.set_label("Running on mains power");
            flow.set_label("—");
            if let Some(sub) = &flow_sub {
                sub.set_label("");
            }
            charger.set_label(&charger_text(&reading));
            for row in [&limit_row, &health_row, &cycles_row] {
                row.set_visible(false);
            }
            return;
        };
        percent.set_label(&format!("{}%", battery.percent));
        bar.set_fraction(battery.percent as f64 / 100.0);
        if battery.charging() {
            bar.add_css_class("charging");
        } else {
            bar.remove_css_class("charging");
        }
        state.set_label(match battery.status.as_str() {
            "Charging" => "Charging",
            "Full" => "Fully charged",
            "Not charging" if battery.limit.is_some_and(|limit| battery.percent >= limit) => {
                "Held at its limit"
            }
            "Not charging" => "Plugged in, not charging",
            _ if reading.plugged => "Plugged in",
            _ => "On battery",
        });
        let duration = slipstream_config::battery::duration;
        time.set_label(&match (battery.hours, battery.status.as_str()) {
            (Some(hours), "Charging") => format!("{} until full", duration(hours)),
            (Some(hours), _) => format!("{} left", duration(hours)),
            // Just after a charger is plugged in or pulled out, while the rate settles.
            (None, "Charging" | "Discharging") => "Working out the time".to_string(),
            (None, _) => String::new(),
        });
        time.set_visible(!time.label().is_empty());

        flow.set_label(
            &battery
                .watts
                .map_or("—".to_string(), |watts| format!("{watts:.1} W")),
        );
        if let Some(sub) = &flow_sub {
            sub.set_label(match (battery.watts, battery.status.as_str()) {
                (None, _) => "Not reported by this battery",
                (_, "Charging") => "Going into the battery",
                (_, "Discharging") => "Being drawn from the battery",
                _ => "Through the battery",
            });
        }
        charger.set_label(&charger_text(&reading));
        limit_row.set_visible(battery.limit.is_some());
        limit.set_label(
            &battery
                .limit
                .map_or(String::new(), |limit| format!("{limit}%")),
        );
        health_row.set_visible(battery.health.is_some());
        health.set_label(
            &battery
                .health
                .map_or(String::new(), |health| format!("{health}%")),
        );
        if let Some(sub) = &health_sub {
            sub.set_label(&match battery.capacity_wh {
                Some((now, new)) => {
                    format!("A full charge holds {now:.1} Wh; it held {new:.1} Wh new")
                }
                None => "What a full charge holds now, against when it was new".to_string(),
            });
        }
        cycles_row.set_visible(battery.cycles.is_some());
        cycles.set_label(
            &battery
                .cycles
                .map_or(String::new(), |cycles| cycles.to_string()),
        );
    };
    update();
    // Every page is built when the window opens, so the battery is only read while this one
    // shows, straight away when it comes into view. Reading stops once the page is gone.
    let update = Rc::new(RefCell::new(update));
    page.connect_map({
        let update = update.clone();
        move |_| (update.borrow_mut())()
    });
    let weak = page.downgrade();
    gtk::glib::timeout_add_local(POWER_EVERY, move || {
        let Some(page) = weak.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if page.is_mapped() {
            (update.borrow_mut())();
        }
        gtk::glib::ControlFlow::Continue
    });
    lock(&page, store);
    page.upcast()
}

/// Locking after a while and before sleep, and the screen turning off.
fn lock(page: &gtk::Box, store: &Store) {
    let lock = group(page, "Lock");
    let current = store.get().lock.after_idle_mins;
    let waits = lock_choices(current);
    let names: Vec<String> = waits.iter().map(|&mins| describe(mins * 60)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let after = gtk::DropDown::from_strings(&names);
    after.set_selected(waits.iter().position(|&mins| mins == current).unwrap_or(0) as u32);
    let after_store = store.clone();
    after.connect_selected_notify(move |after| {
        if let Some(&mins) = waits.get(after.selected() as usize) {
            after_store.change(|settings| settings.lock.after_idle_mins = mins);
        }
    });
    row(
        &lock,
        "Lock after",
        Some(
            "Time with no keyboard or mouse input. A fullscreen window, a playing video or \
             Awake (hold Caps Lock) holds it off.",
        ),
        &after,
    );
    let current = store.get().display.screen_off_mins;
    let waits = lock_choices(current);
    let names: Vec<String> = waits.iter().map(|&mins| describe(mins * 60)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let off_after = gtk::DropDown::from_strings(&names);
    off_after.set_selected(waits.iter().position(|&mins| mins == current).unwrap_or(0) as u32);
    let off_store = store.clone();
    off_after.connect_selected_notify(move |off_after| {
        if let Some(&mins) = waits.get(off_after.selected() as usize) {
            off_store.change(|settings| settings.display.screen_off_mins = mins);
        }
    });
    row(
        &lock,
        "Turn the screen off after",
        Some("Held off by the same things as the lock."),
        &off_after,
    );
    let before_sleep = gtk::Switch::new();
    before_sleep.set_active(store.get().lock.before_sleep);
    let sleep_store = store.clone();
    before_sleep.connect_active_notify(move |switch| {
        let on = switch.is_active();
        sleep_store.change(move |settings| settings.lock.before_sleep = on);
    });
    row(
        &lock,
        "Lock before sleep",
        Some("The laptop wakes up locked."),
        &before_sleep,
    );
}

/// The charger row: its rating when the kernel knows it.
fn charger_text(reading: &supplies::Reading) -> String {
    match (reading.plugged, reading.charger_watts) {
        (true, Some(watts)) => format!("{watts:.0} W, plugged in"),
        (true, None) => "Plugged in".to_string(),
        (false, _) => "Not plugged in".to_string(),
    }
}

/// The waits offered for locking and for turning the screen off, in minutes: Never first, then
/// the usual waits, with a hand-edited one in its place among them.
fn lock_choices(current: u64) -> Vec<u64> {
    let mut choices = vec![0, 1, 5, 10, 15, 30, 60];
    if !choices.contains(&current) {
        choices.push(current);
        choices.sort_unstable();
    }
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_waits_start_at_never() {
        assert_eq!(lock_choices(0), [0, 1, 5, 10, 15, 30, 60]);
        assert_eq!(lock_choices(20), [0, 1, 5, 10, 15, 20, 30, 60]);
    }
}

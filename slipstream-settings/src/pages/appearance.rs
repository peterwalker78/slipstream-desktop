//! Appearance: the colour scheme, motion, the rings' colours and night light.

use super::*;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Appearance",
        "What apps are coloured, what Slipstream marks things with, and how much it moves",
    );
    let colours = group(&page, "Colours");
    let schemes = slipstream_config::ColourScheme::ALL;
    let names: Vec<&str> = schemes.iter().map(|scheme| scheme.label()).collect();
    let scheme = gtk::DropDown::from_strings(&names);
    let chosen = store.get().appearance.colour_scheme;
    scheme.set_selected(schemes.iter().position(|held| *held == chosen).unwrap_or(0) as u32);
    let scheme_store = store.clone();
    scheme.connect_selected_notify(move |scheme| {
        if let Some(chosen) = schemes.get(scheme.selected() as usize).copied() {
            scheme_store.change(move |settings| settings.appearance.colour_scheme = chosen);
        }
    });
    row(
        &colours,
        "Apps",
        Some(
            "Which one apps are asked for, and they change without restarting. Slipstream's own \
             bar, panels and windows are dark either way, and an app that never asks the desktop \
             keeps whatever colours it was set to.",
        ),
        &scheme,
    );
    let borders = group(&page, "Border colours");
    colour_row(
        &borders,
        store,
        "Focus ring",
        "The ring around the focused window, and the keyboard's ring in quick settings and the \
         notification centre.",
        |settings| settings.borders.selected_tile.clone(),
        |settings, hex| settings.borders.selected_tile = hex,
    );
    colour_row(
        &borders,
        store,
        "Bullet time highlight",
        "The rings and frames around what's chosen while the desktop is zoomed out.",
        |settings| settings.borders.bullet_time.clone(),
        |settings, hex| settings.borders.bullet_time = hex,
    );
    let motion = group(&page, "Motion");
    // The same name quick settings gives this switch.
    let reduced = gtk::Switch::new();
    reduced.set_active(store.get().motion.reduced);
    let reduced_store = store.clone();
    reduced.connect_active_notify(move |reduced| {
        let on = reduced.is_active();
        reduced_store.change(|settings| settings.motion.reduced = on);
    });
    row(
        &motion,
        "Reduced motion",
        Some("Windows jump into place and effects become short fades"),
        &reduced,
    );
    let styles = [
        (slipstream_config::Effects::Slipstream, "Slipstream"),
        (slipstream_config::Effects::Matrix, "Matrix rain"),
    ];
    let names: Vec<&str> = styles.iter().map(|(_, name)| *name).collect();
    let effects = gtk::DropDown::from_strings(&names);
    let current = store.get().motion.effects;
    effects.set_selected(
        styles
            .iter()
            .position(|(which, _)| *which == current)
            .unwrap_or(0) as u32,
    );
    let effects_store = store.clone();
    effects.connect_selected_notify(move |effects| {
        if let Some((which, _)) = styles.get(effects.selected() as usize) {
            let which = *which;
            effects_store.change(|settings| settings.motion.effects = which);
        }
    });
    row(
        &motion,
        "Effects",
        Some("What minimised apps turn into: falling code, or streaks of light"),
        &effects,
    );
    let rain = gtk::Switch::new();
    rain.set_active(store.get().motion.close_into_rain);
    let rain_store = store.clone();
    rain.connect_active_notify(move |rain| {
        let on = rain.is_active();
        rain_store.change(|settings| settings.motion.close_into_rain = on);
    });
    row(
        &motion,
        "Close into the rain",
        Some("A closed window falls away in the same effects instead of fading"),
        &rain,
    );

    night_light(&page, store);
    page.upcast()
}

/// Times offered for a night light schedule: every half hour.
fn half_hours() -> Vec<String> {
    (0..48)
        .map(|n| slipstream_config::sun::format_time(n as f64 * 30.0))
        .collect()
}

/// Tonight's sunset and tomorrow's sunrise where the time zone's city is, as the schedule's row
/// says them.
fn sunset_note() -> String {
    let now = gtk::glib::DateTime::now_local();
    let location = slipstream_config::sun::location();
    let times = now.ok().zip(location).and_then(|(now, (lat, lon))| {
        let date = (
            now.year() as i64,
            now.month() as u32,
            now.day_of_month() as u32,
        );
        let offset = now.utc_offset().as_minutes() as f64;
        slipstream_config::sun::sun_times(lat, lon, date, offset)
    });
    match times {
        Some((sunrise, sunset)) => format!(
            "Warms up over half an hour from sunset, about {} today, and cools again from \
             sunrise, about {}, going by your time zone. Turning it by hand in quick settings \
             lasts until the next change.",
            slipstream_config::sun::format_time(sunset),
            slipstream_config::sun::format_time(sunrise)
        ),
        None => "Your time zone doesn't say where the sun sets, so the hours below are used \
                 instead."
            .to_string(),
    }
}

fn night_light(page: &gtk::Box, store: &Store) {
    use slipstream_config::NightSchedule;
    let night = group(page, "Night light");
    let schedules = [
        (NightSchedule::Off, "Only by hand"),
        (NightSchedule::Sunset, "Sunset to sunrise"),
        (NightSchedule::Custom, "Set hours"),
    ];
    let names: Vec<&str> = schedules.iter().map(|(_, name)| *name).collect();
    let schedule = gtk::DropDown::from_strings(&names);
    let current = store.get().display.night_light_schedule;
    schedule.set_selected(
        schedules
            .iter()
            .position(|(which, _)| *which == current)
            .unwrap_or(0) as u32,
    );
    let sub = row(
        &night,
        "Schedule",
        Some("Warmer colours in the evening. Quick settings turns it on and off by hand."),
        &schedule,
    );

    let times = half_hours();
    let names: Vec<&str> = times.iter().map(String::as_str).collect();
    let pick = |value: &str| {
        let minutes = slipstream_config::sun::parse_time(value).unwrap_or(0.0);
        ((minutes / 30.0).round() as u32).min(47)
    };
    let from = gtk::DropDown::from_strings(&names);
    from.set_selected(pick(&store.get().display.night_light_from));
    let to = gtk::DropDown::from_strings(&names);
    to.set_selected(pick(&store.get().display.night_light_to));
    row(&night, "From", None, &from);
    row(
        &night,
        "Until",
        Some("The next morning, if earlier than the start."),
        &to,
    );

    let follow = {
        let (from, to, sub) = (from.clone(), to.clone(), sub.clone());
        move |which: NightSchedule| {
            // The hours count for a set schedule, and for sunset where the sun's times can't be
            // worked out.
            let custom = which == NightSchedule::Custom
                || (which == NightSchedule::Sunset && slipstream_config::sun::location().is_none());
            from.set_sensitive(custom);
            to.set_sensitive(custom);
            if let Some(sub) = &sub {
                sub.set_text(&match which {
                    NightSchedule::Off => "Warmer colours in the evening. Quick settings \
                                           turns it on and off by hand."
                        .to_string(),
                    NightSchedule::Sunset => sunset_note(),
                    NightSchedule::Custom => "Warms up over half an hour from the start, and \
                                              cools over half an hour from the end."
                        .to_string(),
                });
            }
        }
    };
    follow(current);
    let schedule_store = store.clone();
    schedule.connect_selected_notify(move |schedule| {
        if let Some((which, _)) = schedules.get(schedule.selected() as usize) {
            let which = *which;
            follow(which);
            schedule_store.change(move |settings| settings.display.night_light_schedule = which);
        }
    });
    for (dropdown, is_from) in [(from, true), (to, false)] {
        let times = times.clone();
        let store = store.clone();
        dropdown.connect_selected_notify(move |dropdown| {
            if let Some(time) = times.get(dropdown.selected() as usize).cloned() {
                store.change(move |settings| {
                    if is_from {
                        settings.display.night_light_from = time;
                    } else {
                        settings.display.night_light_to = time;
                    }
                });
            }
        });
    }
}

/// The colours offered for the rings, warm and cold, each one told apart from the code rain's
/// green at a glance.
const COLOURS: [(&str, &str); 12] = [
    ("Ice cyan", "#42d3ff"),
    ("Sky", "#7fe3ff"),
    ("Mint", "#3cf0c0"),
    ("Rain green", "#59ff8c"),
    ("Chartreuse", "#c6ff4f"),
    ("Gold", "#ffcf5c"),
    ("Amber", "#ffb547"),
    ("Tangerine", "#ff9b3d"),
    ("Coral", "#ff7a5c"),
    ("Rose", "#ff77b0"),
    ("Lilac", "#b794ff"),
    ("Silver", "#e8eef7"),
];

/// A row with a dot of the colour and the list to choose from.
fn colour_row(
    card: &gtk::Box,
    store: &Store,
    title: &str,
    sub: &str,
    read: fn(&Settings) -> String,
    write: fn(&mut Settings, String),
) {
    let current = read(&store.get());
    let choices = colour_choices(&current);
    let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
    let list = gtk::DropDown::from_strings(&names);
    let selected = choices
        .iter()
        .position(|(_, hex)| same_colour(hex, &current))
        .unwrap_or(0);
    list.set_selected(selected as u32);

    let shown = Rc::new(Cell::new(
        slipstream_config::rgb(&current).unwrap_or([1.0, 1.0, 1.0]),
    ));
    let dot = gtk::DrawingArea::new();
    dot.set_content_width(20);
    dot.set_content_height(20);
    dot.set_valign(gtk::Align::Center);
    let painted = Rc::clone(&shown);
    dot.set_draw_func(move |_, cairo, width, height| {
        let [r, g, b] = painted.get();
        let radius = (width.min(height) as f64) / 2.0 - 1.0;
        cairo.arc(
            width as f64 / 2.0,
            height as f64 / 2.0,
            radius,
            0.0,
            std::f64::consts::TAU,
        );
        cairo.set_source_rgb(r as f64, g as f64, b as f64);
        let _ = cairo.fill();
    });

    let store = store.clone();
    let dot_to_redraw = dot.clone();
    list.connect_selected_notify(move |list| {
        let Some((_, hex)) = choices.get(list.selected() as usize) else {
            return;
        };
        let hex = (*hex).to_string();
        if let Some(rgb) = slipstream_config::rgb(&hex) {
            shown.set(rgb);
            dot_to_redraw.queue_draw();
        }
        store.change(|settings| write(settings, hex.clone()));
    });

    let control = gtk::Box::builder().spacing(10).build();
    control.append(&dot);
    control.append(&list);
    row(card, title, Some(sub), &control);
}

/// The usual colours, plus the one in the file if it is something else, so a hand-edited colour
/// is shown rather than silently swapped for another.
fn colour_choices(current: &str) -> Vec<(&'static str, String)> {
    let mut choices: Vec<(&'static str, String)> = COLOURS
        .iter()
        .map(|(name, hex)| (*name, (*hex).to_string()))
        .collect();
    if slipstream_config::rgb(current).is_some()
        && !COLOURS.iter().any(|(_, hex)| same_colour(hex, current))
    {
        choices.push(("Custom", current.trim().to_lowercase()));
    }
    choices
}

/// Two colours are the same whatever the case, and with or without the hash.
fn same_colour(one: &str, other: &str) -> bool {
    match (slipstream_config::rgb(one), slipstream_config::rgb(other)) {
        (Some(one), Some(other)) => one == other,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_colours_the_list_offers() {
        for hex in [
            slipstream_config::SELECTED_TILE,
            slipstream_config::BULLET_TIME,
        ] {
            assert!(
                COLOURS.iter().any(|(_, listed)| same_colour(listed, hex)),
                "{hex} is not in the list"
            );
            assert_eq!(
                colour_choices(hex).len(),
                COLOURS.len(),
                "no Custom for {hex}"
            );
        }
    }

    #[test]
    fn a_hand_edited_colour_joins_the_list_as_custom() {
        let choices = colour_choices("#123456");
        assert_eq!(choices.len(), COLOURS.len() + 1);
        assert_eq!(
            choices.last(),
            Some(&("Custom", "#123456".to_string())),
            "the file's own colour is offered last"
        );
        // A colour that is only written differently is the one already in the list.
        assert_eq!(colour_choices("42D3FF").len(), COLOURS.len());
        // Something that isn't a colour leaves the list alone; the ring falls back to its default.
        assert_eq!(colour_choices("mint").len(), COLOURS.len());
    }
}

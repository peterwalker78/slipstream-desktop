//! Appearance: the colour scheme, Slipstream's three colours, motion and night light.

use super::*;
use crate::harmony;

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
    self::colours(&colours, store);
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

/// What the theme list says when the colours are none of its sets.
const YOUR_OWN: &str = "Your own";

/// The focus ring, bullet time's colour and the accent, each channel 0–1.
type Three = [[f32; 3]; 3];

/// The colour rows, kept in step with each other: whatever changes one shows the result on all.
struct Colours {
    store: Store,
    theme: gtk::DropDown,
    names: gtk::StringList,
    shown: Cell<Three>,
    wheel: gtk::Scale,
    focus: gtk::ColorDialogButton,
    bullet: gtk::ColorDialogButton,
    accent: gtk::ColorDialogButton,
    /// Set while the rows are being changed to match, so their own handlers stay quiet.
    syncing: Cell<bool>,
}

impl Colours {
    fn current_of(settings: &Settings) -> Three {
        let borders = &settings.borders;
        [
            borders.selected_tile_rgb(),
            borders.bullet_time_rgb(),
            borders.accent_rgb(),
        ]
    }

    /// Saves `three` and shows it on every row.
    fn choose(&self, three: Three) {
        let [focus, bullet, accent] = three.map(harmony::hex);
        self.store.change(|settings| {
            settings.borders.selected_tile = focus;
            settings.borders.bullet_time = bullet;
            settings.borders.accent = accent;
        });
        crate::restyle(three);
        self.show(three);
    }

    /// A focus ring of `focus`, with the two colours that go with it.
    fn choose_focus(&self, focus: [f32; 3]) {
        let (bullet, accent) = harmony::partners(focus);
        self.choose([focus, bullet, accent]);
    }

    fn show(&self, three: Three) {
        self.syncing.set(true);
        self.shown.set(three);
        let sets = harmony::THEMES.len() as u32;
        match matching_theme(three) {
            Some(index) => {
                self.theme.set_selected(index as u32);
                if self.names.n_items() > sets {
                    self.names.remove(sets);
                }
            }
            None => {
                if self.names.n_items() == sets {
                    self.names.append(YOUR_OWN);
                }
                self.theme.set_selected(sets);
            }
        }
        // The rainbow moves to the focus ring's hue, unless it's already within a step of it.
        let hue = harmony::hue(three[0]);
        if (self.wheel.value() - hue).abs() >= 1.0 {
            self.wheel.set_value(hue.round());
        }
        for (button, rgb) in [&self.focus, &self.bullet, &self.accent].iter().zip(three) {
            button.set_rgba(&rgba(rgb));
        }
        self.syncing.set(false);
    }
}

/// Which of the sets `three` is, if it's one of them.
fn matching_theme(three: Three) -> Option<usize> {
    harmony::THEMES.iter().position(|theme| {
        [theme.focus, theme.bullet, theme.accent]
            .map(slipstream_config::rgb)
            .iter()
            .zip(three)
            .all(|(held, rgb)| *held == Some(rgb))
    })
}

fn rgba([r, g, b]: [f32; 3]) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::new(r, g, b, 1.0)
}

fn colour_button(title: &str) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::builder()
        .title(title)
        .with_alpha(false)
        .modal(true)
        .build();
    gtk::ColorDialogButton::new(Some(dialog))
}

/// The theme list's entries: a set's three colours as dots, then its name.
fn theme_entries() -> gtk::SignalListItemFactory {
    const DOT: i32 = 14;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let dots = gtk::DrawingArea::new();
        dots.set_content_width(3 * DOT + 2 * 4);
        dots.set_content_height(DOT);
        dots.set_valign(gtk::Align::Center);
        let entry = gtk::Box::builder().spacing(10).build();
        entry.append(&dots);
        entry.append(&gtk::Label::new(None));
        item.set_child(Some(&entry));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let name = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .map(|name| name.string().to_string())
            .unwrap_or_default();
        let Some(entry) = item.child() else {
            return;
        };
        let dots = entry.first_child().and_downcast::<gtk::DrawingArea>();
        let label = entry.last_child().and_downcast::<gtk::Label>();
        let (Some(dots), Some(label)) = (dots, label) else {
            return;
        };
        label.set_label(&name);
        let set = harmony::THEMES.iter().find(|set| set.name == name);
        dots.set_visible(set.is_some());
        let three = set.map(|set| [set.focus, set.bullet, set.accent].map(slipstream_config::rgb));
        dots.set_draw_func(move |_, cairo, _, height| {
            let radius = height as f64 / 2.0 - 1.0;
            for (n, rgb) in three.iter().flatten().enumerate() {
                let Some([r, g, b]) = rgb else { continue };
                let x = height as f64 / 2.0 + n as f64 * (height as f64 + 4.0);
                cairo.arc(x, height as f64 / 2.0, radius, 0.0, std::f64::consts::TAU);
                cairo.set_source_rgb(*r as f64, *g as f64, *b as f64);
                let _ = cairo.fill();
            }
        });
    });
    factory
}

/// The theme, the focus ring on a rainbow with the other two following it, and each colour on
/// its own for a choice of one's own.
fn colours(card: &gtk::Box, store: &Store) {
    let names: Vec<&str> = harmony::THEMES.iter().map(|theme| theme.name).collect();
    let names = gtk::StringList::new(&names);
    let theme = gtk::DropDown::new(Some(names.clone()), gtk::Expression::NONE);
    theme.set_factory(Some(&theme_entries()));
    let wheel = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 360.0, 1.0);
    wheel.set_round_digits(0);
    wheel.set_draw_value(false);
    wheel.set_width_request(200);
    wheel.add_css_class("hue");
    // The handlers hold it; GTK lets them go with the widgets when the window closes.
    let colours = Rc::new(Colours {
        store: store.clone(),
        theme,
        names,
        shown: Cell::new(Colours::current_of(&store.get())),
        wheel,
        focus: colour_button("Primary colour"),
        bullet: colour_button("Secondary colour"),
        accent: colour_button("Tertiary colour"),
        syncing: Cell::new(false),
    });

    let chosen = Rc::clone(&colours);
    colours.theme.connect_selected_notify(move |theme| {
        if chosen.syncing.get() {
            return;
        }
        if let Some(set) = harmony::THEMES.get(theme.selected() as usize) {
            let three = [set.focus, set.bullet, set.accent]
                .map(|hex| slipstream_config::rgb(hex).expect("a valid theme"));
            chosen.choose(three);
        }
    });
    let turned = Rc::clone(&colours);
    colours.wheel.connect_value_changed(move |wheel| {
        if !turned.syncing.get() {
            turned.choose_focus(harmony::on_wheel(wheel.value()));
        }
    });
    let picked = Rc::clone(&colours);
    colours.focus.connect_rgba_notify(move |button| {
        if !picked.syncing.get() {
            let rgba = button.rgba();
            picked.choose_focus([rgba.red(), rgba.green(), rgba.blue()]);
        }
    });
    for (which, button) in [(1, &colours.bullet), (2, &colours.accent)] {
        let picked = Rc::clone(&colours);
        button.connect_rgba_notify(move |button| {
            if !picked.syncing.get() {
                let rgba = button.rgba();
                let mut three = picked.shown.get();
                three[which] = [rgba.red(), rgba.green(), rgba.blue()];
                picked.choose(three);
            }
        });
    }
    colours.show(colours.shown.get());

    row(
        card,
        "Theme",
        Some("Sets of three colours that go together. Change any of them below to make your own."),
        &colours.theme,
    );
    let focus_control = gtk::Box::builder().spacing(10).build();
    focus_control.append(&colours.wheel);
    focus_control.append(&colours.focus);
    row(
        card,
        "Primary colour",
        Some(
            "Where the keyboard is: the ring around the focused window and in the panels, and the \
             workspace you're on. Also a battery that's charging and plugins that are on. Pick \
             any colour, and the two below are chosen to go with it.",
        ),
        &focus_control,
    );
    row(
        card,
        "Secondary colour",
        Some(
            "Bullet time, and whatever wants your attention: the notification count, Caps Lock, \
             a low battery, and apps and plugins asking to be looked at.",
        ),
        &colours.bullet,
    );
    row(
        card,
        "Tertiary colour",
        Some(
            "What's on or chosen: switches, tiles and ticks, sliders, today in the calendar, the \
             text cursor, toasts and the button Enter presses.",
        ),
        &colours.accent,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_colours_are_the_first_theme() {
        assert_eq!(
            matching_theme(Colours::current_of(&Settings::default())),
            Some(0)
        );
    }

    #[test]
    fn colours_that_match_no_set_are_your_own() {
        let mut three = Colours::current_of(&Settings::default());
        three[2] = slipstream_config::rgb("#123456").unwrap();
        assert_eq!(matching_theme(three), None);
    }
}

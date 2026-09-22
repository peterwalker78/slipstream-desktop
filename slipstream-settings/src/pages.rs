//! The pages, as in the mockup's Settings window, keeping only the settings Slipstream follows.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk::prelude::*;
use gtk4 as gtk;
use slipstream_config::{
    Settings,
    meter::{self as reports, RUN_LIMIT, Reading, Report, Run, Section},
};

use crate::{Store, power as supplies, previews};

pub struct Page {
    pub id: &'static str,
    pub title: &'static str,
    /// Icon names; the first one the icon theme has is shown (Breeze's, then Adwaita's).
    pub icons: &'static [&'static str],
    pub build: fn(&Store) -> gtk::Widget,
}

pub const PAGES: [Page; 10] = [
    Page {
        id: "appearance",
        title: "Appearance",
        icons: &[
            "preferences-desktop-theme-global-symbolic",
            "preferences-desktop-appearance-symbolic",
        ],
        build: appearance,
    },
    Page {
        id: "wallpaper",
        title: "Wallpaper",
        icons: &[
            "preferences-desktop-wallpaper-symbolic",
            "preferences-desktop-wallpaper",
            "image-x-generic-symbolic",
        ],
        build: wallpaper,
    },
    Page {
        id: "screens",
        title: "Screens",
        icons: &[
            "preferences-desktop-display-symbolic",
            "video-display-symbolic",
            "preferences-desktop-display",
        ],
        build: screens,
    },
    Page {
        id: "sound",
        title: "Sound",
        icons: &["audio-volume-high-symbolic", "audio-speakers-symbolic"],
        build: sound,
    },
    Page {
        id: "power",
        title: "Power",
        icons: &[
            "battery-full-charging-symbolic",
            "battery-good-symbolic",
            "preferences-system-power-symbolic",
        ],
        build: power,
    },
    Page {
        id: "notifications",
        title: "Notifications",
        icons: &[
            "preferences-desktop-notification-bell-symbolic",
            "preferences-system-notifications-symbolic",
            "notifications-symbolic",
        ],
        build: notifications,
    },
    Page {
        id: "meter",
        title: "Meter",
        icons: &[
            "speedometer-symbolic",
            "xsi-gauge-symbolic",
            "power-profile-performance-symbolic",
        ],
        build: meter,
    },
    Page {
        id: "workspaces",
        title: "Workspaces",
        icons: &[
            "view-app-grid-symbolic",
            "preferences-desktop-virtual-symbolic",
            "view-grid-symbolic",
        ],
        build: workspaces,
    },
    Page {
        id: "session",
        title: "Session",
        icons: &[
            "system-log-out-symbolic",
            "system-shutdown-symbolic",
            "document-open-recent-symbolic",
        ],
        build: session,
    },
    Page {
        id: "about",
        title: "About",
        icons: &["help-about-symbolic"],
        build: about,
    },
];

pub fn ids() -> Vec<&'static str> {
    PAGES.iter().map(|page| page.id).collect()
}

/// The waits offered before the UI fades to the living wallpaper, in seconds; 0 never fades.
const FADE_CHOICES: [u64; 7] = [30, 60, 120, 300, 600, 1800, 0];

/// The longest a pop-up may wait for a pause in typing, in minutes.
const LONGEST_WAIT_CHOICES: [u64; 6] = [1, 5, 10, 15, 30, 60];

/// How long a variation holds before the next one takes over, in minutes; 0 never changes.
const CHANGE_CHOICES: [u64; 8] = [1, 2, 5, 10, 15, 30, 60, 0];

fn appearance(store: &Store) -> gtk::Widget {
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
        Some(
            "Warmer colours in the evening. Quick settings (Super+A) turns it on and off by hand.",
        ),
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
                                           (Super+A) turns it on and off by hand."
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

/// Where the screens sit. Each one after the first is placed against the one before it, which is
/// all the arranging two or three screens on a desk ever needs; the compositor moves them the
/// moment this is saved.
fn screens(store: &Store) -> gtk::Widget {
    let page = page(
        "Screens",
        "Where your screens sit, so the pointer and Super+arrows cross between them the way they          are really arranged on your desk.",
    );
    let card = group(&page, "Arrangement");
    fill_screens(&card, store);
    let note = group(&page, "About this list");
    row(
        &note,
        "Screens are remembered by what they are",
        Some(
            "A screen is known by what it reports about itself, so moving it to another port              keeps its place, and a different screen on the same port gets its own. Screens that              aren't plugged in now aren't shown.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
}

/// The rows, rebuilt whenever the shape changes. Reads the screens the compositor says are lit
/// (`screens.toml`), so this is the same list, named the same way, that the compositor arranges.
fn fill_screens(card: &gtk::Box, store: &Store) {
    use slipstream_config::{Align, Position, screens as known};
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let remembered = known::read(&known::path());
    let lit: Vec<&known::Known> = remembered.lit().collect();
    if lit.len() < 2 {
        let note = match lit.len() {
            0 => "No screens to arrange. This list fills in when Slipstream is running.",
            _ => "Only one screen. Plug another in and it can be placed against this one.",
        };
        row(
            card,
            "Nothing to arrange",
            Some(note),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    let refill = {
        let card = card.clone();
        let store = store.clone();
        move || {
            let card = card.clone();
            let store = store.clone();
            gtk::glib::idle_add_local_once(move || fill_screens(&card, &store));
        }
    };
    for (index, screen) in lit.iter().enumerate() {
        let size = format!("{}×{}", screen.width, screen.height);
        // The first screen is the one everything else is placed against, so it has nothing to set.
        let Some(before) = lit.get(index.wrapping_sub(1)) else {
            row(
                card,
                screen.name(),
                Some(&format!(
                    "{size} · {} · everything else is placed against this one",
                    screen.connector
                )),
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
            );
            continue;
        };
        let place = store.get().display.place(&screen.monitor);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let names: Vec<String> = Position::ALL
            .iter()
            .map(|position| position.label(before.name()))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let position = gtk::DropDown::from_strings(&refs);
        position.set_selected(
            Position::ALL
                .iter()
                .position(|which| *which == place.position)
                .unwrap_or(0) as u32,
        );

        let align_names: Vec<&str> = Align::ALL
            .iter()
            .map(|align| align.label(place.position.beside()))
            .collect();
        let align = gtk::DropDown::from_strings(&align_names);
        align.set_selected(
            Align::ALL
                .iter()
                .position(|which| *which == place.align)
                .unwrap_or(0) as u32,
        );
        controls.append(&position);
        controls.append(&align);
        row(
            card,
            screen.name(),
            Some(&format!("{size} · {}", screen.connector)),
            &controls,
        );

        let monitor = screen.monitor.clone();
        {
            let (store, align, refill) = (store.clone(), align.clone(), refill.clone());
            let monitor = monitor.clone();
            position.connect_selected_notify(move |position| {
                let Some(which) = Position::ALL.get(position.selected() as usize).copied() else {
                    return;
                };
                let chosen = Align::ALL
                    .get(align.selected() as usize)
                    .copied()
                    .unwrap_or_default();
                let monitor = monitor.clone();
                store.change(move |settings| {
                    settings.display.set_place(slipstream_config::ScreenPlace {
                        monitor: monitor.clone(),
                        position: which,
                        align: chosen,
                    })
                });
                // Beside and stacked name different edges, so the other list has to be rebuilt.
                refill();
            });
        }
        {
            let (store, position) = (store.clone(), position.clone());
            align.connect_selected_notify(move |align| {
                let Some(chosen) = Align::ALL.get(align.selected() as usize).copied() else {
                    return;
                };
                let which = Position::ALL
                    .get(position.selected() as usize)
                    .copied()
                    .unwrap_or_default();
                let monitor = monitor.clone();
                store.change(move |settings| {
                    settings.display.set_place(slipstream_config::ScreenPlace {
                        monitor: monitor.clone(),
                        position: which,
                        align: chosen,
                    })
                });
            });
        }
    }
}

fn wallpaper(store: &Store) -> gtk::Widget {
    let page = page(
        "Wallpaper",
        "The living wallpaper behind your windows. Tick more than one and they take turns.",
    );

    let variations = group(&page, "Variations");
    let chosen: Vec<&str> = store
        .get()
        .wallpaper
        .chosen()
        .iter()
        .map(|variation| variation.id)
        .collect();
    let all = gtk::CheckButton::new();
    let all_sub = row(&variations, "All of them", Some(""), &all);
    let mut pictures = Vec::new();
    let checks: Rc<Vec<(&'static str, gtk::CheckButton)>> = Rc::new(
        slipstream_config::VARIATIONS
            .iter()
            .map(|variation| {
                let check = gtk::CheckButton::new();
                check.set_active(chosen.contains(&variation.id));
                let picture = previews::picture();
                picture.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Preview of {}",
                    variation.title
                ))]);
                picture_row(
                    &variations,
                    &picture,
                    variation.title,
                    Some(variation.blurb),
                    &check,
                );
                pictures.push((variation.id, picture));
                (variation.id, check)
            })
            .collect(),
    );
    previews::fill(pictures, store.get().motion.reduced);

    // Setting ticks from here must not be taken for the user's own changes.
    let settling = Rc::new(Cell::new(false));
    // The "All of them" tick follows the others: ticked when every one is, half-ticked when some
    // are.
    let sync_all = {
        let (all, checks, settling) = (all.clone(), Rc::clone(&checks), Rc::clone(&settling));
        Rc::new(move || {
            let ticked = checks.iter().filter(|(_, check)| check.is_active()).count();
            let every = ticked == checks.len();
            settling.set(true);
            all.set_active(every);
            all.set_inconsistent(!every && ticked > 0);
            settling.set(false);
            if let Some(sub) = &all_sub {
                sub.set_label(&all_blurb(every));
            }
        })
    };
    sync_all();
    let save = {
        let (checks, store) = (Rc::clone(&checks), store.clone());
        move || {
            let ticked: Vec<&str> = checks
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(id, _)| *id)
                .collect();
            store.change(|settings| {
                settings.wallpaper.variations =
                    ticked_list(&settings.wallpaper.variations, &ticked);
            });
        }
    };
    let save = Rc::new(save);
    // Unticking the last one is refused rather than leaving the desktop with no wallpaper.
    for (_, check) in checks.iter() {
        let (checks, settling, sync_all, save) = (
            Rc::clone(&checks),
            Rc::clone(&settling),
            Rc::clone(&sync_all),
            Rc::clone(&save),
        );
        check.connect_toggled(move |check| {
            if settling.get() {
                return;
            }
            if checks.iter().all(|(_, check)| !check.is_active()) {
                settling.set(true);
                check.set_active(true);
                settling.set(false);
                return;
            }
            save();
            sync_all();
        });
    }
    // Ticking "All of them" ticks every one. Unticking it, which can only happen when every one
    // is ticked, goes back to the first alone, since the set may never be empty.
    all.connect_toggled(move |all| {
        if settling.get() {
            return;
        }
        let ticks = all_ticks(all.is_active(), checks.len());
        settling.set(true);
        for ((_, check), tick) in checks.iter().zip(ticks) {
            check.set_active(tick);
        }
        settling.set(false);
        save();
        sync_all();
    });

    let cycle = group(&page, "Cycle");
    let current = store.get().wallpaper.change_every_mins;
    let every = choices(&CHANGE_CHOICES, current);
    let names: Vec<String> = every.iter().map(|&mins| describe(mins * 60)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let change = gtk::DropDown::from_strings(&names);
    change.set_selected(every.iter().position(|&mins| mins == current).unwrap_or(0) as u32);
    let change_store = store.clone();
    change.connect_selected_notify(move |change| {
        if let Some(&mins) = every.get(change.selected() as usize) {
            change_store.change(|settings| settings.wallpaper.change_every_mins = mins);
        }
    });
    row(
        &cycle,
        "Change the picture every",
        Some(
            "With more than one ticked, they take turns in a random order. The next one takes \
             over at the end of a turn, so the screen is nearly empty when the picture changes.",
        ),
        &change,
    );

    let idle = group(&page, "Taking the screen");
    let current = store.get().wallpaper.fade_after_secs;
    let waits = choices(&FADE_CHOICES, current);
    let names: Vec<String> = waits.iter().map(|&secs| describe(secs)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let fade = gtk::DropDown::from_strings(&names);
    fade.set_selected(waits.iter().position(|&secs| secs == current).unwrap_or(0) as u32);
    let fade_store = store.clone();
    fade.connect_selected_notify(move |fade| {
        if let Some(&secs) = waits.get(fade.selected() as usize) {
            fade_store.change(|settings| settings.wallpaper.fade_after_secs = secs);
        }
    });
    row(
        &idle,
        "Fade to the wallpaper after",
        Some(
            "Time with no keyboard or mouse input. A fullscreen window or a playing video keeps \
             the desktop up, and so does Awake: hold Caps Lock down to turn it on or off.",
        ),
        &fade,
    );
    page.upcast()
}

/// The ticks after "All of them" is pressed, for `count` variations: every one when it goes on,
/// and the first alone when it goes off, since the set may never be empty.
fn all_ticks(on: bool, count: usize) -> Vec<bool> {
    (0..count).map(|index| on || index == 0).collect()
}

/// What the "All of them" row says, with every variation ticked or not.
fn all_blurb(every: bool) -> String {
    if every {
        format!(
            "Every one takes its turn, including any added later. Untick to keep only {}.",
            slipstream_config::VARIATIONS[0].title
        )
    } else {
        "Tick to have every one take its turn.".to_string()
    }
}

/// What to write for the ticked variations: nothing at all when every one of them is ticked, so
/// that goes on meaning "all of them" as more are added. Otherwise the ones still ticked keep the
/// order the file had them in, and any newly ticked follow in the order they are listed.
fn ticked_list(current: &[String], ticked: &[&str]) -> Vec<String> {
    if slipstream_config::VARIATIONS
        .iter()
        .all(|variation| ticked.contains(&variation.id))
    {
        return Vec::new();
    }
    // A replaced variation's old id is kept in its place under the id that replaced it.
    let mut list: Vec<String> = Vec::new();
    for id in current {
        let id = slipstream_config::current_id(id);
        if ticked.contains(&id) && !list.iter().any(|kept| kept == id) {
            list.push(id.to_string());
        }
    }
    for id in ticked {
        if !list.iter().any(|kept| kept == id) {
            list.push((*id).to_string());
        }
    }
    list
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

/// The sounds Slipstream makes itself. One switch so far; the muffling in bullet time is not a
/// setting yet.
fn sound(store: &Store) -> gtk::Widget {
    let page = page("Sound", "The sounds Slipstream makes itself");
    let keys = group(&page, "Volume");

    let blip = gtk::CheckButton::new();
    blip.set_active(store.get().sound.volume_blip);
    row(
        &keys,
        "Click when the volume changes",
        Some(
            "A short click through the speakers at the new level, from the volume keys or quick \
             settings' slider, as macOS and Windows do, so the volume can be heard as well as \
             seen. Never on mute, and never on brightness.",
        ),
        &blip,
    );
    let store = store.clone();
    blip.connect_toggled(move |blip| {
        let on = blip.is_active();
        store.change(move |settings| settings.sound.volume_blip = on);
    });
    page.upcast()
}

/// How often the meter's command can be set to run, in seconds.
const METER_EVERY_CHOICES: [u64; 6] = [10, 30, 60, 120, 300, 600];

/// A command being typed is saved once typing pauses for this long, or straight away on Enter.
const COMMAND_PAUSE: Duration = Duration::from_millis(1500);

/// The bar's meter: the command that feeds it, how often it runs, and a way to try it out.
fn meter(store: &Store) -> gtk::Widget {
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

    let current = settings.every_secs.max(10);
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
                reports::run(sh, RUN_LIMIT)
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

/// Pop-ups: whether they wait while you type, for how long at most, and do not disturb.
fn notifications(store: &Store) -> gtk::Widget {
    let page = page(
        "Notifications",
        "Pop-ups that wait for a natural break instead of cutting into what you're doing",
    );
    let typing = group(&page, "While you type");

    let wait = gtk::Switch::new();
    wait.set_active(store.get().notifications.wait_while_typing);
    row(
        &typing,
        "Wait until I pause",
        Some(
            "Pop-ups that arrive while you're typing wait until you stop typing for 15 seconds, then show \
             as one card. The bell counts them straight away, and Super+N shows them at any time. \
             Critical notifications never wait. Only the timing of key presses is used, never \
             which keys.",
        ),
        &wait,
    );

    let current = store.get().notifications.longest_wait_mins;
    let waits = choices(&LONGEST_WAIT_CHOICES, current);
    let names: Vec<String> = waits.iter().map(|&mins| describe(mins * 60)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let longest = gtk::DropDown::from_strings(&names);
    longest.set_selected(waits.iter().position(|&mins| mins == current).unwrap_or(0) as u32);
    longest.set_sensitive(store.get().notifications.wait_while_typing);
    let longest_store = store.clone();
    longest.connect_selected_notify(move |longest| {
        if let Some(&mins) = waits.get(longest.selected() as usize) {
            longest_store.change(|settings| settings.notifications.longest_wait_mins = mins);
        }
    });
    row(
        &typing,
        "Longest wait",
        Some(
            "However long you keep typing, waiting pop-ups show after this. Anything truly urgent \
             usually reaches your phone first.",
        ),
        &longest,
    );

    let follower = longest.clone();
    let wait_store = store.clone();
    wait.connect_active_notify(move |wait| {
        let on = wait.is_active();
        follower.set_sensitive(on);
        wait_store.change(move |settings| settings.notifications.wait_while_typing = on);
    });

    row(
        &typing,
        "Windows don't take the keyboard",
        Some(
            "A window that opens while you're typing in another app waits beside you instead of \
             catching your next words. Alt+Tab goes to it. Windows you open yourself, and dialogs \
             of the app you're in, still come straight to you.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );

    let heard = group(&page, "Sounds");
    let sounds = gtk::Switch::new();
    sounds.set_active(store.get().notifications.sounds);
    let sounds_store = store.clone();
    sounds.connect_active_notify(move |sounds| {
        let on = sounds.is_active();
        sounds_store.change(move |settings| settings.notifications.sounds = on);
    });
    row(
        &heard,
        "Play notification sounds",
        Some(
            "The sound an app asks for, from your sound theme. Never while you're working in the \
             window it came from, and under do not disturb only for critical notifications, \
             which also bring the desktop back from the wallpaper.",
        ),
        &sounds,
    );

    let quiet = group(&page, "Do not disturb");
    let dnd = gtk::Switch::new();
    dnd.set_active(store.get().notifications.do_not_disturb);
    let dnd_store = store.clone();
    dnd.connect_active_notify(move |dnd| {
        let on = dnd.is_active();
        dnd_store.change(move |settings| settings.notifications.do_not_disturb = on);
    });
    row(
        &quiet,
        "Do not disturb",
        Some(
            "Nothing pops up, except critical notifications; everything still reaches the \
             notification centre. Quick settings (Super+A) has the same switch.",
        ),
        &dnd,
    );
    page.upcast()
}

/// The workspaces: how many there are, what each is called, and their order.
fn workspaces(store: &Store) -> gtk::Widget {
    let page = page(
        "Workspaces",
        "As many as you want, up to twenty, each with a name of its own or none. Super and a \
         number goes to one of the first nine.",
    );
    let card = group(&page, "Workspaces");
    fill_workspaces(&card, store);
    let note = group(&page, "Deleting one");
    row(
        &note,
        "Its windows aren't lost",
        Some(
            "They move to the workspace before it, or to the first one. A workspace with no name \
             goes by its number, which changes as the list does.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
}

/// The list's rows, built again from the file whenever its shape changes.
fn fill_workspaces(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let list = store.get().workspaces.entries();
    let count = list.len();
    // Structure changes rebuild the list, after the click that caused them has finished.
    let refill = {
        let card = card.clone();
        let store = store.clone();
        move || {
            let card = card.clone();
            let store = store.clone();
            gtk::glib::idle_add_local_once(move || fill_workspaces(&card, &store));
        }
    };
    for (index, entry) in list.iter().enumerate() {
        let name = gtk::Entry::builder()
            .text(&entry.name)
            .placeholder_text(format!("Workspace {}", index + 1))
            .width_chars(18)
            .max_length(40)
            .build();
        let id = entry.id;
        let rename_store = store.clone();
        name.connect_changed(move |name| {
            let text = name.text().trim().to_string();
            rename_store.change(|settings| {
                settings.workspaces.list = settings.workspaces.entries();
                if let Some(entry) = settings.workspaces.list.iter_mut().find(|e| e.id == id) {
                    entry.name = text;
                }
            });
        });
        let button = |icon: &str, tip: &str, enabled: bool| {
            let button = gtk::Button::from_icon_name(icon);
            button.set_tooltip_text(Some(tip));
            button.set_sensitive(enabled);
            button.add_css_class("flat");
            button
        };
        let up = button("go-up-symbolic", "Move up", index > 0);
        let down = button("go-down-symbolic", "Move down", index + 1 < count);
        let delete = button("user-trash-symbolic", "Delete", count > 1);
        for (control, step) in [(&up, -1isize), (&down, 1)] {
            let store = store.clone();
            let refill = refill.clone();
            control.connect_clicked(move |_| {
                store.change(|settings| {
                    let mut list = settings.workspaces.entries();
                    let to = index as isize + step;
                    if to >= 0 && (to as usize) < list.len() {
                        list.swap(index, to as usize);
                    }
                    settings.workspaces.list = list;
                });
                refill();
            });
        }
        {
            let store = store.clone();
            let refill = refill.clone();
            delete.connect_clicked(move |_| {
                store.change(|settings| {
                    let mut list = settings.workspaces.entries();
                    // The last one stays: a desktop needs somewhere to put a window.
                    if list.len() > 1 {
                        list.retain(|entry| entry.id != id);
                    }
                    settings.workspaces.list = list;
                });
                refill();
            });
        }
        let controls = gtk::Box::builder().spacing(4).build();
        controls.append(&name);
        controls.append(&up);
        controls.append(&down);
        controls.append(&delete);
        let title = format!("{}", index + 1);
        row(card, &title, None, &controls);
    }
    let add = gtk::Button::with_label("Add a workspace");
    add.set_sensitive(count < slipstream_config::MAX_WORKSPACES);
    let add_store = store.clone();
    add.connect_clicked(move |_| {
        add_store.change(|settings| {
            let mut list = settings.workspaces.entries();
            if list.len() < slipstream_config::MAX_WORKSPACES {
                let id = slipstream_config::Workspaces { list: list.clone() }.fresh_id();
                list.push(slipstream_config::WorkspaceEntry {
                    id,
                    name: String::new(),
                });
            }
            settings.workspaces.list = list;
        });
        refill();
    });
    let plural = if count == 1 {
        "workspace"
    } else {
        "workspaces"
    };
    row(
        card,
        &format!("{count} {plural}"),
        Some("A new one goes at the end, empty."),
        &add,
    );
}

/// Logging out: whether the layout is written down, and whether it comes back without asking.
fn session(store: &Store) -> gtk::Widget {
    let page = page(
        "Session",
        "What happens to your windows when you log out, and what comes back when you log in.",
    );
    let restore = group(&page, "Restore");

    let remember = gtk::CheckButton::new();
    remember.set_active(store.get().session.remember);
    row(
        &restore,
        "Remember the layout",
        Some(
            "Writes which apps were open, on which workspace and in which tiling place, to \
             ~/.local/state/slipstream/session.toml. With this off nothing is written at all, and \
             any record already there is deleted. The card that asks whether to log out carries \
             the same switch.",
        ),
        &remember,
    );

    let straight = gtk::CheckButton::new();
    straight.set_active(store.get().session.reopen_without_asking);
    straight.set_sensitive(store.get().session.remember);
    row(
        &restore,
        "Reopen without asking",
        Some("Skips the card at login that offers the layout back, and puts it straight back."),
        &straight,
    );

    // Nothing is recorded with the first switch off, so there would be nothing to reopen.
    let follower = straight.clone();
    let store_remember = store.clone();
    remember.connect_toggled(move |remember| {
        let on = remember.is_active();
        follower.set_sensitive(on);
        store_remember.change(move |settings| settings.session.remember = on);
    });
    let store_straight = store.clone();
    straight.connect_toggled(move |straight| {
        let on = straight.is_active();
        store_straight.change(move |settings| settings.session.reopen_without_asking = on);
    });

    let lock = group(&page, "Lock");
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
            "Time with no keyboard or mouse input. A fullscreen window, a playing video or Awake (hold Caps Lock) holds it off. Super+L or the lock in quick settings locks at any time, and ends Awake.",
        ),
        &after,
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
        Some("Closing the lid or choosing Sleep locks first, so the laptop wakes up locked."),
        &before_sleep,
    );

    let clipboard = group(&page, "Clipboard");
    let history = gtk::Switch::new();
    history.set_active(store.get().clipboard.history);
    let history_store = store.clone();
    history.connect_active_notify(move |switch| {
        let on = switch.is_active();
        history_store.change(move |settings| settings.clipboard.history = on);
    });
    row(
        &clipboard,
        "Clipboard history",
        Some(
            "Super+V lists the last 25 things you copied, text and pictures, to paste any of \
             them again. Kept in memory only and forgotten when you log out; anything a password \
             manager marks as secret is never kept. Turning this off forgets them all.",
        ),
        &history,
    );

    let note = group(&page, "What it brings back");
    // Said here as well as on the card: the gap between "my windows came back" and "my work came
    // back" is where the disappointment lives.
    row(
        &note,
        "Your apps, not your work",
        Some(
            "Each app is started again the way the app explorer starts it, so it comes back on \
             the workspace and in the place it had. What it had open is up to the app: a browser \
             restores its own tabs, a terminal comes back to a fresh shell in your home folder, \
             and an unsaved buffer is only as safe as the editor's own recovery. Window titles \
             are never written down.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
}

fn about(store: &Store) -> gtk::Widget {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .css_classes(["main"])
        .build();
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 2);
    hero.append(&label("Slipstream", "hero"));
    hero.append(&label(&format!("Desktop on {}", os_name()), "page-sub"));
    page.append(&hero);

    let software = group(&page, "Software");
    row(
        &software,
        "Version",
        None,
        &label(env!("CARGO_PKG_VERSION"), "kv"),
    );
    let file = label(&home_relative(store.path()), "kv");
    file.set_selectable(true);
    row(
        &software,
        "Settings file",
        Some("Hand edits apply as soon as they're saved."),
        &file,
    );
    page.upcast()
}

/// How often the Power page reads the battery again.
const POWER_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// The battery's charge and what it's doing, then the details: power flowing, the charger, and
/// how the battery has worn. Read again every couple of seconds while the window is open.
fn power(_: &Store) -> gtk::Widget {
    let page = page("Power", "Battery and charging");

    // The mockup's battery card: the charge, a bar, and the time.
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
    // Reading stops once the page is gone with its window.
    let weak = page.downgrade();
    gtk::glib::timeout_add_local(POWER_EVERY, move || {
        if weak.upgrade().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        update();
        gtk::glib::ControlFlow::Continue
    });
    page.upcast()
}

/// The charger row: its rating when the kernel knows it.
fn charger_text(reading: &supplies::Reading) -> String {
    match (reading.plugged, reading.charger_watts) {
        (true, Some(watts)) => format!("{watts:.0} W, plugged in"),
        (true, None) => "Plugged in".to_string(),
        (false, _) => "Not plugged in".to_string(),
    }
}

/// A page with its heading.
fn page(title: &str, sub: &str) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .css_classes(["main"])
        .build();
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 3);
    heading.append(&label(title, "page-title"));
    heading.append(&label(sub, "page-sub"));
    page.append(&heading);
    page
}

/// A titled card on `page`, for rows.
fn group(page: &gtk::Box, title: &str) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 7);
    // GTK's CSS has no text-transform, so the title is uppercased here.
    group.append(&label(&title.to_uppercase(), "group-title"));
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["card"])
        .build();
    group.append(&card);
    page.append(&group);
    card
}

/// A row in `card`: its title and a line of explanation on the left, `control` on the right.
/// Screen readers get the title as the control's name and the explanation as its description.
/// Returns the explanation's label, to change later.
fn row(
    card: &gtk::Box,
    title: &str,
    sub: Option<&str>,
    control: &impl IsA<gtk::Widget>,
) -> Option<gtk::Label> {
    let (row, sub) = row_box(title, sub, control);
    card.append(&row);
    sub
}

/// A row as `row` makes one, with `lead` before its title: a picture of what it chooses.
fn picture_row(
    card: &gtk::Box,
    lead: &impl IsA<gtk::Widget>,
    title: &str,
    sub: Option<&str>,
    control: &impl IsA<gtk::Widget>,
) {
    let (row, _) = row_box(title, sub, control);
    row.prepend(lead);
    card.append(&row);
}

fn row_box(
    title: &str,
    sub: Option<&str>,
    control: &impl IsA<gtk::Widget>,
) -> (gtk::Box, Option<gtk::Label>) {
    let control: &gtk::Widget = control.upcast_ref();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title = label(title, "row-title");
    text.append(&title);
    control.update_relation(&[gtk::accessible::Relation::LabelledBy(&[title
        .upcast_ref::<gtk::Accessible>(
    )])]);
    let sub = sub.map(|sub| {
        let sub = label(sub, "row-sub");
        sub.set_wrap(true);
        text.append(&sub);
        control.update_relation(&[gtk::accessible::Relation::DescribedBy(&[sub
            .upcast_ref::<gtk::Accessible>(
        )])]);
        sub
    });
    control.set_valign(gtk::Align::Center);
    let row = gtk::Box::builder().spacing(14).css_classes(["row"]).build();
    row.append(&text);
    row.append(control);
    (row, sub)
}

fn label(text: &str, class: &str) -> gtk::Label {
    // Every label wraps, so the window's minimum width is its widest word and control rather than
    // its longest sentence: in a tile narrower than that it would have to be drawn scaled down.
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes([class])
        .build()
}

/// How long before locking by itself, in minutes: Never first, then the usual waits, with a
/// hand-edited one in its place among them.
fn lock_choices(current: u64) -> Vec<u64> {
    let mut choices = vec![0, 1, 5, 10, 15, 30, 60];
    if !choices.contains(&current) {
        choices.push(current);
        choices.sort_unstable();
    }
    choices
}

/// The usual times, plus the one in the file if it's something else (a hand edit), shortest
/// first and Never last.
fn choices(usual: &[u64], current: u64) -> Vec<u64> {
    let mut choices = usual.to_vec();
    if !choices.contains(&current) {
        choices.push(current);
    }
    choices.sort_by_key(|&time| if time == 0 { u64::MAX } else { time });
    choices
}

fn describe(secs: u64) -> String {
    let count = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    match secs {
        0 => "Never".to_string(),
        secs if secs % 3600 == 0 => count(secs / 3600, "hour"),
        secs if secs % 60 == 0 => count(secs / 60, "minute"),
        secs => count(secs, "second"),
    }
}

/// The distribution's name and version from `/etc/os-release`, such as "Bazzite 44".
fn os_name() -> String {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| {
        release.lines().find_map(|line| {
            line.strip_prefix(key)?
                .strip_prefix('=')
                .map(|value| value.trim_matches('"').to_string())
        })
    };
    match (field("NAME"), field("VERSION_ID")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        _ => "Linux".to_string(),
    }
}

fn home_relative(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
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

    #[test]
    fn lock_waits_start_at_never() {
        assert_eq!(lock_choices(0), [0, 1, 5, 10, 15, 30, 60]);
        assert_eq!(lock_choices(20), [0, 1, 5, 10, 15, 20, 30, 60]);
    }

    #[test]
    fn waits_read_naturally() {
        assert_eq!(describe(0), "Never");
        assert_eq!(describe(30), "30 seconds");
        assert_eq!(describe(60), "1 minute");
        assert_eq!(describe(120), "2 minutes");
        assert_eq!(describe(90), "90 seconds");
        assert_eq!(describe(3600), "1 hour");
    }

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

    #[test]
    fn a_hand_edited_wait_joins_the_choices_in_order() {
        assert_eq!(choices(&FADE_CHOICES, 120), FADE_CHOICES);
        assert_eq!(
            choices(&FADE_CHOICES, 90),
            [30, 60, 90, 120, 300, 600, 1800, 0]
        );
        assert_eq!(
            choices(&FADE_CHOICES, 7200).last(),
            Some(&0),
            "Never stays last"
        );
        assert_eq!(choices(&CHANGE_CHOICES, 10), CHANGE_CHOICES);
    }

    #[test]
    fn all_of_them_ticks_every_one_or_goes_back_to_the_first() {
        let count = slipstream_config::VARIATIONS.len();
        let every = all_ticks(true, count);
        assert!(every.iter().all(|&tick| tick));
        let ids: Vec<&str> = slipstream_config::VARIATIONS.iter().map(|v| v.id).collect();
        assert_eq!(
            ticked_list(&["tide".to_string()], &ids),
            Vec::<String>::new()
        );
        let first = all_ticks(false, count);
        assert_eq!(first.iter().filter(|&&tick| tick).count(), 1, "never none");
        assert!(first[0]);
    }

    #[test]
    fn every_variation_ticked_is_written_as_no_list_at_all() {
        let all: Vec<&str> = slipstream_config::VARIATIONS
            .iter()
            .map(|variation| variation.id)
            .collect();
        assert_eq!(
            ticked_list(&["slipstream".to_string()], &all),
            Vec::<String>::new()
        );
    }

    #[test]
    fn ticking_keeps_the_order_the_file_had_and_adds_the_new_ones_after() {
        let current = ["vortex".to_string(), "slipstream".to_string()];
        assert_eq!(
            ticked_list(&current, &["slipstream", "vortex", "tide"]),
            ["vortex", "slipstream", "tide"],
            "the file's own order survives a tick"
        );
        assert_eq!(
            ticked_list(&current, &["vortex"]),
            ["vortex"],
            "unticking one leaves the rest alone"
        );
        // A replaced variation's id is written out as its replacement's, in the same place.
        let old = ["decode".to_string(), "slipstream".to_string()];
        assert_eq!(
            ticked_list(&old, &["slipstream", "vortex", "tide"]),
            ["vortex", "slipstream", "tide"]
        );
        // A file that names none means all of them, so ticking one off writes the rest out.
        let from_all: Vec<&str> = slipstream_config::VARIATIONS
            .iter()
            .map(|variation| variation.id)
            .filter(|id| *id != "glitch")
            .collect();
        assert_eq!(ticked_list(&[], &from_all).len(), from_all.len());
    }
}

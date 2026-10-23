//! The living wallpaper: when it shows, and which variations take turns.

use super::*;
use crate::previews;

/// The waits offered before the UI fades to the living wallpaper, in seconds; 0 never fades.
pub(super) const FADE_CHOICES: [u64; 7] = [30, 60, 120, 300, 600, 1800, 0];

/// How long a variation holds before the next one takes over, in minutes; 0 never changes.
pub(super) const CHANGE_CHOICES: [u64; 8] = [1, 2, 5, 10, 15, 30, 60, 0];

/// How tall the list of variations is: as tall as the preview, about six rows.
const LIST_HEIGHT: i32 = previews::SHOWN.1;

/// The narrowest the list may be beside the preview; with less room it goes beneath it.
const LIST_BESIDE: i32 = 280;

/// The space between the preview and the list.
const GAP: i32 = 16;
/// The narrowest the preview goes in a narrow window, cropped at the sides, so it never holds the
/// window wider than its tile.
const NARROWEST: i32 = 240;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Wallpaper",
        "The living wallpaper behind your windows. Tick more than one and they take turns.",
    );

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

    let turns = group(&page, "Taking turns");
    let chosen: Vec<&str> = store
        .get()
        .wallpaper
        .chosen()
        .iter()
        .map(|variation| variation.id)
        .collect();
    let all = gtk::CheckButton::new();
    let all_sub = row(&turns, "All of them", Some(""), &all);

    // One preview, and beneath it the variations family by family.
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .css_classes(["variations"])
        .build();
    let mut listed: Vec<&'static slipstream_config::Variation> = Vec::new();
    let mut checks: Vec<(&'static str, gtk::CheckButton)> = Vec::new();
    let mut headings: Vec<(usize, &'static str)> = Vec::new();
    for (family, ids) in FAMILIES {
        headings.push((listed.len(), family));
        for id in ids {
            let Some(variation) = slipstream_config::VARIATIONS.iter().find(|v| v.id == *id) else {
                continue;
            };
            let check = gtk::CheckButton::new();
            check.set_active(chosen.contains(&variation.id));
            list.append(&variation_row(variation, &check));
            listed.push(variation);
            checks.push((variation.id, check));
        }
    }
    list.set_header_func(move |row, _| {
        let heading = headings
            .iter()
            .find(|(at, _)| *at == row.index() as usize)
            .map(|(at, family)| {
                let heading = label(&family.to_uppercase(), "group-title");
                heading.set_margin_top(if *at == 0 { 2 } else { 12 });
                heading.set_margin_bottom(4);
                heading.set_margin_start(10);
                heading
            });
        row.set_header(heading.as_ref());
    });
    let viewport = gtk::Viewport::builder()
        .scroll_to_focus(false)
        .child(&list)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(LIST_HEIGHT)
        .max_content_height(LIST_HEIGHT)
        .hexpand(true)
        .child(&viewport)
        .build();
    let picture = previews::picture();
    // Held at its own size: given the card's width it would grow to fill it.
    let (width, height) = previews::SHOWN;
    let holder = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::External)
        .min_content_width(NARROWEST)
        .max_content_width(width)
        .min_content_height(height)
        .max_content_height(height)
        .halign(gtk::Align::Center)
        .child(&picture)
        .build();
    let preview = previews::Preview::new(&picture, store.get().motion.reduced);
    // Stacked until it has been laid out: the tick below only runs once the page is showing, and
    // side by side would hold the whole window wider until then.
    let shelf = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(GAP)
        .css_classes(["row"])
        .build();
    shelf.append(&holder);
    shelf.append(&scroller);
    turns.append(&shelf);
    // Side by side when the list has room beside the preview, otherwise the preview above it.
    shelf.add_tick_callback(|shelf, _| {
        let beside = shelf.width() >= previews::SHOWN.0 + GAP + LIST_BESIDE;
        let orientation = if beside {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        };
        if shelf.orientation() != orientation {
            shelf.set_orientation(orientation);
        }
        gtk::glib::ControlFlow::Continue
    });

    // How far the list is scrolled says which row is picked: the first at the top, the last at
    // the bottom, the rest in between. Picking a row with the keys or the pointer scrolls the
    // list to match. Either way the preview shows it.
    let count = listed.len();
    let scrolling = Rc::new(Cell::new(false));
    let adjustment = scroller.vadjustment();
    {
        let (list, scrolling) = (list.clone(), Rc::clone(&scrolling));
        adjustment.connect_value_changed(move |adjustment| {
            let span = adjustment.upper() - adjustment.page_size();
            if span <= 0.0 || count < 2 {
                return;
            }
            let index = (adjustment.value() / span * (count - 1) as f64).round() as i32;
            if let Some(row) = list.row_at_index(index)
                && !row.is_selected()
            {
                scrolling.set(true);
                list.select_row(Some(&row));
                scrolling.set(false);
            }
        });
    }
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else {
            return;
        };
        let Some(variation) = listed.get(row.index() as usize) else {
            return;
        };
        preview.show(variation.id);
        picture.update_property(&[gtk::accessible::Property::Label(&format!(
            "Preview of {}",
            variation.title
        ))]);
        let span = adjustment.upper() - adjustment.page_size();
        if !scrolling.get() && span > 0.0 && count > 1 {
            adjustment.set_value(span * f64::from(row.index()) / (count - 1) as f64);
        }
    });
    list.select_row(list.row_at_index(0).as_ref());

    let checks = Rc::new(checks);
    // Enter or Space on a row ticks it, as a click on its tick does.
    {
        let checks = Rc::clone(&checks);
        list.connect_row_activated(move |_, row| {
            if let Some((_, check)) = checks.get(row.index() as usize) {
                check.set_active(!check.is_active());
            }
        });
    }

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
                sub.set_label(&all_blurb(ticked, checks.len()));
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
    page.upcast()
}

/// A variation as a row of the list: its tick, then its name and what it does.
fn variation_row(
    variation: &slipstream_config::Variation,
    check: &gtk::CheckButton,
) -> gtk::ListBoxRow {
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    text.append(&label(variation.title, "variation-title"));
    let blurb = label(variation.blurb, "row-sub");
    blurb.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&blurb);
    // The row takes the keyboard, so the tick is only for the pointer.
    check.set_focusable(false);
    let line = gtk::Box::builder()
        .spacing(12)
        .css_classes(["variation"])
        .build();
    line.append(check);
    line.append(&text);
    let row = gtk::ListBoxRow::builder()
        .child(&line)
        .tooltip_text(variation.blurb)
        .build();
    row.update_property(&[
        gtk::accessible::Property::Label(variation.title),
        gtk::accessible::Property::Description(variation.blurb),
    ]);
    let mark = {
        let row = row.clone();
        move |check: &gtk::CheckButton| {
            row.update_state(&[gtk::accessible::State::Checked(if check.is_active() {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
        }
    };
    mark(check);
    check.connect_toggled(mark);
    row
}

/// The ticks after "All of them" is pressed, for `count` variations: every one when it goes on,
/// and the first alone when it goes off, since the set may never be empty.
fn all_ticks(on: bool, count: usize) -> Vec<bool> {
    (0..count).map(|index| on || index == 0).collect()
}

/// What the "All of them" row says, with `ticked` of `count` variations ticked.
fn all_blurb(ticked: usize, count: usize) -> String {
    if ticked == count {
        format!(
            "Every one takes its turn, including any added later. Untick to keep only {}.",
            slipstream_config::VARIATIONS[0].title
        )
    } else if ticked == 1 {
        "One is ticked, so it stays. Tick more below and they take turns.".to_string()
    } else {
        format!("{ticked} of {count} take turns. Tick this to have every one take its turn.")
    }
}

/// The variations in families, each under its own heading in the list. Every
/// variation is in exactly one.
const FAMILIES: [(&str, &[&str]); 3] = [
    (
        "Air and speed",
        &[
            "slipstream",
            "vortex",
            "warp",
            "contours",
            "contrails",
            "tide",
        ],
    ),
    (
        "Nature and mathematics",
        &[
            "attractor",
            "galaxies",
            "physarum",
            "coral",
            "frost",
            "chladni",
            "murmuration",
            "life",
        ],
    ),
    (
        "Machines and signals",
        &["departures", "prompt", "circuit", "sonar", "glitch", "maze"],
    ),
];

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_variation_is_in_one_family() {
        for variation in slipstream_config::VARIATIONS {
            let homes = FAMILIES
                .iter()
                .filter(|(_, ids)| ids.contains(&variation.id))
                .count();
            assert_eq!(homes, 1, "{} is in {homes} families", variation.id);
        }
        let listed: usize = FAMILIES.iter().map(|(_, ids)| ids.len()).sum();
        assert_eq!(listed, slipstream_config::VARIATIONS.len());
    }
}

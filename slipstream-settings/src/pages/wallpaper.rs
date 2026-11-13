//! The wallpaper: the living wallpaper or a picture of your own behind the windows, when the
//! living wallpaper comes in, and which of its variations take turns.

use slipstream_config::{Backdrop, PictureFit};

use super::*;
use crate::{picture_file, previews};

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
        "The living wallpaper or a picture of your own behind your windows. Tick more than one \
         variation to have them take turns.",
    );
    behind(&page, store);

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
            "With more than one ticked, they go in a random order, and the picture changes when \
             the screen is nearly empty.",
        ),
        &change,
    );

    let idle = group(&page, "When you step away");
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
            "Time with no keyboard or mouse input. A fullscreen window, a playing video or \
             Awake (hold Caps Lock) keeps the desktop up.",
        ),
        &fade,
    );
    row(
        &idle,
        "Screenshot the wallpaper",
        Some(
            "Press F7 while the wallpaper is showing, and a screenshot of it is saved to your \
             Screenshots folder. The wallpaper keeps going.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );

    let turns = group(&page, "Living wallpaper");
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

/// The fits offered, in order, with what each is called.
const FITS: [(PictureFit, &str); 2] = [(PictureFit::Fill, "Fill"), (PictureFit::Fit, "Fit")];

/// How big the picture's preview is beside its button.
const THUMB: (i32, i32) = (160, 100);

/// What's behind your windows: the living wallpaper or your own picture, the picture with the
/// button that chooses it, and how it's fitted to the screen.
fn behind(page: &gtk::Box, store: &Store) {
    let card = group(page, "Behind your windows");
    let wallpaper = store.get().wallpaper;
    let living = gtk::CheckButton::new();
    let own = gtk::CheckButton::new();
    own.set_group(Some(&living));
    living.set_active(!wallpaper.shows_picture());
    own.set_active(wallpaper.shows_picture());
    row(
        &card,
        "Living wallpaper",
        Some("Moving pictures that take turns, chosen below."),
        &living,
    );
    row(
        &card,
        "Your own picture",
        Some("A still picture. When you step away, the living wallpaper still comes in."),
        &own,
    );

    let thumb = gtk::Picture::builder()
        .content_fit(content_fit(wallpaper.picture_fit))
        .can_shrink(true)
        .width_request(THUMB.0)
        .height_request(THUMB.1)
        .css_classes(["preview"])
        .alternative_text("Your picture")
        .build();
    thumb.set_overflow(gtk::Overflow::Hidden);
    show_thumb(&thumb);
    let choose = gtk::Button::with_label("Choose picture…");
    choose.set_valign(gtk::Align::Center);
    let control = gtk::Box::builder().spacing(12).build();
    control.append(&thumb);
    control.append(&choose);
    let said = row(&card, "Picture", Some(&named(&wallpaper.picture)), &control)
        .expect("the row has a line of explanation");

    let names: Vec<&str> = FITS.iter().map(|(_, name)| *name).collect();
    let fit = gtk::DropDown::from_strings(&names);
    fit.set_selected(
        FITS.iter()
            .position(|(fit, _)| *fit == wallpaper.picture_fit)
            .unwrap_or(0) as u32,
    );
    {
        let (store, thumb) = (store.clone(), thumb.clone());
        fit.connect_selected_notify(move |fit| {
            if let Some(&(chosen, _)) = FITS.get(fit.selected() as usize) {
                thumb.set_content_fit(content_fit(chosen));
                store.change(|settings| settings.wallpaper.picture_fit = chosen);
            }
        });
    }
    row(
        &card,
        "Fit",
        Some(
            "Fill covers the whole screen and trims what doesn't fit. Fit shows all of the \
             picture, with dark bars where it doesn't reach.",
        ),
        &fit,
    );

    // Ticks set from here aren't taken for a choice of the user's own.
    let settling = Rc::new(Cell::new(false));
    let back_to_living = {
        let (living, settling) = (living.clone(), Rc::clone(&settling));
        move || {
            settling.set(true);
            living.set_active(true);
            settling.set(false);
        }
    };
    // Choosing a picture shows it straight away. Turning away from the dialog, or a file that
    // can't be opened, leaves everything as it was.
    let pick = {
        let (store, own, settling) = (store.clone(), own.clone(), Rc::clone(&settling));
        let (thumb, choose) = (thumb.clone(), choose.clone());
        Rc::new(move || {
            let window = choose.root().and_downcast::<gtk::Window>();
            let (store, own, said, thumb) =
                (store.clone(), own.clone(), said.clone(), thumb.clone());
            let (settling, back_to_living) = (Rc::clone(&settling), back_to_living.clone());
            gtk::glib::spawn_future_local(async move {
                let undo = || {
                    if !store.get().wallpaper.shows_picture() {
                        back_to_living();
                    }
                };
                let images = gtk::FileFilter::new();
                images.set_name(Some("Pictures"));
                images.add_mime_type("image/*");
                let dialog = gtk::FileDialog::builder()
                    .title("Choose a picture")
                    .modal(true)
                    .default_filter(&images)
                    .build();
                let Some(path) = dialog
                    .open_future(window.as_ref())
                    .await
                    .ok()
                    .and_then(|file| file.path())
                else {
                    undo();
                    return;
                };
                said.set_label("Opening…");
                let source = path.clone();
                let prepared = gtk::gio::spawn_blocking(move || {
                    picture_file::prepare(&source, &slipstream_config::picture_path())
                })
                .await
                .unwrap_or_else(|_| Err("it couldn't be read".into()));
                let chosen = path.to_string_lossy().into_owned();
                match prepared {
                    Ok(_) => {
                        // Written even if this is the picture already chosen: its prepared copy
                        // is new, and the desktop only looks again when the file is written.
                        store.change_and_resave(|settings| {
                            settings.wallpaper.show = Backdrop::Picture;
                            settings.wallpaper.picture = chosen.clone();
                        });
                        settling.set(true);
                        own.set_active(true);
                        settling.set(false);
                        said.set_label(&named(&chosen));
                        show_thumb(&thumb);
                    }
                    Err(_) => {
                        said.set_label(&format!(
                            "{} couldn't be opened as a picture, so nothing has changed.",
                            file_name(&chosen)
                        ));
                        undo();
                    }
                }
            });
        })
    };
    {
        let pick = Rc::clone(&pick);
        choose.connect_clicked(move |_| pick());
    }
    living.connect_toggled({
        let (store, settling) = (store.clone(), Rc::clone(&settling));
        move |living| {
            if living.is_active() && !settling.get() {
                store.change(|settings| settings.wallpaper.show = Backdrop::Living);
            }
        }
    });
    // With no picture prepared yet, picking your own picture goes straight to choosing one.
    let store = store.clone();
    own.connect_toggled(move |own| {
        if !own.is_active() || settling.get() {
            return;
        }
        if slipstream_config::picture_path().is_file() {
            store.change(|settings| settings.wallpaper.show = Backdrop::Picture);
        } else {
            pick();
        }
    });
}

/// How the preview fits the picture, the way the desktop will.
fn content_fit(fit: PictureFit) -> gtk::ContentFit {
    match fit {
        PictureFit::Fill => gtk::ContentFit::Cover,
        PictureFit::Fit => gtk::ContentFit::Contain,
    }
}

/// Shows the prepared picture in `thumb`, read off the main thread since it can be large. With
/// none prepared the preview stays dark.
fn show_thumb(thumb: &gtk::Picture) {
    let path = slipstream_config::picture_path();
    if !path.is_file() {
        thumb.set_paintable(None::<&gtk::gdk::Paintable>);
        return;
    }
    let thumb = thumb.downgrade();
    gtk::glib::spawn_future_local(async move {
        let texture =
            gtk::gio::spawn_blocking(move || gtk::gdk::Texture::from_filename(&path).ok())
                .await
                .ok()
                .flatten();
        if let (Some(thumb), Some(texture)) = (thumb.upgrade(), texture) {
            thumb.set_paintable(Some(&texture));
        }
    });
}

/// The last part of a chosen file's path, which is what it's called.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// What the picture row says about the picture chosen.
fn named(picture: &str) -> String {
    if picture.is_empty() {
        "None chosen yet. Most kinds of picture file will do.".to_string()
    } else {
        file_name(picture)
    }
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
            "Every one is used, including any added later. Untick to keep only {}.",
            slipstream_config::VARIATIONS[0].title
        )
    } else if ticked == 1 {
        "Only one is ticked. Tick more below to have them change.".to_string()
    } else {
        format!("{ticked} of {count} ticked. Tick this to use every one.")
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

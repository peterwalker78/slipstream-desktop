//! Shortcuts: whose habits the desktop's keys follow.

use slipstream_config::KeyStyle;

use super::*;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Shortcuts",
        "The shortcuts your hands already know. The shortcut sheet and the tour show the keys of \
         the style chosen here.",
    );
    let styles = group(&page, "Shortcuts");
    let keys = group(&page, "Keys in this style");
    fill_keys(&keys, store);
    let zoom = group(&page, "Accessibility");
    fill_zoom(&zoom, store);
    let (refill_keys, refill_zoom) = (
        refiller(&keys, store, fill_keys),
        refiller(&zoom, store, fill_zoom),
    );
    let refill = move || {
        refill_keys();
        refill_zoom();
    };
    let chosen = store.get().keys.style;
    let mut first: Option<gtk::CheckButton> = None;
    for style in KeyStyle::ALL {
        let pick = gtk::CheckButton::new();
        if let Some(first) = &first {
            pick.set_group(Some(first));
        }
        pick.set_active(style == chosen);
        let (store, refill) = (store.clone(), refill.clone());
        pick.connect_toggled(move |pick| {
            if pick.is_active() {
                store.change(move |settings| settings.keys.style = style);
                refill();
            }
        });
        row(&styles, style.label(), Some(style.summary()), &pick);
        first.get_or_insert(pick);
    }
    fill_app_shortcuts(&group(&page, "Apps' own shortcuts"), store);
    page.upcast()
}

/// Shortcuts apps were allowed to keep, which work whichever window has the keyboard, each of
/// which can be taken away. Built again from the file whenever one is.
fn fill_app_shortcuts(card: &gtk::Box, store: &Store) {
    use slipstream_config::shortcuts;

    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let granted = shortcuts::read(&shortcuts::path());
    let given: Vec<_> = granted
        .apps
        .iter()
        .flat_map(|app| app.shortcuts.iter().map(move |s| (app, s)))
        .collect();
    if given.is_empty() {
        row(
            card,
            "No app has shortcuts of its own",
            Some(
                "An app such as a voice chat can ask for keys that work while you're in another \
                 window. Slipstream asks you first, and what you allow appears here.",
            ),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    let refill = refiller(card, store, fill_app_shortcuts);
    for (app, shortcut) in given {
        let what = if shortcut.description.is_empty() {
            &shortcut.id
        } else {
            &shortcut.description
        };
        let keys = if shortcut.keys.is_empty() {
            "Left out"
        } else {
            &shortcut.keys
        };
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        controls.append(&label(keys, "kv"));
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Take it away, and ask again next time"));
        delete.add_css_class("flat");
        {
            let refill = refill.clone();
            let (app, id) = (app.id.clone(), shortcut.id.clone());
            delete.connect_clicked(move |_| {
                let path = shortcuts::path();
                let mut granted = shortcuts::read(&path);
                if granted.forget(&app, &id)
                    && let Err(err) = shortcuts::write(&path, &granted)
                {
                    eprintln!("slipstream-settings: couldn't take the shortcut away: {err}");
                }
                refill();
            });
        }
        controls.append(&delete);
        row(card, &sentence(what), Some(&app.name), &controls);
    }
}

/// The chosen style's own keys, each beside what it does.
fn fill_keys(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    for (does, keys) in store.get().keys.style.highlights() {
        let keys = if *keys == "Super" {
            "Super, tapped"
        } else {
            keys
        };
        row(card, &sentence(does), None, &label(keys, "kv"));
    }
}

/// The chosen style's keys for zooming the screen.
fn fill_zoom(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let [zoom_in, zoom_out, off] = store.get().keys.style.zoom_keys();
    row(
        card,
        "Zoom",
        Some(
            "Magnifies the screen around the pointer: zoom in, zoom out, and back to normal. \
             Super and the scroll wheel zoom too.",
        ),
        &label(&format!("{zoom_in}   {zoom_out}   {off}"), "kv"),
    );
}

/// `does` as a sentence's start: its first letter a capital.
fn sentence(does: &str) -> String {
    let mut chars = does.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

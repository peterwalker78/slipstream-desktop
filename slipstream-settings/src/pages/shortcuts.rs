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
    page.upcast()
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

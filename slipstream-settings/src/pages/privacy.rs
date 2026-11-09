//! Programs allowed to record the screen without asking.

use super::*;

/// Programs allowed to record the screen without asking, each of which can be taken back. The
/// group shows only once a program has been allowed: Slipstream asks when a program first tries,
/// and this is where an answer is taken back.
pub(super) fn screen_recording(page: &gtk::Box, store: &Store) {
    let card = group(page, "Allowed to record the screen");
    fill_capture_allowed(&card, store);
}

/// The allowed programs, built again from the file whenever one is taken away.
fn fill_capture_allowed(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let allowed = store.get().privacy.screen_capture_allowed;
    // The title above the card goes with it.
    if let Some(group) = card.parent() {
        group.set_visible(!allowed.is_empty());
    }
    let refill = refiller(card, store, fill_capture_allowed);
    for exe in allowed {
        // The program's own name reads as the answer given; the path is what was written down,
        // and two programs of the same name in different places are not the same allowance.
        let name = exe.rsplit('/').next().unwrap_or(&exe).to_string();
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Ask again next time"));
        delete.add_css_class("flat");
        {
            let store = store.clone();
            let refill = refill.clone();
            let exe = exe.clone();
            delete.connect_clicked(move |_| {
                let exe = exe.clone();
                store.change(move |settings| settings.privacy.forget_capture(&exe));
                refill();
            });
        }
        row(card, &name, Some(&exe), &delete);
    }
}

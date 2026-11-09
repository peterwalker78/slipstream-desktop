//! The session: restoring the layout, clipboard history, and programs allowed to record the
//! screen.

use super::*;

/// Restoring the layout, clipboard history, and programs allowed to record the screen.
pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Session",
        "What comes back when you log in, what you've copied, and what programs may do \
         without asking.",
    );
    let restore = group(&page, "Restore");

    let remember = gtk::CheckButton::new();
    remember.set_active(store.get().session.remember);
    row(
        &restore,
        "Remember the layout",
        Some(
            "Saves which apps were open and where, to reopen them at your next login. What each \
             had open is up to the app: a browser restores its tabs, a terminal starts afresh. \
             Turning this off deletes what was saved.",
        ),
        &remember,
    );

    let straight = gtk::CheckButton::new();
    straight.set_active(store.get().session.reopen_without_asking);
    straight.set_sensitive(store.get().session.remember);
    row(
        &restore,
        "Reopen without asking",
        Some("Reopens them at login without asking first."),
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
            "Keeps the last 25 things you copied, to paste again. Forgotten when you log out; \
             passwords are never kept.",
        ),
        &history,
    );

    super::privacy::screen_recording(&page, store);
    page.upcast()
}

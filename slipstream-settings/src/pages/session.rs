//! The session: restoring the layout, locking, the screen going off, clipboard history.

use super::*;

/// Restoring the layout, locking, the screen turning off, and clipboard history.
pub(super) fn build(store: &Store) -> gtk::Widget {
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
            "Saves which apps were open and where, to reopen them at your next login. Turning \
             this off deletes what was saved.",
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

    let note = group(&page, "What it brings back");
    // Said here as well as on the card: the gap between "my windows came back" and "my work came
    // back" is where the disappointment lives.
    row(
        &note,
        "Your apps, not your work",
        Some(
            "Apps reopen where they were, but what they had open is up to each app: a browser \
             restores its tabs, a terminal starts afresh.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
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

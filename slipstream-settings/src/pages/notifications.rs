//! Notifications: pop-ups, their waits and sounds, and do not disturb.

use super::*;

/// The longest a pop-up may wait for a pause in typing, in minutes.
const LONGEST_WAIT_CHOICES: [u64; 6] = [1, 5, 10, 15, 30, 60];

/// Pop-ups: whether they wait while you type and for how long, their sounds, and do not disturb.
pub(super) fn build(store: &Store) -> gtk::Widget {
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
             as one card. The bell counts them straight away, and the notification centre has them \
             at any time. Critical notifications never wait. Only the timing of key presses is \
             used, never which keys.",
        ),
        &wait,
    );

    // The compositor never waits under a minute, whatever the file says.
    let current = store.get().notifications.longest_wait_mins.max(1);
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
             notification centre. Quick settings has the same switch.",
        ),
        &dnd,
    );
    page.upcast()
}

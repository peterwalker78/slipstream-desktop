//! The pages of Settings, holding only the settings Slipstream follows, and the widgets they are
//! built from.

mod about;
mod appearance;
mod keyboard;
mod meter;
mod notifications;
mod power;
mod privacy;
mod screens;
mod session;
mod sound;
mod wallpaper;
mod workspaces;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk::prelude::*;
use gtk4 as gtk;
use slipstream_config::Settings;

use crate::Store;

pub struct Page {
    pub id: &'static str,
    pub title: &'static str,
    /// Icon names; the first one the icon theme has is shown (Breeze's, then Adwaita's).
    pub icons: &'static [&'static str],
    pub build: fn(&Store) -> gtk::Widget,
}

pub const PAGES: [Page; 12] = [
    Page {
        id: "appearance",
        title: "Appearance",
        icons: &[
            "preferences-desktop-theme-global-symbolic",
            "preferences-desktop-appearance-symbolic",
        ],
        build: appearance::build,
    },
    Page {
        id: "wallpaper",
        title: "Wallpaper",
        icons: &[
            "preferences-desktop-wallpaper-symbolic",
            "preferences-desktop-wallpaper",
            "image-x-generic-symbolic",
        ],
        build: wallpaper::build,
    },
    Page {
        id: "screens",
        title: "Screens",
        icons: &[
            "preferences-desktop-display-symbolic",
            "video-display-symbolic",
            "preferences-desktop-display",
        ],
        build: screens::build,
    },
    Page {
        id: "sound",
        title: "Sound",
        icons: &["audio-volume-high-symbolic", "audio-speakers-symbolic"],
        build: sound::build,
    },
    Page {
        id: "power",
        title: "Power",
        icons: &[
            "battery-full-charging-symbolic",
            "battery-good-symbolic",
            "preferences-system-power-symbolic",
        ],
        build: power::build,
    },
    Page {
        id: "notifications",
        title: "Notifications",
        icons: &[
            "preferences-desktop-notification-bell-symbolic",
            "preferences-system-notifications-symbolic",
            "notifications-symbolic",
        ],
        build: notifications::build,
    },
    Page {
        id: "meter",
        title: "Meter",
        icons: &[
            "speedometer-symbolic",
            "xsi-gauge-symbolic",
            "power-profile-performance-symbolic",
        ],
        build: meter::build,
    },
    Page {
        id: "keyboard",
        title: "Keyboard",
        icons: &[
            "input-keyboard-symbolic",
            "preferences-desktop-keyboard-shortcuts-symbolic",
            "preferences-desktop-keyboard-symbolic",
        ],
        build: keyboard::build,
    },
    Page {
        id: "workspaces",
        title: "Workspaces",
        icons: &[
            "view-app-grid-symbolic",
            "preferences-desktop-virtual-symbolic",
            "view-grid-symbolic",
        ],
        build: workspaces::build,
    },
    Page {
        id: "privacy",
        title: "Privacy",
        icons: &[
            "preferences-system-privacy-symbolic",
            "channel-secure-symbolic",
            "security-high-symbolic",
        ],
        build: privacy::build,
    },
    Page {
        id: "session",
        title: "Session",
        icons: &[
            "system-log-out-symbolic",
            "system-shutdown-symbolic",
            "document-open-recent-symbolic",
        ],
        build: session::build,
    },
    Page {
        id: "about",
        title: "About",
        icons: &["help-about-symbolic"],
        build: about::build,
    },
];

pub fn ids() -> Vec<&'static str> {
    PAGES.iter().map(|page| page.id).collect()
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

/// Rebuilds `card` with `fill` once the handler that asked has returned. It holds the card weakly,
/// so the handlers on the card's own rows that call it don't keep the card alive.
fn refiller(
    card: &gtk::Box,
    store: &Store,
    fill: fn(&gtk::Box, &Store),
) -> impl Fn() + Clone + use<> {
    let (card, store) = (card.downgrade(), store.clone());
    move || {
        let (card, store) = (card.clone(), store.clone());
        gtk::glib::idle_add_local_once(move || {
            if let Some(card) = card.upgrade() {
                fill(&card, &store);
            }
        });
    }
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

#[cfg(test)]
mod tests {
    use super::wallpaper::{CHANGE_CHOICES, FADE_CHOICES};
    use super::*;

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
    fn every_page_can_be_found_from_the_app_explorer() {
        let found: Vec<(&str, &str)> = slipstream_config::SETTINGS_PAGES
            .iter()
            .map(|page| (page.id, page.title))
            .collect();
        let shown: Vec<(&str, &str)> = PAGES.iter().map(|page| (page.id, page.title)).collect();
        assert_eq!(found, shown);
    }
}

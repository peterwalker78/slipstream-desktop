//! Notifications once they've arrived (`notify.rs` receives them): the list the notification
//! centre shows, newest first, and the pop-ups at the top right. At most `POPUPS` show at once;
//! each slides in from the right and goes after `POPUP_SHOWN` seconds unless its app asks
//! otherwise. `card` draws them and `arrive` takes them in.
//!
//! Pop-ups that arrive while you're typing, or while the UI is faded, wait. When they can be
//! shown, one comes up on its own and several come up as a single card that opens the centre.
//! Each carries a cross, so one that's in the way goes with a click on it rather than a trip
//! through the notification centre.

mod arrive;
mod card;

use std::{collections::HashMap, path::Path};

use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, icons,
    motion::{HYPR, REDUCED_FADE},
    notify::{self, Image, Incoming, Reason},
    paint::{self, Painted, Painter},
    panel::{self, DESIGN_PX, INK},
    text::{self, Face, Style},
};

pub use card::*;

/// How long a pop-up stays, unless the app asks for longer or shorter.
pub const POPUP_SHOWN: f64 = 4.8;
/// At most this many at once; a new one pushes the oldest off.
const POPUPS: usize = 2;
/// A pop-up held back while the UI was faded shows when it comes back, unless it has waited this
/// long, in wall seconds: by then it's news for the centre, not a ping. One waiting for a pause in
/// typing was waited for with the desktop in front of you, so it's never too old.
const WAIT_LIMIT: f64 = 600.0;
/// The most notifications the centre keeps. Past this the oldest go, as if they had expired, so
/// an app that sends one a second can't grow the list for the life of the session.
const KEPT: usize = 100;

#[derive(Debug, Clone)]
pub struct Notice {
    pub id: u32,
    /// The app's name, from its desktop entry when it names one.
    pub app: String,
    /// The app's ID, lowercased, to find its window.
    pub app_id: Option<String>,
    /// The process whose window it came from, found through the process that sent it. Tried
    /// before `app_id`, since it names the very window rather than any of the app's.
    pub origin: Option<u32>,
    /// Drawn `ICON` design pixels square at the screen's scale.
    pub icon: Option<Pixmap>,
    pub summary: String,
    pub body: String,
    /// When it arrived, as HH:MM.
    pub time: String,
    pub default_action: bool,
    /// Its other actions, identifier and label, as buttons along the bottom of its card.
    pub actions: Vec<(String, String)>,
    /// Stays after it's opened.
    pub resident: bool,
    /// Pops up only, and isn't kept for the notification centre.
    pub transient: bool,
    /// Set by `Notices::add`, so a replaced notification's card is painted again.
    pub version: u64,
    /// Stands for several pop-ups that waited, and opens the centre. Never sent to an app.
    pub digest: bool,
}

struct Popup {
    id: u32,
    /// When it appeared, on the animation clock, and for how long; infinite until dismissed.
    at: f64,
    stay: f64,
}

/// A pop-up waiting for the UI to come back, or for a pause in typing.
struct Waiting {
    id: u32,
    stay: f64,
    /// When it arrived, in wall seconds.
    arrived: f64,
    /// Held for a pause in typing rather than for the faded UI.
    at_pause: bool,
}

/// A waiting pop-up, due to show now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Due {
    pub id: u32,
    pub stay: f64,
    pub at_pause: bool,
}

/// What becomes of a notification's pop-up when it arrives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pop {
    /// Pops up now, for this many seconds (infinite: until it's dismissed).
    Now(f64),
    /// Pops up for this many seconds once the faded UI comes back.
    Later(f64),
    /// Pops up for this many seconds at the next pause in typing.
    AtPause(f64),
    /// Only the centre has it (or nobody, for a transient one).
    Never,
}

impl Pop {
    /// The pop-up for a notification. Do not disturb holds pop-ups back, except critical ones,
    /// and the open centre shows the notification anyway. While the UI has faded, a pop-up's time
    /// starts only when it can be seen, so it waits; a transient one is passing news and isn't
    /// kept for later, and a critical one stays until it's dismissed either way. While you're
    /// `typing`, everything but a critical one waits for the pause, which is seconds away.
    pub fn for_arrival(
        critical: bool,
        transient: bool,
        expire_timeout: i32,
        centre_open: bool,
        do_not_disturb: bool,
        faded: bool,
        typing: bool,
    ) -> Self {
        if centre_open || (do_not_disturb && !critical) {
            return Pop::Never;
        }
        if critical {
            return Pop::Now(f64::INFINITY);
        }
        // An app's own timeout, within reason. Never expiring (0) still pops up for the usual
        // time: the centre keeps the notification for as long as the app wants.
        let stay = if expire_timeout > 0 {
            (expire_timeout as f64 / 1000.0).clamp(2.0, 30.0)
        } else {
            POPUP_SHOWN
        };
        match (faded, transient) {
            (false, _) if typing => Pop::AtPause(stay),
            (false, _) => Pop::Now(stay),
            (true, false) => Pop::Later(stay),
            (true, true) => Pop::Never,
        }
    }
}

#[derive(Default)]
pub struct Notices {
    /// Newest first.
    list: Vec<Notice>,
    unread: usize,
    /// Goes up with every change, so the notification centre knows to repaint.
    revision: u64,
    /// Newest first.
    popups: Vec<Popup>,
    /// Pop-ups held back while the UI was faded, newest first.
    waiting: Vec<Waiting>,
    cards: HashMap<u32, Card>,
    /// Where each pop-up was drawn, in logical pixels from the output's corner.
    hits: Vec<(u32, Rectangle<f64, Logical>)>,
    /// Where each pop-up's close cross was drawn.
    cross_hits: Vec<(u32, Rectangle<f64, Logical>)>,
    /// Where each pop-up's action buttons were drawn: the notification, which action, and where.
    button_hits: Vec<(u32, usize, Rectangle<f64, Logical>)>,
    /// Pop-ups fade in where they rest instead of sliding (`Slipstream::set_reduced_motion`).
    pub reduced_motion: bool,
}

impl Notices {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.cards.clear();
    }

    /// Keeps `notice`, in place of any with the same ID, and pops it up as `pop` says: `now` is
    /// the animation clock, `wall` wall seconds. `unread` counts it for the bell's badge.
    /// Returns the IDs of the oldest notifications it pushed out of a full list.
    pub fn arrive(
        &mut self,
        notice: Notice,
        pop: Pop,
        now: f64,
        wall: f64,
        unread: bool,
    ) -> Vec<u32> {
        let id = notice.id;
        let popup = match pop {
            Pop::Now(stay) => Some(stay),
            Pop::Later(_) | Pop::AtPause(_) | Pop::Never => None,
        };
        let dropped = self.add(notice, now, popup, unread);
        let held = match pop {
            Pop::Later(stay) => Some((stay, false)),
            Pop::AtPause(stay) => Some((stay, true)),
            Pop::Now(_) | Pop::Never => None,
        };
        if let Some((stay, at_pause)) = held
            && self.get(id).is_some()
        {
            self.waiting.insert(
                0,
                Waiting {
                    id,
                    stay,
                    arrived: wall,
                    at_pause,
                },
            );
        }
        dropped
    }

    /// Whether any pop-up is waiting.
    pub fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// When the longest-waiting pop-up held for a pause arrived, in wall seconds.
    pub fn oldest_at_pause(&self) -> Option<f64> {
        self.waiting
            .iter()
            .filter(|waiting| waiting.at_pause)
            .map(|waiting| waiting.arrived)
            .reduce(f64::min)
    }

    /// Every waiting pop-up, newest first, taken off the wait to be shown now. One held for the
    /// faded UI longer than `WAIT_LIMIT` stays only in the centre.
    pub fn take_waiting(&mut self, wall: f64) -> Vec<Due> {
        std::mem::take(&mut self.waiting)
            .into_iter()
            .filter(|waiting| waiting.at_pause || wall - waiting.arrived <= WAIT_LIMIT)
            .map(|waiting| Due {
                id: waiting.id,
                stay: waiting.stay,
                at_pause: waiting.at_pause,
            })
            .collect()
    }

    /// Pops up a notification already kept, for its whole time from `now`.
    pub fn pop_up(&mut self, id: u32, stay: f64, now: f64) {
        if self.get(id).is_none() {
            return;
        }
        self.popups.retain(|shown| shown.id != id);
        self.popups.insert(0, Popup { id, at: now, stay });
        self.popups.truncate(POPUPS);
    }

    /// Keeps `notice`, in place of any with the same ID, and pops it up for `popup` seconds
    /// (infinite: until it's dismissed) when given. `unread` counts it for the bell's badge.
    /// Returns the IDs of the oldest notifications it pushed out of a full list.
    pub fn add(
        &mut self,
        mut notice: Notice,
        now: f64,
        popup: Option<f64>,
        unread: bool,
    ) -> Vec<u32> {
        self.revision += 1;
        notice.version = self.revision;
        let id = notice.id;
        let replacing = self.list.iter().any(|kept| kept.id == id);
        if unread && !replacing && !notice.transient {
            self.unread += 1;
        }
        self.list.retain(|kept| kept.id != id);
        self.list.insert(0, notice);
        self.popups.retain(|shown| shown.id != id);
        self.waiting.retain(|waiting| waiting.id != id);
        if let Some(stay) = popup {
            self.popups.insert(0, Popup { id, at: now, stay });
            self.popups.truncate(POPUPS);
        }
        let mut dropped = Vec::new();
        while self.listed().count() > KEPT {
            let Some(oldest) = self.list.iter().rev().find(|kept| !kept.transient) else {
                break;
            };
            let oldest = oldest.id;
            self.remove(oldest);
            dropped.push(oldest);
        }
        dropped
    }

    pub fn get(&self, id: u32) -> Option<&Notice> {
        self.list.iter().find(|notice| notice.id == id)
    }

    /// Takes a notification away. Returns whether there was one.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.list.len();
        self.list.retain(|notice| notice.id != id);
        self.popups.retain(|popup| popup.id != id);
        self.waiting.retain(|waiting| waiting.id != id);
        self.forget_hits(id);
        let removed = self.list.len() != before;
        if removed {
            self.revision += 1;
        }
        removed
    }

    /// Clear all: every notification the centre lists goes. Returns their IDs.
    pub fn clear(&mut self) -> Vec<u32> {
        let ids = self.ids();
        self.list.retain(|notice| notice.transient);
        self.popups.retain(|popup| !ids.contains(&popup.id));
        self.waiting.clear();
        self.unread = 0;
        self.revision += 1;
        ids
    }

    /// The IDs the notification centre lists, newest first.
    pub fn ids(&self) -> Vec<u32> {
        self.listed().map(|notice| notice.id).collect()
    }

    pub fn listed(&self) -> impl Iterator<Item = &Notice> {
        self.list.iter().filter(|notice| !notice.transient)
    }

    /// Notifications since the centre was last opened that are still there.
    pub fn unread(&self) -> usize {
        self.unread.min(self.listed().count())
    }

    pub fn mark_read(&mut self) {
        self.unread = 0;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Hides every pop-up; the notifications stay in the centre. Returns the transient ones that
    /// went with them.
    pub fn hide_popups(&mut self) -> Vec<u32> {
        self.popups.clear();
        self.waiting.clear();
        self.hits.clear();
        self.cross_hits.clear();
        self.button_hits.clear();
        self.sweep()
    }

    /// Hides one pop-up, as `hide_popups` does.
    pub fn dismiss_popup(&mut self, id: u32) -> Vec<u32> {
        self.popups.retain(|popup| popup.id != id);
        self.forget_hits(id);
        self.sweep()
    }

    /// Drops what a pop-up left behind, so a click where it was doesn't land on it after it has
    /// gone and before the next frame lays the pop-ups out again.
    fn forget_hits(&mut self, id: u32) {
        self.hits.retain(|(hit, _)| *hit != id);
        self.cross_hits.retain(|(hit, _)| *hit != id);
        self.button_hits.retain(|(hit, _, _)| *hit != id);
    }

    /// The close cross under `pointer`, in the space's coordinates, on pop-ups drawn on the
    /// screen whose corner is at `origin`: the notification it closes.
    pub fn cross_hit_from(&self, pointer: (f64, f64), origin: (f64, f64)) -> Option<u32> {
        popup_hit(pointer, origin, &self.cross_hits)
    }

    /// The action button under `pointer`, in the space's coordinates, on pop-ups drawn on the
    /// screen whose corner is at `origin`: the notification and which of its actions.
    pub fn button_hit_from(&self, pointer: (f64, f64), origin: (f64, f64)) -> Option<(u32, usize)> {
        let local = (pointer.0 - origin.0, pointer.1 - origin.1);
        self.button_hits
            .iter()
            .find(|(_, _, area)| area.contains(local))
            .map(|(id, index, _)| (*id, *index))
    }

    /// The pop-up under `pointer`, in the space's coordinates, for pop-ups drawn on the screen
    /// whose corner is at `origin`.
    pub fn popup_hit_from(&self, pointer: (f64, f64), origin: (f64, f64)) -> Option<u32> {
        popup_hit(pointer, origin, &self.hits)
    }

    /// Drops transient notifications that no longer pop up or wait to, and returns the IDs apps
    /// gave them (a digest is Slipstream's own, so it isn't among them).
    pub fn sweep(&mut self) -> Vec<u32> {
        let gone: Vec<(u32, bool)> = self
            .list
            .iter()
            .filter(|notice| {
                notice.transient
                    && !self.popups.iter().any(|popup| popup.id == notice.id)
                    && !self.waiting.iter().any(|waiting| waiting.id == notice.id)
            })
            .map(|notice| (notice.id, notice.digest))
            .collect();
        if !gone.is_empty() {
            self.list
                .retain(|notice| !gone.iter().any(|(id, _)| *id == notice.id));
            self.revision += 1;
        }
        gone.into_iter()
            .filter(|(_, digest)| !digest)
            .map(|(id, _)| id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn notice(id: u32) -> Notice {
        Notice {
            id,
            app: "Mail".into(),
            app_id: None,
            origin: None,
            icon: None,
            summary: format!("Message {id}"),
            body: "Are we still on for the walk at 10 tomorrow?".into(),
            time: "08:47".into(),
            default_action: true,
            actions: Vec::new(),
            resident: false,
            transient: false,
            version: 0,
            digest: false,
        }
    }

    #[test]
    fn newest_first_replacing_in_place_and_counting_unread_once() {
        let mut notices = Notices::default();
        notices.add(notice(1), 0.0, Some(POPUP_SHOWN), true);
        notices.add(notice(2), 0.0, Some(POPUP_SHOWN), true);
        notices.add(notice(1), 1.0, Some(POPUP_SHOWN), true);
        assert_eq!(notices.ids(), [1, 2], "a replacement moves to the top");
        assert_eq!(notices.unread(), 2);
        notices.mark_read();
        assert_eq!(notices.unread(), 0);
        assert!(notices.remove(2));
        assert!(!notices.remove(2));
        assert_eq!(notices.clear(), [1]);
        assert!(notices.ids().is_empty());
    }

    #[test]
    fn at_most_two_pop_ups_and_transient_ones_go_with_theirs() {
        let mut notices = Notices::default();
        notices.add(notice(1), 0.0, Some(POPUP_SHOWN), true);
        notices.add(notice(2), 0.0, Some(POPUP_SHOWN), true);
        let passing = Notice {
            transient: true,
            ..notice(3)
        };
        notices.add(passing, 0.0, Some(POPUP_SHOWN), true);
        assert_eq!(notices.popups.len(), 2);
        assert_eq!(notices.unread(), 2, "a transient one isn't counted");
        assert_eq!(notices.ids(), [2, 1], "nor listed");
        assert_eq!(notices.hide_popups(), [3]);
        assert!(notices.get(3).is_none());
        assert_eq!(notices.ids(), [2, 1], "the rest stay for the centre");
    }

    #[test]
    fn a_popup_waits_for_the_ui() {
        let mut notices = Notices::default();
        let waiting = Pop::for_arrival(false, false, -1, false, false, true, false);
        assert_eq!(waiting, Pop::Later(POPUP_SHOWN));
        notices.arrive(notice(1), waiting, 10.0, 10.0, true);
        notices.arrive(notice(2), Pop::Later(8.0), 20.0, 20.0, true);
        assert!(
            notices.popups.is_empty(),
            "nothing pops up while the UI is faded"
        );
        assert_eq!(notices.unread(), 2, "the bell still counts them");
        assert_eq!(notices.oldest_at_pause(), None);
        let due = notices.take_waiting(100.0);
        assert_eq!(
            due.iter().map(|due| (due.id, due.stay)).collect::<Vec<_>>(),
            [(2, 8.0), (1, POPUP_SHOWN)],
            "newest first, each with its own time"
        );
        assert!(!notices.has_waiting());
        notices.pop_up(2, 8.0, 100.0);
        assert_eq!(
            notices
                .popups
                .iter()
                .map(|popup| (popup.id, popup.at, popup.stay))
                .collect::<Vec<_>>(),
            [(2, 100.0, 8.0)],
            "its whole time from the moment it can be seen"
        );

        // Ten minutes on, a ping is old news: the centre keeps it, and nothing is due.
        notices.hide_popups();
        notices.arrive(notice(4), Pop::Later(POPUP_SHOWN), 200.0, 200.0, true);
        assert!(notices.take_waiting(900.0).is_empty());
        assert_eq!(notices.ids()[0], 4);

        // An app that takes its notification back takes the wait with it.
        notices.arrive(notice(5), Pop::Later(POPUP_SHOWN), 950.0, 950.0, true);
        notices.remove(5);
        assert!(notices.take_waiting(960.0).is_empty());
    }

    #[test]
    fn typing_holds_popups_for_the_pause_however_long_it_takes() {
        assert_eq!(
            Pop::for_arrival(false, false, -1, false, false, false, true),
            Pop::AtPause(POPUP_SHOWN)
        );
        assert_eq!(
            Pop::for_arrival(false, true, -1, false, false, false, true),
            Pop::AtPause(POPUP_SHOWN),
            "passing news waits a few seconds too, rather than being lost"
        );
        assert_eq!(
            Pop::for_arrival(true, false, -1, false, false, false, true),
            Pop::Now(f64::INFINITY),
            "critical ones never wait"
        );
        assert_eq!(
            Pop::for_arrival(false, false, -1, false, false, true, true),
            Pop::Later(POPUP_SHOWN),
            "faded, it waits for the UI"
        );

        let mut notices = Notices::default();
        let passing = Notice {
            transient: true,
            ..notice(1)
        };
        notices.arrive(passing, Pop::AtPause(POPUP_SHOWN), 0.0, 10.0, true);
        assert!(notices.sweep().is_empty(), "a waiting one isn't swept");
        notices.arrive(notice(2), Pop::AtPause(POPUP_SHOWN), 0.0, 20.0, true);
        assert_eq!(notices.oldest_at_pause(), Some(10.0));
        let due = notices.take_waiting(10_000.0);
        assert_eq!(due.len(), 2, "held for a pause, nothing goes stale");
        assert!(due.iter().all(|due| due.at_pause));
    }

    #[test]
    fn a_digest_goes_quietly_when_its_popup_does() {
        let mut notices = Notices::default();
        let digest = Notice {
            transient: true,
            digest: true,
            ..notice(9)
        };
        notices.add(digest, 0.0, Some(POPUP_SHOWN), false);
        assert_eq!(notices.unread(), 0);
        assert!(notices.ids().is_empty(), "the centre doesn't list it");
        assert!(
            notices.dismiss_popup(9).is_empty(),
            "no app is told a digest closed"
        );
        assert!(notices.get(9).is_none());
    }

    #[test]
    fn a_transient_one_arriving_faded_is_not_popped() {
        assert_eq!(
            Pop::for_arrival(false, true, -1, false, false, true, false),
            Pop::Never
        );
        assert_eq!(
            Pop::for_arrival(false, true, -1, false, false, false, false),
            Pop::Now(POPUP_SHOWN),
            "awake, it pops up as always"
        );
        assert_eq!(
            Pop::for_arrival(true, false, -1, false, false, true, false),
            Pop::Now(f64::INFINITY),
            "critical ones stay until dismissed, faded or not"
        );
        assert_eq!(
            Pop::for_arrival(false, false, 0, false, false, false, false),
            Pop::Now(POPUP_SHOWN),
            "never expiring still pops up for the usual time"
        );
        assert_eq!(
            Pop::for_arrival(false, false, 12_000, false, false, true, false),
            Pop::Later(12.0)
        );
        assert_eq!(
            Pop::for_arrival(false, false, -1, true, false, false, false),
            Pop::Never
        );
        assert_eq!(
            Pop::for_arrival(false, false, -1, false, true, true, false),
            Pop::Never
        );
        assert_eq!(
            Pop::for_arrival(true, false, -1, false, true, false, false),
            Pop::Now(f64::INFINITY)
        );

        let mut notices = Notices::default();
        let passing = Notice {
            transient: true,
            ..notice(1)
        };
        notices.arrive(passing, Pop::Never, 0.0, 0.0, true);
        assert_eq!(notices.sweep(), [1], "nothing holds it, so it goes");
    }

    #[test]
    fn the_list_keeps_a_hundred() {
        let mut notices = Notices::default();
        let mut closed = Vec::new();
        for id in 1..=150 {
            closed.extend(notices.add(notice(id), id as f64, Some(POPUP_SHOWN), true));
        }
        assert_eq!(notices.ids().len(), 100);
        assert_eq!(notices.ids().first(), Some(&150), "the newest stay");
        assert_eq!(
            closed,
            (1..=50).collect::<Vec<u32>>(),
            "the oldest go, each said once"
        );
        assert!(notices.get(50).is_none());
        // Transient ones are only passing, so they neither count nor push anything out.
        let passing = Notice {
            transient: true,
            ..notice(151)
        };
        assert!(
            notices
                .add(passing, 151.0, Some(POPUP_SHOWN), true)
                .is_empty()
        );
        assert_eq!(notices.ids().len(), 100);
    }

    #[test]
    fn cards_grow_with_the_body_up_to_their_line_limit() {
        let short = Notice {
            body: String::new(),
            ..notice(1)
        };
        let long = Notice {
            body: "word ".repeat(200),
            ..notice(2)
        };
        let short_h = card_height(&short, POPUP_W, 3);
        assert!(short_h >= 2.0 * PAD_Y + ICON, "at least the icon's height");
        assert_eq!(short_h, 2.0 * PAD_Y + META_H + 1.0 + TITLE_H);
        assert_eq!(
            card_height(&long, POPUP_W, 3),
            2.0 * PAD_Y + META_H + 1.0 + TITLE_H + 2.0 + 3.0 * LINE_H
        );
    }

    #[test]
    fn a_pop_up_carries_a_cross_inside_its_own_corner() {
        let card = paint_popup(&notice(1), 1.25).expect("a painted pop-up");
        let [x, y, w, h] = card.cross.expect("a cross to close it with");
        // Its box is the card's top right corner, in design pixels from the painted area, which
        // holds the shadow as well.
        assert!(
            x > SHADOW + POPUP_W / 2.0 && x + w <= SHADOW + POPUP_W,
            "{x} {w}"
        );
        assert!(y >= SHADOW && y + h <= SHADOW + card.height, "{y} {h}");
    }

    #[test]
    fn a_pop_ups_cross_is_where_the_card_comes_to_rest() {
        let mut notices = Notices::default();
        notices.add(notice(4), 0.0, Some(POPUP_SHOWN), true);
        // `popup_elements` needs a renderer, so lay the hit out the way it does and check the
        // cross falls inside the pop-up rather than beside it.
        let card = paint_popup(&notice(4), 1.0).expect("a painted pop-up");
        let [cx, cy, cw, ch] = card.cross.expect("a cross");
        let left = 1536.0 / DESIGN_PX - POPUP_RIGHT - POPUP_W;
        let cross = Rectangle::new(
            (
                ((left + cx - SHADOW) * DESIGN_PX) as f64,
                ((POPUP_TOP + cy - SHADOW) * DESIGN_PX) as f64,
            )
                .into(),
            ((cw * DESIGN_PX) as f64, (ch * DESIGN_PX) as f64).into(),
        );
        let popup = Rectangle::<f64, Logical>::new(
            ((left * DESIGN_PX) as f64, (POPUP_TOP * DESIGN_PX) as f64).into(),
            (
                (POPUP_W * DESIGN_PX) as f64,
                (card.height * DESIGN_PX) as f64,
            )
                .into(),
        );
        let centre = (
            cross.loc.x + cross.size.w / 2.0,
            cross.loc.y + cross.size.h / 2.0,
        );
        assert!(
            popup.contains(Point::from(centre)),
            "{cross:?} in {popup:?}"
        );
        assert_eq!(
            popup_hit(centre, (0.0, 0.0), &[(4, cross)]),
            Some(4),
            "a click on the cross finds the notification"
        );
    }

    #[test]
    fn pop_ups_are_hit_from_the_screen_they_were_drawn_on() {
        let hits = vec![(
            7,
            Rectangle::new((1100.0, 50.0).into(), (400.0, 90.0).into()),
        )];
        // Drawn on the left screen.
        assert_eq!(popup_hit((1200.0, 80.0), (0.0, 0.0), &hits), Some(7));
        assert_eq!(popup_hit((1536.0 + 1200.0, 80.0), (0.0, 0.0), &hits), None);
        // Drawn on a screen to its right: the same spot on the left screen misses.
        assert_eq!(
            popup_hit((1536.0 + 1200.0, 80.0), (1536.0, 0.0), &hits),
            Some(7)
        );
        assert_eq!(popup_hit((1200.0, 80.0), (1536.0, 0.0), &hits), None);
    }
}

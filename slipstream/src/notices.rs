//! Notifications once they've arrived (`notify.rs` receives them): the list the notification
//! centre shows, newest first, and the pop-ups at the top right, as the mockup's `#popups`. At most
//! two show at once; each slides in from 40 px to the right over 300 ms and goes after 4.8 s.
//!
//! Pop-ups that arrive while you're typing, or while the UI is faded, wait. When they can be
//! shown, one comes up on its own and several come up as a single card that opens the centre.

use std::{collections::HashMap, path::Path};

use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, icons,
    motion::HYPR,
    notify::{self, Image, Incoming, Reason},
    paint::{self, Painted, Painter},
    panel::{INK, MOCKUP_PX},
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
pub const ICON: f32 = 36.0;
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 12.0;
const META_H: f32 = 16.0;
const TITLE_H: f32 = 20.0;
const LINE_H: f32 = 18.0;
const POPUP_W: f32 = 440.0;
const POPUP_RIGHT: f32 = 14.0;
const POPUP_TOP: f32 = 54.0;
const POPUP_GAP: f32 = 10.0;
const POPUP_LINES: usize = 3;
/// Room around a pop-up for its shadow.
const SHADOW: f32 = 48.0;
const POP_IN: f64 = 0.3;
const REDUCED_FADE: f64 = 0.08;
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
    /// Drawn `ICON` mockup pixels square at the screen's scale.
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

struct Card {
    version: u64,
    scale: f64,
    height: f32,
    painted: Painted,
    /// Its action buttons, in mockup pixels from the painted area's corner.
    buttons: Vec<[f32; 4]>,
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
        self.hits.retain(|(hit, _)| *hit != id);
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
        self.sweep()
    }

    /// Hides one pop-up, as `hide_popups` does.
    pub fn dismiss_popup(&mut self, id: u32) -> Vec<u32> {
        self.popups.retain(|popup| popup.id != id);
        self.hits.retain(|(hit, _)| *hit != id);
        self.sweep()
    }

    /// The pop-up at a point, in logical pixels from the output's corner.
    pub fn popup_at(&self, x: f64, y: f64) -> Option<u32> {
        popup_hit((x, y), (0.0, 0.0), &self.hits)
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

    /// The pop-ups for a screen `width` logical pixels wide, and the transient notifications that
    /// expired since the last frame.
    pub fn popup_elements<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
        alpha: f32,
    ) -> (Vec<MemoryRenderBufferRenderElement<R>>, Vec<u32>)
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let before = self.popups.len();
        self.popups.retain(|popup| now - popup.at < popup.stay);
        let expired = if self.popups.len() != before {
            self.sweep()
        } else {
            Vec::new()
        };
        self.hits.clear();
        self.button_hits.clear();
        let left = width as f32 / MOCKUP_PX - POPUP_RIGHT - POPUP_W;
        let mut top = POPUP_TOP;
        let mut elements = Vec::new();
        let Self {
            list,
            popups,
            cards,
            hits,
            button_hits,
            reduced_motion,
            ..
        } = self;
        for popup in popups.iter() {
            let Some(notice) = list.iter().find(|notice| notice.id == popup.id) else {
                continue;
            };
            let fresh = cards
                .get(&popup.id)
                .is_some_and(|card| card.version == notice.version && card.scale == scale);
            if !fresh {
                let Some(card) = paint_popup(notice, scale) else {
                    continue;
                };
                cards.insert(popup.id, card);
            }
            let card = &cards[&popup.id];
            let since = now - popup.at;
            let (fade, slide) = if *reduced_motion {
                ((since / REDUCED_FADE).clamp(0.0, 1.0), 0.0)
            } else {
                let eased = HYPR.at((since / POP_IN).clamp(0.0, 1.0));
                (eased.clamp(0.0, 1.0), 40.0 * (1.0 - eased))
            };
            let at = Point::from((
                ((left - SHADOW) as f64 + slide) * MOCKUP_PX as f64,
                ((top - SHADOW) * MOCKUP_PX) as f64,
            ));
            elements.extend(card.painted.element(renderer, at, fade as f32 * alpha));
            hits.push((
                popup.id,
                Rectangle::new(
                    ((left * MOCKUP_PX) as f64, (top * MOCKUP_PX) as f64).into(),
                    (
                        (POPUP_W * MOCKUP_PX) as f64,
                        (card.height * MOCKUP_PX) as f64,
                    )
                        .into(),
                ),
            ));
            for (index, [bx, by, bw, bh]) in card.buttons.iter().enumerate() {
                button_hits.push((
                    popup.id,
                    index,
                    Rectangle::new(
                        (
                            ((left + bx - SHADOW) * MOCKUP_PX) as f64,
                            ((top + by - SHADOW) * MOCKUP_PX) as f64,
                        )
                            .into(),
                        ((bw * MOCKUP_PX) as f64, (bh * MOCKUP_PX) as f64).into(),
                    ),
                ));
            }
            top += card.height + POPUP_GAP;
        }
        cards.retain(|id, _| popups.iter().any(|popup| popup.id == *id));
        (elements, expired)
    }
}

fn paint_popup(notice: &Notice, scale: f64) -> Option<Card> {
    let height = card_height(notice, POPUP_W, POPUP_LINES);
    let mut buttons = Vec::new();
    let logical = Size::<i32, Logical>::from((
        ((POPUP_W + 2.0 * SHADOW) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * SHADOW) * MOCKUP_PX).ceil() as i32,
    ));
    let painted = Painted::new(logical, scale, |p| {
        p.f *= MOCKUP_PX;
        crate::panel::notice(p, SHADOW, SHADOW, POPUP_W, height);
        let hits = paint_card(p, notice, SHADOW, SHADOW, POPUP_W, POPUP_LINES, false);
        buttons = hits.buttons;
    })?;
    Some(Card {
        version: notice.version,
        scale,
        height,
        painted,
        buttons,
    })
}

fn body_style() -> Style {
    Style::new(Face::Body, 14.0, 0xbcc3cfff)
}

/// The body in lines no wider than `width`, at most `max`; the last is cut short if there's more.
/// Its own line breaks are kept.
fn body_lines(notice: &Notice, width: f32, max: usize) -> Vec<String> {
    let style = body_style();
    let mut lines: Vec<String> = notice
        .body
        .lines()
        .flat_map(|line| text::wrap(line, &style, width))
        .collect();
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.pop() {
            lines.push(text::ellipsize(
                &format!("{}…", last.trim_end()),
                &style,
                width,
            ));
        }
    }
    lines
}

/// How tall a notification's card is, `width` wide with at most `max_lines` of body.
pub fn card_height(notice: &Notice, width: f32, max_lines: usize) -> f32 {
    let text_w = width - 2.0 * PAD_X - ICON - 12.0;
    let lines = body_lines(notice, text_w, max_lines).len();
    let body_h = if lines > 0 {
        2.0 + lines as f32 * LINE_H
    } else {
        0.0
    };
    let buttons = if notice.actions.is_empty() {
        0.0
    } else {
        BUTTON_GAP + BUTTON_H
    };
    2.0 * PAD_Y + (META_H + 1.0 + TITLE_H + body_h).max(ICON) + buttons
}

/// The action buttons along the bottom of a card.
const BUTTON_H: f32 = 28.0;
const BUTTON_GAP: f32 = 10.0;

/// Where a card's controls were painted, each box as x, y, w, h: its cross, and its action
/// buttons in order.
#[derive(Debug, Default)]
pub struct CardHits {
    pub cross: Option<[f32; 4]>,
    pub buttons: Vec<[f32; 4]>,
}

/// Paints a card's contents (not its background) with its corner at (`x`, `y`): the app's icon,
/// its name and the time, the summary and the body, and its action buttons. With `dismiss`, a
/// cross after the time. Says where the cross and the buttons are.
pub fn paint_card(
    p: &mut Painter,
    notice: &Notice,
    x: f32,
    y: f32,
    width: f32,
    max_lines: usize,
    dismiss: bool,
) -> CardHits {
    match &notice.icon {
        Some(icon) => p.image(icon, x + PAD_X, y + PAD_Y),
        None => placeholder(p, &notice.app, x + PAD_X, y + PAD_Y),
    }
    let text_x = x + PAD_X + ICON + 12.0;
    let text_w = width - (text_x - x) - PAD_X;
    let meta_centre = y + PAD_Y + META_H / 2.0;
    let meta = Style::new(Face::Body, 12.5, 0x8f98a8ff);
    let mut right = x + width - PAD_X;
    let mut cross = None;
    if dismiss {
        right -= 14.0;
        p.icon(
            icons::CLOSE,
            right,
            meta_centre - 7.0,
            14.0,
            Some(0x7d8697ff),
        );
        cross = Some([right - 8.0, y + PAD_Y - 6.0, 30.0, META_H + 12.0]);
        right -= 10.0;
    }
    right -= text::width(&notice.time, &meta);
    p.text(&notice.time, right, meta_centre, &meta);
    p.text(
        &text::ellipsize(&notice.app, &meta, right - 10.0 - text_x),
        text_x,
        meta_centre,
        &meta,
    );
    let title = Style::new(Face::BodyBold, 15.0, INK);
    p.text(
        &text::ellipsize(&notice.summary, &title, text_w),
        text_x,
        y + PAD_Y + META_H + 1.0 + TITLE_H / 2.0,
        &title,
    );
    let body = body_style();
    let first = y + PAD_Y + META_H + 1.0 + TITLE_H + 2.0 + LINE_H / 2.0;
    for (i, line) in body_lines(notice, text_w, max_lines).iter().enumerate() {
        p.text(line, text_x, first + i as f32 * LINE_H, &body);
    }
    let mut buttons = Vec::new();
    if !notice.actions.is_empty() {
        let height = card_height(notice, width, max_lines);
        let top = y + height - PAD_Y - BUTTON_H;
        let label = Style::new(Face::Body, 13.5, INK);
        let mut bx = text_x;
        for (_, text) in &notice.actions {
            let w = (text::width(text, &label) + 24.0).min(x + width - PAD_X - bx);
            if w < 40.0 {
                break;
            }
            p.fill(bx, top, w, BUTTON_H, 7.0, 0xffffff14);
            p.border(bx, top, w, BUTTON_H, 7.0, 1.0, 0xffffff24);
            p.text(
                &text::ellipsize(text, &label, w - 24.0),
                bx + 12.0,
                top + BUTTON_H / 2.0,
                &label,
            );
            buttons.push([bx, top, w, BUTTON_H]);
            bx += w + 8.0;
        }
    }
    CardHits { cross, buttons }
}

/// An app with no icon gets a coloured square with its initial, as the explorer draws one.
fn placeholder(p: &mut Painter, name: &str, x: f32, y: f32) {
    const COLOURS: [u32; 6] = [
        0x3cf0c0ff, 0x33ccffff, 0xffb547ff, 0xff7a93ff, 0xa78bfaff, 0x7fe3ffff,
    ];
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    p.fill(
        x,
        y,
        ICON,
        ICON,
        9.0,
        COLOURS[hash as usize % COLOURS.len()],
    );
    let initial: String = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_default();
    let style = Style::new(Face::MonoBold, 17.0, 0x10131aff);
    let w = text::width(&initial, &style);
    p.text(&initial, x + (ICON - w) / 2.0, y + ICON / 2.0, &style);
}

/// A picture sent as pixels, drawn into a `px`-pixel square keeping its proportions.
fn picture(image: &Image, px: u32) -> Option<Pixmap> {
    let mut source = Pixmap::new(image.width, image.height)?;
    for (dst, src) in source
        .data_mut()
        .chunks_exact_mut(4)
        .zip(image.rgba.chunks_exact(4))
    {
        let alpha = src[3] as u16;
        let premultiply = |channel: u8| ((channel as u16 * alpha + 127) / 255) as u8;
        dst.copy_from_slice(&[
            premultiply(src[0]),
            premultiply(src[1]),
            premultiply(src[2]),
            src[3],
        ]);
    }
    let mut out = Pixmap::new(px, px)?;
    let (w, h, side) = (image.width as f32, image.height as f32, px as f32);
    let k = side / w.max(h);
    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    let transform = Transform::from_row(k, 0.0, 0.0, k, (side - w * k) / 2.0, (side - h * k) / 2.0);
    out.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
    Some(out)
}

impl Slipstream {
    pub fn notification_event(&mut self, event: notify::Event) {
        match event {
            notify::Event::Notify(incoming) => self.notification_arrived(*incoming),
            notify::Event::Close(id) => {
                if self.notices.remove(id) {
                    notify::closed(vec![id], Reason::Closed);
                }
            }
        }
    }

    pub fn notification_arrived(&mut self, incoming: Incoming) {
        let scale = self.output_scale().unwrap_or(1.0);
        let px = (ICON as f64 * MOCKUP_PX as f64 * scale).round() as u32;
        let icon = self.notice_icon(&incoming, px);
        let named = |name: &str| Some(name.to_string()).filter(|name| !name.is_empty());
        let app = incoming
            .desktop_entry
            .as_deref()
            .and_then(|entry| self.explorer.app_name(entry))
            .or_else(|| named(&incoming.app_name))
            .unwrap_or_else(|| "Notification".to_string());
        let app_id = incoming
            .desktop_entry
            .clone()
            .or_else(|| named(&incoming.app_name))
            .map(|id| id.to_lowercase());
        let critical = incoming.urgency >= 2;
        let pop = Pop::for_arrival(
            critical,
            incoming.transient,
            incoming.expire_timeout,
            self.centre.is_open(),
            self.settings.notifications.do_not_disturb,
            // Nothing pops up over the lock; it waits for the desktop to come back.
            self.idle.is_faded() || self.lock.is_some(),
            self.settings.notifications.wait_while_typing
                && self.concentration.typing(std::time::Instant::now()),
        );
        let notice = Notice {
            id: incoming.id,
            app,
            app_id,
            icon,
            summary: notify::plain(&incoming.summary),
            body: notify::plain(&incoming.body),
            time: self.status.lock().unwrap().time.clone(),
            default_action: incoming.default_action,
            actions: incoming.actions.clone(),
            resident: incoming.resident,
            transient: incoming.transient,
            version: 0,
            digest: false,
        };
        tracing::info!(id = notice.id, app = notice.app, ?pop, "notification");
        let now = self.clock.tick();
        let wall = self.wall();
        let unread = !self.centre.is_open();
        let dropped = self.notices.arrive(notice, pop, now, wall, unread);
        notify::closed(dropped, Reason::Expired);
        let gone = self.notices.sweep();
        notify::closed(gone, Reason::Expired);
    }

    /// A notification's icon: the picture it sent, else its image or icon by name or file, else
    /// its app's icon.
    fn notice_icon(&mut self, incoming: &Incoming, px: u32) -> Option<Pixmap> {
        if let Some(found) = incoming.image.as_ref().and_then(|image| picture(image, px)) {
            return Some(found);
        }
        let sources = [incoming.image_path.as_deref(), Some(&incoming.app_icon)];
        for source in sources.into_iter().flatten().filter(|s| !s.is_empty()) {
            let source = source.strip_prefix("file://").unwrap_or(source);
            let found = if source.starts_with('/') {
                paint::load_image(Path::new(source), px)
            } else {
                self.explorer.named_icon(source, px)
            };
            if found.is_some() {
                return found;
            }
        }
        [
            incoming.desktop_entry.clone(),
            Some(incoming.app_name.to_lowercase()),
        ]
        .into_iter()
        .flatten()
        .find_map(|id| self.explorer.app_icon(&id, px))
    }

    /// Waiting pop-ups that can show now: the UI is in view, and you've paused typing or they
    /// have waited as long as the settings allow. One shows as itself; several as one card.
    pub fn show_waiting_popups(&mut self, now: f64) {
        if self.idle.is_faded() || self.lock.is_some() || !self.notices.has_waiting() {
            return;
        }
        let wall = self.wall();
        let settings = &self.settings.notifications;
        let typing =
            settings.wait_while_typing && self.concentration.typing(std::time::Instant::now());
        let overdue = self
            .notices
            .oldest_at_pause()
            .is_some_and(|arrived| wall - arrived >= settings.longest_wait_secs());
        if typing && !overdue {
            return;
        }
        let due = self.notices.take_waiting(wall);
        tracing::info!(count = due.len(), overdue, "waiting pop-ups shown");
        match due.as_slice() {
            [] => {}
            [one] => self.notices.pop_up(one.id, one.stay, now),
            several => {
                let digest = self.digest(several);
                let stay = several
                    .iter()
                    .map(|due| due.stay)
                    .fold(POPUP_SHOWN, f64::max);
                self.notices.add(digest, now, Some(stay), false);
            }
        }
    }

    /// One card for several pop-ups that waited: how many, and the newest few by app and title.
    fn digest(&mut self, due: &[Due]) -> Notice {
        let kept: Vec<&Notice> = due
            .iter()
            .filter_map(|due| self.notices.get(due.id))
            .collect();
        let reason = if due.iter().all(|due| due.at_pause) {
            "while you were typing"
        } else {
            "while you were away"
        };
        let line = |notice: &Notice| {
            if notice.summary.is_empty() {
                notice.app.clone()
            } else {
                format!("{}: {}", notice.app, notice.summary)
            }
        };
        let mut lines: Vec<String> = if kept.len() > POPUP_LINES {
            let mut first: Vec<String> =
                kept.iter().take(POPUP_LINES - 1).map(|n| line(n)).collect();
            first.push(format!("and {} more", kept.len() - (POPUP_LINES - 1)));
            first
        } else {
            kept.iter().map(|n| line(n)).collect()
        };
        lines.retain(|line| !line.trim().is_empty());
        let scale = self.output_scale().unwrap_or(1.0);
        let px = (ICON as f64 * MOCKUP_PX as f64 * scale).round() as u32;
        let icon = Pixmap::new(px, px).map(|mut pixmap| {
            icons::draw(&mut pixmap, icons::LOGO, 0.0, 0.0, px as f32, None);
            pixmap
        });
        Notice {
            id: notify::next_id(),
            app: "Slipstream".into(),
            app_id: None,
            icon,
            summary: format!("{} {reason}", due.len()),
            body: lines.join("\n"),
            time: kept.first().map(|n| n.time.clone()).unwrap_or_default(),
            default_action: false,
            actions: Vec::new(),
            resident: false,
            transient: true,
            version: 0,
            digest: true,
        }
    }

    /// A notification clicked, in a pop-up or the centre: its app hears, its window comes
    /// forward, and it goes unless it asked to stay. A digest opens the centre, which lists what
    /// it stands for.
    pub fn open_notice(&mut self, id: u32) {
        let Some(notice) = self.notices.get(id).cloned() else {
            return;
        };
        if notice.digest {
            if !self.centre.is_open() {
                self.toggle_notification_centre();
            }
            return;
        }
        tracing::info!(id, app = notice.app, "notification opened");
        if notice.default_action {
            notify::invoked(id, "default");
        }
        let window = notice
            .app_id
            .as_deref()
            .and_then(|app| self.window_for_app(app));
        if let Some(window) = window {
            self.jump_to(&window);
        }
        if notice.resident {
            let gone = self.notices.dismiss_popup(id);
            notify::closed(gone, Reason::Expired);
        } else {
            self.dismiss_notice(id);
        }
    }

    /// One of a notification's action buttons, from a pop-up or the centre: its app hears which,
    /// and the notification goes unless it asked to stay.
    pub fn invoke_notice_action(&mut self, id: u32, index: usize) {
        let Some(notice) = self.notices.get(id).cloned() else {
            return;
        };
        let Some((action, _)) = notice.actions.get(index) else {
            return;
        };
        tracing::info!(id, app = notice.app, index, "notification action chosen");
        notify::invoked(id, action);
        if notice.resident {
            let gone = self.notices.dismiss_popup(id);
            notify::closed(gone, Reason::Expired);
        } else {
            self.dismiss_notice(id);
        }
    }

    pub fn dismiss_notice(&mut self, id: u32) {
        if self.notices.remove(id) {
            notify::closed(vec![id], Reason::Dismissed);
        }
    }

    pub fn clear_notices(&mut self) {
        let ids = self.notices.clear();
        notify::closed(ids, Reason::Dismissed);
    }
}

/// The pop-up under `pointer`, in the space's coordinates, when the pop-ups were drawn on a
/// screen whose corner is at `origin` and their areas (`hits`) are kept in that screen's own.
pub fn popup_hit(
    pointer: (f64, f64),
    origin: (f64, f64),
    hits: &[(u32, Rectangle<f64, Logical>)],
) -> Option<u32> {
    let local = (pointer.0 - origin.0, pointer.1 - origin.1);
    hits.iter()
        .find(|(_, area)| area.contains(local))
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn notice(id: u32) -> Notice {
        Notice {
            id,
            app: "Mail".into(),
            app_id: None,
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
            "faded, it waits for the UI as before"
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

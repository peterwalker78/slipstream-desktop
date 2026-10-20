//! The lock's card as painted: the clock and date, who is logged in, the password pill and its
//! hint, and the status row, each painted again only when what it shows changes.

use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
};

use super::Facts;
use crate::{
    icons,
    paint::{self, Painter},
    panel::{self, AMBER, DESIGN_PX},
    text::{self, Face, Style},
};

#[derive(Debug, Clone, PartialEq)]
/// What the card is painted from.
pub(super) struct View {
    pub(super) facts: Facts,
    pub(super) dots: usize,
    pub(super) hint: String,
    pub(super) amber: bool,
    pub(super) caret: bool,
}

/// One painted part of the card, with what it was painted from.
struct Piece {
    key: String,
    buffer: MemoryRenderBuffer,
    /// Its size in logical pixels, and in the buffer's pixels.
    logical: Size<i32, Logical>,
    device: (i32, i32),
}

impl Piece {
    /// A piece `w` × `h` design pixels, painted at `scale` in design pixels.
    fn paint(
        key: String,
        w: f32,
        h: f32,
        scale: f64,
        draw: impl FnOnce(&mut Painter),
    ) -> Option<Self> {
        let logical = Size::<i32, Logical>::from((
            (w * DESIGN_PX).ceil().max(1.0) as i32,
            (h * DESIGN_PX).ceil().max(1.0) as i32,
        ));
        let device = (
            (logical.w as f64 * scale).round().max(1.0) as i32,
            (logical.h as f64 * scale).round().max(1.0) as i32,
        );
        let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * DESIGN_PX)?;
        draw(&mut p);
        Some(Self {
            key,
            buffer: paint::buffer(&p.pixmap),
            logical,
            device,
        })
    }

    /// The piece centred across `width` with its top at `y` (logical), shown as `shown` says
    /// about the screen's centre `centre`.
    fn element<R>(
        &self,
        renderer: &mut R,
        (width, y): (i32, f64),
        centre: (f64, f64),
        scale: f64,
        shown: Shown,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Shown { alpha, grow, dx } = shown;
        let x = ((width - self.logical.w) / 2) as f64 + dx;
        let at = Point::<f64, Logical>::from((
            centre.0 + (x - centre.0) * grow,
            centre.1 + (y - centre.1) * grow,
        ));
        let shown = Size::<i32, Logical>::from((
            (self.logical.w as f64 * grow).round() as i32,
            (self.logical.h as f64 * grow).round() as i32,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            &self.buffer,
            Some(alpha),
            Some(Rectangle::from_size(
                (self.device.0 as f64, self.device.1 as f64).into(),
            )),
            Some(shown),
            Kind::Unspecified,
        )
        .ok()
    }
}

// The card's parts, in design pixels: the clock and date at the top, who is logged in with the
// pill and its hint in the middle, and the status row at the foot, with the room shared out
// between them.
const TOP: f32 = 150.0;
const BOTTOM: f32 = 56.0;
const CLOCK_PX: f32 = 220.0;
const DATE_PX: f32 = 34.0;
const TOP_H: f32 = CLOCK_PX + 6.0 + DATE_PX * 1.2;
/// The name is a quiet label over the pill, nearer to it than the column's gaps.
const NAME_PX: f32 = 18.0;
const USER_H: f32 = NAME_PX * 1.2;
const NAME_GAP: f32 = 8.0;
const PILL_H: f32 = 14.0 * 2.0 + 22.0 * 1.2 + 2.0;
const PILL_MIN: f32 = 380.0;
const HINT_PX: f32 = 16.0;
const HINT_H: f32 = HINT_PX * 1.2;
const STATUS_H: f32 = 20.0;
const GAP: f32 = 14.0;
/// The most dots shown, however long the password.
const DOTS_SHOWN: usize = 32;

#[derive(Default)]
pub(super) struct Card {
    top: Option<Piece>,
    user: Option<Piece>,
    pill: Option<Piece>,
    hint: Option<Piece>,
    status: Option<Piece>,
}

/// Keeps `slot` painted from `key`, painting again only when the key changes.
fn keep(slot: &mut Option<Piece>, key: String, paint: impl FnOnce(String) -> Option<Piece>) {
    if slot.as_ref().is_none_or(|piece| piece.key != key) {
        *slot = paint(key);
    }
}

/// How the card is shown as the lock comes and goes: at `alpha`, grown by `grow` about the
/// screen's centre, with the password pill moved `dx` across as it shakes.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shown {
    pub alpha: f32,
    pub grow: f64,
    pub dx: f64,
}

impl Card {
    pub(super) fn elements<R>(
        &mut self,
        renderer: &mut R,
        view: &View,
        size: Size<i32, Logical>,
        scale: f64,
        shown: Shown,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let s = scale.to_bits();
        let facts = &view.facts;
        keep(
            &mut self.top,
            format!("{s}|{}|{}", facts.time, facts.date),
            |key| paint_top(key, &facts.time, &facts.date, scale),
        );
        keep(&mut self.user, format!("{s}|{}", facts.user), |key| {
            paint_user(key, &facts.user, scale)
        });
        let dots = view.dots.min(DOTS_SHOWN);
        keep(
            &mut self.pill,
            format!("{s}|{dots}|{}|{}", view.caret, facts.caps_lock),
            |key| paint_pill(key, dots, view.caret, facts.caps_lock, scale),
        );
        keep(
            &mut self.hint,
            format!("{s}|{}|{}", view.hint, view.amber),
            |key| paint_hint(key, &view.hint, view.amber, scale),
        );
        keep(
            &mut self.status,
            format!("{s}|{:?}|{:?}", facts.network, facts.battery),
            |key| paint_status(key, facts.network.as_deref(), facts.battery, scale),
        );

        let px = DESIGN_PX as f64;
        let height = size.h as f64 / px;
        let middle_h = (USER_H + NAME_GAP + PILL_H + GAP + HINT_H) as f64;
        let top_end = (TOP + TOP_H) as f64;
        let status_y = height - (BOTTOM + STATUS_H) as f64;
        let gap = ((status_y - top_end - middle_h) / 2.0).max(0.0);
        let user_y = top_end + gap;
        let pill_y = user_y + (USER_H + NAME_GAP) as f64;
        let hint_y = pill_y + (PILL_H + GAP) as f64;
        let centre = (size.w as f64 / 2.0, size.h as f64 / 2.0);
        let place = [
            (&self.top, TOP as f64, false),
            (&self.user, user_y, false),
            (&self.pill, pill_y, true),
            (&self.hint, hint_y, false),
            (&self.status, status_y, false),
        ];
        // Only the pill shakes.
        let still = Shown { dx: 0.0, ..shown };
        place
            .into_iter()
            .filter_map(|(piece, y, shakes)| {
                let shown = if shakes { shown } else { still };
                piece
                    .as_ref()?
                    .element(renderer, (size.w, y * px), centre, scale, shown)
            })
            .collect()
    }
}

fn paint_top(key: String, time: &str, date: &str, scale: f64) -> Option<Piece> {
    let clock = Style {
        tracking: 0.01,
        tabular: true,
        ..Style::new(Face::Display, CLOCK_PX, panel::BRIGHT)
    };
    let day = Style::new(Face::Body, DATE_PX, 0xd3d9e4ff);
    let (clock_w, day_w) = (text::width(time, &clock), text::width(date, &day));
    let w = clock_w.max(day_w).max(1.0) + 8.0;
    Piece::paint(key, w, TOP_H, scale, |p| {
        p.text(time, (w - clock_w) / 2.0, CLOCK_PX / 2.0, &clock);
        p.text(
            date,
            (w - day_w) / 2.0,
            CLOCK_PX + 6.0 + DATE_PX * 0.6,
            &day,
        );
    })
}

fn paint_user(key: String, user: &str, scale: f64) -> Option<Piece> {
    let name = Style::new(Face::Body, NAME_PX, 0xb3bbc8ff);
    let w = text::width(user, &name).max(1.0) + 8.0;
    Piece::paint(key, w, USER_H, scale, |p| {
        p.text(user, 4.0, USER_H / 2.0, &name);
    })
}

/// The password pill. With Caps Lock on, the key's arrow sits at its right end, in amber, as
/// Windows marks its password box.
fn paint_pill(key: String, dots: usize, caret: bool, caps_lock: bool, scale: f64) -> Option<Piece> {
    const CAPS_ICON: f32 = 22.0;
    const CAPS_INSET: f32 = 20.0;
    let (label, style) = if dots == 0 {
        (
            "Password".to_string(),
            Style::new(Face::Body, 22.0, panel::SECONDARY),
        )
    } else {
        (
            "•".repeat(dots),
            Style {
                tracking: 0.3,
                ..Style::new(Face::Body, 22.0, panel::BRIGHT)
            },
        )
    };
    let label_w = text::width(&label, &style);
    let caret_w = 5.0;
    // Room for the arrow on both sides, so the dots stay in the middle.
    let caps_room = if caps_lock {
        2.0 * (CAPS_ICON + CAPS_INSET)
    } else {
        0.0
    };
    let w = PILL_MIN.max(label_w + caret_w + 48.0 + caps_room);
    Piece::paint(key, w, PILL_H, scale, |p| {
        p.fill(0.0, 0.0, w, PILL_H, 30.0, 0xffffff1f);
        p.border(0.0, 0.0, w, PILL_H, 30.0, 1.0, 0xffffff30);
        if caps_lock {
            let (x, y) = (w - CAPS_INSET - CAPS_ICON, (PILL_H - CAPS_ICON) / 2.0);
            p.icon(icons::CAPS_LOCK, x, y, CAPS_ICON, Some(AMBER));
        }
        let x = (w - label_w - caret_w) / 2.0;
        p.text(&label, x, PILL_H / 2.0, &style);
        if caret {
            // CSS draws the caret after the text, 2 px on, 1.1 em tall.
            let h = 22.0 * 1.1;
            p.fill(x + label_w + 2.0, (PILL_H - h) / 2.0, 3.0, h, 0.0, AMBER);
        }
    })
}

fn paint_hint(key: String, hint: &str, amber: bool, scale: f64) -> Option<Piece> {
    let style = Style::new(Face::Body, HINT_PX, if amber { AMBER } else { 0xb3bbc8ff });
    let hint_w = text::width(hint, &style);
    let w = hint_w.max(1.0) + 8.0;
    Piece::paint(key, w, HINT_H, scale, |p| {
        p.text(hint, 4.0, HINT_H / 2.0, &style);
    })
}

fn paint_status(
    key: String,
    network: Option<&str>,
    battery: Option<(u8, bool)>,
    scale: f64,
) -> Option<Piece> {
    const INK: u32 = 0xcfd6e2ff;
    const ICON: f32 = 18.0;
    let words = Style::new(Face::Body, 16.0, INK);
    let dot = Style::new(Face::Body, 16.0, 0x6f7888ff);
    // Each part is an icon and its words; the parts are joined by a dot with room either side.
    let mut parts: Vec<(String, String, Option<u32>)> = Vec::new();
    parts.push((
        icons::WIFI.to_string(),
        network.unwrap_or("Not connected").to_string(),
        Some(INK),
    ));
    if let Some((percent, charging)) = battery {
        parts.push((
            icons::battery(percent, charging),
            format!("{percent}%"),
            Some(INK),
        ));
    }
    parts.push((icons::LOGO.to_string(), "Slipstream".to_string(), None));
    let part_w = |words_text: &str| ICON + 10.0 + text::width(words_text, &words);
    let joint = 16.0 + text::width("·", &dot) + 16.0;
    let w = parts.iter().map(|(_, t, _)| part_w(t)).sum::<f32>()
        + joint * (parts.len() - 1) as f32
        + 8.0;
    Piece::paint(key, w, STATUS_H, scale, |p| {
        let mut x = 4.0;
        for (i, (icon, label, ink)) in parts.iter().enumerate() {
            if i > 0 {
                x += 10.0;
                p.text("·", x + 6.0, STATUS_H / 2.0, &dot);
                x += joint - 10.0;
            }
            p.icon(icon, x, (STATUS_H - ICON) / 2.0, ICON, *ink);
            x += ICON + 10.0;
            x += p.text(label, x, STATUS_H / 2.0, &words);
        }
    })
}

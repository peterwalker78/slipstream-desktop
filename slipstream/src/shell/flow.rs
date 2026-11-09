//! How every panel and card comes and goes. It pours down out of the bar: its card stays where
//! it rests and is revealed from its top edge downward behind a lit edge, fully opaque from the
//! start. Closing, it drains back up behind the same edge. Both are a crop of the picture the
//! panel already has, with one small strip for the edge, baked once for the ring colour and the
//! scale; nothing is repainted, and nothing runs once a panel has settled.
//!
//! The edge is shown through a miniature of the failing tube closing windows go through:
//! scanlines across its glow, a red and a blue fringe either side of the line, a flicker, and
//! now and then a frame where it tears sideways.
//!
//! With reduced motion there is no crop and no edge: a panel only fades, no longer than
//! `REDUCED_FADE`.

use std::cell::RefCell;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{
    ImportMem, Renderer,
    element::{
        Kind,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    },
};
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use crate::{anim::Easing, effects::fx, motion::REDUCED_FADE, paint::Painted};

/// Opening: 320 ms on a glide, top first.
const OPEN: f64 = 0.32;
/// Closing: 220 ms, easing in and out. Only the paint lingers: input goes to what's underneath
/// the moment it closes.
const CLOSE: f64 = 0.22;

/// The lit edge: a line `LINE` logical pixels thick, `INSET` in from the card's sides, with a
/// glow reaching `GLOW` above and below it.
const LINE: f64 = 2.0;
const INSET: f64 = 6.0;
const GLOW: f64 = 14.0;
const STRIP_H: f64 = 2.0 * GLOW + LINE;
/// The strip's width in pixels: every column is the same, so it is stretched along the card.
const STRIP_W: usize = 4;
/// The glow's strength where it leaves the line, and how far towards white the line's own
/// colour is taken, which also gives its fringes some red and blue to split off.
const GLOW_PEAK: f32 = 0.55;
const CORE_LIGHT: f32 = 0.35;
/// The fringes' strength, as a share of the line's red and blue.
const FRINGE: f32 = 0.3;
/// About one frame of the tube in this many tears the edge, by up to `TEAR_REACH` logical pixels.
const TEAR_CHANCE: f32 = 0.06;
const TEAR_REACH: f64 = 5.0;

/// A panel's picture as drawn this frame, front first: the lit edge, then the picture.
pub type Pair<R> = [Option<MemoryRenderBufferRenderElement<R>>; 2];

/// One layer of what a panel last drew: its paint, its corner where the panel settles, and its
/// share of the panel's opacity.
pub type Layer = (Painted, Point<f64, Logical>, f32);

/// How much of a panel shows: its opacity, how far down its card is revealed, 0 to 1, and
/// whether the reveal's edge is lit, which it isn't with reduced motion.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    alpha: f32,
    reveal: f32,
    lit: bool,
}

/// Nothing shown.
const GONE: Look = Look {
    alpha: 0.0,
    reveal: 0.0,
    lit: false,
};

/// A panel's own way in from nothing, `since` seconds after it opened.
fn opening(since: f64, reduced_motion: bool) -> Look {
    if reduced_motion {
        Look {
            alpha: (since / REDUCED_FADE).clamp(0.0, 1.0) as f32,
            reveal: 1.0,
            lit: false,
        }
    } else {
        Look {
            alpha: 1.0,
            reveal: Easing::Glide
                .at((since / OPEN).clamp(0.0, 1.0))
                .clamp(0.0, 1.0) as f32,
            lit: true,
        }
    }
}

/// A closing, `since` seconds in, from what was drawn when it closed: drains up behind its edge,
/// or with reduced motion only fades, no longer than `REDUCED_FADE`. `None` once it has gone.
fn closing_look(from: Look, since: f64, reduced_motion: bool) -> Option<Look> {
    let length = if reduced_motion { REDUCED_FADE } else { CLOSE };
    let t = since / length;
    if t >= 1.0 {
        return None;
    }
    let t = t.max(0.0);
    Some(if reduced_motion {
        Look {
            alpha: from.alpha * (1.0 - t) as f32,
            lit: false,
            ..from
        }
    } else {
        Look {
            reveal: from.reveal * (1.0 - Easing::InOutCubic.at(t)) as f32,
            lit: true,
            ..from
        }
    })
}

/// Where a panel is cut off this frame: its opacity, the height on the screen its pictures stop
/// at while it is being revealed, and its lit edge's left end, length and height, while it is
/// drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reveal {
    pub alpha: f32,
    line: Option<f64>,
    edge: Option<(f64, f64, f64)>,
}

impl Reveal {
    fn new(look: Look, card: Rectangle<f64, Logical>) -> Self {
        let moving = look.reveal < 1.0;
        let line = card.loc.y + card.size.h * look.reveal as f64;
        Self {
            alpha: look.alpha,
            line: moving.then_some(line),
            edge: (look.lit && moving && look.reveal > 0.0).then_some((
                card.loc.x + INSET,
                card.size.w - 2.0 * INSET,
                line,
            )),
        }
    }

    /// Whether anything above `bottom` on the screen shows.
    pub fn shows_above(&self, bottom: f64) -> bool {
        self.alpha > 0.0 && self.line.is_none_or(|line| line >= bottom)
    }

    /// `painted` with its corner at `at`, `share` of the panel's opacity, cut off at the line.
    pub fn picture<R>(
        &self,
        renderer: &mut R,
        painted: &Painted,
        at: Point<f64, Logical>,
        share: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.buffer(
            renderer,
            &painted.buffer,
            (painted.logical, painted.device),
            painted.scale,
            at,
            share,
        )
    }

    /// A buffer `logical` pixels big, `device` in the buffer's own, drawn at `scale` with its
    /// corner at `at` on whole screen pixels, cut off at the line. Only the rows above the line
    /// are read, so nothing is squashed.
    pub fn buffer<R>(
        &self,
        renderer: &mut R,
        buffer: &MemoryRenderBuffer,
        (logical, device): (Size<i32, Logical>, (i32, i32)),
        scale: f64,
        at: Point<f64, Logical>,
        share: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let shown = self.line.map_or(logical.h, |line| {
            ((line - at.y).floor() as i32).clamp(0, logical.h)
        });
        if shown <= 0 || logical.h <= 0 || self.alpha <= 0.0 {
            return None;
        }
        let rows = device.1 as f64 * shown as f64 / logical.h as f64;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            buffer,
            Some(self.alpha * share),
            Some(Rectangle::from_size((device.0 as f64, rows).into())),
            Some((logical.w, shown).into()),
            Kind::Unspecified,
        )
        .ok()
    }

    /// The lit edge in `ring`, the focus ring's colour, at `scale` and `now` on the tube's clock,
    /// while the panel is moving.
    pub fn edge<R>(
        &self,
        renderer: &mut R,
        ring: u32,
        scale: f64,
        now: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (left, length, line) = self.edge?;
        let (flicker, torn) = fx::tube(now, TEAR_CHANCE);
        let at =
            Point::<f64, Logical>::from((left + torn as f64 * TEAR_REACH, line - STRIP_H / 2.0));
        let strip = strip(ring & 0xffffff00, scale);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            &strip,
            Some(self.alpha * flicker),
            None,
            Some((length.round() as i32, STRIP_H as i32).into()),
            Kind::Unspecified,
        )
        .ok()
    }

    /// A panel with one picture, `painted` at `at`, and its edge in front of it.
    pub fn pair<R>(
        &self,
        renderer: &mut R,
        painted: &Painted,
        at: Point<f64, Logical>,
        frame: crate::panel::Frame,
    ) -> Pair<R>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        [
            self.edge(renderer, frame.ring, painted.scale, frame.time),
            self.picture(renderer, painted, at, 1.0),
        ]
    }
}

thread_local! {
    /// The lit edge's strip and the colour and scale it was baked for. Every panel shares it;
    /// the compositor draws on one thread.
    static STRIP: RefCell<Option<(u32, f64, MemoryRenderBuffer)>> = const { RefCell::new(None) };
}

/// The lit edge's strip in `ring` at `scale`, baked again only when either changes.
fn strip(ring: u32, scale: f64) -> MemoryRenderBuffer {
    STRIP.with_borrow_mut(|kept| {
        if let Some((colour, at, strip)) = kept.as_ref()
            && *colour == ring
            && *at == scale
        {
            return strip.clone();
        }
        let (pixels, rows) = bake(ring, scale);
        let strip = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Abgr8888,
            (STRIP_W as i32, rows as i32),
            1,
            Transform::Normal,
            None,
        );
        *kept = Some((ring, scale, strip.clone()));
        strip
    })
}

/// The strip's pixels, premultiplied, and how many rows it has: the line across its middle in
/// `ring` taken towards white, and its glow, which is light added to what's behind rather than
/// laid over it, so it carries no alpha. The glow is darker on every other row, as the tube's
/// scanlines are, and just above the line its red is split off, and just below its blue.
fn bake(ring: u32, scale: f64) -> (Vec<u8>, usize) {
    let rows = (STRIP_H * scale).round().max(1.0) as usize;
    let channels = |rgba: u32| {
        let [r, g, b, _] = rgba.to_be_bytes().map(|c| c as f32 / 255.0);
        [r, g, b]
    };
    let core = channels(crate::panel::lighter(ring, CORE_LIGHT));
    let glow = channels(ring);
    let mut pixels = Vec::with_capacity(rows * STRIP_W * 4);
    for row in 0..rows {
        let from_line = (row as f64 + 0.5) / scale - STRIP_H / 2.0;
        let past = from_line.abs() - LINE / 2.0;
        let rgba = if past <= 0.0 {
            [core[0], core[1], core[2], 1.0]
        } else {
            let mut strength = GLOW_PEAK * (1.0 - (past / GLOW).min(1.0) as f32).powi(2);
            if row % 2 == 1 {
                strength *= fx::SCANLINE;
            }
            let mut rgba = [
                glow[0] * strength,
                glow[1] * strength,
                glow[2] * strength,
                0.0,
            ];
            if past <= fx::SPLIT {
                if from_line < 0.0 {
                    rgba[0] += core[0] * FRINGE;
                } else {
                    rgba[2] += core[2] * FRINGE;
                }
            }
            rgba
        };
        let bytes = rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        for _ in 0..STRIP_W {
            pixels.extend_from_slice(&bytes);
        }
    }
    (pixels, rows)
}

/// A panel's way in and out. It opens from nothing, or from wherever a close it interrupts has
/// got to, and closes by draining the picture it last drew back up behind its edge. Neither holds
/// anything up: the panel stops taking keys and clicks the moment it closes, and this only paints.
pub struct Fade {
    /// What the opening starts from.
    from: Look,
    /// What was last drawn, and the card it was drawn over, on the screen.
    drawn: Look,
    card: Rectangle<f64, Logical>,
    closing: Option<Closing>,
}

/// A panel draining away: the picture it closed on, front first, what was drawn when it closed,
/// and when the close started on the animation clock, which the first frame after it fills in.
struct Closing {
    picture: Vec<Layer>,
    from: Look,
    started: Option<f64>,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            from: GONE,
            drawn: GONE,
            card: Rectangle::default(),
            closing: None,
        }
    }
}

impl Fade {
    /// Opens again: from where it is drawn if it was still draining, otherwise from nothing.
    pub fn open(&mut self) {
        self.from = match self.closing.take() {
            Some(_) => self.drawn,
            None => GONE,
        };
    }

    /// How it shows `since` seconds after it opened, over its card at `card` on the screen: its
    /// own way in, carried on from where it starts.
    pub fn opening(
        &mut self,
        since: f64,
        reduced_motion: bool,
        card: Rectangle<f64, Logical>,
    ) -> Reveal {
        let way = opening(since, reduced_motion);
        self.drawn = Look {
            alpha: way.alpha + self.from.alpha * (1.0 - way.alpha),
            reveal: way.reveal + self.from.reveal * (1.0 - way.reveal),
            lit: way.lit,
        };
        self.card = card;
        Reveal::new(self.drawn, card)
    }

    /// Closes, keeping `picture`, what it last drew, to drain away; `None` when nothing is
    /// drawn. A panel drawn some other way hands an empty picture and draws it from `closing`.
    /// Closing again while it drains changes nothing. Returns whether it drains.
    pub fn close(&mut self, picture: Option<Vec<Layer>>) -> bool {
        if self.closing.is_some() {
            return true;
        }
        match picture {
            Some(picture) if self.drawn.alpha > 0.0 && self.drawn.reveal > 0.0 => {
                self.closing = Some(Closing {
                    picture,
                    from: self.drawn,
                    started: None,
                });
                true
            }
            _ => {
                self.drawn = GONE;
                false
            }
        }
    }

    /// Whether it is still draining after closing.
    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }

    /// How it shows at `now` while it drains. `None` once it has gone, and the picture is let go.
    pub fn closing(&mut self, now: f64, reduced_motion: bool) -> Option<Reveal> {
        let closing = self.closing.as_mut()?;
        let started = *closing.started.get_or_insert(now);
        match closing_look(closing.from, now - started, reduced_motion) {
            Some(look) => {
                self.drawn = look;
                Some(Reveal::new(look, self.card))
            }
            None => {
                self.closing = None;
                self.drawn = GONE;
                None
            }
        }
    }

    /// Its picture at `now` while it drains, front first, with the edge in `ring` in front.
    pub fn elements<'a, R>(
        &'a mut self,
        renderer: &'a mut R,
        now: f64,
        ring: u32,
        reduced_motion: bool,
    ) -> impl Iterator<Item = MemoryRenderBufferRenderElement<R>> + 'a
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let reveal = self.closing(now, reduced_motion);
        let layers = self
            .closing
            .as_ref()
            .map_or(&[][..], |closing| closing.picture.as_slice());
        let edge = reveal
            .zip(layers.last())
            .and_then(|(reveal, (painted, ..))| reveal.edge(renderer, ring, painted.scale, now));
        edge.into_iter()
            .chain(layers.iter().filter_map(move |(painted, at, share)| {
                reveal?.picture(renderer, painted, *at, *share)
            }))
    }
}

/// The card in a picture `size` big with its corner at `at`: `margin` in from its top and sides,
/// and `margin + below` up from its foot, all in design pixels.
pub fn card_in(
    at: Point<f64, Logical>,
    size: Size<i32, Logical>,
    margin: f32,
    below: f32,
) -> Rectangle<f64, Logical> {
    let px = crate::panel::DESIGN_PX as f64;
    let (margin, below) = (margin as f64 * px, below as f64 * px);
    Rectangle::new(
        (at.x + margin, at.y + margin).into(),
        (
            size.w as f64 - 2.0 * margin,
            size.h as f64 - 2.0 * margin - below,
        )
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Option<Vec<Layer>> {
        Painted::new((1, 1).into(), 1.0, |_| {})
            .map(|painted| vec![(painted, Point::default(), 1.0)])
    }

    fn card() -> Rectangle<f64, Logical> {
        Rectangle::new((100.0, 50.0).into(), (300.0, 400.0).into())
    }

    #[test]
    fn a_closed_panel_lets_go_at_once_and_only_its_picture_drains() {
        let mut fade = Fade::default();
        fade.open();
        fade.opening(OPEN, false, card());
        assert_eq!(fade.drawn.reveal, 1.0);
        assert!(fade.close(picture()), "closing waits for nothing");
        assert!(fade.is_closing());
        assert_eq!(fade.closing(5.0, false).unwrap().line, None);
        let halfway = fade.closing(5.0 + CLOSE / 2.0, false).unwrap();
        assert_eq!(halfway.alpha, 1.0, "it drains, never fades");
        assert!(halfway.edge.is_some());
        assert_eq!(fade.closing(5.0 + CLOSE + 0.001, false), None);
        assert!(!fade.is_closing(), "and then the picture is let go");
    }

    #[test]
    fn a_panel_closed_before_it_was_drawn_has_nothing_to_drain() {
        let mut fade = Fade::default();
        fade.open();
        assert!(!fade.close(picture()));
        assert!(!fade.close(None));
        // Opened, but not yet revealed at all.
        fade.opening(0.0, false, card());
        assert!(!fade.close(picture()));
    }

    #[test]
    fn reopened_mid_drain_it_carries_on_from_where_it_is_drawn() {
        let mut fade = Fade::default();
        fade.opening(OPEN, false, card());
        fade.close(picture());
        fade.closing(0.0, false);
        let drawn = fade.closing(CLOSE * 0.6, false).unwrap();
        fade.open();
        assert!(!fade.is_closing());
        assert_eq!(fade.opening(0.0, false, card()), drawn);
        assert_eq!(fade.opening(OPEN, false, card()).line, None);
    }

    #[test]
    fn the_reveal_only_grows_opening_and_only_shrinks_closing() {
        let steps = 40;
        let mut last = -1.0;
        for n in 0..=steps {
            let reveal = opening(OPEN * n as f64 / steps as f64, false).reveal;
            assert!(reveal >= last, "{reveal} after {last}");
            last = reveal;
        }
        assert_eq!(opening(0.0, false).reveal, 0.0);
        assert_eq!(last, 1.0, "open ends full");
        let full = opening(OPEN, false);
        let mut last = 2.0;
        for n in 0..steps {
            let reveal = closing_look(full, CLOSE * n as f64 / steps as f64, false)
                .unwrap()
                .reveal;
            assert!(reveal <= last, "{reveal} after {last}");
            last = reveal;
        }
        assert!(last < 0.01, "close ends empty");
        assert_eq!(closing_look(full, CLOSE, false), None);
    }

    #[test]
    fn the_edge_rides_the_reveal_only_while_it_moves() {
        let card = card();
        let half = Reveal::new(opening(OPEN / 2.0, false), card);
        let (left, length, line) = half.edge.unwrap();
        assert_eq!(
            (left, length),
            (card.loc.x + INSET, card.size.w - 2.0 * INSET)
        );
        assert_eq!(Some(line), half.line);
        assert!(line > card.loc.y && line < card.loc.y + card.size.h);
        assert_eq!(Reveal::new(opening(OPEN, false), card).edge, None);
        assert_eq!(Reveal::new(opening(0.0, false), card).edge, None);
    }

    #[test]
    fn with_reduced_motion_it_only_fades_and_no_longer_than_the_short_fade() {
        let mut fade = Fade::default();
        let opened = fade.opening(REDUCED_FADE / 2.0, true, card());
        assert_eq!((opened.alpha, opened.line, opened.edge), (0.5, None, None));
        fade.opening(REDUCED_FADE, true, card());
        fade.close(picture());
        assert_eq!(fade.closing(0.0, true).unwrap().alpha, 1.0);
        let half = fade.closing(REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!((half.alpha, half.line, half.edge), (0.5, None, None));
        assert_eq!(fade.closing(REDUCED_FADE, true), None);
    }

    #[test]
    fn reduced_motion_switched_on_mid_drain_turns_it_into_the_short_fade() {
        let mut fade = Fade::default();
        fade.opening(OPEN, false, card());
        fade.close(picture());
        assert!(fade.closing(0.0, false).unwrap().edge.is_none());
        let now = fade.closing(REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!(now.edge, None);
        assert_eq!(fade.closing(REDUCED_FADE, true), None);
    }

    #[test]
    fn the_strip_has_a_solid_line_a_banded_glow_and_split_fringes() {
        let (pixels, rows) = bake(0x42d3ffcc, 1.0);
        assert_eq!(rows, STRIP_H as usize);
        let row = |n: usize| &pixels[n * STRIP_W * 4..n * STRIP_W * 4 + 4];
        let middle = rows / 2;
        assert_eq!(row(middle)[3], 255, "the line itself is solid");
        // Above the line its red is split off, below it its blue.
        let (above, below) = (row(middle - 2), row(middle + 1));
        assert!(above[0] > below[0] && below[2] > above[2]);
        // The glow is only light, and every other row is darker, nearer the line or not.
        assert_eq!(row(middle - 6)[3], 0);
        assert!(row(middle - 6)[1] < row(middle - 7)[1]);
    }
}

//! How every panel and card comes and goes: as a drawer out of the bar.
//!
//! Opening, a slot opens in the bar's foot line under the button that opened the panel, its seam
//! lit in the focus ring's colour. The panel's bare card slides down out of it, clipped at the
//! bar's foot, with a lit edge riding its leading edge. The contents arrive while it extends: each
//! piece (a tile, a row, a group of buttons) grows into its place as it comes out of the slot,
//! powering up like a picture on a tube, with a flicker and a red and blue split that settle as
//! it lands. Closing, the drawer goes back up into the slot, each piece powering down and
//! shrinking away just before the slot reaches it, and the slot seals with a glint. Faint
//! scanlines lie over the drawer while it moves.
//!
//! Everything is drawn from pictures the panel already has: the card is painted bare once per
//! picture, each piece is a crop of the panel's picture, and the edge, the seam and the scanlines
//! are small strips baked once and shared by every panel. Nothing is repainted per frame, and once
//! a panel has settled it is its own picture again.
//!
//! With reduced motion there is no slot, drawer, piece or edge: a panel only fades, no longer
//! than `REDUCED_FADE`.

use std::cell::RefCell;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{
    ImportMem, Renderer,
    element::{
        Element, Id, Kind, RenderElement, UnderlyingStorage,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    },
    utils::{CommitCounter, DamageSet, OpaqueRegions},
};
use smithay::utils::{
    Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
    user_data::UserDataMap,
};

use crate::{anim::Easing, effects::fx, motion::REDUCED_FADE, paint::Painted};

/// The drawer's way out: 340 ms on a glide.
const OPEN: f64 = 0.34;
/// Its way back in: 240 ms, easing in and out. Only the paint lingers: input goes to what's
/// underneath the moment it closes.
const CLOSE: f64 = 0.24;
/// The slot spreads from the button to the card's width over its first 100 ms.
const SLOT_OPEN: f64 = 0.1;
/// Closing, the slot narrows back over the last 45% of the way, and glints over the last 30%.
const SEAL: f64 = 0.45;
const GLINT: f64 = 0.3;
/// The seam's strength at rest, and while the drawer goes back in before its glint.
const SEAM_REST: f32 = 0.35;
const SEAM_CLOSING: f32 = 0.7;

/// Each piece grows from `GROW_FROM` of its size to all of it over `GROW`, pieces side by side
/// starting `STAGGER` apart from the left, and every piece has landed within `LAND_BY` of the
/// drawer.
const GROW: f64 = 0.14;
const GROW_FROM: f64 = 0.82;
const STAGGER: f64 = 0.02;
const LAND_BY: f64 = 0.1;
/// Closing, each piece shrinks away over this long, ending as the slot reaches its top.
const LEAVE: f64 = 0.09;
/// A piece's colours split by up to this many logical pixels while it powers up or down.
const SPLIT_PX: f64 = 2.5;
/// How strong each split colour is at its widest.
const SPLIT_ALPHA: f32 = 0.5;
/// The scanlines over the moving drawer: half as dark as the edge's.
const SCAN_DARK: f32 = (1.0 - fx::SCANLINE) / 2.0;
/// The card's corner radius in logical pixels: the scanlines keep this far from its top and foot,
/// so they never show past its rounded corners.
const CORNER: f64 = 18.0 * crate::panel::DESIGN_PX as f64;
/// More pieces than this and the picture comes in whole.
const MOST_PIECES: usize = 32;

/// The lit edge: a line `LINE` logical pixels thick, `INSET` in from the card's sides, with a
/// glow reaching `GLOW` above and below it. The seam is the same strip.
const LINE: f64 = 2.0;
const INSET: f64 = 6.0;
const GLOW: f64 = 14.0;
const STRIP_H: f64 = 2.0 * GLOW + LINE;
/// The strips' width in pixels: every column is the same, so they are stretched along the card.
const STRIP_W: usize = 4;
/// The scanline strip's height in device pixels: taller than any screen a drawer crosses.
const SCAN_ROWS: usize = 4096;
/// The glow's strength where it leaves the line, and how far towards white the line's own
/// colour is taken, which also gives its fringes some red and blue to split off.
const GLOW_PEAK: f32 = 0.55;
const CORE_LIGHT: f32 = 0.35;
/// The fringes' strength, as a share of the line's red and blue.
const FRINGE: f32 = 0.3;
/// About one frame of the tube in this many tears the edge, by up to `TEAR_REACH` logical pixels.
const TEAR_CHANCE: f32 = 0.06;
const TEAR_REACH: f64 = 5.0;

/// One layer of what a panel last drew: its paint, its corner where the panel settles, and its
/// share of the panel's opacity. A panel's own picture is its last layer; the others ride along.
pub type Layer = (Painted, Point<f64, Logical>, f32);

/// What a panel shows this frame, borrowed: a picture, its corner and its share of the opacity.
pub type Shown<'a> = (&'a Painted, Point<f64, Logical>, f32);

/// An element of a panel on its way in or out. A piece cropped from a panel's picture is drawn
/// under an id of its own, since many share one buffer, and damage is tracked by id.
pub struct FlowElement<R: Renderer> {
    inner: MemoryRenderBufferRenderElement<R>,
    id: Id,
}

impl<R: Renderer> FlowElement<R> {
    fn with_id(inner: MemoryRenderBufferRenderElement<R>, id: &Id) -> Self {
        Self {
            inner,
            id: id.clone(),
        }
    }
}

impl<R: Renderer> From<MemoryRenderBufferRenderElement<R>> for FlowElement<R> {
    fn from(inner: MemoryRenderBufferRenderElement<R>) -> Self {
        let id = inner.id().clone();
        Self { inner, id }
    }
}

impl<R: Renderer> std::fmt::Debug for FlowElement<R>
where
    MemoryRenderBufferRenderElement<R>: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FlowElement").field(&self.inner).finish()
    }
}

impl<R: Renderer> Element for FlowElement<R> {
    fn id(&self) -> &Id {
        &self.id
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R> RenderElement<R> for FlowElement<R>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.inner
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

/// Up to `N` elements, front first, with nothing on the heap: what a panel with one picture
/// draws in a frame.
pub struct Drawn<R: Renderer, const N: usize = 56> {
    items: [Option<FlowElement<R>>; N],
    len: usize,
}

/// What a panel with one picture draws in a frame.
pub type Pair<R> = Drawn<R>;

impl<R: Renderer, const N: usize> Default for Drawn<R, N> {
    fn default() -> Self {
        Self {
            items: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

impl<R: Renderer, const N: usize> Extend<FlowElement<R>> for Drawn<R, N> {
    fn extend<I: IntoIterator<Item = FlowElement<R>>>(&mut self, iter: I) {
        for element in iter {
            if self.len < N {
                self.items[self.len] = Some(element);
                self.len += 1;
            }
        }
    }
}

impl<R: Renderer, const N: usize> IntoIterator for Drawn<R, N> {
    type Item = FlowElement<R>;
    type IntoIter = std::iter::Flatten<std::array::IntoIter<Option<FlowElement<R>>, N>>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().flatten()
    }
}

/// How much of a panel shows: its opacity, how far out its drawer is, 0 in the slot to 1 in its
/// place, and whether it moves at all, which it doesn't with reduced motion.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    alpha: f32,
    travel: f32,
    lit: bool,
}

/// Nothing shown.
const GONE: Look = Look {
    alpha: 0.0,
    travel: 0.0,
    lit: false,
};

impl Default for Look {
    fn default() -> Self {
        GONE
    }
}

/// The glide, held between 0 and 1.
fn glide(t: f64) -> f64 {
    Easing::Glide.at(t.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

fn in_out(t: f64) -> f64 {
    Easing::InOutCubic.at(t.clamp(0.0, 1.0))
}

/// A panel's way in, `since` seconds after it opened, from `from`.
fn opening_look(since: f64, from: Look, reduced_motion: bool) -> Look {
    if reduced_motion {
        let way = (since / REDUCED_FADE).clamp(0.0, 1.0) as f32;
        Look {
            alpha: way + from.alpha * (1.0 - way),
            travel: 1.0,
            lit: false,
        }
    } else {
        Look {
            alpha: 1.0,
            travel: travel_opening(from.travel, since),
            lit: true,
        }
    }
}

/// How far out the drawer is `since` seconds into an opening that started at `from`.
fn travel_opening(from: f32, since: f64) -> f32 {
    from + (1.0 - from) * glide(since / OPEN) as f32
}

/// How far out the drawer is `since` seconds into a closing that started at `from`.
fn travel_closing(from: f32, since: f64) -> f32 {
    from * (1.0 - in_out(since / CLOSE)) as f32
}

/// A closing, `since` seconds in, from what was drawn when it closed: back into the slot, or
/// with reduced motion only a fade, no longer than `REDUCED_FADE`. `None` once it has gone.
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
            travel: travel_closing(from.travel, since),
            lit: true,
            ..from
        }
    })
}

/// When, into an opening from `from`, the drawer is first out as far as `travel`.
fn opening_reaches(travel: f32, from: f32) -> f64 {
    if travel <= from {
        return 0.0;
    }
    let (mut early, mut late) = (0.0, OPEN);
    for _ in 0..24 {
        let mid = (early + late) / 2.0;
        if travel_opening(from, mid) >= travel {
            late = mid;
        } else {
            early = mid;
        }
    }
    late
}

/// When, into a closing from `from`, the drawer is first back in as far as `travel`.
fn closing_reaches(travel: f32, from: f32) -> f64 {
    if travel >= from {
        return 0.0;
    }
    let (mut early, mut late) = (0.0, CLOSE);
    for _ in 0..24 {
        let mid = (early + late) / 2.0;
        if travel_closing(from, mid) <= travel {
            late = mid;
        } else {
            early = mid;
        }
    }
    late
}

/// When a piece starts growing in an opening from `from`: once the drawer is out as far as
/// `out_at`, which brings the piece out of the slot, later by `column` staggers, and never so
/// late that it lands more than `LAND_BY` after the drawer.
fn piece_starts(out_at: f32, from: f32, column: usize) -> f64 {
    (opening_reaches(out_at, from) + column as f64 * STAGGER).min(OPEN + LAND_BY - GROW)
}

/// How far a piece has grown `since` seconds into an opening, from `from` at the start, growing
/// from `start`.
fn grown_opening(from: f32, start: f64, since: f64) -> f32 {
    let way = Easing::OutCubic.at(((since - start) / GROW).clamp(0.0, 1.0)) as f32;
    from + (1.0 - from) * way
}

/// How far a piece has grown `since` seconds into a closing, from `from` when it closed: it
/// shrinks away over the `LEAVE` before `gone`, when the slot reaches it.
fn grown_closing(from: f32, gone: f64, since: f64) -> f32 {
    let start = (gone - LEAVE).max(0.0);
    if since >= gone {
        0.0
    } else if since <= start {
        from
    } else {
        from * (1.0 - in_out((since - start) / (gone - start)) as f32)
    }
}

/// Where the drawer runs: the card where it rests and the line it comes out of, the bar's foot,
/// or the card's top if that is higher.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Drawer {
    card: Rectangle<f64, Logical>,
    slot: f64,
}

impl Drawer {
    fn new(card: Rectangle<f64, Logical>, foot: f64) -> Self {
        Self {
            card,
            slot: foot.min(card.loc.y),
        }
    }

    /// How far the drawer runs, from all in the slot to its place.
    fn span(&self) -> f64 {
        (self.card.loc.y - self.slot + self.card.size.h).max(1.0)
    }

    /// How far above its place the drawer is, at `travel`.
    fn lift(&self, travel: f32) -> f64 {
        (1.0 - travel as f64) * self.span()
    }

    /// How far out the drawer has to be for a line `y` on the screen, where it rests, to be out of
    /// the slot.
    fn out_at(&self, y: f64) -> f32 {
        (1.0 - (y - self.slot) / self.span()).clamp(0.0, 1.0) as f32
    }

    /// Its leading edge on the screen, at `travel`.
    fn lead(&self, travel: f32) -> f64 {
        self.card.loc.y + self.card.size.h - self.lift(travel)
    }
}

/// How the panel is moving: opening or closing, and how long since it started.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Way {
    Opening { since: f64 },
    Closing { since: f64 },
}

/// A panel as drawn this frame: its opacity, and whether it has settled, which is when anything
/// laid over it, such as the explorer's caret, can be drawn too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reveal {
    pub alpha: f32,
    look: Look,
    way: Way,
}

impl Reveal {
    /// Whether it is drawn as its own picture: settled, or fading with reduced motion.
    pub fn whole(&self) -> bool {
        !self.look.lit || self.settled()
    }

    pub fn settled(&self) -> bool {
        match self.way {
            Way::Opening { since } => {
                self.look.alpha >= 1.0 && (!self.look.lit || since >= OPEN + LAND_BY)
            }
            Way::Closing { .. } => false,
        }
    }

    /// The seam's ends' share of the way from the button to the card's sides, and its strength.
    fn seam(&self) -> (f64, f32) {
        match self.way {
            Way::Opening { since } => {
                let settling = ((since / (OPEN + LAND_BY)).clamp(0.0, 1.0) as f32).powi(4);
                (
                    in_out(since / SLOT_OPEN),
                    SEAM_REST + (1.0 - SEAM_REST) * (1.0 - settling),
                )
            }
            Way::Closing { since } => {
                let q = since / CLOSE;
                let glint = ((q - (1.0 - GLINT)) / GLINT).clamp(0.0, 1.0);
                (
                    in_out((1.0 - q) / SEAL),
                    SEAM_CLOSING
                        + (1.0 - SEAM_CLOSING) * (std::f64::consts::PI * glint).sin() as f32,
                )
            }
        }
    }
}

/// The ids a panel's elements are drawn under, which last as long as the picture they were made
/// for: the bare card, the edge, the seam, the scanlines, two layers riding along, then three
/// for each piece, its crop and its red and blue.
const CARD_ID: usize = 0;
const EDGE_ID: usize = 1;
const SEAM_ID: usize = 2;
const SCAN_ID: usize = 3;
const RIDER_IDS: usize = 4;
const RIDERS: usize = 2;
const PIECE_IDS: usize = RIDER_IDS + RIDERS;

/// A panel's way in and out. It opens from nothing, or from wherever a close it interrupts has
/// got to, and closes by taking the picture it last drew back into the slot. Neither holds
/// anything up: the panel stops taking keys and clicks the moment it closes, and this only paints.
#[derive(Default)]
pub struct Fade {
    /// The middle of the button that opened it, across the screen; the card's middle if none.
    button: Option<f64>,
    /// What the opening starts from, and what was last drawn.
    from: Look,
    drawn: Look,
    /// Each piece's growth as the opening started, and as last drawn, 0 to 1.
    from_grown: Vec<f32>,
    grown: Vec<f32>,
    drawer: Drawer,
    closing: Option<Closing>,
    /// The ids its elements are drawn under, the picture they were made for, and that picture's
    /// bare card and its red and blue, made once each when first needed.
    ids: Vec<Id>,
    made_for: u64,
    card: Option<MemoryRenderBuffer>,
    split: Option<[MemoryRenderBuffer; 2]>,
}

/// A panel going back in: the picture it closed on, front first, and when the close started on
/// the animation clock, which the first frame after it fills in.
struct Closing {
    picture: Vec<Layer>,
    started: Option<f64>,
}

impl Fade {
    /// Opens again: from where it is drawn if it was still going, otherwise from nothing.
    pub fn open(&mut self) {
        if self.closing.take().is_some() {
            self.from = self.drawn;
            self.from_grown.clone_from(&self.grown);
        } else {
            self.from = GONE;
            self.from_grown.iter_mut().for_each(|grown| *grown = 0.0);
            self.grown.iter_mut().for_each(|grown| *grown = 0.0);
        }
    }

    /// Takes it as drawn `since` seconds after it opened, without drawing it.
    #[cfg(test)]
    pub fn drawn_at(&mut self, since: f64, reduced_motion: bool) {
        self.drawn = opening_look(since, self.from, reduced_motion);
    }

    /// Where the button that opened it is: its middle, in logical pixels from the screen's left.
    /// The slot opens from there. Without one it opens from the card's middle.
    pub fn hang_from(&mut self, button: Option<f64>) {
        self.button = button;
    }

    /// Draws the panel `since` seconds after it opened into `out`, front first: `layers`, front
    /// first, with its own picture last, over its card at `card` on the screen, coming out of the
    /// bar in `frame`.
    #[allow(clippy::too_many_arguments)]
    pub fn show<R>(
        &mut self,
        renderer: &mut R,
        frame: crate::panel::Frame,
        since: f64,
        reduced_motion: bool,
        card: Rectangle<f64, Logical>,
        layers: &[Shown<'_>],
        out: &mut impl Extend<FlowElement<R>>,
    ) -> Reveal
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let look = opening_look(since, self.from, reduced_motion);
        self.drawn = look;
        self.drawer = Drawer::new(card, frame.bar.foot() as f64);
        let reveal = Reveal {
            alpha: look.alpha,
            look,
            way: Way::Opening { since },
        };
        self.draw(renderer, reveal, frame.ring, frame.time, layers, out);
        reveal
    }

    /// Closes, keeping `picture`, what it last drew, to take back into the slot; `None` when
    /// nothing is drawn. Closing again while it goes changes nothing. Returns whether it goes.
    pub fn close(&mut self, picture: Option<Vec<Layer>>) -> bool {
        if self.closing.is_some() {
            return true;
        }
        self.from = self.drawn;
        self.from_grown.clone_from(&self.grown);
        match picture {
            Some(picture) if !picture.is_empty() && self.drawn.alpha > 0.0 => {
                self.closing = Some(Closing {
                    picture,
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

    /// Whether it is still going after closing.
    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }

    /// How it shows at `now` while it goes. `None` once it has gone, and the picture is let go.
    fn closing(&mut self, now: f64, reduced_motion: bool) -> Option<Reveal> {
        let closing = self.closing.as_mut()?;
        let started = *closing.started.get_or_insert(now);
        let since = now - started;
        match closing_look(self.from, since, reduced_motion) {
            Some(look) => {
                self.drawn = look;
                Some(Reveal {
                    alpha: look.alpha,
                    look,
                    way: Way::Closing { since },
                })
            }
            None => {
                self.closing = None;
                self.drawn = GONE;
                self.grown.iter_mut().for_each(|grown| *grown = 0.0);
                self.card = None;
                self.split = None;
                None
            }
        }
    }

    /// Its picture at `now` while it goes, front first, with the edge and seam in `ring`.
    pub fn draw_closing<R>(
        &mut self,
        renderer: &mut R,
        now: f64,
        ring: u32,
        reduced_motion: bool,
        out: &mut impl Extend<FlowElement<R>>,
    ) -> Option<Reveal>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let reveal = self.closing(now, reduced_motion)?;
        let picture = std::mem::take(&mut self.closing.as_mut()?.picture);
        let count = picture.len().min(RIDERS + 1);
        let start = picture.len() - count;
        let layers: [Shown<'_>; RIDERS + 1] = std::array::from_fn(|n| {
            let (painted, at, share) = &picture[start + n.min(count.max(1) - 1)];
            (painted, *at, *share)
        });
        self.draw(renderer, reveal, ring, now, &layers[..count], out);
        if let Some(closing) = self.closing.as_mut() {
            closing.picture = picture;
        }
        Some(reveal)
    }

    /// The panel as `reveal` says, its edge and seam in `ring` on the tube's clock at `now`.
    fn draw<R>(
        &mut self,
        renderer: &mut R,
        reveal: Reveal,
        ring: u32,
        now: f64,
        layers: &[Shown<'_>],
        out: &mut impl Extend<FlowElement<R>>,
    ) where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(&(main, at, _)) = layers.last() else {
            return;
        };
        let look = reveal.look;
        // Reduced motion: each picture whole, fading.
        if !look.lit {
            for (painted, at, share) in layers {
                out.extend(
                    painted
                        .element(renderer, *at, look.alpha * share)
                        .map(FlowElement::from),
                );
            }
            return;
        }
        self.made_for_picture(main);
        let scale = main.scale;
        let ring = ring & 0xffffff00;
        let drawer = self.drawer;
        // The seam in the bar's foot, from the button out to the card's sides.
        let (spread, strength) = reveal.seam();
        let button = self
            .button
            .unwrap_or(drawer.card.loc.x + drawer.card.size.w / 2.0);
        let left = button - (button - drawer.card.loc.x) * spread;
        let right = button + (drawer.card.loc.x + drawer.card.size.w - button) * spread;
        let seam = (strength, left, right);
        if reveal.settled() {
            self.card = None;
            self.split = None;
            self.grown.iter_mut().for_each(|grown| *grown = 1.0);
            out.extend(self.strip(renderer, SEAM_ID, ring, scale, seam, drawer.slot));
            for (painted, at, share) in layers {
                out.extend(
                    painted
                        .element(renderer, *at, look.alpha * share)
                        .map(FlowElement::from),
                );
            }
            return;
        }
        let lift = drawer.lift(look.travel);
        let lead = drawer.lead(look.travel);
        let moving = look.travel < 1.0;
        let (flicker, torn) = fx::tube(now, TEAR_CHANCE);
        // The lit edge on the drawer's leading edge, while it moves and is out of the slot.
        if moving && lead > drawer.slot + 1.0 {
            let edge = (
                look.alpha * flicker,
                drawer.card.loc.x + INSET + torn as f64 * TEAR_REACH,
                drawer.card.loc.x + drawer.card.size.w - INSET + torn as f64 * TEAR_REACH,
            );
            out.extend(self.strip(renderer, EDGE_ID, ring, scale, edge, lead));
        }
        let seam = (seam.0 * if moving { flicker } else { 1.0 }, seam.1, seam.2);
        out.extend(self.strip(renderer, SEAM_ID, ring, scale, seam, drawer.slot));
        // Faint scanlines over the drawer while it moves.
        if moving {
            out.extend(self.scanlines(renderer, scale, drawer, lift));
        }
        // The pieces, each growing in or shrinking away.
        let whole = [Rectangle::new(drawer.card.loc - at, drawer.card.size)];
        let rects = pieces(main, &whole);
        if self.grown.len() != rects.len() {
            self.grown.resize(rects.len(), 0.0);
        }
        if self.from_grown.len() != rects.len() {
            self.from_grown.resize(rects.len(), 0.0);
        }
        let mut total = 0.0;
        for (n, rect) in rects.iter().enumerate() {
            let grown = match reveal.way {
                Way::Opening { since } => {
                    let out_at = drawer.out_at(at.y + rect.loc.y + rect.size.h);
                    let start = piece_starts(out_at, self.from.travel, column(rects, n));
                    grown_opening(self.from_grown[n], start, since)
                }
                Way::Closing { since } => {
                    let in_at = drawer.out_at(at.y + rect.loc.y);
                    let gone = closing_reaches(in_at, self.from.travel);
                    grown_closing(self.from_grown[n], gone, since)
                }
            };
            self.grown[n] = grown;
            total += grown;
            if grown > 0.0 {
                let piece = Piece {
                    rect: *rect,
                    grown,
                    at,
                    lift,
                    salt: n,
                };
                self.piece(renderer, main, piece, look.alpha, now, out);
            }
        }
        // Anything riding along comes in with the pieces as a whole.
        let riders = (total / rects.len() as f32).clamp(0.0, 1.0);
        for (n, (painted, at, share)) in layers[..layers.len() - 1].iter().enumerate().take(RIDERS)
        {
            let id = &self.ids[RIDER_IDS + n];
            let alpha = look.alpha * share * riders;
            out.extend(
                slid(
                    renderer,
                    &painted.buffer,
                    painted,
                    *at,
                    lift,
                    drawer.slot,
                    alpha,
                )
                .map(|element| FlowElement::with_id(element, id)),
            );
        }
        // The bare card at the back.
        if self.card.is_none() {
            self.card = main.pieces.glass.and_then(|[x, y, w, h, f]| {
                Painted::new(main.logical, scale, |p| {
                    p.f = f;
                    crate::panel::glass(p, x, y, w, h);
                })
                .map(|card| card.buffer)
            });
        }
        if let Some(card) = self.card.as_ref() {
            out.extend(
                slid(renderer, card, main, at, lift, drawer.slot, look.alpha)
                    .map(|element| FlowElement::with_id(element, &self.ids[CARD_ID])),
            );
        }
    }

    /// Fresh ids, and a fresh bare card and colours, for a picture other than the last one.
    fn made_for_picture(&mut self, main: &Painted) {
        let serial = main.pieces.serial;
        let wanted = PIECE_IDS + 3 * main.pieces.rects.len().clamp(1, MOST_PIECES);
        if self.made_for == serial && self.ids.len() >= wanted && serial != 0 {
            return;
        }
        self.made_for = serial;
        self.ids.clear();
        self.ids.resize_with(wanted, Id::new);
        self.card = None;
        self.split = None;
    }

    /// One of the strips, `which`, in `ring` at `scale`: its strength and its left and right ends,
    /// centred on `y`.
    fn strip<R>(
        &self,
        renderer: &mut R,
        which: usize,
        ring: u32,
        scale: f64,
        (strength, left, right): (f32, f64, f64),
        y: f64,
    ) -> Option<FlowElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if strength <= 0.0 {
            return None;
        }
        let at = Point::<f64, Logical>::from((left, y - STRIP_H / 2.0));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            &strip(ring, scale),
            Some(strength.min(1.0)),
            None,
            Some(((right - left).round().max(2.0) as i32, STRIP_H as i32).into()),
            Kind::Unspecified,
        )
        .ok()
        .map(|element| FlowElement::with_id(element, &self.ids[which]))
    }

    /// The scanlines over the drawer `lift` above its place, on whole screen rows: only over its
    /// part out of the slot, and kept `CORNER` from its top and foot so they stay inside its
    /// rounded corners.
    fn scanlines<R>(
        &self,
        renderer: &mut R,
        scale: f64,
        drawer: Drawer,
        lift: f64,
    ) -> Option<FlowElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let card = drawer.card;
        let top = (drawer.slot.max(card.loc.y - lift + CORNER) * scale).round();
        let bottom = ((card.loc.y + card.size.h - lift - CORNER) * scale).round();
        let rows = (bottom - top).min(SCAN_ROWS as f64 - 2.0);
        if rows < 1.0 {
            return None;
        }
        let left = (card.loc.x * scale).round();
        let shown = Size::<i32, Logical>::from((
            card.size.w.round() as i32,
            (rows / scale).round().max(1.0) as i32,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::<f64, Physical>::from((left, top)),
            &scan(),
            None,
            Some(Rectangle::new(
                (0.0, top.rem_euclid(2.0)).into(),
                (STRIP_W as f64, rows).into(),
            )),
            Some(shown),
            Kind::Unspecified,
        )
        .ok()
        .map(|element| FlowElement::with_id(element, &self.ids[SCAN_ID]))
    }

    /// One piece of `main`, with its colours split while it powers up or down.
    fn piece<R>(
        &mut self,
        renderer: &mut R,
        main: &Painted,
        piece: Piece,
        alpha: f32,
        now: f64,
        out: &mut impl Extend<FlowElement<R>>,
    ) where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(crop) = piece.crop(main, self.drawer.slot) else {
            return;
        };
        let power = 1.0 - piece.grown;
        let flicker = if power > 0.125 {
            fx::tube(now + piece.salt as f64 * 0.37, 0.0).0
        } else {
            1.0
        };
        let alpha = alpha * (piece.grown * 1.4).min(1.0) * flicker;
        let ids = PIECE_IDS + 3 * piece.salt;
        let split = SPLIT_PX * power as f64;
        if split > 0.3 {
            if self.split.is_none() {
                self.split = split_of(&main.buffer, main.device);
            }
            if let Some(colours) = self.split.as_ref() {
                for (n, (colour, side)) in colours.iter().zip([-1.0, 1.0]).enumerate() {
                    let shifted = Crop {
                        at: crop.at + Point::from((side * split, 0.0)),
                        ..crop
                    };
                    out.extend(
                        shifted
                            .element(renderer, colour, main.scale, alpha * SPLIT_ALPHA * power)
                            .map(|element| FlowElement::with_id(element, &self.ids[ids + 1 + n])),
                    );
                }
            }
        }
        out.extend(
            crop.element(renderer, &main.buffer, main.scale, alpha)
                .map(|element| FlowElement::with_id(element, &self.ids[ids])),
        );
    }
}

/// The pieces `main` comes in as: those it was painted with, or `whole`, its card, if it has
/// none or too many.
fn pieces<'a>(
    main: &'a Painted,
    whole: &'a [Rectangle<f64, Logical>; 1],
) -> &'a [Rectangle<f64, Logical>] {
    match main.pieces.rects.as_slice() {
        rects if rects.is_empty() || rects.len() > MOST_PIECES => whole,
        rects => rects,
    }
}

/// How many pieces sit to the left of piece `n` in its row.
fn column(rects: &[Rectangle<f64, Logical>], n: usize) -> usize {
    let rect = rects[n];
    let middle = rect.loc.y + rect.size.h / 2.0;
    rects
        .iter()
        .filter(|other| {
            other.loc.x < rect.loc.x
                && (other.loc.y + other.size.h / 2.0 - middle).abs()
                    < rect.size.h.min(other.size.h) / 2.0
        })
        .count()
}

/// A piece this frame: its box in the picture, how far it has grown, the picture's corner where
/// it rests, how far above that the drawer is, and its place in the list, which keeps its flicker
/// its own.
#[derive(Debug, Clone, Copy)]
struct Piece {
    rect: Rectangle<f64, Logical>,
    grown: f32,
    at: Point<f64, Logical>,
    lift: f64,
    salt: usize,
}

/// Part of a picture drawn somewhere: its box in the picture, in logical pixels, and where on the
/// screen and how big it is drawn.
#[derive(Debug, Clone, Copy)]
struct Crop {
    from: Rectangle<f64, Logical>,
    at: Point<f64, Logical>,
    size: Size<f64, Logical>,
}

impl Piece {
    /// The piece at its size, about its middle, moved with the drawer and cut off at the slot.
    fn crop(&self, main: &Painted, slot: f64) -> Option<Crop> {
        let k = GROW_FROM + (1.0 - GROW_FROM) * self.grown as f64;
        let size = Size::<f64, Logical>::from((self.rect.size.w * k, self.rect.size.h * k));
        let middle = self.at
            + self.rect.loc
            + Point::from((self.rect.size.w / 2.0, self.rect.size.h / 2.0 - self.lift));
        let corner = middle - Point::from((size.w / 2.0, size.h / 2.0));
        let hidden = (slot - corner.y).max(0.0);
        if hidden >= size.h || main.logical.h <= 0 {
            return None;
        }
        Some(Crop {
            from: Rectangle::new(
                self.rect.loc + Point::from((0.0, hidden / k)),
                (self.rect.size.w, self.rect.size.h - hidden / k).into(),
            ),
            at: corner + Point::from((0.0, hidden)),
            size: (size.w, size.h - hidden).into(),
        })
    }
}

impl Crop {
    fn element<R>(
        &self,
        renderer: &mut R,
        buffer: &MemoryRenderBuffer,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let size =
            Size::<i32, Logical>::from((self.size.w.round() as i32, self.size.h.round() as i32));
        if alpha <= 0.0 || size.w <= 0 || size.h <= 0 {
            return None;
        }
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            self.at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            buffer,
            Some(alpha.min(1.0)),
            Some(Rectangle::new(
                (self.from.loc.x * scale, self.from.loc.y * scale).into(),
                (self.from.size.w * scale, self.from.size.h * scale).into(),
            )),
            Some(size),
            Kind::Unspecified,
        )
        .ok()
    }
}

/// `buffer`, the size of `painted`, with its corner at `at` raised by `lift` and cut off above
/// `slot`.
fn slid<R>(
    renderer: &mut R,
    buffer: &MemoryRenderBuffer,
    painted: &Painted,
    at: Point<f64, Logical>,
    lift: f64,
    slot: f64,
    alpha: f32,
) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let logical = painted.logical;
    let top = at.y - lift;
    let hidden = (slot - top).max(0.0).ceil();
    if hidden >= logical.h as f64 {
        return None;
    }
    Crop {
        from: Rectangle::new(
            (0.0, hidden).into(),
            (logical.w as f64, logical.h as f64 - hidden).into(),
        ),
        at: (at.x, top + hidden).into(),
        size: (logical.w as f64, logical.h as f64 - hidden).into(),
    }
    .element(renderer, buffer, painted.scale, alpha)
}

/// The red and the blue of a picture `size` buffer pixels big, each as light added to what's
/// behind, for its split.
fn split_of(buffer: &MemoryRenderBuffer, size: (i32, i32)) -> Option<[MemoryRenderBuffer; 2]> {
    let mut reading = buffer.clone();
    let mut colours = None;
    let mut context = reading.render();
    let _ = context.draw(|pixels: &mut [u8]| {
        let mut red = Vec::with_capacity(pixels.len());
        let mut blue = Vec::with_capacity(pixels.len());
        for pixel in pixels.chunks_exact(4) {
            red.extend_from_slice(&[pixel[0], 0, 0, 0]);
            blue.extend_from_slice(&[0, 0, pixel[2], 0]);
        }
        colours = Some((red, blue));
        Ok::<_, ()>(Vec::new())
    });
    drop(context);
    let (red, blue) = colours?;
    let made = |pixels: &[u8]| {
        MemoryRenderBuffer::from_slice(pixels, Fourcc::Abgr8888, size, 1, Transform::Normal, None)
    };
    Some([made(&red), made(&blue)])
}

thread_local! {
    /// The lit strip and the colour and scale it was baked for, and the scanlines. Every panel
    /// shares them; the compositor draws on one thread.
    static STRIP: RefCell<Option<(u32, f64, MemoryRenderBuffer)>> = const { RefCell::new(None) };
    static SCAN: RefCell<Option<MemoryRenderBuffer>> = const { RefCell::new(None) };
}

/// The lit strip in `ring` at `scale`, baked again only when either changes.
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

/// The scanlines: every other row darkened, baked once.
fn scan() -> MemoryRenderBuffer {
    SCAN.with_borrow_mut(|kept| {
        kept.get_or_insert_with(|| {
            let pixels = scan_pixels();
            MemoryRenderBuffer::from_slice(
                &pixels,
                Fourcc::Abgr8888,
                (STRIP_W as i32, SCAN_ROWS as i32),
                1,
                Transform::Normal,
                None,
            )
        })
        .clone()
    })
}

/// The scanlines' pixels, premultiplied: black at `SCAN_DARK` on even rows, nothing on odd ones.
fn scan_pixels() -> Vec<u8> {
    let dark = (SCAN_DARK * 255.0).round() as u8;
    (0..SCAN_ROWS)
        .flat_map(|row| {
            let pixel = if row % 2 == 0 {
                [0, 0, 0, dark]
            } else {
                [0; 4]
            };
            std::iter::repeat_n(pixel, STRIP_W).flatten()
        })
        .collect()
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

    fn drawer() -> Drawer {
        Drawer::new(
            Rectangle::new((100.0, 40.0).into(), (300.0, 400.0).into()),
            32.0,
        )
    }

    /// Lines across a card, from its top to its foot, where pieces' edges might be.
    fn lines(drawer: Drawer) -> impl Iterator<Item = f64> {
        (0..=40).map(move |n| drawer.card.loc.y + drawer.card.size.h * n as f64 / 40.0)
    }

    #[test]
    fn a_piece_starts_only_once_the_drawer_has_brought_it_out_of_the_slot() {
        let drawer = drawer();
        for bottom in lines(drawer) {
            let out_at = drawer.out_at(bottom);
            let start = piece_starts(out_at, 0.0, 0);
            // Its foot is level with the slot when the drawer is out that far.
            let foot = bottom - drawer.lift(out_at);
            assert!((foot - drawer.slot).abs() < 0.01, "{foot}");
            assert!(travel_opening(0.0, start) >= out_at - 1e-4);
            if start > 0.0 {
                assert!(travel_opening(0.0, start - 0.005) < out_at);
                assert_eq!(grown_opening(0.0, start, start - 0.005), 0.0);
            }
            assert!(grown_opening(0.0, start, start + 0.01) > 0.0);
        }
    }

    #[test]
    fn every_piece_has_landed_a_tenth_of_a_second_after_the_drawer() {
        let drawer = drawer();
        for bottom in lines(drawer) {
            for column in 0..6 {
                let start = piece_starts(drawer.out_at(bottom), 0.0, column);
                assert_eq!(grown_opening(0.0, start, OPEN + LAND_BY), 1.0);
            }
        }
        assert_eq!(travel_opening(0.0, OPEN), 1.0, "the drawer is in its place");
    }

    #[test]
    fn pieces_in_a_row_start_left_to_right() {
        let row = [
            Rectangle::new((0.0, 10.0).into(), (50.0, 40.0).into()),
            Rectangle::new((60.0, 10.0).into(), (50.0, 40.0).into()),
            Rectangle::new((0.0, 60.0).into(), (110.0, 40.0).into()),
        ];
        assert_eq!(
            (column(&row, 0), column(&row, 1), column(&row, 2)),
            (0, 1, 0)
        );
        let out_at = drawer().out_at(200.0);
        let gap = piece_starts(out_at, 0.0, 1) - piece_starts(out_at, 0.0, 0);
        assert!((gap - STAGGER).abs() < 1e-9);
    }

    #[test]
    fn closing_each_piece_has_gone_before_the_slot_reaches_it() {
        let drawer = drawer();
        for top in lines(drawer) {
            let in_at = drawer.out_at(top);
            let gone = closing_reaches(in_at, 1.0);
            assert!(gone <= CLOSE);
            // Where its top is when it has gone: at the slot.
            assert!(top - drawer.lift(travel_closing(1.0, gone)) <= drawer.slot + 0.01);
            assert_eq!(grown_closing(1.0, gone, gone), 0.0);
            if gone > 0.0 {
                let before = gone * 0.98;
                assert!(top - drawer.lift(travel_closing(1.0, before)) > drawer.slot);
            }
            if gone > LEAVE {
                assert_eq!(
                    grown_closing(1.0, gone, gone - LEAVE),
                    1.0,
                    "not before its time"
                );
            }
        }
    }

    #[test]
    fn a_closed_panel_lets_go_at_once_and_only_its_picture_goes_back_in() {
        let mut fade = Fade::default();
        fade.open();
        fade.drawn_at(OPEN, false);
        assert_eq!(fade.drawn.travel, 1.0);
        assert!(fade.close(picture()), "closing waits for nothing");
        assert!(fade.is_closing());
        assert_eq!(fade.closing(5.0, false).unwrap().look.travel, 1.0);
        let halfway = fade.closing(5.0 + CLOSE / 2.0, false).unwrap();
        assert_eq!(halfway.alpha, 1.0, "it goes back in, never fades");
        assert!(halfway.look.travel > 0.0 && halfway.look.travel < 1.0);
        assert_eq!(fade.closing(5.0 + CLOSE + 0.001, false), None);
        assert!(!fade.is_closing(), "and then the picture is let go");
    }

    #[test]
    fn a_panel_closed_before_it_was_drawn_has_nothing_to_take_back() {
        let mut fade = Fade::default();
        fade.open();
        assert!(!fade.close(picture()));
        assert!(!fade.close(None));
    }

    #[test]
    fn reopened_mid_close_it_carries_on_from_where_it_is_drawn() {
        let mut fade = Fade::default();
        fade.drawn_at(OPEN + LAND_BY, false);
        fade.grown = vec![1.0, 0.6];
        fade.close(picture());
        fade.closing(0.0, false);
        let drawn = fade.closing(CLOSE * 0.6, false).unwrap();
        fade.grown = vec![0.4, 0.0];
        fade.open();
        assert!(!fade.is_closing());
        assert_eq!(fade.from, drawn.look);
        assert_eq!(fade.from_grown, [0.4, 0.0], "each piece too");
        assert_eq!(opening_look(0.0, fade.from, false), drawn.look);
        assert_eq!(grown_opening(0.4, 0.05, 0.0), 0.4);
        assert_eq!(opening_look(OPEN, fade.from, false).travel, 1.0);
    }

    #[test]
    fn the_drawer_only_comes_out_opening_and_only_goes_in_closing() {
        let steps = 40;
        let mut last = -1.0;
        for n in 0..=steps {
            let travel = travel_opening(0.0, OPEN * n as f64 / steps as f64);
            assert!(travel >= last, "{travel} after {last}");
            last = travel;
        }
        let mut last = 2.0;
        for n in 0..=steps {
            let travel = travel_closing(1.0, CLOSE * n as f64 / steps as f64);
            assert!(travel <= last, "{travel} after {last}");
            last = travel;
        }
        assert_eq!(last, 0.0, "close ends in the slot");
    }

    #[test]
    fn the_seam_is_bright_while_the_drawer_moves_dim_at_rest_and_glints_shut() {
        let at = |way| Reveal {
            alpha: 1.0,
            look: opening_look(0.0, GONE, false),
            way,
        };
        let (spread, moving) = at(Way::Opening { since: OPEN / 2.0 }).seam();
        assert_eq!(spread, 1.0, "out to the card's sides after the slot opens");
        assert!(moving > 0.9);
        assert_eq!(
            at(Way::Opening { since: 0.0 }).seam().0,
            0.0,
            "from the button"
        );
        let rest = at(Way::Opening {
            since: OPEN + LAND_BY,
        });
        assert_eq!(rest.seam().1, SEAM_REST);
        let glint = at(Way::Closing {
            since: CLOSE * (1.0 - GLINT / 2.0),
        });
        assert!(glint.seam().1 > 0.99);
        assert_eq!(at(Way::Closing { since: CLOSE }).seam().0, 0.0, "sealed");
    }

    #[test]
    fn with_reduced_motion_it_only_fades_and_no_longer_than_the_short_fade() {
        let opened = opening_look(REDUCED_FADE / 2.0, GONE, true);
        assert_eq!((opened.alpha, opened.travel, opened.lit), (0.5, 1.0, false));
        let mut fade = Fade::default();
        fade.drawn_at(REDUCED_FADE, true);
        fade.close(picture());
        assert_eq!(fade.closing(0.0, true).unwrap().alpha, 1.0);
        let half = fade.closing(REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!((half.alpha, half.look.lit), (0.5, false));
        assert!(half.whole(), "drawn as its own picture, no pieces");
        assert_eq!(fade.closing(REDUCED_FADE, true), None);
    }

    #[test]
    fn reduced_motion_switched_on_mid_close_turns_it_into_the_short_fade() {
        let mut fade = Fade::default();
        fade.drawn_at(OPEN, false);
        fade.close(picture());
        assert!(fade.closing(0.0, false).unwrap().look.lit);
        let now = fade.closing(REDUCED_FADE / 2.0, true).unwrap();
        assert!(!now.look.lit && now.whole());
        assert_eq!(fade.closing(REDUCED_FADE, true), None);
    }

    #[test]
    fn a_picture_painted_without_pieces_comes_in_as_one() {
        let card = [Rectangle::new((8.0, 8.0).into(), (40.0, 30.0).into())];
        let bare = Painted::new((64, 64).into(), 1.25, |_| {}).unwrap();
        assert_eq!(pieces(&bare, &card), &card);
        let tiled = Painted::new((64, 64).into(), 1.25, |p| {
            p.piece(10.0, 10.0, 20.0, 8.0);
            p.piece(32.0, 10.0, 20.0, 8.0);
        })
        .unwrap();
        let rects = pieces(&tiled, &card);
        assert_eq!(rects.len(), 2);
        // Recorded on whole device pixels, in logical ones.
        let first = Rectangle::<f64, Logical>::new((9.6, 9.6).into(), (20.8, 8.8).into());
        assert!((rects[0].loc.x - first.loc.x).abs() < 1e-9);
        assert!((rects[0].size.w - first.size.w).abs() < 1e-9);
        assert!((rects[0].size.h - first.size.h).abs() < 1e-9);
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

    #[test]
    fn the_scanlines_are_faint_and_on_every_other_row() {
        let pixels = scan_pixels();
        let row = |n: usize| &pixels[n * STRIP_W * 4..n * STRIP_W * 4 + 4];
        assert_eq!(row(1), [0; 4]);
        assert_eq!(&row(0)[..3], [0, 0, 0]);
        let dark = row(0)[3] as f32 / 255.0;
        assert!(dark > 0.0 && dark <= (1.0 - fx::SCANLINE) / 2.0 + 0.01);
    }
}

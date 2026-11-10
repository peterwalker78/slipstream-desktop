//! How every panel and card comes and goes: as a drawer out of a lit slot.
//!
//! A panel that hangs from the bar comes out of the bar's own foot line: the line brightens in the
//! focus ring's colour from the button that opened it out to the panel's width, and the bar draws
//! that light itself (`BarSlot`). A card that stands on its own comes out of a slot of its own: a
//! thin lit line along its top edge, spreading from its middle to its width. Either way the bare
//! card slides down out of the line, clipped at it, with a lit edge riding its leading edge, and
//! the line goes out as the card lands. Then its pieces (a tile, a row, a group of buttons) grow
//! into their places in reading order, powering up like a picture on a tube, with a flicker and a
//! red and blue split that settle as each lands. Closing, the pieces go first, the card follows
//! them back into the line, and the line seals with a glint. Faint scanlines lie over the drawer
//! while it moves.
//!
//! Everything is drawn from pictures the panel already has: the card is painted bare once per
//! picture, each piece is a crop of the panel's picture, and the edge, the line and the scanlines
//! are small strips baked once and shared by every panel. Nothing is repainted per frame, and once
//! a panel has settled it is its own picture again.
//!
//! With reduced motion there is no slot, drawer, piece or edge: a panel only fades, no longer
//! than `REDUCED_FADE`.

use std::cell::{Cell, RefCell};

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

/// The bare card comes out over 0.3 s on a glide.
const CARD: f64 = 0.3;
/// The pieces start coming in this long after the panel opens, one after another in reading
/// order, their starts spread across `CONTENT` so that each takes `SPAN` of it.
const START: f64 = 0.05;
const CONTENT: f64 = 0.3;
const SPAN: f64 = 0.45;
/// Everything has landed this long after opening.
const LANDED: f64 = START + CONTENT;
/// Closing, the pieces go over `LEAVE`, and the card follows them back in from `BACK_AT` over
/// `BACK`, easing in and out. Only the paint lingers: input goes to what's underneath the moment
/// it closes.
const LEAVE: f64 = 0.2;
const BACK_AT: f64 = 0.16;
const BACK: f64 = 0.22;
const CLOSE: f64 = BACK_AT + BACK;
/// The slot's line spreads from where it opens to the card's width over its first 100 ms, and goes
/// out over `SLOT_OUT` once the card has landed. Closing, it lights again while the pieces go,
/// narrows back over the last `SEAL` of the card's way in, and glints over the last `GLINT`.
const SLOT_OPEN: f64 = 0.1;
const SLOT_OUT: f64 = 0.15;
const SEAL: f64 = 0.45;
const GLINT: f64 = 0.3;
/// How much brighter than while the drawer moves the line is at its glint's peak.
const GLINT_LIGHT: f32 = 0.5;
/// The most a frame moves the animation on, in seconds: a slow frame, such as the first, which
/// paints the panel, holds the motion up rather than skipping part of it.
const MOST_STEP: f64 = 0.034;

/// Each piece grows from `GROW_FROM` of its size to all of it.
const GROW_FROM: f64 = 0.82;
/// A piece is a picture on a tube while it powers up or down: fully so for the first `TUBE_HOLD`
/// of its way, settling to clean by `TUBE_CLEAN`, so a piece at rest is pristine.
const TUBE_HOLD: f64 = 0.55;
const TUBE_CLEAN: f64 = 0.95;
/// On about `DIM_CHANCE` of the tube's frames a piece dims to `DIM` of its brightness.
const DIM_CHANCE: f32 = 0.38;
const DIM: f32 = 0.64;
/// About one of the tube's frames in `PIECE_TEAR_CHANCE` jerks a piece sideways by one to
/// `PIECE_TEAR` logical pixels.
const PIECE_TEAR_CHANCE: f32 = 0.26;
const PIECE_TEAR: f64 = 3.0;
/// A piece's colours split by up to this many logical pixels while it powers up or down.
const SPLIT_PX: f64 = 2.5;
/// How strong each split colour is at its widest.
const SPLIT_ALPHA: f32 = 0.28;
/// The split colours are made at this fraction of the picture's size each way.
const SPLIT_SCALE: usize = 2;
/// The scanlines over the moving drawer: as dark as the edge's.
const SCAN_DARK: f32 = 1.0 - fx::SCANLINE;
/// The card's corner radius in logical pixels: the scanlines keep this far from its top and foot,
/// so they never show past its rounded corners.
const CORNER: f64 = 18.0 * crate::panel::DESIGN_PX as f64;
/// More pieces than this and the picture comes in whole.
const MOST_PIECES: usize = 32;

/// The lit edge: a line `LINE` logical pixels thick, `INSET` in from the card's sides, with a
/// glow reaching `GLOW` above and below it.
const LINE: f64 = 2.0;
const INSET: f64 = 6.0;
const GLOW: f64 = 14.0;
const STRIP_H: f64 = 2.0 * GLOW + LINE;
/// The strips' width in pixels: every column is the same, so they are stretched along the card.
const STRIP_W: usize = 4;
/// The slot's line: `SLOT_W` pixels baked once and stretched along the slot, fading in and out
/// over `SLOT_TAPER` of its length at either end, how far its colour is taken towards white, and
/// how opaque it is while the drawer moves, so its glint can brighten it further.
const SLOT_W: usize = 128;
const SLOT_TAPER: f64 = 0.15;
const SLOT_LIGHT: f32 = 0.15;
const SLOT_ALPHA: f32 = 0.75;
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
/// draws in a frame. By default there is room for all of it, every piece split in two colours.
pub struct Drawn<R: Renderer, const N: usize = { PIECE_IDS + 3 * MOST_PIECES + 2 }> {
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
    from + (1.0 - from) * glide(since / CARD) as f32
}

/// How far out the drawer is `since` seconds into a closing that started at `from`: still, while
/// the pieces go, then back in.
fn travel_closing(from: f32, since: f64) -> f32 {
    from * (1.0 - in_out((since - BACK_AT) / BACK)) as f32
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

/// Where the piece `order`th in line of `count` starts, as a share of the pieces' time: the first
/// at once, the last with just `SPAN` of it left.
fn piece_start(order: usize, count: usize) -> f64 {
    if count <= 1 {
        0.0
    } else {
        order as f64 / (count - 1) as f64 * (1.0 - SPAN)
    }
}

/// How far the piece `order`th in reading order of `count` is through its way in, 0 to 1, `since`
/// seconds into an opening.
fn way_in(order: usize, count: usize, since: f64) -> f64 {
    let pieces = ((since - START) / CONTENT).clamp(0.0, 1.0);
    ((pieces - piece_start(order, count)) / SPAN).clamp(0.0, 1.0)
}

/// How far the piece `order`th in reading order of `count` still is from gone, 1 to 0, `since`
/// seconds into a closing. The pieces' time runs backwards and their order is turned round, so
/// the first in reading order goes first.
fn way_out(order: usize, count: usize, since: f64) -> f64 {
    let pieces = 1.0 - (since / LEAVE).clamp(0.0, 1.0);
    let order = count.saturating_sub(1).saturating_sub(order);
    ((pieces - piece_start(order, count)) / SPAN).clamp(0.0, 1.0)
}

/// How much of a picture on a tube a piece `way` through its way in or out still is: all of it
/// at first, nothing once it has landed.
fn tube(way: f64) -> f32 {
    if way <= 0.0 {
        return 0.0;
    }
    let t = ((way - TUBE_HOLD) / (TUBE_CLEAN - TUBE_HOLD)).clamp(0.0, 1.0);
    (1.0 - t * t * (3.0 - 2.0 * t)) as f32
}

/// How far a piece has grown, from `from` when the opening started, `way` through its way in.
fn grown_opening(from: f32, way: f64) -> f32 {
    from + (1.0 - from) * Easing::OutCubic.at(way) as f32
}

/// How far a piece has grown, from `from` when the closing started, with `way` of it left.
fn grown_closing(from: f32, way: f64) -> f32 {
    from * in_out(way) as f32
}

/// The order pieces come in, reading the way text does: rows from the top, each from the left.
/// A piece whose top is within `ROW` of the first in a row is in that row. Each piece's place in
/// that order goes in `order`.
fn reading_order(rects: &[Rectangle<f64, Logical>], order: &mut Vec<usize>) {
    const ROW: f64 = 16.0;
    let mut line: Vec<usize> = (0..rects.len()).collect();
    line.sort_by(|&a, &b| rects[a].loc.y.total_cmp(&rects[b].loc.y));
    let mut start = 0;
    while start < line.len() {
        let top = rects[line[start]].loc.y;
        let end = start
            + line[start..]
                .iter()
                .take_while(|&&n| rects[n].loc.y - top < ROW)
                .count();
        line[start..end].sort_by(|&a, &b| rects[a].loc.x.total_cmp(&rects[b].loc.x));
        start = end;
    }
    order.clear();
    order.resize(rects.len(), 0);
    for (place, &n) in line.iter().enumerate() {
        order[n] = place;
    }
}

/// Where the drawer runs: the card where it rests, the line it comes out of, and the foot of the
/// card's picture, shadow and all. A panel hanging from the bar comes out of the bar's foot (or
/// the card's top, if that is higher); a card of its own comes out of its own top edge.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Drawer {
    card: Rectangle<f64, Logical>,
    slot: f64,
    bottom: f64,
}

impl Drawer {
    fn new(card: Rectangle<f64, Logical>, foot: Option<f64>, bottom: f64) -> Self {
        Self {
            card,
            slot: foot.map_or(card.loc.y, |foot| foot.min(card.loc.y)),
            bottom: bottom.max(card.loc.y + card.size.h),
        }
    }

    /// How far the drawer runs, from all in the slot, its shadow too, to its place.
    fn span(&self) -> f64 {
        (self.bottom - self.slot).max(1.0)
    }

    /// How far above its place the drawer is, at `travel`.
    fn lift(&self, travel: f32) -> f64 {
        (1.0 - travel as f64) * self.span()
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
            Way::Opening { since } => self.look.alpha >= 1.0 && (!self.look.lit || since >= LANDED),
            Way::Closing { .. } => false,
        }
    }

    /// The slot's line: its ends' share of the way from where it opens out to the card's sides,
    /// and its strength, 0 for none, 1 while the drawer moves, more in its glint. It lights as the
    /// drawer starts and goes out once the card has landed; closing, it lights again while the
    /// pieces go and narrows shut with a glint as the card goes back in.
    fn slot(&self) -> (f64, f32) {
        if !self.look.lit {
            return (0.0, 0.0);
        }
        match self.way {
            Way::Opening { since } => (
                in_out(since / SLOT_OPEN),
                1.0 - in_out((since - CARD) / SLOT_OUT) as f32,
            ),
            Way::Closing { since } => {
                let back = ((since - BACK_AT) / BACK).clamp(0.0, 1.0);
                let glint = ((back - (1.0 - GLINT)) / GLINT).clamp(0.0, 1.0);
                (
                    in_out((1.0 - back) / SEAL),
                    in_out(since / BACK_AT) as f32
                        + GLINT_LIGHT * (std::f64::consts::PI * glint).sin() as f32,
                )
            }
        }
    }
}

/// A panel's slot in the bar's foot line this frame: from `left` to `right` in logical pixels
/// across the screen, and how strongly lit, 1 while the drawer moves and more in its glint. The
/// bar draws it, so the line has one owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSlot {
    pub left: f64,
    pub right: f64,
    pub strength: f32,
}

thread_local! {
    /// The bar slot the panels drawn so far this frame asked for. The compositor draws on one
    /// thread, the panels before the bar.
    static BAR_SLOT: Cell<Option<BarSlot>> = const { Cell::new(None) };
}

/// Asks for `slot` in the bar's foot line this frame; the strongest asked for wins.
fn light_bar_slot(slot: BarSlot) {
    BAR_SLOT.with(|kept| {
        if kept
            .get()
            .is_none_or(|other| other.strength < slot.strength)
        {
            kept.set(Some(slot));
        }
    });
}

/// The bar slot asked for since this was last called, to draw on the screen the panels were
/// drawn on.
pub fn take_bar_slot() -> Option<BarSlot> {
    BAR_SLOT.with(Cell::take)
}

/// The ids a panel's elements are drawn under, which last as long as the picture they were made
/// for: the bare card, the edge, the slot, the scanlines, two layers riding along, then three
/// for each piece, its crop and its red and blue.
const CARD_ID: usize = 0;
const EDGE_ID: usize = 1;
const SLOT_ID: usize = 2;
const SCAN_ID: usize = 3;
const RIDER_IDS: usize = 4;
const RIDERS: usize = 2;
const PIECE_IDS: usize = RIDER_IDS + RIDERS;

/// A panel's way in and out. It opens from nothing, or from wherever a close it interrupts has
/// got to, and closes by taking the picture it last drew back into the slot. Neither holds
/// anything up: the panel stops taking keys and clicks the moment it closes, and this only paints.
#[derive(Default)]
pub struct Fade {
    /// Whether it hangs from the bar, and the middle of the button it opened from, across the
    /// screen; the card's middle if none.
    hangs: bool,
    button: Option<f64>,
    /// What the opening starts from, and what was last drawn.
    from: Look,
    drawn: Look,
    /// Each piece's growth as the opening started, and as last drawn, 0 to 1.
    from_grown: Vec<f32>,
    grown: Vec<f32>,
    /// Each piece's place in reading order, worked out once per picture.
    order: Vec<usize>,
    drawer: Drawer,
    closing: Option<Closing>,
    /// The animation's own time, last drawn, and the clock's time it was drawn at.
    paced: Option<(f64, f64)>,
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
        self.paced = None;
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

    /// Hangs it from the bar, under the button whose middle is at `button`, in logical pixels
    /// from the screen's left: it comes out of the bar's foot line, spreading from there. Without
    /// a button it spreads from the card's middle. A panel that doesn't hang from the bar comes
    /// out of its own top edge.
    pub fn hang_from(&mut self, button: Option<f64>) {
        self.hangs = true;
        self.button = button;
    }

    /// `since`, `raw` seconds in by the clock, moved on from the last frame by no more than
    /// `MOST_STEP`.
    fn pace(&mut self, raw: f64) -> f64 {
        let since = match self.paced {
            Some((shown, at)) if raw >= at => shown + (raw - at).min(MOST_STEP),
            _ => raw.clamp(0.0, MOST_STEP),
        };
        self.paced = Some((since, raw));
        since
    }

    /// Draws the panel `since` seconds after it opened into `out`, front first: `layers`, front
    /// first, with its own picture last, over its card at `card` on the screen, coming out of the
    /// bar in `frame` or out of its own top edge.
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
        let since = self.pace(since);
        let look = opening_look(since, self.from, reduced_motion);
        self.drawn = look;
        let foot = self.hangs.then_some(frame.bar.foot() as f64);
        let bottom = layers
            .last()
            .map_or(0.0, |(main, at, _)| at.y + main.logical.h as f64);
        self.drawer = Drawer::new(card, foot, bottom);
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
        self.paced = None;
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

    /// How it shows at `now` while it goes, `paced` or by the clock alone. `None` once it has
    /// gone, and the picture is let go.
    fn closing(&mut self, now: f64, reduced_motion: bool, paced: bool) -> Option<Reveal> {
        let closing = self.closing.as_mut()?;
        let started = *closing.started.get_or_insert(now);
        let since = if paced {
            self.pace(now - started)
        } else {
            now - started
        };
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

    /// Its picture at `now` while it goes, front first, with the edge and slot in `ring`.
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
        let reveal = self.closing(now, reduced_motion, true)?;
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

    /// The panel as `reveal` says, its edge and slot in `ring` on the tube's clock at `now`.
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
        // The slot's line, from the button, or the card's middle, out to the card's sides.
        let (spread, strength) = reveal.slot();
        if strength > 0.0 {
            let card = drawer.card;
            let middle = card.loc.x + card.size.w / 2.0;
            let from = if self.hangs {
                self.button.unwrap_or(middle)
            } else {
                middle
            };
            let left = from - (from - card.loc.x) * spread;
            let right = from + (card.loc.x + card.size.w - from) * spread;
            if right - left >= 1.0 {
                if self.hangs {
                    light_bar_slot(BarSlot {
                        left,
                        right,
                        strength,
                    });
                } else {
                    let id = &self.ids[SLOT_ID];
                    out.extend(slot_line(
                        renderer,
                        ring,
                        scale,
                        (strength, left, right),
                        drawer.slot,
                        id,
                    ));
                }
            }
        }
        if reveal.settled() {
            self.card = None;
            self.split = None;
            self.grown.iter_mut().for_each(|grown| *grown = 1.0);
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
            out.extend(self.edge(renderer, ring, scale, edge, lead, drawer.slot));
        }
        // Faint scanlines over the drawer while it moves.
        if moving {
            out.extend(self.scanlines(renderer, scale, drawer, lift));
        }
        // The pieces, each growing in or shrinking away in reading order. Their red and blue are
        // made on the first frame, while the drawer is still in the slot, not as they start.
        let whole = [Rectangle::new(drawer.card.loc - at, drawer.card.size)];
        let rects = pieces(main, &whole);
        if self.split.is_none() && rects.len() > 1 {
            self.split = split_of(&main.buffer, main.device);
        }
        let count = rects.len();
        if self.grown.len() != count {
            self.grown.resize(count, 0.0);
        }
        if self.from_grown.len() != count {
            self.from_grown.resize(count, 0.0);
        }
        if self.order.len() != count {
            reading_order(rects, &mut self.order);
        }
        let mut total = 0.0;
        for (n, rect) in rects.iter().enumerate() {
            let order = self.order[n];
            let (grown, way) = match reveal.way {
                Way::Opening { since } => {
                    let way = way_in(order, count, since);
                    (grown_opening(self.from_grown[n], way), way)
                }
                Way::Closing { since } => {
                    let way = way_out(order, count, since);
                    (grown_closing(self.from_grown[n], way), way)
                }
            };
            self.grown[n] = grown;
            total += grown;
            if grown > 0.0 {
                let piece = Piece {
                    rect: *rect,
                    grown,
                    tube: tube(way),
                    at,
                    lift,
                    salt: n,
                };
                self.piece(renderer, main, piece, look.alpha, now, out);
            }
        }
        // Anything riding along comes in with the pieces as a whole.
        let riders = (total / count as f32).clamp(0.0, 1.0);
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
        self.order.clear();
        self.card = None;
        self.split = None;
    }

    /// The lit edge in `ring` at `scale`: its strength and its left and right ends, centred on
    /// `y`, and cut off above `slot`, the line the drawer comes out of.
    fn edge<R>(
        &self,
        renderer: &mut R,
        ring: u32,
        scale: f64,
        (strength, left, right): (f32, f64, f64),
        y: f64,
        slot: f64,
    ) -> Option<FlowElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if strength <= 0.0 {
            return None;
        }
        let strip = strip(ring, scale);
        let rows = (STRIP_H * scale).round().max(1.0);
        let top = ((y - STRIP_H / 2.0) * scale).round();
        let hidden = ((slot * scale).round() - top).clamp(0.0, rows);
        if hidden >= rows {
            return None;
        }
        let tall = ((rows - hidden) / scale).round().max(1.0) as i32;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::<f64, Physical>::from(((left * scale).round(), top + hidden)),
            &strip,
            Some(strength.min(1.0)),
            Some(Rectangle::new(
                (0.0, hidden).into(),
                (STRIP_W as f64, rows - hidden).into(),
            )),
            Some(((right - left).round().max(2.0) as i32, tall).into()),
            Kind::Unspecified,
        )
        .ok()
        .map(|element| FlowElement::with_id(element, &self.ids[EDGE_ID]))
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
        // How much of a tube it still is: the dimming, the tear and the split all go with it.
        let tube = piece.tube;
        let (dim, tear) = if tube > 0.0 {
            let (beat, torn) = fx::tube(now + piece.salt as f64 * 0.37, PIECE_TEAR_CHANCE);
            let low = fx::tube_hash((now / fx::TUBE_FRAME) as i64 as u64, piece.salt as u64 + 7);
            let dim = if low < DIM_CHANCE { DIM } else { beat };
            // A tear jerks it at least a pixel either way.
            let jerk = if torn == 0.0 {
                0.0
            } else {
                torn.signum() as f64 * (1.0 + (PIECE_TEAR - 1.0) * torn.abs() as f64)
            };
            (1.0 - tube * (1.0 - dim), jerk * tube as f64)
        } else {
            (1.0, 0.0)
        };
        let crop = Crop {
            at: crop.at + Point::from((tear, 0.0)),
            ..crop
        };
        let alpha = alpha * (piece.grown * 1.4).min(1.0) * dim;
        let ids = PIECE_IDS + 3 * piece.salt;
        let split = SPLIT_PX * tube as f64;
        if split > 0.3
            && let Some(colours) = self.split.as_ref()
        {
            for (n, (colour, side)) in colours.iter().zip([-1.0, 1.0]).enumerate() {
                let shifted = Crop {
                    at: crop.at + Point::from((side * split, 0.0)),
                    ..crop
                };
                out.extend(
                    shifted
                        .element(
                            renderer,
                            colour,
                            main.scale,
                            main.scale / SPLIT_SCALE as f64,
                            alpha * SPLIT_ALPHA * tube,
                        )
                        .map(|element| FlowElement::with_id(element, &self.ids[ids + 1 + n])),
                );
            }
        }
        out.extend(
            crop.element(renderer, &main.buffer, main.scale, main.scale, alpha)
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

/// A piece this frame: its box in the picture, how far it has grown, the picture's corner where
/// it rests, how far above that the drawer is, and its place in the list, which keeps its flicker
/// its own.
#[derive(Debug, Clone, Copy)]
struct Piece {
    rect: Rectangle<f64, Logical>,
    grown: f32,
    tube: f32,
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
    /// The crop of `buffer`, which has `texels` pixels to a logical one, on a screen at `scale`.
    fn element<R>(
        &self,
        renderer: &mut R,
        buffer: &MemoryRenderBuffer,
        scale: f64,
        texels: f64,
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
                (self.from.loc.x * texels, self.from.loc.y * texels).into(),
                (self.from.size.w * texels, self.from.size.h * texels).into(),
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
    .element(renderer, buffer, painted.scale, painted.scale, alpha)
}

/// The red and the blue of a picture `size` buffer pixels big, each as light added to what's
/// behind, for its split. They are faint and only show for a moment, so they are made at
/// 1/`SPLIT_SCALE` of the picture's size each way: quick to make on the first frame, and small.
fn split_of(buffer: &MemoryRenderBuffer, size: (i32, i32)) -> Option<[MemoryRenderBuffer; 2]> {
    let (wide, tall) = (size.0.max(0) as usize, size.1.max(0) as usize);
    let small = (wide.div_ceil(SPLIT_SCALE), tall.div_ceil(SPLIT_SCALE));
    let mut reading = buffer.clone();
    let mut colours = None;
    let mut context = reading.render();
    let _ = context.draw(|pixels: &mut [u8]| {
        if pixels.len() < wide * tall * 4 {
            return Ok::<_, ()>(Vec::new());
        }
        let mut red = Vec::with_capacity(small.0 * small.1 * 4);
        let mut blue = Vec::with_capacity(small.0 * small.1 * 4);
        for y in (0..tall).step_by(SPLIT_SCALE) {
            for x in (0..wide).step_by(SPLIT_SCALE) {
                let pixel = &pixels[(y * wide + x) * 4..][..4];
                red.extend_from_slice(&[pixel[0], 0, 0, 0]);
                blue.extend_from_slice(&[0, 0, pixel[2], 0]);
            }
        }
        colours = Some((red, blue));
        Ok::<_, ()>(Vec::new())
    });
    drop(context);
    let (red, blue) = colours?;
    let made = |pixels: &[u8]| {
        MemoryRenderBuffer::from_slice(
            pixels,
            Fourcc::Abgr8888,
            (small.0 as i32, small.1 as i32),
            1,
            Transform::Normal,
            None,
        )
    };
    Some([made(&red), made(&blue)])
}

thread_local! {
    /// The lit strip and the colour and scale it was baked for, and the scanlines. Every panel
    /// shares them; the compositor draws on one thread.
    static STRIP: RefCell<Option<(u32, f64, MemoryRenderBuffer)>> = const { RefCell::new(None) };
    static SCAN: RefCell<Option<MemoryRenderBuffer>> = const { RefCell::new(None) };
    /// The slot's line and the colour it was baked for.
    static SLOT: RefCell<Option<(u32, MemoryRenderBuffer)>> = const { RefCell::new(None) };
}

/// A slot's line in `ring` on a screen at `scale`: lit at `strength`, from `left` to `right`, one
/// logical pixel thick on whole screen pixels just above `foot`, the line a drawer comes out of,
/// with its ends tapering off. Drawn under `id`.
pub fn slot_line<R>(
    renderer: &mut R,
    ring: u32,
    scale: f64,
    (strength, left, right): (f32, f64, f64),
    foot: f64,
    id: &Id,
) -> Option<FlowElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let wide = (right - left).round() as i32;
    if strength <= 0.0 || wide < 1 {
        return None;
    }
    let line = SLOT.with_borrow_mut(|kept| {
        if let Some((colour, line)) = kept.as_ref()
            && *colour == ring
        {
            return line.clone();
        }
        let line = MemoryRenderBuffer::from_slice(
            &bake_slot(ring),
            Fourcc::Abgr8888,
            (SLOT_W as i32, 1),
            1,
            Transform::Normal,
            None,
        );
        *kept = Some((ring, line.clone()));
        line
    });
    let thick = scale.round().max(1.0);
    let at = Point::<f64, Physical>::from(((left * scale).round(), (foot * scale).round() - thick));
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at,
        &line,
        Some((SLOT_ALPHA * strength).min(1.0)),
        // The whole strip, stretched along the slot.
        Some(Rectangle::new(
            (0.0, 0.0).into(),
            (SLOT_W as f64, 1.0).into(),
        )),
        Some((wide, 1).into()),
        Kind::Unspecified,
    )
    .ok()
    .map(|element| FlowElement::with_id(element, id))
}

/// The slot's line's pixels, premultiplied: `ring` taken towards white, solid along its middle and
/// fading out smoothly over `SLOT_TAPER` of its length at either end.
fn bake_slot(ring: u32) -> Vec<u8> {
    let [r, g, b, _] = crate::panel::lighter(ring & 0xffffff00, SLOT_LIGHT)
        .to_be_bytes()
        .map(|c| c as f32 / 255.0);
    (0..SLOT_W)
        .flat_map(|x| {
            let along = (x as f64 + 0.5) / SLOT_W as f64;
            let end = (along.min(1.0 - along) / SLOT_TAPER).min(1.0);
            let a = (end * end * (3.0 - 2.0 * end)) as f32;
            [r * a, g * a, b * a, a].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        })
        .collect()
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

    fn card() -> Rectangle<f64, Logical> {
        Rectangle::new((100.0, 40.0).into(), (300.0, 400.0).into())
    }

    #[test]
    fn the_card_comes_out_over_three_tenths_of_a_second_on_a_glide() {
        assert_eq!(travel_opening(0.0, CARD), 1.0);
        assert_eq!(
            travel_opening(0.0, CARD / 2.0),
            Easing::Glide.at(0.5) as f32
        );
        let steps = 40;
        let mut last = -1.0;
        for n in 0..=steps {
            let travel = travel_opening(0.0, CARD * n as f64 / steps as f64);
            assert!(travel >= last, "{travel} after {last}");
            last = travel;
        }
    }

    #[test]
    fn pieces_come_in_in_reading_order_top_left_first() {
        // Two rows of three, recorded out of order, and a footer.
        let rects = [
            Rectangle::new((120.0, 70.0).into(), (50.0, 40.0).into()),
            Rectangle::new((10.0, 10.0).into(), (50.0, 40.0).into()),
            Rectangle::new((10.0, 140.0).into(), (170.0, 20.0).into()),
            Rectangle::new((65.0, 12.0).into(), (50.0, 40.0).into()),
            Rectangle::new((10.0, 70.0).into(), (50.0, 40.0).into()),
            Rectangle::new((120.0, 8.0).into(), (50.0, 40.0).into()),
        ];
        let mut order = Vec::new();
        reading_order(&rects, &mut order);
        assert_eq!(order, [4, 0, 5, 1, 3, 2]);
    }

    #[test]
    fn pieces_start_after_the_card_and_spread_across_the_artifacts_timings() {
        let count = 12;
        // The first starts as the card sets off, the last with just its own span of the time left.
        assert_eq!(way_in(0, count, START), 0.0);
        assert!(way_in(0, count, START + 0.01) > 0.0);
        let last = START + CONTENT * (1.0 - SPAN);
        assert!(way_in(count - 1, count, last) < 1e-9);
        assert!(way_in(count - 1, count, last + 0.01) > 0.0);
        // Each takes the same time, `SPAN` of `CONTENT`, and they start in order.
        for order in 0..count {
            let start = START + CONTENT * piece_start(order, count);
            assert!(way_in(order, count, start + SPAN * CONTENT + 1e-9) > 1.0 - 1e-9);
            assert!(way_in(order, count, start + SPAN * CONTENT * 0.9) < 1.0);
            assert!(way_in(order, count, LANDED) > 1.0 - 1e-9);
        }
        // Each grows on an ease-out cubic.
        assert_eq!(grown_opening(0.0, 0.5), Easing::OutCubic.at(0.5) as f32);
        assert_eq!(grown_opening(0.4, 0.0), 0.4);
        assert!(way_in(0, 1, LANDED) > 1.0 - 1e-9, "one piece alone");
    }

    #[test]
    fn a_piece_is_a_tube_while_it_powers_up_or_down_and_clean_once_landed() {
        assert_eq!(tube(0.0), 0.0, "not there yet");
        assert_eq!(tube(0.01), 1.0);
        assert_eq!(tube(TUBE_HOLD), 1.0, "held over most of its way");
        assert!(tube((TUBE_HOLD + TUBE_CLEAN) / 2.0) < 1.0);
        assert_eq!(tube(TUBE_CLEAN), 0.0);
        assert_eq!(tube(1.0), 0.0, "pristine at rest");
    }

    #[test]
    fn closing_the_pieces_go_in_a_fifth_of_a_second_then_the_card_follows() {
        let count = 12;
        for order in 0..count {
            assert!(way_out(order, count, 0.0) > 1.0 - 1e-9);
            assert_eq!(way_out(order, count, LEAVE), 0.0);
            assert_eq!(grown_closing(1.0, way_out(order, count, LEAVE)), 0.0);
        }
        // The first in reading order goes first.
        assert!(way_out(0, count, 0.05) < way_out(count - 1, count, 0.05));
        // The card stays put while the pieces go, then goes back in.
        assert_eq!(travel_closing(1.0, BACK_AT), 1.0);
        assert!(travel_closing(1.0, BACK_AT + BACK / 2.0) < 1.0);
        assert_eq!(travel_closing(1.0, CLOSE), 0.0, "close ends in the slot");
        let steps = 40;
        let mut last = 2.0;
        for n in 0..=steps {
            let travel = travel_closing(1.0, CLOSE * n as f64 / steps as f64);
            assert!(travel <= last, "{travel} after {last}");
            last = travel;
        }
    }

    #[test]
    fn a_closed_panel_lets_go_at_once_and_only_its_picture_goes_back_in() {
        let mut fade = Fade::default();
        fade.open();
        fade.drawn_at(LANDED, false);
        assert_eq!(fade.drawn.travel, 1.0);
        assert!(fade.close(picture()), "closing waits for nothing");
        assert!(fade.is_closing());
        assert_eq!(fade.closing(5.0, false, false).unwrap().look.travel, 1.0);
        let halfway = fade
            .closing(5.0 + BACK_AT + BACK / 2.0, false, false)
            .unwrap();
        assert_eq!(halfway.alpha, 1.0, "it goes back in, never fades");
        assert!(halfway.look.travel > 0.0 && halfway.look.travel < 1.0);
        assert_eq!(fade.closing(5.0 + CLOSE + 0.001, false, false), None);
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
        fade.drawn_at(LANDED, false);
        fade.grown = vec![1.0, 0.6];
        fade.close(picture());
        fade.closing(0.0, false, false);
        let drawn = fade.closing(BACK_AT + BACK * 0.5, false, false).unwrap();
        fade.grown = vec![0.4, 0.0];
        fade.open();
        assert!(!fade.is_closing());
        assert_eq!(fade.from, drawn.look);
        assert_eq!(fade.from_grown, [0.4, 0.0], "each piece too");
        assert_eq!(opening_look(0.0, fade.from, false), drawn.look);
        assert_eq!(opening_look(CARD, fade.from, false).travel, 1.0);
    }

    #[test]
    fn a_slow_frame_holds_the_motion_up_rather_than_skipping_it() {
        let mut fade = Fade::default();
        fade.open();
        assert_eq!(fade.pace(0.004), 0.004);
        // The first frame took a tenth of a second to paint.
        let next = fade.pace(0.104);
        assert!((next - (0.004 + MOST_STEP)).abs() < 1e-12);
        // Ordinary frames after it move on by the clock.
        assert!((fade.pace(0.121) - (next + 0.017)).abs() < 1e-12);
        fade.open();
        assert_eq!(
            fade.pace(0.5),
            MOST_STEP,
            "a fresh opening starts from the start"
        );
    }

    #[test]
    fn a_panel_hanging_from_the_bar_comes_out_of_its_foot_and_a_card_out_of_its_own_top() {
        assert_eq!(Drawer::new(card(), Some(32.0), 0.0).slot, 32.0);
        assert_eq!(Drawer::new(card(), None, 0.0).slot, 40.0);
        // A card above the bar's foot comes out of its own top either way.
        assert_eq!(Drawer::new(card(), Some(60.0), 0.0).slot, 40.0);
        // All in the slot, nothing of it shows, not even the shadow under its foot.
        let drawer = Drawer::new(card(), None, 470.0);
        assert_eq!(drawer.lift(0.0), 470.0 - 40.0);
        let mut fade = Fade::default();
        assert!(!fade.hangs, "nothing hangs unless it says so");
        fade.hang_from(None);
        assert!(fade.hangs);
    }

    #[test]
    fn the_slot_lights_only_while_the_drawer_moves_and_glints_shut() {
        let at = |way| Reveal {
            alpha: 1.0,
            look: opening_look(0.0, GONE, false),
            way,
        };
        let opening = |since| at(Way::Opening { since }).slot();
        assert_eq!(
            opening(0.0),
            (0.0, 1.0),
            "lit from the start, from the button"
        );
        assert_eq!(opening(CARD / 2.0), (1.0, 1.0), "out to the card's sides");
        assert_eq!(opening(CARD).1, 1.0);
        assert!(opening(CARD + SLOT_OUT / 2.0).1 < 1.0);
        assert_eq!(opening(CARD + SLOT_OUT).1, 0.0, "out once landed");
        assert_eq!(
            opening(LANDED.max(CARD + SLOT_OUT) + 0.01).1,
            0.0,
            "nothing at rest"
        );
        let closing = |since| at(Way::Closing { since }).slot();
        assert_eq!(closing(0.0).1, 0.0);
        assert_eq!(
            closing(BACK_AT),
            (1.0, 1.0),
            "lit again as the card goes back in"
        );
        let glint = closing(BACK_AT + BACK * (1.0 - GLINT / 2.0));
        assert!(glint.1 > 1.0 + GLINT_LIGHT * 0.9);
        assert_eq!(closing(CLOSE).0, 0.0, "sealed");
        // With reduced motion there is no slot.
        let still = Reveal {
            alpha: 0.5,
            look: opening_look(0.01, GONE, true),
            way: Way::Opening { since: 0.01 },
        };
        assert_eq!(still.slot().1, 0.0);
    }

    #[test]
    fn the_strongest_slot_asked_for_is_the_bars_until_it_is_taken() {
        take_bar_slot();
        let slot = |strength| BarSlot {
            left: 10.0,
            right: 20.0,
            strength,
        };
        light_bar_slot(slot(0.5));
        light_bar_slot(slot(1.2));
        light_bar_slot(slot(0.8));
        assert_eq!(take_bar_slot(), Some(slot(1.2)));
        assert_eq!(take_bar_slot(), None, "taken, and gone at rest");
    }

    #[test]
    fn the_slots_line_tapers_off_at_both_ends() {
        let pixels = bake_slot(0x42d3ffff);
        let alpha = |x: usize| pixels[x * 4 + 3];
        assert_eq!(alpha(SLOT_W / 2), 255);
        assert!(alpha(0) < 8 && alpha(SLOT_W - 1) < 8);
        for x in 1..SLOT_W / 2 {
            assert!(
                alpha(x) >= alpha(x - 1),
                "rising smoothly towards the middle"
            );
        }
    }

    #[test]
    fn with_reduced_motion_it_only_fades_and_no_longer_than_the_short_fade() {
        let opened = opening_look(REDUCED_FADE / 2.0, GONE, true);
        assert_eq!((opened.alpha, opened.travel, opened.lit), (0.5, 1.0, false));
        let mut fade = Fade::default();
        fade.drawn_at(REDUCED_FADE, true);
        fade.close(picture());
        assert_eq!(fade.closing(0.0, true, false).unwrap().alpha, 1.0);
        let half = fade.closing(REDUCED_FADE / 2.0, true, false).unwrap();
        assert_eq!((half.alpha, half.look.lit), (0.5, false));
        assert!(half.whole(), "drawn as its own picture, no pieces");
        assert_eq!(fade.closing(REDUCED_FADE, true, false), None);
    }

    #[test]
    fn reduced_motion_switched_on_mid_close_turns_it_into_the_short_fade() {
        let mut fade = Fade::default();
        fade.drawn_at(CARD, false);
        fade.close(picture());
        assert!(fade.closing(0.0, false, false).unwrap().look.lit);
        let now = fade.closing(REDUCED_FADE / 2.0, true, false).unwrap();
        assert!(!now.look.lit && now.whole());
        assert_eq!(fade.closing(REDUCED_FADE, true, false), None);
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
    fn the_scanlines_are_as_dark_as_the_edges_and_on_every_other_row() {
        let pixels = scan_pixels();
        let row = |n: usize| &pixels[n * STRIP_W * 4..n * STRIP_W * 4 + 4];
        assert_eq!(row(1), [0; 4]);
        assert_eq!(&row(0)[..3], [0, 0, 0]);
        let dark = row(0)[3] as f32 / 255.0;
        assert!((dark - SCAN_DARK).abs() < 0.01, "{dark}");
    }
}

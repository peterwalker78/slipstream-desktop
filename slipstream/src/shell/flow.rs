//! How every panel and card comes and goes: as a drawer out of a lit slot.
//!
//! A panel that hangs from the bar comes out of the bar's own foot line: the line brightens in the
//! focus ring's colour from the button that opened it out to the panel's width, and the bar draws
//! that light itself, from the `BarSlot` the drawer's `Reveal` hands back. A card that stands on
//! its own comes out of a slot of its own: a thin lit line along its top edge, spreading from its
//! middle to its width. Either way the bare card slides down out of the line, clipped at it, with
//! a lit edge riding its leading edge, and the line goes out as the card lands. Its pieces (a
//! tile, a row, a group of buttons) grow into their places in reading order meanwhile, powering
//! up like a picture on a tube, with a flicker and a red and blue split that settle as each lands.
//!
//! Closing is the opening run backwards: the drawer slides back up into its slot over about the
//! time it took to come out, and its pieces shrink and power down the same way, last in first
//! out, from the bottom right, each just before the drawer's leading edge rises past where it
//! rests. The slot narrows behind the drawer and glints shut. Faint scanlines lie over the drawer
//! while it moves either way.
//!
//! Everything is drawn from pictures the panel already has: the card is painted bare once for its
//! size, each piece is a crop of the panel's picture, and the edge, the slot and the scanlines are
//! lit lines drawn by one small shader. Nothing is repainted per frame, and once a panel has
//! settled it is its own picture again.
//!
//! With reduced motion there is no slot, drawer, piece or edge: a panel only fades, no longer
//! than `REDUCED_FADE`, and that follows the setting even mid-way.

use smithay::backend::renderer::{
    element::{
        Element, Id, Kind, RenderElement, UnderlyingStorage,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    },
    gles::{GlesError, GlesFrame, GlesRenderer},
    utils::{CommitCounter, DamageSet, OpaqueRegions},
};
use smithay::utils::{
    Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
    user_data::UserDataMap,
};

use crate::{
    anim::Easing,
    effects::{
        fx,
        lit_line::{Line, LitLine, Scanlines},
        shader::Shared,
    },
    motion::REDUCED_FADE,
    paint::{Painted, Split},
    panel::Frame,
};

/// The bare card comes out over 0.3 s on a glide.
const CARD: f64 = 0.3;
/// The pieces start coming in this long after the panel opens, one after another in reading
/// order, their starts spread across `CONTENT` so that each takes `SPAN` of it.
const START: f64 = 0.05;
const CONTENT: f64 = 0.3;
const SPAN: f64 = 0.45;
/// How long each piece takes to come in or go.
const PIECE: f64 = SPAN * CONTENT;
/// Everything has landed this long after opening.
const LANDED: f64 = START + CONTENT;
/// Closing, the card holds for `HOLD` while the first pieces go, then slides back into the slot
/// over `CARD`, the opening's glide run backwards. Only the paint lingers: input goes to what's
/// underneath the moment it closes.
const HOLD: f64 = START;
const CLOSE: f64 = HOLD + CARD;
/// A piece is gone this long before the drawer's leading edge rises past where it rests, and
/// pieces side by side go at least `STAGGER` apart, the right one first.
const LEAD: f64 = 0.03;
const STAGGER: f64 = 0.012;
/// The slot's line spreads from where it opens to the card's width over its first 100 ms, and goes
/// out over `SLOT_OUT` once the card has landed. Closing, it lights over `HOLD`, narrows over the
/// last `SEAL` of the card's way back in, and glints over the last `GLINT`.
const SLOT_OPEN: f64 = 0.1;
const SLOT_OUT: f64 = 0.15;
const SEAL: f64 = 0.45;
const GLINT: f64 = 0.3;
/// How much brighter than while the drawer moves the line is at its glint's peak.
const GLINT_LIGHT: f32 = 0.5;
/// The most the first frame of a way in or out moves it on, in seconds: that frame paints the
/// panel and can be slow, and the motion should start from its start rather than skip ahead.
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
/// The scanlines over the moving drawer: as dark as the edge's.
const SCAN_DARK: f32 = 1.0 - fx::SCANLINE;
/// The card's corner radius in logical pixels: the scanlines keep this far from its top and foot,
/// so they never show past its rounded corners.
const CORNER: f64 = 18.0 * crate::panel::DESIGN_PX as f64;
/// More pieces than this and the picture comes in whole.
pub const MOST_PIECES: usize = 32;

/// The lit edge: a line `LINE` logical pixels thick, `INSET` in from the card's sides, with a
/// glow reaching `GLOW` above and below it, its colour taken `CORE_LIGHT` of the way to white.
const LINE: f64 = 2.0;
const INSET: f64 = 6.0;
const GLOW: f64 = 14.0;
const CORE_LIGHT: f32 = 0.35;
/// About one frame of the tube in this many tears the edge, by up to `TEAR_REACH` logical pixels.
const TEAR_CHANCE: f32 = 0.06;
const TEAR_REACH: f64 = 5.0;
/// A card's own slot: one logical pixel, its colour taken `SLOT_LIGHT` towards white, at
/// `SLOT_ALPHA` while the drawer moves, so its glint can brighten it further.
const SLOT_LIGHT: f32 = 0.15;
const SLOT_ALPHA: f32 = 0.75;

/// One layer of what a panel last drew, kept to take back into the slot: its paint, its corner
/// where the panel settles, and its share of the panel's opacity.
pub struct Layer {
    pub painted: Painted,
    pub at: Point<f64, Logical>,
    pub share: f32,
}

impl Layer {
    /// `painted` with its corner at `at`, wholly opaque.
    pub fn new(painted: Painted, at: Point<f64, Logical>) -> Self {
        Self {
            painted,
            at,
            share: 1.0,
        }
    }

    fn shown(&self) -> Shown<'_> {
        Shown {
            painted: &self.painted,
            at: self.at,
            share: self.share,
        }
    }
}

/// One layer of what a panel shows this frame, borrowed. A panel's own picture is its last
/// layer; any before it (a glow, a stage) ride along in front of it.
#[derive(Clone, Copy)]
pub struct Shown<'a> {
    pub painted: &'a Painted,
    pub at: Point<f64, Logical>,
    pub share: f32,
}

impl<'a> Shown<'a> {
    pub fn new(painted: &'a Painted, at: Point<f64, Logical>) -> Self {
        Self {
            painted,
            at,
            share: 1.0,
        }
    }
}

smithay::backend::renderer::element::render_elements! {
    /// What a panel draws: its pictures, crops of them coming and going, and its lit lines.
    pub FlowElement<=GlesRenderer>;
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Crop=Cropped,
    Line=Shared,
}

impl std::fmt::Debug for FlowElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory(element) => element.fmt(f),
            Self::Crop(element) => element.inner.fmt(f),
            Self::Line(element) => element.fmt(f),
            Self::_GenericCatcher(_) => f.write_str("FlowElement"),
        }
    }
}

/// A crop of a picture drawn under an id of its own: many crops share one buffer, and damage is
/// tracked by id.
pub struct Cropped {
    inner: MemoryRenderBufferRenderElement<GlesRenderer>,
    id: Id,
}

impl Element for Cropped {
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

impl RenderElement<GlesRenderer> for Cropped {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

fn cropped(inner: MemoryRenderBufferRenderElement<GlesRenderer>, id: &Id) -> FlowElement {
    FlowElement::Crop(Cropped {
        inner,
        id: id.clone(),
    })
}

/// Where a drawer comes out of: the bar's foot line under the button that opened it (or the
/// card's middle without one), or a slot along its own top edge.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Hang {
    #[default]
    Own,
    Bar {
        button: Option<f64>,
    },
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

/// How far out the drawer is `since` seconds into a closing that started at `from`: held while
/// the first pieces go, then back in on the opening's glide run backwards.
fn travel_closing(from: f32, since: f64) -> f32 {
    from * glide(1.0 - (since - HOLD) / CARD) as f32
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

/// How far a piece due to be gone at `due` seconds into a closing still is from gone, 1 to 0,
/// `since` seconds in: it takes `PIECE`, or what's left before it is due if that's less.
fn way_out(due: f64, since: f64) -> f64 {
    if due <= 0.0 {
        return 0.0;
    }
    let takes = PIECE.min(due);
    ((due - since) / takes).clamp(0.0, 1.0)
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

/// How far a piece has grown, from `from` when the closing started, with `way` of it left: the
/// opening's ease run backwards.
fn grown_closing(from: f32, way: f64) -> f32 {
    from * Easing::OutCubic.at(way) as f32
}

/// The order pieces come in, reading the way text does: rows from the top, each from the left.
/// A piece whose top is within `ROW` of the first in a row is in that row. `line` gets the pieces
/// in that order, and `order` each piece's place in it.
fn reading_order(rects: &[Rectangle<f64, Logical>], line: &mut Vec<usize>, order: &mut Vec<usize>) {
    const ROW: f64 = 16.0;
    line.clear();
    line.extend(0..rects.len());
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
struct Run {
    card: Rectangle<f64, Logical>,
    slot: f64,
    bottom: f64,
}

impl Run {
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

    /// How long into a closing from `from` the leading edge takes to rise to `y`.
    fn reaches(&self, from: f32, y: f64) -> f64 {
        let rose = |t: f64| self.lead(travel_closing(from, t)) <= y;
        if rose(0.0) {
            return 0.0;
        }
        let (mut early, mut late) = (0.0, CLOSE);
        for _ in 0..24 {
            let middle = (early + late) / 2.0;
            if rose(middle) {
                late = middle;
            } else {
                early = middle;
            }
        }
        late
    }
}

/// How the panel is moving: opening or closing, and how long since it started.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Way {
    Opening { since: f64 },
    Closing { since: f64 },
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

impl BarSlot {
    /// The slot as a lit line in `ring` along the row of screen pixels just above `foot`, on a
    /// screen at `scale`: one screen pixel thick, its ends tapering off. The bar draws it this way
    /// in the styles whose foot line is part of its picture, and a card's own slot is the same.
    pub fn line(&self, foot: f64, ring: u32, scale: f64) -> Line {
        let ring = ring & 0xffffff00;
        let thick = scale.round().max(1.0) / scale;
        Line {
            left: self.left,
            right: self.right,
            y: (foot * scale).round() / scale - thick / 2.0,
            thick,
            glow: 0.0,
            above: 0.0,
            tapered: true,
            fringed: false,
            core: crate::panel::lighter(ring, SLOT_LIGHT),
            halo: ring,
            strength: SLOT_ALPHA * self.strength,
        }
    }

    /// The stronger of two, for a frame with more than one drawer moving.
    pub fn strongest(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(a), Some(b)) => Some(if b.strength > a.strength { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

/// A panel as drawn this frame: its opacity, whether it has settled, which is when anything laid
/// over it, such as the explorer's caret, can be drawn too, and the slot it asks the bar for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reveal {
    pub alpha: f32,
    look: Look,
    way: Way,
    pub bar_slot: Option<BarSlot>,
}

impl Reveal {
    fn new(look: Look, way: Way) -> Self {
        Self {
            alpha: look.alpha,
            look,
            way,
            bar_slot: None,
        }
    }

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
    /// and its strength, 0 for none, 1 while the drawer moves, more in its glint.
    fn slot(&self) -> SlotLight {
        if !self.look.lit {
            return SlotLight::OUT;
        }
        match self.way {
            Way::Opening { since } => SlotLight {
                spread: in_out(since / SLOT_OPEN),
                strength: 1.0 - in_out((since - CARD) / SLOT_OUT) as f32,
            },
            Way::Closing { since } => {
                let back = ((since - HOLD) / CARD).clamp(0.0, 1.0);
                let glint = ((back - (1.0 - GLINT)) / GLINT).clamp(0.0, 1.0);
                SlotLight {
                    spread: in_out((1.0 - back) / SEAL),
                    strength: in_out(since / HOLD) as f32
                        + GLINT_LIGHT * (std::f64::consts::PI * glint).sin() as f32,
                }
            }
        }
    }
}

/// How the slot is lit: how far its ends have spread towards the card's sides, and how strongly.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SlotLight {
    spread: f64,
    strength: f32,
}

impl SlotLight {
    const OUT: Self = Self {
        spread: 0.0,
        strength: 0.0,
    };
}

/// A panel going back in: the picture it closed on, front first, and when the close started on
/// the clock it is drawn by, which the first frame after it fills in.
struct Closing {
    picture: Vec<Layer>,
    started: Option<f64>,
}

/// The bare card a picture is drawn on, painted once for its size, scale and glass.
struct Bare {
    key: BareKey,
    buffer: MemoryRenderBuffer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BareKey {
    logical: Size<i32, Logical>,
    scale: f64,
    glass: crate::paint::Glass,
}

/// The lit lines a drawer draws.
#[derive(Default)]
struct Lines {
    edge: LitLine,
    slot: LitLine,
    scan: LitLine,
}

/// A panel's or card's way in and out, as a drawer. It opens from nothing, or from wherever a
/// close it interrupts has got to, and closes by taking the picture it last drew back into its
/// slot. Neither holds anything up: the panel stops taking keys and clicks the moment it closes,
/// and this only paints.
#[derive(Default)]
pub struct Drawer {
    hang: Hang,
    /// When it opened, on the clock it is drawn by; a card opens on the first frame it is drawn.
    opened: Option<f64>,
    /// What the opening starts from, and what was last drawn.
    from: Look,
    drawn: Look,
    /// Each piece's growth as the way in or out started, and as last drawn, 0 to 1.
    from_grown: Vec<f32>,
    grown: Vec<f32>,
    /// The pieces in reading order, each piece's place in it, and when each is due to be gone
    /// when closing; worked out once per picture and once per close.
    line: Vec<usize>,
    order: Vec<usize>,
    due: Vec<f64>,
    run: Run,
    closing: Option<Closing>,
    /// How far behind the clock the motion runs, after a slow first frame.
    lag: Option<f64>,
    /// The ids its crops are drawn under, and the picture they were made for.
    ids: Vec<Id>,
    made_for: u64,
    bare: Option<Bare>,
    lines: Lines,
}

impl Drawer {
    /// Opens at `now`, out of `hang`: from where it is drawn if it was still going back in,
    /// otherwise from nothing.
    pub fn open(&mut self, now: f64, hang: Hang) {
        self.opened = Some(now);
        self.hang = hang;
        self.lag = None;
        if self.closing.take().is_some() {
            self.from = self.drawn;
            self.from_grown.clone_from(&self.grown);
        } else {
            self.from = GONE;
            self.from_grown.iter_mut().for_each(|grown| *grown = 0.0);
            self.grown.iter_mut().for_each(|grown| *grown = 0.0);
        }
    }

    /// Closes into its slot, keeping `picture`, what it last drew, front first, to take back in;
    /// empty when nothing is drawn. Closing again while it goes changes nothing. Returns whether
    /// it goes.
    pub fn close_into_bar(&mut self, picture: Vec<Layer>) -> bool {
        if self.closing.is_some() {
            return true;
        }
        self.lag = None;
        self.opened = None;
        self.from = self.drawn;
        self.from_grown.clone_from(&self.grown);
        self.due.clear();
        if picture.is_empty() || self.drawn.alpha <= 0.0 {
            self.drawn = GONE;
            return false;
        }
        self.closing = Some(Closing {
            picture,
            started: None,
        });
        true
    }

    /// Whether it is still going back in after closing.
    pub fn closing(&self) -> bool {
        self.closing.is_some()
    }

    /// Takes it as drawn `since` seconds after it opened, without drawing it.
    #[cfg(test)]
    pub fn drawn_at(&mut self, since: f64, reduced_motion: bool) {
        self.drawn = opening_look(since, self.from, reduced_motion);
    }

    /// `raw` seconds into a way in or out by the clock, less what a slow first frame held up.
    fn pace(&mut self, raw: f64) -> f64 {
        let lag = *self.lag.get_or_insert((raw - MOST_STEP).max(0.0));
        (raw - lag).max(0.0)
    }

    /// Draws the panel into `out`, front first: `layers`, with its own picture last, over its
    /// card at `card` on the screen, coming out of the bar in `frame` or out of its own top edge.
    pub fn show(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: Frame,
        card: Rectangle<f64, Logical>,
        layers: &[Shown<'_>],
        out: &mut impl Extend<FlowElement>,
    ) -> Reveal {
        let mut reveal = self.reveal(frame, card, layers);
        self.draw(renderer, frame, &mut reveal, layers, out);
        reveal
    }

    /// Where the opening has got to at `frame`'s time, for a card at `card` showing `layers`,
    /// without drawing it yet: for a panel that draws something in front of it only once it has
    /// settled. `draw` then draws it.
    pub fn reveal(
        &mut self,
        frame: Frame,
        card: Rectangle<f64, Logical>,
        layers: &[Shown<'_>],
    ) -> Reveal {
        let opened = *self.opened.get_or_insert(frame.time);
        let since = self.pace(frame.time - opened);
        let look = opening_look(since, self.from, frame.reduced_motion);
        self.drawn = look;
        let foot = matches!(self.hang, Hang::Bar { .. }).then_some(frame.bar.foot() as f64);
        let bottom = layers
            .last()
            .map_or(0.0, |main| main.at.y + main.painted.logical.h as f64);
        self.run = Run::new(card, foot, bottom);
        Reveal::new(look, Way::Opening { since })
    }

    /// Its picture at `frame`'s time while it goes back in, front first. `None` once it has
    /// gone, and the picture is let go.
    pub fn draw_closing(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: Frame,
        out: &mut impl Extend<FlowElement>,
    ) -> Option<Reveal> {
        let mut reveal = self.closing_reveal(frame.time, frame.reduced_motion)?;
        let picture = std::mem::take(&mut self.closing.as_mut()?.picture);
        // The panel's own picture, last, and what rode along in front of it. A closing drawer
        // always has a picture: an empty one doesn't close.
        let count = picture.len().min(RIDERS + 1);
        let start = picture.len() - count;
        let layers: [Shown<'_>; RIDERS + 1] =
            std::array::from_fn(|n| picture[start + n.min(count.max(1) - 1)].shown());
        self.draw(renderer, frame, &mut reveal, &layers[..count], out);
        if let Some(closing) = self.closing.as_mut() {
            closing.picture = picture;
        }
        Some(reveal)
    }

    /// How it shows at `now` while it goes back in. `None` once it has gone.
    fn closing_reveal(&mut self, now: f64, reduced_motion: bool) -> Option<Reveal> {
        let closing = self.closing.as_mut()?;
        let started = *closing.started.get_or_insert(now);
        let since = self.pace(now - started);
        match closing_look(self.from, since, reduced_motion) {
            Some(look) => {
                self.drawn = look;
                Some(Reveal::new(look, Way::Closing { since }))
            }
            None => {
                self.closing = None;
                self.drawn = GONE;
                self.grown.iter_mut().for_each(|grown| *grown = 0.0);
                None
            }
        }
    }

    /// Works out how far each of `rects`, the pieces of a picture with its corner at `at`, has
    /// grown as `reveal` says, and returns how far they have grown on the whole.
    fn grow(
        &mut self,
        reveal: &Reveal,
        rects: &[Rectangle<f64, Logical>],
        at: Point<f64, Logical>,
    ) -> f32 {
        let count = rects.len();
        if self.order.len() != count {
            reading_order(rects, &mut self.line, &mut self.order);
        }
        self.grown.resize(count, 0.0);
        self.from_grown.resize(count, 0.0);
        if matches!(reveal.way, Way::Closing { .. }) && self.due.len() != count {
            self.plan_close(rects, at);
        }
        let mut total = 0.0;
        for n in 0..count {
            let grown = match reveal.way {
                Way::Opening { since } => {
                    grown_opening(self.from_grown[n], way_in(self.order[n], count, since))
                }
                Way::Closing { since } => {
                    grown_closing(self.from_grown[n], way_out(self.due[n], since))
                }
            };
            self.grown[n] = grown;
            total += grown;
        }
        (total / count.max(1) as f32).clamp(0.0, 1.0)
    }

    /// How far piece `n` is through its way in or out, for its tube.
    fn way(&self, reveal: &Reveal, n: usize) -> f64 {
        match reveal.way {
            Way::Opening { since } => way_in(self.order[n], self.order.len(), since),
            Way::Closing { since } => way_out(self.due[n], since),
        }
    }

    /// When each piece is due to be gone as the drawer goes back in: just before its leading edge
    /// rises past where the piece rests, and the later in reading order the earlier, so they go
    /// last in, first out, from the bottom right.
    fn plan_close(&mut self, rects: &[Rectangle<f64, Logical>], at: Point<f64, Logical>) {
        let from = self.from.travel;
        self.due.clear();
        self.due.extend(
            rects
                .iter()
                .map(|rect| self.run.reaches(from, at.y + rect.loc.y + rect.size.h) - LEAD),
        );
        for pair in self.line.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            self.due[after] = self.due[after].min(self.due[before] - STAGGER);
        }
    }

    /// The panel as `reveal` says, its edge and slot in `frame`'s ring colour on the tube's clock
    /// at `frame`'s time.
    pub fn draw(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: Frame,
        reveal: &mut Reveal,
        layers: &[Shown<'_>],
        out: &mut impl Extend<FlowElement>,
    ) {
        let Some(&main) = layers.last() else {
            return;
        };
        let look = reveal.look;
        let scale = main.painted.scale;
        // Reduced motion, or settled: each picture whole.
        if !look.lit || reveal.settled() {
            if look.lit {
                self.grown.iter_mut().for_each(|grown| *grown = 1.0);
            }
            for layer in layers {
                out.extend(
                    layer
                        .painted
                        .element(renderer, layer.at, look.alpha * layer.share)
                        .map(FlowElement::from),
                );
            }
            reveal.bar_slot = self.slot(renderer, frame, reveal, scale, out);
            return;
        }
        self.made_for_picture(main.painted);
        reveal.bar_slot = self.slot(renderer, frame, reveal, scale, out);
        let ring = frame.ring & 0xffffff00;
        let run = self.run;
        let lift = run.lift(look.travel);
        let lead = run.lead(look.travel);
        let moving = look.travel < 1.0;
        let (flicker, torn) = fx::tube(frame.time, TEAR_CHANCE);
        // The lit edge on the drawer's leading edge, while it moves and is out of the slot.
        if moving && lead > run.slot + 1.0 {
            let tear = torn as f64 * TEAR_REACH;
            let edge = Line {
                left: run.card.loc.x + INSET + tear,
                right: run.card.loc.x + run.card.size.w - INSET + tear,
                y: lead,
                thick: LINE,
                glow: GLOW,
                above: run.slot,
                tapered: false,
                fringed: true,
                core: crate::panel::lighter(ring, CORE_LIGHT),
                halo: ring,
                strength: look.alpha * flicker,
            };
            out.extend(
                self.lines
                    .edge
                    .line(renderer, &edge, scale)
                    .map(FlowElement::Line),
            );
        }
        // Faint scanlines over the drawer while it moves.
        let card = run.card;
        let top = run.slot.max(card.loc.y - lift + CORNER);
        let bottom = card.loc.y + card.size.h - lift - CORNER;
        if moving && bottom > top {
            let scan = Scanlines {
                rect: Rectangle::new((card.loc.x, top).into(), (card.size.w, bottom - top).into()),
                dark: SCAN_DARK * look.alpha,
            };
            out.extend(
                self.lines
                    .scan
                    .scanlines(renderer, &scan, scale)
                    .map(FlowElement::Line),
            );
        }
        // The pieces, each growing in or shrinking away.
        let whole = [Rectangle::new(run.card.loc - main.at, run.card.size)];
        let rects = pieces(main.painted, &whole);
        let riders = self.grow(reveal, rects, main.at);
        for (n, rect) in rects.iter().enumerate() {
            let grown = self.grown[n];
            if grown > 0.0 {
                let piece = Piece {
                    rect: *rect,
                    grown,
                    tube: tube(self.way(reveal, n)),
                    at: main.at,
                    lift,
                    salt: n,
                };
                self.piece(renderer, main.painted, piece, look.alpha, frame.time, out);
            }
        }
        // Anything riding along comes in with the pieces as a whole.
        for (n, layer) in layers[..layers.len() - 1].iter().enumerate().take(RIDERS) {
            let alpha = look.alpha * layer.share * riders;
            out.extend(
                slid(
                    renderer,
                    &layer.painted.buffer,
                    layer.painted,
                    layer.at,
                    lift,
                    run.slot,
                    alpha,
                )
                .map(|element| cropped(element, &self.ids[RIDER_IDS + n])),
            );
        }
        // The bare card at the back.
        if let Some(bare) = self.bare_card(main.painted) {
            out.extend(
                slid(
                    renderer,
                    bare,
                    main.painted,
                    main.at,
                    lift,
                    run.slot,
                    look.alpha,
                )
                .map(|element| cropped(element, &self.ids[CARD_ID])),
            );
        }
    }

    /// The slot this frame: handed back for the bar to light when the drawer hangs from it, and
    /// otherwise drawn into `out` along the card's own top edge.
    fn slot(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: Frame,
        reveal: &Reveal,
        scale: f64,
        out: &mut impl Extend<FlowElement>,
    ) -> Option<BarSlot> {
        let light = reveal.slot();
        if light.strength <= 0.0 {
            return None;
        }
        let card = self.run.card;
        let middle = card.loc.x + card.size.w / 2.0;
        let from = match self.hang {
            Hang::Bar { button } => button.unwrap_or(middle),
            Hang::Own => middle,
        };
        let left = from - (from - card.loc.x) * light.spread;
        let right = from + (card.loc.x + card.size.w - from) * light.spread;
        if right - left < 1.0 {
            return None;
        }
        let slot = BarSlot {
            left,
            right,
            strength: light.strength,
        };
        if let Hang::Bar { .. } = self.hang {
            return Some(slot);
        }
        let line = slot.line(self.run.slot, frame.ring, scale);
        out.extend(
            self.lines
                .slot
                .line(renderer, &line, scale)
                .map(FlowElement::Line),
        );
        None
    }

    /// Fresh ids for a picture other than the last one.
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
    }

    /// The card `main` is painted on, bare, painted again only when its size, scale or glass
    /// change, so it is kept from one opening to the next.
    fn bare_card(&mut self, main: &Painted) -> Option<&MemoryRenderBuffer> {
        let glass = main.pieces.glass?;
        let key = BareKey {
            logical: main.logical,
            scale: main.scale,
            glass,
        };
        if self.bare.as_ref().is_none_or(|bare| bare.key != key) {
            let buffer = Painted::new(main.logical, main.scale, |p| {
                p.f = glass.f;
                crate::panel::glass(p, glass.x, glass.y, glass.w, glass.h);
            })?
            .buffer;
            self.bare = Some(Bare { key, buffer });
        }
        self.bare.as_ref().map(|bare| &bare.buffer)
    }

    /// One piece of `main`, with its colours split while it powers up or down.
    fn piece(
        &mut self,
        renderer: &mut GlesRenderer,
        main: &Painted,
        piece: Piece,
        alpha: f32,
        now: f64,
        out: &mut impl Extend<FlowElement>,
    ) {
        let Some(crop) = piece.crop(main, self.run.slot) else {
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
            && let Some(Split {
                red,
                blue,
                scale: texels,
            }) = main.pieces.split.as_ref()
        {
            for (n, (colour, side)) in [(red, -1.0), (blue, 1.0)].into_iter().enumerate() {
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
                            *texels,
                            alpha * SPLIT_ALPHA * tube,
                        )
                        .map(|element| cropped(element, &self.ids[ids + 1 + n])),
                );
            }
        }
        out.extend(
            crop.element(renderer, &main.buffer, main.scale, main.scale, alpha)
                .map(|element| cropped(element, &self.ids[ids])),
        );
    }
}

/// The ids a drawer's crops are drawn under, which last as long as the picture they were made
/// for: the bare card, two layers riding along, then three for each piece, its crop and its red
/// and blue.
const CARD_ID: usize = 0;
const RIDER_IDS: usize = 1;
const RIDERS: usize = 2;
const PIECE_IDS: usize = RIDER_IDS + RIDERS;

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

/// A piece this frame: its box in the picture, how far it has grown, how much of a tube it still
/// is, the picture's corner where it rests, how far above that the drawer is, and its place in
/// the list, which keeps its flicker its own.
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
    fn element(
        &self,
        renderer: &mut GlesRenderer,
        buffer: &MemoryRenderBuffer,
        scale: f64,
        texels: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
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
fn slid(
    renderer: &mut GlesRenderer,
    buffer: &MemoryRenderBuffer,
    painted: &Painted,
    at: Point<f64, Logical>,
    lift: f64,
    slot: f64,
    alpha: f32,
) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
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

    fn frame(time: f64, reduced_motion: bool) -> Frame {
        Frame {
            size: (1600, 900).into(),
            scale: 1.0,
            time,
            accent: 0xffb547ff,
            attention: 0xff7a45ff,
            ring: 0x42d3ffff,
            bar: crate::bar::Place::default(),
            reduced_motion,
        }
    }

    /// A panel's picture: a card with two rows of two tiles and a footer.
    fn tiled() -> Painted {
        Painted::new((300, 400).into(), 1.0, |p| {
            p.piece(20.0, 20.0, 120.0, 80.0);
            p.piece(160.0, 20.0, 120.0, 80.0);
            p.piece(20.0, 120.0, 120.0, 80.0);
            p.piece(160.0, 120.0, 120.0, 80.0);
            p.piece(20.0, 320.0, 260.0, 40.0);
        })
        .unwrap()
    }

    fn card() -> Rectangle<f64, Logical> {
        Rectangle::new((110.0, 50.0).into(), (280.0, 360.0).into())
    }

    const AT: Point<f64, Logical> = Point::new(100.0, 40.0);

    /// Runs `drawer` as the renderer would, without drawing: where it has got to at `time`, and
    /// how far each piece has grown.
    fn step(
        drawer: &mut Drawer,
        picture: &Painted,
        time: f64,
        reduced_motion: bool,
    ) -> Option<Reveal> {
        let frame = frame(time, reduced_motion);
        let reveal = if drawer.closing() {
            drawer.closing_reveal(time, reduced_motion)?
        } else {
            drawer.reveal(frame, card(), &[Shown::new(picture, AT)])
        };
        if reveal.look.lit && !reveal.settled() {
            drawer.made_for_picture(picture);
            drawer.grow(&reveal, &picture.pieces.rects, AT);
        } else if reveal.look.lit {
            drawer.grown.iter_mut().for_each(|grown| *grown = 1.0);
        }
        Some(reveal)
    }

    fn layers(picture: Painted) -> Vec<Layer> {
        vec![Layer::new(picture, AT)]
    }

    #[test]
    fn the_card_comes_out_over_three_tenths_of_a_second_on_a_glide() {
        assert_eq!(travel_opening(0.0, CARD), 1.0);
        assert_eq!(
            travel_opening(0.0, CARD / 2.0),
            Easing::Glide.at(0.5) as f32
        );
        let mut last = -1.0;
        for n in 0..=40 {
            let travel = travel_opening(0.0, CARD * n as f64 / 40.0);
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
        let (mut line, mut order) = (Vec::new(), Vec::new());
        reading_order(&rects, &mut line, &mut order);
        assert_eq!(order, [4, 0, 5, 1, 3, 2]);
        assert_eq!(line, [1, 3, 5, 4, 0, 2]);
    }

    #[test]
    fn pieces_start_as_the_card_sets_off_and_have_all_landed_soon_after_it() {
        let count = 12;
        assert_eq!(way_in(0, count, START), 0.0);
        assert!(way_in(0, count, START + 0.01) > 0.0);
        let last = START + CONTENT * (1.0 - SPAN);
        assert!(way_in(count - 1, count, last) < 1e-9);
        for order in 0..count {
            let start = START + CONTENT * piece_start(order, count);
            assert!(way_in(order, count, start + PIECE + 1e-9) > 1.0 - 1e-9);
            assert!(way_in(order, count, start + PIECE * 0.9) < 1.0);
            assert!(way_in(order, count, LANDED) > 1.0 - 1e-9);
        }
        assert_eq!(grown_opening(0.4, 0.0), 0.4);
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
    fn closing_runs_the_opening_backwards_over_about_as_long() {
        // The card holds while the first pieces go, then retraces its glide back into the slot.
        assert_eq!(travel_closing(1.0, HOLD), 1.0);
        for n in 0..=20 {
            let t = CARD * n as f64 / 20.0;
            let back = travel_closing(1.0, HOLD + t);
            assert!(
                (back - travel_opening(0.0, CARD - t)).abs() < 1e-6,
                "at {t}"
            );
        }
        assert_eq!(travel_closing(1.0, CLOSE), 0.0, "close ends in the slot");
        assert!((0.3..=0.36).contains(&CLOSE));
        // A piece shrinks on the ease it grew on, run backwards.
        for n in 0..=10 {
            let way = n as f64 / 10.0;
            assert_eq!(grown_closing(1.0, way), grown_opening(0.0, way));
        }
    }

    #[test]
    fn closing_the_pieces_go_last_in_first_out_each_before_the_edge_reaches_it() {
        let picture = tiled();
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Own);
        for n in 0..=30 {
            step(&mut drawer, &picture, n as f64 * 0.017, false);
        }
        assert!(drawer.close_into_bar(layers(tiled())));
        step(&mut drawer, &picture, 1.0, false).unwrap();
        let rects = &picture.pieces.rects;
        // The footer goes first, then the second row right to left, then the first.
        let due = &drawer.due;
        let gone_in: Vec<usize> = {
            let mut by_due: Vec<usize> = (0..rects.len()).collect();
            by_due.sort_by(|&a, &b| due[a].total_cmp(&due[b]));
            by_due
        };
        assert_eq!(gone_in, [4, 3, 2, 1, 0]);
        for (n, rect) in rects.iter().enumerate() {
            let rests = AT.y + rect.loc.y + rect.size.h;
            // Just before it is due, the edge is still below it; when it is gone, it still is.
            let edge = drawer.run.lead(travel_closing(1.0, due[n]));
            assert!(
                edge > rests - 1.0,
                "piece {n}: edge {edge} has passed {rests}"
            );
            assert!(due[n] < CLOSE);
        }
        assert!(
            due[0] > CLOSE * 0.6,
            "the top-left piece stays almost to the end"
        );
    }

    #[test]
    fn a_closed_panel_lets_go_at_once_and_only_its_picture_goes_back_in() {
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Own);
        drawer.drawn_at(LANDED, false);
        assert_eq!(drawer.drawn.travel, 1.0);
        assert!(
            drawer.close_into_bar(layers(tiled())),
            "closing waits for nothing"
        );
        assert!(drawer.closing());
        assert_eq!(drawer.closing_reveal(5.0, false).unwrap().look.travel, 1.0);
        let halfway = drawer
            .closing_reveal(5.0 + HOLD + CARD / 2.0, false)
            .unwrap();
        assert_eq!(halfway.alpha, 1.0, "it goes back in, never fades");
        assert!(halfway.look.travel > 0.0 && halfway.look.travel < 1.0);
        assert_eq!(drawer.closing_reveal(5.0 + CLOSE + 0.001, false), None);
        assert!(!drawer.closing(), "and then the picture is let go");
    }

    #[test]
    fn a_panel_closed_before_it_was_drawn_has_nothing_to_take_back() {
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Own);
        assert!(!drawer.close_into_bar(layers(tiled())));
        assert!(!drawer.close_into_bar(Vec::new()));
    }

    #[test]
    fn a_panel_opens_closes_and_reopens_mid_close_from_what_was_drawn() {
        let picture = tiled();
        let mut drawer = Drawer::default();
        drawer.open(
            10.0,
            Hang::Bar {
                button: Some(300.0),
            },
        );
        let first = step(&mut drawer, &picture, 10.0, false).unwrap();
        assert_eq!(first.look.travel, 0.0, "it starts in the slot");
        let mut t = 10.0;
        while !step(&mut drawer, &picture, t, false).unwrap().settled() {
            t += 0.017;
            assert!(t < 10.5, "it settles");
        }
        assert!(drawer.grown.iter().all(|&grown| grown == 1.0));
        // Closed, then reopened partway back in.
        assert!(drawer.close_into_bar(layers(tiled())));
        let mut drawn = None;
        for n in 0..12 {
            drawn = step(&mut drawer, &picture, 20.0 + n as f64 * 0.017, false);
        }
        let drawn = drawn.unwrap();
        let shrunk = drawer.grown.clone();
        assert!(drawn.look.travel < 1.0 && drawn.look.travel > 0.0);
        assert!(shrunk.iter().any(|&grown| grown < 1.0), "pieces were going");
        drawer.open(
            30.0,
            Hang::Bar {
                button: Some(300.0),
            },
        );
        assert!(!drawer.closing());
        let again = step(&mut drawer, &picture, 30.0, false).unwrap();
        assert_eq!(
            again.look.travel, drawn.look.travel,
            "the card carries on from where it was"
        );
        assert_eq!(drawer.from_grown, shrunk, "and so does each piece");
        let mut t = 30.0;
        while !step(&mut drawer, &picture, t, false).unwrap().settled() {
            t += 0.017;
            assert!(t < 30.5);
        }
    }

    #[test]
    fn the_first_slow_frame_holds_the_motion_up_and_the_rest_keep_to_the_clock() {
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Own);
        // The first frame took a tenth of a second to paint.
        assert_eq!(drawer.pace(0.1), MOST_STEP);
        // After it, a frame moves it on by however long that frame took, even a slow one.
        assert!((drawer.pace(0.117) - (MOST_STEP + 0.017)).abs() < 1e-12);
        assert!((drawer.pace(0.217) - (MOST_STEP + 0.117)).abs() < 1e-12);
        drawer.open(1.0, Hang::Own);
        assert_eq!(
            drawer.pace(0.005),
            0.005,
            "a fresh opening starts from the start"
        );
    }

    #[test]
    fn a_panel_hanging_from_the_bar_comes_out_of_its_foot_and_a_card_out_of_its_own_top() {
        assert_eq!(Run::new(card(), Some(32.0), 0.0).slot, 32.0);
        assert_eq!(Run::new(card(), None, 0.0).slot, 50.0);
        // A card above the bar's foot comes out of its own top either way.
        assert_eq!(Run::new(card(), Some(60.0), 0.0).slot, 50.0);
        // All in the slot, nothing of it shows, not even the shadow under its foot.
        let run = Run::new(card(), None, 470.0);
        assert_eq!(run.lift(0.0), 470.0 - 50.0);
    }

    #[test]
    fn a_hanging_drawer_hands_the_bar_its_slot_while_it_moves_and_glints_shut() {
        let picture = tiled();
        let mut drawer = Drawer::default();
        drawer.open(
            0.0,
            Hang::Bar {
                button: Some(300.0),
            },
        );
        let reveal = |drawer: &mut Drawer, t| {
            let reveal = step(drawer, &picture, t, false).unwrap();
            (reveal, reveal.slot())
        };
        let (_, start) = reveal(&mut drawer, 0.0);
        assert_eq!(
            (start.spread, start.strength),
            (0.0, 1.0),
            "lit from the button"
        );
        let mut t = 0.0;
        while t < CARD {
            t += 0.017;
            reveal(&mut drawer, t);
        }
        assert_eq!(reveal(&mut drawer, CARD / 2.0 + MOST_STEP).1.spread, 1.0);
        assert_eq!(
            reveal(&mut drawer, CARD + SLOT_OUT + 0.05).1.strength,
            0.0,
            "out once landed"
        );
        let at = |way| Reveal::new(opening_look(0.0, GONE, false), way).slot();
        assert_eq!(at(Way::Closing { since: 0.0 }).strength, 0.0);
        assert_eq!(
            at(Way::Closing { since: HOLD }).strength,
            1.0,
            "lit again as it goes back in"
        );
        let glint = at(Way::Closing {
            since: HOLD + CARD * (1.0 - GLINT / 2.0),
        });
        assert!(glint.strength > 1.0 + GLINT_LIGHT * 0.9);
        assert!(glint.spread < 1.0, "narrowing");
        assert_eq!(at(Way::Closing { since: CLOSE }).spread, 0.0, "sealed");
    }

    #[test]
    fn the_strongest_slot_is_the_one_the_bar_lights() {
        let slot = |strength| {
            Some(BarSlot {
                left: 10.0,
                right: 20.0,
                strength,
            })
        };
        assert_eq!(BarSlot::strongest(slot(0.5), slot(1.2)), slot(1.2));
        assert_eq!(BarSlot::strongest(slot(0.8), None), slot(0.8));
        assert_eq!(BarSlot::strongest(None, None), None);
    }

    #[test]
    fn every_drawer_effect_has_a_reduced_motion_version_that_follows_the_setting() {
        // Opening: no slot, no drawer, no pieces, no edge; a fade no longer than the short one.
        let picture = tiled();
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Bar { button: None });
        step(&mut drawer, &picture, 0.0, true);
        let half = step(&mut drawer, &picture, REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!(
            (half.alpha, half.look.travel, half.look.lit),
            (0.5, 1.0, false)
        );
        assert!(half.whole(), "drawn as its own picture");
        assert_eq!(half.slot(), SlotLight::OUT);
        assert!(
            step(&mut drawer, &picture, REDUCED_FADE, true)
                .unwrap()
                .settled()
        );
        // Closing: the same short fade.
        drawer.close_into_bar(layers(tiled()));
        assert_eq!(drawer.closing_reveal(0.0, true).unwrap().alpha, 1.0);
        let going = drawer.closing_reveal(REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!((going.alpha, going.look.lit), (0.5, false));
        assert_eq!(going.slot(), SlotLight::OUT);
        assert_eq!(drawer.closing_reveal(REDUCED_FADE, true), None);
        // Switched on mid-close, the drawer turns into the short fade at once.
        let mut drawer = Drawer::default();
        drawer.open(0.0, Hang::Own);
        drawer.drawn_at(CARD, false);
        drawer.close_into_bar(layers(tiled()));
        assert!(drawer.closing_reveal(0.0, false).unwrap().look.lit);
        let now = drawer.closing_reveal(REDUCED_FADE / 2.0, true).unwrap();
        assert!(!now.look.lit && now.whole());
        assert_eq!(drawer.closing_reveal(REDUCED_FADE, true), None);
    }

    #[test]
    fn a_picture_painted_without_pieces_comes_in_as_one() {
        let card = [Rectangle::new((8.0, 8.0).into(), (40.0, 30.0).into())];
        let bare = Painted::new((64, 64).into(), 1.25, |_| {}).unwrap();
        assert_eq!(pieces(&bare, &card), &card);
        assert!(bare.pieces.split.is_none(), "nothing to split");
        let tiled = Painted::new((64, 64).into(), 1.25, |p| {
            p.piece(10.0, 10.0, 20.0, 8.0);
            p.piece(32.0, 10.0, 20.0, 8.0);
        })
        .unwrap();
        let rects = pieces(&tiled, &card);
        assert_eq!(rects.len(), 2);
        assert!(
            tiled.pieces.split.is_some(),
            "its colours are split as it is painted"
        );
        // Recorded on whole device pixels, in logical ones.
        assert!((rects[0].loc.x - 9.6).abs() < 1e-9);
        assert!((rects[0].size.w - 20.8).abs() < 1e-9);
        assert!((rects[0].size.h - 8.8).abs() < 1e-9);
    }

    #[test]
    fn the_bare_card_is_painted_once_and_kept_across_openings() {
        let picture = Painted::new((300, 400).into(), 1.0, |p| {
            crate::panel::glass(p, 20.0, 20.0, 260.0, 300.0);
            p.piece(30.0, 30.0, 100.0, 40.0);
        })
        .unwrap();
        let mut drawer = Drawer::default();
        assert!(drawer.bare_card(&picture).is_some());
        let first = drawer.bare.as_ref().map(|bare| bare.buffer.clone());
        drawer.open(0.0, Hang::Own);
        drawer.close_into_bar(Vec::new());
        drawer.open(1.0, Hang::Own);
        drawer.bare_card(&picture);
        let second = drawer.bare.as_ref().map(|bare| bare.buffer.clone());
        assert_eq!(
            format!("{first:?}"),
            format!("{second:?}"),
            "the same buffer, not a repaint"
        );
    }
}

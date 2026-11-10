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
//!
//! `plan` works out each frame (the timing, where the drawer runs, the order its pieces go in and
//! their crops) without drawing anything; this module draws it.

mod plan;

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

use self::plan::{Crop, Piece, Plan, pieces, slid};
pub use self::plan::{Hang, MOST_PIECES, Reveal, card_in};
use crate::{
    bar::BarSlot,
    effects::{
        fx,
        lit_line::{Line, LitLine, Scanlines},
        shader::Shared,
    },
    paint::{Painted, Split},
    panel::Frame,
};

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

/// What a panel draws this frame: its own picture with up to `RIDERS` layers riding along in
/// front of it, front first, held without allocating.
pub struct Layers<'a> {
    shown: [Shown<'a>; RIDERS + 1],
    first: usize,
}

impl<'a> Layers<'a> {
    /// `main`, with `rider` in front of it if there is one.
    pub fn new(main: Shown<'a>, rider: Option<Shown<'a>>) -> Self {
        let mut shown = [main; RIDERS + 1];
        let first = match rider {
            Some(rider) => {
                shown[RIDERS - 1] = rider;
                RIDERS - 1
            }
            None => RIDERS,
        };
        Self { shown, first }
    }

    /// What `picture`, front first, shows: its last layer and up to `RIDERS` before it.
    fn of(picture: &'a [Layer]) -> Option<Self> {
        let (main, riders) = picture.split_last()?;
        let riders = &riders[riders.len().saturating_sub(RIDERS)..];
        let mut shown = [main.shown(); RIDERS + 1];
        let first = RIDERS - riders.len();
        for (slot, rider) in shown[first..RIDERS].iter_mut().zip(riders) {
            *slot = rider.shown();
        }
        Some(Self { shown, first })
    }

    pub fn as_slice(&self) -> &[Shown<'a>] {
        &self.shown[self.first..]
    }
}

/// What the bare card a picture is drawn on was painted for: its size, scale and glass.
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
    plan: Plan,
    /// The picture it closed on, front first, while it goes back in.
    picture: Vec<Layer>,
    /// The ids its crops are drawn under, and the picture they were made for.
    ids: Vec<Id>,
    made_for: u64,
    /// A picture's split colours, made the first time it moves rather than whenever it is
    /// painted, and the picture they were made for.
    split: Option<(u64, Split)>,
    /// The bare card, painted once for what it is kept for.
    bare: Option<(BareKey, Painted)>,
    lines: Lines,
}

impl Drawer {
    /// Opens at `now`, out of `hang`: from where it is drawn if it was still going back in,
    /// otherwise from nothing.
    pub fn open(&mut self, now: f64, hang: Hang) {
        self.plan.open(now, hang);
        self.picture = Vec::new();
    }

    /// Closes into its slot, keeping `picture`, what it last drew, front first, to take back in;
    /// empty when nothing is drawn. Closing again while it goes changes nothing. Returns whether
    /// it goes.
    pub fn close(&mut self, picture: Vec<Layer>) -> bool {
        if self.plan.closing() {
            return true;
        }
        let goes = self.plan.close(!picture.is_empty());
        if goes {
            self.picture = picture;
        }
        goes
    }

    /// Whether it is still going back in after closing.
    pub fn closing(&self) -> bool {
        self.plan.closing()
    }

    /// Takes it as drawn `since` seconds after it opened, without drawing it.
    #[cfg(test)]
    pub fn drawn_at(&mut self, since: f64, reduced_motion: bool) {
        self.plan.drawn_at(since, reduced_motion);
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
        let bottom = layers
            .last()
            .map_or(0.0, |main| main.at.y + main.painted.logical.h as f64);
        self.plan.reveal(
            frame.time,
            frame.reduced_motion,
            card,
            frame.bar.foot() as f64,
            bottom,
        )
    }

    /// Its picture at `frame`'s time while it goes back in, front first. `None` once it has
    /// gone, and the picture is let go.
    pub fn draw_closing(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: Frame,
        out: &mut impl Extend<FlowElement>,
    ) -> Option<Reveal> {
        let Some(mut reveal) = self.plan.closing_reveal(frame.time, frame.reduced_motion) else {
            self.picture = Vec::new();
            return None;
        };
        // Drawing changes the rest of the drawer, so the picture is held apart while it draws.
        let picture = std::mem::take(&mut self.picture);
        if let Some(layers) = Layers::of(&picture) {
            self.draw(renderer, frame, &mut reveal, layers.as_slice(), out);
        }
        self.picture = picture;
        Some(reveal)
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
        let run = self.plan.run();
        let whole = [Rectangle::new(run.card.loc - main.at, run.card.size)];
        let rects = pieces(&main.painted.pieces.rects, &whole);
        let serial = main.painted.pieces.serial;
        // Reduced motion, or settled: each picture whole.
        if reveal.whole() {
            self.plan.grow(reveal, rects, main.at, serial);
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
        let riders = self.plan.grow(reveal, rects, main.at, serial);
        for (n, rect) in rects.iter().enumerate() {
            let grown = self.plan.grown()[n];
            if grown > 0.0 {
                let piece = Piece {
                    rect: *rect,
                    grown,
                    at: main.at,
                    lift,
                };
                let tube = self.plan.tube(reveal, n);
                let look = PieceLook {
                    tube,
                    salt: n,
                    alpha: look.alpha,
                    now: frame.time,
                };
                self.piece(renderer, main.painted, piece, look, out);
            }
        }
        // Anything riding along comes in with the pieces as a whole.
        for (n, layer) in layers[..layers.len() - 1].iter().enumerate().take(RIDERS) {
            let alpha = look.alpha * layer.share * riders;
            let painted = layer.painted;
            out.extend(
                slid(painted.logical, layer.at, lift, run.slot)
                    .and_then(|crop| {
                        crop.element(
                            renderer,
                            &painted.buffer,
                            painted.scale,
                            painted.scale,
                            alpha,
                        )
                    })
                    .map(|element| cropped(element, &self.ids[RIDER_IDS + n])),
            );
        }
        // The bare card at the back.
        let painted = main.painted;
        if let Some(crop) = slid(painted.logical, main.at, lift, run.slot)
            && let Some(bare) = self.bare_card(painted)
        {
            out.extend(
                crop.element(renderer, bare, painted.scale, painted.scale, look.alpha)
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
        let slot = self.plan.slot(reveal)?;
        if self.plan.hangs_from_bar() {
            return Some(slot);
        }
        let line = own_slot(slot, self.plan.run().slot, frame.ring, scale);
        out.extend(
            self.lines
                .slot
                .line(renderer, &line, scale)
                .map(FlowElement::Line),
        );
        None
    }

    /// Fresh ids for a picture other than the last one, and its split colours if it comes in
    /// piece by piece.
    fn made_for_picture(&mut self, main: &Painted) {
        let serial = main.pieces.serial;
        let count = main.pieces.rects.len();
        if (1..=MOST_PIECES).contains(&count)
            && self
                .split
                .as_ref()
                .is_none_or(|(made_for, _)| *made_for != serial)
        {
            self.split = Split::of(main).map(|split| (serial, split));
        }
        let wanted = PIECE_IDS + 3 * count.clamp(1, MOST_PIECES);
        if self.made_for == serial && self.ids.len() >= wanted && serial != 0 {
            return;
        }
        self.made_for = serial;
        self.ids.clear();
        self.ids.resize_with(wanted, Id::new);
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
        if self.bare.as_ref().is_none_or(|(kept, _)| *kept != key) {
            let bare = Painted::new(main.logical, main.scale, |p| {
                p.f = glass.f;
                crate::panel::glass(p, glass.x, glass.y, glass.w, glass.h);
            })?;
            self.bare = Some((key, bare));
        }
        self.bare.as_ref().map(|(_, bare)| &bare.buffer)
    }

    /// One piece of `main`, with its colours split while it powers up or down.
    fn piece(
        &self,
        renderer: &mut GlesRenderer,
        main: &Painted,
        piece: Piece,
        look: PieceLook,
        out: &mut impl Extend<FlowElement>,
    ) {
        if main.logical.h <= 0 {
            return;
        }
        let Some(crop) = piece.crop(self.plan.run().slot) else {
            return;
        };
        let PieceLook {
            tube,
            salt,
            alpha,
            now,
        } = look;
        // How much of a tube it still is: the dimming, the tear and the split all go with it.
        let (dim, tear) = if tube > 0.0 {
            let (beat, torn) = fx::tube(now + salt as f64 * 0.37, PIECE_TEAR_CHANCE);
            let low = fx::tube_hash((now / fx::TUBE_FRAME) as i64 as u64, salt as u64 + 7);
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
        let ids = PIECE_IDS + 3 * salt;
        let split = SPLIT_PX * tube as f64;
        if split > 0.3
            && let Some((_, colours)) = self
                .split
                .as_ref()
                .filter(|(made_for, _)| *made_for == main.pieces.serial)
        {
            let sides = [(&colours.red, -1.0), (&colours.blue, 1.0)];
            for (n, (colour, side)) in sides.into_iter().enumerate() {
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
                            colours.scale,
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

/// How a piece looks this frame: how much of a picture on a tube it still is, its place in the
/// list, which keeps its flicker its own, the panel's opacity, and the tube's clock.
#[derive(Debug, Clone, Copy)]
struct PieceLook {
    tube: f32,
    salt: usize,
    alpha: f32,
    now: f64,
}

/// A card's own slot as a lit line in `ring` along the row of screen pixels just above `top`, on
/// a screen at `scale`: one screen pixel thick, its ends tapering off.
fn own_slot(slot: BarSlot, top: f64, ring: u32, scale: f64) -> Line {
    let ring = ring & 0xffffff00;
    let thick = scale.round().max(1.0) / scale;
    Line {
        left: slot.left,
        right: slot.right,
        y: (top * scale).round() / scale - thick / 2.0,
        thick,
        glow: 0.0,
        above: 0.0,
        tapered: true,
        fringed: false,
        core: crate::panel::lighter(ring, SLOT_LIGHT),
        halo: ring,
        strength: SLOT_ALPHA * slot.strength,
    }
}

/// The ids a drawer's crops are drawn under, which last as long as the picture they were made
/// for: the bare card, two layers riding along, then three for each piece, its crop and its red
/// and blue.
const CARD_ID: usize = 0;
const RIDER_IDS: usize = 1;
const RIDERS: usize = 2;
const PIECE_IDS: usize = RIDER_IDS + RIDERS;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture painted with `pieces`, each (x, y, w, h), on a glass card.
    fn painted(pieces: &[(f32, f32, f32, f32)]) -> Painted {
        Painted::new((300, 400).into(), 1.0, |p| {
            crate::panel::glass(p, 20.0, 20.0, 260.0, 300.0);
            for &(x, y, w, h) in pieces {
                p.piece(x, y, w, h);
            }
        })
        .unwrap()
    }

    #[test]
    fn pieces_are_recorded_on_whole_device_pixels_in_logical_ones() {
        let tiled = Painted::new((64, 64).into(), 1.25, |p| {
            p.piece(10.0, 10.0, 20.0, 8.0);
            p.piece(32.0, 10.0, 20.0, 8.0);
        })
        .unwrap();
        let rects = &tiled.pieces.rects;
        assert_eq!(rects.len(), 2);
        assert!((rects[0].loc.x - 9.6).abs() < 1e-9);
        assert!((rects[0].size.w - 20.8).abs() < 1e-9);
        assert!((rects[0].size.h - 8.8).abs() < 1e-9);
    }

    #[test]
    fn the_bare_card_is_painted_once_and_kept_across_openings() {
        let picture = painted(&[(30.0, 30.0, 100.0, 40.0)]);
        let mut drawer = Drawer::default();
        assert!(drawer.bare_card(&picture).is_some());
        let kept = |drawer: &Drawer| drawer.bare.as_ref().map(|(_, bare)| bare.pieces.serial);
        let first = kept(&drawer);
        drawer.open(0.0, Hang::Own);
        drawer.close(Vec::new());
        drawer.open(1.0, Hang::Own);
        drawer.bare_card(&picture);
        assert_eq!(kept(&drawer), first, "the same paint, not a repaint");
    }

    #[test]
    fn the_split_colours_are_made_once_a_picture_moves_and_kept_for_it() {
        let picture = painted(&[(30.0, 30.0, 100.0, 40.0), (150.0, 30.0, 100.0, 40.0)]);
        let mut drawer = Drawer::default();
        assert!(drawer.split.is_none(), "nothing made as it is painted");
        drawer.made_for_picture(&picture);
        let made = drawer
            .split
            .as_ref()
            .map(|(serial, split)| (*serial, split.scale));
        assert_eq!(made, Some((picture.pieces.serial, 0.5)));
        let first = drawer.ids[CARD_ID].clone();
        drawer.made_for_picture(&picture);
        assert_eq!(drawer.ids[CARD_ID], first, "kept from frame to frame");
        // A picture without pieces comes in whole, with nothing to split.
        let bare = painted(&[]);
        drawer.made_for_picture(&bare);
        assert_eq!(
            drawer.split.as_ref().map(|(serial, _)| *serial),
            Some(picture.pieces.serial)
        );
    }

    #[test]
    fn a_closing_drawer_draws_its_own_picture_last_with_up_to_two_riding_along() {
        let layer = |y| Layer::new(painted(&[]), Point::from((0.0, y)));
        let picture: Vec<Layer> = (0..4).map(|n| layer(n as f64)).collect();
        let layers = Layers::of(&picture).unwrap();
        let at: Vec<f64> = layers.as_slice().iter().map(|shown| shown.at.y).collect();
        assert_eq!(at, [1.0, 2.0, 3.0]);
        let one = [layer(7.0)];
        let layers = Layers::of(&one).unwrap();
        assert_eq!(layers.as_slice().len(), 1);
        assert!(Layers::of(&[]).is_none());
        let main = Shown::new(&one[0].painted, Point::from((0.0, 7.0)));
        let rider = Shown::new(&picture[0].painted, Point::from((0.0, 1.0)));
        let both = Layers::new(main, Some(rider));
        let at: Vec<f64> = both.as_slice().iter().map(|shown| shown.at.y).collect();
        assert_eq!(at, [1.0, 7.0]);
        assert_eq!(Layers::new(main, None).as_slice().len(), 1);
    }
}

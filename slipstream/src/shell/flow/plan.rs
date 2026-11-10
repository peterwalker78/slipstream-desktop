//! A drawer's plan: when it moves and how far, the order its pieces come and go in, how far each
//! has grown, the slot it comes out of and the crops it is drawn as. Nothing here draws; the
//! drawer turns each frame's plan into elements.

use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::{anim::Easing, bar::BarSlot, motion::REDUCED_FADE};

/// The bare card comes out over 0.3 s on a glide.
pub(super) const CARD: f64 = 0.3;
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
/// More pieces than this and the picture comes in whole.
pub const MOST_PIECES: usize = 32;

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
pub(super) struct Look {
    pub alpha: f32,
    pub travel: f32,
    pub lit: bool,
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

/// The pieces a picture comes in as: `rects`, those it was painted with, or `whole`, its card,
/// if it has none or too many.
pub(super) fn pieces<'a>(
    rects: &'a [Rectangle<f64, Logical>],
    whole: &'a [Rectangle<f64, Logical>; 1],
) -> &'a [Rectangle<f64, Logical>] {
    if rects.is_empty() || rects.len() > MOST_PIECES {
        whole
    } else {
        rects
    }
}

/// Where the drawer runs: the card where it rests, the line it comes out of, and the foot of the
/// card's picture, shadow and all. A panel hanging from the bar comes out of the bar's foot (or
/// the card's top, if that is higher); a card of its own comes out of its own top edge.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Run {
    pub card: Rectangle<f64, Logical>,
    pub slot: f64,
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
    pub fn lift(&self, travel: f32) -> f64 {
        (1.0 - travel as f64) * self.span()
    }

    /// Its leading edge on the screen, at `travel`.
    pub fn lead(&self, travel: f32) -> f64 {
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

/// A panel as drawn this frame: how much of it shows, whether it has settled, which is when
/// anything laid over it, such as the explorer's caret, can be drawn too, and the slot it asks
/// the bar for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reveal {
    pub(super) look: Look,
    way: Way,
    pub bar_slot: Option<BarSlot>,
}

impl Reveal {
    fn new(look: Look, way: Way) -> Self {
        Self {
            look,
            way,
            bar_slot: None,
        }
    }

    /// Its opacity.
    pub fn alpha(&self) -> f32 {
        self.look.alpha
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

/// A drawer going back in: when the close started on the clock it is drawn by, which the first
/// frame after it fills in.
struct Closing {
    started: Option<f64>,
}

/// A drawer's way in and out. It opens from nothing, or from wherever a close it interrupts has
/// got to, and closes by going back into its slot.
#[derive(Default)]
pub(super) struct Plan {
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
    /// The picture the reading order was worked out for.
    serial: u64,
    run: Run,
    closing: Option<Closing>,
    /// How far behind the clock the motion runs, after a slow first frame.
    lag: Option<f64>,
}

impl Plan {
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

    /// Closes into its slot, taking back what it last drew if `drew` says it drew anything.
    /// Closing again while it goes changes nothing. Returns whether it goes.
    pub fn close(&mut self, drew: bool) -> bool {
        if self.closing.is_some() {
            return true;
        }
        self.lag = None;
        self.opened = None;
        self.from = self.drawn;
        self.from_grown.clone_from(&self.grown);
        self.due.clear();
        if !drew || self.drawn.alpha <= 0.0 {
            self.drawn = GONE;
            return false;
        }
        self.closing = Some(Closing { started: None });
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

    /// Where the opening has got to at `now`, for a card at `card` whose picture's foot, shadow
    /// and all, is at `bottom`, under a bar whose foot is at `bar_foot`.
    pub fn reveal(
        &mut self,
        now: f64,
        reduced_motion: bool,
        card: Rectangle<f64, Logical>,
        bar_foot: f64,
        bottom: f64,
    ) -> Reveal {
        let opened = *self.opened.get_or_insert(now);
        let since = self.pace(now - opened);
        let look = opening_look(since, self.from, reduced_motion);
        self.drawn = look;
        let foot = matches!(self.hang, Hang::Bar { .. }).then_some(bar_foot);
        self.run = Run::new(card, foot, bottom);
        Reveal::new(look, Way::Opening { since })
    }

    /// How it shows at `now` while it goes back in. `None` once it has gone.
    pub fn closing_reveal(&mut self, now: f64, reduced_motion: bool) -> Option<Reveal> {
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

    /// Where the drawer runs, as last revealed.
    pub fn run(&self) -> Run {
        self.run
    }

    /// Works out how far each of `rects`, the pieces of the picture `serial` with its corner at
    /// `at`, has grown as `reveal` says, and returns how far they have grown on the whole. Drawn
    /// whole, every piece has.
    pub fn grow(
        &mut self,
        reveal: &Reveal,
        rects: &[Rectangle<f64, Logical>],
        at: Point<f64, Logical>,
        serial: u64,
    ) -> f32 {
        let count = rects.len();
        if reveal.whole() {
            // Settled, every piece of what is drawn now is whole, however many the picture has
            // come to since it opened, so that a close takes all of them back.
            if reveal.look.lit {
                self.grown.clear();
                self.grown.resize(count, 1.0);
            }
            return 1.0;
        }
        if self.serial != serial {
            self.serial = serial;
            self.order.clear();
        }
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

    /// How far each piece has grown, as last worked out.
    pub fn grown(&self) -> &[f32] {
        &self.grown
    }

    /// How much of a picture on a tube piece `n` still is, as `reveal` says.
    pub fn tube(&self, reveal: &Reveal, n: usize) -> f32 {
        tube(match reveal.way {
            Way::Opening { since } => way_in(self.order[n], self.order.len(), since),
            Way::Closing { since } => way_out(self.due[n], since),
        })
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

    /// The slot this frame, from where it opens out towards the card's sides, as `reveal` says.
    /// `None` while it is out or too short to see.
    pub fn slot(&self, reveal: &Reveal) -> Option<BarSlot> {
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
        (right - left >= 1.0).then_some(BarSlot {
            left,
            right,
            strength: light.strength,
        })
    }

    /// Whether it comes out of the bar, which then lights its slot.
    pub fn hangs_from_bar(&self) -> bool {
        matches!(self.hang, Hang::Bar { .. })
    }
}

/// A piece this frame: its box in the picture, how far it has grown, the picture's corner where
/// it rests, and how far above that the drawer is.
#[derive(Debug, Clone, Copy)]
pub(super) struct Piece {
    pub rect: Rectangle<f64, Logical>,
    pub grown: f32,
    pub at: Point<f64, Logical>,
    pub lift: f64,
}

/// Part of a picture drawn somewhere: its box in the picture, in logical pixels, and where on the
/// screen and how big it is drawn.
#[derive(Debug, Clone, Copy)]
pub(super) struct Crop {
    pub from: Rectangle<f64, Logical>,
    pub at: Point<f64, Logical>,
    pub size: Size<f64, Logical>,
}

impl Piece {
    /// The piece at its size, about its middle, moved with the drawer and cut off at the slot.
    pub fn crop(&self, slot: f64) -> Option<Crop> {
        let k = GROW_FROM + (1.0 - GROW_FROM) * self.grown as f64;
        let size = Size::<f64, Logical>::from((self.rect.size.w * k, self.rect.size.h * k));
        let middle = self.at
            + self.rect.loc
            + Point::from((self.rect.size.w / 2.0, self.rect.size.h / 2.0 - self.lift));
        let corner = middle - Point::from((size.w / 2.0, size.h / 2.0));
        let hidden = (slot - corner.y).max(0.0);
        if hidden >= size.h {
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

/// A picture `logical` big with its corner at `at`, raised by `lift` and cut off above `slot`.
pub(super) fn slid(
    logical: Size<i32, Logical>,
    at: Point<f64, Logical>,
    lift: f64,
    slot: f64,
) -> Option<Crop> {
    let top = at.y - lift;
    let hidden = (slot - top).max(0.0).ceil();
    if hidden >= logical.h as f64 {
        return None;
    }
    Some(Crop {
        from: Rectangle::new(
            (0.0, hidden).into(),
            (logical.w as f64, logical.h as f64 - hidden).into(),
        ),
        at: (at.x, top + hidden).into(),
        size: (logical.w as f64, logical.h as f64 - hidden).into(),
    })
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

    /// A panel's pieces: two rows of two tiles and a footer.
    fn tiled() -> Vec<Rectangle<f64, Logical>> {
        [
            (20.0, 20.0, 120.0, 80.0),
            (160.0, 20.0, 120.0, 80.0),
            (20.0, 120.0, 120.0, 80.0),
            (160.0, 120.0, 120.0, 80.0),
            (20.0, 320.0, 260.0, 40.0),
        ]
        .map(|(x, y, w, h)| Rectangle::new((x, y).into(), (w, h).into()))
        .to_vec()
    }

    fn card() -> Rectangle<f64, Logical> {
        Rectangle::new((110.0, 50.0).into(), (280.0, 360.0).into())
    }

    const AT: Point<f64, Logical> = Point::new(100.0, 40.0);
    /// The foot of the tiled picture, 400 pixels tall.
    const BOTTOM: f64 = 440.0;
    const FOOT: f64 = 32.0;

    /// A frame of `plan` at `time` with `rects` the pieces of picture `serial`, as the drawer
    /// works it out before drawing it.
    fn frame_of(
        plan: &mut Plan,
        rects: &[Rectangle<f64, Logical>],
        serial: u64,
        time: f64,
        reduced_motion: bool,
    ) -> Option<Reveal> {
        let reveal = if plan.closing() {
            plan.closing_reveal(time, reduced_motion)?
        } else {
            plan.reveal(time, reduced_motion, card(), FOOT, BOTTOM)
        };
        plan.grow(&reveal, rects, AT, serial);
        Some(reveal)
    }

    /// A frame of the tiled picture.
    fn step(plan: &mut Plan, time: f64, reduced_motion: bool) -> Option<Reveal> {
        frame_of(plan, &tiled(), 1, time, reduced_motion)
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
        let rects = tiled();
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        for n in 0..=30 {
            step(&mut plan, n as f64 * 0.017, false);
        }
        assert!(plan.close(true));
        step(&mut plan, 1.0, false).unwrap();
        // The footer goes first, then the second row right to left, then the first.
        let due = &plan.due;
        let gone_in: Vec<usize> = {
            let mut by_due: Vec<usize> = (0..rects.len()).collect();
            by_due.sort_by(|&a, &b| due[a].total_cmp(&due[b]));
            by_due
        };
        assert_eq!(gone_in, [4, 3, 2, 1, 0]);
        for (n, rect) in rects.iter().enumerate() {
            let rests = AT.y + rect.loc.y + rect.size.h;
            // Just before it is due, the edge is still below it; when it is gone, it still is.
            let edge = plan.run.lead(travel_closing(1.0, due[n]));
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
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        plan.drawn_at(LANDED, false);
        assert_eq!(plan.drawn.travel, 1.0);
        assert!(plan.close(true), "closing waits for nothing");
        assert!(plan.closing());
        assert_eq!(plan.closing_reveal(5.0, false).unwrap().look.travel, 1.0);
        let halfway = plan.closing_reveal(5.0 + HOLD + CARD / 2.0, false).unwrap();
        assert_eq!(halfway.alpha(), 1.0, "it goes back in, never fades");
        assert!(halfway.look.travel > 0.0 && halfway.look.travel < 1.0);
        assert_eq!(plan.closing_reveal(5.0 + CLOSE + 0.001, false), None);
        assert!(!plan.closing(), "and then the picture is let go");
    }

    #[test]
    fn a_panel_closed_before_it_was_drawn_has_nothing_to_take_back() {
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        assert!(!plan.close(true));
        assert!(!plan.close(false));
    }

    #[test]
    fn a_panel_opens_closes_and_reopens_mid_close_from_what_was_drawn() {
        let mut plan = Plan::default();
        let hang = Hang::Bar {
            button: Some(300.0),
        };
        plan.open(10.0, hang);
        let first = step(&mut plan, 10.0, false).unwrap();
        assert_eq!(first.look.travel, 0.0, "it starts in the slot");
        let mut t = 10.0;
        while !step(&mut plan, t, false).unwrap().settled() {
            t += 0.017;
            assert!(t < 10.5, "it settles");
        }
        assert!(plan.grown.iter().all(|&grown| grown == 1.0));
        // Closed, then reopened partway back in.
        assert!(plan.close(true));
        let mut drawn = None;
        for n in 0..12 {
            drawn = step(&mut plan, 20.0 + n as f64 * 0.017, false);
        }
        let drawn = drawn.unwrap();
        let shrunk = plan.grown.clone();
        assert!(drawn.look.travel < 1.0 && drawn.look.travel > 0.0);
        assert!(shrunk.iter().any(|&grown| grown < 1.0), "pieces were going");
        plan.open(30.0, hang);
        assert!(!plan.closing());
        let again = step(&mut plan, 30.0, false).unwrap();
        assert_eq!(
            again.look.travel, drawn.look.travel,
            "the card carries on from where it was"
        );
        assert_eq!(plan.from_grown, shrunk, "and so does each piece");
        let mut t = 30.0;
        while !step(&mut plan, t, false).unwrap().settled() {
            t += 0.017;
            assert!(t < 30.5);
        }
    }

    #[test]
    fn pieces_painted_after_it_settled_go_back_in_with_the_rest() {
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        let mut t = 0.0;
        while !step(&mut plan, t, false).unwrap().settled() {
            t += 0.017;
        }
        // Repainted while open, with two more pieces than it opened with.
        let mut more = tiled();
        more.push(Rectangle::new((20.0, 220.0).into(), (120.0, 80.0).into()));
        more.push(Rectangle::new((160.0, 220.0).into(), (120.0, 80.0).into()));
        frame_of(&mut plan, &more, 2, t + 0.5, false).unwrap();
        assert!(plan.close(true));
        let going = frame_of(&mut plan, &more, 2, 5.0, false).unwrap();
        assert!(!going.whole());
        assert_eq!(plan.grown().len(), more.len());
        assert!(
            plan.grown().iter().all(|&grown| grown > 0.9),
            "every piece starts back from whole: {:?}",
            plan.grown()
        );
    }

    #[test]
    fn the_first_slow_frame_holds_the_motion_up_and_the_rest_keep_to_the_clock() {
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        // The first frame took a tenth of a second to paint.
        assert_eq!(plan.pace(0.1), MOST_STEP);
        // After it, a frame moves it on by however long that frame took, even a slow one.
        assert!((plan.pace(0.117) - (MOST_STEP + 0.017)).abs() < 1e-12);
        assert!((plan.pace(0.217) - (MOST_STEP + 0.117)).abs() < 1e-12);
        plan.open(1.0, Hang::Own);
        assert_eq!(
            plan.pace(0.005),
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
        let mut plan = Plan::default();
        plan.open(
            0.0,
            Hang::Bar {
                button: Some(300.0),
            },
        );
        let reveal = |plan: &mut Plan, t| {
            let reveal = step(plan, t, false).unwrap();
            (reveal, reveal.slot())
        };
        let (first, start) = reveal(&mut plan, 0.0);
        assert_eq!(
            (start.spread, start.strength),
            (0.0, 1.0),
            "lit from the button"
        );
        assert_eq!(plan.slot(&first), None, "too short to see yet");
        let mut t = 0.0;
        while t < CARD {
            t += 0.017;
            reveal(&mut plan, t);
        }
        let (spread, wide) = reveal(&mut plan, CARD / 2.0 + MOST_STEP);
        assert_eq!(wide.spread, 1.0);
        let card = card();
        assert_eq!(
            plan.slot(&spread).map(|slot| (slot.left, slot.right)),
            Some((card.loc.x, card.loc.x + card.size.w)),
            "the card's width"
        );
        assert_eq!(
            reveal(&mut plan, CARD + SLOT_OUT + 0.05).1.strength,
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
    fn every_drawer_effect_has_a_reduced_motion_version_that_follows_the_setting() {
        // Opening: no slot, no drawer, no pieces, no edge; a fade no longer than the short one.
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Bar { button: None });
        step(&mut plan, 0.0, true);
        let half = step(&mut plan, REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!(
            (half.alpha(), half.look.travel, half.look.lit),
            (0.5, 1.0, false)
        );
        assert!(half.whole(), "drawn as its own picture");
        assert_eq!(half.slot(), SlotLight::OUT);
        assert!(step(&mut plan, REDUCED_FADE, true).unwrap().settled());
        // Closing: the same short fade.
        plan.close(true);
        assert_eq!(plan.closing_reveal(0.0, true).unwrap().alpha(), 1.0);
        let going = plan.closing_reveal(REDUCED_FADE / 2.0, true).unwrap();
        assert_eq!((going.alpha(), going.look.lit), (0.5, false));
        assert_eq!(going.slot(), SlotLight::OUT);
        assert_eq!(plan.closing_reveal(REDUCED_FADE, true), None);
        // Switched on mid-close, the drawer turns into the short fade at once.
        let mut plan = Plan::default();
        plan.open(0.0, Hang::Own);
        plan.drawn_at(CARD, false);
        plan.close(true);
        assert!(plan.closing_reveal(0.0, false).unwrap().look.lit);
        let now = plan.closing_reveal(REDUCED_FADE / 2.0, true).unwrap();
        assert!(!now.look.lit && now.whole());
        assert_eq!(plan.closing_reveal(REDUCED_FADE, true), None);
    }

    #[test]
    fn a_picture_painted_without_pieces_or_with_too_many_comes_in_as_one() {
        let whole = [Rectangle::new((8.0, 8.0).into(), (40.0, 30.0).into())];
        assert_eq!(pieces(&[], &whole), &whole);
        let many = vec![whole[0]; MOST_PIECES + 1];
        assert_eq!(pieces(&many, &whole), &whole);
        let rects = tiled();
        assert_eq!(pieces(&rects, &whole), rects.as_slice());
    }
}

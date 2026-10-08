//! One window kept in view on every workspace: docked to the bar, hanging from it as a pane of
//! glass in front of the desktop.
//!
//! The pane hangs from the bar's lower edge at its right-hand end, framed all round in the
//! bar's own material, so the two are one piece. It has three sizes. Docking lifts the
//! window out of its tile and up to the bar, further away as it goes and leaning back a little; undocking brings it forward
//! again into a tile. The glass is rigid throughout (`pane.rs`).
//!
//! This is only where the pane rests and how it is drawn on the way there. Which window is
//! docked, and what the keys do, is the compositor's, and changes on the keypress as ever.

use crate::{
    anim::Easing,
    layout::Rect,
    pane::{Axis, Camera, Pose},
};

/// How much of the bar's material shows round the pane, in logical pixels: down its sides,
/// under its foot, and between it and the bar, so the focus ring has room all the way round.
pub const FRAME: i32 = 4;
/// How long a window takes from its tile up to the bar, in animation seconds.
const UP: f64 = 0.38;
/// From one size to the next.
const BETWEEN: f64 = 0.3;
/// From the bar down into a tile.
const DOWN: f64 = 0.36;
/// How far the pane leans back on its way between the bar and a tile, in radians: its top,
/// the edge towards the bar, the further away.
const LEAN: f64 = 0.13;
/// How far it leans between sizes.
const STIR: f64 = 0.05;
/// How far the light on the pane's leading edge reaches, in logical pixels on the screen.
const EDGE_REACH: f64 = 7.0;
/// Half the height of the band of light that crosses the pane in flight, as a share of the pane's.
const BAND: f64 = 0.28;
/// How long the seam between the bar and a pane that has just landed stays lit.
const SEAT: f64 = 0.42;
/// How long the light takes to run the length of the pane's slot in the bar, once the pane has
/// landed.
const FILL: f64 = 0.3;
/// How much of the way down from the bar the slot's light takes to run out; its card closes over
/// the rest.
const DRAIN: f64 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Size {
    #[default]
    Small,
    Medium,
    Large,
}

impl Size {
    /// The size after this one, round and round.
    pub fn next(self) -> Size {
        match self {
            Size::Small => Size::Medium,
            Size::Medium => Size::Large,
            Size::Large => Size::Small,
        }
    }

    /// The share of the tiling area the pane takes, across and down.
    fn share(self) -> (f64, f64) {
        match self {
            Size::Small => (0.26, 0.3),
            Size::Medium => (0.36, 0.42),
            Size::Large => (0.5, 0.5),
        }
    }
}

/// The window docked to the bar, and the flight that is still bringing its pane to rest.
#[derive(Debug, Clone, PartialEq)]
pub struct Docked<W> {
    pub window: W,
    pub size: Size,
    pub flight: Option<Flight>,
    /// When the pane last came to rest against the bar, in animation seconds, if it flew there:
    /// the seam is lit for a moment afterwards.
    pub seated: Option<f64>,
    /// When the light in its slot in the bar starts to run, for a pane that flew up from among
    /// the windows: as it lands.
    pub lit_from: Option<f64>,
}

/// What the pane's slot in the bar shows at a moment, so that the slot, the bay and the seam are
/// one movement: the slot opens dark while its pane is in the air, the flash of the landing
/// crosses its edge, and the light runs into it from there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotLook {
    /// How much of the slot's card is there, 0 to 1.
    pub card: f32,
    /// The stretch of the strip its light is in, as shares of its length the way the light
    /// runs: it fills from the start, and runs out from the start too.
    pub lit: (f32, f32),
    /// How strongly its edge is lit, 0 to 1.
    pub flash: f32,
}

impl SlotLook {
    /// A pane hanging from the bar, long since landed.
    pub const AT_REST: SlotLook = SlotLook {
        card: 1.0,
        lit: (0.0, 1.0),
        flash: 0.0,
    };

    /// The slot of a pane that is docked: on `flight` if it is still flying, its slot's light
    /// starting at `lit_from`, last come to rest at `seated`.
    pub fn docked(
        flight: Option<&Flight>,
        lit_from: Option<f64>,
        seated: Option<f64>,
        now: f64,
    ) -> SlotLook {
        // Still on its way up from a tile: the slot opens with the bay, and waits.
        if let Some(flight) = flight.filter(|flight| flight.arriving() && !flight.done(now)) {
            let open = flight.bay(now).map_or(1.0, |(_, open)| open);
            return SlotLook {
                card: open,
                lit: (0.0, 0.0),
                flash: 0.0,
            };
        }
        let run = lit_from.map_or(1.0, |from| ((now - from) / FILL).clamp(0.0, 1.0));
        SlotLook {
            card: 1.0,
            lit: (0.0, Easing::OutCubic.at(run) as f32),
            flash: seam(seated, now),
        }
    }

    /// The slot of a pane on `flight` down from the bar: its light runs out, and then the card
    /// closes behind it.
    pub fn left(flight: &Flight, now: f64) -> SlotLook {
        let t = flight.progress(now);
        let closing = ((t - DRAIN) / (1.0 - DRAIN)).clamp(0.0, 1.0);
        SlotLook {
            card: (1.0 - closing) as f32,
            lit: ((t / DRAIN).clamp(0.0, 1.0) as f32, 1.0),
            flash: 0.0,
        }
    }
}

/// Where the docked window rests in tiling area `area`: under the bar at the right-hand end,
/// with room for the frame round it.
pub fn rect(area: Rect, size: Size) -> Rect {
    let (across, down) = size.share();
    let w = ((area.w as f64 * across).round() as i32).max(1);
    let h = ((area.h as f64 * down).round() as i32).max(1);
    Rect {
        x: area.x + area.w - FRAME - w,
        y: area.y + FRAME,
        w,
        h,
    }
}

/// Where it rests when streams run down the edge beside it. The tiling area runs on a little way
/// under the streams, where its windows keep clear with a margin of their own; the pane has no
/// margin, so it stops short by that much and is left a window's gap from them, as the tiles are.
pub fn rect_beside_streams(area: Rect, size: Size) -> Rect {
    let rect = rect(area, size);
    Rect {
        x: rect.x - crate::layout::OUTER_GAP,
        ..rect
    }
}

/// The bar's material round a pane resting at `rect`, its top on the bar's lower edge.
pub fn frame(rect: Rect) -> Rect {
    Rect {
        x: rect.x - FRAME,
        y: rect.y - FRAME,
        w: rect.w + 2 * FRAME,
        h: rect.h + 2 * FRAME,
    }
}

/// How strongly the seam between the bar and the pane is lit at `now`, for a pane that came to
/// rest at `seated`: full as it lands, and gone soon after.
pub fn seam(seated: Option<f64>, now: f64) -> f32 {
    let Some(seated) = seated else {
        return 0.0;
    };
    let t = (now - seated) / SEAT;
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    ((1.0 - t) * (1.0 - t)) as f32
}

/// Somewhere the pane can be: the rect it fills on the screen, and whether it hangs from the bar
/// there or lies among the windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub rect: [f64; 4],
    pub bar: bool,
}

impl Stop {
    /// Hanging from the bar at `rect`.
    pub fn at_bar(rect: Rect) -> Self {
        Self {
            rect: [rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64],
            bar: true,
        }
    }

    /// Among the windows, where a tile or a floating window is drawn.
    pub fn among(rect: [f64; 4]) -> Self {
        Self { rect, bar: false }
    }
}

/// How the pane is lit in flight, for `pane::Look`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lit {
    /// The edge leading the pane, lit: which, and how strongly.
    pub edge: (Axis, bool, f32),
    /// How far that light reaches into the pane, in the pane's own pixels: the same few pixels
    /// on the screen however far away the pane is.
    pub reach: f32,
    /// A band of light crossing the pane as it changes depth, as where two panes of glass meet:
    /// the y it is at in the space, half its height and its strength.
    pub band: (f64, f64, f32),
}

/// The pane on its way from one stop to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    from: Stop,
    to: Stop,
    start: f64,
    over: f64,
}

impl Flight {
    pub fn new(from: Stop, to: Stop, start: f64) -> Self {
        let over = match (from.bar, to.bar) {
            (false, true) => UP,
            (true, false) => DOWN,
            _ => BETWEEN,
        };
        Self {
            from,
            to,
            start,
            over,
        }
    }

    pub fn done(&self, now: f64) -> bool {
        now >= self.end()
    }

    /// When the pane comes to rest.
    pub fn end(&self) -> f64 {
        self.start + self.over
    }

    fn progress(&self, now: f64) -> f64 {
        // Exactly 1 once it is over, whatever the division makes of the last instant.
        if self.done(now) {
            return 1.0;
        }
        ((now - self.start) / self.over).clamp(0.0, 1.0)
    }

    /// Whether the pane is on its way up to the bar from among the windows.
    fn arriving(&self) -> bool {
        self.to.bar && !self.from.bar
    }

    /// Whether the pane is heading away from the eye: to somewhere it is drawn smaller.
    fn receding(&self) -> bool {
        self.to.rect[2] < self.from.rect[2]
    }

    /// The pane's width at the screen's own depth: the wider of its two stops, so the narrower
    /// one is the same pane further away.
    fn full(&self) -> f64 {
        self.from.rect[2].max(self.to.rect[2]).max(1.0)
    }

    /// The bay in the bar's material that the pane leaves or is heading for, and how much of it
    /// is there at `now`. One opens ahead of a pane coming to the bar, quickly, so the pane lands
    /// in it; the one a pane has left closes behind it.
    pub fn bay(&self, now: f64) -> Option<([f64; 4], f32)> {
        let t = self.progress(now);
        if self.to.bar {
            Some((self.to.rect, (t * 2.5).min(1.0) as f32))
        } else if self.from.bar {
            Some((self.from.rect, (1.0 - t * 4.0).max(0.0) as f32))
        } else {
            None
        }
    }

    /// The pane at `now` for the eye at `camera`: its pose, and how it is lit.
    ///
    /// The stops are rects as they appear. A narrower one is the same pane further off, so the
    /// depth is whatever makes the pane look that wide, and its middle is carried out along the
    /// line from the eye so that it appears where the rect is.
    pub fn at(&self, now: f64, camera: &Camera) -> (Pose, Lit) {
        let t = self.progress(now);
        let receding = self.receding();
        // Coming towards the eye a pane is quick off the mark and then accelerates into place;
        // going away it speeds off.
        // Exactly at either end, so a pane that has landed lies exactly where its window is.
        let (p, deep) = if t >= 1.0 {
            (1.0, 0.0)
        } else if t <= 0.0 {
            (0.0, 0.0)
        } else if receding {
            (t.powf(2.2), (std::f64::consts::PI * t).sin())
        } else {
            (Easing::Arrive.at(t), (std::f64::consts::PI * t).sin())
        };
        let mix = |a: f64, b: f64| a + (b - a) * p;
        let rect: [f64; 4] = std::array::from_fn(|i| mix(self.from.rect[i], self.to.rect[i]));
        // It leans back towards the bar, going up or coming down, so one is the other played
        // backwards; between sizes it only stirs.
        let lean = if self.from.bar && self.to.bar {
            STIR
        } else {
            LEAN
        };
        let full = self.full();
        let k = (rect[2] / full).max(1e-3);
        let pose = Pose {
            x: camera.x + (rect[0] + rect[2] / 2.0 - camera.x) / k,
            y: camera.y + (rect[1] + rect[3] / 2.0 - camera.y) / k,
            z: camera.distance * (1.0 / k - 1.0),
            yaw: 0.0,
            pitch: -lean * deep,
            w: full,
            h: rect[3] / k,
        };
        // Heading up to the bar its top edge leads; coming down from it, or growing, its foot.
        let rising = self.to.rect[1] + self.to.rect[3] < self.from.rect[1] + self.from.rect[3];
        let lit = Lit {
            edge: (Axis::Down, !rising, 0.95 * deep as f32),
            reach: (EDGE_REACH / k) as f32,
            // It crosses from the leading edge to the trailing one over the flight.
            band: (
                if rising {
                    rect[1] + rect[3] * (1.6 * t - 0.3)
                } else {
                    rect[1] + rect[3] * (1.3 - 1.6 * t)
                },
                (rect[3] * BAND).max(1.0),
                0.34 * deep as f32,
            ),
        };
        (pose, lit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::project;

    const AREA: Rect = Rect {
        x: 0,
        y: 32,
        w: 1490,
        h: 832,
    };
    const CAMERA: Camera = Camera {
        x: 768.0,
        y: 432.0,
        distance: 1689.6,
    };
    const TILE: [f64; 4] = [8.0, 40.0, 733.0, 816.0];

    fn corners(pose: &Pose) -> ((f64, f64), (f64, f64)) {
        (
            project(pose, &CAMERA, (0.0, 0.0)).unwrap(),
            project(pose, &CAMERA, (1.0, 1.0)).unwrap(),
        )
    }

    fn lies_over(pose: &Pose, rect: [f64; 4]) -> bool {
        let ((x0, y0), (x1, y1)) = corners(pose);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        near(x0, rect[0])
            && near(y0, rect[1])
            && near(x1, rect[0] + rect[2])
            && near(y1, rect[1] + rect[3])
    }

    #[test]
    fn the_frame_hangs_from_the_bar_with_no_gap_and_the_pane_sits_clear_inside_it() {
        for size in [Size::Small, Size::Medium, Size::Large] {
            let pane = rect(AREA, size);
            // The frame's top is the bar's lower edge, and it ends at the tiling area's.
            let frame = frame(pane);
            assert_eq!(frame.y, AREA.y);
            assert_eq!(frame.x + frame.w, AREA.x + AREA.w);
            // The pane is the frame's width in from every side, the bar's included, which is
            // room for the whole of the focus ring.
            assert_eq!(pane.y, AREA.y + FRAME);
            assert_eq!(frame.h, pane.h + 2 * FRAME);
            assert_eq!(frame.w, pane.w + 2 * FRAME);
        }
    }

    #[test]
    fn the_sizes_go_round_and_each_is_bigger_than_the_last() {
        assert_eq!(Size::Small.next(), Size::Medium);
        assert_eq!(Size::Medium.next(), Size::Large);
        assert_eq!(Size::Large.next(), Size::Small);
        let [small, medium, large] =
            [Size::Small, Size::Medium, Size::Large].map(|s| rect(AREA, s));
        assert!(small.w < medium.w && medium.w < large.w);
        assert!(small.h < medium.h && medium.h < large.h);
        // The largest is half the tiling area each way, and no more.
        assert_eq!((large.w, large.h), (AREA.w / 2, AREA.h / 2));
    }

    #[test]
    fn a_flight_starts_and_ends_flat_exactly_over_its_stops() {
        let small = rect(AREA, Size::Small);
        let large = rect(AREA, Size::Large);
        let flights = [
            Flight::new(Stop::among(TILE), Stop::at_bar(small), 10.0),
            Flight::new(Stop::at_bar(small), Stop::at_bar(large), 10.0),
            Flight::new(Stop::at_bar(large), Stop::among(TILE), 10.0),
        ];
        for flight in flights {
            let (first, lit) = flight.at(10.0, &CAMERA);
            assert!(lies_over(&first, flight.from.rect));
            assert_eq!(first.pitch, 0.0);
            assert_eq!(lit.edge.2, 0.0);
            let (last, lit) = flight.at(flight.end(), &CAMERA);
            assert!(lies_over(&last, flight.to.rect));
            assert_eq!(last.pitch, 0.0);
            assert_eq!(lit.band.2, 0.0);
            assert!(flight.done(flight.end()));
            assert!(!flight.done(flight.end() - 0.01));
        }
    }

    #[test]
    fn going_up_to_the_bar_the_pane_recedes_leans_back_and_its_top_edge_leads() {
        let small = rect(AREA, Size::Small);
        let flight = Flight::new(Stop::among(TILE), Stop::at_bar(small), 0.0);
        let (start, _) = flight.at(0.0, &CAMERA);
        let (half, lit) = flight.at(UP * 0.5, &CAMERA);
        let (end, _) = flight.at(UP, &CAMERA);
        assert_eq!(start.z, 0.0);
        assert!(half.z > 0.0 && end.z > half.z);
        // The same pane all the way: only its depth, place and lean change.
        assert_eq!(start.w, end.w);
        // A lean and no more, never a turn to one side, and its top is the further away.
        assert_eq!(half.yaw, 0.0);
        assert!(half.pitch < -0.1 && half.pitch > -0.2);
        let m = crate::pane::matrix(&half, &CAMERA);
        let depth = |y: f64| m[6] * half.w / 2.0 + m[7] * y + m[8];
        assert!(depth(0.0) > depth(half.h));
        assert_eq!(lit.edge.0, Axis::Down);
        assert!(!lit.edge.1);
        assert!(lit.edge.2 > 0.9);
        // Slow to leave: after half the time it has gone under a third of the way.
        let ((x0, _), _) = corners(&half);
        assert!(x0 < TILE[0] + 0.4 * (small.x as f64 - TILE[0]));
    }

    #[test]
    fn coming_down_the_pane_approaches_and_its_foot_leads() {
        let small = rect(AREA, Size::Small);
        let flight = Flight::new(Stop::at_bar(small), Stop::among(TILE), 0.0);
        let (start, _) = flight.at(0.0, &CAMERA);
        let (half, lit) = flight.at(DOWN * 0.5, &CAMERA);
        assert!(start.z > half.z && half.z > 0.0);
        assert!(lit.edge.1);
        // Coming down is going up played backwards: the same lean.
        let up = Flight::new(Stop::among(TILE), Stop::at_bar(small), 0.0);
        let (rising, _) = up.at(UP * 0.5, &CAMERA);
        assert_eq!(half.pitch, rising.pitch);
    }

    #[test]
    fn a_bay_opens_ahead_of_a_pane_coming_to_the_bar_and_closes_behind_one_leaving() {
        let (small, large) = (rect(AREA, Size::Small), rect(AREA, Size::Large));
        let up = Flight::new(Stop::among(TILE), Stop::at_bar(small), 0.0);
        let (at, shown) = up.bay(0.0).unwrap();
        assert_eq!(at, Stop::at_bar(small).rect);
        assert_eq!(shown, 0.0);
        // Open well before the pane is there.
        assert_eq!(up.bay(UP * 0.4).unwrap().1, 1.0);
        let grow = Flight::new(Stop::at_bar(small), Stop::at_bar(large), 0.0);
        assert_eq!(grow.bay(0.0).unwrap().0, Stop::at_bar(large).rect);
        let down = Flight::new(Stop::at_bar(large), Stop::among(TILE), 0.0);
        let (at, shown) = down.bay(0.0).unwrap();
        assert_eq!(at, Stop::at_bar(large).rect);
        assert_eq!(shown, 1.0);
        assert_eq!(down.bay(DOWN * 0.25).unwrap().1, 0.0);
    }

    #[test]
    fn the_slot_opens_dark_while_its_pane_is_in_the_air_and_fills_from_the_landing() {
        let bar = Stop::at_bar(rect(AREA, Size::Small));
        let flight = Flight::new(Stop::among(TILE), bar, 10.0);
        let landed = flight.end();
        let look = |now: f64| {
            let flying = (!flight.done(now)).then_some(&flight);
            SlotLook::docked(flying, Some(landed), Some(landed), now)
        };
        // Nothing of it before the pane leaves its tile; open, and still dark, before it lands.
        assert_eq!(look(10.0).card, 0.0);
        let nearly = look(landed - 0.01);
        assert_eq!(nearly.card, 1.0);
        assert_eq!(nearly.lit, (0.0, 0.0));
        assert_eq!(nearly.flash, 0.0);
        // The landing lights its edge with the seam, and the light sets off along the strip.
        let landing = look(landed);
        assert_eq!(landing.flash, seam(Some(landed), landed));
        assert_eq!(landing.lit, (0.0, 0.0));
        let part = look(landed + FILL / 2.0);
        assert!(part.lit.1 > 0.3 && part.lit.1 < 1.0, "{part:?}");
        assert_eq!(look(landed + FILL).lit, (0.0, 1.0));
        assert_eq!(look(landed + 5.0), SlotLook::AT_REST);
    }

    #[test]
    fn a_pane_changing_size_keeps_its_slot_lit() {
        let from = Stop::at_bar(rect(AREA, Size::Small));
        let to = Stop::at_bar(rect(AREA, Size::Medium));
        let flight = Flight::new(from, to, 10.0);
        // It flew up long ago, so its light has long been running.
        let look = SlotLook::docked(Some(&flight), Some(2.0), Some(flight.end()), 10.1);
        assert_eq!(look.card, 1.0);
        assert_eq!(look.lit, (0.0, 1.0));
    }

    #[test]
    fn a_pane_docked_without_a_flight_has_its_slot_whole_at_once() {
        assert_eq!(SlotLook::docked(None, None, None, 3.0), SlotLook::AT_REST);
    }

    #[test]
    fn the_slot_of_a_pane_coming_down_drains_and_then_closes() {
        let bar = Stop::at_bar(rect(AREA, Size::Small));
        let flight = Flight::new(bar, Stop::among(TILE), 10.0);
        let at = |share: f64| SlotLook::left(&flight, 10.0 + DOWN * share);
        assert_eq!(at(0.0), SlotLook::AT_REST);
        // Half way through the drain the card is whole and the light has left its first half.
        let draining = at(DRAIN / 2.0);
        assert_eq!(draining.card, 1.0);
        assert!((draining.lit.0 - 0.5).abs() < 1e-3 && draining.lit.1 == 1.0);
        // Dark before it starts to close, and gone as the pane lands.
        let dark = at(DRAIN);
        assert_eq!(dark.card, 1.0);
        assert_eq!(dark.lit.0, 1.0);
        assert!(at((1.0 + DRAIN) / 2.0).card < 0.6);
        assert_eq!(at(1.0).card, 0.0);
    }

    #[test]
    fn the_seam_is_lit_as_the_pane_lands_and_not_for_long() {
        assert_eq!(seam(None, 5.0), 0.0);
        assert_eq!(seam(Some(5.0), 4.9), 0.0);
        assert_eq!(seam(Some(5.0), 5.0), 1.0);
        assert!(seam(Some(5.0), 5.0 + SEAT * 0.5) < 0.3);
        assert_eq!(seam(Some(5.0), 5.0 + SEAT + 0.001), 0.0);
    }
}

//! One minimised window kept in view: pinned to the screen as a pane of glass in front of the
//! desktop, beside the streams, while its stream carries on saying how hard the app is working.
//!
//! The pane has two sizes, and they are one pane at two depths: a quarter of the tiling area, and
//! the same picture further away. Pinning brings it forward out of its stream, turning to face
//! the screen as it comes, and putting it away sends it back the way it came. The glass is rigid
//! throughout (`pane.rs`).
//!
//! This is only where the pane rests and how it is drawn on the way there. Which window is
//! pinned, and what the keys do, is the compositor's, and changes on the keypress as ever.

use crate::{
    anim::Easing,
    layout::{INNER_GAP, OUTER_GAP, Rect},
    pane::{Axis, Camera, Pose},
};

/// How big the small pane is beside the quarter, each way.
const SMALL: f64 = 0.42;
/// How long the pane takes out of its stream, in animation seconds.
const OUT: f64 = 0.34;
/// From one size to the other.
const BETWEEN: f64 = 0.3;
/// Back into its stream.
const AWAY: f64 = 0.34;
/// How far round the pane is turned in the mouth of its stream, in radians: almost edge on.
const TURNED: f64 = 1.15;
/// How far a pane turns on its way between sizes, as tiles passing through each other do.
const TURN: f64 = 0.1;
/// How much darker the pane is in the mouth of its stream.
const SHADE: f32 = 0.4;
/// How far the light on the pane's leading edge reaches, in logical pixels on the screen.
const EDGE_REACH: f64 = 7.0;
/// Half the width of the band of light that crosses the pane in flight, as a share of the pane's.
const BAND: f64 = 0.28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Quarter,
}

impl Size {
    /// The size after this one, or none: the pane goes back into its stream.
    pub fn next(self) -> Option<Size> {
        match self {
            Size::Small => Some(Size::Quarter),
            Size::Quarter => None,
        }
    }
}

/// A window pinned to the screen, and the flight that is still bringing its pane to rest.
#[derive(Debug, Clone, PartialEq)]
pub struct Pinned<W> {
    pub window: W,
    pub size: Size,
    pub flight: Option<Flight>,
}

/// Where the pane rests in tiling area `area`: the top right quarter, exactly where a tile there
/// would be, or the same corner smaller.
pub fn rect(area: Rect, size: Size) -> Rect {
    let right = area.x + area.w - OUTER_GAP;
    let left = area.x + (area.w + INNER_GAP) / 2;
    let w = (right - left).max(1);
    let h = ((area.h - 2 * OUTER_GAP - INNER_GAP) / 2).max(1);
    let (w, h) = match size {
        Size::Quarter => (w, h),
        Size::Small => (
            ((w as f64 * SMALL).round() as i32).max(1),
            ((h as f64 * SMALL).round() as i32).max(1),
        ),
    };
    Rect {
        x: right - w,
        y: area.y + OUTER_GAP,
        w,
        h,
    }
}

/// Somewhere the pane can be: the rect it appears to fill on the screen, how far it is turned
/// about its upright middle, and how much of it is there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub rect: [f64; 4],
    pub yaw: f64,
    pub alpha: f64,
    /// In the mouth of a stream rather than at rest.
    pub stream: bool,
}

impl Stop {
    pub fn resting(rect: Rect) -> Self {
        Self {
            rect: [rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64],
            yaw: 0.0,
            alpha: 1.0,
            stream: false,
        }
    }

    /// In the mouth of its stream: as wide as the stream's `button`, hanging from it, far away,
    /// turned almost edge on and not there yet. `pane` is the pane at rest, for its proportions.
    pub fn in_stream(button: Rect, pane: Rect) -> Self {
        let w = button.w as f64;
        let h = w * pane.h as f64 / pane.w.max(1) as f64;
        Self {
            rect: [button.x as f64, (button.y + button.h) as f64, w, h],
            yaw: TURNED,
            alpha: 0.0,
            stream: true,
        }
    }
}

/// How the pane is lit in flight, for `pane::Look`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lit {
    pub alpha: f32,
    pub shade: f32,
    /// The edge leading the pane, lit: which, and how strongly.
    pub edge: (Axis, bool, f32),
    /// How far that light reaches into the pane, in the pane's own pixels: the same few pixels
    /// on the screen however far away the pane is.
    pub reach: f32,
    /// A band of light crossing the pane as it changes depth, as where two panes of glass meet:
    /// the x it is at in the space, half its width and its strength.
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
        let over = if to.stream {
            AWAY
        } else if from.stream {
            OUT
        } else {
            BETWEEN
        };
        Self {
            from,
            to,
            start,
            over,
        }
    }

    pub fn done(&self, now: f64) -> bool {
        now - self.start >= self.over
    }

    /// Whether the pane is heading away from the eye: into its stream, or to the smaller size.
    fn receding(&self) -> bool {
        self.to.stream || (!self.from.stream && self.to.rect[2] < self.from.rect[2])
    }

    /// The pane at `now` for the eye at `camera`: its pose, where `full` is the pane's size at
    /// the screen's own depth, and how it is lit.
    ///
    /// The stops are rects as they appear. A smaller one is the same pane further off, so the
    /// depth is whatever makes a pane `full` wide look that wide, and its middle is carried out
    /// along the line from the eye so that it appears where the rect is.
    pub fn at(&self, now: f64, full: (f64, f64), camera: &Camera) -> (Pose, Lit) {
        let t = ((now - self.start) / self.over).clamp(0.0, 1.0);
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
        let sign = if receding { -1.0 } else { 1.0 };
        let yaw = mix(self.from.yaw, self.to.yaw) + TURN * deep * sign;
        // Solid almost at once on the way out of the stream, and held almost all the way back in.
        let seen = if self.to.stream {
            ((t - 0.75) * 4.0).clamp(0.0, 1.0)
        } else {
            (t * 4.0).clamp(0.0, 1.0)
        };
        let alpha = self.from.alpha + (self.to.alpha - self.from.alpha) * seen;
        let k = (rect[2] / full.0.max(1.0)).max(1e-3);
        let pose = Pose {
            x: camera.x + (rect[0] + rect[2] / 2.0 - camera.x) / k,
            y: camera.y + (rect[1] + rect[3] / 2.0 - camera.y) / k,
            z: camera.distance * (1.0 / k - 1.0),
            yaw,
            pitch: 0.0,
            w: full.0,
            h: rect[3] / k,
        };
        // In shadow in the stream's mouth, clear by the time it rests.
        let near_stream = match (self.from.stream, self.to.stream) {
            (true, _) => 1.0 - p,
            (_, true) => p,
            _ => 0.0,
        };
        let lit = Lit {
            alpha: alpha as f32,
            shade: SHADE * (near_stream * near_stream) as f32,
            // The pane hangs by its right-hand side, so its left edge leads it out and its right
            // one leads it home.
            edge: (Axis::Across, receding, 0.95 * deep as f32),
            reach: (EDGE_REACH / k) as f32,
            // It crosses from the leading edge to the trailing one over the flight.
            band: (
                if receding {
                    rect[0] + rect[2] * (1.3 - 1.6 * t)
                } else {
                    rect[0] + rect[2] * (1.6 * t - 0.3)
                },
                (rect[2] * BAND).max(1.0),
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

    fn corners(pose: &Pose) -> ((f64, f64), (f64, f64)) {
        (
            project(pose, &CAMERA, (0.0, 0.0)).unwrap(),
            project(pose, &CAMERA, (1.0, 1.0)).unwrap(),
        )
    }

    #[test]
    fn the_quarter_sits_where_a_top_right_tile_would() {
        let quarter = rect(AREA, Size::Quarter);
        assert_eq!(quarter.y, AREA.y + OUTER_GAP);
        assert_eq!(quarter.x + quarter.w, AREA.x + AREA.w - OUTER_GAP);
        // Half the area each way, less the gaps a tile keeps.
        assert_eq!(quarter.w, (AREA.w - 2 * OUTER_GAP - INNER_GAP) / 2);
        assert_eq!(quarter.h, (AREA.h - 2 * OUTER_GAP - INNER_GAP) / 2);
    }

    #[test]
    fn the_small_pane_keeps_the_quarters_corner_and_shape() {
        let (small, quarter) = (rect(AREA, Size::Small), rect(AREA, Size::Quarter));
        assert_eq!(small.y, quarter.y);
        assert_eq!(small.x + small.w, quarter.x + quarter.w);
        assert!(small.w < quarter.w / 2 && small.h < quarter.h / 2);
        let shape = |r: Rect| r.w as f64 / r.h as f64;
        assert!((shape(small) - shape(quarter)).abs() < 0.02);
    }

    #[test]
    fn the_sizes_go_small_quarter_and_away() {
        assert_eq!(Size::Small.next(), Some(Size::Quarter));
        assert_eq!(Size::Quarter.next(), None);
    }

    #[test]
    fn a_flight_ends_flat_exactly_over_where_the_pane_rests() {
        let (small, quarter) = (rect(AREA, Size::Small), rect(AREA, Size::Quarter));
        let full = (quarter.w as f64, quarter.h as f64);
        for to in [small, quarter] {
            let flight = Flight::new(Stop::resting(small), Stop::resting(to), 10.0);
            let (pose, lit) = flight.at(10.0 + BETWEEN, full, &CAMERA);
            let ((x0, y0), (x1, y1)) = corners(&pose);
            assert!((x0 - to.x as f64).abs() < 1e-6 && (y0 - to.y as f64).abs() < 1e-6);
            assert!((x1 - (to.x + to.w) as f64).abs() < 1e-6);
            assert!((y1 - (to.y + to.h) as f64).abs() < 1e-6);
            assert_eq!(pose.yaw, 0.0);
            assert_eq!(lit.alpha, 1.0);
            assert_eq!(lit.shade, 0.0);
            assert!(flight.done(10.0 + BETWEEN));
        }
    }

    #[test]
    fn the_small_pane_is_the_quarter_further_away() {
        let (small, quarter) = (rect(AREA, Size::Small), rect(AREA, Size::Quarter));
        let full = (quarter.w as f64, quarter.h as f64);
        let flight = Flight::new(Stop::resting(quarter), Stop::resting(small), 0.0);
        let (near, _) = flight.at(0.0, full, &CAMERA);
        let (far, _) = flight.at(BETWEEN, full, &CAMERA);
        assert_eq!(near.z, 0.0);
        assert!(far.z > CAMERA.distance);
        // The same pane all the way: only its depth and place change.
        assert_eq!(near.w, far.w);
    }

    #[test]
    fn the_pane_leaves_its_stream_turned_dark_and_unseen_and_comes_to_rest_facing() {
        let quarter = rect(AREA, Size::Quarter);
        let small = rect(AREA, Size::Small);
        let button = Rect {
            x: 1490,
            y: 42,
            w: 36,
            h: 40,
        };
        let full = (quarter.w as f64, quarter.h as f64);
        let flight = Flight::new(Stop::in_stream(button, quarter), Stop::resting(small), 0.0);
        let (first, lit) = flight.at(0.0, full, &CAMERA);
        assert_eq!(lit.alpha, 0.0);
        assert_eq!(lit.shade, SHADE);
        assert_eq!(first.yaw, TURNED);
        assert!(first.z > 10.0 * CAMERA.distance);
        // Solid a quarter of the way through, long before it lands.
        let (_, lit) = flight.at(OUT * 0.25, full, &CAMERA);
        assert_eq!(lit.alpha, 1.0);
        // Its left edge leads it out, lit most in the middle of the flight.
        let (_, lit) = flight.at(OUT * 0.5, full, &CAMERA);
        assert_eq!(lit.edge.0, Axis::Across);
        assert!(!lit.edge.1);
        assert!(lit.edge.2 > 0.9);
    }

    #[test]
    fn going_back_into_its_stream_the_pane_holds_until_the_last_and_its_right_edge_leads() {
        let quarter = rect(AREA, Size::Quarter);
        let button = Rect {
            x: 1490,
            y: 42,
            w: 36,
            h: 40,
        };
        let full = (quarter.w as f64, quarter.h as f64);
        let flight = Flight::new(
            Stop::resting(quarter),
            Stop::in_stream(button, quarter),
            0.0,
        );
        let (_, lit) = flight.at(AWAY * 0.7, full, &CAMERA);
        assert_eq!(lit.alpha, 1.0);
        assert!(lit.edge.1);
        let (last, lit) = flight.at(AWAY, full, &CAMERA);
        assert_eq!(lit.alpha, 0.0);
        assert_eq!(last.yaw, TURNED);
        // Slow to leave: after half the time it has gone under a third of the way.
        let (half, _) = flight.at(AWAY * 0.5, full, &CAMERA);
        let ((x0, _), _) = corners(&half);
        assert!(x0 < quarter.x as f64 + 0.3 * (button.x - quarter.x) as f64);
    }
}

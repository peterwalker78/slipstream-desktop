//! Screen zoom: the whole picture magnified about the pointer, for a closer look at anything.
//!
//! The zoom is applied to each finished frame, so every window, X11 or Wayland, and the desktop's
//! own panels are magnified alike. It is centred on the pointer itself: the point under the
//! pointer stays where it is and everything else spreads out around it, so moving the pointer to
//! an edge of the screen brings that edge into view, and whatever the pointer is over on screen is
//! what it is over underneath. Clicks need no mapping.
//!
//! Kept for the session only: zooming in again goes back to the last level used.

use smithay::utils::{Logical, Physical, Point};

/// The levels the keys step through. Off is 1.
const LEVELS: [f64; 7] = [1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];
/// The closest the zoom goes.
pub const MAX: f64 = 8.0;
/// Where zooming in starts, until another level has been used.
const FIRST: f64 = 2.0;
/// How long a step between levels takes.
const STEP_SECS: f64 = 0.15;
/// How much one notch of the wheel zooms by.
const PER_NOTCH: f64 = 1.25;

/// A zoom key's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Off,
}

#[derive(Debug, Clone)]
pub struct Zoom {
    /// The level being gone to; 1 is off.
    target: f64,
    /// The level the current step started from, and when, in wall seconds.
    from: f64,
    since: f64,
    /// What zooming in from off returns to.
    last: f64,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            target: 1.0,
            from: 1.0,
            since: f64::NEG_INFINITY,
            last: FIRST,
        }
    }
}

impl Zoom {
    /// The level shown at `now`, part way through a step or settled.
    pub fn level(&self, now: f64) -> f64 {
        let t = ((now - self.since) / STEP_SECS).clamp(0.0, 1.0);
        if t >= 1.0 {
            return self.target;
        }
        // Eased out, and evenly in ratio, so 4× to 8× feels the same as 1× to 2×.
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from * (self.target / self.from).powf(eased)
    }

    /// Whether anything is magnified at `now`.
    pub fn is_on(&self, now: f64) -> bool {
        self.level(now) > 1.0
    }

    /// A key pressed. With `reduced_motion` the level jumps rather than gliding.
    pub fn step(&mut self, step: Step, now: f64, reduced_motion: bool) {
        let level = self.target;
        let to = match step {
            Step::In if level <= 1.0 => self.last,
            Step::In => LEVELS
                .into_iter()
                .find(|l| *l > level + 1e-6)
                .unwrap_or(MAX),
            Step::Out => LEVELS
                .into_iter()
                .rev()
                .find(|l| *l < level - 1e-6)
                .unwrap_or(1.0),
            Step::Off => 1.0,
        };
        self.go(to, now, !reduced_motion);
    }

    /// The wheel turned with Super held: `notches` up (negative) zooms in, down zooms out. The
    /// wheel is gradual already, so each turn takes effect at once.
    pub fn wheel(&mut self, notches: f64, now: f64) {
        let to = self.level(now) * PER_NOTCH.powf(-notches);
        self.go(to, now, false);
    }

    fn go(&mut self, to: f64, now: f64, glide: bool) {
        let to = to.clamp(1.0, MAX);
        // Within a hair of 1 is off, so the wheel can always get all the way back.
        let to = if to < 1.01 { 1.0 } else { to };
        self.from = if glide { self.level(now) } else { to };
        self.since = now;
        self.target = to;
        if to > 1.0 {
            self.last = to;
        }
    }
}

/// How one screen is magnified: by `level`, about `centre`, in the screen's own physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub centre: Point<i32, Physical>,
    pub level: f64,
}

impl View {
    /// Nothing magnified.
    pub const NONE: View = View {
        centre: Point::new(0, 0),
        level: 1.0,
    };

    /// The view of a screen at `screen_loc` with scale `scale`, for a pointer at `pointer`.
    pub fn about(
        pointer: Point<f64, Logical>,
        screen_loc: Point<i32, Logical>,
        scale: f64,
        level: f64,
    ) -> Self {
        let local = (pointer - screen_loc.to_f64()).to_physical(scale);
        View {
            centre: local.to_i32_round(),
            level,
        }
    }

    /// Where a point of the unmagnified screen is shown.
    #[cfg(test)]
    pub fn to_screen(self, at: Point<f64, Physical>) -> Point<f64, Physical> {
        let centre = self.centre.to_f64();
        centre + (at - centre).upscale(self.level)
    }

    /// What is under a point of the screen, in the unmagnified screen's pixels.
    #[cfg(test)]
    pub fn to_content(self, at: Point<f64, Physical>) -> Point<f64, Physical> {
        let centre = self.centre.to_f64();
        centre + (at - centre).downscale(self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point<f64, Physical> {
        Point::from((x, y))
    }

    #[test]
    fn zooming_in_from_off_starts_at_twice_and_steps_up_to_the_most() {
        let mut zoom = Zoom::default();
        assert!(!zoom.is_on(0.0));
        let mut seen = Vec::new();
        for _ in 0..7 {
            zoom.step(Step::In, 0.0, true);
            seen.push(zoom.level(0.0));
        }
        assert_eq!(seen, [2.0, 3.0, 4.0, 6.0, 8.0, 8.0, 8.0]);
    }

    #[test]
    fn zooming_out_steps_down_to_off() {
        let mut zoom = Zoom::default();
        zoom.step(Step::In, 0.0, true);
        zoom.step(Step::Out, 0.0, true);
        assert_eq!(zoom.level(0.0), 1.5);
        zoom.step(Step::Out, 0.0, true);
        assert!(!zoom.is_on(0.0));
        zoom.step(Step::Out, 0.0, true);
        assert_eq!(zoom.level(0.0), 1.0);
    }

    #[test]
    fn off_remembers_the_level_for_next_time() {
        let mut zoom = Zoom::default();
        zoom.step(Step::In, 0.0, true);
        zoom.step(Step::In, 0.0, true);
        zoom.step(Step::Off, 0.0, true);
        assert!(!zoom.is_on(0.0));
        zoom.step(Step::In, 0.0, true);
        assert_eq!(zoom.level(0.0), 3.0);
    }

    #[test]
    fn a_step_glides_unless_motion_is_reduced() {
        let mut zoom = Zoom::default();
        zoom.step(Step::In, 10.0, false);
        let halfway = zoom.level(10.0 + STEP_SECS / 2.0);
        assert!(halfway > 1.0 && halfway < 2.0, "{halfway}");
        assert_eq!(zoom.level(10.0 + STEP_SECS), 2.0);

        let mut reduced = Zoom::default();
        reduced.step(Step::In, 10.0, true);
        assert_eq!(reduced.level(10.0), 2.0);
    }

    #[test]
    fn the_wheel_zooms_by_notches_and_comes_all_the_way_back() {
        let mut zoom = Zoom::default();
        zoom.wheel(-2.0, 0.0);
        assert!((zoom.level(0.0) - 1.5625).abs() < 1e-9);
        zoom.wheel(-40.0, 0.0);
        assert_eq!(zoom.level(0.0), MAX);
        zoom.wheel(40.0, 0.0);
        assert!(!zoom.is_on(0.0));
    }

    #[test]
    fn the_point_under_the_pointer_stays_put() {
        let view = View::about(
            Point::from((1700.0, 300.0)),
            Point::from((1536, 0)),
            1.25,
            2.0,
        );
        assert_eq!(view.centre, Point::from((205, 375)));
        let under = view.centre.to_f64();
        assert_eq!(view.to_screen(under), under);
        assert_eq!(view.to_content(under), under);
    }

    #[test]
    fn mapping_to_the_screen_and_back_agree() {
        let view = View {
            centre: Point::from((400, 300)),
            level: 2.0,
        };
        // Twice as far from the centre on screen as underneath.
        assert_eq!(view.to_screen(p(500.0, 300.0)), p(600.0, 300.0));
        assert_eq!(view.to_content(p(600.0, 300.0)), p(500.0, 300.0));
        for at in [p(0.0, 0.0), p(123.5, 77.25), p(1535.0, 959.0)] {
            let back = view.to_content(view.to_screen(at));
            assert!((back - at).x.abs() < 1e-9 && (back - at).y.abs() < 1e-9);
        }
    }

    #[test]
    fn the_whole_screen_stays_reachable() {
        // A pointer in a corner shows that corner: the view never leaves the screen.
        let (w, h) = (1536.0, 960.0);
        for (x, y) in [(0, 0), (1535, 959), (768, 480), (1535, 0)] {
            let view = View {
                centre: Point::from((x, y)),
                level: 4.0,
            };
            let top_left = view.to_content(p(0.0, 0.0));
            let bottom_right = view.to_content(p(w, h));
            assert!(top_left.x >= 0.0 && top_left.y >= 0.0, "{top_left:?}");
            assert!(
                bottom_right.x <= w && bottom_right.y <= h,
                "{bottom_right:?}"
            );
        }
    }

    #[test]
    fn nothing_magnified_is_the_identity() {
        let at = p(321.0, 654.0);
        assert_eq!(View::NONE.to_screen(at), at);
    }
}

//! Screen zoom: the whole picture magnified about the pointer, for a closer look at anything.
//!
//! The zoom is applied to each finished frame, so every window, X11 or Wayland, and the desktop's
//! own panels are magnified alike. It is centred on the pointer itself: the point under the
//! pointer stays where it is and everything else spreads out around it, so moving the pointer to
//! an edge of the screen brings that edge into view, and whatever the pointer is over on screen is
//! what it is over underneath. Clicks need no mapping.
//!
//! While typing, it follows the caret instead: whichever moved last, the pointer or the caret,
//! leads. The view stays put until the caret comes near one of its edges, then moves to show the
//! caret in the middle, so it isn't nudged along on every key. Apps that say where their caret is
//! (`caret.rs`) are followed; with the others, X11 ones among them, the pointer goes on leading.
//!
//! Kept for the session only: zooming in again goes back to the last level used.

use smithay::utils::{Logical, Physical, Point, Rectangle};

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
/// How near an edge of the view the caret comes, as a share of the view's width or height,
/// before the view moves to it.
const EDGE: f64 = 0.1;

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
    /// Where the view is centred while the caret leads; none while the pointer does.
    caret: Option<Pan>,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            target: 1.0,
            from: 1.0,
            since: f64::NEG_INFINITY,
            last: FIRST,
            caret: None,
        }
    }
}

/// How far through a step or a pan begun at `since` things are at `now`, eased out.
fn eased(since: f64, now: f64) -> f64 {
    let t = ((now - since) / STEP_SECS).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl Zoom {
    /// The level shown at `now`, part way through a step or settled.
    pub fn level(&self, now: f64) -> f64 {
        let t = eased(self.since, now);
        if t >= 1.0 {
            return self.target;
        }
        // Evenly in ratio, so 4× to 8× feels the same as 1× to 2×.
        self.from * (self.target / self.from).powf(t)
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
        } else {
            self.caret = None;
        }
    }

    /// Whether the screen is zoomed or on its way there.
    pub fn is_zooming(&self) -> bool {
        self.target > 1.0
    }

    /// The point the view is centred on at `now`, with the pointer at `pointer`.
    pub fn centre(&self, pointer: Point<f64, Logical>, now: f64) -> Point<f64, Logical> {
        self.caret.map_or(pointer, |pan| pan.at(now))
    }

    /// The pointer moved, and leads again.
    pub fn pointer_moved(&mut self) {
        self.caret = None;
    }

    /// The caret moved to `caret` on the screen `screen`, with the pointer at `pointer`. The caret
    /// leads from here until the pointer moves, and the view goes to it only when it is near an
    /// edge or out of sight: in one move with `reduced_motion`, gliding without.
    pub fn caret_moved(
        &mut self,
        caret: Rectangle<f64, Logical>,
        screen: Rectangle<f64, Logical>,
        pointer: Point<f64, Logical>,
        now: f64,
        reduced_motion: bool,
    ) {
        if !self.is_zooming() {
            return;
        }
        let pan = self.caret.get_or_insert(Pan::still(pointer));
        // A caret on another screen is looked for from its own middle.
        let start = if screen.contains(pan.to) {
            pan.to
        } else {
            caret.loc + caret.size.downscale(2.0).to_point()
        };
        let to = follow(start, caret, screen, self.target);
        let gap = to - pan.to;
        if gap.x.abs() >= 0.5 || gap.y.abs() >= 0.5 {
            pan.go(to, now, !reduced_motion);
        }
    }
}

/// The centre of a view following the caret, on its way from one place to the next.
#[derive(Debug, Clone, Copy)]
struct Pan {
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    since: f64,
}

impl Pan {
    fn still(at: Point<f64, Logical>) -> Self {
        Pan {
            from: at,
            to: at,
            since: f64::NEG_INFINITY,
        }
    }

    fn at(&self, now: f64) -> Point<f64, Logical> {
        self.from + (self.to - self.from).upscale(eased(self.since, now))
    }

    fn go(&mut self, to: Point<f64, Logical>, now: f64, glide: bool) {
        self.from = if glide { self.at(now) } else { to };
        self.to = to;
        self.since = now;
    }
}

/// Where to centre a view at `level` on `screen`, now centred on `centre`, so that `caret` shows.
/// The centre stays put while the caret is clear of the view's edges; otherwise it moves to put
/// the caret in the middle of the view, or as near as the screen's own edges allow. Each axis is
/// taken on its own, so reaching the end of a line doesn't move the view up or down as well.
pub fn follow(
    centre: Point<f64, Logical>,
    caret: Rectangle<f64, Logical>,
    screen: Rectangle<f64, Logical>,
    level: f64,
) -> Point<f64, Logical> {
    if level <= 1.0 {
        return centre;
    }
    // A view centred on c shows `seen` of the screen from start + (c - start) × (1 - 1/level),
    // which is what keeps c itself in place.
    let shrink = 1.0 - 1.0 / level;
    let axis = |centre: f64, lo: f64, len: f64, start: f64, screen_len: f64| {
        let seen = screen_len / level;
        let shown_from = start + (centre - start) * shrink;
        let margin = seen * EDGE;
        if lo >= shown_from + margin && lo + len <= shown_from + seen - margin {
            return centre;
        }
        let shown_from = (lo + len / 2.0 - seen / 2.0).clamp(start, start + screen_len - seen);
        start + (shown_from - start) / shrink
    };
    Point::from((
        axis(
            centre.x,
            caret.loc.x,
            caret.size.w,
            screen.loc.x,
            screen.size.w,
        ),
        axis(
            centre.y,
            caret.loc.y,
            caret.size.h,
            screen.loc.y,
            screen.size.h,
        ),
    ))
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

    /// The view of a screen at `screen_loc` with scale `scale`, centred on `centre`.
    pub fn about(
        centre: Point<f64, Logical>,
        screen_loc: Point<i32, Logical>,
        scale: f64,
        level: f64,
    ) -> Self {
        let local = (centre - screen_loc.to_f64()).to_physical(scale);
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

    fn l(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    /// A caret two pixels wide and twenty high at `x`, `y`.
    fn caret(x: f64, y: f64) -> Rectangle<f64, Logical> {
        Rectangle::new((x, y).into(), (2.0, 20.0).into())
    }

    fn screen() -> Rectangle<f64, Logical> {
        Rectangle::new((0.0, 0.0).into(), (1600.0, 1000.0).into())
    }

    /// Where a point of the screen is shown when the view at `level` is centred on `centre`.
    fn shown(centre: Point<f64, Logical>, at: Point<f64, Logical>, level: f64) -> (f64, f64) {
        let view = View {
            centre: centre.to_i32_round().to_physical(1),
            level,
        };
        let on = view.to_screen(Point::from((at.x, at.y)));
        (on.x, on.y)
    }

    #[test]
    fn the_view_stays_put_while_the_caret_is_clear_of_its_edges() {
        // At 4×, centred on the middle, the view shows 600..1000 across and 375..625 down.
        let centre = l(800.0, 500.0);
        for x in [650.0, 800.0, 950.0] {
            assert_eq!(follow(centre, caret(x, 490.0), screen(), 4.0), centre);
        }
    }

    #[test]
    fn a_caret_near_the_edge_brings_the_view_round_to_it() {
        let centre = l(800.0, 500.0);
        let to = follow(centre, caret(975.0, 490.0), screen(), 4.0);
        // Only across: the line it is on was in view already.
        assert_eq!(to.y, 500.0);
        let (x, _) = shown(to, l(976.0, 500.0), 4.0);
        assert!((x - 800.0).abs() < 4.0, "the caret is shown at {x}");
    }

    #[test]
    fn typing_along_a_line_moves_the_view_now_and_then_not_on_every_key() {
        let mut centre = l(800.0, 500.0);
        let mut moves = 0;
        for key in 0..300 {
            let to = follow(
                centre,
                caret(700.0 + key as f64 * 3.0, 490.0),
                screen(),
                4.0,
            );
            if to != centre {
                moves += 1;
                centre = to;
            }
        }
        // 900 pixels of typing across views 400 wide, moved to the middle each time.
        assert!((1..=6).contains(&moves), "{moves} moves");
    }

    #[test]
    fn a_caret_at_the_screen_edge_keeps_the_view_on_the_screen_and_still() {
        let first = follow(l(800.0, 500.0), caret(1597.0, 970.0), screen(), 4.0);
        let view = View {
            centre: first.to_i32_round().to_physical(1),
            level: 4.0,
        };
        let bottom_right = view.to_content(p(1600.0, 1000.0));
        assert!(
            bottom_right.x <= 1600.5 && bottom_right.y <= 1000.5,
            "{bottom_right:?}"
        );
        // The caret stays within the margin there, and the view stays where it is.
        assert_eq!(follow(first, caret(1597.0, 970.0), screen(), 4.0), first);
    }

    #[test]
    fn the_caret_leads_until_the_pointer_moves() {
        let mut zoom = Zoom::default();
        let pointer = l(100.0, 100.0);
        // Not zoomed: nothing to follow.
        zoom.caret_moved(caret(1200.0, 600.0), screen(), pointer, 0.0, true);
        assert_eq!(zoom.centre(pointer, 0.0), pointer);

        zoom.step(Step::In, 0.0, true);
        zoom.caret_moved(caret(1200.0, 600.0), screen(), pointer, 1.0, true);
        let centre = zoom.centre(pointer, 1.0);
        assert_ne!(centre, pointer);
        let (x, y) = shown(centre, l(1201.0, 610.0), 2.0);
        assert!(
            (x - 800.0).abs() < 4.0 && (y - 500.0).abs() < 4.0,
            "{x}, {y}"
        );

        zoom.pointer_moved();
        assert_eq!(zoom.centre(pointer, 1.0), pointer);
    }

    #[test]
    fn following_the_caret_glides_unless_motion_is_reduced() {
        let pointer = l(100.0, 100.0);
        let mut zoom = Zoom::default();
        zoom.step(Step::In, 0.0, true);
        zoom.caret_moved(caret(1200.0, 600.0), screen(), pointer, 1.0, false);
        let halfway = zoom.centre(pointer, 1.0 + STEP_SECS / 2.0);
        let there = zoom.centre(pointer, 1.0 + STEP_SECS);
        assert!(halfway.x > pointer.x && halfway.x < there.x, "{halfway:?}");

        let mut reduced = Zoom::default();
        reduced.step(Step::In, 0.0, true);
        reduced.caret_moved(caret(1200.0, 600.0), screen(), pointer, 1.0, true);
        assert_eq!(reduced.centre(pointer, 1.0), there);
    }

    #[test]
    fn zooming_off_hands_the_view_back_to_the_pointer() {
        let pointer = l(100.0, 100.0);
        let mut zoom = Zoom::default();
        zoom.step(Step::In, 0.0, true);
        zoom.caret_moved(caret(1200.0, 600.0), screen(), pointer, 1.0, true);
        zoom.step(Step::Off, 2.0, true);
        zoom.step(Step::In, 3.0, true);
        assert_eq!(zoom.centre(pointer, 3.0), pointer);
    }
}

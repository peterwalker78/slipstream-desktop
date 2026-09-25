//! Alt+Tab's deck: every open window lifted out of its place into a deck of glass panes receding
//! into depth, the chosen one at the front. Each Tab sends the front pane round to the back; letting
//! go of Alt flies every pane back to where its window is, the chosen one first.
//!
//! This is only where the panes are drawn. Which window is chosen, and what letting go does, is
//! the switcher's (`switcher.rs`), and it changes on the keypress as ever.

use crate::{
    anim::Easing,
    motion::HYPR,
    pane::{Camera, Pose},
};

/// Lifting out of place into the deck.
const OPEN: f64 = 0.45;
/// Each Tab's turn of the deck.
const STEP: f64 = 0.42;
/// Flying back when Alt is let go.
pub const RELEASE: f64 = 0.5;
/// The panes not chosen follow the chosen one back this much later.
const FOLLOW: f64 = 0.06;
/// Each pane further back lifts out a little later than the one in front.
const STAGGER: f64 = 0.05;
/// How many panes deep the deck is seen; any further back fade into the dark.
const DEPTH: f64 = 4.6;

pub struct Deck<W> {
    /// Most recently used first, as the switcher has them.
    pub windows: Vec<W>,
    /// When the deck starts to open, on wall time: once Alt has been held long enough.
    opened: f64,
    /// Each window's place in the deck, 0 at the front, as it was when the last turn began and
    /// where that turn takes it, and whether it goes round the side (front to back, or back to
    /// front) rather than straight.
    from: Vec<f64>,
    to: Vec<f64>,
    round: Vec<bool>,
    turned: f64,
    /// Where each pane was drawn last, for the flight back to start from.
    pub last: Vec<Option<Pose>>,
    /// When Alt was let go, and each pane's pose then.
    released: Option<(f64, Vec<Option<Pose>>)>,
    chosen: usize,
}

impl<W: Clone + PartialEq> Deck<W> {
    /// A deck over `windows` with `selected` at the front, opening at `opened`.
    pub fn new(windows: Vec<W>, selected: usize, opened: f64) -> Self {
        let n = windows.len();
        let ranks: Vec<f64> = (0..n).map(|i| rank(i, selected, n)).collect();
        Self {
            windows,
            opened,
            from: ranks.clone(),
            to: ranks,
            round: vec![false; n],
            turned: f64::MIN,
            last: vec![None; n],
            released: None,
            chosen: selected,
        }
    }

    /// The selection moved to `selected` at `now`: the deck turns to bring it to the front.
    pub fn select(&mut self, selected: usize, now: f64) {
        let n = self.windows.len();
        for i in 0..n {
            let (at, _) = self.place(i, now);
            let target = rank(i, selected, n);
            // Front to back, or back to front: round the side, not through the others.
            self.round[i] = (target - at).abs() > (n as f64 - 1.0) / 2.0 && n > 2;
            self.from[i] = at;
            self.to[i] = target;
        }
        self.turned = now;
        self.chosen = selected;
    }

    /// `window` has closed: its pane goes, and the rest keep their places.
    pub fn forget(&mut self, window: &W) {
        if let Some(i) = self.windows.iter().position(|w| w == window) {
            self.windows.remove(i);
            self.from.remove(i);
            self.to.remove(i);
            self.round.remove(i);
            self.last.remove(i);
            if self.chosen > i {
                self.chosen -= 1;
            }
            self.chosen = self.chosen.min(self.windows.len().saturating_sub(1));
        }
    }

    /// Alt let go at `now`: every pane heads back from where it is.
    pub fn release(&mut self, now: f64) {
        if self.released.is_none() {
            self.released = Some((now, self.last.clone()));
        }
    }

    pub fn releasing(&self) -> bool {
        self.released.is_some()
    }

    /// Whether anything of it is on screen yet.
    pub fn visible(&self, now: f64) -> bool {
        now >= self.opened
    }

    pub fn done(&self, now: f64) -> bool {
        self.released
            .as_ref()
            .is_some_and(|(at, _)| now - at >= RELEASE + FOLLOW)
    }

    /// Where window `i` is in the deck at `now`, 0 at the front, and how far round the side it
    /// has swung (0 to 1 and back) on its way.
    fn place(&self, i: usize, now: f64) -> (f64, f64) {
        let t = ((now - self.turned) / STEP).clamp(0.0, 1.0);
        let e = Easing::InOutCubic.at(t);
        let at = self.from[i] + (self.to[i] - self.from[i]) * e;
        let swing = if self.round[i] && t < 1.0 {
            (std::f64::consts::PI * e).sin()
        } else {
            0.0
        };
        (at, swing)
    }

    /// How dark the desktop behind the deck is, 0 to 1.
    pub fn dim(&self, now: f64) -> f64 {
        match &self.released {
            Some((at, _)) => 1.0 - Easing::OutCubic.at((now - at) / (RELEASE - 0.05)),
            None => Easing::OutCubic.at((now - self.opened) / OPEN),
        }
    }

    /// Where window `i`'s pane is drawn at `now`, its alpha and its shade, given where its window
    /// is (`live`, flat on the screen) and the screen's size. The deck is laid out for the window's
    /// own proportions, `own` (w, h).
    pub fn pose(
        &self,
        i: usize,
        live: Pose,
        own: (f64, f64),
        screen: (f64, f64),
        now: f64,
    ) -> (Pose, f32, f32) {
        let (at, swing) = self.place(i, now);
        let dealt = deck_pose(at, swing, own, screen);
        let alpha = (DEPTH - at).clamp(0.0, 1.0);
        let shade = (at * 0.12).clamp(0.0, 0.6) as f32;
        if let Some((released, poses)) = &self.released {
            let delay = if i == self.chosen { 0.0 } else { FOLLOW };
            let t = HYPR.at((now - released - delay) / RELEASE);
            let from = poses.get(i).copied().flatten().unwrap_or(dealt);
            let back = 1.0 - Easing::OutCubic.at((now - released - delay) / RELEASE);
            // A pane too far back to be seen comes into view as it flies home.
            let alpha = alpha + (1.0 - alpha) * (1.0 - back);
            return (from.mix(&live, t), alpha as f32, shade * back as f32);
        }
        let t = Easing::OutCubic.at((now - self.opened - self.from[i].min(DEPTH) * STAGGER) / OPEN);
        let opening_alpha = if at < DEPTH {
            1.0
        } else {
            1.0 - t * (1.0 - alpha)
        };
        (
            live.mix(&dealt, t),
            opening_alpha.min(1.0) as f32,
            shade * t as f32,
        )
    }

    /// Which window is at the front: the chosen one.
    pub fn front(&self) -> usize {
        self.chosen
    }
}

/// The eye for a deck on a screen `w` × `h`: a little above the middle, so the deck lies below
/// the line of sight.
pub fn camera(screen: (f64, f64)) -> Camera {
    Camera {
        x: screen.0 / 2.0,
        y: screen.1 * 0.52,
        distance: 1.06 * screen.0,
    }
}

/// Window `i`'s place with `selected` at the front, of `n`.
fn rank(i: usize, selected: usize, n: usize) -> f64 {
    ((i + n - selected) % n.max(1)) as f64
}

/// A pane `at` places back in the deck, `swing` of the way round the side, for a window `own`
/// in size on a screen `screen` in size. The deck steps back and to the right from the front
/// pane, which stands left of middle, each pane turned to face down the deck.
fn deck_pose(at: f64, swing: f64, own: (f64, f64), screen: (f64, f64)) -> Pose {
    let (sw, sh) = screen;
    // Laid out as for a 1600 × 1000 screen, and scaled to this one.
    let (kx, ky) = (sw / 1600.0, sh / 1000.0);
    let fit = (600.0 * kx / own.0.max(1.0)).min(420.0 * ky / own.1.max(1.0));
    Pose {
        x: kx * (540.0 + at * 180.0 - swing * 330.0),
        y: ky * (570.0 - at * 24.0 + swing * 150.0),
        z: kx * (at * 430.0 - swing * 120.0),
        yaw: 0.55 - swing * 0.35,
        pitch: 0.0,
        w: own.0 * fit,
        h: own.1 * fit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f64, f64) = (1536.0, 960.0);
    const OWN: (f64, f64) = (760.0, 900.0);

    fn live() -> Pose {
        Pose::flat([16.0, 50.0, 760.0, 900.0])
    }

    fn deck() -> Deck<&'static str> {
        Deck::new(vec!["a", "b", "c", "d"], 1, 10.0)
    }

    #[test]
    fn it_starts_where_the_windows_are_and_ends_dealt() {
        let deck = deck();
        let (start, ..) = deck.pose(1, live(), OWN, SCREEN, 10.0);
        assert_eq!(start, live());
        let (dealt, alpha, _) = deck.pose(1, live(), OWN, SCREEN, 12.0);
        assert_eq!(dealt, deck_pose(0.0, 0.0, OWN, SCREEN));
        assert_eq!(alpha, 1.0);
    }

    #[test]
    fn a_tab_sends_the_front_pane_round_to_the_back() {
        let mut deck = deck();
        deck.select(2, 20.0);
        let (mid, swing) = deck.place(1, 20.0 + STEP / 2.0);
        assert!(mid > 0.5 && swing > 0.9, "on its way round the side");
        assert_eq!(deck.place(1, 20.0 + STEP), (3.0, 0.0));
        assert_eq!(deck.place(2, 20.0 + STEP), (0.0, 0.0));
        assert_eq!(deck.front(), 2);
    }

    #[test]
    fn letting_go_flies_every_pane_home() {
        let mut deck = deck();
        deck.last = vec![Some(deck_pose(3.0, 0.0, OWN, SCREEN)); 4];
        deck.release(30.0);
        assert!(!deck.done(30.0 + RELEASE));
        assert!(deck.done(30.0 + RELEASE + FOLLOW + 1e-9));
        let (home, alpha, shade) = deck.pose(1, live(), OWN, SCREEN, 30.0 + RELEASE);
        assert_eq!(home, live());
        assert_eq!((alpha, shade), (1.0, 0.0));
        assert!(deck.dim(30.0 + RELEASE) <= 0.0);
    }

    #[test]
    fn a_closed_window_leaves_the_deck() {
        let mut deck = deck();
        deck.forget(&"a");
        assert_eq!(deck.windows, ["b", "c", "d"]);
        assert_eq!(deck.front(), 0);
    }
}

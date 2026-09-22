//! Gravity, the arrangements beside tiling. Super+PgUp and Super+PgDn move the focused window one
//! rung along a fixed ladder of layouts, lightest first:
//!
//! distant · orbit · grid · centre · wide · spotlight
//!
//! Every press moves exactly one rung, and the opposite key steps straight back, so the same keys
//! from the same layout always give the same result. The ends stop rather than wrap. Heavier
//! gives the window the centre, then more of the screen; lighter makes every window the same
//! size, then puts the window in orbit around another, then off to the strip along the bottom.
//!
//! **Tiling is not a rung.** It sat in the middle of the ladder once, which made stepping past
//! the middle turn gravity off and on again — a confusing way back to tiling, and a toast every
//! time. Super+T is the way back. From tiling these keys turn gravity on at the end they point
//! at: heavier gives the window the centre, lighter puts every window in the grid.
//!
//! Windows keep their order from the tiling tree in every layout, so focusing another window never
//! moves anything. Pure logic, generic over the window type, so it's unit-tested without Wayland.

use crate::layout::Rect;

/// A place on the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rung {
    /// In the strip along the bottom, around another window's centre.
    Distant,
    /// In a column beside another window's centre.
    Orbit,
    /// Every window the same size, in an even grid.
    Grid,
    /// Gravity off: the tiling tree. Not on the ladder — Super+T is the way to it and back — but
    /// still what every window is on while gravity is off, and what a record calls that.
    Tiling,
    /// The middle of the screen, the others in columns either side.
    Centre,
    /// Seven tenths of the width, the others in one column.
    Wide,
    /// The whole width, the others in a strip below.
    Spotlight,
}

impl Rung {
    /// Lightest first. Tiling is deliberately absent: gravity is either on or off.
    pub const LADDER: [Rung; 6] = [
        Rung::Distant,
        Rung::Orbit,
        Rung::Grid,
        Rung::Centre,
        Rung::Wide,
        Rung::Spotlight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rung::Distant => "distant",
            Rung::Orbit => "orbit",
            Rung::Grid => "grid",
            Rung::Tiling => "tiling",
            Rung::Centre => "centre",
            Rung::Wide => "wide",
            Rung::Spotlight => "spotlight",
        }
    }

    /// `label` read back, from a recorded layout. Anything else is no rung at all rather than a
    /// guess: a hand-edited record shouldn't rearrange a workspace. Tiling is read back as well
    /// as the ladder's rungs, since that is what a record calls a window with gravity off.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::LADDER
            .into_iter()
            .chain([Rung::Tiling])
            .find(|rung| rung.label() == label)
    }

    /// Its place on the ladder, from 0 for the lightest, or `None` for tiling, which is off it.
    pub fn position(self) -> Option<usize> {
        Self::LADDER.iter().position(|rung| *rung == self)
    }
}

/// How the centre window is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Centre,
    Wide,
    Spotlight,
}

impl Shape {
    fn rung(self) -> Rung {
        match self {
            Shape::Centre => Rung::Centre,
            Shape::Wide => Rung::Wide,
            Shape::Spotlight => Rung::Spotlight,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Mode<T> {
    Off,
    Grid,
    Mass {
        centre: T,
        shape: Shape,
        /// The windows in the strip along the bottom. The rest orbit.
        distant: Vec<T>,
    },
}

/// What a step did, for the tag and messages that report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The window is on this rung now.
    Moved(Rung),
    /// Heavier from the top rung: nothing changed.
    Heaviest,
    /// Lighter from the bottom rung: nothing changed.
    Lightest,
    /// The window is the only one on its workspace, so every rung would look the same.
    Alone,
}

#[derive(Debug, Clone)]
pub struct Gravity<T> {
    mode: Mode<T>,
    pinned: Vec<T>,
    /// The orbit slots from the last layout, so pinned windows keep their place.
    slots: Vec<T>,
}

impl<T> Default for Gravity<T> {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            pinned: Vec::new(),
            slots: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> Gravity<T> {
    /// Whether gravity rather than the tiling tree places the windows.
    pub fn is_on(&self) -> bool {
        self.mode != Mode::Off
    }

    /// `None` unless a window has the centre.
    pub fn centre(&self) -> Option<&T> {
        match &self.mode {
            Mode::Mass { centre, .. } => Some(centre),
            _ => None,
        }
    }

    /// Where `id` is on the ladder. Every window is on `Tiling` while gravity is off, and on
    /// `Grid` in the grid.
    pub fn rung(&self, id: &T) -> Rung {
        match &self.mode {
            Mode::Off => Rung::Tiling,
            Mode::Grid => Rung::Grid,
            Mode::Mass {
                centre,
                shape,
                distant,
            } => {
                if centre == id {
                    shape.rung()
                } else if distant.contains(id) {
                    Rung::Distant
                } else {
                    Rung::Orbit
                }
            }
        }
    }

    /// Gravity as it was, put back from a recorded layout: the centre with its rung, the distant
    /// windows, and whether the grid was on. A centre that never came back leaves gravity off,
    /// because a workspace of orbit windows with nothing to orbit is not a layout anyone recorded.
    pub fn restore(centre: Option<(T, Rung)>, distant: Vec<T>, pinned: Vec<T>, grid: bool) -> Self {
        let shape = |rung| match rung {
            Rung::Wide => Some(Shape::Wide),
            Rung::Spotlight => Some(Shape::Spotlight),
            Rung::Centre => Some(Shape::Centre),
            _ => None,
        };
        let mode = match centre.and_then(|(centre, rung)| Some((centre, shape(rung)?))) {
            Some((centre, shape)) => Mode::Mass {
                distant: distant.into_iter().filter(|w| *w != centre).collect(),
                centre,
                shape,
            },
            None if grid => Mode::Grid,
            None => Mode::Off,
        };
        Self {
            mode,
            pinned,
            slots: Vec::new(),
        }
    }

    /// Back to tiling, with every weight cleared.
    pub fn off(&mut self) {
        self.mode = Mode::Off;
        self.slots.clear();
    }

    /// One rung heavier (`heavier`) or lighter for `id`. `others` are the workspace's other
    /// windows, most recently used first: stepping down from the grid puts `id` in orbit around
    /// the first of them, the window used before it.
    pub fn step(&mut self, id: &T, heavier: bool, others: &[T]) -> Step {
        if others.is_empty() {
            return Step::Alone;
        }
        let rung = self.rung(id);
        // Tiling is off the ladder, so from it these keys turn gravity on at the end they point
        // at rather than stepping: heavier takes the centre, lighter goes to the grid.
        if rung == Rung::Tiling {
            return if heavier {
                self.mode = Mode::Mass {
                    centre: id.clone(),
                    shape: Shape::Centre,
                    distant: Vec::new(),
                };
                Step::Moved(Rung::Centre)
            } else {
                self.mode = Mode::Grid;
                self.slots.clear();
                Step::Moved(Rung::Grid)
            };
        }
        let at = rung
            .position()
            .expect("every rung but tiling is on the ladder");
        let to = match (rung, heavier) {
            (Rung::Spotlight, true) => return Step::Heaviest,
            (Rung::Distant, false) => return Step::Lightest,
            (_, true) => Rung::LADDER[at + 1],
            (_, false) => Rung::LADDER[at - 1],
        };
        match (rung, to) {
            (Rung::Distant, Rung::Orbit) => {
                if let Mode::Mass { distant, .. } = &mut self.mode {
                    distant.retain(|w| w != id);
                }
            }
            (Rung::Orbit, Rung::Distant) => {
                if let Mode::Mass { distant, .. } = &mut self.mode {
                    distant.push(id.clone());
                }
            }
            (_, Rung::Grid) => {
                self.mode = Mode::Grid;
                self.slots.clear();
            }
            (Rung::Grid, Rung::Orbit) => {
                self.mode = Mode::Mass {
                    centre: others[0].clone(),
                    shape: Shape::Centre,
                    distant: Vec::new(),
                };
            }
            // Heavier out of the grid: this window takes the centre and the rest orbit it. The
            // ladder's two halves meet here now that tiling is not between them.
            (Rung::Grid, Rung::Centre) => {
                self.mode = Mode::Mass {
                    centre: id.clone(),
                    shape: Shape::Centre,
                    distant: Vec::new(),
                };
            }
            (_, Rung::Centre | Rung::Wide | Rung::Spotlight) => {
                if let Mode::Mass { shape, .. } = &mut self.mode {
                    *shape = match to {
                        Rung::Wide => Shape::Wide,
                        Rung::Spotlight => Shape::Spotlight,
                        _ => Shape::Centre,
                    };
                }
            }
            _ => unreachable!("every step is to a neighbouring rung"),
        }
        Step::Moved(to)
    }

    /// Whether `id` is pinned to its orbit slot.
    pub fn is_pinned(&self, id: &T) -> bool {
        self.pinned.contains(id)
    }

    /// Pins or unpins an orbit window to its slot. Returns whether it's pinned now.
    #[allow(dead_code)] // Bullet time's `p` key.
    pub fn toggle_pin(&mut self, id: &T) -> bool {
        if self.pinned.contains(id) {
            self.pinned.retain(|w| w != id);
            false
        } else {
            self.pinned.push(id.clone());
            true
        }
    }

    /// A window left the workspace. If it was the centre, the most recently used of
    /// `remaining` (most recent first) takes its place, on the same rung, and is returned; with
    /// none left, gravity turns off.
    pub fn remove(&mut self, id: &T, remaining: &[T]) -> Option<T> {
        self.pinned.retain(|w| w != id);
        self.slots.retain(|w| w != id);
        let Mode::Mass {
            centre, distant, ..
        } = &mut self.mode
        else {
            return None;
        };
        distant.retain(|w| w != id);
        if centre != id {
            return None;
        }
        match remaining.first() {
            Some(next) => {
                distant.retain(|w| w != next);
                *centre = next.clone();
                Some(next.clone())
            }
            None => {
                self.off();
                None
            }
        }
    }

    /// Where each of `windows`, in the tiling tree's order, goes in `area`, with the same gaps as
    /// tiling. Empty while gravity is off, or while its centre isn't among `windows`.
    pub fn rects(
        &mut self,
        windows: &[T],
        area: Rect,
        outer_gap: i32,
        inner_gap: i32,
    ) -> Vec<(T, Rect)> {
        // Normalised to the area: x, y, width, height.
        let placed: Vec<(T, [f64; 4])> = match &self.mode {
            Mode::Off => return Vec::new(),
            Mode::Grid => grid(windows),
            Mode::Mass {
                centre,
                shape,
                distant,
            } => {
                if !windows.contains(centre) {
                    return Vec::new();
                }
                let (centre, shape) = (centre.clone(), *shape);
                let others = windows.iter().filter(|w| **w != centre);
                let far: Vec<T> = others
                    .clone()
                    .filter(|w| distant.contains(w))
                    .cloned()
                    .collect();
                let near: Vec<T> = others.filter(|w| !distant.contains(w)).cloned().collect();
                let slots = self.orbit_slots(near);
                mass(centre, shape, &slots, &far)
            }
        };

        let half = inner_gap / 2;
        let a = area.inset(outer_gap - half);
        let (ax, ay, aw, ah) = (a.x as f64, a.y as f64, a.w as f64, a.h as f64);
        placed
            .into_iter()
            .map(|(w, [x, y, nw, nh])| {
                // Edges rounded rather than sizes, so neighbours share an edge exactly.
                let (left, top) = ((ax + x * aw).round(), (ay + y * ah).round());
                let (right, bottom) = ((ax + (x + nw) * aw).round(), (ay + (y + nh) * ah).round());
                let rect = Rect {
                    x: left as i32 + half,
                    y: top as i32 + half,
                    w: ((right - left) as i32 - 2 * half).max(1),
                    h: ((bottom - top) as i32 - 2 * half).max(1),
                };
                (w, rect)
            })
            .collect()
    }

    /// The orbit windows in slot order: pinned ones keep last layout's slot, the rest fill in
    /// in the tree's order.
    fn orbit_slots(&mut self, near: Vec<T>) -> Vec<T> {
        let mut slots: Vec<Option<T>> = vec![None; near.len()];
        for w in near.iter().filter(|w| self.pinned.contains(w)) {
            if let Some(i) = self.slots.iter().position(|slot| slot == w) {
                if i < slots.len() && slots[i].is_none() {
                    slots[i] = Some(w.clone());
                }
            }
        }
        let free: Vec<T> = near
            .iter()
            .filter(|w| !slots.iter().flatten().any(|slot| slot == *w))
            .cloned()
            .collect();
        let mut free = free.into_iter();
        let slots: Vec<T> = slots
            .into_iter()
            .filter_map(|slot| slot.or_else(|| free.next()))
            .collect();
        self.slots = slots.clone();
        slots
    }
}

/// How wide the centre is, on the centre rung and on the wide rung.
const CENTRE_W: f64 = 0.59;
const WIDE_W: f64 = 0.7;
/// How much of the height is left above the strip along the bottom.
const ABOVE_STRIP: f64 = 0.75;
/// A distant window's share of the spotlight's strip, beside an orbit window's.
const DISTANT_SHARE: f64 = 0.5;

/// Every window the same size: as many columns as the smallest square grid needs, filled row by
/// row, with a short last row's windows sharing its width.
fn grid<T: Clone>(windows: &[T]) -> Vec<(T, [f64; 4])> {
    let n = windows.len();
    if n == 0 {
        return Vec::new();
    }
    let columns = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(columns);
    windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let row = i / columns;
            let in_row = if row == rows - 1 {
                n - columns * (rows - 1)
            } else {
                columns
            };
            let column = i % columns;
            let (cw, rh) = (1.0 / in_row as f64, 1.0 / rows as f64);
            (w.clone(), [column as f64 * cw, row as f64 * rh, cw, rh])
        })
        .collect()
}

/// The centre on its rung, `slots` orbiting it and `far` in the strip along the bottom.
fn mass<T: Clone>(centre: T, shape: Shape, slots: &[T], far: &[T]) -> Vec<(T, [f64; 4])> {
    let mut placed = Vec::new();
    if shape == Shape::Spotlight {
        if slots.is_empty() && far.is_empty() {
            return vec![(centre, [0.0, 0.0, 1.0, 1.0])];
        }
        placed.push((centre, [0.0, 0.0, 1.0, ABOVE_STRIP]));
        let shares: Vec<(T, f64)> = slots
            .iter()
            .map(|w| (w.clone(), 1.0))
            .chain(far.iter().map(|w| (w.clone(), DISTANT_SHARE)))
            .collect();
        strip(&mut placed, &shares);
        return placed;
    }

    let top = if far.is_empty() { 1.0 } else { ABOVE_STRIP };
    match (shape, slots.len()) {
        (_, 0) => placed.push((centre, [0.0, 0.0, 1.0, top])),
        (Shape::Centre, 1) => {
            placed.push((centre, [0.0, 0.0, CENTRE_W, top]));
            placed.push((slots[0].clone(), [CENTRE_W, 0.0, 1.0 - CENTRE_W, top]));
        }
        (Shape::Centre, _) => {
            let side = (1.0 - CENTRE_W) / 2.0;
            placed.push((centre, [side, 0.0, CENTRE_W, top]));
            let weight = |slot: usize| 1.0 / (1.0 + slot as f64 * 0.45);
            // Even slots go in the right column, odd ones in the left.
            for (x, parity) in [(1.0 - side, 0), (0.0, 1)] {
                let column: Vec<(usize, &T)> = slots
                    .iter()
                    .enumerate()
                    .filter(|(slot, _)| slot % 2 == parity)
                    .collect();
                let total: f64 = column.iter().map(|(slot, _)| weight(*slot)).sum();
                let mut y = 0.0;
                for (slot, w) in column {
                    let h = weight(slot) / total * top;
                    placed.push((w.clone(), [x, y, side, h]));
                    y += h;
                }
            }
        }
        (_, n) => {
            placed.push((centre, [0.0, 0.0, WIDE_W, top]));
            for (i, w) in slots.iter().enumerate() {
                let h = top / n as f64;
                placed.push((w.clone(), [WIDE_W, i as f64 * h, 1.0 - WIDE_W, h]));
            }
        }
    }
    let shares: Vec<(T, f64)> = far.iter().map(|w| (w.clone(), 1.0)).collect();
    strip(&mut placed, &shares);
    placed
}

/// Windows side by side along the bottom quarter, each as wide as its share.
fn strip<T: Clone>(placed: &mut Vec<(T, [f64; 4])>, shares: &[(T, f64)]) {
    let total: f64 = shares.iter().map(|(_, share)| share).sum();
    let mut x = 0.0;
    for (w, share) in shares {
        let width = share / total;
        placed.push((w.clone(), [x, ABOVE_STRIP, width, 1.0 - ABOVE_STRIP]));
        x += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1000,
        h: 800,
    };

    fn rect_of(rects: &[(&'static str, Rect)], id: &str) -> Rect {
        rects.iter().find(|(w, _)| *w == id).unwrap().1
    }

    /// The other windows of `all`, in the order given (standing in for most recent first).
    fn others<'a>(all: &[&'a str], id: &str) -> Vec<&'a str> {
        all.iter().filter(|w| **w != id).copied().collect()
    }

    fn step(
        gravity: &mut Gravity<&'static str>,
        all: &[&'static str],
        id: &str,
        heavier: bool,
    ) -> Step {
        let id = all.iter().find(|w| **w == id).unwrap();
        gravity.step(id, heavier, &others(all, id))
    }

    #[test]
    fn tiling_is_never_a_step() {
        let all = ["a", "b", "c", "d"];
        assert!(!Rung::LADDER.contains(&Rung::Tiling));
        // Walking the whole ladder both ways never reports tiling, so gravity is never turned
        // off and on again in passing, and nothing announces it.
        let mut gravity = Gravity::default();
        let mut seen = Vec::new();
        for heavier in [false, false, false, true, true, true, true, true, true] {
            if let Step::Moved(rung) = step(&mut gravity, &all, "a", heavier) {
                seen.push(rung);
            }
        }
        assert!(
            !seen.contains(&Rung::Tiling),
            "stepped through tiling: {seen:?}"
        );
        assert!(gravity.is_on(), "the ladder never turns gravity off");
    }

    #[test]
    fn the_first_press_from_tiling_enters_the_ladder_at_the_end_it_points_at() {
        let all = ["a", "b", "c"];
        // Heavier takes the centre; lighter goes to the grid. Neither passes through the other.
        let mut heavier = Gravity::default();
        assert_eq!(
            step(&mut heavier, &all, "a", true),
            Step::Moved(Rung::Centre)
        );
        let mut lighter = Gravity::default();
        assert_eq!(
            step(&mut lighter, &all, "a", false),
            Step::Moved(Rung::Grid)
        );
    }

    #[test]
    fn heavier_climbs_every_rung_from_the_bottom_to_the_top_in_order() {
        let all = ["a", "b", "c", "d"];
        let mut gravity = Gravity::default();
        // Down to the bottom first.
        let mut seen = Vec::new();
        loop {
            match step(&mut gravity, &all, "a", false) {
                Step::Moved(rung) => seen.push(rung),
                Step::Lightest => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(seen, [Rung::Grid, Rung::Orbit, Rung::Distant]);
        assert_eq!(gravity.rung(&"a"), Rung::Distant);
        let mut climbed = vec![Rung::Distant];
        loop {
            match step(&mut gravity, &all, "a", true) {
                Step::Moved(rung) => climbed.push(rung),
                Step::Heaviest => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(climbed, Rung::LADDER, "no rung missed, none repeated");
    }

    /// Every layout the ladder can reach for one window, as the windows' rectangles.
    fn layout(
        gravity: &mut Gravity<&'static str>,
        all: &[&'static str],
    ) -> Vec<(&'static str, Rect)> {
        let mut rects = gravity.rects(all, AREA, 16, 10);
        rects.sort_by_key(|(w, _)| *w);
        rects
    }

    #[test]
    fn the_opposite_key_always_steps_straight_back() {
        let all = ["a", "b", "c", "d", "e"];
        let grid = Rung::Grid.position().unwrap();
        let centre = Rung::Centre.position().unwrap();
        for start in 0..Rung::LADDER.len() {
            // Walk "c" to each rung, then check a step each way from there comes back. Gravity
            // starts off, and the first press enters the ladder: lighter at the grid, heavier at
            // the centre.
            let mut gravity = Gravity::default();
            if start <= grid {
                for _ in start..=grid {
                    step(&mut gravity, &all, "c", false);
                }
            } else {
                for _ in centre..=start {
                    step(&mut gravity, &all, "c", true);
                }
            }
            assert_eq!(gravity.rung(&"c"), Rung::LADDER[start]);
            for heavier in [true, false] {
                let mut there = gravity.clone();
                let before = (there.rung(&"c"), layout(&mut there, &all));
                if let Step::Moved(_) = step(&mut there, &all, "c", heavier) {
                    step(&mut there, &all, "c", !heavier);
                }
                assert_eq!(
                    (there.rung(&"c"), layout(&mut there, &all)),
                    before,
                    "from {:?}, {} and back",
                    Rung::LADDER[start],
                    if heavier { "heavier" } else { "lighter" }
                );
            }
        }
    }

    #[test]
    fn every_rung_looks_different_with_three_or_more_windows() {
        for count in 3..=7 {
            let all: Vec<&'static str> = ["a", "b", "c", "d", "e", "f", "g"][..count].to_vec();
            let mut gravity = Gravity::default();
            while step(&mut gravity, &all, "a", false) != Step::Lightest {}
            let mut layouts: Vec<Vec<(&str, Rect)>> = Vec::new();
            loop {
                let mut rects = if gravity.is_on() {
                    layout(&mut gravity, &all)
                } else {
                    Vec::new()
                };
                rects.sort_by_key(|(w, _)| *w);
                assert!(
                    !layouts.contains(&rects),
                    "{count} windows: {:?} repeats a layout",
                    gravity.rung(&"a")
                );
                layouts.push(rects);
                if step(&mut gravity, &all, "a", true) == Step::Heaviest {
                    break;
                }
            }
            assert_eq!(layouts.len(), Rung::LADDER.len());
        }
    }

    #[test]
    fn heavier_rungs_give_the_window_more_of_the_screen() {
        let all = ["a", "b", "c", "d"];
        let mut gravity = Gravity::default();
        let area = |gravity: &mut Gravity<&'static str>| {
            let r = rect_of(&gravity.rects(&all, AREA, 0, 0), "a");
            r.w * r.h
        };
        let mut sizes = Vec::new();
        for _ in 0..3 {
            step(&mut gravity, &all, "a", true);
            sizes.push(area(&mut gravity));
        }
        assert!(sizes.windows(2).all(|pair| pair[0] < pair[1]), "{sizes:?}");
        // Below the centre: orbit is smaller than a grid cell, and the grid smaller than the
        // centre. (A lone distant window spans the strip's whole width, so it isn't compared.)
        step(&mut gravity, &all, "a", false);
        step(&mut gravity, &all, "a", false);
        step(&mut gravity, &all, "a", false);
        assert_eq!(gravity.rung(&"a"), Rung::Grid);
        let grid = area(&mut gravity);
        step(&mut gravity, &all, "a", false);
        let orbit = area(&mut gravity);
        assert!(orbit < grid && grid < sizes[0], "{orbit} {grid} {sizes:?}");
    }

    #[test]
    fn focus_never_moves_a_window() {
        // `rects` takes no notion of recency: the tree's order places everything.
        let all = ["a", "b", "c", "d", "e"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "c", true);
        let first = gravity.rects(&all, AREA, 16, 10);
        assert_eq!(first, gravity.rects(&all, AREA, 16, 10));
        // Orbit slots follow the tree: "a" is slot 0 (right column), "b" slot 1 (left column).
        let (a, b, c) = (
            rect_of(&first, "a"),
            rect_of(&first, "b"),
            rect_of(&first, "c"),
        );
        assert!(a.x > c.x && b.x < c.x);
    }

    #[test]
    fn the_ends_stop_and_say_so() {
        let all = ["a", "b"];
        let mut gravity = Gravity::default();
        for _ in 0..3 {
            step(&mut gravity, &all, "a", true);
        }
        assert_eq!(gravity.rung(&"a"), Rung::Spotlight);
        assert_eq!(step(&mut gravity, &all, "a", true), Step::Heaviest);
        assert_eq!(gravity.rung(&"a"), Rung::Spotlight);
        let mut gravity = Gravity::default();
        for _ in 0..3 {
            step(&mut gravity, &all, "a", false);
        }
        assert_eq!(step(&mut gravity, &all, "a", false), Step::Lightest);
        assert_eq!(
            gravity.rung(&"a"),
            Rung::Distant,
            "never into the code rain"
        );
    }

    #[test]
    fn a_lone_window_has_nothing_to_arrange() {
        let mut gravity: Gravity<&str> = Gravity::default();
        assert_eq!(gravity.step(&"a", true, &[]), Step::Alone);
        assert_eq!(gravity.step(&"a", false, &[]), Step::Alone);
        assert!(!gravity.is_on());
    }

    #[test]
    fn lighter_from_the_grid_orbits_the_window_used_before() {
        let all = ["a", "b", "c"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "b", false);
        assert_eq!(gravity.rung(&"a"), Rung::Grid);
        // "c" was used before "b".
        assert_eq!(
            gravity.step(&"b", false, &["c", "a"]),
            Step::Moved(Rung::Orbit)
        );
        assert_eq!(gravity.centre(), Some(&"c"));
        assert_eq!(gravity.rung(&"a"), Rung::Orbit);
    }

    #[test]
    fn other_windows_step_along_their_own_ladder() {
        let all = ["a", "b", "c"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "a", true);
        step(&mut gravity, &all, "a", true);
        assert_eq!(gravity.rung(&"a"), Rung::Wide);
        // "b" orbits; lighter sends it to the strip, and "a" keeps its rung.
        assert_eq!(
            step(&mut gravity, &all, "b", false),
            Step::Moved(Rung::Distant)
        );
        assert_eq!(gravity.rung(&"a"), Rung::Wide);
        let rects = gravity.rects(&all, AREA, 16, 10);
        assert!(
            rect_of(&rects, "b").y > 550,
            "the strip runs along the bottom"
        );
        // Heavier twice from orbit: the grid, then the centre. Tiling is not on the way, so
        // gravity stays on throughout.
        step(&mut gravity, &all, "b", true);
        assert_eq!(step(&mut gravity, &all, "b", true), Step::Moved(Rung::Grid));
        assert_eq!(
            step(&mut gravity, &all, "b", true),
            Step::Moved(Rung::Centre)
        );
        assert!(gravity.is_on());
    }

    #[test]
    fn the_grid_fills_rows_and_shares_a_short_last_row() {
        let rects = Gravity {
            mode: Mode::Grid,
            ..Gravity::default()
        }
        .rects(&["a", "b", "c"], AREA, 0, 0);
        let (a, b, c) = (
            rect_of(&rects, "a"),
            rect_of(&rects, "b"),
            rect_of(&rects, "c"),
        );
        assert_eq!((a.w, b.w, a.y, b.y), (500, 500, 0, 0));
        assert_eq!((c.x, c.y, c.w), (0, 400, 1000));
    }

    #[test]
    fn wide_and_spotlight_shapes() {
        let all = ["a", "b", "c"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "a", true);
        step(&mut gravity, &all, "a", true);
        let wide = gravity.rects(&all, AREA, 0, 0);
        assert_eq!(
            rect_of(&wide, "a"),
            Rect {
                x: 0,
                y: 0,
                w: 700,
                h: 800
            }
        );
        assert_eq!(
            rect_of(&wide, "b"),
            Rect {
                x: 700,
                y: 0,
                w: 300,
                h: 400
            }
        );
        step(&mut gravity, &all, "a", true);
        let spot = gravity.rects(&all, AREA, 0, 0);
        assert_eq!(
            rect_of(&spot, "a"),
            Rect {
                x: 0,
                y: 0,
                w: 1000,
                h: 600
            }
        );
        assert_eq!(
            rect_of(&spot, "c"),
            Rect {
                x: 500,
                y: 600,
                w: 500,
                h: 200
            }
        );
    }

    #[test]
    fn gaps_match_tiling_between_neighbours() {
        let all = ["a", "b"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "a", true);
        let rects = gravity.rects(&all, AREA, 16, 10);
        let (a, b) = (rect_of(&rects, "a"), rect_of(&rects, "b"));
        assert_eq!(b.x - (a.x + a.w), 10, "tiling's inner gap");
    }

    #[test]
    fn when_the_centre_leaves_the_most_recent_window_takes_its_rung() {
        let all = ["a", "b", "c"];
        let mut gravity = Gravity::default();
        step(&mut gravity, &all, "a", true);
        step(&mut gravity, &all, "a", true);
        step(&mut gravity, &all, "b", false);
        assert_eq!(gravity.remove(&"a", &["b", "c"]), Some("b"));
        assert_eq!(
            gravity.rung(&"b"),
            Rung::Wide,
            "taken out of the strip, onto the centre's rung"
        );
        assert_eq!(gravity.remove(&"c", &["b"]), None);
        assert_eq!(gravity.remove(&"b", &[]), None);
        assert!(!gravity.is_on());
    }

    #[test]
    fn a_recorded_gravity_comes_back() {
        let mut gravity =
            Gravity::restore(Some(("a", Rung::Spotlight)), vec!["c"], vec!["b"], false);
        assert_eq!(gravity.rung(&"a"), Rung::Spotlight);
        assert_eq!(gravity.rung(&"b"), Rung::Orbit);
        assert_eq!(gravity.rung(&"c"), Rung::Distant);
        assert!(gravity.is_pinned(&"b"));
        let rects = gravity.rects(&["a", "b", "c"], AREA, 16, 10);
        assert!(
            rect_of(&rects, "c").w < rect_of(&rects, "b").w,
            "distant takes less of the strip"
        );
        let grid: Gravity<&str> = Gravity::restore(None, vec![], vec![], true);
        assert_eq!(grid.rung(&"a"), Rung::Grid);
        let off: Gravity<&str> = Gravity::restore(None, vec!["a"], vec![], false);
        assert!(!off.is_on());
        let orbit_centre: Gravity<&str> =
            Gravity::restore(Some(("a", Rung::Orbit)), vec![], vec![], false);
        assert!(
            !orbit_centre.is_on(),
            "a centre recorded on no centre rung is ignored"
        );
    }

    #[test]
    fn labels_read_back() {
        for rung in Rung::LADDER {
            assert_eq!(Rung::from_label(rung.label()), Some(rung));
        }
        assert_eq!(Rung::from_label("heavy"), None);
    }

    #[test]
    fn a_pinned_window_keeps_its_slot_as_others_open() {
        let mut gravity = Gravity::default();
        gravity.step(&"z", true, &["b", "c"]);
        let first = gravity.rects(&["z", "b", "c"], AREA, 16, 10);
        gravity.toggle_pin(&"c");
        // "a" opens, earlier in the tree; unpinned, "c" would move down a slot.
        let second = gravity.rects(&["a", "z", "b", "c"], AREA, 16, 10);
        assert_eq!(rect_of(&first, "c").x, rect_of(&second, "c").x);
    }
}

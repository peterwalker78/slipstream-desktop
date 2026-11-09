//! A tile dragged to swap places: it lifts and follows the pointer, the other tiles show where a
//! drop would put them, and a frame marks where the dragged one would land.
//!
//! All of it is paint. The layout changes only when the tile is let go, by the same swap the
//! preview was worked out from, and the tiles then settle from wherever they are drawn.

use crate::anim::{Easing, Tween};
use crate::layout::Rect;
use crate::motion::Frame;
use crate::workspace::Workspace;

/// How far, in logical pixels, the pointer moves from the press before the tile lifts, so a
/// click that doesn't move leaves it where it is.
pub const LIFT_AFTER: f64 = 6.0;
/// How far a lifted tile grows about its centre.
pub const LIFT_SCALE: f64 = 1.025;
/// How long it takes to lift.
const LIFT: f64 = 0.2;
/// How long the other tiles take to move into a previewed layout, or back to their own places.
const MAKE_WAY: f64 = 0.4;
/// How long the landing frame takes to move from one destination to the next.
const LANDING_MOVE: f64 = 0.22;
/// How long tiles take to settle into place once the dragged one is let go.
pub const SETTLE: f64 = 0.32;
/// How strongly the landing frame's edge shows, and how strongly it tints its inside.
pub const LANDING_EDGE: f32 = 0.75;
pub const LANDING_FILL: f32 = 0.08;

/// The tiles on `ws` that a drop swapping `dragged` with `over` would move, and where to: the
/// same `Workspace::swap` and the same `rects_within` the drop and the retile after it run, on a
/// copy of the workspace, so what the preview shows is what the drop does. Empty when the swap
/// would change nothing.
pub fn moved_by_swap<T: Clone + PartialEq>(
    ws: &Workspace<T>,
    dragged: &T,
    over: &T,
    area: Rect,
    min: &dyn Fn(&T) -> (i32, i32),
) -> Vec<(T, Rect)> {
    let mut after = ws.clone();
    if !after.swap(dragged, over) {
        return Vec::new();
    }
    let before = ws.clone().rects_within(area, min);
    after
        .rects_within(area, min)
        .into_iter()
        .filter(|place| !before.contains(place))
        .collect()
}

/// A tile being dragged.
#[derive(Debug)]
pub struct Lift<T> {
    pub window: T,
    /// Where the pointer took hold, and where it is now.
    grabbed: (f64, f64),
    pointer: (f64, f64),
    /// From 0 on the ground to 1 lifted.
    raise: Tween<1>,
    /// Where it would land, x, y, w and h.
    landing: Tween<4>,
    /// The other tiles drawn somewhere other than their own places, on their way to the layout
    /// a drop would give or back from it.
    making_way: Vec<(T, Tween<4>)>,
    /// Let go: the tiles settle by the usual window motion now, and this only keeps the shadow
    /// while the tile comes back down.
    released: bool,
    reduced: bool,
}

impl<T: Clone + PartialEq> Lift<T> {
    /// `window` taken hold of with the pointer at `pointer`, its tile at `home`.
    pub fn new(window: T, pointer: (f64, f64), home: [f64; 4], now: f64, reduced: bool) -> Self {
        let mut raise = Tween::at_rest([0.0]);
        raise.retarget([1.0], now, if reduced { 0.0 } else { LIFT }, Easing::Glide);
        Self {
            window,
            grabbed: pointer,
            pointer,
            raise,
            landing: Tween::at_rest(home),
            making_way: Vec::new(),
            released: false,
            reduced,
        }
    }

    fn duration(&self, duration: f64) -> f64 {
        if self.reduced { 0.0 } else { duration }
    }

    pub fn released(&self) -> bool {
        self.released
    }

    /// The pointer has moved to `pointer`.
    pub fn follow(&mut self, pointer: (f64, f64)) {
        self.pointer = pointer;
    }

    fn offset(&self) -> (f64, f64) {
        (
            self.pointer.0 - self.grabbed.0,
            self.pointer.1 - self.grabbed.1,
        )
    }

    /// Sends every tile on the workspace towards `places`, each window's place were it let go
    /// now, the dragged one's being where the landing frame goes. `shown` is where the usual
    /// motion draws a window and where it is headed, for the tiles that start making way.
    pub fn aim(
        &mut self,
        places: &[(T, [f64; 4])],
        shown: impl Fn(&T) -> Option<([f64; 4], [f64; 4])>,
        now: f64,
    ) {
        let (landing, making_way) = (self.duration(LANDING_MOVE), self.duration(MAKE_WAY));
        for (id, to) in places {
            if *id == self.window {
                self.landing.retarget(*to, now, landing, Easing::Glide);
                continue;
            }
            match self.making_way.iter_mut().find(|(other, _)| other == id) {
                Some((_, tween)) => tween.retarget(*to, now, making_way, Easing::Glide),
                None => {
                    if let Some((drawn, target)) = shown(id)
                        && target != *to
                    {
                        let mut tween = Tween::at_rest(drawn);
                        tween.retarget(*to, now, making_way, Easing::Glide);
                        self.making_way.push((id.clone(), tween));
                    }
                }
            }
        }
        self.making_way
            .retain(|(id, _)| places.iter().any(|(placed, _)| placed == id));
    }

    /// How far `id` is carried from its place: the dragged tile goes where the pointer takes
    /// it. Added after the tile is fitted to its place, so the edge of the screen doesn't squeeze
    /// it on the way.
    pub fn carried(&self, id: &T) -> (f64, f64) {
        if self.released || *id != self.window {
            (0.0, 0.0)
        } else {
            self.offset()
        }
    }

    /// How `id` is drawn this frame, given how the usual motion would draw it, before it is
    /// `carried`.
    pub fn frame(&self, id: &T, frame: Frame, now: f64) -> Frame {
        if self.released {
            return frame;
        }
        if *id == self.window {
            return Frame {
                scale: frame.scale * self.scale(now),
                ..frame
            };
        }
        match self.making_way.iter().find(|(other, _)| other == id) {
            // Back in its own place and still: drawn as usual again.
            Some((_, tween)) if tween.done(now) && tween.to == frame.rect => frame,
            // Stretched to the place it is shown in, as any moving tile is: the client keeps its
            // own size until the drop.
            Some((_, tween)) => Frame {
                rect: tween.value(now),
                moving: true,
                ..frame
            },
            None => frame,
        }
    }

    fn scale(&self, now: f64) -> f64 {
        if self.reduced {
            1.0
        } else {
            1.0 + (LIFT_SCALE - 1.0) * self.raise.value(now)[0]
        }
    }

    /// How strongly `id`'s shadow shows, drawn at `scale`: fully once lifted, and fading as it
    /// comes back down after the drop.
    pub fn shadow(&self, id: &T, scale: f64, now: f64) -> f64 {
        if *id != self.window {
            0.0
        } else if self.released {
            ((scale - 1.0) / (LIFT_SCALE - 1.0)).clamp(0.0, 1.0)
        } else {
            self.raise.value(now)[0].clamp(0.0, 1.0)
        }
    }

    /// The landing frame, while the tile is held.
    pub fn landing(&self, now: f64) -> Option<[f64; 4]> {
        (!self.released).then(|| self.landing.value(now))
    }

    /// The tile is let go, `drawn` being where the usual motion draws a window now. Returns where
    /// each tile the lift has moved is drawn, for the usual motion to settle it from, and the
    /// dragged tile's scale.
    pub fn release(
        &mut self,
        drawn: impl Fn(&T) -> Option<[f64; 4]>,
        now: f64,
    ) -> (Vec<(T, [f64; 4])>, f64) {
        let mut from: Vec<(T, [f64; 4])> = self
            .making_way
            .drain(..)
            .map(|(id, tween)| (id, tween.value(now)))
            .collect();
        if let Some(rect) = drawn(&self.window) {
            let (dx, dy) = self.offset();
            from.push((
                self.window.clone(),
                [rect[0] + dx, rect[1] + dy, rect[2], rect[3]],
            ));
        }
        let scale = self.scale(now);
        self.released = true;
        (from, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspaces;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1200,
        h: 800,
    };

    fn none(_: &u32) -> (i32, i32) {
        (0, 0)
    }

    /// A workspace with `count` tiles, numbered from 1.
    fn workspace(count: u32) -> Workspaces<u32> {
        let mut spaces = Workspaces::new(&[(1, String::new())], 8, 8);
        for id in 1..=count {
            let beside = (id > 1).then_some(id - 1);
            spaces.insert(0, id, beside.as_ref(), AREA);
        }
        spaces
    }

    /// The layout the preview shows, laid over the one before, is the layout the drop gives.
    fn assert_preview_is_the_drop(spaces: &mut Workspaces<u32>, a: u32, b: u32) {
        let ws = spaces.get_mut(0);
        let before = ws.rects_within(AREA, &none);
        let moved = moved_by_swap(ws, &a, &b, AREA, &none);
        assert!(!moved.is_empty(), "swapping {a} and {b} moves something");
        let previewed: Vec<(u32, Rect)> = before
            .iter()
            .map(|(id, rect)| {
                let to = moved.iter().find(|(m, _)| m == id).map_or(*rect, |m| m.1);
                (*id, to)
            })
            .collect();
        assert!(ws.swap(&a, &b));
        let mut dropped = ws.rects_within(AREA, &none);
        let mut previewed = previewed;
        dropped.sort_by_key(|(id, _)| *id);
        previewed.sort_by_key(|(id, _)| *id);
        assert_eq!(previewed, dropped);
    }

    #[test]
    fn the_preview_shows_the_layout_the_drop_gives() {
        for (a, b) in [(1, 2), (1, 3), (3, 1), (2, 4)] {
            assert_preview_is_the_drop(&mut workspace(4), a, b);
        }
    }

    #[test]
    fn the_preview_shows_the_layout_the_drop_gives_under_gravity() {
        let mut spaces = workspace(4);
        spaces.get_mut(0).gravity.step(&1, true, &[2, 3, 4]);
        assert!(spaces.get(0).gravity.is_on());
        assert_preview_is_the_drop(&mut spaces, 1, 3);
    }

    #[test]
    fn a_swap_that_changes_nothing_previews_nothing() {
        let spaces = workspace(2);
        assert!(moved_by_swap(spaces.get(0), &1, &1, AREA, &none).is_empty());
        assert!(moved_by_swap(spaces.get(0), &1, &9, AREA, &none).is_empty());
    }

    fn still(rect: [f64; 4]) -> Frame {
        Frame::still(rect)
    }

    const HOME_A: [f64; 4] = [0.0, 0.0, 600.0, 800.0];
    const HOME_B: [f64; 4] = [600.0, 0.0, 600.0, 800.0];

    fn homes(id: &u32) -> Option<([f64; 4], [f64; 4])> {
        match id {
            1 => Some((HOME_A, HOME_A)),
            2 => Some((HOME_B, HOME_B)),
            _ => None,
        }
    }

    #[test]
    fn the_dragged_tile_follows_the_pointer_and_lifts() {
        let lift = {
            let mut lift = Lift::new(1u32, (100.0, 100.0), HOME_A, 0.0, false);
            lift.follow((150.0, 80.0));
            lift
        };
        let frame = lift.frame(&1, still(HOME_A), 1.0);
        assert_eq!(lift.carried(&1), (50.0, -20.0));
        assert_eq!(lift.carried(&2), (0.0, 0.0));
        assert!((frame.scale - LIFT_SCALE).abs() < 1e-9);
        assert_eq!(lift.shadow(&1, frame.scale, 1.0), 1.0);
        assert_eq!(lift.frame(&2, still(HOME_B), 1.0), still(HOME_B));
    }

    #[test]
    fn the_others_make_way_and_come_back() {
        let mut lift = Lift::new(1u32, (100.0, 100.0), HOME_A, 0.0, false);
        lift.aim(&[(1, HOME_B), (2, HOME_A)], homes, 0.0);
        let midway = lift.frame(&2, still(HOME_B), 0.2);
        assert!(midway.moving, "stretched while it is out of its place");
        assert!(midway.rect[0] < HOME_B[0] && midway.rect[0] > HOME_A[0]);
        assert_eq!(lift.frame(&2, still(HOME_B), 1.0).rect, HOME_A);
        assert_eq!(lift.landing(1.0), Some(HOME_B));
        // Back over its own place: the other goes home and is drawn as usual once there.
        lift.aim(&[(1, HOME_A), (2, HOME_B)], homes, 1.0);
        assert_eq!(lift.frame(&2, still(HOME_B), 2.0), still(HOME_B));
        assert_eq!(lift.landing(2.0), Some(HOME_A));
    }

    #[test]
    fn letting_go_settles_from_where_tiles_are_drawn() {
        let mut lift = Lift::new(1u32, (100.0, 100.0), HOME_A, 0.0, false);
        lift.follow((700.0, 100.0));
        lift.aim(&[(1, HOME_B), (2, HOME_A)], homes, 0.0);
        let (from, scale) = lift.release(|id| homes(id).map(|(drawn, _)| drawn), 0.2);
        let other = from.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert!(
            other[0] < HOME_B[0],
            "the other starts from where it was drawn"
        );
        let dragged = from.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert_eq!(dragged, [600.0, 0.0, 600.0, 800.0]);
        assert!((scale - LIFT_SCALE).abs() < 1e-9);
        assert!(lift.landing(0.2).is_none());
        // From here the usual motion draws it.
        assert_eq!(lift.frame(&1, still(HOME_B), 0.2), still(HOME_B));
        assert_eq!(lift.carried(&1), (0.0, 0.0));
    }

    #[test]
    fn reduced_motion_jumps_but_still_follows_and_shows_the_landing() {
        let mut lift = Lift::new(1u32, (100.0, 100.0), HOME_A, 0.0, true);
        lift.follow((700.0, 100.0));
        let frame = lift.frame(&1, still(HOME_A), 0.0);
        assert_eq!(frame.scale, 1.0, "no lift");
        assert_eq!(lift.carried(&1), (600.0, 0.0), "follows the pointer");
        lift.aim(&[(1, HOME_B), (2, HOME_A)], homes, 0.0);
        assert_eq!(lift.frame(&2, still(HOME_B), 0.0).rect, HOME_A, "jumps");
        assert_eq!(lift.landing(0.0), Some(HOME_B));
    }
}

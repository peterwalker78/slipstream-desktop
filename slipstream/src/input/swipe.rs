//! Three-finger swipes on a touchpad: left and right step between workspaces (in bullet time,
//! the workspace being looked at), up opens bullet time, and down (or up again) leaves it.
//! Swipes with any other number of fingers go to the window under the pointer, as they always
//! have.
//!
//! On the desktop a sideways swipe carries the view with the fingers: it can be held part way,
//! slowed, or brought back, and where it goes is settled when the fingers lift. Past halfway, or
//! flicked on, it goes on to the next workspace; otherwise it slides back. Up and down, and
//! sideways in bullet time, act once, as soon as the swipe has gone far enough one way.

use std::collections::VecDeque;

use crate::Slipstream;

/// The fingers a swipe needs to be ours.
pub const FINGERS: u32 = 3;
/// How far a swipe travels, in libinput's pointer units, before it acts.
const DISTANCE: f64 = 60.0;
/// How far a sideways swipe travels before the view starts to follow it.
const FOLLOW_AFTER: f64 = 10.0;
/// How much further it has to have gone along its direction than across it.
const DOMINANCE: f64 = 1.5;
/// How far the fingers travel to carry the view one whole workspace across.
const WORKSPACE_TRAVEL: f64 = 400.0;
/// Where the view would coast to after the fingers lift: this many seconds on at the speed they
/// were going, so a flick goes on even if it hasn't reached halfway.
const COAST: f64 = 0.2;
/// The speed at the lift is read over this many seconds before it.
const SPEED_OVER: f64 = 0.08;
/// Past the first or last workspace the view gives a little, and never more than this far.
const GIVE: f64 = 0.12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// What a swipe's latest movement asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Moved {
    /// Nothing yet, or nothing more.
    Nothing,
    /// Act once, in this direction.
    Once(Direction),
    /// Carry the view this far towards the next workspace, in workspaces: below zero, towards
    /// the previous one.
    Follow(f64),
}

/// A three-finger swipe under way.
#[derive(Debug, Default)]
pub struct Swipe {
    travelled: (f64, f64),
    /// Whether a sideways swipe may carry the view.
    follows: bool,
    done: bool,
    /// While following: where the view is meant to go, and recent progress with its time in
    /// seconds, for the speed at the lift.
    pub drag: Option<Drag>,
    recent: VecDeque<(f64, f64)>,
}

/// The view a swipe is carrying.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub screen: String,
    /// The workspace it started on, how far off it the view already was, and the workspaces
    /// either side it can reach.
    pub from: usize,
    pub offset: f64,
    pub back: Option<usize>,
    pub ahead: Option<usize>,
}

impl Drag {
    /// Where the view is, in workspaces, with the swipe `progress` workspaces on. Towards a
    /// workspace past the one beside it, a full swipe still reaches it; with none there the
    /// view gives a little, and less the further it's pulled.
    pub fn position(&self, progress: f64) -> f64 {
        let at = self.offset + progress;
        let reach = if at >= 0.0 { self.ahead } else { self.back };
        match reach {
            Some(to) => self.from as f64 + at.abs() * (to as f64 - self.from as f64),
            None => self.from as f64 + at.signum() * GIVE * (1.0 - (-at.abs() / GIVE).exp()),
        }
    }

    /// Where the view settles when the fingers lift `progress` workspaces on, going `speed`
    /// workspaces a second.
    pub fn settle(&self, progress: f64, speed: f64) -> usize {
        let coasted = self.offset + progress + speed * COAST;
        match (self.ahead, self.back) {
            (Some(ahead), _) if coasted >= 0.5 => ahead,
            (_, Some(back)) if coasted <= -0.5 => back,
            _ => self.from,
        }
    }
}

impl Swipe {
    /// A swipe starting now. `follows` says whether a sideways one carries the view.
    pub fn new(follows: bool) -> Self {
        Self {
            follows,
            ..Self::default()
        }
    }

    /// The fingers moved by (`dx`, `dy`) at `time` seconds.
    pub fn moved(&mut self, dx: f64, dy: f64, time: f64) -> Moved {
        self.travelled.0 += dx;
        self.travelled.1 += dy;
        let (x, y) = self.travelled;
        if self.done {
            return Moved::Nothing;
        }
        let sideways = x.abs() >= y.abs() * DOMINANCE;
        if self.follows && (!self.recent.is_empty() || sideways && x.abs() >= FOLLOW_AFTER) {
            // Fingers moving left bring in the workspace to the right, as they'd move a page.
            let progress = -x / WORKSPACE_TRAVEL;
            self.recent.push_back((time, progress));
            while self
                .recent
                .front()
                .is_some_and(|&(then, _)| time - then > SPEED_OVER)
            {
                self.recent.pop_front();
            }
            return Moved::Follow(progress);
        }
        let direction = if x.abs() >= DISTANCE && sideways {
            if x < 0.0 {
                Direction::Left
            } else {
                Direction::Right
            }
        } else if y.abs() >= DISTANCE && y.abs() >= x.abs() * DOMINANCE {
            if y < 0.0 {
                Direction::Up
            } else {
                Direction::Down
            }
        } else {
            return Moved::Nothing;
        };
        self.done = true;
        Moved::Once(direction)
    }

    /// How far on a followed swipe is, in workspaces, and how fast it was going when the
    /// fingers lifted at `time`: fingers held still before lifting have no speed.
    pub fn lifted(&self, time: f64) -> Option<(f64, f64)> {
        let &(last_time, progress) = self.recent.back()?;
        let &(first_time, first) = self.recent.front()?;
        let speed = if time - last_time <= SPEED_OVER && last_time > first_time {
            (progress - first) / (last_time - first_time)
        } else {
            0.0
        };
        Some((progress, speed))
    }
}

impl Slipstream {
    /// Whether a swipe starting now with `fingers` is the desktop's rather than the app's. The
    /// lock screen and an app holding the shortcuts keep them, as they keep the keys.
    pub(super) fn takes_swipe(&self, fingers: u32) -> bool {
        fingers == FINGERS && self.lock.is_none() && !self.shortcuts_inhibited()
    }

    /// A three-finger swipe begins: sideways on the desktop it will carry the view.
    pub fn swipe_began(&mut self) {
        self.swipe = Some(Swipe::new(self.bullet.is_none()));
    }

    /// A three-finger swipe moved by (`dx`, `dy`) at `time` seconds.
    pub fn swipe_moved(&mut self, dx: f64, dy: f64, time: f64) {
        let Some(swipe) = self.swipe.as_mut() else {
            return;
        };
        match swipe.moved(dx, dy, time) {
            Moved::Nothing => {}
            Moved::Once(direction) => self.swiped(direction),
            Moved::Follow(progress) => {
                if self.swipe.as_ref().is_some_and(|s| s.drag.is_none()) {
                    let drag = self.carried_view();
                    if let Some(swipe) = self.swipe.as_mut() {
                        swipe.drag = Some(drag);
                    }
                }
                let Some(drag) = self.swipe.as_ref().and_then(|s| s.drag.as_ref()) else {
                    return;
                };
                let (screen, at) = (drag.screen.clone(), drag.position(progress));
                self.motion.hold_camera(&screen, at);
            }
        }
    }

    /// The fingers lifted at `time` seconds, or the swipe was `cancelled`: a view being carried
    /// goes on to the next workspace or back to where it was.
    pub fn swipe_ended(&mut self, time: f64, cancelled: bool) {
        let Some(swipe) = self.swipe.take() else {
            return;
        };
        let (Some(drag), Some((progress, speed))) = (swipe.drag.as_ref(), swipe.lifted(time))
        else {
            return;
        };
        let to = if cancelled {
            drag.from
        } else {
            drag.settle(progress, speed)
        };
        tracing::info!(
            workspace = to + 1,
            progress,
            speed,
            "three-finger swipe let go"
        );
        let wall = self.wall();
        // Something else may have moved this screen on, or the keyboard to another screen, while
        // the fingers were down: then the view goes back to what the screen shows now.
        let showing = self
            .screens
            .iter()
            .find(|screen| screen.output.name() == drag.screen)
            .map(|screen| screen.workspace);
        let still = self.focused_screen_name() == drag.screen;
        match showing {
            Some(shown) if still && shown == drag.from && to != drag.from => {
                self.switch_workspace(to)
            }
            Some(shown) => self.motion.slide_to(&drag.screen, shown, wall),
            None => {}
        }
    }

    /// What a sideways swipe starting now carries: the focused screen's view, from where it is,
    /// between the workspaces either side that Super+Ctrl+←/→ would go to.
    fn carried_view(&mut self) -> Drag {
        let screen = self.focused_screen_name();
        let from = self.active_workspace();
        let wall = self.wall();
        Drag {
            offset: self.motion.camera(&screen, wall) - from as f64,
            back: self.workspace_beside(-1),
            ahead: self.workspace_beside(1),
            screen,
            from,
        }
    }

    /// A three-finger swipe went far enough. Fingers move the desktop as they'd move a page:
    /// swiping left brings in the workspace to the right. In bullet time that's the workspace
    /// being looked at, with its most recent window chosen, as its number key does.
    pub fn swiped(&mut self, direction: Direction) {
        tracing::info!(?direction, "three-finger swipe");
        let step: i32 = match direction {
            Direction::Up => return self.toggle_bullet_time(),
            Direction::Down if self.bullet.is_some() => return self.toggle_bullet_time(),
            Direction::Down => return,
            Direction::Left => 1,
            Direction::Right => -1,
        };
        match self.bullet.as_ref().map(|mode| mode.view) {
            Some(view) => {
                let to = view as i32 + step;
                if (0..self.workspaces.count() as i32).contains(&to) {
                    self.bullet_pan(to as usize, true);
                }
            }
            None => self.switch_workspace_by(step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 60.0;

    fn drag(back: Option<usize>, ahead: Option<usize>) -> Drag {
        Drag {
            screen: "eDP-1".into(),
            from: 1,
            offset: 0.0,
            back,
            ahead,
        }
    }

    #[test]
    fn a_swipe_acts_once_it_has_gone_far_enough() {
        let mut swipe = Swipe::new(false);
        assert_eq!(swipe.moved(-30.0, 2.0, 0.0), Moved::Nothing);
        assert_eq!(swipe.moved(-35.0, 3.0, 0.0), Moved::Once(Direction::Left));
        assert_eq!(
            swipe.moved(-200.0, 0.0, 0.0),
            Moved::Nothing,
            "only once a swipe"
        );
    }

    #[test]
    fn a_diagonal_swipe_is_nothing() {
        let mut swipe = Swipe::new(true);
        assert_eq!(swipe.moved(70.0, -70.0, 0.0), Moved::Nothing);
        assert_eq!(swipe.moved(0.0, -60.0, 0.0), Moved::Once(Direction::Up));
    }

    #[test]
    fn each_direction_is_read() {
        for (dx, dy, direction) in [
            (80.0, 0.0, Direction::Right),
            (-80.0, 0.0, Direction::Left),
            (0.0, -80.0, Direction::Up),
            (0.0, 80.0, Direction::Down),
        ] {
            assert_eq!(Swipe::new(false).moved(dx, dy, 0.0), Moved::Once(direction));
        }
        assert_eq!(
            Swipe::new(true).moved(0.0, -80.0, 0.0),
            Moved::Once(Direction::Up),
            "up still acts once on the desktop"
        );
    }

    #[test]
    fn a_sideways_swipe_on_the_desktop_follows_the_fingers_both_ways() {
        let mut swipe = Swipe::new(true);
        assert_eq!(swipe.moved(-5.0, 0.0, 0.0), Moved::Nothing);
        assert_eq!(swipe.moved(-15.0, 0.0, FRAME), Moved::Follow(0.05));
        assert_eq!(swipe.moved(-180.0, 0.0, 2.0 * FRAME), Moved::Follow(0.5));
        assert_eq!(
            swipe.moved(0.0, 90.0, 3.0 * FRAME),
            Moved::Follow(0.5),
            "once following, it stays a sideways swipe"
        );
        assert_eq!(swipe.moved(400.0, 0.0, 4.0 * FRAME), Moved::Follow(-0.5));
    }

    #[test]
    fn the_view_moves_with_the_swipe_and_gives_only_a_little_past_the_ends() {
        let middle = drag(Some(0), Some(2));
        assert_eq!(middle.position(0.0), 1.0);
        assert_eq!(middle.position(0.25), 1.25);
        assert_eq!(middle.position(-0.25), 0.75);
        let past = drag(Some(0), Some(3));
        assert_eq!(
            past.position(0.5),
            2.0,
            "half a swipe, halfway to the one past"
        );
        let last = drag(Some(0), None);
        assert!(last.position(0.1) > 1.0);
        assert!(
            last.position(5.0) <= 1.0 + GIVE,
            "the end gives, but only so far"
        );
        assert!(
            last.position(5.0) > last.position(0.3),
            "and more the harder it's pulled"
        );
    }

    #[test]
    fn letting_go_past_halfway_or_with_a_flick_goes_on_and_otherwise_back() {
        let middle = drag(Some(0), Some(2));
        assert_eq!(middle.settle(0.6, 0.0), 2, "past halfway");
        assert_eq!(middle.settle(0.4, 0.0), 1, "held short of halfway");
        assert_eq!(middle.settle(0.2, 2.0), 2, "flicked on");
        assert_eq!(middle.settle(0.6, -2.0), 1, "sent back");
        assert_eq!(middle.settle(-0.7, 0.0), 0, "the other way");
        assert_eq!(drag(Some(0), None).settle(2.0, 5.0), 1, "nowhere to go");
    }

    #[test]
    fn fingers_held_still_before_lifting_have_no_speed() {
        let mut swipe = Swipe::new(true);
        let mut time = 0.0;
        for _ in 0..10 {
            time += FRAME;
            swipe.moved(-24.0, 0.0, time);
        }
        let (progress, speed) = swipe.lifted(time).unwrap();
        assert!((progress - 0.6).abs() < 1e-9);
        assert!((speed - 3.6).abs() < 0.01, "moving: {speed}");
        let (_, held) = swipe.lifted(time + 0.5).unwrap();
        assert_eq!(held, 0.0);
        assert_eq!(Swipe::new(true).lifted(0.0), None, "never followed");
    }
}

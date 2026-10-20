//! Three-finger swipes on a touchpad: left and right step between workspaces (in bullet time,
//! the workspace being looked at), up opens bullet time, and down leaves it. Swipes with any other
//! number of fingers go to the window under the pointer, as they always have.
//!
//! A swipe carries what it moves with the fingers: the view sideways, or bullet time's zoom up
//! and down. It can be held part way, slowed, or brought back, and where it goes is settled when
//! the fingers lift. Past halfway, or flicked on, it goes on; otherwise it goes back to where it
//! was. Nothing changes but the picture until then: bullet time's letters and slow motion come
//! once it has opened, and focus moves once a workspace has been reached.

use std::collections::VecDeque;

use crate::{Slipstream, anim::Tween, motion};

/// The fingers a swipe needs to be ours.
pub const FINGERS: u32 = 3;
/// How far a swipe travels, in libinput's pointer units, before it starts to carry anything.
const FOLLOW_AFTER: f64 = 10.0;
/// How much further it has to have gone along its direction than across it.
const DOMINANCE: f64 = 1.5;
/// How far the fingers travel to carry the view one whole workspace across, and bullet time all
/// the way open or closed.
const WORKSPACE_TRAVEL: f64 = 400.0;
const ZOOM_TRAVEL: f64 = 300.0;
/// Where a swipe would coast to after the fingers lift: this many seconds on at the speed they
/// were going, so a flick goes on even if it hasn't reached halfway.
const COAST: f64 = 0.2;
/// The speed at the lift is read over this many seconds before it.
const SPEED_OVER: f64 = 0.08;
/// Past the first or last workspace the view gives a little, and never more than this far.
const GIVE: f64 = 0.12;

/// The way a swipe goes. Fingers move the desktop as they'd move a page: swiping left brings in
/// the workspace to the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Which way a swipe has been taken to be going, once it has gone far enough to tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Sideways,
    Upright,
}

/// A three-finger swipe under way.
#[derive(Debug, Default)]
pub struct Swipe {
    travelled: (f64, f64),
    axis: Option<Axis>,
    /// What it carries, once it carries anything.
    pub carry: Option<Carry>,
    /// Recent progress, with its time in seconds, for the speed at the lift.
    recent: VecDeque<(f64, f64)>,
}

/// What a swipe carries.
#[derive(Debug, Clone, PartialEq)]
pub enum Carry {
    /// A screen's view, between workspaces.
    View(Drag),
    /// Bullet time's zoom, from where it was when the swipe took hold: 0 in, 1 all the way out.
    Zoom(f64),
}

/// The view a sideways swipe is carrying.
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

/// Bullet time's zoom with the swipe `progress` on from `from`: up is out.
fn zoom_at(from: f64, progress: f64) -> f64 {
    (from + progress).clamp(0.0, 1.0)
}

/// Whether bullet time ends up open when the fingers lift `progress` on from `from`, going
/// `speed` a second.
fn zoom_settles_open(from: f64, progress: f64, speed: f64) -> bool {
    from + progress + speed * COAST >= 0.5
}

impl Swipe {
    /// The fingers moved by (`dx`, `dy`) at `time` seconds: which way the swipe goes, once it
    /// has gone far enough to tell, and how far along that way it is, in workspaces sideways or
    /// in bullet time's zoom upright.
    pub fn moved(&mut self, dx: f64, dy: f64, time: f64) -> Option<(Axis, f64)> {
        self.travelled.0 += dx;
        self.travelled.1 += dy;
        let (x, y) = self.travelled;
        if self.axis.is_none() {
            self.axis = if x.abs() >= FOLLOW_AFTER && x.abs() >= y.abs() * DOMINANCE {
                Some(Axis::Sideways)
            } else if y.abs() >= FOLLOW_AFTER && y.abs() >= x.abs() * DOMINANCE {
                Some(Axis::Upright)
            } else {
                None
            };
        }
        let axis = self.axis?;
        let progress = match axis {
            Axis::Sideways => -x / WORKSPACE_TRAVEL,
            Axis::Upright => -y / ZOOM_TRAVEL,
        };
        self.recent.push_back((time, progress));
        while self
            .recent
            .front()
            .is_some_and(|&(then, _)| time - then > SPEED_OVER)
        {
            self.recent.pop_front();
        }
        Some((axis, progress))
    }

    /// How far on the swipe is, and how fast it was going when the fingers lifted at `time`:
    /// fingers held still before lifting have no speed.
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

    pub fn swipe_began(&mut self) {
        self.swipe = Some(Swipe::default());
    }

    /// A three-finger swipe moved by (`dx`, `dy`) at `time` seconds.
    pub fn swipe_moved(&mut self, dx: f64, dy: f64, time: f64) {
        let Some((axis, progress)) = self.swipe.as_mut().and_then(|s| s.moved(dx, dy, time)) else {
            return;
        };
        if self.swipe.as_ref().is_some_and(|s| s.carry.is_none()) {
            let carry = self.take_hold(axis);
            if let Some(swipe) = self.swipe.as_mut() {
                swipe.carry = Some(carry);
            }
        }
        match self.swipe.as_ref().and_then(|s| s.carry.clone()) {
            Some(Carry::View(drag)) => {
                let wall = self.wall();
                self.motion
                    .hold_camera(&drag.screen, drag.position(progress), wall)
            }
            Some(Carry::Zoom(from)) => self.overview = Tween::at_rest([zoom_at(from, progress)]),
            None => {}
        }
    }

    /// The fingers lifted at `time` seconds, or the swipe was `cancelled`: what it carried goes
    /// on, or back to where it was.
    pub fn swipe_ended(&mut self, time: f64, cancelled: bool) {
        let Some(swipe) = self.swipe.take() else {
            return;
        };
        let (Some(carry), Some((progress, speed))) = (swipe.carry.clone(), swipe.lifted(time))
        else {
            return;
        };
        let (progress, speed) = if cancelled {
            (0.0, 0.0)
        } else {
            (progress, speed)
        };
        match carry {
            Carry::View(drag) => self.let_go_of_view(&drag, drag.settle(progress, speed), speed),
            Carry::Zoom(from) => {
                self.let_go_of_zoom(zoom_settles_open(from, progress, speed), speed)
            }
        }
    }

    /// What a swipe that has just shown which way it goes takes hold of. Sideways, the focused
    /// screen's view, from where it is: on the desktop between the workspaces either side that
    /// Super+Ctrl+←/→ would go to, in bullet time between those either side of the one being
    /// looked at. Upright, bullet time's zoom.
    fn take_hold(&mut self, axis: Axis) -> Carry {
        let wall = self.wall();
        if axis == Axis::Upright {
            if self.bullet.is_none() {
                // Opening, everything is placed and every panel closed, as bullet time does.
                self.close_panels();
                for index in 0..self.workspaces.count() {
                    self.place_workspace(index);
                }
            }
            return Carry::Zoom(self.overview.value(wall)[0]);
        }
        let screen = self.focused_screen_name();
        let (from, back, ahead) = match self.bullet.as_ref().map(|mode| mode.view) {
            Some(view) => {
                let count = self.workspaces.count();
                (
                    view,
                    view.checked_sub(1),
                    Some(view + 1).filter(|&i| i < count),
                )
            }
            None => (
                self.active_workspace(),
                self.workspace_beside(-1),
                self.workspace_beside(1),
            ),
        };
        Carry::View(Drag {
            offset: self.motion.camera(&screen, wall) - from as f64,
            screen,
            from,
            back,
            ahead,
        })
    }

    /// A sideways swipe let go going `speed`, to settle on workspace `to`. Something else may
    /// have moved things on while the fingers were down, or the keyboard to another screen: then
    /// the view goes back to what its screen is showing. Either way it finishes from where the
    /// fingers left it, at their pace, rather than as a keypress's slide would.
    fn let_go_of_view(&mut self, drag: &Drag, to: usize, speed: f64) {
        tracing::info!(workspace = to + 1, speed, "three-finger swipe let go");
        let still = self.focused_screen_name() == drag.screen;
        let showing = self
            .screens
            .iter()
            .find(|screen| screen.output.name() == drag.screen)
            .map(|screen| screen.workspace);
        let settle = match self.bullet.as_ref().map(|mode| mode.view) {
            Some(view) if still && view == drag.from => {
                self.bullet_pan(to, to != drag.from);
                Some(to)
            }
            Some(view) => Some(view),
            None if still && showing == Some(drag.from) && to != drag.from => {
                self.switch_workspace(to);
                Some(to)
            }
            None => showing,
        };
        if let Some(index) = settle {
            let wall = self.wall();
            self.motion.let_go_camera(&drag.screen, index, wall, speed);
        }
    }

    /// An upright swipe let go going `speed`, with bullet time to end up `open` or not. Opening
    /// or leaving happens as Super+Tab or Esc would, but the zoom finishes from where the fingers
    /// left it, at their pace.
    fn let_go_of_zoom(&mut self, open: bool, speed: f64) {
        tracing::info!(open, speed, "three-finger swipe let go");
        match (open, self.bullet.is_some()) {
            (true, false) => self.enter_bullet_time(),
            (false, true) => self.bullet_back(),
            _ => {}
        }
        let wall = self.wall();
        let at = self.overview.value(wall)[0];
        let to = if open { 1.0 } else { 0.0 };
        self.overview = motion::let_go(at, to, wall, speed, self.clock.reduced_motion);
    }

    /// A whole three-finger swipe at once, as a debug step makes it.
    pub fn swiped(&mut self, direction: Direction) {
        tracing::info!(?direction, "three-finger swipe");
        let step: i32 = match direction {
            Direction::Up if self.bullet.is_none() => return self.enter_bullet_time(),
            Direction::Down if self.bullet.is_some() => return self.bullet_back(),
            Direction::Up | Direction::Down => return,
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
    fn a_swipe_takes_its_way_once_it_has_gone_far_enough_and_keeps_it() {
        let mut swipe = Swipe::default();
        assert_eq!(swipe.moved(-5.0, 0.0, 0.0), None);
        assert_eq!(swipe.moved(-15.0, 0.0, FRAME), Some((Axis::Sideways, 0.05)));
        assert_eq!(
            swipe.moved(-180.0, 0.0, 2.0 * FRAME),
            Some((Axis::Sideways, 0.5))
        );
        assert_eq!(
            swipe.moved(0.0, 90.0, 3.0 * FRAME),
            Some((Axis::Sideways, 0.5)),
            "once sideways, it stays sideways"
        );
        assert_eq!(
            swipe.moved(400.0, 0.0, 4.0 * FRAME),
            Some((Axis::Sideways, -0.5)),
            "and goes back past where it started"
        );
    }

    #[test]
    fn up_is_out_and_a_diagonal_swipe_waits_to_tell() {
        let mut swipe = Swipe::default();
        assert_eq!(swipe.moved(20.0, -20.0, 0.0), None);
        assert_eq!(swipe.moved(0.0, -40.0, FRAME), Some((Axis::Upright, 0.2)));
        let mut down = Swipe::default();
        assert_eq!(down.moved(0.0, 150.0, 0.0), Some((Axis::Upright, -0.5)));
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
    fn bullet_time_opens_and_closes_with_the_fingers() {
        assert_eq!(zoom_at(0.0, 0.3), 0.3);
        assert_eq!(zoom_at(0.0, -0.3), 0.0, "down on the desktop does nothing");
        assert_eq!(zoom_at(1.0, 0.4), 1.0, "up in bullet time goes no further");
        assert_eq!(zoom_at(1.0, -0.3), 0.7);
        assert!(zoom_settles_open(0.0, 0.6, 0.0), "past halfway, it opens");
        assert!(
            !zoom_settles_open(0.0, 0.4, 0.0),
            "held short, it falls back"
        );
        assert!(zoom_settles_open(0.0, 0.2, 2.0), "flicked up, it opens");
        assert!(
            !zoom_settles_open(1.0, -0.6, 0.0),
            "down past halfway, it closes"
        );
        assert!(
            zoom_settles_open(1.0, -0.6, 2.0),
            "sent back up, it stays open"
        );
    }

    #[test]
    fn fingers_held_still_before_lifting_have_no_speed() {
        let mut swipe = Swipe::default();
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
        assert_eq!(Swipe::default().lifted(0.0), None, "never took hold");
    }
}

//! Where windows are painted while they move: a tween per window on the animation clock, and a
//! camera per screen that slides between workspaces. Layout and focus change on the keypress (Law 1); this
//! only decides where things are drawn on the way there. Input still uses the real positions.
//!
//! Generic over the window type so it's unit-tested without Wayland.

use crate::{
    anim::{Easing, Tween},
    layout::Rect,
};

/// The mockup's `--hypr` curve, with its slight overshoot.
pub const HYPR: Easing = Easing::Bezier(0.05, 0.9, 0.1, 1.05);
/// Retiles and workspace slides, in animation seconds.
pub const MOVE: f64 = 0.34;
/// A window opening: it grows from `OPEN_SCALE` and fades in.
pub const OPEN: f64 = 0.24;
pub const OPEN_SCALE: f64 = 0.82;
/// With reduced motion, moves jump and effects become short fades.
const REDUCED_FADE: f64 = 0.08;
/// Fades for a window that is travelling while it fades. Flatter than CSS's `ease-in` and
/// `ease-out`, which still leave it about a third transparent halfway: these hold it above
/// four fifths opaque to the middle of the journey, so it reads as a window going somewhere
/// rather than as a ghost crossing whatever is behind it.
const HOLD_THEN_GO: Easing = Easing::Bezier(0.8, 0.0, 1.0, 1.0);
const ARRIVE_THEN_STAY: Easing = Easing::Bezier(0.0, 0.0, 0.2, 1.0);
/// How far behind the leading window the last one in a formation sets off, in seconds: on a
/// workspace switch the window at the front of the travel goes first and the rest draft in
/// behind it.
pub const FORMATION: f64 = 0.08;
/// The same for windows moving together in a retile, which travel less far.
pub const RETILE_FORMATION: f64 = 0.05;
/// Space between workspaces as they slide past, in logical pixels: the mockup's 160 stage pixels
/// at the laptop's 1.25×.
pub const WORKSPACE_GAP: i32 = 128;

/// How to draw one window this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// x, y, width and height in logical pixels.
    pub rect: [f64; 4],
    /// The rect is still travelling, so the window is stretched to it rather than drawn at its
    /// own size.
    pub moving: bool,
    /// Scale about the window's centre.
    pub scale: f64,
    pub alpha: f64,
    /// Hasn't drawn anything yet; its open animation starts when it does.
    pub waiting: bool,
}

impl Frame {
    /// A window with nothing animating, such as an X11 menu.
    pub fn still(rect: [f64; 4]) -> Self {
        Self {
            rect,
            moving: false,
            scale: 1.0,
            alpha: 1.0,
            waiting: false,
        }
    }
}

#[derive(Debug)]
struct Entry<T> {
    id: T,
    rect: Tween<4>,
    scale: Tween<1>,
    alpha: Tween<1>,
    waiting: bool,
}

#[derive(Debug)]
pub struct Motion<T> {
    entries: Vec<Entry<T>>,
    /// The workspace each screen is looking at, fractional while it slides, kept by the screen's
    /// name so that unplugging one doesn't shuffle the others' slides.
    cameras: Vec<(String, Tween<1>)>,
    pub reduced_motion: bool,
}

impl<T: Clone + PartialEq> Motion<T> {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            entries: Vec::new(),
            cameras: Vec::new(),
            reduced_motion,
        }
    }

    fn move_duration(&self) -> f64 {
        if self.reduced_motion { 0.0 } else { MOVE }
    }

    fn entry_mut(&mut self, id: &T) -> Option<&mut Entry<T>> {
        self.entries.iter_mut().find(|entry| &entry.id == id)
    }

    /// Sends `id` towards `rect`. A window seen for the first time appears there, invisible
    /// until `shown`.
    pub fn place(&mut self, id: &T, rect: Rect, now: f64) {
        self.place_over(id, rect, now, MOVE);
    }

    /// `place`, over a given time rather than the usual one: a window pouring into the code rain
    /// with a screenful of others goes quicker than one leaving on its own.
    pub fn place_over(&mut self, id: &T, rect: Rect, now: f64, duration: f64) {
        let to = [rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64];
        let duration = if self.reduced_motion { 0.0 } else { duration };
        let first_scale = if self.reduced_motion { 1.0 } else { OPEN_SCALE };
        match self.entry_mut(id) {
            Some(entry) => entry.rect.retarget(to, now, duration, HYPR),
            None => self.entries.push(Entry {
                id: id.clone(),
                rect: Tween::at_rest(to),
                scale: Tween::at_rest([first_scale]),
                alpha: Tween::at_rest([0.0]),
                waiting: true,
            }),
        }
    }

    /// `id` has drawn its first frame: open it.
    pub fn shown(&mut self, id: &T, now: f64) {
        let reduced = self.reduced_motion;
        let Some(entry) = self.entry_mut(id).filter(|entry| entry.waiting) else {
            return;
        };
        entry.waiting = false;
        if reduced {
            entry
                .alpha
                .retarget([1.0], now, REDUCED_FADE, Easing::Linear);
        } else {
            entry.scale.retarget([1.0], now, OPEN, HYPR);
            entry.alpha.retarget([1.0], now, OPEN, HYPR);
        }
    }

    /// Moves where `id` is drawn by `dx` without changing where it's headed. A window sent to
    /// another workspace keeps its place on screen while the view slides after it.
    pub fn carry(&mut self, id: &T, dx: f64, now: f64) {
        if let Some(entry) = self.entry_mut(id) {
            let mut value = entry.rect.value(now);
            value[0] += dx;
            entry.rect = Tween::at_rest(value);
        }
    }

    /// Fades `id` to `alpha` over `duration`, straight, which is what a fade in place wants.
    pub fn fade(&mut self, id: &T, alpha: f64, now: f64, duration: f64) {
        self.fade_on(id, alpha, now, duration, Easing::Linear);
    }

    /// Fades out late: `id` holds its opacity almost all the way and goes out as it arrives.
    ///
    /// For a window travelling while it fades. A straight fade leaves it half transparent in the
    /// middle of the screen, where it reads as the desktop dissolving rather than as that window
    /// going to that place, and it mixes with whatever it is crossing on the way.
    pub fn fade_out_landing(&mut self, id: &T, now: f64, duration: f64) {
        self.fade_on(id, 0.0, now, duration, HOLD_THEN_GO);
    }

    /// Fades in early: `id` is solid almost at once and travels the rest of the way in view, the
    /// other half of `fade_out_landing`.
    pub fn fade_in_leaving(&mut self, id: &T, now: f64, duration: f64) {
        self.fade_on(id, 1.0, now, duration, ARRIVE_THEN_STAY);
    }

    fn fade_on(&mut self, id: &T, alpha: f64, now: f64, duration: f64, easing: Easing) {
        let duration = if self.reduced_motion {
            duration.min(REDUCED_FADE)
        } else {
            duration
        };
        if let Some(entry) = self.entry_mut(id) {
            // A fade takes the opening's place, so `shown` will never open this window. Its scale
            // has to be settled here as well, or it keeps `OPEN_SCALE` for good: a window poured
            // into the code rain before it ever drew — a layout coming back with a minimised app
            // in it — would come back small, centred in its tile.
            if entry.waiting {
                entry.waiting = false;
                entry.scale = Tween::at_rest([1.0]);
            }
            entry.alpha.retarget([alpha], now, duration, easing);
        }
    }

    /// Draws `id` at `rect` straight away, with no glide.
    pub fn jump(&mut self, id: &T, rect: Rect) {
        if let Some(entry) = self.entry_mut(id) {
            entry.rect =
                Tween::at_rest([rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64]);
        }
    }

    pub fn remove(&mut self, id: &T) {
        self.entries.retain(|entry| &entry.id != id);
    }

    pub fn frame(&self, id: &T, now: f64) -> Option<Frame> {
        let entry = self.entries.iter().find(|entry| &entry.id == id)?;
        Some(Frame {
            rect: entry.rect.value(now),
            moving: !entry.rect.done(now),
            scale: entry.scale.value(now)[0],
            // The curve overshoots; opacity can't.
            alpha: entry.alpha.value(now)[0].clamp(0.0, 1.0),
            waiting: entry.waiting,
        })
    }

    /// Slides `screen`'s view to workspace `index` (0-based). Higher numbers sit to the right.
    pub fn slide_to(&mut self, screen: &str, index: usize, now: f64) {
        let duration = self.move_duration();
        self.camera_mut(screen)
            .retarget([index as f64], now, duration, HYPR);
    }

    /// Where `screen`'s view is for a window `place` of the way across the screen (0 at the left
    /// edge, 1 at the right): the window at the front of the travel moves with the view and those
    /// behind follow a little later, so a workspace flies in formation rather than as one sheet.
    pub fn camera_in_formation(&self, screen: &str, now: f64, place: f64) -> f64 {
        let Some((_, camera)) = self.cameras.iter().find(|(name, _)| name == screen) else {
            return 0.0;
        };
        let travel = camera.to[0] - camera.from[0];
        if self.reduced_motion || travel == 0.0 {
            return camera.value(now)[0];
        }
        // Going to a higher workspace the windows travel left, so the leftmost leads.
        let behind = if travel > 0.0 { place } else { 1.0 - place };
        camera.value(now - behind.clamp(0.0, 1.0) * FORMATION)[0]
    }

    /// Holds `id` where it is for `delay` before its glide sets off: windows moving together
    /// leave one after another, the leader first.
    pub fn hold(&mut self, id: &T, now: f64, delay: f64) {
        if self.reduced_motion || delay <= 0.0 {
            return;
        }
        if let Some(entry) = self.entry_mut(id)
            && entry.rect.start == now
            && !entry.rect.done(now)
        {
            entry.rect.start = now + delay;
        }
    }

    /// Where `id` is headed, x, y, w and h.
    pub fn target(&self, id: &T) -> Option<[f64; 4]> {
        self.entries
            .iter()
            .find(|entry| &entry.id == id)
            .map(|entry| entry.rect.to)
    }

    /// Where `screen`'s view is, in workspaces. A screen nobody has slid yet is on its own
    /// workspace already, so `jump_camera` sets it when the screen lights up.
    pub fn camera(&self, screen: &str, now: f64) -> f64 {
        self.cameras
            .iter()
            .find(|(name, _)| name == screen)
            .map_or(0.0, |(_, camera)| camera.value(now)[0])
    }

    /// Puts `screen`'s view on `index` with no slide: a screen coming on, or one taking over a
    /// workspace from a screen that has gone out.
    pub fn jump_camera(&mut self, screen: &str, index: usize) {
        *self.camera_mut(screen) = Tween::at_rest([index as f64]);
    }

    pub fn forget_camera(&mut self, screen: &str) {
        self.cameras.retain(|(name, _)| name != screen);
    }

    fn camera_mut(&mut self, screen: &str) -> &mut Tween<1> {
        if let Some(index) = self.cameras.iter().position(|(name, _)| name == screen) {
            return &mut self.cameras[index].1;
        }
        self.cameras
            .push((screen.to_string(), Tween::at_rest([0.0])));
        &mut self.cameras.last_mut().unwrap().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window travelling into the code rain has to stay visible for the journey. A straight
    /// fade leaves it half transparent in the middle of the screen, crossing everything else.
    #[test]
    fn a_landing_fade_holds_its_opacity_until_the_end() {
        let mut motion: Motion<u32> = Motion::new(false);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        motion.place(&1, rect, 0.0);
        // Solid on the spot, as a window that has been on screen a while is.
        motion.fade(&1, 1.0, 0.0, 0.0);
        motion.fade_out_landing(&1, 0.0, 1.0);
        let alpha = |at: f64| motion.frame(&1, at).unwrap().alpha;
        assert!(alpha(0.5) > 0.8, "half way it is still nearly solid");
        assert!(alpha(0.9) < 0.35, "and it is gone as it lands");
        assert!(alpha(1.0) <= 0.001);
    }

    #[test]
    fn a_leaving_fade_is_solid_almost_at_once() {
        let mut motion: Motion<u32> = Motion::new(false);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        motion.place(&1, rect, 0.0);
        motion.fade_in_leaving(&1, 0.0, 1.0);
        assert!(
            motion.frame(&1, 0.5).unwrap().alpha > 0.8,
            "solid for most of the way out"
        );
    }

    const LEFT: Rect = Rect {
        x: 16,
        y: 16,
        w: 500,
        h: 600,
    };
    const RIGHT: Rect = Rect {
        x: 526,
        y: 16,
        w: 500,
        h: 600,
    };

    fn opened(reduced: bool) -> Motion<&'static str> {
        let mut motion = Motion::new(reduced);
        motion.place(&"a", LEFT, 0.0);
        motion.shown(&"a", 0.0);
        motion
    }

    #[test]
    fn a_window_stays_invisible_until_it_draws_then_grows_and_fades_in() {
        let mut motion = Motion::new(false);
        motion.place(&"a", LEFT, 0.0);
        let waiting = motion.frame(&"a", 5.0).unwrap();
        assert!(waiting.waiting);
        assert_eq!(waiting.alpha, 0.0, "nothing to see before its first buffer");

        motion.shown(&"a", 5.0);
        let start = motion.frame(&"a", 5.0).unwrap();
        assert_eq!((start.scale, start.alpha), (OPEN_SCALE, 0.0));
        let mid = motion.frame(&"a", 5.0 + OPEN / 2.0).unwrap();
        assert!(mid.scale > OPEN_SCALE && mid.alpha > 0.0);
        let end = motion.frame(&"a", 5.0 + OPEN).unwrap();
        assert_eq!((end.scale, end.alpha, end.waiting), (1.0, 1.0, false));
    }

    #[test]
    fn a_window_faded_before_it_ever_drew_comes_back_full_size() {
        // A layout coming back with a minimised app in it: the window is poured into the code
        // rain the moment it maps, so it never draws at full size and `shown` never opens it.
        let mut motion = Motion::new(false);
        motion.place(&"a", LEFT, 0.0);
        motion.fade(&"a", 0.0, 0.0, 0.26);
        assert_eq!(motion.frame(&"a", 0.26).unwrap().scale, 1.0);

        // Back out of its stream: it condenses in at its own size, not at the opening's.
        motion.fade(&"a", 1.0, 1.0, 0.26);
        let back = motion.frame(&"a", 1.0 + 0.26).unwrap();
        assert_eq!((back.scale, back.alpha), (1.0, 1.0));
    }

    #[test]
    fn retiling_glides_from_wherever_the_window_is_drawn() {
        let mut motion = opened(false);
        motion.place(&"a", RIGHT, 1.0);
        let start = motion.frame(&"a", 1.0).unwrap();
        assert_eq!(start.rect, [16.0, 16.0, 500.0, 600.0]);
        assert!(start.moving);

        let before = motion.frame(&"a", 1.1).unwrap().rect;
        motion.place(&"a", LEFT, 1.1);
        assert_eq!(
            motion.frame(&"a", 1.1).unwrap().rect,
            before,
            "a retarget mid-flight never jumps"
        );

        let end = motion.frame(&"a", 1.1 + MOVE).unwrap();
        assert_eq!(end.rect, [16.0, 16.0, 500.0, 600.0]);
        assert!(!end.moving);
    }

    #[test]
    fn a_carried_window_keeps_its_place_on_screen() {
        let mut motion = opened(false);
        motion.carry(&"a", -2080.0, 1.0);
        motion.place(&"a", RIGHT, 1.0);
        assert_eq!(motion.frame(&"a", 1.0).unwrap().rect[0], 16.0 - 2080.0);
        assert_eq!(motion.frame(&"a", 1.0 + MOVE).unwrap().rect[0], 526.0);
    }

    #[test]
    fn the_camera_slides_between_workspaces() {
        let mut motion: Motion<&str> = Motion::new(false);
        motion.slide_to("eDP-1", 2, 1.0);
        assert_eq!(motion.camera("eDP-1", 1.0), 0.0);
        let mid = motion.camera("eDP-1", 1.0 + MOVE / 2.0);
        assert!(mid > 0.0 && mid < 2.2, "part way there, allowing overshoot");
        assert_eq!(motion.camera("eDP-1", 1.0 + MOVE), 2.0);
    }

    #[test]
    fn every_screen_slides_on_its_own() {
        let mut motion: Motion<&str> = Motion::new(false);
        motion.jump_camera("DP-2", 1);
        motion.slide_to("eDP-1", 2, 1.0);
        assert_eq!(motion.camera("eDP-1", 1.0 + MOVE), 2.0);
        assert_eq!(
            motion.camera("DP-2", 1.0 + MOVE),
            1.0,
            "the other screen stays put"
        );
        motion.forget_camera("DP-2");
        assert_eq!(motion.camera("DP-2", 1.0), 0.0);
    }

    #[test]
    fn reduced_motion_jumps_and_only_fades_briefly() {
        let mut motion = opened(true);
        let start = motion.frame(&"a", 0.0).unwrap();
        assert_eq!(start.scale, 1.0, "no growing");
        assert_eq!(motion.frame(&"a", REDUCED_FADE).unwrap().alpha, 1.0);

        motion.place(&"a", RIGHT, 1.0);
        let moved = motion.frame(&"a", 1.0).unwrap();
        assert_eq!(moved.rect[0], 526.0);
        assert!(!moved.moving);

        motion.slide_to("eDP-1", 3, 1.0);
        assert_eq!(motion.camera("eDP-1", 1.0), 3.0);
    }

    #[test]
    fn the_window_leading_a_switch_goes_first() {
        let mut motion: Motion<&str> = Motion::new(false);
        motion.slide_to("eDP-1", 1, 1.0);
        let mid = 1.0 + MOVE / 2.0;
        // Travelling left, the leftmost window is at the front.
        let left = motion.camera_in_formation("eDP-1", mid, 0.0);
        let right = motion.camera_in_formation("eDP-1", mid, 1.0);
        assert_eq!(left, motion.camera("eDP-1", mid));
        assert!(right < left, "the window behind is still catching up");
        assert_eq!(
            motion.camera_in_formation("eDP-1", 1.0 + MOVE + FORMATION, 1.0),
            1.0
        );
        // And coming back the other way, the rightmost leads.
        motion.slide_to("eDP-1", 0, 5.0);
        let mid = 5.0 + MOVE / 2.0;
        assert!(
            motion.camera_in_formation("eDP-1", mid, 0.0)
                > motion.camera_in_formation("eDP-1", mid, 1.0)
        );
    }

    #[test]
    fn a_held_window_waits_then_glides() {
        let mut motion = opened(false);
        motion.place(&"a", RIGHT, 1.0);
        motion.hold(&"a", 1.0, 0.05);
        assert_eq!(motion.frame(&"a", 1.04).unwrap().rect[0], 16.0);
        assert_eq!(motion.frame(&"a", 1.05 + MOVE).unwrap().rect[0], 526.0);
        assert_eq!(motion.target(&"a"), Some([526.0, 16.0, 500.0, 600.0]));
    }

    #[test]
    fn removed_windows_are_forgotten() {
        let mut motion = opened(false);
        motion.remove(&"a");
        assert_eq!(motion.frame(&"a", 0.0), None);
    }
}

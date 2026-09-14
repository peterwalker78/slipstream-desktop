//! When the UI steps aside for the living wallpaper. After a spell with no input it fades out; any
//! input brings it straight back, and the input that wakes it goes nowhere, so nothing is typed
//! blind. It stays up while Caps Lock is on, while a window fills the screen, and while an app asks
//! for the screen to stay awake (video players do).
//!
//! The wait comes from the settings file (`fade-after-secs`, 120 seconds by default; 0 never fades)
//! and can change while Slipstream runs.
//!
//! The same input also counts towards locking the screen by itself (`[lock] after-idle-mins`, off by
//! default). That wait is wall time since the last input, so bullet time can't stretch it, and
//! only a fullscreen window or an app asking for the screen to stay awake holds it off. Caps Lock
//! doesn't, since anyone can leave it on, and nor does an open panel or card, since neither is a
//! sign anyone is there.

use std::time::{Duration, Instant};

use smithay::{reexports::wayland_server::protocol::wl_surface::WlSurface, utils::IsAlive};

use crate::anim::{Easing, Tween};

/// Fading out and back in take the same time, the UI passing through the wallpaper like a pane of
/// glass either way (`glass.rs`). Fading out eases in and out. Coming back, the glass sets off at
/// once and reaches the front in `TRAVEL`, still moving, and the rest of the time is the view
/// shaking as it lands against the screen.
const FADE: f64 = 0.6;
const REDUCED_FADE: f64 = 0.08;
/// Coming back, how long the glass takes to reach the front.
const TRAVEL: f64 = 0.38;
/// Quick from the first frame, and still moving when it gets there.
const ARRIVE: Easing = Easing::Bezier(0.2, 0.4, 0.6, 0.8);
/// How far the view is knocked as the glass lands, in logical pixels, and how quickly the shake
/// dies away, in seconds.
const SHAKE: f64 = 5.0;
const SHAKE_DECAY: f64 = 0.055;

pub struct Idle {
    last_input: Instant,
    timeout: Option<Duration>,
    faded: bool,
    /// The UI's opacity, on the animation clock.
    opacity: Tween<1>,
    pub reduced_motion: bool,
    /// Surfaces whose apps asked for the screen to stay awake.
    pub inhibitors: Vec<WlSurface>,
    /// The last input, or the last moment something held the lock off.
    lock_idle_since: Instant,
    /// How long without input before the screen locks; `None` never.
    lock_after: Option<Duration>,
    /// The UI's opacity held for a debug step, whatever the fade is doing.
    pub pinned: Option<f64>,
    /// When the glass coming back lands against the screen, on the animation clock.
    landed: Option<f64>,
    /// The shake held for a debug step, in logical pixels.
    pub pinned_shake: Option<(f64, f64)>,
}

impl Idle {
    /// Fades after `fade_after_secs` without input; 0 never fades.
    pub fn new(reduced_motion: bool, fade_after_secs: u64) -> Self {
        Self {
            last_input: Instant::now(),
            timeout: timeout(fade_after_secs),
            faded: false,
            opacity: Tween::at_rest([1.0]),
            reduced_motion,
            inhibitors: Vec::new(),
            lock_idle_since: Instant::now(),
            lock_after: None,
            pinned: None,
            landed: None,
            pinned_shake: None,
        }
    }

    /// The wait before locking, from the settings, counted from the last input; 0 never locks.
    pub fn set_lock_after(&mut self, mins: u64) {
        self.lock_after = (mins > 0).then(|| Duration::from_secs(mins.saturating_mul(60)));
    }

    /// Whether the screen locks by itself after a while idle.
    pub fn locks_by_itself(&self) -> bool {
        self.lock_after.is_some()
    }

    /// Whether the screen should lock by itself at `now`: its wait has passed with no input,
    /// and nothing has held it off in that time — `held` (a fullscreen window) or an app asking
    /// for the screen to stay awake.
    pub fn lock_due(&mut self, now: Instant, held: bool) -> bool {
        self.inhibitors.retain(|surface| surface.alive());
        if held || !self.inhibitors.is_empty() {
            self.lock_idle_since = now;
            return false;
        }
        self.lock_after
            .is_some_and(|after| now.saturating_duration_since(self.lock_idle_since) >= after)
    }

    /// Starts the wait before locking again, as input does.
    pub fn restart_lock_wait(&mut self, now: Instant) {
        self.lock_idle_since = now;
    }

    /// A new wait from the settings, counted from the last input; 0 never fades.
    pub fn set_fade_after(&mut self, secs: u64) {
        self.timeout = timeout(secs);
    }

    fn duration(&self) -> f64 {
        if self.reduced_motion {
            REDUCED_FADE
        } else {
            FADE
        }
    }

    /// Something was pressed or moved. Returns true if the UI was faded out, so this input only
    /// brings it back and shouldn't reach anything.
    pub fn input(&mut self, now: f64) -> bool {
        self.last_input = Instant::now();
        self.lock_idle_since = self.last_input;
        if !self.faded {
            return false;
        }
        let hidden = self.opacity.value(now)[0] < 0.5;
        self.show(now);
        hidden
    }

    fn show(&mut self, now: f64) {
        self.faded = false;
        if self.reduced_motion {
            self.landed = None;
            self.opacity
                .retarget([1.0], now, REDUCED_FADE, Easing::OutCubic);
        } else {
            self.landed = Some(now + TRAVEL);
            self.opacity.retarget([1.0], now, TRAVEL, ARRIVE);
        }
    }

    /// How far the view is knocked aside at `now`, in logical pixels, while it shakes from the
    /// glass landing; `None` the rest of the time. A quick jolt that rings down to nothing by the
    /// time the fade's time is up.
    pub fn shake(&self, now: f64) -> Option<(f64, f64)> {
        if self.pinned_shake.is_some() {
            return self.pinned_shake;
        }
        let t = now - self.landed?;
        let window = FADE - TRAVEL;
        if self.faded || self.pinned.is_some() || !(0.0..window).contains(&t) {
            return None;
        }
        let strength = SHAKE * (-t / SHAKE_DECAY).exp() * (1.0 - t / window).powi(2);
        let turn = std::f64::consts::TAU * t;
        Some((
            strength * (turn * 19.0).sin(),
            0.7 * strength * (turn * 23.0 + 1.3).sin(),
        ))
    }

    /// Whether the UI has stepped aside for the wallpaper, or is on its way out.
    pub fn is_faded(&self) -> bool {
        self.faded
    }

    /// Fades the UI out now.
    pub fn fade(&mut self, now: f64) {
        if !self.faded {
            self.faded = true;
            let duration = self.duration();
            self.opacity
                .retarget([0.0], now, duration, Easing::InOutCubic);
        }
    }

    /// The UI's opacity now, fading it out once idle long enough, unless `keep_up` or an app
    /// asks for the screen to stay awake.
    pub fn update(&mut self, now: f64, keep_up: bool) -> f64 {
        self.inhibitors.retain(|surface| surface.alive());
        if keep_up || !self.inhibitors.is_empty() {
            self.last_input = Instant::now();
            if self.faded {
                self.show(now);
            }
        } else if self
            .timeout
            .is_some_and(|timeout| self.last_input.elapsed() >= timeout)
        {
            self.fade(now);
        }
        self.pinned
            .unwrap_or_else(|| self.opacity.value(now)[0])
            .clamp(0.0, 1.0)
    }
}

fn timeout(secs: u64) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_lock_is_off_by_default_and_waits_its_minutes_unless_inhibited_or_fullscreen() {
        let start = Instant::now();
        let mut idle = Idle::new(false, 120);
        idle.lock_idle_since = start;
        let later = |mins: u64| start + Duration::from_secs(mins * 60);
        assert!(!idle.lock_due(later(600), false), "off by default");

        idle.set_lock_after(5);
        assert!(!idle.lock_due(later(4), false));
        assert!(idle.lock_due(later(5), false));

        idle.restart_lock_wait(later(5));
        assert!(
            !idle.lock_due(later(9), false),
            "input starts the wait again"
        );
        assert!(
            !idle.lock_due(later(11), true),
            "a fullscreen window holds it off"
        );
        assert!(
            !idle.lock_due(later(15), false),
            "and the wait counts from when it stopped holding"
        );
        assert!(idle.lock_due(later(16), false));
        // Only the fade's reasons to stay up that mean someone may be watching count: an open
        // panel keeps the fade off (`update`'s `keep_up`) but not the lock.
        idle.update(0.0, true);
        assert!(idle.lock_due(later(16), false));
        idle.set_lock_after(0);
        assert!(!idle.lock_due(later(100), false), "0 is never");
    }

    #[test]
    fn a_new_wait_counts_from_the_last_input() {
        let mut idle = Idle::new(false, 0);
        idle.last_input = Instant::now() - Duration::from_secs(5);
        assert_eq!(idle.update(0.0, false), 1.0, "0 never fades");
        idle.set_fade_after(60);
        assert_eq!(idle.update(FADE, false), 1.0, "5 s idle isn't 60");
        idle.set_fade_after(2);
        assert_eq!(idle.update(FADE, false), 1.0, "the fade starts from full");
        assert_eq!(idle.update(2.0 * FADE, false), 0.0);
    }

    #[test]
    fn idle_fades_the_ui_and_the_next_input_only_wakes_it() {
        let mut idle = Idle {
            timeout: Some(Duration::ZERO),
            ..Idle::new(false, 120)
        };
        assert_eq!(idle.update(0.0, false), 1.0, "the fade starts from full");
        assert_eq!(idle.update(FADE, false), 0.0);
        assert!(idle.input(FADE + 0.1), "the waking input is swallowed");
        assert_eq!(idle.update(FADE + 0.1 + FADE, true), 1.0);
        assert!(!idle.input(FADE + 1.0), "later input goes through");
    }

    #[test]
    fn fading_out_and_back_in_take_the_same_time() {
        let mut idle = Idle {
            timeout: Some(Duration::ZERO),
            ..Idle::new(false, 120)
        };
        idle.update(0.0, false);
        assert!(idle.update(FADE - 0.01, false) > 0.0);
        assert_eq!(idle.update(FADE, false), 0.0);
        idle.input(FADE);
        assert!(idle.update(FADE + TRAVEL - 0.01, true) < 1.0);
        assert_eq!(
            idle.update(FADE + TRAVEL, true),
            1.0,
            "the glass is at the front"
        );
        assert!(
            idle.shake(FADE + TRAVEL + 0.01).is_some(),
            "and shakes as it lands"
        );
        assert_eq!(idle.shake(2.0 * FADE), None, "until the fade's time is up");
    }

    #[test]
    fn the_shake_rings_down_to_nothing() {
        let mut idle = Idle::new(false, 120);
        idle.fade(0.0);
        idle.input(1.0);
        let landed = 1.0 + TRAVEL;
        let size = |t: f64| idle.shake(landed + t).map_or(0.0, |(x, y)| x.hypot(y));
        let window = FADE - TRAVEL;
        let early = (0..10).map(|i| size(i as f64 * 0.004)).fold(0.0, f64::max);
        let late = (0..10)
            .map(|i| size(window * 0.8 + i as f64 * 0.004))
            .fold(0.0, f64::max);
        assert!(
            early > 2.0 && early <= SHAKE * 1.25,
            "a knock you can see: {early}"
        );
        assert!(late < 0.05, "gone by the end: {late}");

        let mut calm = Idle::new(true, 120);
        calm.fade(0.0);
        calm.input(1.0);
        assert_eq!(
            calm.shake(1.0 + TRAVEL + 0.01),
            None,
            "no shake with reduced motion"
        );
    }

    #[test]
    fn coming_back_answers_the_key_at_once() {
        let mut idle = Idle {
            timeout: Some(Duration::ZERO),
            ..Idle::new(false, 120)
        };
        idle.update(0.0, false);
        let early_out = 1.0 - idle.update(FADE / 8.0, false);
        idle.update(FADE, false);
        idle.input(FADE);
        let early_back = idle.update(FADE + FADE / 8.0, true);
        assert!(
            early_back > 5.0 * early_out,
            "back {early_back} in the first eighth, against {early_out} out"
        );
    }

    #[test]
    fn caps_lock_and_friends_keep_the_ui_up() {
        let mut idle = Idle {
            timeout: Some(Duration::ZERO),
            ..Idle::new(false, 120)
        };
        for now in [0.0, 1.0, 5.0] {
            assert_eq!(idle.update(now, true), 1.0);
        }
    }

    #[test]
    fn a_timeout_of_zero_seconds_never_fades() {
        let mut idle = Idle {
            timeout: None,
            ..Idle::new(false, 120)
        };
        assert_eq!(idle.update(1000.0, false), 1.0);
    }
}

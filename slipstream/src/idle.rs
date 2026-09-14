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

/// The UI passes through the wallpaper like a pane of glass either way (`glass.rs`). Fading out
/// eases in and out, but the UI has gone before it slows, so what shows is the pane speeding
/// away. Coming back mirrors that: it speeds up all the way into place, after hurrying through
/// the first stretch where nothing of the UI can be seen yet, so the waking key is answered at
/// once.
const FADE: f64 = 0.6;
const RETURN: f64 = 0.5;
const REDUCED_FADE: f64 = 0.08;

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

    /// Whether the UI fades to the wallpaper at all.
    pub fn fades(&self) -> bool {
        self.timeout.is_some()
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
            self.opacity
                .retarget([1.0], now, REDUCED_FADE, Easing::OutCubic);
        } else {
            self.opacity.retarget([1.0], now, RETURN, Easing::Arrive);
        }
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
    fn fading_out_takes_its_time_and_coming_back_a_little_less() {
        let mut idle = Idle {
            timeout: Some(Duration::ZERO),
            ..Idle::new(false, 120)
        };
        idle.update(0.0, false);
        assert!(idle.update(FADE - 0.01, false) > 0.0);
        assert_eq!(idle.update(FADE, false), 0.0);
        idle.input(FADE);
        assert!(idle.update(FADE + RETURN - 0.01, true) < 1.0);
        assert_eq!(idle.update(FADE + RETURN, true), 1.0);
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

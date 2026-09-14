//! The animation clock and tweens. Every animation Slipstream draws reads this clock rather than
//! wall time, so bullet time slows all of them at once and brings them back in sync.
//!
//! Window animations (`motion.rs`) run on it, and bullet time slows it.
#![allow(dead_code)]

use std::time::Instant;

/// Bullet time's speed.
pub const SLOW: f64 = 0.25;
/// How long, in wall time, the clock takes to slow down on entering bullet time.
const RAMP: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Normal,
    /// Easing from rate `from` down to `SLOW`, started at wall time `t0`.
    Slowing {
        from: f64,
        t0: f64,
    },
    Slow,
    /// Repaying the deficit after bullet time: started at wall time `t0` with clock `anim0` and
    /// rate `r0`, lasting `window` seconds, with speed-up coefficient `k`.
    CatchingUp {
        t0: f64,
        anim0: f64,
        r0: f64,
        window: f64,
        k: f64,
    },
}

#[derive(Debug, Clone)]
pub struct Clock {
    started: Instant,
    /// Wall seconds since start, at the last tick.
    wall: f64,
    /// Animation seconds, at the last tick.
    anim: f64,
    phase: Phase,
    /// Skip the slow-down and catch-up ramps (`SLIPSTREAM_REDUCED_MOTION=1`).
    pub reduced_motion: bool,
}

impl Clock {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            started: Instant::now(),
            wall: 0.0,
            anim: 0.0,
            phase: Phase::Normal,
            reduced_motion,
        }
    }

    /// Advances to the present and returns the animation time.
    pub fn tick(&mut self) -> f64 {
        let wall = self.started.elapsed().as_secs_f64();
        self.advance_to(wall)
    }

    /// The animation time as of the last tick.
    pub fn now(&self) -> f64 {
        self.anim
    }

    pub fn in_bullet_time(&self) -> bool {
        matches!(self.phase, Phase::Slowing { .. } | Phase::Slow)
    }

    /// Whether the clock is running at anything other than normal speed.
    pub fn is_dilated(&self) -> bool {
        self.phase != Phase::Normal
    }

    /// The clock's current speed relative to wall time.
    pub fn rate(&self) -> f64 {
        self.rate_at(self.wall)
    }

    pub fn enter_bullet_time(&mut self) {
        if self.in_bullet_time() {
            return;
        }
        self.phase = if self.reduced_motion {
            Phase::Slow
        } else {
            // Starting from the current rate keeps any deficit still being repaid.
            Phase::Slowing {
                from: self.rate(),
                t0: self.wall,
            }
        };
    }

    pub fn leave_bullet_time(&mut self) {
        if !self.in_bullet_time() {
            return;
        }
        let deficit = self.wall - self.anim;
        if self.reduced_motion || deficit <= 1e-6 {
            self.anim = self.wall;
            self.phase = Phase::Normal;
            return;
        }
        let r0 = self.rate();
        let window = (0.4 + 0.02 * deficit).clamp(0.4, 1.5);
        let k = 60.0 * (deficit / window + (1.0 - r0) / 4.0);
        self.phase = Phase::CatchingUp {
            t0: self.wall,
            anim0: self.anim,
            r0,
            window,
            k,
        };
    }

    fn rate_at(&self, wall: f64) -> f64 {
        match self.phase {
            Phase::Normal => 1.0,
            Phase::Slow => SLOW,
            Phase::Slowing { from, t0 } => {
                let t = ((wall - t0) / RAMP).clamp(0.0, 1.0);
                from + (SLOW - from) * ease_in_out_cubic(t)
            }
            Phase::CatchingUp {
                t0, r0, window, k, ..
            } => {
                let u = ((wall - t0) / window).clamp(0.0, 1.0);
                1.0 - (1.0 - r0) * (1.0 - u).powi(3) + k * u.powi(3) * (1.0 - u).powi(2)
            }
        }
    }

    fn advance_to(&mut self, wall: f64) -> f64 {
        let wall = wall.max(self.wall);
        match self.phase {
            Phase::Normal => self.anim += wall - self.wall,
            Phase::Slow => self.anim += (wall - self.wall) * SLOW,
            Phase::Slowing { t0, .. } => {
                // Trapezoid steps: the slow-down is brief, and any error is repaid on leaving,
                // which measures the deficit directly.
                let rate_then = self.rate_at(self.wall);
                let rate_now = self.rate_at(wall);
                self.anim += (wall - self.wall) * (rate_then + rate_now) / 2.0;
                if wall - t0 >= RAMP {
                    self.phase = Phase::Slow;
                }
            }
            Phase::CatchingUp {
                t0,
                anim0,
                r0,
                window,
                k,
            } => {
                let u = (wall - t0) / window;
                if u >= 1.0 {
                    self.anim = wall;
                    self.phase = Phase::Normal;
                } else {
                    // Closed form, so the landing is exact rather than a sum of frame errors.
                    let slow_part = (1.0 - r0) * (1.0 - (1.0 - u).powi(4)) / 4.0;
                    let boost = k * (u.powi(4) / 4.0 - 2.0 * u.powi(5) / 5.0 + u.powi(6) / 6.0);
                    self.anim = anim0 + window * (u - slow_part + boost);
                }
            }
        }
        self.wall = wall;
        self.anim
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    OutCubic,
    InOutCubic,
    /// Speeding up all the way into place, after a quick start over the first tenth: for the UI
    /// coming back from the wallpaper, whose first stretch can't be seen anyway.
    Arrive,
    /// A CSS `cubic-bezier(x1, y1, x2, y2)` curve, as the mockup's transitions use.
    Bezier(f64, f64, f64, f64),
}

pub fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

impl Easing {
    pub fn at(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::OutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::InOutCubic => ease_in_out_cubic(t),
            Easing::Arrive => arrive(t),
            Easing::Bezier(x1, y1, x2, y2) => bezier(x1, y1, x2, y2, t),
        }
    }
}

/// `Easing::Arrive`: two stretches of steady change in speed that meet without a jump. The first
/// tenth of the time covers the first seventh of the way, slowing down; the rest speeds up from
/// there to the end, finishing nearly four times as fast as it started.
fn arrive(t: f64) -> f64 {
    const QUICK_TIME: f64 = 0.1;
    const QUICK_WAY: f64 = 0.14;
    const SLOWEST: f64 = 0.4;
    let first = 2.0 * QUICK_WAY / QUICK_TIME - SLOWEST;
    let last = 2.0 * (1.0 - QUICK_WAY) / (1.0 - QUICK_TIME) - SLOWEST;
    if t < QUICK_TIME {
        first * t + 0.5 * (SLOWEST - first) / QUICK_TIME * t * t
    } else {
        let s = t - QUICK_TIME;
        QUICK_WAY + SLOWEST * s + 0.5 * (last - SLOWEST) / (1.0 - QUICK_TIME) * s * s
    }
}

/// Evaluates a CSS cubic-bezier timing function at progress `x`.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let coord = |a: f64, b: f64, s: f64| {
        3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3)
    };
    let slope = |a: f64, b: f64, s: f64| {
        3.0 * a * (1.0 - s).powi(2) + 6.0 * (b - a) * s * (1.0 - s) + 3.0 * (1.0 - b) * s * s
    };
    // Newton's method on x(s) = x, falling back to bisection where the slope is flat.
    let mut s = x;
    for _ in 0..8 {
        let dx = slope(x1, x2, s);
        if dx.abs() < 1e-6 {
            break;
        }
        s = (s - (coord(x1, x2, s) - x) / dx).clamp(0.0, 1.0);
    }
    if (coord(x1, x2, s) - x).abs() > 1e-4 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            s = (lo + hi) / 2.0;
            if coord(x1, x2, s) < x {
                lo = s;
            } else {
                hi = s;
            }
        }
    }
    coord(y1, y2, s)
}

/// A value moving from `from` to `to` on the animation clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween<const N: usize> {
    pub from: [f64; N],
    pub to: [f64; N],
    pub start: f64,
    pub duration: f64,
    pub easing: Easing,
}

impl<const N: usize> Tween<N> {
    /// Already at `value`.
    pub fn at_rest(value: [f64; N]) -> Self {
        Self {
            from: value,
            to: value,
            start: 0.0,
            duration: 0.0,
            easing: Easing::Linear,
        }
    }

    pub fn value(&self, now: f64) -> [f64; N] {
        if self.duration <= 0.0 || now >= self.start + self.duration {
            return self.to;
        }
        let e = self.easing.at((now - self.start) / self.duration);
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * e)
    }

    pub fn done(&self, now: f64) -> bool {
        self.duration <= 0.0 || now >= self.start + self.duration
    }

    /// Heads for `to` from wherever the tween is now, so a retarget mid-flight never jumps.
    pub fn retarget(&mut self, to: [f64; N], now: f64, duration: f64, easing: Easing) {
        if to == self.to {
            return;
        }
        *self = Self {
            from: self.value(now),
            to,
            start: now,
            duration,
            easing,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 60.0;

    #[test]
    fn arriving_ends_where_it_should_and_speeds_up_into_place() {
        let at = |t: f64| Easing::Arrive.at(t);
        assert_eq!(at(0.0), 0.0);
        assert!((at(1.0) - 1.0).abs() < 1e-9);
        let speed = |t: f64| (at(t + 1e-4) - at(t)) / 1e-4;
        assert!(
            (speed(0.1 - 2e-4) - speed(0.1 + 1e-4)).abs() < 0.01,
            "no jump in speed"
        );
        let mut last = speed(0.1);
        for i in 3..=19 {
            let now = speed(i as f64 * 0.05);
            assert!(now > last, "faster at {}", i as f64 * 0.05);
            last = now;
        }
        assert!(
            speed(0.0) > speed(0.1),
            "the unseen first stretch goes by quickly"
        );
    }

    /// Bullet time held for `held` seconds, then released; returns the clock and the wall time
    /// catch-up finished at, checking the rate as it goes.
    fn run_bullet_time(held: f64) -> (Clock, f64) {
        let mut clock = Clock::new(false);
        let mut wall = 1.0;
        clock.advance_to(wall);
        clock.enter_bullet_time();
        while wall < 1.0 + held {
            wall += FRAME;
            clock.advance_to(wall);
        }
        let rate_before = clock.rate();
        clock.leave_bullet_time();
        assert!(
            (clock.rate() - rate_before).abs() < 1e-9,
            "no jump in speed on release"
        );
        let mut first_frame = true;
        while clock.is_dilated() {
            wall += FRAME;
            clock.advance_to(wall);
            let rate = clock.rate_at(wall);
            if clock.is_dilated() {
                assert!(
                    rate >= SLOW - 1e-9,
                    "rate {rate} dipped below bullet time's"
                );
                // The speed-up eases in: the first frame after release only nudges the speed.
                // (A u² curve jumped from 0.25× to 0.68× here after 15 s of bullet time.)
                if first_frame {
                    assert!(
                        rate - rate_before < 0.15,
                        "speed lurched from {rate_before} to {rate} after {held} s"
                    );
                    first_frame = false;
                }
            }
            assert!(wall < 1.0 + held + 2.0, "catch-up never finished");
        }
        (clock, wall)
    }

    #[test]
    fn catch_up_lands_on_wall_time_for_many_lengths_of_bullet_time() {
        for held in [0.05, 0.2, 1.0, 3.0, 15.0, 60.0] {
            let (clock, wall) = run_bullet_time(held);
            assert!(
                (clock.now() - wall).abs() < FRAME,
                "after {held} s the clock was off by {}",
                clock.now() - wall
            );
        }
    }

    #[test]
    fn catch_up_is_continuous_just_before_it_lands() {
        let mut clock = Clock::new(false);
        clock.advance_to(0.0);
        clock.enter_bullet_time();
        clock.advance_to(5.0);
        clock.leave_bullet_time();
        let Phase::CatchingUp { t0, window, .. } = clock.phase else {
            panic!("not catching up");
        };
        let almost = clock.clone().advance_to(t0 + window - 1e-4);
        assert!(
            (almost - (t0 + window)).abs() < 1e-3,
            "position jumped at the end"
        );
    }

    #[test]
    fn reduced_motion_skips_the_ramps() {
        let mut clock = Clock::new(true);
        clock.advance_to(1.0);
        clock.enter_bullet_time();
        assert_eq!(clock.rate(), SLOW);
        clock.advance_to(3.0);
        clock.leave_bullet_time();
        assert_eq!(clock.now(), 3.0);
        assert!(!clock.is_dilated());
    }

    #[test]
    fn bezier_matches_its_endpoints_and_overshoots_like_css() {
        let drop = Easing::Bezier(0.05, 0.9, 0.1, 1.05);
        assert!(drop.at(0.0).abs() < 1e-6);
        assert!((drop.at(1.0) - 1.0).abs() < 1e-6);
        assert!(drop.at(0.3) > 0.8, "a fast start");
        let ease = Easing::Bezier(0.25, 0.1, 0.25, 1.0);
        assert!(
            (ease.at(0.5) - 0.8024).abs() < 0.01,
            "CSS 'ease' at half way"
        );
    }

    #[test]
    fn a_retargeted_tween_starts_from_where_it_was() {
        let mut tween = Tween::at_rest([0.0, 0.0]);
        tween.retarget([100.0, 0.0], 0.0, 1.0, Easing::Linear);
        tween.retarget([0.0, 50.0], 0.5, 1.0, Easing::Linear);
        assert_eq!(tween.value(0.5), [50.0, 0.0]);
        assert_eq!(tween.value(1.5), [0.0, 50.0]);
    }
}

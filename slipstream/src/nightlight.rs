//! Night light: warmer colours, as the mockup's, which multiplies `#ff8a2a` over the screen at 24%.
//! Done in each screen's gamma ramp rather than drawn, so it covers everything, the pointer and
//! fullscreen games included, and costs nothing per frame. Only on real hardware: nested, the host
//! desktop owns the screen's colours.
//!
//! It can follow a schedule, sunset to sunrise or set hours. When the schedule turns it, the
//! screen warms up (or cools down) over half an hour, the way the light outside changes, rather
//! than all at once; turning it by hand eases over a second and lasts until the next change.

/// How much of red, green and blue is left: `1 − 0.24 × (1 − overlay)` for each of the overlay's
/// channels.
pub const WARMTH: [f64; 3] = [
    1.0 - 0.24 * (1.0 - 0xff as f64 / 255.0),
    1.0 - 0.24 * (1.0 - 0x8a as f64 / 255.0),
    1.0 - 0.24 * (1.0 - 0x2a as f64 / 255.0),
];

/// How fast the warmth follows a change it isn't fading through: all of it in a second.
const EASE_PER_SEC: f64 = 1.0;
/// The smallest change worth sending to the screens.
const STEP: f64 = 1.0 / 256.0;

/// Red, green and blue ramps of `length` steps: straight lines, scaled towards `WARMTH` by
/// `strength`, 0 (off) to 1 (fully warm).
pub fn ramps(length: usize, strength: f64) -> [Vec<u16>; 3] {
    let last = length.saturating_sub(1).max(1) as f64;
    let strength = strength.clamp(0.0, 1.0);
    std::array::from_fn(|channel| {
        let top = 1.0 - (1.0 - WARMTH[channel]) * strength;
        (0..length)
            .map(|step| (step as f64 / last * top * 65535.0).round() as u16)
            .collect()
    })
}

/// Night light as it stands.
#[derive(Default)]
pub struct Night {
    /// The warmth on the screens now, 0 to 1.
    pub current: f64,
    /// What was last sent to the gamma ramps.
    applied: Option<f64>,
    /// Whether the schedule said night when last looked at; `None` without a schedule, or when it
    /// has just changed and must be looked at afresh.
    last_night: Option<bool>,
    /// The time zone's place, looked up once.
    location: Option<Option<(f64, f64)>>,
    last_tick: Option<f64>,
}

impl Night {
    /// The schedule changed: it is looked at afresh on the next pass.
    pub fn schedule_changed(&mut self) {
        self.last_night = None;
    }
}

/// What the local clock says: minutes after midnight, the date, and how far ahead of UTC it is.
fn local_clock() -> (f64, (i64, u32, u32), f64) {
    // SAFETY: `time` and `localtime_r` write only into the values handed to them.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return (12.0 * 60.0, (1970, 1, 1), 0.0);
        }
        (
            (tm.tm_hour * 60 + tm.tm_min) as f64 + tm.tm_sec as f64 / 60.0,
            (
                tm.tm_year as i64 + 1900,
                tm.tm_mon as u32 + 1,
                tm.tm_mday as u32,
            ),
            tm.tm_gmtoff as f64 / 60.0,
        )
    }
}

/// The warmth night light should have: `manual` is the switch, and `scheduled` whether the schedule
/// says night and how far its fade is. The switch wins against the schedule until the schedule
/// next turns, which sets the switch to match.
pub fn target(manual: bool, scheduled: Option<(bool, f64)>) -> f64 {
    match (manual, scheduled) {
        (true, None) => 1.0,
        (false, None) => 0.0,
        // Warming through the evening, or cooling after morning: the fade.
        (true, Some((true, strength))) | (false, Some((false, strength))) => strength,
        // Turned by hand against the schedule.
        (true, Some((false, _))) => 1.0,
        (false, Some((true, _))) => 0.0,
    }
}

impl crate::Slipstream {
    /// Once a pass of the event loop: follows the schedule, eases towards the warmth wanted, and
    /// sends it to the screens when it has moved.
    pub fn tick_night_light(&mut self) {
        let wall = self.wall();
        let dt = self
            .night
            .last_tick
            .map_or(0.0, |last| (wall - last).clamp(0.0, 0.25));
        self.night.last_tick = Some(wall);
        let display = self.settings.display.clone();
        let (minute, date, offset) = local_clock();
        let location = *self
            .night
            .location
            .get_or_insert_with(slipstream_config::sun::location);
        let scheduled = display
            .night_window(date, offset, location)
            .map(|(start, end)| {
                slipstream_config::sun::scheduled(
                    minute,
                    start,
                    end,
                    slipstream_config::NIGHT_FADE_MINS,
                )
            });
        match scheduled {
            None => self.night.last_night = None,
            Some((night, _)) if self.night.last_night != Some(night) => {
                self.night.last_night = Some(night);
                tracing::info!(night, "night light's schedule turned");
                if display.night_light != night && self.owns_state {
                    self.change_settings(move |settings| settings.display.night_light = night);
                }
            }
            Some(_) => {}
        }
        let target = target(display.night_light, scheduled);
        let current = self.night.current;
        self.night.current = if self.clock.reduced_motion {
            target
        } else {
            let step = EASE_PER_SEC * dt;
            current + (target - current).clamp(-step, step)
        };
        let moved = self.night.applied.is_none_or(|applied| {
            (applied - self.night.current).abs() >= STEP
                || (self.night.current == target && applied != target)
        });
        if moved {
            self.night.applied = Some(self.night.current);
            self.set_night_light(self.night.current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_wins_until_the_schedule_turns() {
        assert_eq!(target(true, None), 1.0);
        assert_eq!(target(false, None), 0.0);
        assert_eq!(target(true, Some((true, 0.4))), 0.4);
        assert_eq!(target(false, Some((false, 0.7))), 0.7);
        assert_eq!(target(false, Some((true, 0.9))), 0.0);
        assert_eq!(target(true, Some((false, 0.0))), 1.0);
    }

    #[test]
    fn halfway_is_halfway_warm() {
        let [red, green, blue] = ramps(256, 0.5);
        let [_, full_green, full_blue] = ramps(256, 1.0);
        assert_eq!(red[255], 65535);
        assert!(green[255] > full_green[255] && blue[255] > full_blue[255]);
    }

    #[test]
    fn off_is_the_identity() {
        let [red, green, blue] = ramps(256, 0.0);
        for ramp in [red, green, blue] {
            assert_eq!((ramp[0], ramp[255]), (0, 65535));
            assert!(ramp.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn on_keeps_red_and_takes_some_blue_never_darkening_much() {
        let [red, green, blue] = ramps(256, 1.0);
        assert_eq!(red[255], 65535);
        assert!(green[255] < red[255] && blue[255] < green[255]);
        assert!(
            blue[255] as f64 > 0.75 * 65535.0,
            "a warm tint, not a dim screen"
        );
    }
}

//! How long the battery lasts, or how long until it's full, steadied so the estimate doesn't jump
//! with every change in load. The kernel's rate is instantaneous: a compile or a video call can
//! halve an estimate for a few seconds. The rate is averaged over a few minutes instead, and the
//! average starts over whenever the battery turns from charging to discharging or back.

/// How quickly the averaged rate follows the kernel's: about two thirds of the way in this long.
const STEADY_SECS: f64 = 180.0;
/// After a charger is plugged in or pulled out, no estimate for this long: a charger ramps up,
/// and the first readings say little.
const SETTLE_SECS: f64 = 15.0;
/// Estimates longer than this are nonsense, usually a rate near zero.
const LONGEST_HOURS: f64 = 48.0;

#[derive(Debug, Clone, Default)]
pub struct Estimate {
    /// The battery's status at the last reading; empty before the first.
    status: String,
    /// The averaged rate, in the battery's own units.
    rate: Option<f64>,
    /// When the averaged rate last took a reading.
    at: f64,
    /// No estimate is given before this.
    settled_at: f64,
}

impl Estimate {
    /// Takes a reading at `now`, in seconds on any steady clock: the battery's status
    /// (`Charging`, `Discharging`, ...), how much it holds now and when full, and the rate the
    /// kernel reports, all in the battery's units (µWh and µW, or µAh and µA). Returns the hours
    /// until empty while discharging, or until full while charging.
    pub fn update(
        &mut self,
        status: &str,
        amount: Option<f64>,
        full: Option<f64>,
        rate: Option<f64>,
        now: f64,
    ) -> Option<f64> {
        if status != self.status {
            // The first reading has nothing to settle from, so it counts straight away.
            self.settled_at = if self.status.is_empty() {
                now
            } else {
                now + SETTLE_SECS
            };
            self.status = status.to_string();
            self.rate = None;
        }
        // Some firmware reports the rate out of the battery as negative, and some reports zero
        // now and then; a zero says nothing about the load.
        if let Some(rate) = rate.map(f64::abs).filter(|rate| *rate > 0.0) {
            self.rate = Some(match self.rate {
                Some(average) if self.at >= self.settled_at => {
                    let weight = 1.0 - (-(now - self.at).max(0.0) / STEADY_SECS).exp();
                    average + weight * (rate - average)
                }
                // While settling, and at the first reading after, the latest reading stands in,
                // so the average starts from where the charger has got to rather than from its
                // first moment.
                _ => rate,
            });
            self.at = now;
        }
        if now < self.settled_at {
            return None;
        }
        let rate = self.rate?;
        let hours = match status {
            "Discharging" => amount? / rate,
            "Charging" => (full? - amount?).max(0.0) / rate,
            _ => return None,
        };
        (hours.is_finite() && hours > 0.0 && hours <= LONGEST_HOURS).then_some(hours)
    }
}

/// `hours` as `3 h 10 min`: to the minute under an hour, and to five minutes over, so a long
/// estimate doesn't tick over for nothing.
pub fn duration(hours: f64) -> String {
    let mut minutes = (hours * 60.0).round() as u64;
    if minutes > 60 {
        minutes = (minutes + 2) / 5 * 5;
    }
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A battery holding 30 of 60 units, discharging, read every 2 s with the rates given.
    fn discharging(estimate: &mut Estimate, start: f64, rates: &[f64]) -> Vec<Option<f64>> {
        rates
            .iter()
            .enumerate()
            .map(|(i, rate)| {
                estimate.update(
                    "Discharging",
                    Some(30.0),
                    Some(60.0),
                    Some(*rate),
                    start + 2.0 * i as f64,
                )
            })
            .collect()
    }

    #[test]
    fn the_first_reading_gives_an_estimate_straight_away() {
        let mut estimate = Estimate::default();
        assert_eq!(discharging(&mut estimate, 0.0, &[10.0]), vec![Some(3.0)]);
    }

    #[test]
    fn a_burst_of_load_barely_moves_the_estimate() {
        let mut estimate = Estimate::default();
        discharging(&mut estimate, 0.0, &[10.0; 30]);
        // Ten seconds at double the drain.
        let hours = discharging(&mut estimate, 60.0, &[20.0; 5]);
        let last = hours.last().unwrap().unwrap();
        assert!(last > 2.8, "{last} h: the raw rate would say 1.5 h");
        let hours = discharging(&mut estimate, 70.0, &[10.0; 5]);
        assert!(hours.last().unwrap().unwrap() > 2.8);
    }

    #[test]
    fn a_lasting_change_in_load_comes_through_in_a_few_minutes() {
        let mut estimate = Estimate::default();
        discharging(&mut estimate, 0.0, &[10.0; 30]);
        let hours = discharging(&mut estimate, 60.0, &[20.0; 300]);
        let last = hours.last().unwrap().unwrap();
        assert!((last - 1.5).abs() < 0.1, "{last} h after ten minutes");
    }

    #[test]
    fn a_charger_starts_the_estimate_over_after_settling() {
        let mut estimate = Estimate::default();
        discharging(&mut estimate, 0.0, &[10.0; 30]);
        let charging = |estimate: &mut Estimate, at: f64, rate: f64| {
            estimate.update("Charging", Some(30.0), Some(60.0), Some(rate), at)
        };
        assert_eq!(charging(&mut estimate, 60.0, 5.0), None, "settling");
        assert_eq!(charging(&mut estimate, 70.0, 15.0), None, "settling");
        // Settled: from the latest rate, not the average across the ramp.
        assert_eq!(charging(&mut estimate, 76.0, 30.0), Some(1.0));
    }

    #[test]
    fn zero_rates_and_full_batteries_give_nothing() {
        let mut estimate = Estimate::default();
        assert_eq!(
            estimate.update("Discharging", Some(30.0), Some(60.0), Some(0.0), 0.0),
            None
        );
        assert_eq!(
            estimate.update("Full", Some(60.0), Some(60.0), Some(1.0), 0.0),
            None
        );
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(3.0 + 10.0 / 60.0), "3 h 10 min");
        assert_eq!(duration(3.0 + 12.0 / 60.0), "3 h 10 min");
        assert_eq!(duration(3.0 + 13.0 / 60.0), "3 h 15 min");
        assert_eq!(duration(2.0), "2 h");
        assert_eq!(duration(0.75), "45 min");
        assert_eq!(duration(47.0 / 60.0), "47 min");
        assert_eq!(duration(1.0), "1 h");
    }
}

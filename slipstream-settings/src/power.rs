//! The battery and chargers, as the kernel reports them under `/sys/class/power_supply`.

use std::{collections::HashMap, fs, path::Path};

const SUPPLIES: &str = "/sys/class/power_supply";

/// One power supply's attributes, by file name.
pub type Supply = HashMap<String, String>;

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub percent: u8,
    /// The kernel's status: `Charging`, `Discharging`, `Full` or `Not charging`.
    pub status: String,
    /// Power flowing in while charging, or out while running on the battery.
    pub watts: Option<f64>,
    /// Hours until full while charging, or until empty while discharging.
    pub hours: Option<f64>,
    /// How much a full charge holds now, as a percentage of what it held new.
    pub health: Option<u32>,
    /// What a full charge holds now, and held new, in watt-hours.
    pub capacity_wh: Option<(f64, f64)>,
    pub cycles: Option<u32>,
    /// The percentage the firmware stops charging at, when it's below 100.
    pub limit: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub battery: Option<Battery>,
    /// Whether a charger is plugged in.
    pub plugged: bool,
    /// The charger's rating in watts, where the kernel knows it (USB power delivery does).
    pub charger_watts: Option<f64>,
}

impl Battery {
    pub fn charging(&self) -> bool {
        self.status == "Charging"
    }
}

/// Reads every supply on this machine.
pub fn read() -> Reading {
    reading(&supplies(Path::new(SUPPLIES)))
}

fn supplies(root: &Path) -> Vec<Supply> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| {
            fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter(|file| file.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|file| {
                    let value = fs::read_to_string(file.path()).ok()?;
                    Some((
                        file.file_name().to_string_lossy().into_owned(),
                        value.trim().to_string(),
                    ))
                })
                .collect()
        })
        .collect()
}

fn reading(supplies: &[Supply]) -> Reading {
    // A wireless mouse's or headset's battery has the scope "Device"; it powers nothing here.
    let system = |supply: &&Supply| supply.get("scope").is_none_or(|scope| scope != "Device");
    let battery = supplies
        .iter()
        .filter(system)
        .find(|supply| supply.get("type").is_some_and(|kind| kind == "Battery"))
        .and_then(battery);
    let chargers: Vec<&Supply> = supplies
        .iter()
        .filter(system)
        .filter(|supply| {
            supply.get("type").is_some_and(|kind| kind != "Battery")
                && supply.get("online").is_some_and(|online| online == "1")
        })
        .collect();
    let charger_watts = chargers
        .iter()
        .filter_map(|charger| {
            Some(number(charger, "voltage_now")? * number(charger, "current_now")?)
        })
        .map(|microwatts| microwatts / 1e12)
        .filter(|watts| *watts >= 1.0)
        .reduce(f64::max);
    let plugged = !chargers.is_empty()
        || battery.as_ref().is_some_and(|battery| {
            matches!(
                battery.status.as_str(),
                "Charging" | "Full" | "Not charging"
            )
        });
    Reading {
        battery,
        plugged,
        charger_watts,
    }
}

fn number(supply: &Supply, name: &str) -> Option<f64> {
    supply.get(name)?.parse().ok()
}

fn battery(supply: &Supply) -> Option<Battery> {
    let status = supply.get("status").cloned().unwrap_or_default();
    // Batteries report either energy (µWh, µW) or charge (µAh, µA). Either way, the ratio of an
    // amount to its rate is hours.
    let by_energy = supply.contains_key("energy_now");
    let (now, full, design, rate) = if by_energy {
        (
            number(supply, "energy_now"),
            number(supply, "energy_full"),
            number(supply, "energy_full_design"),
            number(supply, "power_now"),
        )
    } else {
        (
            number(supply, "charge_now"),
            number(supply, "charge_full"),
            number(supply, "charge_full_design"),
            number(supply, "current_now"),
        )
    };
    // Some firmware reports the rate out of the battery as negative.
    let rate = rate.map(f64::abs).filter(|rate| *rate > 0.0);
    let voltage = number(supply, "voltage_now");
    // Each detail is left out on its own when the battery doesn't report what it needs.
    let watts = (|| {
        if by_energy {
            Some(rate? / 1e6)
        } else {
            Some(rate? * voltage? / 1e12)
        }
    })();
    let percent = number(supply, "capacity")
        .or_else(|| Some(100.0 * now? / full?))?
        .round()
        .clamp(0.0, 100.0) as u8;
    let hours = (|| match status.as_str() {
        "Discharging" => Some(now? / rate?),
        "Charging" => Some((full? - now?).max(0.0) / rate?),
        _ => None,
    })()
    .filter(|hours| hours.is_finite() && *hours > 0.0 && *hours <= 48.0);
    let health = (|| Some((100.0 * full? / design?).round() as u32))().filter(|health| *health > 0);
    // Charge becomes energy through the battery's nominal voltage.
    let capacity_wh = (|| {
        let scale = if by_energy {
            1e6
        } else {
            1e12 / number(supply, "voltage_min_design")?
        };
        Some((full? / scale, design? / scale))
    })();
    Some(Battery {
        percent,
        watts: watts.filter(|watts| *watts >= 0.05),
        hours,
        health,
        capacity_wh,
        cycles: number(supply, "cycle_count")
            .map(|cycles| cycles as u32)
            .filter(|cycles| *cycles > 0),
        limit: number(supply, "charge_control_end_threshold")
            .map(|limit| limit as u8)
            .filter(|limit| *limit < 100),
        status,
    })
}

/// `hours` as `3 h 10 min`.
pub fn duration(hours: f64) -> String {
    let minutes = (hours * 60.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(pairs: &[(&str, &str)]) -> Supply {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn charging_by_charge() -> Vec<Supply> {
        vec![
            supply(&[("type", "Mains"), ("online", "1")]),
            supply(&[
                ("type", "Battery"),
                ("status", "Charging"),
                ("capacity", "26"),
                ("charge_now", "1452000"),
                ("charge_full", "5630000"),
                ("charge_full_design", "5570000"),
                ("current_now", "1793000"),
                ("voltage_now", "11635000"),
                ("cycle_count", "65"),
            ]),
            supply(&[
                ("type", "USB"),
                ("online", "1"),
                ("current_now", "2750000"),
                ("voltage_now", "20000000"),
            ]),
        ]
    }

    #[test]
    fn a_battery_charging_by_charge_reads_in_watts_and_hours() {
        let reading = reading(&charging_by_charge());
        assert!(reading.plugged);
        assert_eq!(reading.charger_watts, Some(55.0));
        let battery = reading.battery.unwrap();
        assert_eq!(battery.percent, 26);
        assert!(battery.charging());
        assert!((battery.watts.unwrap() - 20.86).abs() < 0.01);
        assert_eq!(duration(battery.hours.unwrap()), "2 h 20 min");
        assert_eq!(battery.health, Some(101));
        assert_eq!(battery.cycles, Some(65));
        // Charge needs a nominal voltage to become watt-hours, and this one gives none.
        assert_eq!(battery.capacity_wh, None);
        assert_eq!(battery.limit, None);
    }

    #[test]
    fn a_battery_discharging_by_energy_counts_down() {
        let reading = reading(&[supply(&[
            ("type", "Battery"),
            ("status", "Discharging"),
            ("energy_now", "30000000"),
            ("energy_full", "50000000"),
            ("energy_full_design", "57000000"),
            ("power_now", "-10000000"),
            ("charge_control_end_threshold", "80"),
        ])]);
        assert!(!reading.plugged);
        let battery = reading.battery.unwrap();
        assert_eq!(battery.percent, 60);
        assert_eq!(battery.watts, Some(10.0));
        assert_eq!(duration(battery.hours.unwrap()), "3 h");
        assert_eq!(battery.health, Some(88));
        assert_eq!(battery.capacity_wh, Some((50.0, 57.0)));
        assert_eq!(battery.limit, Some(80));
    }

    #[test]
    fn a_battery_held_at_its_limit_is_plugged_in_without_a_charger_listed() {
        let reading = reading(&[supply(&[
            ("type", "Battery"),
            ("status", "Not charging"),
            ("capacity", "80"),
        ])]);
        assert!(reading.plugged);
        assert_eq!(reading.battery.unwrap().hours, None);
    }

    #[test]
    fn a_mouse_battery_is_not_the_laptop_battery() {
        let reading = reading(&[supply(&[
            ("type", "Battery"),
            ("scope", "Device"),
            ("capacity", "40"),
            ("status", "Discharging"),
        ])]);
        assert_eq!(reading.battery, None);
    }
}

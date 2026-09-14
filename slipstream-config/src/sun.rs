//! Sunrise and sunset for night light's schedule, worked out without asking where you are: the
//! time zone's own coordinates, from the tz database's `zone1970.tab`, are close enough for a
//! warm screen to arrive around dusk.

use std::path::Path;

/// Where tzdata lists each zone's principal city.
const ZONE_TABLE: &str = "/usr/share/zoneinfo/zone1970.tab";

/// The zone's name, `Europe/London`: from `TZ`, or from where `/etc/localtime` points.
pub fn zone_name() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':');
        if tz.contains('/') && !tz.starts_with('/') {
            return Some(tz.to_string());
        }
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    zone_from_path(&target)
}

/// `Europe/London` from a path ending `…/zoneinfo/Europe/London`.
pub fn zone_from_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let at = text.find("zoneinfo/")?;
    let zone = &text[at + "zoneinfo/".len()..];
    (!zone.is_empty()).then(|| zone.to_string())
}

/// The latitude and longitude of `zone`'s city, in degrees, from the table's text.
pub fn location_in(table: &str, zone: &str) -> Option<(f64, f64)> {
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split('\t');
            let _countries = fields.next()?;
            let coordinates = fields.next()?;
            (fields.next()? == zone).then(|| coordinates_of(coordinates))?
        })
}

/// This machine's time zone's location, if the table has it.
pub fn location() -> Option<(f64, f64)> {
    let zone = zone_name()?;
    let table = std::fs::read_to_string(ZONE_TABLE).ok()?;
    location_in(&table, &zone)
}

/// `+513030-0000731` as degrees: ±DDMM[SS] then ±DDDMM[SS].
fn coordinates_of(text: &str) -> Option<(f64, f64)> {
    let split = text[1..].find(['+', '-'])? + 1;
    let (lat, lon) = text.split_at(split);
    Some((angle(lat, 2)?, angle(lon, 3)?))
}

fn angle(text: &str, degree_digits: usize) -> Option<f64> {
    let sign = match text.chars().next()? {
        '+' => 1.0,
        '-' => -1.0,
        _ => return None,
    };
    let digits = &text[1..];
    let part = |from: usize, to: usize| -> Option<f64> {
        digits
            .get(from..to)
            .map_or(Some(0.0), |part| part.parse().ok())
    };
    let degrees = part(0, degree_digits)?;
    let minutes = part(degree_digits, degree_digits + 2)?;
    let seconds = if digits.len() > degree_digits + 2 {
        part(degree_digits + 2, degree_digits + 4)?
    } else {
        0.0
    };
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

/// The day of the year, 1 to 366.
fn day_of_year(year: i64, month: u32, day: u32) -> u32 {
    const BEFORE: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    BEFORE[(month.clamp(1, 12) - 1) as usize] + day + u32::from(leap && month > 2)
}

/// Sunrise and sunset on a date, in local minutes after midnight, for a place `lat`, `lon`
/// degrees and a clock `offset_mins` ahead of UTC. NOAA's approximation, good to within ten minutes.
/// `None` in a polar day or night, when the sun doesn't rise or set.
pub fn sun_times(
    lat: f64,
    lon: f64,
    date: (i64, u32, u32),
    offset_mins: f64,
) -> Option<(f64, f64)> {
    let n = day_of_year(date.0, date.1, date.2) as f64;
    let g = 2.0 * std::f64::consts::PI / 365.0 * (n - 1.0);
    let eqtime = 229.18
        * (0.000_075 + 0.001_868 * g.cos()
            - 0.032_077 * g.sin()
            - 0.014_615 * (2.0 * g).cos()
            - 0.040_849 * (2.0 * g).sin());
    let decl = 0.006_918 - 0.399_912 * g.cos() + 0.070_257 * g.sin() - 0.006_758 * (2.0 * g).cos()
        + 0.000_907 * (2.0 * g).sin()
        - 0.002_697 * (3.0 * g).cos()
        + 0.001_48 * (3.0 * g).sin();
    let lat = lat.to_radians();
    let cos_ha = 90.833_f64.to_radians().cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None;
    }
    let ha = cos_ha.acos().to_degrees();
    let wrap = |minutes: f64| minutes.rem_euclid(1440.0);
    let sunrise = wrap(720.0 - 4.0 * (lon + ha) - eqtime + offset_mins);
    let sunset = wrap(720.0 - 4.0 * (lon - ha) - eqtime + offset_mins);
    Some((sunrise, sunset))
}

/// `"21:30"` as minutes after midnight.
pub fn parse_time(text: &str) -> Option<f64> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some((hours * 60 + minutes) as f64)
}

/// Minutes after midnight as `"21:30"`.
pub fn format_time(minutes: f64) -> String {
    let minutes = minutes.rem_euclid(1440.0).round() as u32 % 1440;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Night light on a schedule at local minute `now`, with night from `start` to `end` (wrapping
/// past midnight): whether it's night, and how warm, 0 to 1. Warming takes `fade` minutes from
/// `start`, and cooling `fade` minutes from `end`.
pub fn scheduled(now: f64, start: f64, end: f64, fade: f64) -> (bool, f64) {
    let since = |from: f64| (now - from).rem_euclid(1440.0);
    let night_length = (end - start).rem_euclid(1440.0);
    let into_night = since(start);
    let night = into_night < night_length;
    let fade = fade.max(0.0);
    let strength = if night {
        if fade > 0.0 {
            (into_night / fade).min(1.0)
        } else {
            1.0
        }
    } else {
        let into_day = since(end);
        if fade > 0.0 {
            (1.0 - into_day / fade).max(0.0)
        } else {
            0.0
        }
    };
    (night, strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str =
        "# comment\nGB,GG,IM,JE\t+513030-0000731\tEurope/London\nNO,SJ\t+5955+01045\tEurope/Oslo\n";

    #[test]
    fn a_zone_is_found_with_its_coordinates() {
        let (lat, lon) = location_in(TABLE, "Europe/London").unwrap();
        assert!((lat - 51.508).abs() < 0.01 && (lon + 0.125).abs() < 0.01);
        let (lat, lon) = location_in(TABLE, "Europe/Oslo").unwrap();
        assert!((lat - 59.9167).abs() < 0.01 && (lon - 10.75).abs() < 0.01);
        assert_eq!(location_in(TABLE, "Mars/Olympus"), None);
    }

    #[test]
    fn the_zone_comes_from_where_localtime_points() {
        assert_eq!(
            zone_from_path(Path::new("../usr/share/zoneinfo/Europe/London")).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(zone_from_path(Path::new("/etc/localtime")), None);
    }

    #[test]
    fn london_in_mid_september_rises_about_half_six_and_sets_about_quarter_past_seven() {
        // British Summer Time, an hour ahead of UTC.
        // The approximation drifts by a few minutes near an equinox, which is nothing to a warm-up
        // that takes half an hour.
        let (sunrise, sunset) = sun_times(51.508, -0.125, (2026, 9, 14), 60.0).unwrap();
        assert!(
            (sunrise - (6.0 * 60.0 + 33.0)).abs() < 10.0,
            "{}",
            format_time(sunrise)
        );
        assert!(
            (sunset - (19.0 * 60.0 + 13.0)).abs() < 10.0,
            "{}",
            format_time(sunset)
        );
    }

    #[test]
    fn midsummer_at_the_pole_has_no_sunset() {
        assert_eq!(sun_times(89.0, 0.0, (2026, 6, 21), 0.0), None);
    }

    #[test]
    fn times_read_and_write_back() {
        assert_eq!(parse_time("21:30"), Some(1290.0));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(format_time(1290.0), "21:30");
        assert_eq!(format_time(-30.0), "23:30");
    }

    #[test]
    fn a_night_across_midnight_warms_up_and_cools_down() {
        let (start, end) = (21.0 * 60.0, 7.0 * 60.0);
        assert_eq!(scheduled(20.0 * 60.0, start, end, 30.0), (false, 0.0));
        let (night, halfway) = scheduled(21.0 * 60.0 + 15.0, start, end, 30.0);
        assert!(night && (halfway - 0.5).abs() < 1e-9);
        assert_eq!(scheduled(2.0 * 60.0, start, end, 30.0), (true, 1.0));
        let (night, cooling) = scheduled(7.0 * 60.0 + 10.0, start, end, 30.0);
        assert!(!night && (cooling - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(scheduled(12.0 * 60.0, start, end, 30.0), (false, 0.0));
    }
}

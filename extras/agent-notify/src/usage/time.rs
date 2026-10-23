//! Times in the usage replies: ISO 8601 in UTC or with an offset, to seconds since the epoch.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// Reads a number of at most `width` digits, after any spaces, as C's strptime does: it stops
/// early once another digit would take it past `max`.
fn number(text: &str, width: usize, min: i64, max: i64) -> Option<(i64, &str)> {
    let text = text.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let digits = text.as_bytes();
    if !digits.first()?.is_ascii_digit() {
        return None;
    }
    let mut value = 0;
    let mut used = 0;
    loop {
        value = value * 10 + i64::from(digits[used] - b'0');
        used += 1;
        if used == width || value * 10 > max || !digits.get(used).is_some_and(u8::is_ascii_digit) {
            break;
        }
    }
    (min..=max).contains(&value).then(|| (value, &text[used..]))
}

/// Days from 1970-01-01 to a date, months and days past their ends carrying over.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `YYYY-MM-DDTHH:MM:SSZ`, exactly, as seconds since the epoch.
pub fn utc(text: &str) -> Option<i64> {
    let (year, rest) = number(text, 4, 0, 9999)?;
    let (month, rest) = number(rest.strip_prefix('-')?, 2, 1, 12)?;
    let (day, rest) = number(rest.strip_prefix('-')?, 2, 1, 31)?;
    let (hour, rest) = number(rest.strip_prefix('T')?, 2, 0, 23)?;
    let (minute, rest) = number(rest.strip_prefix(':')?, 2, 0, 59)?;
    let (second, rest) = number(rest.strip_prefix(':')?, 2, 0, 61)?;
    if rest != "Z" {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

/// How a reset time reads.
pub enum Reset {
    /// Not given as text: no time.
    None,
    At(i64),
    /// Text that isn't a date and time at all: the meter is left out.
    Unreadable,
}

/// A date and time with optional fractional seconds and any UTC offset (`Z`, `+01:00`, or
/// nothing, which is taken as UTC). Err for one shaped like a date that isn't a real one, which
/// makes the whole reply unreadable.
pub fn reset(value: &serde_json::Value) -> Result<Reset, ()> {
    let Some(text) = value.as_str() else {
        return Ok(Reset::None);
    };
    let Some((date, time)) = text.split_once('T') else {
        return Ok(Reset::Unreadable);
    };
    if date.is_empty()
        || !date
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        return Ok(Reset::Unreadable);
    }
    let clock_end = time
        .find(|ch: char| !(ch.is_ascii_digit() || ch == ':'))
        .unwrap_or(time.len());
    if clock_end == 0 {
        return Ok(Reset::Unreadable);
    }
    let (clock, mut zone) = time.split_at(clock_end);
    if let Some(fraction) = zone.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            zone = &fraction[digits..];
        }
    }
    if zone.contains('\n') {
        return Ok(Reset::Unreadable);
    }
    let at = utc(&format!("{date}T{clock}Z")).ok_or(())?;
    let offset = match zone.as_bytes() {
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()) =>
        {
            let digit = |byte: &u8| i64::from(byte - b'0');
            let secs = (digit(h1) * 10 + digit(h2)) * 3600 + (digit(m1) * 10 + digit(m2)) * 60;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => 0,
    };
    Ok(Reset::At(at - offset))
}

/// A time of day here, as HH:MM.
pub fn local_clock(epoch: i64) -> String {
    // SAFETY: localtime_r reads the time zone and fills a struct owned here.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = epoch as libc::time_t;
        if libc::localtime_r(&time, &mut tm).is_null() {
            return String::from("--:--");
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utc_times_read_as_strptime_reads_them() {
        assert_eq!(utc("2026-10-23T14:05:00Z"), Some(1_792_764_300));
        assert_eq!(utc("2026-1-3T4:5:6Z"), Some(1_767_413_106));
        assert_eq!(
            utc("2026-02-30T00:00:00Z"),
            Some(1_772_409_600),
            "carried into March"
        );
        assert_eq!(utc("2026-10-23T14:05:00.5Z"), None);
        assert_eq!(utc("2026-10-23T14:05:00"), None);
        assert_eq!(utc("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn a_reset_time_takes_its_offset_into_account() {
        let at = |text: &str| match reset(&json!(text)) {
            Ok(Reset::At(at)) => Some(at),
            _ => None,
        };
        assert_eq!(at("2026-10-23T14:05:00.123456Z"), Some(1_792_764_300));
        assert_eq!(at("2026-10-23T15:05:00+01:00"), Some(1_792_764_300));
        assert_eq!(at("2026-10-23T09:05:00-05:00"), Some(1_792_764_300));
        assert_eq!(
            at("2026-10-23T14:05:00"),
            Some(1_792_764_300),
            "no zone is UTC"
        );
        assert!(matches!(reset(&json!(null)), Ok(Reset::None)));
        assert!(matches!(reset(&json!("tomorrow")), Ok(Reset::Unreadable)));
        assert!(
            reset(&json!("2026-10-23T14:05")).is_err(),
            "a date without seconds isn't one"
        );
    }
}

//! The local date and time: in the time zone the system has now, written out in the locale's own
//! words for days and months, as `date` would.

use std::{ffi::CStr, ptr, time::SystemTime};

unsafe extern "C" {
    /// Reads the time zone again, from `TZ` or the system's own. `localtime_r` uses the one read
    /// last, so without it a change of time zone would only show after a restart.
    fn tzset();
}

/// `unix`, in seconds since the Unix epoch, taken apart in the local time zone as it is now.
pub fn at(unix: i64) -> Option<libc::tm> {
    let time = unix as libc::time_t;
    // SAFETY: `tzset` takes nothing and only reads the time zone; `localtime_r` only reads the
    // time it's given and writes the `tm` it's given.
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&time, &mut tm).is_null()).then_some(tm)
    }
}

/// Now, taken apart in the local time zone.
pub fn now() -> Option<libc::tm> {
    let unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    at(unix as i64)
}

/// `tm` written out by `strftime`'s `pattern`, with day and month names in the locale the
/// environment chooses for times (`LC_ALL`, `LC_TIME`, then `LANG`). `None` if it comes out empty.
pub fn format(tm: &libc::tm, pattern: &CStr) -> Option<String> {
    format_in(tm, pattern, c"")
}

fn format_in(tm: &libc::tm, pattern: &CStr, locale: &CStr) -> Option<String> {
    // SAFETY: `newlocale` only reads the name it's given and the environment, and returns null
    // for a locale that isn't installed, which falls back to "C" as `date` does.
    let names = unsafe {
        let names = libc::newlocale(libc::LC_TIME_MASK, locale.as_ptr(), ptr::null_mut());
        if names.is_null() {
            libc::newlocale(libc::LC_TIME_MASK, c"C".as_ptr(), ptr::null_mut())
        } else {
            names
        }
    };
    if names.is_null() {
        return None;
    }
    let mut text = [0u8; 128];
    // SAFETY: `strftime_l` writes at most `text.len()` bytes into `text`, and `names` is a live
    // locale, freed once here.
    let length = unsafe {
        let length = libc::strftime_l(
            text.as_mut_ptr().cast(),
            text.len(),
            pattern.as_ptr(),
            tm,
            names,
        );
        libc::freelocale(names);
        length
    };
    (length > 0).then(|| String::from_utf8_lossy(&text[..length]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_s_time_and_date_are_written_out_like_date_does() {
        // SAFETY: an all-zero `tm` is a valid value; the fields used are set below.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = 2026 - 1900;
        tm.tm_mon = 8;
        tm.tm_mday = 4;
        tm.tm_wday = 5;
        tm.tm_hour = 7;
        tm.tm_min = 5;
        assert_eq!(
            format_in(&tm, c"%H:%M|%a %-d %b", c"C").as_deref(),
            Some("07:05|Fri 4 Sep")
        );
        assert_eq!(
            format_in(&tm, c"%a", c"no_SUCH.locale").as_deref(),
            Some("Fri"),
            "an unknown locale falls back to C"
        );
    }

    #[test]
    fn now_is_a_real_date() {
        let tm = now().unwrap();
        assert!(tm.tm_year + 1900 >= 2026 && (0..12).contains(&tm.tm_mon));
    }
}

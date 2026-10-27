//! What Focus says: how long things took, which apps the time went to, the line each session adds
//! to the log, and how a search's words are read as minutes.

use crate::timer::{Finished, Kind, MOST_MINUTES};

/// The minutes a search asks for: a number, with `m`, `min` or `minutes` after it if you like.
pub fn minutes(words: &str) -> Option<i64> {
    let words = words.trim().to_lowercase();
    let digits = words.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ' ');
    let unit = words[digits.len()..].trim();
    if !["", "m", "min", "mins", "minute", "minutes"].contains(&unit) {
        return None;
    }
    let minutes: i64 = digits.trim().parse().ok()?;
    (1..=MOST_MINUTES).contains(&minutes).then_some(minutes)
}

/// `secs` as the card says a stretch of time: `1 h 5 min`, `25 min`, `under a minute`.
pub fn duration(secs: i64) -> String {
    let minutes = (secs + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The time left before `ends` at `now`, rounded up, as the chip shows it: `18 min`.
pub fn left(ends: i64, now: i64) -> String {
    let minutes = ((ends - now).max(0) + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// An app's id as a name: the last part of `org.mozilla.firefox`, with a capital, so `Firefox`;
/// `code` is `Code`.
pub fn app_name(id: &str) -> String {
    // An app chooses its own id, so nothing in it may start a line of the log.
    let id: String = id.chars().filter(|c| !c.is_control()).collect();
    let id = id.as_str();
    let last = id.rsplit('.').next().unwrap_or(id).replace(['-', '_'], " ");
    let mut chars = last.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => id.to_string(),
    }
}

/// The two apps that had the keyboard longest, by name, as `Code and Firefox`. An app with less
/// than a minute isn't named.
pub fn top_apps(apps: &[(String, i64)]) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    for (app, secs) in apps {
        let name = app_name(app);
        if *secs >= 60 && !names.contains(&name) {
            names.push(name);
        }
    }
    match names.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [one, two, ..] => Some(format!("{one} and {two}")),
    }
}

/// A focus session in a few words: `25 minutes, mostly in Code and Firefox`, or for one stopped
/// early, `12 of 25 minutes, mostly in Code`.
pub fn summary(finished: &Finished) -> String {
    let spent = (finished.spent + 30) / 60;
    let planned = (finished.planned + 30) / 60;
    let unit = if planned == 1 { "minute" } else { "minutes" };
    let time = if finished.early {
        format!("{spent} of {planned} {unit}")
    } else if spent == 1 {
        "1 minute".to_string()
    } else {
        format!("{spent} minutes")
    };
    match top_apps(&finished.apps) {
        Some(apps) => format!("{time}, mostly in {apps}"),
        None => time,
    }
}

/// A moment as the person's clock and calendar show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    /// `2026-10-27`
    pub date: String,
    /// `14:10`
    pub time: String,
}

/// `unix`, in seconds since the Unix epoch, in the local time zone.
pub fn local(unix: i64) -> Local {
    // SAFETY: `localtime_r` only reads the time it's given and writes the `tm` it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = unix as libc::time_t;
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return Local {
            date: "?".into(),
            time: "?".into(),
        };
    }
    Local {
        date: format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        time: format!("{:02}:{:02}", tm.tm_hour, tm.tm_min),
    }
}

/// The line a finished focus session adds to the log, started at `when`:
/// `- 2026-10-27 14:10, 25 min, Code and Firefox`. None for a break, or a focus session of under
/// a minute, which isn't worth a line.
pub fn log_line(finished: &Finished, when: &Local) -> Option<String> {
    if finished.kind != Kind::Focus || finished.spent < 60 {
        return None;
    }
    let spent = (finished.spent + 30) / 60;
    let mut line = format!("- {} {}, ", when.date, when.time);
    if finished.early {
        let planned = (finished.planned + 30) / 60;
        line.push_str(&format!("{spent} of {planned} min, stopped early"));
    } else {
        line.push_str(&format!("{spent} min"));
    }
    if let Some(apps) = top_apps(&finished.apps) {
        line.push_str(&format!(", {apps}"));
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(spent: i64, planned: i64, apps: &[(&str, i64)]) -> Finished {
        Finished {
            kind: Kind::Focus,
            started: 0,
            planned,
            spent,
            early: spent < planned,
            unseen: false,
            apps: apps
                .iter()
                .map(|(app, secs)| (app.to_string(), *secs))
                .collect(),
        }
    }

    #[test]
    fn numbers_in_a_search_are_minutes() {
        for (words, said) in [
            ("45", Some(45)),
            (" 45 ", Some(45)),
            ("45m", Some(45)),
            ("45 min", Some(45)),
            ("1 minute", Some(1)),
            ("90 Minutes", Some(90)),
            ("600", Some(600)),
            ("", None),
            ("0", None),
            ("601", None),
            ("-5", None),
            ("4.5", None),
            ("45 hours", None),
            ("tea", None),
        ] {
            assert_eq!(minutes(words), said, "{words:?}");
        }
    }

    #[test]
    fn an_app_s_id_reads_as_its_name() {
        assert_eq!(app_name("org.mozilla.firefox"), "Firefox");
        assert_eq!(app_name("code"), "Code");
        assert_eq!(app_name("com.visualstudio.code"), "Code");
        assert_eq!(app_name("org.gnome.Nautilus"), "Nautilus");
        assert_eq!(app_name("gnome-text-editor"), "Gnome text editor");
        assert!(!app_name("org.example.App\n- 2026-10-27 09:00, 25 min").contains('\n'));
    }

    #[test]
    fn a_session_is_summed_up_by_its_time_and_its_top_two_apps() {
        let apps = [("code", 900), ("org.mozilla.firefox", 400), ("foot", 200)];
        assert_eq!(
            summary(&finished(1500, 1500, &apps)),
            "25 minutes, mostly in Code and Firefox"
        );
        assert_eq!(
            summary(&finished(720, 1500, &[("code", 700), ("foot", 20)])),
            "12 of 25 minutes, mostly in Code",
            "under a minute isn't named"
        );
        assert_eq!(summary(&finished(60, 60, &[])), "1 minute");
        // Two ids with the same name are one app.
        let same = [("code", 900), ("com.visualstudio.code", 300), ("foot", 200)];
        assert_eq!(
            summary(&finished(1500, 1500, &same)),
            "25 minutes, mostly in Code and Foot"
        );
    }

    #[test]
    fn the_log_gets_a_line_for_each_focus_session_worth_one() {
        let when = Local {
            date: "2026-10-27".into(),
            time: "14:10".into(),
        };
        let apps = [("code", 900), ("org.mozilla.firefox", 400)];
        assert_eq!(
            log_line(&finished(1500, 1500, &apps), &when).as_deref(),
            Some("- 2026-10-27 14:10, 25 min, Code and Firefox")
        );
        assert_eq!(
            log_line(&finished(720, 1500, &[]), &when).as_deref(),
            Some("- 2026-10-27 14:10, 12 of 25 min, stopped early")
        );
        assert_eq!(log_line(&finished(40, 1500, &[]), &when), None);
        let rest = Finished {
            kind: Kind::Break,
            ..finished(300, 300, &[])
        };
        assert_eq!(log_line(&rest, &when), None);
    }

    #[test]
    fn times_read_as_the_card_says_them() {
        assert_eq!(duration(20), "under a minute");
        assert_eq!(duration(25 * 60), "25 min");
        assert_eq!(duration(65 * 60), "1 h 5 min");
        assert_eq!(left(1000, 1000 - 17 * 60 - 1), "18 min");
        assert_eq!(left(1000, 2000), "0 min");
        assert_eq!(left(7200, 0), "2 h");
    }

    #[test]
    fn the_local_time_has_a_date_and_a_clock() {
        let now = local(1_793_121_600);
        assert_eq!(now.date.len(), 10);
        assert_eq!(now.time.len(), 5);
    }
}

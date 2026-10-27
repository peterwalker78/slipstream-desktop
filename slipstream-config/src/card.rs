//! A plugin card's sections: named groups of gauges, each saying how much of some allowance is
//! used, as the card's `sections` give them (`plugins::Card` reads the rest of a card).
//!
//! ```json
//! {"sections": [{"name": "Storage", "detail": "Pro", "note": null, "stale": false,
//!                "meters": [{"label": "Daily", "percent": 42, "resets": 1790000000}]}]}
//! ```
//!
//! `resets` is when that allowance starts over, in seconds since the Unix epoch, and `window` how
//! long it lasts, in seconds, so that how far through it is can be shown. `detail`, `note`,
//! `stale`, `resets` and `window` are optional; `stale` dims a section whose numbers are old. A
//! meter without a `percent` measures nothing used, and shows only its time, as a timer does.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// A plugin's program still running after this long is stopped.
pub const RUN_LIMIT: Duration = Duration::from_secs(30);

/// What a report shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub detail: Option<String>,
    pub gauges: Vec<Gauge>,
    /// Why the numbers are what they are, e.g. that they couldn't be refreshed.
    pub note: Option<String>,
    /// The numbers are old: the card says so, or its program has stopped answering.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    pub label: String,
    pub percent: u8,
    /// It says how much is used: a gauge without a percent shows only its time.
    pub measured: bool,
    /// How long until it starts over, e.g. `2 h 5 min`.
    pub resets_in: Option<String>,
    /// How far through its window it is, from 0 to 1, when the report says how long that is.
    pub through: Option<f32>,
}

/// The sections as a plugin printed them.
#[derive(Debug, Deserialize)]
pub struct Report {
    #[serde(default)]
    sections: Vec<ReportSection>,
}

#[derive(Debug, Deserialize)]
struct ReportSection {
    name: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    meters: Vec<ReportMeter>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    stale: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ReportMeter {
    label: String,
    #[serde(default)]
    percent: Option<f64>,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    resets: Option<f64>,
    /// How long the allowance lasts, in seconds.
    #[serde(default)]
    window: Option<f64>,
}

impl Report {
    /// The report in `text`, or why it isn't one.
    pub fn parse(text: &str) -> Result<Report, String> {
        serde_json::from_str(text.trim()).map_err(|err| err.to_string())
    }

    /// What it shows at `now`, in seconds since the Unix epoch. `failing` marks every section old.
    pub fn reading(&self, failing: bool, now: i64) -> Reading {
        Reading {
            sections: self
                .sections
                .iter()
                .map(|section| Section {
                    name: section.name.clone(),
                    detail: section.detail.clone().filter(|detail| !detail.is_empty()),
                    gauges: section
                        .meters
                        .iter()
                        .map(|meter| Gauge {
                            label: meter.label.clone(),
                            percent: match meter.percent {
                                Some(percent) if percent.is_finite() => {
                                    percent.round().clamp(0.0, 100.0) as u8
                                }
                                _ => 0,
                            },
                            measured: meter.percent.is_some(),
                            resets_in: meter
                                .resets
                                .filter(|at| at.is_finite())
                                .and_then(|at| (at as i64).checked_sub(now))
                                .filter(|left| *left > 0)
                                .map(time_left),
                            through: through(meter.resets, meter.window, now),
                        })
                        .collect(),
                    note: section.note.clone().filter(|note| !note.is_empty()),
                    stale: failing || section.stale.unwrap_or(false),
                })
                .collect(),
        }
    }
}

/// How far through a window `window` seconds long that ends at `resets` it is at `now`.
fn through(resets: Option<f64>, window: Option<f64>, now: i64) -> Option<f32> {
    let (resets, window) = (resets?, window?);
    if !resets.is_finite() || !window.is_finite() || window <= 0.0 {
        return None;
    }
    Some((1.0 - (resets - now as f64) / window).clamp(0.0, 1.0) as f32)
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// `5 d 23 h`, `2 h 5 min`, `12 min`, rounded up so it never says a minute less than is left.
pub fn time_left(secs: i64) -> String {
    if secs < 60 {
        return "under a minute".into();
    }
    if secs >= 86_400 {
        let (days, hours) = (secs / 86_400, secs % 86_400 / 3600);
        return if hours == 0 {
            format!("{days} d")
        } else {
            format!("{days} d {hours} h")
        };
    }
    let minutes = (secs + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"{"sections": [
        {"name": "Storage", "detail": "Pro", "note": null, "stale": false, "updated": 1,
         "meters": [{"label": "Daily", "percent": 42.4, "resets": 10000},
                    {"label": "Weekly", "percent": 180, "resets": 1000.5}]},
        {"name": "Mail", "meters": [], "note": "Couldn't reach the server", "stale": true}
    ]}"#;

    #[test]
    fn a_report_reads_with_its_optional_fields_left_out() {
        let report = Report::parse(REPORT).unwrap();
        let shown = report.reading(false, 2800);
        let storage = &shown.sections[0];
        assert_eq!(storage.detail.as_deref(), Some("Pro"));
        assert!(!storage.stale);
        assert_eq!(
            storage.gauges[0],
            Gauge {
                label: "Daily".into(),
                percent: 42,
                measured: true,
                resets_in: Some("2 h".into()),
                through: None,
            }
        );
        assert_eq!(storage.gauges[1].percent, 100, "clamped");
        assert_eq!(storage.gauges[1].resets_in, None, "already past");

        // A day-long window ending 6 hours from now is three quarters through.
        let report = Report::parse(
            r#"{"sections": [{"name": "Day", "meters": [
                {"label": "Daily", "percent": 10, "resets": 21600, "window": 86400},
                {"label": "Odd", "percent": 10, "resets": 21600, "window": 0}]}]}"#,
        )
        .unwrap();
        let day = &report.reading(false, 0).sections[0];
        assert_eq!(day.gauges[0].through, Some(0.75));
        assert_eq!(day.gauges[1].through, None, "no window, no position in it");
        let timer = Report::parse(
            r#"{"sections": [{"name": "Timer", "meters": [
                {"label": "Tea", "resets": 120, "window": 240}]}]}"#,
        )
        .unwrap();
        let tea = &timer.reading(false, 0).sections[0].gauges[0];
        assert!(!tea.measured && tea.percent == 0);
        assert_eq!(tea.through, Some(0.5));
        let mail = &shown.sections[1];
        assert!(mail.stale && mail.gauges.is_empty());
        assert_eq!(mail.detail, None);
        assert_eq!(mail.note.as_deref(), Some("Couldn't reach the server"));

        assert!(
            report
                .reading(true, 0)
                .sections
                .iter()
                .all(|section| section.stale),
            "a failing program dims everything"
        );
        assert!(Report::parse("not json").is_err());
        assert!(
            Report::parse(r#"{"sections": [{"meters": []}]}"#)
                .unwrap_err()
                .contains("name"),
            "a name is needed, and the error says so"
        );
        assert_eq!(Report::parse("{}").unwrap().sections.len(), 0);
    }

    #[test]
    fn a_reset_far_in_the_past_is_no_time_left() {
        let report = Report::parse(
            r#"{"sections": [{"name": "Odd", "meters": [
                {"label": "Old", "percent": 1, "resets": -1e300}]}]}"#,
        )
        .unwrap();
        let odd = &report.reading(false, 1_800_000_000).sections[0];
        assert_eq!(odd.gauges[0].resets_in, None);
    }

    #[test]
    fn time_left_reads_naturally_and_never_short() {
        assert_eq!(time_left(30), "under a minute");
        assert_eq!(time_left(61), "2 min");
        assert_eq!(time_left(3600), "1 h");
        assert_eq!(time_left(2 * 3600 + 4 * 60 + 1), "2 h 5 min");
        assert_eq!(time_left(86_400), "1 d");
        assert_eq!(time_left(5 * 86_400 + 23 * 3600 + 59), "5 d 23 h");
    }
}

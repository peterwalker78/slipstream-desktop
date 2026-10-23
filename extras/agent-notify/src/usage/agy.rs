//! Antigravity: `agy -p /usage`, the CLI's own quota readout.
//!
//! It prints its quotas one to a line, tab-separated: the models, the window and what is left of
//! it, the percentage left, and when it resets. It is run from the usage folder because it keeps
//! its conversations by folder, though this one leaves none.

use std::time::Duration;

use serde_json::{Value, json};

use super::{Usage, now, output_within, rounded, time};

pub fn fetch(usage: &Usage) -> bool {
    let now = now();
    let mut command = usage.env.command("agy");
    command.args(["-p", "/usage"]).current_dir(&usage.dir);
    let printed = output_within(command, Duration::from_secs(60)).unwrap_or_default();
    if parse(&printed, now).is_some_and(|report| usage.write("agy", &report)) {
        return true;
    }
    usage.keep_stale("agy", "Antigravity", "Antigravity didn't say");
    false
}

/// The section for what `/usage` printed; None if it named no quota.
pub fn parse(printed: &str, now: i64) -> Option<Value> {
    let meters: Vec<Value> = printed
        .trim_end_matches('\n')
        .split('\n')
        .filter_map(meter)
        .collect();
    if meters.is_empty() {
        return None;
    }
    Some(json!({
        "name": "Antigravity", "detail": null, "meters": meters,
        "note": null, "stale": false, "updated": now,
    }))
}

/// A quota line as a meter of what is used; None for any other line.
fn meter(line: &str) -> Option<Value> {
    let [models, window, left, resets] = line.split('\t').collect::<Vec<_>>()[..] else {
        return None;
    };
    let models = models
        .strip_suffix(" Models")
        .or_else(|| models.strip_suffix(" models"))
        .unwrap_or(models);
    let window = window.strip_suffix(" Limit Remaining").unwrap_or(window);
    let left: f64 = left.strip_suffix('%').unwrap_or(left).parse().ok()?;
    if !left.is_finite() {
        return None;
    }
    let resets = time::utc(resets).map_or(Value::Null, |at| json!(at));
    let label = format!("{window} · {models}");
    Some(json!({"label": label, "percent": rounded(100.0 - left), "resets": resets}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_quota_line_is_a_meter_of_what_is_used() {
        let printed = "Quota\nGemini Models\tDaily Limit Remaining\t72.5%\t2026-10-24T00:00:00Z\nClaude models\tWeekly Limit Remaining\t100%\tsoon\nnot a quota\n\n";
        let report = parse(printed, 7).unwrap();
        assert_eq!(
            report["meters"],
            json!([
                {"label": "Daily · Gemini", "percent": 28, "resets": 1_792_800_000},
                {"label": "Weekly · Claude", "percent": 0, "resets": null},
            ])
        );
        assert_eq!(report["updated"], 7);
    }

    #[test]
    fn nothing_readable_is_no_report() {
        assert!(parse("Not signed in\n", 0).is_none());
        assert!(parse("a\tb\tlots\tc\n", 0).is_none());
    }
}

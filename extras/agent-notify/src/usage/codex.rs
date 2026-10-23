//! Codex: `account/rateLimits/read` from `codex app-server`, which looks after its own login.
//!
//! The app server answers on stdout while its stdin stays open; it's closed once the answer is in.

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::claude::title;
use super::{Usage, now, rounded};
use crate::payload::{Key, at, first, to_text};

const ASK: &str = concat!(
    r#"{"id":0,"method":"initialize","params":{"clientInfo":{"name":"agent-notify","version":"1"}}}"#,
    "\n",
    r#"{"method":"initialized"}"#,
    "\n",
    r#"{"id":1,"method":"account/rateLimits/read"}"#,
    "\n",
);

/// How long the app server has to answer.
const LIMIT: Duration = Duration::from_secs(30);

pub fn fetch(usage: &Usage) -> bool {
    let now = now();
    let Some(reply) = ask(usage) else {
        usage.keep_stale("codex", "Codex", "Codex didn't answer");
        return false;
    };
    if at(&reply, &[Key("result"), Key("rateLimits")]).is_some() {
        if let Some(report) = report(&reply, now) {
            if usage.write("codex", &report) {
                return true;
            }
        }
    }
    let error = at(&reply, &[Key("error"), Key("message")])
        .map_or_else(|| "no limits in its answer".to_owned(), to_text);
    usage.keep_stale("codex", "Codex", &format!("Codex: {error}"));
    false
}

/// The app server's reply to the request, if it gives one in time.
fn ask(usage: &Usage) -> Option<Value> {
    let mut command = usage.env.command("codex");
    command.args(["-s", "read-only", "-a", "never", "app-server"]);
    let mut server = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = server.stdin.take()?;
    let stdout = server.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_f64) == Some(1.0) {
                let _ = sender.send(message);
                break;
            }
        }
    });
    let _ = stdin.write_all(ASK.as_bytes());
    let reply = receiver.recv_timeout(LIMIT).ok();
    drop(stdin);
    let _ = server.kill();
    let _ = server.wait();
    reply
}

/// "Daily", "Weekly", "5-hour": what a window of this many minutes is called.
fn window(minutes: Option<&Value>) -> Option<String> {
    let Some(minutes) = minutes.filter(|minutes| !minutes.is_null()) else {
        return Some("Limit".to_owned());
    };
    let minutes = minutes.as_f64()?;
    let whole = minutes.trunc() as i64;
    let number = |value: f64| {
        if value.fract() == 0.0 && value.abs() < 1e17 {
            format!("{}", value as i64)
        } else {
            format!("{value}")
        }
    };
    Some(if whole % 1440 == 0 {
        let days = minutes / 1440.0;
        if days == 1.0 {
            "Daily".to_owned()
        } else if days == 7.0 {
            "Weekly".to_owned()
        } else if days == 30.0 || days == 31.0 {
            "Monthly".to_owned()
        } else {
            format!("{}-day", number(days))
        }
    } else if whole % 60 == 0 {
        format!("{}-hour", number(minutes / 60.0))
    } else {
        format!("{}-minute", number(minutes))
    })
}

/// A limit's primary and secondary windows as meters, labelled after `prefix`.
fn meters(limits: &Value, prefix: &str) -> Option<Vec<Value>> {
    if !(limits.is_object() || limits.is_null()) {
        return None;
    }
    let mut meters = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(limit) = limits.get(key).filter(|limit| !limit.is_null()) else {
            continue;
        };
        if !limit.is_object() {
            return None;
        }
        let percent = match at(limit, &[Key("usedPercent")]) {
            None => 0.0,
            Some(percent) => percent.as_f64()?,
        };
        meters.push(json!({
            "label": format!("{prefix}{}", window(limit.get("windowDurationMins"))?),
            "percent": rounded(percent),
            "resets": limit.get("resetsAt").cloned().unwrap_or(Value::Null),
        }));
    }
    Some(meters)
}

/// The section for the app server's reply: its main limits, then any others it lists by ID.
pub fn report(reply: &Value, now: i64) -> Option<Value> {
    let result = reply.get("result").unwrap_or(&Value::Null);
    if !(result.is_object() || result.is_null()) {
        return None;
    }
    let main = result.get("rateLimits").unwrap_or(&Value::Null);
    let mut all = meters(main, "")?;
    let others: Vec<(Value, &Value)> = match at(result, &[Key("rateLimitsByLimitId")]) {
        None => Vec::new(),
        Some(Value::Object(by_id)) => by_id
            .iter()
            .map(|(id, limits)| (json!(id), limits))
            .collect(),
        Some(Value::Array(list)) => list
            .iter()
            .enumerate()
            .map(|(index, limits)| (json!(index), limits))
            .collect(),
        Some(_) => return None,
    };
    let main_id = main.get("limitId").unwrap_or(&Value::Null);
    for (id, limits) in others {
        if &id == main_id {
            continue;
        }
        if !(limits.is_object() || limits.is_null()) {
            return None;
        }
        let name = first(limits, &[&[Key("limitName")], &[Key("limitId")]])
            .map_or_else(|| "null".to_owned(), to_text);
        all.extend(meters(limits, &format!("{name} · "))?);
    }
    let detail = match at(main, &[Key("planType")]) {
        None => Value::Null,
        Some(Value::String(plan)) if plan.is_empty() => Value::Null,
        Some(Value::String(plan)) => json!(title(plan)),
        Some(_) => return None,
    };
    let reached = main
        .get("rateLimitReachedType")
        .is_some_and(|reached| !reached.is_null());
    let mut section = Map::new();
    section.insert("name".into(), json!("Codex"));
    section.insert("detail".into(), detail);
    section.insert("meters".into(), Value::Array(all));
    section.insert(
        "note".into(),
        if reached {
            json!("Limit reached")
        } else {
            Value::Null
        },
    );
    section.insert("stale".into(), json!(false));
    section.insert("updated".into(), json!(now));
    Some(Value::Object(section))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_named_for_their_length() {
        let name = |minutes: Value| window(Some(&minutes)).unwrap();
        assert_eq!(name(json!(1440)), "Daily");
        assert_eq!(name(json!(10080)), "Weekly");
        assert_eq!(name(json!(43200)), "Monthly");
        assert_eq!(name(json!(2880)), "2-day");
        assert_eq!(name(json!(300)), "5-hour");
        assert_eq!(name(json!(45)), "45-minute");
        assert_eq!(window(None).unwrap(), "Limit");
        assert!(window(Some(&json!("soon"))).is_none());
    }

    #[test]
    fn the_main_limits_come_first_then_the_others_by_name() {
        let reply = json!({"id": 1, "result": {
        "rateLimits": {"limitId": "codex", "planType": "plus",
            "primary": {"usedPercent": 12.4, "windowDurationMins": 300, "resetsAt": 1_790_000_000},
            "secondary": {"usedPercent": 40, "windowDurationMins": 10080, "resetsAt": 1_790_500_000}},
        "rateLimitsByLimitId": {
            "codex": {"limitId": "codex"},
            "codex-mini": {"limitName": "Mini", "primary": {"usedPercent": 99.5, "windowDurationMins": 300}},
        }}});
        let report = report(&reply, 9).unwrap();
        let labels: Vec<&str> = report["meters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["5-hour", "Weekly", "Mini · 5-hour"]);
        assert_eq!(report["meters"][2]["percent"], 100);
        assert_eq!(report["detail"], "Plus");
        assert_eq!(report["note"], Value::Null);
    }

    #[test]
    fn a_reached_limit_says_so() {
        let reply = json!({"result": {"rateLimits": {"rateLimitReachedType": "primary"}}});
        assert_eq!(report(&reply, 0).unwrap()["note"], "Limit reached");
    }
}

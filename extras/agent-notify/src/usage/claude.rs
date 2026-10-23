//! Claude Code: `GET api.anthropic.com/api/oauth/usage` with Claude Code's own login.
//!
//! The login is only ever read. Renewing it would replace the refresh token and sign Claude Code
//! out, so an expired one waits for Claude Code to renew it. The request goes through curl, with
//! the token on its stdin, never on its command line where any process listing shows it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::{Value, json};

use super::time::{self, Reset};
use super::{Usage, now, rounded};
use crate::env::Env;
use crate::payload::{Key, at, first, to_text};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";

pub fn credentials(env: &Env) -> PathBuf {
    env.home.join(".claude/.credentials.json")
}

pub fn fetch(usage: &Usage) -> bool {
    let now = now();
    let login = fs::read(credentials(usage.env))
        .ok()
        .and_then(|text| serde_json::from_slice::<Value>(&text).ok());
    let login = login.unwrap_or(Value::Null);
    let oauth = |key| at(&login, &[Key("claudeAiOauth"), Key(key)]);
    let plan = oauth("subscriptionType").map(to_text).unwrap_or_default();
    let expires = match oauth("expiresAt") {
        None => Some(0.0),
        Some(value) => value.as_f64().map(|ms| (ms / 1000.0).floor()),
    };
    if !expires.is_some_and(|expires| expires >= 0.0 && expires > now as f64) {
        usage.keep_stale(
            "claude",
            "Claude",
            "Login expired: Claude Code renews it when it next runs",
        );
        return false;
    }
    let header = match oauth("accessToken") {
        Some(Value::String(token)) => format!("Authorization: Bearer {token}\n"),
        _ => String::new(),
    };
    let body = usage.file("claude", "body");
    let headers = usage.file("claude", "headers");
    let code = ask(usage, &header, &body, &headers);
    let mut fetched = false;
    match code.as_str() {
        "200" => {
            let reply = fs::read(&body)
                .ok()
                .and_then(|text| serde_json::from_slice::<Value>(&text).ok());
            fetched = reply
                .and_then(|reply| report(&reply, &plan, now))
                .is_some_and(|report| usage.write("claude", &report));
            if !fetched {
                usage.keep_stale(
                    "claude",
                    "Claude",
                    "The usage reply wasn't in a shape agent-notify knows",
                );
            }
        }
        "429" => {
            let wait = fs::read_to_string(&headers)
                .ok()
                .and_then(|text| retry_after(&text))
                .unwrap_or(0)
                .max(300);
            let _ = crate::state::write_line(&usage.file("claude", "retry"), now + wait);
            let clock = time::local_clock(now + wait);
            usage.keep_stale(
                "claude",
                "Claude",
                &format!("Rate limited: asking again at {clock}"),
            );
        }
        "401" | "403" => {
            usage.keep_stale(
                "claude",
                "Claude",
                "Login refused: Claude Code renews it when it next runs",
            );
        }
        "000" | "" => {
            usage.keep_stale("claude", "Claude", "Couldn't reach Anthropic");
        }
        other => {
            usage.keep_stale(
                "claude",
                "Claude",
                &format!("Anthropic answered HTTP {other}"),
            );
        }
    }
    let _ = fs::remove_file(&body);
    let _ = fs::remove_file(&headers);
    fetched
}

/// Asks with curl, and returns the HTTP status it got ("000" or nothing if it got none).
fn ask(usage: &Usage, header: &str, body: &PathBuf, headers: &PathBuf) -> String {
    let mut command = usage.env.command("curl");
    command
        .args([
            "-sS",
            "--max-time",
            "20",
            "-H",
            "@-",
            "-H",
            "anthropic-beta: oauth-2025-04-20",
        ])
        .args(["-H", "Accept: application/json", "-D"])
        .arg(headers)
        .arg("-o")
        .arg(body)
        .args(["-w", "%{http_code}", URL])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut curl) = command.spawn() else {
        return String::new();
    };
    if let Some(mut stdin) = curl.stdin.take() {
        let _ = stdin.write_all(header.as_bytes());
    }
    curl.wait_with_output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

/// The seconds a 429's Retry-After asks for.
fn retry_after(headers: &str) -> Option<i64> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        let name = name.as_bytes();
        let matches = name.len() == 11
            && matches!(name[0], b'R' | b'r')
            && &name[1..6] == b"etry-"
            && matches!(name[6], b'A' | b'a')
            && &name[7..] == b"fter";
        if !matches {
            return None;
        }
        let digits: String = value
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Some(digits.parse().unwrap_or(0))
    })
}

/// The first letter in capitals.
pub fn title(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// One meter; None when its reset time isn't a time at all, Err when the reply is unreadable.
fn meter(
    label: String,
    percent: Option<&Value>,
    resets: Option<&Value>,
) -> Result<Option<Value>, ()> {
    let percent = match percent {
        None => 0.0,
        Some(percent) => percent.as_f64().ok_or(())?,
    };
    let resets = match time::reset(resets.unwrap_or(&Value::Null))? {
        Reset::None => Value::Null,
        Reset::At(at) => json!(at),
        Reset::Unreadable => return Ok(None),
    };
    Ok(Some(
        json!({"label": label, "percent": rounded(percent), "resets": resets}),
    ))
}

/// The section for the reply, from its `limits` where it has them, or else its five-hour and
/// seven-day windows. None if the reply isn't in a shape this knows.
pub fn report(reply: &Value, plan: &str, now: i64) -> Option<Value> {
    if !(reply.is_object() || reply.is_null()) {
        return None;
    }
    let mut meters = Vec::new();
    match reply.get("limits") {
        Some(Value::Array(limits)) if !limits.is_empty() => {
            for limit in limits {
                if !limit.is_object() {
                    return None;
                }
                let kind = limit.get("kind");
                let label = if kind.and_then(Value::as_str) == Some("session") {
                    "Session".to_owned()
                } else if kind.and_then(Value::as_str) == Some("weekly_all") {
                    "Weekly".to_owned()
                } else if limit.get("group").and_then(Value::as_str) == Some("weekly") {
                    let scope = first(
                        limit,
                        &[
                            &[Key("scope"), Key("model"), Key("display_name")],
                            &[Key("scope"), Key("surface"), Key("display_name")],
                            &[Key("scope"), Key("surface")],
                        ],
                    );
                    format!(
                        "Weekly · {}",
                        scope.map_or_else(|| "other".to_owned(), to_text)
                    )
                } else {
                    title(&kind?.as_str()?.replace('_', " "))
                };
                let percent = at(limit, &[Key("percent")]);
                meters.extend(meter(label, percent, limit.get("resets_at")).ok()?);
            }
        }
        _ => {
            for (label, key) in [("Session", "five_hour"), ("Weekly", "seven_day")] {
                let Some(window) = reply.get(key).filter(|window| !window.is_null()) else {
                    continue;
                };
                if !window.is_object() {
                    return None;
                }
                let percent = at(window, &[Key("utilization")]);
                meters.extend(meter(label.to_owned(), percent, window.get("resets_at")).ok()?);
            }
        }
    }
    let detail = if plan.is_empty() {
        Value::Null
    } else {
        json!(title(plan))
    };
    Some(json!({
        "name": "Claude", "detail": detail, "meters": meters,
        "note": null, "stale": false, "updated": now,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_labelled_by_kind_and_scope() {
        let reply = json!({"limits": [
            {"kind": "session", "percent": 41.6, "resets_at": "2026-10-23T19:00:00.000000+00:00"},
            {"kind": "weekly_all", "percent": 12},
            {"kind": "weekly_model", "group": "weekly", "percent": 3, "scope": {"model": {"display_name": "Opus"}}},
            {"kind": "weekly_surface", "group": "weekly", "percent": 0, "scope": {"surface": "web"}},
            {"kind": "extra_usage", "percent": 50.5},
        ]});
        let report = report(&reply, "max", 1_000).unwrap();
        let labels: Vec<&str> = report["meters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "Session",
                "Weekly",
                "Weekly · Opus",
                "Weekly · web",
                "Extra usage"
            ]
        );
        assert_eq!(report["meters"][0]["percent"], 42);
        assert_eq!(report["meters"][0]["resets"], 1_792_782_000);
        assert_eq!(report["meters"][4]["percent"], 51);
        assert_eq!(report["detail"], "Max");
        assert_eq!(report["updated"], 1_000);
    }

    #[test]
    fn without_limits_the_windows_are_read() {
        let reply = json!({"five_hour": {"utilization": 7, "resets_at": null}, "seven_day": null});
        let report = report(&reply, "", 5).unwrap();
        assert_eq!(
            report["meters"],
            json!([{"label": "Session", "percent": 7, "resets": null}])
        );
        assert_eq!(report["detail"], Value::Null);
    }

    #[test]
    fn a_reply_of_an_unknown_shape_is_refused() {
        assert!(report(&json!([1]), "", 0).is_none());
        assert!(report(&json!({"limits": [{"percent": 3}]}), "", 0).is_none());
        assert!(
            report(
                &json!({"limits": [{"kind": "session", "percent": "most"}]}),
                "",
                0
            )
            .is_none()
        );
    }

    #[test]
    fn a_rate_limit_says_how_long_to_wait() {
        assert_eq!(retry_after("HTTP/2 429\r\nretry-after: 120\r\n"), Some(120));
        assert_eq!(retry_after("Retry-After: soon\r\n"), Some(0));
        assert_eq!(retry_after("x-retry-after: 5\r\n"), None);
    }
}

//! `agent-notify usage [--refresh]`: how much of each agent's plan limits is used, as JSON for a
//! desktop meter.
//!
//! It prints straight from a cache, `~/.cache/agent-notify/usage/`, and refreshes the cache in the
//! background once it's older than `usage-refresh-secs`. The numbers come from where the agents'
//! own usage screens get them (see each provider). None is a documented API: they're asked at
//! most every few minutes, and a refusal keeps the last numbers on show with a note saying why
//! they're old. Each limit notifies once per window as its use passes each level in
//! `usage-alerts`.

mod agy;
mod claude;
mod codex;
mod time;

use std::fs::{self, File};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agents;
use crate::config::Config;
use crate::env::Env;
use crate::notify::{Notification, Notifier};
use crate::state;
use crate::text::{self, is_number};

pub use time::now;

/// The agents usage can be read for, in the order the report lists them.
const PROVIDERS: [&str; 3] = ["claude", "codex", "agy"];

pub fn run(env: &Env, args: &[String]) -> ExitCode {
    let usage = Usage {
        env,
        config: Config::load(env.config_file()),
        dir: env.usage_dir(),
    };
    if fs::create_dir_all(&usage.dir).is_ok() {
        let _ = fs::set_permissions(&usage.dir, fs::Permissions::from_mode(0o700));
    }
    let refresh = usage.refresh_secs();
    match args.first().map(String::as_str) {
        Some("--refresh") => {
            usage.refresh(60, true);
            usage.print()
        }
        Some("--background") => {
            usage.refresh(refresh, false);
            ExitCode::SUCCESS
        }
        None => {
            let providers = usage.providers();
            if !providers
                .iter()
                .any(|provider| File::open(usage.cache(provider)).is_ok())
            {
                // Nothing to show yet, so this once it's worth the wait.
                usage.refresh(refresh, true);
            } else if providers
                .iter()
                .any(|provider| usage.due(provider, refresh))
            {
                usage.refresh_in_background();
            }
            usage.print()
        }
        Some(_) => {
            eprintln!("usage: agent-notify usage [--refresh]");
            ExitCode::FAILURE
        }
    }
}

pub struct Usage<'a> {
    env: &'a Env,
    config: Config,
    dir: PathBuf,
}

impl Usage<'_> {
    /// The providers to report: agents in the config that usage can be read for.
    fn providers(&self) -> Vec<&'static str> {
        PROVIDERS
            .into_iter()
            .filter(|&provider| self.config.wants(provider))
            .filter(|&provider| match provider {
                "claude" => File::open(claude::credentials(self.env)).is_ok(),
                other => self.env.have(other),
            })
            .collect()
    }

    fn refresh_secs(&self) -> i64 {
        // The endpoints rate-limit; once a minute is the least anyone should wait.
        self.config.number("usage-refresh-secs", 300).max(60) as i64
    }

    fn cache(&self, provider: &str) -> PathBuf {
        self.dir.join(format!("{provider}.json"))
    }

    fn file(&self, provider: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{provider}.{extension}"))
    }

    /// Whether `provider` was last asked at least `min_age` seconds ago, and isn't waiting out a
    /// rate limit.
    fn due(&self, provider: &str, min_age: i64) -> bool {
        let read =
            |extension| state::number_in(&self.file(provider, extension)).unwrap_or(0) as i64;
        let now = now();
        now - read("asked") >= min_age && now >= read("retry")
    }

    /// Replaces the provider's section, if `section` is an object.
    fn write(&self, provider: &str, section: &Value) -> bool {
        section.is_object()
            && state::write_private(&self.cache(provider), format!("{section}\n").as_bytes())
                .is_ok()
    }

    /// The last numbers stay, marked stale, with `note` saying why. False if they can't be kept.
    fn keep_stale(&self, provider: &str, name: &str, note: &str) -> bool {
        let section = match fs::read(self.cache(provider)) {
            Ok(text) => match serde_json::from_slice::<Value>(&text) {
                Ok(Value::Object(mut section)) => {
                    section.insert("note".into(), json!(note));
                    section.insert("stale".into(), json!(true));
                    Value::Object(section)
                }
                _ => return false,
            },
            Err(_) => {
                json!({"name": name, "meters": [], "note": note, "stale": true, "updated": 0})
            }
        };
        self.write(provider, &section)
    }

    /// Asks every provider last asked at least `min_age` seconds ago. One refresh runs at a time;
    /// another waits for it when `wait`, and otherwise leaves it to it.
    fn refresh(&self, min_age: i64, wait: bool) {
        let Ok(lock) = File::create(self.dir.join("lock")) else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(if wait { 60 } else { 0 });
        // SAFETY: flock only takes a lock on a descriptor that stays open below.
        while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for provider in self.providers() {
            if !self.due(provider, min_age) {
                continue;
            }
            let _ = state::write_line(&self.file(provider, "asked"), now());
            let fetched = match provider {
                "claude" => claude::fetch(self),
                "codex" => codex::fetch(self),
                _ => agy::fetch(self),
            };
            if fetched {
                let _ = fs::remove_file(self.file(provider, "retry"));
                self.alerts(provider);
            }
        }
    }

    /// Starts a refresh detached, with nothing of the caller's open, so whoever runs `usage`
    /// gets its output and its exit straight away.
    fn refresh_in_background(&self) {
        let mut command = self.env.command(self.env.self_path());
        command
            .args(["usage", "--background"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe and touches nothing of the parent's.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let _ = command.spawn();
    }

    /// Notifies once per limit window as its use passes each level in usage-alerts.
    fn alerts(&self, provider: &str) {
        let Ok(text) = fs::read(self.cache(provider)) else {
            return;
        };
        let Ok(section) = serde_json::from_slice::<Value>(&text) else {
            return;
        };
        let Some(meters) = section.get("meters").and_then(Value::as_array) else {
            return;
        };
        let levels: Vec<u64> = self
            .config
            .get_or("usage-alerts", "75 90 100")
            .split_whitespace()
            .filter(|level| is_number(level))
            .filter_map(|level| level.parse().ok())
            .collect();
        let app = agents::display_name(provider);
        let alerts_file = self.dir.join("alerts");
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&alerts_file);
        let now = now();
        let mut notifier = Notifier::from_env();
        for meter in meters {
            let label = meter.get("label").map(tsv).unwrap_or_default();
            let Some(percent) = meter.get("percent").map(tsv).filter(|p| is_number(p)) else {
                continue;
            };
            let resets = meter.get("resets").and_then(crate::payload::present);
            let resets = tsv(resets.unwrap_or(&json!(0)));
            let key = format!("{provider}|{label}");
            let alerts = fs::read_to_string(&alerts_file).unwrap_or_default();
            let used = percent.parse().unwrap_or(0);
            let Some(reached) = newly_reached(&alerts, &key, &resets, used, &levels) else {
                continue;
            };
            // One line per limit: its current window's.
            let mut kept: String = alerts
                .lines()
                .filter(|line| line.split('\t').next() != Some(key.as_str()))
                .map(|line| format!("{line}\n"))
                .collect();
            kept.push_str(&format!("{key}\t{resets}\t{reached}\n"));
            let _ = state::write_whole(&alerts_file, kept.as_bytes(), 0o666);
            let resets_at = Some(&resets)
                .filter(|at| is_number(at))
                .and_then(|at| at.parse::<i64>().ok());
            let body = match resets_at {
                Some(at) if at > now => format!("Resets in {}", text::duration(at - now)),
                _ => String::new(),
            };
            let (summary, urgency, sound) = if reached >= 100 {
                (
                    format!("{app} · {label}: limit reached"),
                    2,
                    Some(self.config.sound_attention()),
                )
            } else {
                (format!("{app} · {label}: {percent}% used"), 1, None)
            };
            let note = Notification {
                app,
                replaces: 0,
                urgency,
                sound,
                summary: &summary,
                body: &body,
                sender_pid: None,
            };
            let _ = notifier.notify(&note);
        }
    }

    /// The report: every provider's section, each marked stale once its numbers are older than
    /// three refreshes, however they were kept.
    fn print(&self) -> ExitCode {
        let old = self.refresh_secs() * 3;
        let now = now();
        let mut sections = Vec::new();
        for provider in self.providers() {
            let Ok(text) = fs::read(self.cache(provider)) else {
                continue;
            };
            let Ok(Value::Object(mut section)) = serde_json::from_slice::<Value>(&text) else {
                return ExitCode::FAILURE;
            };
            let updated = match section.get("updated").and_then(crate::payload::present) {
                None => 0.0,
                Some(Value::Number(updated)) => updated.as_f64().unwrap_or(0.0),
                Some(_) => return ExitCode::FAILURE,
            };
            let stale = section
                .get("stale")
                .and_then(crate::payload::present)
                .is_some()
                || now as f64 - updated > old as f64;
            section.insert("stale".into(), json!(stale));
            sections.push(Value::Object(section));
        }
        println!("{}", json!({"sections": sections}));
        ExitCode::SUCCESS
    }
}

/// The level a limit has newly passed: the last of `levels` that `used` has reached, if it is
/// higher than the one already announced for this window in `alerts`. A window is known by its
/// reset time, which wobbles by a second or so between answers.
fn newly_reached(alerts: &str, key: &str, resets: &str, used: u64, levels: &[u64]) -> Option<u64> {
    let announced: Vec<&str> = alerts
        .lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|row| row[0] == key)
        .filter(|row| {
            let then = number_of(row.get(1).copied().unwrap_or_default());
            (then - number_of(resets)).abs() < 3600.0
        })
        .map(|row| row.get(2).copied().unwrap_or_default())
        .collect();
    let announced: u64 = match announced.as_slice() {
        [one] if is_number(one) => one.parse().unwrap_or(0),
        _ => 0,
    };
    let reached = levels
        .iter()
        .copied()
        .rfind(|&level| used >= level)
        .unwrap_or(0);
    (reached > announced).then_some(reached)
}

/// A value as it appears in the alerts file: text with tabs, newlines and backslashes escaped.
fn tsv(value: &Value) -> String {
    match value {
        Value::String(text) => text
            .replace('\\', r"\\")
            .replace('\t', r"\t")
            .replace('\n', r"\n")
            .replace('\r', r"\r"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Text as a number, the way awk reads one: the number it starts with, or 0.
fn number_of(text: &str) -> f64 {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .take_while(|&(at, ch)| {
            ch.is_ascii_digit() || ch == '.' || (at == 0 && (ch == '-' || ch == '+'))
        })
        .count();
    text[..end].parse().unwrap_or(0.0)
}

/// A number rounded to a whole one, as JSON.
fn rounded(number: f64) -> Value {
    let whole = number.round();
    if whole.abs() < 9e15 {
        json!(whole as i64)
    } else {
        json!(whole)
    }
}

/// Runs a command and returns what it printed, stopping it after `limit` with whatever it had
/// printed by then.
fn output_within(mut command: Command, limit: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut printed = Vec::new();
        let _ = stdout.read_to_end(&mut printed);
        let _ = sender.send(printed);
    });
    let mut printed = receiver.recv_timeout(limit).ok();
    let _ = child.kill();
    let _ = child.wait();
    if printed.is_none() {
        // Stopped: its output ends now, unless something it started still holds it open.
        printed = receiver.recv_timeout(Duration::from_secs(1)).ok();
    }
    printed.map(|printed| String::from_utf8_lossy(&printed).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_file_values_are_escaped_like_tsv() {
        assert_eq!(tsv(&json!("Weekly\t· Opus")), r"Weekly\t· Opus");
        assert_eq!(tsv(&json!(1_790_000_000)), "1790000000");
        assert_eq!(tsv(&Value::Null), "");
    }

    #[test]
    fn numbers_read_from_text_as_awk_reads_them() {
        assert_eq!(number_of("1790000000"), 1_790_000_000.0);
        assert_eq!(number_of("2026-10-23"), 2026.0);
        assert_eq!(number_of("soon"), 0.0);
        assert_eq!(number_of("12.5"), 12.5);
    }

    #[test]
    fn each_level_is_announced_once_per_window() {
        let levels = [75, 90, 100];
        assert_eq!(
            newly_reached("", "claude|Session", "1000", 80, &levels),
            Some(75)
        );
        let announced = "claude|Session\t1001\t75\ncodex|Weekly\t5\t100\n";
        assert_eq!(
            newly_reached(announced, "claude|Session", "1000", 80, &levels),
            None
        );
        assert_eq!(
            newly_reached(announced, "claude|Session", "1000", 95, &levels),
            Some(90)
        );
        let next_window = "claude|Session\t1000\t100\n";
        assert_eq!(
            newly_reached(next_window, "claude|Session", "90000", 76, &levels),
            Some(75)
        );
        assert_eq!(newly_reached("", "claude|Session", "0", 50, &levels), None);
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(rounded(42.5), json!(43));
        assert_eq!(rounded(-42.5), json!(-43));
        assert_eq!(rounded(0.4), json!(0));
    }
}

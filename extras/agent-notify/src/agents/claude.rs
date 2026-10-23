//! Claude Code: hooks in `~/.claude/settings.json`, and the records it keeps of background
//! sessions, which `keep` starts and attaches to.
//!
//! Each running session has an entry in `~/.claude/sessions/<pid>.json`; one running in the
//! background names its job there, and the job's record, `~/.claude/jobs/<id>/state.json`, holds
//! its name, whether it is busy, and the subagents it has running. A resumed session has a new
//! session ID but keeps its job's.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::env::Env;
use crate::text;

pub fn hooks(command: &dyn Fn(&str) -> String) -> Value {
    let hook = |event: &str, timeout: u64| super::command_hook(command(event), timeout);
    json!({
        "SessionStart": {"hooks": [hook("session_start", 10)]},
        "UserPromptSubmit": {"hooks": [hook("start", 10)]},
        "Stop": {"hooks": [hook("finish", 10)]},
        "PostToolUse": {"hooks": [hook("working", 5)]},
        "Notification": {
            "matcher": "permission_prompt|elicitation_dialog",
            "hooks": [hook("attention", 10)],
        },
    })
}

/// Claude Code's subcommands, which aren't sessions and run as they are.
pub const COMMANDS: [&str; 21] = [
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "doctor",
    "gateway",
    "import",
    "install",
    "kill",
    "logs",
    "mcp",
    "plugin",
    "plugins",
    "purge",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "ultrareview",
    "update",
    "upgrade",
];

/// Options after which a command line isn't an interactive session to keep.
pub const NOT_A_SESSION: [&str; 8] = [
    "-p",
    "--print",
    "--bg",
    "--background",
    "-h",
    "--help",
    "-v",
    "--version",
];

/// Whether `id` is shaped like a job ID: letters, digits, `_` and `-`.
pub fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// The job ID in what `claude --bg` prints: the first line saying `claude attach ID …`.
pub fn attach_id(printed: &str) -> Option<&str> {
    printed.lines().find_map(|line| {
        let rest = line
            .trim_start_matches(' ')
            .strip_prefix("claude attach ")?;
        let end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))?;
        (end > 0 && end <= 80 && rest[end..].starts_with(' ')).then(|| &rest[..end])
    })
}

/// The background job the session running as process `pid` belongs to.
pub fn job_id(env: &Env, pid: u32) -> Option<String> {
    let entry = fs::read_to_string(env.home.join(format!(".claude/sessions/{pid}.json"))).ok()?;
    let entry: Value = serde_json::from_str(&entry).ok()?;
    let id =
        crate::payload::at(&entry, &[crate::payload::Key("jobId")]).map(crate::payload::to_text)?;
    let id: String = id
        .chars()
        .filter(|&ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        .collect();
    (!id.is_empty()).then_some(id)
}

pub fn job_file(env: &Env, id: &str) -> PathBuf {
    env.home.join(format!(".claude/jobs/{id}/state.json"))
}

/// What a background session's record says about it.
pub struct Job {
    /// The session's name, on one line.
    pub name: String,
    /// The subagents it has running, by their task: up to eight.
    pub helpers: Vec<String>,
}

/// The record of the background session running as process `pid`, if it has one that can be
/// read. A record that isn't the expected JSON names nothing.
pub fn job_of(env: &Env, pid: u32) -> Option<Job> {
    let id = job_id(env, pid)?;
    let text = fs::read(job_file(env, &id)).ok()?;
    let record: Value = serde_json::from_slice(&text).unwrap_or(Value::Null);
    Some(Job {
        name: job_name(&record),
        helpers: helpers(&record),
    })
}

fn job_name(record: &Value) -> String {
    match record.get("name") {
        Some(Value::String(name)) => text::collapse_whitespace(name),
        _ => String::new(),
    }
}

/// The subagents still running, in the order the record lists them. A malformed entry ends the
/// list there.
fn helpers(record: &Value) -> Vec<String> {
    let entries: Vec<&Value> = match record.get("fan") {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(Value::Object(items)) => items.values().collect(),
        _ => Vec::new(),
    };
    let mut helpers = Vec::new();
    for entry in entries {
        let entry = match entry {
            Value::Object(entry) => entry,
            Value::Null => continue,
            _ => break,
        };
        let running = entry.get("kind").and_then(Value::as_str) == Some("agent")
            && entry.get("doneAt").is_none_or(Value::is_null);
        if !running {
            continue;
        }
        let label = match entry.get("label") {
            None | Some(Value::Null | Value::Bool(false)) => String::new(),
            Some(Value::String(label)) => text::collapse_whitespace(label),
            Some(_) => break,
        };
        let label = text::strip_controls(&label, false);
        let label = text::cut(&label, 60);
        if !label.trim_start_matches(' ').is_empty() {
            helpers.push(label.to_owned());
        }
        if helpers.len() == 8 {
            break;
        }
    }
    helpers
}

/// Whether the background job is busy.
pub fn is_working(env: &Env, id: &str) -> bool {
    let Ok(text) = fs::read(job_file(env, id)) else {
        return false;
    };
    serde_json::from_slice::<Value>(&text)
        .is_ok_and(|record| record.get("state").and_then(Value::as_str) == Some("working"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_id_is_read_from_the_line_that_says_how_to_attach() {
        let printed = "Started in the background.\n  claude attach 7f3e_a-9 to reopen it\n";
        assert_eq!(attach_id(printed), Some("7f3e_a-9"));
        assert_eq!(attach_id("claude attach  x\n"), None, "no ID");
        assert_eq!(
            attach_id(&format!("claude attach {} x\n", "a".repeat(81))),
            None
        );
        assert_eq!(
            attach_id("claude attach abc\n"),
            None,
            "nothing after the ID"
        );
    }

    #[test]
    fn running_subagents_are_listed_by_task_up_to_eight() {
        let record = json!({"fan": [
            {"kind": "agent", "label": "reading\tthe tests", "doneAt": null},
            {"kind": "agent", "label": "done already", "doneAt": 17},
            {"kind": "shell", "label": "not an agent"},
            null,
            {"kind": "agent", "label": "   "},
            {"kind": "agent", "label": "fixing the docs"},
        ]});
        assert_eq!(helpers(&record), ["reading the tests", "fixing the docs"]);
        let many = json!({"fan": (0..12).map(|n| json!({"kind": "agent", "label": format!("task {n}")})).collect::<Vec<_>>()});
        assert_eq!(helpers(&many).len(), 8);
    }

    #[test]
    fn a_malformed_entry_ends_the_list() {
        let record = json!({"fan": [{"kind": "agent", "label": "first"}, "odd", {"kind": "agent", "label": "after"}]});
        assert_eq!(helpers(&record), ["first"]);
        let record =
            json!({"fan": [{"kind": "agent", "label": 3}, {"kind": "agent", "label": "after"}]});
        assert!(helpers(&record).is_empty());
    }
}

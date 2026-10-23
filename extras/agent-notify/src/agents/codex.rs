//! Codex CLI: hooks in `$CODEX_HOME/hooks.json`, which it runs only once they are trusted.

use serde_json::{Value, json};

pub fn hooks(command: &dyn Fn(&str) -> String) -> Value {
    let hook = |event: &str, timeout: u64| super::command_hook(command(event), timeout);
    json!({
        "SessionStart": {"hooks": [hook("session_start", 10)]},
        "PostToolUse": {"matcher": "*", "hooks": [hook("working", 5)]},
        "UserPromptSubmit": {"hooks": [hook("start", 10)]},
        "Stop": {"hooks": [hook("finish", 10)]},
        "PermissionRequest": {"matcher": "*", "hooks": [hook("attention", 10)]},
    })
}

/// What `setup` says once Codex's hooks are in place.
pub const TRUST_HINT: &str =
    "  Codex runs new hooks only once you trust them: open /hooks in Codex.";

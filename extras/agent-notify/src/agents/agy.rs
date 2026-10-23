//! Antigravity CLI: named hooks in `~/.gemini/config/hooks.json`, so this tool's are one entry.
//!
//! It sends nothing when a prompt arrives, only before each call to its model (`step`), and has
//! no event for a permission prompt, so it gets no attention hook.

use serde_json::{Value, json};

pub fn hooks(command: &dyn Fn(&str) -> String) -> Value {
    let hook = |event: &str, timeout: u64| super::command_hook(command(event), timeout);
    json!({
        "SessionStart": [hook("session_start", 10)],
        "PreInvocation": [hook("step", 10)],
        "PostToolUse": [{"matcher": "*", "hooks": [hook("working", 5)]}],
        "Stop": [hook("finish", 10)],
    })
}

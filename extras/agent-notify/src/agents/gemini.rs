//! Gemini CLI: hooks in `~/.gemini/settings.json`, each with a name, timeouts in milliseconds.

use serde_json::{Value, json};

pub fn hooks(command: &dyn Fn(&str) -> String) -> Value {
    let hook = |name: &str, event: &str| {
        let hook =
            json!({"type": "command", "name": name, "command": command(event), "timeout": 5000});
        json!({"hooks": [hook]})
    };
    json!({
        "SessionStart": hook("agent-notify-session", "session_start"),
        "AfterTool": hook("agent-notify-working", "working"),
        "BeforeAgent": hook("agent-notify-start", "start"),
        "AfterAgent": hook("agent-notify-finish", "finish"),
        "Notification": hook("agent-notify-attention", "attention"),
    })
}

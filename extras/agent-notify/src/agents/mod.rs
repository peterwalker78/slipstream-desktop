//! The agents this tool knows: what each is called, where its hooks live and how they are wired.

pub mod agy;
pub mod claude;
pub mod codex;
pub mod gemini;

use std::path::PathBuf;

use serde_json::Value;

use crate::env::Env;

/// Claude Code, Gemini CLI, Codex CLI and Antigravity CLI, by the command each is run as.
pub const KNOWN: [&str; 4] = ["claude", "gemini", "codex", "agy"];

/// What a notification names the agent as.
pub fn display_name(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "gemini" => "Gemini CLI",
        "codex" => "Codex",
        "agy" => "Antigravity",
        other => other,
    }
}

/// The agent's own subcommands: a process running one is a client of its sessions, not one.
pub fn subcommands(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &claude::COMMANDS,
        _ => &[],
    }
}

/// What a desktop trace calls the agent: one short lowercase word.
pub fn label(agent: &str) -> &str {
    match agent {
        "agy" => "antigravity",
        other => other,
    }
}

/// How `setup` puts an agent's hooks in its settings.
pub enum Wiring {
    /// Hook groups by event name, merged into the file's `hooks` beside the person's own.
    Groups(Value),
    /// One entry of the file, named for this tool, put in or taken out whole.
    Entry(Value),
}

/// The file an agent reads its hooks from.
pub fn hooks_file(env: &Env, agent: &str) -> Option<PathBuf> {
    Some(match agent {
        "claude" => env.home.join(".claude/settings.json"),
        "gemini" => env.home.join(".gemini/settings.json"),
        "codex" => env.codex_home().join("hooks.json"),
        "agy" => env.home.join(".gemini/config/hooks.json"),
        _ => return None,
    })
}

/// One hook entry: run `command`, with `timeout` (in the agent's own unit) to finish.
fn command_hook(command: String, timeout: u64) -> Value {
    serde_json::json!({"type": "command", "command": command, "timeout": timeout})
}

/// The hooks `setup` wires for an agent, each running `command(EVENT)`.
pub fn wiring(agent: &str, command: &dyn Fn(&str) -> String) -> Option<Wiring> {
    Some(match agent {
        "claude" => Wiring::Groups(claude::hooks(command)),
        "gemini" => Wiring::Groups(gemini::hooks(command)),
        "codex" => Wiring::Groups(codex::hooks(command)),
        "agy" => Wiring::Entry(agy::hooks(command)),
        _ => return None,
    })
}

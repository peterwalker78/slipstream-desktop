//! agent-notify: desktop notifications for terminal coding agents, run from their hooks.
//!
//! Agents run it from their hooks when a task starts, when it finishes, and when it's waiting on
//! you, and after each tool. It notifies over the standard org.freedesktop.Notifications
//! interface, from inside the agent's own process tree, so a server that looks can tie each
//! notification to the terminal window it came from (`hook`). The same hooks keep a note of what
//! each session is doing, for a desktop that draws agents at work (`note`). A session started
//! with `keep` runs in the background, attached to its terminal, so it carries on when the window
//! closes or you log out (`keep`). `usage` reports plan limits for a desktop meter, and `bar` is
//! a module for a bar that takes Waybar's custom-module format.
//!
//! Hooks run on every tool call, so a hook does as little as it can: it reads its JSON, a few
//! files under /proc and the note, and connects to the session bus only when it has something to
//! say. It never fails the agent: errors go to a log, and the exit status is always 0.

mod agents;
mod bar;
mod config;
mod env;
mod hook;
mod keep;
mod log;
mod note;
mod notify;
mod payload;
mod procs;
mod setup;
mod state;
mod status;
mod text;
mod transcript;
mod usage;

use std::ffi::OsString;
use std::process::ExitCode;

use crate::notify::{Notification, Notifier};

const HELP: &str = "\
agent-notify: desktop notifications for terminal coding agents.

Agents run it from their hooks when a task starts, when it finishes, and when it's waiting on you.
A task that ran for long-task-secs or more is announced with a sound; anything shorter gets a quiet
notification. Only an agent waiting on you after that long is critical, so a desktop that honours
the spec wakes the screen for it. Each session's notification replaces its previous one, and is
withdrawn when you send the next prompt.

The same hooks keep a note in $XDG_RUNTIME_DIR/slipstream/working for each open session, saying
what the agent is working on, whether it is working or idle, and what its subagents are doing, for
a desktop that shows agents at work.

  agent-notify setup [AGENT...]   wire up the agents in the config (naming some saves them there)
  agent-notify setup --remove     take every hook back out
  agent-notify status             what's configured, wired and listening, and where each session
                                  the desktop shows is running
  agent-notify test [--long]      send a sample notification
  agent-notify usage [--refresh]  plan limits used, as JSON for a desktop meter
  agent-notify bar                a bar module: one line of JSON (Waybar's custom-module format)
                                  each time the agents at work change, empty while none are
  agent-notify keep claude [ARGS] start a session in the background, attached to this terminal;
                                  alias claude='agent-notify keep claude' makes it the default
  agent-notify hook AGENT EVENT   what the hooks run; EVENT is session_start, start, finish,
                                  attention, working (the agent or a subagent has used a tool),
                                  or step (the agent is calling its model: a start if it was idle)

Agents: claude (Claude Code), gemini (Gemini CLI), codex (Codex CLI), agy (Antigravity CLI).

Config, ~/.config/agent-notify/config, one \"key = value\" per line:
  agents = claude gemini
  long-task-secs = 120
  sound-finished = complete
  sound-attention = window-attention
  usage-refresh-secs = 300
  usage-alerts = 75 90 100

Hooks that go wrong say why in ~/.local/state/agent-notify/agent-notify.log.
";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let text: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match text.first().map(String::as_str) {
        Some("hook") => {
            if let [_, agent, event, ..] = text.as_slice() {
                hook::run(agent, event);
            }
            ExitCode::SUCCESS
        }
        Some("setup") => setup::run(&env::Env::from_process(), &text[1..]),
        Some("status") => status::run(&env::Env::from_process()),
        Some("usage") => usage::run(&env::Env::from_process(), &text[1..]),
        Some("test") => sample(
            &env::Env::from_process(),
            text.get(1).map(String::as_str) == Some("--long"),
        ),
        Some("bar") => bar::run(&env::Env::from_process()),
        Some("keep") if args.len() >= 2 => {
            keep::run(&env::Env::from_process(), &args[1], &args[2..])
        }
        None | Some("keep" | "-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprint!("agent-notify: no command {other:?}\n\n{HELP}");
            ExitCode::FAILURE
        }
    }
}

/// `agent-notify test [--long]`: a sample notification.
fn sample(env: &env::Env, long: bool) -> ExitCode {
    let config = config::Config::load(env.config_file());
    let note = if long {
        Notification {
            app: "agent-notify",
            replaces: 0,
            urgency: 2,
            sound: Some(config.sound_finished()),
            summary: "Sample task finished",
            body: "A long one: critical, with a sound",
            sender_pid: None,
        }
    } else {
        Notification {
            app: "agent-notify",
            replaces: 0,
            urgency: 1,
            sound: None,
            summary: "Sample task finished",
            body: "A short one: quiet",
            sender_pid: None,
        }
    };
    match Notifier::from_env().notify(&note) {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agent-notify: couldn't send a notification: {err}");
            ExitCode::FAILURE
        }
    }
}

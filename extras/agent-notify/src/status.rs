//! `agent-notify status`: what's configured, wired and listening, and where each session the
//! desktop shows is running.

use std::fs;
use std::process::ExitCode;

use crate::agents::{self, claude};
use crate::config::Config;
use crate::env::Env;
use crate::note::Note;
use crate::notify::Notifier;
use crate::procs::Procs;
use crate::setup::is_wired;
use crate::state;
use crate::text::is_number;

pub fn run(env: &Env) -> ExitCode {
    let config = Config::load(env.config_file());
    println!("config: {}", config.path().display());
    println!("agents: {}", config.get_or("agents", "(none)"));
    println!("long task: {}s", config.get_or("long-task-secs", "120"));
    for agent in agents::KNOWN {
        let yes_no = |yes: bool| if yes { "yes" } else { "no" };
        println!(
            "  {:<12} installed: {:<3}  hooks: {}",
            agents::display_name(agent),
            yes_no(env.have(agent)),
            yes_no(is_wired(env, agent))
        );
    }
    match Notifier::from_env().server() {
        Ok((name, caps)) if caps.is_empty() => println!("notification server: {name}"),
        Ok((name, caps)) => println!("notification server: {name} ({})", caps.join(" ")),
        Err(_) => println!("notification server: none answering"),
    }
    sessions(env, &Procs::system());
    ExitCode::SUCCESS
}

/// The sessions the desktop shows a trace for, and where each one is: the terminal it is in, or
/// how to reach or stop one Claude Code keeps running with no terminal attached.
fn sessions(env: &Env, procs: &Procs) {
    let Some(dir) = env.notes_dir().filter(|dir| dir.is_dir()) else {
        return;
    };
    println!("sessions on the desktop:");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut any = false;
    for name in names {
        let Some(pid) = Some(&name)
            .filter(|name| is_number(name))
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if !procs.exists(pid) {
            continue;
        }
        any = true;
        let note = Note::parse(&fs::read_to_string(dir.join(&name)).unwrap_or_default());
        let status = if note.idle { "idle" } else { "working" };
        println!("  {pid:<8} {status:<7} {}", note.name);
        let agent = agents::KNOWN
            .into_iter()
            .find(|agent| procs.is_agent(agent, pid));
        let job = agent
            .filter(|&agent| agent == "claude")
            .and_then(|_| claude::job_id(env, pid));
        let terminal = |pid: u32| {
            procs
                .terminal_of(pid)
                .map(|(name, pid)| format!("{name} (PID {pid})"))
        };
        if let (Some(agent), Some(job)) = (agent, job) {
            let window = state::number_in(&env.state_dir().join(format!("{agent}-{job}.window")))
                .and_then(|window| u32::try_from(window).ok())
                .filter(|&window| procs.comm(window).as_deref() == Some("agent-notify"));
            match window.and_then(terminal) {
                Some(place) => println!("           in {place}"),
                None => println!(
                    "           no terminal: \"claude attach {job}\" opens it, \"claude stop {job}\" ends it"
                ),
            }
        } else if let Some(place) = terminal(pid) {
            println!("           in {place}");
        } else {
            println!("           no terminal");
        }
    }
    if !any {
        println!("  none");
    }
}

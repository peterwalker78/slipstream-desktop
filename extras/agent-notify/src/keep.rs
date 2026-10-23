//! `agent-notify keep AGENT [ARGS]`: starts a session in the background and attaches this
//! terminal to it, so it carries on when the window closes or you log out.
//!
//! The terminal is outside the session's process tree, so this process stays in the window,
//! under its shell, while it is attached: the hooks name it with the sender-pid hint as the
//! session's window, and it leaves the desktop a note that attaches the window to the session
//! again at the next login (the layout record looks for notes under the window's shell, which
//! the session isn't). Only Claude Code runs a background session; any other agent, and anything
//! that isn't an interactive session (print mode, a subcommand, no terminal), runs as it is.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agents::claude;
use crate::env::Env;
use crate::state;
use crate::text::shell_quote;

pub fn run(env: &Env, agent: &OsStr, args: &[OsString]) -> ExitCode {
    let interactive = is_terminal(libc::STDIN_FILENO) && is_terminal(libc::STDOUT_FILENO);
    if agent != "claude" || !interactive {
        return exec(env, agent, args);
    }
    // The ID goes into the note run again at the next login, so only one shaped like an ID is
    // kept there; anything else is claude's to answer.
    if let [attach, id] = args {
        if let Some(id) = id
            .to_str()
            .filter(|id| attach == "attach" && claude::is_id(id))
        {
            attach_to(env, "claude", id);
        }
    }
    let first = args.first().and_then(|arg| arg.to_str());
    if first.is_some_and(|first| claude::COMMANDS.contains(&first))
        || args.iter().any(|arg| {
            arg.to_str()
                .is_some_and(|arg| claude::NOT_A_SESSION.contains(&arg))
        })
    {
        return exec(env, agent, args);
    }
    // Claude won't start a background session in a folder it hasn't been trusted in yet: then
    // it runs as usual, and asks.
    let started = env
        .command("claude")
        .arg("--bg")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let printed = started.map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    match printed.as_deref().ok().and_then(claude::attach_id) {
        Some(id) => attach_to(env, "claude", id),
        None => exec(env, agent, args),
    }
}

fn is_terminal(fd: libc::c_int) -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// Runs the agent in this process's place.
fn exec(env: &Env, agent: &OsStr, args: &[OsString]) -> ExitCode {
    let err = env.command(agent).args(args).exec();
    eprintln!("agent-notify: {}: {err}", agent.to_string_lossy());
    ExitCode::from(if err.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    })
}

/// Set by SIGHUP or SIGTERM: the terminal has gone, or the session is ending.
static HUNG_UP: AtomicBool = AtomicBool::new(false);

extern "C" fn hung_up(_signal: libc::c_int) {
    HUNG_UP.store(true, Ordering::SeqCst);
}

fn on_signal(signal: libc::c_int, handler: libc::sighandler_t) {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe; sigaction is
    // given a zeroed, then filled, struct.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::null_mut());
    }
}

/// Whether this process still has a terminal: a hung-up one can still be closing round it.
fn terminal_still_there() -> bool {
    // SAFETY: both only read the terminal's settings into structs owned here.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        let mut size: libc::winsize = std::mem::zeroed();
        libc::tcgetattr(libc::STDIN_FILENO, &mut settings) == 0
            && libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) == 0
    }
}

/// Attaches this terminal to background session `id`, reopening it if it has stopped, and
/// stays until it detaches.
///
/// Closing the window or logging out hangs this process up with it, and a session still working
/// carries on. Leaving the session in the window (detaching, or quitting the agent view) stops
/// it, as quitting the agent does anywhere else: its conversation is kept, and attaching opens
/// it again.
fn attach_to(env: &Env, agent: &str, id: &str) -> ! {
    let me = std::process::id();
    let window = state::make_state(env).map(|dir| dir.join(format!("{agent}-{id}.window")));
    if let Some(window) = &window {
        let _ = state::write_line(window, me);
    }
    let resume = env
        .resume_dir()
        .filter(|dir| fs::create_dir_all(dir).is_ok())
        .map(|dir| dir.join(me.to_string()));
    if let Some(resume) = &resume {
        let again = format!(
            "{} keep {agent} attach {id}",
            shell_quote(&env.self_path().to_string_lossy())
        );
        let _ = state::write_line(resume, again);
    }
    on_signal(
        libc::SIGHUP,
        hung_up as extern "C" fn(libc::c_int) as libc::sighandler_t,
    );
    on_signal(
        libc::SIGTERM,
        hung_up as extern "C" fn(libc::c_int) as libc::sighandler_t,
    );
    let _ = env.command(agent).args(["attach", id]).status();
    if HUNG_UP.load(Ordering::SeqCst) {
        // A session with nothing under way would sit in memory with no terminal to reach it
        // by, so it is stopped. Its conversation is kept, and attaching opens it again.
        if !claude::is_working(env, id) {
            stop(env, agent, id);
        }
        std::process::exit(0);
    }
    if !terminal_still_there() {
        std::process::exit(0);
    }
    // Extra presses of the key that detached mustn't cut the stop short, or close the shell.
    on_signal(libc::SIGINT, libc::SIG_IGN);
    stop(env, agent, id);
    for file in window.iter().chain(&resume) {
        let _ = fs::remove_file(file);
    }
    std::process::exit(0);
}

fn stop(env: &Env, agent: &str, id: &str) {
    let _ = env
        .command(agent)
        .args(["stop", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

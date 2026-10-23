//! What the agents' hooks run: `agent-notify hook AGENT EVENT`, with the agent's JSON on stdin.
//!
//! A task that ran for `long-task-secs` or more is announced with a sound; anything shorter gets
//! a quiet notification. Only an agent waiting on you after that long is critical, so a desktop
//! that honours the spec wakes the screen for it: one that has simply finished can wait until
//! you're back. Each session's notification replaces its previous one, and is withdrawn when you
//! send the next prompt, since by then you've seen it.
//!
//! Each hook also writes the session's working note afresh from what the agent itself records,
//! where it keeps a record, so one hook missed or run out of order is put right by the next.

use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agents::{self, claude};
use crate::config::Config;
use crate::env::{self, Env};
use crate::log;
use crate::note::Note;
use crate::notify::{Notification, Notifier};
use crate::payload::{Index, Key, Payload, Step};
use crate::procs::Procs;
use crate::state;
use crate::text;
use crate::transcript;

/// Where the hook's JSON says the agent is working.
const FOLDER: &[&[Step]] = &[&[Key("cwd")], &[Key("workspacePaths"), Index(0)]];

/// The marker beside the state that says a session is waiting on you, for `agent-notify bar`.
pub fn waiting_marker(state_dir: &Path, pid: u32) -> PathBuf {
    state_dir.join(format!("{pid}.waiting"))
}

/// What the agent is doing, as the working note says it.
#[derive(Clone, Copy, PartialEq)]
enum Activity {
    Working,
    Idle,
    /// Idle, until you answer it.
    Waiting,
}

/// What a notification says happened.
#[derive(Clone, Copy, PartialEq)]
enum Announce {
    Finished,
    NeedsYou,
}

/// Runs a hook. Whatever happens, the agent sees nothing: no output, and success. Anything on
/// stdout could end up in the model's context, and some exit codes block the agent. What goes
/// wrong is logged instead.
pub fn run(agent: &str, event: &str) {
    silence_output();
    let env = Env::from_process();
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let context = format!("hook {agent} {event}");
    let hook = Hook {
        env: &env,
        agent,
        payload: Payload::parse(&String::from_utf8_lossy(&input)),
        procs: Procs::system(),
        context: &context,
    };
    if let Err(panicked) = panic::catch_unwind(AssertUnwindSafe(|| hook.handle(event))) {
        let message = panicked
            .downcast_ref::<&str>()
            .map(|text| text.to_string())
            .or_else(|| panicked.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        log::error(&env, &context, &format_args!("panicked: {message}"));
    }
}

/// Points stdout and stderr at /dev/null, for this process and anything it starts.
fn silence_output() {
    let Ok(null) = fs::OpenOptions::new().write(true).open("/dev/null") else {
        return;
    };
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: both descriptors are open for the whole run; dup2 replaces one atomically.
        unsafe { libc::dup2(null.as_raw_fd(), fd) };
    }
}

/// Seconds since boot: a clock that NTP and time zone changes can't move.
fn uptime_secs() -> i64 {
    let uptime = fs::read_to_string("/proc/uptime").ok();
    let whole = uptime
        .as_deref()
        .and_then(|text| text.split(['.', ' ']).next()?.parse().ok());
    whole.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64)
    })
}

/// The notification's body: the project and how long the task took, then the detail.
fn body(project: &str, elapsed: Option<i64>, detail: &str) -> String {
    let mut line = project.to_owned();
    if let Some(elapsed) = elapsed {
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(&text::duration(elapsed));
    }
    [line.as_str(), detail]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How loud a notification is: 0-2, and with its sound or not.
///
/// Only a long task is worth a sound: a short one finished while you watched. And only one
/// waiting on you is worth the screen. A finish can wait until you're back, so it isn't critical
/// and a desktop that has faded to its wallpaper stays there.
fn loudness(announce: Announce, long: bool) -> (u8, bool) {
    match (long, announce) {
        (true, Announce::Finished) => (1, true),
        (true, Announce::NeedsYou) => (2, true),
        (false, _) => (1, false),
    }
}

/// A session's state: when its task started, and its last notification.
struct Session {
    id: String,
    started: PathBuf,
    notification: PathBuf,
}

impl Session {
    fn new(state: &Path, agent: &str, id: String) -> Session {
        Session {
            started: state.join(format!("{agent}-{id}.started")),
            notification: state.join(format!("{agent}-{id}.id")),
            id,
        }
    }

    /// The id of the session's last notification, if it is one.
    fn last_notification(&self) -> Option<u32> {
        let id = state::first_line(&self.notification)?;
        id.parse().ok().filter(|_| text::is_number(&id))
    }
}

struct Hook<'a> {
    env: &'a Env,
    agent: &'a str,
    payload: Payload,
    procs: Procs,
    context: &'a str,
}

impl Hook<'_> {
    fn handle(&self, event: &str) {
        match event {
            // A new session has opened: an idle note if there isn't one yet, named for the agent
            // and its folder until a task gives it a better name.
            "session_start" => self.working_note(Activity::Idle, true),
            "working" => self.working_note(Activity::Working, false),
            _ => self.turn(event),
        }
    }

    /// A task starting (`start`, or Antigravity's `step`), finishing, or waiting on you.
    fn turn(&self, event: &str) {
        // Not what an agent sends: nothing to say.
        if !self.payload.is_object() {
            return;
        }
        let Some(state) = state::make_state(self.env) else {
            return;
        };
        let session = Session::new(&state, self.agent, self.session_id());
        let now = uptime_secs();
        // Antigravity says nothing when a prompt arrives, only before each call to its model:
        // the first of those since the last finish is the start.
        let event = match event {
            "step" if fs::File::open(&session.started).is_ok() => {
                return self.working_note(Activity::Working, false);
            }
            "step" => "start",
            other => other,
        };
        match event {
            "start" => self.start(&session, now),
            "finish" => {
                let elapsed = self.elapsed(&session, now);
                let _ = fs::remove_file(&session.started);
                self.working_note(Activity::Idle, false);
                self.announce(&state, &session, Announce::Finished, elapsed);
            }
            "attention" => {
                let elapsed = self.elapsed(&session, now);
                self.working_note(Activity::Waiting, false);
                self.announce(&state, &session, Announce::NeedsYou, elapsed);
            }
            _ => {}
        }
    }

    /// The session's ID, as a file name can hold it.
    fn session_id(&self) -> String {
        let id = self
            .payload
            .field(&[&[Key("session_id")], &[Key("conversationId")]]);
        let id: String = id
            .chars()
            .filter(|&ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            .take(80)
            .collect();
        if id.is_empty() { "default".into() } else { id }
    }

    fn elapsed(&self, session: &Session, now: i64) -> Option<i64> {
        state::number_in(&session.started).map(|started| now - started as i64)
    }

    fn start(&self, session: &Session, now: i64) {
        if let Err(err) = state::write_line(&session.started, now) {
            self.log(&err);
        }
        self.working_note(Activity::Working, false);
        // You're back and typing: the last word from this session has done its job.
        if state::first_line(&session.notification).is_some_and(|id| !id.is_empty()) {
            if let Some(id) = session.last_notification() {
                if let Err(err) = Notifier::from_env().close(id) {
                    self.log(&err);
                }
            }
            let _ = fs::remove_file(&session.notification);
        }
    }

    fn announce(&self, state: &Path, session: &Session, announce: Announce, elapsed: Option<i64>) {
        let config = Config::load(self.env.config_file());
        let app = agents::display_name(self.agent);
        let (summary, detail, sound) = match announce {
            Announce::Finished => (
                format!("{app} finished"),
                self.first_field(&["last_assistant_message", "prompt_response"]),
                config.sound_finished(),
            ),
            Announce::NeedsYou => (
                format!("{app} needs you"),
                self.wants(),
                config.sound_attention(),
            ),
        };
        let long = elapsed.is_some_and(|elapsed| elapsed >= config.long_task_secs() as i64);
        let (urgency, with_sound) = loudness(announce, long);
        let project = self.payload.field(FOLDER);
        let body = body(text::last_part(&project), elapsed, &detail);
        let note = Notification {
            app,
            replaces: session.last_notification().unwrap_or(0),
            urgency,
            sound: with_sound.then_some(sound),
            summary: &summary,
            body: &body,
            sender_pid: self.kept_window(state, &session.id),
        };
        match Notifier::from_env().notify(&note) {
            Ok(id) => {
                if let Err(err) = state::write_line(&session.notification, id) {
                    self.log(&err);
                }
            }
            Err(err) => self.log(&err),
        }
    }

    /// The first of these fields that says anything.
    fn first_field(&self, keys: &[&'static str]) -> String {
        keys.iter()
            .map(|&key| self.payload.field(&[&[Key(key)]]))
            .find(|value| !value.is_empty())
            .unwrap_or_default()
    }

    /// What an agent waiting on you wants.
    fn wants(&self) -> String {
        let message = self.payload.field(&[&[Key("message")]]);
        if !message.is_empty() {
            return message;
        }
        let command = self.payload.field(&[&[Key("tool_input"), Key("command")]]);
        let tool = self.payload.field(&[&[Key("tool_name")]]);
        if !command.is_empty() {
            format!("Wants to run: {command}")
        } else if !tool.is_empty() {
            format!("Waiting to use {tool}")
        } else {
            "Waiting for your answer".to_owned()
        }
    }

    /// A session kept running in the background has no window above it: the terminal attached
    /// to it, if that is still the process `keep` left there.
    fn kept_window(&self, state: &Path, session: &str) -> Option<u32> {
        let job = session.split('-').next().unwrap_or(session);
        let window = state::number_in(&state.join(format!("{}-{job}.window", self.agent)))?;
        let window = u32::try_from(window).ok()?;
        (self.procs.comm(window).as_deref() == Some("agent-notify")).then_some(window)
    }

    /// The agent's own process. Tools that manage agent sessions from outside the process tree
    /// can set AGENT_NOTIFY_PID to it, skipping the walk up the tree. None for a subagent's hook:
    /// only the top-level session leaves a note.
    fn agent_pid(&self) -> Option<u32> {
        let given = env::var("AGENT_NOTIFY_PID").and_then(|pid| pid.into_string().ok());
        if let Some(pid) = given.filter(|pid| text::is_number(pid)) {
            return pid.parse().ok();
        }
        self.procs
            .agent_above(self.agent, std::os::unix::process::parent_id())
    }

    /// Writes the note that tells the desktop what the agent is doing, only where a desktop has
    /// made the folder for them, and with `only_new` only if there's no note yet. An agent idle
    /// with subagents still running is working through them.
    fn working_note(&self, activity: Activity, only_new: bool) {
        let Some(dir) = self.env.notes_dir().filter(|dir| dir.is_dir()) else {
            return;
        };
        let Some(pid) = self.agent_pid() else { return };
        let path = dir.join(pid.to_string());
        if only_new && path.exists() {
            return;
        }
        let job = match self.agent {
            "claude" => claude::job_of(self.env, pid),
            _ => None,
        };
        let (task, helpers) = match job {
            Some(job) => (job.name, job.helpers),
            // A subagent's hooks name its own task, not the session's.
            None if self.payload.get(&[&[Key("agent_id")]]).is_some() => Default::default(),
            None => (transcript::task(&self.payload), Vec::new()),
        };
        let idle = activity != Activity::Working && helpers.is_empty();
        let marker = waiting_marker(&self.env.state_dir(), pid);
        if activity == Activity::Waiting {
            let _ = fs::write(&marker, "");
        } else {
            let _ = fs::remove_file(&marker);
        }
        let name = self.note_name(&path, &task);
        if let Err(err) = (Note {
            name,
            idle,
            helpers,
        })
        .write(&path)
        {
            self.log(&err);
        }
    }

    /// What the trace is called: the agent and its task; or, without a task, what the note
    /// already says; or else the agent and the folder it works in.
    fn note_name(&self, note: &Path, task: &str) -> String {
        let label = agents::label(self.agent);
        let task = text::strip_controls(task, false);
        let task = text::cut(&task, 60);
        if !task.is_empty() {
            return format!("{label}: {task}");
        }
        let written = fs::read_to_string(note).unwrap_or_default();
        let written = written.split('\n').next().unwrap_or_default();
        if !written.is_empty() {
            return written.to_owned();
        }
        let folder = self.payload.text(FOLDER).unwrap_or_default();
        match text::chomp(&folder) {
            "" => label.to_owned(),
            folder => format!("{label}: {}", text::last_part(folder)),
        }
    }

    fn log(&self, err: &dyn std::fmt::Display) {
        log::error(self.env, self.context, err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_names_the_project_the_time_and_the_detail() {
        assert_eq!(body("kiln", Some(432), "Done."), "kiln · 7m 12s\nDone.");
        assert_eq!(body("", Some(5), ""), "5s");
        assert_eq!(body("kiln", None, "Done."), "kiln\nDone.");
        assert_eq!(
            body("", None, "Waiting for your answer"),
            "Waiting for your answer"
        );
    }

    #[test]
    fn only_a_long_wait_on_you_is_critical_and_only_long_tasks_make_a_sound() {
        assert_eq!(loudness(Announce::NeedsYou, true), (2, true));
        assert_eq!(loudness(Announce::Finished, true), (1, true));
        assert_eq!(loudness(Announce::NeedsYou, false), (1, false));
        assert_eq!(loudness(Announce::Finished, false), (1, false));
    }
}

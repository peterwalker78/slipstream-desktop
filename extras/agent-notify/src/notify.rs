//! Desktop notifications, over the standard org.freedesktop.Notifications interface on the session
//! bus, so they work on any desktop. A notification is sent from inside the agent's own process
//! tree, which lets a server that looks tie it to the terminal window it came from.
//!
//! With `AGENT_NOTIFY_RECORD` naming a file, each call is written there as a line of JSON instead,
//! and nothing is sent: that is how the tests see what would have been.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
/// Hooks have a few seconds before the agent gives up on them.
const TIMEOUT: Duration = Duration::from_secs(5);

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Notification<'a> {
    pub app: &'a str,
    /// The notification this one takes the place of, or 0.
    pub replaces: u32,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    /// A sound theme name, or None for silence.
    pub sound: Option<&'a str>,
    pub summary: &'a str,
    pub body: &'a str,
    /// The process whose window the notification is about, when that isn't one above this one.
    pub sender_pid: Option<u32>,
}

/// A notification hint's value, of the type the specification gives it.
enum Hint<'a> {
    Byte(u8),
    Text(&'a str),
    Flag(bool),
    Pid(i64),
}

impl<'a> Hint<'a> {
    fn value(self) -> Value<'a> {
        match self {
            Hint::Byte(byte) => Value::from(byte),
            Hint::Text(text) => Value::from(text),
            Hint::Flag(flag) => Value::from(flag),
            Hint::Pid(pid) => Value::from(pid),
        }
    }
}

/// The hint as its D-Bus type and value, as busctl writes them.
impl std::fmt::Display for Hint<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Hint::Byte(byte) => write!(out, "y {byte}"),
            Hint::Text(text) => write!(out, "s {text}"),
            Hint::Flag(flag) => write!(out, "b {flag}"),
            Hint::Pid(pid) => write!(out, "x {pid}"),
        }
    }
}

impl Notification<'_> {
    fn hints(&self) -> Vec<(&'static str, Hint<'_>)> {
        let mut hints = vec![("urgency", Hint::Byte(self.urgency))];
        hints.push(match self.sound {
            Some(sound) => ("sound-name", Hint::Text(sound)),
            None => ("suppress-sound", Hint::Flag(true)),
        });
        if let Some(pid) = self.sender_pid {
            hints.push(("sender-pid", Hint::Pid(i64::from(pid))));
        }
        hints
    }
}

pub enum Notifier {
    /// Connected the first time something is sent, so commands that send nothing never do.
    Bus(Option<Connection>),
    Record(PathBuf),
}

impl Notifier {
    pub fn from_env() -> Notifier {
        match crate::env::var("AGENT_NOTIFY_RECORD") {
            Some(path) => Notifier::Record(path.into()),
            None => Notifier::Bus(None),
        }
    }

    /// Sends a notification and returns its id.
    pub fn notify(&mut self, note: &Notification) -> Result<u32, Error> {
        let bus = match self {
            Notifier::Bus(slot) => connect(slot)?,
            Notifier::Record(path) => {
                let hints: serde_json::Map<String, serde_json::Value> = note
                    .hints()
                    .into_iter()
                    .map(|(name, hint)| (name.to_owned(), json!(hint.to_string())))
                    .collect();
                let call = json!({
                    "call": "Notify", "app": note.app, "replaces": note.replaces,
                    "icon": "utilities-terminal", "summary": note.summary, "body": note.body,
                    "hints": hints, "timeout": -1,
                });
                return record(path, &call);
            }
        };
        let hints: HashMap<&str, Value> = note
            .hints()
            .into_iter()
            .map(|(name, hint)| (name, hint.value()))
            .collect();
        let actions: Vec<&str> = Vec::new();
        let body = (
            note.app,
            note.replaces,
            "utilities-terminal",
            note.summary,
            note.body,
            actions,
            hints,
            -1i32,
        );
        let reply = bus.call_method(Some(DESTINATION), PATH, Some(INTERFACE), "Notify", &body)?;
        Ok(reply.body().deserialize::<u32>()?)
    }

    /// Withdraws a notification.
    pub fn close(&mut self, id: u32) -> Result<(), Error> {
        let bus = match self {
            Notifier::Bus(slot) => connect(slot)?,
            Notifier::Record(path) => {
                return record(path, &json!({"call": "CloseNotification", "id": id})).map(|_| ());
            }
        };
        bus.call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "CloseNotification",
            &(id,),
        )?;
        Ok(())
    }

    /// The server's name and what it can do.
    pub fn server(&mut self) -> Result<(String, Vec<String>), Error> {
        let bus = match self {
            Notifier::Bus(slot) => connect(slot)?,
            Notifier::Record(_) => return Ok(("a recording".into(), Vec::new())),
        };
        let info = bus.call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "GetServerInformation",
            &(),
        )?;
        let (name, _vendor, _version, _spec): (String, String, String, String) =
            info.body().deserialize()?;
        let caps = bus.call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "GetCapabilities",
            &(),
        )?;
        Ok((name, caps.body().deserialize()?))
    }
}

fn connect(slot: &mut Option<Connection>) -> Result<&Connection, Error> {
    if slot.is_none() {
        *slot = Some(
            zbus::blocking::connection::Builder::session()?
                .method_timeout(TIMEOUT)
                .build()?,
        );
    }
    Ok(slot.as_ref().expect("connected above"))
}

/// Appends a call to the record; a notification's id is how many have been recorded.
fn record(path: &PathBuf, call: &serde_json::Value) -> Result<u32, Error> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{call}")?;
    let recorded = std::fs::read_to_string(path)?;
    Ok(recorded
        .lines()
        .filter(|line| line.contains(r#""call":"Notify""#))
        .count() as u32)
}

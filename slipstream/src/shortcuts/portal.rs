//! The global shortcuts portal's backend, `org.freedesktop.impl.portal.GlobalShortcuts`, on the
//! bus thread.
//!
//! xdg-desktop-portal passes each app's request on to it. Every call that needs an answer from the
//! desktop goes to the main loop as an `Event` and waits there for the reply, so the bus thread
//! never blocks; the main loop sends the `Activated` and `Deactivated` signals back out through
//! `emit`, one thread sending them in the order the keys went down and up.

use std::{
    collections::HashMap,
    sync::{OnceLock, mpsc},
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{
    interface,
    object_server::{ObjectServer, SignalEmitter},
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Str, Value},
};

pub const INTERFACE: &str = "org.freedesktop.impl.portal.GlobalShortcuts";

/// The portal's answers: done, turned down by the person, or something else went wrong.
const SUCCESS: u32 = 0;
const CANCELLED: u32 = 1;
const FAILED: u32 = 2;

/// A shortcut as an app asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub id: String,
    pub description: String,
    /// The keys it would like, as the XDG shortcuts specification writes them.
    pub preferred: Option<String>,
}

/// A shortcut as the app is told it is bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub description: String,
    /// The keys, as a person reads them.
    pub keys: String,
}

/// What the bus thread hands the main loop.
#[derive(Debug)]
pub enum Event {
    /// An app opened a session. `app` is its app ID: xdg-desktop-portal turns away apps that
    /// have none, though an older one may pass an empty ID on.
    Opened { session: String, app: String },
    /// The app wants these bound. `None` back means the person said no.
    Bind {
        session: String,
        wanted: Vec<Wanted>,
        reply: async_channel::Sender<Option<Vec<Listed>>>,
    },
    List {
        session: String,
        reply: async_channel::Sender<Vec<Listed>>,
    },
    /// The app closed the session, or went away.
    Closed { session: String },
}

/// What the main loop sends apps, through the portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Activated {
        session: String,
        id: String,
        time: u64,
    },
    Deactivated {
        session: String,
        id: String,
        time: u64,
    },
    /// The session's shortcuts are now these: one was taken away in Settings.
    Changed {
        session: String,
        shortcuts: Vec<Listed>,
    },
}

type Shortcuts = Vec<(String, HashMap<String, OwnedValue>)>;

fn text(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value.to_string()))
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let value = options.get(key)?;
    <&str>::try_from(value).ok().map(str::to_string)
}

/// The list as the portal sends it: each id with its description and its keys.
pub fn listed(list: &[Listed]) -> Shortcuts {
    list.iter()
        .map(|shortcut| {
            let properties = HashMap::from([
                ("description".to_string(), text(&shortcut.description)),
                ("trigger_description".to_string(), text(&shortcut.keys)),
            ]);
            (shortcut.id.clone(), properties)
        })
        .collect()
}

/// The list as the app sends it. An id given twice is asked about once.
pub fn wanted(shortcuts: Shortcuts) -> Vec<Wanted> {
    let mut wanted: Vec<Wanted> = Vec::new();
    for (id, options) in shortcuts {
        if id.is_empty() || wanted.iter().any(|w| w.id == id) {
            continue;
        }
        wanted.push(Wanted {
            description: string(&options, "description").unwrap_or_default(),
            preferred: string(&options, "preferred_trigger").filter(|t| !t.is_empty()),
            id,
        });
    }
    wanted
}

fn results(shortcuts: Option<&[Listed]>) -> HashMap<String, OwnedValue> {
    let Some(shortcuts) = shortcuts else {
        return HashMap::new();
    };
    // The list always has this one signature, so turning it into a value can't fail.
    match OwnedValue::try_from(Value::from(listed(shortcuts))) {
        Ok(value) => HashMap::from([("shortcuts".to_string(), value)]),
        Err(_) => HashMap::new(),
    }
}

/// The backend, served at the portal's object path.
pub struct Backend {
    pub events: Sender<Event>,
}

#[interface(name = "org.freedesktop.impl.portal.GlobalShortcuts")]
impl Backend {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let session = session_handle.as_str().to_string();
        let object = Session {
            path: session.clone(),
            events: self.events.clone(),
        };
        if server.at(&session_handle, object).await.is_err()
            || self.events.send(Event::Opened { session, app }).is_err()
        {
            return (FAILED, HashMap::new());
        }
        (SUCCESS, HashMap::new())
    }

    async fn bind_shortcuts(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        shortcuts: Shortcuts,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let (reply, answer) = async_channel::bounded(1);
        let event = Event::Bind {
            session: session_handle.as_str().to_string(),
            wanted: wanted(shortcuts),
            reply,
        };
        if self.events.send(event).is_err() {
            return (FAILED, HashMap::new());
        }
        match answer.recv().await {
            Ok(Some(bound)) => (SUCCESS, results(Some(&bound))),
            Ok(None) => (CANCELLED, HashMap::new()),
            Err(_) => (FAILED, HashMap::new()),
        }
    }

    async fn list_shortcuts(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let (reply, answer) = async_channel::bounded(1);
        let event = Event::List {
            session: session_handle.as_str().to_string(),
            reply,
        };
        if self.events.send(event).is_err() {
            return (FAILED, HashMap::new());
        }
        match answer.recv().await {
            Ok(list) => (SUCCESS, results(Some(&list))),
            Err(_) => (FAILED, HashMap::new()),
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// A session, at the path xdg-desktop-portal chose for it, so the portal can close it.
struct Session {
    path: String,
    events: Sender<Event>,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) {
        let _ = self.events.send(Event::Closed {
            session: self.path.clone(),
        });
        // Taken off the bus once this call has returned, rather than from inside it.
        let (server, path) = (server.clone(), self.path.clone());
        connection
            .executor()
            .spawn(
                async move {
                    let _ = server.remove::<Session, _>(path.as_str()).await;
                },
                "closing a shortcuts session",
            )
            .detach();
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

pub(super) fn send(connection: &zbus::blocking::Connection, signal: Signal) -> zbus::Result<()> {
    let path = crate::portal::PATH;
    let none = HashMap::<String, OwnedValue>::new();
    match signal {
        Signal::Activated { session, id, time } => {
            let session = ObjectPath::try_from(session)?;
            let body = (session, id, time, none);
            connection.emit_signal(None::<&str>, path, INTERFACE, "Activated", &body)
        }
        Signal::Deactivated { session, id, time } => {
            let session = ObjectPath::try_from(session)?;
            let body = (session, id, time, none);
            connection.emit_signal(None::<&str>, path, INTERFACE, "Deactivated", &body)
        }
        Signal::Changed { session, shortcuts } => {
            let session = ObjectPath::try_from(session)?;
            let body = (session, listed(&shortcuts));
            connection.emit_signal(None::<&str>, path, INTERFACE, "ShortcutsChanged", &body)
        }
    }
}

/// Sends a signal to apps through the portal, after every one sent before it. Nothing happens
/// when the portal backend isn't on the bus.
pub fn emit(signal: Signal) {
    static OUTBOX: OnceLock<mpsc::Sender<Signal>> = OnceLock::new();
    let Some(connection) = crate::portal::connection() else {
        return;
    };
    let outbox = OUTBOX.get_or_init(|| {
        let (outbox, signals) = mpsc::channel::<Signal>();
        std::thread::spawn(move || {
            for signal in signals {
                if let Err(err) = send(&connection, signal) {
                    tracing::warn!("couldn't send a shortcut's signal: {err}");
                }
            }
        });
        outbox
    });
    let _ = outbox.send(signal);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_app_asks_for_is_read_once_per_id() {
        let ask = |id: &str, trigger: &str| {
            let mut options = HashMap::from([("description".to_string(), text("Talk"))]);
            if !trigger.is_empty() {
                options.insert("preferred_trigger".to_string(), text(trigger));
            }
            (id.to_string(), options)
        };
        let wanted = wanted(vec![
            ask("talk", "CTRL+ALT+t"),
            ask("talk", "CTRL+ALT+x"),
            ask("mute", ""),
            ask("", "F9"),
        ]);
        assert_eq!(
            wanted,
            vec![
                Wanted {
                    id: "talk".into(),
                    description: "Talk".into(),
                    preferred: Some("CTRL+ALT+t".into()),
                },
                Wanted {
                    id: "mute".into(),
                    description: "Talk".into(),
                    preferred: None,
                },
            ]
        );
    }

    #[test]
    fn the_app_is_told_each_shortcut_and_its_keys() {
        let list = listed(&[Listed {
            id: "talk".into(),
            description: "Push to talk".into(),
            keys: "Ctrl+Alt+T".into(),
        }]);
        assert_eq!(list[0].0, "talk");
        assert_eq!(
            string(&list[0].1, "trigger_description").unwrap(),
            "Ctrl+Alt+T"
        );
        assert!(results(Some(&[])).contains_key("shortcuts"));
        assert!(results(None).is_empty());
    }
}

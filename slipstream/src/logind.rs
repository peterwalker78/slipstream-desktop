//! logind and the lock screen, in the login session.
//!
//! logind asks the session to lock (`loginctl lock-session`, or `lock-sessions` from anywhere) and
//! to unlock, and says when the machine is about to sleep. Someone locked out gets back in from a
//! text console with `loginctl unlock-session ID`, naming this session: without an ID, loginctl
//! unlocks the console's own session, and says nothing. The log names the ID at startup
//! (`logind: this session's lock and sleep`), and `loginctl list-sessions` lists it.
//! `loginctl unlock-sessions` unlocks every session, with an administrator's password. Slipstream tells it whether the session
//! is locked (`SetLockedHint`), and holds a delay inhibitor for sleep, so it can lock and draw the
//! lock before the machine goes down: it wakes up locked.
//!
//! All of it is on threads of its own, each blocking on the system bus, and every signal comes back
//! to the event loop through a channel. A signal only counts when logind itself sent it: any
//! process on the system bus can emit a signal named `Unlock`, and it is the sender the bus
//! stamps on it, not what it says, that shows where it came from.

use std::{
    os::fd::OwnedFd,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use smithay::reexports::calloop::channel::Sender;

/// logind answers locally, at once; a bus that doesn't gets a deadline rather than a thread that
/// waits for ever.
const TIMEOUT: Duration = Duration::from_secs(2);
const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
/// How long a screen has to draw the lock after logind says the machine is about to sleep.
pub const SLEEP_GRACE: f64 = 1.0;

/// What logind said, for this session.
#[derive(Debug)]
pub enum Event {
    Lock,
    Unlock,
    /// The machine is about to sleep (`true`), or has woken (`false`).
    Sleep(bool),
    /// A delay inhibitor for sleep, to hold until the lock is on screen.
    Inhibitor(OwnedFd),
    /// logind couldn't be reached, so sleep won't wait for the lock, and neither `loginctl`
    /// command reaches this session.
    Unreachable,
}

/// What the session does about an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Lock,
    Unlock,
    /// Lock, and let go of the sleep inhibitor once every screen has drawn it.
    LockForSleep,
    /// Let go of the sleep inhibitor now.
    Release,
    /// Take a new sleep inhibitor, for the next time.
    Reinhibit,
    Hold,
    /// Say that logind can't be reached.
    Warn,
}

/// The session's response to `event`, with the before-sleep setting `before_sleep`.
pub fn respond(event: &Event, before_sleep: bool) -> Response {
    match event {
        Event::Lock => Response::Lock,
        Event::Unlock => Response::Unlock,
        Event::Sleep(true) if before_sleep => Response::LockForSleep,
        Event::Sleep(true) => Response::Release,
        Event::Sleep(false) => Response::Reinhibit,
        Event::Unreachable => Response::Warn,
        Event::Inhibitor(_) => Response::Hold,
    }
}

/// The sleep inhibitor, and the wait for the lock to be drawn before letting it go.
pub struct SleepDelay<F> {
    held: Option<F>,
    /// When the wait began, on wall time.
    waiting_since: Option<f64>,
}

impl<F> Default for SleepDelay<F> {
    fn default() -> Self {
        Self {
            held: None,
            waiting_since: None,
        }
    }
}

impl<F> SleepDelay<F> {
    /// A new inhibitor from logind. One already held is dropped, which gives it back.
    pub fn hold(&mut self, inhibitor: F) {
        self.held = Some(inhibitor);
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    /// The machine is about to sleep and the lock is up: wait for it to be drawn.
    pub fn wait(&mut self, now: f64) {
        if self.held.is_some() {
            self.waiting_since = Some(now);
        }
    }

    /// Lets the inhibitor go now, so the machine can sleep.
    pub fn release(&mut self) {
        self.waiting_since = None;
        self.held = None;
    }

    /// While waiting: lets go once every screen has drawn the lock, or at `SLEEP_GRACE` whatever
    /// has been drawn. Returns whether it let go.
    pub fn tick(&mut self, now: f64, every_screen_drawn: bool) -> bool {
        let Some(since) = self.waiting_since else {
            return false;
        };
        if every_screen_drawn || now - since >= SLEEP_GRACE {
            self.release();
            return true;
        }
        false
    }

    pub fn waiting(&self) -> bool {
        self.waiting_since.is_some()
    }
}

/// What the event loop asks of logind.
enum Command {
    LockedHint(bool),
    Inhibit,
}

/// The session's line to logind. Without a system bus, or nested, it has none and asks nothing.
#[derive(Default)]
pub struct Logind {
    commands: Option<mpsc::Sender<Command>>,
}

impl Logind {
    /// Connects on a thread of its own; events come back through `events`.
    pub fn start(events: Sender<Event>) -> Self {
        let (commands, inbox) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("logind".into())
            .spawn(move || run(events, inbox));
        if let Err(err) = spawned {
            tracing::warn!("couldn't start talking to logind: {err}");
            return Self::default();
        }
        // The first sleep inhibitor, before anything can ask to sleep.
        let _ = commands.send(Command::Inhibit);
        Self {
            commands: Some(commands),
        }
    }

    /// Tells logind whether the session is locked.
    pub fn locked_hint(&self, locked: bool) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::LockedHint(locked));
        }
    }

    /// Asks for a new sleep inhibitor.
    pub fn inhibit(&self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Inhibit);
        }
    }
}

/// The logind thread: finds this session, listens for its signals on another thread, and
/// carries out commands until the compositor goes.
fn run(events: Sender<Event>, inbox: mpsc::Receiver<Command>) {
    let connection = zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(TIMEOUT))
        .and_then(|builder| builder.build());
    let connection = match connection {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!("no system bus, so logind can't lock or unlock the session: {err}");
            let _ = events.send(Event::Unreachable);
            return;
        }
    };
    let session = match session_path(&connection) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!("logind doesn't know this session: {err}");
            let _ = events.send(Event::Unreachable);
            return;
        }
    };
    let owner = match logind_owner(&connection) {
        Ok(owner) => Owner::new(owner),
        Err(err) => {
            tracing::warn!("couldn't find logind on the system bus: {err}");
            let _ = events.send(Event::Unreachable);
            return;
        }
    };
    // The plain ID is what `loginctl unlock-session` needs from a text console.
    let id = session_id(&connection, &session).unwrap_or_default();
    tracing::info!(
        id = id.as_str(),
        session = session.as_str(),
        "logind: this session's lock and sleep"
    );
    let watching = {
        let (connection, owner) = (connection.clone(), owner.clone());
        std::thread::Builder::new()
            .name("logind-owner".into())
            .spawn(move || follow_owner(&connection, &owner))
    };
    // logind may have restarted between finding its name and following it: look once more now
    // that it's followed.
    if let Ok(now) = logind_owner(&connection) {
        owner.changed(LOGIND, &now);
    }
    let listening = watching.and_then(|_| {
        let (connection, session, owner, events) = (
            connection.clone(),
            session.clone(),
            owner.clone(),
            events.clone(),
        );
        std::thread::Builder::new()
            .name("logind-signals".into())
            .spawn(move || listen(&connection, &session, &owner, &events))
    });
    if let Err(err) = listening {
        tracing::warn!("couldn't listen to logind: {err}");
        let _ = events.send(Event::Unreachable);
        return;
    }
    for command in inbox {
        match command {
            Command::LockedHint(locked) => {
                let set = connection.call_method(
                    Some(LOGIND),
                    session.as_str(),
                    Some(SESSION),
                    "SetLockedHint",
                    &locked,
                );
                if let Err(err) = set {
                    tracing::warn!(locked, "logind didn't take the locked hint: {err}");
                }
            }
            Command::Inhibit => match inhibit(&connection) {
                Ok(fd) => {
                    let _ = events.send(Event::Inhibitor(fd));
                }
                Err(err) => {
                    tracing::warn!("no sleep inhibitor, so sleep may not wait for the lock: {err}")
                }
            },
        }
    }
}

/// This process's session object.
fn session_path(connection: &zbus::blocking::Connection) -> zbus::Result<String> {
    let reply = connection.call_method(
        Some(LOGIND),
        MANAGER_PATH,
        Some(MANAGER),
        "GetSessionByPID",
        &std::process::id(),
    )?;
    let path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize()?;
    Ok(path.as_str().to_string())
}

/// The session's plain ID, as `loginctl list-sessions` shows it.
fn session_id(connection: &zbus::blocking::Connection, session: &str) -> zbus::Result<String> {
    let reply = connection.call_method(
        Some(LOGIND),
        session,
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &(SESSION, "Id"),
    )?;
    let id: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
    Ok(String::try_from(id).unwrap_or_default())
}

/// The unique bus name logind speaks from now, which a signal's sender has to be. It changes if
/// logind restarts, and `follow_owner` keeps it up to date.
#[derive(Clone, Default)]
pub struct Owner(Arc<Mutex<String>>);

impl Owner {
    pub fn new(name: String) -> Self {
        Self(Arc::new(Mutex::new(name)))
    }

    pub fn get(&self) -> String {
        self.0.lock().unwrap().clone()
    }

    /// The bus says `name` moved to `new`: follow it if it's logind. An empty `new` means logind
    /// has gone, and nothing counts until it's back.
    pub fn changed(&self, name: &str, new: &str) {
        if name == LOGIND {
            tracing::info!(owner = new, "logind's bus name changed");
            *self.0.lock().unwrap() = new.to_string();
        }
    }
}

/// Follows logind to a new bus name when it restarts. The match rule names the bus as the sender,
/// but a well-known name in a rule isn't checked against who really sent a message, so the
/// explicit sender check below is what keeps another client from forging a `NameOwnerChanged`.
fn follow_owner(connection: &zbus::blocking::Connection, owner: &Owner) {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|rule| rule.interface("org.freedesktop.DBus"))
        .and_then(|rule| rule.member("NameOwnerChanged"))
        .and_then(|rule| rule.arg(0, LOGIND))
        .map(|rule| rule.build());
    let messages = rule.and_then(|rule| {
        zbus::blocking::MessageIterator::for_match_rule(rule, connection, Some(8))
    });
    let messages = match messages {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!("couldn't follow logind's bus name: {err}");
            return;
        }
    };
    for message in messages.flatten() {
        if message.header().sender().map(|name| name.as_str()) != Some("org.freedesktop.DBus") {
            continue;
        }
        if let Ok((name, _old, new)) = message.body().deserialize::<(String, String, String)>() {
            owner.changed(&name, &new);
        }
    }
}

/// The unique bus name logind speaks from, which a signal's sender has to be.
fn logind_owner(connection: &zbus::blocking::Connection) -> zbus::Result<String> {
    let reply = connection.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &LOGIND,
    )?;
    reply.body().deserialize::<String>()
}

fn inhibit(connection: &zbus::blocking::Connection) -> zbus::Result<OwnedFd> {
    let reply = connection.call_method(
        Some(LOGIND),
        MANAGER_PATH,
        Some(MANAGER),
        "Inhibit",
        &(
            "sleep",
            "Slipstream",
            "Lock the screen before sleeping",
            "delay",
        ),
    )?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
    Ok(OwnedFd::from(fd))
}

/// What a signal on the system bus means for this session, if anything: only one logind sent,
/// on this session's object or the manager's.
pub fn signal_event(
    sender: Option<&str>,
    path: Option<&str>,
    interface: Option<&str>,
    member: Option<&str>,
    sleeping: impl FnOnce() -> Option<bool>,
    owner: &Owner,
    session: &str,
) -> Option<Event> {
    let owner = owner.get();
    if owner.is_empty() || sender != Some(owner.as_str()) {
        return None;
    }
    match (path?, interface?, member?) {
        (path, SESSION, "Lock") if path == session => Some(Event::Lock),
        (path, SESSION, "Unlock") if path == session => Some(Event::Unlock),
        (MANAGER_PATH, MANAGER, "PrepareForSleep") => sleeping().map(Event::Sleep),
        _ => None,
    }
}

/// Passes logind's signals for this session on to the event loop, for as long as the bus is up.
/// The rule doesn't name a sender, since logind's name can change; `signal_event` checks each
/// signal against the name logind has now.
fn listen(
    connection: &zbus::blocking::Connection,
    session: &str,
    owner: &Owner,
    events: &Sender<Event>,
) {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path_namespace(MANAGER_PATH)
        .map(|rule| rule.build());
    let messages = rule.and_then(|rule| {
        zbus::blocking::MessageIterator::for_match_rule(rule, connection, Some(64))
    });
    let messages = match messages {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!("couldn't listen to logind: {err}");
            let _ = events.send(Event::Unreachable);
            return;
        }
    };
    for message in messages {
        let Ok(message) = message else {
            continue;
        };
        let header = message.header();
        let event = signal_event(
            header.sender().map(|name| name.as_str()),
            header.path().map(|path| path.as_str()),
            header.interface().map(|name| name.as_str()),
            header.member().map(|name| name.as_str()),
            || message.body().deserialize::<bool>().ok(),
            owner,
            session,
        );
        if let Some(event) = event {
            tracing::info!(?event, "logind");
            if events.send(event).is_err() {
                return;
            }
        }
    }
    tracing::warn!("logind's signals stopped");
    let _ = events.send(Event::Unreachable);
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = ":1.7";
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_32";

    fn signal(sender: &str, path: &str, interface: &str, member: &str) -> Option<Event> {
        signal_event(
            Some(sender),
            Some(path),
            Some(interface),
            Some(member),
            || Some(true),
            &Owner::new(OWNER.into()),
            SESSION_PATH,
        )
    }

    #[test]
    fn signals_follow_logind_to_a_new_bus_name() {
        let owner = Owner::new(OWNER.into());
        let unlock = |sender: &str, owner: &Owner| {
            signal_event(
                Some(sender),
                Some(SESSION_PATH),
                Some(SESSION),
                Some("Unlock"),
                || None,
                owner,
                SESSION_PATH,
            )
        };
        assert!(unlock(OWNER, &owner).is_some());
        // logind restarts: gone, then back under another name.
        owner.changed(LOGIND, "");
        assert!(unlock(OWNER, &owner).is_none());
        assert!(
            unlock("", &owner).is_none(),
            "nobody counts while it's gone"
        );
        owner.changed(LOGIND, ":1.40");
        assert!(unlock(":1.40", &owner).is_some());
        assert!(
            unlock(OWNER, &owner).is_none(),
            "the old name counts no more"
        );
        // Another name moving changes nothing.
        owner.changed("org.example.Locker", ":1.99");
        assert!(unlock(":1.40", &owner).is_some());
        assert_eq!(respond(&Event::Unreachable, true), Response::Warn);
    }

    #[test]
    fn lock_and_unlock_signals_drive_the_lock() {
        // A channel stands in for the bus: what logind sends, in order, and what comes of it.
        let (bus, heard) = mpsc::channel();
        for event in [
            signal(OWNER, SESSION_PATH, SESSION, "Lock"),
            signal(OWNER, SESSION_PATH, SESSION, "Unlock"),
            signal(OWNER, MANAGER_PATH, MANAGER, "PrepareForSleep"),
        ]
        .into_iter()
        .flatten()
        {
            bus.send(event).unwrap();
        }
        drop(bus);
        let responses: Vec<Response> = heard.iter().map(|event| respond(&event, true)).collect();
        assert_eq!(
            responses,
            [Response::Lock, Response::Unlock, Response::LockForSleep]
        );
        assert_eq!(respond(&Event::Sleep(true), false), Response::Release);
        assert_eq!(respond(&Event::Sleep(false), true), Response::Reinhibit);

        // Anyone else on the bus, or another session's object, unlocks nothing.
        assert!(signal(":1.99", SESSION_PATH, SESSION, "Unlock").is_none());
        assert!(
            signal(
                OWNER,
                "/org/freedesktop/login1/session/_33",
                SESSION,
                "Unlock"
            )
            .is_none()
        );
        assert!(signal(OWNER, SESSION_PATH, "org.example.Session", "Unlock").is_none());
        assert!(
            signal_event(
                None,
                Some(SESSION_PATH),
                Some(SESSION),
                Some("Unlock"),
                || None,
                &Owner::new(OWNER.into()),
                SESSION_PATH
            )
            .is_none()
        );
    }

    #[test]
    fn sleep_waits_for_a_frame_then_releases() {
        let mut delay = SleepDelay::default();
        assert!(!delay.tick(0.0, true), "nothing held, nothing to wait for");
        delay.hold("inhibitor");
        delay.wait(10.0);
        assert!(delay.waiting());
        assert!(!delay.tick(10.2, false), "no screen has drawn the lock yet");
        assert!(delay.held());
        assert!(delay.tick(10.3, true), "every screen has");
        assert!(!delay.held() && !delay.waiting());

        delay.hold("inhibitor");
        delay.wait(20.0);
        assert!(!delay.tick(20.0 + SLEEP_GRACE - 0.01, false));
        assert!(
            delay.tick(20.0 + SLEEP_GRACE, false),
            "never longer than the grace"
        );
        assert!(!delay.held());
    }
}

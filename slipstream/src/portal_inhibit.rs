//! The inhibit portal's backend, `org.freedesktop.impl.portal.Inhibit`: how a sandboxed app asks
//! the desktop to stay awake while it plays a film, or not to end the session while it writes a
//! disc. xdg-desktop-portal passes each request on here, and only a backend that can see the
//! desktop can honour it.
//!
//! What each flag holds off:
//!
//! - Idle: the same as a Wayland idle inhibitor (`idle.rs`): no fade, no lock by itself, the
//!   screens stay on. An inhibitor counts only while its window is on screen, so a hold counts
//!   only while one of the app's windows is. An app with no window at all is taken at its word,
//!   as `org.freedesktop.ScreenSaver` takes X11 apps.
//! - Logout: the way out names the app and its reason on the card that asks first.
//! - User switch and suspend: nothing. Slipstream has no switching of users and never suspends by
//!   itself, and a suspend someone asks for is never refused.
//!
//! A hold lasts until xdg-desktop-portal closes its request, which it does when the app asks or
//! leaves the bus. If xdg-desktop-portal itself leaves, everything it asked for goes with it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{
    interface,
    message::Header,
    object_server::{Interface, ObjectServer, SignalEmitter},
    zvariant::{OwnedObjectPath, OwnedValue},
};

/// The flags this desktop has something to hold off for.
const LOGOUT: u32 = 1;
const IDLE: u32 = 8;

/// The portal's answers to CreateMonitor.
const SUCCESS: u32 = 0;
const FAILED: u32 = 2;

/// The most holds kept for one app, which can't otherwise be told apart: every request comes from
/// xdg-desktop-portal.
const PER_APP: usize = 16;

/// One app's hold, as the event loop sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// The app's ID, which its windows carry too.
    pub app: String,
    /// The reason the app gave, for the way out. Often empty.
    pub reason: String,
    pub idle: bool,
    pub logout: bool,
}

impl Hold {
    pub fn new(app: &str, flags: u32, reason: &str) -> Self {
        Self {
            app: app.chars().take(255).collect(),
            reason: reason.chars().take(200).collect(),
            idle: flags & IDLE != 0,
            logout: flags & LOGOUT != 0,
        }
    }

    /// Whether it holds off anything here at all.
    fn holds(&self) -> bool {
        self.idle || self.logout
    }
}

/// Whether an app's idle hold counts, given whether each of its windows is on screen: while one
/// of them is, or always for an app with none.
pub fn counts(shown: impl IntoIterator<Item = bool>) -> bool {
    let mut windows = false;
    for on_screen in shown {
        if on_screen {
            return true;
        }
        windows = true;
    }
    !windows
}

/// Every request and monitor, by the path xdg-desktop-portal gave it, with the connection that
/// made it.
#[derive(Debug, Default)]
struct Table {
    holds: HashMap<String, (String, Hold)>,
    monitors: HashMap<String, String>,
}

impl Table {
    /// Keeps `hold` at `path`; false if its app already has `PER_APP`.
    fn add(&mut self, path: &str, owner: &str, hold: Hold) -> bool {
        let app = &hold.app;
        if self.holds.values().filter(|(_, h)| h.app == *app).count() >= PER_APP {
            return false;
        }
        self.holds
            .insert(path.to_string(), (owner.to_string(), hold));
        true
    }

    fn remove(&mut self, path: &str) -> bool {
        self.holds.remove(path).is_some()
    }

    /// Lets go of everything `owner` asked for, now it has left the bus: the paths of its requests
    /// and of its monitors.
    fn drop_owner(&mut self, owner: &str) -> (Vec<String>, Vec<String>) {
        let requests: Vec<String> = self
            .holds
            .iter()
            .filter(|(_, (who, _))| who == owner)
            .map(|(path, _)| path.clone())
            .collect();
        let monitors: Vec<String> = self
            .monitors
            .iter()
            .filter(|(_, who)| *who == owner)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &requests {
            self.holds.remove(path);
        }
        for path in &monitors {
            self.monitors.remove(path);
        }
        (requests, monitors)
    }

    /// The holds that hold anything, in a steady order, so the way out's list doesn't shuffle.
    fn holds(&self) -> Vec<Hold> {
        let mut holds: Vec<(&String, &Hold)> = self
            .holds
            .iter()
            .filter(|(_, (_, hold))| hold.holds())
            .map(|(path, (_, hold))| (path, hold))
            .collect();
        holds.sort_by(|a, b| a.0.cmp(b.0));
        holds.into_iter().map(|(_, hold)| hold.clone()).collect()
    }
}

/// The table, and where every change to it goes: the event loop, as the whole list.
#[derive(Clone)]
struct Shared {
    table: Arc<Mutex<Table>>,
    events: Sender<Vec<Hold>>,
}

impl Shared {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn changed(&self) {
        let holds = self.table().holds();
        let _ = self.events.send(holds);
    }
}

fn sender(header: &Header<'_>) -> zbus::fdo::Result<String> {
    header
        .sender()
        .map(|name| name.to_string())
        .ok_or_else(|| zbus::fdo::Error::Failed("no sender".into()))
}

/// Takes the object at `path` off the bus once the call closing it has returned, rather than from
/// inside it.
fn take_off<I: Interface>(connection: &zbus::Connection, server: &ObjectServer, path: &str) {
    let (server, path) = (server.clone(), path.to_string());
    connection
        .executor()
        .spawn(
            async move {
                let _ = server.remove::<I, _>(path.as_str()).await;
            },
            "closing an inhibit request",
        )
        .detach();
}

/// The backend, served at the portal's object path.
#[derive(Clone)]
pub struct Backend {
    shared: Shared,
}

impl Backend {
    /// A backend whose holds go to the event loop through `events`, as the whole list each time.
    pub fn new(events: Sender<Vec<Hold>>) -> Self {
        Self {
            shared: Shared {
                table: Arc::default(),
                events,
            },
        }
    }

    /// `owner` has left the bus: what it asked for goes, and comes off the bus.
    pub fn gone(&self, connection: &zbus::blocking::Connection, owner: &str) {
        let (requests, monitors) = self.shared.table().drop_owner(owner);
        let server = connection.object_server();
        for path in &requests {
            let _ = server.remove::<Request, _>(path.as_str());
        }
        for path in &monitors {
            let _ = server.remove::<Monitor, _>(path.as_str());
        }
        if !requests.is_empty() {
            tracing::info!(
                count = requests.len(),
                "the inhibit portal has gone, and its holds with it"
            );
            self.shared.changed();
        }
    }
}

#[interface(name = "org.freedesktop.impl.portal.Inhibit")]
impl Backend {
    #[allow(clippy::too_many_arguments)]
    async fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _window: String,
        flags: u32,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let owner = sender(&header)?;
        let reason = options
            .get("reason")
            .and_then(|value| <&str>::try_from(value).ok())
            .unwrap_or_default();
        let path = handle.as_str().to_string();
        if !self
            .shared
            .table()
            .add(&path, &owner, Hold::new(&app_id, flags, reason))
        {
            return Err(zbus::fdo::Error::LimitsExceeded("too many holds".into()));
        }
        let request = Request {
            path: path.clone(),
            shared: self.shared.clone(),
        };
        if !server.at(&handle, request).await.unwrap_or(false) {
            self.shared.table().remove(&path);
            return Err(zbus::fdo::Error::Failed(
                "couldn't export the request".into(),
            ));
        }
        tracing::info!(app = %app_id, flags, "an app asked the desktop to hold off");
        self.shared.changed();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_monitor(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        _window: String,
    ) -> u32 {
        let Ok(owner) = sender(&header) else {
            return FAILED;
        };
        let path = session_handle.as_str().to_string();
        let monitor = Monitor {
            path: path.clone(),
            shared: self.shared.clone(),
        };
        if !server.at(&session_handle, monitor).await.unwrap_or(false) {
            return FAILED;
        }
        // No StateChanged ever follows: the session only ever runs, and the way out closes
        // windows rather than asking apps first, so a monitor has nothing to hear.
        self.shared.table().monitors.insert(path, owner);
        SUCCESS
    }

    /// Nothing waits on the answer: the session never asks apps whether it may end.
    async fn query_end_response(&self, _session_handle: OwnedObjectPath) {}
}

/// A hold, at the path xdg-desktop-portal chose for it, so it can be closed.
struct Request {
    path: String,
    shared: Shared,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    async fn close(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) {
        if self.shared.table().remove(&self.path) {
            tracing::info!("an app let go of its hold");
            self.shared.changed();
        }
        take_off::<Request>(connection, server, &self.path);
    }
}

/// A monitoring session, at the path xdg-desktop-portal chose for it, so it can be closed.
struct Monitor {
    path: String,
    shared: Shared,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Monitor {
    async fn close(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) {
        self.shared.table().monitors.remove(&self.path);
        take_off::<Monitor>(connection, server, &self.path);
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

impl crate::Slipstream {
    /// Every hold apps now have through the portal.
    pub fn portal_holds(&mut self, holds: Vec<Hold>) {
        self.idle.portal = holds;
        self.follow_idle_inhibitors();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERFACE: &str = "org.freedesktop.impl.portal.Inhibit";

    #[test]
    fn each_flag_holds_off_what_this_desktop_has() {
        let idle = Hold::new("org.example.Player", 8, "Playing");
        assert!(idle.idle && !idle.logout);
        let logout = Hold::new("org.example.Burner", 1, "Writing a disc");
        assert!(logout.logout && !logout.idle);
        let both = Hold::new("org.example.Burner", 1 | 2 | 4 | 8, "");
        assert!(both.idle && both.logout);
        // No switching of users and no suspending by itself: nothing to hold off.
        assert!(!Hold::new("org.example.App", 2 | 4, "").holds());
    }

    #[test]
    fn an_idle_hold_counts_while_one_of_its_windows_is_on_screen() {
        assert!(counts([false, true]));
        assert!(!counts([false, false]), "every window hidden");
        assert!(counts([]), "no window at all: taken at its word");
    }

    #[test]
    fn a_hold_ends_when_closed_or_when_the_portal_leaves() {
        let mut table = Table::default();
        let request = |n: u32| format!("/org/freedesktop/portal/desktop/request/1_2/t{n}");
        assert!(table.add(&request(1), ":1.5", Hold::new("org.example.A", 8, "")));
        assert!(table.add(&request(2), ":1.5", Hold::new("org.example.B", 1, "Saving")));
        assert!(table.add(&request(3), ":1.5", Hold::new("org.example.C", 4, "")));
        table.monitors.insert("/session/1".into(), ":1.5".into());
        let apps = |table: &Table| -> Vec<String> {
            table.holds().into_iter().map(|hold| hold.app).collect()
        };
        assert_eq!(apps(&table), ["org.example.A", "org.example.B"]);
        assert!(table.remove(&request(1)));
        assert!(!table.remove(&request(1)), "once only");
        assert_eq!(apps(&table), ["org.example.B"]);
        assert_eq!(table.drop_owner(":1.99"), (vec![], vec![]), "a stranger");
        let (mut requests, monitors) = table.drop_owner(":1.5");
        requests.sort();
        assert_eq!(requests, [request(2), request(3)]);
        assert_eq!(monitors, ["/session/1"]);
        assert!(table.holds().is_empty());
    }

    #[test]
    fn one_app_cannot_pile_up_holds_without_end() {
        let mut table = Table::default();
        for n in 0..PER_APP {
            assert!(table.add(&format!("/r/{n}"), ":1.5", Hold::new("busy", 8, "")));
        }
        assert!(!table.add("/r/x", ":1.5", Hold::new("busy", 8, "")));
        assert!(table.add("/r/y", ":1.5", Hold::new("other", 8, "")));
    }

    /// The backend on a private bus, called as xdg-desktop-portal calls it: a hold reaches the
    /// event loop, and ends with Request.Close or with its caller leaving the bus. Run it under
    /// `dbus-run-session`, never on a real bus.
    #[test]
    #[ignore = "needs a private bus: dbus-run-session -- cargo test -- --ignored"]
    fn holds_end_with_close_or_the_caller_leaving() {
        use smithay::reexports::calloop::{EventLoop, channel};
        use std::time::{Duration, Instant};
        use zbus::zvariant::{ObjectPath, Value};

        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(
            !address.is_empty() && !address.contains("/run/user/"),
            "not on a private bus: {address}"
        );
        let (sender, events) = channel::channel();
        let backend = Backend::new(sender);
        let name = "org.example.SlipstreamInhibitTest";
        let path = crate::portal::PATH;
        let service = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(name))
            .and_then(|builder| builder.serve_at(path, backend.clone()))
            .and_then(|builder| builder.build())
            .unwrap();
        {
            let (service, backend) = (service.clone(), backend.clone());
            std::thread::spawn(move || {
                crate::screensaver::follow_departures(&service, |gone| {
                    backend.gone(&service, gone)
                });
            });
        }
        let mut event_loop: EventLoop<'static, Vec<Vec<Hold>>> = EventLoop::try_new().unwrap();
        event_loop
            .handle()
            .insert_source(events, |event, _, got| {
                if let channel::Event::Msg(holds) = event {
                    got.push(holds);
                }
            })
            .unwrap();
        let mut got = Vec::new();
        let mut wait_for = |count: usize, got: &mut Vec<Vec<Hold>>| {
            let started = Instant::now();
            while !got.iter().any(|holds| holds.len() == count)
                && started.elapsed() < Duration::from_secs(5)
            {
                event_loop
                    .dispatch(Some(Duration::from_millis(20)), got)
                    .unwrap();
            }
            assert!(
                got.iter().any(|holds| holds.len() == count),
                "never {count} holds: {got:?}"
            );
            got.clear();
        };
        let inhibit = |caller: &zbus::blocking::Connection, request: &str, flags: u32| {
            let options = HashMap::from([("reason", Value::from("Playing"))]);
            let handle = ObjectPath::try_from(request).unwrap();
            caller
                .call_method(
                    Some(name),
                    path,
                    Some(INTERFACE),
                    "Inhibit",
                    &(handle, "org.example.Player", "", flags, options),
                )
                .unwrap();
        };

        let portal = zbus::blocking::Connection::session().unwrap();
        inhibit(&portal, "/org/freedesktop/portal/desktop/request/1_9/a", 8);
        wait_for(1, &mut got);
        portal
            .call_method(
                Some(name),
                "/org/freedesktop/portal/desktop/request/1_9/a",
                Some("org.freedesktop.impl.portal.Request"),
                "Close",
                &(),
            )
            .unwrap();
        wait_for(0, &mut got);

        // A monitor answers at once, and a hold left when the caller goes goes with it.
        let response: u32 = portal
            .call_method(
                Some(name),
                path,
                Some(INTERFACE),
                "CreateMonitor",
                &(
                    ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_9/m").unwrap(),
                    ObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_9/m").unwrap(),
                    "org.example.Player",
                    "",
                ),
            )
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        assert_eq!(response, SUCCESS);
        inhibit(
            &portal,
            "/org/freedesktop/portal/desktop/request/1_9/b",
            1 | 8,
        );
        wait_for(1, &mut got);
        drop(portal);
        wait_for(0, &mut got);
    }
}

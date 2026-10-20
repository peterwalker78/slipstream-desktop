//! `org.freedesktop.ScreenSaver` on the session bus: how X11 games, Steam and Electron apps ask
//! for the screen to stay awake, where Wayland apps use an idle inhibitor. A hold here counts
//! exactly as an inhibitor does (`idle.rs`): the UI doesn't fade, the screen doesn't lock by
//! itself, and the screens stay on.
//!
//! Each hold belongs to the bus connection that asked for it. Only that connection can let it go,
//! and when the connection closes (the app quits or crashes) every hold it had goes with it, so a
//! game that dies mid-match can't keep the screen awake for good.
//!
//! The interface is served at both paths apps use: `/org/freedesktop/ScreenSaver` and the older
//! `/ScreenSaver`.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{interface, message::Header, names::BusName};

const NAME: &str = "org.freedesktop.ScreenSaver";
const PATHS: [&str; 2] = ["/org/freedesktop/ScreenSaver", "/ScreenSaver"];
/// The most holds kept for one connection; a program asking for more is refused rather than
/// being allowed to grow the table without end.
const PER_CLIENT: usize = 16;

/// The connection, kept open while the session runs.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Whether the desktop has stepped aside for the wallpaper or locked, for `GetActive`, which is
/// answered on the bus's thread.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Kept up to date by the event loop as the UI fades and comes back.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
}

/// What reaches the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// How many holds there are now.
    Holds(usize),
    /// An app said someone is there, as input would.
    Activity,
}

/// One hold: whose it is, and the name it gave, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hold {
    owner: String,
    app: String,
}

/// Every hold, by cookie.
#[derive(Debug, Default)]
struct Holds {
    held: HashMap<u32, Hold>,
    next: u32,
}

impl Holds {
    /// A new hold for `owner`, and its cookie; `None` if it already has `PER_CLIENT`.
    fn add(&mut self, owner: &str, app: &str) -> Option<u32> {
        if self
            .held
            .values()
            .filter(|hold| hold.owner == owner)
            .count()
            >= PER_CLIENT
        {
            return None;
        }
        // Never 0, which some apps take to mean "not held".
        loop {
            self.next = self.next.wrapping_add(1);
            if self.next != 0 && !self.held.contains_key(&self.next) {
                break;
            }
        }
        self.held.insert(
            self.next,
            Hold {
                owner: owner.to_string(),
                app: app.chars().take(64).collect(),
            },
        );
        Some(self.next)
    }

    /// Lets go of `cookie`, if `owner` holds it: one app can't end another's.
    fn remove(&mut self, cookie: u32, owner: &str) -> bool {
        if self
            .held
            .get(&cookie)
            .is_some_and(|hold| hold.owner == owner)
        {
            self.held.remove(&cookie);
            return true;
        }
        false
    }

    /// Lets go of everything `owner` held, now it has left the bus.
    fn drop_owner(&mut self, owner: &str) -> bool {
        let before = self.held.len();
        self.held.retain(|_, hold| hold.owner != owner);
        self.held.len() != before
    }

    fn count(&self) -> usize {
        self.held.len()
    }
}

/// The shared table and where changes go: one per session, served at both paths.
#[derive(Clone)]
struct Server {
    holds: Arc<Mutex<Holds>>,
    events: Arc<Mutex<Sender<Event>>>,
}

impl Server {
    fn holds(&self) -> std::sync::MutexGuard<'_, Holds> {
        self.holds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, event: Event) {
        let _ = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(event);
    }

    /// The owner of `name` has gone: its holds go too.
    fn gone(&self, name: &str) {
        let mut holds = self.holds();
        if holds.drop_owner(name) {
            tracing::info!(
                count = holds.count(),
                "an app holding the screen awake has quit"
            );
            let count = holds.count();
            drop(holds);
            self.send(Event::Holds(count));
        }
    }
}

#[interface(name = "org.freedesktop.ScreenSaver")]
impl Server {
    async fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        application_name: String,
        _reason_for_inhibit: String,
    ) -> zbus::fdo::Result<u32> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("no sender".into()))?
            .to_string();
        let (cookie, count) = {
            let mut holds = self.holds();
            let cookie = holds
                .add(&owner, &application_name)
                .ok_or_else(|| zbus::fdo::Error::LimitsExceeded("too many holds".into()))?;
            (cookie, holds.count())
        };
        tracing::info!(app = %application_name, count, "an app is holding the screen awake");
        self.send(Event::Holds(count));
        // The app may have gone between asking and now, and its leaving been seen first; its
        // hold would then outlive it.
        let still_there = match zbus::fdo::DBusProxy::new(connection).await {
            Ok(bus) => match BusName::try_from(owner.as_str()) {
                Ok(name) => bus.name_has_owner(name).await.unwrap_or(true),
                Err(_) => true,
            },
            Err(_) => true,
        };
        if !still_there {
            self.gone(&owner);
        }
        Ok(cookie)
    }

    async fn un_inhibit(&self, #[zbus(header)] header: Header<'_>, cookie: u32) {
        let Some(owner) = header.sender().map(|name| name.to_string()) else {
            return;
        };
        let count = {
            let mut holds = self.holds();
            if !holds.remove(cookie, &owner) {
                return;
            }
            holds.count()
        };
        tracing::info!(count, "an app let the screen sleep again");
        self.send(Event::Holds(count));
    }

    async fn get_active(&self) -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    async fn simulate_user_activity(&self) {
        self.send(Event::Activity);
    }
}

/// Takes the name on the session bus and serves it, sending changes to `events`. If another
/// desktop's screensaver service has the name, it says so in the log and gives up.
pub fn serve(events: Sender<Event>) {
    std::thread::spawn(move || {
        let server = Server {
            holds: Arc::default(),
            events: Arc::new(Mutex::new(events)),
        };
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(NAME))
            .and_then(|builder| builder.serve_at(PATHS[0], server.clone()))
            .and_then(|builder| builder.serve_at(PATHS[1], server.clone()))
            .and_then(|builder| builder.build());
        let connection = match connection {
            Ok(connection) => connection,
            Err(err) => {
                tracing::warn!(
                    "couldn't serve {NAME} (another desktop's may have the name), so X11 apps \
                     can't keep the screen awake: {err}"
                );
                return;
            }
        };
        tracing::info!("serving {NAME} on the session bus");
        let _ = CONNECTION.set(connection.clone());
        follow_departures(&connection, &server);
    });
}

/// Ends the holds of every connection that leaves the bus, for as long as the session runs. Only
/// the bus itself can say a name has gone; a signal claiming it from anyone else is ignored.
fn follow_departures(connection: &zbus::blocking::Connection, server: &Server) {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|rule| rule.interface("org.freedesktop.DBus"))
        .and_then(|rule| rule.member("NameOwnerChanged"))
        .map(|rule| rule.build());
    let messages = rule.and_then(|rule| {
        zbus::blocking::MessageIterator::for_match_rule(rule, connection, Some(64))
    });
    let messages = match messages {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!("couldn't watch for apps quitting, so their screen holds stay: {err}");
            return;
        }
    };
    for message in messages.flatten() {
        if message.header().sender().map(|name| name.as_str()) != Some("org.freedesktop.DBus") {
            continue;
        }
        if let Ok((name, _old, new)) = message.body().deserialize::<(String, String, String)>() {
            // A unique name losing its owner is a connection closing.
            if new.is_empty() && name.starts_with(':') {
                server.gone(&name);
            }
        }
    }
}

impl crate::Slipstream {
    pub fn screensaver_event(&mut self, event: Event) {
        match event {
            Event::Holds(count) => {
                self.idle.bus_holds = count;
                self.follow_idle_inhibitors();
            }
            // Not at the lock: an app mustn't keep the screens lit for whoever walks past.
            Event::Activity if self.lock.is_none() => {
                self.wake_ui();
            }
            Event::Activity => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_is_only_let_go_by_its_owner() {
        let mut holds = Holds::default();
        let game = holds.add(":1.10", "Game").unwrap();
        let player = holds.add(":1.11", "Player").unwrap();
        assert_ne!(game, player);
        assert_ne!(game, 0);
        assert!(!holds.remove(game, ":1.11"), "another app can't end it");
        assert_eq!(holds.count(), 2);
        assert!(holds.remove(game, ":1.10"));
        assert!(!holds.remove(game, ":1.10"), "once only");
        assert_eq!(holds.count(), 1);
    }

    #[test]
    fn an_app_that_quits_takes_its_holds_with_it() {
        let mut holds = Holds::default();
        holds.add(":1.10", "Game").unwrap();
        holds.add(":1.10", "Game").unwrap();
        holds.add(":1.11", "Player").unwrap();
        assert!(holds.drop_owner(":1.10"));
        assert_eq!(holds.count(), 1);
        assert!(
            !holds.drop_owner(":1.99"),
            "a stranger leaving changes nothing"
        );
    }

    #[test]
    fn one_app_cannot_pile_up_holds_without_end() {
        let mut holds = Holds::default();
        for _ in 0..PER_CLIENT {
            assert!(holds.add(":1.10", "Busy").is_some());
        }
        assert!(holds.add(":1.10", "Busy").is_none());
        assert!(holds.add(":1.11", "Other").is_some(), "others still can");
    }

    /// The service on a private bus: an app's hold is counted, only its own UnInhibit or its
    /// leaving the bus ends it. Run it under `dbus-run-session`, never on a real bus.
    #[test]
    #[ignore = "needs a private bus: dbus-run-session -- cargo test -- --ignored"]
    fn holds_end_with_uninhibit_or_the_app_leaving() {
        use smithay::reexports::calloop::{EventLoop, channel};
        use std::time::{Duration, Instant};

        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(
            !address.is_empty() && !address.contains("/run/user/"),
            "not on a private bus: {address}"
        );
        let (sender, events) = channel::channel();
        serve(sender);
        let mut event_loop: EventLoop<'static, Vec<Event>> = EventLoop::try_new().unwrap();
        event_loop
            .handle()
            .insert_source(events, |event, _, got| {
                if let channel::Event::Msg(event) = event {
                    got.push(event);
                }
            })
            .unwrap();
        let mut got = Vec::new();
        let mut wait_for = |want: Event, got: &mut Vec<Event>| {
            let started = Instant::now();
            while !got.contains(&want) && started.elapsed() < Duration::from_secs(5) {
                event_loop
                    .dispatch(Some(Duration::from_millis(20)), got)
                    .unwrap();
            }
            assert!(got.contains(&want), "no {want:?} in {got:?}");
            got.clear();
        };
        let call = |app: &zbus::blocking::Connection, path: &str, method: &str, body: &(u32,)| {
            app.call_method(Some(NAME), path, Some(NAME), method, body)
        };
        // Wait for the name.
        let game = zbus::blocking::Connection::session().unwrap();
        let started = Instant::now();
        let cookie: u32 = loop {
            match game.call_method(
                Some(NAME),
                PATHS[0],
                Some(NAME),
                "Inhibit",
                &("Game", "playing"),
            ) {
                Ok(reply) => break reply.body().deserialize().unwrap(),
                Err(_) if started.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(err) => panic!("never served: {err}"),
            }
        };
        wait_for(Event::Holds(1), &mut got);

        // Another app can't end the game's hold; the game can, at the older path too.
        let stranger = zbus::blocking::Connection::session().unwrap();
        call(&stranger, PATHS[0], "UnInhibit", &(cookie,)).unwrap();
        call(&game, PATHS[1], "UnInhibit", &(cookie,)).unwrap();
        wait_for(Event::Holds(0), &mut got);

        // A hold left when the app quits goes with it.
        let player = zbus::blocking::Connection::session().unwrap();
        player
            .call_method(
                Some(NAME),
                PATHS[1],
                Some(NAME),
                "Inhibit",
                &("Player", "video"),
            )
            .unwrap();
        wait_for(Event::Holds(1), &mut got);
        drop(player);
        wait_for(Event::Holds(0), &mut got);

        game.call_method(
            Some(NAME),
            PATHS[0],
            Some(NAME),
            "SimulateUserActivity",
            &(),
        )
        .unwrap();
        wait_for(Event::Activity, &mut got);
        let active: bool = game
            .call_method(Some(NAME), PATHS[0], Some(NAME), "GetActive", &())
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        assert!(!active);
    }
}

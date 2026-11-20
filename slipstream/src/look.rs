//! Looking at the window that holds a process, on the session bus: the app you're using asks
//! Slipstream to show you another window for a moment, without the keyboard leaving it.
//!
//! `Start(u pid)` finds the window above process `pid`, as a notification finds the window it
//! came from, brings it into view with the usual workspace slide, raises it and rings it in the
//! focus colour. The keyboard stays where it was, so the asking app still hears the key it is
//! waiting on. `End(b stay)` either goes back to where you were or makes the looked-at window the
//! one you're using. `Ended(b stayed)` says a look is over, whichever way it ended.
//!
//! Only the app holding the keyboard may ask, so nothing in the background can move the screen
//! about: the process behind the call, read from the bus, has to be the one whose window has the
//! keyboard, a process it started, or one that started it.

use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{interface, message::Header, object_server::SignalEmitter};

pub const NAME: &str = "io.github.peterwalker78.Slipstream";
pub const PATH: &str = "/io/github/peterwalker78/Slipstream";
const INTERFACE: &str = "io.github.peterwalker78.Slipstream.Look";

/// How long a look lasts if nothing ends it.
pub const LONGEST: Duration = Duration::from_secs(60);

/// The connection, once the name is ours, for sending `Ended`.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// What reaches the event loop. `caller` is the process behind the call and its ancestors,
/// nearest first, empty when that couldn't be told for certain.
#[derive(Debug)]
pub enum Event {
    Start {
        caller: Vec<u32>,
        pid: u32,
        reply: async_channel::Sender<Result<(), Refusal>>,
    },
    End {
        caller: Vec<u32>,
        stay: bool,
        reply: async_channel::Sender<bool>,
    },
}

/// Why a look was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Locked,
    NotInUse,
    NoWindow,
}

impl Refusal {
    /// What the caller is told.
    pub fn sentence(self) -> &'static str {
        match self {
            Refusal::Locked => "The screen is locked.",
            Refusal::NotInUse => "Only the app you're using can show you another window.",
            Refusal::NoWindow => "No window holds that process.",
        }
    }
}

/// Whether the caller owns the window with the keyboard. `caller` is the calling process and its
/// ancestors, `holder` the process behind that window and its ancestors, `compositor` this
/// process and its ancestors, all nearest first. The caller owns it if it is that process or was
/// started under it (a command run in a terminal), or if that process was started under the
/// caller. The processes the session itself runs under are above every app, so being an
/// ancestor only counts below the compositor.
pub fn owns(caller: &[u32], holder: &[u32], compositor: &[u32]) -> bool {
    let (Some(me), Some(owner)) = (caller.first(), holder.first()) else {
        return false;
    };
    caller.contains(owner) || (holder.contains(me) && !compositor.contains(me))
}

/// Whether `Start` may go ahead, before the window is looked for.
pub fn may_start(
    locked: bool,
    caller: &[u32],
    holder: &[u32],
    compositor: &[u32],
) -> Result<(), Refusal> {
    if locked {
        return Err(Refusal::Locked);
    }
    if !owns(caller, holder, compositor) {
        return Err(Refusal::NotInUse);
    }
    Ok(())
}

/// What a look depends on, read once a pass.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    /// The window that asked still has the keyboard: it hasn't closed, and nothing else has
    /// taken the keyboard from it.
    pub caller_has_keyboard: bool,
    /// The window being looked at is still open.
    pub target_open: bool,
    pub locked: bool,
    /// How long the look has been going.
    pub elapsed: Duration,
}

/// Why a look ended by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Locked,
    KeyboardMoved,
    TargetClosed,
    TimedOut,
}

impl Why {
    /// Whether the view goes back to where it was. It does when the keyboard is still with the
    /// window that asked; when the keyboard has gone elsewhere, or the lock is up, the view stays
    /// with whatever took it.
    pub fn goes_back(self) -> bool {
        matches!(self, Why::TargetClosed | Why::TimedOut)
    }
}

/// Whether, and why, a look is over.
pub fn ended(facts: Facts) -> Option<Why> {
    if facts.locked {
        Some(Why::Locked)
    } else if !facts.caller_has_keyboard {
        Some(Why::KeyboardMoved)
    } else if !facts.target_open {
        Some(Why::TargetClosed)
    } else if facts.elapsed >= LONGEST {
        Some(Why::TimedOut)
    } else {
        None
    }
}

struct Server {
    events: Mutex<Sender<Event>>,
}

impl Server {
    fn send(&self, event: Event) -> bool {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(event)
            .is_ok()
    }
}

#[interface(name = "io.github.peterwalker78.Slipstream.Look")]
impl Server {
    async fn start(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        pid: u32,
    ) -> (bool, String) {
        let caller = crate::notify::origin(connection, &header).await;
        let (reply, answer) = async_channel::bounded(1);
        if !self.send(Event::Start { caller, pid, reply }) {
            return (false, "Slipstream isn't running.".into());
        }
        match answer.recv().await {
            Ok(Ok(())) => (true, "Showing the window.".into()),
            Ok(Err(refusal)) => (false, refusal.sentence().into()),
            Err(_) => (false, "Slipstream isn't running.".into()),
        }
    }

    async fn end(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        stay: bool,
    ) -> bool {
        let caller = crate::notify::origin(connection, &header).await;
        let (reply, answer) = async_channel::bounded(1);
        if !self.send(Event::End {
            caller,
            stay,
            reply,
        }) {
            return false;
        }
        answer.recv().await.unwrap_or(false)
    }

    #[zbus(signal)]
    async fn ended(emitter: &SignalEmitter<'_>, stayed: bool) -> zbus::Result<()>;
}

/// Takes the name on the session bus and serves the interface, sending requests to `events`.
pub fn serve(events: Sender<Event>) {
    std::thread::spawn(move || {
        let server = Server {
            events: Mutex::new(events),
        };
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(NAME))
            .and_then(|builder| builder.serve_at(PATH, server))
            .and_then(|builder| builder.build());
        match connection {
            Ok(connection) => {
                tracing::info!("serving {NAME} on the session bus");
                let _ = CONNECTION.set(connection);
            }
            Err(err) => tracing::warn!(
                "couldn't serve {NAME} (another Slipstream may have the name), so apps can't \
                 show you another window: {err}"
            ),
        }
    });
}

/// Tells whoever listens that a look is over. Sent from a thread of its own, so the main loop
/// never waits on the bus.
pub fn send_ended(stayed: bool) {
    let Some(connection) = CONNECTION.get().cloned() else {
        return;
    };
    std::thread::spawn(move || {
        if let Err(err) = connection.emit_signal(None::<&str>, PATH, INTERFACE, "Ended", &(stayed,))
        {
            tracing::warn!("couldn't send Ended: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session: the user manager (10) started the compositor (20), which started a terminal
    /// (30), whose shell (31) runs a tool (32). Another app (40) also runs under the compositor.
    const COMPOSITOR: [u32; 2] = [20, 10];
    const TERMINAL: [u32; 3] = [30, 20, 10];
    const TOOL: [u32; 5] = [32, 31, 30, 20, 10];
    const OTHER: [u32; 3] = [40, 20, 10];

    #[test]
    fn the_app_with_the_keyboard_may_look() {
        assert!(owns(&TERMINAL, &TERMINAL, &COMPOSITOR));
        assert_eq!(may_start(false, &TERMINAL, &TERMINAL, &COMPOSITOR), Ok(()));
    }

    #[test]
    fn a_command_run_in_it_may_look_for_it() {
        assert!(owns(&TOOL, &TERMINAL, &COMPOSITOR));
    }

    #[test]
    fn so_may_the_process_that_started_it() {
        // The terminal's window has the keyboard; the program that opened it asks.
        assert!(owns(&TERMINAL, &TOOL, &COMPOSITOR));
    }

    #[test]
    fn another_app_may_not() {
        assert!(!owns(&OTHER, &TERMINAL, &COMPOSITOR));
        assert_eq!(
            may_start(false, &OTHER, &TERMINAL, &COMPOSITOR),
            Err(Refusal::NotInUse)
        );
    }

    #[test]
    fn nor_may_the_session_every_app_runs_under() {
        assert!(!owns(&COMPOSITOR, &TERMINAL, &COMPOSITOR));
        assert!(!owns(&[10], &TERMINAL, &COMPOSITOR));
    }

    #[test]
    fn a_caller_the_bus_couldnt_name_may_not() {
        assert!(!owns(&[], &TERMINAL, &COMPOSITOR));
        assert!(!owns(&TERMINAL, &[], &COMPOSITOR));
    }

    #[test]
    fn nothing_is_looked_at_behind_the_lock() {
        assert_eq!(
            may_start(true, &TERMINAL, &TERMINAL, &COMPOSITOR),
            Err(Refusal::Locked)
        );
    }

    fn going() -> Facts {
        Facts {
            caller_has_keyboard: true,
            target_open: true,
            locked: false,
            elapsed: Duration::from_secs(1),
        }
    }

    #[test]
    fn a_look_goes_on_while_nothing_changes() {
        assert_eq!(ended(going()), None);
    }

    #[test]
    fn a_look_ends_when_the_keyboard_moves() {
        let facts = Facts {
            caller_has_keyboard: false,
            ..going()
        };
        assert_eq!(ended(facts), Some(Why::KeyboardMoved));
        assert!(
            !Why::KeyboardMoved.goes_back(),
            "the view stays with what took the keyboard"
        );
    }

    #[test]
    fn a_look_ends_at_the_lock_and_stays_put() {
        let facts = Facts {
            locked: true,
            caller_has_keyboard: false,
            ..going()
        };
        assert_eq!(ended(facts), Some(Why::Locked));
        assert!(!Why::Locked.goes_back());
    }

    #[test]
    fn a_look_ends_when_its_window_closes_and_goes_back() {
        let facts = Facts {
            target_open: false,
            ..going()
        };
        assert_eq!(ended(facts), Some(Why::TargetClosed));
        assert!(Why::TargetClosed.goes_back());
    }

    #[test]
    fn a_look_ends_after_a_minute_and_goes_back() {
        let almost = Facts {
            elapsed: LONGEST - Duration::from_millis(1),
            ..going()
        };
        assert_eq!(ended(almost), None);
        let facts = Facts {
            elapsed: LONGEST,
            ..going()
        };
        assert_eq!(ended(facts), Some(Why::TimedOut));
        assert!(Why::TimedOut.goes_back());
    }
}

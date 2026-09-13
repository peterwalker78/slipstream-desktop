//! Taking logind's lid switch off it while another screen is connected.
//!
//! By default `systemd-logind` suspends the machine when the lid shuts, which would make
//! "close the laptop and carry on on the monitor" impossible however the compositor behaved. A
//! desktop that wants to decide for itself takes a `handle-lid-switch` inhibitor and holds the
//! file descriptor logind hands back; the lid then arrives as an ordinary libinput switch event
//! and `udev.rs` puts the panel out. The descriptor is dropped as soon as the last external
//! screen goes, so a laptop on its own suspends the way its owner set it up to.

use std::{
    os::fd::OwnedFd,
    sync::{Arc, Mutex},
    time::Duration,
};

use smithay::reexports::calloop::channel::Sender;

/// logind answers locally, at once. A bus that doesn't answer in this long leaves the lid to
/// logind rather than a thread waiting on it for ever.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Where logind's answer goes: back to the event loop in the session.
type Answer = Arc<Mutex<dyn FnMut(Option<OwnedFd>) + Send>>;

/// Held while the compositor is the one deciding what the lid means.
pub struct LidInhibitor {
    held: Option<OwnedFd>,
    /// Whether a screen that isn't the laptop's panel is connected, so the lid should be ours.
    wanted: bool,
    /// A request is on its way to logind, on a thread of its own.
    asking: bool,
    /// Asks logind for the inhibitor: the system bus in the session, a stand-in in tests.
    ask: fn() -> Option<OwnedFd>,
    answer: Answer,
}

impl LidInhibitor {
    /// An inhibitor whose answers from logind come back to the event loop through `answers`, to be
    /// passed to `answered`.
    pub fn new(answers: Sender<Option<OwnedFd>>) -> Self {
        Self::with(
            take,
            Arc::new(Mutex::new(move |fd| {
                let _ = answers.send(fd);
            })),
        )
    }

    fn with(ask: fn() -> Option<OwnedFd>, answer: Answer) -> Self {
        Self {
            held: None,
            wanted: false,
            asking: false,
            ask,
            answer,
        }
    }

    /// Asks logind for the inhibitor when `wanted` and it isn't held, and gives it back when it
    /// isn't wanted. The call goes on a thread, so a system bus that doesn't answer never holds up
    /// the desktop; until the answer arrives, the lid is logind's, as it was.
    pub fn set(&mut self, wanted: bool) {
        self.wanted = wanted;
        if wanted && self.held.is_none() && !self.asking {
            self.asking = true;
            let (ask, answer) = (self.ask, self.answer.clone());
            std::thread::spawn(move || {
                let fd = ask();
                (answer.lock().unwrap())(fd);
            });
        } else if !wanted && self.held.take().is_some() {
            tracing::info!("logind has the lid switch back");
        }
    }

    /// logind's answer to the request `set` made. If the other screen went while logind was
    /// asked, the descriptor is dropped here, which gives the lid straight back.
    pub fn answered(&mut self, fd: Option<OwnedFd>) {
        self.asking = false;
        if let (true, Some(fd)) = (self.wanted, fd) {
            self.held = Some(fd);
            tracing::info!("logind's lid switch is ours while another screen is connected");
        }
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }
}

/// One call on the system bus, with a deadline, off the event loop.
fn take() -> Option<OwnedFd> {
    let connection = zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(TIMEOUT))
        .and_then(|builder| builder.build());
    let connection = match connection {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!("no system bus, so the lid stays logind's: {err}");
            return None;
        }
    };
    let reply = connection.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &(
            "handle-lid-switch",
            "Slipstream",
            "Another screen is connected",
            "block",
        ),
    );
    match reply {
        Ok(reply) => match reply.body().deserialize::<zbus::zvariant::OwnedFd>() {
            Ok(fd) => Some(OwnedFd::from(fd)),
            Err(err) => {
                tracing::warn!("logind's inhibitor came back without a descriptor: {err}");
                None
            }
        },
        Err(err) => {
            tracing::warn!("logind wouldn't give up the lid switch: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn descriptor() -> Option<OwnedFd> {
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().ok()?;
        Some(ours.into())
    }

    fn inhibitor(ask: fn() -> Option<OwnedFd>) -> (LidInhibitor, mpsc::Receiver<Option<OwnedFd>>) {
        let (sender, answers) = mpsc::channel();
        let answer: Answer = Arc::new(Mutex::new(move |fd| {
            let _ = sender.send(fd);
        }));
        (LidInhibitor::with(ask, answer), answers)
    }

    fn wait(answers: &mpsc::Receiver<Option<OwnedFd>>) -> Option<OwnedFd> {
        answers
            .recv_timeout(Duration::from_secs(2))
            .expect("logind's stand-in answers")
    }

    #[test]
    fn the_lid_is_held_only_once_logind_answers() {
        let (mut lid, answers) = inhibitor(descriptor);
        lid.set(true);
        assert!(!lid.held(), "nothing is held while the question is out");
        lid.set(true);
        let fd = wait(&answers);
        assert!(
            answers.recv_timeout(Duration::from_millis(100)).is_err(),
            "asked once"
        );
        lid.answered(fd);
        assert!(lid.held());
        lid.set(false);
        assert!(!lid.held());
    }

    #[test]
    fn an_answer_nobody_wants_any_more_gives_the_lid_back() {
        let (mut lid, answers) = inhibitor(descriptor);
        lid.set(true);
        lid.set(false);
        lid.answered(wait(&answers));
        assert!(!lid.held(), "the screen went while logind was asked");
        let (mut lid, answers) = inhibitor(|| None);
        lid.set(true);
        lid.answered(wait(&answers));
        assert!(!lid.held(), "a refusal leaves the lid to logind");
        lid.set(true);
        assert!(wait(&answers).is_none(), "and a later screen asks again");
    }
}

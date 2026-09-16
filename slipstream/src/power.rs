//! Sleep, Restart and Shut down, handed to logind, and what happens when logind says no.
//!
//! The request goes to logind on the system bus — `Suspend`, `Reboot` and `PowerOff` on
//! `org.freedesktop.login1.Manager` — which systemd-logind and elogind both answer. `loginctl`
//! once had the same three as commands of its own, and newer systemd has dropped them, so asking
//! it would fail on a machine where logind itself is perfectly willing.
//!
//! logind can refuse: another session is logged in and there is no agent to ask for a password, or
//! something holds a shutdown inhibitor (a system update, say). By then the way out has closed
//! every app and faded the screen to black, so a refusal that nobody hears of leaves a black,
//! empty session with no way back. The call is made on a thread of its own, and what logind said
//! comes back to the event loop, which clears the way out and says what happened.
//!
//! A nested compositor never asks logind: a stand-in logs the request and grants it, and the
//! compositor then stops as Log out does, or refuses it the way an inhibitor would, so that path
//! can be checked without a person at the laptop.

use crate::{
    exit::Intent,
    logind::{LOGIND, MANAGER, MANAGER_PATH, TIMEOUT},
};

/// What logind is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Sleep,
    Restart,
    ShutDown,
}

impl Power {
    pub fn for_intent(intent: Intent) -> Option<Self> {
        match intent {
            Intent::LogOut => None,
            Intent::Restart => Some(Power::Restart),
            Intent::ShutDown => Some(Power::ShutDown),
        }
    }

    /// The method on logind's manager, which systemd-logind and elogind share.
    pub fn method(self) -> &'static str {
        match self {
            Power::Sleep => "Suspend",
            Power::Restart => "Reboot",
            Power::ShutDown => "PowerOff",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Power::Sleep => "Sleep",
            Power::Restart => "Restart",
            Power::ShutDown => "Shut down",
        }
    }

    /// Whether every app has already been asked to close by the time this runs.
    pub fn ends_the_session(self) -> bool {
        self != Power::Sleep
    }
}

/// logind turned the request down, or it couldn't be made at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub power: Power,
    /// What logind said, or why it couldn't be asked: one line.
    pub reason: String,
}

impl Refused {
    /// The toast's title and body.
    pub fn message(&self) -> (String, String) {
        let title = format!("{} was refused", self.power.name());
        let mut body = format!("{}.", self.reason.trim_end_matches('.'));
        if self.power.ends_the_session() {
            body.push_str(" The apps were already closed; the desktop is still here.");
        }
        (title, body)
    }
}

/// Who answers a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answerer {
    /// logind, on the system bus.
    Logind,
    /// A nested run's stand-in, which asks nothing: it grants the request, or with `refuse`
    /// turns it down as an inhibitor would (`SLIPSTREAM_POWER_REFUSE=1`).
    StandIn { refuse: bool },
}

impl Answerer {
    /// The stand-in while machine commands are held, else logind.
    pub fn current() -> Self {
        if crate::launch::machine_commands_held() {
            Answerer::StandIn {
                refuse: std::env::var("SLIPSTREAM_POWER_REFUSE").as_deref() == Ok("1"),
            }
        } else {
            Answerer::Logind
        }
    }
}

/// What a request comes back with. A request logind grants comes back with nothing: logind
/// sleeps the machine or ends the session itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Refused(Refused),
    /// The stand-in granted it, and nothing else will act on it.
    StoodIn(Power),
}

/// Asks for `power` on a thread and calls `answered` there with anything to report. `ask` reaches
/// logind in the session and stands in for it in tests; the stand-in answerer never calls it.
pub fn request(
    power: Power,
    answerer: Answerer,
    ask: impl FnOnce(Power) -> Result<(), String> + Send + 'static,
    answered: impl FnOnce(Answer) + Send + 'static,
) {
    std::thread::spawn(move || {
        let result = match answerer {
            Answerer::Logind => ask(power),
            Answerer::StandIn { refuse } => stand_in(power, refuse),
        };
        match result {
            Err(reason) => {
                tracing::warn!(method = power.method(), reason, "logind refused");
                answered(Answer::Refused(Refused { power, reason }));
            }
            Ok(()) if answerer != Answerer::Logind => answered(Answer::StoodIn(power)),
            Ok(()) => {}
        }
    });
}

/// What logind would have said, without asking it.
fn stand_in(power: Power, refuse: bool) -> Result<(), String> {
    tracing::info!(method = power.method(), "nested: not asking logind");
    if refuse {
        Err("Operation inhibited by \"test\"".to_string())
    } else {
        Ok(())
    }
}

/// Asks logind, and waits for its answer. Not interactive: with no agent to ask for a password
/// there is nothing to wait on, and an answer that comes back at once is what the way out needs.
pub fn ask(power: Power) -> Result<(), String> {
    let connection = zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(TIMEOUT))
        .and_then(|builder| builder.build())
        .map_err(|err| format!("logind couldn't be reached: {err}"))?;
    connection
        .call_method(
            Some(LOGIND),
            MANAGER_PATH,
            Some(MANAGER),
            power.method(),
            &false,
        )
        .map(|_| ())
        .map_err(refusal)
}

/// One line for the toast: what logind said, else the error it raised.
fn refusal(err: zbus::Error) -> String {
    match err {
        zbus::Error::MethodError(name, description, _) => {
            description.unwrap_or_else(|| name.as_str().to_string())
        }
        err => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    /// `request` with a stand-in for logind, waiting for its answer.
    fn answer(
        power: Power,
        result: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Option<Refused> {
        let (sender, receiver) = mpsc::channel();
        let asked = sender.clone();
        request(
            power,
            Answerer::Logind,
            move |wanted| {
                let _ = asked.send(Err(wanted));
                result()
            },
            move |answer| {
                let _ = sender.send(Ok(answer));
            },
        );
        let first = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(first.err(), Some(power), "logind is asked first");
        match receiver
            .recv_timeout(Duration::from_millis(300))
            .ok()?
            .ok()?
        {
            Answer::Refused(refusal) => Some(refusal),
            Answer::StoodIn(_) => panic!("logind's answers never come from the stand-in"),
        }
    }

    /// `request` with the stand-in, and an ask that must never be made.
    fn stood_in(power: Power, refuse: bool) -> Answer {
        let (sender, receiver) = mpsc::channel();
        request(
            power,
            Answerer::StandIn { refuse },
            |wanted| panic!("{wanted:?} was asked of logind in a nested run"),
            move |answer| {
                let _ = sender.send(answer);
            },
        );
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the stand-in answers")
    }

    #[test]
    fn a_nested_run_asks_nothing_of_logind() {
        for power in [Power::Restart, Power::ShutDown, Power::Sleep] {
            assert_eq!(stood_in(power, false), Answer::StoodIn(power));
        }
        let Answer::Refused(refusal) = stood_in(Power::Restart, true) else {
            panic!("SLIPSTREAM_POWER_REFUSE turns it down");
        };
        assert_eq!(refusal.reason, "Operation inhibited by \"test\"");
        assert!(Power::Restart.ends_the_session() && !Power::Sleep.ends_the_session());
    }

    #[test]
    fn a_refused_restart_comes_back_with_what_logind_said() {
        let refusal = answer(Power::Restart, || {
            Err("Operation inhibited by \"rpm-ostree\" (PID 812).".to_string())
        })
        .expect("a failure is reported");
        assert_eq!(refusal.power, Power::Restart);
        let (title, body) = refusal.message();
        assert_eq!(title, "Restart was refused");
        assert_eq!(
            body,
            "Operation inhibited by \"rpm-ostree\" (PID 812). The apps were already closed; the \
             desktop is still here."
        );
    }

    #[test]
    fn a_request_that_goes_through_says_nothing() {
        assert_eq!(answer(Power::ShutDown, || Ok(())), None);
    }

    #[test]
    fn a_request_that_cannot_be_made_still_explains_itself() {
        let refusal = answer(Power::Sleep, || {
            Err("logind couldn't be reached: no system bus".to_string())
        })
        .unwrap();
        assert!(refusal.reason.starts_with("logind couldn't be reached"));
        assert!(
            !refusal.message().1.contains("apps"),
            "sleep closes nothing"
        );
    }

    #[test]
    fn the_requests_are_logind_s_own() {
        assert_eq!(Power::Restart.method(), "Reboot");
        assert_eq!(Power::ShutDown.method(), "PowerOff");
        assert_eq!(Power::Sleep.method(), "Suspend");
        assert_eq!(Power::for_intent(Intent::LogOut), None);
    }
}

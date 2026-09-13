//! Sleep, Restart and Shut down, handed to logind, and what happens when logind says no.
//!
//! `loginctl reboot` can be refused: another session is logged in and there is no agent to ask
//! for a password, or something holds a shutdown inhibitor (a system update, say). By then the way
//! out has closed every app and faded the screen to black, so a refusal that nobody hears of
//! leaves a black, empty session with no way back. The command runs on its own thread, and its
//! exit status comes back to the event loop, which clears the way out and says what happened.
//!
//! A nested compositor never asks logind: a stand-in logs the command and grants it, and the
//! compositor then stops as Log out does, or refuses it the way an inhibitor would, so that path
//! can be checked without a person at the laptop.

use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output, Stdio},
};

use crate::exit::Intent;

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

    /// The command, which works with systemd-logind and elogind alike.
    pub fn command(self) -> Vec<String> {
        let verb = match self {
            Power::Sleep => "suspend",
            Power::Restart => "reboot",
            Power::ShutDown => "poweroff",
        };
        vec!["loginctl".to_string(), verb.to_string()]
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
    /// What loginctl said, or why it couldn't run: one line.
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

/// What came of running the command: nothing to report, or a refusal.
pub fn outcome(power: Power, result: std::io::Result<Output>) -> Option<Refused> {
    let reason = match result {
        Ok(output) if output.status.success() => return None,
        Ok(output) => String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("loginctl stopped with {}", output.status)),
        Err(err) => format!("loginctl couldn't be run: {err}"),
    };
    Some(Refused { power, reason })
}

/// Who answers a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answerer {
    /// logind, through the runner.
    Logind,
    /// A nested run's stand-in, which runs nothing: it grants the request, or with `refuse`
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

/// Runs `power`'s command on a thread and calls `answered` there with anything to report.
/// `runner` is the real command in the session and a stand-in in tests; the stand-in answerer
/// never calls it.
pub fn request(
    power: Power,
    answerer: Answerer,
    runner: impl FnOnce(&[String]) -> std::io::Result<Output> + Send + 'static,
    answered: impl FnOnce(Answer) + Send + 'static,
) {
    std::thread::spawn(move || {
        let command = power.command();
        let result = match answerer {
            Answerer::Logind => runner(&command),
            Answerer::StandIn { refuse } => stand_in(&command, refuse),
        };
        match outcome(power, result) {
            Some(refusal) => {
                tracing::warn!(?command, reason = refusal.reason, "logind refused");
                answered(Answer::Refused(refusal));
            }
            None if answerer != Answerer::Logind => answered(Answer::StoodIn(power)),
            None => {}
        }
    });
}

/// What loginctl would have said, without running it.
fn stand_in(command: &[String], refuse: bool) -> std::io::Result<Output> {
    tracing::info!("nested: not running {command:?}");
    let (code, stderr) = if refuse {
        (1, "Operation inhibited by \"test\"")
    } else {
        (0, "")
    };
    Ok(Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

/// Runs a command to completion, keeping what it printed on its error stream.
pub fn run(command: &[String]) -> std::io::Result<Output> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| std::io::Error::other("no command"))?;
    crate::launch::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    fn output(code: i32, stderr: &str) -> std::io::Result<Output> {
        Ok(Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// `request` with a stand-in for loginctl, waiting for its answer.
    fn answer(
        power: Power,
        result: impl FnOnce() -> std::io::Result<Output> + Send + 'static,
    ) -> Option<Refused> {
        let (sender, receiver) = mpsc::channel();
        let asked = sender.clone();
        request(
            power,
            Answerer::Logind,
            move |command| {
                let _ = asked.send(Err(command.join(" ")));
                result()
            },
            move |answer| {
                let _ = sender.send(Ok(answer));
            },
        );
        let command = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(command.is_err(), "the runner is asked first");
        match receiver
            .recv_timeout(Duration::from_millis(300))
            .ok()?
            .ok()?
        {
            Answer::Refused(refusal) => Some(refusal),
            Answer::StoodIn(_) => panic!("logind's answers never come from the stand-in"),
        }
    }

    /// `request` with the stand-in, and a runner that must never be called.
    fn stood_in(power: Power, refuse: bool) -> Answer {
        let (sender, receiver) = mpsc::channel();
        request(
            power,
            Answerer::StandIn { refuse },
            |command| panic!("{command:?} ran in a nested run"),
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
            output(
                1,
                "\nCall to Reboot failed: Operation inhibited by \"rpm-ostree\" (PID 812).\n",
            )
        })
        .expect("a failure is reported");
        assert_eq!(refusal.power, Power::Restart);
        let (title, body) = refusal.message();
        assert_eq!(title, "Restart was refused");
        assert_eq!(
            body,
            "Call to Reboot failed: Operation inhibited by \"rpm-ostree\" (PID 812). The apps \
             were already closed; the desktop is still here."
        );
    }

    #[test]
    fn a_request_that_goes_through_says_nothing() {
        assert_eq!(answer(Power::ShutDown, || output(0, "")), None);
    }

    #[test]
    fn a_command_that_cannot_run_or_says_nothing_still_explains_itself() {
        let refusal = answer(Power::Sleep, || {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap();
        assert!(refusal.reason.starts_with("loginctl couldn't be run"));
        assert!(
            !refusal.message().1.contains("apps"),
            "sleep closes nothing"
        );
        let silent = outcome(Power::ShutDown, output(1, "")).unwrap();
        assert!(silent.reason.contains("stopped with"), "{}", silent.reason);
    }

    #[test]
    fn the_commands_are_logind_s() {
        assert_eq!(Power::Restart.command(), ["loginctl", "reboot"]);
        assert_eq!(Power::ShutDown.command(), ["loginctl", "poweroff"]);
        assert_eq!(Power::Sleep.command(), ["loginctl", "suspend"]);
        assert_eq!(Power::for_intent(Intent::LogOut), None);
    }
}

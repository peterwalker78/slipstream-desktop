//! Talking to polkit's own helper, which does the actual checking.
//!
//! The agent never checks a password itself, and nothing it says is believed by polkit. It hands
//! the user's name and polkit's cookie to `polkit-agent-helper-1`, which runs the PAM
//! conversation as root and tells polkit itself whether it went through. The helper is reached
//! the way polkit's own agent library reaches it: through `/run/polkit/agent-helper.socket`
//! where systemd listens on it (the name, then the cookie, a line each), or else by running the
//! setuid program with the name as its argument and the cookie on its standard input.
//!
//! The conversation is a line at a time: `PAM_PROMPT_ECHO_OFF <prompt>` (a password) and
//! `PAM_PROMPT_ECHO_ON <prompt>` wait for a line back; `PAM_ERROR_MSG` and `PAM_TEXT_INFO` are
//! for showing; `SUCCESS` or `FAILURE` ends it. Messages come escaped as GLib's `g_strescape`
//! escapes them.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixStream},
    },
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
};

use zeroize::Zeroizing;

/// Where systemd listens for the helper, on systems that start it per connection.
const SOCKET: &str = "/run/polkit/agent-helper.socket";

/// Where distributions put the setuid helper. Fedora, Arch and Debian use the first; openSUSE
/// keeps it under libexec; older Debian and Ubuntu under `policykit-1`; NixOS has its setuid
/// wrappers in a directory of their own.
const HELPERS: [&str; 5] = [
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
    "/usr/lib/policykit-1/polkit-agent-helper-1",
    "/run/wrappers/bin/polkit-agent-helper-1",
];

/// The longest line read from the helper. PAM messages are short; anything longer isn't one.
const LINE_LIMIT: u64 = 4096;

/// What the helper said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// It is waiting for an answer: hidden as it's typed (a password) or shown.
    Asks {
        echo: bool,
        prompt: String,
    },
    Info(String),
    Error(String),
    Done(Verdict),
}

/// How a conversation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The helper told polkit the answer was right.
    Authenticated,
    /// The answer was wrong.
    Refused,
    /// The helper couldn't be run, stopped without saying, or said something it shouldn't.
    Failed,
}

/// Whether there is a helper to ask on this machine at all.
pub fn available() -> bool {
    Path::new(SOCKET).exists() || setuid_helper().is_some()
}

/// The first of `HELPERS` that is a setuid-root program. One that isn't could never check
/// anyone's password but its owner's, so it doesn't count.
fn setuid_helper() -> Option<&'static str> {
    HELPERS.into_iter().find(|path| {
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.uid() == 0 && meta.mode() & 0o4000 != 0)
    })
}

/// How a conversation is stopped from outside: the helper it is talking to is killed, or its
/// socket shut, which wakes the thread reading from it.
enum Stop {
    Process(OwnedFd),
    Socket(UnixStream),
}

/// A conversation under way, on a thread of its own. Dropping it ends it: the helper is stopped
/// and anything it says afterwards goes nowhere.
pub struct Conversation {
    answers: Sender<Zeroizing<Vec<u8>>>,
    stop: Option<Stop>,
}

impl Conversation {
    /// Answers the helper's question. The bytes are wiped once they've been written.
    pub fn answer(&self, text: Zeroizing<Vec<u8>>) {
        let _ = self.answers.send(text);
    }
}

impl Drop for Conversation {
    fn drop(&mut self) {
        match self.stop.take() {
            // SAFETY: a pidfd for the helper, taken before anything could reap it, so the signal
            // can only reach that process; no siginfo, no flags.
            Some(Stop::Process(pidfd)) => unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    libc::SIGTERM,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            },
            Some(Stop::Socket(stream)) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            None => {}
        }
    }
}

/// Starts checking `user` for polkit's `cookie`. What the helper says goes to `said`, from the
/// conversation's thread, ending with `Said::Done`. `None` when no helper could be reached.
pub fn start(
    user: &str,
    cookie: &str,
    said: impl Fn(Said) + Send + Sync + 'static,
) -> Option<Conversation> {
    // Either would end its line early and start another: a name or cookie that holds one isn't
    // what polkit meant.
    if !one_line(user) || !one_line(cookie) {
        return None;
    }
    let said: Tell = Arc::new(said);
    if let Some(conversation) = by_socket(user, cookie, said.clone()) {
        return Some(conversation);
    }
    by_program(user, cookie, said)
}

/// Where what the helper says goes, shared by the two ways of reaching it.
type Tell = Arc<dyn Fn(Said) + Send + Sync>;

fn one_line(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

fn by_socket(user: &str, cookie: &str, said: Tell) -> Option<Conversation> {
    if !Path::new(SOCKET).exists() {
        return None;
    }
    let mut stream = match UnixStream::connect(SOCKET) {
        Ok(stream) => stream,
        Err(err) => {
            tracing::warn!("couldn't reach polkit's helper at {SOCKET}, so running it: {err}");
            return None;
        }
    };
    let preamble = format!("{user}\n{cookie}\n");
    let reader = stream.try_clone().ok()?;
    let stop = stream.try_clone().ok()?;
    stream.write_all(preamble.as_bytes()).ok()?;
    let (answers, waiting) = mpsc::channel();
    std::thread::spawn(move || {
        let verdict = converse(BufReader::new(reader), stream, &waiting, &*said);
        said(Said::Done(verdict));
    });
    Some(Conversation {
        answers,
        stop: Some(Stop::Socket(stop)),
    })
}

fn by_program(user: &str, cookie: &str, said: Tell) -> Option<Conversation> {
    let Some(helper) = setuid_helper() else {
        tracing::warn!("polkit's helper isn't installed, so no password can be checked");
        return None;
    };
    let spawned = crate::launch::command(helper)
        .arg(user)
        // A pipe, never a terminal: the helper refuses to run on one.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!("couldn't run polkit's helper: {err}");
            return None;
        }
    };
    // Taken before the thread below can reap it, so the pid can't have been reused.
    let pidfd = crate::capture::pidfd_open(child.id());
    let (mut stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
    if stdin.write_all(format!("{cookie}\n").as_bytes()).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let (answers, waiting) = mpsc::channel();
    std::thread::spawn(move || {
        let verdict = converse(BufReader::new(stdout), stdin, &waiting, &*said);
        said(Said::Done(verdict));
        // Reaped once it has gone. Its standard input closed when `converse` returned.
        let _ = child.wait();
    });
    Some(Conversation {
        answers,
        stop: pidfd.map(Stop::Process),
    })
}

/// The conversation itself: reads what the helper says, passes it on, and answers its questions
/// from `answers`, until it says how it went. The writer is dropped on return, which tells a
/// helper still waiting that no answer is coming.
fn converse(
    mut reader: impl BufRead,
    mut writer: impl Write,
    answers: &Receiver<Zeroizing<Vec<u8>>>,
    said: &dyn Fn(Said),
) -> Verdict {
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader
            .by_ref()
            .take(LINE_LIMIT)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => return Verdict::Failed,
            Ok(_) => {}
        }
        if line.pop() != Some(b'\n') {
            // Cut off by the limit, or by the helper going away mid-line.
            return Verdict::Failed;
        }
        let text = String::from_utf8_lossy(&line);
        let asked = if let Some(prompt) = text.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
            Some((false, prompt))
        } else {
            text.strip_prefix("PAM_PROMPT_ECHO_ON ")
                .map(|prompt| (true, prompt))
        };
        if let Some((echo, prompt)) = asked {
            said(Said::Asks {
                echo,
                prompt: unescape(prompt),
            });
            let Ok(answer) = answers.recv() else {
                return Verdict::Failed;
            };
            // A newline would end the answer early and start another.
            if answer.contains(&b'\n') {
                return Verdict::Failed;
            }
            let sent = writer
                .write_all(&answer)
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush());
            drop(answer);
            if sent.is_err() {
                return Verdict::Failed;
            }
        } else if let Some(message) = text.strip_prefix("PAM_ERROR_MSG ") {
            said(Said::Error(unescape(message)));
        } else if let Some(message) = text.strip_prefix("PAM_TEXT_INFO ") {
            said(Said::Info(unescape(message)));
        } else if text == "SUCCESS" {
            return Verdict::Authenticated;
        } else if text == "FAILURE" {
            return Verdict::Refused;
        } else {
            return Verdict::Failed;
        }
    }
}

/// Undoes `g_strescape`: backslashed letters, quotes and backslashes, and octal bytes.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if byte != b'\\' || i == bytes.len() {
            out.push(byte);
            continue;
        }
        let next = bytes[i];
        i += 1;
        match next {
            b'0'..=b'7' => {
                let mut value = u32::from(next - b'0');
                for _ in 0..2 {
                    match bytes.get(i) {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(value as u8);
            }
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Plays the helper's side of a conversation from `script` on a socket pair: lines starting
    /// `>` are sent, lines starting `<` are what it expects back. Returns what it was sent.
    fn conversation(
        script: &'static str,
        answers: Vec<&'static str>,
    ) -> (Verdict, Vec<Said>, String) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let helper = std::thread::spawn(move || {
            let mut reader = BufReader::new(theirs.try_clone().unwrap());
            let mut writer = theirs;
            let mut heard = String::new();
            for line in script.lines() {
                if let Some(say) = line.strip_prefix('>') {
                    if writer.write_all(format!("{say}\n").as_bytes()).is_err() {
                        break;
                    }
                } else if line.starts_with('<') {
                    let mut got = String::new();
                    if reader.read_line(&mut got).unwrap_or(0) == 0 {
                        break;
                    }
                    heard.push_str(&got);
                }
            }
            heard
        });
        let (sender, waiting) = mpsc::channel();
        for answer in answers {
            sender
                .send(Zeroizing::new(answer.as_bytes().to_vec()))
                .unwrap();
        }
        drop(sender);
        let said = Arc::new(Mutex::new(Vec::new()));
        let log = said.clone();
        let verdict = converse(
            BufReader::new(ours.try_clone().unwrap()),
            ours,
            &waiting,
            &move |line| log.lock().unwrap().push(line),
        );
        let heard = helper.join().unwrap();
        let said = said.lock().unwrap().clone();
        (verdict, said, heard)
    }

    #[test]
    fn a_right_password_is_passed_on_and_ends_in_success() {
        let (verdict, said, heard) = conversation(
            ">PAM_PROMPT_ECHO_OFF Password: \n<\n>SUCCESS",
            vec!["hunter2"],
        );
        assert_eq!(verdict, Verdict::Authenticated);
        assert_eq!(heard, "hunter2\n");
        assert_eq!(
            said,
            vec![Said::Asks {
                echo: false,
                prompt: "Password: ".into()
            }]
        );
    }

    #[test]
    fn a_wrong_one_is_refused_and_messages_are_shown() {
        let (verdict, said, _) = conversation(
            ">PAM_TEXT_INFO Place your finger\\nor type\n>PAM_PROMPT_ECHO_OFF Password:\n<\n\
             >PAM_ERROR_MSG Sorry \\\"no\\\" \\303\\251\n>FAILURE",
            vec!["wrong"],
        );
        assert_eq!(verdict, Verdict::Refused);
        assert_eq!(said[0], Said::Info("Place your finger\nor type".into()));
        assert_eq!(said[2], Said::Error("Sorry \"no\" é".into()));
    }

    #[test]
    fn anything_unexpected_fails_rather_than_succeeds() {
        // Nonsense, a helper that goes away, and no answer to give it.
        assert_eq!(conversation(">WHATEVER", vec![]).0, Verdict::Failed);
        assert_eq!(conversation("", vec![]).0, Verdict::Failed);
        let (verdict, _, heard) = conversation(">PAM_PROMPT_ECHO_OFF Password:\n<", vec![]);
        assert_eq!(verdict, Verdict::Failed);
        assert_eq!(heard, "", "nothing is sent when there is nothing to send");
        // SUCCESS has to be the whole line.
        assert_eq!(conversation(">SUCCESSFUL", vec![]).0, Verdict::Failed);
    }

    #[test]
    fn an_answer_cannot_smuggle_in_a_second_line() {
        let (verdict, _, heard) = conversation(
            ">PAM_PROMPT_ECHO_OFF Password:\n<\n<\n>SUCCESS",
            vec!["one\ntwo"],
        );
        assert_eq!(verdict, Verdict::Failed);
        assert_eq!(heard, "");
    }

    #[test]
    fn names_and_cookies_are_one_line_each() {
        assert!(one_line("alex"));
        assert!(one_line("3-a1b2c3-1-d4e5"));
        assert!(!one_line(""));
        assert!(!one_line("alex\nroot"));
        assert!(start("alex", "a\nb", |_| {}).is_none());
    }

    #[test]
    fn escapes_are_undone() {
        assert_eq!(unescape(r"a\tb\\c\101\"), "a\tb\\cA\\");
    }
}

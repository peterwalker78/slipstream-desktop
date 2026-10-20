//! The session's polkit authentication agent: how a program that needs an administrator's say-so
//! (a software centre installing for everyone, `pkexec`, a firmware update) gets a password
//! asked for, instead of simply being refused.
//!
//! The agent registers itself with polkit on the system bus for this login session. polkit calls
//! `BeginAuthentication` with what the program wants to do and who may answer, and waits for the
//! reply; the card (`prompt.rs`) asks, and polkit's own setuid helper (`helper.rs`) checks the
//! password and tells polkit directly. Nothing the agent says is taken as proof of anything:
//! replying is only how the card says it has gone.
//!
//! zbus serves the bus from its own threads; each call waits there on its answer, so a slow
//! question never holds up a frame.

mod helper;
mod prompt;

use std::{
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_char},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};

use smithay::{
    desktop::Window,
    reexports::calloop::channel::Sender,
    utils::{IsAlive, Logical, Point},
};
use zbus::{
    interface,
    message::Header,
    names::BusName,
    zvariant::{OwnedValue, Value},
};

pub use helper::Said;
use helper::{Conversation, Verdict};
pub use prompt::{Act, Key, Prompt, key_for};

use crate::Slipstream;

const AGENT_PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";
const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
/// The longest message shown. polkit's own are a sentence; `pkexec` puts the command line in its.
const MESSAGE_CHARS: usize = 400;
/// The most people offered to answer, and the most questions waiting their turn behind the card.
const IDENTITIES: usize = 8;
const WAITING: usize = 8;

/// The connection, kept open for as long as the session runs.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Whether the card is up, for the log's filter, which reaches it with no `Slipstream` to hand:
/// a key logged while it is up would be a character of a password.
static ASKING: AtomicBool = AtomicBool::new(false);

pub fn asking() -> bool {
    ASKING.load(Ordering::Relaxed)
}

/// Someone polkit will take a password from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub uid: u32,
    /// The login name, which the check is for.
    pub name: String,
    /// The name from the password database's comment field, if it has one.
    pub full_name: String,
}

impl Identity {
    /// What the card calls them.
    pub fn shown_name(&self) -> &str {
        if self.full_name.is_empty() {
            &self.name
        } else {
            &self.full_name
        }
    }
}

/// How a question ended, for polkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The helper has told polkit the password was right.
    Done,
    Cancelled,
}

#[derive(Default)]
struct Slot {
    answer: Option<Answer>,
    waker: Option<Waker>,
}

fn lock_slot(slot: &Mutex<Slot>) -> std::sync::MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The answer to polkit's waiting call. Dropped without being sent, it answers "cancelled", so
/// whatever takes a question away (the lock, a full queue, the session ending) never leaves
/// polkit waiting for good.
struct Reply(Arc<Mutex<Slot>>);

impl Reply {
    fn send(self, answer: Answer) {
        lock_slot(&self.0).answer = Some(answer);
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        let mut slot = lock_slot(&self.0);
        slot.answer.get_or_insert(Answer::Cancelled);
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    }
}

/// Where the bus call waits for its `Reply`.
struct Waiting(Arc<Mutex<Slot>>);

impl Future for Waiting {
    type Output = Answer;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Answer> {
        let mut slot = lock_slot(&self.0);
        match slot.answer.take() {
            Some(answer) => Poll::Ready(answer),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn reply() -> (Reply, Waiting) {
    let slot = Arc::new(Mutex::new(Slot::default()));
    (Reply(slot.clone()), Waiting(slot))
}

/// A question from polkit.
pub struct Request {
    /// polkit's name for this question, which the helper hands back to it.
    pub cookie: String,
    /// What the program wants to do, in polkit's words.
    pub message: String,
    /// Who may answer: never empty.
    pub identities: Vec<Identity>,
    reply: Option<Reply>,
}

impl Request {
    /// A question nobody is waiting on, for the card's debug step and tests: answering it checks
    /// nothing and tells nobody.
    pub fn stand_in(message: String, identities: Vec<Identity>) -> Self {
        Self {
            cookie: String::new(),
            message,
            identities,
            reply: None,
        }
    }

    fn answer(mut self, answer: Answer) {
        if let Some(reply) = self.reply.take() {
            reply.send(answer);
        }
    }
}

/// What reaches the event loop.
pub enum Event {
    Begin(Box<Request>),
    /// polkit has given up on the question with this cookie.
    Cancel(String),
    /// The helper said something during check number `attempt`.
    Helper {
        attempt: u64,
        said: Said,
    },
}

/// The errors polkit understands from an agent.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum Refusal {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

struct Agent {
    events: Mutex<Sender<Event>>,
    /// Calls are taken only from polkit itself. Off only in tests, on a bus of two.
    polkit_only: bool,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), Refusal> {
        self.check_caller(connection, &header).await?;
        let identities = users(&identities);
        if identities.is_empty() {
            return Err(Refusal::Failed(
                "nobody polkit named has an account here".into(),
            ));
        }
        tracing::info!(action = %action_id, "a program asked for a password");
        let (reply, waiting) = reply();
        let request = Request {
            cookie,
            message: plain(&message),
            identities,
            reply: Some(reply),
        };
        // An event loop that has gone drops the request, which answers it.
        let _ = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(Event::Begin(Box::new(request)));
        match waiting.await {
            Answer::Done => Ok(()),
            Answer::Cancelled => Err(Refusal::Cancelled("Dismissed".into())),
        }
    }

    async fn cancel_authentication(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        cookie: String,
    ) -> Result<(), Refusal> {
        self.check_caller(connection, &header).await?;
        let _ = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(Event::Cancel(cookie));
        Ok(())
    }
}

impl Agent {
    /// Refuses a call from anything but polkit. The system bus lets any process send to the
    /// agent's name, and a card asking for a password must only ever be polkit's question.
    async fn check_caller(
        &self,
        connection: &zbus::Connection,
        header: &Header<'_>,
    ) -> Result<(), Refusal> {
        if !self.polkit_only {
            return Ok(());
        }
        let refused = || Refusal::Failed("only polkit asks this agent".into());
        let sender = header.sender().ok_or_else(refused)?;
        let bus = zbus::fdo::DBusProxy::new(connection).await?;
        let name = BusName::try_from(AUTHORITY).map_err(zbus::Error::from)?;
        match bus.get_name_owner(name).await {
            Ok(owner) if owner.as_str() == sender.as_str() => Ok(()),
            _ => Err(refused()),
        }
    }
}

/// The people among polkit's identities: users only, since polkit has already turned groups into
/// their members, each once, and only those with an account here.
fn users(identities: &[(String, HashMap<String, OwnedValue>)]) -> Vec<Identity> {
    let mut found: Vec<Identity> = Vec::new();
    for (kind, details) in identities {
        if kind != "unix-user" {
            continue;
        }
        let Some(uid) = details
            .get("uid")
            .and_then(|uid| u32::try_from(&**uid).ok())
        else {
            continue;
        };
        if found.iter().any(|known| known.uid == uid) {
            continue;
        }
        if let Some(identity) = identity(uid) {
            found.push(identity);
        }
        if found.len() == IDENTITIES {
            break;
        }
    }
    found
}

/// A user's login name and full name, from the password database.
fn identity(uid: u32) -> Option<Identity> {
    // SAFETY: getpwuid_r writes into `entry` and `buffer`, both valid for the sizes given, and
    // `found` points at `entry` only on success. The strings are copied out before the buffer
    // goes.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buffer = vec![0 as c_char; 16 * 1024];
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let failed = libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        );
        if failed != 0 || found.is_null() || entry.pw_name.is_null() {
            return None;
        }
        let name = CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned();
        let full_name = if entry.pw_gecos.is_null() {
            String::new()
        } else {
            let gecos = CStr::from_ptr(entry.pw_gecos).to_string_lossy();
            // The comment field's first part is the name; the rest are rooms and phone numbers.
            gecos.split(',').next().unwrap_or("").trim().to_string()
        };
        let full_name = if full_name == name {
            String::new()
        } else {
            full_name
        };
        Some(Identity {
            uid,
            name,
            full_name,
        })
    }
}

/// A message as one plain paragraph: a program can put its own words in one (`pkexec` names the
/// command), and line breaks there could make it look like more than polkit said.
fn plain(message: &str) -> String {
    message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(MESSAGE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The language polkit should word its messages in.
fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "C".to_string())
}

/// Registers the agent for this login session and serves it, sending what polkit asks to
/// `events`. Without polkit's helper there is nothing to check a password with, so it doesn't
/// register at all; without polkit, or with another agent already registered for the session, it
/// says so in the log and gives up.
pub fn serve(events: Sender<Event>) {
    if !helper::available() {
        tracing::warn!("polkit's helper isn't installed, so programs can't ask for a password");
        return;
    }
    std::thread::spawn(move || {
        let agent = Agent {
            events: Mutex::new(events),
            polkit_only: true,
        };
        match register(agent) {
            Ok(connection) => {
                tracing::info!("answering polkit's password requests");
                let _ = CONNECTION.set(connection);
            }
            Err(err) => tracing::warn!(
                "couldn't register for polkit's password requests (another agent may have): {err}"
            ),
        }
    });
}

fn register(agent: Agent) -> zbus::Result<zbus::blocking::Connection> {
    let connection = zbus::blocking::connection::Builder::system()?
        .serve_at(AGENT_PATH, agent)?
        .build()?;
    let id = match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => id,
        _ => {
            let session = crate::logind::session_path(&connection)?;
            crate::logind::session_id(&connection, &session)?
        }
    };
    let subject = (
        "unix-session",
        HashMap::from([("session-id", Value::from(id.as_str()))]),
    );
    connection.call_method(
        Some(AUTHORITY),
        AUTHORITY_PATH,
        Some("org.freedesktop.PolicyKit1.Authority"),
        "RegisterAuthenticationAgent",
        &(subject, locale(), AGENT_PATH),
    )?;
    Ok(connection)
}

/// The agent's side of the session: the card, the questions waiting behind it, and the check
/// under way.
#[derive(Default)]
pub struct Polkit {
    pub prompt: Option<Prompt>,
    waiting: VecDeque<Request>,
    /// Where the helper's words go; none for a nested run, whose cards check nothing.
    pub events: Option<Sender<Event>>,
    conversation: Option<Conversation>,
    /// The window that had the keyboard when the card came up, to have it back after.
    had_keyboard: Option<Window>,
    attempts: u64,
    /// The check under way has been sent an answer.
    answered: bool,
}

impl Polkit {
    pub fn is_up(&self) -> bool {
        self.prompt.is_some()
    }

    fn send(&mut self, answer: zeroize::Zeroizing<Vec<u8>>) {
        if let Some(conversation) = &self.conversation {
            conversation.answer(answer);
            self.answered = true;
        }
    }
}

impl Slipstream {
    pub fn polkit_event(&mut self, event: Event) {
        match event {
            Event::Begin(request) => {
                // Nothing is asked behind the lock; dropping the request answers it.
                if self.lock.is_some() {
                    tracing::info!("password request cancelled: the screen is locked");
                    return;
                }
                if self.polkit.waiting.len() >= WAITING {
                    tracing::info!("password request cancelled: too many are waiting");
                    return;
                }
                self.polkit.waiting.push_back(*request);
                self.show_next_polkit();
            }
            Event::Cancel(cookie) => {
                if self
                    .polkit
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| prompt.request.cookie == cookie)
                {
                    tracing::info!("polkit withdrew its password request");
                    self.close_polkit(Answer::Cancelled);
                } else {
                    self.polkit
                        .waiting
                        .retain(|request| request.cookie != cookie);
                }
            }
            Event::Helper { attempt, said } => self.polkit_said(attempt, said),
        }
    }

    /// Opens a card nobody is waiting on, for the debug step.
    pub fn open_stand_in_polkit(&mut self, message: String) {
        // SAFETY: getuid has no failure mode.
        let me = unsafe { libc::getuid() };
        let identities = identity(me).into_iter().collect();
        self.polkit
            .waiting
            .push_back(Request::stand_in(message, identities));
        self.show_next_polkit();
    }

    fn show_next_polkit(&mut self) {
        if self.polkit.prompt.is_some() || self.lock.is_some() {
            return;
        }
        let Some(request) = self.polkit.waiting.pop_front() else {
            return;
        };
        let now = self.wall();
        self.explorer.close();
        self.quick.close();
        self.centre.close();
        self.snip = None;
        // SAFETY: getuid has no failure mode.
        let me = unsafe { libc::getuid() };
        self.polkit.prompt = Some(Prompt::new(request, me, now, self.clock.reduced_motion));
        // First, so nothing after is logged at trace level.
        ASKING.store(true, Ordering::Relaxed);
        // A core dump taken while the password is in memory would hold it.
        // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) };
        self.polkit.had_keyboard = self.focused_window();
        self.clear_focus();
        // A cursor a client drew is its pixels, which could be drawn over the card. The pointer
        // leaves whatever client it was over, so none can set another while the card is up.
        let at = self.pointer_location();
        self.pointer_moved_to(at, smithay::backend::input::InputTime::now());
        self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        self.start_polkit_check();
        self.wake_ui();
    }

    /// Starts a check for whoever the card has chosen, stopping any before it. False when the
    /// helper couldn't be reached, which the card then says.
    fn start_polkit_check(&mut self) -> bool {
        self.polkit.conversation = None;
        let Some(prompt) = self.polkit.prompt.as_mut() else {
            return false;
        };
        self.polkit.attempts += 1;
        self.polkit.answered = false;
        prompt.attempt = self.polkit.attempts;
        let Some(events) = self.polkit.events.clone() else {
            return true;
        };
        if prompt.request.reply.is_none() {
            return true;
        }
        let attempt = prompt.attempt;
        let events = Mutex::new(events);
        let started = helper::start(
            &prompt.identity().name,
            &prompt.request.cookie,
            move |said| {
                let _ = events
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(Event::Helper { attempt, said });
            },
        );
        match started {
            Some(conversation) => {
                self.polkit.conversation = Some(conversation);
                true
            }
            None => {
                prompt.ended(Verdict::Failed);
                false
            }
        }
    }

    fn polkit_said(&mut self, attempt: u64, said: Said) {
        let Some(prompt) = self.polkit.prompt.as_mut() else {
            return;
        };
        if prompt.attempt != attempt {
            return;
        }
        match said {
            Said::Asks { echo, prompt: text } => {
                if let Some(answer) = prompt.asks(echo, text) {
                    self.polkit.send(answer);
                }
            }
            Said::Info(text) => prompt.told(text, false),
            Said::Error(text) => prompt.told(text, true),
            Said::Done(Verdict::Authenticated) => {
                tracing::info!("password accepted for polkit");
                self.close_polkit(Answer::Done);
            }
            Said::Done(verdict) => {
                tracing::info!(?verdict, "password not accepted for polkit");
                prompt.ended(verdict);
                self.polkit.conversation = None;
                // A wrong password gets another go straight away. A check that failed outright,
                // or was refused before it was sent anything, waits for Enter before trying
                // again, so a helper that can never succeed (a locked account) can't spin.
                if verdict == Verdict::Refused && std::mem::take(&mut self.polkit.answered) {
                    self.start_polkit_check();
                }
            }
        }
    }

    /// A key for the card, as the keyboard filter passed it on.
    pub fn polkit_key(&mut self, key: Key) {
        let now = self.wall();
        let Some(prompt) = self.polkit.prompt.as_mut() else {
            return;
        };
        let act = prompt.key(key, now);
        self.act_on_polkit(act);
    }

    pub fn polkit_click(&mut self, x: f64, y: f64) {
        let now = self.wall();
        let Some(prompt) = self.polkit.prompt.as_mut() else {
            return;
        };
        let act = prompt.click(x, y, now);
        self.act_on_polkit(act);
    }

    pub fn polkit_hover(&mut self, pos: Point<f64, Logical>) {
        let Some(screen) = self.focused_screen_geometry() else {
            return;
        };
        let pos = pos - screen.loc.to_f64();
        if let Some(prompt) = self.polkit.prompt.as_mut() {
            prompt.hover(pos.x, pos.y);
        }
    }

    fn act_on_polkit(&mut self, act: Act) {
        match act {
            Act::Nothing => {}
            Act::Cancel => {
                tracing::info!("password request cancelled");
                self.close_polkit(Answer::Cancelled);
            }
            Act::Chose => {
                self.start_polkit_check();
            }
            Act::Submit => {
                if self.polkit.conversation.is_none() && !self.start_polkit_check() {
                    return;
                }
                let Some(prompt) = self.polkit.prompt.as_mut() else {
                    return;
                };
                if let Some(answer) = prompt.answer() {
                    self.polkit.send(answer);
                }
            }
        }
    }

    /// Takes the card down, answers polkit, and gives the keyboard back; the next question
    /// waiting, if any, comes up.
    fn close_polkit(&mut self, answer: Answer) {
        self.polkit.conversation = None;
        let Some(prompt) = self.polkit.prompt.take() else {
            return;
        };
        // The password the card held is wiped as it drops.
        prompt.request.answer(answer);
        ASKING.store(false, Ordering::Relaxed);
        if self.lock.is_none() {
            // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
            unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1) };
        }
        let back = self
            .polkit
            .had_keyboard
            .take()
            .filter(|window| window.alive() && self.workspaces.find(window).is_some());
        match back {
            Some(window) => self.focus_window(&window),
            None => self.restore_focus(),
        }
        // The pointer enters whatever it is over again.
        let at = self.pointer_location();
        self.pointer_moved_to(at, smithay::backend::input::InputTime::now());
        self.show_next_polkit();
    }

    /// The screen is locking: every question is cancelled, the one on the card and those
    /// waiting. The lock has the keyboard now and gives it back itself.
    pub fn cancel_polkit_for_lock(&mut self) {
        if self.polkit.prompt.is_none() && self.polkit.waiting.is_empty() {
            return;
        }
        tracing::info!("password requests cancelled: the screen locked");
        self.polkit.conversation = None;
        self.polkit.prompt = None;
        self.polkit.waiting.clear();
        self.polkit.had_keyboard = None;
        ASKING.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::calloop::{EventLoop, channel};
    use std::time::Duration;

    #[test]
    fn a_question_dropped_unanswered_is_cancelled() {
        let (dropped, waiting) = reply();
        drop(dropped);
        assert_eq!(
            zbus::block_on(waiting),
            Answer::Cancelled,
            "never left waiting"
        );
        let (sent, waiting) = reply();
        sent.send(Answer::Done);
        assert_eq!(zbus::block_on(waiting), Answer::Done);
    }

    #[test]
    fn messages_stay_one_plain_paragraph() {
        assert_eq!(plain(" Run `x'\nas root\t "), "Run `x' as root");
        assert_eq!(plain(&"a".repeat(1000)).len(), MESSAGE_CHARS);
    }

    #[test]
    fn only_users_with_an_account_are_offered_each_once() {
        let user = |uid: u32| {
            (
                "unix-user".to_string(),
                HashMap::from([("uid".to_string(), OwnedValue::from(uid))]),
            )
        };
        let group = (
            "unix-group".to_string(),
            HashMap::from([("gid".to_string(), OwnedValue::from(10u32))]),
        );
        // Root has an account everywhere; this uid nowhere.
        let found = users(&[group, user(0), user(0), user(4_000_000_000)]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].uid, found[0].name.as_str()), (0, "root"));
    }

    /// The agent on a private bus, standing in for the system bus: a call from anything but
    /// polkit's name is refused, polkit's waits for the card's answer, and a card taken away is
    /// polkit's own `Cancelled` error. Run it under `dbus-run-session`, never on a real bus.
    #[test]
    #[ignore = "needs a private bus: dbus-run-session -- cargo test -- --ignored"]
    fn only_polkit_is_answered_and_it_waits_for_the_card() {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(
            !address.is_empty() && !address.contains("/run/user/"),
            "not on a private bus: {address}"
        );
        let (sender, events) = channel::channel();
        let agent = Agent {
            events: Mutex::new(sender),
            polkit_only: true,
        };
        let server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .serve_at(AGENT_PATH, agent)
            .unwrap()
            .build()
            .unwrap();
        let destination = server.unique_name().unwrap().to_string();
        let stranger = zbus::blocking::Connection::session().unwrap();
        let polkit = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(AUTHORITY)
            .unwrap()
            .build()
            .unwrap();
        let ask = |client: &zbus::blocking::Connection, cookie: &'static str| {
            let client = client.clone();
            let destination = destination.clone();
            std::thread::spawn(move || {
                let identities = vec![("unix-user", HashMap::from([("uid", Value::from(0u32))]))];
                client.call_method(
                    Some(destination.as_str()),
                    AGENT_PATH,
                    Some("org.freedesktop.PolicyKit1.AuthenticationAgent"),
                    "BeginAuthentication",
                    &(
                        "org.example.install",
                        "Installing needs a password",
                        "",
                        HashMap::<&str, &str>::new(),
                        cookie,
                        identities,
                    ),
                )
            })
        };
        let mut event_loop: EventLoop<'static, Vec<Request>> = EventLoop::try_new().unwrap();
        event_loop
            .handle()
            .insert_source(events, |event, _, got| {
                if let channel::Event::Msg(Event::Begin(request)) = event {
                    got.push(*request);
                }
            })
            .unwrap();
        let mut got = Vec::new();
        match ask(&stranger, "cookie-0").join().unwrap() {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(name.as_str(), "org.freedesktop.PolicyKit1.Error.Failed")
            }
            other => panic!("a stranger's question was taken: {other:?}"),
        }
        let first = ask(&polkit, "cookie-1");
        let second = ask(&polkit, "cookie-2");
        let started = std::time::Instant::now();
        while got.len() < 2 && started.elapsed() < Duration::from_secs(5) {
            event_loop
                .dispatch(Some(Duration::from_millis(20)), &mut got)
                .unwrap();
        }
        assert_eq!(
            got.len(),
            2,
            "both of polkit's arrive while neither is answered, and nothing else did"
        );
        got.sort_by(|a, b| a.cookie.cmp(&b.cookie));
        let second_request = got.pop().unwrap();
        let first_request = got.pop().unwrap();
        assert_eq!(first_request.message, "Installing needs a password");
        assert_eq!(first_request.identities[0].name, "root");
        first_request.answer(Answer::Done);
        assert!(first.join().unwrap().is_ok());
        drop(second_request);
        match second.join().unwrap() {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(name.as_str(), "org.freedesktop.PolicyKit1.Error.Cancelled")
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }
}

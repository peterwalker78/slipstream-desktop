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
    /// The action asked for and the process asking, which tell a repeat of a question that was
    /// just dismissed.
    source: Source,
    reply: Option<Reply>,
}

/// An action's ID and the asking process's pid, as polkit gives them.
type Source = (String, String);

impl Request {
    /// A question nobody is waiting on, for the card's debug step and tests: answering it checks
    /// nothing and tells nobody.
    pub fn stand_in(message: String, identities: Vec<Identity>) -> Self {
        Self {
            cookie: String::new(),
            message,
            identities,
            source: Source::default(),
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
    /// polkit has gone from the bus, and every question with it.
    Gone,
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
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), Refusal> {
        self.check_caller(connection, &header).await?;
        // SAFETY: getuid has no failure mode.
        let identities = users(&identities, unsafe { libc::getuid() });
        if identities.is_empty() {
            return Err(Refusal::Failed(
                "nobody polkit named has an account here".into(),
            ));
        }
        tracing::info!(action = %action_id, "a program asked for a password");
        let (reply, waiting) = reply();
        let asker = details
            .get("polkit.subject-pid")
            .or_else(|| details.get("polkit.caller-pid"))
            .cloned()
            .unwrap_or_default();
        let request = Request {
            cookie,
            message: plain(&message),
            identities,
            source: (action_id, asker),
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
/// their members, each once, and only those with an account here. Past `IDENTITIES` only `me`
/// is still taken, so this user is always offered, and is then the one chosen to begin with.
fn users(identities: &[(String, HashMap<String, OwnedValue>)], me: u32) -> Vec<Identity> {
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
        if found.len() >= IDENTITIES && uid != me {
            continue;
        }
        if let Some(identity) = identity(uid) {
            found.push(identity);
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
/// `events`, and registers again whenever polkit restarts, which forgets every agent. Without
/// polkit's helper there is nothing to check a password with, so it doesn't register at all;
/// with another agent already registered for the session, it says so in the log.
pub fn serve(events: Sender<Event>) {
    if !helper::available() {
        tracing::warn!("polkit's helper isn't installed, so programs can't ask for a password");
        return;
    }
    std::thread::spawn(move || {
        let agent = Agent {
            events: Mutex::new(events.clone()),
            polkit_only: true,
        };
        let connected = zbus::blocking::connection::Builder::system()
            .and_then(|builder| builder.serve_at(AGENT_PATH, agent))
            .and_then(|builder| builder.build())
            .and_then(|connection| session_id(&connection).map(|id| (connection, id)));
        let (connection, session) = match connected {
            Ok(connected) => connected,
            Err(err) => {
                tracing::warn!("couldn't answer polkit's password requests: {err}");
                return;
            }
        };
        match register(&connection, &session) {
            Ok(()) => tracing::info!("answering polkit's password requests"),
            Err(err) => tracing::warn!(
                "couldn't register for polkit's password requests (another agent may have): {err}"
            ),
        }
        let _ = CONNECTION.set(connection.clone());
        follow_polkit(&connection, &session, &events);
    });
}

/// This login session's ID, which the agent registers for.
fn session_id(connection: &zbus::blocking::Connection) -> zbus::Result<String> {
    match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => Ok(id),
        _ => {
            let session = crate::logind::session_path(connection)?;
            crate::logind::session_id(connection, &session)
        }
    }
}

fn register(connection: &zbus::blocking::Connection, session: &str) -> zbus::Result<()> {
    let subject = (
        "unix-session",
        HashMap::from([("session-id", Value::from(session))]),
    );
    connection.call_method(
        Some(AUTHORITY),
        AUTHORITY_PATH,
        Some("org.freedesktop.PolicyKit1.Authority"),
        "RegisterAuthenticationAgent",
        &(subject, locale(), AGENT_PATH),
    )?;
    Ok(())
}

/// Follows polkit on and off the bus for as long as the session runs: when it goes, its questions
/// go with it; when it comes back, the agent registers again. Only the bus itself can say a name
/// has moved; a signal claiming it from anyone else is ignored.
fn follow_polkit(connection: &zbus::blocking::Connection, session: &str, events: &Sender<Event>) {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|rule| rule.interface("org.freedesktop.DBus"))
        .and_then(|rule| rule.member("NameOwnerChanged"))
        .and_then(|rule| rule.arg(0, AUTHORITY))
        .map(|rule| rule.build());
    let messages = rule.and_then(|rule| {
        zbus::blocking::MessageIterator::for_match_rule(rule, connection, Some(8))
    });
    let messages = match messages {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!("couldn't follow polkit, so a restart of it goes unanswered: {err}");
            return;
        }
    };
    for message in messages.flatten() {
        if message.header().sender().map(|name| name.as_str()) != Some("org.freedesktop.DBus") {
            continue;
        }
        let Ok((name, _old, new)) = message.body().deserialize::<(String, String, String)>() else {
            continue;
        };
        if name != AUTHORITY {
            continue;
        }
        if new.is_empty() {
            let _ = events.send(Event::Gone);
            continue;
        }
        match register(connection, session) {
            Ok(()) => tracing::info!("polkit restarted; answering its password requests again"),
            Err(err) => tracing::warn!("polkit restarted, and couldn't register again: {err}"),
        }
    }
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
    /// Keys still being swallowed after a card went away by itself.
    settle: Option<Settle>,
    /// Questions dismissed lately, so the same one can't be put straight back up.
    repeats: Repeats,
}

/// How long keys go on being swallowed after a card goes away by itself, counted from the last
/// one typed.
const SETTLE: f64 = 1.0;

/// After a card is withdrawn, or closes without the Esc or Enter that ends it on purpose, keys
/// are swallowed until `SETTLE` passes without one. Otherwise the rest of a password, and its
/// Enter, would land in whatever window gets the keyboard back, which may be the very program
/// that withdrew the question.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settle {
    last: f64,
}

impl Settle {
    fn key(&mut self, now: f64) {
        self.last = self.last.max(now);
    }

    /// How long until the keyboard can go back, or `None` if it can now.
    fn left(&self, now: f64) -> Option<f64> {
        let left = self.last + SETTLE - now;
        (left > 0.0).then_some(left)
    }
}

/// How long a dismissed question is refused if the same process asks it again.
const REPEAT_QUIET: f64 = 10.0;

/// The questions dismissed lately, by action and asking process: a program that puts the same
/// question straight back up each time it is dismissed is refused for a while, rather than
/// keeping the card in front of everything.
#[derive(Debug, Default)]
struct Repeats {
    dismissed: Vec<(Source, f64)>,
}

impl Repeats {
    fn dismissed(&mut self, source: &Source, now: f64) {
        self.dismissed.retain(|(known, _)| known != source);
        if self.dismissed.len() == 32 {
            self.dismissed.remove(0);
        }
        self.dismissed.push((source.clone(), now));
    }

    fn refuses(&mut self, source: &Source, now: f64) -> bool {
        self.dismissed.retain(|(_, at)| now - at < REPEAT_QUIET);
        self.dismissed.iter().any(|(known, _)| known == source)
    }
}

impl Polkit {
    pub fn is_up(&self) -> bool {
        self.prompt.is_some()
    }

    /// Whether every key is the agent's: the card is up, or has just gone by itself.
    pub fn takes_keys(&self) -> bool {
        self.prompt.is_some() || self.settle.is_some()
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
                let now = self.wall();
                if self.polkit.repeats.refuses(&request.source, now) {
                    tracing::info!(
                        action = %request.source.0,
                        "password request cancelled: asked again straight after it was dismissed"
                    );
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
                    self.close_polkit(Answer::Cancelled, false);
                } else {
                    self.polkit
                        .waiting
                        .retain(|request| request.cookie != cookie);
                }
            }
            Event::Gone => {
                if self.polkit.prompt.is_some() || !self.polkit.waiting.is_empty() {
                    tracing::info!("polkit has gone, and its password requests with it");
                }
                self.polkit.waiting.clear();
                self.close_polkit(Answer::Cancelled, false);
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
        // While keys were settling after the last card the keyboard was already nobody's, and
        // the window that had it before that is still the one to give it back to.
        if self.polkit.settle.take().is_none() {
            self.polkit.had_keyboard = self.focused_window();
        }
        // A grab would keep a client's pointer, and its cursor, over the card: a menu's grab, or
        // a button held down in a window. Ended as the lock ends them.
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        if pointer.is_grabbed() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            pointer.unset_grab(self, serial, smithay::backend::input::InputTime::now());
        }
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        self.clear_focus();
        // A cursor a client drew is its pixels, which could be drawn over the card. The pointer
        // leaves whatever client it was over, and no client's cursor is taken while it is up.
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
                // Ended by Enter if a password went; a fingerprint can end it with typing still
                // under way.
                let by_enter = self.polkit.answered;
                self.close_polkit(Answer::Done, by_enter);
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
                self.dismissed_polkit(false);
            }
            Act::CancelAll => {
                tracing::info!(
                    waiting = self.polkit.waiting.len(),
                    "password request cancelled, and every one waiting"
                );
                self.dismissed_polkit(true);
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

    /// Esc, or Shift+Esc for every question waiting too. Each is remembered as dismissed, so the
    /// same program asking the same again at once is refused.
    fn dismissed_polkit(&mut self, all: bool) {
        let now = self.wall();
        if let Some(prompt) = &self.polkit.prompt {
            self.polkit.repeats.dismissed(&prompt.request.source, now);
        }
        if all {
            // Dropping each answers it.
            for request in std::mem::take(&mut self.polkit.waiting) {
                self.polkit.repeats.dismissed(&request.source, now);
            }
        }
        self.close_polkit(Answer::Cancelled, true);
    }

    /// Takes the card down and answers polkit; the next question waiting, if any, comes up. The
    /// keyboard goes back at once when the card was ended `by_user`, with Esc or Enter; otherwise
    /// keys are swallowed until typing stops (`Settle`).
    fn close_polkit(&mut self, answer: Answer, by_user: bool) {
        self.polkit.conversation = None;
        let Some(prompt) = self.polkit.prompt.take() else {
            return;
        };
        // The password the card held is wiped as it drops.
        prompt.request.answer(answer);
        if self.lock.is_none() {
            // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
            unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1) };
        }
        if by_user {
            self.give_keyboard_back();
        } else {
            tracing::info!("keys held back until typing stops");
            let now = self.wall();
            self.polkit.settle = Some(Settle { last: now });
            self.wait_for_settling(SETTLE);
        }
        self.show_next_polkit();
    }

    /// A key while keys are settling: swallowed, and the wait starts again.
    pub fn polkit_settling_key(&mut self) {
        let now = self.wall();
        if let Some(settle) = self.polkit.settle.as_mut() {
            settle.key(now);
        }
    }

    /// Looks again in `after` seconds whether typing has stopped, and gives the keyboard back
    /// once it has. A card coming up in the meantime, or the lock, ends the wait.
    fn wait_for_settling(&mut self, after: f64) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let inserted = self.loop_handle.insert_source(
            Timer::from_duration(std::time::Duration::from_secs_f64(after)),
            |_, _, state| {
                let now = state.wall();
                let Some(settle) = state.polkit.settle else {
                    return TimeoutAction::Drop;
                };
                match settle.left(now) {
                    Some(left) => {
                        TimeoutAction::ToDuration(std::time::Duration::from_secs_f64(left))
                    }
                    None => {
                        state.polkit.settle = None;
                        state.give_keyboard_back();
                        TimeoutAction::Drop
                    }
                }
            },
        );
        if inserted.is_err() {
            // Without a timer nothing would ever end the wait.
            self.polkit.settle = None;
            self.give_keyboard_back();
        }
    }

    /// The keyboard back to the window that had it before the card, if it's still here, and the
    /// pointer into whatever it is over.
    fn give_keyboard_back(&mut self) {
        ASKING.store(false, Ordering::Relaxed);
        let back = self
            .polkit
            .had_keyboard
            .take()
            .filter(|window| window.alive() && self.workspaces.find(window).is_some());
        match back {
            Some(window) => self.focus_window(&window),
            None => self.restore_focus(),
        }
        let at = self.pointer_location();
        self.pointer_moved_to(at, smithay::backend::input::InputTime::now());
    }

    /// The screen is locking: every question is cancelled, the one on the card and those
    /// waiting. The lock has the keyboard now and gives it back itself.
    pub fn cancel_polkit_for_lock(&mut self) {
        if !self.polkit.takes_keys() && self.polkit.waiting.is_empty() {
            return;
        }
        tracing::info!("password requests cancelled: the screen locked");
        self.polkit.conversation = None;
        self.polkit.prompt = None;
        self.polkit.settle = None;
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
        let found = users(&[group, user(0), user(0), user(4_000_000_000)], 0);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].uid, found[0].name.as_str()), (0, "root"));
    }

    /// However many people polkit offers, this user is among them, and so is chosen to begin
    /// with, rather than the first offered, who may be root.
    #[test]
    fn this_user_is_always_offered_however_many_are() {
        // SAFETY: getuid has no failure mode.
        let me = unsafe { libc::getuid() };
        let user = |uid: u32| {
            (
                "unix-user".to_string(),
                HashMap::from([("uid".to_string(), OwnedValue::from(uid))]),
            )
        };
        // Root first, then the system's own accounts, more than the limit on any distribution,
        // then this user.
        let mut offered = vec![user(0)];
        offered.extend((1..1000).filter(|uid| *uid != me).map(user));
        offered.push(user(me));
        let found = users(&offered, me);
        assert!(found.len() <= IDENTITIES + 1, "{} offered", found.len());
        assert!(found.iter().any(|identity| identity.uid == me));
        let prompt = Prompt::new(Request::stand_in(String::new(), found), me, 0.0, false);
        assert_eq!(prompt.identity().uid, me);
    }

    /// A card withdrawn under someone typing: keys go on being swallowed until a second passes
    /// with none, so the rest of a password can't reach the window that gets the keyboard back.
    #[test]
    fn keys_are_held_back_until_typing_stops_after_a_withdrawal() {
        let mut settle = Settle { last: 10.0 };
        assert_eq!(settle.left(10.0), Some(SETTLE), "never less than a second");
        settle.key(10.6);
        settle.key(11.3);
        assert!(settle.left(12.2).is_some(), "still typing a second ago");
        assert!(settle.left(12.3).is_none(), "a second with no typing");
        // A key that arrives out of order never shortens the wait.
        settle.key(11.0);
        assert!(settle.left(12.2).is_some());
    }

    #[test]
    fn a_question_dismissed_is_refused_if_asked_again_at_once() {
        let mut repeats = Repeats::default();
        let install = ("org.example.install".to_string(), "4242".to_string());
        let other_process = ("org.example.install".to_string(), "4343".to_string());
        assert!(!repeats.refuses(&install, 0.0));
        repeats.dismissed(&install, 0.0);
        assert!(repeats.refuses(&install, 1.0));
        assert!(
            !repeats.refuses(&other_process, 1.0),
            "only from the same process"
        );
        assert!(
            !repeats.refuses(&install, REPEAT_QUIET),
            "and only for a while"
        );
        for n in 0..100 {
            repeats.dismissed(&(n.to_string(), String::new()), 0.0);
        }
        assert!(repeats.dismissed.len() <= 32, "the list stays short");
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

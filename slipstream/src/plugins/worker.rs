//! One plugin's thread: it runs the plugin's program as its manifest says, reads the cards it
//! prints, and publishes what the panel should show. Nothing here runs on the event loop, and
//! nothing waits on it: stopping is a flag the thread is woken to look at, and the thread ends
//! the program's whole process group on its way out. A button pressed on the card is run by the
//! same thread, in its turn, so it never overlaps a run of the program and stops with it. Before
//! each start of the program, its approved copy is checked against the fingerprint it was
//! approved with.
//!
//! A plugin that needs `notifications` is handed each one as it arrives (`Worker::hear`): written
//! to a program that keeps running as it comes, and to one that runs every so often as what it
//! reads at its next run, and so, for a plugin that needs them, is what's playing, which windows
//! are open and what's copied. A notification or a sound on a card goes to the `Sink`, a few a
//! minute at most.

use std::{
    collections::VecDeque,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

use slipstream_config::{
    card::{RUN_LIMIT, unix_now},
    command::{self, Lines, Run, Said, Waker},
    plugins::{self, Card, Need, Plugin, sandbox, store},
};

/// A notification a plugin sent from its card, or a sound it played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The plugin's id, and its name, which its notification is signed with.
    pub id: String,
    pub name: String,
    /// Empty for a sound on its own.
    pub title: String,
    pub body: String,
    /// A name from the sound theme, for a plugin that needs `sound`.
    pub sound: Option<String>,
}

/// Where the notifications plugins send go, from their threads.
pub type Sink = Arc<dyn Fn(Sent) + Send + Sync>;

/// A plugin approved to run, as it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Approved {
    /// The plugin, its folder the approved copy.
    pub plugin: Plugin,
    /// The fingerprint the settings had it switched on with when it started: its own, or for a
    /// plugin that comes with Slipstream, the one before a like-for-like update. It runs while
    /// the settings still say that.
    pub granted: String,
    /// Its own folder for what it keeps (`SLIPSTREAM_PLUGIN_STATE`).
    pub state: PathBuf,
    /// The store its copy is in, which clears the copy away once nothing runs from it and it is
    /// no longer the approved one.
    pub store: Option<PathBuf>,
}

/// The copies workers run from, once for each worker.
static IN_USE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The copies workers run from, held so that none starts or finishes with one meanwhile.
pub fn copies_in_use() -> MutexGuard<'static, Vec<PathBuf>> {
    IN_USE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The least time between two cards published, however fast a program prints.
pub const UPDATE_GAP: Duration = Duration::from_millis(250);
/// How often a card's times left are counted down between runs.
const RECOUNT: Duration = Duration::from_secs(30);
/// A program that keeps running and exits sooner than this after starting is waited on longer
/// each time.
const STEADY: Duration = Duration::from_secs(10);
/// The wait before a program that kept running is started again after it exits, at first.
const RESTART: Duration = Duration::from_secs(1);
/// The longest wait before a program that keeps exiting is started again.
const LONGEST_BACKOFF: Duration = Duration::from_secs(300);

/// What a plugin shows: its last card, and why it has none or why that card is old.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Published {
    pub card: Option<Card>,
    pub problem: Option<String>,
}

/// The most presses waiting to be run; more are dropped.
const MOST_PRESSES: usize = 8;
/// The most notifications waiting to be handed to a program; more are dropped. A line holds at
/// most 560 characters of text, a little over 2 KB at four bytes each, so they always fit in a
/// pipe's 64 KB at once.
const MOST_HEARD: usize = 24;
/// The most of what it is told kept for a program that keeps running and is slow to read it.
const MOST_UNREAD: usize = 64 << 10;

/// Why a rest ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Woken {
    Stop,
    Pressed,
    Waited,
}

/// How a worker is told to stop, or to run a button's press.
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    /// Buttons pressed and not yet run, oldest first.
    presses: Mutex<VecDeque<String>>,
    /// Notifications arrived and not yet handed on, as the lines the program reads.
    heard: Mutex<VecDeque<String>>,
    /// Cuts short the wait for a line from a program that keeps running, while one runs.
    waker: Mutex<Option<Waker>>,
}

impl Control {
    fn pressed(&self) -> bool {
        !self.presses.lock().unwrap().is_empty()
    }
}

/// A running worker, and what it last published. Dropping it stops the worker.
pub struct Worker {
    control: Arc<Control>,
    published: Arc<Mutex<Published>>,
    thread: Thread,
}

impl Worker {
    /// Starts `approved`'s worker, the notifications it sends going to `sink`.
    pub fn start(approved: Approved, sink: Sink) -> Worker {
        let control = Arc::new(Control::default());
        let published = Arc::new(Mutex::new(Published::default()));
        let job = Job {
            control: control.clone(),
            published: published.clone(),
            plugin: approved.plugin,
            state: approved.state,
            store: approved.store,
            last: None,
            fresh: None,
            failures: Throttle::default(),
            sink,
            sent: Throttle::default(),
        };
        let thread = std::thread::Builder::new()
            .name("plugin".into())
            .spawn(move || job.go())
            .expect("a thread for a plugin");
        Worker {
            control,
            published,
            thread: thread.thread().clone(),
        }
    }

    pub fn published(&self) -> Published {
        self.published.lock().unwrap().clone()
    }

    /// Button `button` was pressed: the program runs with `press <button>` after it, in its
    /// turn, and then as usual again for one that runs every so often.
    pub fn press(&self, button: &str) {
        {
            let mut presses = self.control.presses.lock().unwrap();
            if presses.len() < MOST_PRESSES {
                presses.push_back(button.to_string());
            }
        }
        self.nudge();
    }

    /// A notification arrived, as the line its program reads: handed on in its turn.
    pub fn hear(&self, line: &str) {
        self.ear().tell(line, None);
    }

    /// How things are now, as a line its program reads, such as which windows are open: handed
    /// on in its turn, in place of any line about the same thing not yet handed on, since only
    /// the latest matters. `about` is the line's one key, as `windows`.
    pub fn hear_latest(&self, line: &str, about: &str) {
        self.ear().tell(line, Some(about));
    }

    /// A way to tell it things from another thread.
    pub fn ear(&self) -> Ear {
        Ear {
            control: self.control.clone(),
            thread: self.thread.clone(),
        }
    }

    fn stop(&self) {
        self.control.stop.store(true, Ordering::SeqCst);
        self.nudge();
    }

    fn nudge(&self) {
        self.ear().nudge();
    }
}

/// What tells a worker the lines its program reads, from any thread. Telling one that has
/// stopped does nothing.
#[derive(Clone)]
pub struct Ear {
    control: Arc<Control>,
    thread: Thread,
}

impl Ear {
    /// As `Worker::hear_latest`.
    pub fn hear_latest(&self, line: &str, about: &str) {
        self.tell(line, Some(about));
    }

    fn tell(&self, line: &str, about: Option<&str>) {
        {
            let mut heard = self.control.heard.lock().unwrap();
            if let Some(about) = about {
                let starts = format!("{{\"{about}\":");
                heard.retain(|told| !told.starts_with(&starts));
            }
            if heard.len() >= MOST_HEARD {
                heard.pop_front();
            }
            heard.push_back(line.to_string());
        }
        self.nudge();
    }

    /// Wakes the thread, whether it rests or waits for a line.
    fn nudge(&self) {
        self.thread.unpark();
        if let Some(waker) = &*self.control.waker.lock().unwrap() {
            waker.wake();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker thread's own state.
struct Job {
    plugin: Plugin,
    state: PathBuf,
    store: Option<PathBuf>,
    control: Arc<Control>,
    published: Arc<Mutex<Published>>,
    /// The last line that read as a card, read again to count its times left down, and the card
    /// it read as, until that has been published.
    last: Option<String>,
    fresh: Option<Card>,
    failures: Throttle,
    sink: Sink,
    /// Lets a few of the notifications its cards send through a minute.
    sent: Throttle,
}

impl Job {
    fn go(mut self) {
        copies_in_use().push(self.plugin.dir.clone());
        match self.plugin.manifest.every() {
            Some(every) => self.every(every),
            None => self.continuous(),
        }
    }

    /// The program, with `args`, if its copy still holds what was approved; says why not
    /// otherwise.
    fn program(&self, args: &[&str]) -> Result<Command, String> {
        store::verify(&self.plugin.dir, &self.plugin.fingerprint)?;
        program(&self.plugin, &self.state, args)
    }

    /// What a run reads on its standard input: what it was told since the last, for a plugin
    /// that needs to be told anything, and nothing to read for any other.
    fn input(&self) -> Option<Vec<u8>> {
        self.plugin.manifest.hears().then(|| {
            let mut heard = self.control.heard.lock().unwrap();
            heard.drain(..).collect::<String>().into_bytes()
        })
    }

    fn stopped(&self) -> bool {
        self.control.stop.load(Ordering::SeqCst)
    }

    /// Runs the presses waiting, oldest first. False when told to stop, which ends the one
    /// running.
    fn run_presses(&mut self) -> bool {
        loop {
            let Some(button) = self.control.presses.lock().unwrap().pop_front() else {
                return true;
            };
            let program = match self.program(&["press", &button]) {
                Ok(program) => program,
                Err(why) => {
                    self.failed(&why);
                    continue;
                }
            };
            let input = self.input();
            match command::run_tidily(program, RUN_LIMIT, &|| self.stopped(), input.as_deref()) {
                None => return false,
                Some(Run::Printed(_)) => {}
                Some(ran) => self.failed(&format!("A press: {}", why(&ran))),
            }
        }
    }

    /// Publishes the last card read, `stale` when its program has since failed, with `problem`.
    fn publish(&mut self, stale: bool, problem: Option<String>) {
        let fresh = self.fresh.take().filter(|_| !stale);
        let card = fresh.or_else(|| {
            let line = self.last.as_ref()?;
            let mut card = Card::parse(line, stale, unix_now()).ok()?;
            unasked(&self.plugin.manifest, &mut card);
            Some(card)
        });
        *self.published.lock().unwrap() = Published { card, problem };
    }

    /// Takes `line` as the new card if it reads as one, sending the notification on it; says why
    /// not otherwise.
    fn read(&mut self, line: &str) -> Result<(), String> {
        let mut card = Card::parse(line, false, unix_now())
            .map_err(|err| format!("Its card can't be read: {err}"))?;
        self.last = Some(line.to_string());
        if let Some(why) = unasked(&self.plugin.manifest, &mut card) {
            self.failed(why);
        }
        let sound = card.sound.take();
        let notify = card.notify.take();
        if notify.is_some() && !self.plugin.manifest.needs(Need::Notify) {
            self.failed("Its card sends a notification, but it doesn't say it needs notify");
        }
        match notify.filter(|_| self.plugin.manifest.needs(Need::Notify)) {
            Some(notify) => self.send(notify.title, notify.body, sound),
            None if sound.is_some() => self.send(String::new(), String::new(), sound),
            None => {}
        }
        self.fresh = Some(card);
        Ok(())
    }

    /// Sends a notification titled `title`, or with no title only `sound`, a few a minute.
    fn send(&mut self, title: String, body: String, sound: Option<String>) {
        if self.sent.admit(Instant::now()).is_some() {
            (self.sink)(Sent {
                id: self.plugin.id.clone(),
                name: self.plugin.manifest.name.clone(),
                title,
                body,
                sound,
            });
        } else {
            let id = &self.plugin.id;
            tracing::debug!("plugin {id}: a notification more than a few a minute, left out");
        }
    }

    /// Waits up to `wait`, or until told to stop or to run again, and says which.
    fn rest(&self, wait: Duration) -> Woken {
        let until = Instant::now() + wait;
        loop {
            if self.stopped() {
                return Woken::Stop;
            }
            if self.control.pressed() {
                return Woken::Pressed;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Woken::Waited;
            }
            std::thread::park_timeout(left);
        }
    }

    /// Logs why the plugin has nothing new to show, a few times a minute at most.
    fn failed(&mut self, why: &str) {
        if let Some(dropped) = self.failures.admit(Instant::now()) {
            let id = &self.plugin.id;
            if dropped > 0 {
                tracing::warn!("plugin {id}: {why} ({dropped} more not logged)");
            } else {
                tracing::warn!("plugin {id}: {why}");
            }
        }
    }

    /// A program that runs every `every`, and again after one of its buttons is pressed. Between
    /// runs its card is read again now and then, so the times left on it count down.
    fn every(&mut self, every: Duration) {
        loop {
            let problem = match self.program(&[]) {
                Ok(program) => {
                    let input = self.input();
                    let stop = || self.stopped();
                    let Some(ran) =
                        command::run_tidily(program, RUN_LIMIT, &stop, input.as_deref())
                    else {
                        return;
                    };
                    match ran {
                        Run::Printed(printed) => self.read(&printed).err(),
                        failed => Some(why(&failed)),
                    }
                }
                Err(why) => Some(why),
            };
            if let Some(problem) = &problem {
                self.failed(problem);
            }
            let stale = problem.is_some();
            self.publish(stale, problem.clone());
            let next = Instant::now() + every;
            loop {
                let left = next.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match self.rest(left.min(RECOUNT)) {
                    Woken::Stop => return,
                    Woken::Pressed if !self.run_presses() => return,
                    // Run again, to show what the press changed.
                    Woken::Pressed => break,
                    // Only time to count the card's times left down.
                    Woken::Waited => self.publish(stale, problem.clone()),
                }
            }
        }
    }

    /// A program that keeps running and prints a card a line, started again whenever it exits,
    /// after a wait that grows while it keeps exiting soon after starting.
    fn continuous(&mut self) {
        let mut backoff = RESTART;
        loop {
            let started = Instant::now();
            // Stopping ends the program too, which isn't it exiting by itself.
            if !self.follow() || self.stopped() {
                return;
            }
            backoff = next_backoff(backoff, started.elapsed());
            let problem = format!(
                "Its program stopped; starting it again in {:.0} s",
                backoff.as_secs_f64().ceil()
            );
            self.failed(&problem);
            self.publish(true, Some(problem));
            match self.rest(backoff) {
                Woken::Stop => return,
                Woken::Pressed if !self.run_presses() => return,
                Woken::Pressed | Woken::Waited => {}
            }
        }
    }

    /// Follows one run of a program that keeps running until it exits. False when told to stop,
    /// which ends the program.
    fn follow(&mut self) -> bool {
        let errors = Arc::new(Mutex::new(Throttle::default()));
        let id = self.plugin.id.clone();
        let program = match self.program(&[]) {
            Ok(program) => program,
            Err(why) => {
                self.failed(&why);
                self.publish(true, Some(why));
                return !self.stopped();
            }
        };
        let listens = self.plugin.manifest.hears();
        let lines = match Lines::start(program, listens, move |line| {
            if let Some(dropped) = errors.lock().unwrap().admit(Instant::now()) {
                let more = if dropped > 0 {
                    format!(" ({dropped} more lines not logged)")
                } else {
                    String::new()
                };
                tracing::info!("plugin {id} says: {line}{more}");
            }
        }) {
            Ok(lines) => lines,
            Err(err) => {
                let problem = format!("Couldn't start: {err}");
                self.failed(&problem);
                self.publish(true, Some(problem));
                return !self.stopped();
            }
        };
        // So that being stopped, or a press, doesn't wait for the program's next line.
        *self.control.waker.lock().unwrap() = Some(lines.waker());
        let followed = self.follow_lines(&lines);
        *self.control.waker.lock().unwrap() = None;
        followed
    }

    /// Reads the cards a program that keeps running prints, until it exits (true) or the worker
    /// is told to stop (false), running presses as they come.
    fn follow_lines(&mut self, lines: &Lines) -> bool {
        let mut waiting = false;
        let mut last = Instant::now() - UPDATE_GAP;
        // What it has been told and hasn't read yet.
        let mut unread: Vec<u8> = Vec::new();
        loop {
            if self.stopped() {
                return false;
            }
            if self.control.pressed() && !self.run_presses() {
                return false;
            }
            if let Some(heard) = self.input() {
                if unread.len() + heard.len() <= MOST_UNREAD {
                    unread.extend(heard);
                }
                let taken = lines.say(&unread);
                unread.drain(..taken);
            }
            let wait = if waiting {
                (last + UPDATE_GAP).saturating_duration_since(Instant::now())
            } else if !unread.is_empty() {
                // Tried again shortly, until it has read what it was told.
                UPDATE_GAP
            } else {
                RECOUNT
            };
            let ended = match lines.next(wait.max(Duration::from_millis(1))) {
                Said::Line(line) if !line.trim().is_empty() => {
                    match self.read(&line) {
                        Ok(()) => waiting = true,
                        Err(why) => self.failed(&why),
                    }
                    false
                }
                Said::Line(_) | Said::Woken => false,
                Said::Nothing => {
                    // Quiet a while: the times left on the card still count down.
                    if !waiting && last.elapsed() >= RECOUNT {
                        waiting = true;
                    }
                    false
                }
                Said::Ended => true,
            };
            // Only the newest card counts, and one is shown a quarter of a second at most.
            if waiting && (ended || last.elapsed() >= UPDATE_GAP) {
                self.publish(false, None);
                waiting = false;
                last = Instant::now();
            }
            if ended {
                return true;
            }
        }
    }
}

impl Drop for Job {
    /// The copy goes once nothing runs from it and it is no longer the approved one.
    fn drop(&mut self) {
        let mut in_use = copies_in_use();
        if let Some(at) = in_use.iter().position(|dir| *dir == self.plugin.dir) {
            in_use.remove(at);
        }
        let Some(store) = &self.store else {
            return;
        };
        let approved = store::current(store).is_some_and(|name| self.plugin.dir.ends_with(name));
        if !approved && !in_use.contains(&self.plugin.dir) {
            let _ = store::remove_tree(&self.plugin.dir);
        }
    }
}

/// Takes out what `card` asks Slipstream for that its plugin doesn't say it needs: a sound, or a
/// button that opens a web address, copies or controls what's playing. Says why, if it took any.
fn unasked(manifest: &plugins::Manifest, card: &mut Card) -> Option<&'static str> {
    let needs = |need| manifest.needs(need);
    let mut why = None;
    if card.sound.is_some() && !needs(Need::Sound) {
        card.sound = None;
        why = Some("Its card plays a sound, but it doesn't say it needs sound");
    }
    card.buttons.retain(|button| {
        let unasked = if button.open.is_some() && !needs(Need::Open) {
            Some("A button opens a web address, but it doesn't say it needs open")
        } else if button.copy.is_some() && !needs(Need::Clipboard) {
            Some("A button copies, but it doesn't say it needs clipboard")
        } else if plugins::MEDIA_BUTTONS.contains(&button.id.as_str()) && !needs(Need::Media) {
            Some("A button controls what's playing, but it doesn't say it needs media")
        } else {
            None
        };
        why = why.or(unasked);
        unasked.is_none()
    });
    why
}

/// The plugin's program, with `args` after its own words: run as its manifest writes it, in its
/// folder, with `state` as its own folder. It runs in its sandbox, from a cleared environment
/// (`plugins::ENVIRONMENT`), unless it needs `everything`, when it runs with the session's own.
pub fn program(plugin: &Plugin, state: &Path, args: &[&str]) -> Result<Command, String> {
    let manifest = &plugin.manifest;
    let exec = &manifest.exec;
    let program = if exec[0].contains('/') {
        plugin.dir.join(&exec[0]).into_os_string()
    } else {
        exec[0].clone().into()
    };
    if let Err(err) = std::fs::create_dir_all(state) {
        tracing::warn!("plugin {}: no folder for its state: {err}", plugin.id);
    }
    let mut reads = plugins::read_folders(manifest)?;
    let mut writes = plugins::write_folders(manifest)?;
    let mut command = if manifest.unconfined() {
        Command::new(program)
    } else {
        let bwrap = sandbox::bwrap()?;
        let session = sandbox::Session::now();
        let mut command = Command::new(bwrap);
        // Local sockets are for the display, and for looking names up online; a plugin with
        // neither has no use for them.
        let seccomp = seccomp_file(manifest.needs(Need::Windows) || manifest.needs(Need::Network))?;
        let seccomp_fd = seccomp.as_ref().map(|file| file.as_raw_fd());
        // The folders it reads and changes, open where they were checked to be, for bubblewrap
        // to bind from.
        let mut folders = sandbox::open(&mut reads)?;
        folders.extend(sandbox::open(&mut writes)?);
        // SAFETY: system calls between fork and exec, which allocate nothing. The filter's file
        // and the folders' are moved in, so they stay open for as long as the command can be
        // started.
        unsafe {
            command.pre_exec(move || {
                // A session keyring of its own, empty: the one it would share can hold the keys
                // that unlock your files or shares.
                let joined = libc::syscall(
                    libc::SYS_keyctl,
                    libc::KEYCTL_JOIN_SESSION_KEYRING,
                    std::ptr::null::<libc::c_char>(),
                );
                let error = std::io::Error::last_os_error();
                // A kernel without keyrings has none to keep out.
                if joined < 0 && error.raw_os_error() != Some(libc::ENOSYS) {
                    return Err(error);
                }
                // The filter's file and the folders', left open across the exec for bubblewrap.
                for fd in seccomp.iter().chain(&folders).map(|file| file.as_raw_fd()) {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        command
            .args(sandbox::args(
                manifest,
                &plugin.dir,
                state,
                &reads,
                &writes,
                &session,
                seccomp_fd,
            ))
            .arg(program)
            .env_clear();
        // A proxy's address can carry its password, so only a plugin that goes online sees it.
        let online = manifest.needs(Need::Network);
        for name in plugins::ENVIRONMENT {
            if !online && name.to_lowercase().ends_with("_proxy") {
                continue;
            }
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(sandbox::environment(manifest, &session));
        command
    };
    command
        .args(&exec[1..])
        .args(args)
        .current_dir(&plugin.dir)
        .env("SLIPSTREAM_PLUGIN_STATE", state);
    for (n, folder) in reads.iter().enumerate() {
        command.env(format!("SLIPSTREAM_PLUGIN_READ_{n}"), &folder.named);
    }
    for (n, folder) in writes.iter().enumerate() {
        command.env(format!("SLIPSTREAM_PLUGIN_WRITE_{n}"), &folder.named);
    }
    Ok(command)
}

/// The filter of system calls a sandboxed plugin is refused (`seccomp`), in a file of its own in
/// memory, open for the next program started and closed again with the command: bubblewrap reads
/// it from there. None where there is no filter for this kind of computer.
fn seccomp_file(local: bool) -> Result<Option<std::fs::File>, String> {
    use std::io::{Seek, Write};
    use std::os::fd::FromRawFd;
    let Some(program) = plugins::seccomp::program(local) else {
        return Ok(None);
    };
    let why = |err: std::io::Error| format!("Its sandbox's filter couldn't be made: {err}");
    // SAFETY: a name that is a valid C string; the descriptor returned is owned from here.
    let fd = unsafe { libc::memfd_create(c"slipstream-plugin-filter".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(why(std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just opened above and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(&program).map_err(why)?;
    file.rewind().map_err(why)?;
    Ok(Some(file))
}

/// The wait before starting a program that keeps running again: `RESTART`, doubled each time it
/// ran for less than `STEADY`, up to `LONGEST_BACKOFF`.
fn next_backoff(backoff: Duration, ran: Duration) -> Duration {
    if ran >= STEADY {
        RESTART
    } else {
        (backoff * 2).min(LONGEST_BACKOFF)
    }
}

/// Why a run has nothing new to show.
pub(super) fn why(run: &Run) -> String {
    match run {
        Run::Printed(_) => "It ran".into(),
        Run::Failed { error, .. } if error.starts_with("bwrap: ") => {
            format!(
                "Its sandbox couldn't be set up: {}",
                &error["bwrap: ".len()..]
            )
        }
        Run::Failed { code, error } => {
            let stopped = match code {
                Some(code) => format!("Stopped with error {code}"),
                None => "Stopped by a signal".into(),
            };
            if error.is_empty() {
                stopped
            } else {
                format!("{stopped}: {error}")
            }
        }
        Run::TimedOut => format!(
            "Still running after {} s, so it was stopped",
            RUN_LIMIT.as_secs()
        ),
        Run::CouldntStart(err) => format!("Couldn't start: {err}"),
    }
}

/// Lets a few log lines through a minute, and counts the rest.
#[derive(Debug, Default)]
struct Throttle {
    since: Option<Instant>,
    let_through: u32,
    dropped: u32,
}

const LINES_A_MINUTE: u32 = 5;

impl Throttle {
    /// Whether a line at `now` is logged: with how many were held back since the last one that
    /// was, or `None` to drop it.
    fn admit(&mut self, now: Instant) -> Option<u32> {
        let fresh = self
            .since
            .is_none_or(|since| now.saturating_duration_since(since) >= Duration::from_secs(60));
        if fresh {
            self.since = Some(now);
            self.let_through = 0;
            let dropped = std::mem::take(&mut self.dropped);
            self.let_through += 1;
            return Some(dropped);
        }
        if self.let_through < LINES_A_MINUTE {
            self.let_through += 1;
            Some(0)
        } else {
            self.dropped += 1;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_keeps_exiting_waits_longer_each_time() {
        let quick = Duration::from_secs(1);
        assert_eq!(next_backoff(RESTART, quick), RESTART * 2);
        assert_eq!(next_backoff(LONGEST_BACKOFF, quick), LONGEST_BACKOFF);
        assert_eq!(next_backoff(LONGEST_BACKOFF, STEADY), RESTART);
    }

    #[test]
    fn the_log_lets_a_few_lines_a_minute_through() {
        let mut throttle = Throttle::default();
        let now = Instant::now();
        for _ in 0..LINES_A_MINUTE {
            assert!(throttle.admit(now).is_some());
        }
        assert_eq!(throttle.admit(now), None);
        assert_eq!(throttle.admit(now), None);
        assert_eq!(throttle.admit(now + Duration::from_secs(61)), Some(2));
    }
}

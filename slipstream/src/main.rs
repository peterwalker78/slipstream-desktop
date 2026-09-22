//! The Slipstream Wayland compositor.
//!
//! Started from Smithay's `smallvil` example (MIT licence, see `LICENSE-smallvil-MIT.txt`).
//!
//! Inside another desktop it runs nested, as a window. From the login screen (`--session`) or a
//! text console (`--tty`) it drives the screens, keyboard and mouse itself.

#![allow(irrefutable_let_patterns)]

mod alert;
mod anim;
mod appearance;
mod apps;
mod auth;
mod awake;
mod bar;
mod battery;
mod bullet;
mod calc;
mod calendar;
mod capture;
mod card;
mod centre;
mod clipboard;
mod concentration;
mod connect;
mod cursor;
mod debug;
mod emoji;
mod exit;
mod explorer;
mod fallback;
mod files;
mod floating;
mod focus;
mod gamma;
mod ghost;
mod glass;
mod glmatrix;
mod grabs;
mod gravity;
mod handlers;
mod history;
mod icons;
mod idle;
mod ime;
mod inhibit;

mod input;
mod keys;
mod known;
mod launch;
mod layers;
mod layout;
mod live;
mod lock;
mod logind;
mod media;
mod meter;
mod motion;
mod nightlight;
mod notices;
mod notify;
mod offer;
mod osd;
mod overview;
mod paint;
mod panel;
mod power;
mod quick;
mod rain;
mod render;
mod restore;
mod saver;
mod screen;
mod screenshot;
mod session;
mod settings;
mod share;
mod sheet;
mod snip;
mod sound;
mod state;
mod status;
mod switcher;
mod takeback;
mod text;
mod tilt;
mod toast;
mod udev;
mod usage;
mod watch;
mod winit;
mod workspace;
mod xwayland;

use std::{
    io::IsTerminal,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::reexports::{
    calloop::{
        EventLoop,
        signals::{Signal, Signals},
        timer::{TimeoutAction, Timer},
    },
    wayland_server::Display,
};
pub use state::Slipstream;

/// Our own session target. `graphical-session.target` refuses a manual start, so it can only come
/// up as a dependency of this one (`session/slipstream-session.target`).
const SESSION_TARGET: &str = "slipstream-session.target";
/// Torn down with `--job-mode=replace-irreversibly` on the way out.
const SHUTDOWN_TARGET: &str = "slipstream-shutdown.target";

/// What the user manager and D-Bus need to know about the session. `DISPLAY` isn't here: XWayland
/// picks its number later and imports it itself.
const SESSION_VARS: [&str; 8] = [
    "WAYLAND_DISPLAY",
    live::LIVE_SOCKET,
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
    "XCURSOR_THEME",
    "XCURSOR_SIZE",
];

/// Whether this is the login session, for the teardown, which the watchdog reaches from another
/// thread with no `Slipstream` to hand.
static SESSION: AtomicBool = AtomicBool::new(false);
static SESSION_ENDED: AtomicBool = AtomicBool::new(false);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // For bug reports, and for the install scripts to check a binary runs on this system at all.
    if args.first().map(String::as_str) == Some("--version") {
        println!("slipstream {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // The lock screen's password check (`auth.rs`): nothing else runs, and nothing is logged.
    if args.first().map(String::as_str) == Some(auth::FLAG) {
        std::process::exit(auth::authenticate());
    }
    // The screen-sharing portal's chooser: no compositor, just a question passed to the running
    // one and its answer printed.
    if args.iter().any(|arg| arg == "--choose-share") {
        std::process::exit(share::choose());
    }
    // A living-wallpaper variation drawn off screen for the Settings app's previews: no display,
    // no GPU, and nothing else started.
    if args.first().map(String::as_str) == Some("--wallpaper-preview") {
        let done = saver::preview::Request::parse(&args[1..])
            .and_then(|request| saver::preview::run(&request));
        if let Err(err) = done {
            eprintln!(
                "slipstream --wallpaper-preview: {err}\nusage: slipstream --wallpaper-preview ID \
                 [--size WxH] [--frames N] [--from SECS] [--every SECS] [--out DIR]"
            );
            std::process::exit(2);
        }
        std::process::exit(0);
    }
    let session = args.iter().any(|arg| arg == "--session");
    let hardware = session
        || args.iter().any(|arg| arg == "--tty")
        || (std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none());

    init_logging();
    install_panic_hook();

    // Started from a terminal in a Slipstream session, a nested window would open on that session
    // itself. Before the socket or the window exists, so nothing reaches it.
    if !hardware
        && live::nests_on_live(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os(live::LIVE_SOCKET).as_deref(),
            std::env::var_os("SLIPSTREAM_NEST_ON_LIVE").as_deref(),
        )
    {
        tracing::error!(
            "refusing to open a nested window on the Slipstream session it is running in; use a \
             headless Weston"
        );
        std::process::exit(2);
    }

    // Held until the process ends. Only the compositor holding it offers, reopens or writes the
    // recorded layout, or writes the settings file.
    let state_dir = session::state_dir();
    let owner = match live::OwnerLock::take(&state_dir) {
        Ok(Some(lock)) => {
            tracing::info!(folder = %state_dir.display(), "this Slipstream owns the state folder");
            Some(Ok(lock))
        }
        Ok(None) => {
            tracing::warn!(
                "another Slipstream owns {}: not recording or reopening the layout, and not \
                 writing settings",
                state_dir.display()
            );
            None
        }
        // A folder that can't be locked (or made) isn't another compositor's: carry on as the
        // owner rather than lose the session's own settings and record.
        Err(err) => {
            tracing::warn!(folder = %state_dir.display(), "couldn't lock the state folder: {err}");
            Some(Err(err))
        }
    };

    let mut event_loop: EventLoop<'static, Slipstream> = EventLoop::try_new()?;

    // Before any thread is spawned: `Signals` blocks the signal for the calling thread, and
    // threads inherit that mask, so doing it here is what stops SIGTERM being delivered to (and
    // killing us through) some other thread instead.
    if session {
        watch_for_sigterm(&event_loop);
    }

    let display: Display<Slipstream> = Display::new()?;

    let mut state = Slipstream::new(&mut event_loop, display);
    state.owns_state = owner.is_some();
    // The explorer follows apps being installed and removed, as KDE's menus do.
    watch::watch_apps(&event_loop.handle());
    watch::watch_recent_files(&event_loop.handle());
    // Settings apply as soon as they're saved.
    watch::watch_settings(&event_loop.handle());
    // A fallback face loaded on its own thread wakes the loop, which takes it in between paints.
    {
        use smithay::reexports::calloop::ping;
        let (waker, landed) = ping::make_ping()?;
        event_loop
            .handle()
            .insert_source(landed, |_, _, state| state.fallback_fonts_landed())
            .map_err(|err| err.error)?;
        fallback::set_waker(waker);
    }
    // What a player is doing after a media key.
    {
        use smithay::reexports::calloop::channel;
        let (sender, replies) = channel::channel();
        event_loop
            .handle()
            .insert_source(replies, |event, _, state| {
                if let channel::Event::Msg(playing) = event {
                    state.media_played(playing);
                }
            })
            .map_err(|err| err.error)?;
        state.media_replies = Some(sender);
    }
    // The screen-sharing portal's chooser asks here what to share.
    share::listen(&event_loop.handle(), &state.socket_name.to_string_lossy());
    // The session's notification server. Nested, the desktop Slipstream runs inside keeps the
    // name, unless SLIPSTREAM_NOTIFICATIONS=1.
    if session || std::env::var("SLIPSTREAM_NOTIFICATIONS").as_deref() == Ok("1") {
        use smithay::reexports::calloop::channel;
        let (sender, notifications) = channel::channel();
        event_loop
            .handle()
            .insert_source(notifications, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.notification_event(event);
                }
            })
            .map_err(|err| err.error)?;
        notify::serve(sender);
    }
    // Dark or light, for every app that asks the desktop. Nested, the desktop Slipstream runs
    // inside already answers that for its own apps.
    if session || std::env::var("SLIPSTREAM_APPEARANCE").as_deref() == Ok("1") {
        appearance::serve(state.settings.appearance.colour_scheme);
    }

    if hardware {
        // Only the portal captures a real screen, whatever the environment asks.
        if std::env::var_os("SLIPSTREAM_CAPTURE_ANYONE").is_some() {
            tracing::warn!("SLIPSTREAM_CAPTURE_ANYONE is ignored outside a nested run");
        }
        // Debug steps press keys and run commands; a login session takes none, whatever its
        // environment says.
        if std::env::var_os("SLIPSTREAM_DEBUG").is_some() {
            tracing::warn!("SLIPSTREAM_DEBUG is ignored outside a nested run");
        }
        // Before anything starts, so every child connects here and none reaches for X11.
        // The socket's name also goes in SLIPSTREAM_LIVE_SOCKET, so a Slipstream started from
        // inside this session knows not to nest on it.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
            std::env::set_var(live::LIVE_SOCKET, &state.socket_name);
            std::env::remove_var("DISPLAY");
        }
        udev::run(&mut event_loop, &mut state)?;
        tracing::info!(
            "Slipstream is running on WAYLAND_DISPLAY={}. Super+Return opens a terminal, Super+E \
             files, Super+R a command; Ctrl+Alt+Del is the way out",
            state.socket_name.to_string_lossy()
        );
    } else {
        // Nested, the volume, the radios and logind belong to the desktop around this window, so
        // commands that would change them are only logged.
        let held = launch::holds_machine_commands(
            true,
            std::env::var_os("SLIPSTREAM_REAL_COMMANDS").as_deref(),
        );
        launch::hold_machine_commands(held);
        if held {
            tracing::info!(
                "nested: volume, brightness, radio, power-mode and logind commands are logged, \
                 not run"
            );
        }
        // Tools such as grim speak the capture protocols directly; a nested check may let them.
        let open = capture::opens_to_anyone(
            true,
            std::env::var_os("SLIPSTREAM_CAPTURE_ANYONE").as_deref(),
        );
        capture::open_to_anyone(open);
        if open {
            tracing::info!("capture open to every unsandboxed client (nested test opt-in)");
        }
        // Debug steps, and the `lock` step among them, which a login session never has.
        state.debug = debug::Script::from_env();
        lock::allow_steps(lock::steps_allowed_for(true));
        // Before the window, whose output connecting already asks things of the backend (the lid
        // switch, for one).
        state.nested = true;
        // Open a Wayland/X11 window for our nested compositor
        crate::winit::init_winit(&mut event_loop, &mut state)?;

        // Set WAYLAND_DISPLAY to our socket name, so child processes connect to Slipstream rather
        // than the host compositor
        unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };
        tracing::info!(
            "listening on WAYLAND_DISPLAY={}. Nested, Alt is the Mod key: Alt+arrows move focus, \
             Alt+1–5 and Alt+Ctrl+←/→ switch workspace, Alt+Shift+1–5 and Alt+Shift+←/→ move the \
             window there, Alt+Return opens a terminal, Alt+Shift+Esc (or closing the window) quits",
            state.socket_name.to_string_lossy()
        );
    }

    state.session = session;
    SESSION.store(session, Ordering::Relaxed);
    if session {
        // In this order, and on this thread: the target starts units that need to know where the
        // display is, and starting it before the import finishes would bring the portal up with
        // no WAYLAND_DISPLAY — connected to nothing, failing every request, and looking like it
        // works. Unlocking the wallet is the one part that can go on its own thread.
        import_environment();
        start_session_target();
        std::thread::spawn(unlock_kwallet);
        // logind's lock, unlock and sleep, for this session.
        use smithay::reexports::calloop::channel;
        let (sender, events) = channel::channel();
        event_loop
            .handle()
            .insert_source(events, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.logind_event(event);
                }
            })
            .map_err(|err| err.error)?;
        state.logind = logind::Logind::start(sender);
    }
    // X11 apps: Steam, and Flatpaks without Wayland access. SLIPSTREAM_XWAYLAND=0 turns it off.
    if std::env::var("SLIPSTREAM_XWAYLAND").as_deref() != Ok("0") {
        state.start_xwayland();
    }

    // Optionally start one client inside Slipstream: `slipstream --command foot`
    state.concentration.asked(std::time::Instant::now());
    spawn_client(&args);

    // Debug steps also run between frames: a locked host session stops nested redraws.
    if !state.debug.is_empty() {
        let tick = Duration::from_millis(50);
        event_loop
            .handle()
            .insert_source(Timer::from_duration(tick), move |_, _, state| {
                state.run_due_debug_steps();
                TimeoutAction::ToDuration(tick)
            })
            .map_err(|err| err.error)?;
    }

    let started = Instant::now();
    let heartbeat = Arc::new(AtomicU64::new(0));
    if hardware {
        start_watchdog(heartbeat.clone(), started);
    }

    // Wakes at least once a second, so the watchdog can tell idle from stuck.
    event_loop.run(Some(Duration::from_secs(1)), &mut state, move |state| {
        heartbeat.store(started.elapsed().as_secs(), Ordering::Relaxed);
        // The way out's timers run here, so a window closing ends the grace on the same pass.
        state.tick_exit();
        // And a recorded layout's, so it finishes on the pass its last window arrives.
        state.tick_restore();
        // The layout goes to disk here once the desktop has settled, so a crash still leaves one.
        state.tick_record();
        // The lock by itself after idle, and a sleep waiting for the lock to be drawn.
        state.tick_lock();
        // Low battery: the wallpaper's pace, the warnings and the countdown to sleep.
        state.tick_battery();
        // Night light's schedule, and its warmth easing in and out.
        state.tick_night_light();
        state.drop_dead_streams();
        state.refresh_pointer_focus();
        // The portal picks windows to share from this list.
        let windows = state.all_open_windows();
        state.sync_toplevels(windows);
        state.space.refresh();
        state.popups.cleanup();
        // Send queued replies after every pass, not only on redraw (as smallvil did). Anvil
        // does the same.
        let _ = state.display_handle.flush_clients();
    })?;

    // So the login screen, or the next session, doesn't inherit warmed colours.
    if hardware && state.night.current > 0.0 {
        state.set_night_light(0.0);
    }
    // Quitting from inside bullet time would otherwise leave the desktop's sound muffled.
    sound::restore();
    end_session();
    tracing::info!("stopped cleanly");
    Ok(())
}

/// Quits if the event loop stops turning. On real hardware a frozen compositor leaves only the
/// power button; quitting brings the login screen back.
fn start_watchdog(heartbeat: Arc<AtomicU64>, started: Instant) {
    const STUCK_SECS: u64 = 15;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let last = heartbeat.load(Ordering::Relaxed);
            if started.elapsed().as_secs().saturating_sub(last) > STUCK_SECS {
                tracing::error!(
                    "the event loop has been stuck for over {STUCK_SECS} seconds; quitting so the \
                     login screen comes back"
                );
                // This is the exit that most needs the teardown: a session that crashed leaves a
                // stale WAYLAND_DISPLAY in the user manager, pointing the next login's portals
                // and services at a socket that isn't there. It runs on this thread, not the
                // frozen one, so a wedged event loop can't hold it up.
                end_session();
                std::process::exit(2);
            }
        }
    });
}

/// logind ending the session, or a system shutdown. The decision has already been taken outside
/// Slipstream, so this doesn't go through the way out's overlay: it tears the session down and
/// stops the loop, which runs the same tail as any other exit.
fn watch_for_sigterm(event_loop: &EventLoop<'static, Slipstream>) {
    let signals = match Signals::new(&[Signal::SIGTERM]) {
        Ok(signals) => signals,
        Err(err) => {
            tracing::warn!("couldn't watch for SIGTERM: {err}");
            return;
        }
    };
    let inserted =
        event_loop
            .handle()
            .insert_source(signals, |event, _, state: &mut Slipstream| {
                tracing::info!(signal = ?event.signal(), "asked to stop");
                end_session();
                state.loop_signal.stop();
            });
    if let Err(err) = inserted {
        tracing::warn!("couldn't watch for SIGTERM: {}", err.error);
    }
}

/// Tells the user manager a graphical session is up. `graphical-session.target` has
/// `RefuseManualStart=yes`, so it can only be brought up as a dependency of a session target of
/// our own — and until something does, every unit with `Requisite=graphical-session.target`
/// refuses to start: the portal first among them, which is what stops Flatpaks from opening a
/// file dialog or sharing a screen.
fn start_session_target() {
    let _ = launch::command("systemctl")
        .args(["--user", "reset-failed"])
        .status();
    let result = launch::command("systemctl")
        .args(["--user", "start", SESSION_TARGET])
        .status();
    match result {
        Ok(status) if status.success() => tracing::info!("the session target is up"),
        Ok(status) => tracing::warn!("couldn't start {SESSION_TARGET}: {status}"),
        Err(err) => tracing::warn!("couldn't run systemctl: {err}"),
    }
}

/// The other end of `import_environment` and `start_session_target`, on every way out: the
/// normal one, and the watchdog's. Called while the Wayland socket is still open, so units
/// stopping with it can still talk to the compositor. Safe to call twice.
fn end_session() {
    if !SESSION.load(Ordering::Relaxed) {
        return;
    }
    // Once, however many ways out reach here.
    if SESSION_ENDED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = launch::command("systemctl")
        .args([
            "--user",
            "start",
            "--job-mode=replace-irreversibly",
            SHUTDOWN_TARGET,
        ])
        .status();
    // So a later desktop session doesn't inherit a dead display.
    let mut unset = vec!["--user".to_string(), "unset-environment".to_string()];
    unset.extend(SESSION_VARS.iter().map(|name| name.to_string()));
    unset.push("DISPLAY".to_string());
    let _ = launch::command("systemctl").args(&unset).status();
}

/// A panic goes into the log like any other error, and a panic on the event loop's thread, which
/// ends the compositor, tears the session down on its way out as every other exit does. Without
/// that, the user manager kept this session's WAYLAND_DISPLAY and pointed the next login's
/// portals at a socket that was gone. A panic on another thread (a D-Bus server, a reaper) leaves
/// the compositor running, so it only logs.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        tracing::error!(thread = name, "panicked: {info}");
        if name == "main" {
            end_session();
        }
        default(info);
    }));
}

fn init_logging() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    // Colour codes only on a terminal: the login session writes to a log file.
    let format = tracing_subscriber::fmt::layer().with_ansi(std::io::stderr().is_terminal());
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(QuietWhileLocked)
        .with(format)
        .init();
}

/// Nothing at trace level is logged while the screen is locked. Smithay traces every key it
/// handles, by keysym and by keycode, and a key typed at the lock is a character of a password.
struct QuietWhileLocked;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for QuietWhileLocked {
    fn register_callsite(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // Asked every time, since the answer changes as the lock comes and goes.
        if *metadata.level() == tracing::Level::TRACE {
            tracing::subscriber::Interest::sometimes()
        } else {
            tracing::subscriber::Interest::always()
        }
    }

    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        !quiet(*metadata.level(), lock::held())
    }
}

/// Whether a log line or span at `level` is dropped.
fn quiet(level: tracing::Level, locked: bool) -> bool {
    locked && level == tracing::Level::TRACE
}

/// Tells D-Bus and systemd where the display is, so anything they start (portals, services)
/// opens on Slipstream.
///
/// **On the calling thread, and before the session target starts.** It used to run on a detached
/// thread that was never joined; a unit started before the import landed would come up with no
/// `WAYLAND_DISPLAY`, connect to nothing and fail every request — which looks like working.
fn import_environment() {
    // Both, because they populate different places: systemd's user manager environment for units,
    // and dbus-daemon's activation environment for anything it starts itself.
    let mut import = vec!["--user".to_string(), "import-environment".to_string()];
    import.extend(SESSION_VARS.iter().map(|name| name.to_string()));
    let _ = launch::command("systemctl").args(&import).status();
    let mut activation = vec!["--systemd".to_string()];
    activation.extend(SESSION_VARS.iter().map(|name| name.to_string()));
    let result = launch::command("dbus-update-activation-environment")
        .args(&activation)
        .status();
    match result {
        Ok(status) if status.success() => {
            tracing::info!("session environment shared with D-Bus and systemd")
        }
        Ok(status) => tracing::warn!("dbus-update-activation-environment failed: {status}"),
        Err(err) => tracing::warn!("couldn't run dbus-update-activation-environment: {err}"),
    }
}

/// Does what Plasma's `plasma-kwallet-pam.service` does. At login `pam_kwallet` keeps the password
/// on a socket and waits for the session to connect and send its environment; if nothing connects,
/// the wallet stays locked and the first app that wants a secret gets a wallet password prompt.
fn unlock_kwallet() {
    if std::env::var_os("PAM_KWALLET5_LOGIN").is_none() {
        return;
    }
    let candidates = [
        // Fedora, openSUSE
        "/usr/libexec/pam_kwallet_init".to_string(),
        // Arch
        "/usr/lib/pam_kwallet_init".to_string(),
        // Debian, Ubuntu
        format!(
            "/usr/lib/{}-linux-gnu/libexec/pam_kwallet_init",
            std::env::consts::ARCH
        ),
    ];
    let Some(init) = candidates.iter().find(|path| launch::installed(path)) else {
        tracing::warn!(
            "pam_kwallet is set up but pam_kwallet_init isn't installed; KDE Wallet stays locked"
        );
        return;
    };
    let result = launch::command(init)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => tracing::info!("login password handed to KDE Wallet"),
        Ok(status) => tracing::warn!("{init} failed: {status}"),
        Err(err) => tracing::warn!("couldn't run {init}: {err}"),
    }
}

fn spawn_client(args: &[String]) {
    let command = args
        .iter()
        .position(|arg| arg == "-c" || arg == "--command")
        .and_then(|i| args.get(i + 1));
    if let Some(command) = command {
        if let Err(err) = crate::launch::command(command).spawn() {
            tracing::warn!("couldn't start {command}: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_at_trace_level_is_logged_while_locked() {
        use tracing::Level;
        assert!(super::quiet(Level::TRACE, true));
        assert!(!super::quiet(Level::TRACE, false));
        assert!(!super::quiet(Level::DEBUG, true));
        assert!(!super::quiet(Level::INFO, true));
    }
}

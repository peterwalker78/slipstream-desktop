//! The compositor's state: everything Slipstream knows, and what happens to it. This file holds
//! the state itself and what the rest share; the parts are in the files beside it.

use std::{
    collections::HashMap,
    ffi::OsString,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

use smithay::{
    backend::{
        input::{ButtonState, InputTime},
        renderer::utils::with_renderer_surface_state,
    },
    desktop::{PopupManager, Space, Window, WindowSurfaceType, space::SpaceElement},
    input::{
        Seat, SeatState,
        keyboard::{Keysym, xkb},
        pointer::{ButtonEvent, CursorImageStatus, MotionEvent},
    },
    output::Output,
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, channel,
            generic::Generic,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, IsAlive, Logical, Monotonic, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufGlobal, DmabufState},
        foreign_toplevel_list::ForeignToplevelListState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        image_capture_source::{
            ImageCaptureSourceState, OutputCaptureSourceState, ToplevelCaptureSourceState,
        },
        image_copy_capture::ImageCopyCaptureState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        seat::WaylandFocus,
        selection::{
            data_device::DataDeviceState, ext_data_control,
            primary_selection::PrimarySelectionState, wlr_data_control,
        },
        shell::xdg::{
            SurfaceCachedState, XdgShellState, XdgToplevelSurfaceData,
            decoration::XdgDecorationState,
        },
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Surface, X11Wm},
};

use crate::{
    anim::{self, Tween},
    bar,
    bullet::{self, Target, Typed},
    capture::{self, Captures},
    card, clipboard,
    connect::{self, Connect},
    debug,
    exit::{self, Act, Exit, Intent},
    explorer::{Explorer, Item, Outcome, System},
    focus::KeyboardFocus,
    gravity::{Rung, Step},
    idle::Idle,
    inhibit::LidInhibitor,
    keys::{self, Mods},
    known, launch,
    layout::{self, Direction, Rect},
    motion::{self, Motion},
    offer::{self, Offer},
    osd,
    rain::Rain,
    restore, resume,
    saver::Saver,
    screen::Screens,
    session,
    share::{self, Picker},
    status,
    toast::Toast,
    udev::UdevData,
    workspace::{Workspace, Workspaces},
};

mod app_shortcuts;
mod arrangements;
mod background;
mod bullet_time;
mod debug_steps;
mod protocols;
pub use protocols::Protocols;
mod docked;
mod keyboard_focus;
mod lifecycle;
mod screens;
mod streams;
mod ui;
mod window_props;
mod windows;
mod workspaces;

pub use screens::is_internal_panel;
pub(crate) use window_props::{
    PlacesItself, deactivate, is_child_of, logged_app, min_size, own_opacity, places_itself,
    takes_no_input, unseen, window_app_id, window_title,
};
use window_props::{configure, pretty_name, toplevel_data};

/// How long the recorded layout waits for the desktop entries to be read before giving up on
/// this login. The scan takes a fraction of a second; this is only a backstop.
const CATALOGUE_WAIT: f64 = 10.0;

/// How still the desktop has to be before the layout is written down again. Opening a window
/// retiles several times over as the client catches up, and none of those are worth a file.
const RECORD_SETTLE: f64 = 2.0;

/// How long a desktop that hasn't moved goes before the record is checked against it anyway.
const RECORD_REFRESH: f64 = 60.0;

/// How long a window takes to pour into its stream in the code rain, on its own and when a whole
/// workspace goes at once. A screenful moving together needs to be quicker: at the single
/// window's pace the same motion reads as the desktop sliding away rather than as each window
/// finding its own stream.
const POUR: f64 = crate::motion::MOVE;
const POUR_ALL: f64 = 0.2;

pub struct Slipstream {
    /// The window docked to the bar, one on its way down, the pane's size, and their slots in the bar.
    pub docking: crate::dock::Docking,
    /// The layout as written down for the next login, and when it is next due.
    pub record: session::Recording,
    /// Screenshots and snips: the shots asked for and being saved, a snip being chosen or flying off, and the flash.
    pub shots: crate::screenshot::Shots,
    /// What the frames last drew: where things landed, for the pointer, the windows drawn from outside the space, and each screen's drawing buffers.
    pub drawn: crate::render::Frames,
    /// What the keyboard's own keys are doing: Super and layout taps, Caps Lock, and presses held back from apps.
    pub keys: crate::input::KeyTracking,
    /// The Wayland protocols' own state.
    pub protocols: Protocols,
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, Slipstream>,

    /// What each screen's layer surfaces leave for tiling, as last tiled.
    pub layer_zones: Vec<(String, Rectangle<i32, Logical>)>,
    /// The card asking what to share, while an app waits for the answer.
    pub share: Option<Picker>,
    /// The card asking for a password on polkit's behalf, and the questions behind it.
    pub polkit: crate::polkit::Polkit,
    /// What is being captured, and what is waiting for the next frame of it.
    pub captures: Captures,
    pub popups: PopupManager,

    pub seat: Seat<Self>,
    pub cursor_status: CursorImageStatus,

    // Slipstream State
    pub bindings: Vec<keys::Binding>,
    /// Awake is on: the UI doesn't fade to the wallpaper. Held Caps Lock toggles it.
    pub awake: bool,
    /// Running as a window inside another desktop, where Alt stands in for Super.
    pub nested: bool,
    /// Started from the login screen, so the session environment is shared with D-Bus.
    pub session: bool,
    /// This compositor holds the state folder's owner lock (`live.rs`), so it is the one that
    /// offers, reopens and writes the recorded layout, and writes the settings file.
    pub owns_state: bool,
    /// The hardware backend, when running as a session rather than nested.
    pub udev: Option<UdevData>,
    /// The X11 window manager, once XWayland is up.
    pub xwm: Option<X11Wm>,
    /// An X11 window waiting for its Wayland surface before it can take keyboard focus.
    pub pending_focus: Option<Window>,
    /// Windows, most recently focused first, for Alt+Tab.
    pub focus_history: Vec<Window>,
    /// Alt+Tab's switcher, while Alt is held (`switcher.rs`).
    pub switcher: Option<crate::switcher::Switcher<Window>>,
    /// Whether the switcher settles when Super is let go, rather than Alt.
    pub switcher_on_super: bool,
    /// Gravity's arrangements, while Super is held after Super+T (`arrange.rs`).
    pub arrange: Option<crate::arrange::Arrange<Window>>,
    /// The arrangement a quick Super+T turns gravity on at: the last one kept.
    pub arrangement: Rung,
    /// The windows filling a screen at their own request (F11, video, games). At most one per
    /// workspace, so a video can fill one screen while another is worked on.
    pub fullscreen: Vec<Window>,
    /// Floating windows that opened before knowing their size, and their parents, to centre once
    /// they do.
    pub centre_when_sized: Vec<(Window, Option<Window>)>,
    /// The size each floating window on screen had when it was last placed.
    pub floating_sizes: Vec<(Window, (i32, i32))>,
    /// Floating windows tiled while they fill the screen, and where they float again after.
    pub refloat: Vec<(Window, crate::floating::Float<Window>)>,
    /// The screen zoom, for the session.
    pub zoom: crate::zoom::Zoom,
    /// Super+D: the workspace it left, and the empty one it went to.
    pub desktop_return: Option<(usize, usize)>,
    /// A gap or a window being dragged with the pointer (`grabs.rs`).
    pub drag: Option<crate::grabs::Drag>,
    /// A dragged gap has moved since the tiles last followed it.
    pub drag_retile_due: bool,
    /// Slipstream's own cursor over a gap or during a drag, drawn instead of the app's.
    pub cursor_override: Option<smithay::input::pointer::CursorIcon>,
    pub workspaces: Workspaces<Window>,
    /// The lit screens, left to right, and which workspace each is showing (`screen.rs`).
    pub screens: Screens<Output>,
    /// Held while an external screen is connected, so logind leaves the lid to us.
    pub lid_inhibitor: LidInhibitor,
    /// Nested only: the screen standing in for the laptop's panel while a headless check has
    /// the lid shut.
    pub nested_panel: Option<Output>,
    /// The name of the screen a nested check's lid has put out, which is arranged as a panel
    /// from then on, as the laptop's own is.
    pub nested_panel_name: Option<String>,
    /// Screen names in the order they connected, for arranging them.
    pub screen_order: Vec<String>,
    /// The laptop's lid is shut. With another screen connected the panel goes out and the
    /// desktop carries on there; with nothing else connected the lid is left alone, since
    /// putting out the only screen would leave nothing to come back to.
    pub lid_closed: bool,
    /// Every animation reads this clock, so bullet time can slow them all at once.
    pub clock: anim::Clock,
    /// Where windows are drawn on their way to where the layout put them.
    pub motion: Motion<Window>,
    /// The app explorer, on a tapped Super.
    pub explorer: Explorer,
    /// Quick settings (Super+A).
    pub quick: crate::quick::QuickSettings,
    /// The user's name, as quick settings shows it.
    pub user: String,
    /// The notification centre (Super+N).
    pub centre: crate::centre::Centre,
    /// The shortcut sheet (Super+/).
    pub sheet: crate::sheet::Sheet,
    /// Night light's warmth and schedule (`nightlight.rs`).
    pub night: crate::nightlight::Night,
    /// Low battery's warnings and its countdown card (`battery.rs`).
    pub battery: crate::battery::Battery,
    /// The clipboard history (Super+V).
    pub history: crate::history::History,
    /// Where copies read for the history come back from their threads.
    pub clip_answers: channel::Sender<crate::history::Clip>,
    /// Notifications apps have sent, and their pop-ups.
    pub notices: crate::notices::Notices,
    /// Messages near the top of the screen.
    pub toast: Toast,
    /// The volume and brightness display, low on the screen.
    pub osd: crate::osd::Osd,
    /// The traces the faded desktop shows for agents at work, and when the agents were last asked
    /// after.
    pub scope: crate::scope::Scope,
    pub agents_read: Option<std::time::Instant>,
    /// The way out, while it's on screen: Ctrl+Alt+Del, or Log out, Restart or Shut down.
    pub exit: Option<Exit<Window>>,
    /// The windows Super+H put into the code rain, so the next press brings back those and
    /// nothing else that happens to be minimised.
    hidden: Option<Vec<Window>>,
    /// Toplevels a client has created but not yet shown. They take no tile and no focus until
    /// their first buffer arrives, and plenty never arrive at all.
    pub unmapped: Vec<Window>,
    /// Apps running with no window up, for the notification centre: clients whose windows have
    /// all closed, and tray items.
    pub background: background::Background,
    /// Windows that asked for fullscreen before they had anything to show — players and games
    /// usually do — and get it the moment they map.
    pub fullscreen_on_map: Vec<Window>,
    /// The recorded layout going back up, while the apps are still starting.
    pub restoring: Option<restore::Plan<Window>>,
    /// The card at login asking whether to put the last layout back.
    pub offer: Option<Offer>,
    /// The card asking what a screen Slipstream has never seen should show.
    pub connect: Option<Connect>,
    /// Shortcuts apps keep while another app has the keyboard, and the card that asks about
    /// them.
    pub shortcuts: crate::shortcuts::Shortcuts,
    /// The screens this machine has shown a desktop on, and what each one comes back to
    /// (`known.rs`). Read at startup, written as screens come and go.
    pub known: known::Remembered,
    /// Whether the recorded layout has been looked at yet. It waits for the desktop entries to be
    /// read, since without them nothing in the record can be matched to an app to run.
    restore_looked: bool,
    /// Gravity's tier tags, each shown on its window for a moment: the window, its new tier, and
    /// when the tag appeared on the animation clock.
    pub tags: Vec<(Window, Rung, f64)>,
    /// Minimised windows, each a stream of code rain on the right.
    pub rain: Rain,
    /// When the UI fades to show the living wallpaper.
    pub idle: Idle,
    /// Whether you're typing, which decides what may take the keyboard.
    pub concentration: crate::concentration::Concentration,
    /// The living wallpaper, one per screen: each keeps its own grid, sized to that screen, and
    /// its own turn through the variations.
    pub savers: HashMap<String, Saver>,
    /// What a new screen's wallpaper starts from.
    wallpaper: slipstream_config::Wallpaper,
    /// Input arrived since `ext-idle-notify` was last told: it's told once a pass rather than on
    /// every event, since each telling re-arms a timer per listener.
    pub input_since_notified: bool,
    /// The screens are off after a spell with no input; the next input lights them again.
    pub screens_off: bool,
    /// Which screens' gamma ramps a program of the user's own is driving.
    pub gamma: crate::gamma::Gamma,
    /// Screen captures asked for through `wlr-screencopy`, waiting for their screen to draw.
    pub screencopy: crate::screencopy::Screencopy,
    /// Which windows have been told they hold the pointer or the keys, and the one refused.
    pub takeback: crate::takeback::TakeBack,
    /// Bullet time while it's open (Super+Tab).
    pub bullet: Option<bullet::Mode<Window>>,
    /// A three-finger swipe under way (`input/swipe.rs`).
    pub swipe: Option<crate::input::swipe::Swipe>,
    /// How far out bullet time's overview is zoomed, 0 to 1, on wall time so it's never slowed.
    pub overview: Tween<1>,
    /// The last labels bullet time used, kept while the same windows are open.
    bullet_labels: Vec<(Target<Window>, String)>,
    /// Whether the pointer was kept from windows when last looked, so a change is noticed.
    pub pointer_was_blocked: bool,
    /// Scroll short of a whole notch, over the explorer or the centre.
    pub wheel_rest: f64,
    /// Where what a player is doing after a media key comes back to.
    pub media_replies: Option<smithay::reexports::calloop::channel::Sender<crate::media::Playing>>,
    /// Closed windows fading where they were.
    pub ghosts: Vec<crate::ghost::Ghost>,
    /// The minimum size each window had when it was last tiled, so a client changing its own
    /// retiles.
    tiled_mins: Vec<(Window, (i32, i32))>,
    /// Clock and battery readings, refreshed off the main thread.
    pub status: Arc<Mutex<status::Reading>>,
    /// The person's plugins, each that's on running its program.
    pub plugins: crate::plugins::Plugins,
    /// The panel that drops from the plugin chip.
    pub plugin_panel: crate::plugin_panel::PluginPanel,
    pub debug: debug::Script,
    /// The settings as applied: the file's, with any environment overrides.
    pub settings: slipstream_config::Settings,
    /// The system's keyboard layouts, used while Settings chooses none.
    pub layouts: crate::layouts::Layouts,
    /// The focused window's ring colour, parsed from the settings once rather than every frame.
    /// The panels' keyboard ring takes it too: one colour for where the keys are aimed.
    pub ring_rgb: [f32; 3],
    /// Bullet time's rings and frames, from the settings in the same way.
    pub bullet_rgb: [f32; 3],
    /// What's on or chosen inside the panels, from the settings in the same way.
    pub accent_rgb: [f32; 3],
    /// Where logind's refusals, and a nested run's stand-in answers, come back to the event loop
    /// (`power.rs`).
    power_answers: channel::Sender<crate::power::Answer>,
    /// The lock screen, while it's up (`lock.rs`).
    pub lock: Option<crate::lock::Lock<Window>>,
    /// The lock going away, while it fades.
    pub unlocking: Option<crate::lock::Unlocking>,
    /// Alt+Tab's deck of glass panes, while it's open and while its panes fly home (`deck.rs`).
    pub deck: Option<crate::deck::Deck<Window>>,
    /// Two tiles passing through each other as they trade places (`pane.rs`).
    pub pass: Option<crate::pane::Pass<Window>>,
    /// Where password checks answer, with their numbers.
    pub lock_answers: channel::Sender<(u64, crate::auth::Verdict)>,
    /// The session's line to logind, for locking, unlocking and sleep (`logind.rs`).
    pub logind: crate::logind::Logind,
    /// The sleep inhibitor, held until the lock is on screen.
    pub sleep_delay: crate::logind::SleepDelay<std::os::fd::OwnedFd>,
    /// A lock that went up by itself has been refused and said so already this session.
    pub lock_refusal_told: bool,
}

impl Slipstream {
    pub fn new(event_loop: &mut EventLoop<'static, Self>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();

        let dh = display.handle();

        // The protocols' handler traits are implemented in `crate::handlers` and beside each
        // feature that speaks one.
        let mut protocols = Protocols::new(&dh, &event_loop.handle());
        let popups = PopupManager::default();
        let mut seat: Seat<Self> = protocols.seat_state.new_wl_seat(&dh, "seat0");

        let settings = crate::settings::startup();
        crate::layouts::add_keyboard(&mut seat, &settings.keyboard);
        // There is always a pointer, plugged in or not, so the rest of the code can rely on one.
        seat.add_pointer();

        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        // logind's answers to Sleep, Restart and Shut down arrive here from the thread that asked.
        let (power_answers, answers) = channel::channel();
        event_loop
            .handle()
            .insert_source(answers, |event, _, state| {
                if let channel::Event::Msg(answer) = event {
                    state.power_answered(answer);
                }
            })
            .expect("a channel source always inserts");

        // Apps started on threads of their own (`launch::start_app`) report back here.
        let (reports, launched) = channel::channel();
        event_loop
            .handle()
            .insert_source(launched, |event, _, state| {
                if let channel::Event::Msg((done, result)) = event {
                    let done: crate::launch::Done = done;
                    done(state, result);
                }
            })
            .expect("a channel source always inserts");
        crate::launch::report_to(reports);

        // Copies read for the clipboard history, from their threads.
        let (clip_answers, clips) = channel::channel();
        event_loop
            .handle()
            .insert_source(clips, |event, _, state| {
                if let channel::Event::Msg(clip) = event {
                    state.clip_read(clip);
                }
            })
            .expect("a channel source always inserts");

        // Screenshots, from the threads that wrote them.
        let (screenshot_answers, saved) = channel::channel();
        event_loop
            .handle()
            .insert_source(saved, |event, _, state| {
                if let channel::Event::Msg(saved) = event {
                    state.screenshot_saved(saved);
                }
            })
            .expect("a channel source always inserts");

        // Password checks answer here from the thread that waits for each.
        let (lock_answers, answers) = channel::channel();
        event_loop
            .handle()
            .insert_source(answers, |event, _, state| {
                if let channel::Event::Msg((id, verdict)) = event {
                    state.lock_checked(id, verdict);
                }
            })
            .expect("a channel source always inserts");

        // logind's answer to taking the lid switch, from the thread that asked.
        let (lid_answers, answers) = channel::channel();
        event_loop
            .handle()
            .insert_source(answers, |event, _, state| {
                if let channel::Event::Msg(fd) = event {
                    state.lid_inhibitor.answered(fd);
                }
            })
            .expect("a channel source always inserts");

        match slipstream_config::plugins::move_meter_once() {
            Some(Ok(dir)) => tracing::info!(
                "the settings file's [meter] is a plugin now, in {}, off until it is turned on",
                dir.display()
            ),
            Some(Err(why)) => {
                tracing::warn!("the settings file's [meter] couldn't be moved: {why}")
            }
            None => {}
        }
        let mut plugins = crate::plugins::Plugins::default();
        plugins.configure(&settings);
        let ring_rgb = settings.borders.selected_tile_rgb();
        let bullet_rgb = settings.borders.bullet_time_rgb();
        let accent_rgb = settings.borders.accent_rgb();

        let reduced_motion = settings.motion.reduced;
        let mut state = Self {
            docking: Default::default(),
            record: Default::default(),
            shots: crate::screenshot::Shots::new(screenshot_answers),
            drawn: Default::default(),
            keys: Default::default(),
            start_time,
            display_handle: dh,
            protocols,

            space,
            loop_signal,
            loop_handle: event_loop.handle(),
            socket_name,

            layer_zones: Vec::new(),
            share: None,
            polkit: Default::default(),
            captures: Captures::default(),
            popups,
            seat,
            cursor_status: CursorImageStatus::default_named(),

            bindings: keys::checked(keys::defaults(settings.keys.style), settings.keys.style),
            awake: false,
            nested: false,
            owns_state: true,
            session: false,
            udev: None,
            xwm: None,
            pending_focus: None,
            focus_history: Vec::new(),
            switcher: None,
            switcher_on_super: false,
            arrange: None,
            arrangement: Rung::Centre,
            fullscreen: Vec::new(),
            refloat: Vec::new(),
            floating_sizes: Vec::new(),
            centre_when_sized: Vec::new(),
            zoom: Default::default(),
            history: Default::default(),
            battery: Default::default(),
            night: Default::default(),
            clip_answers,
            desktop_return: None,
            drag: None,
            drag_retile_due: false,
            cursor_override: None,
            workspaces: Workspaces::new(
                &workspace_entries(&settings),
                crate::layout::OUTER_GAP,
                crate::layout::INNER_GAP,
            ),
            screens: Screens::default(),
            lid_inhibitor: LidInhibitor::new(lid_answers),
            nested_panel: None,
            nested_panel_name: None,
            screen_order: Vec::new(),
            lid_closed: false,
            clock: anim::Clock::new(false),
            motion: Motion::new(false),
            explorer: Explorer::new(false),
            quick: Default::default(),
            user: crate::quick::user_name(),
            centre: Default::default(),
            sheet: Default::default(),
            notices: Default::default(),
            toast: Toast::new(false),
            osd: crate::osd::Osd::new(false),
            scope: {
                let mut scope = crate::scope::Scope::new(false);
                scope.set_colours(ring_rgb, bullet_rgb);
                scope
            },
            agents_read: None,
            exit: None,
            hidden: None,
            unmapped: Vec::new(),
            background: background::Background::default(),
            fullscreen_on_map: Vec::new(),
            restoring: None,
            offer: None,
            connect: None,
            shortcuts: crate::shortcuts::Shortcuts::new(slipstream_config::shortcuts::read(
                &slipstream_config::shortcuts::path(),
            )),
            known: known::read(&known::path()),
            restore_looked: false,
            tags: Vec::new(),
            rain: Rain::new(false, settings.motion.effects, ring_rgb),
            idle: Idle::new(false, settings.wallpaper.fade_after_secs),
            concentration: Default::default(),
            savers: HashMap::new(),
            wallpaper: settings.wallpaper.clone(),
            input_since_notified: false,
            screens_off: false,
            gamma: crate::gamma::Gamma::default(),
            screencopy: crate::screencopy::Screencopy::default(),
            takeback: Default::default(),
            bullet: None,
            swipe: None,
            overview: Tween::at_rest([0.0]),
            bullet_labels: Vec::new(),
            pointer_was_blocked: false,
            wheel_rest: 0.0,
            media_replies: None,
            ghosts: Vec::new(),
            tiled_mins: Vec::new(),
            status: status::start(),
            plugins,
            plugin_panel: crate::plugin_panel::PluginPanel::default(),
            debug: debug::Script::default(),
            settings,
            layouts: crate::layouts::Layouts::from_env(),
            ring_rgb,
            bullet_rgb,
            accent_rgb,
            power_answers,
            lock: None,
            unlocking: None,
            deck: None,
            pass: None,
            lock_answers,
            logind: Default::default(),
            sleep_delay: Default::default(),
            lock_refusal_told: false,
        };
        state.set_reduced_motion(reduced_motion);
        state
            .idle
            .set_lock_after(state.settings.lock.after_idle_mins);
        state
            .idle
            .set_screen_off_after(state.settings.display.screen_off_mins);
        let keys = state.settings.keys.clone();
        state.set_own_keys(&keys);
        state
    }

    /// The person's own shortcuts that work with `keys`' style.
    pub fn set_own_keys(&mut self, keys: &slipstream_config::Keys) {
        let name = |id: &str| {
            crate::apps::find(id)
                .map(|app| app.name)
                .unwrap_or_else(|| id.to_string())
        };
        let own =
            crate::shortcuts::own::usable(keys.style, &keys.apps, self.shortcuts.granted(), name);
        self.shortcuts.set_own(own);
    }

    /// Every effect has a reduced-motion path: moves jump and effects become short fades. This
    /// is the one place the setting reaches the parts that animate, at startup and whenever the
    /// settings change, so none of them can be left out of step with the rest.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.clock.reduced_motion = on;
        self.motion.reduced_motion = on;
        self.explorer.reduced_motion = on;
        self.history.reduced_motion = on;
        self.toast.reduced_motion = on;
        self.osd.reduced_motion = on;
        self.scope.reduced_motion = on;
        self.rain.reduced_motion = on;
        self.idle.reduced_motion = on;
        for saver in self.savers.values_mut() {
            saver.reduced_motion = on;
        }
        self.notices.reduced_motion = on;
        self.quick.reduced_motion = on;
        self.centre.reduced_motion = on;
        if let Some(exit) = self.exit.as_mut() {
            exit.reduced_motion = on;
        }
        if let Some(offer) = self.offer.as_mut() {
            offer.reduced_motion = on;
        }
        if let Some(connect) = self.connect.as_mut() {
            connect.reduced_motion = on;
        }
        if let Some(ask) = self.shortcuts.ask_mut() {
            ask.reduced_motion = on;
        }
        if let Some(share) = self.share.as_mut() {
            share.reduced_motion = on;
        }
        if let Some(prompt) = self.polkit.prompt.as_mut() {
            prompt.reduced_motion = on;
        }
        if let Some(lock) = self.lock.as_mut() {
            lock.reduced_motion = on;
        }
        if let Some(unlocking) = self.unlocking.as_mut() {
            unlocking.reduced_motion = on;
        }
    }

    /// The `motion` debug step: each animating part's reduced-motion flag, as it stands. Cards
    /// that aren't up have none to show.
    fn log_reduced_motion(&self) {
        let card = |flag: Option<bool>| flag.map_or("not open".to_string(), |on| on.to_string());
        let savers: Vec<bool> = self
            .savers
            .values()
            .map(|saver| saver.reduced_motion)
            .collect();
        tracing::info!(
            clock = self.clock.reduced_motion,
            motion = self.motion.reduced_motion,
            explorer = self.explorer.reduced_motion,
            toast = self.toast.reduced_motion,
            osd = self.osd.reduced_motion,
            rain = self.rain.reduced_motion,
            idle = self.idle.reduced_motion,
            savers = ?savers,
            notices = self.notices.reduced_motion,
            quick = self.quick.reduced_motion,
            centre = self.centre.reduced_motion,
            exit = card(self.exit.as_ref().map(|exit| exit.reduced_motion)),
            offer = card(self.offer.as_ref().map(|offer| offer.reduced_motion)),
            share = card(self.share.as_ref().map(|share| share.reduced_motion)),
            lock = card(self.lock.as_ref().map(|lock| lock.reduced_motion)),
            "reduced motion"
        );
    }

    fn init_wayland_listener(
        display: Display<Slipstream>,
        event_loop: &mut EventLoop<Self>,
    ) -> OsString {
        // The next free `wayland-N` name.
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                let data = ClientState::for_connection(&client_stream);
                // A client that can't be taken on (out of file descriptors, say) is turned away;
                // the desktop carries on for everyone else.
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(data))
                {
                    tracing::warn!("couldn't take on a new client: {err}");
                }
            })
            .expect("Failed to init the wayland event source.");

        // Client requests are read and dispatched whenever the display's socket is readable.
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: we don't drop the display
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    /// A floating window drew itself at a new size: it's placed again, so it stays inside its area
    /// and its drawing matches.
    pub fn retile_if_floating_resized(&mut self, window: &Window) {
        if !self.workspaces.is_floating(window) {
            return;
        }
        self.centre_now_sized(window);
        let size = crate::floating::own_size(window);
        let placed = self
            .floating_sizes
            .iter()
            .find(|(w, _)| w == window)
            .map(|(_, placed)| *placed);
        if placed != Some(size) {
            self.retile();
        }
    }

    /// Retiles when a window's client has changed its minimum size since it was last tiled.
    pub fn retile_if_min_changed(&mut self, window: &Window) {
        let changed = self
            .tiled_mins
            .iter()
            .find(|(w, _)| w == window)
            .is_some_and(|(_, min)| *min != min_size(window));
        if changed {
            tracing::info!(window = logged_app(window), min = ?min_size(window), "minimum size changed");
            self.retile();
        }
    }

    /// What the pointer is over, popups included, and where that surface sits. Always a Wayland
    /// surface: XWayland's windows have one too.
    ///
    /// For a window drawn scaled into its tile, "where the surface sits" is worked out afresh
    /// from each position: the point is mapped back into the window's own size, and the surface
    /// is placed so that the client gets that point.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(KeyboardFocus, Point<f64, Logical>)> {
        if let Some(under) = self.layer_surface_under(pos, &crate::layers::FRONT) {
            return Some(under);
        }
        if let Some((window, local)) = self.window_under(pos) {
            return window
                .surface_under(local, WindowSurfaceType::ALL)
                .map(|(s, p)| (KeyboardFocus::Wayland(s), pos - (local - p.to_f64())));
        }
        self.layer_surface_under(pos, &crate::layers::BACK)
    }

    /// The window drawn at `pos`, topmost first, and the point in the window's own coordinates
    /// (its surface's, unscaled). A window scaled into its tile is hit where it is drawn, not
    /// across the neighbour its real size would overlap.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<f64, Logical>)> {
        // The docked pane is drawn in front of every window whichever was raised last, so it is
        // asked first: a tile under it that has the keyboard must not take its clicks.
        let docked = self.docked_on_show();
        let rest = self
            .space
            .elements()
            .rev()
            .filter(|window| docked.as_ref() != Some(*window));
        docked.iter().chain(rest).find_map(|window| {
            // A window too see-through to be seen can't be aimed at: clicks go to what's under it.
            if unseen(window) {
                return None;
            }
            if let Some((_, drawn)) = self.drawn.fitted.iter().find(|(w, _)| w == window) {
                let own = window.geometry();
                if !drawn.contains(pos) || own.size.w <= 0 {
                    return None;
                }
                let k = drawn.size.w / own.size.w as f64;
                let local = (pos - drawn.loc).downscale(k) + own.loc.to_f64();
                return window
                    .is_in_input_region(&local)
                    .then(|| (window.clone(), local));
            }
            let location = self.space.element_location(window)? - window.geometry().loc;
            let bbox = self.space.element_bbox(window)?;
            let local = pos - location.to_f64();
            (bbox.to_f64().contains(pos) && window.is_in_input_region(&local))
                .then(|| (window.clone(), local))
        })
    }

    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat.get_pointer().unwrap().current_location()
    }

    /// How `output` is magnified now. Only the screen with the pointer on it is: the zoom is
    /// centred on the pointer, and goes with it from screen to screen.
    pub fn zoom_view(&self, output: &Output) -> crate::zoom::View {
        let level = self.zoom.level(self.clock.wall());
        let pointer = self.pointer_location();
        match self.space.output_geometry(output) {
            Some(geo) if level > 1.0 && geo.to_f64().contains(pointer) => crate::zoom::View::about(
                pointer,
                geo.loc,
                output.current_scale().fractional_scale(),
                level,
            ),
            _ => crate::zoom::View::NONE,
        }
    }

    /// A zoom key pressed.
    pub fn zoom_step(&mut self, step: crate::zoom::Step) {
        let now = self.clock.wall();
        self.zoom.step(step, now, self.clock.reduced_motion);
    }

    /// The workspace the keyboard is on: the one the focused screen is showing.
    pub fn active_workspace(&self) -> usize {
        self.screens.workspace()
    }

    /// The workspace on the focused screen, where a new window goes and what the keys act on.
    pub fn current_workspace(&self) -> &Workspace<Window> {
        self.workspaces.get(self.active_workspace())
    }

    pub fn current_workspace_mut(&mut self) -> &mut Workspace<Window> {
        let active = self.active_workspace();
        self.workspaces.get_mut(active)
    }

    /// The focused screen's name, which is what the per-screen slide animations are keyed by.
    pub fn focused_screen_name(&self) -> String {
        self.screens
            .focused_output()
            .map(|output| output.name())
            .unwrap_or_default()
    }

    /// Forgets a screen's buffers when it goes out, so an unplugged monitor doesn't leave a
    /// screenful of pixmaps behind.
    pub fn forget_screen_drawing(&mut self, screen: &str) {
        self.drawn.chromes.remove(screen);
        self.savers.remove(screen);
    }

    /// The bar button at this point on the focused screen, which is where the panels that ask
    /// are drawn.
    pub fn focused_bar_target(&self, x: f64, y: f64) -> Option<bar::Target> {
        let name = self.screens.focused_output()?.name();
        self.drawn.chromes.get(&name)?.bar.target_at(x, y)
    }

    /// The screen the explorer, the panels, the cards, pop-ups and bullet time are drawn on, and
    /// where it sits: the focused one. Every hover and click on them is measured from here.
    pub fn overlay_screen(&self) -> Option<(usize, Rect)> {
        let index = self.screens.focused_index();
        Some((index, self.screen_rect(index)?))
    }

    /// Where the focused screen sits, for the cards drawn in its own coordinates.
    pub fn focused_screen_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.screens.focused_output()?;
        self.space.output_geometry(&output)
    }

    /// The screen the pointer is on.
    pub fn screen_under_pointer(&self) -> Option<usize> {
        self.screen_at(self.pointer_location())
    }

    /// The screen a point is on, in the space's logical pixels.
    pub fn screen_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        self.screens.iter().position(|screen| {
            self.space
                .output_geometry(&screen.output)
                .is_some_and(|geo| geo.to_f64().contains(pos))
        })
    }

    /// The whole of one screen, where a fullscreen window goes.
    pub fn screen_rect(&self, index: usize) -> Option<Rect> {
        let screen = self.screens.get(index)?;
        let geo = self.space.output_geometry(&screen.output)?;
        Some(Rect {
            x: geo.loc.x,
            y: geo.loc.y,
            w: geo.size.w,
            h: geo.size.h,
        })
    }

    /// The area windows tile into on one screen: below the bar, and on the screen that has the
    /// code rain, left of it.
    ///
    /// The rain stays on the leftmost screen rather than following the keyboard: it takes width
    /// from the tiling area, and a strip that appeared and disappeared as focus moved between
    /// screens would re-tile both of them every time.
    pub fn screen_area(&self, index: usize) -> Option<Rect> {
        let screen = self.screen_rect(index)?;
        let reserve = if index == 0 { self.rain.reserve() } else { 0 };
        let area = Rect {
            y: screen.y + bar::HEIGHT,
            w: (screen.w - reserve).max(1),
            h: (screen.h - bar::HEIGHT).max(1),
            ..screen
        };
        // Less whatever docks and panels along the edges have reserved.
        let zone = self.screens.get(index).map(|screen| {
            smithay::desktop::layer_map_for_output(&screen.output).non_exclusive_zone()
        });
        Some(match zone {
            Some(zone) if zone.size.w > 0 && zone.size.h > 0 => {
                crate::layers::within_zone(area, screen, zone)
            }
            _ => area,
        })
    }

    /// The whole of the focused screen.
    pub fn output_rect(&self) -> Option<Rect> {
        self.screen_rect(self.screens.focused_index())
    }

    /// The area windows tile into on the focused screen, which is where a new window goes.
    pub fn output_area(&self) -> Option<Rect> {
        self.screen_area(self.screens.focused_index())
    }

    /// Where workspace `index` is laid out: on the screen showing it, or — when no screen is —
    /// on the focused one, which is where it would appear if it were switched to.
    pub fn area_for_workspace(&self, index: usize) -> Option<Rect> {
        self.screen_area(self.screens.screen_for_workspace(index))
    }

    /// The whole of the screen workspace `index` is on.
    pub fn screen_rect_for_workspace(&self, index: usize) -> Option<Rect> {
        self.screen_rect(self.screens.screen_for_workspace(index))
    }

    pub fn output_scale(&self) -> Option<f64> {
        let output = self
            .screens
            .focused_output()
            .or_else(|| self.space.outputs().next().cloned())?;
        Some(output.current_scale().fractional_scale())
    }

    /// The focused window's title, for the bar.
    pub fn focused_title(&self) -> String {
        self.focused_window()
            .map(|window| window_title(&window))
            .unwrap_or_default()
    }

    /// An open window of the app with this app ID or desktop entry ID, minimised ones included.
    pub fn window_for_app(&self, app: &str) -> Option<Window> {
        let app = app.to_lowercase();
        let app = app.strip_suffix(".desktop").unwrap_or(&app);
        self.all_open_windows()
            .into_iter()
            .find(|window| window.alive() && window_app_id(window).as_deref() == Some(app))
    }

    /// The window a process tree belongs to: of `pids`, nearest first, the first that has an open
    /// window. When that process has several, the focused one, else the one focused most recently.
    pub fn window_for_pids(&self, pids: &[u32]) -> Option<Window> {
        let windows: Vec<(Window, u32)> = self
            .all_open_windows()
            .into_iter()
            .filter(|window| window.alive())
            .filter_map(|window| Some((window.clone(), self.window_pid(&window)?)))
            .collect();
        let pid = pids
            .iter()
            .find(|pid| windows.iter().any(|(_, owner)| owner == *pid))?;
        let mine: Vec<&Window> = windows
            .iter()
            .filter(|(_, owner)| owner == pid)
            .map(|(window, _)| window)
            .collect();
        self.focus_history
            .iter()
            .find(|recent| mine.contains(recent))
            .or(mine.first().copied())
            .cloned()
    }

    /// A click on the bar.
    pub fn bar_clicked(&mut self, target: bar::Target) {
        match target {
            bar::Target::Workspace(index) => self.switch_workspace(index),
            bar::Target::Apps => self.toggle_explorer(),
            bar::Target::Tray => self.toggle_quick_settings(),
            bar::Target::Plugins => self.toggle_plugin_panel(),
            bar::Target::Clock | bar::Target::Bell => self.toggle_notification_centre(),
            bar::Target::Overview => self.toggle_bullet_time(),
            bar::Target::Dock => self.dock_clicked(),
            bar::Target::Awake => self.set_awake(false),
            bar::Target::Layout => self.next_layout(),
            bar::Target::Sharing => {
                self.stop_sharing();
                self.show_toast(
                    "Sharing stopped",
                    "Nothing on this desktop is being shared now.",
                );
            }
        }
    }
}

/// How recently a window was focused: 0 for the focused window, higher for older ones.
fn recency(history: &[Window], window: &Window) -> usize {
    history
        .iter()
        .position(|w| w == window)
        .unwrap_or(usize::MAX)
}

/// The settings' workspace list as ids and names.
fn workspace_entries(settings: &slipstream_config::Settings) -> Vec<(u32, String)> {
    settings
        .workspaces
        .entries()
        .into_iter()
        .map(|entry| (entry.id, entry.name))
        .collect()
}

/// What Slipstream keeps about each connected client.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// Whether the client may see the capture globals and the toplevel list, worked out once
    /// when it connects (`capture::client_may_capture`): the screen-sharing portal, and nothing
    /// else.
    pub may_capture: bool,
    /// Whether the client may see the data-control globals, worked out once when it connects
    /// (`clipboard::client_may_control`): any client proven to be outside a sandbox.
    pub may_control_clipboard: bool,
    /// Whether the client may be an input method or a virtual keyboard (`ime::client_may_type`):
    /// likewise, any client proven to be outside a sandbox.
    pub may_type: bool,
    /// Whether the client is proven to be outside a sandbox, which it has to be to draw layer
    /// surfaces or set gamma ramps.
    pub unsandboxed: bool,
    /// What to call this client on a card that asks about it, and the path an answer is
    /// remembered against. `None` for a sandboxed client, one `/proc` couldn't be read for, or
    /// one whose executable anyone could rewrite — none of which can be asked about, because
    /// there is nothing to name and nothing safe to remember.
    pub asks_as: Option<(String, std::path::PathBuf)>,
}

impl ClientState {
    /// A new connection's state. One look at the connecting process, through `/proc`, decides
    /// everything it may reach.
    pub fn for_connection(stream: &UnixStream) -> Self {
        let peer = capture::unsandboxed_peer(stream);
        let asks_as = peer
            .as_ref()
            .and_then(|peer| Some((peer.name()?, peer.settled_exe()?.to_path_buf())));
        Self {
            asks_as,
            may_capture: capture::client_may_capture(peer.as_ref()),
            may_control_clipboard: clipboard::client_may_control(peer.as_ref()),
            may_type: crate::ime::client_may_type(peer.as_ref()),
            unsandboxed: peer.is_some(),
            ..Self::default()
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

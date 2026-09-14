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
    card, clipboard, debug,
    exit::{self, Act, Exit, Intent},
    explorer::{Explorer, Item, Outcome},
    focus::KeyboardFocus,
    gravity::{Rung, Step},
    idle::Idle,
    inhibit::LidInhibitor,
    keys::{self, Mods},
    launch,
    layout::{self, Direction, Rect},
    motion::{self, Motion},
    offer::{self, Offer},
    osd,
    rain::Rain,
    render::Chrome,
    restore,
    saver::Saver,
    screen::Screens,
    session,
    share::{self, Picker},
    status,
    tilt::Tilt,
    toast::Toast,
    udev::UdevData,
    workspace::{Workspace, Workspaces},
};

/// How long the recorded layout waits for the desktop entries to be read before giving up on
/// this login. The scan takes a fraction of a second; this is only a backstop.
const CATALOGUE_WAIT: f64 = 10.0;

/// How still the desktop has to be before the layout is written down again. Opening a window
/// retiles several times over as the client catches up, and none of those are worth a file.
const RECORD_SETTLE: f64 = 2.0;

pub struct Slipstream {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, Slipstream>,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub viewporter_state: ViewporterState,
    /// Portal dialogs parented to the app that opened them.
    pub xdg_foreign_state: XdgForeignState,
    /// Apps asking for one of their windows to be brought forward, with proof of the click or key
    /// that asked for it (`handlers/mod.rs`).
    pub xdg_activation_state: XdgActivationState,
    pub seat_state: SeatState<Slipstream>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    /// Clipboard control for `wl-copy`, `wl-paste` and clipboard managers, in both protocols'
    /// forms, filtered down to the clients `clipboard::may_control` allows.
    pub ext_data_control: ext_data_control::DataControlState,
    pub wlr_data_control: wlr_data_control::DataControlState,
    pub xwayland_shell_state: XWaylandShellState,
    /// Set up by the hardware backend once it knows the GPU's buffer formats.
    pub dmabuf: Option<(DmabufState, DmabufGlobal)>,
    /// Screen sharing: the capture protocols the portal talks to, and the list of windows it
    /// picks from. Every global is filtered down to the clients `capture::may_capture` allows, so
    /// no app but the portal can see them at all.
    pub image_capture_source: ImageCaptureSourceState,
    pub output_capture_source: OutputCaptureSourceState,
    pub toplevel_capture_source: ToplevelCaptureSourceState,
    pub image_copy_capture: ImageCopyCaptureState,
    pub foreign_toplevel_list: ForeignToplevelListState,
    /// The card asking what to share, while an app waits for the answer.
    pub share: Option<Picker>,
    /// What is being captured, and what is waiting for the next frame of it.
    pub captures: Captures,
    pub popups: PopupManager,

    pub seat: Seat<Self>,
    pub cursor_status: CursorImageStatus,

    // Slipstream State
    pub bindings: Vec<keys::Binding>,
    /// Keys whose press ran a binding; their releases are swallowed too.
    pub suppressed_keys: Vec<Keysym>,
    /// Super on its own, on its way to being a tap that opens the explorer.
    pub super_tap: keys::SuperTap,
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
    /// The window filling the screen at its own request (F11, video, games).
    pub fullscreen: Option<Window>,
    /// `shot:` debug steps waiting for the next frame.
    pub screenshots: Vec<String>,
    /// Print and Super+Shift+S, waiting for their screen's next frame (`screenshot.rs`).
    pub screenshot_requests: Vec<crate::screenshot::Request>,
    /// Where finished screenshots come back to the event loop from the thread that wrote them.
    pub screenshot_answers: channel::Sender<crate::screenshot::Saved>,
    /// The screen a screenshot was just taken of, and when, for its flash.
    pub flash: Option<(String, f64)>,
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
    /// The app explorer (Super+Space).
    pub explorer: Explorer,
    /// Quick settings (Super+A).
    pub quick: crate::quick::QuickSettings,
    /// The user's name, as quick settings shows it.
    pub user: String,
    /// The notification centre (Super+N).
    pub centre: crate::centre::Centre,
    /// The shortcut sheet (Super+/).
    pub sheet: crate::sheet::Sheet,
    /// Notifications apps have sent, and their pop-ups.
    pub notices: crate::notices::Notices,
    /// Messages near the top of the screen.
    pub toast: Toast,
    /// The volume and brightness display, low on the screen.
    pub osd: crate::osd::Osd,
    /// The way out, while it's on screen: Super+Shift+Esc, or Log out, Restart or Shut down.
    pub exit: Option<Exit<Window>>,
    /// What was open when the way out's card went up, ready to be written down if the switch on
    /// it is on. Taken then rather than at the end, because by the end the windows have gone.
    exit_record: Option<session::Record>,
    /// When the layout is next written down: `RECORD_SETTLE` after the last thing that moved, on
    /// wall time. `None` while nothing has changed since the last write.
    record_due: Option<f64>,
    /// What the file on disk holds, so a desktop that hasn't moved isn't written over and over.
    saved_record: Option<session::Record>,
    /// Toplevels a client has created but not yet shown. They take no tile and no focus until
    /// their first buffer arrives, and plenty never arrive at all.
    pub unmapped: Vec<Window>,
    /// Windows that asked for fullscreen before they had anything to show — players and games
    /// usually do — and get it the moment they map.
    pub fullscreen_on_map: Vec<Window>,
    /// The recorded layout going back up, while the apps are still starting.
    pub restoring: Option<restore::Plan<Window>>,
    /// The card at login asking whether to put the last layout back.
    pub offer: Option<Offer>,
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
    /// The living wallpaper.
    /// The living wallpaper, one per screen: each keeps its own grid, sized to that screen, and
    /// its own turn through the variations.
    pub savers: HashMap<String, Saver>,
    /// What a new screen's wallpaper starts from.
    wallpaper: slipstream_config::Wallpaper,
    pub idle_inhibit_state: IdleInhibitManagerState,
    /// Virtual machines and remote desktops asking for every key (`takeback.rs`).
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    /// Which windows have been told they hold the pointer or the keys, and the one refused.
    pub takeback: crate::takeback::TakeBack,
    /// Bullet time while it's open (Super+Tab).
    pub bullet: Option<bullet::Mode<Window>>,
    /// How far out bullet time's overview is zoomed, 0 to 1, on wall time so it's never slowed.
    pub overview: Tween<1>,
    /// The last labels bullet time used, kept while the same windows are open.
    bullet_labels: Vec<(Target<Window>, String)>,
    /// Where windows and workspace frames were drawn in the overview, for clicks.
    pub overview_hits: Vec<(Window, Rectangle<f64, Logical>)>,
    /// Where notification pop-ups, the toast and the volume display were drawn last frame, from
    /// the space's origin: the pointer over them is over glass, not the window beneath.
    pub chrome_areas: Vec<Rectangle<f64, Logical>>,
    /// Whether the pointer was kept from windows when last looked, so a change is noticed.
    pub pointer_was_blocked: bool,
    /// Scroll short of a whole notch, over the explorer or the centre.
    pub wheel_rest: f64,
    /// Where what a player is doing after a media key comes back to.
    pub media_replies: Option<smithay::reexports::calloop::channel::Sender<crate::media::Playing>>,
    /// Windows too big for their tiles, and where they were last drawn scaled into them, for
    /// the pointer. Global logical coordinates.
    pub fitted: Vec<(Window, Rectangle<f64, Logical>)>,
    /// Closed windows fading where they were.
    pub ghosts: Vec<crate::ghost::Ghost>,
    /// What each window was last drawn from, for its fade when it closes.
    pub pictures: Vec<(Window, crate::ghost::Picture)>,
    /// Windows drawn in the last frame that aren't in the space (bullet time's other workspaces,
    /// a workspace sliding away), so they get frame callbacks and keep drawing.
    pub drawn_off_space: Vec<Window>,
    /// The minimum size each window had when it was last tiled, so a client changing its own
    /// retiles.
    tiled_mins: Vec<(Window, (i32, i32))>,
    pub frame_hits: Vec<(usize, Rectangle<f64, Logical>)>,
    /// How the overview was last tilted on screen, to map clicks back onto it.
    pub overview_tilt: Tilt,
    /// The desktop's own drawing, one set per screen: every buffer in it is sized to the screen
    /// it is for, so sharing one between two screens of different sizes would repaint it twice a
    /// frame and, in the wallpaper's case, restart the animation.
    pub chromes: HashMap<String, Chrome>,
    /// Clock and battery readings, refreshed off the main thread.
    pub status: Arc<Mutex<status::Reading>>,
    pub debug: debug::Script,
    /// The settings as applied: the file's, with any environment overrides.
    pub settings: slipstream_config::Settings,
    /// The focused window's ring colour, parsed from the settings once rather than every frame.
    /// The panels' keyboard ring takes it too: one colour for where the keys are aimed.
    pub ring_rgb: [f32; 3],
    /// Bullet time's rings and frames, from the settings in the same way.
    pub bullet_rgb: [f32; 3],
    /// Where logind's refusals, and a nested run's stand-in answers, come back to the event loop
    /// (`power.rs`).
    power_answers: channel::Sender<crate::power::Answer>,
    /// The lock screen, while it's up (`lock.rs`).
    pub lock: Option<crate::lock::Lock<Window>>,
    /// The lock going away, while it fades.
    pub unlocking: Option<crate::lock::Unlocking>,
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

        // Here we initialize implementations of some wayland protocols
        // Some of them require us to implement traits on the Slipstream state,
        // you can find those implementations in the `crate::handlers` module

        // Initialize protocols needed for displaying windows
        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();

        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        // Sharp text at 1.25× for apps that support it (Qt 6, GTK 4, Firefox).
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&dh);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        // Apps name the cursor they want from the theme (GTK 4, Qt 6, Chromium) instead of
        // drawing one of their own at the wrong size.
        CursorShapeManagerState::new::<Self>(&dh);
        // When each frame reached the screen, so video players and games pace themselves.
        PresentationState::new::<Self>(&dh, Clock::<Monotonic>::new().id() as u32);
        // One-colour buffers, which toolkits use for backgrounds and fades.
        SinglePixelBufferState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);

        // Clipboard, drag-and-drop, and middle-click paste.
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        // Both selections for clipboard tools and managers, without a window of their own, and
        // only for unsandboxed clients.
        let clipboard_globals = clipboard::state(&dh, &primary_selection_state);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");

        // The layout comes from XKB_DEFAULT_LAYOUT and friends, which the session script sets
        // from the system keyboard settings.
        seat.add_keyboard(Default::default(), 200, 25).unwrap();

        // Notify clients that we have a pointer (mouse)
        // Here we assume that there is always pointer plugged in
        seat.add_pointer();

        // A space represents a two-dimensional plane. Windows and Outputs can be mapped onto it.
        //
        // Windows get a position and stacking order through mapping.
        // Outputs become views of a part of the Space and can be rendered via Space::render_output.
        let space = Space::default();

        // Setup a wayland socket that will be used to accept clients
        let socket_name = Self::init_wayland_listener(display, event_loop);

        // Get the loop signal, used to stop the event loop
        let loop_signal = event_loop.get_signal();

        // Screen sharing: the portal's own globals, offered to the portal alone.
        let capture_globals = capture::state(&dh);

        // Video players ask for the screen to stay awake, which keeps the UI from fading.
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);

        // Games: relative motion for mouse look, and the pointer locked or confined to their
        // window. Touchpad swipes and pinches reach apps too (zooming a page or a picture).
        RelativePointerManagerState::new::<Self>(&dh);
        PointerConstraintsState::new::<Self>(&dh);
        PointerGesturesState::new::<Self>(&dh);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&dh);

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

        let settings = crate::settings::startup();
        let ring_rgb = settings.borders.selected_tile_rgb();
        let bullet_rgb = settings.borders.bullet_time_rgb();

        let reduced_motion = settings.motion.reduced;
        let mut state = Self {
            start_time,
            display_handle: dh,

            space,
            loop_signal,
            loop_handle: event_loop.handle(),
            socket_name,

            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            output_manager_state,
            fractional_scale_state,
            viewporter_state,
            xdg_foreign_state,
            xdg_activation_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            ext_data_control: clipboard_globals.ext,
            wlr_data_control: clipboard_globals.wlr,
            xwayland_shell_state,
            dmabuf: None,
            image_capture_source: capture_globals.source,
            output_capture_source: capture_globals.output_source,
            toplevel_capture_source: capture_globals.toplevel_source,
            image_copy_capture: capture_globals.copy,
            foreign_toplevel_list: capture_globals.toplevel_list,
            share: None,
            captures: Captures::default(),
            popups,
            seat,
            cursor_status: CursorImageStatus::default_named(),

            bindings: keys::checked(keys::defaults()),
            suppressed_keys: Vec::new(),
            super_tap: keys::SuperTap::default(),
            nested: false,
            owns_state: true,
            session: false,
            udev: None,
            xwm: None,
            pending_focus: None,
            focus_history: Vec::new(),
            switcher: None,
            fullscreen: None,
            screenshots: Vec::new(),
            screenshot_requests: Vec::new(),
            screenshot_answers,
            flash: None,
            desktop_return: None,
            drag: None,
            drag_retile_due: false,
            cursor_override: None,
            // The same gaps as the mockup: 16 px at the edges, 10 px between windows.
            workspaces: Workspaces::new(&workspace_entries(&settings), 16, 10),
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
            exit: None,
            exit_record: None,
            record_due: None,
            saved_record: None,
            unmapped: Vec::new(),
            fullscreen_on_map: Vec::new(),
            restoring: None,
            offer: None,
            restore_looked: false,
            tags: Vec::new(),
            rain: Rain::new(false, ring_rgb),
            idle: Idle::new(false, settings.wallpaper.fade_after_secs),
            concentration: Default::default(),
            savers: HashMap::new(),
            wallpaper: settings.wallpaper.clone(),
            idle_inhibit_state,
            keyboard_shortcuts_inhibit_state,
            takeback: Default::default(),
            bullet: None,
            overview: Tween::at_rest([0.0]),
            bullet_labels: Vec::new(),
            overview_hits: Vec::new(),
            chrome_areas: Vec::new(),
            pointer_was_blocked: false,
            wheel_rest: 0.0,
            media_replies: None,
            fitted: Vec::new(),
            ghosts: Vec::new(),
            pictures: Vec::new(),
            drawn_off_space: Vec::new(),
            tiled_mins: Vec::new(),
            frame_hits: Vec::new(),
            overview_tilt: Tilt::default(),
            chromes: HashMap::new(),
            status: status::start(),
            debug: debug::Script::default(),
            settings,
            ring_rgb,
            bullet_rgb,
            power_answers,
            lock: None,
            unlocking: None,
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
    }

    /// Every effect has a reduced-motion path: moves jump and effects become short fades. This
    /// is the one place the setting reaches the parts that animate, at startup and whenever the
    /// settings change, so none of them can be left out of step with the rest.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.clock.reduced_motion = on;
        self.motion.reduced_motion = on;
        self.explorer.reduced_motion = on;
        self.toast.reduced_motion = on;
        self.osd.reduced_motion = on;
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
        if let Some(share) = self.share.as_mut() {
            share.reduced_motion = on;
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
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                let data = ClientState::for_connection(&client_stream);
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(data))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
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
        let (window, local) = self.window_under(pos)?;
        window
            .surface_under(local, WindowSurfaceType::ALL)
            .map(|(s, p)| (KeyboardFocus::Wayland(s), pos - (local - p.to_f64())))
    }

    /// The window drawn at `pos`, topmost first, and the point in the window's own coordinates
    /// (its surface's, unscaled). A window scaled into its tile is hit where it is drawn, not
    /// across the neighbour its real size would overlap.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<f64, Logical>)> {
        self.space.elements().rev().find_map(|window| {
            if let Some((_, drawn)) = self.fitted.iter().find(|(w, _)| w == window) {
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

    /// This screen's set of drawing buffers, made the first time it is drawn.
    pub fn chrome_for(&mut self, screen: &str) -> &mut Chrome {
        self.chromes.entry(screen.to_string()).or_default()
    }

    /// This screen's living wallpaper, started the first time it is drawn.
    pub fn saver_for(&mut self, screen: &str) -> &mut Saver {
        let reduced = self.clock.reduced_motion;
        let wallpaper = &self.wallpaper;
        self.savers
            .entry(screen.to_string())
            .or_insert_with(|| Saver::new(reduced, wallpaper))
    }

    /// Forgets a screen's buffers when it goes out, so an unplugged monitor doesn't leave a
    /// screenful of pixmaps behind.
    pub fn forget_screen_drawing(&mut self, screen: &str) {
        self.chromes.remove(screen);
        self.savers.remove(screen);
    }

    /// The bar button at this point on the focused screen, which is where the panels that ask
    /// are drawn.
    pub fn focused_bar_target(&self, x: f64, y: f64) -> Option<bar::Target> {
        let name = self.screens.focused_output()?.name();
        self.chromes.get(&name)?.bar.target_at(x, y)
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
        Some(Rect {
            y: screen.y + bar::HEIGHT,
            w: (screen.w - reserve).max(1),
            h: (screen.h - bar::HEIGHT).max(1),
            ..screen
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

    /// A click on the bar.
    pub fn bar_clicked(&mut self, target: bar::Target) {
        match target {
            bar::Target::Workspace(index) => self.switch_workspace(index),
            bar::Target::Apps => self.toggle_explorer(),
            bar::Target::Tray => self.toggle_quick_settings(),
            bar::Target::Clock | bar::Target::Bell => self.toggle_notification_centre(),
            bar::Target::Overview => self.toggle_bullet_time(),
            bar::Target::Sharing => {
                self.stop_sharing();
                self.show_toast(
                    "Sharing stopped",
                    "Nothing on this desktop is being shared now.",
                );
            }
        }
    }

    /// Places the windows of the workspace each screen is showing, and takes every other
    /// workspace's windows off screen. The layout changes immediately; clients catch up when
    /// they handle the configure.
    pub fn retile(&mut self) {
        if self.screens.is_empty() {
            return;
        }
        let shown: Vec<(usize, usize)> = self
            .screens
            .iter()
            .enumerate()
            .map(|(index, screen)| (index, screen.workspace))
            .collect();
        // Every screen lays its own workspace out in its own area, so a window's tile depends on
        // which screen is looking at it.
        let mut rects: Vec<(Window, Rect)> = Vec::new();
        for (index, workspace) in &shown {
            let Some(area) = self.screen_area(*index) else {
                continue;
            };
            rects.extend(
                self.workspaces
                    .get_mut(*workspace)
                    .rects_within(area, &min_size),
            );
        }
        let offscreen: Vec<Window> = self
            .space
            .elements()
            // X11 menus and tooltips aren't tiled; they go when their app unmaps them.
            .filter(|w| !is_override_redirect(w) && !rects.iter().any(|(on, _)| on == *w))
            .cloned()
            .collect();
        for window in offscreen {
            // Hidden by a switch or a minimise, it's no longer the one being looked at: apps
            // notify again, and terminals stop showing a focused cursor.
            deactivate(&window);
            self.space.unmap_elem(&window);
        }
        // A fullscreen window on a workspace being shown covers that screen, above its
        // neighbours. Fullscreen covers the bar too.
        let fullscreen = self.fullscreen.clone().filter(|window| {
            self.workspaces
                .find(window)
                .is_some_and(|workspace| shown.iter().any(|(_, shown)| *shown == workspace))
        });
        let full_screen_rect = fullscreen
            .as_ref()
            .and_then(|window| self.workspaces.find(window))
            .and_then(|workspace| self.screen_rect_for_workspace(workspace));
        let now = self.clock.tick();
        self.tiled_mins = rects
            .iter()
            .map(|(window, _)| (window.clone(), min_size(window)))
            .collect();
        for (window, tile) in rects {
            let full = fullscreen.as_ref() == Some(&window);
            let r = match (full, full_screen_rect) {
                (true, Some(screen)) => screen,
                _ => tile,
            };
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
            // The space holds the real position for input; the drawing glides there, except
            // while a gap is dragged, when the tiles keep up with the pointer.
            self.motion.place(&window, r, now);
            if matches!(self.drag, Some(crate::grabs::Drag::Gap { .. })) {
                self.motion.jump(&window, r);
            }
            // Moving a window that's already mapped keeps its place in the stacking order;
            // mapping it again would put the tiles in tree order, and the focused window's menus
            // could end up under a neighbour.
            if self.space.element_location(&window).is_some() {
                self.space.relocate_element(&window, (r.x, r.y));
            } else {
                self.space.map_element(window, (r.x, r.y), false);
            }
        }
        // The focused window above its neighbours, so its popups are drawn over them and found
        // first by the pointer.
        if let Some(focused) = self.focused_window()
            && self.space.element_location(&focused).is_some()
        {
            self.space.raise_element(&focused, false);
        }
        // A maximised window is above its neighbours, so it's drawn over them and the pointer
        // finds it first; a fullscreen one is above that.
        for (_, workspace) in &shown {
            if let Some(window) = self.workspaces.get(*workspace).maximised.clone() {
                self.space.raise_element(&window, false);
            }
        }
        if let Some(window) = fullscreen {
            self.space.raise_element(&window, false);
        }
        // Workspaces no screen is showing follow too, so a change to a tiling area (the code
        // rain taking or giving back width) reaches their windows before bullet time shows them.
        for index in 0..self.workspaces.count() {
            self.place_workspace(index);
        }
        self.update_window_scales();
        // Every layout change of any kind ends here: opening, closing, moving, swapping, gravity,
        // the code rain and switching workspace all retile.
        self.note_layout_change();
    }

    /// Tells every window the scale of the screen it's mostly on, so it draws sharp there: a
    /// window moved to a 1× monitor stops drawing 1.25× buffers, and the panel's windows keep
    /// 1.25× even when the monitor lit first.
    fn update_window_scales(&self) {
        use smithay::wayland::{
            compositor::send_surface_state, fractional_scale::with_fractional_scale,
        };
        for window in self.space.elements() {
            let Some(output) = self.output_for_window(window) else {
                continue;
            };
            let scale = output.current_scale();
            let (fractional, integer) = (scale.fractional_scale(), scale.integer_scale());
            window.with_surfaces(|surface, states| {
                with_fractional_scale(states, |preferred| {
                    preferred.set_preferred_scale(fractional)
                });
                send_surface_state(surface, states, integer, smithay::utils::Transform::Normal);
            });
        }
    }

    /// The screen a window is mostly on, or the one under its corner. Worked out from where the
    /// space has it now rather than the space's list of outputs per window, which only catches up
    /// at the next refresh.
    pub fn output_for_window(&self, window: &Window) -> Option<Output> {
        let geometry = self.space.element_geometry(window)?;
        let overlap = |output: &Output| {
            self.space
                .output_geometry(output)
                .and_then(|screen| screen.intersection(geometry))
                .map_or(0, |overlap| overlap.size.w as i64 * overlap.size.h as i64)
        };
        self.space
            .outputs()
            .filter(|output| overlap(output) > 0)
            .max_by_key(|output| overlap(output))
            .or_else(|| {
                self.space.outputs().find(|output| {
                    self.space
                        .output_geometry(output)
                        .is_some_and(|screen| screen.contains(geometry.loc))
                })
            })
            .cloned()
    }

    /// Puts the screens side by side again after one comes or goes: laptop panels first at the
    /// left edge, then the others in the order they connected. The code rain stays on the panel,
    /// whichever order the screens lit up in, the lid opening included.
    fn arrange_screens(&mut self) {
        let lit: Vec<(String, i32)> = self
            .screen_order
            .iter()
            .filter_map(|name| {
                let output = self
                    .screens
                    .iter()
                    .find(|s| s.output.name() == *name)?
                    .output
                    .clone();
                let width = self.space.output_geometry(&output)?.size.w;
                Some((name.clone(), width))
            })
            .collect();
        let nested_panel = self.nested_panel_name.clone();
        let places = arrange(&lit, |name| {
            is_internal_panel(name) || nested_panel.as_deref() == Some(name)
        });
        let mut moved = false;
        for (name, x) in places {
            let Some(output) = self
                .screens
                .iter()
                .find(|screen| screen.output.name() == name)
                .map(|screen| screen.output.clone())
            else {
                continue;
            };
            let at = self.space.output_geometry(&output).map(|geo| geo.loc.x);
            if at != Some(x) {
                self.space.map_output(&output, (x, 0));
                output.change_current_state(None, None, None, Some((x, 0).into()));
                self.screens.add(output, x, self.workspaces.count());
                moved = true;
            }
        }
        if moved {
            tracing::info!(
                screens = ?self.screens.iter().map(|s| (s.output.name(), s.x)).collect::<Vec<_>>(),
                "screens arranged"
            );
            // The pointer stays on a screen that's there.
            let pos = self.pointer_location();
            self.pointer_moved_to(self.clamp_to_outputs(pos), InputTime::now());
        }
    }

    /// A toplevel that has just committed its first buffer: now it is a window, so it takes a
    /// tile and the keyboard. Clients create toplevels they never show, and one of those must
    /// cost nothing (`new_toplevel` in `handlers/xdg_shell.rs`).
    pub fn map_if_ready(&mut self, surface: &WlSurface) {
        let Some(at) = self
            .unmapped
            .iter()
            .position(|window| window.wl_surface().as_deref() == Some(surface))
        else {
            return;
        };
        let has_buffer =
            with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false);
        if !has_buffer {
            return;
        }
        let window = self.unmapped.remove(at);
        self.add_window(window.clone());
        // It asked to be fullscreen while it had nothing to show; now it has.
        if let Some(at) = self
            .fullscreen_on_map
            .iter()
            .position(|waiting| waiting == &window)
        {
            self.fullscreen_on_map.remove(at);
            self.set_fullscreen(&window, true);
        }
    }

    /// Answers a toplevel's commit with its initial configure, if it hasn't had one: the size and
    /// tiled edges of the tile it will be given, so its first buffer fits and it doesn't open at a
    /// size of its own and snap. A window still waiting to map is predicted as `add_window` will
    /// place it, beside the focused window; one already tiled (which unmapped itself and is coming
    /// back) gets its own tile.
    pub fn send_initial_configure(&mut self, surface: &WlSurface) {
        let Some(window) = self
            .unmapped
            .iter()
            .chain(self.space.elements())
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
        else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        if toplevel.is_initial_configure_sent() {
            return;
        }
        if let Some((r, full)) = self.expected_place(&window) {
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
        }
        toplevel.send_configure();
    }

    /// Where `window` will be, and whether fullscreen: its tile if it has one, or the tile it would
    /// get beside the focused window on the workspace on screen.
    fn expected_place(&self, window: &Window) -> Option<(Rect, bool)> {
        if self.fullscreen_on_map.contains(window) || self.fullscreen.as_ref() == Some(window) {
            let workspace = self
                .workspaces
                .find(window)
                .unwrap_or_else(|| self.active_workspace());
            return self.screen_rect_for_workspace(workspace).map(|r| (r, true));
        }
        let (workspace, area) = match self.workspaces.find(window) {
            Some(index) => (index, self.area_for_workspace(index)?),
            None => (self.active_workspace(), self.output_area()?),
        };
        let mut layout = self.workspaces.get(workspace).layout.clone();
        if self.workspaces.find(window).is_none() {
            layout.insert(window.clone(), self.focused_window().as_ref(), area);
        }
        layout
            .rects_within(area, &min_size)
            .into_iter()
            .find(|(tiled, _)| tiled == window)
            .map(|(_, r)| (r, false))
    }

    /// Tiles a new window beside the focused one, on the workspace on screen, and focuses it.
    pub fn add_window(&mut self, window: Window) {
        let area = self.output_area().unwrap_or(Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        });
        // A window a recorded place is still waiting for goes where it was instead.
        if self.claim_restored(&window, area) {
            return;
        }
        // A window you didn't ask for, or one that would take the keyboard while you type in
        // another app, pours into the code rain instead, and a toast says so.
        if !self.fullscreen_on_screen()
            && !(self.asked_for(&window) && self.may_take_keyboard(&window, None))
        {
            self.pour_new(window);
            return;
        }
        // Split the focused window, so new windows appear next to what you're working on.
        let beside = self.focused_window();
        let active = self.active_workspace();
        self.workspaces
            .insert(active, window.clone(), beside.as_ref(), area);
        // Over a fullscreen window on screen, a new window waits behind it without the keyboard,
        // so a game or a video isn't interrupted, unless it's that window's own dialog.
        let covering = self
            .fullscreen
            .clone()
            .filter(|full| *full != window && self.fullscreen_on_screen());
        match covering {
            Some(full) if !is_child_of(&window, &full) => {
                self.retile();
                self.next_in_line(&window);
                tracing::info!(
                    workspace = active + 1,
                    windows = self.current_workspace().layout.len(),
                    x11 = window.x11_surface().is_some(),
                    "new window tiled behind the fullscreen one"
                );
                return;
            }
            Some(full) => {
                self.set_fullscreen(&full, false);
                self.focus_window(&window);
            }
            None => {
                self.retile();
                if self.may_take_keyboard(&window, None) {
                    self.focus_window(&window);
                } else {
                    self.next_in_line(&window);
                }
            }
        }
        tracing::info!(
            workspace = active + 1,
            windows = self.current_workspace().layout.len(),
            x11 = window.x11_surface().is_some(),
            min = ?min_size(&window),
            focused = self.focused_window().as_ref() == Some(&window),
            "new window tiled"
        );
    }

    /// Next in line after the focused window, so Alt+Tab reaches it first.
    fn next_in_line(&mut self, window: &Window) {
        self.focus_history.retain(|w| w != window);
        let at = self.focus_history.len().min(1);
        self.focus_history.insert(at, window.clone());
    }

    /// Whether `window` may take the keyboard now. While you're typing, only the app you're typing
    /// in may move it: to a dialog of its own, a window of the same process, or one from a program
    /// it started, such as a command run in a terminal. `asked_by` is the client that asked for an
    /// activation, which counts when it is the app being typed in.
    fn may_take_keyboard(&self, window: &Window, asked_by: Option<&ClientId>) -> bool {
        if !self.concentration.typing(std::time::Instant::now()) {
            return true;
        }
        let Some(typed_in) = self.focused_window() else {
            return true;
        };
        if *window == typed_in || is_child_of(window, &typed_in) {
            return true;
        }
        let typed_client = typed_in
            .wl_surface()
            .and_then(|surface| surface.client())
            .map(|client| client.id());
        if asked_by.is_some() && asked_by == typed_client.as_ref() {
            return true;
        }
        match (self.window_pid(window), self.window_pid(&typed_in)) {
            (Some(pid), Some(typed_pid)) => crate::concentration::descends_from(pid, typed_pid),
            _ => false,
        }
    }

    /// An app asked for `surface`'s window to come forward (xdg-activation). It does when the
    /// request carries a click or key press made since the keyboard last moved, and the typing rule
    /// allows it. Otherwise the window only moves up to next in line for Alt+Tab.
    pub fn activation_requested(
        &mut self,
        data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
        surface: &WlSurface,
    ) {
        let Some(window) = self
            .all_open_windows()
            .into_iter()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
        else {
            return;
        };
        let app = logged_app(&window);
        if self.focused_window().as_ref() == Some(&window) {
            return;
        }
        if !self.activation_is_recent(data) {
            tracing::info!(app, "activation asked without a recent click or key");
            self.next_in_line(&window);
            return;
        }
        if !self.may_take_keyboard(&window, data.client_id.as_ref()) {
            tracing::info!(app, "activation waits: typing in another app");
            self.next_in_line(&window);
            return;
        }
        tracing::info!(app, "activated");
        self.activate_window(&window);
    }

    /// Whether you asked for `window`: a dialog of the window you're using, a window from its
    /// process or a program it started, or one that appeared soon after a click, a key press or a
    /// launch from the desktop.
    fn asked_for(&self, window: &Window) -> bool {
        // A game or a video asking to fill the screen from its first frame was started on purpose,
        // however long it took to load.
        if self.concentration.recently_asked(std::time::Instant::now())
            || self.fullscreen_on_map.contains(window)
        {
            return true;
        }
        let Some(focused) = self.focused_window() else {
            return false;
        };
        if is_child_of(window, &focused) {
            return true;
        }
        match (self.window_pid(window), self.window_pid(&focused)) {
            (Some(pid), Some(focused_pid)) => crate::concentration::descends_from(pid, focused_pid),
            _ => false,
        }
    }

    /// A new window nobody asked for goes straight into a stream of code rain, and a toast names
    /// it and the way to bring it in.
    fn pour_new(&mut self, window: Window) {
        let Some(scale) = self.output_scale() else {
            return;
        };
        let name = self.stream_name(&window);
        let icon_px = crate::rain::icon_px(scale);
        let icon = window_app_id(&window).and_then(|id| self.explorer.app_icon(&id, icon_px));
        let pid = self.window_pid(&window);
        let now = self.clock.tick();
        self.rain.add(window.clone(), name.clone(), icon, pid, now);
        self.retile();
        self.show_toast(
            &format!("{name} opened in the code rain"),
            "Super+Shift+M or a click on its stream brings it in.",
        );
        tracing::info!(
            window = logged_app(&window),
            "new window poured into code rain"
        );
    }

    /// A token's proof that someone asked: a click or key press on this seat since the keyboard
    /// last moved, within a few seconds. Tokens Slipstream hands out itself carry no serial and
    /// get longer, since an app can take a while to start.
    fn activation_is_recent(
        &self,
        data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
    ) -> bool {
        let age = data.timestamp.elapsed();
        match &data.serial {
            Some((serial, seat)) => {
                let keyboard = self.seat.get_keyboard().unwrap();
                Seat::from_resource(seat).as_ref() == Some(&self.seat)
                    && age < std::time::Duration::from_secs(15)
                    && keyboard
                        .last_enter()
                        .is_some_and(|entered| serial.is_no_older_than(&entered))
            }
            None => data.client_id.is_none() && age < std::time::Duration::from_secs(60),
        }
    }

    /// Brings a window forward wherever it is: out of the code rain, or on its own workspace.
    pub fn activate_window(&mut self, window: &Window) {
        if self.rain.contains(window) {
            self.restore(window);
            return;
        }

        if let Some(workspace) = self.workspaces.find(window) {
            self.go_to_workspace(workspace);
        }
        self.focus_window(window);
    }

    /// Takes a closed window out of its workspace, re-tiles, and moves focus on.
    pub fn remove_window(&mut self, window: &Window) {
        self.leave_a_ghost(window);
        if self.pending_focus.as_ref() == Some(window) {
            self.pending_focus = None;
        }
        if self.fullscreen.as_ref() == Some(window) {
            self.fullscreen = None;
        }
        self.focus_history.retain(|w| w != window);
        if let Some(plan) = self.restoring.as_mut() {
            plan.release(window);
        }
        if let Some(switcher) = self.switcher.as_mut()
            && !switcher.forget(window)
        {
            self.switcher = None;
        }
        let was_on = self.take_off_workspace(window);
        self.rain.remove(window);
        self.tags.retain(|(tagged, ..)| tagged != window);
        if let Some(mode) = self.bullet.as_mut() {
            if mode
                .selected
                .as_ref()
                .is_some_and(|target| target.window() == window)
            {
                mode.selected = None;
            }
            mode.labels.retain(|(target, _)| target.window() != window);
        }
        self.overview_hits.retain(|(hit, _)| hit != window);
        self.fitted.retain(|(hit, _)| hit != window);
        self.tiled_mins.retain(|(hit, _)| hit != window);
        self.motion.remove(window);
        self.space.unmap_elem(window);
        self.retile();
        // Whatever is left on the workspace takes focus, so closing a tile never leaves the
        // keyboard aimed at nothing — which would make the focus keys do nothing at all.
        if was_on == Some(self.active_workspace()) {
            self.restore_focus();
        }
        tracing::info!(
            windows = self.current_workspace().layout.len(),
            "window closed; re-tiled"
        );
    }

    /// The window for an X11 surface: tiled on some workspace, minimised into the code rain, or a
    /// menu on screen.
    pub fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.all_open_windows()
            .into_iter()
            .chain(self.space.elements().cloned())
            .find(|window| window.x11_surface() == Some(surface))
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.space
            .elements()
            .find(|window| focus.is_window(window))
            .cloned()
    }

    pub fn focus_window(&mut self, window: &Window) {
        // Nothing takes the keyboard while locked: a window that maps or asks gets it at unlock,
        // if the one that had it has gone.
        if self.lock.is_some() {
            tracing::debug!(window = logged_app(window), "no focus while locked");
            return;
        }
        let Some(target) = KeyboardFocus::for_window(window) else {
            return;
        };
        // Choosing another window on a fullscreen window's workspace ends the fullscreen first, so
        // the chosen window is never a sliver drawn over it or hidden behind it.
        if let Some(full) = self.fullscreen.clone()
            && full != *window
            && self.workspaces.find(&full).is_some()
            && self.workspaces.find(&full) == self.workspaces.find(window)
        {
            self.set_fullscreen(&full, false);
        }
        // A maximised window gives way the same way: the one chosen is underneath it.
        if self.workspaces.focused(window) {
            self.retile();
        }
        // Focusing a window on another screen moves the keyboard to that screen: whatever the
        // keys do next, they should do it where you are now looking.
        if let Some(index) = self
            .workspaces
            .find(window)
            .and_then(|workspace| self.screens.showing(workspace))
        {
            self.screens.focus(index);
        }
        // Alt+Tab's switcher keeps its own order, taken when it opened.
        self.remember_focus(window);
        // An X11 window's Wayland surface arrives just after it maps; it's focused again then.
        if window.x11_surface().is_some() && window.wl_surface().is_none() {
            self.pending_focus = Some(window.clone());
        }
        self.space.raise_element(window, true);
        if let (Some(xwm), Some(surface)) = (self.xwm.as_mut(), window.x11_surface()) {
            let _ = xwm.raise_window(surface);
        }
        let serial = SERIAL_COUNTER.next_serial();
        // In the session log, so a desktop that won't take the keyboard can be told apart from an
        // app that isn't listening: this says which app the keys are being sent to.
        tracing::info!(window = logged_app(window), "keyboard focus");
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Some(target), serial);
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }
        // Which window had focus is part of the record, and focus can move without a retile.
        self.note_layout_change();
    }

    pub fn clear_focus(&mut self) {
        if let Some(window) = self.focused_window() {
            deactivate(&window);
        }
        let serial = SERIAL_COUNTER.next_serial();
        tracing::info!("keyboard focus on nothing");
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Option::<KeyboardFocus>::None, serial);
    }

    /// Focuses the workspace's remembered window, or its newest, or nothing if it's empty. The
    /// remembered one is only taken if it is still here: it is set when a workspace is left, and
    /// the window can have closed or moved since.
    pub fn restore_focus(&mut self) {
        let ws = self.current_workspace();
        let here = ws.layout.windows();
        // A fullscreen window here keeps the keyboard: a window closing behind it doesn't end it.
        // Nor does it end a maximise.
        let fullscreen = self
            .fullscreen
            .clone()
            .filter(|window| here.contains(window));
        let next = fullscreen
            .or_else(|| ws.maximised.clone())
            .or_else(|| {
                ws.last_focus
                    .clone()
                    .filter(|window| window.alive() && here.contains(window))
            })
            .or_else(|| here.last().cloned());
        match next {
            Some(window) => self.focus_window(&window),
            None => self.clear_focus(),
        }
    }

    pub fn focus_direction(&mut self, direction: Direction) {
        // Nothing focused (a window closed while the pointer was elsewhere, say): the first focus
        // key picks up whatever is on the workspace instead of doing nothing.
        if self.focused_window().is_none() {
            self.restore_focus();
            return;
        }
        let (Some(area), Some(current)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        let active = self.screens.workspace();
        // Neighbours by their own tiles, even under a maximised window.
        let rects = self
            .workspaces
            .get_mut(active)
            .tiles_within(area, &min_size);
        if let Some(next) = layout::neighbour(&rects, &current, direction) {
            self.focus_window(&next);
        }
    }

    /// A left click at a point on screen, in logical pixels, as the mouse would make it:
    /// motion, press, release. Debug scripts use it to reach what only the pointer can, such as
    /// an app's own menus.
    fn debug_click(&mut self, x: f64, y: f64) {
        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
        let at = Point::<f64, Logical>::from((origin.0 as f64 + x, origin.1 as f64 + y));
        let time = InputTime::now();
        // Focus follows a click on a window, as the real button handler does. The bar and the
        // panels have their own debug steps, so this doesn't repeat their click handling.
        if let Some((window, _)) = self.window_under(at) {
            self.focus_window(&window);
        }
        let pointer = self.seat.get_pointer().unwrap();
        let under = self.surface_under(at);
        tracing::debug!(?at, on_a_surface = under.is_some(), "debug click");
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: at,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pointer.button(
                self,
                &ButtonEvent {
                    button: 0x110,
                    state,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    /// A press, a drag and a release, for the `drag:` debug step: selecting text in a terminal,
    /// which is one of the things clients do most and the one most likely to make them ask for a
    /// size of their own. In steps, since a client that follows the pointer only sees where it
    /// has been told it went.
    fn debug_drag(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        const STEPS: usize = 8;
        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
        let at = |x: f64, y: f64| {
            Point::<f64, Logical>::from((origin.0 as f64 + x, origin.1 as f64 + y))
        };
        let time = InputTime::now();
        if let Some((window, _)) = self.window_under(at(x1, y1)) {
            self.focus_window(&window);
        }
        let move_to = |state: &mut Self, x: f64, y: f64| {
            let location = at(x, y);
            let under = state.surface_under(location);
            let pointer = state.seat.get_pointer().unwrap();
            pointer.motion(
                state,
                under,
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(state);
        };
        let click = |state: &mut Self, button_state: ButtonState| {
            let pointer = state.seat.get_pointer().unwrap();
            pointer.button(
                state,
                &ButtonEvent {
                    button: 0x110,
                    state: button_state,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(state);
        };
        move_to(self, x1, y1);
        click(self, ButtonState::Pressed);
        for step in 1..=STEPS {
            let t = step as f64 / STEPS as f64;
            move_to(self, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
        }
        click(self, ButtonState::Released);
        tracing::debug!(from = ?(x1, y1), to = ?(x2, y2), "debug drag");
    }

    /// Super+Alt+arrow: swaps the focused window with the one in that direction, so a tile can
    /// be moved about the workspace without the mouse. The tiling keeps its shape; the two
    /// windows change places inside it, and focus rides along with the window that moved.
    pub fn move_tile(&mut self, direction: Direction) {
        let (Some(area), Some(current)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        // Gravity places windows by weight, so swapping two of them would be undone by the next
        // retile. Point at the keys that do move a window there.
        if self.current_workspace().gravity.is_on() {
            self.show_toast(
                "Gravity on",
                "Super+PgUp and Super+PgDn move windows here. Super+T goes back to tiling.",
            );
            return;
        }
        let active = self.screens.workspace();
        // Neighbours by their own tiles, even under a maximised window.
        let rects = self
            .workspaces
            .get_mut(active)
            .tiles_within(area, &min_size);
        let Some(next) = layout::neighbour(&rects, &current, direction) else {
            return;
        };
        if self.current_workspace_mut().layout.swap(&current, &next) {
            self.retile();
            tracing::info!(?direction, "moved window within the workspace");
        }
    }

    /// Super+[ ] and Super+Shift+[ ]: the split beside the focused tile moves a step, on the
    /// press, and the tiles glide to their new sizes.
    pub fn resize_focused(&mut self, how: layout::Resize) {
        use layout::{Resize, Resized};
        let (Some(area), Some(window)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        let ws = self.current_workspace();
        // A window filling the screen or the tiling area has no neighbour to give room to.
        if self.fullscreen.as_ref() == Some(&window) || ws.maximised.as_ref() == Some(&window) {
            return;
        }
        if ws.gravity.is_on() {
            self.show_toast("Gravity arranges these", "Super+T goes back to tiling.");
            return;
        }
        let resized = self
            .current_workspace_mut()
            .layout
            .resize(&window, how, area, &min_size);
        match resized {
            Resized::Changed(ratio) => {
                tracing::info!(?how, ratio, "resized the tile");
                self.retile();
            }
            Resized::AtLimit => {
                let (title, body) = match how {
                    Resize::Wider => ("As wide as it goes", "Super+[ makes it narrower."),
                    Resize::Narrower => ("As narrow as it goes", "Super+] makes it wider."),
                    Resize::Taller => ("As tall as it goes", "Super+Shift+[ makes it shorter."),
                    Resize::Shorter => ("As short as it goes", "Super+Shift+] makes it taller."),
                };
                self.show_toast(title, body);
            }
            Resized::NothingBeside => self.show_toast(
                "Nothing beside it",
                "Open another window to share the space",
            ),
        }
    }

    /// Puts workspace `index` (0-based) on the focused screen, remembering focus on the one
    /// being left. If another screen is showing it the two trade workspaces, so a workspace is
    /// never on two screens at once and no window has to move.
    pub fn switch_workspace(&mut self, index: usize) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        let leaving = self.focused_window();
        if leaving.is_some() {
            let active = self.active_workspace();
            self.workspaces.get_mut(active).last_focus = leaving;
        }
        let here = self.focused_screen_name();
        let show = self.screens.show(index, self.workspaces.count());
        if !show.changed {
            return;
        }
        // The camera runs on wall time: moving about is never slowed, even in bullet time.
        let wall = self.wall();
        self.motion.slide_to(&here, index, wall);
        if let Some(other) = show.swapped {
            let name = self
                .screens
                .get(other)
                .map(|screen| screen.output.name())
                .unwrap_or_default();
            self.motion.slide_to(&name, show.from, wall);
        }
        self.retile();
        self.restore_focus();
        tracing::info!(
            workspace = index + 1,
            windows = self.workspaces.get(index).layout.len(),
            traded = show.swapped.is_some(),
            "switched workspace"
        );
    }

    /// Goes to workspace `index` because something on it is wanted — Alt+Tab, the explorer, a
    /// click on a stream in the code rain. If a screen is already showing it the keyboard moves
    /// there instead of dragging the workspace over to this one.
    /// Super+D: the lowest-numbered empty workspace no screen is showing, remembering where it
    /// came from. Pressed again while still on that workspace, and it's still empty, it goes back.
    pub fn show_desktop(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let active = self.active_workspace();
        if let Some((from, to)) = self.desktop_return.take()
            && to == active
            && self.workspaces.get(active).layout.is_empty()
        {
            tracing::info!(workspace = from + 1, "back from the empty workspace");
            self.go_to_workspace(from);
            return;
        }
        let empty: Vec<bool> = (0..self.workspaces.count())
            .map(|index| self.workspaces.get(index).layout.is_empty())
            .collect();
        let shown: Vec<usize> = self.screens.iter().map(|screen| screen.workspace).collect();
        match crate::workspace::lowest_empty(&empty, &shown) {
            Some(to) => {
                tracing::info!(workspace = to + 1, "to an empty workspace");
                self.desktop_return = Some((active, to));
                self.switch_workspace(to);
            }
            None => self.show_toast("No empty workspace", "Settings › Workspaces adds one"),
        }
    }

    pub fn go_to_workspace(&mut self, index: usize) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        if let Some(other) = self.screens.showing(index) {
            self.focus_screen_at(other);
            return;
        }
        self.switch_workspace(index);
    }

    /// The workspace a number key names, 1-based. A number past the last workspace says how many
    /// there are rather than doing nothing or landing somewhere unasked.
    pub fn workspace_for_key(&mut self, number: u8) -> Option<usize> {
        let count = self.workspaces.count();
        let index = (number as usize).checked_sub(1)?;
        if index < count {
            return Some(index);
        }
        let plural = if count == 1 {
            "workspace"
        } else {
            "workspaces"
        };
        self.show_toast(
            &format!("No workspace {number}"),
            &format!("There are {count} {plural}. Settings → Workspaces adds more."),
        );
        None
    }

    pub fn switch_workspace_by(&mut self, delta: i32) {
        let target =
            (self.active_workspace() as i32 + delta).clamp(0, self.workspaces.count() as i32 - 1);
        self.switch_workspace(target as usize);
    }

    /// Points the keyboard at another screen: Super+P, a click on it, or the screen it was on
    /// going out. The workspaces stay where they are.
    pub fn focus_screen_at(&mut self, index: usize) {
        if !self.screens.focus(index) {
            return;
        }
        self.restore_focus();
        if let Some(screen) = self.screens.get(index) {
            tracing::info!(
                screen = screen.output.name(),
                workspace = screen.workspace + 1,
                "the keyboard moved to another screen"
            );
        }
    }

    /// A screen lit up: it joins the row, left to right, and shows a workspace no other screen
    /// has. Called by both backends when an output is mapped.
    pub fn screen_connected(&mut self, output: &Output, x: i32) {
        // A screen never shares a workspace, so there is always one more than the lit screens.
        if self.screens.index_of(output).is_none()
            && self.workspaces.count() < self.screens.len() + 1
        {
            let entries = workspace_entries(&self.settings);
            self.set_workspaces(&entries, self.screens.len() + 1);
        }
        let index = self.screens.add(output.clone(), x, self.workspaces.count());
        if !self.screen_order.contains(&output.name()) {
            self.screen_order.push(output.name());
        }
        self.arrange_screens();
        let index = self.screens.index_of(output).unwrap_or(index);
        let workspace = self
            .screens
            .get(index)
            .map(|screen| screen.workspace)
            .unwrap_or(0);
        // A screen arrives already looking at its workspace; there is nothing to slide from.
        self.motion.jump_camera(&output.name(), workspace);
        self.hold_the_lid();
        self.retile();
        if self.focused_window().is_none() {
            self.restore_focus();
        }
        tracing::info!(
            screen = output.name(),
            workspace = workspace + 1,
            screens = self.screens.len(),
            "a screen joined the desktop"
        );
    }

    /// A screen went out: unplugged, or the lid shut on it.
    ///
    /// Nothing moves between workspaces. If the keyboard was on that screen, whichever screen
    /// takes over shows what the lost one was showing, so shutting the lid carries on what you
    /// were doing rather than leaving it behind on a panel nobody can see.
    pub fn screen_disconnected(&mut self, output: &Output) {
        let was_focused = self.screens.focused_output().as_ref() == Some(output);
        let Some(lost) = self.screens.remove(output) else {
            return;
        };
        self.screen_order.retain(|name| *name != output.name());
        self.arrange_screens();
        self.motion.forget_camera(&output.name());
        self.forget_screen_drawing(&output.name());
        let mut carried = false;
        if was_focused && !self.screens.is_empty() {
            let here = self.focused_screen_name();
            carried = self.screens.show(lost, self.workspaces.count()).changed;
            if carried {
                self.motion.jump_camera(&here, lost);
            }
        }
        self.hold_the_lid();
        self.retile();
        self.restore_focus();
        tracing::info!(
            screen = output.name(),
            workspace = lost + 1,
            screens = self.screens.len(),
            carried,
            "a screen left the desktop"
        );
        if carried {
            let name = self.focused_screen_name();
            let now = self.clock.tick();
            self.toast.show(
                "One screen left",
                &format!("Workspace {} carried on to {name}.", lost + 1),
                now,
            );
        }
    }

    /// A made-up screen for headless checks: a real `Output` with a mode and a place in the
    /// space, mapped to the right of the others, but no backend drawing it. Everything that
    /// decides where windows go — areas, workspaces, focus, the lid — works on it exactly as it
    /// does on a screen with a picture, which is what these checks are for.
    pub fn add_made_up_screen(&mut self, w: i32, h: i32) {
        let name = format!("HEADLESS-{}", self.screens.len() + 1);
        let output = Output::new(
            name.clone(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "Slipstream".into(),
                model: "Made up".into(),
                serial_number: "Unknown".into(),
            },
        );
        let mode = smithay::output::Mode {
            size: (w, h).into(),
            refresh: 60_000,
        };
        let x = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .map(|geo| geo.loc.x + geo.size.w)
            .max()
            .unwrap_or(0);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(smithay::utils::Transform::Normal),
            None,
            Some((x, 0).into()),
        );
        output.create_global::<Slipstream>(&self.display_handle);
        self.space.map_output(&output, (x, 0));
        self.screen_connected(&output, x);
        tracing::info!(
            screen = name,
            w,
            h,
            x,
            "a made-up screen for a headless check"
        );
    }

    /// Takes a made-up screen away, the way unplugging one does.
    pub fn drop_made_up_screen(&mut self, name: &str) {
        let Some(output) = self
            .space
            .outputs()
            .find(|output| output.name() == name)
            .cloned()
        else {
            tracing::warn!(screen = name, "no screen by that name");
            return;
        };
        self.space.unmap_output(&output);
        self.screen_disconnected(&output);
    }

    /// Writes the screens into the log: what each is showing, where it is, and where windows
    /// tile on it. Checks read this back.
    pub fn log_screens(&self) {
        for (index, screen) in self.screens.iter().enumerate() {
            let area = self.screen_area(index);
            tracing::info!(
                screen = screen.output.name(),
                workspace = screen.workspace + 1,
                focused = index == self.screens.focused_index(),
                rect = ?self.screen_rect(index),
                area = ?area,
                windows = self.workspaces.get(screen.workspace).layout.len(),
                "screen"
            );
        }
    }

    pub fn log_windows(&self) {
        for window in self.space.elements() {
            let told = window.toplevel().map(|toplevel| {
                (
                    toplevel.with_pending_state(|state| state.size),
                    toplevel.with_committed_state(|state| state.and_then(|state| state.size)),
                )
            });
            tracing::info!(
                window = logged_app(window),
                placed = ?self.space.element_location(window),
                own = ?window.geometry(),
                told = ?told.map(|(pending, _)| pending),
                acked = ?told.map(|(_, acked)| acked),
                "window"
            );
        }
    }

    /// logind's lid switch is ours while there is a screen that isn't the laptop's own panel.
    /// Nested, the window isn't a screen and the lid belongs to the desktop around it.
    fn hold_the_lid(&mut self) {
        if self.nested {
            return;
        }
        let external = self
            .screens
            .iter()
            .any(|screen| !is_internal_panel(&screen.output.name()));
        self.lid_inhibitor.set(external);
    }

    /// Super+P: the keyboard moves to the next screen along, wrapping round at the end. With one
    /// screen it says so rather than doing nothing silently.
    pub fn focus_next_screen(&mut self) {
        if self.screens.len() < 2 {
            let now = self.clock.tick();
            self.toast
                .show("One screen", "Nothing to move the keyboard to.", now);
            return;
        }
        let next = (self.screens.focused_index() + 1) % self.screens.len();
        self.focus_screen_at(next);
    }

    /// Super+Shift+P: the focused window moves to the next screen — that is, on to the workspace
    /// that screen is showing — and the keyboard follows it, as Win+Shift+arrow does on Windows.
    pub fn move_focused_to_next_screen(&mut self) {
        if self.screens.len() < 2 {
            let now = self.clock.tick();
            self.toast
                .show("One screen", "Nothing to move the window to.", now);
            return;
        }
        let Some(window) = self.focused_window() else {
            return;
        };
        let next = (self.screens.focused_index() + 1) % self.screens.len();
        let Some(target) = self.screens.get(next).map(|screen| screen.workspace) else {
            return;
        };
        self.move_window_to_workspace(&window, target, false);
        self.focus_screen_at(next);
        self.focus_window(&window);
    }

    /// Moves the focused window to workspace `index` (0-based) and follows it there, as moving a
    /// window to another desktop does on Windows.
    pub fn move_focused_to_workspace(&mut self, index: usize) {
        if let Some(window) = self.focused_window() {
            self.move_window_to_workspace(&window, index, true);
        }
    }

    /// Moves `window` to workspace `index` (0-based). With `follow`, the view goes with it and it
    /// keeps focus; otherwise (bullet time sending it) the workspace on screen stays put.
    pub fn move_window_to_workspace(&mut self, window: &Window, index: usize, follow: bool) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        // It tiles into the area of the screen showing where it's going, which is not this one
        // when the window is being sent to another screen.
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let Some(from) = self.workspaces.find(window) else {
            return;
        };
        if from == index {
            return;
        }
        self.take_off_workspace(window);
        let beside = self.workspaces.get(index).last_focus.clone();
        self.workspaces
            .insert(index, window.clone(), beside.as_ref(), area);
        // The window rides across to its new workspace, then settles into its tile.
        let now = self.clock.tick();
        let step = (area.w + motion::WORKSPACE_GAP) as f64;
        self.motion
            .carry(window, (from as f64 - index as f64) * step, now);
        if follow {
            let here = self.focused_screen_name();
            let show = self.screens.show(index, self.workspaces.count());
            let wall = self.wall();
            if show.changed {
                self.motion.slide_to(&here, index, wall);
                if let Some(other) = show.swapped {
                    let name = self
                        .screens
                        .get(other)
                        .map(|screen| screen.output.name())
                        .unwrap_or_default();
                    self.motion.slide_to(&name, show.from, wall);
                }
            }
            self.retile();
            self.focus_window(window);
        } else {
            self.retile();
            if self.focused_window().is_none() {
                self.restore_focus();
            }
        }
        tracing::info!(workspace = index + 1, follow, "moved window to workspace");
    }

    /// Wall-clock seconds since start, for animations that must never be slowed.
    pub fn wall(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Where the windows of workspace `index` go, gravity included: in the area of the screen
    /// showing it, or of the focused screen when none is.
    fn workspace_rects(&mut self, index: usize) -> Vec<(Window, Rect)> {
        let Some(area) = self.area_for_workspace(index) else {
            return Vec::new();
        };
        self.workspaces.get_mut(index).rects_within(area, &min_size)
    }

    /// Sends the windows of a workspace that isn't on screen towards their tiles, so bullet time's
    /// overview shows the change. They're told their new sizes too: otherwise a window keeps
    /// drawing at the size it had before, say, the code rain took width, and spills out of its
    /// workspace's frame. (The workspace on screen is placed by `retile`.)
    fn place_workspace(&mut self, index: usize) {
        if self.screens.showing(index).is_some() {
            return;
        }
        let screen = self.screen_rect_for_workspace(index);
        let now = self.clock.tick();
        for (window, tile) in self.workspace_rects(index) {
            let full = self.fullscreen.as_ref() == Some(&window);
            let r = match (full, screen) {
                (true, Some(screen)) => screen,
                _ => tile,
            };
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
            self.motion.place(&window, r, now);
        }
    }

    pub fn move_focused_by(&mut self, delta: i32) {
        let target =
            (self.active_workspace() as i32 + delta).clamp(0, self.workspaces.count() as i32 - 1);
        self.move_focused_to_workspace(target as usize);
    }

    fn remember_focus(&mut self, window: &Window) {
        self.focus_history.retain(|w| w != window && w.alive());
        self.focus_history.insert(0, window.clone());
    }

    /// Alt+Tab or Alt+Shift+Tab: the switcher opens over every open window, most recently used
    /// first, or its selection moves one along. Nothing else changes until Alt is let go.
    pub fn cycle_windows(&mut self, forward: bool) {
        if self.lock.is_some() {
            return;
        }
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.step(forward);
            return;
        }
        let open = self.all_open_windows();
        self.focus_history.retain(|w| w.alive());
        // Every open window, including any never focused (one that opened behind a fullscreen
        // game, say), after the ones that have been.
        let mut windows: Vec<Window> = self
            .focus_history
            .iter()
            .filter(|window| open.contains(window))
            .cloned()
            .collect();
        windows.extend(
            open.into_iter()
                .filter(|window| window.alive() && !self.focus_history.contains(window)),
        );
        let now = self.wall();
        self.switcher =
            crate::switcher::Switcher::start(windows, forward, now, self.clock.reduced_motion);
    }

    /// Ends the way out while it is still asking, as Esc on its card does.
    pub fn cancel_exit(&mut self) {
        tracing::info!("the way out was cancelled");
        self.exit = None;
        self.exit_record = None;
    }

    /// Alt let go after Alt+Tab, or a tile clicked: the switcher's choice is committed. A
    /// minimised window comes back out of the code rain; any other is gone to, with one slide to
    /// its workspace, and focused.
    pub fn finish_cycle(&mut self) {
        self.end_cycle(crate::switcher::End::Release);
    }

    /// Esc with Alt held, or the screen locking: the switcher goes and nothing changes.
    pub fn cancel_cycle(&mut self) {
        self.end_cycle(crate::switcher::End::Escape);
    }

    fn end_cycle(&mut self, end: crate::switcher::End) {
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        if self.lock.is_some() {
            return;
        }
        let rain = &self.rain;
        match switcher.finish(end, |window| rain.contains(window)) {
            None => tracing::info!("switcher cancelled"),
            Some(crate::switcher::Commit::Restore(window)) => self.restore(&window),
            Some(crate::switcher::Commit::Focus(window)) => {
                if !window.alive() {
                    return;
                }
                if let Some(workspace) = self.workspaces.find(&window) {
                    self.go_to_workspace(workspace);
                }
                self.focus_window(&window);
            }
        }
    }

    /// A window on the workspace on screen fills the screen, so the bar is hidden.
    /// A fullscreen window on the workspace `index`, so the bar and the wallpaper give way to it.
    pub fn fullscreen_on(&self, index: usize) -> bool {
        self.fullscreen
            .as_ref()
            .is_some_and(|window| self.workspaces.find(window) == Some(index))
    }

    /// A new screen's wallpaper, at the settings the others are running.
    pub fn new_saver(&self) -> Saver {
        Saver::new(self.clock.reduced_motion, &self.wallpaper)
    }

    pub fn fullscreen_on_screen(&self) -> bool {
        self.fullscreen
            .as_ref()
            .is_some_and(|window| self.workspaces.find(window) == Some(self.active_workspace()))
    }

    /// An app asked to fill the screen, or to stop.
    pub fn set_fullscreen(&mut self, window: &Window, fullscreen: bool) {
        if fullscreen {
            self.fullscreen = Some(window.clone());
        } else if self.fullscreen.as_ref() == Some(window) {
            self.fullscreen = None;
        } else {
            return;
        }
        if let Some(surface) = window.x11_surface() {
            let _ = surface.set_fullscreen(fullscreen);
        }
        self.retile();
        tracing::info!(fullscreen, "window fullscreen changed");
    }

    /// Asks the focused window to close, as its own close button would.
    pub fn close_focused(&mut self) {
        if let Some(window) = self.focused_window() {
            self.close_window(&window);
        }
    }

    /// Asks `window` to close. It goes when its app closes it, after any "save changes?" prompt.
    pub fn close_window(&mut self, window: &Window) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        } else if let Some(surface) = window.x11_surface() {
            let _ = surface.close();
        }
    }

    /// Every window a person would call open: tiled on any workspace, or minimised into the code
    /// rain. Not popups, not override-redirect X11 surfaces, not Slipstream's own overlays. The
    /// one definition of what's open, used by the way out to list, to ask, and to wait.
    pub fn mapped_windows(&self) -> Vec<exit::Open<Window>> {
        self.workspaces
            .all_windows()
            .into_iter()
            .map(|window| (window, false))
            .chain(
                self.rain
                    .streams
                    .iter()
                    .map(|stream| (stream.window.clone(), true)),
            )
            .filter(|(window, _)| window.alive())
            .map(|(window, minimised)| exit::Open {
                app: self.window_name(&window),
                title: window_title(&window),
                minimised,
                window,
            })
            .collect()
    }

    /// Super+Shift+Esc, or Log out, Restart or Shut down: the way out asks first, then gives the
    /// apps a moment to close themselves. Only the watchdog's emergency quit skips it.
    pub fn begin_exit(&mut self, intent: Intent) {
        if self.exit.is_some() {
            return;
        }
        self.close_panels();
        let open = self.mapped_windows();
        tracing::info!(intent = intent.verb(), windows = open.len(), "the way out");
        let wall = self.wall();
        self.exit_record = Some(self.take_record());
        self.exit = Some(Exit::new(
            intent,
            open,
            wall,
            self.clock.reduced_motion,
            self.settings.session.remember,
        ));
    }

    /// Something moved. The layout goes to disk once the desktop has been still for
    /// `RECORD_SETTLE`, so every retile can say so without costing anything.
    fn note_layout_change(&mut self) {
        if self.settings.session.remember {
            self.record_due = Some(self.wall() + RECORD_SETTLE);
        }
    }

    /// A pass of the event loop: the layout is written down once the desktop has settled, so the
    /// watchdog's emergency exit, a wedged GPU or a power cut still leave something to come
    /// back to, which is when a person most wants their desktop back. Wall time, so bullet time
    /// can't hold the save off.
    pub fn tick_record(&mut self) {
        let Some(due) = self.record_due else {
            return;
        };
        // The record belongs to the compositor that owns the state folder.
        if !self.owns_state {
            self.record_due = None;
            return;
        }
        if self.wall() < due {
            return;
        }
        // Until the offer has been answered and the windows have landed, what's on screen is a
        // layout half-built from the file itself, and no record of what the person was doing.
        if !self.restore_looked || self.offer.is_some() || self.restoring.is_some() {
            return;
        }
        // The way out has its own record, taken whole when its card went up. Saving again while
        // the apps close one by one would write a half-empty desktop over a good one.
        if self.exit.is_some() {
            return;
        }
        self.record_due = None;
        if !self.settings.session.remember {
            return;
        }
        let record = self.take_record();
        // An empty desktop is never written over a real record: it says nothing worth reopening,
        // and it is what "start clean" leaves on screen until the first window opens.
        if record.windows.is_empty() {
            return;
        }
        if self
            .saved_record
            .as_ref()
            .is_some_and(|saved| saved.same_desktop(&record))
        {
            return;
        }
        let path = session::path();
        match session::write(&path, &record) {
            Ok(()) => {
                tracing::debug!(windows = record.windows.len(), "layout written down");
                self.saved_record = Some(record);
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't write the layout down: {err}");
            }
        }
    }

    /// The desktop as it stands: which app is on which workspace, where it sits in the tiling, what
    /// gravity was doing with it, and what is in the code rain. Apps and places only — no window
    /// titles are ever written down.
    fn take_record(&self) -> session::Record {
        let mut windows = Vec::new();
        let mut workspaces = Vec::new();
        let focused = self.focused_window();
        for index in 0..self.workspaces.count() {
            let workspace = self.workspaces.get(index);
            let here = workspace.layout.windows();
            let slot_of = |window: &Window| here.iter().position(|w| w == window);
            for (slot, window) in here.iter().enumerate() {
                let Some(app) = window_app_id(window) else {
                    continue;
                };
                windows.push(session::Win {
                    app,
                    // Counting from 1 in the file; the name goes with the tree below.
                    workspace: Some(index + 1),
                    slot,
                    focused: focused.as_ref() == Some(window),
                    in_rain: false,
                    gravity: workspace
                        .gravity
                        .is_on()
                        .then(|| workspace.gravity.rung(window).label().to_string()),
                    pinned: workspace.gravity.is_pinned(window),
                });
            }
            if let Some(shape) = workspace.layout.shape() {
                // Slot numbers, so the tree means something without the windows themselves.
                let shape = shape.map(&slot_of);
                if let Some(shape) = shape {
                    workspaces.push(session::Tiling {
                        index: index + 1,
                        name: workspace.name.clone(),
                        tiles: session::encode(&shape),
                    });
                }
            }
        }
        for (slot, stream) in self.rain.streams.iter().enumerate() {
            if let Some(app) = window_app_id(&stream.window) {
                windows.push(session::Win {
                    app,
                    workspace: None,
                    slot,
                    in_rain: true,
                    ..session::Win::default()
                });
            }
        }
        session::Record::new(self.active_workspace() + 1, windows, workspaces)
    }

    /// Looks for a recorded layout once the desktop entries have been read, and offers it — or
    /// puts it straight back, with `reopen-without-asking` on. Runs on passes of the event loop
    /// until it can answer, since the apps are read on another thread a moment after startup.
    fn look_for_a_recorded_layout(&mut self) {
        // Nothing was recorded, so there is nothing to put back and nothing sitting on disk. And a
        // compositor that doesn't own the state folder leaves its record to the one that does.
        if !self.settings.session.remember || !self.owns_state {
            self.restore_looked = true;
            return;
        }
        // Without the entries every app in the record would look lost. Give them a moment; if
        // they never arrive, say so rather than throwing the layout away silently.
        if !self.explorer.apps_read() {
            if self.wall() > CATALOGUE_WAIT {
                self.restore_looked = true;
                tracing::warn!("the apps weren't read in time; the recorded layout is left alone");
            }
            return;
        }
        self.restore_looked = true;
        let path = session::path();
        let record = match session::read(&path) {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't read the layout: {err}");
                return;
            }
        };
        let plan = self.plan_from(&record);
        if plan.is_empty() {
            tracing::info!(
                lost = plan.lost().len(),
                "nothing in the record can be reopened"
            );
            return;
        }
        tracing::info!(
            windows = plan.len(),
            lost = plan.lost().len(),
            saved_at = record.saved_at,
            "a recorded layout is waiting"
        );
        if self.settings.session.reopen_without_asking {
            self.start_restore(plan);
            return;
        }
        let wall = self.wall();
        self.offer = Some(Offer::new(
            plan.len(),
            plan.app_names(),
            plan.lost().to_vec(),
            wall,
            self.clock.reduced_motion,
        ));
        // Held until it's answered, so the plan's expiry starts when the apps are launched.
        self.restoring = Some(plan);
    }

    /// A record as a plan, with each app matched to its desktop entry. An app with no entry is
    /// left out and named on the card; its id is never run as a command, because any client can
    /// set any `app_id`.
    fn plan_from(&self, record: &session::Record) -> restore::Plan<Window> {
        let wall = self.wall();
        let count = self.workspaces.count();
        restore::Plan::new(
            record,
            wall,
            |id| {
                self.explorer
                    .app_for(id)
                    .filter(|app| !app.exec.is_empty())
                    .map(|app| (app.name, app.exec, app.terminal))
            },
            count,
            |index| {
                let name = record
                    .workspaces
                    .iter()
                    .find(|tiling| tiling.index == index)
                    .map(|tiling| tiling.name.as_str())
                    .unwrap_or("");
                restore::workspace_now(name, index, count, |name| self.workspaces.named(name))
            },
        )
    }

    /// Enter on the offer card, or `reopen-without-asking`: launch every recorded app and let the
    /// windows claim their places as they arrive.
    fn start_restore(&mut self, mut plan: restore::Plan<Window>) {
        let wall = self.wall();
        plan.restart(wall);
        let launching = plan.to_launch();
        tracing::info!(apps = launching.len(), "putting the layout back");
        for (exec, in_terminal) in launching {
            // A failure is in the log; the card already named anything it can't bring back.
            let _ = launch::app(&exec, in_terminal);
        }
        self.restoring = Some(plan);
    }

    /// A new window that a recorded place was waiting for: it goes where it was recorded rather
    /// than beside whatever has focus. Returns whether it claimed one, so `add_window` knows to
    /// leave it alone.
    fn claim_restored(&mut self, window: &Window, area: Rect) -> bool {
        let Some(app) = window_app_id(window) else {
            return false;
        };
        let wall = self.wall();
        let Some(mut plan) = self.restoring.take() else {
            return false;
        };
        let claim = plan.claim(&app, window, wall);
        self.restoring = Some(plan);
        let Some(claim) = claim else {
            return false;
        };
        // A window recorded in the code rain is tiled first and poured into its stream at the end,
        // when the streams can be put back in the order they were in.
        let index = claim.workspace.unwrap_or_else(|| self.active_workspace());
        if self.workspaces.find(window) != Some(index) {
            self.take_off_workspace(window);
            self.workspaces.insert(index, window.clone(), None, area);
        }
        if claim.workspace.is_some() {
            self.rebuild_workspace(index, area);
        }
        self.retile();
        // Nothing takes focus while the desktop is filling up: the recorded window gets it back
        // once everything has landed, and a window opening meanwhile shouldn't steal it.
        if self.focused_window().is_none() {
            self.restore_focus();
        }
        tracing::debug!(
            app,
            workspace = index + 1,
            "a window claimed its recorded place"
        );
        true
    }

    /// A Wayland window's `app_id` arrives on its first commit, after `new_toplevel` has already
    /// tiled it, so a layout coming back looks again here: the window moves to where it was
    /// recorded the moment it says which app it is. (An X11 window has its class from the start
    /// and claims its place in `add_window`.)
    pub fn claim_on_commit(&mut self, surface: &WlSurface) {
        let wall = self.wall();
        if self
            .restoring
            .as_ref()
            .is_none_or(|plan| plan.settled(wall))
        {
            return;
        }
        let Some(window) = self
            .space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
        else {
            return;
        };
        // Already placed, or not tiled anywhere yet: nothing to do either way.
        if self
            .restoring
            .as_ref()
            .is_some_and(|plan| plan.holds(&window))
            || self.workspaces.find(&window).is_none()
        {
            return;
        }
        let Some(area) = self.output_area() else {
            return;
        };
        self.claim_restored(&window, area);
    }

    /// Puts workspace `index`'s recorded tree back around the windows that have claimed its slots.
    /// Anything on the workspace the record didn't know about is tiled in again afterwards, so a
    /// window opened by hand mid-restore isn't thrown away.
    fn rebuild_workspace(&mut self, index: usize, area: Rect) {
        let Some(shape) = self.restoring.as_ref().and_then(|plan| plan.tree(index)) else {
            return;
        };
        let recorded = shape.leaves();
        let workspace = self.workspaces.get_mut(index);
        let extras: Vec<Window> = workspace
            .layout
            .windows()
            .into_iter()
            .filter(|window| !recorded.contains(window))
            .collect();
        workspace.layout.rebuild(&shape);
        for extra in extras {
            workspace.layout.insert(extra, None, area);
        }
    }

    /// A pass of the event loop while a layout is going back up: the offer is looked for once, and
    /// the restore finishes when every recorded window has come back or the wait is over.
    pub fn tick_restore(&mut self) {
        if !self.restore_looked {
            self.look_for_a_recorded_layout();
        }
        // While the offer is still up nothing has been launched, so nothing can have settled.
        if self.offer.is_some() {
            return;
        }
        let wall = self.wall();
        if self
            .restoring
            .as_ref()
            .is_some_and(|plan| plan.settled(wall))
        {
            self.finish_restore();
        }
    }

    /// The last of it, once the windows have landed or the wait is up: gravity, the code rain,
    /// the workspace that was in front, and finally focus.
    fn finish_restore(&mut self) {
        let Some(plan) = self.restoring.take() else {
            return;
        };
        for index in 0..self.workspaces.count() {
            let gravity = plan.gravity(index);
            if gravity.is_on() {
                self.workspaces.get_mut(index).gravity = gravity;
            }
        }
        // Quietly: a toast per stream would bury the desktop you just asked for.
        for window in plan.to_minimise() {
            self.minimise(&window);
        }
        let active = plan.active();
        if active != self.active_workspace() {
            let here = self.focused_screen_name();
            self.screens.show(active, self.workspaces.count());
            // The layout coming back is where the session left off, not a move to watch: the
            // view is put there rather than slid there.
            self.motion.jump_camera(&here, active);
        }
        self.retile();
        match plan.focus() {
            Some(window) if self.workspaces.find(&window) == Some(active) => {
                self.focus_window(&window)
            }
            _ => self.restore_focus(),
        }
        let missing = plan.len() - plan.claimed();
        tracing::info!(
            windows = plan.claimed(),
            missing,
            workspace = active + 1,
            "the layout is back"
        );
        if missing > 0 {
            let plural = if missing == 1 { "window" } else { "windows" };
            self.show_toast(
                &format!("{missing} {plural} didn’t come back"),
                "Their apps were asked to start but no window arrived. Everything else is where \
                 it was.",
            );
        }
    }

    /// Esc on the offer card. The record is left where it is: it's replaced at the next logout,
    /// and deleting it would turn "not this time" into "never".
    pub fn offer_key(&mut self, sym: Keysym) {
        let Some(offer) = self.offer.as_mut() else {
            return;
        };
        let act = offer.key(sym);
        self.act_on_offer(act);
    }

    pub fn offer_click(&mut self, x: f64, y: f64) {
        let Some(offer) = self.offer.as_mut() else {
            return;
        };
        let act = offer.click(x, y);
        self.act_on_offer(act);
    }

    /// The pointer moved while the offer is up: whatever it's over lights up.
    pub fn offer_hover(&mut self, pos: Point<f64, Logical>) {
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(offer) = self.offer.as_mut() {
            offer.hover(pos.x, pos.y);
        }
    }

    fn act_on_offer(&mut self, act: offer::Act) {
        match act {
            offer::Act::Nothing => {}
            offer::Act::Reopen => {
                self.offer = None;
                if let Some(plan) = self.restoring.take() {
                    self.start_restore(plan);
                }
            }
            offer::Act::Clean => {
                self.offer = None;
                self.restoring = None;
                tracing::info!("starting clean; the recorded layout is left alone");
            }
        }
    }

    /// Makes the workspaces the settings' list (`Workspaces::reconcile`): windows stay with their
    /// workspace through renames and reordering, a deleted workspace's windows join the one before
    /// it, and every screen follows the workspace it was showing.
    fn set_workspaces(&mut self, entries: &[(u32, String)], min: usize) {
        if self.bullet.is_some() {
            // Its frames, labels and pans are all by index; coming back out is simpler than
            // re-aiming every one of them.
            self.bullet_back();
        }
        let area = self.output_area().unwrap_or(Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        });
        let before = self.workspaces.count();
        let map = self.workspaces.reconcile(entries, min, area);
        let count = self.workspaces.count();
        self.screens.remap(&map, count);
        for screen in self.screens.iter() {
            self.motion
                .jump_camera(&screen.output.name(), screen.workspace);
        }
        self.retile();
        self.restore_focus();
        tracing::info!(before, count, "the workspace list changed");
    }

    /// A chooser has connected: the portal wants to know what to share. The card goes up with
    /// the portal's own list, in words a person can choose from.
    pub fn share_asked(&mut self, stream: std::os::unix::net::UnixStream) {
        share::read_request(&self.loop_handle, stream, |state, request| match request {
            Ok((list, reply)) => state.open_share_picker(&list, Some(reply)),
            Err(why) => tracing::warn!("the share picker's request was refused: {why}"),
        });
    }

    /// Opens the picker on the portal's list. `reply` is where the answer goes; a debug step has
    /// none.
    pub fn open_share_picker(&mut self, list: &str, reply: Option<std::os::unix::net::UnixStream>) {
        // Nothing can be chosen behind the lock, and a picker left waiting would take the first
        // Enter after unlocking. Dropping `reply` answers the portal with nothing: declined.
        if self.lock.is_some() {
            tracing::info!("sharing declined: the screen is locked");
            return;
        }
        if self.share.is_some() {
            // One question at a time. Dropping the stream answers the second asker with nothing,
            // which its portal reads as declined.
            tracing::info!("a second app asked to share while the picker was up; declined");
            return;
        }
        let choices = share::parse(list);
        if choices.is_empty() {
            tracing::info!("asked to share, with nothing to choose from");
            return;
        }
        let rows = choices
            .iter()
            .map(|choice| self.share_row(choice))
            .collect();
        tracing::info!(choices = choices.len(), "an app asked to share the screen");
        let now = self.start_time.elapsed().as_secs_f64();
        self.explorer.close();
        self.quick.close();
        self.centre.close();
        self.share = Some(Picker::new(
            choices,
            rows,
            reply,
            now,
            self.clock.reduced_motion,
        ));
        self.wake_ui();
    }

    /// What a row of the picker says about a source.
    fn share_row(&self, choice: &share::Choice) -> card::Row {
        match &choice.source {
            share::Source::Screen { name } => {
                let output = self.space.outputs().find(|output| output.name() == *name);
                let size = output
                    .and_then(|output| output.current_mode())
                    .map(|mode| format!("{}×{}", mode.size.w, mode.size.h));
                let count = self.space.outputs().count();
                card::Row {
                    name: if count > 1 {
                        format!("Screen {name}")
                    } else {
                        "The whole screen".to_string()
                    },
                    title: String::new(),
                    note: size.map(|size| (size, card::DIM)),
                }
            }
            share::Source::Window { identifier } => match self.captures.window_for(identifier) {
                Some(window) => card::Row {
                    name: self.window_name(&window),
                    title: window_title(&window),
                    note: self
                        .rain
                        .streams
                        .iter()
                        .any(|stream| stream.window == window)
                        .then(|| ("minimised".to_string(), card::DIM)),
                },
                None => card::Row {
                    name: "A window".to_string(),
                    title: String::new(),
                    note: None,
                },
            },
        }
    }

    pub fn share_key(&mut self, sym: Keysym, shift: bool) {
        let Some(picker) = self.share.as_mut() else {
            return;
        };
        let act = picker.key(sym, shift);
        self.act_on_share(act);
    }

    pub fn share_click(&mut self, x: f64, y: f64) {
        let Some(picker) = self.share.as_mut() else {
            return;
        };
        let act = picker.click(x, y);
        self.act_on_share(act);
    }

    pub fn share_hover(&mut self, pos: Point<f64, Logical>) {
        if self.lock.is_some() {
            return;
        }
        let Some(screen) = self.focused_screen_geometry() else {
            return;
        };
        let pos = pos - screen.loc.to_f64();
        if let Some(picker) = self.share.as_mut() {
            picker.hover(pos.x, pos.y);
        }
    }

    fn act_on_share(&mut self, act: share::Act) {
        let share = match act {
            share::Act::Nothing => return,
            share::Act::Share => true,
            share::Act::Cancel => false,
        };
        if let Some(picker) = self.share.take() {
            match picker.chosen() {
                Some(choice) if share => tracing::info!(source = ?choice.source, "sharing"),
                _ => tracing::info!("sharing declined"),
            }
            picker.answer(share);
        }
    }

    /// Writes the record down, or deletes it, as the switch on the card said. Called once, at the
    /// point of no return.
    fn keep_record(&mut self, remember: bool) {
        if !self.owns_state {
            return;
        }
        let path = session::path();
        let done = match (remember, self.exit_record.take()) {
            (true, Some(record)) => {
                let windows = record.windows.len();
                session::write(&path, &record).inspect(|()| {
                    tracing::info!(windows, path = %path.display(), "layout written down");
                })
            }
            // Off means no record sitting in the state directory at all, not a stale one.
            _ => session::forget(&path),
        };
        if let Err(err) = done {
            tracing::warn!(path = %path.display(), "couldn't keep the layout: {err}");
        }
    }

    /// A pass of the event loop while the way out is on screen: what's still open, and whether a
    /// timer has run out. Wall time, so bullet time can't stretch a logout.
    pub fn tick_exit(&mut self) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let open = self.mapped_windows();
        let act = exit.tick(self.wall(), &open);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }

    /// A key while the way out is taking every one of them.
    pub fn exit_key(&mut self, sym: Keysym) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let wall = self.wall();
        let act = exit.key(sym, wall);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }

    fn act_on_exit(&mut self, act: Act<Window>) {
        match act {
            Act::Nothing => {}
            Act::Close(windows) => {
                for window in windows {
                    self.close_window(&window);
                }
            }
            // Nothing was lost: the desktop is as it was, minus whatever already closed.
            Act::Cancel => {
                self.exit = None;
                self.exit_record = None;
            }
            // The switch on the card is the setting, so it is still on the next time.
            Act::Remember(on) => {
                self.change_settings(move |settings| settings.session.remember = on)
            }
            Act::Go(intent) => {
                let remember = self.exit.as_ref().is_some_and(Exit::remember);
                self.keep_record(remember);
                match intent {
                    Intent::LogOut => {
                        tracing::info!("logging out");
                        self.loop_signal.stop();
                    }
                    // These end the session the same way, having asked every app first.
                    Intent::Restart | Intent::ShutDown => {
                        if let Some(power) = crate::power::Power::for_intent(intent) {
                            self.request_power(power);
                        }
                    }
                }
            }
        }
    }

    /// Asks logind for Sleep, Restart or Shut down, on a thread of its own. The answer comes back
    /// through `power_answers` to `power_answered`.
    pub fn request_power(&mut self, power: crate::power::Power) {
        let answers = self.power_answers.clone();
        crate::power::request(
            power,
            crate::power::Answerer::current(),
            crate::power::run,
            move |answer| {
                let _ = answers.send(answer);
            },
        );
    }

    fn power_answered(&mut self, answer: crate::power::Answer) {
        match answer {
            crate::power::Answer::Refused(refusal) => self.power_refused(refusal),
            // Nested, no logind ends the session, so it ends here, as Log out does. The record was
            // already kept on the way out.
            crate::power::Answer::StoodIn(power) if power.ends_the_session() => {
                tracing::info!(?power, "nested: stopping instead");
                self.loop_signal.stop();
            }
            crate::power::Answer::StoodIn(_) => {}
        }
    }

    /// logind said no. Restart and Shut down have closed every app and faded the screen to black
    /// by now: the way out is cleared, which lifts the black, and a toast says what was refused.
    fn power_refused(&mut self, refusal: crate::power::Refused) {
        if refusal.power != crate::power::Power::Sleep {
            self.exit = None;
            self.exit_record = None;
        }
        let (title, body) = refusal.message();
        self.wake_ui();
        self.show_toast(&title, &body);
    }

    /// The pointer moved while the way out is up: whatever it's over lights up. `pos` is in the
    /// whole desktop's coordinates, as the pointer keeps them; the card knows its own screen's.
    pub fn exit_hover(&mut self, pos: Point<f64, Logical>) {
        if self.exit.is_none() {
            return;
        }
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(exit) = self.exit.as_mut() {
            exit.hover(pos.x, pos.y);
        }
    }

    /// A click on the way out's card.
    pub fn exit_click(&mut self, x: f64, y: f64) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let wall = self.wall();
        let act = exit.click(x, y, wall);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }

    /// Super+PgUp and Super+PgDn: the focused window moves one rung heavier or lighter along
    /// gravity's ladder of layouts.
    pub fn weigh(&mut self, heavier: bool) {
        if let Some(window) = self.focused_window() {
            self.weigh_window(&window, heavier);
        }
    }

    pub fn weigh_window(&mut self, window: &Window, heavier: bool) {
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let history = self.focus_history.clone();
        let ws = self.workspaces.get_mut(index);
        let mut others: Vec<Window> = ws
            .layout
            .windows()
            .into_iter()
            .filter(|other| other != window)
            .collect();
        others.sort_by_key(|other| recency(&history, other));
        let from = ws.gravity.rung(window);
        let step = ws.gravity.step(window, heavier, &others);
        if matches!(step, Step::Moved(_)) {
            // Gravity takes over the sizes, so a maximise ends rather than coming back after it.
            ws.maximised = None;
        }
        self.report_step(window, from, step);
        self.retile();
    }

    /// Super+F: the focused tiled window fills the tiling area above its neighbours, or goes back
    /// to its tile.
    pub fn toggle_maximise(&mut self) {
        let Some(window) = self.focused_window() else {
            return;
        };
        let Some(index) = self.workspaces.find(&window) else {
            return;
        };
        let ws = self.workspaces.get_mut(index);
        // Gravity sizes windows by weight, which a maximise would fight.
        if ws.gravity.is_on() {
            self.show_toast("Gravity arranges these", "Super+T goes back to tiling.");
            return;
        }
        let maximised = ws.toggle_maximised(&window);
        self.retile();
        tracing::info!(window = logged_app(&window), maximised, "maximise");
    }

    /// Super+T: gravity on, with the focused window as the centre, or back to tiling.
    pub fn toggle_gravity(&mut self) {
        let gravity = &mut self.current_workspace_mut().gravity;
        if gravity.is_on() {
            gravity.off();
            self.show_toast(
                "Dwindle tiling",
                "Back to dwindle tiling. Weights are cleared.",
            );
            if let Some(window) = self.focused_window() {
                self.tag(window, Rung::Tiling);
            }
            self.retile();
        } else {
            self.weigh(true);
        }
    }

    /// Tags the window with its new rung, or says why nothing moved. Leaving tiling also says
    /// what happened, since a stray press rearranges every window on the workspace.
    fn report_step(&mut self, window: &Window, from: Rung, step: Step) {
        let name = self.window_name(window);
        match step {
            Step::Moved(rung) => {
                self.tag(window.clone(), rung);
                match (from, rung) {
                    (Rung::Tiling, Rung::Centre) => self.show_toast(
                        "Gravity on",
                        &format!("{name} is the centre. Super+PgDn steps back to tiling."),
                    ),
                    (Rung::Tiling, Rung::Grid) => self.show_toast(
                        "Grid",
                        "Every window the same size. Super+PgUp steps back to tiling.",
                    ),
                    _ => {}
                }
            }
            Step::Heaviest => self.show_toast(
                "As heavy as it goes",
                &format!("{name} is in the spotlight. Super+PgDn makes it lighter."),
            ),
            Step::Lightest => self.show_toast(
                "As light as it goes",
                &format!("Super+M sends {name} to the code rain. Super+PgUp makes it heavier."),
            ),
            Step::Alone => self.show_toast(
                "Nothing to arrange",
                &format!("{name} is the only window here."),
            ),
        }
    }

    /// Shows `window`'s new rung on it for a moment, and logs the change.
    fn tag(&mut self, window: Window, rung: Rung) {
        tracing::info!(window = logged_app(&window), rung = rung.label(), "gravity");
        let now = self.clock.tick();
        self.tags.retain(|(tagged, ..)| *tagged != window);
        self.tags.push((window, rung, now));
    }

    /// Takes `window` off its workspace (closed, minimised or moved) and returns which workspace
    /// it was on. A window that inherits gravity's centre gets the centre tag, so the amber ring
    /// moving to it has a visible cause.
    fn take_off_workspace(&mut self, window: &Window) -> Option<usize> {
        let history = &self.focus_history;
        let removed = self.workspaces.remove(window, |w| recency(history, w))?;
        if let Some(centre) = removed.new_centre {
            let rung = self.workspaces.get(removed.workspace).gravity.rung(&centre);
            self.tag(centre, rung);
        }
        Some(removed.workspace)
    }

    /// What to call a window in messages: its app's name, else its title.
    /// What a stream's header calls a window. The app's name, or failing that its app id — never
    /// its title: a title is a working directory, a document or a correspondent, and in a column
    /// 36 pixels wide it arrives sideways and cut in half anyway.
    pub fn stream_name(&self, window: &Window) -> String {
        window_app_id(window)
            .map(|id| {
                self.explorer.app_name(&id).unwrap_or_else(|| {
                    // `org.kde.kate` reads better as Kate than as its reverse-DNS name.
                    let leaf = id.rsplit('.').next().unwrap_or(&id).to_string();
                    let mut letters = leaf.chars();
                    match letters.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
                        None => leaf,
                    }
                })
            })
            .unwrap_or_else(|| "This window".to_string())
    }

    pub fn window_name(&self, window: &Window) -> String {
        window_app_id(window)
            .and_then(|id| self.explorer.app_name(&id))
            .or_else(|| Some(window_title(window)).filter(|title| !title.is_empty()))
            .unwrap_or_else(|| "This window".to_string())
    }

    /// Super+Tab: bullet time, or back out of it with nothing changed.
    pub fn toggle_bullet_time(&mut self) {
        if self.bullet.is_some() {
            self.bullet_back();
        } else {
            self.enter_bullet_time();
        }
    }

    fn overview_duration(&self) -> f64 {
        if self.clock.reduced_motion {
            0.0
        } else {
            motion::MOVE
        }
    }

    /// Windows of workspace `index`, most recently used first.
    fn windows_by_recency(&self, index: usize) -> Vec<Window> {
        let mut windows = self.workspaces.get(index).layout.windows();
        windows.sort_by_key(|window| recency(&self.focus_history, window));
        windows
    }

    fn bullet_targets(&self, home: usize) -> Vec<Target<Window>> {
        let mut others: Vec<Window> = (0..self.workspaces.count())
            .filter(|&index| index != home)
            .flat_map(|index| self.workspaces.get(index).layout.windows())
            .collect();
        others.sort_by_key(|window| recency(&self.focus_history, window));
        let streams = self
            .rain
            .streams
            .iter()
            .map(|stream| stream.window.clone())
            .collect();
        bullet::targets(self.windows_by_recency(home), streams, others)
    }

    pub fn enter_bullet_time(&mut self) {
        self.close_panels();
        // Workspaces off screen can hold sizes from before the code rain took or gave back space.
        for index in 0..self.workspaces.count() {
            self.place_workspace(index);
        }
        let home = self.active_workspace();
        let targets = self.bullet_targets(home);
        let labels = bullet::labels(&self.bullet_labels, &targets);
        self.bullet_labels = labels.clone();
        self.bullet = Some(bullet::Mode {
            home,
            view: home,
            selected: self.focused_window().map(Target::Window),
            labels,
            typed: String::new(),
            pointer: bullet::Pointer::default(),
        });
        self.clock.enter_bullet_time();
        if self.muffles_sound() {
            crate::sound::muffle(true);
        }
        let (wall, duration) = (self.wall(), self.overview_duration());
        self.overview.retarget([1.0], wall, duration, motion::HYPR);
        tracing::info!(targets = targets.len(), "bullet time");
    }

    /// Out of bullet time with nothing changed, if it's open.
    pub fn leave_bullet_time_now(&mut self) {
        if self.bullet.is_some() {
            self.bullet_back();
        }
    }

    fn leave_bullet_time(&mut self) {
        let Some(mode) = self.bullet.take() else {
            return;
        };
        if mode.pointer.drag.is_some() {
            self.set_cursor_override(None);
        }
        self.clock.leave_bullet_time();
        if self.muffles_sound() {
            crate::sound::muffle(false);
        }
        let (wall, duration) = (self.wall(), self.overview_duration());
        self.overview.retarget([0.0], wall, duration, motion::HYPR);
    }

    /// Whether bullet time muffles the sound. A nested run shares the login session's PipeWire, so
    /// muffling there would muffle the real desktop's sound around it: it's the session's to do,
    /// unless a test asks for it with `SLIPSTREAM_MUFFLE=1`.
    fn muffles_sound(&self) -> bool {
        self.session || std::env::var("SLIPSTREAM_MUFFLE").as_deref() == Ok("1")
    }

    /// Esc: back where bullet time started, with nothing changed.
    fn bullet_back(&mut self) {
        self.leave_bullet_time();
        let (wall, active) = (self.wall(), self.active_workspace());
        let here = self.focused_screen_name();
        self.motion.slide_to(&here, active, wall);
    }

    /// Enter, a click or a label: go to the chosen window, bring back the chosen stream, or land
    /// on the workspace being looked at. Focus moves at once; the zoom back in follows.
    fn bullet_go(&mut self, target: Option<Target<Window>>) {
        let Some(view) = self.bullet.as_ref().map(|mode| mode.view) else {
            return;
        };
        self.leave_bullet_time();
        match target {
            Some(target) if self.rain.contains(target.window()) => {
                self.switch_workspace(view);
                self.restore(target.window());
            }
            Some(target) => {
                let window = target.window().clone();
                if let Some(index) = self.workspaces.find(&window) {
                    self.go_to_workspace(index);
                }
                self.focus_window(&window);
            }
            None => self.switch_workspace(view),
        }
        let (wall, active) = (self.wall(), self.active_workspace());
        let here = self.focused_screen_name();
        self.motion.slide_to(&here, active, wall);
    }

    /// Looks at workspace `index`, choosing its most recent window when `choose` is set.
    fn bullet_pan(&mut self, index: usize, choose: bool) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        let wall = self.wall();
        let here = self.focused_screen_name();
        self.motion.slide_to(&here, index, wall);
        let most_recent = self.windows_by_recency(index).into_iter().next();
        if let Some(mode) = self.bullet.as_mut() {
            mode.view = index;
            if choose {
                mode.selected = most_recent.map(Target::Window);
            }
        }
    }

    /// Chooses `target`, looking at its workspace if it's on another.
    fn bullet_select(&mut self, target: Option<Target<Window>>) {
        let on = match &target {
            Some(Target::Window(window)) => self.workspaces.find(window),
            _ => None,
        };
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        mode.selected = target;
        let view = mode.view;
        if let Some(index) = on.filter(|&index| index != view) {
            self.bullet_pan(index, false);
        }
    }

    fn bullet_cycle(&mut self, forward: bool) {
        let Some(mode) = self.bullet.as_ref() else {
            return;
        };
        let targets = self.bullet_targets(mode.home);
        let next = bullet::cycle(&targets, mode.selected.as_ref(), forward);
        self.bullet_select(next);
    }

    /// Arrows choose the nearest window that way. Past the last one, ←/→ glide on to the
    /// neighbouring workspace.
    fn bullet_arrow(&mut self, direction: Direction) {
        let Some(mode) = self.bullet.clone() else {
            return;
        };
        let mut rects = self.workspace_rects(mode.view);
        // Most recently used first: with two windows equally ahead, the arrow picks the last used.
        rects.sort_by_key(|(window, _)| recency(&self.focus_history, window));
        let current = match &mode.selected {
            Some(Target::Window(window)) if rects.iter().any(|(tiled, _)| tiled == window) => {
                window.clone()
            }
            _ => {
                let most_recent = self.windows_by_recency(mode.view).into_iter().next();
                if most_recent.is_some() {
                    return self.bullet_select(most_recent.map(Target::Window));
                }
                return self.bullet_cross(mode.view, direction);
            }
        };
        match bullet::nearest(&rects, &current, direction) {
            Some(window) => self.bullet_select(Some(Target::Window(window))),
            None => self.bullet_cross(mode.view, direction),
        }
    }

    fn bullet_cross(&mut self, from: usize, direction: Direction) {
        let going_right = match direction {
            Direction::Right => true,
            Direction::Left => false,
            _ => return,
        };
        let Some(index) = (if going_right {
            Some(from + 1).filter(|&index| index < self.workspaces.count())
        } else {
            from.checked_sub(1)
        }) else {
            return;
        };
        self.bullet_pan(index, false);
        let mut rects = self.workspace_rects(index);
        rects.sort_by_key(|(window, _)| recency(&self.focus_history, window));
        let arriving = bullet::entering(&rects, going_right).map(Target::Window);
        if let Some(mode) = self.bullet.as_mut() {
            mode.selected = arriving;
        }
    }

    /// Shift+1–5 and Shift+←/→: sends the chosen window to another workspace, and looks there.
    fn bullet_send(&mut self, index: usize) {
        let Some(Some(Target::Window(window))) =
            self.bullet.as_ref().map(|mode| mode.selected.clone())
        else {
            return;
        };
        self.move_window_to_workspace(&window, index, false);
        self.bullet_pan(index.min(self.workspaces.count().saturating_sub(1)), false);
    }

    fn bullet_letter(&mut self, letter: char) {
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        mode.typed.push(letter);
        match bullet::resolve(&mode.labels, &mode.typed) {
            Typed::Exact(target) => {
                mode.typed.clear();
                self.bullet_go(Some(target));
            }
            Typed::Partial => {}
            Typed::NoMatch => mode.typed.clear(),
        }
    }

    /// A key while bullet time is open: the everyday keys, without Super.
    pub fn bullet_key(&mut self, key: Keysym, mods: Mods) {
        let Some(mode) = self.bullet.as_ref() else {
            return;
        };
        let (view, typing) = (mode.view, !mode.typed.is_empty());
        let chosen = match &mode.selected {
            Some(Target::Window(window)) => Some(window.clone()),
            _ => None,
        };
        let Some(command) = bullet::command(key, mods) else {
            return;
        };
        let count = self.workspaces.count();
        // Esc lets go of a window being dragged before anything else.
        if key == Keysym::Escape && self.cancel_bullet_drag() {
            return;
        }
        match command {
            // Esc clears a half-typed label first; Super+Tab always goes straight back.
            bullet::Command::Back if typing && key == Keysym::Escape => {
                if let Some(mode) = self.bullet.as_mut() {
                    mode.typed.clear();
                }
            }
            bullet::Command::Back => self.bullet_back(),
            bullet::Command::Go => {
                let target = self.bullet.as_ref().and_then(|mode| mode.selected.clone());
                self.bullet_go(target);
            }
            bullet::Command::Cycle { forward } => self.bullet_cycle(forward),
            bullet::Command::SendBeside(direction) => {
                let from = chosen
                    .as_ref()
                    .and_then(|window| self.workspaces.find(window))
                    .unwrap_or(view);
                let to = if direction == Direction::Left {
                    from.checked_sub(1)
                } else {
                    Some(from + 1).filter(|&to| to < count)
                };
                if let Some(to) = to {
                    self.bullet_send(to);
                }
            }
            bullet::Command::Choose(direction) => self.bullet_arrow(direction),
            bullet::Command::Weigh { heavier } => {
                if let Some(window) = chosen {
                    self.weigh_window(&window, heavier);
                    if self.rain.contains(&window) {
                        self.bullet_select(Some(Target::Stream(window)));
                    }
                }
            }
            bullet::Command::Close => {
                if let Some(window) = chosen {
                    self.close_window(&window);
                }
            }
            bullet::Command::Minimise => {
                if let Some(window) = chosen {
                    self.minimise(&window);
                    self.bullet_select(Some(Target::Stream(window)));
                }
            }
            bullet::Command::Send(index) if index < count => self.bullet_send(index),
            bullet::Command::Look(index) if index < count => self.bullet_pan(index, true),
            bullet::Command::Send(_) | bullet::Command::Look(_) => {}
            bullet::Command::Jump(letter) => self.bullet_letter(letter),
        }
    }

    /// The bar's title while bullet time is open: the chosen window's title (its app's name when
    /// it has none), or the workspace being looked at and how full it is.
    pub fn bullet_bar_title(&self) -> String {
        let Some(mode) = self.bullet.as_ref() else {
            return self.focused_title();
        };
        let chosen = mode.selected.as_ref().map(|target| {
            let window = target.window();
            let title = window_title(window);
            let name = if title.is_empty() {
                self.window_name(window)
            } else {
                title
            };
            (name, matches!(target, Target::Stream(_)))
        });
        let view = mode.view.min(self.workspaces.count().saturating_sub(1));
        bullet::bar_title(
            chosen
                .as_ref()
                .map(|(name, minimised)| (name.as_str(), *minimised)),
            &self.workspaces.label(view),
            view,
            self.workspaces.get(view).layout.len(),
        )
    }

    /// A click on the bar while bullet time is open. A workspace's number looks at it, as its
    /// number key does; a panel's button goes back out with nothing changed and opens the panel.
    fn bullet_bar_clicked(&mut self, target: bar::Target) {
        match bullet::bar_click(target) {
            bullet::BarClick::Look(index) => self.bullet_pan(index, true),
            bullet::BarClick::Leave => {
                self.bullet_back();
                self.bar_clicked(target);
            }
            bullet::BarClick::Stay => self.bar_clicked(target),
            bullet::BarClick::Back => self.bullet_back(),
        }
    }

    /// Where `pos` on screen is in the overview's flat coordinates, which its hit areas use.
    fn overview_point(&self, pos: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        let screen = self.output_rect()?;
        let local = (pos.x - screen.x as f64, pos.y - screen.y as f64);
        self.overview_tilt.unproject(local).map(Point::from)
    }

    /// The window drawn at the flat point `at` in the overview, and where it's drawn.
    fn overview_window_at(
        &self,
        at: Point<f64, Logical>,
    ) -> Option<(Window, Rectangle<f64, Logical>)> {
        self.overview_hits
            .iter()
            .find(|(_, area)| area.contains(at))
            .cloned()
    }

    /// The workspace whose frame is at the flat point `at` in the overview.
    fn overview_frame_at(&self, at: Point<f64, Logical>) -> Option<usize> {
        self.frame_hits
            .iter()
            .find(|(_, area)| area.contains(at))
            .map(|(index, _)| *index)
    }

    /// The pointer moved in bullet time: the window under it shows its close button, and a left
    /// press on a window that has moved far enough becomes a drag towards another workspace.
    pub fn bullet_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        if self.bullet.is_none() {
            return;
        }
        let flat = self.overview_point(pos);
        let hit = flat.and_then(|at| self.overview_window_at(at));
        let over = flat.and_then(|at| self.overview_frame_at(at));
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        let pointer = &mut mode.pointer;
        pointer.on_close = matches!((&hit, flat), (Some((_, shown)), Some(at)) if bullet::close_button(*shown).contains(at));
        pointer.hover = hit.map(|(window, _)| window);
        let mut started = false;
        if let (None, Some((bullet::Press::Window(window), from))) = (&pointer.drag, &pointer.press)
            && bullet::is_drag(*from, pos)
        {
            pointer.drag = Some(bullet::Drag {
                window: window.clone(),
                at: flat.unwrap_or_default(),
                over,
            });
            pointer.press = None;
            started = true;
        }
        if let (Some(drag), Some(at)) = (pointer.drag.as_mut(), flat) {
            drag.at = at;
            drag.over = over;
        }
        if started {
            self.set_cursor_override(Some(smithay::input::pointer::CursorIcon::Grabbing));
        }
    }

    /// A pointer button in bullet time. On a window, a left press goes to it when let go, unless
    /// it became a drag, which sends the window to the workspace it's let go over; a left click on
    /// its close button closes it; the middle button closes it and the right one minimises it.
    /// Anything else (the bar, a stream, a frame) is a click, as before.
    pub fn bullet_button(&mut self, button: u32, pressed: bool, pos: Point<f64, Logical>) {
        const LEFT: u32 = 0x110;
        const RIGHT: u32 = 0x111;
        const MIDDLE: u32 = 0x112;
        if !pressed {
            if button != LEFT {
                return;
            }
            let Some(mode) = self.bullet.as_mut() else {
                return;
            };
            let (press, drag) = (mode.pointer.press.take(), mode.pointer.drag.take());
            if let Some(drag) = drag {
                self.set_cursor_override(None);
                let from = self.workspaces.find(&drag.window);
                match drag.over {
                    Some(index) if Some(index) != from => {
                        self.bullet_select(Some(Target::Window(drag.window)));
                        self.bullet_send(index);
                    }
                    _ => {}
                }
                return;
            }
            let hit = self
                .overview_point(pos)
                .and_then(|at| Some((at, self.overview_window_at(at)?)));
            match press {
                Some((bullet::Press::Close(window), _)) => {
                    let still_on = matches!(&hit, Some((at, (under, shown))) if *under == window && bullet::close_button(*shown).contains(*at));
                    if still_on {
                        self.bullet_close(&window);
                    }
                }
                Some((bullet::Press::Window(window), _)) => {
                    self.bullet_go(Some(Target::Window(window)));
                }
                None => {}
            }
            return;
        }
        // The bar and the code rain first, as a click always has.
        if self.bar_target_at(pos).is_some() || self.rain_window_at(pos).is_some() {
            if button == LEFT {
                self.bullet_click(pos);
            }
            return;
        }
        let hit = self
            .overview_point(pos)
            .and_then(|at| Some((at, self.overview_window_at(at)?)));
        let Some((at, (window, shown))) = hit else {
            if button == LEFT {
                self.bullet_click(pos);
            }
            return;
        };
        match button {
            LEFT => {
                let press = if bullet::close_button(shown).contains(at) {
                    bullet::Press::Close(window)
                } else {
                    bullet::Press::Window(window)
                };
                if let Some(mode) = self.bullet.as_mut() {
                    mode.pointer.press = Some((press, pos));
                }
            }
            MIDDLE => self.bullet_close(&window),
            RIGHT => {
                self.minimise(&window);
                self.bullet_select(Some(Target::Stream(window)));
            }
            _ => {}
        }
    }

    /// Closes a window from bullet time, which stays open.
    fn bullet_close(&mut self, window: &Window) {
        if let Some(mode) = self.bullet.as_mut() {
            if mode.pointer.hover.as_ref() == Some(window) {
                mode.pointer.hover = None;
                mode.pointer.on_close = false;
            }
        }
        self.close_window(window);
    }

    /// Esc while a window is dragged in bullet time: the drag ends, with nothing sent. Whether
    /// there was one.
    fn cancel_bullet_drag(&mut self) -> bool {
        let dragging = self
            .bullet
            .as_mut()
            .and_then(|mode| mode.pointer.drag.take())
            .is_some();
        if dragging {
            self.set_cursor_override(None);
        }
        dragging
    }

    /// A click in bullet time: on the bar it does what the bar says; on a window or stream it goes
    /// there; on a workspace's frame it lands on that workspace.
    pub fn bullet_click(&mut self, pos: Point<f64, Logical>) {
        // The bar is drawn upright over the tilted overview, so it's asked first.
        if let Some(target) = self.bar_target_at(pos) {
            return self.bullet_bar_clicked(target);
        }
        if let Some(window) = self.rain_window_at(pos) {
            return self.bullet_go(Some(Target::Stream(window)));
        }
        let Some(screen) = self.output_rect() else {
            return;
        };
        // The overview is tilted on screen; its hit areas are kept flat.
        let local = (pos.x - screen.x as f64, pos.y - screen.y as f64);
        let Some(local) = self.overview_tilt.unproject(local) else {
            return;
        };
        let window = self
            .overview_hits
            .iter()
            .find(|(_, area)| area.contains(local))
            .map(|(window, _)| window.clone());
        if let Some(window) = window {
            return self.bullet_go(Some(Target::Window(window)));
        }
        let frame = self
            .frame_hits
            .iter()
            .find(|(_, area)| area.contains(local))
            .map(|(index, _)| *index);
        if let Some(index) = frame {
            if let Some(mode) = self.bullet.as_mut() {
                mode.view = index;
            }
            self.bullet_go(None);
        }
    }

    /// Any input. Returns true if the UI was faded out, so the input only brings it back.
    pub fn wake_ui(&mut self) -> bool {
        let now = self.clock.tick();
        self.idle.input(now)
    }

    /// Faces for characters the embedded fonts lack have landed: everything painted with text
    /// is painted again, now that those characters can be drawn.
    pub fn fallback_fonts_landed(&mut self) {
        if !crate::fallback::take_landed() {
            return;
        }
        for chrome in self.chromes.values_mut() {
            chrome.forget_painted_text();
        }
        self.notices.forget_painted_text();
        self.centre.forget_painted_text();
        self.explorer.forget_painted_text();
        self.quick.forget_painted_text();
        self.sheet.forget_painted_text();
        self.toast.forget_painted_text();
        self.rain.forget_painted_text();
        if let Some(exit) = self.exit.as_mut() {
            exit.forget_painted_text();
        }
        if let Some(offer) = self.offer.as_mut() {
            offer.forget_painted_text();
        }
        if let Some(share) = self.share.as_mut() {
            share.forget_painted_text();
        }
    }

    /// How visible the UI is now. It fades after a while with no input, unless Caps Lock is on,
    /// a window fills the screen, or the explorer is open.
    pub fn ui_opacity(&mut self, now: f64) -> f32 {
        let caps_lock = self
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().caps_lock);
        let keep_up = caps_lock
            || self.fullscreen_on_screen()
            || self.explorer.is_open()
            || self.quick.is_open()
            || self.centre.is_open()
            || self.bullet.is_some()
            // The ask card waits as long as it takes; the wallpaper mustn't swallow it. The
            // offer at login is the same, and so is a layout still coming back.
            || self.exit.is_some()
            || self.offer.is_some()
            || self.share.is_some()
            || self.restoring.is_some()
            || self.lock.is_some()
            || self.unlocking.is_some();
        let opacity = self.idle.update(now, keep_up) as f32;
        // Pop-ups that arrived while it was faded show now it's coming back, and ones that
        // arrived while you typed show at the pause.
        self.show_waiting_popups(now);
        opacity
    }

    pub fn show_toast(&mut self, title: &str, body: &str) {
        let now = self.clock.tick();
        self.toast.show(title, body, now);
    }

    /// The volume keys. The level is set outright rather than stepped, so what the display says
    /// and what the machine does can't drift apart, and the new level is shown at once — the
    /// reading catches up when the change has been made.
    pub fn change_volume(&mut self, step: i8) {
        let (level, muted) = {
            let mut reading = self.status.lock().unwrap();
            let level = (reading.volume as i16 + step as i16).clamp(0, 100) as u8;
            reading.volume = level;
            (level, reading.muted)
        };
        status::change(status::set_volume(level));
        self.show_osd(osd::Kind::Volume { muted }, level);
        // The click is played through the speakers after the change, so it sounds at the level
        // that was just set. Muted, there would be nothing to hear.
        if self.settings.sound.volume_blip && !muted {
            crate::sound::blip();
        }
    }

    /// The mute keys, for the speakers or the microphone.
    pub fn toggle_mute(&mut self, microphone: bool) {
        let (muted, level) = {
            let mut reading = self.status.lock().unwrap();
            let muted = if microphone {
                reading.mic_muted = !reading.mic_muted;
                reading.mic_muted
            } else {
                reading.muted = !reading.muted;
                reading.muted
            };
            (muted, reading.volume)
        };
        status::change(status::set_mute_on(microphone, muted));
        let kind = if microphone {
            osd::Kind::Microphone { muted }
        } else {
            osd::Kind::Volume { muted }
        };
        self.show_osd(kind, level);
    }

    /// The brightness keys. Without a backlight there is nothing to show, so a toast says so.
    pub fn change_brightness(&mut self, step: i8) {
        let Some((command, level)) = crate::launch::brightness(step) else {
            self.show_toast(
                "No brightness control",
                "This screen’s brightness can’t be set from here.",
            );
            return;
        };
        self.status.lock().unwrap().brightness = Some(level);
        status::change(command);
        self.show_osd(osd::Kind::Brightness, level);
    }

    /// A media key: to whichever player is playing, and what it's doing afterwards on the display.
    /// Nested, the players belong to the desktop around this window, so the key is only logged.
    pub fn media_key(&mut self, transport: crate::media::Transport) {
        if launch::machine_commands_held() {
            tracing::info!(?transport, "nested: media key logged, not sent");
            return;
        }
        if let Some(reply) = self.media_replies.clone() {
            crate::media::send(transport, reply);
        }
    }

    /// What a player is doing after a media key.
    pub fn media_played(&mut self, playing: crate::media::Playing) {
        match playing {
            crate::media::Playing::Nobody => self.show_toast(
                "Nothing to play",
                "No music or video app is open to take the media keys.",
            ),
            crate::media::Playing::Track { playing, title } => {
                if self.quick.is_open() {
                    return;
                }
                let now = self.clock.tick();
                self.osd.show_with(osd::Kind::Media { playing }, title, now);
            }
        }
    }

    /// The display for a key that changed the volume or the brightness, unless quick settings is
    /// open: it shows the same levels on its own sliders, and two displays of one thing is one
    /// too many.
    pub fn show_osd(&mut self, kind: osd::Kind, level: u8) {
        if self.quick.is_open() {
            self.osd.hide();
            return;
        }
        let now = self.clock.tick();
        self.osd.show(kind, level, now);
    }

    /// As `show_osd`, with a line under the label.
    pub fn show_osd_with(&mut self, kind: osd::Kind, detail: &str) {
        if self.quick.is_open() {
            self.osd.hide();
            return;
        }
        let now = self.clock.tick();
        self.osd.show_with(kind, detail.to_string(), now);
    }

    /// Applies the settings file's new contents straight away (`watch.rs` follows it).
    /// The keyboard's ring inside a panel: the window ring's colour at the same four fifths'
    /// opacity it is drawn with, packed as `0xrrggbbaa` for the painter.
    pub fn panel_ring(&self) -> u32 {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        let [r, g, b] = self.ring_rgb;
        (channel(r) << 24) | (channel(g) << 16) | (channel(b) << 8) | 0xcc
    }

    pub fn apply_settings(&mut self, settings: slipstream_config::Settings) {
        if settings == self.settings {
            return;
        }
        tracing::info!(?settings, "settings changed");
        self.idle.set_fade_after(settings.wallpaper.fade_after_secs);
        self.idle.set_lock_after(settings.lock.after_idle_mins);
        let night_light = settings.display.night_light;
        if night_light != self.settings.display.night_light {
            self.set_night_light(night_light);
        }
        self.set_reduced_motion(settings.motion.reduced);
        let now = self.clock.tick();
        self.wallpaper = settings.wallpaper.clone();
        for saver in self.savers.values_mut() {
            saver.set_wallpaper(&settings.wallpaper, now);
        }
        self.ring_rgb = settings.borders.selected_tile_rgb();
        self.rain.set_ring_colour(self.ring_rgb);
        self.bullet_rgb = settings.borders.bullet_time_rgb();
        if settings.workspaces != self.settings.workspaces {
            let entries = workspace_entries(&settings);
            self.set_workspaces(&entries, self.screens.len());
        }
        if settings.session.remember != self.settings.session.remember {
            if settings.session.remember {
                // Write what's open now rather than waiting for the next thing to move: the
                // switch was just turned on, and a layout is what it promises.
                self.record_due = Some(self.wall());
            } else {
                // Off means no record sitting in the state directory at all, not a stale one.
                self.record_due = None;
                self.saved_record = None;
                let path = session::path();
                // A record in a folder another Slipstream owns isn't this one's to delete.
                if self.owns_state
                    && let Err(err) = session::forget(&path)
                {
                    tracing::warn!(path = %path.display(), "couldn't forget the layout: {err}");
                }
            }
        }
        self.settings = settings;
    }

    pub fn toggle_explorer(&mut self) {
        if self.explorer.is_open() {
            self.explorer.close();
        } else {
            self.close_panels();
            let now = self.clock.tick();
            self.explorer.open(now);
        }
    }

    pub fn explorer_key(&mut self, sym: Keysym, ch: Option<char>, mods: Mods) {
        match self.explorer.key(sym, ch, mods) {
            Outcome::Nothing => {}
            Outcome::Close => self.explorer.close(),
            Outcome::Activate(item, fresh) => self.explorer_activate(item, fresh),
        }
    }

    /// A click while the explorer is open, in logical pixels from the output's corner: on an item
    /// it opens it (a `middle` click starts another of an open app). Outside it closes, and on
    /// another of the bar's buttons that button then does its own thing, as quick settings and
    /// the centre hand them on.
    pub fn explorer_click(&mut self, x: f64, y: f64, middle: bool) {
        match self.explorer.click(x, y, middle) {
            Outcome::Activate(item, fresh) => self.explorer_activate(item, fresh),
            Outcome::Close => {
                self.explorer.close();
                let target = (!self.fullscreen_on_screen())
                    .then(|| self.focused_bar_target(x, y))
                    .flatten()
                    .filter(|target| *target != bar::Target::Apps);
                if let Some(target) = target {
                    self.bar_clicked(target);
                }
            }
            Outcome::Nothing => {}
        }
    }

    /// Opens what was chosen in the explorer. An app that's already open is focused, on
    /// whichever workspace it's on, rather than started again, unless `fresh` asks for a new
    /// one (Shift+Enter or a middle click).
    pub fn explorer_activate(&mut self, item: Item, fresh: bool) {
        self.explorer.close();
        match item {
            Item::App(app) if fresh => {
                tracing::info!(app = app.id, command = ?app.exec, "starting another from the explorer");
                self.start_app(&app);
            }
            Item::App(app) => {
                let names = [
                    Some(app.id.to_lowercase()),
                    app.wm_class.as_ref().map(|class| class.to_lowercase()),
                ];
                let open = self.all_open_windows().into_iter().find(|window| {
                    window.alive()
                        && window_app_id(window).is_some_and(|id| names.contains(&Some(id)))
                });
                match open {
                    Some(window) if self.rain.contains(&window) => self.restore(&window),
                    Some(window) => {
                        self.activate_window(&window);
                        tracing::info!(app = app.id, "focused an open app from the explorer");
                    }
                    None => {
                        tracing::info!(app = app.id, command = ?app.exec, "launching from the explorer");
                        self.start_app(&app);
                    }
                }
            }
            Item::File(path) => self.open_path(&path),
            Item::Run(query) => self.run_query(&query),
            Item::Shortcuts => self.toggle_sheet(),
            Item::LogOut => {
                tracing::info!("log out chosen in the explorer");
                self.begin_exit(Intent::LogOut);
            }
        }
    }

    /// Starts an app from its desktop entry, and says so if it can't be.
    fn start_app(&mut self, app: &crate::apps::App) {
        self.concentration.asked(std::time::Instant::now());
        match launch::app(&app.exec, app.terminal) {
            Ok(()) => {}
            Err(launch::Failure::NothingInstalled) => self.no_terminal(),
            Err(launch::Failure::Spawn(err)) => {
                self.show_toast(&format!("Couldn’t start {}", app.name), &err.to_string())
            }
        }
    }

    /// Opens a file or folder in its default app.
    fn open_path(&mut self, path: &std::path::Path) {
        self.concentration.asked(std::time::Instant::now());
        let command = ["xdg-open".to_string(), path.to_string_lossy().into_owned()];
        if let Err(err) = launch::spawn(&command) {
            self.show_toast("Couldn’t open it", &err.to_string());
        }
    }

    /// The explorer's Run: a URL or a path is opened, anything else is run as a program with its
    /// arguments, split as a desktop entry's command line is and never through a shell.
    fn run_query(&mut self, query: &str) {
        use crate::apps::{Run, run_query};
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let run = match run_query(query, &home, |_| false) {
            // A path is looked at on a thread, so a hung mount can hold the event loop up for a
            // moment at most. One that doesn't answer in time is handed on to be opened anyway.
            Run::Path(path) => {
                let (typed, folder) = (query.to_string(), home.clone());
                let looked = launch::within(std::time::Duration::from_millis(150), move || {
                    let run = run_query(&typed, &folder, launch::executable);
                    let missing = matches!(&run, Run::Path(path) if !path.exists());
                    (run, missing)
                });
                match looked {
                    Some((_, true)) => {
                        let (title, body) = launch::nothing_at(query);
                        self.show_toast(title, &body);
                        return;
                    }
                    Some((run, false)) => run,
                    None => Run::Path(path),
                }
            }
            run => run,
        };
        match run {
            Run::Url(url) => {
                tracing::info!("opening a URL from the explorer");
                if let Err(err) = launch::spawn(&["xdg-open".to_string(), url]) {
                    self.show_toast("Couldn’t open it", &err.to_string());
                }
            }
            Run::Path(path) => {
                tracing::info!("opening a path from the explorer");
                self.open_path(&path);
            }
            Run::Command(words) => {
                tracing::info!(command = ?words, "running a command from the explorer");
                let program = words.first().cloned().unwrap_or_default();
                if let Err(err) = launch::spawn(&words) {
                    let (title, body) = launch::couldnt_run(&program, &err);
                    self.show_toast(title, &body);
                }
            }
        }
    }

    /// Super+Return, Super+E and All settings: starts the app, or says what's missing.
    pub fn launch_app(&mut self, app: crate::keys::App) {
        use crate::keys::App;
        self.concentration.asked(std::time::Instant::now());
        match launch::launch(app) {
            Ok(()) => {}
            Err(launch::Failure::NothingInstalled) => match app {
                App::Terminal => self.no_terminal(),
                App::Files => self.show_toast(
                    "No file manager",
                    "Install Dolphin or Nautilus, or set a default for folders.",
                ),
                App::Settings => self.show_toast(
                    "No Settings app",
                    "slipstream-settings isn’t installed beside Slipstream or on the PATH.",
                ),
            },
            Err(launch::Failure::Spawn(err)) => {
                let name = match app {
                    App::Terminal => "the terminal",
                    App::Files => "the file manager",
                    App::Settings => "Settings",
                };
                self.show_toast(&format!("Couldn’t start {name}"), &err.to_string());
            }
        }
    }

    fn no_terminal(&mut self) {
        self.show_toast(
            "No terminal",
            "Set $TERMINAL or install one: Konsole, Ptyxis, foot…",
        );
    }

    /// Quick settings' chevrons: the system's own settings page for Wi-Fi or Bluetooth.
    pub fn open_settings_page(&mut self, page: launch::Page) {
        let Some(command) = launch::settings_page(page) else {
            match page {
                launch::Page::Network => self.show_toast(
                    "No network settings app",
                    "Install plasma-nm or nm-connection-editor",
                ),
                launch::Page::Bluetooth => {
                    self.show_toast("No Bluetooth settings app", "Install bluedevil or blueman")
                }
            }
            return;
        };
        tracing::info!(?page, ?command, "opening a system settings page");
        if let Err(err) = launch::spawn(&command) {
            self.show_toast("Couldn’t open the settings page", &err.to_string());
        }
    }

    /// The app IDs and X11 classes of every open window, for the explorer's running dots.
    /// Every window there is: tiled on any workspace, or minimised into the code rain.
    pub fn all_open_windows(&self) -> Vec<Window> {
        self.workspaces
            .all_windows()
            .into_iter()
            .chain(self.rain.streams.iter().map(|stream| stream.window.clone()))
            .collect()
    }

    pub fn running_apps(&self) -> Vec<String> {
        self.all_open_windows()
            .iter()
            .filter(|window| window.alive())
            .filter_map(window_app_id)
            .collect()
    }

    /// Keeps the picture of a window that is going, where it was drawn, to fade out there. Only
    /// a window on screen leaves one: not one minimised, on a workspace out of sight, under the
    /// lock or in bullet time's overview.
    fn leave_a_ghost(&mut self, window: &Window) {
        let picture = self
            .pictures
            .iter()
            .position(|(drawn, _)| drawn == window)
            .map(|at| self.pictures.swap_remove(at).1);
        let Some(picture) = picture else {
            return;
        };
        let on_screen = self
            .workspaces
            .find(window)
            .is_some_and(|index| self.screens.showing(index).is_some());
        if !on_screen || self.lock.is_some() || self.bullet.is_some() || self.idle.is_faded() {
            return;
        }
        let now = self.clock.tick();
        let rect = match self.fitted.iter().find(|(fitted, _)| fitted == window) {
            Some((_, drawn)) => Some(*drawn),
            None => self.motion.frame(window, now).map(|frame| {
                let [x, y, w, h] = frame.rect;
                // The window's size as it was drawn: by now a closing client may report none.
                let own = picture.own();
                let (w, h) = if frame.moving {
                    (w, h)
                } else {
                    (own.w as f64, own.h as f64)
                };
                Rectangle::new((x, y).into(), (w, h).into())
            }),
        };
        let Some(rect) = rect else {
            return;
        };
        let reduced = self.clock.reduced_motion;
        self.ghosts
            .push(crate::ghost::Ghost::new(picture, rect, now, reduced));
    }

    /// Takes out any stream whose window has gone without the compositor hearing of it, so a
    /// closed app never leaves a column of rain behind or comes back as an empty tile. Closing
    /// normally already goes through `remove_window`; this is the backstop, run once a pass.
    pub fn drop_dead_streams(&mut self) {
        let dead: Vec<Window> = self
            .rain
            .streams
            .iter()
            .filter(|stream| !stream.window.alive())
            .map(|stream| stream.window.clone())
            .collect();
        for window in dead {
            tracing::info!("a minimised window had gone; its stream goes too");
            self.remove_window(&window);
        }
    }

    /// Super+M: the focused window pours into a stream of code rain on the right.
    pub fn minimise_focused(&mut self) {
        if let Some(window) = self.focused_window() {
            self.minimise(&window);
        }
    }

    /// Takes `window` out of its layout into the code rain. The app keeps running.
    pub fn minimise(&mut self, window: &Window) {
        // It pours towards the rain, which is on the leftmost screen whichever screen it was on.
        let (Some(screen), Some(scale)) = (self.screen_rect(0), self.output_scale()) else {
            return;
        };
        if self.rain.contains(window) || self.workspaces.find(window).is_none() {
            return;
        }
        if self.fullscreen.as_ref() == Some(window) {
            self.fullscreen = None;
        }
        self.take_off_workspace(window);
        let name = self.stream_name(window);
        let icon_px = crate::rain::icon_px(scale);
        let icon = window_app_id(window).and_then(|id| self.explorer.app_icon(&id, icon_px));
        let pid = self.window_pid(window);
        let top = bar::HEIGHT;
        let now = self.clock.tick();
        self.rain.add(window.clone(), name, icon, pid, now);
        // It pours into its stream as it fades, while the rest retile into the space it left.
        let column = Rain::column(self.rain.len() - 1, screen, top);
        self.motion.place(window, column, now);
        self.motion.fade(window, 0.0, now, 0.26);
        self.retile();
        self.restore_focus();
        tracing::info!(
            window = logged_app(window),
            screen = self.screens.get(0).map(|screen| screen.output.name()),
            "minimised to code rain"
        );
    }

    /// Super+Shift+M: the most recently minimised window comes back.
    pub fn restore_latest(&mut self) {
        match self.rain.streams.last().map(|stream| stream.window.clone()) {
            Some(window) => self.restore(&window),
            None => self.show_toast(
                "Nothing in the code rain",
                "Super+M minimises the focused window into it.",
            ),
        }
    }

    /// A window condenses out of its stream, beside the window you're using on the workspace on
    /// screen, and takes focus.
    pub fn restore(&mut self, window: &Window) {
        let Some(screen) = self.screen_rect(0) else {
            return;
        };
        let Some(index) = self.rain.remove(window) else {
            return;
        };
        let area = self.output_area().unwrap_or(screen);
        let now = self.clock.tick();
        self.motion
            .jump(window, Rain::column(index, screen, bar::HEIGHT));
        self.motion.fade(window, 1.0, now, 0.26);
        let beside = self.focused_window();
        let active = self.active_workspace();
        self.workspaces
            .insert(active, window.clone(), beside.as_ref(), area);
        self.retile();
        self.focus_window(window);
        tracing::info!(window = logged_app(window), "restored from code rain");
    }

    /// The minimised window whose stream is under `pos`. The rain is drawn on the leftmost
    /// screen only, so nothing on any other screen is a stream.
    pub fn rain_window_at(&self, pos: Point<f64, Logical>) -> Option<Window> {
        let showing = self.screens.get(0).map(|screen| screen.workspace)?;
        if self.fullscreen_on(showing) {
            return None;
        }
        let index = self
            .rain
            .stream_hit(pos.x, pos.y, self.screen_rect(0)?, bar::HEIGHT)?;
        Some(self.rain.streams[index].window.clone())
    }

    /// The process behind a window: the X11 client's own claim, or the Wayland socket's peer.
    /// The process behind a window: the X11 client's own claim, or the Wayland socket's peer.
    /// Its app's meter is read from there (`usage::Meter`).
    fn window_pid(&self, window: &Window) -> Option<u32> {
        if let Some(surface) = window.x11_surface() {
            return surface.pid();
        }
        let client = window.toplevel()?.wl_surface().client()?;
        client
            .get_credentials(&self.display_handle)
            .ok()
            .map(|credentials| credentials.pid as u32)
    }

    /// `shot:` paths for the backend to save once it has drawn a frame.
    pub fn take_screenshots(&mut self) -> Vec<String> {
        std::mem::take(&mut self.screenshots)
    }

    /// Runs the timed debug steps now due. Screenshots wait for the next frame.
    pub fn run_due_debug_steps(&mut self) {
        for step in self.debug.due(self.start_time.elapsed()) {
            tracing::info!(?step, "debug step");
            // While locked, only steps that go through the same paths as the keyboard, the
            // pointer and apps do anything, so a script can't reach round the lock.
            if self.lock.is_some() && !step.works_while_locked() {
                tracing::info!("debug step ignored: locked");
                continue;
            }
            match step {
                debug::Step::Workspace(n) => self.switch_workspace((n as usize).saturating_sub(1)),
                debug::Step::MoveTo(n) => {
                    self.move_focused_to_workspace((n as usize).saturating_sub(1))
                }
                debug::Step::Close => self.close_focused(),
                debug::Step::Run(command) => {
                    let command: Vec<String> =
                        command.split_whitespace().map(String::from).collect();
                    let _ = launch::spawn(&command);
                }
                debug::Step::Wallpaper(id) => {
                    let now = self.clock.tick();
                    // Every screen's wallpaper, and `any` would stop at the first.
                    let shown: Vec<bool> = self
                        .savers
                        .values_mut()
                        .map(|saver| saver.show(&id, now))
                        .collect();
                    if !shown.into_iter().any(|shown| shown) {
                        tracing::warn!(id, "no living-wallpaper variation by that name");
                    }
                }
                debug::Step::Demand(value) => self.rain.pin_demand(Some(value)),
                debug::Step::Fade(value) => {
                    self.idle.pinned = Some(1.0 - value.clamp(0.0, 1.0) as f64);
                }
                debug::Step::AddScreen(w, h) => self.add_made_up_screen(w, h),
                debug::Step::DropScreen(name) => self.drop_made_up_screen(&name),
                debug::Step::Lid(closed) => self.lid_switched(closed),
                debug::Step::Screens => self.log_screens(),
                debug::Step::Windows => self.log_windows(),
                debug::Step::Motion => self.log_reduced_motion(),
                debug::Step::NextScreen => self.focus_next_screen(),
                debug::Step::MoveToNextScreen => self.move_focused_to_next_screen(),
                debug::Step::LogOut => self.begin_exit(Intent::LogOut),
                debug::Step::Restart => self.begin_exit(Intent::Restart),
                debug::Step::ShutDown => self.begin_exit(Intent::ShutDown),
                debug::Step::ExitKey(name) => {
                    self.exit_key(xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS))
                }
                debug::Step::ExitClick(x, y) => self.exit_click(x, y),
                debug::Step::OfferKey(name) => {
                    self.offer_key(xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS))
                }
                debug::Step::OfferClick(x, y) => self.offer_click(x, y),
                debug::Step::SharePicker(kinds) => {
                    let mut list = String::new();
                    if kinds.contains('m') {
                        for output in self.space.outputs() {
                            list.push_str(&format!("Monitor: {} test\n", output.name()));
                        }
                    }
                    if kinds.contains('w') {
                        for window in self.all_open_windows() {
                            if let Some(listed) = self.captures.identifier_of(&window) {
                                list.push_str(&format!(
                                    "Window: {} ({listed})\n",
                                    window_title(&window)
                                ));
                            }
                        }
                    }
                    self.open_share_picker(&list, None);
                }
                debug::Step::ShareKey(name) => {
                    let (shift, name) = match name.strip_prefix("shift+") {
                        Some(name) => (true, name),
                        None => (false, name.as_str()),
                    };
                    self.share_key(xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS), shift)
                }
                debug::Step::ShareClick(x, y) => self.share_click(x, y),
                debug::Step::StopSharing => self.bar_clicked(bar::Target::Sharing),
                // Straight out, as the watchdog goes: every headless check ends with this. A key
                // still held back here means a press whose release never matched it.
                debug::Step::Quit => {
                    if !self.suppressed_keys.is_empty() {
                        tracing::warn!(keys = ?self.suppressed_keys, "keys still suppressed at quit");
                    }
                    self.loop_signal.stop();
                }
                debug::Step::Keys(keys) => {
                    for (name, pressed) in keys {
                        self.inject_key(&name, pressed);
                    }
                }
                debug::Step::Screenshot(path) => self.screenshots.push(path),
                // Nested only, never in a login session (`lock::steps_allowed`).
                debug::Step::Lock if !crate::lock::steps_allowed() => {
                    tracing::warn!("debug step ignored: the lock's step runs nested only");
                }
                debug::Step::Lock => {
                    self.lock_now();
                }
                debug::Step::Explore => self.toggle_explorer(),
                // Key steps act only on an open panel. A closed one keeps its last selection, and a
                // mistimed step would otherwise press it.
                debug::Step::Type(_) | debug::Step::Key(_) if !self.explorer.is_open() => {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::Type(text) => {
                    for ch in text.chars() {
                        self.explorer_key(Keysym::NoSymbol, Some(ch), Mods::default());
                    }
                }
                debug::Step::Key(name) => {
                    let (mods, key) = debug::mods_and_key(&name);
                    self.explorer_key(key, None, mods);
                }
                debug::Step::MoveTile(direction) => self.move_tile(direction),
                debug::Step::Click(x, y) => self.debug_click(x, y),
                debug::Step::Drag(x1, y1, x2, y2) => self.debug_drag(x1, y1, x2, y2),
                debug::Step::Heavier => self.weigh(true),
                debug::Step::Lighter => self.weigh(false),
                debug::Step::Gravity => self.toggle_gravity(),
                debug::Step::Minimise => self.minimise_focused(),
                debug::Step::Restore => self.restore_latest(),
                debug::Step::Idle => {
                    let now = self.clock.tick();
                    self.idle.fade(now);
                }
                debug::Step::Wake => {
                    self.wake_ui();
                }
                debug::Step::Button(button) => {
                    for pressed in [ButtonState::Pressed, ButtonState::Released] {
                        self.button_event(button, pressed, InputTime::now());
                    }
                }
                debug::Step::ButtonHeld(button, pressed) => {
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    self.button_event(button, state, InputTime::now());
                }
                debug::Step::Pointer(screen, x, y) => {
                    // On the named screen, or the focused one.
                    let rect = match &screen {
                        Some(name) => self
                            .screens
                            .iter()
                            .position(|s| s.output.name() == *name)
                            .and_then(|index| self.screen_rect(index)),
                        None => self.output_rect(),
                    };
                    let origin = rect.map_or((0, 0), |r| (r.x, r.y));
                    let at = Point::from((origin.0 as f64 + x, origin.1 as f64 + y));
                    self.pointer_moved_to(at, InputTime::now());
                }
                debug::Step::Cycle(tabs) => {
                    for _ in 0..tabs {
                        self.cycle_windows(true);
                    }
                    self.finish_cycle();
                }
                debug::Step::Bullet => self.toggle_bullet_time(),
                debug::Step::BulletKey(name) => {
                    let (mods, key) = debug::mods_and_key(&name);
                    self.bullet_key(key, mods);
                }
                debug::Step::BulletClick(x, y) => {
                    if self.bullet.is_some() {
                        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
                        let at = Point::from((origin.0 as f64 + x, origin.1 as f64 + y));
                        self.bullet_click(at);
                    }
                }
                // Only the card: a debug step that really changed the volume would change it on
                // the laptop the nested run is on.
                debug::Step::Osd(what) => {
                    let (kind, level) = match what.split_once('@') {
                        Some((kind, level)) => (kind, level.parse().unwrap_or(50)),
                        None => (what.as_str(), 50),
                    };
                    let kind = match kind {
                        "muted" => Some(osd::Kind::Volume { muted: true }),
                        "volume" => Some(osd::Kind::Volume { muted: false }),
                        "mic" => Some(osd::Kind::Microphone { muted: false }),
                        "micmuted" => Some(osd::Kind::Microphone { muted: true }),
                        "brightness" => Some(osd::Kind::Brightness),
                        other => {
                            tracing::warn!(other, "no such display");
                            None
                        }
                    };
                    if let Some(kind) = kind {
                        self.show_osd(kind, level);
                    }
                }
                debug::Step::Quick => self.toggle_quick_settings(),
                debug::Step::QuickKey(_) | debug::Step::QuickClick(..) if !self.quick.is_open() => {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::QuickKey(name) => {
                    let (shift, name) = match name.strip_prefix("shift+") {
                        Some(rest) => (true, rest),
                        None => (false, name.as_str()),
                    };
                    let key = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
                    self.quick_key(key, shift);
                }
                debug::Step::QuickClick(x, y) => self.quick_click(x, y),
                debug::Step::Centre => self.toggle_notification_centre(),
                debug::Step::CentreKey(_) | debug::Step::CentreClick(..)
                    if !self.centre.is_open() =>
                {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::CentreKey(name) => {
                    let (shift, name) = match name.strip_prefix("shift+") {
                        Some(rest) => (true, rest),
                        None => (false, name.as_str()),
                    };
                    let key = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
                    self.centre_key(key, shift);
                }
                debug::Step::CentreClick(x, y) => self.centre_click(x, y),
                debug::Step::Notify(app, summary, body, actions) => {
                    self.notification_arrived(crate::notify::Incoming {
                        id: crate::notify::next_id(),
                        app_name: app,
                        summary,
                        body,
                        default_action: true,
                        actions,
                        urgency: 1,
                        expire_timeout: -1,
                        ..Default::default()
                    });
                }
            }
        }
    }
}

/// X11 menus, tooltips and drag images: they place themselves and are never tiled.
/// Whether an output is the laptop's own panel, by the kind of connector it is on. Embedded
/// DisplayPort, LVDS and DSI are the three a built-in screen turns up on; everything else is
/// something plugged in.
/// Where each lit screen goes, left edge first: `screens` are names and widths in the order they
/// connected, and `panel` says which are laptop panels. Panels go first, in their own order, and
/// every other screen follows to their right in its order.
pub fn arrange(screens: &[(String, i32)], panel: impl Fn(&str) -> bool) -> Vec<(String, i32)> {
    let (panels, others): (Vec<_>, Vec<_>) = screens.iter().partition(|(name, _)| panel(name));
    let mut x = 0;
    panels
        .into_iter()
        .chain(others)
        .map(|(name, width)| {
            let at = x;
            x += width;
            (name.clone(), at)
        })
        .collect()
}

pub fn is_internal_panel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["edp", "lvds", "dsi"]
        .iter()
        .any(|kind| name.starts_with(kind))
}

fn is_override_redirect(window: &Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| surface.is_override_redirect())
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

/// The most a client's minimum size is taken at, in logical pixels: wider and taller than any
/// tiling area, so a real minimum is never cut short.
const MIN_SIZE_LIMIT: i32 = 16384;

/// The smallest size a window's client will draw at, width and height, 0 where it sets none.
pub(crate) fn min_size(window: &Window) -> (i32, i32) {
    if let Some(surface) = window.x11_surface() {
        return clamp_min_size(surface.min_size().map_or((0, 0), |size| (size.w, size.h)));
    }
    window.toplevel().map_or((0, 0), |toplevel| {
        with_states(toplevel.wl_surface(), |states| {
            let size = states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .min_size;
            clamp_min_size((size.w, size.h))
        })
    })
}

/// A minimum size as a client sent it, brought into range. Neither `xdg_toplevel.set_min_size`
/// nor X11's size hints are checked before they arrive, and a width near `i32::MAX` would
/// overflow the tiling's arithmetic.
fn clamp_min_size((w, h): (i32, i32)) -> (i32, i32) {
    (w.clamp(0, MIN_SIZE_LIMIT), h.clamp(0, MIN_SIZE_LIMIT))
}

pub(crate) fn window_title(window: &Window) -> String {
    if let Some(surface) = window.x11_surface() {
        return surface.title();
    }
    window
        .toplevel()
        .and_then(|toplevel| {
            with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()?
                    .lock()
                    .ok()?
                    .title
                    .clone()
            })
        })
        .unwrap_or_default()
}

/// Tells a window the size of `r`, its tile or (`full`) the whole screen, and which of its edges
/// are tiled.
/// Tells a window it isn't the active one any more, and sends that to its client.
fn deactivate(window: &Window) {
    window.set_activated(false);
    if let Some(toplevel) = window.toplevel()
        && toplevel.is_initial_configure_sent()
    {
        toplevel.send_pending_configure();
    }
}

/// Whether `window` is `parent`'s own: an xdg toplevel whose parent is its surface, or an X11
/// window transient for it.
fn is_child_of(window: &Window, parent: &Window) -> bool {
    if let (Some(child), Some(parent)) = (window.toplevel(), parent.wl_surface()) {
        return child.parent().as_ref() == Some(&*parent);
    }
    if let (Some(child), Some(parent)) = (window.x11_surface(), parent.x11_surface()) {
        return child.is_transient_for() == Some(parent.window_id());
    }
    false
}

/// The size to ask of a window for tile `r`, given its minimum size. The tile itself when the
/// window can go that small. When it can't, the window is drawn scaled down into its tile, so it
/// is asked for the tile's shape at the smallest size it accepts: scaled, it then fills the tile
/// exactly, instead of leaving a band of the tile empty below or beside it.
fn size_for_tile(r: Rect, (min_w, min_h): (i32, i32)) -> Rect {
    if r.w <= 0 || r.h <= 0 || (r.w >= min_w && r.h >= min_h) {
        return r;
    }
    let k = (min_w as f64 / r.w as f64).max(min_h as f64 / r.h as f64);
    Rect {
        w: ((r.w as f64 * k).ceil() as i32).max(min_w),
        h: ((r.h as f64 * k).ceil() as i32).max(min_h),
        ..r
    }
}

fn configure(window: &Window, r: Rect, full: bool) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            state.size = Some((r.w, r.h).into());
            // Tiled edges tell clients to drop rounded corners and shadows there.
            for edge in [
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ] {
                if full {
                    state.states.unset(edge);
                } else {
                    state.states.set(edge);
                }
            }
            if full {
                state.states.set(xdg_toplevel::State::Fullscreen);
            } else {
                state.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
        // Before a client's first commit, its initial configure carries this state instead.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    } else if let Some(surface) = window.x11_surface() {
        let _ = surface.configure(Rectangle::new((r.x, r.y).into(), (r.w, r.h).into()));
    }
}

/// A window's Wayland app ID or X11 class, lowercased, to match it to its desktop entry.
/// A tiling tree with its windows swapped for their slot numbers, for writing down. A window
/// `slot_of` doesn't know about takes the whole branch with it, so the tree that's written is
/// always one the slot numbers can be read back against.
pub(crate) fn window_app_id(window: &Window) -> Option<String> {
    if let Some(surface) = window.x11_surface() {
        return Some(surface.class().to_lowercase());
    }
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .ok()?
            .app_id
            .clone()
    })
    .map(|id| id.to_lowercase())
}

/// How the log names a window: by its app id, never by its title, which can hold a folder, a
/// document's name, a URL or who a message is from. `window_name` is for text on the screen.
pub(crate) fn logged_app(window: &Window) -> String {
    window_app_id(window).unwrap_or_else(|| "unknown app".to_string())
}

/// Data associated with a wayland client that connects to Slipstream.
/// One instance of this type per client.
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
}

impl ClientState {
    /// A new connection's state. One look at the connecting process, through `/proc`, decides
    /// everything it may reach.
    pub fn for_connection(stream: &UnixStream) -> Self {
        let peer = capture::unsandboxed_peer(stream);
        Self {
            may_capture: capture::client_may_capture(peer.as_ref()),
            may_control_clipboard: clipboard::client_may_control(peer.as_ref()),
            ..Self::default()
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_window_that_wont_fit_its_tile_is_asked_for_the_tile_s_shape() {
        let tile = Rect {
            x: 10,
            y: 20,
            w: 700,
            h: 900,
        };
        assert_eq!(
            size_for_tile(tile, (400, 300)),
            tile,
            "it fits: the tile itself"
        );
        let asked = size_for_tile(tile, (1034, 277));
        assert_eq!((asked.x, asked.y), (10, 20));
        assert!(asked.w >= 1034 && asked.h >= 277);
        let (shape, asked_shape) = (
            tile.w as f64 / tile.h as f64,
            asked.w as f64 / asked.h as f64,
        );
        assert!(
            (shape - asked_shape).abs() < 0.01,
            "{shape} against {asked_shape}"
        );
        let tall = size_for_tile(tile, (100, 1200));
        assert!(tall.h >= 1200 && (tall.w as f64 / tall.h as f64 - shape).abs() < 0.01);
    }

    #[test]
    fn internal_panels_go_first() {
        let named = |list: &[(&str, i32)]| -> Vec<(String, i32)> {
            list.iter()
                .map(|(name, w)| (name.to_string(), *w))
                .collect()
        };
        // The monitor lit first, or the panel came back when the lid opened.
        let places = arrange(
            &named(&[("HDMI-A-1", 1920), ("eDP-1", 1536)]),
            is_internal_panel,
        );
        assert_eq!(places, named(&[("eDP-1", 0), ("HDMI-A-1", 1536)]));
        // The panel goes out and comes back: still first.
        let without = arrange(&named(&[("HDMI-A-1", 1920)]), is_internal_panel);
        assert_eq!(without, named(&[("HDMI-A-1", 0)]));
        let back = arrange(
            &named(&[("HDMI-A-1", 1920), ("eDP-1", 1536)]),
            is_internal_panel,
        );
        assert_eq!(back, places);
        // Other screens keep the order they connected in.
        let three = arrange(
            &named(&[("DP-3", 2560), ("eDP-1", 1536), ("HDMI-A-1", 1920)]),
            is_internal_panel,
        );
        assert_eq!(
            three,
            named(&[("eDP-1", 0), ("DP-3", 1536), ("HDMI-A-1", 4096)])
        );
    }

    use super::*;

    #[test]
    fn a_minimum_size_from_a_client_is_brought_into_range() {
        assert_eq!(clamp_min_size((940, 0)), (940, 0), "a real minimum stands");
        assert_eq!(
            clamp_min_size((i32::MAX, i32::MIN)),
            (MIN_SIZE_LIMIT, 0),
            "nothing past the limit, nothing below zero"
        );
    }
}

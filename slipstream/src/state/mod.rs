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
    explorer::{Explorer, Item, Outcome},
    focus::KeyboardFocus,
    gravity::{Rung, Step},
    idle::Idle,
    inhibit::LidInhibitor,
    keys::{self, Mods},
    known, launch,
    layout::{self, Direction, Rect},
    meter,
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

mod bullet_time;
mod debug_steps;
mod lifecycle;
mod screens;
mod streams;
mod ui;
mod windows;
mod workspaces;

/// Whether `window` is being moved or resized by the pointer, so it follows it without gliding.
fn space_placed_now(drag: &Option<crate::grabs::Drag>, window: &Window) -> bool {
    matches!(
        drag,
        Some(crate::grabs::Drag::Move { window: dragged, .. } | crate::grabs::Drag::Resize { window: dragged, .. })
            if dragged == window
    )
}

/// A floating window where it goes, and the size the keys asked of it, if any.
type PlacedFloat = (Window, Rect, Option<(i32, i32)>);

/// How long the recorded layout waits for the desktop entries to be read before giving up on
/// this login. The scan takes a fraction of a second; this is only a backstop.
const CATALOGUE_WAIT: f64 = 10.0;

/// How still the desktop has to be before the layout is written down again. Opening a window
/// retiles several times over as the client catches up, and none of those are worth a file.
const RECORD_SETTLE: f64 = 2.0;

/// How long a window takes to pour into its stream in the code rain, on its own and when a whole
/// workspace goes at once. A screenful moving together needs to be quicker: at the single
/// window's pace the same motion reads as the desktop sliding away rather than as each window
/// finding its own stream.
const POUR: f64 = crate::motion::MOVE;
const POUR_ALL: f64 = 0.2;

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
    /// Other programs' panels, launchers and overlays (`layers.rs`).
    pub layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState,
    /// What each screen's layer surfaces leave for tiling, as last tiled.
    pub layer_zones: Vec<(String, Rectangle<i32, Logical>)>,
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
    /// Caps Lock, while it's being told apart as a tap or a hold.
    pub caps_key: crate::awake::CapsKey,
    /// The Caps Lock press being told apart woke the faded UI, so a tap of it goes nowhere.
    pub caps_waking: bool,
    /// A Caps Lock tap that only woke the UI, whose release is to go nowhere too.
    pub caps_woke: Option<smithay::input::keyboard::Keycode>,
    /// Awake is on: the UI doesn't fade to the wallpaper. Held Caps Lock toggles it.
    pub awake: bool,
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
    /// `shot:` debug steps waiting for the next frame.
    pub screenshots: Vec<String>,
    /// Super+Shift+S while it's being chosen (`snip.rs`).
    pub snip: Option<crate::snip::Snip>,
    /// Snips breaking up and flying into their toast.
    pub snip_flights: Vec<crate::snip::Flight>,
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
    /// The way out, while it's on screen: Ctrl+Alt+Del, or Log out, Restart or Shut down.
    pub exit: Option<Exit<Window>>,
    /// The windows Super+H put into the code rain, so the next press brings back those and
    /// nothing else that happens to be minimised.
    hidden: Option<Vec<Window>>,
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
    /// The card asking what a screen Slipstream has never seen should show.
    pub connect: Option<Connect>,
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
    /// The living wallpaper.
    /// The living wallpaper, one per screen: each keeps its own grid, sized to that screen, and
    /// its own turn through the variations.
    pub savers: HashMap<String, Saver>,
    /// What a new screen's wallpaper starts from.
    wallpaper: slipstream_config::Wallpaper,
    pub idle_inhibit_state: IdleInhibitManagerState,
    /// `ext-idle-notify`: what tells other programs the seat has gone quiet.
    pub idle_notifier_state: IdleNotifierState<Slipstream>,
    /// Input arrived since `ext-idle-notify` was last told: it's told once a pass rather than on
    /// every event, since each telling re-arms a timer per listener.
    pub input_since_notified: bool,
    /// The screens are off after a spell with no input; the next input lights them again.
    pub screens_off: bool,
    /// Which screens' gamma ramps a program of the user's own is driving.
    pub gamma: crate::gamma::Gamma,
    /// Screen captures asked for through `wlr-screencopy`, waiting for their screen to draw.
    pub screencopy: crate::screencopy::Screencopy,
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
    /// What the bar's meter shows, from its command; `None` without one.
    pub meter: Arc<Mutex<Option<meter::Reading>>>,
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
        // Typing other scripts through IBus or fcitx5, and on-screen keyboards.
        crate::ime::init(&dh);
        let layer_shell_state = crate::layers::state(&dh);
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
        // `wlr-gamma-control`: wlsunset and the like drive a screen's ramps themselves, and the
        // night light stands aside for whichever screens they have taken.
        crate::gamma::state(&dh);
        // `wlr-screencopy`: grim, wf-recorder and the screenshot scripts people already have.
        // Offered to every client, and refused unless the program has been allowed (`screencopy.rs`).
        crate::screencopy::state(&dh);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);
        // `ext-idle-notify`: swayidle and anything else that waits for the seat to go quiet.
        // Slipstream does its own fading and locking on its own timers; this only tells other
        // programs what the seat is doing, and honours the same inhibitors we do.
        let idle_notifier_state = IdleNotifierState::<Self>::new(&dh, event_loop.handle());

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

        let settings = crate::settings::startup();
        meter::configure(&settings.meter);
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
            layer_shell_state,
            layer_zones: Vec::new(),
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
            caps_key: Default::default(),
            caps_waking: false,
            caps_woke: None,
            awake: false,
            super_tap: keys::SuperTap::default(),
            nested: false,
            owns_state: true,
            session: false,
            udev: None,
            xwm: None,
            pending_focus: None,
            focus_history: Vec::new(),
            switcher: None,
            arrange: None,
            arrangement: Rung::Centre,
            fullscreen: Vec::new(),
            refloat: Vec::new(),
            floating_sizes: Vec::new(),
            centre_when_sized: Vec::new(),
            screenshots: Vec::new(),
            screenshot_requests: Vec::new(),
            snip: None,
            history: Default::default(),
            battery: Default::default(),
            night: Default::default(),
            clip_answers,
            snip_flights: Vec::new(),
            screenshot_answers,
            flash: None,
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
            exit: None,
            exit_record: None,
            hidden: None,
            record_due: None,
            saved_record: None,
            unmapped: Vec::new(),
            fullscreen_on_map: Vec::new(),
            restoring: None,
            offer: None,
            connect: None,
            known: known::read(&known::path()),
            restore_looked: false,
            tags: Vec::new(),
            rain: Rain::new(false, settings.motion.effects, ring_rgb),
            idle: Idle::new(false, settings.wallpaper.fade_after_secs),
            concentration: Default::default(),
            savers: HashMap::new(),
            wallpaper: settings.wallpaper.clone(),
            idle_inhibit_state,
            idle_notifier_state,
            input_since_notified: false,
            screens_off: false,
            gamma: crate::gamma::Gamma::default(),
            screencopy: crate::screencopy::Screencopy::default(),
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
            meter: meter::start(),
            debug: debug::Script::default(),
            settings,
            ring_rgb,
            bullet_rgb,
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
        state
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
            bar::Target::Tray | bar::Target::Meter => self.toggle_quick_settings(),
            bar::Target::Clock | bar::Target::Bell => self.toggle_notification_centre(),
            bar::Target::Overview => self.toggle_bullet_time(),
            bar::Target::Awake => self.set_awake(false),
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

/// How a workspace is named in a sentence: `Workspace 3`, or its own name.
fn workspace_called(label: &str) -> String {
    if label.chars().all(|ch| ch.is_ascii_digit()) {
        format!("Workspace {label}")
    } else {
        label.to_string()
    }
}

/// X11 menus, tooltips and drag images: they place themselves and are never tiled.
/// Whether an output is the laptop's own panel, by the kind of connector it is on. Embedded
/// DisplayPort, LVDS and DSI are the three a built-in screen turns up on; everything else is
/// something plugged in.
/// One screen to be placed: its connector's name, how big it is, and where its owner says it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Placing {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub place: slipstream_config::ScreenPlace,
}

/// Where each lit screen's top left corner goes. Panels come first, then everything else in the
/// order it connected; each screen is then placed against the one before it by its own setting —
/// to the right by default, which is what Slipstream did before there was any say in it.
///
/// The result is shifted so nothing sits at a negative coordinate: a screen placed left of or
/// above the panel moves everything else along rather than putting the desktop off the top left.
pub fn arrange(screens: &[Placing], panel: impl Fn(&str) -> bool) -> Vec<(String, i32, i32)> {
    use slipstream_config::{Align, Position};
    let (panels, others): (Vec<_>, Vec<_>) = screens.iter().partition(|s| panel(&s.name));
    let order: Vec<&Placing> = panels.into_iter().chain(others).collect();
    let mut placed: Vec<(String, i32, i32, i32, i32)> = Vec::new();
    for screen in order {
        let Some((px, py, pw, ph)) = placed.last().map(|(_, x, y, w, h)| (*x, *y, *w, *h)) else {
            placed.push((screen.name.clone(), 0, 0, screen.width, screen.height));
            continue;
        };
        let (w, h) = (screen.width, screen.height);
        // Along the axis the screens are stacked on, they touch; across it, the setting says
        // which edges line up.
        let across = |mine: i32, theirs: i32| match screen.place.align {
            Align::Start => 0,
            Align::Centre => (theirs - mine) / 2,
            Align::End => theirs - mine,
        };
        let (x, y) = match screen.place.position {
            Position::RightOf => (px + pw, py + across(h, ph)),
            Position::LeftOf => (px - w, py + across(h, ph)),
            Position::Above => (px + across(w, pw), py - h),
            Position::Below => (px + across(w, pw), py + ph),
        };
        placed.push((screen.name.clone(), x, y, w, h));
    }
    let left = placed.iter().map(|(_, x, ..)| *x).min().unwrap_or(0);
    let top = placed.iter().map(|(_, _, y, ..)| *y).min().unwrap_or(0);
    placed
        .into_iter()
        .map(|(name, x, y, _, _)| (name, x - left, y - top))
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
pub(crate) fn deactivate(window: &Window) {
    window.set_activated(false);
    if let Some(toplevel) = window.toplevel()
        && toplevel.is_initial_configure_sent()
    {
        toplevel.send_pending_configure();
    }
}

/// Whether `window` is `parent`'s own: an xdg toplevel whose parent is its surface, or an X11
/// window transient for it.
pub(crate) fn is_child_of(window: &Window, parent: &Window) -> bool {
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
    /// Whether the client may be an input method or a virtual keyboard (`ime::client_may_type`):
    /// likewise, any client proven to be outside a sandbox.
    pub may_type: bool,
    /// Whether the client may draw layer surfaces (`layers::client_may_draw`): likewise.
    pub may_draw_layers: bool,
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
            may_draw_layers: crate::layers::client_may_draw(peer.as_ref()),
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

    /// Screens to place, each `(name, width, height)`, all following the one before them to the
    /// right with their tops level — the default when nothing has been arranged by hand.
    fn to_place(list: &[(&str, i32, i32)]) -> Vec<Placing> {
        list.iter()
            .map(|(name, w, h)| Placing {
                name: name.to_string(),
                width: *w,
                height: *h,
                place: slipstream_config::ScreenPlace {
                    monitor: name.to_string(),
                    ..Default::default()
                },
            })
            .collect()
    }

    fn at(list: &[(&str, i32, i32)]) -> Vec<(String, i32, i32)> {
        list.iter()
            .map(|(name, x, y)| (name.to_string(), *x, *y))
            .collect()
    }

    #[test]
    fn internal_panels_go_first() {
        // The monitor lit first, or the panel came back when the lid opened.
        let places = arrange(
            &to_place(&[("HDMI-A-1", 1920, 1080), ("eDP-1", 1536, 960)]),
            is_internal_panel,
        );
        assert_eq!(places, at(&[("eDP-1", 0, 0), ("HDMI-A-1", 1536, 0)]));
        // The panel goes out and comes back: still first.
        let without = arrange(&to_place(&[("HDMI-A-1", 1920, 1080)]), is_internal_panel);
        assert_eq!(without, at(&[("HDMI-A-1", 0, 0)]));
        let back = arrange(
            &to_place(&[("HDMI-A-1", 1920, 1080), ("eDP-1", 1536, 960)]),
            is_internal_panel,
        );
        assert_eq!(back, places);
        // Other screens keep the order they connected in.
        let three = arrange(
            &to_place(&[
                ("DP-3", 2560, 1440),
                ("eDP-1", 1536, 960),
                ("HDMI-A-1", 1920, 1080),
            ]),
            is_internal_panel,
        );
        assert_eq!(
            three,
            at(&[("eDP-1", 0, 0), ("DP-3", 1536, 0), ("HDMI-A-1", 4096, 0)])
        );
    }

    /// `list` is `(name, width, height, position, align)`, the panel first.
    fn arranged(
        list: &[(
            &str,
            i32,
            i32,
            slipstream_config::Position,
            slipstream_config::Align,
        )],
    ) -> Vec<(String, i32, i32)> {
        let screens: Vec<Placing> = list
            .iter()
            .map(|(name, w, h, position, align)| Placing {
                name: name.to_string(),
                width: *w,
                height: *h,
                place: slipstream_config::ScreenPlace {
                    monitor: name.to_string(),
                    position: *position,
                    align: *align,
                    ..Default::default()
                },
            })
            .collect();
        arrange(&screens, is_internal_panel)
    }

    #[test]
    fn a_screen_can_be_put_above_the_panel() {
        use slipstream_config::{Align, Position};
        // A monitor on a stand behind the laptop: the desktop shifts down so nothing is off the
        // top of the layout, and the panel ends up below it.
        let places = arranged(&[
            ("eDP-1", 1536, 960, Position::RightOf, Align::Start),
            ("DP-2", 1920, 1080, Position::Above, Align::Start),
        ]);
        assert_eq!(places, at(&[("eDP-1", 0, 1080), ("DP-2", 0, 0)]));
    }

    #[test]
    fn a_screen_can_be_put_left_of_the_panel() {
        use slipstream_config::{Align, Position};
        let places = arranged(&[
            ("eDP-1", 1536, 960, Position::RightOf, Align::Start),
            ("DP-2", 1920, 1080, Position::LeftOf, Align::Start),
        ]);
        // Nothing sits at a negative coordinate: the monitor takes x 0 and the panel follows it.
        assert_eq!(places, at(&[("eDP-1", 1920, 0), ("DP-2", 0, 0)]));
    }

    #[test]
    fn screens_beside_each_other_line_up_by_the_edges_asked_for() {
        use slipstream_config::{Align, Position};
        let tops = arranged(&[
            ("eDP-1", 1536, 960, Position::RightOf, Align::Start),
            ("DP-2", 1920, 1080, Position::RightOf, Align::Start),
        ]);
        assert_eq!(tops, at(&[("eDP-1", 0, 0), ("DP-2", 1536, 0)]));
        // Middles level: the taller monitor starts 60 above the panel, so the layout shifts down
        // by 60 and the panel sits there instead. Both middles land on 540.
        let middles = arranged(&[
            ("eDP-1", 1536, 960, Position::RightOf, Align::Start),
            ("DP-2", 1920, 1080, Position::RightOf, Align::Centre),
        ]);
        assert_eq!(middles, at(&[("eDP-1", 0, 60), ("DP-2", 1536, 0)]));
        assert_eq!(60 + 960 / 2, 1080 / 2, "the middles really are level");
        let bottoms = arranged(&[
            ("eDP-1", 1536, 960, Position::RightOf, Align::Start),
            ("DP-2", 1920, 1080, Position::RightOf, Align::End),
        ]);
        // Bottoms level: the taller screen starts above the panel, so everything shifts down.
        assert_eq!(bottoms, at(&[("eDP-1", 0, 120), ("DP-2", 1536, 0)]));
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

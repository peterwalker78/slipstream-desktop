//! Compositor key bindings.
//!
//! The defaults and the reserved list follow one rule: a compositor binding is global and reaches
//! us before the focused app, so anything the kernel, input methods, screen readers, games or
//! ordinary apps rely on must never be bound.

use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

use slipstream_config::KeyStyle;

use crate::layout::{Direction, Resize};
use crate::zoom::Step;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open the way out: lock, log out, restart or shut down, chosen on a card.
    WayOut,
    /// Move keyboard focus to the neighbouring tiled window.
    Focus(Direction),
    /// Swap the focused window with the tiled window in that direction, so a tile can be moved
    /// around the workspace from the keyboard.
    MoveTile(Direction),
    /// Move the split beside the focused tile: wider or narrower, taller or shorter.
    Resize(Resize),
    /// Turn the layout a quarter, clockwise or anticlockwise.
    Rotate { clockwise: bool },
    /// Go to a workspace by its number; the number keys reach 1–9.
    Workspace(u8),
    /// Go to the previous (−1) or next (+1) workspace.
    WorkspaceBy(i8),
    /// Move the focused window to a workspace by its number (1–9 by key) and follow it.
    MoveToWorkspace(u8),
    /// Move the focused window to the previous or next workspace and follow it.
    MoveToWorkspaceBy(i8),
    /// Ask the focused window to close.
    Close,
    /// Ask every window of the focused window's app to close.
    Quit,
    /// Copy, paste or cut in the focused app, by sending it its own keys for that.
    Edit(Edit),
    /// Start an app.
    Launch(App),
    /// Go to virtual terminal 1–12. Never a binding: Ctrl+Alt+F1–F12 produce it through the
    /// keyboard layout, on real hardware only.
    SwitchVt(i32),
    /// Volume up or down by this many percent.
    Volume(i8),
    /// Mute or unmute the speakers, or the microphone.
    ToggleMute { microphone: bool },
    /// Screen brightness up or down by this many percent.
    Brightness(i8),
    /// Alt+Tab: focus the next (or, with Shift, the previous) most recently used window.
    CycleWindows { forward: bool },
    /// Open or close the app explorer.
    Explorer,
    /// Gravity on, at the arrangement last kept, or back to tiling; held, the arrangements side by
    /// side.
    ToggleGravity,
    /// Minimise the focused window into the code rain.
    Minimise,
    /// Bring back the most recently minimised window.
    Restore,
    /// Put every window on this workspace into the code rain; again, bring those same windows
    /// back. Windows' own "minimise all", which Super+M only does one window at a time.
    HideAll,
    /// Send the focused window up to the bar, where it hangs in front of every workspace; on
    /// the window already there, its next size.
    Dock,
    /// Bring the docked window down from the bar, a standard window again.
    Undock,
    /// Open bullet time, the overview of every workspace, or go back out of it.
    BulletTime,
    /// Open or close quick settings.
    QuickSettings,
    /// Open or close the notification centre.
    NotificationCentre,
    /// Move the keyboard to the next screen along.
    NextScreen,
    /// Send the focused window to the next screen along; the keyboard stays where it is.
    MoveToNextScreen,
    /// Trade workspaces with the next screen along.
    SwapScreens,
    /// The focused tiled window fills the tiling area above its neighbours, or goes back.
    Maximise,
    /// Save the focused screen, or the focused window, as a PNG and copy it.
    Screenshot { window: bool },
    /// Go to an empty workspace, or back from it.
    ShowDesktop,
    /// Open or close the shortcut sheet: every key, listed.
    ShortcutSheet,
    /// Lock the screen.
    Lock,
    /// A media key: to whichever player is playing.
    Media(crate::media::Transport),
    /// Take the pointer and the keyboard shortcuts back from the app holding them.
    TakeBack,
    /// Magnify the screen about the pointer, step by step, or stop.
    Zoom(Step),
    /// Open or close the clipboard history.
    ClipboardHistory,
    /// Float the focused window, or put it back into the tiling.
    ToggleFloating,
    /// Move the keyboard between the floating windows and the tiles.
    SwitchFloatingFocus,
}

/// What `Action::Edit` does in the focused app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Copy,
    Paste,
    Cut,
}

/// The shortcut sheet's groups, in the order it shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    AppsAndWindows,
    WorkspacesAndArranging,
    BulletTime,
    PanelsAndSystem,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::AppsAndWindows,
        Group::WorkspacesAndArranging,
        Group::BulletTime,
        Group::PanelsAndSystem,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::AppsAndWindows => "Apps and windows",
            Group::WorkspacesAndArranging => "Workspaces and arranging",
            Group::BulletTime => "Bullet time",
            Group::PanelsAndSystem => "Panels and system",
        }
    }
}

impl Action {
    /// What the action does, as the shortcut sheet says it. Actions that are two directions of
    /// one thing say both, so their keys share a row.
    pub fn describe(self) -> &'static str {
        match self {
            Action::WayOut => "lock, log out, restart, shut down",
            Action::Focus(_) => "move focus",
            Action::MoveTile(_) => "move the window",
            Action::Resize(Resize::Wider | Resize::Narrower) => "narrower, wider",
            Action::Resize(Resize::Taller | Resize::Shorter) => "shorter, taller",
            Action::Rotate { .. } => "turn the layout a quarter, either way",
            Action::Workspace(_) => "go to a workspace",
            Action::WorkspaceBy(_) => "previous, next workspace",
            Action::MoveToWorkspace(_) => "send window to a workspace",
            Action::MoveToWorkspaceBy(_) => "send window to previous, next",
            Action::Close => "close window",
            Action::Quit => "quit the app",
            Action::Edit(_) => "copy, paste, cut in any app",
            Action::Launch(App::Terminal) => "terminal",
            Action::Launch(App::Files) => "files",
            Action::Launch(App::Settings) => "settings",
            Action::Launch(App::Browser) => "browser",
            Action::SwitchVt(_) => "virtual terminal",
            Action::Volume(_) => "volume up, down",
            Action::ToggleMute { microphone: false } => "mute",
            Action::ToggleMute { microphone: true } => "mute the microphone",
            Action::Brightness(_) => "brightness up, down",
            Action::CycleWindows { .. } => "switch window",
            Action::Explorer => "search apps and files",
            Action::ToggleGravity => "tiling or gravity; hold to choose",
            Action::Minimise => "minimise to a stream",
            Action::Restore => "bring back the last minimised",
            Action::HideAll => "hide every window, and back",
            Action::Dock => "dock to the bar; again to resize",
            Action::Undock => "bring the docked window back down",
            Action::BulletTime => "every workspace at once",
            Action::QuickSettings => "quick settings",
            Action::NotificationCentre => "notifications",
            Action::NextScreen => "keyboard to the next screen",
            Action::MoveToNextScreen => "send window to the next screen",
            Action::SwapScreens => "swap workspaces with the next screen",
            Action::Maximise => "maximise",
            Action::Screenshot { window: false } => "screenshot",
            Action::Screenshot { window: true } => "snip a region or a window",
            Action::ShowDesktop => "an empty workspace, and back",
            Action::ShortcutSheet => "every key",
            Action::Lock => "lock the screen",
            Action::Media(_) => "play, pause, next, previous",
            Action::TakeBack => "take the mouse and keys back from an app",
            Action::Zoom(Step::In | Step::Out) => "zoom the screen out, in",
            Action::Zoom(Step::Off) => "stop zooming",
            Action::ClipboardHistory => "clipboard history",
            Action::ToggleFloating => "float the window, or tile it",
            Action::SwitchFloatingFocus => "keyboard between floating and tiled",
        }
    }

    /// Where the shortcut sheet lists it.
    pub fn group(self) -> Group {
        match self {
            Action::Focus(_)
            | Action::MoveTile(_)
            | Action::Close
            | Action::Quit
            | Action::Edit(_)
            | Action::Launch(_)
            | Action::CycleWindows { .. }
            | Action::Explorer
            | Action::Minimise
            | Action::Restore
            | Action::HideAll
            | Action::Dock
            | Action::Undock
            | Action::Maximise
            | Action::ToggleFloating
            | Action::SwitchFloatingFocus => Group::AppsAndWindows,
            Action::Resize(_)
            | Action::Rotate { .. }
            | Action::Workspace(_)
            | Action::WorkspaceBy(_)
            | Action::MoveToWorkspace(_)
            | Action::MoveToWorkspaceBy(_)
            | Action::ToggleGravity
            | Action::NextScreen
            | Action::MoveToNextScreen
            | Action::SwapScreens
            | Action::ShowDesktop => Group::WorkspacesAndArranging,
            Action::BulletTime => Group::BulletTime,
            Action::WayOut
            | Action::SwitchVt(_)
            | Action::Volume(_)
            | Action::ToggleMute { .. }
            | Action::Brightness(_)
            | Action::QuickSettings
            | Action::NotificationCentre
            | Action::Screenshot { .. }
            | Action::ShortcutSheet
            | Action::Lock
            | Action::Media(_)
            | Action::TakeBack
            | Action::Zoom(_)
            | Action::ClipboardHistory => Group::PanelsAndSystem,
        }
    }
}

/// A key's name as people write it: `Esc`, `PgUp`, `←`, `]`, `Volume Up`, `A`.
pub fn key_name(key: Keysym) -> String {
    let name = match key {
        Keysym::Escape => "Esc",
        Keysym::Return | Keysym::KP_Enter => "Enter",
        Keysym::space => "Space",
        Keysym::Tab | Keysym::ISO_Left_Tab => "Tab",
        Keysym::Left => "←",
        Keysym::Right => "→",
        Keysym::Up => "↑",
        Keysym::Down => "↓",
        Keysym::Prior => "PgUp",
        Keysym::Next => "PgDn",
        Keysym::Delete => "Del",
        Keysym::BackSpace => "Backspace",
        Keysym::Print => "Print",
        Keysym::slash => "/",
        Keysym::bracketleft | Keysym::braceleft => "[",
        Keysym::bracketright | Keysym::braceright => "]",
        Keysym::minus | Keysym::underscore => "-",
        Keysym::equal | Keysym::plus => "=",
        Keysym::XF86_AudioRaiseVolume => "Volume Up",
        Keysym::XF86_AudioLowerVolume => "Volume Down",
        Keysym::XF86_AudioMute => "Mute",
        Keysym::XF86_AudioMicMute => "Mic Mute",
        Keysym::XF86_AudioPlay => "Play",
        Keysym::XF86_AudioPause => "Pause",
        Keysym::XF86_AudioNext => "Next Track",
        Keysym::XF86_AudioPrev => "Previous Track",
        Keysym::XF86_AudioStop => "Stop",
        Keysym::XF86_MonBrightnessUp => "Brightness Up",
        Keysym::XF86_MonBrightnessDown => "Brightness Down",
        _ => {
            let typed = xkb::keysym_to_utf8(key);
            let mut chars = typed.chars().filter(|ch| !ch.is_control());
            return match (chars.next(), chars.next()) {
                (Some(ch), None) => ch.to_uppercase().collect(),
                _ => xkb::keysym_get_name(key),
            };
        }
    };
    name.to_string()
}

/// The modifiers as a prefix, `Super+Ctrl+Alt+Shift+`, in the order people say them.
pub fn mods_prefix(mods: Mods) -> String {
    let mut prefix = String::new();
    for (held, name) in [
        (mods.logo, "Super+"),
        (mods.ctrl, "Ctrl+"),
        (mods.alt, "Alt+"),
        (mods.shift, "Shift+"),
    ] {
        if held {
            prefix.push_str(name);
        }
    }
    prefix
}

/// The first binding for `action` in `bindings`.
pub fn binding_for(bindings: &[Binding], action: Action) -> Option<Binding> {
    bindings.iter().find(|b| b.action == action).copied()
}

/// A key combination for prose, toasts and keycaps: `Super+Shift+S`.
pub fn label(mods: Mods, key: Keysym) -> String {
    mods_prefix(mods) + &key_name(key)
}

/// Apps with their own keys, Windows-style. `launch.rs` picks whichever is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Terminal,
    /// Super+E, as on Windows.
    Files,
    /// Super+I, as on Windows: Slipstream's own settings app.
    Settings,
    /// Super+B: whichever browser the desktop is set to open a web page with.
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Slipstream's Mod key: Super on real hardware (see `from_state`).
    pub logo: bool,
}

impl Mods {
    /// Nested inside another desktop, that desktop keeps Super for itself (KDE binds
    /// Super+Shift+Esc, for one), so Alt stands in for Super.
    pub fn from_state(m: &ModifiersState, nested: bool) -> Self {
        let mut mods = Self {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            logo: m.logo,
        };
        if nested && mods.alt && !mods.logo {
            mods.alt = false;
            mods.logo = true;
        }
        mods
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub mods: Mods,
    pub key: Keysym,
    pub action: Action,
}

const fn mods(logo: bool, ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        logo,
    }
}

const NONE: Mods = mods(false, false, false, false);
const ALT: Mods = mods(false, false, true, false);
const ALT_SHIFT: Mods = mods(false, false, true, true);
const CTRL_ALT: Mods = mods(false, true, true, false);
const MOD: Mods = mods(true, false, false, false);
const MOD_SHIFT: Mods = mods(true, false, false, true);
const MOD_CTRL: Mods = mods(true, true, false, false);
const MOD_ALT: Mods = mods(true, false, true, false);
const MOD_ALT_SHIFT: Mods = mods(true, false, true, true);
const MOD_CTRL_ALT: Mods = mods(true, true, true, false);

fn bind(mods: Mods, key: Keysym, action: Action) -> Binding {
    Binding { mods, key, action }
}

/// The four arrows, each with the direction it names.
const ARROWS: [(Keysym, Direction); 4] = [
    (Keysym::Left, Direction::Left),
    (Keysym::Right, Direction::Right),
    (Keysym::Up, Direction::Up),
    (Keysym::Down, Direction::Down),
];

/// The bindings for `style`. The Mac and Omarchy styles list their own keys before the ones
/// every style shares, so that where an action has both, theirs is named first. The Windows style
/// is where the shared keys come from, so they sit among its own.
pub fn defaults(style: KeyStyle) -> Vec<Binding> {
    match style {
        KeyStyle::Windows => windows(),
        KeyStyle::Mac => [mac(), shared()].concat(),
        KeyStyle::Omarchy => [omarchy(), shared()].concat(),
    }
}

/// Whether a tapped Super opens the explorer, as the Windows key opens Start. A Mac's ⌘ and
/// Omarchy's Super do nothing on their own, and both are held so often that a stray tap would.
pub fn taps_super(style: KeyStyle) -> bool {
    style == KeyStyle::Windows
}

/// Keys every style has: the way out, Alt+F4, the task switcher, the terminal, the arrows and
/// the laptop's own keys.
fn shared() -> Vec<Binding> {
    let mut bindings = vec![way_out()];
    bindings.extend(focus());
    bindings.extend(switching());
    bindings.extend(hardware());
    bindings
}

/// Windows' own security screen, on the chord every Windows user already reaches for under
/// stress. The kernel never sees it: libinput hands it to us first.
fn way_out() -> Binding {
    bind(CTRL_ALT, Keysym::Delete, Action::WayOut)
}

/// Super+arrows move the focus to the neighbouring tile.
fn focus() -> impl Iterator<Item = Binding> {
    ARROWS
        .into_iter()
        .map(|(key, direction)| bind(MOD, key, Action::Focus(direction)))
}

/// Alt+F4, the task switcher and the terminal.
fn switching() -> Vec<Binding> {
    vec![
        // On real hardware only: nested inside KDE, KWin takes Alt+F4 for its own window.
        bind(ALT, Keysym::F4, Action::Close),
        // Letting go of Alt settles on the window.
        bind(ALT, Keysym::Tab, Action::CycleWindows { forward: true }),
        bind(
            ALT_SHIFT,
            Keysym::Tab,
            Action::CycleWindows { forward: false },
        ),
        bind(
            ALT_SHIFT,
            Keysym::ISO_Left_Tab,
            Action::CycleWindows { forward: false },
        ),
        bind(MOD, Keysym::Return, Action::Launch(App::Terminal)),
    ]
}

/// The media, volume and brightness keys (the Fn row on laptops), which need no modifier.
fn hardware() -> Vec<Binding> {
    vec![
        bind(
            NONE,
            Keysym::XF86_AudioPlay,
            Action::Media(crate::media::Transport::PlayPause),
        ),
        bind(
            NONE,
            Keysym::XF86_AudioPause,
            Action::Media(crate::media::Transport::PlayPause),
        ),
        bind(
            NONE,
            Keysym::XF86_AudioNext,
            Action::Media(crate::media::Transport::Next),
        ),
        bind(
            NONE,
            Keysym::XF86_AudioPrev,
            Action::Media(crate::media::Transport::Previous),
        ),
        bind(
            NONE,
            Keysym::XF86_AudioStop,
            Action::Media(crate::media::Transport::Stop),
        ),
        bind(NONE, Keysym::XF86_AudioRaiseVolume, Action::Volume(5)),
        bind(NONE, Keysym::XF86_AudioLowerVolume, Action::Volume(-5)),
        bind(
            NONE,
            Keysym::XF86_AudioMute,
            Action::ToggleMute { microphone: false },
        ),
        bind(
            NONE,
            Keysym::XF86_AudioMicMute,
            Action::ToggleMute { microphone: true },
        ),
        bind(NONE, Keysym::XF86_MonBrightnessUp, Action::Brightness(5)),
        bind(NONE, Keysym::XF86_MonBrightnessDown, Action::Brightness(-5)),
    ]
}

/// The tile swapping places with its neighbour, on `mods` and the arrows.
fn move_tile(mods: Mods) -> impl Iterator<Item = Binding> {
    ARROWS
        .into_iter()
        .map(move |(key, direction)| bind(mods, key, Action::MoveTile(direction)))
}

/// Width on `narrower` and `wider`, height on the same keys with Shift. Keys are matched on the
/// raw symbol, so Shift usually keeps them as they are; the shifted symbols a layout may report
/// instead are bound too.
fn resize(narrower: [Keysym; 2], wider: [Keysym; 2]) -> Vec<Binding> {
    let mut bindings = vec![
        bind(MOD, narrower[0], Action::Resize(Resize::Narrower)),
        bind(MOD, wider[0], Action::Resize(Resize::Wider)),
    ];
    for key in narrower {
        bindings.push(bind(MOD_SHIFT, key, Action::Resize(Resize::Shorter)));
    }
    for key in wider {
        bindings.push(bind(MOD_SHIFT, key, Action::Resize(Resize::Taller)));
    }
    bindings
}

/// The number keys: `go` to a workspace, `send` the focused window there. Nine keys reach the
/// first nine workspaces; past that, bullet time and the previous/next keys do.
fn workspace_numbers(go: Mods, send: Mods) -> impl Iterator<Item = Binding> {
    DIGITS.into_iter().enumerate().flat_map(move |(i, key)| {
        let n = i as u8 + 1;
        [
            bind(go, key, Action::Workspace(n)),
            bind(send, key, Action::MoveToWorkspace(n)),
        ]
    })
}

/// Zooming out and in on `mods` with - and =. Plus is bound as well as =, for the layouts that
/// give it a key of its own.
fn zoom(mods: Mods) -> [Binding; 3] {
    [
        bind(mods, Keysym::minus, Action::Zoom(Step::Out)),
        bind(mods, Keysym::equal, Action::Zoom(Step::In)),
        bind(mods, Keysym::plus, Action::Zoom(Step::In)),
    ]
}

/// Super with the wheel zooms the screen, in every style: up closer, down back out.
pub fn zooms_with_wheel(mods: Mods) -> bool {
    mods == MOD
}

/// Windows' keys, wherever Windows has one for the job.
fn windows() -> Vec<Binding> {
    let mut bindings = vec![
        way_out(),
        // A game holding the pointer, or a virtual machine holding every key, gives them back.
        bind(MOD, Keysym::Escape, Action::TakeBack),
    ];
    bindings.extend(focus());
    // Add Alt and the window comes with the focus: the tile swaps places with its neighbour.
    bindings.extend(move_tile(MOD_ALT));
    bindings.extend(resize(
        [Keysym::bracketleft, Keysym::braceleft],
        [Keysym::bracketright, Keysym::braceright],
    ));
    bindings.extend([
        // Windows' virtual desktop keys.
        bind(MOD_CTRL, Keysym::Left, Action::WorkspaceBy(-1)),
        bind(MOD_CTRL, Keysym::Right, Action::WorkspaceBy(1)),
        bind(MOD_SHIFT, Keysym::Left, Action::MoveToWorkspaceBy(-1)),
        bind(MOD_SHIFT, Keysym::Right, Action::MoveToWorkspaceBy(1)),
    ]);
    bindings.extend(switching());
    bindings.extend([
        bind(MOD, Keysym::e, Action::Launch(App::Files)),
        bind(MOD, Keysym::i, Action::Launch(App::Settings)),
        bind(MOD, Keysym::b, Action::Launch(App::Browser)),
        // A tapped Super opens the explorer, as the Windows key opens Start (see `SuperTap`), and
        // Win+S opens the same search box. Super+Space stays IBus's, to switch input methods, as
        // Win+Space switches layouts on Windows.
        bind(MOD, Keysym::s, Action::Explorer),
        // Windows' own display key: Win+P is where a Windows user looks for anything to do with
        // screens. With one screen it says there's nowhere to go.
        bind(MOD, Keysym::p, Action::NextScreen),
        bind(MOD_SHIFT, Keysym::p, Action::MoveToNextScreen),
        bind(MOD_CTRL, Keysym::p, Action::SwapScreens),
        // Windows' quick settings key.
        bind(MOD, Keysym::a, Action::QuickSettings),
        // Windows' screenshot keys: Print and Win+Print for the screen, Win+Shift+S to snip a
        // region or a window (gamescope keeps it while focused, as it does Super+S).
        bind(NONE, Keysym::Print, Action::Screenshot { window: false }),
        bind(MOD, Keysym::Print, Action::Screenshot { window: false }),
        bind(MOD_SHIFT, Keysym::s, Action::Screenshot { window: true }),
        // Windows' show-desktop key: an empty workspace, and back.
        bind(MOD, Keysym::d, Action::ShowDesktop),
        bind(MOD, Keysym::l, Action::Lock),
        // Win+X is where Windows keeps shutting down and signing out.
        bind(MOD, Keysym::x, Action::WayOut),
        // Windows' clipboard history key.
        bind(MOD, Keysym::v, Action::ClipboardHistory),
        // Floating: Super+V is the clipboard history, so these take the V with a modifier.
        bind(MOD_SHIFT, Keysym::v, Action::ToggleFloating),
        bind(MOD_CTRL, Keysym::v, Action::SwitchFloatingFocus),
        // Windows' notification centre key.
        bind(MOD, Keysym::n, Action::NotificationCentre),
        // Every key, one keystroke away.
        bind(MOD, Keysym::slash, Action::ShortcutSheet),
        bind(MOD, Keysym::t, Action::ToggleGravity),
        // Turning the layout, which Windows has no key for.
        bind(MOD, Keysym::r, Action::Rotate { clockwise: true }),
        bind(MOD_SHIFT, Keysym::r, Action::Rotate { clockwise: false }),
        // Fill the tiling area, keeping the neighbours' tiles underneath. gamescope keeps Super+F
        // while it's focused (`passes_to_gamescope`).
        bind(MOD, Keysym::f, Action::Maximise),
        // Minimise into the code rain on M: Super+↓, Windows' own minimise key, moves focus here.
        bind(MOD, Keysym::m, Action::Minimise),
        // Windows' Task View key.
        bind(MOD, Keysym::Tab, Action::BulletTime),
        // Bring back the last window minimised.
        bind(MOD_SHIFT, Keysym::m, Action::Restore),
        // Windows' own minimise-all key is Super+M, which is one window here, as its restore
        // key Super+Shift+M is. Super+H is free, and next to them on the keyboard.
        bind(MOD, Keysym::h, Action::HideAll),
        // A window docked to the bar as a pane of glass, and back with Shift as Super+M's
        // restore is. W is free, beside the other window keys, and nothing a game or an input
        // method relies on.
        bind(MOD, Keysym::w, Action::Dock),
        bind(MOD_SHIFT, Keysym::w, Action::Undock),
        // Windows' Magnifier stops on Win+Esc, which takes the keys back here, so 0 stops
        // zooming, as Ctrl+0 puts an app's own zoom back to its actual size.
        bind(MOD, Keysym::_0, Action::Zoom(Step::Off)),
    ]);
    // Magnifier's Win+Plus and Win+Minus.
    bindings.extend(zoom(MOD));
    bindings.extend(hardware());
    bindings.extend(workspace_numbers(MOD, MOD_SHIFT));
    bindings
}

/// A Mac's keys, with Super standing in for ⌘: it is the system's own key on a PC keyboard, and
/// Alt, which sits where ⌘ does, belongs to apps' menus. Apps keep their own Ctrl keys. A Mac's
/// Control+arrow keys move between Spaces; on Linux Ctrl+arrows move by word in every app, so
/// here they take Super as well.
fn mac() -> Vec<Binding> {
    let mut bindings = vec![
        // Spotlight. Ctrl+Space, a Mac's input source key, stays the input methods'.
        bind(MOD, Keysym::space, Action::Explorer),
        // ⌘Tab, beside Alt+Tab; letting go of Super settles on the window.
        bind(MOD, Keysym::Tab, Action::CycleWindows { forward: true }),
        bind(
            MOD_SHIFT,
            Keysym::Tab,
            Action::CycleWindows { forward: false },
        ),
        bind(
            MOD_SHIFT,
            Keysym::ISO_Left_Tab,
            Action::CycleWindows { forward: false },
        ),
        // ⌘W closes the window, ⌘Q quits the app.
        bind(MOD, Keysym::w, Action::Close),
        bind(MOD, Keysym::q, Action::Quit),
        // ⌘M minimises; ⌘H hides, which for a window here is the same thing; ⌥⌘M minimises
        // every window.
        bind(MOD, Keysym::m, Action::Minimise),
        bind(MOD, Keysym::h, Action::Minimise),
        bind(MOD_SHIFT, Keysym::m, Action::Restore),
        bind(MOD_ALT, Keysym::m, Action::HideAll),
        // Control-⌘-F: full screen.
        bind(MOD_CTRL, Keysym::f, Action::Maximise),
        // Mission Control, and moving between Spaces.
        bind(MOD_CTRL, Keysym::Up, Action::BulletTime),
        bind(MOD_CTRL, Keysym::Left, Action::WorkspaceBy(-1)),
        bind(MOD_CTRL, Keysym::Right, Action::WorkspaceBy(1)),
        bind(MOD_SHIFT, Keysym::Left, Action::MoveToWorkspaceBy(-1)),
        bind(MOD_SHIFT, Keysym::Right, Action::MoveToWorkspaceBy(1)),
        // Shift-⌘-3 for the screen, 4 and 5 to choose a region or a window. A PC keyboard has
        // Print, so that works too.
        bind(MOD_SHIFT, Keysym::_3, Action::Screenshot { window: false }),
        bind(MOD_SHIFT, Keysym::_4, Action::Screenshot { window: true }),
        bind(MOD_SHIFT, Keysym::_5, Action::Screenshot { window: true }),
        bind(NONE, Keysym::Print, Action::Screenshot { window: false }),
        // Control-⌘-Q locks; Shift-⌘-Q asks before logging out, as the way out does.
        bind(MOD_CTRL, Keysym::q, Action::Lock),
        bind(MOD_SHIFT, Keysym::q, Action::WayOut),
        // ⌘, opens an app's settings; here, the desktop's.
        bind(MOD, Keysym::comma, Action::Launch(App::Settings)),
        // ⌥⌘D shows the Dock; here it docks the window to the bar.
        bind(MOD_ALT, Keysym::d, Action::Dock),
        bind(MOD_ALT_SHIFT, Keysym::d, Action::Undock),
        // A Mac has no keys for these, so they are the same as in the Windows style.
        bind(MOD, Keysym::Escape, Action::TakeBack),
        bind(MOD, Keysym::e, Action::Launch(App::Files)),
        bind(MOD, Keysym::b, Action::Launch(App::Browser)),
        bind(MOD, Keysym::p, Action::NextScreen),
        bind(MOD_SHIFT, Keysym::p, Action::MoveToNextScreen),
        bind(MOD_CTRL, Keysym::p, Action::SwapScreens),
        bind(MOD, Keysym::a, Action::QuickSettings),
        bind(MOD, Keysym::n, Action::NotificationCentre),
        bind(MOD, Keysym::d, Action::ShowDesktop),
        bind(MOD, Keysym::v, Action::ClipboardHistory),
        bind(MOD_SHIFT, Keysym::v, Action::ToggleFloating),
        bind(MOD_CTRL, Keysym::v, Action::SwitchFloatingFocus),
        bind(MOD, Keysym::slash, Action::ShortcutSheet),
        bind(MOD, Keysym::t, Action::ToggleGravity),
        bind(MOD, Keysym::r, Action::Rotate { clockwise: true }),
        bind(MOD_SHIFT, Keysym::r, Action::Rotate { clockwise: false }),
        // A Mac turns zoom off on ⌥⌘8, which sends a window to workspace 8 here, so ⌥⌘0 does:
        // ⌘0 puts an app back to its actual size.
        bind(MOD_ALT, Keysym::_0, Action::Zoom(Step::Off)),
    ];
    // ⌥⌘= and ⌥⌘-, a Mac's zoom keys.
    bindings.extend(zoom(MOD_ALT));
    bindings.extend(move_tile(MOD_ALT));
    bindings.extend(resize(
        [Keysym::bracketleft, Keysym::braceleft],
        [Keysym::bracketright, Keysym::braceright],
    ));
    // Shift-⌘ and a number is a screenshot, so Alt sends the window, as it moves a tile.
    bindings.extend(workspace_numbers(MOD, MOD_ALT));
    bindings
}

/// Omarchy's keys.
fn omarchy() -> Vec<Binding> {
    let mut bindings = vec![
        // Its launcher. Its input methods switch on Ctrl+Space, which stays theirs.
        bind(MOD, Keysym::space, Action::Explorer),
        bind(MOD, Keysym::w, Action::Close),
        bind(MOD_SHIFT, Keysym::Return, Action::Launch(App::Browser)),
        bind(MOD_SHIFT, Keysym::b, Action::Launch(App::Browser)),
        bind(MOD_SHIFT, Keysym::f, Action::Launch(App::Files)),
        bind(MOD, Keysym::t, Action::ToggleFloating),
        bind(MOD_SHIFT, Keysym::t, Action::SwitchFloatingFocus),
        bind(MOD, Keysym::f, Action::Maximise),
        // Its workspace layout key, and its split toggle.
        bind(MOD, Keysym::l, Action::ToggleGravity),
        bind(MOD, Keysym::j, Action::Rotate { clockwise: true }),
        bind(MOD_SHIFT, Keysym::j, Action::Rotate { clockwise: false }),
        // Its pop-out key floats a window and pins it over every workspace: here, the dock.
        bind(MOD, Keysym::o, Action::Dock),
        bind(MOD_SHIFT, Keysym::o, Action::Undock),
        bind(MOD, Keysym::Tab, Action::WorkspaceBy(1)),
        bind(MOD_SHIFT, Keysym::Tab, Action::WorkspaceBy(-1)),
        bind(MOD_SHIFT, Keysym::ISO_Left_Tab, Action::WorkspaceBy(-1)),
        // Its system menu, and the lock.
        bind(MOD, Keysym::Escape, Action::WayOut),
        bind(MOD_CTRL, Keysym::l, Action::Lock),
        bind(MOD, Keysym::k, Action::ShortcutSheet),
        bind(MOD_CTRL, Keysym::v, Action::ClipboardHistory),
        // Its copy, paste and cut, for every app, terminals included.
        bind(MOD, Keysym::c, Action::Edit(Edit::Copy)),
        bind(MOD, Keysym::v, Action::Edit(Edit::Paste)),
        bind(MOD, Keysym::x, Action::Edit(Edit::Cut)),
        // Its audio panel's key: volume heads quick settings.
        bind(MOD_CTRL, Keysym::a, Action::QuickSettings),
        // Its notification history key.
        bind(MOD_ALT_SHIFT, Keysym::comma, Action::NotificationCentre),
        // Print chooses a region or a window, and Enter takes the whole screen.
        bind(NONE, Keysym::Print, Action::Screenshot { window: true }),
        bind(MOD_SHIFT, Keysym::s, Action::Screenshot { window: true }),
        // Omarchy has no keys for these, so they sit where its own keys don't.
        bind(MOD_SHIFT, Keysym::Escape, Action::TakeBack),
        bind(MOD_CTRL, Keysym::Up, Action::BulletTime),
        bind(MOD, Keysym::i, Action::Launch(App::Settings)),
        bind(MOD, Keysym::p, Action::NextScreen),
        bind(MOD_SHIFT, Keysym::p, Action::MoveToNextScreen),
        bind(MOD_CTRL, Keysym::p, Action::SwapScreens),
        bind(MOD, Keysym::m, Action::Minimise),
        bind(MOD_SHIFT, Keysym::m, Action::Restore),
        bind(MOD, Keysym::h, Action::HideAll),
        bind(MOD, Keysym::d, Action::ShowDesktop),
        // Its zoom key, and the one that puts the zoom back.
        bind(MOD_CTRL, Keysym::z, Action::Zoom(Step::In)),
        bind(MOD_CTRL_ALT, Keysym::z, Action::Zoom(Step::Off)),
    ];
    // Super with = and - resizes, so zooming out, which Omarchy has no key for, takes Ctrl too.
    bindings.extend(zoom(MOD_CTRL));
    // Shift and an arrow swaps the tile with its neighbour.
    bindings.extend(move_tile(MOD_SHIFT));
    bindings.extend(resize(
        [Keysym::minus, Keysym::underscore],
        [Keysym::equal, Keysym::plus],
    ));
    bindings.extend(workspace_numbers(MOD, MOD_SHIFT));
    bindings
}

/// How long Super can be held and still count as a tap.
const TAP_MS: u32 = 400;

/// Super tapped on its own, which opens the explorer as the Windows key opens Start.
///
/// Armed by Super pressed with no other key held; disarmed by any other key press (modifiers
/// included, so the Copilot key's Super+Shift+F23 never taps), a pointer button or the wheel;
/// fires when Super is let go within `TAP_MS` while still armed.
#[derive(Debug, Default)]
pub struct SuperTap {
    /// When Super went down, in milliseconds, while a tap is still possible.
    armed_at: Option<u32>,
}

impl SuperTap {
    /// A key pressed or let go at `time` (milliseconds). `alone` is whether no other key was held
    /// as it went down, and whether a tap is allowed at all. True when this completes a tap.
    pub fn key(&mut self, key: Keysym, pressed: bool, time: u32, alone: bool) -> bool {
        let is_super = key == Keysym::Super_L || key == Keysym::Super_R;
        match (is_super, pressed) {
            (true, true) => {
                self.armed_at = alone.then_some(time);
                false
            }
            (true, false) => self
                .armed_at
                .take()
                .is_some_and(|at| time.wrapping_sub(at) < TAP_MS),
            (false, true) => {
                self.armed_at = None;
                false
            }
            (false, false) => false,
        }
    }

    /// A pointer button or the wheel: whatever Super is held for, it isn't a tap.
    pub fn disarm(&mut self) {
        self.armed_at = None;
    }
}

/// Alt and Shift pressed together and let go, with nothing else pressed in between: the next
/// keyboard layout, as on Windows. Alt+Shift+Tab and Alt+Shift with any other key stay what they
/// were, since the other key disarms it. Neither key is kept from the app.
#[derive(Debug, Default)]
pub struct LayoutTap {
    armed: bool,
}

impl LayoutTap {
    /// A key pressed or let go. `others` is how many other keys were down as it was pressed, and
    /// `only_alt_and_shift` whether Alt and Shift are the only modifiers held with it counted.
    /// True when this completes the switch.
    pub fn key(
        &mut self,
        key: Keysym,
        pressed: bool,
        others: usize,
        only_alt_and_shift: bool,
    ) -> bool {
        let pair = matches!(
            key,
            Keysym::Alt_L
                | Keysym::Alt_R
                | Keysym::Meta_L
                | Keysym::Meta_R
                | Keysym::Shift_L
                | Keysym::Shift_R
        );
        match (pair, pressed) {
            (true, true) => {
                self.armed = others == 1 && only_alt_and_shift;
                false
            }
            (true, false) => std::mem::take(&mut self.armed),
            (false, true) => {
                self.armed = false;
                false
            }
            (false, false) => false,
        }
    }

    /// A pointer button or the wheel: Alt and Shift are held for a click, not a switch.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

/// The number keys 1–9, in order.
pub const DIGITS: [Keysym; 9] = [
    Keysym::_1,
    Keysym::_2,
    Keysym::_3,
    Keysym::_4,
    Keysym::_5,
    Keysym::_6,
    Keysym::_7,
    Keysym::_8,
    Keysym::_9,
];

/// The keyboard's own hardware keys: volume, brightness, media and the like, which stay the
/// desktop's even while an app holds every other key.
pub fn is_hardware_key(key: Keysym) -> bool {
    (0x1008_ff00..=0x1008_ffff).contains(&key.raw())
}

pub fn action_for(bindings: &[Binding], mods: Mods, key: Keysym) -> Option<Action> {
    bindings
        .iter()
        .find(|b| b.mods == mods && b.key == key)
        .map(|b| b.action)
}

/// Drops any binding that takes a combination `style` reserves, with a warning. Every binding
/// goes through this, so a future config file can't break input methods, screen readers or games.
pub fn checked(bindings: Vec<Binding>, style: KeyStyle) -> Vec<Binding> {
    bindings
        .into_iter()
        .filter(|b| {
            let reserved = is_reserved_in(style, b.mods, b.key);
            if reserved {
                tracing::warn!(
                    ?b,
                    "ignoring binding: reserved for input methods, screen readers or apps"
                );
            }
            !reserved
        })
        .collect()
}

const F_KEYS: [Keysym; 12] = [
    Keysym::F1,
    Keysym::F2,
    Keysym::F3,
    Keysym::F4,
    Keysym::F5,
    Keysym::F6,
    Keysym::F7,
    Keysym::F8,
    Keysym::F9,
    Keysym::F10,
    Keysym::F11,
    Keysym::F12,
];

const MODIFIER_KEYS: [Keysym; 9] = [
    Keysym::Shift_L,
    Keysym::Shift_R,
    Keysym::Control_L,
    Keysym::Control_R,
    Keysym::Alt_L,
    Keysym::Alt_R,
    Keysym::Super_L,
    Keysym::Super_R,
    Keysym::ISO_Level3_Shift,
];

/// Combinations that belong to something other than the window manager in `style`. A Mac and
/// Omarchy both open their launcher on Super+Space and switch input methods on Ctrl+Space, so
/// those styles take Super+Space; Ctrl+Space stays reserved in every style.
pub fn is_reserved_in(style: KeyStyle, mods: Mods, key: Keysym) -> bool {
    let launcher = mods == MOD && key == Keysym::space && !taps_super(style);
    is_reserved(mods, key) && !launcher
}

/// Combinations that belong to something other than the window manager.
pub fn is_reserved(mods: Mods, key: Keysym) -> bool {
    let f_key = F_KEYS.contains(&key);
    // Ctrl+Alt+F1–F12 switch virtual terminals. Ctrl+Alt+Del is not reserved: it is the chord a
    // Windows user reaches for under stress, the kernel never sees it while we hold the keyboard
    // through libinput, and leaving it unanswered is worse than answering it.
    if mods.ctrl && mods.alt && f_key {
        return true;
    }
    // Alt+SysRq is the kernel's emergency interface.
    if mods.alt && (key == Keysym::Sys_Req || key == Keysym::Print) {
        return true;
    }
    // Input-method and layout switching: Ctrl+Space, Alt+Shift, Ctrl+Shift, and IBus's
    // Super+Space, which a tapped Super already covers here.
    if (mods.ctrl || mods.logo) && key == Keysym::space {
        return true;
    }
    if MODIFIER_KEYS.contains(&key) || key == Keysym::Menu {
        return true;
    }
    // Emoji and Unicode entry (Super+. Super+; Ctrl+. Ctrl+;), and the whole Ctrl+Shift layer,
    // which terminals and IDEs use (including Ctrl+Shift+U).
    if (mods.logo || mods.ctrl) && (key == Keysym::period || key == Keysym::semicolon) {
        return true;
    }
    if mods.ctrl && mods.shift {
        return true;
    }
    // Orca: Insert and Caps Lock act as its modifier; Super+Alt+S toggles it.
    if key == Keysym::Insert || key == Keysym::Caps_Lock {
        return true;
    }
    if mods.logo && mods.alt && key == Keysym::s {
        return true;
    }
    if !mods.logo {
        // Steam overlay (Shift+Tab), Steam screenshots (F12), MangoHud (Shift+F-keys).
        // Only plain Shift+Tab: Alt+Shift+Tab is Windows' reverse task switch.
        if mods.shift
            && !mods.alt
            && !mods.ctrl
            && (key == Keysym::Tab || key == Keysym::ISO_Left_Tab)
        {
            return true;
        }
        if key == Keysym::F12 || (mods.shift && f_key) {
            return true;
        }
        // App navigation: Ctrl+Tab, Alt+←/→, Ctrl+Alt+←/→, F5, F10, F11.
        if mods.ctrl && key == Keysym::Tab {
            return true;
        }
        if mods.alt && (key == Keysym::Left || key == Keysym::Right) {
            return true;
        }
        if key == Keysym::F5 || key == Keysym::F10 || key == Keysym::F11 {
            return true;
        }
    }
    false
}

/// Super+letter keys gamescope uses for itself. While a gamescope window is focused these
/// go to it instead of running Slipstream's binding.
pub fn passes_to_gamescope(mods: Mods, key: Keysym) -> bool {
    const GAMESCOPE: [Keysym; 9] = [
        Keysym::f,
        Keysym::n,
        Keysym::b,
        Keysym::u,
        Keysym::y,
        Keysym::i,
        Keysym::o,
        Keysym::s,
        Keysym::g,
    ];
    mods.logo && !mods.ctrl && !mods.alt && GAMESCOPE.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        logo: true,
    };

    /// The Windows style's keys, which most of these tests are about.
    fn defaults() -> Vec<Binding> {
        super::defaults(KeyStyle::Windows)
    }

    fn windows_checked(bindings: Vec<Binding>) -> Vec<Binding> {
        checked(bindings, KeyStyle::Windows)
    }

    #[test]
    fn defaults_never_use_reserved_combos() {
        for style in KeyStyle::ALL {
            for b in super::defaults(style) {
                assert!(
                    !is_reserved_in(style, b.mods, b.key),
                    "{style:?}: {b:?} takes a reserved combination"
                );
            }
        }
    }

    #[test]
    fn defaults_are_unique() {
        for style in KeyStyle::ALL {
            let all = super::defaults(style);
            for (i, a) in all.iter().enumerate() {
                assert!(
                    all[i + 1..]
                        .iter()
                        .all(|b| (b.mods, b.key) != (a.mods, a.key)),
                    "{style:?}: {a:?} is bound twice"
                );
            }
        }
    }

    #[test]
    fn every_style_keeps_the_ways_out_and_the_key_list() {
        // Whatever the style, a window holding every key can be made to give them back, the
        // screen can be locked and left, and every key can be looked up.
        for style in KeyStyle::ALL {
            let bindings = checked(super::defaults(style), style);
            for action in [
                Action::TakeBack,
                Action::Lock,
                Action::WayOut,
                Action::ShortcutSheet,
                Action::Close,
                Action::Launch(App::Terminal),
                Action::BulletTime,
                Action::Restore,
                Action::ToggleFloating,
                Action::ToggleGravity,
                Action::Maximise,
                Action::QuickSettings,
                Action::ClipboardHistory,
                Action::NotificationCentre,
            ] {
                assert!(
                    binding_for(&bindings, action).is_some(),
                    "{style:?} has no key for {action:?}"
                );
            }
            let explorer = binding_for(&bindings, Action::Explorer).is_some();
            assert!(explorer || taps_super(style), "{style:?} can't open apps");
            assert_eq!(
                action_for(&bindings, CTRL_ALT, Keysym::Delete),
                Some(Action::WayOut),
                "{style:?}: Ctrl+Alt+Del is the way out everywhere"
            );
        }
    }

    #[test]
    fn every_style_has_the_keys_every_style_shares() {
        for style in KeyStyle::ALL {
            let bindings = super::defaults(style);
            for shared in shared() {
                assert!(bindings.contains(&shared), "{style:?} lacks {shared:?}");
            }
        }
    }

    #[test]
    fn the_settings_apps_highlights_are_the_styles_own_keys() {
        for style in KeyStyle::ALL {
            let bindings = checked(super::defaults(style), style);
            for (does, keys) in style.highlights() {
                let found = match *keys {
                    "Super" => taps_super(style) && *does == Action::Explorer.describe(),
                    _ => bindings
                        .iter()
                        .any(|b| label(b.mods, b.key) == *keys && b.action.describe() == *does),
                };
                assert!(found, "{style:?}: {keys} doesn't {does}");
            }
        }
    }

    #[test]
    fn only_the_windows_style_taps_super_and_the_others_take_super_space() {
        assert!(taps_super(KeyStyle::Windows));
        let windows = checked(defaults(), KeyStyle::Windows);
        assert_eq!(action_for(&windows, SUPER, Keysym::space), None);
        for style in [KeyStyle::Mac, KeyStyle::Omarchy] {
            assert!(!taps_super(style));
            let bindings = checked(super::defaults(style), style);
            assert_eq!(
                action_for(&bindings, SUPER, Keysym::space),
                Some(Action::Explorer)
            );
            // Ctrl+Space, their input method key, is never taken.
            assert!(is_reserved_in(style, MOD_CTRL, Keysym::space));
        }
    }

    #[test]
    fn mac_keys_are_where_a_mac_has_them() {
        let mac = checked(super::defaults(KeyStyle::Mac), KeyStyle::Mac);
        let at = |mods, key| action_for(&mac, mods, key);
        assert_eq!(
            at(SUPER, Keysym::Tab),
            Some(Action::CycleWindows { forward: true })
        );
        assert_eq!(at(SUPER, Keysym::q), Some(Action::Quit));
        assert_eq!(at(SUPER, Keysym::w), Some(Action::Close));
        assert_eq!(at(SUPER, Keysym::m), Some(Action::Minimise));
        assert_eq!(at(MOD_CTRL, Keysym::f), Some(Action::Maximise));
        assert_eq!(at(MOD_CTRL, Keysym::q), Some(Action::Lock));
        assert_eq!(at(MOD_CTRL, Keysym::Up), Some(Action::BulletTime));
        assert_eq!(
            at(MOD_SHIFT, Keysym::_3),
            Some(Action::Screenshot { window: false })
        );
        assert_eq!(
            at(MOD_SHIFT, Keysym::_4),
            Some(Action::Screenshot { window: true })
        );
        assert_eq!(at(MOD_ALT, Keysym::_3), Some(Action::MoveToWorkspace(3)));
        // Ctrl+arrows move by word in apps, so Spaces take Super as well.
        assert_eq!(at(mods(false, true, false, false), Keysym::Left), None);
    }

    #[test]
    fn omarchy_keys_are_where_omarchy_has_them() {
        let omarchy = checked(super::defaults(KeyStyle::Omarchy), KeyStyle::Omarchy);
        let at = |mods, key| action_for(&omarchy, mods, key);
        assert_eq!(at(SUPER, Keysym::w), Some(Action::Close));
        assert_eq!(at(SUPER, Keysym::t), Some(Action::ToggleFloating));
        assert_eq!(at(SUPER, Keysym::f), Some(Action::Maximise));
        assert_eq!(at(SUPER, Keysym::Tab), Some(Action::WorkspaceBy(1)));
        assert_eq!(at(SUPER, Keysym::Escape), Some(Action::WayOut));
        assert_eq!(at(SUPER, Keysym::k), Some(Action::ShortcutSheet));
        assert_eq!(at(MOD_CTRL, Keysym::l), Some(Action::Lock));
        assert_eq!(at(MOD_CTRL, Keysym::v), Some(Action::ClipboardHistory));
        assert_eq!(
            at(MOD_SHIFT, Keysym::Left),
            Some(Action::MoveTile(Direction::Left))
        );
        assert_eq!(at(MOD_SHIFT, Keysym::_2), Some(Action::MoveToWorkspace(2)));
        assert_eq!(
            at(MOD_SHIFT, Keysym::Return),
            Some(Action::Launch(App::Browser))
        );
        assert_eq!(at(MOD_SHIFT, Keysym::f), Some(Action::Launch(App::Files)));
        assert_eq!(
            at(SUPER, Keysym::minus),
            Some(Action::Resize(Resize::Narrower))
        );
        assert_eq!(
            at(NONE, Keysym::Print),
            Some(Action::Screenshot { window: true })
        );
        assert_eq!(at(SUPER, Keysym::c), Some(Action::Edit(Edit::Copy)));
        assert_eq!(at(SUPER, Keysym::v), Some(Action::Edit(Edit::Paste)));
        assert_eq!(at(SUPER, Keysym::x), Some(Action::Edit(Edit::Cut)));
    }

    #[test]
    fn every_style_zooms_in_out_and_off() {
        let zoom_in = Some(Action::Zoom(Step::In));
        let zoom_out = Some(Action::Zoom(Step::Out));
        let off = Some(Action::Zoom(Step::Off));
        // Windows' Win+Plus and Win+Minus; Win+Esc stays the key that takes the keys back.
        let windows = checked(defaults(), KeyStyle::Windows);
        let at = |mods, key| action_for(&windows, mods, key);
        assert_eq!(at(SUPER, Keysym::equal), zoom_in);
        assert_eq!(at(SUPER, Keysym::plus), zoom_in);
        assert_eq!(at(SUPER, Keysym::minus), zoom_out);
        assert_eq!(at(SUPER, Keysym::_0), off);
        assert_eq!(at(SUPER, Keysym::Escape), Some(Action::TakeBack));
        // A Mac's ⌥⌘= and ⌥⌘-; ⌥⌘8 still sends a window to workspace 8.
        let mac = checked(super::defaults(KeyStyle::Mac), KeyStyle::Mac);
        let at = |mods, key| action_for(&mac, mods, key);
        assert_eq!(at(MOD_ALT, Keysym::equal), zoom_in);
        assert_eq!(at(MOD_ALT, Keysym::minus), zoom_out);
        assert_eq!(at(MOD_ALT, Keysym::_0), off);
        assert_eq!(at(MOD_ALT, Keysym::_8), Some(Action::MoveToWorkspace(8)));
        // Omarchy's Super+Ctrl+Z, with Super+= and Super+- still resizing.
        let omarchy = checked(super::defaults(KeyStyle::Omarchy), KeyStyle::Omarchy);
        let at = |mods, key| action_for(&omarchy, mods, key);
        assert_eq!(at(MOD_CTRL, Keysym::z), zoom_in);
        assert_eq!(at(MOD_CTRL, Keysym::equal), zoom_in);
        assert_eq!(at(MOD_CTRL, Keysym::minus), zoom_out);
        assert_eq!(at(MOD_CTRL_ALT, Keysym::z), off);
        assert_eq!(
            at(SUPER, Keysym::equal),
            Some(Action::Resize(Resize::Wider))
        );
    }

    #[test]
    fn the_settings_app_names_each_styles_zoom_keys() {
        for style in KeyStyle::ALL {
            let bindings = checked(super::defaults(style), style);
            for (keys, step) in style
                .zoom_keys()
                .into_iter()
                .zip([Step::In, Step::Out, Step::Off])
            {
                assert!(
                    bindings
                        .iter()
                        .any(|b| label(b.mods, b.key) == keys && b.action == Action::Zoom(step)),
                    "{style:?}: {keys} doesn't zoom {step:?}"
                );
            }
        }
    }

    #[test]
    fn super_alone_with_the_wheel_zooms() {
        assert!(zooms_with_wheel(SUPER));
        assert!(!zooms_with_wheel(NONE));
        assert!(!zooms_with_wheel(MOD_CTRL));
        assert!(!zooms_with_wheel(MOD_SHIFT));
    }

    #[test]
    fn the_way_out_is_on_ctrl_alt_del_and_never_on_escape() {
        let bindings = defaults();
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        assert_eq!(
            action_for(&bindings, ctrl_alt, Keysym::Delete),
            Some(Action::WayOut)
        );
        // Nothing on Escape ends the session: Ctrl+Alt+Del is the way out from the keyboard, and
        // quick settings has the same destinations for the pointer.
        let super_shift = Mods {
            shift: true,
            ..SUPER
        };
        assert_eq!(action_for(&bindings, super_shift, Keysym::Escape), None);
        assert_eq!(action_for(&bindings, Mods::default(), Keysym::Escape), None);
        // Super+Esc on its own takes the mouse and keys back; it never logs out.
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::Escape),
            Some(Action::TakeBack)
        );
    }

    #[test]
    fn only_the_laptops_own_keys_count_as_hardware_keys() {
        assert!(is_hardware_key(Keysym::XF86_AudioRaiseVolume));
        assert!(is_hardware_key(Keysym::XF86_MonBrightnessDown));
        assert!(!is_hardware_key(Keysym::Escape));
        assert!(!is_hardware_key(Keysym::Print));
        assert!(!is_hardware_key(Keysym::Super_L));
    }

    #[test]
    fn hide_all_is_its_own_key_beside_minimise() {
        let bindings = defaults();
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::h),
            Some(Action::HideAll)
        );
        // Super+M stays one window, as on Windows, and Super+D still only shows the desktop.
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::m),
            Some(Action::Minimise)
        );
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::d),
            Some(Action::ShowDesktop)
        );
    }

    #[test]
    fn super_w_docks_a_window_and_with_shift_brings_it_back() {
        let bindings = defaults();
        let super_shift = Mods {
            shift: true,
            ..SUPER
        };
        assert_eq!(action_for(&bindings, SUPER, Keysym::w), Some(Action::Dock));
        assert_eq!(
            action_for(&bindings, super_shift, Keysym::w),
            Some(Action::Undock)
        );
    }

    #[test]
    fn mod_arrows_move_focus() {
        let bindings = defaults();
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::Right),
            Some(Action::Focus(Direction::Right))
        );
        assert_eq!(action_for(&bindings, Mods::default(), Keysym::Right), None);
    }

    #[test]
    fn mod_alt_arrows_move_the_tile_itself() {
        let bindings = defaults();
        let super_alt = Mods { alt: true, ..SUPER };
        assert_eq!(
            action_for(&bindings, super_alt, Keysym::Down),
            Some(Action::MoveTile(Direction::Down))
        );
        // Orca's toggle is the one Super+Alt combination that must not be ours.
        assert!(is_reserved(super_alt, Keysym::s));
        assert!(!is_reserved(super_alt, Keysym::Down));
    }

    #[test]
    fn windows_style_workspace_keys() {
        let bindings = defaults();
        let super_ctrl = Mods {
            ctrl: true,
            ..SUPER
        };
        let super_shift = Mods {
            shift: true,
            ..SUPER
        };
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::_3),
            Some(Action::Workspace(3))
        );
        assert_eq!(
            action_for(&bindings, super_shift, Keysym::_5),
            Some(Action::MoveToWorkspace(5))
        );
        assert_eq!(
            action_for(&bindings, super_ctrl, Keysym::Right),
            Some(Action::WorkspaceBy(1))
        );
        assert_eq!(
            action_for(&bindings, super_shift, Keysym::Left),
            Some(Action::MoveToWorkspaceBy(-1))
        );
        assert_eq!(action_for(&bindings, alt, Keysym::F4), Some(Action::Close));
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::e),
            Some(Action::Launch(App::Files))
        );
        assert_eq!(
            action_for(&bindings, SUPER, Keysym::r),
            Some(Action::Rotate { clockwise: true })
        );
        // Super+Space is the input methods' again: a tapped Super opens the explorer.
        assert_eq!(action_for(&bindings, SUPER, Keysym::space), None);
    }

    #[test]
    fn nested_alt_stands_in_for_super() {
        let alt = ModifiersState {
            alt: true,
            ..ModifiersState::default()
        };
        let nested = Mods::from_state(&alt, true);
        assert_eq!(
            action_for(&defaults(), nested, Keysym::Right),
            Some(Action::Focus(Direction::Right))
        );
        let hardware = Mods::from_state(&alt, false);
        assert_eq!(action_for(&defaults(), hardware, Keysym::Right), None);
    }

    #[test]
    fn well_known_combinations_are_reserved() {
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        let ctrl_shift = Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        };
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert!(is_reserved(ctrl, Keysym::space), "input-method switch");
        assert!(
            is_reserved(SUPER, Keysym::space),
            "IBus input-method switch"
        );
        assert!(is_reserved(ctrl_alt, Keysym::F2), "virtual terminal switch");
        // The one chord taken back off the list: nothing below us answers it.
        assert!(!is_reserved(ctrl_alt, Keysym::Delete), "the way out");
        assert!(is_reserved(ctrl_shift, Keysym::c), "terminal copy");
        assert!(is_reserved(shift, Keysym::Tab), "Steam overlay");
        assert!(is_reserved(alt, Keysym::Left), "browser back");
        assert!(
            is_reserved(Mods::default(), Keysym::Insert),
            "Orca modifier"
        );
    }

    #[test]
    fn planned_slipstream_keys_are_free() {
        let super_ctrl = Mods {
            ctrl: true,
            ..SUPER
        };
        let super_shift = Mods {
            shift: true,
            ..SUPER
        };
        for (mods, key) in [
            (SUPER, Keysym::Tab),
            (SUPER, Keysym::z),
            (SUPER, Keysym::Left),
            (super_ctrl, Keysym::Right),
            (super_shift, Keysym::_1),
            (SUPER, Keysym::t),
            (SUPER, Keysym::r),
            (SUPER, Keysym::i),
            (SUPER, Keysym::a),
            (SUPER, Keysym::n),
            (SUPER, Keysym::f),
            (SUPER, Keysym::slash),
        ] {
            assert!(!is_reserved(mods, key), "{key:?} should be available");
        }
    }

    #[test]
    fn checked_drops_reserved_bindings() {
        let input_method_switch = Binding {
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            key: Keysym::space,
            action: Action::Explorer,
        };
        let kept = defaults()[0];
        assert_eq!(windows_checked(vec![input_method_switch, kept]), vec![kept]);
    }

    #[test]
    fn alt_tab_switches_windows() {
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        let alt_shift = Mods { shift: true, ..alt };
        let bindings = windows_checked(defaults());
        assert_eq!(
            action_for(&bindings, alt, Keysym::Tab),
            Some(Action::CycleWindows { forward: true })
        );
        assert_eq!(
            action_for(&bindings, alt_shift, Keysym::ISO_Left_Tab),
            Some(Action::CycleWindows { forward: false })
        );
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        assert!(
            is_reserved(shift, Keysym::Tab),
            "Steam overlay stays reserved"
        );
    }

    #[test]
    fn super_f_maximises_and_stays_gamescope_s_while_it_is_focused() {
        assert_eq!(
            action_for(&windows_checked(defaults()), SUPER, Keysym::f),
            Some(Action::Maximise)
        );
        assert!(passes_to_gamescope(SUPER, Keysym::f));
    }

    #[test]
    fn every_default_binding_is_described() {
        for style in KeyStyle::ALL {
            for b in checked(super::defaults(style), style) {
                assert!(!b.action.describe().is_empty(), "{b:?} has no description");
                assert!(!b.action.group().title().is_empty());
            }
        }
        let super_shift = Mods {
            shift: true,
            ..SUPER
        };
        assert_eq!(label(super_shift, Keysym::Escape), "Super+Shift+Esc");
        assert_eq!(label(SUPER, Keysym::slash), "Super+/");
        assert_eq!(label(super_shift, Keysym::braceright), "Super+Shift+]");
        assert_eq!(label(Mods::default(), Keysym::Print), "Print");
        assert_eq!(label(SUPER, Keysym::a), "Super+A");
        assert_eq!(
            action_for(&windows_checked(defaults()), SUPER, Keysym::slash),
            Some(Action::ShortcutSheet)
        );
    }

    #[test]
    fn alt_and_shift_together_switch_layouts_but_not_with_another_key() {
        let mut tap = LayoutTap::default();
        // Alt down, Shift down with Alt held, both let go: one switch, on the first release.
        assert!(!tap.key(Keysym::Alt_L, true, 0, false));
        assert!(!tap.key(Keysym::Shift_L, true, 1, true));
        assert!(tap.key(Keysym::Shift_L, false, 1, false));
        assert!(!tap.key(Keysym::Alt_L, false, 0, false));
        // The other way round, Shift first.
        assert!(!tap.key(Keysym::Shift_R, true, 0, false));
        assert!(!tap.key(Keysym::Alt_L, true, 1, true));
        assert!(tap.key(Keysym::Alt_L, false, 1, false));
        // Alt+Shift+Tab, the switcher going backwards.
        tap.key(Keysym::Alt_L, true, 0, false);
        tap.key(Keysym::Shift_L, true, 1, true);
        tap.key(Keysym::Tab, true, 2, true);
        tap.key(Keysym::Tab, false, 2, true);
        assert!(!tap.key(Keysym::Shift_L, false, 1, false));
        // Ctrl held too.
        tap.key(Keysym::Control_L, true, 0, false);
        tap.key(Keysym::Alt_L, true, 1, false);
        assert!(!tap.key(Keysym::Shift_L, true, 2, false));
        assert!(!tap.key(Keysym::Shift_L, false, 2, false));
        // A click with them held.
        tap.key(Keysym::Alt_L, true, 0, false);
        tap.key(Keysym::Shift_L, true, 1, true);
        tap.disarm();
        assert!(!tap.key(Keysym::Shift_L, false, 1, false));
        // Shift alone is no switch.
        tap.key(Keysym::Shift_L, true, 0, false);
        assert!(!tap.key(Keysym::Shift_L, false, 0, false));
    }

    #[test]
    fn a_tap_opens_the_explorer() {
        let mut tap = SuperTap::default();
        assert!(!tap.key(Keysym::Super_L, true, 1000, true));
        assert!(tap.key(Keysym::Super_L, false, 1100, true));
        assert!(
            !tap.key(Keysym::Super_L, false, 1150, true),
            "once per press"
        );
        assert!(!tap.key(Keysym::Super_R, true, 2000, true));
        assert!(tap.key(Keysym::Super_R, false, 2399, true));
    }

    #[test]
    fn a_chord_disarms_it() {
        let mut tap = SuperTap::default();
        tap.key(Keysym::Super_L, true, 1000, true);
        tap.key(Keysym::m, true, 1050, true);
        tap.key(Keysym::m, false, 1080, true);
        assert!(!tap.key(Keysym::Super_L, false, 1100, true));
        // Super pressed while another key is already down never arms.
        tap.key(Keysym::Super_L, true, 2000, false);
        assert!(!tap.key(Keysym::Super_L, false, 2100, true));
    }

    #[test]
    fn a_slow_press_doesnt() {
        let mut tap = SuperTap::default();
        tap.key(Keysym::Super_L, true, 1000, true);
        assert!(!tap.key(Keysym::Super_L, false, 1000 + TAP_MS, true));
        tap.key(Keysym::Super_L, true, u32::MAX - 10, true);
        assert!(
            tap.key(Keysym::Super_L, false, 50, true),
            "the clock wrapping is still a tap"
        );
    }

    #[test]
    fn shift_between_disarms_it() {
        // The Copilot key: Super, Shift and F23 together.
        let mut tap = SuperTap::default();
        tap.key(Keysym::Super_L, true, 1000, true);
        tap.key(Keysym::Shift_L, true, 1001, true);
        tap.key(Keysym::F23, true, 1002, true);
        tap.key(Keysym::F23, false, 1060, true);
        tap.key(Keysym::Shift_L, false, 1061, true);
        assert!(!tap.key(Keysym::Super_L, false, 1062, true));
    }

    #[test]
    fn a_pointer_button_disarms_it() {
        let mut tap = SuperTap::default();
        tap.key(Keysym::Super_L, true, 1000, true);
        tap.disarm();
        assert!(!tap.key(Keysym::Super_L, false, 1100, true));
    }

    #[test]
    fn gamescope_keys_pass_through() {
        assert!(passes_to_gamescope(SUPER, Keysym::s));
        assert!(!passes_to_gamescope(SUPER, Keysym::Tab));
        assert!(!passes_to_gamescope(Mods::default(), Keysym::s));
    }
}

//! Timed debug steps, for checking features without a person at the keyboard.
//!
//! `SLIPSTREAM_DEBUG="2000:ws:2;3000:shot:/tmp/frame.png;4000:quit"` runs each step once, that
//! many milliseconds after start. Steps: `ws:N` (switch to workspace N), `move:N` (move the
//! focused window to workspace N), `close`, `shot:PATH` (save the next rendered frame as a PNG),
//! `run:COMMAND` (start a program the way key bindings do, so X11 apps get XWayland's display),
//! `explore` (open or close the app explorer), `hkey:NAME` (a key in the shortcut sheet, which is
//! how the tour is stepped through), `type:TEXT` and `key:NAME` (type into the open
//! explorer, or press a key there by its xkb name, such as `Down`, `Return` or `ctrl+BackSpace`),
//! `heavier`,
//! `lighter` and `gravity` (Super+PgUp, Super+PgDn and Super+T on the focused window),
//! `tile:DIRECTION` (Super+Alt+arrow: move the focused tile `left`, `right`, `up` or `down`),
//! `click:X,Y` (a left click on a window, in logical pixels: focus follows it and the client
//! gets the press, which is the way to open an app's own menus and check popup grabs; the bar
//! and the panels have their own click steps),
//! `button:left`, `button:middle` or `button:right` (a press and release through the real click
//! routing, wherever `pointer:X,Y` last put the pointer; `button:left+` presses and holds,
//! `button:left-` lets go, for a drag),
//! `cycle` or `cycle:N` (Alt held, Tab pressed N times, then Alt let go), `tab` (one Tab with Alt
//! held down, and kept held) and `letgo` (Alt let go after `tab`),
//! `minimise` and `restore` (Super+M and Super+Shift+M), `hideall` (Super+H: every window on the
//! workspace into the code rain, and the same ones back), `idle` (fade the UI out now) and `wake`
//! (as any input would), `bullet` (Super+Tab), `bkey:NAME` (a key in bullet time, by xkb name
//! with optional `shift+` or `ctrl+`, such as `bkey:Right` or `bkey:shift+2`), `bclick:X,Y` (a
//! click in bullet time, in logical pixels on the screen, the bar's buttons included), `quick` (Super+A), `qkey:NAME` (a key
//! in quick settings, by xkb name with optional `shift+`), `qclick:X,Y` (a click while quick
//! settings is open, in logical pixels on the screen), `centre`, `ckey:NAME` and `cclick:X,Y`
//! (the same for the notification centre, Super+N), `notify:APP|TITLE|BODY` (a notification, as
//! if an app had sent one), `critical:APP|TITLE|BODY` (a critical one, which wakes the faded UI),
//! `wallpaper:ID` (show one living-wallpaper variation now, by its id in
//! the settings file), `wayout` (the way out on its chooser, as Ctrl+Alt+Del opens it), `logout`,
//! `restart` and `shutdown` (the same overlay with its destination already named, as quick
//! settings' power buttons open it) with `xkey:NAME` for a key in it and `xclick:X,Y`
//! for a click on one of its buttons, `drag:X1,Y1 X2,Y2` (a press, a drag and a release, as
//! selecting text in a terminal does), `okey:NAME` and `oclick:X,Y` for the offer at login that
//! asks whether to put the last layout back, `share:KINDS` (the screen-sharing picker, as the
//! portal would open it, offering screens for `m`, windows for `w`, or both for `mw`) with
//! `skey:NAME` and `sclick:X,Y` to answer it, `unshare` (a click on the bar's red dot),
//! `press:NAME`, `release:NAME` and `chord:super+shift+s` (keys by xkb name, through the same
//! routing as the keyboard, and meaning what they mean in the login session: Super is Super even
//! nested), `motion` (every animating part's reduced-motion flag, into the log), `lock` (the lock
//! screen; nested runs only), `unlock` (the lock taken down as a right password would), `arrive`
//! (the desktop condensing out of the code rain, as at login), `battery:N` (pretend the battery
//! is at N percent and unplugged; `battery:N+` charging, `battery:off` reads it again), and `quit`. While locked, only the steps that go through the
//! keyboard's, the pointer's and apps' own paths act.
//!
//! Key and click steps for a panel do nothing, and say so in the log, while that panel is closed:
//! it keeps its last selection, which a mistimed step would otherwise press.
//!
//! Nested, commands that change the machine are only logged (`launch::machine_commands_held`):
//! `restart` and `shutdown` end in `nested: not asking logind` and the compositor stops
//! as Log out does, and `SLIPSTREAM_POWER_REFUSE=1` has the request refused instead, as an
//! inhibitor would. Outside a nested run they really restart and shut the machine down. `quit`
//! stops the event loop on the spot, as the watchdog does, and never goes near the overlay: every
//! headless check ends with it.

use std::time::Duration;

use smithay::input::keyboard::{Keysym, xkb};

use crate::{keys::Mods, layout::Direction};

/// A panel key step's name: an xkb keysym name after any `shift+`, `ctrl+`, `alt+` or `super+`.
pub fn mods_and_key(name: &str) -> (Mods, Keysym) {
    let mut mods = Mods::default();
    let mut rest = name.trim();
    loop {
        if let Some(after) = rest.strip_prefix("shift+") {
            mods.shift = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("ctrl+") {
            mods.ctrl = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("alt+") {
            mods.alt = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("super+") {
            mods.logo = true;
            rest = after;
        } else {
            break;
        }
    }
    (mods, xkb::keysym_from_name(rest, xkb::KEYSYM_NO_FLAGS))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Workspace(u8),
    /// Logs every mapped window's place, its own size and the size it was last told.
    Windows,
    MoveTo(u8),
    Close,
    Screenshot(String),
    Run(String),
    Explore,
    /// A key in the shortcut sheet, by its xkb name: what steps through the tour.
    SheetKey(String),
    Type(String),
    Key(String),
    MoveTile(Direction),
    /// A left click on the screen, in logical pixels.
    Click(f64, f64),
    /// A press at the first point, a drag to the second and a release, in logical pixels: how
    /// text is selected in a terminal.
    Drag(f64, f64, f64, f64),
    Heavier,
    Lighter,
    Gravity,
    Minimise,
    Restore,
    /// Super+H: every window on this workspace into the code rain, and back.
    HideAll,
    Idle,
    Wake,
    /// Alt held, Tab pressed this many times, and Alt let go.
    Cycle(usize),
    /// One Tab with Alt held, and Alt kept down.
    Tab,
    /// Alt let go after `Tab`.
    LetGo,
    /// The lock taken down, as a right password would.
    Unlock,
    /// The desktop condenses out of the code rain, as at login.
    Arrive,
    /// The pointer moves to a point on a screen, as the mouse would move it: `pointer:X,Y` on the
    /// focused screen, `pointer:NAME@X,Y` on the screen of that output's name.
    Pointer(Option<String>, f64, f64),
    /// A button pressed and let go where the pointer is, by its Linux button code.
    Button(u32),
    /// A button pressed and held (`button:left+`), or let go (`button:left-`).
    ButtonHeld(u32, bool),
    Bullet,
    BulletKey(String),
    BulletClick(f64, f64),
    Quick,
    QuickKey(String),
    QuickClick(f64, f64),
    Centre,
    CentreKey(String),
    CentreClick(f64, f64),
    /// App, title, body, and action buttons as identifier and label.
    Notify(String, String, String, Vec<(String, String)>),
    /// A critical notification: app, title, body.
    NotifyCritical(String, String, String),
    /// A living-wallpaper variation, by id.
    Wallpaper(String),
    /// Every minimised app's load, pinned, so a stream's two ends can be looked at without
    /// finding an app that is really working that hard.
    Demand(f32),
    /// The fade to the wallpaper held this far along, 0 (the UI) to 1 (the wallpaper), so its
    /// frames can be looked at one by one.
    Fade(f32),
    /// Count each frame drawn from here on as this many milliseconds, whatever it really cost,
    /// so a renderer that manages a dozen frames a second still lays them a fortieth of a second
    /// apart through an animation. The script's own times step with it; 0 is real time again.
    Slow(f64),
    /// The volume or brightness display, as a keypress would put it up. It only draws the card:
    /// nothing is muted and no level is changed, so a nested run can't touch the real machine.
    Osd(String),
    /// The way out on its chooser, as Ctrl+Alt+Del opens it.
    WayOut,
    LogOut,
    Restart,
    ShutDown,
    ExitKey(String),
    ExitClick(f64, f64),
    /// A key for the offer at login, and a click on its card.
    OfferKey(String),
    OfferClick(f64, f64),
    /// A key for the card asking what a screen just plugged in should show.
    ConnectKey(String),
    /// The focused window fills its screen, or stops.
    Fullscreen(bool),
    /// Moves the keyboard to the neighbouring tile, as Super+arrow does.
    FocusTile(String),
    /// The share picker, offering screens (`m`), windows (`w`) or both.
    SharePicker(String),
    ShareKey(String),
    ShareClick(f64, f64),
    StopSharing,
    /// A second screen, made up: width and height in logical pixels. Nested runs have one real
    /// output and no way to plug another in, so this is how the two-screen rules are checked.
    AddScreen(i32, i32),
    /// Takes a made-up screen away again, by name.
    DropScreen(String),
    /// The laptop's lid, as libinput's switch would report it.
    Lid(bool),
    /// Writes the screens, their areas and what each is showing into the log.
    Screens,
    /// Writes each animating part's reduced-motion flag into the log.
    Motion,
    /// Super+P and Super+Shift+P.
    NextScreen,
    MoveToNextScreen,
    SwapScreens,
    /// Keys pressed and let go through the real keyboard routing, in order, by xkb name.
    Keys(Vec<(String, bool)>),
    /// Locks the screen. Nested runs only.
    Lock,
    /// Pretends the battery is at this charge, and whether it's charging; `None` reads it again.
    Battery(Option<(u8, bool)>),
    Quit,
}

impl Step {
    /// Whether a step still acts while the screen is locked: those that go through the same paths
    /// as the keyboard, the pointer, apps and the machine's own events, and those that only
    /// watch. The rest reach past the keyboard filter, and are ignored.
    pub fn works_while_locked(&self) -> bool {
        matches!(
            self,
            Step::Screenshot(_)
                | Step::Run(_)
                | Step::Keys(_)
                | Step::Pointer(..)
                | Step::Button(_)
                | Step::ButtonHeld(..)
                | Step::Idle
                | Step::Wake
                | Step::Notify(..)
                | Step::NotifyCritical(..)
                | Step::Osd(_)
                | Step::Wallpaper(_)
                | Step::Demand(_)
                | Step::Fade(_)
                | Step::SharePicker(_)
                | Step::AddScreen(..)
                | Step::DropScreen(_)
                | Step::Lid(_)
                | Step::Screens
                | Step::Fullscreen(_)
                | Step::FocusTile(_)
                | Step::Windows
                | Step::Motion
                | Step::Lock
                | Step::Unlock
                | Step::Slow(_)
                | Step::Battery(_)
                | Step::Quit
        )
    }
}

/// The key events of `press:NAME`, `release:NAME` or `chord:mod+…+NAME`. A chord presses its
/// keys in order and lets them go in reverse, as fingers do. Modifiers can be written as
/// `super`, `shift`, `ctrl` and `alt`; anything else is an xkb keysym name.
pub fn key_events(kind: &str, text: &str) -> Vec<(String, bool)> {
    let name = |part: &str| {
        match part.trim().to_ascii_lowercase().as_str() {
            "super" | "logo" | "win" | "mod4" => "Super_L",
            "shift" => "Shift_L",
            "ctrl" | "control" => "Control_L",
            "alt" => "Alt_L",
            "altgr" => "ISO_Level3_Shift",
            _ => part.trim(),
        }
        .to_string()
    };
    match kind {
        "press" => vec![(name(text), true)],
        "release" => vec![(name(text), false)],
        _ => {
            let keys: Vec<String> = text
                .split('+')
                .filter(|part| !part.trim().is_empty())
                .map(name)
                .collect();
            let presses = keys.iter().cloned().map(|key| (key, true));
            let releases = keys.iter().rev().cloned().map(|key| (key, false));
            presses.chain(releases).collect()
        }
    }
}

#[derive(Debug, Default)]
pub struct Script {
    steps: Vec<(Duration, Step)>,
}

impl Script {
    pub fn from_env() -> Self {
        std::env::var("SLIPSTREAM_DEBUG")
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn parse(text: &str) -> Self {
        let point = |at: &str| -> Option<(f64, f64)> {
            let (x, y) = at.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        let mut steps: Vec<(Duration, Step)> = text
            .split(';')
            .filter(|item| !item.trim().is_empty())
            .filter_map(|item| {
                let mut parts = item.trim().splitn(3, ':');
                let ms: u64 = parts.next()?.parse().ok()?;
                let step = match (parts.next()?, parts.next()) {
                    ("ws", Some(n)) => Step::Workspace(n.parse().ok()?),
                    ("move", Some(n)) => Step::MoveTo(n.parse().ok()?),
                    ("close", None) => Step::Close,
                    ("shot", Some(path)) => Step::Screenshot(path.to_string()),
                    ("run", Some(command)) => Step::Run(command.to_string()),
                    ("explore", None) => Step::Explore,
                    ("hkey", Some(name)) => Step::SheetKey(name.to_string()),
                    ("type", Some(text)) => Step::Type(text.to_string()),
                    ("key", Some(name)) => Step::Key(name.to_string()),
                    ("tile", Some(direction)) => Step::MoveTile(match direction.trim() {
                        "left" => Direction::Left,
                        "right" => Direction::Right,
                        "up" => Direction::Up,
                        "down" => Direction::Down,
                        _ => return None,
                    }),
                    ("click", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::Click(x, y)
                    }
                    ("drag", Some(at)) => {
                        let (from, to) = at.split_once(' ')?;
                        let (x1, y1) = point(from)?;
                        let (x2, y2) = point(to)?;
                        Step::Drag(x1, y1, x2, y2)
                    }
                    ("heavier", None) => Step::Heavier,
                    ("lighter", None) => Step::Lighter,
                    ("gravity", None) => Step::Gravity,
                    ("minimise", None) => Step::Minimise,
                    ("hideall", None) => Step::HideAll,
                    ("restore", None) => Step::Restore,
                    ("idle", None) => Step::Idle,
                    ("wake", None) => Step::Wake,
                    ("cycle", None) => Step::Cycle(1),
                    ("tab", None) => Step::Tab,
                    ("letgo", None) => Step::LetGo,
                    ("unlock", None) => Step::Unlock,
                    ("arrive", None) => Step::Arrive,
                    ("cycle", Some(tabs)) => Step::Cycle(tabs.trim().parse().ok()?),
                    ("pointer", Some(at)) => match at.split_once('@') {
                        Some((screen, at)) => {
                            let (x, y) = point(at)?;
                            Step::Pointer(Some(screen.trim().to_string()), x, y)
                        }
                        None => {
                            let (x, y) = point(at)?;
                            Step::Pointer(None, x, y)
                        }
                    },
                    // BTN_LEFT, BTN_RIGHT and BTN_MIDDLE.
                    ("button", Some(which)) => {
                        let which = which.trim();
                        let (name, held) = match which.strip_suffix('+') {
                            Some(name) => (name, Some(true)),
                            None => match which.strip_suffix('-') {
                                Some(name) => (name, Some(false)),
                                None => (which, None),
                            },
                        };
                        let code = match name {
                            "left" => 0x110,
                            "right" => 0x111,
                            "middle" => 0x112,
                            _ => return None,
                        };
                        match held {
                            Some(pressed) => Step::ButtonHeld(code, pressed),
                            None => Step::Button(code),
                        }
                    }
                    ("bullet", None) => Step::Bullet,
                    ("bkey", Some(name)) => Step::BulletKey(name.to_string()),
                    ("bclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::BulletClick(x, y)
                    }
                    ("quick", None) => Step::Quick,
                    ("qkey", Some(name)) => Step::QuickKey(name.to_string()),
                    ("qclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::QuickClick(x, y)
                    }
                    ("centre", None) => Step::Centre,
                    ("ckey", Some(name)) => Step::CentreKey(name.to_string()),
                    ("cclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::CentreClick(x, y)
                    }
                    ("notify", Some(text)) => {
                        let mut fields = text.splitn(4, '|');
                        let app = fields.next()?.to_string();
                        let title = fields.next().unwrap_or_default().to_string();
                        let body = fields.next().unwrap_or_default().to_string();
                        // Action buttons, `id=Label,id=Label`.
                        let actions = fields
                            .next()
                            .unwrap_or_default()
                            .split(',')
                            .filter_map(|pair| pair.split_once('='))
                            .map(|(id, label)| (id.trim().to_string(), label.trim().to_string()))
                            .collect();
                        Step::Notify(app, title, body, actions)
                    }
                    ("critical", Some(text)) => {
                        let mut fields = text.splitn(3, '|');
                        let app = fields.next()?.to_string();
                        let title = fields.next().unwrap_or_default().to_string();
                        let body = fields.next().unwrap_or_default().to_string();
                        Step::NotifyCritical(app, title, body)
                    }
                    ("wallpaper", Some(id)) => Step::Wallpaper(id.trim().to_string()),
                    ("demand", Some(value)) => Step::Demand(value.trim().parse().ok()?),
                    ("fade", Some(value)) => Step::Fade(value.trim().parse().ok()?),
                    ("slow", Some(ms)) => Step::Slow(ms.trim().parse::<f64>().ok()? / 1000.0),
                    ("osd", Some(what)) => Step::Osd(what.trim().to_string()),
                    ("screen", Some(size)) => {
                        let (w, h) = size.trim().split_once('x')?;
                        Step::AddScreen(w.trim().parse().ok()?, h.trim().parse().ok()?)
                    }
                    ("unscreen", Some(name)) => Step::DropScreen(name.trim().to_string()),
                    ("lid", Some(state)) => Step::Lid(state.trim() == "closed"),
                    ("screens", None) => Step::Screens,
                    ("windows", None) => Step::Windows,
                    ("motion", None) => Step::Motion,
                    ("nextscreen", None) => Step::NextScreen,
                    ("movescreen", None) => Step::MoveToNextScreen,
                    ("swapscreens", None) => Step::SwapScreens,
                    ("wayout", None) => Step::WayOut,
                    ("logout", None) => Step::LogOut,
                    ("restart", None) => Step::Restart,
                    ("shutdown", None) => Step::ShutDown,
                    ("xkey", Some(name)) => Step::ExitKey(name.to_string()),
                    ("okey", Some(name)) => Step::OfferKey(name.to_string()),
                    ("sckey", Some(name)) => Step::ConnectKey(name.to_string()),
                    ("full", None) => Step::Fullscreen(true),
                    ("unfull", None) => Step::Fullscreen(false),
                    ("focus", Some(way)) => Step::FocusTile(way.trim().to_string()),
                    ("oclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::OfferClick(x, y)
                    }
                    ("share", Some(kinds)) => Step::SharePicker(kinds.trim().to_string()),
                    ("skey", Some(name)) => Step::ShareKey(name.to_string()),
                    ("sclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::ShareClick(x, y)
                    }
                    ("unshare", None) => Step::StopSharing,
                    ("xclick", Some(at)) => {
                        let (x, y) = point(at)?;
                        Step::ExitClick(x, y)
                    }
                    (kind @ ("press" | "release" | "chord"), Some(text)) => {
                        Step::Keys(key_events(kind, text))
                    }
                    ("lock", None) => Step::Lock,
                    ("battery", Some("off")) => Step::Battery(None),
                    ("battery", Some(level)) => {
                        let charging = level.ends_with('+');
                        Step::Battery(Some((
                            level.trim_end_matches('+').trim().parse().ok()?,
                            charging,
                        )))
                    }
                    ("quit", None) => Step::Quit,
                    _ => {
                        tracing::warn!("ignoring debug step {item:?}");
                        return None;
                    }
                };
                Some((Duration::from_millis(ms), step))
            })
            .collect();
        steps.sort_by_key(|(at, _)| *at);
        Self { steps }
    }

    /// The steps now due, in order. Each is returned only once.
    pub fn due(&mut self, elapsed: Duration) -> Vec<Step> {
        let n = self
            .steps
            .iter()
            .take_while(|(at, _)| *at <= elapsed)
            .count();
        self.steps.drain(..n).map(|(_, step)| step).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_asks_for_a_stepped_clock_by_its_milliseconds() {
        let script = Script::parse("1000:slow:20;2000:slow:0");
        assert_eq!(
            script.steps,
            vec![
                (Duration::from_millis(1000), Step::Slow(0.02)),
                (Duration::from_millis(2000), Step::Slow(0.0)),
            ]
        );
        assert!(Script::parse("1000:slow:slowly").steps.is_empty());
    }

    #[test]
    fn a_chord_presses_in_order_and_releases_in_reverse() {
        let script = Script::parse("1000:chord:super+shift+s;1200:press:Alt_L;1300:release:Tab");
        let pressed = |name: &str| (name.to_string(), true);
        let released = |name: &str| (name.to_string(), false);
        assert_eq!(
            script.steps,
            [
                (
                    Duration::from_millis(1000),
                    Step::Keys(vec![
                        pressed("Super_L"),
                        pressed("Shift_L"),
                        pressed("s"),
                        released("s"),
                        released("Shift_L"),
                        released("Super_L"),
                    ])
                ),
                (
                    Duration::from_millis(1200),
                    Step::Keys(vec![pressed("Alt_L")])
                ),
                (
                    Duration::from_millis(1300),
                    Step::Keys(vec![released("Tab")])
                ),
            ]
        );
    }

    #[test]
    fn parses_steps_in_time_order() {
        let mut script = Script::parse("3000:shot:/tmp/a:b.png; 1000:ws:2 ;2000:move:3;4000:quit");
        assert_eq!(script.due(Duration::from_millis(999)), vec![]);
        assert_eq!(
            script.due(Duration::from_millis(2500)),
            vec![Step::Workspace(2), Step::MoveTo(3)]
        );
        assert_eq!(
            script.due(Duration::from_secs(10)),
            vec![Step::Screenshot("/tmp/a:b.png".into()), Step::Quit]
        );
        assert_eq!(script.due(Duration::from_secs(20)), vec![]);
    }

    #[test]
    fn ignores_steps_it_does_not_understand() {
        let mut script = Script::parse("100:fly:away;200:close;junk");
        assert_eq!(script.due(Duration::from_secs(1)), vec![Step::Close]);
    }

    #[test]
    fn moving_a_tile_takes_a_direction() {
        let mut script = Script::parse("100:tile:up;200:tile:sideways");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![Step::MoveTile(Direction::Up)]
        );
    }

    #[test]
    fn while_locked_a_button_takes_the_real_click_path_and_panel_steps_do_nothing() {
        // `button:` goes through the pointer's own routing, which the lock stops at the top.
        assert!(Step::Button(0x110).works_while_locked());
        assert_eq!(
            Script::parse("100:button:left+;200:button:left-").steps,
            [
                (Duration::from_millis(100), Step::ButtonHeld(0x110, true)),
                (Duration::from_millis(200), Step::ButtonHeld(0x110, false)),
            ]
        );
        for step in [
            Step::Key("Return".into()),
            Step::QuickClick(10.0, 10.0),
            Step::CentreClick(10.0, 10.0),
            Step::BulletKey("Return".into()),
            Step::Click(10.0, 10.0),
        ] {
            assert!(!step.works_while_locked(), "{step:?}");
        }
    }

    #[test]
    fn buttons_and_panel_keys_are_named() {
        let mut script = Script::parse("100:button:middle;200:button:other;300:key:ctrl+BackSpace");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![Step::Button(0x112), Step::Key("ctrl+BackSpace".into())]
        );
        let (mods, key) = mods_and_key("ctrl+BackSpace");
        assert!(mods.ctrl && !mods.shift);
        assert_eq!(key, Keysym::BackSpace);
    }

    #[test]
    fn clicks_take_a_point() {
        let mut script = Script::parse("100:qclick:1400, 90;200:bclick:nowhere;300:qkey:shift+Tab");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![
                Step::QuickClick(1400.0, 90.0),
                Step::QuickKey("shift+Tab".into())
            ]
        );
    }

    #[test]
    fn the_way_out_has_its_own_steps() {
        let mut script = Script::parse("100:logout;200:xkey:Return;300:shutdown");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![Step::LogOut, Step::ExitKey("Return".into()), Step::ShutDown]
        );
    }

    #[test]
    fn a_notification_keeps_colons_in_its_text() {
        let mut script = Script::parse("100:notify:Mail|Sam Carter|Walk at 10:00?");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![Step::Notify(
                "Mail".into(),
                "Sam Carter".into(),
                "Walk at 10:00?".into(),
                Vec::new()
            )]
        );
        let mut script = Script::parse("100:notify:Mail|Sam|Walk?|yes=Yes, see you,later=Later");
        assert_eq!(
            script.due(Duration::from_secs(1)),
            vec![Step::Notify(
                "Mail".into(),
                "Sam".into(),
                "Walk?".into(),
                vec![
                    ("yes".into(), "Yes".into()),
                    ("later".into(), "Later".into())
                ]
            )]
        );
    }
}

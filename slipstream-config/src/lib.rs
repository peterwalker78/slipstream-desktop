//! Slipstream's settings file, `~/.config/slipstream/settings.toml`. The compositor reads it at
//! startup and again whenever it changes; `slipstream-settings` and quick settings write it. Every
//! setting's name, default and meaning lives here, so they never disagree.
//!
//! ```toml
//! [appearance]
//! colour-scheme = "dark"
//!
//! [wallpaper]
//! fade-after-secs = 120
//! variations = ["slipstream"]
//! change-every-mins = 10
//!
//! [motion]
//! reduced = false
//! effects = "slipstream"
//! close-into-rain = true
//!
//! [display]
//! night-light = false
//! night-light-schedule = "off"
//! night-light-from = "21:00"
//! night-light-to = "07:00"
//! screen-off-mins = 15
//! variable-refresh = "games"
//!
//! [notifications]
//! do-not-disturb = false
//! wait-while-typing = true
//! longest-wait-mins = 15
//! sounds = true
//! sound-theme = "ocean"
//!
//! [sound]
//! volume-blip = true
//!
//! [clipboard]
//! history = true
//!
//! [keys]
//! style = "windows"
//!
//! [mouse]
//! speed = 0.0
//! natural-scrolling = false
//! acceleration = "adaptive"
//!
//! [touchpad]
//! tap-to-click = true
//! natural-scrolling = true
//! speed = 0.0
//! disable-while-typing = true
//! scrolling = "two-finger"
//!
//! [keyboard]
//! repeat-delay-ms = 200
//! repeat-rate = 25
//!
//! [[keyboard.layouts]]
//! layout = "gb"
//! variant = ""
//!
//! [lock]
//! after-idle-mins = 0
//! before-sleep = true
//!
//! [meter]
//! command = ""
//! every-secs = 60
//! hidden = []
//!
//! [session]
//! remember = false
//! reopen-without-asking = false
//!
//! [borders]
//! selected-tile = "#42d3ff"
//! bullet-time = "#ffb547"
//!
//! [privacy]
//! screen-capture-allowed = ["/usr/bin/wf-recorder"]
//!
//! [[workspaces.list]]
//! id = 1
//! name = "Mail"
//!
//! [[workspaces.list]]
//! id = 2
//! name = ""
//! ```
//!
//! Missing keys take their defaults and unknown ones are ignored, so an older compositor still
//! reads a newer file.

pub mod battery;
mod display;
mod file;
mod input;
pub mod meter;
mod pages;
pub mod screens;
pub mod sun;
mod wallpaper;
mod workspaces;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use display::{
    Align, Display, NIGHT_FADE_MINS, NightSchedule, Position, SCALE_RANGE, ScreenPlace,
    VariableRefresh,
};
pub use file::{
    FILE_NAME, civil, move_aside, path, read, replace, replace_private, write, xdg_home,
};
pub use input::{
    Acceleration, Keyboard, KeyboardLayout, LAYOUT_LISTS, LayoutChoice, Mouse, Scrolling, Touchpad,
    layout_catalogue, layout_label, options_switch_layouts, system_layouts,
    system_layouts_from_env,
};
pub use pages::{SETTINGS_PAGES, SettingsPage};
pub use wallpaper::{VARIATIONS, Variation, Wallpaper, current_id};
pub use workspaces::{MAX_WORKSPACES, WorkspaceEntry, Workspaces, unused_id, workspace_label};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    pub appearance: Appearance,
    pub wallpaper: Wallpaper,
    pub motion: Motion,
    pub display: Display,
    pub notifications: Notifications,
    pub borders: Borders,
    pub session: Session,
    pub lock: Lock,
    pub sound: Sound,
    pub clipboard: Clipboard,
    pub privacy: Privacy,
    pub meter: Meter,
    pub workspaces: Workspaces,
    pub keys: Keys,
    pub mouse: Mouse,
    pub touchpad: Touchpad,
    pub keyboard: Keyboard,
}

/// The keyboard shortcuts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Keys {
    /// Whose shortcuts the desktop's keys follow.
    pub style: KeyStyle,
}

/// Which desktop's habits the shortcuts follow. The actions are the same in each; the keys
/// differ, and a style leaves out a key its desktop has no habit for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyStyle {
    #[default]
    Windows,
    Mac,
    Omarchy,
}

impl KeyStyle {
    pub const ALL: [KeyStyle; 3] = [KeyStyle::Windows, KeyStyle::Mac, KeyStyle::Omarchy];

    /// What it's called in the Settings app.
    pub fn label(self) -> &'static str {
        match self {
            KeyStyle::Windows => "Windows style",
            KeyStyle::Mac => "Mac style",
            KeyStyle::Omarchy => "Omarchy style",
        }
    }

    /// What the style is like, in a line for the Settings app.
    pub fn summary(self) -> &'static str {
        match self {
            KeyStyle::Windows => {
                "Tap Super to open apps, as the Windows key opens Start. Alt+Tab, Alt+F4, \
                 Super+L, Super+D and Super+Shift+S do what they do on Windows."
            }
            KeyStyle::Mac => {
                "Super is ⌘: Super+Space opens apps, and Super+Tab, Super+Q, Super+W and \
                 Super+Shift+3 and 4 do what ⌘ does on a Mac. Apps keep their own shortcuts, so \
                 copying is still Ctrl+C."
            }
            KeyStyle::Omarchy => {
                "Omarchy's keys: Super+Space opens apps, Super+W closes, Super+T floats, \
                 Super+C and Super+V copy and paste in any app, and Super+K lists every key."
            }
        }
    }

    /// The keys that set the style apart, each beside what it does as the shortcut sheet says
    /// it. `Super` alone is a tap.
    pub fn highlights(self) -> &'static [(&'static str, &'static str)] {
        match self {
            KeyStyle::Windows => &[
                ("search apps and files", "Super"),
                ("switch window", "Alt+Tab"),
                ("close window", "Alt+F4"),
                ("every workspace at once", "Super+Tab"),
                ("maximise", "Super+F"),
                ("float the window, or tile it", "Super+Shift+V"),
                ("snip a region or a window", "Super+Shift+S"),
                ("lock the screen", "Super+L"),
                ("every key", "Super+/"),
            ],
            KeyStyle::Mac => &[
                ("search apps and files", "Super+Space"),
                ("switch window", "Super+Tab"),
                ("close window", "Super+W"),
                ("quit the app", "Super+Q"),
                ("every workspace at once", "Super+Ctrl+↑"),
                ("maximise", "Super+Ctrl+F"),
                ("snip a region or a window", "Super+Shift+4"),
                ("lock the screen", "Super+Ctrl+Q"),
                ("every key", "Super+/"),
            ],
            KeyStyle::Omarchy => &[
                ("search apps and files", "Super+Space"),
                ("close window", "Super+W"),
                ("float the window, or tile it", "Super+T"),
                ("tiling or gravity; hold to choose", "Super+L"),
                ("previous, next workspace", "Super+Tab"),
                ("every workspace at once", "Super+Ctrl+↑"),
                ("snip a region or a window", "Print"),
                ("lock the screen", "Super+Ctrl+L"),
                ("every key", "Super+K"),
            ],
        }
    }

    /// The keys that zoom the screen in, out and back off, for the Settings app.
    pub fn zoom_keys(self) -> [&'static str; 3] {
        match self {
            KeyStyle::Windows => ["Super+=", "Super+-", "Super+0"],
            KeyStyle::Mac => ["Super+Alt+=", "Super+Alt+-", "Super+Alt+0"],
            KeyStyle::Omarchy => ["Super+Ctrl+Z", "Super+Ctrl+-", "Super+Ctrl+Alt+Z"],
        }
    }
}

/// What apps are told to colour themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Appearance {
    /// Dark or light, for every app that asks the desktop which to be. Slipstream's own chrome is
    /// dark either way.
    pub colour_scheme: ColourScheme,
}

/// Dark or light.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColourScheme {
    #[default]
    Dark,
    Light,
}

impl ColourScheme {
    pub const ALL: [ColourScheme; 2] = [ColourScheme::Dark, ColourScheme::Light];

    /// What it's called in the Settings app.
    pub fn label(self) -> &'static str {
        match self {
            ColourScheme::Dark => "Dark",
            ColourScheme::Light => "Light",
        }
    }

    /// The number the appearance portal's `color-scheme` key uses. Its third value, 0 for no
    /// preference, is never sent: Slipstream always has one.
    pub fn portal_value(self) -> u32 {
        match self {
            ColourScheme::Dark => 1,
            ColourScheme::Light => 2,
        }
    }

    /// The same preference as GNOME's `color-scheme` setting spells it, which is what GTK reads
    /// when it doesn't know the portal's own key.
    pub fn gnome_value(self) -> &'static str {
        match self {
            ColourScheme::Dark => "prefer-dark",
            ColourScheme::Light => "prefer-light",
        }
    }

    /// A stock GTK theme of that colour, for apps that follow a theme's name rather than a
    /// preference. Adwaita ships with GTK itself, so it is on every machine GTK apps run on.
    pub fn gtk_theme(self) -> &'static str {
        match self {
            ColourScheme::Dark => "Adwaita-dark",
            ColourScheme::Light => "Adwaita",
        }
    }
}

/// How windows and effects move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Motion {
    /// Reduced motion: moves jump, and every effect becomes a short fade.
    pub reduced: bool,
    /// What minimised apps turn into, and how a closed window falls away.
    pub effects: Effects,
    /// A closed window falls away in the chosen effects. Off, it fades. Reduced motion turns it
    /// off too.
    pub close_into_rain: bool,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            reduced: false,
            effects: Effects::default(),
            close_into_rain: true,
        }
    }
}

/// The look of the minimised apps' streams and of closing windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effects {
    /// Streaks of light falling at different depths, and a closed window dropping away with a
    /// wake of light.
    #[default]
    Slipstream,
    /// Code rain: glyphs falling in columns, and a closed window read out into them.
    Matrix,
}

/// Notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Notifications {
    /// Do not disturb: notifications still reach the notification centre, but don't pop up.
    pub do_not_disturb: bool,
    /// Pop-ups wait while you're typing and show at the next pause, together. Critical ones
    /// don't wait.
    pub wait_while_typing: bool,
    /// The longest a pop-up waits for a pause, in minutes. Urgent messages tend to reach a phone
    /// first, so a quarter of an hour of unbroken typing is left alone. At least 1.
    pub longest_wait_mins: u64,
    /// Plays the sound a notification asks for, by file or by name from the sound theme. Not
    /// while you're looking at the window it came from, and under do not disturb only for
    /// critical ones.
    pub sounds: bool,
    /// The freedesktop sound theme sound names are looked up in, falling back through the
    /// themes it inherits from to `freedesktop`.
    pub sound_theme: String,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            do_not_disturb: false,
            wait_while_typing: true,
            longest_wait_mins: 15,
            sounds: true,
            sound_theme: "ocean".into(),
        }
    }
}

impl Notifications {
    /// `longest_wait_mins` in seconds, never under a minute.
    pub fn longest_wait_secs(&self) -> f64 {
        self.longest_wait_mins.max(1) as f64 * 60.0
    }
}

/// The sounds Slipstream makes itself, as against the ones apps make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Sound {
    /// A short click at the new level whenever the volume changes, from the keys or from quick
    /// settings' slider, as macOS and Windows do, so the setting can be heard as well as seen.
    /// Never on mute, never on brightness.
    pub volume_blip: bool,
}

impl Default for Sound {
    fn default() -> Self {
        Self { volume_blip: true }
    }
}

/// A meter on the bar, beside quick settings, showing how much of some allowance a command reports
/// used: a quota, a plan's limits, a disk. The command prints the JSON report described in
/// [`meter`], which quick settings shows in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Meter {
    /// Run with `sh -c`. Empty means no meter.
    pub command: String,
    /// How often the command runs, in seconds. At least 10, at most a day.
    pub every_secs: u64,
    /// Sections of the report not to show, by name. Quick settings' cog chooses them.
    pub hidden: Vec<String>,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            command: String::new(),
            every_secs: 60,
            hidden: Vec::new(),
        }
    }
}

impl Meter {
    /// Whether the section called `name` is shown.
    pub fn shows(&self, name: &str) -> bool {
        !self.hidden.iter().any(|hidden| hidden == name)
    }

    /// `every_secs`, kept between 10 s and a day: a hand-edited huge value would otherwise
    /// overflow the meter's next deadline.
    pub fn every(&self) -> Duration {
        Duration::from_secs(self.every_secs.clamp(10, 86_400))
    }
}

/// What's open when the session ends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Session {
    /// Record which apps were open, on which workspace and in which tiling place, so the layout
    /// can be put back. Off by default: with it off nothing is written to disk at all. The way
    /// out's card carries the same switch, and turning it on there turns it on here.
    pub remember: bool,
    /// Put the layout back at the next login without offering first. Only means anything with
    /// `remember` on, since with it off there is nothing recorded to put back.
    pub reopen_without_asking: bool,
}

/// What programs are allowed to do without being asked again.
///
/// Only the compositor's own card adds to these lists, when someone answers "Always allow". The
/// Settings app can take an entry away; it can never put one in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Privacy {
    /// Programs allowed to record the screen without asking, by the full path of the executable
    /// that asked, as `/proc/<pid>/exe` gives it. Empty to begin with: every program asks the
    /// first time. Take an entry away to be asked again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub screen_capture_allowed: Vec<String>,
}

impl Privacy {
    /// Whether this executable has been allowed to record the screen already.
    pub fn allows_capture(&self, exe: &str) -> bool {
        self.screen_capture_allowed.iter().any(|it| it == exe)
    }

    /// Remembers this executable. Saying yes twice doesn't list it twice.
    pub fn allow_capture(&mut self, exe: String) {
        if !self.allows_capture(&exe) {
            self.screen_capture_allowed.push(exe);
        }
    }

    /// Forgets it, so it is asked again the next time.
    pub fn forget_capture(&mut self, exe: &str) {
        self.screen_capture_allowed.retain(|it| it != exe);
    }
}

/// The clipboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Clipboard {
    /// Keep the last things copied, text and pictures, for Super+V to paste again. In memory
    /// only, forgotten at logout, and never what a password manager marks as secret. Turning it
    /// off forgets everything kept.
    pub history: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self { history: true }
    }
}

/// The lock screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Lock {
    /// Lock after this many minutes with no keyboard or mouse input; 0 never does. Off by
    /// default: only Super+L, quick settings, `loginctl lock-session` and sleep lock.
    pub after_idle_mins: u64,
    /// Lock just before the machine sleeps, so it wakes up locked.
    pub before_sleep: bool,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            after_idle_mins: 0,
            before_sleep: true,
        }
    }
}

/// The rings that mark what's selected. Each is `#rrggbb`; a value that isn't falls back to the
/// default, so a mistyped hand edit leaves the desktop usable rather than colourless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Borders {
    /// The focused window's ring, and the keyboard's ring in quick settings and the notification
    /// centre: one colour for "this is what the keys are aimed at".
    pub selected_tile: String,
    /// Bullet time's rings around the chosen window and workspace, and its workspace frames.
    pub bullet_time: String,
}

/// Ice cyan. It sits 73° of hue from the code rain, so the ring reads against it rather than
/// dissolving into it, and it is Slipstream's colour for what's active.
pub const SELECTED_TILE: &str = "#42d3ff";
/// Slipstream's amber, near-opposite the ring's cyan: cold means the live desktop, warm the slow
/// world.
pub const BULLET_TIME: &str = "#ffb547";

impl Default for Borders {
    fn default() -> Self {
        Self {
            selected_tile: SELECTED_TILE.to_string(),
            bullet_time: BULLET_TIME.to_string(),
        }
    }
}

impl Borders {
    pub fn selected_tile_rgb(&self) -> [f32; 3] {
        rgb(&self.selected_tile).unwrap_or_else(|| rgb(SELECTED_TILE).expect("a valid default"))
    }

    pub fn bullet_time_rgb(&self) -> [f32; 3] {
        rgb(&self.bullet_time).unwrap_or_else(|| rgb(BULLET_TIME).expect("a valid default"))
    }
}

/// `#rrggbb` or `rrggbb`, either case, as three channels from 0 to 1. Anything else is `None`.
pub fn rgb(text: &str) -> Option<[f32; 3]> {
    let digits = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| {
        u8::from_str_radix(&digits[at..at + 2], 16)
            .ok()
            .map(|value| value as f32 / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{HEADER, parse};

    #[test]
    fn an_empty_file_means_every_default() {
        let settings = parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.wallpaper.fade_after_secs, 120);
        assert_eq!(settings.wallpaper.variations, ["slipstream"]);
        assert_eq!(settings.wallpaper.change_every_mins, 10);
        assert!(!settings.motion.reduced);
        assert!(settings.motion.close_into_rain);
        assert_eq!(settings.motion.effects, Effects::Slipstream);
        assert!(!settings.display.night_light);
        assert!(!settings.notifications.do_not_disturb);
        assert!(settings.notifications.wait_while_typing);
        assert_eq!(settings.notifications.longest_wait_mins, 15);
        assert!(settings.notifications.sounds);
        assert_eq!(settings.notifications.sound_theme, "ocean");
        assert!(!settings.session.remember);
        assert!(!settings.session.reopen_without_asking);
        assert_eq!(
            settings.borders.selected_tile_rgb().map(round3),
            [0.259, 0.827, 1.0]
        );
        assert_eq!(
            settings.borders.bullet_time_rgb().map(round3),
            [1.0, 0.71, 0.278]
        );
    }

    #[test]
    fn effects_are_read_by_name() {
        let settings = parse("[motion]\neffects = \"slipstream\"\n").unwrap();
        assert_eq!(settings.motion.effects, Effects::Slipstream);
        assert!(
            settings.motion.close_into_rain,
            "the rest keep their defaults"
        );
    }

    #[test]
    fn lock_defaults() {
        let settings = parse("").unwrap();
        assert_eq!(settings.lock.after_idle_mins, 0, "nothing locks by itself");
        assert_eq!(settings.display.screen_off_mins, 15);
        assert!(settings.lock.before_sleep);
        let set = parse("[lock]\nafter-idle-mins = 5\nbefore-sleep = false\n").unwrap();
        assert_eq!(
            set.lock,
            Lock {
                after_idle_mins: 5,
                before_sleep: false
            }
        );
    }

    fn round3(channel: f32) -> f32 {
        (channel * 1000.0).round() / 1000.0
    }

    #[test]
    fn a_colour_is_read_with_or_without_its_hash_in_either_case() {
        assert_eq!(rgb("#000000"), Some([0.0, 0.0, 0.0]));
        assert_eq!(rgb("FFFFFF"), Some([1.0, 1.0, 1.0]));
        assert_eq!(rgb(" #ffb547 "), rgb("#FFB547"));
    }

    #[test]
    fn a_colour_that_is_not_a_colour_falls_back_to_the_default() {
        for text in ["", "#fff", "mint", "#12345g", "#1234567", "rgb(1,2,3)"] {
            assert_eq!(rgb(text), None, "{text}");
        }
        let borders = Borders {
            selected_tile: "mint".into(),
            bullet_time: "#12345g".into(),
        };
        assert_eq!(borders.selected_tile_rgb(), rgb(SELECTED_TILE).unwrap());
        assert_eq!(borders.bullet_time_rgb(), rgb(BULLET_TIME).unwrap());
    }

    #[test]
    fn missing_keys_keep_their_defaults_and_unknown_ones_are_ignored() {
        let settings =
            parse("[motion]\nreduced = true\nfuture-setting = 3\n\n[portal]\nzones = 4\n").unwrap();
        assert!(settings.motion.reduced);
        assert_eq!(settings.wallpaper, Wallpaper::default());
    }

    #[test]
    fn what_is_written_reads_back() {
        let settings = Settings {
            appearance: Appearance {
                colour_scheme: ColourScheme::Light,
            },
            session: Session {
                remember: true,
                reopen_without_asking: true,
            },
            lock: Lock {
                after_idle_mins: 10,
                before_sleep: false,
            },
            wallpaper: Wallpaper {
                fade_after_secs: 300,
                variations: vec!["vortex".into(), "slipstream".into()],
                change_every_mins: 5,
            },
            motion: Motion {
                reduced: true,
                ..Motion::default()
            },
            display: Display {
                night_light: true,
                night_light_schedule: NightSchedule::Custom,
                night_light_from: "22:15".into(),
                night_light_to: "06:45".into(),
                screen_off_mins: 30,
                variable_refresh: VariableRefresh::Always,
                screens: vec![ScreenPlace {
                    monitor: "Made Up MU27 0001".into(),
                    position: Position::Above,
                    align: Align::Centre,
                    mode: "2560x1440@144".into(),
                    scale: 1.25,
                }],
            },
            notifications: Notifications {
                do_not_disturb: true,
                wait_while_typing: false,
                longest_wait_mins: 30,
                sounds: false,
                sound_theme: "freedesktop".into(),
            },
            borders: Borders {
                selected_tile: "#b794ff".into(),
                bullet_time: "#ffcf5c".into(),
            },
            sound: Sound { volume_blip: false },
            clipboard: Clipboard { history: false },
            privacy: Privacy {
                screen_capture_allowed: vec!["/usr/bin/made-up-recorder".into()],
            },
            meter: Meter {
                command: "df --output=pcent /home | report".into(),
                every_secs: 300,
                hidden: vec!["Mail".into()],
            },
            workspaces: Workspaces {
                list: vec![
                    WorkspaceEntry {
                        id: 3,
                        name: "Mail".into(),
                    },
                    WorkspaceEntry {
                        id: 1,
                        name: String::new(),
                    },
                ],
            },
            keys: Keys {
                style: KeyStyle::Omarchy,
            },
            mouse: Mouse {
                speed: 0.5,
                ..Mouse::default()
            },
            touchpad: Touchpad {
                tap_to_click: false,
                ..Touchpad::default()
            },
            keyboard: Keyboard {
                layouts: vec![KeyboardLayout::new("gb", "dvorak")],
                ..Keyboard::default()
            },
        };
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("fade-after-secs = 300"), "{text}");
        assert!(text.contains(r#"style = "omarchy""#), "{text}");
        assert!(text.contains("tap-to-click = false"), "{text}");
        assert!(text.contains(r#"variant = "dvorak""#), "{text}");
        assert!(
            text.contains(r#"variations = ["vortex", "slipstream"]"#),
            "{text}"
        );
        assert!(text.contains("change-every-mins = 5"), "{text}");
        assert!(text.contains("night-light = true"), "{text}");
        assert!(text.contains(r#"mode = "2560x1440@144""#), "{text}");
        assert!(
            text.contains(r#"screen-capture-allowed = ["/usr/bin/made-up-recorder"]"#),
            "{text}"
        );
        assert!(
            text.contains(r#"night-light-schedule = "custom""#),
            "{text}"
        );
        assert!(text.contains("do-not-disturb = true"), "{text}");
        assert!(text.contains("history = false"), "{text}");
        assert!(text.contains(r##"selected-tile = "#b794ff""##), "{text}");
        assert_eq!(parse(&format!("{HEADER}{text}")).unwrap(), settings);
    }

    #[test]
    fn the_capture_allowlist_holds_each_program_once_and_can_forget_one() {
        let mut privacy = Privacy::default();
        assert!(!privacy.allows_capture("/usr/bin/grim"));
        privacy.allow_capture("/usr/bin/grim".into());
        privacy.allow_capture("/usr/bin/grim".into());
        assert_eq!(privacy.screen_capture_allowed, ["/usr/bin/grim"]);
        assert!(privacy.allows_capture("/usr/bin/grim"));
        // A different path is a different program, even with the same name.
        assert!(!privacy.allows_capture("/home/someone/bin/grim"));
        privacy.forget_capture("/usr/bin/grim");
        assert!(!privacy.allows_capture("/usr/bin/grim"));
        assert!(privacy.screen_capture_allowed.is_empty());
    }

    #[test]
    fn an_empty_allowlist_is_left_out_of_the_file() {
        let text = toml::to_string(&Settings::default()).unwrap();
        assert!(!text.contains("screen-capture-allowed"), "{text}");
    }
}

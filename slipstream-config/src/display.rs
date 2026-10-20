//! The screens: night light, turning them off, and where each one sits.

use serde::{Deserialize, Serialize};

use crate::sun;

/// The screens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Display {
    /// Night light: warmer colours on every screen. On a schedule, Slipstream turns it on and off
    /// itself; turning it by hand in between lasts until the schedule's next change.
    pub night_light: bool,
    /// When night light comes on by itself.
    pub night_light_schedule: NightSchedule,
    /// A custom schedule's start and end, `HH:MM` local time; night runs past midnight if the end
    /// is earlier than the start.
    pub night_light_from: String,
    pub night_light_to: String,
    /// Turn the screens off after this many minutes with no keyboard or mouse input; 0 never
    /// does. Held off by the same things that hold off the lock: Awake, a fullscreen window, or an
    /// app asking for the screen to stay awake. The lock screen turns them off after a minute.
    pub screen_off_mins: u64,
    /// When a screen that can vary its refresh rate does, so a game's frames are shown as soon
    /// as they're ready rather than on the screen's fixed beat.
    pub variable_refresh: VariableRefresh,
    /// Where each screen sits, for the ones that shouldn't simply follow the one before. A screen
    /// with no entry goes to the right of the one before it, tops level.
    #[serde(rename = "screen", skip_serializing_if = "Vec::is_empty")]
    pub screens: Vec<ScreenPlace>,
}

/// Where one screen sits against the one before it in the row.
// No `Eq`: a scale is a float.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ScreenPlace {
    /// The screen this is about, as `screens.toml` knows it: what its EDID says, else its
    /// connector's name.
    pub monitor: String,
    pub position: Position,
    pub align: Align,
    /// The mode to run it in, as `1920x1080@60` or just `1920x1080` for any rate at that size.
    /// Empty leaves it on the one the screen itself asks for. A mode the screen turns out not to
    /// have is ignored, so a file carried to another desk doesn't blank a monitor.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// What to scale it by. Zero works it out from the screen's own size and resolution.
    #[serde(skip_serializing_if = "is_zero")]
    pub scale: f64,
}

pub(crate) fn is_zero(scale: &f64) -> bool {
    *scale == 0.0
}

/// The scales a screen may be set to. Below a half nothing is readable and above four nothing
/// fits; the same range the automatic one is clamped to.
pub const SCALE_RANGE: (f64, f64) = (0.5, 4.0);

/// Which side of the screen before it a screen sits on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    #[default]
    RightOf,
    LeftOf,
    Above,
    Below,
}

/// How a screen lines up with the one before it across the other axis: beside it, whether their
/// top edges, middles or bottom edges are level; above or below it, their left edges, middles or
/// right edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
}

impl Position {
    pub const ALL: [Position; 4] = [
        Position::RightOf,
        Position::LeftOf,
        Position::Above,
        Position::Below,
    ];

    /// Side by side rather than stacked.
    pub fn beside(self) -> bool {
        matches!(self, Position::RightOf | Position::LeftOf)
    }

    /// What it's called in the Settings app, with `other` the screen it's placed against.
    pub fn label(self, other: &str) -> String {
        match self {
            Position::RightOf => format!("Right of {other}"),
            Position::LeftOf => format!("Left of {other}"),
            Position::Above => format!("Above {other}"),
            Position::Below => format!("Below {other}"),
        }
    }
}

impl Align {
    pub const ALL: [Align; 3] = [Align::Start, Align::Centre, Align::End];

    /// What it's called in the Settings app: the edges differ depending on which way the screens
    /// are stacked.
    pub fn label(self, beside: bool) -> &'static str {
        match (self, beside) {
            (Align::Start, true) => "Tops level",
            (Align::Centre, true) => "Middles level",
            (Align::End, true) => "Bottoms level",
            (Align::Start, false) => "Left edges level",
            (Align::Centre, false) => "Middles level",
            (Align::End, false) => "Right edges level",
        }
    }
}

impl Display {
    /// Where `monitor` sits, or the default of "to the right, tops level".
    pub fn place(&self, monitor: &str) -> ScreenPlace {
        self.screens
            .iter()
            .find(|place| place.monitor == monitor)
            .cloned()
            .unwrap_or_else(|| ScreenPlace {
                monitor: monitor.to_string(),
                ..ScreenPlace::default()
            })
    }

    /// Records where `monitor` sits. Leaves whatever else is set for it — a screen's mode and scale are
    /// nothing to do with where it is, and rewriting the whole entry would drop them every time
    /// the arrangement changed.
    pub fn set_place(&mut self, monitor: &str, position: Position, align: Align) {
        let slot = self.entry(monitor);
        slot.position = position;
        slot.align = align;
    }

    /// The mode to run `monitor` in, as `1920x1080@60`. Empty goes back to the screen's own
    /// preferred mode.
    pub fn set_mode(&mut self, monitor: &str, mode: &str) {
        self.entry(monitor).mode = mode.to_string();
    }

    /// What to scale `monitor` by. Zero goes back to working it out from the screen itself.
    pub fn set_scale(&mut self, monitor: &str, scale: f64) {
        self.entry(monitor).scale = if scale == 0.0 {
            0.0
        } else {
            scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1)
        };
    }

    /// What is set for `monitor`, making an entry for it if there is none yet.
    fn entry(&mut self, monitor: &str) -> &mut ScreenPlace {
        if let Some(index) = self.screens.iter().position(|held| held.monitor == monitor) {
            return &mut self.screens[index];
        }
        self.screens.push(ScreenPlace {
            monitor: monitor.to_string(),
            ..ScreenPlace::default()
        });
        self.screens.last_mut().expect("just pushed")
    }
}

impl Default for Display {
    fn default() -> Self {
        Self {
            night_light: false,
            night_light_schedule: NightSchedule::Off,
            night_light_from: "21:00".into(),
            night_light_to: "07:00".into(),
            screen_off_mins: 15,
            variable_refresh: VariableRefresh::Games,
            screens: Vec::new(),
        }
    }
}

/// When a screen that can vary its refresh rate (FreeSync, Adaptive-Sync) does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VariableRefresh {
    /// Every screen keeps its fixed rate.
    Off,
    /// While a fullscreen game is drawn straight to the screen. Elsewhere the rate stays fixed,
    /// so the pointer and animations move evenly.
    #[default]
    Games,
    /// Whenever the screen can.
    Always,
}

impl VariableRefresh {
    pub const ALL: [VariableRefresh; 3] = [
        VariableRefresh::Off,
        VariableRefresh::Games,
        VariableRefresh::Always,
    ];

    /// What it's called in the Settings app.
    pub fn label(self) -> &'static str {
        match self {
            VariableRefresh::Off => "Off",
            VariableRefresh::Games => "For fullscreen games",
            VariableRefresh::Always => "Always",
        }
    }
}

/// Night light's schedule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NightSchedule {
    /// Only by hand.
    #[default]
    Off,
    /// From sunset to sunrise where the time zone's city is.
    Sunset,
    /// Between `night-light-from` and `night-light-to`.
    Custom,
}

/// How long night light takes to warm up or cool down when its schedule turns it, in minutes.
pub const NIGHT_FADE_MINS: f64 = 30.0;

impl Display {
    /// Tonight's night, in local minutes after midnight: when it starts and ends for `date`, at a
    /// clock `offset_mins` ahead of UTC. `None` without a schedule, and for sunset where the sun
    /// doesn't set or the time zone's place is unknown, in which case the custom hours stand in.
    pub fn night_window(
        &self,
        date: (i64, u32, u32),
        offset_mins: f64,
        location: Option<(f64, f64)>,
    ) -> Option<(f64, f64)> {
        let custom = || {
            Some((
                sun::parse_time(&self.night_light_from).unwrap_or(21.0 * 60.0),
                sun::parse_time(&self.night_light_to).unwrap_or(7.0 * 60.0),
            ))
        };
        match self.night_light_schedule {
            NightSchedule::Off => None,
            NightSchedule::Custom => custom(),
            NightSchedule::Sunset => location
                .and_then(|(lat, lon)| sun::sun_times(lat, lon, date, offset_mins))
                .map(|(sunrise, sunset)| (sunset, sunrise))
                .or_else(custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screens_place_mode_and_scale_dont_overwrite_each_other() {
        let mut display = Display::default();
        display.set_place("MU27", Position::Above, Align::Centre);
        display.set_mode("MU27", "2560x1440@144");
        display.set_scale("MU27", 1.5);
        // Moving it about must not forget what it is set to, and vice versa.
        display.set_place("MU27", Position::LeftOf, Align::End);
        let place = display.place("MU27");
        assert_eq!(place.position, Position::LeftOf);
        assert_eq!(place.align, Align::End);
        assert_eq!(place.mode, "2560x1440@144");
        assert_eq!(place.scale, 1.5);
        // One entry per screen, however many times it is set.
        assert_eq!(display.screens.len(), 1);
    }

    #[test]
    fn a_scale_is_kept_inside_what_can_be_read() {
        let mut display = Display::default();
        display.set_scale("MU27", 99.0);
        assert_eq!(display.place("MU27").scale, SCALE_RANGE.1);
        display.set_scale("MU27", 0.01);
        assert_eq!(display.place("MU27").scale, SCALE_RANGE.0);
        // Zero is not a scale: it means work it out from the screen.
        display.set_scale("MU27", 0.0);
        assert_eq!(display.place("MU27").scale, 0.0);
    }
}

//! The mouse, the touchpad and the keyboard: how the pointer moves and scrolls, how keys repeat,
//! and which keyboard layouts there are.

use serde::{Deserialize, Serialize};

/// Mice, trackballs and pointing sticks: every pointer that isn't a touchpad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Mouse {
    /// From -1 (slowest) to 1 (fastest); 0 is the device's usual speed.
    pub speed: f64,
    /// The wheel moves the page the way it turns, as a touchpad's natural scrolling does.
    pub natural_scrolling: bool,
    pub acceleration: Acceleration,
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            speed: 0.0,
            natural_scrolling: false,
            acceleration: Acceleration::Adaptive,
        }
    }
}

impl Mouse {
    pub fn speed(&self) -> f64 {
        clamp_speed(self.speed)
    }
}

/// How the pointer's travel follows the hand's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Acceleration {
    /// Faster movements carry the pointer further.
    #[default]
    Adaptive,
    /// The same movement always carries it the same distance, however fast.
    Flat,
}

impl Acceleration {
    pub const ALL: [Acceleration; 2] = [Acceleration::Adaptive, Acceleration::Flat];

    pub fn label(self) -> &'static str {
        match self {
            Acceleration::Adaptive => "Adaptive",
            Acceleration::Flat => "Flat",
        }
    }
}

/// Touchpads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Touchpad {
    /// A tap is a click: one finger the left button, two the right, three the middle.
    pub tap_to_click: bool,
    /// The content follows the fingers, as on a phone. On by default, as Windows, macOS and
    /// GNOME have it for touchpads.
    pub natural_scrolling: bool,
    /// From -1 (slowest) to 1 (fastest); 0 is the touchpad's usual speed.
    pub speed: f64,
    /// The touchpad ignores touches while keys are being typed, so a palm can't move the caret.
    pub disable_while_typing: bool,
    pub scrolling: Scrolling,
}

impl Default for Touchpad {
    fn default() -> Self {
        Self {
            tap_to_click: true,
            natural_scrolling: true,
            speed: 0.0,
            disable_while_typing: true,
            scrolling: Scrolling::TwoFinger,
        }
    }
}

impl Touchpad {
    pub fn speed(&self) -> f64 {
        clamp_speed(self.speed)
    }
}

/// How a touchpad scrolls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scrolling {
    #[default]
    TwoFinger,
    /// One finger along the right edge, or the bottom edge to scroll sideways.
    Edge,
}

impl Scrolling {
    pub const ALL: [Scrolling; 2] = [Scrolling::TwoFinger, Scrolling::Edge];

    pub fn label(self) -> &'static str {
        match self {
            Scrolling::TwoFinger => "Two fingers",
            Scrolling::Edge => "Along the edge",
        }
    }
}

/// A hand-edited speed outside -1 to 1, or not a number at all, is kept in range.
fn clamp_speed(speed: f64) -> f64 {
    if speed.is_nan() {
        0.0
    } else {
        speed.clamp(-1.0, 1.0)
    }
}

/// The keyboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Keyboard {
    /// How long a key is held before it starts repeating, in milliseconds.
    pub repeat_delay_ms: u32,
    /// How many times a second a held key repeats.
    pub repeat_rate: u32,
    /// The layouts, the first one in use at login. Empty means the system's own, as set when the
    /// system was installed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layouts: Vec<KeyboardLayout>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            repeat_delay_ms: 200,
            repeat_rate: 25,
            layouts: Vec::new(),
        }
    }
}

impl Keyboard {
    /// `repeat_delay_ms`, kept between a tenth of a second and two seconds.
    pub fn repeat_delay(&self) -> u32 {
        self.repeat_delay_ms.clamp(100, 2000)
    }

    /// `repeat_rate`, kept between 1 and 100 a second.
    pub fn repeat_rate(&self) -> u32 {
        self.repeat_rate.clamp(1, 100)
    }

    /// The layouts in use: the chosen ones, or the system's when none are.
    pub fn layouts_or(&self, system: &[KeyboardLayout]) -> Vec<KeyboardLayout> {
        if self.layouts.is_empty() {
            system.to_vec()
        } else {
            self.layouts.clone()
        }
    }

    /// The xkb layout and variant lists for the chosen layouts, comma-separated, or `None` when
    /// none are chosen and the system's should be left to apply.
    pub fn xkb_names(&self) -> Option<(String, String)> {
        if self.layouts.is_empty() {
            return None;
        }
        let join = |part: fn(&KeyboardLayout) -> &str| {
            self.layouts.iter().map(part).collect::<Vec<_>>().join(",")
        };
        Some((
            join(|layout| layout.layout.trim()),
            join(|layout| layout.variant.trim()),
        ))
    }

    /// Chooses `layouts`, or none when they are the system's own, so the system's choice is
    /// followed again if it changes.
    pub fn set_layouts(&mut self, layouts: Vec<KeyboardLayout>, system: &[KeyboardLayout]) {
        self.layouts = if layouts == system {
            Vec::new()
        } else {
            layouts
        };
    }
}

/// One keyboard layout, by its xkb names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyboardLayout {
    /// Such as `gb` or `de`.
    pub layout: String,
    /// Such as `dvorak`; empty for the layout's basic form.
    pub variant: String,
}

impl KeyboardLayout {
    pub fn new(layout: &str, variant: &str) -> Self {
        Self {
            layout: layout.to_string(),
            variant: variant.to_string(),
        }
    }
}

/// The system's layouts, from the comma-separated lists the session reads from the system's
/// keyboard settings into `XKB_DEFAULT_LAYOUT` and `XKB_DEFAULT_VARIANT`.
pub fn system_layouts(layout: &str, variant: &str) -> Vec<KeyboardLayout> {
    let mut variants = variant.split(',');
    layout
        .split(',')
        .map(str::trim)
        .filter(|layout| !layout.is_empty())
        .map(|layout| KeyboardLayout::new(layout, variants.next().unwrap_or("").trim()))
        .collect()
}

/// The system's layouts as this process was started with them, or US English, which is what
/// xkb falls back to when nothing says otherwise.
pub fn system_layouts_from_env() -> Vec<KeyboardLayout> {
    let var = |name| std::env::var(name).unwrap_or_default();
    let layouts = system_layouts(&var("XKB_DEFAULT_LAYOUT"), &var("XKB_DEFAULT_VARIANT"));
    if layouts.is_empty() {
        vec![KeyboardLayout::new("us", "")]
    } else {
        layouts
    }
}

/// Whether xkb options already switch layouts with a key of their own (`grp:…`), in which case
/// Slipstream's switch key stands aside rather than switching twice.
pub fn options_switch_layouts(options: &str) -> bool {
    options
        .split(',')
        .any(|option| option.trim().starts_with("grp:"))
}

/// What the bar shows for layout `index` of `layouts`: its code in capitals, with the variant
/// when another layout in the list has the same code.
pub fn layout_label(layouts: &[KeyboardLayout], index: usize) -> Option<String> {
    let this = layouts.get(index)?;
    let shared = layouts
        .iter()
        .filter(|other| other.layout == this.layout)
        .count()
        > 1;
    let mut label = this.layout.to_uppercase();
    if shared && !this.variant.is_empty() {
        label.push(' ');
        label.push_str(&this.variant);
    }
    Some(label)
}

/// A layout xkb knows, with its name as people know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChoice {
    pub layout: KeyboardLayout,
    /// Such as `English (UK)` or `English (UK, Dvorak)`.
    pub name: String,
}

/// Where xkeyboard-config lists its layouts, newest location first.
pub const LAYOUT_LISTS: [&str; 2] = [
    "/usr/share/xkeyboard-config-2/rules/evdev.lst",
    "/usr/share/X11/xkb/rules/evdev.lst",
];

/// Every layout and variant in an xkeyboard-config `.lst` file, sorted by name.
pub fn layout_catalogue(text: &str) -> Vec<LayoutChoice> {
    let mut section = "";
    let mut choices = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('!') {
            section = name.trim();
            continue;
        }
        let line = line.trim();
        let Some((code, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim();
        match section {
            "layout" => choices.push(LayoutChoice {
                layout: KeyboardLayout::new(code, ""),
                name: rest.to_string(),
            }),
            "variant" => {
                if let Some((layout, name)) = rest.split_once(": ") {
                    choices.push(LayoutChoice {
                        layout: KeyboardLayout::new(layout, code),
                        name: name.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    choices.sort_by(|a, b| a.name.cmp(&b.name));
    choices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::parse;

    #[test]
    fn defaults_keep_the_keyboard_as_it_was_and_scroll_touchpads_naturally() {
        let settings = parse("").unwrap();
        assert_eq!(settings.keyboard.repeat_delay(), 200);
        assert_eq!(settings.keyboard.repeat_rate(), 25);
        assert!(settings.keyboard.layouts.is_empty(), "the system's layout");
        assert_eq!(settings.keyboard.xkb_names(), None);
        assert!(settings.touchpad.tap_to_click);
        assert!(settings.touchpad.natural_scrolling);
        assert!(settings.touchpad.disable_while_typing);
        assert_eq!(settings.touchpad.scrolling, Scrolling::TwoFinger);
        assert!(!settings.mouse.natural_scrolling);
        assert_eq!(settings.mouse.acceleration, Acceleration::Adaptive);
        assert_eq!(settings.mouse.speed(), 0.0);
    }

    #[test]
    fn input_settings_read_back() {
        let settings = parse(
            "[mouse]\nspeed = -0.4\nnatural-scrolling = true\nacceleration = \"flat\"\n\n\
             [touchpad]\ntap-to-click = false\nscrolling = \"edge\"\nspeed = 0.3\n\n\
             [keyboard]\nrepeat-delay-ms = 500\nrepeat-rate = 30\n\n\
             [[keyboard.layouts]]\nlayout = \"gb\"\n\n\
             [[keyboard.layouts]]\nlayout = \"de\"\nvariant = \"nodeadkeys\"\n",
        )
        .unwrap();
        assert_eq!(settings.mouse.speed(), -0.4);
        assert!(settings.mouse.natural_scrolling);
        assert_eq!(settings.mouse.acceleration, Acceleration::Flat);
        assert!(!settings.touchpad.tap_to_click);
        assert!(
            settings.touchpad.natural_scrolling,
            "the rest keep defaults"
        );
        assert_eq!(settings.touchpad.scrolling, Scrolling::Edge);
        assert_eq!(settings.keyboard.repeat_delay(), 500);
        assert_eq!(
            settings.keyboard.xkb_names(),
            Some(("gb,de".into(), ",nodeadkeys".into()))
        );
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(parse(&text).unwrap(), settings);
    }

    #[test]
    fn no_chosen_layouts_leaves_them_out_of_the_file() {
        let text = toml::to_string(&crate::Settings::default()).unwrap();
        assert!(!text.contains("layouts"), "{text}");
    }

    #[test]
    fn hand_edited_numbers_are_kept_in_range() {
        let settings = parse(
            "[mouse]\nspeed = 7.0\n[touchpad]\nspeed = -3.0\n\
             [keyboard]\nrepeat-delay-ms = 5\nrepeat-rate = 0\n",
        )
        .unwrap();
        assert_eq!(settings.mouse.speed(), 1.0);
        assert_eq!(settings.touchpad.speed(), -1.0);
        assert_eq!(settings.keyboard.repeat_delay(), 100);
        assert_eq!(settings.keyboard.repeat_rate(), 1);
        let nan = Mouse {
            speed: f64::NAN,
            ..Mouse::default()
        };
        assert_eq!(nan.speed(), 0.0);
    }

    #[test]
    fn the_system_layouts_pair_each_layout_with_its_variant() {
        assert_eq!(
            system_layouts("gb,de", ",nodeadkeys"),
            [
                KeyboardLayout::new("gb", ""),
                KeyboardLayout::new("de", "nodeadkeys")
            ]
        );
        assert_eq!(system_layouts("fr", ""), [KeyboardLayout::new("fr", "")]);
        assert!(system_layouts("", "").is_empty());
    }

    #[test]
    fn choosing_the_system_layouts_goes_back_to_following_the_system() {
        let system = [KeyboardLayout::new("gb", "")];
        let mut keyboard = Keyboard::default();
        keyboard.set_layouts(
            vec![system[0].clone(), KeyboardLayout::new("de", "")],
            &system,
        );
        assert_eq!(keyboard.layouts.len(), 2);
        assert_eq!(keyboard.layouts_or(&system).len(), 2);
        keyboard.set_layouts(system.to_vec(), &system);
        assert!(keyboard.layouts.is_empty());
        assert_eq!(keyboard.layouts_or(&system), system);
    }

    #[test]
    fn a_layout_switch_option_is_recognised() {
        assert!(options_switch_layouts("compose:ralt,grp:alt_shift_toggle"));
        assert!(!options_switch_layouts("ctrl:nocaps"));
        assert!(!options_switch_layouts(""));
    }

    #[test]
    fn the_bar_label_is_the_code_and_the_variant_only_when_needed() {
        let layouts = [
            KeyboardLayout::new("us", ""),
            KeyboardLayout::new("us", "intl"),
            KeyboardLayout::new("de", "nodeadkeys"),
        ];
        assert_eq!(layout_label(&layouts, 0).as_deref(), Some("US"));
        assert_eq!(layout_label(&layouts, 1).as_deref(), Some("US intl"));
        assert_eq!(layout_label(&layouts, 2).as_deref(), Some("DE"));
        assert_eq!(layout_label(&layouts, 3), None);
    }

    #[test]
    fn the_catalogue_lists_layouts_and_variants_by_name() {
        let text = "! model\n  pc105           Generic 105-key PC\n\n\
                    ! layout\n  gb              English (UK)\n  de              German\n\n\
                    ! variant\n  dvorak          gb: English (UK, Dvorak)\n\n\
                    ! option\n  grp                  Switching to another layout\n";
        let catalogue = layout_catalogue(text);
        let names: Vec<&str> = catalogue
            .iter()
            .map(|choice| choice.name.as_str())
            .collect();
        assert_eq!(names, ["English (UK)", "English (UK, Dvorak)", "German"]);
        assert_eq!(catalogue[1].layout, KeyboardLayout::new("gb", "dvorak"));
    }
}

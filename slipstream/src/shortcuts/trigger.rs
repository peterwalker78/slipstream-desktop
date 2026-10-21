//! A shortcut's keys: read from the way the XDG shortcuts specification writes them
//! (`CTRL+ALT+m`, `LOGO+F9`), written back the same way, and checked against everything the
//! keys already mean.
//!
//! A shortcut an app keeps works while another app has the keyboard, so its keys never reach that
//! app. It can't take a key Slipstream uses in any keyboard style (switching style would quietly
//! break one or the other), a combination kept for input methods, screen readers and games, or a
//! key that types: a bare letter would stop working in every text box.

use smithay::input::keyboard::{Keysym, xkb};

use slipstream_config::KeyStyle;

use crate::keys::{self, Mods};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    pub mods: Mods,
    /// Lower case for letters, as the keyboard's unshifted symbol reads.
    pub key: Keysym,
}

/// Why keys can't be a shortcut, in words for the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Slipstream does something with them.
    Slipstreams,
    /// Input methods, screen readers, games or apps rely on them.
    Kept,
    /// They type, or move around in text.
    Types,
    /// Another app has them; its name.
    Taken(String),
    /// They are one of the person's own shortcuts, opening this app.
    Opens(String),
}

impl Refusal {
    /// The sentence the card shows, naming the keys.
    pub fn say(&self, keys: &str) -> String {
        match self {
            Refusal::Slipstreams => format!("{keys} is one of Slipstream's own shortcuts."),
            Refusal::Kept => {
                format!("{keys} is kept for typing aids, screen readers, games and apps.")
            }
            Refusal::Types => {
                format!("{keys} types or edits text. Hold Ctrl, Alt or Super with it.")
            }
            Refusal::Taken(app) => format!("{app} already uses {keys}."),
            Refusal::Opens(app) => format!("{keys} already opens {app}."),
        }
    }
}

/// Keys that need nothing held with them: they type nothing and move nothing in a text box.
fn stands_alone(key: Keysym) -> bool {
    let raw = key.raw();
    (Keysym::F1.raw()..=Keysym::F35.raw()).contains(&raw)
        || matches!(key, Keysym::Pause | Keysym::Scroll_Lock)
        || keys::is_hardware_key(key)
}

/// Letters are kept lower case, which is how the unshifted key reads whatever the layout's
/// Shift does.
fn lower(key: Keysym) -> Keysym {
    let raw = key.raw();
    if (Keysym::A.raw()..=Keysym::Z.raw()).contains(&raw) {
        Keysym::from(raw + 0x20)
    } else {
        key
    }
}

impl Trigger {
    /// Reads `CTRL+ALT+m`. Modifier names are taken in any case, and `SUPER` and `META` as well
    /// as the specification's `LOGO`; `NUM` (Num Lock) is accepted and ignored, since Slipstream
    /// matches keys the same with Num Lock on or off. `None` for anything else.
    pub fn parse(spec: &str) -> Option<Self> {
        let mut parts: Vec<&str> = spec.trim().split('+').map(str::trim).collect();
        // `CTRL++` is Ctrl and the plus key.
        if spec.trim().ends_with("++") {
            parts.truncate(parts.len().saturating_sub(2));
            parts.push("plus");
        }
        let name = parts.pop().filter(|name| !name.is_empty())?;
        let mut mods = Mods::default();
        for part in parts {
            match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => mods.ctrl = true,
                "ALT" => mods.alt = true,
                "SHIFT" => mods.shift = true,
                "LOGO" | "SUPER" | "META" => mods.logo = true,
                "NUM" => {}
                _ => return None,
            }
        }
        let mut key = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
        if key.raw() == 0 {
            key = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
        }
        (key.raw() != 0 && !key.is_modifier_key()).then(|| Self {
            mods,
            key: lower(key),
        })
    }

    /// The keys pressed, as they arrive: `key` is the unshifted symbol.
    pub fn from_press(mods: Mods, key: Keysym) -> Self {
        Self {
            mods,
            key: lower(key),
        }
    }

    /// As the specification writes it: `LOGO+CTRL+m`.
    pub fn spec(&self) -> String {
        let mut spec = String::new();
        for (held, name) in [
            (self.mods.logo, "LOGO+"),
            (self.mods.ctrl, "CTRL+"),
            (self.mods.alt, "ALT+"),
            (self.mods.shift, "SHIFT+"),
        ] {
            if held {
                spec.push_str(name);
            }
        }
        spec + &xkb::keysym_get_name(self.key)
    }

    /// As a person reads it: `Super+Ctrl+M`.
    pub fn label(&self) -> String {
        keys::label(self.mods, self.key)
    }

    pub fn matches(&self, mods: Mods, key: Keysym) -> bool {
        *self == Self::from_press(mods, key)
    }

    /// Whether these keys can be an app's shortcut. Another app holding them is checked by the
    /// caller, which knows the apps.
    pub fn check(&self) -> Result<(), Refusal> {
        self.check_in(&KeyStyle::ALL)
    }

    /// The same, against the keys of `styles` only.
    pub fn check_in(&self, styles: &[KeyStyle]) -> Result<(), Refusal> {
        let (mods, key) = (self.mods, self.key);
        if styles
            .iter()
            .any(|style| keys::is_reserved_in(*style, mods, key))
        {
            return Err(Refusal::Kept);
        }
        if styles
            .iter()
            .any(|style| keys::action_for(&keys::defaults(*style), mods, key).is_some())
        {
            return Err(Refusal::Slipstreams);
        }
        if !(mods.ctrl || mods.alt || mods.logo) && !stands_alone(key) {
            return Err(Refusal::Types);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> Mods {
        Mods {
            ctrl,
            alt,
            shift,
            logo,
        }
    }

    #[test]
    fn the_specifications_triggers_are_read() {
        let t = Trigger::parse("CTRL+ALT+m").unwrap();
        assert_eq!(t.mods, mods(true, true, false, false));
        assert_eq!(t.key, Keysym::m);
        let t = Trigger::parse("LOGO+F9").unwrap();
        assert_eq!(t.mods, mods(false, false, false, true));
        assert_eq!(t.key, Keysym::F9);
        // An upper-case letter is the same key.
        assert_eq!(Trigger::parse("ctrl+alt+M").unwrap().key, Keysym::m);
        assert_eq!(Trigger::parse("NUM+Pause").unwrap().mods, Mods::default());
        assert_eq!(Trigger::parse("CTRL++").unwrap().key, Keysym::plus);
    }

    #[test]
    fn nonsense_is_not_a_trigger() {
        for spec in ["", "CTRL+", "HYPER+a", "CTRL+notakey", "CTRL+Shift_L"] {
            assert_eq!(Trigger::parse(spec), None, "{spec}");
        }
    }

    #[test]
    fn a_trigger_is_written_as_it_is_read() {
        for spec in ["LOGO+CTRL+m", "ALT+F7", "Pause", "CTRL+ALT+SHIFT+p"] {
            assert_eq!(Trigger::parse(spec).unwrap().spec(), spec);
        }
        assert_eq!(Trigger::parse("CTRL+ALT+m").unwrap().label(), "Ctrl+Alt+M");
    }

    #[test]
    fn a_press_matches_whatever_shift_does_to_the_letter() {
        let t = Trigger::parse("CTRL+ALT+m").unwrap();
        assert!(t.matches(mods(true, true, false, false), Keysym::m));
        assert!(t.matches(mods(true, true, false, false), Keysym::M));
        assert!(!t.matches(mods(true, false, false, false), Keysym::m));
        assert!(!t.matches(mods(true, true, true, false), Keysym::m));
    }

    #[test]
    fn an_app_never_takes_a_key_slipstream_uses_in_any_style() {
        for style in KeyStyle::ALL {
            for binding in keys::defaults(style) {
                let trigger = Trigger::from_press(binding.mods, binding.key);
                assert!(trigger.check().is_err(), "{style:?} {}", trigger.label());
            }
        }
        // Super+L locks the screen.
        assert_eq!(
            Trigger::parse("LOGO+l").unwrap().check(),
            Err(Refusal::Slipstreams)
        );
    }

    #[test]
    fn an_app_never_takes_a_key_kept_for_typing_aids_or_games() {
        for spec in [
            "CTRL+SHIFT+m",
            "CTRL+space",
            "F12",
            "SHIFT+F3",
            "CTRL+ALT+F2",
        ] {
            assert_eq!(
                Trigger::parse(spec).unwrap().check(),
                Err(Refusal::Kept),
                "{spec}"
            );
        }
    }

    #[test]
    fn an_app_never_takes_a_key_that_types() {
        for spec in ["m", "SHIFT+m", "space", "Return", "Left", "grave"] {
            assert_eq!(
                Trigger::parse(spec).unwrap().check(),
                Err(Refusal::Types),
                "{spec}"
            );
        }
    }

    #[test]
    fn keys_that_type_nothing_can_stand_alone() {
        for spec in ["Pause", "Scroll_Lock", "F9", "F13", "CTRL+ALT+m", "ALT+F7"] {
            assert_eq!(Trigger::parse(spec).unwrap().check(), Ok(()), "{spec}");
        }
    }
}

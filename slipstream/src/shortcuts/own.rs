//! Shortcuts of the person's own, each opening an app: the settings file's `[[keys.app]]`.
//!
//! One works only while its keys mean nothing else: not a key of the style in force, not one kept
//! for input methods, screen readers and games, not a key that types, and not an app's own
//! shortcut. One that clashes after the style changes is kept, does nothing, and works again if
//! the style changes back; the Settings app says why it does nothing meanwhile.

use slipstream_config::{AppKey, KeyStyle, shortcuts::Granted};

use super::trigger::{Refusal, Trigger};

/// One that works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnKey {
    pub trigger: Trigger,
    /// The app's desktop file ID.
    pub app: String,
    /// The app's name, for the key list.
    pub name: String,
}

/// Why `trigger` can't open an app in `style`, if it can't.
pub fn refusal(style: KeyStyle, trigger: Trigger, granted: &Granted) -> Option<Refusal> {
    if let Err(refusal) = trigger.check_in(&[style]) {
        return Some(refusal);
    }
    granted
        .held_by_another(&trigger.spec(), "")
        .map(|app| Refusal::Taken(app.to_string()))
}

/// The ones that work in `style`, in the file's order. Of two on the same keys, the first works.
pub fn usable(
    style: KeyStyle,
    keys: &[AppKey],
    granted: &Granted,
    name: impl Fn(&str) -> String,
) -> Vec<OwnKey> {
    let mut usable: Vec<OwnKey> = Vec::new();
    for key in keys {
        let Some(trigger) = Trigger::parse(&key.keys) else {
            tracing::warn!(
                keys = key.keys,
                "ignoring a shortcut whose keys can't be read"
            );
            continue;
        };
        if key.app.is_empty()
            || refusal(style, trigger, granted).is_some()
            || usable.iter().any(|own| own.trigger == trigger)
        {
            continue;
        }
        usable.push(OwnKey {
            trigger,
            app: key.app.clone(),
            name: name(&key.app),
        });
    }
    usable
}

/// `slipstream --check-keys`, for the Settings app: a line for each of `specs`, of its keys as
/// the specification writes them, as a person reads them, and why they can't open an app in
/// `style`, empty when they can, separated by tabs. Keys that can't be read have only the reason.
pub fn check_listing(style: KeyStyle, specs: &[String], granted: &Granted) -> String {
    let mut text = String::new();
    for spec in specs {
        let line = match Trigger::parse(spec) {
            Some(trigger) => {
                let label = trigger.label();
                let why = refusal(style, trigger, granted)
                    .map(|refusal| refusal.say(&label))
                    .unwrap_or_default();
                format!("{}\t{label}\t{why}", trigger.spec())
            }
            None => format!("{spec}\t\tThose keys can't be read."),
        };
        text.push_str(&line);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use slipstream_config::shortcuts::Shortcut;

    use super::*;
    use crate::keys;

    fn trigger(spec: &str) -> Trigger {
        Trigger::parse(spec).unwrap()
    }

    fn app_key(keys: &str, app: &str) -> AppKey {
        AppKey {
            keys: keys.into(),
            app: app.into(),
        }
    }

    #[test]
    fn own_shortcuts_never_take_a_key_of_the_style_in_force() {
        for style in KeyStyle::ALL {
            for binding in keys::checked(keys::defaults(style), style) {
                let trigger = Trigger::from_press(binding.mods, binding.key);
                assert!(
                    refusal(style, trigger, &Granted::new()).is_some(),
                    "{style:?} {}",
                    trigger.label()
                );
            }
        }
    }

    #[test]
    fn own_shortcuts_never_take_keys_kept_for_typing_aids_screen_readers_and_games() {
        let kept = [
            "CTRL+ALT+F1",
            "CTRL+ALT+F12",
            "ALT+Sys_Req",
            "CTRL+space",
            "LOGO+period",
            "LOGO+semicolon",
            "CTRL+SHIFT+u",
            "CTRL+SHIFT+g",
            "Insert",
            "LOGO+ALT+s",
            "SHIFT+Tab",
            "F12",
            "SHIFT+F3",
            "CTRL+Tab",
            "ALT+Left",
            "CTRL+ALT+Right",
        ];
        for style in KeyStyle::ALL {
            for spec in kept {
                assert!(
                    refusal(style, trigger(spec), &Granted::new()).is_some(),
                    "{style:?} {spec}"
                );
            }
            // Super+Space is IBus's in the Windows style and the launcher in the others.
            assert!(refusal(style, trigger("LOGO+space"), &Granted::new()).is_some());
        }
        // Keys that type stay the focused app's.
        assert_eq!(
            refusal(KeyStyle::Windows, trigger("g"), &Granted::new()),
            Some(Refusal::Types)
        );
    }

    #[test]
    fn an_apps_own_shortcut_is_not_taken() {
        let mut granted = Granted::new();
        let spec = trigger("CTRL+ALT+m").spec();
        granted.set(
            "org.example.Chat",
            "Chat",
            vec![Shortcut {
                id: "talk".into(),
                trigger: spec.clone(),
                ..Shortcut::default()
            }],
        );
        assert_eq!(
            refusal(KeyStyle::Windows, trigger(&spec), &granted),
            Some(Refusal::Taken("Chat".into()))
        );
    }

    /// Keys free in `free` that `taken` uses for itself.
    fn clash(free: KeyStyle, taken: KeyStyle) -> Trigger {
        keys::checked(keys::defaults(taken), taken)
            .into_iter()
            .map(|b| Trigger::from_press(b.mods, b.key))
            .find(|t| refusal(free, *t, &Granted::new()).is_none())
            .expect("the styles differ somewhere")
    }

    #[test]
    fn changing_style_keeps_own_shortcuts_and_a_clash_only_waits() {
        let keys = vec![app_key(
            &clash(KeyStyle::Windows, KeyStyle::Mac).spec(),
            "org.example.Files",
        )];
        let name = |id: &str| id.to_string();
        let windows = usable(KeyStyle::Windows, &keys, &Granted::new(), name);
        assert_eq!(windows.len(), 1);
        assert!(usable(KeyStyle::Mac, &keys, &Granted::new(), name).is_empty());
        // Nothing is taken out of the file, so it works again in the Windows style.
        assert_eq!(
            usable(KeyStyle::Windows, &keys, &Granted::new(), name),
            windows
        );
    }

    #[test]
    fn of_two_own_shortcuts_on_the_same_keys_the_first_works() {
        let keys = vec![
            app_key("LOGO+CTRL+g", "org.example.Files"),
            app_key("SUPER+ctrl+G", "org.example.Notes"),
            app_key("not keys", "org.example.Notes"),
        ];
        let own = usable(KeyStyle::Windows, &keys, &Granted::new(), |id| {
            id.rsplit('.').next().unwrap().to_string()
        });
        assert_eq!(own.len(), 1);
        assert_eq!(
            (own[0].app.as_str(), own[0].name.as_str()),
            ("org.example.Files", "Files")
        );
    }

    #[test]
    fn the_settings_app_is_told_why_keys_cant_be_used() {
        let taken = clash(KeyStyle::Mac, KeyStyle::Windows);
        let specs = [
            "LOGO+CTRL+g".to_string(),
            taken.spec(),
            "nonsense+".to_string(),
        ];
        let listing = check_listing(KeyStyle::Windows, &specs, &Granted::new());
        let lines: Vec<Vec<&str>> = listing.lines().map(|l| l.split('\t').collect()).collect();
        assert_eq!(lines[0], ["LOGO+CTRL+g", "Super+Ctrl+G", ""]);
        assert_eq!(
            lines[1][2],
            format!("{} is one of Slipstream's own shortcuts.", taken.label())
        );
        assert_eq!(lines[2][2], "Those keys can't be read.");
    }
}

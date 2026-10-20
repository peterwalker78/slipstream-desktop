//! Mice and touchpads set up as the settings say: speed, acceleration, scrolling, tapping and
//! ignoring the touchpad while typing. Applied to each device as it's plugged in, and to every
//! device again when the settings change. Only the hardware backend has libinput devices; nested,
//! the host desktop's settings apply.

use slipstream_config::{Acceleration, Mouse, Scrolling, Touchpad};
use smithay::reexports::input::{self as libinput, AccelProfile, ScrollMethod};

/// One libinput setting to change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Call {
    Speed(f64),
    Profile(AccelProfile),
    NaturalScrolling(bool),
    TapToClick(bool),
    DisableWhileTyping(bool),
    Scroll(ScrollMethod),
}

/// What a device can be set to, as libinput reports it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Supports {
    /// It takes taps, which only touchpads do.
    pub touchpad: bool,
    pub speed: bool,
    pub profiles: Vec<AccelProfile>,
    pub natural_scrolling: bool,
    pub disable_while_typing: bool,
    pub scroll_methods: Vec<ScrollMethod>,
}

impl Supports {
    fn of(device: &libinput::Device) -> Self {
        Self {
            touchpad: device.config_tap_finger_count() > 0,
            speed: device.config_accel_is_available(),
            profiles: device.config_accel_profiles(),
            natural_scrolling: device.config_scroll_has_natural_scroll(),
            disable_while_typing: device.config_dwt_is_available(),
            scroll_methods: device.config_scroll_methods(),
        }
    }
}

/// The calls that put a device with `supports` as the settings say, leaving out whatever it
/// can't do. Touchpads follow the touchpad's settings; every other pointer the mouse's.
pub fn plan(supports: &Supports, mouse: &Mouse, touchpad: &Touchpad) -> Vec<Call> {
    let mut calls = Vec::new();
    if supports.touchpad {
        calls.push(Call::TapToClick(touchpad.tap_to_click));
        if supports.speed {
            calls.push(Call::Speed(touchpad.speed()));
        }
        if supports.natural_scrolling {
            calls.push(Call::NaturalScrolling(touchpad.natural_scrolling));
        }
        if supports.disable_while_typing {
            calls.push(Call::DisableWhileTyping(touchpad.disable_while_typing));
        }
        let method = match touchpad.scrolling {
            Scrolling::TwoFinger => ScrollMethod::TwoFinger,
            Scrolling::Edge => ScrollMethod::Edge,
        };
        if supports.scroll_methods.contains(&method) {
            calls.push(Call::Scroll(method));
        }
        return calls;
    }
    if supports.speed {
        calls.push(Call::Speed(mouse.speed()));
    }
    let profile = match mouse.acceleration {
        Acceleration::Adaptive => AccelProfile::Adaptive,
        Acceleration::Flat => AccelProfile::Flat,
    };
    if supports.profiles.contains(&profile) {
        calls.push(Call::Profile(profile));
    }
    if supports.natural_scrolling {
        calls.push(Call::NaturalScrolling(mouse.natural_scrolling));
    }
    calls
}

/// Sets `device` up as the settings say.
pub fn configure(device: &mut libinput::Device, mouse: &Mouse, touchpad: &Touchpad) {
    for call in plan(&Supports::of(device), mouse, touchpad) {
        let result = match call {
            Call::Speed(speed) => device.config_accel_set_speed(speed),
            Call::Profile(profile) => device.config_accel_set_profile(profile),
            Call::NaturalScrolling(on) => device.config_scroll_set_natural_scroll_enabled(on),
            Call::TapToClick(on) => device.config_tap_set_enabled(on),
            Call::DisableWhileTyping(on) => device.config_dwt_set_enabled(on),
            Call::Scroll(method) => device.config_scroll_set_method(method),
        };
        if let Err(err) = result {
            tracing::warn!(device = %device.name(), ?call, ?err, "an input setting didn't take");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touchpad() -> Supports {
        Supports {
            touchpad: true,
            speed: true,
            profiles: vec![AccelProfile::Adaptive, AccelProfile::Flat],
            natural_scrolling: true,
            disable_while_typing: true,
            scroll_methods: vec![ScrollMethod::TwoFinger, ScrollMethod::Edge],
        }
    }

    fn mouse() -> Supports {
        Supports {
            touchpad: false,
            speed: true,
            profiles: vec![AccelProfile::Adaptive, AccelProfile::Flat],
            natural_scrolling: true,
            disable_while_typing: false,
            scroll_methods: vec![ScrollMethod::OnButtonDown],
        }
    }

    #[test]
    fn a_touchpad_follows_the_touchpad_settings() {
        let pad = Touchpad {
            speed: 0.25,
            natural_scrolling: false,
            scrolling: Scrolling::Edge,
            ..Touchpad::default()
        };
        let calls = plan(&touchpad(), &Mouse::default(), &pad);
        assert_eq!(
            calls,
            [
                Call::TapToClick(true),
                Call::Speed(0.25),
                Call::NaturalScrolling(false),
                Call::DisableWhileTyping(true),
                Call::Scroll(ScrollMethod::Edge),
            ]
        );
    }

    #[test]
    fn by_default_a_touchpad_taps_and_scrolls_naturally_with_two_fingers() {
        let calls = plan(&touchpad(), &Mouse::default(), &Touchpad::default());
        assert!(calls.contains(&Call::TapToClick(true)));
        assert!(calls.contains(&Call::NaturalScrolling(true)));
        assert!(calls.contains(&Call::Scroll(ScrollMethod::TwoFinger)));
    }

    #[test]
    fn a_mouse_follows_the_mouse_settings() {
        let settings = Mouse {
            speed: -0.5,
            natural_scrolling: true,
            acceleration: Acceleration::Flat,
        };
        let calls = plan(&mouse(), &settings, &Touchpad::default());
        assert_eq!(
            calls,
            [
                Call::Speed(-0.5),
                Call::Profile(AccelProfile::Flat),
                Call::NaturalScrolling(true),
            ]
        );
    }

    #[test]
    fn what_a_device_cannot_do_is_left_alone() {
        // A keyboard: no pointer settings at all.
        assert!(
            plan(
                &Supports::default(),
                &Mouse::default(),
                &Touchpad::default()
            )
            .is_empty()
        );
        // A touchpad without edge scrolling keeps the method it has.
        let pad = Supports {
            scroll_methods: vec![ScrollMethod::TwoFinger],
            ..touchpad()
        };
        let edge = Touchpad {
            scrolling: Scrolling::Edge,
            ..Touchpad::default()
        };
        let calls = plan(&pad, &Mouse::default(), &edge);
        assert!(!calls.iter().any(|call| matches!(call, Call::Scroll(_))));
        // A pointer with only one acceleration profile.
        let flat_only = Supports {
            profiles: vec![AccelProfile::Flat],
            ..mouse()
        };
        let calls = plan(&flat_only, &Mouse::default(), &Touchpad::default());
        assert!(!calls.iter().any(|call| matches!(call, Call::Profile(_))));
    }

    #[test]
    fn a_hand_edited_speed_is_kept_in_libinputs_range() {
        let fast = Mouse {
            speed: 4.0,
            ..Mouse::default()
        };
        assert_eq!(
            plan(&mouse(), &fast, &Touchpad::default())[0],
            Call::Speed(1.0)
        );
    }
}

//! Keyboard layouts and key repeat. With no layouts chosen in Settings the keymap comes from
//! `XKB_DEFAULT_LAYOUT` and friends, which the session script sets from the system's keyboard
//! settings, so nothing changes until someone chooses. With more than one layout, Alt and Shift
//! pressed together and let go switch to the next, as on Windows, and the bar shows which is in
//! use.

use slipstream_config::{Keyboard, KeyboardLayout};
use smithay::input::{
    Seat,
    keyboard::{KeyboardHandle, XkbConfig},
};

use crate::state::Slipstream;

/// The system's layouts, as the session started with them.
#[derive(Debug, Clone)]
pub struct Layouts {
    system: Vec<KeyboardLayout>,
    /// The system's xkb options switch layouts with a key of their own already.
    system_switches: bool,
}

impl Layouts {
    pub fn from_env() -> Self {
        Self {
            system: slipstream_config::system_layouts_from_env(),
            system_switches: slipstream_config::options_switch_layouts(
                &std::env::var("XKB_DEFAULT_OPTIONS").unwrap_or_default(),
            ),
        }
    }

    /// The layouts in the keymap, in order.
    pub fn in_use(&self, keyboard: &Keyboard) -> Vec<KeyboardLayout> {
        keyboard.layouts_or(&self.system)
    }

    /// Whether Alt+Shift switches layouts: only with more than one, and not when the system's
    /// own options already do it on a key of their own, which would switch twice.
    pub fn switch_key(&self, keyboard: &Keyboard) -> bool {
        !self.system_switches && self.in_use(keyboard).len() > 1
    }
}

/// Adds the seat's keyboard with the chosen layouts and repeat. Layouts xkb can't make a keymap
/// of (a mistyped hand edit) fall back to the system's, so there is always a keyboard.
pub fn add_keyboard(
    seat: &mut Seat<Slipstream>,
    keyboard: &Keyboard,
) -> KeyboardHandle<Slipstream> {
    let (delay, rate) = (
        keyboard.repeat_delay() as i32,
        keyboard.repeat_rate() as i32,
    );
    let names = keyboard.xkb_names();
    if let Some(chosen) = names.as_ref().map(xkb_config)
        && let Ok(handle) = seat.add_keyboard(chosen, delay, rate)
    {
        return handle;
    }
    if names.is_some() {
        tracing::warn!(layouts = ?keyboard.layouts, "xkb can't make a keymap of these layouts");
    }
    seat.add_keyboard(XkbConfig::default(), delay, rate)
        .expect("the system's own keymap")
}

/// Layout and variant lists as xkb takes them; the model and options stay the system's.
fn xkb_config((layout, variant): &(String, String)) -> XkbConfig<'_> {
    XkbConfig {
        layout,
        variant,
        ..XkbConfig::default()
    }
}

impl Slipstream {
    /// The keyboard settings changed from `old`: repeat applies at once, and new layouts make a
    /// new keymap, starting on the first.
    pub fn apply_keyboard(&mut self, old: &Keyboard) {
        let Some(handle) = self.seat.get_keyboard() else {
            return;
        };
        let new = self.settings.keyboard.clone();
        if (new.repeat_delay(), new.repeat_rate()) != (old.repeat_delay(), old.repeat_rate()) {
            handle.change_repeat_info(new.repeat_rate() as i32, new.repeat_delay() as i32);
        }
        if new.layouts != old.layouts {
            let names = new.xkb_names();
            let config = names.as_ref().map(xkb_config).unwrap_or_default();
            if handle.set_xkb_config(self, config).is_err() {
                tracing::warn!(layouts = ?new.layouts, "xkb can't make a keymap of these layouts");
            } else {
                tracing::info!(layouts = ?self.layouts.in_use(&new), "keyboard layouts changed");
            }
        }
    }

    /// The next layout, round to the first after the last.
    pub fn next_layout(&mut self) {
        let Some(handle) = self.seat.get_keyboard() else {
            return;
        };
        handle.with_xkb_state(self, |mut context| context.cycle_next_layout());
        tracing::info!(label = ?self.layout_label(), "keyboard layout switched");
    }

    /// What the bar shows for the layout in use, when there is more than one to switch between.
    pub fn layout_label(&self) -> Option<String> {
        let layouts = self.layouts.in_use(&self.settings.keyboard);
        if layouts.len() < 2 {
            return None;
        }
        let active = self
            .seat
            .get_keyboard()?
            .modifier_state()
            .serialized
            .layout_effective;
        slipstream_config::layout_label(&layouts, active as usize)
    }
}

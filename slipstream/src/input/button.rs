//! Where a pointer button goes: a card or panel of Slipstream's own, the bar, a notification, a
//! drag, or the window under the pointer.

use smithay::{
    backend::input::{ButtonState, InputTime},
    input::pointer::ButtonEvent,
    utils::SERIAL_COUNTER,
};

use super::{BTN_LEFT, BTN_MIDDLE};
use crate::{focus::KeyboardFocus, keys::Mods, screen::Overlay, state::Slipstream};

impl Slipstream {
    /// What is up on the focused screen that a press on another screen has to respect.
    fn overlay_up(&self) -> Overlay {
        if self.share.is_some()
            || self.polkit.is_up()
            || self.battery.card.is_some()
            || self.offer.is_some()
            || self.connect.is_some()
            || self.shortcuts.ask().is_some()
            || self.exit.as_ref().is_some_and(|exit| exit.modal())
            || self.switcher.is_some()
            || self.arrange.is_some()
            || self.bullet.is_some()
        {
            Overlay::Card
        } else if self.panel_open() {
            Overlay::Panel
        } else {
            Overlay::None
        }
    }

    /// A pointer button pressed or let go: from a mouse or touchpad, or the `button:` debug step,
    /// which takes the same path so a script exercises the real click routing.
    pub fn button_event(&mut self, button: u32, button_state: ButtonState, time: InputTime) {
        self.keys.super_tap.disarm();
        self.keys.layout_tap.disarm();
        let pressed = button_state == ButtonState::Pressed;
        // Reaching for the mouse ends any typing: what a click opens was asked for.
        if pressed {
            self.concentration.end();
            self.concentration.input(std::time::Instant::now());
        }
        // A click while the UI is faded only brings it back.
        if self.wake_ui() && pressed {
            return;
        }
        // While locked a click does nothing at all: no app, no screen, no card. A release still
        // goes to the seat, which sends it nowhere (nothing has pointer focus), so a button held as
        // the lock went up isn't left pressed, with a grab that sticks, once it's gone.
        if self.lock.is_some() {
            if !pressed {
                self.forward_button(button, button_state, SERIAL_COUNTER.next_serial(), time);
            }
            return;
        }
        // Clicking anything on another screen moves the keyboard there, whether or not
        // the click lands on a window. Not while something drawn on the focused screen has the
        // pointer: a card asking a question ignores a press elsewhere, so a blind click can't
        // answer it, and a panel closes as it would for any click outside it.
        if pressed
            && let Some(index) = self.screen_under_pointer()
            && index != self.screens.focused_index()
        {
            match crate::screen::press_elsewhere(self.overlay_up()) {
                crate::screen::PressElsewhere::Ignore => return,
                crate::screen::PressElsewhere::ClosePanels => {
                    self.close_panels();
                    return;
                }
                crate::screen::PressElsewhere::MoveKeyboard => {
                    if self.screens.focus(index) {
                        self.restore_focus();
                    }
                }
            }
        }
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        let serial = SERIAL_COUNTER.next_serial();

        // A snip being chosen takes every button: a drag, a click on a window, or a right click to
        // give up.
        if self.shots.snip.is_some() {
            self.snip_button(button, pressed);
            return;
        }
        // So does the password card, over everything else but the lock. A release still goes to
        // the seat, which sends it nowhere (nothing has pointer focus), as at the lock, so a
        // button held as the card came up isn't left pressed.
        if self.polkit.is_up() {
            if let Some(pos) = self.pressed_at(pressed) {
                self.polkit_click(pos.x, pos.y);
            } else if !pressed {
                self.forward_button(button, button_state, serial, time);
            }
            return;
        }
        if self.card_click(button, pressed) {
            return;
        }
        if !pointer.is_grabbed() {
            let at = pointer.current_location();
            // A notification's pop-up.
            if !self.fullscreen_on_screen() && self.popup_click(button, pressed) {
                return;
            }
            // A click on a stream of code rain brings its window back.
            if let Some(window) = self.rain_window_at(at) {
                if pressed {
                    self.restore(&window);
                }
                return;
            }
            // The bar takes its own clicks; nothing below it sees them.
            if let Some(target) = self.bar_target_at(at) {
                if pressed {
                    self.bar_clicked(target);
                }
                return;
            }
        }

        // A left press on the gap between tiles resizes them, and with Super held on a window it
        // drags that window to swap it. Either way the press reaches no app.
        if pressed {
            let super_held = Mods::from_state(&keyboard.modifier_state(), self.nested).logo;
            if self.start_drag(pointer.current_location(), button, serial, super_held) {
                self.forward_button(button, button_state, serial, time);
                return;
            }
        }

        if pressed && !pointer.is_grabbed() {
            if self.layer_clicked(pointer.current_location()) {
                // A panel or launcher of another program's: the press goes to it, and it took
                // the keyboard if it wanted it.
            } else if let Some((window, _)) = self.window_under(pointer.current_location()) {
                self.focus_window(&window);
            } else {
                self.space.elements().for_each(|window| {
                    window.set_activated(false);
                    if let Some(toplevel) = window.toplevel() {
                        toplevel.send_pending_configure();
                    }
                });
                keyboard.set_focus(self, Option::<KeyboardFocus>::None, serial);
            }
        };
        self.forward_button(button, button_state, serial, time);
    }

    /// A press, in the focused screen's own coordinates: cards and panels are drawn there.
    fn pressed_at(
        &self,
        pressed: bool,
    ) -> Option<smithay::utils::Point<f64, smithay::utils::Logical>> {
        let pointer = self.seat.get_pointer()?;
        (pressed)
            .then(|| self.focused_screen_geometry())
            .flatten()
            .map(|screen| pointer.current_location() - screen.loc.to_f64())
    }

    /// Hands the button to the seat, for whatever has the pointer.
    fn forward_button(
        &mut self,
        button: u32,
        state: ButtonState,
        serial: smithay::utils::Serial,
        time: InputTime,
    ) {
        let pointer = self.seat.get_pointer().unwrap();
        let event = ButtonEvent {
            button,
            state,
            serial,
            time,
        };
        pointer.button(self, &event);
        pointer.frame(self);
    }

    /// Whether a card, panel or mode that's up took the button. Each takes every button while
    /// it's up: its own, and a click beside it that mustn't reach through to the windows
    /// underneath (or that closes a panel).
    fn card_click(&mut self, button: u32, pressed: bool) -> bool {
        let at = self.pressed_at(pressed);
        let clicked: Option<fn(&mut Self, f64, f64)> = if self.share.is_some() {
            Some(Self::share_click)
        } else if self.battery.card.is_some() {
            Some(Self::battery_click)
        } else if self.offer.is_some() {
            Some(Self::offer_click)
        } else if self.connect.is_some() {
            Some(Self::connect_click)
        } else if self.shortcuts.ask().is_some() {
            Some(Self::shortcuts_click)
        } else if self.exit.as_ref().is_some_and(|exit| exit.modal()) {
            Some(Self::exit_click)
        } else {
            None
        };
        if let Some(clicked) = clicked {
            if let Some(pos) = at {
                clicked(self, pos.x, pos.y);
            }
            return true;
        }
        // Gravity's arrangements: a click on a tile keeps it.
        if self.arrange.is_some() {
            let tile = self
                .arrange
                .as_ref()
                .filter(|arrange| arrange.visible(self.wall()))
                .and_then(|arrange| arrange.tile_at(at?));
            if let Some(tile) = tile {
                self.pick_arrangement(tile);
            }
            return true;
        }
        // Alt+Tab's switcher: a click on a tile goes to that window.
        if self.switcher.is_some() {
            let tile = self
                .switcher
                .as_ref()
                .filter(|switcher| switcher.visible(self.wall()))
                .and_then(|switcher| switcher.tile_at(at?));
            if let (Some(tile), Some(switcher)) = (tile, self.switcher.as_mut()) {
                switcher.select(tile);
                let now = self.wall();
                if let Some(deck) = self.deck.as_mut() {
                    deck.select(tile, now);
                }
                self.finish_cycle();
            }
            return true;
        }
        // In bullet time a button goes to what's under it: see `bullet_button`.
        if self.bullet.is_some() {
            let pos = self.pointer_location();
            self.bullet_button(button, pressed, pos);
            return true;
        }
        // The open explorer: on an item it opens it, outside it closes. A middle click on an app
        // starts another, as Shift+Enter does.
        if self.explorer.is_open() {
            if let Some(pos) = at {
                self.explorer_click(pos.x, pos.y, button == BTN_MIDDLE);
            }
            return true;
        }
        // The clipboard history: a row pastes, a click outside closes it.
        if self.history.is_open() {
            if let Some(pos) = at {
                self.history_click(pos);
            }
            return true;
        }
        // Quick settings' own, or one outside that closes it; letting go ends a slider's drag.
        if self.quick.is_open() {
            match at {
                Some(pos) => self.quick_click(pos.x, pos.y),
                None if !pressed => self.quick.release(),
                None => {}
            }
            return true;
        }
        // The plugin panel's own, or one outside that closes it.
        if self.plugin_panel.is_open() {
            if let Some(pos) = at
                && pressed
            {
                self.plugin_click(pos.x, pos.y);
            }
            return true;
        }
        // The key list: a click outside closes it.
        if self.sheet.is_open() {
            if let Some(pos) = at {
                self.sheet_click(pos.x, pos.y);
            }
            return true;
        }
        // The notification centre: the button matters on a background app's chip.
        if self.centre.is_open() {
            if let Some(pos) = at {
                self.centre_click(pos.x, pos.y, button);
            }
            return true;
        }
        false
    }

    /// Whether the button was on a notification's pop-up. A left click opens it; its cross, or
    /// another button, dismisses it.
    fn popup_click(&mut self, button: u32, pressed: bool) -> bool {
        // Pop-ups are drawn on the focused screen, and their areas kept in its coordinates.
        let origin = self
            .overlay_screen()
            .map_or((0.0, 0.0), |(_, screen)| (screen.x as f64, screen.y as f64));
        let pos = self.pointer_location();
        // The cross takes the left button only; any other on it dismisses the pop-up below it
        // all the same.
        let cross = self
            .notices
            .cross_hit_from((pos.x, pos.y), origin)
            .filter(|_| button == BTN_LEFT);
        if let Some(id) = cross {
            if pressed {
                let gone = self.notices.dismiss_popup(id);
                crate::notify::closed(gone, crate::notify::Reason::Dismissed);
            }
            return true;
        }
        if let Some((id, action)) = self.notices.button_hit_from((pos.x, pos.y), origin) {
            if pressed && button == BTN_LEFT {
                self.invoke_notice_action(id, action);
            }
            return true;
        }
        if let Some(id) = self.notices.popup_hit_from((pos.x, pos.y), origin) {
            if pressed {
                if button == BTN_LEFT {
                    self.open_notice(id);
                } else {
                    let gone = self.notices.dismiss_popup(id);
                    crate::notify::closed(gone, crate::notify::Reason::Dismissed);
                }
            }
            return true;
        }
        false
    }
}

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
        } else if self.explorer.is_open()
            || self.quick.is_open()
            || self.centre.is_open()
            || self.sheet.is_open()
            || self.history.is_open()
        {
            Overlay::Panel
        } else {
            Overlay::None
        }
    }

    /// A pointer button pressed or let go: from a mouse or touchpad, or the `button:` debug step,
    /// which takes the same path so a script exercises the real click routing.
    pub fn button_event(&mut self, button: u32, button_state: ButtonState, time: InputTime) {
        self.super_tap.disarm();
        self.layout_tap.disarm();
        // Reaching for the mouse ends any typing: what a click opens was asked for.
        if button_state == ButtonState::Pressed {
            self.concentration.end();
            self.concentration.input(std::time::Instant::now());
        }
        // A click while the UI is faded only brings it back.
        if self.wake_ui() && button_state == ButtonState::Pressed {
            return;
        }
        // While locked a click does nothing at all: no app, no screen, no card. A release still
        // goes to the seat, which sends it nowhere (nothing has pointer focus), so a button held as
        // the lock went up isn't left pressed, with a grab that sticks, once it's gone.
        if self.lock.is_some() {
            if button_state == ButtonState::Released {
                let pointer = self.seat.get_pointer().unwrap();
                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                    },
                );
                pointer.frame(self);
            }
            return;
        }
        // Clicking anything on another screen moves the keyboard there, whether or not
        // the click lands on a window. Not while something drawn on the focused screen has the
        // pointer: a card asking a question ignores a press elsewhere, so a blind click can't
        // answer it, and a panel closes as it would for any click outside it.
        if button_state == ButtonState::Pressed
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

        // A press, in the focused screen's own coordinates: cards and panels are drawn there.
        let pressed_at = (button_state == ButtonState::Pressed)
            .then(|| self.focused_screen_geometry())
            .flatten()
            .map(|screen| pointer.current_location() - screen.loc.to_f64());

        // A snip being chosen takes every button: a drag, a click on a window, or a right click to
        // give up.
        if self.snip.is_some() {
            self.snip_button(button, button_state == ButtonState::Pressed);
            return;
        }

        // So does the password card, over everything else but the lock. A release still goes to
        // the seat, which sends it nowhere (nothing has pointer focus), as at the lock, so a button
        // held as the card came up isn't left pressed.
        if self.polkit.is_up() {
            if let Some(pos) = pressed_at {
                self.polkit_click(pos.x, pos.y);
            } else if button_state == ButtonState::Released {
                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time,
                    },
                );
                pointer.frame(self);
            }
            return;
        }
        // So does the share picker, and each card while it's asking: its own buttons, and a click
        // beside it that mustn't reach through to the windows underneath.
        if self.share.is_some() {
            if let Some(pos) = pressed_at {
                self.share_click(pos.x, pos.y);
            }
            return;
        }
        if self.battery.card.is_some() {
            if let Some(pos) = pressed_at {
                self.battery_click(pos.x, pos.y);
            }
            return;
        }
        if self.offer.is_some() {
            if let Some(pos) = pressed_at {
                self.offer_click(pos.x, pos.y);
            }
            return;
        }
        if self.connect.is_some() {
            if let Some(pos) = pressed_at {
                self.connect_click(pos.x, pos.y);
            }
            return;
        }
        if self.shortcuts.ask().is_some() {
            if let Some(pos) = pressed_at {
                self.shortcuts_click(pos.x, pos.y);
            }
            return;
        }
        if self.exit.as_ref().is_some_and(|exit| exit.modal()) {
            if let Some(pos) = pressed_at {
                self.exit_click(pos.x, pos.y);
            }
            return;
        }

        // Gravity's arrangements take every click while they're up: one on a tile keeps it.
        if self.arrange.is_some() {
            let tile = self
                .arrange
                .as_ref()
                .filter(|arrange| arrange.visible(self.wall()))
                .and_then(|arrange| arrange.tile_at(pressed_at?));
            if let Some(tile) = tile {
                self.pick_arrangement(tile);
            }
            return;
        }

        // Alt+Tab's switcher takes every click while it's up: one on a tile goes to that window.
        if self.switcher.is_some() {
            let tile = self
                .switcher
                .as_ref()
                .filter(|switcher| switcher.visible(self.wall()))
                .and_then(|switcher| switcher.tile_at(pressed_at?));
            if let (Some(tile), Some(switcher)) = (tile, self.switcher.as_mut()) {
                switcher.select(tile);
                let now = self.wall();
                if let Some(deck) = self.deck.as_mut() {
                    deck.select(tile, now);
                }
                self.finish_cycle();
            }
            return;
        }

        // In bullet time a button goes to what's under it: see `bullet_button`.
        if self.bullet.is_some() {
            let pos = pointer.current_location();
            self.bullet_button(button, button_state == ButtonState::Pressed, pos);
            return;
        }

        // The open explorer takes every click: on an item it opens it, outside it closes. A middle
        // click on an app starts another, as Shift+Enter does.
        if self.explorer.is_open() {
            if let Some(pos) = pressed_at {
                self.explorer_click(pos.x, pos.y, button == BTN_MIDDLE);
            }
            return;
        }

        // The clipboard history: a row pastes, a click outside closes it.
        if self.history.is_open() {
            if let Some(pos) = pressed_at {
                self.history_click(pos);
            }
            return;
        }

        // So does the shortcut sheet: a click outside it closes it.
        if self.sheet.is_open() {
            if let Some(pos) = pressed_at {
                self.sheet_click(pos.x, pos.y);
            }
            return;
        }

        // Open quick settings takes every click too: its own, or one outside that closes it.
        if self.quick.is_open() {
            if let Some(pos) = pressed_at {
                self.quick_click(pos.x, pos.y);
            } else if button_state == ButtonState::Released {
                self.quick.release();
            }
            return;
        }

        // So does the open notification centre.
        if self.centre.is_open() {
            if let Some(pos) = pressed_at {
                self.centre_click(pos.x, pos.y);
            }
            return;
        }

        // A left click on a notification's pop-up opens it; its cross, or another button, dismisses
        // it.
        if !pointer.is_grabbed() && !self.fullscreen_on_screen() {
            // Pop-ups are drawn on the focused screen, and their areas kept in its coordinates.
            let origin = self
                .overlay_screen()
                .map_or((0.0, 0.0), |(_, screen)| (screen.x as f64, screen.y as f64));
            let pos = pointer.current_location();
            // The cross takes the left button only; any other on it dismisses the pop-up below it
            // all the same.
            let cross = self
                .notices
                .cross_hit_from((pos.x, pos.y), origin)
                .filter(|_| button == BTN_LEFT);
            if let Some(id) = cross {
                if button_state == ButtonState::Pressed {
                    let gone = self.notices.dismiss_popup(id);
                    crate::notify::closed(gone, crate::notify::Reason::Dismissed);
                }
                return;
            }
            if let Some((id, action)) = self.notices.button_hit_from((pos.x, pos.y), origin) {
                if button_state == ButtonState::Pressed && button == BTN_LEFT {
                    self.invoke_notice_action(id, action);
                }
                return;
            }
            if let Some(id) = self.notices.popup_hit_from((pos.x, pos.y), origin) {
                if button_state == ButtonState::Pressed {
                    if button == BTN_LEFT {
                        self.open_notice(id);
                    } else {
                        let gone = self.notices.dismiss_popup(id);
                        crate::notify::closed(gone, crate::notify::Reason::Dismissed);
                    }
                }
                return;
            }
        }

        // A click on a stream of code rain brings its window back.
        if !pointer.is_grabbed() {
            if let Some(window) = self.rain_window_at(pointer.current_location()) {
                if button_state == ButtonState::Pressed {
                    self.restore(&window);
                }
                return;
            }
        }

        // The bar takes its own clicks; nothing below it sees them.
        if !pointer.is_grabbed() {
            if let Some(target) = self.bar_target_at(pointer.current_location()) {
                if button_state == ButtonState::Pressed {
                    self.bar_clicked(target);
                }
                return;
            }
        }

        // A left press on the gap between tiles resizes them, and with Super held on a window it
        // drags that window to swap it. Either way the press reaches no app.
        if button_state == ButtonState::Pressed {
            let super_held = Mods::from_state(&keyboard.modifier_state(), self.nested).logo;
            if self.start_drag(pointer.current_location(), button, serial, super_held) {
                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time,
                    },
                );
                pointer.frame(self);
                return;
            }
        }

        if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
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

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }
}

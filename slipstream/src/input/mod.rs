//! Input from the backends: pointer motion, the wheel and touchpad gestures here; where a click
//! goes in `button`, where a key goes in `keyboard`, and what the desktop's own swipes do in
//! `swipe`.

mod button;
mod keyboard;
pub mod swipe;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, Event, GestureBeginEvent,
        GestureEndEvent as GestureEndEventTrait,
        GesturePinchUpdateEvent as GesturePinchUpdateEventTrait,
        GestureSwipeUpdateEvent as GestureSwipeUpdateEventTrait, InputBackend, InputEvent,
        InputTime, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        Switch, SwitchState, SwitchToggleEvent,
    },
    input::{
        keyboard::Keysym,
        pointer::{
            AxisFrame, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
            GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent, RelativeMotionEvent,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

use crate::{focus::KeyboardFocus, keys::Mods, state::Slipstream, takeback::Constraint};

/// Linux's codes for the left and middle pointer buttons.
pub(crate) const BTN_LEFT: u32 = 0x110;
pub(crate) const BTN_MIDDLE: u32 = 0x112;

impl Slipstream {
    /// The bar button at `pos`. Every screen has a bar, so the one being clicked is the one the
    /// point is on.
    pub(crate) fn bar_target_at(&self, pos: Point<f64, Logical>) -> Option<crate::bar::Target> {
        let index = self.screen_at(pos)?;
        // A fullscreen window hides that screen's bar, and only that screen's.
        if self.fullscreen_on(self.screens.get(index)?.workspace) {
            return None;
        }
        let rect = self.screen_rect(index)?;
        let name = self.screens.get(index)?.output.name();
        let (x, y) = (pos.x - rect.x as f64, pos.y - rect.y as f64);
        self.chromes.get(&name)?.bar.target_at(x, y)
    }

    /// Whether something of Slipstream's own is taking the pointer: the explorer, quick settings,
    /// the notification centre, bullet time, the share picker, the login offer, or the way out
    /// while it's asking. Clicks already stop at each of these; motion and the wheel stop here.
    pub fn pointer_blocked(&self) -> bool {
        self.lock.is_some()
            || self.switcher.is_some()
            || self.arrange.is_some()
            || self.explorer.is_open()
            || self.sheet.is_open()
            || self.history.is_open()
            || self.quick.is_open()
            || self.centre.is_open()
            || self.bullet.is_some()
            || self.share.is_some()
            || self.snip.is_some()
            || self.battery.card.is_some()
            || self.offer.is_some()
            || self.connect.is_some()
            || self.exit.as_ref().is_some_and(|exit| exit.modal())
    }

    /// The surface the pointer's events go to at `pos`: whatever is under it, or nothing while an
    /// overlay has the pointer, so the window beneath gets a leave rather than hover and motion
    /// for a pointer that is over glass.
    pub fn pointer_target(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(KeyboardFocus, Point<f64, Logical>)> {
        if self.pointer_blocked() || self.over_chrome(pos) {
            return None;
        }
        self.surface_under(pos)
    }

    /// The wheel over something of Slipstream's own: in the explorer and the notification centre,
    /// each notch (`notch` of scroll) moves the selection a row, as ↓ and ↑ do, and the list
    /// follows it. A touchpad's small movements add up to a notch first.
    fn overlay_wheel(&mut self, amount: f64, notch: f64) {
        let list = self.explorer.is_open() || self.centre.is_open() || self.history.is_open();
        if !list {
            self.wheel_rest = 0.0;
            return;
        }
        self.wheel_rest += amount;
        while self.wheel_rest.abs() >= notch {
            let down = self.wheel_rest > 0.0;
            self.wheel_rest -= notch.copysign(self.wheel_rest);
            let key = if down { Keysym::Down } else { Keysym::Up };
            if self.history.is_open() {
                self.history_key(key);
            } else if self.explorer.is_open() {
                self.explorer_key(key, None, Mods::default());
            } else if self.centre.is_open() {
                self.centre_key(key, false);
            }
        }
    }

    /// Whether `pos` is over a notification pop-up, the toast or the volume display.
    fn over_chrome(&self, pos: Point<f64, Logical>) -> bool {
        self.chrome_areas.iter().any(|area| area.contains(pos))
    }

    /// Once a pass of the event loop: if something opened or closed over the pointer since it
    /// last moved (a panel, bullet time, a pop-up), the window beneath hears of it now, with a
    /// leave or an enter, rather than at the next motion.
    pub fn refresh_pointer_focus(&mut self) {
        let pos = self.pointer_location();
        let blocked = self.pointer_blocked() || self.over_chrome(pos);
        if blocked != self.pointer_was_blocked {
            self.pointer_was_blocked = blocked;
            self.pointer_moved_to(pos, InputTime::now());
        }
    }

    /// Where the pointer's motion to `pos` goes: nowhere over a gap between tiles, which shows
    /// the resize cursor instead, and otherwise as `pointer_target` says. Another grab (a menu's)
    /// keeps the pointer as it is.
    fn motion_target(
        &mut self,
        pos: Point<f64, Logical>,
    ) -> Option<(KeyboardFocus, Point<f64, Logical>)> {
        let grabbed = self.seat.get_pointer().unwrap().is_grabbed();
        if !grabbed && self.pointer_blocked() {
            // No resize cursor over an overlay. A window dragged in bullet time keeps its own.
            let dragging = self
                .bullet
                .as_ref()
                .is_some_and(|mode| mode.pointer.drag.is_some());
            if !dragging && self.drag.is_none() {
                self.set_cursor_override(None);
            }
        } else if !grabbed && self.hover_divider(pos) {
            return None;
        }
        self.pointer_target(pos)
    }

    /// The pointer moved to `pos`: what Slipstream draws itself follows it, after the clients
    /// have heard.
    fn pointer_followers(&mut self, pos: Point<f64, Logical>) {
        self.exit_hover(pos);
        self.offer_hover(pos);
        self.connect_hover(pos);
        self.battery_hover(pos);
        self.share_hover(pos);
        self.snip_pointer_moved(pos);
        self.bullet_pointer_moved(pos);
        // A slider held in quick settings.
        if self.quick.is_open()
            && let Some(screen) = self.focused_screen_geometry()
        {
            let request = self.quick.drag(pos.x - screen.loc.x as f64);
            self.quick_request(request);
        }
    }

    /// The pointer is now at `pos`, from an absolute device (the host's pointer when nested, a
    /// tablet) or the `pointer:` debug step.
    pub fn pointer_moved_to(&mut self, pos: Point<f64, Logical>, time: InputTime) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();
        let under = self.motion_target(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time,
            },
        );
        pointer.frame(self);
        self.pointer_followers(pos);
    }

    /// The screen a touchscreen or drawing tablet points at. Its coordinates are a fraction of
    /// one screen, so it has to be told which; the built-in panel is the answer for a built-in
    /// touchscreen, which is what almost every absolute device on a laptop is. Failing that, the
    /// screen the keyboard is on, which at least follows the work.
    ///
    /// Not the first output the space happens to hold: that is the order they were mapped in, so a
    /// laptop booted with a monitor already plugged in could send its own touchscreen to the
    /// monitor.
    fn absolute_screen(&self) -> Option<smithay::output::Output> {
        self.space
            .outputs()
            .find(|output| crate::state::is_internal_panel(&output.name()))
            .or_else(|| {
                self.screens
                    .focused_output()
                    .and_then(|focused| self.space.outputs().find(|o| **o == focused))
            })
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    /// Keeps the pointer on screen: `pos`, or the nearest point on any output.
    pub fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let mut nearest: Option<(f64, Point<f64, Logical>)> = None;
        for geo in self
            .space
            .outputs()
            .filter_map(|o| self.space.output_geometry(o))
        {
            let geo = geo.to_f64();
            if geo.contains(pos) {
                return pos;
            }
            // `max` keeps the range the right way round for an output with no size yet.
            let on = Point::<f64, Logical>::from((
                pos.x
                    .clamp(geo.loc.x, (geo.loc.x + geo.size.w - 1.0).max(geo.loc.x)),
                pos.y
                    .clamp(geo.loc.y, (geo.loc.y + geo.size.h - 1.0).max(geo.loc.y)),
            ));
            let distance = (on.x - pos.x).powi(2) + (on.y - pos.y).powi(2);
            if nearest.is_none_or(|(best, _)| distance < best) {
                nearest = Some((distance, on));
            }
        }
        nearest.map_or(pos, |(_, on)| on)
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        self.something_changed();
        match event {
            InputEvent::Keyboard { event, .. } => {
                self.key_event(event.key_code(), event.state(), Event::time(&event), false);
            }
            // Mice and touchpads on real hardware.
            InputEvent::PointerMotion { event, .. } => {
                self.wake_ui();
                let pointer = self.seat.get_pointer().unwrap();
                let delta = event.delta();
                let from = pointer.current_location();
                let constraint = self.active_constraint();
                let relative = RelativeMotionEvent {
                    delta,
                    delta_unaccel: event.delta_unaccel(),
                    time: event.time(),
                };
                // A locked pointer stays put: the app hears only how far the mouse moved.
                if matches!(constraint, Some(Constraint::Locked)) {
                    let under = self.pointer_target(from);
                    pointer.relative_motion(self, under, &relative);
                    pointer.frame(self);
                    return;
                }
                let mut pos = self.clamp_to_outputs(from + delta);
                // A confined pointer moves along each axis only as far as it stays inside.
                if let Some(Constraint::Confined {
                    region,
                    origin,
                    surface,
                }) = constraint
                {
                    let inside = |state: &Self, point: Point<f64, Logical>| {
                        let over = state
                            .pointer_target(point)
                            .and_then(|(focus, _)| focus.wl_surface().map(|s| s.into_owned()))
                            .is_some_and(|under| under == surface);
                        over && region
                            .as_ref()
                            .is_none_or(|region| region.contains((point - origin).to_i32_round()))
                    };
                    if !inside(self, Point::from((pos.x, from.y))) {
                        pos.x = from.x;
                    }
                    if !inside(self, Point::from((pos.x, pos.y))) {
                        pos.y = from.y;
                    }
                }
                let serial = SERIAL_COUNTER.next_serial();
                let under = self.motion_target(pos);
                pointer.motion(
                    self,
                    under.clone(),
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.relative_motion(self, under, &relative);
                pointer.frame(self);
                self.update_pointer_constraint();
                self.pointer_followers(pos);
            }
            // The host's pointer when nested; tablets and touchscreens on real hardware.
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.wake_ui();
                let Some(output_geo) = self
                    .absolute_screen()
                    .and_then(|output| self.space.output_geometry(&output))
                else {
                    return;
                };

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                self.pointer_moved_to(pos, event.time());
            }
            InputEvent::PointerButton { event, .. } => {
                self.button_event(event.button_code(), event.state(), event.time());
            }
            InputEvent::PointerAxis { event, .. } => {
                self.super_tap.disarm();
                self.wake_ui();
                // A wheel's notch is 120 in v120 units, and 15 of scroll.
                let amount = |axis: Axis| {
                    event
                        .amount(axis)
                        .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.)
                };
                // The wheel over an overlay would otherwise scroll the window hidden under it. In
                // the explorer and the notification centre it moves through the list instead.
                if self.pointer_blocked() || self.over_chrome(self.pointer_location()) {
                    let notch = if event.source() == AxisSource::Finger {
                        30.0
                    } else {
                        15.0
                    };
                    self.overlay_wheel(amount(Axis::Vertical), notch);
                    return;
                }
                let source = event.source();

                let horizontal_amount = amount(Axis::Horizontal);
                let vertical_amount = amount(Axis::Vertical);
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            // Touchpad swipes, pinches and holds go to the window under the pointer, except a
            // three-finger swipe, which is the desktop's.
            InputEvent::GestureSwipeBegin { event, .. } => {
                let fingers = event.fingers();
                if self.takes_swipe(fingers) {
                    self.swipe_began();
                    return;
                }
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                pointer.gesture_swipe_begin(
                    self,
                    &GestureSwipeBeginEvent {
                        serial,
                        time: Event::time(&event),
                        fingers,
                    },
                );
            }
            InputEvent::GestureSwipeUpdate { event, .. } => {
                let delta = GestureSwipeUpdateEventTrait::delta(&event);
                if self.swipe.is_some() {
                    let time = Event::time(&event).micros() as f64 / 1e6;
                    self.swipe_moved(delta.x, delta.y, time);
                    return;
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.gesture_swipe_update(
                    self,
                    &GestureSwipeUpdateEvent {
                        time: Event::time(&event),
                        delta,
                    },
                );
            }
            InputEvent::GestureSwipeEnd { event, .. } => {
                if self.swipe.is_some() {
                    let time = Event::time(&event).micros() as f64 / 1e6;
                    self.swipe_ended(time, GestureEndEventTrait::cancelled(&event));
                    return;
                }
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let cancelled = GestureEndEventTrait::cancelled(&event);
                pointer.gesture_swipe_end(
                    self,
                    &GestureSwipeEndEvent {
                        serial,
                        time: Event::time(&event),
                        cancelled,
                    },
                );
            }
            InputEvent::GesturePinchBegin { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let fingers = event.fingers();
                pointer.gesture_pinch_begin(
                    self,
                    &GesturePinchBeginEvent {
                        serial,
                        time: Event::time(&event),
                        fingers,
                    },
                );
            }
            InputEvent::GesturePinchUpdate { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                pointer.gesture_pinch_update(
                    self,
                    &GesturePinchUpdateEvent {
                        time: Event::time(&event),
                        delta: GesturePinchUpdateEventTrait::delta(&event),
                        scale: event.scale(),
                        rotation: event.rotation(),
                    },
                );
            }
            InputEvent::GesturePinchEnd { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let cancelled = GestureEndEventTrait::cancelled(&event);
                pointer.gesture_pinch_end(
                    self,
                    &GesturePinchEndEvent {
                        serial,
                        time: Event::time(&event),
                        cancelled,
                    },
                );
            }
            InputEvent::GestureHoldBegin { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let fingers = event.fingers();
                pointer.gesture_hold_begin(
                    self,
                    &GestureHoldBeginEvent {
                        serial,
                        time: Event::time(&event),
                        fingers,
                    },
                );
            }
            InputEvent::GestureHoldEnd { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let cancelled = GestureEndEventTrait::cancelled(&event);
                pointer.gesture_hold_end(
                    self,
                    &GestureHoldEndEvent {
                        serial,
                        time: Event::time(&event),
                        cancelled,
                    },
                );
            }
            // The laptop's lid. It only reaches a compositor at all while logind's own handling
            // is inhibited (`inhibit.rs`); otherwise the machine suspends before this is read.
            InputEvent::SwitchToggle { event } if event.switch() == Some(Switch::Lid) => {
                let closed = event.state() == SwitchState::On;
                tracing::info!(closed, "the lid");
                self.lid_switched(closed);
            }
            _ => {}
        }
    }
}

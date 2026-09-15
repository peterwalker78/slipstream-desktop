use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
        GestureEndEvent as GestureEndEventTrait,
        GesturePinchUpdateEvent as GesturePinchUpdateEventTrait,
        GestureSwipeUpdateEvent as GestureSwipeUpdateEventTrait, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent, Switch, SwitchState, SwitchToggleEvent,
    },
    input::{
        keyboard::{FilterResult, Keycode, Keysym, xkb},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            RelativeMotionEvent,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{compositor::with_states, seat::WaylandFocus, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{
    exit::Intent,
    focus::KeyboardFocus,
    keys::{self, Action, Mods},
    state::Slipstream,
    takeback::Constraint,
};

/// What the keyboard filter decided a key is for.
enum KeyUse {
    Action(Action),
    /// A key for the open explorer, with the character it types and the modifiers held.
    Explorer(Keysym, Option<char>, Mods),
    /// A key for bullet time, with its modifiers.
    Bullet(Keysym, Mods),
    /// A key for quick settings, and whether Shift is held.
    Quick(Keysym, bool),
    /// A key for the notification centre, and whether Shift is held.
    Centre(Keysym, bool),
    /// A key for the way out, while its card is taking them all.
    Exit(Keysym),
    /// A key for the share picker, with Shift for Shift+Tab.
    Share(Keysym, bool),
    /// A key for the offer at login, likewise.
    Offer(Keysym),
    /// A key for the low battery card.
    Battery(Keysym),
    /// A key for the lock screen's password pill.
    Lock(crate::lock::Key),
    /// A key let go while locked.
    LockRelease,
    /// Esc during a drag with the pointer.
    CancelDrag,
    /// Esc while Alt+Tab's switcher is up.
    CancelSwitch,
    /// A key for the shortcut sheet, with the character it types.
    Sheet(Keysym, Option<char>),
    /// A key while a snip is being chosen.
    Snip(Keysym),
    /// A key for the clipboard history.
    History(Keysym),
}

impl Slipstream {
    fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                tracing::info!("quit requested from the keyboard");
                self.begin_exit(Intent::LogOut);
            }
            Action::Focus(direction) => self.focus_direction(direction),
            Action::MoveTile(direction) => self.move_tile(direction),
            Action::Resize(how) => self.resize_focused(how),
            Action::Workspace(n) => {
                if let Some(index) = self.workspace_for_key(n) {
                    self.switch_workspace(index)
                }
            }
            Action::WorkspaceBy(delta) => self.switch_workspace_by(delta as i32),
            Action::MoveToWorkspace(n) => {
                if let Some(index) = self.workspace_for_key(n) {
                    self.move_focused_to_workspace(index)
                }
            }
            Action::MoveToWorkspaceBy(delta) => self.move_focused_by(delta as i32),
            Action::NextScreen => self.focus_next_screen(),
            Action::MoveToNextScreen => self.move_focused_to_next_screen(),
            Action::SwapScreens => self.swap_with_next_screen(),
            Action::Close => self.close_focused(),
            Action::Launch(app) => self.launch_app(app),
            Action::Screenshot { window } => self.take_screenshot(window),
            Action::ShowDesktop => self.show_desktop(),
            Action::ShortcutSheet => self.toggle_sheet(),
            Action::Lock => {
                self.lock_now();
            }
            Action::SwitchVt(vt) => self.switch_vt(vt),
            Action::Volume(percent) => self.change_volume(percent),
            Action::ToggleMute { microphone } => self.toggle_mute(microphone),
            Action::Media(transport) => self.media_key(transport),
            Action::Brightness(percent) => self.change_brightness(percent),
            Action::CycleWindows { forward } => self.cycle_windows(forward),
            Action::Explorer => self.toggle_explorer(),
            Action::Weigh { heavier } => self.weigh(heavier),
            Action::ToggleGravity => self.toggle_gravity(),
            Action::Minimise => self.minimise_focused(),
            Action::Restore => self.restore_latest(),
            Action::BulletTime => self.toggle_bullet_time(),
            Action::QuickSettings => self.toggle_quick_settings(),
            Action::NotificationCentre => self.toggle_notification_centre(),
            Action::Maximise => self.toggle_maximise(),
            Action::TakeBack => self.take_back(),
            Action::ClipboardHistory => self.toggle_history(),
            Action::ToggleFloating => self.toggle_floating(),
            Action::SwitchFloatingFocus => self.switch_floating_focus(),
        }
    }

    /// A key binding pressed: an open panel makes way for it first.
    fn run_bound(&mut self, action: Action) {
        // Alt+F4 with a panel open closes the panel, never the window behind it.
        let panel_open = self.explorer.is_open()
            || self.quick.is_open()
            || self.centre.is_open()
            || self.sheet.is_open()
            || self.history.is_open();
        if action == Action::Close && panel_open {
            self.close_panels();
            return;
        }
        // Volume and brightness keys leave quick settings and the explorer open, showing the
        // change there or keeping a half-typed query.
        let media = matches!(
            action,
            Action::Volume(_)
                | Action::ToggleMute { .. }
                | Action::Brightness(_)
                | Action::Media(_)
        );
        if self.quick.is_open() && !media && action != Action::QuickSettings {
            self.quick.close();
        }
        if self.centre.is_open() && action != Action::NotificationCentre {
            self.centre.close();
        }
        if self.explorer.is_open() && !media && action != Action::Explorer {
            self.explorer.close();
        }
        if self.sheet.is_open() && !media && action != Action::ShortcutSheet {
            self.sheet.close();
        }
        if self.history.is_open() && !media && action != Action::ClipboardHistory {
            self.history.close();
        }
        self.run_action(action)
    }

    /// Super tapped on its own: whatever Super+Space does now. The lock, a card or bullet time
    /// keeps the keys to itself, so there it does nothing.
    fn super_tapped(&mut self) {
        let keys_taken = self.lock.is_some()
            || self.share.is_some()
            || self.offer.is_some()
            || self.exit.as_ref().is_some_and(|exit| exit.modal())
            || self.bullet.is_some();
        if keys_taken {
            return;
        }
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        if let Some(action) = keys::action_for(&self.bindings, logo, Keysym::space) {
            tracing::info!(?action, "Super tapped");
            self.run_bound(action);
        }
    }

    /// The app id of the window with keyboard focus, if it has one.
    fn focused_app_id(&self) -> Option<String> {
        let surface = match self.seat.get_keyboard()?.current_focus()? {
            KeyboardFocus::Wayland(surface) => surface,
            // X11 apps name themselves with WM_CLASS; gamescope's is "gamescope".
            KeyboardFocus::X11(surface) => return Some(surface.class()),
        };
        with_states(&surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()?
                .lock()
                .ok()?
                .app_id
                .clone()
        })
    }

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

    /// What is up on the focused screen that a press on another screen has to respect.
    fn overlay_up(&self) -> crate::screen::Overlay {
        use crate::screen::Overlay;
        if self.share.is_some()
            || self.battery.card.is_some()
            || self.offer.is_some()
            || self.exit.as_ref().is_some_and(|exit| exit.modal())
            || self.switcher.is_some()
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

    /// Whether `pos` is over a notification pop-up, the toast or the volume display.
    pub fn over_chrome(&self, pos: Point<f64, Logical>) -> bool {
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

    /// The pointer is now at `pos`, from an absolute device (the host's pointer when nested, a
    /// tablet) or the `pointer:` debug step.
    /// The pointer moved: a slider held in quick settings follows it.
    fn drag_quick_slider(&mut self, pos: Point<f64, Logical>) {
        if !self.quick.is_open() {
            return;
        }
        let Some(screen) = self.focused_screen_geometry() else {
            return;
        };
        let request = self.quick.drag(pos.x - screen.loc.x as f64);
        self.quick_request(request);
    }

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
        self.exit_hover(pos);
        self.offer_hover(pos);
        self.battery_hover(pos);
        self.share_hover(pos);
        self.snip_pointer_moved(pos);
        self.bullet_pointer_moved(pos);
        self.drag_quick_slider(pos);
    }

    /// Keeps the pointer on screen: `pos`, or the nearest point on any output.
    pub fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let screens: Vec<_> = self
            .space
            .outputs()
            .filter_map(|o| self.space.output_geometry(o))
            .map(|geo| geo.to_f64())
            .collect();
        if screens.iter().any(|geo| geo.contains(pos)) {
            return pos;
        }
        screens
            .iter()
            .map(|geo| {
                Point::<f64, Logical>::from((
                    pos.x.clamp(geo.loc.x, geo.loc.x + geo.size.w - 1.0),
                    pos.y.clamp(geo.loc.y, geo.loc.y + geo.size.h - 1.0),
                ))
            })
            .min_by(|a, b| {
                let distance =
                    |p: &Point<f64, Logical>| (p.x - pos.x).powi(2) + (p.y - pos.y).powi(2);
                distance(a).total_cmp(&distance(b))
            })
            .unwrap_or(pos)
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
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
                self.exit_hover(pos);
                self.offer_hover(pos);
                self.battery_hover(pos);
                self.share_hover(pos);
                self.snip_pointer_moved(pos);
                self.bullet_pointer_moved(pos);
                self.drag_quick_slider(pos);
            }
            // The host's pointer when nested; tablets and touchscreens on real hardware.
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.wake_ui();
                let Some(output_geo) = self
                    .space
                    .outputs()
                    .next()
                    .and_then(|output| self.space.output_geometry(output))
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
                // The wheel over an overlay would otherwise scroll the window hidden under it. In
                // the explorer and the notification centre it moves through the list instead.
                if self.pointer_blocked() || self.over_chrome(self.pointer_location()) {
                    let amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                        event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                    });
                    let notch = if event.source() == AxisSource::Finger {
                        30.0
                    } else {
                        15.0
                    };
                    self.overlay_wheel(amount, notch);
                    return;
                }
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
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
            // Touchpad swipes, pinches and holds go to the window under the pointer.
            InputEvent::GestureSwipeBegin { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let fingers = event.fingers();
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
                let pointer = self.seat.get_pointer().unwrap();
                let delta = GestureSwipeUpdateEventTrait::delta(&event);
                pointer.gesture_swipe_update(
                    self,
                    &GestureSwipeUpdateEvent {
                        time: Event::time(&event),
                        delta,
                    },
                );
            }
            InputEvent::GestureSwipeEnd { event, .. } => {
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

impl Slipstream {
    /// A pointer button pressed or let go: from a mouse or touchpad, or the `button:` debug step,
    /// which takes the same path so a script exercises the real click routing.
    pub fn button_event(&mut self, button: u32, button_state: ButtonState, time: InputTime) {
        self.super_tap.disarm();
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

        // A snip being chosen takes every button: a drag, a click on a window, or a right click to
        // give up.
        if self.snip.is_some() {
            self.snip_button(button, button_state == ButtonState::Pressed);
            return;
        }

        // So does the share picker's.
        if self.share.is_some() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.share_click(pos.x, pos.y);
            }
            return;
        }
        // The offer's card takes every click the same way the way out's does.
        // The low battery card takes every click, as the offer's does.
        if self.battery.card.is_some() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.battery_click(pos.x, pos.y);
            }
            return;
        }
        if self.offer.is_some() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.offer_click(pos.x, pos.y);
            }
            return;
        }
        // The way out's card takes every click: its own buttons, and one beside it that
        // mustn't reach through to the windows it's asking about.
        if self.exit.as_ref().is_some_and(|exit| exit.modal()) {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.exit_click(pos.x, pos.y);
            }
            return;
        }

        // Alt+Tab's switcher takes every click while it's up: one on a tile goes to that window.
        if self.switcher.is_some() {
            if button_state == ButtonState::Pressed {
                let pos = pointer.current_location();
                let tile = self
                    .switcher
                    .as_ref()
                    .filter(|switcher| switcher.visible(self.wall()))
                    .and_then(|switcher| {
                        let screen = self.focused_screen_geometry()?;
                        switcher.tile_at(pos - screen.loc.to_f64())
                    });
                if let (Some(tile), Some(switcher)) = (tile, self.switcher.as_mut()) {
                    switcher.select(tile);
                    self.finish_cycle();
                }
            }
            return;
        }

        // In bullet time a button goes to what's under it: see `bullet_button`.
        if self.bullet.is_some() {
            let pos = pointer.current_location();
            self.bullet_button(button, button_state == ButtonState::Pressed, pos);
            return;
        }

        // The open explorer takes every click: on an item it opens it, outside it closes.
        if self.explorer.is_open() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                // A middle click on an app starts another, as Shift+Enter does.
                const BTN_MIDDLE: u32 = 0x112;
                self.explorer_click(pos.x, pos.y, button == BTN_MIDDLE);
            }
            return;
        }

        // The clipboard history: a row pastes, a click outside closes it.
        if self.history.is_open() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.history_click(pos);
            }
            return;
        }

        // So does the shortcut sheet: a click outside it closes it.
        if self.sheet.is_open() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.sheet_click(pos.x, pos.y);
            }
            return;
        }

        // Open quick settings takes every click too: its own, or one outside that closes it.
        if self.quick.is_open() {
            let screen = self.focused_screen_geometry();
            match (button_state, screen) {
                (ButtonState::Pressed, Some(screen)) => {
                    let pos = pointer.current_location() - screen.loc.to_f64();
                    self.quick_click(pos.x, pos.y);
                }
                (ButtonState::Released, _) => self.quick.release(),
                _ => {}
            }
            return;
        }

        // So does the open notification centre.
        if self.centre.is_open() {
            let screen = self.focused_screen_geometry();
            if let (ButtonState::Pressed, Some(screen)) = (button_state, screen) {
                let pos = pointer.current_location() - screen.loc.to_f64();
                self.centre_click(pos.x, pos.y);
            }
            return;
        }

        // A left click on a notification's pop-up opens it; another button dismisses it.
        if !pointer.is_grabbed() && !self.fullscreen_on_screen() {
            const BTN_LEFT: u32 = 0x110;
            // Pop-ups are drawn on the focused screen, and their areas kept in its coordinates.
            let origin = self
                .overlay_screen()
                .map_or((0.0, 0.0), |(_, screen)| (screen.x as f64, screen.y as f64));
            let pos = pointer.current_location();
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

    /// A key pressed or let go: from the keyboard, or `injected` by a debug step, which takes the
    /// same path so a script exercises the real routing.
    pub fn key_event(
        &mut self,
        keycode: Keycode,
        key_state: KeyState,
        time: InputTime,
        injected: bool,
    ) {
        let serial = SERIAL_COUNTER.next_serial();
        let pressed = key_state == KeyState::Pressed;
        // A key while the UI is faded only brings it back; that press goes nowhere. The lock
        // doesn't fade, so a key typed at it is never lost.
        let waking = self.wake_ui() && pressed && self.lock.is_none();
        // Looked up before `input`, which holds the keyboard lock while the filter runs.
        let gamescope_focused = self.focused_app_id().as_deref() == Some("gamescope");
        // A Super tap is off for games (fullscreen or gamescope), and for a real keyboard when
        // nested, where Alt stands in for Super and a tapped Alt is an app's menu bar.
        let fullscreen_focused = self
            .fullscreen
            .as_ref()
            .is_some_and(|full| self.focused_window().as_ref() == Some(full));
        // An app holding the shortcuts gets Super on its own too.
        let inhibited = self.shortcuts_inhibited();
        let tap_allowed =
            (injected || !self.nested) && !gamescope_focused && !fullscreen_focused && !inhibited;
        // Whether another key is already down, before `input` counts this one.
        let alone = !self
            .seat
            .get_keyboard()
            .unwrap()
            .pressed_keys()
            .iter()
            .any(|held| *held != keycode);
        let millis = time.millis();
        let mut tapped = false;
        // For `concentration`: a press that isn't a modifier, and whether a shortcut modifier was
        // held. Which key it was isn't kept.
        let mut typed_shortcut: Option<bool> = None;

        // A bound press is swallowed, and so is its matching release, so the focused
        // app never sees half a keypress.
        let action = self
            .seat
            .get_keyboard()
            .unwrap()
            .input::<Option<KeyUse>, _>(
                self,
                keycode,
                key_state,
                serial,
                time,
                |state, modifiers, handle| {
                    // Latin symbols keep bindings working on non-Latin layouts.
                    let key = handle
                        .raw_latin_sym_or_raw_current_sym()
                        .unwrap_or_else(|| handle.modified_sym());
                    // Injected keys mean what they mean in the login session: no
                    // nested Alt standing in for Super.
                    let mods = Mods::from_state(modifiers, state.nested && !injected);
                    if pressed && !key.is_modifier_key() {
                        typed_shortcut = Some(mods.ctrl || mods.alt || mods.logo);
                    }
                    // RUST_LOG=slipstream=trace shows every key as Slipstream sees it, except while
                    // locked, when a key is a character of a password.
                    if state.lock.is_none() {
                        tracing::trace!(?key, ?mods, pressed, "key");
                    }
                    // Caps Lock and Num Lock say which way they went, on the lock screen too,
                    // where a password typed in capitals is the thing to know about. The key
                    // itself still reaches the app as usual. xkb locks a modifier on the press
                    // but only unlocks it on the release, so the state is read on the release:
                    // on the press that turns it off, it still reads as on.
                    if !pressed && matches!(key, Keysym::Caps_Lock | Keysym::Num_Lock) {
                        if key == Keysym::Caps_Lock {
                            let on = modifiers.caps_lock;
                            // It holds off the wallpaper fade, not the lock; at the lock screen
                            // there's no fade to hold off, and the note is about the password.
                            let note = crate::osd::caps_lock_note(
                                on,
                                state.lock.is_some(),
                                state.idle.fades(),
                                state.idle.locks_by_itself(),
                            );
                            state.show_osd_with(crate::osd::Kind::CapsLock { on }, note);
                        } else {
                            let on = modifiers.num_lock;
                            let note = crate::osd::num_lock_note(on);
                            state.show_osd_with(crate::osd::Kind::NumLock { on }, note);
                        }
                    }
                    // Every key passes here, the ones panels and cards take included, so any of
                    // them disarms the tap. Super's own release still goes on as usual. A Super
                    // pressed at the lock never arms it, so letting go after unlocking isn't a tap.
                    let may_arm = alone && !waking && state.lock.is_none();
                    tapped = state.super_tap.key(key, pressed, millis, may_arm) && tap_allowed;
                    if waking {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(None);
                    }
                    // Ctrl+Alt+F1–F12 arrive as XF86Switch_VT_n. They belong to the
                    // session, never to an app.
                    let sym = handle.modified_sym().raw();
                    let first_vt = Keysym::XF86_Switch_VT_1.raw();
                    if pressed && (first_vt..=Keysym::XF86_Switch_VT_12.raw()).contains(&sym) {
                        state.suppressed_keys.push(key);
                        let vt = (sym - first_vt + 1) as i32;
                        return FilterResult::Intercept(Some(KeyUse::Action(Action::SwitchVt(vt))));
                    }
                    // Esc lets go of a drag under way, and a dragged gap goes back.
                    if pressed && state.drag.is_some() && handle.modified_sym() == Keysym::Escape {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::CancelDrag));
                    }
                    // While locked every key is the lock's: the pill takes characters, the media
                    // keys still act, and nothing else runs or reaches an app, press or release.
                    if state.lock.is_some() {
                        if !pressed {
                            if let Some(i) = state.suppressed_keys.iter().position(|k| *k == key) {
                                state.suppressed_keys.remove(i);
                            }
                            return FilterResult::Intercept(Some(KeyUse::LockRelease));
                        }
                        // Its release is swallowed too, should the lock be gone by then.
                        state.suppressed_keys.push(key);
                        if let Some(action) = crate::lock::allowed(key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        let typed = crate::lock::key_for(key, handle.modified_sym(), modifiers);
                        return FilterResult::Intercept(Some(KeyUse::Lock(typed)));
                    }
                    // Super+L locks whatever holds the keys: the share picker, the cards and bullet
                    // time all take keys before bindings, and locking closes each of them.
                    if pressed && keys::action_for(&state.bindings, mods, key) == Some(Action::Lock)
                    {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Action(Action::Lock)));
                    }
                    // The share picker takes every key: an app is waiting on it, and a
                    // keypress meant for a window could otherwise reach the window
                    // that's about to be shared.
                    // A snip being chosen takes every key: Esc, Enter or a window's letter.
                    if pressed && state.snip.is_some() {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Snip(handle.modified_sym())));
                    }
                    if pressed && state.share.is_some() {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Share(
                            handle.modified_sym(),
                            mods.shift,
                        )));
                    }
                    // The offer at login takes every key until it is answered: the
                    // desktop it is asking about doesn't exist yet.
                    // The low battery card takes every key until it's answered.
                    if pressed && state.battery.card.is_some() {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Battery(
                            handle.modified_sym(),
                        )));
                    }
                    if pressed && state.offer.is_some() {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Offer(handle.modified_sym())));
                    }
                    // The way out takes every key while it's asking, and none at all
                    // while the apps are closing: a save prompt needs them more.
                    if pressed && state.exit.as_ref().is_some_and(|exit| exit.modal()) {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Exit(handle.modified_sym())));
                    }
                    // The open explorer takes every key, but bindings still work: its own chord
                    // closes it, and any other closes it and then runs.
                    if pressed && state.explorer.is_open() {
                        state.suppressed_keys.push(key);
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        let sym = handle.modified_sym();
                        // Nothing types with Super, Ctrl or Alt held: those are chords.
                        let ch = xkb::keysym_to_utf8(sym)
                            .chars()
                            .next()
                            .filter(|ch| !ch.is_control() && !(mods.logo || mods.ctrl || mods.alt));
                        return FilterResult::Intercept(Some(KeyUse::Explorer(sym, ch, mods)));
                    }
                    // The shortcut sheet takes every key as well: typing filters it, and bindings
                    // close it and run.
                    // The clipboard history takes every key; bindings close it and run.
                    if pressed && state.history.is_open() {
                        state.suppressed_keys.push(key);
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        return FilterResult::Intercept(Some(KeyUse::History(
                            handle.modified_sym(),
                        )));
                    }
                    if pressed && state.sheet.is_open() {
                        state.suppressed_keys.push(key);
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        let sym = handle.modified_sym();
                        let ch = xkb::keysym_to_utf8(sym)
                            .chars()
                            .next()
                            .filter(|ch| !ch.is_control() && !(mods.logo || mods.ctrl || mods.alt));
                        return FilterResult::Intercept(Some(KeyUse::Sheet(sym, ch)));
                    }
                    // Open quick settings takes every key as well, but bindings still work:
                    // Super+A closes it, and Super+Space opens the explorer instead.
                    if pressed && state.quick.is_open() {
                        state.suppressed_keys.push(key);
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        return FilterResult::Intercept(Some(KeyUse::Quick(key, mods.shift)));
                    }
                    // So does the notification centre.
                    if pressed && state.centre.is_open() {
                        state.suppressed_keys.push(key);
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                        return FilterResult::Intercept(Some(KeyUse::Centre(key, mods.shift)));
                    }
                    // Bullet time takes every key too.
                    if pressed && state.bullet.is_some() {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Bullet(key, mods)));
                    }
                    // Esc with Alt still held puts Alt+Tab's switcher away, changing nothing.
                    if pressed && state.switcher.is_some() && key == Keysym::Escape {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::CancelSwitch));
                    }
                    // An app holding the shortcuts (a virtual machine, a remote desktop) gets every
                    // key but the one that takes them back. The hardware keys (volume, brightness,
                    // media) stay the laptop's.
                    if inhibited {
                        let action = keys::action_for(&state.bindings, mods, key);
                        let kept = matches!(action, Some(Action::TakeBack))
                            || (action.is_some() && keys::is_hardware_key(key));
                        if pressed && kept {
                            state.suppressed_keys.push(key);
                            return FilterResult::Intercept(Some(KeyUse::Action(action.unwrap())));
                        }
                        if !pressed {
                            if let Some(i) = state.suppressed_keys.iter().position(|k| *k == key) {
                                state.suppressed_keys.remove(i);
                                return FilterResult::Intercept(None);
                            }
                        }
                        return FilterResult::Forward;
                    }
                    // gamescope uses some Super keys itself and never asks to block ours.
                    if gamescope_focused && keys::passes_to_gamescope(mods, key) {
                        return FilterResult::Forward;
                    }
                    if pressed {
                        if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                            state.suppressed_keys.push(key);
                            return FilterResult::Intercept(Some(KeyUse::Action(action)));
                        }
                    } else if let Some(i) = state.suppressed_keys.iter().position(|k| *k == key) {
                        state.suppressed_keys.remove(i);
                        return FilterResult::Intercept(None);
                    }
                    FilterResult::Forward
                },
            );
        match action {
            Some(Some(KeyUse::Action(action))) => self.run_bound(action),
            Some(Some(KeyUse::Explorer(sym, ch, mods))) => self.explorer_key(sym, ch, mods),
            Some(Some(KeyUse::Bullet(key, mods))) => self.bullet_key(key, mods),
            Some(Some(KeyUse::Quick(key, shift))) => self.quick_key(key, shift),
            Some(Some(KeyUse::Centre(key, shift))) => self.centre_key(key, shift),
            Some(Some(KeyUse::Exit(key))) => self.exit_key(key),
            Some(Some(KeyUse::Offer(key))) => self.offer_key(key),
            Some(Some(KeyUse::Battery(key))) => self.battery_key(key),
            Some(Some(KeyUse::Share(key, shift))) => self.share_key(key, shift),
            Some(Some(KeyUse::Lock(key))) => self.lock_key(key),
            Some(Some(KeyUse::CancelDrag)) => self.cancel_drag(),
            Some(Some(KeyUse::CancelSwitch)) => self.cancel_cycle(),
            Some(Some(KeyUse::Sheet(sym, ch))) => self.sheet_key(sym, ch),
            Some(Some(KeyUse::Snip(sym))) => self.snip_key(sym),
            Some(Some(KeyUse::History(sym))) => self.history_key(sym),
            // Smithay tells the next focus which forwarded keys are still held. A key held as the
            // lock went up and let go under it would be announced as held at unlock, and the app
            // would repeat it, so the release is recorded here, with no focus to send it to.
            Some(Some(KeyUse::LockRelease)) => {
                let keyboard = self.seat.get_keyboard().unwrap();
                if keyboard.current_focus().is_none() && !keyboard.is_grabbed() {
                    keyboard.input_forward(self, keycode, KeyState::Released, serial, time, false);
                }
            }
            _ => {}
        }
        // A press the filter passed on reached the focused app. One Slipstream kept for itself
        // (the explorer, Alt+Tab, a binding) is talking to the desktop, which ends any typing.
        if let Some(shortcut) = typed_shortcut {
            if action.is_some() {
                self.concentration.end();
                self.concentration.input(std::time::Instant::now());
            } else if self.seat.get_keyboard().unwrap().current_focus().is_some() {
                self.concentration.key(std::time::Instant::now(), shortcut);
            }
        }
        if tapped {
            self.super_tapped();
        }
        // Alt+Tab settles when Alt is let go, as on Windows.
        if self.switcher.is_some() && !self.seat.get_keyboard().unwrap().modifier_state().alt {
            self.finish_cycle();
        }
    }

    /// A debug step's key, by its xkb name, pressed or let go through `key_event`. A name with no
    /// key in the current keymap is logged and skipped.
    pub fn inject_key(&mut self, name: &str, pressed: bool) {
        let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
        let Some(keycode) = (sym != Keysym::NoSymbol)
            .then(|| self.keycode_for(sym))
            .flatten()
        else {
            tracing::warn!(name, "debug step: no key for that name in the keymap");
            return;
        };
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.key_event(keycode, state, InputTime::now(), true);
    }

    /// The key that types `sym` without modifiers in the active layout.
    fn keycode_for(&mut self, sym: Keysym) -> Option<Keycode> {
        let keyboard = self.seat.get_keyboard()?;
        keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().ok()?;
            let layout = xkb.active_layout();
            (8..=255)
                .map(Keycode::new)
                .find(|code| xkb.raw_syms_for_key_in_layout(*code, layout).contains(&sym))
        })
    }
}

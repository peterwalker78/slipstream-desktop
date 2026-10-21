//! The timed debug steps (`SLIPSTREAM_DEBUG`), run as their times come round.

use super::*;

/// A notification from a debug step, before what it says: never expiring by itself.
fn debug_notice() -> crate::notify::Incoming {
    crate::notify::Incoming {
        id: crate::notify::next_id(),
        expire_timeout: -1,
        ..Default::default()
    }
}

impl Slipstream {
    /// `shot:` paths for the backend to save once it has drawn a frame.
    pub fn take_screenshots(&mut self) -> Vec<String> {
        std::mem::take(&mut self.shots.screenshots)
    }

    /// Runs the timed debug steps now due. Screenshots wait for the next frame.
    pub fn run_due_debug_steps(&mut self) {
        for step in self
            .debug
            .due(std::time::Duration::from_secs_f64(self.clock.wall()))
        {
            tracing::info!(?step, "debug step");
            // While locked, only steps that go through the same paths as the keyboard, the
            // pointer and apps do anything, so a script can't reach round the lock.
            if self.lock.is_some() && !step.works_while_locked() {
                tracing::info!("debug step ignored: locked");
                continue;
            }
            self.run_debug_step(step);
        }
    }

    fn run_debug_step(&mut self, step: debug::Step) {
        {
            match step {
                debug::Step::Workspace(n) => self.switch_workspace((n as usize).saturating_sub(1)),
                debug::Step::MoveTo(n) => {
                    self.move_focused_to_workspace((n as usize).saturating_sub(1))
                }
                debug::Step::Close => self.close_focused(),
                debug::Step::Run(command) => {
                    let command: Vec<String> =
                        command.split_whitespace().map(String::from).collect();
                    launch::run(&command, launch::ignore());
                }
                debug::Step::Wallpaper(id) => self.debug_wallpaper(&id),
                debug::Step::Demand(value) => self.rain.pin_demand(Some(value)),
                debug::Step::Fade(value) => {
                    self.idle.pinned = Some(1.0 - value.clamp(0.0, 1.0) as f64);
                }
                debug::Step::Slow(step) => self.clock.set_step(step),
                debug::Step::AddScreen(w, h) => self.add_made_up_screen(w, h),
                debug::Step::DropScreen(name) => self.drop_made_up_screen(&name),
                debug::Step::Lid(closed) => self.lid_switched(closed),
                debug::Step::Screens => self.log_screens(),
                debug::Step::Windows => self.log_windows(),
                debug::Step::Motion => self.log_reduced_motion(),
                debug::Step::NextScreen => self.focus_next_screen(),
                debug::Step::MoveToNextScreen => self.move_focused_to_next_screen(),
                debug::Step::SwapScreens => self.swap_with_next_screen(),
                debug::Step::WayOut => self.open_way_out(),
                debug::Step::LogOut => self.begin_exit(Intent::LogOut),
                debug::Step::Restart => self.begin_exit(Intent::Restart),
                debug::Step::ShutDown => self.begin_exit(Intent::ShutDown),
                debug::Step::ExitKey(name) => {
                    self.exit_key(xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS))
                }
                debug::Step::ExitClick(x, y) => self.exit_click(x, y),
                debug::Step::OfferKey(name) => {
                    self.offer_key(xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS))
                }
                debug::Step::OfferClick(x, y) => self.offer_click(x, y),
                debug::Step::Fullscreen(on) => {
                    if let Some(window) = self.focused_window() {
                        self.set_fullscreen(&window, on);
                    }
                }
                debug::Step::FocusTile(way) => self.debug_focus_tile(&way),
                debug::Step::ConnectKey(name) => {
                    self.connect_key(xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS))
                }
                debug::Step::SharePicker(kinds) => self.debug_share_picker(&kinds),
                debug::Step::Polkit(message) => self.open_stand_in_polkit(message),
                debug::Step::ShareKey(name) => {
                    let (key, shift) = shifted_key(&name);
                    self.share_key(key, shift)
                }
                debug::Step::ShareClick(x, y) => self.share_click(x, y),
                debug::Step::StopSharing => self.bar_clicked(bar::Target::Sharing),
                debug::Step::Quit => self.debug_quit(),
                debug::Step::Keys(keys) => {
                    for (name, pressed) in keys {
                        self.inject_key(&name, pressed);
                    }
                }
                debug::Step::Screenshot(path) => self.shots.screenshots.push(path),
                // Nested only, never in a login session (`lock::steps_allowed`).
                debug::Step::Lock if !crate::lock::steps_allowed() => {
                    tracing::warn!("debug step ignored: the lock's step runs nested only");
                }
                debug::Step::Lock => {
                    self.lock_now();
                }
                debug::Step::Battery(reading) => self.battery.pinned = reading,
                debug::Step::Explore => self.toggle_explorer(),
                debug::Step::ScreensOff(off) => self.set_screens_off(off),
                debug::Step::Tour(step, t) => self.debug_tour(step, t),
                debug::Step::SheetKey(name) => {
                    let sym = xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS);
                    self.sheet_key(sym, None);
                }
                // Key steps act only on an open panel. A closed one keeps its last selection, and a
                // mistimed step would otherwise press it.
                debug::Step::Type(_) | debug::Step::Key(_) if !self.explorer.is_open() => {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::Type(text) => {
                    for ch in text.chars() {
                        self.explorer_key(Keysym::NoSymbol, Some(ch), Mods::default());
                    }
                }
                debug::Step::Key(name) => {
                    let (mods, key) = debug::mods_and_key(&name);
                    self.explorer_key(key, None, mods);
                }
                debug::Step::MoveTile(direction) => self.move_tile(direction),
                debug::Step::Click(x, y) => self.debug_click(x, y),
                debug::Step::Drag(x1, y1, x2, y2) => self.debug_drag(x1, y1, x2, y2),
                debug::Step::Heavier => self.weigh(true),
                debug::Step::Lighter => self.weigh(false),
                debug::Step::Gravity => self.toggle_gravity(),
                debug::Step::Minimise => self.minimise_focused(),
                debug::Step::Restore => self.restore_latest(),
                debug::Step::HideAll => self.hide_all(),
                debug::Step::Dock => self.dock_focused(),
                debug::Step::Undock => self.undock(),
                debug::Step::Float => self.toggle_floating(),
                debug::Step::Idle => {
                    let now = self.clock.tick();
                    self.idle.fade(now);
                }
                debug::Step::Wake => {
                    self.wake_ui();
                }
                debug::Step::Button(button) => {
                    for pressed in [ButtonState::Pressed, ButtonState::Released] {
                        self.button_event(button, pressed, InputTime::now());
                    }
                }
                debug::Step::ButtonHeld(button, pressed) => {
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    self.button_event(button, state, InputTime::now());
                }
                debug::Step::Pointer(screen, x, y) => self.debug_pointer(screen.as_deref(), x, y),
                debug::Step::Cycle(tabs) => {
                    for _ in 0..tabs {
                        self.cycle_windows(true);
                    }
                    self.finish_cycle();
                }
                debug::Step::Tab => self.cycle_windows(true),
                debug::Step::LetGo => self.finish_cycle(),
                debug::Step::Unlock => self.unlock(),
                debug::Step::Bullet => self.toggle_bullet_time(),
                debug::Step::Swipe(direction) => self.swiped(direction),
                debug::Step::Fingers(Some((dx, dy))) => {
                    if self.swipe.is_none() {
                        self.swipe_began();
                    }
                    self.swipe_moved(dx, dy, self.wall());
                }
                debug::Step::Fingers(None) => self.swipe_ended(self.wall(), false),
                debug::Step::BulletKey(name) => {
                    let (mods, key) = debug::mods_and_key(&name);
                    self.bullet_key(key, mods);
                }
                debug::Step::BulletClick(x, y) => {
                    if self.bullet.is_some() {
                        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
                        let at = Point::from((origin.0 as f64 + x, origin.1 as f64 + y));
                        self.bullet_click(at);
                    }
                }
                debug::Step::Osd(what) => self.debug_osd(&what),
                debug::Step::Quick => self.toggle_quick_settings(),
                debug::Step::QuickKey(_) | debug::Step::QuickClick(..) if !self.quick.is_open() => {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::QuickKey(name) => {
                    let (key, shift) = shifted_key(&name);
                    self.quick_key(key, shift);
                }
                debug::Step::QuickClick(x, y) => self.quick_click(x, y),
                debug::Step::Centre => self.toggle_notification_centre(),
                debug::Step::CentreKey(_) | debug::Step::CentreClick(..)
                    if !self.centre.is_open() =>
                {
                    tracing::info!("debug step ignored: panel closed");
                }
                debug::Step::CentreKey(name) => {
                    let (key, shift) = shifted_key(&name);
                    self.centre_key(key, shift);
                }
                debug::Step::CentreClick(x, y) => self.centre_click(x, y),
                debug::Step::Notify(app_name, summary, body, actions) => {
                    self.notification_arrived(crate::notify::Incoming {
                        app_name,
                        summary,
                        body,
                        default_action: true,
                        actions,
                        urgency: 1,
                        ..debug_notice()
                    })
                }
                debug::Step::AskShortcuts(app, wanted) => self.debug_ask_shortcuts(app, wanted),
                debug::Step::NotifyCritical(app_name, summary, body) => {
                    self.notification_arrived(crate::notify::Incoming {
                        app_name,
                        summary,
                        body,
                        urgency: 2,
                        ..debug_notice()
                    })
                }
            }
        }
    }

    /// Every screen's wallpaper shows variation `id`.
    fn debug_wallpaper(&mut self, id: &str) {
        let now = self.clock.tick();
        // Every screen's wallpaper, and `any` would stop at the first.
        let shown: Vec<bool> = self
            .savers
            .values_mut()
            .map(|saver| saver.show(id, now))
            .collect();
        if !shown.into_iter().any(|shown| shown) {
            tracing::warn!(id, "no living-wallpaper variation by that name");
        }
    }

    /// The keyboard to the neighbouring tile `way` (left, right, up or down).
    fn debug_focus_tile(&mut self, way: &str) {
        let direction = match way {
            "left" => Direction::Left,
            "right" => Direction::Right,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => return,
        };
        self.focus_direction(direction);
    }

    /// The share picker, offering the screens (`m` in `kinds`), the windows (`w`), or both, as
    /// the portal would list them.
    fn debug_share_picker(&mut self, kinds: &str) {
        let mut list = String::new();
        if kinds.contains('m') {
            for output in self.space.outputs() {
                list.push_str(&format!("Monitor: {} test\n", output.name()));
            }
        }
        if kinds.contains('w') {
            for window in self.all_open_windows() {
                if let Some(listed) = self.captures.identifier_of(&window) {
                    list.push_str(&format!("Window: {} ({listed})\n", window_title(&window)));
                }
            }
        }
        self.open_share_picker(&list, None);
    }

    /// Straight out, as the watchdog goes: every headless check ends with this. A key still held
    /// back here means a press whose release never matched it.
    fn debug_quit(&mut self) {
        if !self.keys.suppressed_keys.is_empty() {
            tracing::warn!(keys = ?self.keys.suppressed_keys, "keys still suppressed at quit");
        }
        self.loop_signal.stop();
    }

    /// The tour at lesson `step`, its miniature held at `t` through its loop if given.
    fn debug_tour(&mut self, step: usize, t: Option<f32>) {
        self.close_panels();
        let now = self.clock.tick();
        self.sheet.reduced_motion = self.clock.reduced_motion;
        self.sheet.hold_tour(step, t, now);
    }

    /// The pointer to (`x`, `y`) on the named screen, or the focused one.
    fn debug_pointer(&mut self, screen: Option<&str>, x: f64, y: f64) {
        let rect = match screen {
            Some(name) => self
                .screens
                .iter()
                .position(|s| s.output.name() == name)
                .and_then(|index| self.screen_rect(index)),
            None => self.output_rect(),
        };
        let origin = rect.map_or((0, 0), |r| (r.x, r.y));
        let at = Point::from((origin.0 as f64 + x, origin.1 as f64 + y));
        self.pointer_moved_to(at, InputTime::now());
    }

    /// The volume or brightness display, `kind@level`. Only the card: a debug step that really
    /// changed the volume would change it on the laptop the nested run is on.
    fn debug_osd(&mut self, what: &str) {
        let (kind, level) = match what.split_once('@') {
            Some((kind, level)) => (kind, level.parse().unwrap_or(50)),
            None => (what, 50),
        };
        let kind = match kind {
            "muted" => osd::Kind::Volume { muted: true },
            "volume" => osd::Kind::Volume { muted: false },
            "mic" => osd::Kind::Microphone { muted: false },
            "micmuted" => osd::Kind::Microphone { muted: true },
            "brightness" => osd::Kind::Brightness,
            other => {
                tracing::warn!(other, "no such display");
                return;
            }
        };
        self.show_osd(kind, level);
    }

    /// An app asking for shortcuts through the portal, as `app` would.
    fn debug_ask_shortcuts(&mut self, app: String, wanted: Vec<(String, String, Option<String>)>) {
        use crate::shortcuts::portal::{Event, Wanted};
        let session = format!("/org/slipstream/debug/{}", app.replace('.', "_"));
        self.shortcut_event(Event::Opened {
            session: session.clone(),
            app,
        });
        // Nobody waits on the answer; the card's own answers are what's checked.
        let (reply, _) = async_channel::bounded(1);
        let wanted = wanted
            .into_iter()
            .map(|(id, description, preferred)| Wanted {
                id,
                description,
                preferred,
            })
            .collect();
        self.shortcut_event(Event::Bind {
            session,
            wanted,
            reply,
        });
    }

    /// A left click at a point on screen, in logical pixels, as the mouse would make it:
    /// motion, press, release. Debug scripts use it to reach what only the pointer can, such as
    /// an app's own menus.
    pub(super) fn debug_click(&mut self, x: f64, y: f64) {
        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
        let at = Point::<f64, Logical>::from((origin.0 as f64 + x, origin.1 as f64 + y));
        let time = InputTime::now();
        // Focus follows a click on a window, as the real button handler does. The bar and the
        // panels have their own debug steps, so this doesn't repeat their click handling.
        if let Some((window, _)) = self.window_under(at) {
            self.focus_window(&window);
        }
        let pointer = self.seat.get_pointer().unwrap();
        let under = self.surface_under(at);
        tracing::debug!(?at, on_a_surface = under.is_some(), "debug click");
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: at,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pointer.button(
                self,
                &ButtonEvent {
                    button: 0x110,
                    state,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    /// A press, a drag and a release, for the `drag:` debug step: selecting text in a terminal,
    /// which is one of the things clients do most and the one most likely to make them ask for a
    /// size of their own. In steps, since a client that follows the pointer only sees where it
    /// has been told it went.
    pub(super) fn debug_drag(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        const STEPS: usize = 8;
        let origin = self.output_rect().map_or((0, 0), |r| (r.x, r.y));
        let at = |x: f64, y: f64| {
            Point::<f64, Logical>::from((origin.0 as f64 + x, origin.1 as f64 + y))
        };
        let time = InputTime::now();
        if let Some((window, _)) = self.window_under(at(x1, y1)) {
            self.focus_window(&window);
        }
        let move_to = |state: &mut Self, x: f64, y: f64| {
            let location = at(x, y);
            let under = state.surface_under(location);
            let pointer = state.seat.get_pointer().unwrap();
            pointer.motion(
                state,
                under,
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(state);
        };
        let click = |state: &mut Self, button_state: ButtonState| {
            let pointer = state.seat.get_pointer().unwrap();
            pointer.button(
                state,
                &ButtonEvent {
                    button: 0x110,
                    state: button_state,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(state);
        };
        move_to(self, x1, y1);
        click(self, ButtonState::Pressed);
        for step in 1..=STEPS {
            let t = step as f64 / STEPS as f64;
            move_to(self, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
        }
        click(self, ButtonState::Released);
        tracing::debug!(from = ?(x1, y1), to = ?(x2, y2), "debug drag");
    }
}

/// A key a step names, which may start `shift+`, and whether it does.
fn shifted_key(name: &str) -> (Keysym, bool) {
    let (shift, name) = match name.strip_prefix("shift+") {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    (xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS), shift)
}

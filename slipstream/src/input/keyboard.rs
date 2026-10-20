//! Where a key goes: the lock, a card or panel of Slipstream's own, a binding, or the focused app.

use smithay::{
    backend::input::{InputTime, KeyState},
    input::keyboard::{FilterResult, Keycode, Keysym, xkb},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::SERIAL_COUNTER,
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{
    awake::Step,
    focus::KeyboardFocus,
    keys::{self, Action, Mods},
    state::Slipstream,
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
    /// A key for the offer at login.
    Offer(Keysym),
    /// A key for the card asking what a screen just plugged in should show.
    Connect(Keysym),
    /// A key for the low battery card.
    Battery(Keysym),
    /// A key for the lock screen's password pill.
    Lock(crate::lock::Key),
    /// A key for the card asking for a password on polkit's behalf.
    Polkit(crate::polkit::Key),
    /// A key let go while locked.
    LockRelease,
    /// Esc during a drag with the pointer.
    CancelDrag,
    /// Esc while Alt+Tab's switcher is up.
    CancelSwitch,
    /// Esc while gravity's arrangements are up.
    CancelArrange,
    /// ← or → while they're up: forward for →.
    StepArrange(bool),
    /// A key for the shortcut sheet, with the character it types.
    Sheet(Keysym, Option<char>),
    /// A key while a snip is being chosen.
    Snip(Keysym),
    /// A key for the clipboard history.
    History(Keysym),
    /// A zoom key, whatever holds the keyboard.
    Zoom(crate::zoom::Step),
}

/// The character `sym` types into a panel's search: none with Super, Ctrl or Alt held, since
/// those are chords.
fn typed_char(sym: Keysym, mods: Mods) -> Option<char> {
    if mods.logo || mods.ctrl || mods.alt {
        return None;
    }
    xkb::keysym_to_utf8(sym)
        .chars()
        .next()
        .filter(|ch| !ch.is_control())
}

/// Forgets one press of `key` kept back from the focused app, whose release is then kept back
/// too. Whether there was one.
fn release_suppressed(suppressed: &mut Vec<Keysym>, key: Keysym) -> bool {
    let Some(i) = suppressed.iter().position(|k| *k == key) else {
        return false;
    };
    suppressed.remove(i);
    true
}

impl Slipstream {
    fn run_action(&mut self, action: Action) {
        // The docked pane is on no workspace, so the keys that move or reshape the focused
        // window within one have nothing to act on while the keyboard is in it.
        if matches!(
            action,
            Action::MoveTile(_)
                | Action::Resize(_)
                | Action::MoveToWorkspace(_)
                | Action::MoveToWorkspaceBy(_)
                | Action::MoveToNextScreen
                | Action::Maximise
                | Action::ToggleFloating
        ) && self
            .focused_window()
            .is_some_and(|window| self.is_docked(&window))
        {
            return;
        }
        match action {
            Action::WayOut => self.open_way_out(),
            Action::Focus(direction) => self.focus_direction(direction),
            Action::MoveTile(direction) => self.move_tile(direction),
            Action::Resize(how) => self.resize_focused(how),
            Action::Rotate { clockwise } => self.rotate_layout(clockwise),
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
            Action::Quit => self.quit_focused_app(),
            Action::Edit(edit) => self.send_edit(edit),
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
            Action::ToggleGravity => self.toggle_gravity(),
            Action::Minimise => self.minimise_focused(),
            Action::Restore => self.restore_latest(),
            Action::HideAll => self.hide_all(),
            Action::Dock => self.dock_focused(),
            Action::Undock => self.undock(),
            Action::BulletTime => self.toggle_bullet_time(),
            Action::QuickSettings => self.toggle_quick_settings(),
            Action::NotificationCentre => self.toggle_notification_centre(),
            Action::Maximise => self.toggle_maximise(),
            Action::TakeBack => self.take_back(),
            Action::ClipboardHistory => self.toggle_history(),
            Action::ToggleFloating => self.toggle_floating(),
            Action::SwitchFloatingFocus => self.switch_floating_focus(),
            Action::Zoom(step) => self.zoom_step(step),
        }
    }

    /// A key binding pressed: an open panel makes way for it first.
    fn run_bound(&mut self, action: Action) {
        // On the tour, a key the lesson teaches is tried on its miniature, not the real windows.
        if self.sheet.touring() {
            let now = self.clock.tick();
            if self.sheet.practise(&action, now) {
                return;
            }
        }
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

    /// Super tapped on its own: the app explorer, as the Windows key opens Start. It is no
    /// binding — nothing is held with it — so the action is named here. The lock, a card or
    /// bullet time keeps the keys to itself, so there it does nothing.
    fn super_tapped(&mut self) {
        let keys_taken = self.lock.is_some()
            || self.polkit.is_up()
            || self.share.is_some()
            || self.offer.is_some()
            || self.connect.is_some()
            || self.exit.as_ref().is_some_and(|exit| exit.modal())
            || self.bullet.is_some();
        if keys_taken {
            return;
        }
        tracing::info!("Super tapped");
        self.run_bound(Action::Explorer);
    }

    /// Whether the window with keyboard focus is gamescope, which uses some Super keys itself and
    /// never asks to block ours. X11 apps name themselves with WM_CLASS.
    fn gamescope_focused(&self) -> bool {
        const GAMESCOPE: &str = "gamescope";
        let Some(focus) = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
        else {
            return false;
        };
        match focus {
            KeyboardFocus::Wayland(surface) => with_states(&surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .is_some_and(|data| {
                        data.lock()
                            .is_ok_and(|data| data.app_id.as_deref() == Some(GAMESCOPE))
                    })
            }),
            KeyboardFocus::X11(surface) => surface.class() == GAMESCOPE,
        }
    }

    /// A key pressed or let go: from the keyboard, or `injected` by a debug step, which takes the
    /// same path so a script exercises the real routing. Caps Lock is kept back first, until it's
    /// clear whether it was tapped or held (`awake.rs`).
    pub fn key_event(
        &mut self,
        keycode: Keycode,
        key_state: KeyState,
        time: InputTime,
        injected: bool,
    ) {
        let pressed = key_state == KeyState::Pressed;
        // The release of a Caps Lock press that only woke the faded UI goes nowhere either.
        if !pressed && self.caps_woke == Some(keycode) {
            self.caps_woke = None;
            return;
        }
        let caps = pressed && self.is_caps_lock(keycode);
        // Holding Caps Lock turns Awake on from the faded UI as well, since that's when it's
        // wanted; only the lock screen keeps Caps Lock as plain Caps Lock.
        let may_hold = self.lock.is_none();
        match self.caps_key.key(keycode, pressed, time, caps, may_hold) {
            Step::Pass => {}
            Step::Wait(press) => {
                // Kept back from xkb, but still a sign someone is there. If this press is what
                // brings the UI back, a tap of it goes nowhere, as any waking key does.
                self.caps_waking = self.wake_ui();
                let _ = self.loop_handle.insert_source(
                    Timer::from_duration(crate::awake::HOLD),
                    move |_, _, state| {
                        state.caps_lock_held(press, injected);
                        TimeoutAction::Drop
                    },
                );
                return;
            }
            Step::Replay(caps, at) => {
                if std::mem::take(&mut self.caps_waking) {
                    self.caps_woke = Some(caps);
                } else {
                    self.route_key(caps, KeyState::Pressed, at, injected);
                }
            }
            Step::Swallow => {
                self.wake_ui();
                return;
            }
        }
        self.route_key(keycode, key_state, time, injected);
    }

    /// Caps Lock's wait is over. Still down, it's a hold, and toggles Awake; if the lock came up
    /// meanwhile, the press goes to it as a plain Caps Lock.
    fn caps_lock_held(&mut self, press: u64, injected: bool) {
        if self.lock.is_some() {
            if let Some((keycode, at)) = self.caps_key.settle() {
                self.route_key(keycode, KeyState::Pressed, at, injected);
            }
            return;
        }
        if self.caps_key.elapsed(press) {
            self.caps_waking = false;
            self.set_awake(!self.awake);
        }
    }

    /// Whether `keycode` is Caps Lock in the active layout. Checked by symbol, so a layout that
    /// makes the key something else (Escape, Ctrl) leaves it alone.
    fn is_caps_lock(&mut self, keycode: Keycode) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        keyboard.with_xkb_state(self, |context| {
            let Ok(xkb) = context.xkb().lock() else {
                return false;
            };
            let layout = xkb.active_layout();
            xkb.raw_syms_for_key_in_layout(keycode, layout)
                .contains(&Keysym::Caps_Lock)
        })
    }

    fn route_key(
        &mut self,
        keycode: Keycode,
        key_state: KeyState,
        time: InputTime,
        injected: bool,
    ) {
        let serial = SERIAL_COUNTER.next_serial();
        let pressed = key_state == KeyState::Pressed;
        let keyboard = self.seat.get_keyboard().unwrap();
        // A key while the UI is faded only brings it back; that press goes nowhere. The lock
        // doesn't fade, so a key typed at it is never lost.
        let waking = self.wake_ui() && pressed && self.lock.is_none();
        // Looked up before `input`, which holds the keyboard lock while the filter runs.
        let gamescope_focused = self.gamescope_focused();
        // A Super tap is off for games (fullscreen or gamescope), and for a real keyboard when
        // nested, where Alt stands in for Super and a tapped Alt is an app's menu bar.
        let fullscreen_focused = self
            .focused_window()
            .is_some_and(|window| self.is_fullscreen(&window));
        // An app holding the shortcuts gets Super on its own too.
        let inhibited = self.shortcuts_inhibited();
        let tap_allowed = keys::taps_super(self.settings.keys.style)
            && (injected || !self.nested)
            && !gamescope_focused
            && !fullscreen_focused
            && !inhibited;
        // How many other keys are already down, before `input` counts this one.
        let others = keyboard
            .pressed_keys()
            .iter()
            .filter(|held| **held != keycode)
            .count();
        let alone = others == 0;
        let switches_layouts = self.layouts.switch_key(&self.settings.keyboard);
        let mut switch_layout = false;
        let millis = time.millis();
        let mut tapped = false;
        // For `concentration`: a press that isn't a modifier, and whether a shortcut modifier was
        // held. Which key it was isn't kept.
        let mut typed_shortcut: Option<bool> = None;

        // A bound press is swallowed, and so is its matching release, so the focused
        // app never sees half a keypress.
        let action = keyboard.input::<Option<KeyUse>, _>(
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
                // a password is being typed: at the lock, or on the password card.
                if state.lock.is_none() && !state.polkit.is_up() {
                    tracing::trace!(?key, ?mods, pressed, "key");
                }
                // Num Lock says which way it went; Caps Lock has its chip on the bar and its
                // warning at the lock instead. The key itself still reaches the app as usual.
                // xkb locks a modifier on the press but only unlocks it on the release, so
                // the state is read on the release: on the press that turns it off, it still
                // reads as on.
                if !pressed && key == Keysym::Num_Lock {
                    let on = modifiers.num_lock;
                    let note = crate::osd::num_lock_note(on);
                    state.show_osd_with(crate::osd::Kind::NumLock { on }, note);
                }
                // Every key passes here, the ones panels and cards take included, so any of
                // them disarms the tap. Super's own release still goes on as usual. A Super
                // pressed at the lock never arms it, so letting go after unlocking isn't a tap.
                let may_arm = alone && !waking && state.lock.is_none();
                tapped = state.super_tap.key(key, pressed, millis, may_arm) && tap_allowed;
                let only_alt_and_shift = modifiers.alt
                    && modifiers.shift
                    && !modifiers.ctrl
                    && !modifiers.logo
                    && !waking;
                switch_layout = state
                    .layout_tap
                    .key(key, pressed, others, only_alt_and_shift)
                    && switches_layouts;
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
                // The zoom keys work whatever holds the keyboard, the lock, the cards and an app
                // holding the shortcuts included: anyone who needs the zoom to read the screen
                // needs it there too. They only change how the screen is drawn.
                if pressed
                    && let Some(Action::Zoom(step)) = keys::action_for(&state.bindings, mods, key)
                {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Zoom(step)));
                }
                // While locked every key is the lock's: the pill takes characters, the media
                // keys still act, and nothing else runs or reaches an app, press or release.
                if state.lock.is_some() {
                    if !pressed {
                        release_suppressed(&mut state.suppressed_keys, key);
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
                if pressed && keys::action_for(&state.bindings, mods, key) == Some(Action::Lock) {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Action(Action::Lock)));
                }
                // The password card takes every key, before any other card: what is typed there
                // is a password, and must reach nothing else.
                if pressed && state.polkit.is_up() {
                    state.suppressed_keys.push(key);
                    let typed = crate::polkit::key_for(key, handle.modified_sym(), modifiers);
                    return FilterResult::Intercept(Some(KeyUse::Polkit(typed)));
                }
                // A snip being chosen takes every key: Esc, Enter or a window's letter.
                if pressed && state.snip.is_some() {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Snip(handle.modified_sym())));
                }
                // The share picker takes every key: an app is waiting on it, and a keypress
                // meant for a window could otherwise reach the window that's about to be shared.
                if pressed && state.share.is_some() {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Share(
                        handle.modified_sym(),
                        mods.shift,
                    )));
                }
                // The low battery card takes every key until it's answered.
                if pressed && state.battery.card.is_some() {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Battery(handle.modified_sym())));
                }
                // The offer at login takes every key until it is answered: the desktop it is
                // asking about doesn't exist yet.
                if pressed && state.offer.is_some() {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Offer(handle.modified_sym())));
                }
                // The screen card takes every key until it's answered, and ignores them for
                // its first moment, so a key already on its way can't answer it.
                if pressed && state.connect.is_some() {
                    state.suppressed_keys.push(key);
                    return FilterResult::Intercept(Some(KeyUse::Connect(handle.modified_sym())));
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
                    return FilterResult::Intercept(Some(KeyUse::Explorer(
                        sym,
                        typed_char(sym, mods),
                        mods,
                    )));
                }
                // The clipboard history takes every key; bindings close it and run.
                if pressed && state.history.is_open() {
                    state.suppressed_keys.push(key);
                    if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                        return FilterResult::Intercept(Some(KeyUse::Action(action)));
                    }
                    return FilterResult::Intercept(Some(KeyUse::History(handle.modified_sym())));
                }
                // The shortcut sheet takes every key as well: typing filters it, and bindings
                // close it and run.
                if pressed && state.sheet.is_open() {
                    state.suppressed_keys.push(key);
                    if let Some(action) = keys::action_for(&state.bindings, mods, key) {
                        return FilterResult::Intercept(Some(KeyUse::Action(action)));
                    }
                    let sym = handle.modified_sym();
                    return FilterResult::Intercept(Some(KeyUse::Sheet(
                        sym,
                        typed_char(sym, mods),
                    )));
                }
                // Open quick settings takes every key as well, but bindings still work:
                // Super+A closes it, and a tapped Super opens the explorer instead.
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
                // With Super still held after Super+T, Esc puts the workspace back and the
                // arrows move along the arrangements.
                if pressed && state.arrange.is_some() {
                    let arrange_key = match key {
                        Keysym::Escape => Some(KeyUse::CancelArrange),
                        Keysym::Left => Some(KeyUse::StepArrange(false)),
                        Keysym::Right => Some(KeyUse::StepArrange(true)),
                        _ => None,
                    };
                    if let Some(arrange_key) = arrange_key {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(arrange_key));
                    }
                }
                // An app holding the shortcuts (a virtual machine, a remote desktop) gets every
                // key but the one that takes them back. The hardware keys (volume, brightness,
                // media) stay the laptop's.
                if inhibited {
                    if pressed
                        && let Some(action) = keys::action_for(&state.bindings, mods, key)
                        && (action == Action::TakeBack || keys::is_hardware_key(key))
                    {
                        state.suppressed_keys.push(key);
                        return FilterResult::Intercept(Some(KeyUse::Action(action)));
                    }
                    if !pressed && release_suppressed(&mut state.suppressed_keys, key) {
                        return FilterResult::Intercept(None);
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
                } else if release_suppressed(&mut state.suppressed_keys, key) {
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
            Some(Some(KeyUse::Connect(key))) => self.connect_key(key),
            Some(Some(KeyUse::Battery(key))) => self.battery_key(key),
            Some(Some(KeyUse::Share(key, shift))) => self.share_key(key, shift),
            Some(Some(KeyUse::Lock(key))) => self.lock_key(key),
            Some(Some(KeyUse::Polkit(key))) => self.polkit_key(key),
            Some(Some(KeyUse::CancelDrag)) => self.cancel_drag(),
            Some(Some(KeyUse::CancelSwitch)) => self.cancel_cycle(),
            Some(Some(KeyUse::CancelArrange)) => self.cancel_arrangement(),
            Some(Some(KeyUse::StepArrange(forward))) => self.step_arrangement(forward),
            Some(Some(KeyUse::Sheet(sym, ch))) => self.sheet_key(sym, ch),
            Some(Some(KeyUse::Snip(sym))) => self.snip_key(sym),
            Some(Some(KeyUse::History(sym))) => self.history_key(sym),
            Some(Some(KeyUse::Zoom(step))) => self.zoom_step(step),
            // Smithay tells the next focus which forwarded keys are still held. A key held as the
            // lock went up and let go under it would be announced as held at unlock, and the app
            // would repeat it, so the release is recorded here, with no focus to send it to.
            Some(Some(KeyUse::LockRelease))
                if keyboard.current_focus().is_none() && !keyboard.is_grabbed() =>
            {
                keyboard.input_forward(self, keycode, KeyState::Released, serial, time, false);
            }
            _ => {}
        }
        // A press the filter passed on reached the focused app. One Slipstream kept for itself
        // (the explorer, Alt+Tab, a binding) is talking to the desktop, which ends any typing.
        if let Some(shortcut) = typed_shortcut {
            if action.is_some() {
                self.concentration.end();
                self.concentration.input(std::time::Instant::now());
            } else if keyboard.current_focus().is_some() {
                self.concentration.key(std::time::Instant::now(), shortcut);
            }
        }
        if tapped {
            self.super_tapped();
        }
        if switch_layout {
            self.next_layout();
        }
        // The switcher settles when the modifier it was opened with is let go: Alt for Alt+Tab,
        // Super for a Mac's ⌘Tab.
        let held = keyboard.modifier_state();
        let holding = if self.switcher_on_super {
            held.logo
        } else {
            held.alt
        };
        if self.switcher.is_some() && !holding {
            self.finish_cycle();
        }
        // Gravity's arrangement settles when Super is let go.
        if self.arrange.is_some() && !Mods::from_state(&keyboard.modifier_state(), self.nested).logo
        {
            self.finish_arrangement();
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
    pub(crate) fn keycode_for(&mut self, sym: Keysym) -> Option<Keycode> {
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

//! Where the keyboard goes: focusing a window, moving focus by direction and across screens,
//! and whether a new or activated window may take the keyboard at all.

use super::*;

impl Slipstream {
    /// Next in line after the focused window, so Alt+Tab reaches it first.
    pub(crate) fn next_in_line(&mut self, window: &Window) {
        self.focus_history.retain(|w| w != window);
        let at = self.focus_history.len().min(1);
        self.focus_history.insert(at, window.clone());
    }

    /// Whether `window` may take the keyboard now. While you're typing, only the app you're typing
    /// in may move it: to a dialog of its own, a window of the same process, or one from a program
    /// it started, such as a command run in a terminal. `asked_by` is the client that asked for an
    /// activation, which counts when it is the app being typed in.
    pub(crate) fn may_take_keyboard(&self, window: &Window, asked_by: Option<&ClientId>) -> bool {
        if !self.concentration.typing(std::time::Instant::now()) {
            return true;
        }
        let Some(typed_in) = self.focused_window() else {
            return true;
        };
        if *window == typed_in || is_child_of(window, &typed_in) {
            return true;
        }
        let typed_client = typed_in
            .wl_surface()
            .and_then(|surface| surface.client())
            .map(|client| client.id());
        if asked_by.is_some() && asked_by == typed_client.as_ref() {
            return true;
        }
        match (self.window_pid(window), self.window_pid(&typed_in)) {
            (Some(pid), Some(typed_pid)) => crate::concentration::descends_from(pid, typed_pid),
            _ => false,
        }
    }

    /// An app asked for `surface`'s window to come forward (xdg-activation). It does when the
    /// request carries a click or key press made since the keyboard last moved, and the typing rule
    /// allows it. Otherwise the window only moves up to next in line for Alt+Tab.
    pub fn activation_requested(
        &mut self,
        data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
        surface: &WlSurface,
    ) {
        let Some(window) = self
            .all_open_windows()
            .into_iter()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
        else {
            return;
        };
        let app = logged_app(&window);
        if self.focused_window().as_ref() == Some(&window) {
            return;
        }
        if !self.activation_is_recent(data) {
            tracing::info!(app, "activation asked without a recent click or key");
            self.next_in_line(&window);
            return;
        }
        if !self.may_take_keyboard(&window, data.client_id.as_ref()) {
            tracing::info!(app, "activation waits: typing in another app");
            self.next_in_line(&window);
            return;
        }
        tracing::info!(app, "activated");
        self.activate_window(&window);
    }

    /// Whether you asked for `window`: a dialog of the window you're using, a window from its
    /// process or a program it started, or one that appeared soon after a click, a key press or a
    /// launch from the desktop.
    pub(super) fn asked_for(&self, window: &Window) -> bool {
        // A game or a video asking to fill the screen from its first frame was started on purpose,
        // however long it took to load.
        if self.concentration.recently_asked(std::time::Instant::now())
            || self.fullscreen_on_map.contains(window)
        {
            return true;
        }
        let Some(focused) = self.focused_window() else {
            return false;
        };
        if is_child_of(window, &focused) {
            return true;
        }
        match (self.window_pid(window), self.window_pid(&focused)) {
            (Some(pid), Some(focused_pid)) => crate::concentration::descends_from(pid, focused_pid),
            _ => false,
        }
    }

    /// A token's proof that someone asked: a click or key press on this seat since the keyboard
    /// last moved, within a few seconds. Tokens Slipstream hands out itself carry no serial and
    /// get longer, since an app can take a while to start.
    pub(super) fn activation_is_recent(
        &self,
        data: &smithay::wayland::xdg_activation::XdgActivationTokenData,
    ) -> bool {
        let age = data.timestamp.elapsed();
        match &data.serial {
            Some((serial, seat)) => {
                let keyboard = self.seat.get_keyboard().unwrap();
                Seat::from_resource(seat).as_ref() == Some(&self.seat)
                    && age < std::time::Duration::from_secs(15)
                    && keyboard
                        .last_enter()
                        .is_some_and(|entered| serial.is_no_older_than(&entered))
            }
            None => data.client_id.is_none() && age < std::time::Duration::from_secs(60),
        }
    }

    /// Brings a window forward wherever it is: out of the code rain, or on its own workspace.
    pub fn activate_window(&mut self, window: &Window) {
        if self.rain.contains(window) {
            self.restore(window);
            return;
        }

        if let Some(workspace) = self.workspaces.find(window) {
            self.switch_workspace(workspace);
        }
        self.focus_window(window);
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.space
            .elements()
            .find(|window| focus.is_window(window))
            .cloned()
    }

    pub fn focus_window(&mut self, window: &Window) {
        // Nothing takes the keyboard while locked: a window that maps or asks gets it at unlock,
        // if the one that had it has gone.
        if self.lock.is_some() {
            tracing::debug!(window = logged_app(window), "no focus while locked");
            return;
        }
        // Nor while the password card is up, or keys are settling after it: the keyboard goes back
        // when they're done.
        if self.polkit.takes_keys() {
            tracing::debug!(
                window = logged_app(window),
                "no focus while a password is asked"
            );
            return;
        }
        // An X11 window that takes no input is never given the keyboard (ICCCM 4.1.7), nor is one
        // too see-through to be seen: whether it appears or is clicked, typing stays where it was.
        if takes_no_input(window) || unseen(window) {
            return;
        }
        let Some(target) = KeyboardFocus::for_window(window) else {
            return;
        };
        // Choosing another window on a fullscreen window's workspace ends the fullscreen first, so
        // the chosen window is never a sliver drawn over it or hidden behind it.
        if let Some(full) = self
            .workspaces
            .find(window)
            .and_then(|index| self.fullscreen_at(index))
            && full != *window
        {
            self.set_fullscreen(&full, false);
        }
        // A maximised window gives way the same way: the one chosen is underneath it.
        if self.workspaces.focused(window) {
            self.retile();
        }
        // Focusing a window on another screen moves the keyboard to that screen: whatever the
        // keys do next, they should do it where you are now looking.
        if let Some(index) = self
            .workspaces
            .find(window)
            .and_then(|workspace| self.screens.showing(workspace))
        {
            self.screens.focus(index);
        }
        // Alt+Tab's switcher keeps its own order, taken when it opened.
        self.remember_focus(window);
        // An X11 window's Wayland surface arrives just after it maps; it's focused again then.
        if window.x11_surface().is_some() && window.wl_surface().is_none() {
            self.pending_focus = Some(window.clone());
        }
        self.space.raise_element(window, true);
        // Floating windows stay above the tiles, and the one chosen, with its dialogs, above them.
        if let Some(index) = self.workspaces.find(window) {
            // Remembered straight away rather than only when the workspace is left: a screen the
            // keyboard moves off keeps naming this window in its bar.
            self.workspaces.get_mut(index).last_focus = Some(window.clone());
            self.workspaces
                .get_mut(index)
                .floating
                .raise(window, is_child_of);
            self.restack_floating(index);
        }
        if let (Some(xwm), Some(surface)) = (self.xwm.as_mut(), window.x11_surface()) {
            let _ = xwm.raise_window(surface);
        }
        let serial = SERIAL_COUNTER.next_serial();
        // In the session log, so a desktop that won't take the keyboard can be told apart from an
        // app that isn't listening: this says which app the keys are being sent to.
        tracing::info!(window = logged_app(window), "keyboard focus");
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Some(target), serial);
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }
        // Which window had focus is part of the record, and focus can move without a retile.
        self.note_layout_change();
    }

    pub fn clear_focus(&mut self) {
        if let Some(window) = self.focused_window() {
            deactivate(&window);
        }
        let serial = SERIAL_COUNTER.next_serial();
        tracing::info!("keyboard focus on nothing");
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Option::<KeyboardFocus>::None, serial);
    }

    /// Whether the keyboard is still somewhere sensible after the workspaces changed: on a window
    /// of the workspace the focused screen shows, or on another program's launcher or panel.
    pub(super) fn focus_survives_workspace_change(&self) -> bool {
        if self.keyboard_layer().is_some() {
            return true;
        }
        self.focused_window()
            .is_some_and(|window| self.workspaces.find(&window) == Some(self.active_workspace()))
    }

    /// Focuses the workspace's remembered window, or its newest, or nothing if it's empty. The
    /// remembered one is only taken if it is still here: it is set when a workspace is left, and
    /// the window can have closed or moved since.
    pub fn restore_focus(&mut self) {
        let ws = self.current_workspace();
        let here = ws.windows();
        // A fullscreen window here keeps the keyboard: a window closing behind it doesn't end it.
        // Nor does it end a maximise.
        let fullscreen = self
            .fullscreen
            .iter()
            .find(|window| here.contains(window))
            .cloned();
        let next = fullscreen
            .or_else(|| ws.maximised.clone())
            .or_else(|| {
                ws.last_focus
                    .clone()
                    .filter(|window| window.alive() && here.contains(window))
            })
            // A dialog closing gives the keyboard back to what was used before it.
            .or_else(|| {
                self.focus_history
                    .iter()
                    .find(|window| window.alive() && here.contains(window))
                    .cloned()
            })
            .or_else(|| here.last().cloned());
        match next {
            Some(window) => self.focus_window(&window),
            None => self.clear_focus(),
        }
    }

    /// Super+arrows: focus the neighbouring window that way. At the edge of the screen, left or
    /// right carries on to the screen beside it.
    pub fn focus_direction(&mut self, direction: Direction) {
        // Nothing focused (a window closed while the pointer was elsewhere, say): the first focus
        // key picks up whatever is on the workspace instead of doing nothing. An empty workspace
        // has nothing to pick up, so the key goes on to the screen beside.
        if self.focused_window().is_none() {
            if self.current_workspace().is_empty() && self.focus_across(direction, None) {
                return;
            }
            self.restore_focus();
            return;
        }
        let (Some(area), Some(current)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        // The docked pane is on no workspace: from it, a focus key goes back to the windows.
        if self.is_docked(&current) {
            self.restore_focus();
            return;
        }
        // From a floating window, the keys go between floating windows.
        if self.workspaces.is_floating(&current) {
            self.focus_floating_direction(&current, direction);
            return;
        }
        let active = self.screens.workspace();
        // Neighbours by their own tiles, even under a maximised window.
        let rects = self
            .workspaces
            .get_mut(active)
            .tiles_within(area, &min_size);
        if let Some(next) = layout::neighbour(&rects, &current, direction) {
            self.focus_window(&next);
            return;
        }
        // Up from the top of the tiles is the pane hanging from the bar, on the screen it is on.
        if direction == Direction::Up
            && self.screens.focused_index() == 0
            && let Some(docked) = self.docked_on_show()
        {
            self.focus_window(&docked);
            return;
        }
        let from = rects
            .iter()
            .find(|(window, _)| *window == current)
            .map(|(_, rect)| *rect);
        self.focus_across(direction, from);
    }

    /// The screen `direction` of the focused one, by where the screens actually sit — they can be
    /// stacked as well as side by side, so up and down cross screens too.
    pub(super) fn screen_towards(&self, direction: Direction) -> Option<usize> {
        let rects: Vec<Rect> = (0..self.screens.len())
            .map(|index| {
                self.screen_rect(index).unwrap_or(Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                })
            })
            .collect();
        crate::screen::towards(&rects, self.screens.focused_index(), direction)
    }

    /// Moves the keyboard to the screen `direction` of this one, on to the tile nearest the edge
    /// crossed and most level with `from`. Returns whether there was a screen there.
    pub(super) fn focus_across(&mut self, direction: Direction, from: Option<Rect>) -> bool {
        let Some(target) = self.screen_towards(direction) else {
            return false;
        };
        if let Some(leaving) = self.focused_window() {
            let active = self.active_workspace();
            self.workspaces.get_mut(active).last_focus = Some(leaving);
        }
        self.screens.focus(target);
        let workspace = self.active_workspace();
        let entering = self.output_area().and_then(|area| {
            let rects = self
                .workspaces
                .get_mut(workspace)
                .tiles_within(area, &min_size);
            layout::entering(&rects, direction, from)
        });
        match entering {
            Some(window) => self.focus_window(&window),
            None => self.restore_focus(),
        }
        tracing::info!(
            ?direction,
            workspace = workspace + 1,
            "focus crossed to the next screen"
        );
        true
    }
}

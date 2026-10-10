//! Windows: tiling them, mapping new ones, focus, moving and resizing tiles, fullscreen, closing,
//! and gravity's weights and arrangements.

use super::*;

impl Slipstream {
    /// Places the windows of the workspace each screen is showing, and takes every other
    /// workspace's windows off screen. The layout changes immediately; clients catch up when
    /// they handle the configure.
    pub fn retile(&mut self) {
        if self.screens.is_empty() {
            return;
        }
        // A screen that changed size moves its layer surfaces first, and so what they reserve.
        for output in self.space.outputs() {
            smithay::desktop::layer_map_for_output(output).arrange();
        }
        let shown: Vec<(usize, usize)> = self
            .screens
            .iter()
            .enumerate()
            .map(|(index, screen)| (index, screen.workspace))
            .collect();
        // Every screen lays its own workspace out in its own area, so a window's tile depends on
        // which screen is looking at it.
        let mut rects: Vec<(Window, Rect)> = Vec::new();
        // Floating windows, back to front, each with the size the keys asked for, if any.
        let mut floats: Vec<PlacedFloat> = Vec::new();
        for (index, workspace) in &shown {
            let Some(area) = self.screen_area(*index) else {
                continue;
            };
            let ws = self.workspaces.get_mut(*workspace);
            rects.extend(ws.rects_within(area, &min_size));
            floats.extend(
                ws.floating
                    .rects(area, &crate::floating::own_size)
                    .into_iter()
                    .map(|(window, rect)| {
                        let asked = ws.floating.get(&window).and_then(|float| float.asked);
                        (window, rect, asked)
                    }),
            );
        }
        // The docked window belongs to no workspace and stays mapped all the same.
        let docked = self.docked_on_show();
        let offscreen: Vec<Window> = self
            .space
            .elements()
            // X11 menus and tooltips aren't tiled; they go when their app unmaps them.
            .filter(|w| {
                !is_override_redirect(w)
                    && !rects.iter().any(|(on, _)| on == *w)
                    && !floats.iter().any(|(on, ..)| on == *w)
                    && docked.as_ref() != Some(*w)
            })
            .cloned()
            .collect();
        for window in offscreen {
            // Hidden by a switch or a minimise, it's no longer the one being looked at: apps
            // notify again, and terminals stop showing a focused cursor.
            deactivate(&window);
            self.space.unmap_elem(&window);
        }
        // A fullscreen window on a workspace being shown covers that screen, above its
        // neighbours. Fullscreen covers the bar too. One per screen: a video can fill the monitor
        // while the laptop is worked on, so this is a window and a rect for each shown workspace,
        // not one for the desktop.
        let fullscreens: Vec<(Window, Rect)> = shown
            .iter()
            .filter_map(|(_, workspace)| {
                let window = self.fullscreen_at(*workspace)?;
                Some((window, self.screen_rect_for_workspace(*workspace)?))
            })
            .collect();
        let now = self.clock.tick();
        self.tiled_mins = rects
            .iter()
            .map(|(window, _)| (window.clone(), min_size(window)))
            .collect();
        for (window, tile) in rects {
            let full_rect = fullscreens
                .iter()
                .find(|(full, _)| *full == window)
                .map(|(_, screen)| *screen);
            let full = full_rect.is_some();
            let r = full_rect.unwrap_or(tile);
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
            // The space holds the real position for input; the drawing glides there, except
            // while a gap is dragged, when the tiles keep up with the pointer.
            self.motion.place(&window, r, now);
            if matches!(self.drag, Some(crate::grabs::Drag::Gap { .. })) {
                self.motion.jump(&window, r);
            }
            // Moving a window that's already mapped keeps its place in the stacking order;
            // mapping it again would put the tiles in tree order, and the focused window's menus
            // could end up under a neighbour.
            if self.space.element_location(&window).is_some() {
                self.space.relocate_element(&window, (r.x, r.y));
            } else {
                self.space.map_element(window, (r.x, r.y), false);
            }
        }
        self.floating_sizes = floats
            .iter()
            .map(|(window, ..)| (window.clone(), crate::floating::own_size(window)))
            .collect();
        for (window, r, asked) in floats {
            crate::floating::configure_floating(&window, asked, r);
            self.motion.place(&window, r, now);
            // Under the pointer it keeps up with the pointer.
            if space_placed_now(&self.drag, &window) {
                self.motion.jump(&window, r);
            }
            if self.space.element_location(&window).is_some() {
                self.space.relocate_element(&window, (r.x, r.y));
            } else {
                self.space.map_element(window, (r.x, r.y), false);
            }
        }
        // The focused window above its neighbours, so its popups are drawn over them and found
        // first by the pointer.
        if let Some(focused) = self.focused_window()
            && self.space.element_location(&focused).is_some()
        {
            self.space.raise_element(&focused, false);
        }
        // A maximised window is above its neighbours, so it's drawn over them and the pointer
        // finds it first; floating windows are above that, and a fullscreen one above everything.
        for (_, workspace) in &shown {
            if let Some(window) = self.workspaces.get(*workspace).maximised.clone() {
                self.space.raise_element(&window, false);
            }
            self.restack_floating(*workspace);
        }
        // The docked pane is in front of them all, and a fullscreen window in front of that.
        self.place_docked(now);
        for (window, _) in &fullscreens {
            self.space.raise_element(window, false);
        }
        // Workspaces no screen is showing follow too, so a change to a tiling area (the code
        // rain taking or giving back width) reaches their windows before bullet time shows them.
        for index in 0..self.workspaces.count() {
            self.place_workspace(index);
        }
        self.update_window_scales();
        // Every layout change of any kind ends here: opening, closing, moving, swapping, gravity,
        // the code rain and switching workspace all retile.
        self.note_layout_change();
    }

    /// Tells every window the scale of the screen it's mostly on, so it draws sharp there: a
    /// window moved to a 1× monitor stops drawing 1.25× buffers, and the panel's windows keep
    /// 1.25× even when the monitor lit first.
    pub(super) fn update_window_scales(&self) {
        use smithay::wayland::{
            compositor::send_surface_state, fractional_scale::with_fractional_scale,
        };
        for window in self.space.elements() {
            let Some(output) = self.output_for_window(window) else {
                continue;
            };
            let scale = output.current_scale();
            let (fractional, integer) = (scale.fractional_scale(), scale.integer_scale());
            window.with_surfaces(|surface, states| {
                with_fractional_scale(states, |preferred| {
                    preferred.set_preferred_scale(fractional)
                });
                send_surface_state(surface, states, integer, smithay::utils::Transform::Normal);
            });
        }
    }

    /// The screen a window is mostly on, or the one under its corner. Worked out from where the
    /// space has it now rather than the space's list of outputs per window, which only catches up
    /// at the next refresh.
    pub fn output_for_window(&self, window: &Window) -> Option<Output> {
        let geometry = self.space.element_geometry(window)?;
        let overlap = |output: &Output| {
            self.space
                .output_geometry(output)
                .and_then(|screen| screen.intersection(geometry))
                .map_or(0, |overlap| overlap.size.w as i64 * overlap.size.h as i64)
        };
        self.space
            .outputs()
            .filter(|output| overlap(output) > 0)
            .max_by_key(|output| overlap(output))
            .or_else(|| {
                self.space.outputs().find(|output| {
                    self.space
                        .output_geometry(output)
                        .is_some_and(|screen| screen.contains(geometry.loc))
                })
            })
            .cloned()
    }

    /// Puts the screens side by side again after one comes or goes: laptop panels first at the
    /// left edge, then the others in the order they connected. The code rain stays on the panel,
    /// whichever order the screens lit up in, the lid opening included.
    pub(super) fn arrange_screens(&mut self) {
        // Modes and scales first: both change how big a screen is, and where the screens sit is
        // worked out from their sizes. Doing it here covers every way a screen's size can change
        // — starting up, plugging one in, or a choice made in Settings — from one place.
        self.follow_screen_settings();
        let lit: Vec<Placing> = self
            .screen_order
            .iter()
            .filter_map(|name| {
                let output = self
                    .screens
                    .iter()
                    .find(|s| s.output.name() == *name)?
                    .output
                    .clone();
                let size = self.space.output_geometry(&output)?.size;
                Some(Placing {
                    name: name.clone(),
                    width: size.w,
                    height: size.h,
                    place: self.settings.display.place(&self.monitor_id(&output)),
                })
            })
            .collect();
        let nested_panel = self.nested_panel_name.clone();
        let places = arrange(&lit, |name| {
            is_internal_panel(name) || nested_panel.as_deref() == Some(name)
        });
        let mut moved = false;
        for (name, x, y) in places {
            let Some(output) = self
                .screens
                .iter()
                .find(|screen| screen.output.name() == name)
                .map(|screen| screen.output.clone())
            else {
                continue;
            };
            let at = self.space.output_geometry(&output).map(|geo| geo.loc);
            if at.map(|loc| (loc.x, loc.y)) != Some((x, y)) {
                self.space.map_output(&output, (x, y));
                output.change_current_state(None, None, None, Some((x, y).into()));
                self.screens
                    .add(output, x, y, self.workspaces.count(), None);
                moved = true;
            }
        }
        if moved {
            tracing::info!(
                screens = ?self.screens.iter().map(|s| (s.output.name(), s.x)).collect::<Vec<_>>(),
                "screens arranged"
            );
            // The pointer stays on a screen that's there.
            let pos = self.pointer_location();
            self.pointer_moved_to(self.clamp_to_outputs(pos), InputTime::now());
        }
    }

    /// A toplevel that has just committed its first buffer: now it is a window, so it takes a
    /// tile and the keyboard. Clients create toplevels they never show, and one of those must
    /// cost nothing (`new_toplevel` in `handlers/xdg_shell.rs`).
    pub fn map_if_ready(&mut self, surface: &WlSurface) {
        let Some(at) = self
            .unmapped
            .iter()
            .position(|window| window.wl_surface().as_deref() == Some(surface))
        else {
            return;
        };
        let has_buffer =
            with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false);
        if !has_buffer {
            return;
        }
        let window = self.unmapped.remove(at);
        self.add_window(window.clone());
        // It asked to be fullscreen while it had nothing to show; now it has.
        if let Some(at) = self
            .fullscreen_on_map
            .iter()
            .position(|waiting| waiting == &window)
        {
            self.fullscreen_on_map.remove(at);
            self.set_fullscreen(&window, true);
        }
    }

    /// Answers a toplevel's commit with its initial configure, if it hasn't had one: the size and
    /// tiled edges of the tile it will be given, so its first buffer fits and it doesn't open at a
    /// size of its own and snap. A window still waiting to map is predicted as `add_window` will
    /// place it, beside the focused window; one already tiled (which unmapped itself and is coming
    /// back) gets its own tile.
    pub fn send_initial_configure(&mut self, surface: &WlSurface) {
        let Some(window) = self
            .unmapped
            .iter()
            .chain(self.space.elements())
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
        else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        if toplevel.is_initial_configure_sent() {
            return;
        }
        if let Some((r, full)) = self.expected_place(&window) {
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
        }
        toplevel.send_configure();
    }

    /// Where `window` will be, and whether fullscreen: its tile if it has one, or the tile it would
    /// get beside the focused window on the workspace on screen.
    pub(super) fn expected_place(&self, window: &Window) -> Option<(Rect, bool)> {
        // A window that will float chooses its own size.
        let fullscreen_asked =
            self.fullscreen_on_map.contains(window) || self.is_fullscreen(window);
        if !fullscreen_asked
            && (self.workspaces.is_floating(window)
                || (self.workspaces.find(window).is_none()
                    && crate::floating::opens_floating(window)))
        {
            return None;
        }
        if self.fullscreen_on_map.contains(window) || self.is_fullscreen(window) {
            let workspace = self
                .workspaces
                .find(window)
                .unwrap_or_else(|| self.active_workspace());
            return self.screen_rect_for_workspace(workspace).map(|r| (r, true));
        }
        let (workspace, area) = match self.workspaces.find(window) {
            Some(index) => (index, self.area_for_workspace(index)?),
            None => (self.active_workspace(), self.output_area()?),
        };
        let mut layout = self.workspaces.get(workspace).layout.clone();
        if self.workspaces.find(window).is_none() {
            layout.insert(window.clone(), self.focused_window().as_ref(), area);
        }
        layout
            .rects_within(area, &min_size)
            .into_iter()
            .find(|(tiled, _)| tiled == window)
            .map(|(_, r)| (r, false))
    }

    /// Tiles a new window beside the focused one, on the workspace on screen, and focuses it.
    pub fn add_window(&mut self, window: Window) {
        let area = self.output_area().unwrap_or(Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        });
        // A window a recorded place is still waiting for goes where it was instead.
        if self.claim_restored(&window, area) {
            return;
        }
        // A window you didn't ask for, or one that would take the keyboard while you type in
        // another app, pours into the code rain instead, and a toast says so.
        if !self.fullscreen_on_screen()
            && !(self.asked_for(&window) && self.may_take_keyboard(&window, None))
        {
            self.pour_new(window);
            return;
        }
        // Dialogs and fixed-height windows float, unless they're about to fill the screen.
        if crate::floating::opens_floating(&window) && !self.fullscreen_on_map.contains(&window) {
            self.add_floating(window);
            return;
        }
        // Split the focused window, so new windows appear next to what you're working on.
        let beside = self.focused_window();
        let active = self.active_workspace();
        self.workspaces
            .insert(active, window.clone(), beside.as_ref(), area);
        // Over a fullscreen window on screen, a new window waits behind it without the keyboard,
        // so a game or a video isn't interrupted, unless it's that window's own dialog.
        let covering = self
            .fullscreen_at(active)
            .filter(|full| *full != window && self.fullscreen_on_screen());
        match covering {
            Some(full) if !is_child_of(&window, &full) => {
                self.retile();
                self.next_in_line(&window);
                tracing::info!(
                    workspace = active + 1,
                    windows = self.current_workspace().len(),
                    x11 = window.x11_surface().is_some(),
                    "new window tiled behind the fullscreen one"
                );
                return;
            }
            Some(full) => {
                self.set_fullscreen(&full, false);
                self.focus_window(&window);
            }
            None => {
                self.retile();
                if self.may_take_keyboard(&window, None) {
                    self.focus_window(&window);
                } else {
                    self.next_in_line(&window);
                }
            }
        }
        tracing::info!(
            workspace = active + 1,
            windows = self.current_workspace().len(),
            x11 = window.x11_surface().is_some(),
            min = ?min_size(&window),
            focused = self.focused_window().as_ref() == Some(&window),
            "new window tiled"
        );
    }

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

    /// A new window nobody asked for goes straight into a stream of code rain, and a toast names
    /// it and the way to bring it in.
    pub(super) fn pour_new(&mut self, window: Window) {
        let Some(scale) = self.output_scale() else {
            return;
        };
        let name = self.stream_name(&window);
        let icon_px = crate::rain::icon_px(scale);
        let icon = window_app_id(&window).and_then(|id| self.explorer.app_icon(&id, icon_px));
        let pid = self.window_pid(&window);
        let now = self.clock.tick();
        self.rain.add(window.clone(), name.clone(), icon, pid, now);
        self.retile();
        self.show_toast(
            "Opened in a stream",
            &format!("{name}: Super+Shift+M or a click on its stream brings it in."),
        );
        tracing::info!(
            window = logged_app(&window),
            "new window poured into code rain"
        );
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

    /// The client that closed its last window is now running invisibly in the background; record
    /// it for the notification centre. Called after `remove_window` so the closing window is
    /// already gone from `all_open_windows`.
    pub(crate) fn track_closed_app(&mut self, wl_surface: &WlSurface) {
        let Some(client) = wl_surface.client() else {
            return;
        };
        let client_id = client.id();
        let app_id = with_states(wl_surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()?
                .lock()
                .ok()?
                .app_id
                .clone()
        });
        let Some(app_id) = app_id else {
            return;
        };
        let still_open = self.all_open_windows().into_iter().any(|w| {
            w.toplevel()
                .and_then(|t| t.wl_surface().client())
                .map(|c| c.id() == client_id)
                .unwrap_or(false)
        });
        if !still_open && !self.background_apps.iter().any(|a| a.client_id == client_id) {
            let name = pretty_name(&app_id);
            self.background_apps.push(BackgroundApp { name, client_id });
        }
    }

    /// A client that was only running in the background has opened a new window; remove it from
    /// the background list.
    pub(crate) fn untrack_background_app(&mut self, wl_surface: &WlSurface) {
        if let Some(client) = wl_surface.client() {
            let id = client.id();
            self.background_apps.retain(|app| app.client_id != id);
        }
    }

    /// Takes a closed window out of its workspace, re-tiles, and moves focus on.
    pub fn remove_window(&mut self, window: &Window) {
        self.leave_a_ghost(window);
        if self.pending_focus.as_ref() == Some(window) {
            self.pending_focus = None;
        }
        self.fullscreen.retain(|held| held != window);
        self.refloat.retain(|(held, _)| held != window);
        self.floating_sizes.retain(|(held, _)| held != window);
        self.centre_when_sized.retain(|(held, _)| held != window);
        self.focus_history.retain(|w| w != window);
        if let Some(plan) = self.restoring.as_mut() {
            plan.release(window);
        }
        if let Some(switcher) = self.switcher.as_mut()
            && !switcher.forget(window)
        {
            self.switcher = None;
            self.deck = None;
        }
        if let Some(deck) = self.deck.as_mut() {
            deck.forget(window);
        }
        if self
            .pass
            .as_ref()
            .is_some_and(|pass| pass.front == *window || pass.back == *window)
        {
            self.pass = None;
        }
        let was_on = self.take_off_workspace(window);
        self.forget_docked(window);
        self.rain.remove(window);
        self.tags.retain(|(tagged, ..)| tagged != window);
        if let Some(mode) = self.bullet.as_mut() {
            if mode
                .selected
                .as_ref()
                .is_some_and(|target| target.window() == window)
            {
                mode.selected = None;
            }
            mode.labels.retain(|(target, _)| target.window() != window);
        }
        self.overview_hits.retain(|(hit, _)| hit != window);
        self.fitted.retain(|(hit, _)| hit != window);
        self.tiled_mins.retain(|(hit, _)| hit != window);
        self.motion.remove(window);
        self.space.unmap_elem(window);
        self.retile();
        // Whatever is left on the workspace takes focus, so closing a tile never leaves the
        // keyboard aimed at nothing — which would make the focus keys do nothing at all.
        if was_on == Some(self.active_workspace()) {
            self.restore_focus();
        }
        tracing::info!(
            windows = self.current_workspace().len(),
            "window closed; re-tiled"
        );
    }

    /// The window for an X11 surface: tiled on some workspace, minimised into the code rain, or a
    /// menu on screen.
    pub fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.all_open_windows()
            .into_iter()
            .chain(self.space.elements().cloned())
            .find(|window| window.x11_surface() == Some(surface))
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

    /// Focuses the workspace's remembered window, or its newest, or nothing if it's empty. The
    /// remembered one is only taken if it is still here: it is set when a workspace is left, and
    /// the window can have closed or moved since.
    /// Whether the keyboard is still somewhere sensible after the workspaces changed: on a window
    /// of the workspace the focused screen shows, or on another program's launcher or panel.
    pub(super) fn focus_survives_workspace_change(&self) -> bool {
        if self.keyboard_layer().is_some() {
            return true;
        }
        self.focused_window()
            .is_some_and(|window| self.workspaces.find(&window) == Some(self.active_workspace()))
    }

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

    /// Super+Alt+arrow: swaps the focused window with the one in that direction, so a tile can
    /// be moved about the workspace without the mouse. The arrangement keeps its shape; the two
    /// windows change places inside it, and focus rides along with the window that moved. Under
    /// gravity the places carry their roles, so moving into the centre takes the centre.
    pub fn move_tile(&mut self, direction: Direction) {
        let (Some(area), Some(current)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        if self.workspaces.is_floating(&current) {
            self.move_floating(&current, direction);
            return;
        }
        let active = self.screens.workspace();
        // Neighbours by their own tiles, even under a maximised window.
        let rects = self
            .workspaces
            .get_mut(active)
            .tiles_within(area, &min_size);
        let Some(next) = layout::neighbour(&rects, &current, direction) else {
            // At the edge of the screen, the window goes on to the screen beside.
            if let Some(target) = self.screen_towards(direction)
                && let Some(workspace) = self.screens.get(target).map(|screen| screen.workspace)
            {
                self.move_window_to_workspace(&current, workspace, false);
                self.focus_screen_at(target);
                self.focus_window(&current);
                tracing::info!(?direction, "moved window to the next screen");
            }
            return;
        };
        let now = self.clock.tick();
        let before = (
            self.motion.frame(&current, now).map(|frame| frame.rect),
            self.motion.frame(&next, now).map(|frame| frame.rect),
        );
        if self.current_workspace_mut().swap(&current, &next) {
            self.retile();
            // The two pass through each other as panes of glass on their way.
            self.pass = match (
                before,
                self.motion.target(&current),
                self.motion.target(&next),
            ) {
                ((Some(a), Some(b)), Some(to_a), Some(to_b)) if !self.clock.reduced_motion => {
                    Some(crate::pane::Pass::new(
                        current.clone(),
                        next.clone(),
                        [a, b],
                        [to_a, to_b],
                        now,
                    ))
                }
                _ => None,
            };
            tracing::info!(?direction, "moved window within the workspace");
        }
    }

    /// Super+[ ] and Super+Shift+[ ]: the split beside the focused tile moves a step, on the
    /// press, and the tiles glide to their new sizes.
    /// Super+R and Super+Shift+R: the workspace's whole layout turns a quarter, clockwise or
    /// anticlockwise, every tile going with it. Which window has the keyboard doesn't matter.
    pub fn rotate_layout(&mut self, clockwise: bool) {
        let Some(window) = self.focused_window() else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            self.show_toast(
                "This window floats",
                "Super+Shift+V puts it back in the tiling.",
            );
            return;
        }
        let ws = self.current_workspace();
        if ws.gravity.is_on() {
            self.show_toast(
                "Only tiling turns",
                "Gravity has no rows or columns to turn. Super+T goes back to tiling.",
            );
            return;
        }
        let active = self.active_workspace();
        if !self.workspaces.get_mut(active).layout.rotate(clockwise) {
            return;
        }
        tracing::info!(clockwise, "turned the layout");
        self.retile();
    }

    pub fn resize_focused(&mut self, how: layout::Resize) {
        use layout::Resized;
        let (Some(area), Some(window)) = (self.output_area(), self.focused_window()) else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            self.resize_floating(&window, how);
            return;
        }
        let ws = self.current_workspace();
        // A window filling the screen or the tiling area has no neighbour to give room to.
        if self.is_fullscreen(&window) || ws.maximised.as_ref() == Some(&window) {
            return;
        }
        // Under gravity a window's size is its weight: wider or taller is heavier.
        if ws.gravity.is_on() {
            let heavier = matches!(how, layout::Resize::Wider | layout::Resize::Taller);
            self.weigh_window(&window, heavier);
            return;
        }
        let resized = self
            .current_workspace_mut()
            .layout
            .resize(&window, how, area, &min_size);
        match resized {
            Resized::Changed(ratio) => {
                tracing::info!(?how, ratio, "resized the tile");
                self.retile();
            }
            // At its limit, or alone, the tile simply doesn't move: held keys would otherwise
            // repeat the same message.
            Resized::AtLimit | Resized::NothingBeside => {}
        }
    }

    /// A fullscreen window on the workspace `index`, so the bar and the wallpaper give way to it.
    pub fn fullscreen_on(&self, index: usize) -> bool {
        self.fullscreen_at(index).is_some()
    }

    /// The window filling workspace `index`'s screen, if one is. At most one can: a second window
    /// going fullscreen there takes the first out of it (`set_fullscreen`).
    pub fn fullscreen_at(&self, index: usize) -> Option<Window> {
        self.fullscreen
            .iter()
            .find(|window| self.workspaces.find(window) == Some(index))
            .cloned()
    }

    /// Whether `window` is filling its screen.
    pub fn is_fullscreen(&self, window: &Window) -> bool {
        self.fullscreen.contains(window)
    }

    /// A new screen's wallpaper, at the settings the others are running.
    pub fn new_saver(&self) -> Saver {
        Saver::new(self.clock.reduced_motion, &self.wallpaper)
    }

    pub fn fullscreen_on_screen(&self) -> bool {
        self.fullscreen_on(self.active_workspace())
    }

    /// An app asked to fill the screen, or to stop.
    pub fn set_fullscreen(&mut self, window: &Window, fullscreen: bool) {
        if fullscreen {
            // A docked window comes down from the bar to fill the screen: only a window on a
            // workspace can.
            if self.is_docked(window) {
                let beside = self.focused_window();
                self.come_down(beside.as_ref());
            }
            // A floating window fills the screen from the tiling, and floats again after.
            if let Some(index) = self.workspaces.find(window)
                && let Some(float) = self.workspaces.get_mut(index).floating.remove(window)
                && let Some(area) = self.area_for_workspace(index)
            {
                self.workspaces.insert(index, window.clone(), None, area);
                self.refloat.push((window.clone(), float));
            }
            // Only one window can fill a screen, so one already filling this workspace's is
            // taken out of it properly rather than left believing it is still fullscreen.
            if let Some(index) = self.workspaces.find(window)
                && let Some(held) = self.fullscreen_at(index)
                && held != *window
            {
                tracing::info!(
                    workspace = index + 1,
                    left = logged_app(&held),
                    "another window takes this screen"
                );
                self.set_fullscreen(&held, false);
            }
            if !self.fullscreen.contains(window) {
                self.fullscreen.push(window.clone());
            }
        } else if self.fullscreen.contains(window) {
            self.fullscreen.retain(|held| held != window);
            if let Some(at) = self.refloat.iter().position(|(held, _)| held == window) {
                let (_, float) = self.refloat.remove(at);
                if let Some(index) = self.workspaces.find(window) {
                    self.take_off_workspace(window);
                    self.workspaces.get_mut(index).floating.put(float);
                }
            }
        } else {
            return;
        }
        if let Some(surface) = window.x11_surface() {
            let _ = surface.set_fullscreen(fullscreen);
        }
        self.retile();
        tracing::info!(fullscreen, "window fullscreen changed");
    }

    /// Asks the focused window to close, as its own close button would.
    pub fn close_focused(&mut self) {
        if let Some(window) = self.focused_window() {
            self.close_window(&window);
        }
    }

    /// Asks `window` to close. It goes when its app closes it, after any "save changes?" prompt.
    pub fn close_window(&mut self, window: &Window) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        } else if let Some(surface) = window.x11_surface() {
            let _ = surface.close();
        }
    }

    /// The focused window moves one rung heavier or lighter along
    /// gravity's ladder of layouts.
    pub fn weigh(&mut self, heavier: bool) {
        if let Some(window) = self.focused_window() {
            self.weigh_window(&window, heavier);
        }
    }

    pub fn weigh_window(&mut self, window: &Window, heavier: bool) {
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let history = self.focus_history.clone();
        let ws = self.workspaces.get_mut(index);
        let mut others: Vec<Window> = ws
            .layout
            .windows()
            .into_iter()
            .filter(|other| other != window)
            .collect();
        others.sort_by_key(|other| recency(&history, other));
        let step = ws.gravity.step(window, heavier, &others);
        if matches!(step, Step::Moved(_)) {
            // Gravity takes over the sizes, so a maximise ends rather than coming back after it.
            ws.maximised = None;
        }
        self.report_step(window, step);
        self.retile();
    }

    /// Super+F: the focused tiled window fills the tiling area above its neighbours, or goes back
    /// to its tile.
    pub fn toggle_maximise(&mut self) {
        let Some(window) = self.focused_window() else {
            return;
        };
        let Some(index) = self.workspaces.find(&window) else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            self.show_toast(
                "Floating windows keep their size",
                "Super+Shift+V puts it back in the tiling, where Super+F fills the space.",
            );
            return;
        }
        // In any arrangement, the window fills the area over the others until Super+F again.
        let maximised = self.workspaces.get_mut(index).toggle_maximised(&window);
        self.retile();
        tracing::info!(window = logged_app(&window), maximised, "maximise");
    }

    /// Super+T. A quick press turns gravity on, at the arrangement last kept, or back to tiling;
    /// with Super held the arrangements appear side by side, and each further T moves along
    /// them (`arrange.rs`).
    pub fn toggle_gravity(&mut self) {
        if self.arrange.is_some() {
            self.step_arrangement(true);
            return;
        }
        let Some(window) = self.focused_window() else {
            return;
        };
        let Some(index) = self.workspaces.find(&window) else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            return;
        }
        let ws = self.workspaces.get_mut(index);
        // One window looks the same in every arrangement.
        if ws.layout.windows().len() < 2 && !ws.gravity.is_on() {
            return;
        }
        let to = if ws.gravity.is_on() {
            Rung::Tiling
        } else {
            self.arrangement
        };
        let before = (ws.gravity.clone(), ws.maximised.clone());
        self.arrange = Some(crate::arrange::Arrange::start(
            to,
            window.clone(),
            index,
            before,
            self.wall(),
            self.clock.reduced_motion,
        ));
        self.arrange_to(to);
    }

    /// The next arrangement along the strip, or the one before, put in force at once.
    pub fn step_arrangement(&mut self, forward: bool) {
        if let Some(rung) = self.arrange.as_mut().map(|arrange| arrange.step(forward)) {
            self.arrange_to(rung);
        }
    }

    /// A tile of the strip clicked: that arrangement, kept.
    pub fn pick_arrangement(&mut self, index: usize) {
        if let Some(rung) = self
            .arrange
            .as_mut()
            .and_then(|arrange| arrange.select(index))
        {
            self.arrange_to(rung);
            self.finish_arrangement();
        }
    }

    pub(super) fn arrange_to(&mut self, rung: Rung) {
        let Some((window, index)) = self
            .arrange
            .as_ref()
            .map(|arrange| (arrange.window.clone(), arrange.workspace))
        else {
            return;
        };
        if !window.alive() || self.workspaces.find(&window) != Some(index) {
            self.arrange = None;
            return;
        }
        let ws = self.workspaces.get_mut(index);
        ws.gravity.arrange(&window, rung);
        if rung != Rung::Tiling {
            // Gravity takes over the sizes, so a maximise ends rather than coming back after it.
            ws.maximised = None;
        }
        self.tag(window, rung);
        self.retile();
    }

    /// Super let go: the arrangement on the strip stays, and a quick Super+T turns gravity on at
    /// it next time.
    pub fn finish_arrangement(&mut self) {
        let Some(arrange) = self.arrange.take() else {
            return;
        };
        let rung = self
            .workspaces
            .get_mut(arrange.workspace)
            .gravity
            .rung(&arrange.window);
        if matches!(
            rung,
            Rung::Grid | Rung::Centre | Rung::Wide | Rung::Spotlight
        ) {
            self.arrangement = rung;
        }
    }

    /// Esc with Super held: the workspace goes back to how it was before Super+T.
    pub fn cancel_arrangement(&mut self) {
        let Some(arrange) = self.arrange.take() else {
            return;
        };
        let (gravity, maximised) = arrange.before;
        let ws = self.workspaces.get_mut(arrange.workspace);
        ws.gravity = gravity;
        ws.maximised = maximised;
        let rung = ws.gravity.rung(&arrange.window);
        if arrange.window.alive() {
            self.tag(arrange.window, rung);
        }
        self.retile();
    }

    /// The strip's element, once Super has been held long enough, for a screen `screen` big.
    pub fn arrange_element<R>(
        &mut self,
        renderer: &mut R,
        screen: smithay::utils::Size<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>>
    where
        R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let now = self.wall();
        let mut arrange = self.arrange.take()?;
        let element = if arrange.visible(now) {
            let pictures = self.arrangement_pictures(&arrange.window, arrange.workspace);
            let ring = self.panel_ring();
            arrange.element(renderer, screen, scale, now, ring, pictures)
        } else {
            None
        };
        self.arrange = Some(arrange);
        element
    }

    /// Each arrangement on the strip as it would place this workspace's windows, `window` marked.
    pub(super) fn arrangement_pictures(
        &mut self,
        window: &Window,
        index: usize,
    ) -> Vec<Vec<crate::arrange::Pane>> {
        let Some(area) = self.area_for_workspace(index) else {
            return Vec::new();
        };
        let ws = self.workspaces.get_mut(index);
        let windows = ws.layout.windows();
        let (outer, inner) = (ws.layout.outer_gap, ws.layout.inner_gap);
        crate::arrange::CHOICES
            .iter()
            .map(|rung| {
                let rects = if *rung == Rung::Tiling {
                    ws.layout.rects_within(area, &min_size)
                } else {
                    let mut gravity = ws.gravity.clone();
                    gravity.arrange(window, *rung);
                    gravity.rects(&windows, area, outer, inner)
                };
                crate::arrange::picture(&rects, area, Some(window))
            })
            .collect()
    }

    /// Tags the window with its new rung. The tag is the whole answer: a step past either end of
    /// the ladder, or with nothing else to arrange, changes nothing and says nothing.
    pub(super) fn report_step(&mut self, window: &Window, step: Step) {
        if let Step::Moved(rung) = step {
            self.tag(window.clone(), rung);
        }
    }

    /// Shows `window`'s new rung on it for a moment, and logs the change.
    pub(super) fn tag(&mut self, window: Window, rung: Rung) {
        tracing::info!(window = logged_app(&window), rung = rung.label(), "gravity");
        let now = self.clock.tick();
        self.tags.retain(|(tagged, ..)| *tagged != window);
        self.tags.push((window, rung, now));
    }

    /// Takes `window` off its workspace (closed, minimised or moved) and returns which workspace
    /// it was on. A window that inherits gravity's centre gets the centre tag, so its move into
    /// the middle has a visible cause.
    pub(crate) fn take_off_workspace(&mut self, window: &Window) -> Option<usize> {
        let history = &self.focus_history;
        let removed = self.workspaces.remove(window, |w| recency(history, w))?;
        if let Some(centre) = removed.new_centre {
            let rung = self.workspaces.get(removed.workspace).gravity.rung(&centre);
            self.tag(centre, rung);
        }
        Some(removed.workspace)
    }

    /// What to call a window in messages: its app's name, else its title.
    /// What a stream's header calls a window. The app's name, or failing that its app id — never
    /// its title: a title is a working directory, a document or a correspondent, and in a column
    /// 36 pixels wide it arrives sideways and cut in half anyway.
    pub fn stream_name(&self, window: &Window) -> String {
        window_app_id(window)
            .map(|id| {
                self.explorer.app_name(&id).unwrap_or_else(|| {
                    // `org.kde.kate` reads better as Kate than as its reverse-DNS name.
                    let leaf = id.rsplit('.').next().unwrap_or(&id).to_string();
                    let mut letters = leaf.chars();
                    match letters.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
                        None => leaf,
                    }
                })
            })
            .unwrap_or_else(|| "This window".to_string())
    }

    pub fn window_name(&self, window: &Window) -> String {
        window_app_id(window)
            .and_then(|id| self.explorer.app_name(&id))
            .or_else(|| Some(window_title(window)).filter(|title| !title.is_empty()))
            .unwrap_or_else(|| "This window".to_string())
    }
}

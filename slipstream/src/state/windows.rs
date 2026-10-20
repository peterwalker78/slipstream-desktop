//! Windows: tiling them, mapping new ones, moving and resizing tiles, maximise, fullscreen and
//! closing. Where the keyboard goes is in `keyboard_focus.rs`, gravity in `arrangements.rs`.

use super::*;

/// A floating window where it goes, and the size the keys asked of it, if any.
type PlacedFloat = (Window, Rect, Option<(i32, i32)>);

/// Whether `window` is being moved or resized by the pointer, so it follows it without gliding.
fn dragged_by_pointer(drag: &Option<crate::grabs::Drag>, window: &Window) -> bool {
    matches!(
        drag,
        Some(crate::grabs::Drag::Move { window: dragged, .. } | crate::grabs::Drag::Resize { window: dragged, .. })
            if dragged == window
    )
}

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
            let r = full_rect.unwrap_or(tile);
            configure(&window, r, full_rect.is_some());
            // The space holds the real position for input; the drawing glides there, except
            // while a gap is dragged, when the tiles keep up with the pointer.
            self.motion.place(&window, r, now);
            if matches!(self.drag, Some(crate::grabs::Drag::Gap { .. })) {
                self.motion.jump(&window, r);
            }
            self.put_in_space(&window, r);
        }
        self.floating_sizes = floats
            .iter()
            .map(|(window, ..)| (window.clone(), crate::floating::own_size(window)))
            .collect();
        for (window, r, asked) in floats {
            crate::floating::configure_floating(&window, asked, r);
            self.motion.place(&window, r, now);
            // Under the pointer it keeps up with the pointer.
            if dragged_by_pointer(&self.drag, &window) {
                self.motion.jump(&window, r);
            }
            self.put_in_space(&window, r);
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

    /// Puts `window` at `r`'s corner in the space. One already mapped is moved, which keeps its
    /// place in the stacking order; mapping it again would put the tiles in tree order, and the
    /// focused window's menus could end up under a neighbour.
    pub(super) fn put_in_space(&mut self, window: &Window, r: Rect) {
        if self.space.element_location(window).is_some() {
            self.space.relocate_element(window, (r.x, r.y));
        } else {
            self.space.map_element(window.clone(), (r.x, r.y), false);
        }
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
            configure(&window, r, full);
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
        if fullscreen_asked {
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
        // Exception: a window from an explicit launch or file-manager open (xdg-activation token
        // with a serial) always tiles — whether it gets keyboard focus is decided below.
        let now = std::time::Instant::now();
        let explicitly_asked =
            self.concentration.explicitly_asked(now) || self.fullscreen_on_map.contains(&window);
        if !self.fullscreen_on_screen()
            && !explicitly_asked
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

    /// A new window nobody asked for goes straight into a stream of code rain, and a toast names
    /// it and the way to bring it in.
    pub(super) fn pour_new(&mut self, window: Window) {
        let Some(scale) = self.output_scale() else {
            return;
        };
        let name = self.add_stream(&window, scale);
        self.retile();
        let restore = self.key_label(crate::keys::Action::Restore);
        self.show_toast(
            "Opened in a stream",
            &format!("{name}: {restore} or a click on its stream brings it in."),
        );
        tracing::info!(
            window = logged_app(&window),
            "new window poured into code rain"
        );
    }

    /// The client that closed its last window is now running invisibly in the background; record
    /// it for the notification centre. Only apps from the apps list count: a portal's file
    /// chooser or a password prompt closing leaves a system service running, not an app. Called
    /// after `remove_window` so the closing window is already gone from `all_open_windows`.
    pub(crate) fn track_closed_app(&mut self, wl_surface: &WlSurface) {
        let Some(client) = wl_surface.client() else {
            return;
        };
        let client_id = client.id();
        let Some(app_id) = toplevel_data(wl_surface, |data| data.app_id.clone()) else {
            return;
        };
        let Some(name) = self.explorer.app_name(&app_id) else {
            return;
        };
        let still_open = self.all_open_windows().into_iter().any(|w| {
            w.toplevel()
                .and_then(|t| t.wl_surface().client())
                .is_some_and(|c| c.id() == client_id)
        });
        if !still_open
            && !self
                .background_apps
                .iter()
                .any(|a| a.client_id == client_id)
        {
            self.background_apps.push(BackgroundApp {
                name,
                app_id,
                client_id,
                icon: None,
            });
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
        self.drawn.overview_hits.retain(|(hit, _)| hit != window);
        self.drawn.fitted.retain(|(hit, _)| hit != window);
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

    /// Super+R and Super+Shift+R: the workspace's whole layout turns a quarter, clockwise or
    /// anticlockwise, every tile going with it. Which window has the keyboard doesn't matter.
    pub fn rotate_layout(&mut self, clockwise: bool) {
        let Some(window) = self.focused_window() else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            let float = self.key_label(crate::keys::Action::ToggleFloating);
            self.show_toast(
                "This window floats",
                &format!("{float} puts it back in the tiling."),
            );
            return;
        }
        let ws = self.current_workspace();
        if ws.gravity.is_on() {
            let gravity = self.key_label(crate::keys::Action::ToggleGravity);
            self.show_toast(
                "Only tiling turns",
                &format!("Gravity has no rows or columns to turn. {gravity} goes back to tiling."),
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

    /// Super+[ ] and Super+Shift+[ ]: the split beside the focused tile moves a step, on the
    /// press, and the tiles glide to their new sizes.
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

    /// The window filling `output`, if one is.
    pub fn fullscreen_on_output(&self, output: &Output) -> Option<Window> {
        self.screens
            .index_of(output)
            .and_then(|index| self.screens.get(index))
            .and_then(|screen| self.fullscreen_at(screen.workspace))
    }

    /// Whether `window` is filling its screen.
    pub fn is_fullscreen(&self, window: &Window) -> bool {
        self.fullscreen.contains(window)
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

    /// Asks every window of the focused window's app to close, as quitting it would. A window that
    /// names no app is closed on its own.
    pub fn quit_focused_app(&mut self) {
        let Some(focused) = self.focused_window() else {
            return;
        };
        let windows = match window_app_id(&focused) {
            Some(app) => self
                .all_open_windows()
                .into_iter()
                .filter(|window| window_app_id(window).as_deref() == Some(app.as_str()))
                .collect(),
            None => vec![focused],
        };
        for window in windows {
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
            let float = self.key_label(crate::keys::Action::ToggleFloating);
            let maximise = self.key_label(crate::keys::Action::Maximise);
            self.show_toast(
                "Floating windows keep their size",
                &format!("{float} puts it back in the tiling, where {maximise} fills the space."),
            );
            return;
        }
        // In any arrangement, the window fills the area over the others until Super+F again.
        let maximised = self.workspaces.get_mut(index).toggle_maximised(&window);
        self.retile();
        tracing::info!(window = logged_app(&window), maximised, "maximise");
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

    /// What a stream's header calls a window. The app's name, or failing that its app id — never
    /// its title: a title is a working directory, a document or a correspondent, and in a column
    /// 36 pixels wide it arrives sideways and cut in half anyway.
    pub fn stream_name(&self, window: &Window) -> String {
        window_app_id(window)
            .map(|id| {
                self.explorer
                    .app_name(&id)
                    .unwrap_or_else(|| pretty_name(&id))
            })
            .unwrap_or_else(|| "This window".to_string())
    }

    /// What to call a window in messages: its app's name, else its title.
    pub fn window_name(&self, window: &Window) -> String {
        window_app_id(window)
            .and_then(|id| self.explorer.app_name(&id))
            .or_else(|| Some(window_title(window)).filter(|title| !title.is_empty()))
            .unwrap_or_else(|| "This window".to_string())
    }
}

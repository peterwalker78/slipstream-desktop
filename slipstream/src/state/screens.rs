//! Screens: connecting and losing them, the card that asks about a new one, the lid, and what
//! each screen is remembered as.

use super::*;

impl Slipstream {
    /// A screen lit up: it joins the row, left to right, and shows a workspace of its own rather
    /// than taking one of the user's. Called by both backends when an output is mapped.
    pub fn screen_connected(&mut self, output: &Output, x: i32) {
        let new_screen = self.screens.index_of(output).is_none();
        // A screen never shares a workspace, so there is always one more than the lit screens.
        if new_screen && self.workspaces.count() < self.screens.len() + 1 {
            let entries = workspace_entries(&self.settings);
            self.set_workspaces(&entries, self.screens.len() + 1);
        }
        // What this screen should be looking at. A screen met before takes back what it had; one
        // never seen here gets a workspace of its own, so plugging a monitor in never takes a
        // workspace off the screen already in use.
        let mut wanted = None;
        let mut ask = None;
        // Only once something else is lit. The first screen of a session has no one to take a
        // workspace from, so it starts on the first workspace as it always has, and there is
        // nothing worth asking about.
        if new_screen && !self.screens.is_empty() {
            let monitor = self.monitor_id(output);
            match self.known.get(&monitor) {
                Some(known) if known.own_workspace => wanted = Some(self.workspaces.add_scratch()),
                Some(known) => {
                    wanted = known
                        .workspace_id
                        .and_then(|id| self.workspaces.index_of_id(id))
                }
                None => {
                    wanted = Some(self.workspaces.add_scratch());
                    ask = Some(monitor);
                }
            }
        }
        // Placed at the right edge of what's lit for now; `arrange_screens` just below puts it
        // where its setting says, which is the only place it ever really sits.
        let index = self
            .screens
            .add(output.clone(), x, 0, self.workspaces.count(), wanted);
        if !self.screen_order.contains(&output.name()) {
            self.screen_order.push(output.name());
        }
        self.arrange_screens();
        let index = self.screens.index_of(output).unwrap_or(index);
        let workspace = self
            .screens
            .get(index)
            .map(|screen| screen.workspace)
            .unwrap_or(0);
        // A screen arrives already looking at its workspace; there is nothing to slide from. One
        // that carried this screen's workspace on while it was gone may have gone back to its
        // own, and is put there too.
        for screen in self.screens.iter() {
            self.motion
                .jump_camera(&screen.output.name(), screen.workspace);
        }
        self.hold_the_lid();
        self.retile();
        if self.focused_window().is_none() {
            self.restore_focus();
        }
        if new_screen {
            self.remember_screen(output, workspace);
        }
        if let Some(monitor) = ask {
            self.ask_about_screen(output, &monitor, workspace);
        }
        tracing::info!(
            screen = output.name(),
            workspace = workspace + 1,
            own_workspace = self.workspaces.is_scratch(workspace),
            screens = self.screens.len(),
            "a screen joined the desktop"
        );
    }

    /// A screen nobody has seen on this machine before: it has been given a workspace of its own,
    /// and the card says so and offers the other answer. Asked once per screen, ever — the answer
    /// is written down against the screen, and a screen met before never asks again.
    pub(super) fn ask_about_screen(&mut self, output: &Output, monitor: &str, workspace: usize) {
        // What it would have taken under the old rule: the lowest-numbered workspace from the
        // settings' list that no screen is showing. With none free there is nothing to offer.
        let share = (0..self.workspaces.count())
            .find(|index| {
                !self.workspaces.is_scratch(*index)
                    && !self.screens.iter().any(|screen| screen.workspace == *index)
            })
            .map(|index| (index, self.workspace_prose(index)));
        let now = self.wall();
        self.connect = Some(Connect::new(
            monitor.to_string(),
            output.name(),
            workspace,
            self.workspace_prose(workspace),
            share,
            now,
            self.settings.motion.reduced,
        ));
        tracing::info!(
            screen = output.name(),
            monitor,
            workspace = workspace + 1,
            "asking what a screen never seen here should show"
        );
    }

    /// What a workspace is called in a sentence: its name, or "workspace 6". The bar's bare
    /// number reads as a number there and as nothing in prose.
    pub(super) fn workspace_prose(&self, index: usize) -> String {
        let label = self.workspaces.label(index);
        match label.chars().all(|c| c.is_ascii_digit()) {
            true => format!("workspace {label}"),
            false => label,
        }
    }

    pub fn connect_key(&mut self, sym: Keysym) {
        let now = self.wall();
        let Some(act) = self.connect.as_ref().map(|card| card.key(sym, now)) else {
            return;
        };
        self.act_on_connect(act);
    }

    pub fn connect_click(&mut self, x: f64, y: f64) {
        let Some(act) = self.connect.as_ref().map(|card| card.click(x, y)) else {
            return;
        };
        self.act_on_connect(act);
    }

    pub fn connect_hover(&mut self, pos: Point<f64, Logical>) {
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(card) = self.connect.as_mut()
            && card.hover(pos.x, pos.y)
        {
            self.note_layout_change();
        }
    }

    pub(super) fn act_on_connect(&mut self, act: connect::Act) {
        let Some(card) = self.connect.as_ref() else {
            return;
        };
        let (monitor, connector) = (card.monitor.clone(), card.connector.clone());
        match act {
            connect::Act::Nothing => return,
            connect::Act::Own => {
                tracing::info!(monitor, "a workspace of its own");
            }
            connect::Act::Share => {
                let Some(share) = card.share else {
                    return;
                };
                let Some(index) = self
                    .screens
                    .iter()
                    .position(|screen| screen.output.name() == connector)
                else {
                    return;
                };
                // Show it there, then drop the workspace it was given: it is empty, nothing is
                // looking at it any more, and it sits at the end, so no other workspace moves.
                // The keyboard stays where it was: the card was answered on the screen in use.
                let keyboard = self.screens.focused_index();
                self.screens.focus(index);
                self.screens.show(share, self.workspaces.count());
                self.screens.focus(keyboard);
                self.prune_scratch();
                for screen in self.screens.iter() {
                    self.motion
                        .jump_camera(&screen.output.name(), screen.workspace);
                }
                self.retile();
                self.note_layout_change();
                tracing::info!(
                    monitor,
                    workspace = share + 1,
                    "showing a workspace instead"
                );
            }
        }
        // Whichever way it was answered, this screen never asks again.
        let screen = self
            .screens
            .iter()
            .find(|screen| screen.output.name() == connector)
            .map(|screen| (screen.output.clone(), screen.workspace));
        self.connect = None;
        if let Some((output, workspace)) = screen {
            self.remember_screen(&output, workspace);
        }
        self.note_layout_change();
    }

    /// Drops empty workspaces a screen was given but nothing is looking at any more. Only ones at
    /// the end go, so no surviving workspace changes its number and nothing that remembers one by
    /// index has to be put right. Returns how many went.
    pub(super) fn prune_scratch(&mut self) -> usize {
        let shown: Vec<usize> = self.screens.iter().map(|screen| screen.workspace).collect();
        let floor = workspace_entries(&self.settings)
            .len()
            .max(self.screens.len() + 1);
        let dropped = self.workspaces.prune_scratch(&shown, floor);
        if dropped > 0 {
            let count = self.workspaces.count();
            self.screens.trim_homes(count);
        }
        dropped
    }

    /// The output a client's `wl_output` stands for, if it's one of ours and still lit.
    pub fn output_named(
        &self,
        resource: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    ) -> Option<Output> {
        let wanted = Output::from_resource(resource)?;
        self.space
            .outputs()
            .find(|output| **output == wanted)
            .cloned()
    }

    /// Moves `window` to the workspace `output` is showing, leaving the keyboard where it is.
    /// Nothing happens if it is already there, or if that screen isn't lit.
    pub fn send_window_to_screen(&mut self, window: &Window, output: &Output) {
        let Some(target) = self
            .screens
            .iter()
            .find(|screen| screen.output == *output)
            .map(|screen| screen.workspace)
        else {
            return;
        };
        if self.workspaces.find(window) == Some(target) {
            return;
        }
        self.move_window_to_workspace(window, target, false);
    }

    /// What a screen is known by between sessions: what its EDID says, else its connector's name.
    pub fn monitor_id(&self, output: &Output) -> String {
        let physical = output.physical_properties();
        known::identity(
            &physical.make,
            &physical.model,
            &physical.serial_number,
            &output.name(),
        )
    }

    /// Writes down what `output` is showing, so it comes back to the same place next session. A
    /// scratch workspace is remembered as "one of its own" rather than by id: the id is this
    /// session's and means nothing next time.
    pub fn remember_screen(&mut self, output: &Output, workspace: usize) {
        let monitor = self.monitor_id(output);
        let shows =
            (!self.workspaces.is_scratch(workspace)).then(|| self.workspaces.get(workspace).id);
        let before = self.known.get(&monitor).cloned();
        self.known.set(&monitor, &output.name(), shows);
        // How big it is and that it's plugged in, which is what the Settings app offers an
        // arrangement for.
        if let Some(size) = self.space.output_geometry(output).map(|geo| geo.size) {
            let label = known::short_name(&output.physical_properties().model, &output.name());
            self.known
                .set_lit(&monitor, &output.name(), &label, size.w, size.h);
        }
        // What it can do and what it is doing. Only the compositor can read a connector's modes,
        // and the Settings app has to offer them, so they go through the file with the rest.
        let running = output
            .current_mode()
            .map(|mode| crate::screen::mode_label(&mode))
            .unwrap_or_default();
        let scale = output.current_scale().fractional_scale();
        self.known
            .set_modes(&monitor, self.screen_modes(output), &running, scale);
        if before.as_ref() == self.known.get(&monitor) {
            return;
        }
        if let Err(err) = known::write(&known::path(), &self.known) {
            tracing::warn!("couldn't write down what the screens show: {err}");
        }
    }

    /// A screen went out: unplugged, or the lid shut on it.
    ///
    /// Nothing moves between workspaces. If the keyboard was on that screen, whichever screen
    /// takes over shows what the lost one was showing, so shutting the lid carries on what you
    /// were doing rather than leaving it behind on a panel nobody can see. Both go back when the
    /// screen returns.
    pub fn screen_disconnected(&mut self, output: &Output) {
        let was_focused = self.screens.focused_output().as_ref() == Some(output);
        // Captures waiting on it will never be filled; the programs asking are told now.
        self.fail_screencopy_on(output);
        let Some(lost) = self.screens.remove(output) else {
            return;
        };
        self.screen_order.retain(|name| *name != output.name());
        // Layer surfaces on it have nowhere to be drawn: they're asked to close.
        {
            let mut map = smithay::desktop::layer_map_for_output(output);
            for layer in map.layers().cloned().collect::<Vec<_>>() {
                layer.layer_surface().send_close();
                map.unmap_layer(&layer);
            }
        }
        self.arrange_screens();
        self.motion.forget_camera(&output.name());
        self.forget_screen_drawing(&output.name());
        let mut carried = false;
        if was_focused && !self.screens.is_empty() {
            let here = self.focused_screen_name();
            carried = self.screens.carry(lost);
            if carried {
                self.motion.jump_camera(&here, lost);
            }
        }
        // The Settings app offers an arrangement for the screens that are plugged in, so it has
        // to be told when one goes.
        let monitor = self.monitor_id(output);
        self.known.set_dark(&monitor);
        if let Err(err) = known::write(&known::path(), &self.known) {
            tracing::warn!("couldn't write down what the screens show: {err}");
        }
        // A card asking about this screen has nothing left to ask about.
        if self
            .connect
            .as_ref()
            .is_some_and(|card| card.connector == output.name())
        {
            self.connect = None;
        }
        // Its own workspace goes with it, unless something is still open there: an empty one is
        // clutter in bullet time and on the bar, and a full one is work nobody asked to move.
        let dropped = self.prune_scratch();
        self.hold_the_lid();
        self.retile();
        self.restore_focus();
        tracing::info!(
            screen = output.name(),
            workspace = lost + 1,
            screens = self.screens.len(),
            carried,
            dropped,
            "a screen left the desktop"
        );
        if carried {
            let name = self.focused_screen_name();
            let label = self.workspaces.label(lost);
            let now = self.clock.tick();
            self.toast.show(
                "One screen left",
                &format!("{} carried on to {name}.", workspace_called(&label)),
                now,
            );
        }
    }

    /// A made-up screen for headless checks: a real `Output` with a mode and a place in the
    /// space, mapped to the right of the others, but no backend drawing it. Everything that
    /// decides where windows go — areas, workspaces, focus, the lid — works on it exactly as it
    /// does on a screen with a picture, which is what these checks are for.
    pub fn add_made_up_screen(&mut self, w: i32, h: i32) {
        let name = format!("HEADLESS-{}", self.screens.len() + 1);
        let output = Output::new(
            name.clone(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "Slipstream".into(),
                model: "Made up".into(),
                serial_number: "Unknown".into(),
            },
        );
        let mode = smithay::output::Mode {
            size: (w, h).into(),
            refresh: 60_000,
        };
        let x = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .map(|geo| geo.loc.x + geo.size.w)
            .max()
            .unwrap_or(0);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(smithay::utils::Transform::Normal),
            None,
            Some((x, 0).into()),
        );
        output.create_global::<Slipstream>(&self.display_handle);
        self.space.map_output(&output, (x, 0));
        self.screen_connected(&output, x);
        tracing::info!(
            screen = name,
            w,
            h,
            x,
            "a made-up screen for a headless check"
        );
    }

    /// Takes a made-up screen away, the way unplugging one does.
    pub fn drop_made_up_screen(&mut self, name: &str) {
        let Some(output) = self
            .space
            .outputs()
            .find(|output| output.name() == name)
            .cloned()
        else {
            tracing::warn!(screen = name, "no screen by that name");
            return;
        };
        self.space.unmap_output(&output);
        self.screen_disconnected(&output);
    }

    /// Writes the screens into the log: what each is showing, where it is, and where windows
    /// tile on it. Checks read this back.
    pub fn log_screens(&self) {
        tracing::info!(
            workspaces = self.workspaces.count(),
            scratch = (0..self.workspaces.count())
                .filter(|index| self.workspaces.is_scratch(*index))
                .count(),
            fullscreen = self.fullscreen.len(),
            "workspaces"
        );
        for (index, screen) in self.screens.iter().enumerate() {
            let area = self.screen_area(index);
            tracing::info!(
                screen = screen.output.name(),
                workspace = screen.workspace + 1,
                own = self.workspaces.is_scratch(screen.workspace),
                fullscreen = self
                    .fullscreen_at(screen.workspace)
                    .map(|window| logged_app(&window))
                    .unwrap_or_default(),
                focused = index == self.screens.focused_index(),
                rect = ?self.screen_rect(index),
                area = ?area,
                windows = self.workspaces.get(screen.workspace).layout.len(),
                "screen"
            );
        }
    }

    pub fn log_windows(&self) {
        for window in self.space.elements() {
            let told = window.toplevel().map(|toplevel| {
                (
                    toplevel.with_pending_state(|state| state.size),
                    toplevel.with_committed_state(|state| state.and_then(|state| state.size)),
                )
            });
            tracing::info!(
                window = logged_app(window),
                placed = ?self.space.element_location(window),
                own = ?window.geometry(),
                told = ?told.map(|(pending, _)| pending),
                acked = ?told.map(|(_, acked)| acked),
                "window"
            );
        }
    }

    /// logind's lid switch is ours while there is a screen that isn't the laptop's own panel.
    /// Nested, the window isn't a screen and the lid belongs to the desktop around it.
    pub(super) fn hold_the_lid(&mut self) {
        if self.nested {
            return;
        }
        let external = self
            .screens
            .iter()
            .any(|screen| !is_internal_panel(&screen.output.name()));
        self.lid_inhibitor.set(external);
    }

    /// Super+P: the keyboard moves to the next screen along, wrapping round at the end. With one
    /// screen it says so rather than doing nothing silently.
    pub fn focus_next_screen(&mut self) {
        if self.screens.len() < 2 {
            let now = self.clock.tick();
            self.toast
                .show("One screen", "Nothing to move the keyboard to.", now);
            return;
        }
        let next = (self.screens.focused_index() + 1) % self.screens.len();
        self.focus_screen_at(next);
    }

    /// Super+Shift+P: the focused window moves to the next screen — that is, on to the workspace
    /// that screen is showing — as Win+Shift+arrow does on Windows.
    pub fn move_focused_to_next_screen(&mut self) {
        if self.screens.len() < 2 {
            let now = self.clock.tick();
            self.toast
                .show("One screen", "Nothing to move the window to.", now);
            return;
        }
        let Some(window) = self.focused_window() else {
            return;
        };
        let next = (self.screens.focused_index() + 1) % self.screens.len();
        let Some(target) = self.screens.get(next).map(|screen| screen.workspace) else {
            return;
        };
        // The keyboard stays put. Throwing a window at the screen beside you is a thing you do
        // while working on this one, and Super+P is one key away if you want to follow it.
        self.move_window_to_workspace(&window, target, false);
        self.restore_focus();
        let now = self.clock.tick();
        let label = self.workspaces.label(target);
        let screen = self
            .screens
            .get(next)
            .map(|screen| screen.output.name())
            .unwrap_or_default();
        self.toast
            .show("Window sent", &format!("{screen} · {label}"), now);
    }

    /// Puts every lit screen into the mode and scale the settings ask for. A screen with nothing
    /// said about it keeps the mode it chose for itself and the scale worked out from its size.
    pub fn follow_screen_settings(&mut self) {
        let screens: Vec<Output> = self
            .screens
            .iter()
            .map(|screen| screen.output.clone())
            .collect();
        for output in screens {
            let monitor = self.monitor_id(&output);
            let place = self.settings.display.place(&monitor);
            let (mode, scale) = (place.mode, place.scale);
            if !mode.is_empty() {
                self.set_screen_mode(&output, &mode);
            }
            self.set_screen_scale(&output, scale);
        }
    }

    /// Scales a screen by `scale`, or works it out from the screen itself where it is zero.
    ///
    /// Everything that follows from a screen's size follows from here: the windows are told their
    /// new scale so their text is drawn sharply, and anything recording the screen is told, since
    /// a capture sized for the old scale would fail every frame after this.
    pub fn set_screen_scale(&mut self, output: &Output, scale: f64) {
        let want = match scale {
            0.0 => self.automatic_scale(output),
            given => given.clamp(
                slipstream_config::SCALE_RANGE.0,
                slipstream_config::SCALE_RANGE.1,
            ),
        };
        if (output.current_scale().fractional_scale() - want).abs() < 0.001 {
            return;
        }
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(want)),
            None,
        );
        tracing::info!(screen = output.name(), scale = want, "scaled a screen");
        self.update_window_scales();
        self.capture_size_changed(output);
    }
}

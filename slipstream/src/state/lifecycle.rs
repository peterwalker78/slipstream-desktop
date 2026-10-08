//! The session's comings and goings: the way out, recording the layout and putting it back at
//! login, the screen-sharing picker, and asking logind to sleep or shut down.

use super::*;

impl Slipstream {
    /// Every window a person would call open: on any workspace, docked to the bar, or minimised
    /// into the code rain. Not popups, not override-redirect X11 surfaces, not Slipstream's own overlays. The
    /// one definition of what's open, used by the way out to list, to ask, and to wait.
    pub fn mapped_windows(&self) -> Vec<exit::Open<Window>> {
        self.workspaces
            .all_windows()
            .into_iter()
            .map(|window| (window, false))
            .chain(
                self.dock
                    .iter()
                    .map(|docked| (docked.window.clone(), false)),
            )
            .chain(
                self.rain
                    .streams
                    .iter()
                    .map(|stream| (stream.window.clone(), true)),
            )
            .filter(|(window, _)| window.alive())
            .map(|(window, minimised)| exit::Open {
                app: self.window_name(&window),
                title: window_title(&window),
                minimised,
                window,
            })
            .collect()
    }

    /// Ctrl+Alt+Del, or Log out, Restart or Shut down: the way out asks first, then gives the
    /// apps a moment to close themselves. Only the watchdog's emergency quit skips it.
    /// Ctrl+Alt+Del: the way out, on its chooser. Pressed again while it is up, it closes, as
    /// every other panel's own key does.
    pub fn open_way_out(&mut self) {
        if self.exit.is_some() {
            self.cancel_exit();
            return;
        }
        self.close_panels();
        let open = self.mapped_windows();
        tracing::info!(windows = open.len(), "the way out: asking which");
        let wall = self.wall();
        self.exit_record = Some(self.take_record());
        self.exit = Some(Exit::choosing(
            open,
            wall,
            self.clock.reduced_motion,
            self.settings.session.remember,
        ));
    }

    pub fn begin_exit(&mut self, intent: Intent) {
        if self.exit.is_some() {
            return;
        }
        self.close_panels();
        let open = self.mapped_windows();
        tracing::info!(intent = intent.verb(), windows = open.len(), "the way out");
        let wall = self.wall();
        self.exit_record = Some(self.take_record());
        self.exit = Some(Exit::new(
            intent,
            open,
            wall,
            self.clock.reduced_motion,
            self.settings.session.remember,
        ));
    }

    /// Something moved. The layout goes to disk once the desktop has been still for
    /// `RECORD_SETTLE`, so every retile can say so without costing anything.
    pub(super) fn note_layout_change(&mut self) {
        if self.settings.session.remember {
            self.record_due = Some(self.wall() + RECORD_SETTLE);
        }
    }

    /// A pass of the event loop: the layout is written down once the desktop has settled, so the
    /// watchdog's emergency exit, a wedged GPU or a power cut still leave something to come
    /// back to, which is when a person most wants their desktop back. Wall time, so bullet time
    /// can't hold the save off.
    pub fn tick_record(&mut self) {
        let Some(due) = self.record_due else {
            return;
        };
        // The record belongs to the compositor that owns the state folder.
        if !self.owns_state {
            self.record_due = None;
            return;
        }
        if self.wall() < due {
            return;
        }
        // Until the offer has been answered and the windows have landed, what's on screen is a
        // layout half-built from the file itself, and no record of what the person was doing.
        if !self.restore_looked || self.offer.is_some() || self.restoring.is_some() {
            return;
        }
        // The way out has its own record, taken whole when its card went up. Saving again while
        // the apps close one by one would write a half-empty desktop over a good one.
        if self.exit.is_some() {
            return;
        }
        self.record_due = None;
        if !self.settings.session.remember {
            return;
        }
        let record = self.take_record();
        // An empty desktop is never written over a real record: it says nothing worth reopening,
        // and it is what "start clean" leaves on screen until the first window opens.
        if record.windows.is_empty() {
            return;
        }
        if self
            .saved_record
            .as_ref()
            .is_some_and(|saved| saved.same_desktop(&record))
        {
            return;
        }
        let path = session::path();
        match session::write(&path, &record) {
            Ok(()) => {
                tracing::debug!(windows = record.windows.len(), "layout written down");
                self.saved_record = Some(record);
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't write the layout down: {err}");
            }
        }
    }

    /// The desktop as it stands: which app is on which workspace, where it sits in the tiling, what
    /// gravity was doing with it, and what is in the code rain. Apps and places only — no window
    /// titles are ever written down.
    pub(super) fn take_record(&self) -> session::Record {
        let mut windows = Vec::new();
        let mut workspaces = Vec::new();
        let focused = self.focused_window();
        for index in 0..self.workspaces.count() {
            let workspace = self.workspaces.get(index);
            let here = workspace.layout.windows();
            let slot_of = |window: &Window| here.iter().position(|w| w == window);
            for (slot, window) in here.iter().enumerate() {
                let Some(app) = window_app_id(window) else {
                    continue;
                };
                windows.push(session::Win {
                    app,
                    // Counting from 1 in the file; the name goes with the tree below.
                    workspace: Some(index + 1),
                    slot,
                    focused: focused.as_ref() == Some(window),
                    in_rain: false,
                    gravity: workspace
                        .gravity
                        .is_on()
                        .then(|| workspace.gravity.rung(window).label().to_string()),
                    pinned: workspace.gravity.is_pinned(window),
                });
            }
            if let Some(shape) = workspace.layout.shape() {
                // Slot numbers, so the tree means something without the windows themselves.
                let shape = shape.map(&slot_of);
                if let Some(shape) = shape {
                    workspaces.push(session::Tiling {
                        index: index + 1,
                        name: workspace.name.clone(),
                        tiles: session::encode(&shape),
                    });
                }
            }
        }
        for (slot, stream) in self.rain.streams.iter().enumerate() {
            if let Some(app) = window_app_id(&stream.window) {
                windows.push(session::Win {
                    app,
                    workspace: None,
                    slot,
                    in_rain: true,
                    ..session::Win::default()
                });
            }
        }
        // A docked window has no place in any workspace's tree: it is kept as one more in the
        // rain, so its app is opened again and is a click from being brought back.
        if let Some(app) = self
            .dock
            .as_ref()
            .and_then(|docked| window_app_id(&docked.window))
        {
            windows.push(session::Win {
                app,
                workspace: None,
                slot: self.rain.streams.len(),
                in_rain: true,
                ..session::Win::default()
            });
        }
        session::Record::new(self.active_workspace() + 1, windows, workspaces)
    }

    /// Looks for a recorded layout once the desktop entries have been read, and offers it — or
    /// puts it straight back, with `reopen-without-asking` on. Runs on passes of the event loop
    /// until it can answer, since the apps are read on another thread a moment after startup.
    pub(super) fn look_for_a_recorded_layout(&mut self) {
        // Nothing was recorded, so there is nothing to put back and nothing sitting on disk. And a
        // compositor that doesn't own the state folder leaves its record to the one that does.
        if !self.settings.session.remember || !self.owns_state {
            self.restore_looked = true;
            return;
        }
        // Without the entries every app in the record would look lost. Give them a moment; if
        // they never arrive, say so rather than throwing the layout away silently.
        if !self.explorer.apps_read() {
            if self.wall() > CATALOGUE_WAIT {
                self.restore_looked = true;
                tracing::warn!("the apps weren't read in time; the recorded layout is left alone");
            }
            return;
        }
        self.restore_looked = true;
        let path = session::path();
        let record = match session::read(&path) {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't read the layout: {err}");
                return;
            }
        };
        let plan = self.plan_from(&record);
        if plan.is_empty() {
            tracing::info!(
                lost = plan.lost().len(),
                "nothing in the record can be reopened"
            );
            return;
        }
        tracing::info!(
            windows = plan.len(),
            lost = plan.lost().len(),
            saved_at = record.saved_at,
            "a recorded layout is waiting"
        );
        if self.settings.session.reopen_without_asking {
            self.start_restore(plan);
            return;
        }
        let wall = self.wall();
        self.offer = Some(Offer::new(
            plan.len(),
            plan.app_names(),
            plan.lost().to_vec(),
            wall,
            self.clock.reduced_motion,
        ));
        // Held until it's answered, so the plan's expiry starts when the apps are launched.
        self.restoring = Some(plan);
    }

    /// A record as a plan, with each app matched to its desktop entry. An app with no entry is
    /// left out and named on the card; its id is never run as a command, because any client can
    /// set any `app_id`.
    pub(super) fn plan_from(&self, record: &session::Record) -> restore::Plan<Window> {
        let wall = self.wall();
        let count = self.workspaces.count();
        restore::Plan::new(
            record,
            wall,
            |id| {
                self.explorer
                    .app_for(id)
                    .filter(|app| !app.exec.is_empty())
                    .map(|app| (app.name, app.exec, app.terminal))
            },
            count,
            |index| {
                let name = record
                    .workspaces
                    .iter()
                    .find(|tiling| tiling.index == index)
                    .map(|tiling| tiling.name.as_str())
                    .unwrap_or("");
                restore::workspace_now(name, index, count, |name| self.workspaces.named(name))
            },
        )
    }

    /// Enter on the offer card, or `reopen-without-asking`: launch every recorded app and let the
    /// windows claim their places as they arrive.
    pub(super) fn start_restore(&mut self, mut plan: restore::Plan<Window>) {
        let wall = self.wall();
        plan.restart(wall);
        let launching = plan.to_launch();
        tracing::info!(apps = launching.len(), "putting the layout back");
        for (exec, in_terminal) in launching {
            // A failure is in the log; the card already named anything it can't bring back.
            let _ = launch::app(&exec, in_terminal);
        }
        self.restoring = Some(plan);
    }

    /// A new window that a recorded place was waiting for: it goes where it was recorded rather
    /// than beside whatever has focus. Returns whether it claimed one, so `add_window` knows to
    /// leave it alone.
    pub(super) fn claim_restored(&mut self, window: &Window, area: Rect) -> bool {
        let Some(app) = window_app_id(window) else {
            return false;
        };
        let wall = self.wall();
        let Some(mut plan) = self.restoring.take() else {
            return false;
        };
        let claim = plan.claim(&app, window, wall);
        self.restoring = Some(plan);
        let Some(claim) = claim else {
            return false;
        };
        // A window recorded in the code rain is tiled first and poured into its stream at the end,
        // when the streams can be put back in the order they were in.
        let index = claim.workspace.unwrap_or_else(|| self.active_workspace());
        if self.workspaces.find(window) != Some(index) {
            self.take_off_workspace(window);
            self.workspaces.insert(index, window.clone(), None, area);
        }
        if claim.workspace.is_some() {
            self.rebuild_workspace(index, area);
        }
        self.retile();
        // Nothing takes focus while the desktop is filling up: the recorded window gets it back
        // once everything has landed, and a window opening meanwhile shouldn't steal it.
        if self.focused_window().is_none() {
            self.restore_focus();
        }
        tracing::debug!(
            app,
            workspace = index + 1,
            "a window claimed its recorded place"
        );
        true
    }

    /// A Wayland window's `app_id` arrives on its first commit, after `new_toplevel` has already
    /// tiled it, so a layout coming back looks again here: the window moves to where it was
    /// recorded the moment it says which app it is. (An X11 window has its class from the start
    /// and claims its place in `add_window`.)
    pub fn claim_on_commit(&mut self, surface: &WlSurface) {
        let wall = self.wall();
        if self
            .restoring
            .as_ref()
            .is_none_or(|plan| plan.settled(wall))
        {
            return;
        }
        let Some(window) = self
            .space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
        else {
            return;
        };
        // Already placed, or not tiled anywhere yet: nothing to do either way.
        if self
            .restoring
            .as_ref()
            .is_some_and(|plan| plan.holds(&window))
            || self.workspaces.find(&window).is_none()
        {
            return;
        }
        let Some(area) = self.output_area() else {
            return;
        };
        self.claim_restored(&window, area);
    }

    /// Puts workspace `index`'s recorded tree back around the windows that have claimed its slots.
    /// Anything on the workspace the record didn't know about is tiled in again afterwards, so a
    /// window opened by hand mid-restore isn't thrown away.
    pub(super) fn rebuild_workspace(&mut self, index: usize, area: Rect) {
        let Some(shape) = self.restoring.as_ref().and_then(|plan| plan.tree(index)) else {
            return;
        };
        let recorded = shape.leaves();
        let workspace = self.workspaces.get_mut(index);
        let extras: Vec<Window> = workspace
            .layout
            .windows()
            .into_iter()
            .filter(|window| !recorded.contains(window))
            .collect();
        workspace.layout.rebuild(&shape);
        for extra in extras {
            workspace.layout.insert(extra, None, area);
        }
    }

    /// A pass of the event loop while a layout is going back up: the offer is looked for once, and
    /// the restore finishes when every recorded window has come back or the wait is over.
    pub fn tick_restore(&mut self) {
        if !self.restore_looked {
            self.look_for_a_recorded_layout();
        }
        // While the offer is still up nothing has been launched, so nothing can have settled.
        if self.offer.is_some() {
            return;
        }
        let wall = self.wall();
        if self
            .restoring
            .as_ref()
            .is_some_and(|plan| plan.settled(wall))
        {
            self.finish_restore();
        }
    }

    /// The last of it, once the windows have landed or the wait is up: gravity, the code rain,
    /// the workspace that was in front, and finally focus.
    pub(super) fn finish_restore(&mut self) {
        let Some(plan) = self.restoring.take() else {
            return;
        };
        for index in 0..self.workspaces.count() {
            let gravity = plan.gravity(index);
            if gravity.is_on() {
                self.workspaces.get_mut(index).gravity = gravity;
            }
        }
        // Quietly: a toast per stream would bury the desktop you just asked for.
        for window in plan.to_minimise() {
            self.minimise(&window);
        }
        let active = plan.active();
        if active != self.active_workspace() {
            let here = self.focused_screen_name();
            let show = self.screens.show(active, self.workspaces.count());
            // The layout coming back is where the session left off, not a move to watch: the
            // view is put there rather than slid there.
            if show.moved_to.is_none() {
                self.motion.jump_camera(&here, active);
            }
        }
        self.retile();
        match plan.focus() {
            Some(window) if self.workspaces.find(&window) == Some(active) => {
                self.focus_window(&window)
            }
            _ => self.restore_focus(),
        }
        let missing = plan.len() - plan.claimed();
        tracing::info!(
            windows = plan.claimed(),
            missing,
            workspace = active + 1,
            "the layout is back"
        );
        if missing > 0 {
            let plural = if missing == 1 { "window" } else { "windows" };
            self.notify_self(
                &format!("{missing} {plural} didn’t come back"),
                "Their apps were asked to start but no window arrived. Everything else is where \
                 it was.",
                false,
            );
        }
    }

    /// Esc on the offer card. The record is left where it is: it's replaced at the next logout,
    /// and deleting it would turn "not this time" into "never".
    pub fn offer_key(&mut self, sym: Keysym) {
        let Some(offer) = self.offer.as_mut() else {
            return;
        };
        let act = offer.key(sym);
        self.act_on_offer(act);
    }

    pub fn offer_click(&mut self, x: f64, y: f64) {
        let Some(offer) = self.offer.as_mut() else {
            return;
        };
        let act = offer.click(x, y);
        self.act_on_offer(act);
    }

    /// The pointer moved while the offer is up: whatever it's over lights up.
    pub fn offer_hover(&mut self, pos: Point<f64, Logical>) {
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(offer) = self.offer.as_mut() {
            offer.hover(pos.x, pos.y);
        }
    }

    pub(super) fn act_on_offer(&mut self, act: offer::Act) {
        match act {
            offer::Act::Nothing => {}
            offer::Act::Reopen => {
                self.offer = None;
                if let Some(plan) = self.restoring.take() {
                    self.start_restore(plan);
                }
            }
            offer::Act::Clean => {
                self.offer = None;
                self.restoring = None;
                tracing::info!("starting clean; the recorded layout is left alone");
            }
        }
    }

    /// Makes the workspaces the settings' list (`Workspaces::reconcile`): windows stay with their
    /// workspace through renames and reordering, a deleted workspace's windows join the one before
    /// it, and every screen follows the workspace it was showing.
    pub(super) fn set_workspaces(&mut self, entries: &[(u32, String)], min: usize) {
        if self.bullet.is_some() {
            // Its frames, labels and pans are all by index; coming back out is simpler than
            // re-aiming every one of them.
            self.bullet_back();
        }
        let area = self.output_area().unwrap_or(Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        });
        let before = self.workspaces.count();
        let map = self.workspaces.reconcile(entries, min, area);
        let count = self.workspaces.count();
        self.screens.remap(&map, count);
        for screen in self.screens.iter() {
            self.motion
                .jump_camera(&screen.output.name(), screen.workspace);
        }
        self.retile();
        // A rename or a reorder leaves the focused window where it was, and the keyboard stays
        // with it: Settings saves on every key typed into a name, and taking the keyboard from
        // Settings each time would send the next key somewhere else. Only a focus the change has
        // stranded (its workspace deleted, or no longer on the screen) is put right.
        if !self.focus_survives_workspace_change() {
            self.restore_focus();
        }
        tracing::info!(before, count, "the workspace list changed");
    }

    /// A chooser has connected: the portal wants to know what to share. The card goes up with
    /// the portal's own list, in words a person can choose from.
    pub fn share_asked(&mut self, stream: std::os::unix::net::UnixStream) {
        share::read_request(&self.loop_handle, stream, |state, request| match request {
            Ok((list, reply)) => state.open_share_picker(&list, Some(reply)),
            Err(why) => tracing::warn!("the share picker's request was refused: {why}"),
        });
    }

    /// Opens the picker on the portal's list. `reply` is where the answer goes; a debug step has
    /// none.
    pub fn open_share_picker(&mut self, list: &str, reply: Option<std::os::unix::net::UnixStream>) {
        // Nothing can be chosen behind the lock, and a picker left waiting would take the first
        // Enter after unlocking. Dropping `reply` answers the portal with nothing: declined.
        if self.lock.is_some() {
            tracing::info!("sharing declined: the screen is locked");
            return;
        }
        if self.share.is_some() {
            // One question at a time. Dropping the stream answers the second asker with nothing,
            // which its portal reads as declined.
            tracing::info!("a second app asked to share while the picker was up; declined");
            return;
        }
        let choices = share::parse(list);
        if choices.is_empty() {
            tracing::info!("asked to share, with nothing to choose from");
            return;
        }
        let rows = choices
            .iter()
            .map(|choice| self.share_row(choice))
            .collect();
        tracing::info!(choices = choices.len(), "an app asked to share the screen");
        let now = self.start_time.elapsed().as_secs_f64();
        self.explorer.close();
        self.quick.close();
        self.centre.close();
        self.share = Some(Picker::new(
            choices,
            rows,
            reply,
            now,
            self.clock.reduced_motion,
        ));
        self.wake_ui();
    }

    /// What a row of the picker says about a source.
    pub(super) fn share_row(&self, choice: &share::Choice) -> card::Row {
        match &choice.source {
            share::Source::Screen { name } => {
                let output = self.space.outputs().find(|output| output.name() == *name);
                let size = output
                    .and_then(|output| output.current_mode())
                    .map(|mode| format!("{}×{}", mode.size.w, mode.size.h));
                let count = self.space.outputs().count();
                card::Row {
                    name: if count > 1 {
                        format!("Screen {name}")
                    } else {
                        "The whole screen".to_string()
                    },
                    title: String::new(),
                    note: size.map(|size| (size, crate::panel::HINT)),
                }
            }
            share::Source::Window { identifier } => match self.captures.window_for(identifier) {
                Some(window) => card::Row {
                    name: self.window_name(&window),
                    title: window_title(&window),
                    note: self
                        .rain
                        .streams
                        .iter()
                        .any(|stream| stream.window == window)
                        .then(|| ("minimised".to_string(), crate::panel::HINT)),
                },
                None => card::Row {
                    name: "A window".to_string(),
                    title: String::new(),
                    note: None,
                },
            },
        }
    }

    pub fn share_key(&mut self, sym: Keysym, shift: bool) {
        let now = self.wall();
        let Some(picker) = self.share.as_mut() else {
            return;
        };
        let act = picker.key(sym, shift, now);
        self.act_on_share(act);
    }

    pub fn share_click(&mut self, x: f64, y: f64) {
        let Some(picker) = self.share.as_mut() else {
            return;
        };
        let act = picker.click(x, y);
        self.act_on_share(act);
    }

    pub fn share_hover(&mut self, pos: Point<f64, Logical>) {
        if self.lock.is_some() {
            return;
        }
        let Some(screen) = self.focused_screen_geometry() else {
            return;
        };
        let pos = pos - screen.loc.to_f64();
        if let Some(picker) = self.share.as_mut() {
            picker.hover(pos.x, pos.y);
        }
    }

    pub(super) fn act_on_share(&mut self, act: share::Act) {
        let yes = match act {
            share::Act::Nothing => return,
            share::Act::Share => true,
            share::Act::Cancel => false,
        };
        let Some(picker) = self.share.take() else {
            return;
        };
        match picker.asking.clone() {
            // A program asked to record the screen itself. Its frames have been waiting on this.
            share::Asking::Capture { exe, .. } => {
                if yes {
                    self.allow_capture(exe, picker.remembers());
                } else {
                    self.refuse_capture(exe);
                }
            }
            share::Asking::Portal => {
                match picker.chosen() {
                    Some(choice) if yes => tracing::info!(source = ?choice.source, "sharing"),
                    _ => tracing::info!("sharing declined"),
                }
                picker.answer(yes);
            }
        }
    }

    /// Writes the record down, or deletes it, as the switch on the card said. Called once, at the
    /// point of no return.
    pub(super) fn keep_record(&mut self, remember: bool) {
        if !self.owns_state {
            return;
        }
        let path = session::path();
        let done = match (remember, self.exit_record.take()) {
            (true, Some(record)) => {
                let windows = record.windows.len();
                session::write(&path, &record).inspect(|()| {
                    tracing::info!(windows, path = %path.display(), "layout written down");
                })
            }
            // Off means no record sitting in the state directory at all, not a stale one.
            _ => session::forget(&path),
        };
        if let Err(err) = done {
            tracing::warn!(path = %path.display(), "couldn't keep the layout: {err}");
        }
    }

    /// A pass of the event loop while the way out is on screen: what's still open, and whether a
    /// timer has run out. Wall time, so bullet time can't stretch a logout.
    pub fn tick_exit(&mut self) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let open = self.mapped_windows();
        let act = exit.tick(self.wall(), &open);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }

    /// A key while the way out is taking every one of them.
    pub fn exit_key(&mut self, sym: Keysym) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let wall = self.wall();
        let act = exit.key(sym, wall);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }

    pub(super) fn act_on_exit(&mut self, act: Act<Window>) {
        match act {
            Act::Nothing => {}
            Act::Close(windows) => {
                for window in windows {
                    self.close_window(&window);
                }
            }
            // Nothing was lost: the desktop is as it was, minus whatever already closed.
            Act::Cancel => {
                self.exit = None;
                self.exit_record = None;
            }
            // The one answer that keeps the session: the card goes and the lock comes up.
            Act::Lock => {
                self.exit = None;
                self.exit_record = None;
                self.lock_now();
            }
            // The switch on the card is the setting, so it is still on the next time.
            Act::Remember(on) => {
                self.change_settings(move |settings| settings.session.remember = on)
            }
            Act::Go(intent) => {
                let remember = self.exit.as_ref().is_some_and(Exit::remember);
                self.keep_record(remember);
                match intent {
                    Intent::LogOut => {
                        tracing::info!("logging out");
                        self.loop_signal.stop();
                    }
                    // These end the session the same way, having asked every app first.
                    Intent::Restart | Intent::ShutDown => {
                        if let Some(power) = crate::power::Power::for_intent(intent) {
                            self.request_power(power);
                        }
                    }
                }
            }
        }
    }

    /// Asks logind for Sleep, Restart or Shut down, on a thread of its own. The answer comes back
    /// through `power_answers` to `power_answered`.
    pub fn request_power(&mut self, power: crate::power::Power) {
        let answers = self.power_answers.clone();
        crate::power::request(
            power,
            crate::power::Answerer::current(),
            crate::power::ask,
            move |answer| {
                let _ = answers.send(answer);
            },
        );
    }

    pub(super) fn power_answered(&mut self, answer: crate::power::Answer) {
        match answer {
            crate::power::Answer::Refused(refusal) => self.power_refused(refusal),
            // Nested, no logind ends the session, so it ends here, as Log out does. The record was
            // already kept on the way out.
            crate::power::Answer::StoodIn(power) if power.ends_the_session() => {
                tracing::info!(?power, "nested: stopping instead");
                self.loop_signal.stop();
            }
            crate::power::Answer::StoodIn(_) => {}
        }
    }

    /// logind said no. Restart and Shut down have closed every app and faded the screen to black
    /// by now: the way out is cleared, which lifts the black, and a toast says what was refused.
    pub(super) fn power_refused(&mut self, refusal: crate::power::Refused) {
        if refusal.power != crate::power::Power::Sleep {
            self.exit = None;
            self.exit_record = None;
        }
        let (title, body) = refusal.message();
        self.wake_ui();
        self.notify_self(&title, &body, false);
    }

    /// The pointer moved while the way out is up: whatever it's over lights up. `pos` is in the
    /// whole desktop's coordinates, as the pointer keeps them; the card knows its own screen's.
    pub fn exit_hover(&mut self, pos: Point<f64, Logical>) {
        if self.exit.is_none() {
            return;
        }
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(exit) = self.exit.as_mut() {
            exit.hover(pos.x, pos.y);
        }
    }

    /// A click on the way out's card.
    pub fn exit_click(&mut self, x: f64, y: f64) {
        let Some(mut exit) = self.exit.take() else {
            return;
        };
        let wall = self.wall();
        let act = exit.click(x, y, wall);
        self.exit = Some(exit);
        self.act_on_exit(act);
    }
}

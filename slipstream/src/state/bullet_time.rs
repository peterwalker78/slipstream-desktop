//! Bullet time (Super+Tab): opening it, its keys, the pointer in it, and leaving.

use super::*;

impl Slipstream {
    /// Super+Tab: bullet time, or back out of it with nothing changed.
    pub fn toggle_bullet_time(&mut self) {
        if self.bullet.is_some() {
            self.bullet_back();
        } else {
            self.enter_bullet_time();
        }
    }

    pub(super) fn overview_duration(&self) -> f64 {
        if self.clock.reduced_motion {
            0.0
        } else {
            motion::MOVE
        }
    }

    /// Windows of workspace `index`, most recently used first.
    pub(super) fn windows_by_recency(&self, index: usize) -> Vec<Window> {
        let mut windows = self.workspaces.get(index).windows();
        windows.sort_by_key(|window| recency(&self.focus_history, window));
        windows
    }

    pub(super) fn bullet_targets(&self, home: usize) -> Vec<Target<Window>> {
        let mut others: Vec<Window> = (0..self.workspaces.count())
            .filter(|&index| index != home)
            .flat_map(|index| self.workspaces.get(index).windows())
            .collect();
        others.sort_by_key(|window| recency(&self.focus_history, window));
        let streams = self
            .rain
            .streams
            .iter()
            .map(|stream| stream.window.clone())
            .collect();
        // The docked pane is in view from every workspace, so it is lettered with the home
        // workspace's windows, after them.
        let mut here = self.windows_by_recency(home);
        here.extend(self.docked_on_show());
        bullet::targets(here, streams, others)
    }

    pub fn enter_bullet_time(&mut self) {
        self.close_panels();
        // Workspaces off screen can hold sizes from before the code rain took or gave back space.
        for index in 0..self.workspaces.count() {
            self.place_workspace(index);
        }
        let home = self.active_workspace();
        let targets = self.bullet_targets(home);
        let labels = bullet::labels(&self.bullet_labels, &targets);
        self.bullet_labels = labels.clone();
        self.bullet = Some(bullet::Mode {
            home,
            view: home,
            selected: self.focused_window().map(Target::Window),
            labels,
            typed: String::new(),
            pointer: bullet::Pointer::default(),
        });
        self.clock.enter_bullet_time();
        if self.muffles_sound() {
            crate::sound::muffle(true);
        }
        let (wall, duration) = (self.wall(), self.overview_duration());
        self.overview.retarget([1.0], wall, duration, motion::HYPR);
        tracing::info!(targets = targets.len(), "bullet time");
    }

    /// Out of bullet time with nothing changed, if it's open.
    pub fn leave_bullet_time_now(&mut self) {
        if self.bullet.is_some() {
            self.bullet_back();
        }
    }

    pub(super) fn leave_bullet_time(&mut self) {
        let Some(mode) = self.bullet.take() else {
            return;
        };
        if mode.pointer.drag.is_some() {
            self.set_cursor_override(None);
        }
        self.clock.leave_bullet_time();
        if self.muffles_sound() {
            crate::sound::muffle(false);
        }
        let (wall, duration) = (self.wall(), self.overview_duration());
        self.overview.retarget([0.0], wall, duration, motion::HYPR);
    }

    /// Whether bullet time muffles the sound. A nested run shares the login session's PipeWire, so
    /// muffling there would muffle the real desktop's sound around it: it's the session's to do,
    /// unless a test asks for it with `SLIPSTREAM_MUFFLE=1`.
    pub(super) fn muffles_sound(&self) -> bool {
        self.session || std::env::var("SLIPSTREAM_MUFFLE").as_deref() == Ok("1")
    }

    /// Esc: back where bullet time started, with nothing changed.
    pub(super) fn bullet_back(&mut self) {
        self.leave_bullet_time();
        let (wall, active) = (self.wall(), self.active_workspace());
        let here = self.focused_screen_name();
        self.motion.slide_to(&here, active, wall);
    }

    /// Enter, a click or a label: go to the chosen window, bring back the chosen stream, or land
    /// on the workspace being looked at. Focus moves at once; the zoom back in follows.
    pub(super) fn bullet_go(&mut self, target: Option<Target<Window>>) {
        let Some(view) = self.bullet.as_ref().map(|mode| mode.view) else {
            return;
        };
        self.leave_bullet_time();
        match target {
            Some(target) if self.rain.contains(target.window()) => {
                self.switch_workspace(view);
                self.restore(target.window());
            }
            Some(target) => {
                let window = target.window().clone();
                if let Some(index) = self.workspaces.find(&window) {
                    self.switch_workspace(index);
                }
                self.focus_window(&window);
            }
            None => self.switch_workspace(view),
        }
        // The view was panned on the screen bullet time was on, and the keyboard may have gone to
        // another: every screen slides back to what it's showing.
        let wall = self.wall();
        for screen in self.screens.iter() {
            self.motion
                .slide_to(&screen.output.name(), screen.workspace, wall);
        }
    }

    /// Looks at workspace `index`, choosing its most recent window when `choose` is set.
    pub(super) fn bullet_pan(&mut self, index: usize, choose: bool) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        let wall = self.wall();
        let here = self.focused_screen_name();
        self.motion.slide_to(&here, index, wall);
        let most_recent = self.windows_by_recency(index).into_iter().next();
        if let Some(mode) = self.bullet.as_mut() {
            mode.view = index;
            if choose {
                mode.selected = most_recent.map(Target::Window);
            }
        }
    }

    /// Chooses `target`, looking at its workspace if it's on another.
    pub(super) fn bullet_select(&mut self, target: Option<Target<Window>>) {
        let on = match &target {
            Some(Target::Window(window)) => self.workspaces.find(window),
            _ => None,
        };
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        mode.selected = target;
        let view = mode.view;
        if let Some(index) = on.filter(|&index| index != view) {
            self.bullet_pan(index, false);
        }
    }

    pub(super) fn bullet_cycle(&mut self, forward: bool) {
        let Some(mode) = self.bullet.as_ref() else {
            return;
        };
        let targets = self.bullet_targets(mode.home);
        let next = bullet::cycle(&targets, mode.selected.as_ref(), forward);
        self.bullet_select(next);
    }

    /// Arrows choose the nearest window that way. Past the last one, ←/→ glide on to the
    /// neighbouring workspace.
    pub(super) fn bullet_arrow(&mut self, direction: Direction) {
        let Some(mode) = self.bullet.clone() else {
            return;
        };
        // From the docked pane, which hangs above the windows at the right, down or left is
        // back among them.
        if let Some(Target::Window(window)) = &mode.selected
            && self.is_docked(window)
        {
            if matches!(direction, Direction::Down | Direction::Left) {
                let most_recent = self.windows_by_recency(mode.view).into_iter().next();
                if most_recent.is_some() {
                    self.bullet_select(most_recent.map(Target::Window));
                }
            }
            return;
        }
        let mut rects = self.workspace_rects(mode.view);
        // Most recently used first: with two windows equally ahead, the arrow picks the last used.
        rects.sort_by_key(|(window, _)| recency(&self.focus_history, window));
        let current = match &mode.selected {
            Some(Target::Window(window)) if rects.iter().any(|(tiled, _)| tiled == window) => {
                window.clone()
            }
            _ => {
                let most_recent = self.windows_by_recency(mode.view).into_iter().next();
                if most_recent.is_some() {
                    return self.bullet_select(most_recent.map(Target::Window));
                }
                return self.bullet_cross(mode.view, direction);
            }
        };
        match bullet::nearest(&rects, &current, direction) {
            Some(window) => self.bullet_select(Some(Target::Window(window))),
            // Up from the top of the windows is the pane hanging from the bar.
            None if direction == Direction::Up && self.docked_on_show().is_some() => {
                let docked = self.docked_on_show();
                self.bullet_select(docked.map(Target::Window));
            }
            None => self.bullet_cross(mode.view, direction),
        }
    }

    pub(super) fn bullet_cross(&mut self, from: usize, direction: Direction) {
        let going_right = match direction {
            Direction::Right => true,
            Direction::Left => false,
            _ => return,
        };
        let Some(index) = (if going_right {
            Some(from + 1).filter(|&index| index < self.workspaces.count())
        } else {
            from.checked_sub(1)
        }) else {
            return;
        };
        self.bullet_pan(index, false);
        let mut rects = self.workspace_rects(index);
        rects.sort_by_key(|(window, _)| recency(&self.focus_history, window));
        let arriving = bullet::entering(&rects, going_right).map(Target::Window);
        if let Some(mode) = self.bullet.as_mut() {
            mode.selected = arriving;
        }
    }

    /// Shift+1–9 and Shift+←/→: sends the chosen window to another workspace, and looks there.
    pub(super) fn bullet_send(&mut self, index: usize) {
        let Some(Some(Target::Window(window))) =
            self.bullet.as_ref().map(|mode| mode.selected.clone())
        else {
            return;
        };
        // A docked window comes down from the bar to go to a workspace.
        if self.is_docked(&window) {
            self.come_down(None);
        }
        self.move_window_to_workspace(&window, index, false);
        self.bullet_pan(index.min(self.workspaces.count().saturating_sub(1)), false);
    }

    pub(super) fn bullet_letter(&mut self, letter: char) {
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        mode.typed.push(letter);
        match bullet::resolve(&mode.labels, &mode.typed) {
            Typed::Exact(target) => {
                mode.typed.clear();
                self.bullet_go(Some(target));
            }
            Typed::Partial => {}
            Typed::NoMatch => mode.typed.clear(),
        }
    }

    /// A key while bullet time is open: the everyday keys, without Super.
    pub fn bullet_key(&mut self, key: Keysym, mods: Mods) {
        let Some(mode) = self.bullet.as_ref() else {
            return;
        };
        let (view, typing) = (mode.view, !mode.typed.is_empty());
        let chosen = match &mode.selected {
            Some(Target::Window(window)) => Some(window.clone()),
            _ => None,
        };
        let Some(command) = bullet::command(key, mods) else {
            return;
        };
        let count = self.workspaces.count();
        // Esc lets go of a window being dragged before anything else.
        if key == Keysym::Escape && self.cancel_bullet_drag() {
            return;
        }
        match command {
            // Esc clears a half-typed label first; Super+Tab always goes straight back.
            bullet::Command::Back if typing && key == Keysym::Escape => {
                if let Some(mode) = self.bullet.as_mut() {
                    mode.typed.clear();
                }
            }
            bullet::Command::Back => self.bullet_back(),
            bullet::Command::Go => {
                let target = self.bullet.as_ref().and_then(|mode| mode.selected.clone());
                self.bullet_go(target);
            }
            bullet::Command::Cycle { forward } => self.bullet_cycle(forward),
            bullet::Command::SendBeside(direction) => {
                let from = chosen
                    .as_ref()
                    .and_then(|window| self.workspaces.find(window))
                    .unwrap_or(view);
                let to = if direction == Direction::Left {
                    from.checked_sub(1)
                } else {
                    Some(from + 1).filter(|&to| to < count)
                };
                if let Some(to) = to {
                    self.bullet_send(to);
                }
            }
            bullet::Command::Choose(direction) => self.bullet_arrow(direction),
            bullet::Command::Weigh { heavier } => {
                if let Some(window) = chosen {
                    self.weigh_window(&window, heavier);
                    if self.rain.contains(&window) {
                        self.bullet_select(Some(Target::Stream(window)));
                    }
                }
            }
            bullet::Command::Close => {
                if let Some(window) = chosen {
                    self.close_window(&window);
                }
            }
            bullet::Command::Minimise => {
                if let Some(window) = chosen {
                    if self.is_docked(&window) {
                        self.come_down(None);
                    }
                    self.minimise(&window);
                    self.bullet_select(Some(Target::Stream(window)));
                }
            }
            bullet::Command::Send(index) if index < count => self.bullet_send(index),
            bullet::Command::Look(index) if index < count => self.bullet_pan(index, true),
            bullet::Command::Send(_) | bullet::Command::Look(_) => {}
            bullet::Command::Jump(letter) => self.bullet_letter(letter),
        }
    }

    /// The bar's title while bullet time is open: the chosen window's title (its app's name when
    /// it has none), or the workspace being looked at and how full it is.
    pub fn bullet_bar_title(&self) -> String {
        let Some(mode) = self.bullet.as_ref() else {
            return self.focused_title();
        };
        let chosen = mode.selected.as_ref().map(|target| {
            let window = target.window();
            let title = window_title(window);
            let name = if title.is_empty() {
                self.window_name(window)
            } else {
                title
            };
            (name, matches!(target, Target::Stream(_)))
        });
        let view = mode.view.min(self.workspaces.count().saturating_sub(1));
        bullet::bar_title(
            chosen
                .as_ref()
                .map(|(name, minimised)| (name.as_str(), *minimised)),
            &self.workspaces.label(view),
            view,
            self.workspaces.get(view).len(),
        )
    }

    /// A click on the bar while bullet time is open. A workspace's number looks at it, as its
    /// number key does; a panel's button goes back out with nothing changed and opens the panel.
    pub(super) fn bullet_bar_clicked(&mut self, target: bar::Target) {
        // The docked window's slot goes to its pane, as a click on the pane does.
        if target == bar::Target::Dock
            && let Some(docked) = self.docked_on_show()
        {
            return self.bullet_go(Some(Target::Window(docked)));
        }
        match bullet::bar_click(target) {
            bullet::BarClick::Look(index) => self.bullet_pan(index, true),
            bullet::BarClick::Leave => {
                self.bullet_back();
                self.bar_clicked(target);
            }
            bullet::BarClick::Stay => self.bar_clicked(target),
            bullet::BarClick::Back => self.bullet_back(),
        }
    }

    /// Where `pos` on screen is in the overview's flat coordinates, which its hit areas use.
    pub(super) fn overview_point(&self, pos: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        let screen = self.output_rect()?;
        let local = (pos.x - screen.x as f64, pos.y - screen.y as f64);
        self.overview_tilt.unproject(local).map(Point::from)
    }

    /// The window drawn at the flat point `at` in the overview, and where it's drawn.
    pub(super) fn overview_window_at(
        &self,
        at: Point<f64, Logical>,
    ) -> Option<(Window, Rectangle<f64, Logical>)> {
        self.overview_hits
            .iter()
            .find(|(_, area)| area.contains(at))
            .cloned()
    }

    /// The workspace whose frame is at the flat point `at` in the overview.
    pub(super) fn overview_frame_at(&self, at: Point<f64, Logical>) -> Option<usize> {
        self.frame_hits
            .iter()
            .find(|(_, area)| area.contains(at))
            .map(|(index, _)| *index)
    }

    /// The pointer moved in bullet time: the window under it shows its close button, and a left
    /// press on a window that has moved far enough becomes a drag towards another workspace.
    pub fn bullet_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        if self.bullet.is_none() {
            return;
        }
        let flat = self.overview_point(pos);
        let hit = flat.and_then(|at| self.overview_window_at(at));
        let over = flat.and_then(|at| self.overview_frame_at(at));
        let Some(mode) = self.bullet.as_mut() else {
            return;
        };
        let pointer = &mut mode.pointer;
        pointer.on_close = matches!((&hit, flat), (Some((_, shown)), Some(at)) if bullet::close_button(*shown).contains(at));
        pointer.hover = hit.map(|(window, _)| window);
        let mut started = false;
        if let (None, Some((bullet::Press::Window(window), from))) = (&pointer.drag, &pointer.press)
            && bullet::is_drag(*from, pos)
        {
            pointer.drag = Some(bullet::Drag {
                window: window.clone(),
                at: flat.unwrap_or_default(),
                over,
            });
            pointer.press = None;
            started = true;
        }
        if let (Some(drag), Some(at)) = (pointer.drag.as_mut(), flat) {
            drag.at = at;
            drag.over = over;
        }
        if started {
            self.set_cursor_override(Some(smithay::input::pointer::CursorIcon::Grabbing));
        }
    }

    /// A pointer button in bullet time. On a window, a left press goes to it when let go, unless
    /// it became a drag, which sends the window to the workspace it's let go over; a left click on
    /// its close button closes it; the middle button closes it and the right one minimises it.
    /// Anything else (the bar, a stream, a frame) is a click, as before.
    pub fn bullet_button(&mut self, button: u32, pressed: bool, pos: Point<f64, Logical>) {
        const LEFT: u32 = 0x110;
        const RIGHT: u32 = 0x111;
        const MIDDLE: u32 = 0x112;
        if !pressed {
            if button != LEFT {
                return;
            }
            let Some(mode) = self.bullet.as_mut() else {
                return;
            };
            let (press, drag) = (mode.pointer.press.take(), mode.pointer.drag.take());
            if let Some(drag) = drag {
                self.set_cursor_override(None);
                let from = self.workspaces.find(&drag.window);
                match drag.over {
                    Some(index) if Some(index) != from => {
                        self.bullet_select(Some(Target::Window(drag.window)));
                        self.bullet_send(index);
                    }
                    _ => {}
                }
                return;
            }
            let hit = self
                .overview_point(pos)
                .and_then(|at| Some((at, self.overview_window_at(at)?)));
            match press {
                Some((bullet::Press::Close(window), _)) => {
                    let still_on = matches!(&hit, Some((at, (under, shown))) if *under == window && bullet::close_button(*shown).contains(*at));
                    if still_on {
                        self.bullet_close(&window);
                    }
                }
                Some((bullet::Press::Window(window), _)) => {
                    self.bullet_go(Some(Target::Window(window)));
                }
                None => {}
            }
            return;
        }
        // The bar and the code rain first, as a click always has.
        if self.bar_target_at(pos).is_some() || self.rain_window_at(pos).is_some() {
            if button == LEFT {
                self.bullet_click(pos);
            }
            return;
        }
        // Then the docked pane, upright in front of the overview: the same three buttons as on
        // any window.
        if let Some(window) = self.docked_at(pos) {
            match button {
                LEFT => self.bullet_go(Some(Target::Window(window))),
                MIDDLE => self.bullet_close(&window),
                RIGHT => {
                    self.come_down(None);
                    self.minimise(&window);
                    self.bullet_select(Some(Target::Stream(window)));
                }
                _ => {}
            }
            return;
        }
        let hit = self
            .overview_point(pos)
            .and_then(|at| Some((at, self.overview_window_at(at)?)));
        let Some((at, (window, shown))) = hit else {
            if button == LEFT {
                self.bullet_click(pos);
            }
            return;
        };
        match button {
            LEFT => {
                let press = if bullet::close_button(shown).contains(at) {
                    bullet::Press::Close(window)
                } else {
                    bullet::Press::Window(window)
                };
                if let Some(mode) = self.bullet.as_mut() {
                    mode.pointer.press = Some((press, pos));
                }
            }
            MIDDLE => self.bullet_close(&window),
            RIGHT => {
                self.minimise(&window);
                self.bullet_select(Some(Target::Stream(window)));
            }
            _ => {}
        }
    }

    /// Closes a window from bullet time, which stays open.
    pub(super) fn bullet_close(&mut self, window: &Window) {
        if let Some(mode) = self.bullet.as_mut() {
            if mode.pointer.hover.as_ref() == Some(window) {
                mode.pointer.hover = None;
                mode.pointer.on_close = false;
            }
        }
        self.close_window(window);
    }

    /// Esc while a window is dragged in bullet time: the drag ends, with nothing sent. Whether
    /// there was one.
    pub(super) fn cancel_bullet_drag(&mut self) -> bool {
        let dragging = self
            .bullet
            .as_mut()
            .and_then(|mode| mode.pointer.drag.take())
            .is_some();
        if dragging {
            self.set_cursor_override(None);
        }
        dragging
    }

    /// A click in bullet time: on the bar it does what the bar says; on a window or stream it goes
    /// there; on a workspace's frame it lands on that workspace.
    pub fn bullet_click(&mut self, pos: Point<f64, Logical>) {
        // The bar is drawn upright over the tilted overview, so it's asked first.
        if let Some(target) = self.bar_target_at(pos) {
            return self.bullet_bar_clicked(target);
        }
        if let Some(window) = self.rain_window_at(pos) {
            return self.bullet_go(Some(Target::Stream(window)));
        }
        if let Some(window) = self.docked_at(pos) {
            return self.bullet_go(Some(Target::Window(window)));
        }
        let Some(screen) = self.output_rect() else {
            return;
        };
        // The overview is tilted on screen; its hit areas are kept flat.
        let local = (pos.x - screen.x as f64, pos.y - screen.y as f64);
        let Some(local) = self.overview_tilt.unproject(local) else {
            return;
        };
        let window = self
            .overview_hits
            .iter()
            .find(|(_, area)| area.contains(local))
            .map(|(window, _)| window.clone());
        if let Some(window) = window {
            return self.bullet_go(Some(Target::Window(window)));
        }
        let frame = self
            .frame_hits
            .iter()
            .find(|(_, area)| area.contains(local))
            .map(|(index, _)| *index);
        if let Some(index) = frame {
            if let Some(mode) = self.bullet.as_mut() {
                mode.view = index;
            }
            self.bullet_go(None);
        }
    }
}

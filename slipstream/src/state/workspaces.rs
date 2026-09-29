//! Workspaces: switching between them, sending windows to them, and Alt+Tab's cycle.

use super::*;

impl Slipstream {
    /// Goes to workspace `index` (0-based), remembering focus on the one being left. If another
    /// screen is showing it, the keyboard moves to that screen and neither screen changes what
    /// it shows; otherwise the focused screen slides over to it.
    pub fn switch_workspace(&mut self, index: usize) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        let leaving = self.focused_window();
        if leaving.is_some() {
            let active = self.active_workspace();
            self.workspaces.get_mut(active).last_focus = leaving;
        }
        let here = self.focused_screen_name();
        let show = self.screens.show(index, self.workspaces.count());
        if !show.changed {
            return;
        }
        if show.moved_to.is_none() {
            // The camera runs on wall time: moving about is never slowed, even in bullet time.
            let wall = self.wall();
            self.motion.slide_to(&here, index, wall);
            self.retile();
        }
        self.restore_focus();
        tracing::info!(
            workspace = index + 1,
            windows = self.workspaces.get(index).layout.len(),
            to_another_screen = show.moved_to.is_some(),
            "switched workspace"
        );
    }

    /// Super+Ctrl+P: the focused screen and the next one along trade workspaces, windows and
    /// all. The keyboard stays on this screen, which now shows what the other one was.
    pub fn swap_with_next_screen(&mut self) {
        if self.screens.len() < 2 {
            return;
        }
        let leaving = self.focused_window();
        if leaving.is_some() {
            let active = self.active_workspace();
            self.workspaces.get_mut(active).last_focus = leaving;
        }
        let next = (self.screens.focused_index() + 1) % self.screens.len();
        if !self.screens.swap_with(next) {
            return;
        }
        // The windows glide across to their new screens; the views are already there.
        for screen in self.screens.iter() {
            self.motion
                .jump_camera(&screen.output.name(), screen.workspace);
        }
        self.retile();
        self.restore_focus();
        tracing::info!("swapped workspaces with the next screen");
    }

    /// Super+D: the lowest-numbered empty workspace no screen is showing, remembering where it
    /// came from. Pressed again while still on that workspace, and it's still empty, it goes back.
    pub fn show_desktop(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let active = self.active_workspace();
        if let Some((from, to)) = self.desktop_return.take()
            && to == active
            && self.workspaces.get(active).is_empty()
        {
            tracing::info!(workspace = from + 1, "back from the empty workspace");
            self.switch_workspace(from);
            return;
        }
        let empty: Vec<bool> = (0..self.workspaces.count())
            .map(|index| self.workspaces.get(index).is_empty())
            .collect();
        let shown: Vec<usize> = self.screens.iter().map(|screen| screen.workspace).collect();
        match crate::workspace::lowest_empty(&empty, &shown) {
            Some(to) => {
                tracing::info!(workspace = to + 1, "to an empty workspace");
                self.desktop_return = Some((active, to));
                self.switch_workspace(to);
            }
            None => self.show_toast("No empty workspace", "Settings › Workspaces adds one."),
        }
    }

    /// The workspace a number key names, 1-based. A number past the last workspace says how many
    /// there are rather than doing nothing or landing somewhere unasked.
    pub fn workspace_for_key(&mut self, number: u8) -> Option<usize> {
        let count = self.workspaces.count();
        let index = (number as usize).checked_sub(1)?;
        if index < count {
            return Some(index);
        }
        let plural = if count == 1 {
            "workspace"
        } else {
            "workspaces"
        };
        self.show_toast(
            &format!("No workspace {number}"),
            &format!("There are {count} {plural}. Settings › Workspaces adds more."),
        );
        None
    }

    /// Super+Ctrl+←/→: the previous or next workspace on this screen, passing over any another
    /// screen is showing.
    pub fn switch_workspace_by(&mut self, delta: i32) {
        let count = self.workspaces.count() as i32;
        let mut target = self.active_workspace() as i32;
        loop {
            target += delta.signum();
            if target < 0 || target >= count || delta == 0 {
                return;
            }
            if self.screens.showing(target as usize).is_none() {
                break;
            }
        }
        self.switch_workspace(target as usize);
    }

    /// Points the keyboard at another screen: Super+P, a click on it, or the screen it was on
    /// going out. The workspaces stay where they are.
    pub fn focus_screen_at(&mut self, index: usize) {
        if !self.screens.focus(index) {
            return;
        }
        self.restore_focus();
        if let Some(screen) = self.screens.get(index) {
            tracing::info!(
                screen = screen.output.name(),
                workspace = screen.workspace + 1,
                "the keyboard moved to another screen"
            );
        }
    }

    /// Moves the focused window to workspace `index` (0-based) and follows it there, as moving a
    /// window to another desktop does on Windows.
    pub fn move_focused_to_workspace(&mut self, index: usize) {
        if let Some(window) = self.focused_window() {
            self.move_window_to_workspace(&window, index, true);
        }
    }

    /// Moves `window` to workspace `index` (0-based). With `follow`, the view goes with it and it
    /// keeps focus; otherwise (bullet time sending it) the workspace on screen stays put.
    pub fn move_window_to_workspace(&mut self, window: &Window, index: usize, follow: bool) {
        let index = index.min(self.workspaces.count().saturating_sub(1));
        // It tiles into the area of the screen showing where it's going, which is not this one
        // when the window is being sent to another screen.
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let Some(from) = self.workspaces.find(window) else {
            return;
        };
        if from == index {
            return;
        }
        // Between workspaces laid out on the same screen, the window rides across the gap between
        // them; to another screen it simply glides to its new tile there.
        let same_screen =
            self.screens.screen_for_workspace(from) == self.screens.screen_for_workspace(index);
        // A floating window floats on where it goes, in the same part of the screen.
        let float = self.workspaces.get(from).floating.get(window).cloned();
        self.take_off_workspace(window);
        match float {
            Some(float) => self.workspaces.get_mut(index).floating.put(float),
            None => {
                let beside = self.workspaces.get(index).last_focus.clone();
                self.workspaces
                    .insert(index, window.clone(), beside.as_ref(), area);
            }
        }
        // The window rides across to its new workspace, then settles into its tile.
        if same_screen {
            let now = self.clock.tick();
            let step = (area.w + motion::WORKSPACE_GAP) as f64;
            self.motion
                .carry(window, (from as f64 - index as f64) * step, now);
        }
        if follow {
            let here = self.focused_screen_name();
            let show = self.screens.show(index, self.workspaces.count());
            if show.changed && show.moved_to.is_none() {
                let wall = self.wall();
                self.motion.slide_to(&here, index, wall);
            }
            self.retile();
            self.focus_window(window);
        } else {
            self.retile();
            if self.focused_window().is_none() {
                self.restore_focus();
            }
        }
        tracing::info!(workspace = index + 1, follow, "moved window to workspace");
    }

    /// Seconds since start, for animations that bullet time must never slow. A recording's
    /// `slow:` steps these too, so the whole picture stays together.
    pub fn wall(&self) -> f64 {
        self.clock.wall()
    }

    /// Where the windows of workspace `index` go, gravity included: in the area of the screen
    /// showing it, or of the focused screen when none is.
    pub(super) fn workspace_rects(&mut self, index: usize) -> Vec<(Window, Rect)> {
        let Some(area) = self.area_for_workspace(index) else {
            return Vec::new();
        };
        self.workspaces.get_mut(index).rects_within(area, &min_size)
    }

    /// Sends the windows of a workspace that isn't on screen towards their tiles, so bullet time's
    /// overview shows the change. They're told their new sizes too: otherwise a window keeps
    /// drawing at the size it had before, say, the code rain took width, and spills out of its
    /// workspace's frame. (The workspace on screen is placed by `retile`.)
    pub(super) fn place_workspace(&mut self, index: usize) {
        if self.screens.showing(index).is_some() {
            return;
        }
        let screen = self.screen_rect_for_workspace(index);
        let now = self.clock.tick();
        for (window, tile) in self.workspace_rects(index) {
            let full = self.is_fullscreen(&window);
            let r = match (full, screen) {
                (true, Some(screen)) => screen,
                _ => tile,
            };
            let asked = if full {
                r
            } else {
                size_for_tile(r, min_size(&window))
            };
            configure(&window, asked, full);
            self.motion.place(&window, r, now);
        }
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let floating = self.workspaces.get(index).floating.clone();
        for (window, r) in floating.rects(area, &crate::floating::own_size) {
            let asked = floating.get(&window).and_then(|float| float.asked);
            crate::floating::configure_floating(&window, asked, r);
            self.motion.place(&window, r, now);
        }
    }

    pub fn move_focused_by(&mut self, delta: i32) {
        let target =
            (self.active_workspace() as i32 + delta).clamp(0, self.workspaces.count() as i32 - 1);
        self.move_focused_to_workspace(target as usize);
    }

    pub(super) fn remember_focus(&mut self, window: &Window) {
        self.focus_history.retain(|w| w != window && w.alive());
        self.focus_history.insert(0, window.clone());
    }

    /// Alt+Tab or Alt+Shift+Tab: the switcher opens over every open window, most recently used
    /// first, or its selection moves one along. Nothing else changes until Alt is let go.
    pub fn cycle_windows(&mut self, forward: bool) {
        if self.lock.is_some() {
            return;
        }
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.step(forward);
            let selected = switcher.selected_index();
            let now = self.wall();
            if let Some(deck) = self.deck.as_mut() {
                deck.select(selected, now);
            }
            return;
        }
        let open = self.all_open_windows();
        self.focus_history.retain(|w| w.alive());
        // Every open window, including any never focused (one that opened behind a fullscreen
        // game, say), after the ones that have been.
        let mut windows: Vec<Window> = self
            .focus_history
            .iter()
            .filter(|window| open.contains(window))
            .cloned()
            .collect();
        windows.extend(
            open.into_iter()
                .filter(|window| window.alive() && !self.focus_history.contains(window)),
        );
        let now = self.wall();
        self.switcher =
            crate::switcher::Switcher::start(windows, forward, now, self.clock.reduced_motion);
        // The windows lift into a deck of glass panes once Alt has been held a moment; with
        // reduced motion the switcher's flat card stands in.
        self.deck = self
            .switcher
            .as_ref()
            .filter(|_| !self.clock.reduced_motion)
            .map(|switcher| {
                crate::deck::Deck::new(
                    switcher.windows().to_vec(),
                    switcher.selected_index(),
                    now + crate::switcher::APPEAR,
                )
            });
    }

    /// Ends the way out while it is still asking, as Esc on its card does.
    pub fn cancel_exit(&mut self) {
        tracing::info!("the way out was cancelled");
        self.exit = None;
        self.exit_record = None;
    }

    /// Alt let go after Alt+Tab, or a tile clicked: the switcher's choice is committed. A
    /// minimised window comes back out of the code rain; any other is gone to, with one slide to
    /// its workspace, and focused.
    pub fn finish_cycle(&mut self) {
        self.end_cycle(crate::switcher::End::Release);
    }

    /// Esc with Alt held, or the screen locking: the switcher goes and nothing changes.
    pub fn cancel_cycle(&mut self) {
        self.end_cycle(crate::switcher::End::Escape);
    }

    pub(super) fn end_cycle(&mut self, end: crate::switcher::End) {
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        // A deck that was showing flies its panes home; a quick flip never showed one.
        let now = self.wall();
        match self.deck.as_mut() {
            Some(deck) if deck.visible(now) && self.lock.is_none() => deck.release(now),
            _ => self.deck = None,
        }
        if self.lock.is_some() {
            return;
        }
        let rain = &self.rain;
        match switcher.finish(end, |window| rain.contains(window)) {
            None => tracing::info!("switcher cancelled"),
            Some(crate::switcher::Commit::Restore(window)) => self.restore(&window),
            Some(crate::switcher::Commit::Focus(window)) => {
                if !window.alive() {
                    return;
                }
                if let Some(workspace) = self.workspaces.find(&window) {
                    self.switch_workspace(workspace);
                }
                self.focus_window(&window);
            }
        }
    }
}

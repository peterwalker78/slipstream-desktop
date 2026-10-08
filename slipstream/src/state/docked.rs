//! The docked pane: which window hangs from the bar in front of the desktop, at what size, and
//! the keys and clicks that work it (`dock.rs` has where it rests and how it flies).
//!
//! A docked window is on no workspace and in no stream: it is a third place a window can be. It
//! is mapped on the screen the streams are on, above every other window there, whichever
//! workspace is shown, and the focus and tiling keys pass it by.

use super::*;
use crate::dock::{self, Docked, Flight, Size, Stop};

impl Slipstream {
    pub fn is_docked(&self, window: &Window) -> bool {
        self.dock
            .as_ref()
            .is_some_and(|docked| docked.window == *window)
    }

    /// Where the pane rests at `size`, in the space's coordinates: in the tiling area of the
    /// screen the streams are on.
    pub fn dock_rect(&self, size: Size) -> Option<Rect> {
        self.screen_area(0).map(|area| dock::rect(area, size))
    }

    /// Where the docked pane rests now.
    pub fn docked_rect(&self) -> Option<Rect> {
        self.dock_rect(self.dock.as_ref()?.size)
    }

    /// Super+W: the window with the keyboard goes up to the bar, taking the place of any docked
    /// there already. Once it is there, its next size.
    pub fn dock_focused(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let Some(window) = self.focused_window() else {
            // Nothing has the keyboard: it goes into the pane, if there is one.
            if let Some(window) = self.docked_on_show() {
                self.focus_window(&window);
            }
            return;
        };
        match self.dock.as_ref().map(|docked| docked.size) {
            Some(size) if self.is_docked(&window) => self.dock_window(&window, size.next()),
            _ => self.dock_window(&window, self.dock_size),
        }
    }

    /// Docks `window`, which is on a workspace, at `size`, or brings the pane it already has to
    /// `size`. Whatever was docked before comes down beside where `window` was.
    fn dock_window(&mut self, window: &Window, size: Size) {
        let Some(rect) = self.dock_rect(size) else {
            return;
        };
        let now = self.clock.tick();
        let from = if self.is_docked(window) {
            self.docked_rect().map(Stop::at_bar)
        } else {
            if self.workspaces.find(window).is_none() {
                return;
            }
            let from = self
                .motion
                .frame(window, now)
                .map(|frame| Stop::among(frame.rect));
            self.come_down(Some(window));
            self.fullscreen.retain(|held| held != window);
            self.take_off_workspace(window);
            from
        };
        let flight = from
            .filter(|_| !self.clock.reduced_motion)
            .map(|from| Flight::new(from, Stop::at_bar(rect), now));
        tracing::info!(window = logged_app(window), ?size, "docked to the bar");
        self.dock_size = size;
        self.dock = Some(Docked {
            window: window.clone(),
            size,
            flight,
            seated: flight.map(|flight| flight.end()),
        });
        // It is where it is going at once; the flight only paints the way there.
        self.motion.place(window, rect, now);
        self.motion.jump(window, rect);
        self.motion.fade(window, 1.0, now, 0.0);
        self.retile();
    }

    /// Super+Shift+W: the docked window comes down from the bar and is a standard window again,
    /// on the workspace on screen, with the keyboard.
    pub fn undock(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let beside = self.focused_window();
        let Some((window, pane)) = self.come_down(beside.as_ref()) else {
            return;
        };
        self.retile();
        // Its place among the windows is known now: the pane flies there, and the window is
        // there already.
        if !self.clock.reduced_motion
            && let Some(to) = self.motion.target(&window)
        {
            let now = self.clock.tick();
            self.dock_leaving = Some((
                window.clone(),
                Flight::new(Stop::at_bar(pane), Stop::among(to), now),
            ));
            self.motion.jump(
                &window,
                Rect {
                    x: to[0].round() as i32,
                    y: to[1].round() as i32,
                    w: to[2].round() as i32,
                    h: to[3].round() as i32,
                },
            );
        }
        self.focus_window(&window);
    }

    /// Takes the docked window off the bar and puts it on the workspace on screen, beside
    /// `beside`, where it still stands at its pane's place until the next retile moves it. Says
    /// which window it was and where its pane hung.
    pub(super) fn come_down(&mut self, beside: Option<&Window>) -> Option<(Window, Rect)> {
        let pane = self.docked_rect();
        let docked = self.dock.take()?;
        let window = docked.window;
        let screen = self.screen_rect(0)?;
        let area = self.output_area().unwrap_or(screen);
        let pane = pane.unwrap_or(area);
        let active = self.active_workspace();
        self.motion.jump(&window, pane);
        if crate::floating::opens_floating(&window) {
            let size = crate::floating::own_size(&window);
            self.workspaces
                .get_mut(active)
                .floating
                .add(window.clone(), size, None, area);
        } else {
            let beside = beside.filter(|beside| **beside != window);
            self.workspaces.insert(active, window.clone(), beside, area);
        }
        tracing::info!(window = logged_app(&window), "undocked from the bar");
        Some((window, pane))
    }

    /// A click on the pane's tab in the bar: the keyboard goes into the pane, or from it back to
    /// the windows.
    pub fn dock_clicked(&mut self) {
        let Some(window) = self.docked_on_show() else {
            return;
        };
        if self.focused_window().as_ref() == Some(&window) {
            self.restore_focus();
        } else {
            self.focus_window(&window);
        }
    }

    /// A window that has closed is no longer docked, nor on its way down.
    pub(super) fn forget_docked(&mut self, window: &Window) {
        if self.is_docked(window) {
            self.dock = None;
        }
        if self
            .dock_leaving
            .as_ref()
            .is_some_and(|(leaving, _)| leaving == window)
        {
            self.dock_leaving = None;
        }
    }

    /// Whether the pane is on show: not while a fullscreen window has the screen it lives on.
    fn dock_shown(&self) -> bool {
        self.screens
            .get(0)
            .is_some_and(|screen| !self.fullscreen_on(screen.workspace))
    }

    /// The window `retile` keeps mapped although no workspace on screen holds it.
    pub fn docked_on_show(&self) -> Option<Window> {
        self.dock
            .as_ref()
            .filter(|_| self.dock_shown())
            .map(|docked| docked.window.clone())
    }

    /// Places the docked window where its pane rests, at that size, above everything else on its
    /// screen. Part of every retile, so the pane follows its screen and the streams.
    pub(super) fn place_docked(&mut self, now: f64) {
        let (Some(window), Some(rect)) = (self.docked_on_show(), self.docked_rect()) else {
            return;
        };
        configure(&window, size_for_tile(rect, min_size(&window)), false);
        self.motion.place(&window, rect, now);
        if self.space.element_location(&window).is_some() {
            self.space.relocate_element(&window, (rect.x, rect.y));
        } else {
            self.space
                .map_element(window.clone(), (rect.x, rect.y), false);
        }
        self.space.raise_element(&window, false);
    }
}

//! Minimised windows: pouring them into the streams at the edge and bringing them back, and the
//! ghost a closing window leaves.

use super::*;

impl Slipstream {
    /// The app IDs and X11 classes of every open window, for the explorer's running dots.
    /// Every window there is: on any workspace, docked to the bar, or minimised into the code
    /// rain.
    pub fn all_open_windows(&self) -> Vec<Window> {
        self.workspaces
            .all_windows()
            .into_iter()
            .chain(self.dock.iter().map(|docked| docked.window.clone()))
            .chain(self.rain.streams.iter().map(|stream| stream.window.clone()))
            .collect()
    }

    pub fn running_apps(&self) -> Vec<String> {
        self.all_open_windows()
            .iter()
            .filter(|window| window.alive())
            .filter_map(window_app_id)
            .collect()
    }

    /// Keeps the picture of a window that is going, where it was drawn, to fade out there. Only
    /// a window on screen leaves one, among the tiles or docked to the bar: not one minimised,
    /// on a workspace out of sight, under the lock or in bullet time's overview.
    pub(super) fn leave_a_ghost(&mut self, window: &Window) {
        let picture = self
            .pictures
            .iter()
            .position(|(drawn, _)| drawn == window)
            .map(|at| self.pictures.swap_remove(at).1);
        let Some(picture) = picture else {
            return;
        };
        // The docked pane is on no workspace, and on screen whichever is shown.
        let on_screen = self.docked_on_show().as_ref() == Some(window)
            || self
                .workspaces
                .find(window)
                .is_some_and(|index| self.screens.showing(index).is_some());
        if !on_screen || self.lock.is_some() || self.bullet.is_some() || self.idle.is_faded() {
            return;
        }
        let now = self.clock.tick();
        let rect = match self.fitted.iter().find(|(fitted, _)| fitted == window) {
            Some((_, drawn)) => Some(*drawn),
            None => self.motion.frame(window, now).map(|frame| {
                let [x, y, w, h] = frame.rect;
                // The window's size as it was drawn: by now a closing client may report none.
                let own = picture.own();
                let (w, h) = if frame.moving {
                    (w, h)
                } else {
                    (own.w as f64, own.h as f64)
                };
                Rectangle::new((x, y).into(), (w, h).into())
            }),
        };
        let Some(rect) = rect else {
            return;
        };
        let reduced = self.clock.reduced_motion;
        let ghost = if self.settings.motion.close_into_rain && !reduced {
            crate::ghost::Ghost::falling(picture, rect, now, self.settings.motion.effects)
        } else {
            crate::ghost::Ghost::new(picture, rect, now, reduced)
        };
        self.ghosts.push(ghost);
    }

    /// Takes out any stream whose window has gone without the compositor hearing of it, so a
    /// closed app never leaves a column of rain behind or comes back as an empty tile. Closing
    /// normally already goes through `remove_window`; this is the backstop, run once a pass.
    pub fn drop_dead_streams(&mut self) {
        let dead: Vec<Window> = self
            .rain
            .streams
            .iter()
            .filter(|stream| !stream.window.alive())
            .map(|stream| stream.window.clone())
            .collect();
        for window in dead {
            tracing::info!("a minimised window had gone; its stream goes too");
            self.remove_window(&window);
        }
    }

    /// Super+M: the focused window pours into a stream of code rain on the right.
    /// Super+H: every window on this workspace into the code rain, and the same windows back on
    /// the next press. Windows' "minimise all", which Super+M only does one at a time.
    ///
    /// The list is kept rather than emptying the rain on the way back: anything minimised on its
    /// own before stays where it was put.
    pub fn hide_all(&mut self) {
        if self.lock.is_some() {
            return;
        }
        // Anything hidden that is still in the rain comes back, and the list is spent either way:
        // windows closed while hidden simply drop out of it.
        let coming_back: Vec<Window> = self
            .hidden
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|window| window.alive() && self.rain.contains(window))
            .collect();
        if !coming_back.is_empty() {
            tracing::info!(
                windows = coming_back.len(),
                "bringing the hidden windows back"
            );
            self.restore_all(&coming_back);
            return;
        }
        let active = self.active_workspace();
        let windows = self.workspaces.get(active).windows();
        if windows.is_empty() {
            return;
        }
        tracing::info!(windows = windows.len(), "hiding every window");
        // Every window takes its stream first, then they all pour into their own: aiming each
        // one as it went would send the first at a column the rest had not been counted into,
        // and would retile the workspace once per window on the way.
        let going: Vec<Window> = windows
            .into_iter()
            .filter(|window| self.take_into_rain(window))
            .collect();
        for window in &going {
            self.pour_into_stream(window, POUR_ALL);
        }
        self.retile();
        self.restore_focus();
        self.hidden = Some(going);
    }

    pub fn minimise_focused(&mut self) {
        if let Some(window) = self.focused_window() {
            // The docked window leaves the bar for a stream of its own, from where its pane is.
            if self.is_docked(&window) {
                self.come_down(None);
            }
            self.minimise(&window);
        }
    }

    /// Takes `window` out of its layout into the code rain. The app keeps running.
    pub fn minimise(&mut self, window: &Window) {
        if !self.take_into_rain(window) {
            return;
        }
        self.pour_into_stream(window, POUR);
        self.retile();
        self.restore_focus();
    }

    /// Takes `window` off its workspace and gives it a stream, without moving anything yet.
    ///
    /// Splitting this from the pouring is what lets a screenful go at once: every window gets its
    /// stream first, so each is then aimed at the column it will really have, and the tiling area
    /// gives up its width once rather than once per window. Returns whether it went.
    pub(super) fn take_into_rain(&mut self, window: &Window) -> bool {
        let Some(scale) = self.output_scale() else {
            return false;
        };
        if self.rain.contains(window) || self.workspaces.find(window).is_none() {
            return false;
        }
        self.fullscreen.retain(|held| held != window);
        self.take_off_workspace(window);
        let name = self.stream_name(window);
        let icon_px = crate::rain::icon_px(scale);
        let icon = window_app_id(window).and_then(|id| self.explorer.app_icon(&id, icon_px));
        let pid = self.window_pid(window);
        let now = self.clock.tick();
        self.rain.add(window.clone(), name, icon, pid, now);
        tracing::info!(
            window = logged_app(window),
            screen = self.screens.get(0).map(|screen| screen.output.name()),
            "minimised to code rain"
        );
        true
    }

    /// Sends `window` into its own stream over `over` seconds, fading over the same time so it is
    /// still there when it lands: a window that has gone before it arrives reads as the desktop
    /// clearing, not as that window going into that stream.
    pub(super) fn pour_into_stream(&mut self, window: &Window, over: f64) {
        let (Some(screen), Some(index)) = (self.screen_rect(0), self.rain.index_of(window)) else {
            return;
        };
        let now = self.clock.tick();
        let column = Rain::column(index, screen, bar::HEIGHT);
        self.motion.place_over(window, column, now, over);
        self.motion.fade_out_landing(window, now, over);
    }

    /// Super+Shift+M: the most recently minimised window comes back.
    pub fn restore_latest(&mut self) {
        if let Some(window) = self.rain.streams.last().map(|stream| stream.window.clone()) {
            self.restore(&window);
        }
    }

    /// A window condenses out of its stream, beside the window you're using on the workspace on
    /// screen, and takes focus.
    pub fn restore(&mut self, window: &Window) {
        self.restore_all(std::slice::from_ref(window));
    }

    /// Several windows condense out of their streams together: each starts from the stream it is
    /// in now, before any of them have left, and the workspace retiles once around all of them
    /// rather than once per window.
    pub fn restore_all(&mut self, windows: &[Window]) {
        let Some(screen) = self.screen_rect(0) else {
            return;
        };
        // Where each one's stream sits while they are all still in the rain. Taken first,
        // because every window that leaves shifts the streams still to its left.
        let from: Vec<(Window, Rect)> = windows
            .iter()
            .filter_map(|window| {
                let index = self.rain.index_of(window)?;
                Some((window.clone(), Rain::column(index, screen, bar::HEIGHT)))
            })
            .collect();
        if from.is_empty() {
            return;
        }
        let area = self.output_area().unwrap_or(screen);
        let now = self.clock.tick();
        let active = self.active_workspace();
        let beside = self.focused_window();
        for (window, column) in &from {
            self.rain.remove(window);
            self.motion.jump(window, *column);
            self.motion.fade_in_leaving(window, now, 0.26);
            if crate::floating::opens_floating(window) {
                let size = crate::floating::own_size(window);
                self.workspaces
                    .get_mut(active)
                    .floating
                    .add(window.clone(), size, None, area);
            } else {
                self.workspaces
                    .insert(active, window.clone(), beside.as_ref(), area);
            }
            tracing::info!(window = logged_app(window), "restored from code rain");
        }
        self.retile();
        if let Some((window, _)) = from.last() {
            self.focus_window(&window.clone());
        }
    }

    /// The minimised window whose stream is under `pos`. The rain is drawn on the leftmost
    /// screen only, so nothing on any other screen is a stream.
    pub fn rain_window_at(&self, pos: Point<f64, Logical>) -> Option<Window> {
        let showing = self.screens.get(0).map(|screen| screen.workspace)?;
        if self.fullscreen_on(showing) {
            return None;
        }
        let index = self
            .rain
            .stream_hit(pos.x, pos.y, self.screen_rect(0)?, bar::HEIGHT)?;
        Some(self.rain.streams[index].window.clone())
    }

    /// The process behind a window: the X11 client's own claim, or the Wayland socket's peer.
    /// Its app's meter is read from there (`usage::Meter`).
    pub fn window_pid(&self, window: &Window) -> Option<u32> {
        if let Some(surface) = window.x11_surface() {
            return surface.pid();
        }
        let client = window.toplevel()?.wl_surface().client()?;
        client
            .get_credentials(&self.display_handle)
            .ok()
            .map(|credentials| credentials.pid as u32)
    }
}

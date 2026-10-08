//! The pinned pane: which minimised window is kept in view in front of the desktop, at what
//! size, and the keys and clicks that work it (`pin.rs` has where it rests and how it flies).
//!
//! A pinned window stays minimised as far as the workspaces are concerned: it keeps its stream,
//! which goes on showing its load, and belongs to no workspace, so the focus and tiling keys
//! pass it by. It is mapped on the screen the streams are on, above every other window there.

use super::*;
use crate::pin::{self, Flight, Pinned, Size, Stop};

impl Slipstream {
    pub fn is_pinned(&self, window: &Window) -> bool {
        self.pin
            .as_ref()
            .is_some_and(|pinned| pinned.window == *window)
    }

    /// The tiling area of the screen the streams are on, which is where the pane lives.
    fn pin_area(&self) -> Option<Rect> {
        self.screen_area(0)
    }

    /// Where the pane rests at `size`, in the space's coordinates.
    pub fn pin_rect(&self, size: Size) -> Option<Rect> {
        self.pin_area().map(|area| pin::rect(area, size))
    }

    /// The pane in the mouth of `window`'s stream.
    fn pin_in_stream(&self, window: &Window) -> Option<Stop> {
        let index = self.rain.index_of(window)?;
        let button = Rain::button(index, self.screen_rect(0)?, bar::HEIGHT);
        Some(Stop::in_stream(button, self.pin_rect(Size::Quarter)?))
    }

    /// Super+W: the keyboard goes into the pinned pane, and back to where it was. With nothing
    /// pinned, the window minimised last is pinned first.
    pub fn pin_keyboard(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let Some(window) = self.pin.as_ref().map(|pinned| pinned.window.clone()) else {
            if let Some(window) = self.rain.streams.last().map(|stream| stream.window.clone()) {
                self.pin_window(&window, Size::Small);
                self.focus_window(&window);
            }
            return;
        };
        if self.focused_window().as_ref() == Some(&window) {
            self.restore_focus();
        } else if self.pin_shown() {
            self.focus_window(&window);
        }
    }

    /// Super+Shift+W: the pane's next size. The window minimised last is pinned small, then
    /// comes forward to a quarter of the screen, then goes back into its stream.
    pub fn pin_next_size(&mut self) {
        if self.lock.is_some() {
            return;
        }
        match self
            .pin
            .as_ref()
            .map(|pinned| (pinned.window.clone(), pinned.size))
        {
            None => {
                if let Some(window) = self.rain.streams.last().map(|stream| stream.window.clone()) {
                    self.pin_window(&window, Size::Small);
                }
            }
            Some((window, size)) => match size.next() {
                Some(next) => self.pin_window(&window, next),
                None => self.unpin(),
            },
        }
    }

    /// A click on a stream's button: its window is pinned, or put away again if it is the one
    /// pinned already.
    pub fn pin_clicked(&mut self, window: &Window) {
        if self.is_pinned(window) {
            self.unpin();
        } else {
            self.pin_window(window, Size::Small);
        }
    }

    /// Pins `window`, which is minimised, at `size`, or brings the pane it already has to
    /// `size`. Whatever was pinned before goes back into its own stream.
    pub fn pin_window(&mut self, window: &Window, size: Size) {
        if !self.rain.contains(window) {
            return;
        }
        let (Some(rect), Some(out)) = (self.pin_rect(size), self.pin_in_stream(window)) else {
            return;
        };
        let pinned = self
            .pin
            .as_ref()
            .map(|pinned| (pinned.window == *window, pinned.size));
        let from = match pinned {
            Some((true, size)) => self.pin_rect(size).map_or(out, Stop::resting),
            Some((false, _)) => {
                self.unpin();
                out
            }
            None => out,
        };
        let now = self.clock.tick();
        let flight =
            (!self.clock.reduced_motion).then(|| Flight::new(from, Stop::resting(rect), now));
        tracing::info!(window = logged_app(window), ?size, "pinned to the screen");
        self.pin = Some(Pinned {
            window: window.clone(),
            size,
            flight,
        });
        // It is where it is going at once; the flight only paints the way there.
        self.motion.place(window, rect, now);
        self.motion.jump(window, rect);
        self.motion.fade(window, 1.0, now, 0.0);
        self.retile();
    }

    /// The pinned pane goes back into its stream, and the keyboard, if it was in the pane, back
    /// to the workspace.
    pub fn unpin(&mut self) {
        let Some(pinned) = self.pin.take() else {
            return;
        };
        let window = pinned.window;
        let had_keyboard = self.focused_window().as_ref() == Some(&window);
        let now = self.clock.tick();
        self.pin_leaving = None;
        if !self.clock.reduced_motion
            && let (Some(rect), Some(home)) =
                (self.pin_rect(pinned.size), self.pin_in_stream(&window))
        {
            self.pin_leaving = Some((window.clone(), Flight::new(Stop::resting(rect), home, now)));
        }
        // As any other minimised window is kept: in its stream's column, not drawn.
        if let (Some(screen), Some(index)) = (self.screen_rect(0), self.rain.index_of(&window)) {
            self.motion
                .jump(&window, Rain::column(index, screen, bar::HEIGHT));
        }
        self.motion.fade(&window, 0.0, now, 0.0);
        tracing::info!(window = logged_app(&window), "unpinned, back in its stream");
        self.retile();
        if had_keyboard {
            self.restore_focus();
        }
    }

    /// A window that has closed, or come back to a workspace, is no longer pinned.
    pub(super) fn forget_pinned(&mut self, window: &Window) {
        if self.is_pinned(window) {
            self.pin = None;
        }
        if self
            .pin_leaving
            .as_ref()
            .is_some_and(|(leaving, _)| leaving == window)
        {
            self.pin_leaving = None;
        }
    }

    /// Whether the pane is on show: not while a fullscreen window has the screen it lives on.
    fn pin_shown(&self) -> bool {
        self.screens
            .get(0)
            .is_some_and(|screen| !self.fullscreen_on(screen.workspace))
    }

    /// The window `retile` keeps mapped although no workspace on screen holds it.
    pub(super) fn pinned_on_show(&self) -> Option<Window> {
        self.pin
            .as_ref()
            .filter(|_| self.pin_shown())
            .map(|pinned| pinned.window.clone())
    }

    /// Places the pinned window where its pane rests, at the size it rests at, above everything
    /// else on its screen. Part of every retile, so the pane follows its screen and the streams.
    pub(super) fn place_pinned(&mut self, now: f64) {
        let Some(window) = self.pinned_on_show() else {
            return;
        };
        let Some(size) = self.pin.as_ref().map(|pinned| pinned.size) else {
            return;
        };
        let (Some(rect), Some(full)) = (self.pin_rect(size), self.pin_rect(Size::Quarter)) else {
            return;
        };
        // The app is always asked for the quarter. Small, the same picture is drawn smaller, so
        // changing size never makes the app lay itself out again.
        configure(&window, size_for_tile(full, min_size(&window)), false);
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

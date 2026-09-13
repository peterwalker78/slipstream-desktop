//! Arranging tiles with the pointer: dragging the gap between two tiles resizes them, and
//! Super+drag swaps two windows.
//!
//! Both are pointer grabs. While one is held no client has the pointer, so the press, the motion
//! and the release never reach an app. A grab always ends: on the release, on Esc, when the
//! screen locks and on a switch to another virtual terminal, so the pointer is never left caught.
//!
//! Smithay holds the pointer's lock while it calls a grab, so nothing here, nor anything it calls
//! on `Slipstream`, may ask the pointer for anything.

use smithay::{
    backend::input::InputTime,
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};

use crate::{Slipstream, focus::KeyboardFocus, layout::Divider, state::min_size};

/// A drag under way.
#[derive(Debug)]
pub enum Drag {
    /// A gap on workspace `workspace`, whose split had ratio `from` when the drag began.
    Gap {
        workspace: usize,
        path: Vec<bool>,
        from: f32,
    },
    /// A window being dragged onto another tile: the one under the pointer, and whether Esc
    /// called it off.
    Swap {
        window: Window,
        over: Option<Window>,
        cancelled: bool,
    },
}

/// The pointer's cursor over a divider: side by side, the gap moves left and right.
fn resize_cursor(divider: &Divider) -> CursorIcon {
    if divider.vertical {
        CursorIcon::ColResize
    } else {
        CursorIcon::RowResize
    }
}

impl Slipstream {
    /// The gap under `pos` that the pointer can take hold of, with its workspace: only on the
    /// plain desktop, with nothing of Slipstream's own over it, and not while a window fills the
    /// screen or the tiling area, or gravity arranges the workspace.
    pub fn divider_at(&self, pos: Point<f64, Logical>) -> Option<(usize, Divider)> {
        if self.pointer_blocked() {
            return None;
        }
        let index = self.screen_at(pos)?;
        let workspace = self.screens.get(index)?.workspace;
        let ws = self.workspaces.get(workspace);
        if self.fullscreen_on(workspace) || ws.maximised.is_some() || ws.gravity.is_on() {
            return None;
        }
        let area = self.screen_area(index)?;
        ws.layout
            .dividers(area, &min_size)
            .into_iter()
            .find(|divider| divider.contains(pos.x, pos.y))
            .map(|divider| (workspace, divider))
    }

    /// The pointer moved to `pos` with no grab of its own: over a gap it shows the resize cursor.
    /// Whether it is over one, so no client gets the pointer there.
    pub fn hover_divider(&mut self, pos: Point<f64, Logical>) -> bool {
        if self.drag.is_some() {
            return true;
        }
        let over = self.divider_at(pos);
        self.set_cursor_override(over.as_ref().map(|(_, divider)| resize_cursor(divider)));
        over.is_some()
    }

    /// Slipstream's own cursor in place of the one clients set, or `None` to give it back.
    pub fn set_cursor_override(&mut self, icon: Option<CursorIcon>) {
        if self.cursor_override != icon {
            tracing::info!("cursor {}", icon.map_or("from the app", |icon| icon.name()));
            self.cursor_override = icon;
        }
    }

    /// A left press at `pos`: on a gap it starts a resize, and with Super held on a tiled window
    /// it starts a swap. Whether the press was taken, in which case a grab now has the pointer
    /// (or, under gravity, a toast said why not) and no client is to get it.
    pub fn start_drag(
        &mut self,
        pos: Point<f64, Logical>,
        button: u32,
        serial: Serial,
        super_held: bool,
    ) -> bool {
        const BTN_LEFT: u32 = 0x110;
        if button != BTN_LEFT || self.lock.is_some() {
            return false;
        }
        let pointer = self.seat.get_pointer().unwrap();
        if pointer.is_grabbed() {
            return false;
        }
        let start = GrabStartData {
            focus: None,
            button,
            location: pos,
        };
        if let Some((workspace, divider)) = self.divider_at(pos) {
            let Some(from) = self
                .workspaces
                .get(workspace)
                .layout
                .ratio_of(&divider.path)
            else {
                return false;
            };
            tracing::info!(vertical = divider.vertical, from, "dragging a gap");
            self.drag = Some(Drag::Gap {
                workspace,
                path: divider.path.clone(),
                from,
            });
            self.set_cursor_override(Some(resize_cursor(&divider)));
            pointer.set_grab(self, GapGrab { start, divider }, serial, Focus::Clear);
            return true;
        }
        if !super_held || self.pointer_blocked() {
            return false;
        }
        let Some((window, _)) = self.window_under(pos) else {
            return false;
        };
        let Some(workspace) = self.workspaces.find(&window) else {
            return false;
        };
        if self.fullscreen.as_ref() == Some(&window) {
            return false;
        }
        if self.workspaces.get(workspace).gravity.is_on() {
            self.show_toast(
                "Gravity on",
                "Super+PgUp and Super+PgDn move windows here. Super+T goes back to tiling.",
            );
            return true;
        }
        tracing::info!("dragging a window to swap it");
        self.focus_window(&window);
        self.drag = Some(Drag::Swap {
            window: window.clone(),
            over: None,
            cancelled: false,
        });
        self.set_cursor_override(Some(CursorIcon::Grabbing));
        pointer.set_grab(self, SwapGrab { start }, serial, Focus::Clear);
        true
    }

    /// The gap being dragged, with the pointer `at` along its axis: its split takes that ratio
    /// in the tree now, and the tiles follow at the next frame.
    fn drag_gap(&mut self, workspace: usize, divider: &Divider, at: f64) {
        let ratio = divider.ratio_at(at);
        let layout = &mut self.workspaces.get_mut(workspace).layout;
        if layout.ratio_of(&divider.path) != Some(ratio) {
            layout.set_ratio(&divider.path, ratio);
            self.drag_retile_due = true;
        }
    }

    /// Before a frame is drawn: the tiles follow a dragged gap, so clients are configured at most
    /// once a frame however fast the pointer reports.
    pub fn retile_for_drag(&mut self) {
        if std::mem::take(&mut self.drag_retile_due) {
            self.retile();
        }
    }

    /// The window being dragged is over `pos`: the tile there on its workspace is the one it
    /// would swap with.
    fn drag_over(&mut self, pos: Point<f64, Logical>) {
        let under = self.window_under(pos).map(|(window, _)| window);
        let Some(Drag::Swap { window, .. }) = &self.drag else {
            return;
        };
        let home = self.workspaces.find(window);
        let target = under.filter(|other| other != window && self.workspaces.find(other) == home);
        if let Some(Drag::Swap { over, .. }) = &mut self.drag {
            *over = target;
        }
    }

    /// The dragged window was let go: over another tile, the two swap places, and the keyboard
    /// stays with the one dragged.
    fn drop_window(&mut self) {
        let Some(Drag::Swap {
            window,
            over: Some(over),
            cancelled: false,
        }) = self.drag.take()
        else {
            return;
        };
        let Some(workspace) = self.workspaces.find(&window) else {
            return;
        };
        if self
            .workspaces
            .get_mut(workspace)
            .layout
            .swap(&window, &over)
        {
            tracing::info!("swapped two tiles by dragging");
            self.retile();
        }
    }

    /// A grab has ended, however it ended: the tiles settle where the drag left them, and the
    /// cursor is the app's again.
    fn drag_ended(&mut self) {
        match self.drag.take() {
            Some(Drag::Gap {
                workspace, path, ..
            }) => {
                self.drag_retile_due = false;
                let ratio = self.workspaces.get(workspace).layout.ratio_of(&path);
                tracing::info!(?ratio, "gap drag ended");
                self.retile();
            }
            Some(Drag::Swap { .. }) => tracing::info!("window drag ended"),
            None => {}
        }
        self.set_cursor_override(None);
    }

    /// Esc during a drag: a gap goes back to where it started, and a window stays where it was.
    pub fn cancel_drag(&mut self) {
        match &mut self.drag {
            Some(Drag::Gap {
                workspace,
                path,
                from,
            }) => {
                let (workspace, path, from) = (*workspace, path.clone(), *from);
                self.workspaces
                    .get_mut(workspace)
                    .layout
                    .set_ratio(&path, from);
            }
            Some(Drag::Swap { cancelled, .. }) => *cancelled = true,
            None => return,
        }
        tracing::info!("drag called off");
        self.end_drag();
    }

    /// Ends a drag under way, if there is one: for Esc, a switch to another virtual terminal,
    /// and anything else that must leave the pointer free.
    pub fn end_drag(&mut self) {
        if self.drag.is_none() {
            return;
        }
        let pointer = self.seat.get_pointer().unwrap();
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), InputTime::now());
        // Another grab had replaced it and already ended it, or nothing was holding it.
        if self.drag.is_some() {
            self.drag_ended();
        }
    }

    /// The tile a dragged window would swap with, for its outline.
    pub fn swap_target(&self) -> Option<&Window> {
        match &self.drag {
            Some(Drag::Swap {
                over: Some(over),
                cancelled: false,
                ..
            }) => Some(over),
            _ => None,
        }
    }
}

/// Dragging the gap between two tiles.
pub struct GapGrab {
    start: GrabStartData<Slipstream>,
    divider: Divider,
}

/// Dragging a window onto another tile with Super held.
pub struct SwapGrab {
    start: GrabStartData<Slipstream>,
}

/// Gives the pointer back once every button is up, after `released` has had its say.
fn on_button(
    grab: &mut dyn PointerGrab<Slipstream>,
    data: &mut Slipstream,
    handle: &mut PointerInnerHandle<'_, Slipstream>,
    event: &ButtonEvent,
    released: impl FnOnce(&mut Slipstream),
) {
    // With no focus, the button reaches no client; Smithay still counts it as held.
    handle.button(data, event);
    if handle.current_pressed().is_empty() {
        released(data);
        handle.unset_grab(grab, data, event.serial, event.time, true);
    }
}

macro_rules! pointer_grab {
    ($grab:ty, motion: $motion:expr, released: $released:expr) => {
        impl PointerGrab<Slipstream> for $grab {
            fn motion(
                &mut self,
                data: &mut Slipstream,
                handle: &mut PointerInnerHandle<'_, Slipstream>,
                _focus: Option<(KeyboardFocus, Point<f64, Logical>)>,
                event: &MotionEvent,
            ) {
                handle.motion(data, None, event);
                let motion: fn(&mut Self, &mut Slipstream, Point<f64, Logical>) = $motion;
                motion(self, data, event.location);
            }

            fn relative_motion(
                &mut self,
                data: &mut Slipstream,
                handle: &mut PointerInnerHandle<'_, Slipstream>,
                _focus: Option<(KeyboardFocus, Point<f64, Logical>)>,
                event: &RelativeMotionEvent,
            ) {
                handle.relative_motion(data, None, event);
            }

            fn button(
                &mut self,
                data: &mut Slipstream,
                handle: &mut PointerInnerHandle<'_, Slipstream>,
                event: &ButtonEvent,
            ) {
                let released: fn(&mut Slipstream) = $released;
                on_button(self, data, handle, event, released);
            }

            // The wheel is swallowed: no client has the pointer while it's held.
            fn axis(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _details: AxisFrame,
            ) {
            }

            fn frame(
                &mut self,
                data: &mut Slipstream,
                handle: &mut PointerInnerHandle<'_, Slipstream>,
            ) {
                handle.frame(data);
            }

            fn gesture_swipe_begin(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GestureSwipeBeginEvent,
            ) {
            }

            fn gesture_swipe_update(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GestureSwipeUpdateEvent,
            ) {
            }

            fn gesture_swipe_end(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GestureSwipeEndEvent,
            ) {
            }

            fn gesture_pinch_begin(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GesturePinchBeginEvent,
            ) {
            }

            fn gesture_pinch_update(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GesturePinchUpdateEvent,
            ) {
            }

            fn gesture_pinch_end(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GesturePinchEndEvent,
            ) {
            }

            fn gesture_hold_begin(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GestureHoldBeginEvent,
            ) {
            }

            fn gesture_hold_end(
                &mut self,
                _data: &mut Slipstream,
                _handle: &mut PointerInnerHandle<'_, Slipstream>,
                _event: &GestureHoldEndEvent,
            ) {
            }

            fn start_data(&self) -> &GrabStartData<Slipstream> {
                &self.start
            }

            fn unset(&mut self, data: &mut Slipstream) {
                data.drag_ended();
            }
        }
    };
}

pointer_grab!(
    GapGrab,
    motion: |grab, data, at| {
        let Some(Drag::Gap { workspace, .. }) = data.drag else {
            return;
        };
        let along = if grab.divider.vertical { at.x } else { at.y };
        data.drag_gap(workspace, &grab.divider, along);
    },
    released: |_| {}
);

pointer_grab!(
    SwapGrab,
    motion: |_, data, at| data.drag_over(at),
    released: |data| data.drop_window()
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, Rect};

    #[test]
    fn a_drag_snaps_near_thirds() {
        let divider = Divider {
            strip: Rect {
                x: 495,
                y: 16,
                w: 10,
                h: 568,
            },
            vertical: true,
            split: Rect {
                x: 0,
                y: 0,
                w: 900,
                h: 600,
            },
            path: Vec::new(),
            min: (0, 0),
        };
        assert_eq!(divider.ratio_at(310.0), 1.0 / 3.0, "within 0.02 of a third");
        assert_eq!(divider.ratio_at(590.0), 2.0 / 3.0);
        assert_eq!(divider.ratio_at(455.0), 0.5);
        assert!(
            (divider.ratio_at(360.0) - 0.4).abs() < 1e-6,
            "0.4 is left alone"
        );
        assert_eq!(divider.ratio_at(-50.0), layout::MIN_RATIO);
        assert_eq!(divider.ratio_at(2000.0), layout::MAX_RATIO);
    }
}

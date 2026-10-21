//! The caret of whatever is being typed into, for the zoom to follow.
//!
//! Apps say where their caret is through `zwp_text_input_v3`, which is meant for input methods:
//! Smithay passes the rectangle on to the input method and keeps it nowhere else. So while the
//! screen is zoomed, each text input of the focused app is watched, its requests read on their
//! way through to Smithay, which handles them as before. Apps report a caret only to a text input
//! that has been entered, which happens only with an input method running, so without one
//! Slipstream stands in as it for as long as the zoom is on. It never types anything.

use std::{
    os::fd::OwnedFd,
    sync::{Arc, Mutex},
};

use smithay::{
    reexports::{
        wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3,
        wayland_server::{
            Resource,
            backend::{
                ClientId, Handle, ObjectData, ObjectId,
                protocol::{Argument, Message},
            },
        },
    },
    utils::{Logical, Rectangle},
    wayland::{input_method::InputMethodSeat, seat::WaylandFocus, text_input::TextInputSeat},
};

use crate::Slipstream;

/// One text input, watched: Smithay's handling of it, and the caret it has reported.
struct Watched {
    smithay: Arc<dyn ObjectData<Slipstream>>,
    caret: Mutex<Caret>,
}

#[derive(Default)]
struct Caret {
    /// Sent, and waiting for the app's commit.
    pending: Option<Rectangle<i32, Logical>>,
    /// The last committed. Only a change from it is a move: the first report, which comes as the
    /// zoom goes on or the app is switched to, leaves the pointer leading, and so does a commit
    /// that leaves the caret where it was.
    last: Option<Rectangle<i32, Logical>>,
}

impl ObjectData<Slipstream> for Watched {
    fn request(
        self: Arc<Self>,
        handle: &Handle,
        state: &mut Slipstream,
        client: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<Slipstream>>> {
        let id = msg.sender_id.clone();
        let moved = {
            let mut caret = self.caret.lock().unwrap();
            match (msg.opcode, msg.args.as_slice()) {
                (
                    zwp_text_input_v3::REQ_SET_CURSOR_RECTANGLE_OPCODE,
                    [
                        Argument::Int(x),
                        Argument::Int(y),
                        Argument::Int(w),
                        Argument::Int(h),
                    ],
                ) => {
                    caret.pending = Some(Rectangle::new((*x, *y).into(), (*w, *h).into()));
                    None
                }
                (zwp_text_input_v3::REQ_COMMIT_OPCODE, _) => {
                    let rect = caret.pending.take();
                    let moved = rect.filter(|rect| caret.last.is_some_and(|last| last != *rect));
                    caret.last = rect.or(caret.last);
                    moved
                }
                _ => None,
            }
        };
        // Smithay reads its own data off the object as it takes the request, so that goes back
        // on for the length of it.
        let _ = handle.set_object_data(id.clone(), self.smithay.clone());
        let child = self.smithay.clone().request(handle, state, client, msg);
        let _ = handle.set_object_data::<Slipstream>(id.clone(), self);
        if let Some(rect) = moved {
            state.caret_moved(&id, rect);
        }
        child
    }

    fn destroyed(
        self: Arc<Self>,
        handle: &Handle,
        state: &mut Slipstream,
        client: ClientId,
        object: ObjectId,
    ) {
        self.smithay
            .clone()
            .destroyed(handle, state, client, object);
    }
}

impl Slipstream {
    /// While the screen is zoomed, watches the focused app's text inputs for their caret. Called
    /// when the zoom changes and when the keyboard moves to another app.
    pub fn watch_carets(&mut self) {
        let zooming = self.zoom.is_zooming();
        let text_input = self.seat.text_input().clone();
        if zooming {
            let handle = self.display_handle.backend_handle();
            text_input.with_focused_text_input(|text_input, _| watch(&handle, text_input.id()));
        }
        // After the watching: standing in sends the focused app the `enter` that starts its
        // reports. A real input method is left to do its own job.
        if !self.seat.input_method().has_instance() {
            text_input.set_compositor_input_method(zooming);
        }
    }

    /// A watched text input's caret moved to `rect`, in its surface's coordinates.
    fn caret_moved(&mut self, from: &ObjectId, rect: Rectangle<i32, Logical>) {
        let Some(surface) = self.seat.text_input().focus() else {
            return;
        };
        if !surface.id().same_client_as(from) {
            return;
        }
        let Some(origin) = self.space.elements().find_map(|window| {
            (window.wl_surface().as_deref() == Some(&surface)).then(|| {
                self.space
                    .element_location(window)
                    .map(|at| at - window.geometry().loc)
            })?
        }) else {
            return;
        };
        let caret = Rectangle::new((origin + rect.loc).to_f64(), rect.size.to_f64());
        let middle = caret.loc + caret.size.downscale(2.0).to_point();
        let Some(screen) = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .find(|geo| geo.to_f64().contains(middle))
        else {
            return;
        };
        let (now, reduced) = (self.clock.wall(), self.clock.reduced_motion);
        let pointer = self.pointer_location();
        self.zoom
            .caret_moved(caret, screen.to_f64(), pointer, now, reduced);
        self.something_changed();
    }
}

/// Puts a watch on a text input, unless it has one.
fn watch(handle: &Handle, id: ObjectId) {
    let watched = handle
        .get_object_data_any(id.clone())
        .is_ok_and(|data| data.is::<Watched>());
    if watched {
        return;
    }
    let Ok(smithay) = handle.get_object_data::<Slipstream>(id.clone()) else {
        return;
    };
    let watched = Watched {
        smithay,
        caret: Mutex::default(),
    };
    let _ = handle.set_object_data::<Slipstream>(id, Arc::new(watched));
}

//! `wlr-virtual-pointer-unstable-v1`: a mouse made of software.
//!
//! This is how wayvnc, wlrctl and other remote-control tools move the pointer, click and scroll.
//! Without it a VNC viewer sees the screen and can type (`ime.rs` offers the virtual keyboard)
//! but its mouse does nothing.
//!
//! What a virtual pointer sends goes through the same paths as a real mouse: motion moves the one
//! pointer and wakes the UI, buttons are `button_event`, and the wheel is held back for the
//! overlays exactly as a real wheel is.
//!
//! **Unsandboxed clients only**, as the virtual keyboard is: a client that can click anywhere can
//! do anything the user can, so the global is shown only to a client whose process was proven,
//! when it connected, to be outside any sandbox (`ime::may_type`).

use std::sync::Mutex;

use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, InputTime},
    input::pointer::AxisFrame,
    output::Output,
    reexports::{
        wayland_protocols_wlr::virtual_pointer::v1::server::{
            zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
            zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            protocol::{wl_output::WlOutput, wl_pointer},
        },
    },
    utils::{Logical, Point},
};

use crate::state::Slipstream;

/// One wheel notch is this much scroll, as for a real wheel (`v120` steps are 120 to a notch).
const NOTCH: f64 = 15.0;

/// Which screen absolute motion is spread over, and the axis events waiting for their `frame`.
#[derive(Debug, Default)]
pub struct VirtualPointer {
    /// The screen the client named, if it did. Without one, absolute motion goes to the same
    /// screen a touchscreen would.
    output: Option<Output>,
    pending: Mutex<Pending>,
}

#[derive(Debug, Default)]
struct Pending {
    source: Option<AxisSource>,
    time: u32,
    /// Horizontal, then vertical.
    value: [Option<f64>; 2],
    discrete: [Option<i32>; 2],
    stop: [bool; 2],
}

impl Pending {
    fn any(&self) -> bool {
        self.value.iter().any(Option::is_some)
            || self.discrete.iter().any(Option::is_some)
            || self.stop.iter().any(|stop| *stop)
    }
}

fn index(axis: Axis) -> usize {
    match axis {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}

fn axis_of(axis: WEnum<wl_pointer::Axis>) -> Option<Axis> {
    match axis {
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(Axis::Horizontal),
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(Axis::Vertical),
        _ => None,
    }
}

fn source_of(source: WEnum<wl_pointer::AxisSource>) -> Option<AxisSource> {
    match source {
        WEnum::Value(wl_pointer::AxisSource::Wheel) => Some(AxisSource::Wheel),
        WEnum::Value(wl_pointer::AxisSource::Finger) => Some(AxisSource::Finger),
        WEnum::Value(wl_pointer::AxisSource::Continuous) => Some(AxisSource::Continuous),
        WEnum::Value(wl_pointer::AxisSource::WheelTilt) => Some(AxisSource::WheelTilt),
        _ => None,
    }
}

pub fn state(display: &DisplayHandle) {
    display.create_global::<Slipstream, ZwlrVirtualPointerManagerV1, _>(2, ());
}

impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for Slipstream {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrVirtualPointerManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        crate::ime::may_type(&client)
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for Slipstream {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        // There is one seat, so the `seat` argument has nothing to choose between.
        let (id, output): (_, Option<WlOutput>) = match request {
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointer { id, .. } => (id, None),
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput {
                id,
                output,
                ..
            } => (id, output),
            _ => return,
        };
        data_init.init(
            id,
            VirtualPointer {
                output: output.as_ref().and_then(Output::from_resource),
                pending: Mutex::default(),
            },
        );
    }
}

impl Dispatch<ZwlrVirtualPointerV1, VirtualPointer> for Slipstream {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        data: &VirtualPointer,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_virtual_pointer_v1::Request;
        let mut pending = data.pending.lock().unwrap();
        match request {
            Request::Motion { time, dx, dy } => {
                state.wake_ui();
                let pos = state.clamp_to_outputs(state.pointer_location() + Point::from((dx, dy)));
                state.pointer_moved_to(pos, InputTime::from_millis(time));
            }
            Request::MotionAbsolute {
                time,
                x,
                y,
                x_extent,
                y_extent,
            } => {
                if x_extent == 0 || y_extent == 0 {
                    return;
                }
                state.wake_ui();
                let Some(geo) = data
                    .output
                    .clone()
                    .or_else(|| state.absolute_screen())
                    .and_then(|output| state.space.output_geometry(&output))
                else {
                    return;
                };
                let fraction = |at: u32, extent: u32| (f64::from(at) / f64::from(extent)).min(1.0);
                let pos: Point<f64, Logical> = (
                    f64::from(geo.loc.x) + fraction(x, x_extent) * f64::from(geo.size.w - 1),
                    f64::from(geo.loc.y) + fraction(y, y_extent) * f64::from(geo.size.h - 1),
                )
                    .into();
                state.pointer_moved_to(pos, InputTime::from_millis(time));
            }
            Request::Button {
                time,
                button,
                state: button_state,
            } => {
                let button_state = match button_state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => ButtonState::Pressed,
                    WEnum::Value(wl_pointer::ButtonState::Released) => ButtonState::Released,
                    _ => return,
                };
                state.button_event(button, button_state, InputTime::from_millis(time));
            }
            Request::Axis { time, axis, value } => {
                let Some(axis) = axis_of(axis) else {
                    resource.post_error(zwlr_virtual_pointer_v1::Error::InvalidAxis, "bad axis");
                    return;
                };
                pending.time = time;
                pending.value[index(axis)] = Some(value);
            }
            Request::AxisSource { axis_source } => {
                let Some(source) = source_of(axis_source) else {
                    resource.post_error(
                        zwlr_virtual_pointer_v1::Error::InvalidAxisSource,
                        "bad axis source",
                    );
                    return;
                };
                pending.source = Some(source);
            }
            Request::AxisStop { time, axis } => {
                let Some(axis) = axis_of(axis) else {
                    resource.post_error(zwlr_virtual_pointer_v1::Error::InvalidAxis, "bad axis");
                    return;
                };
                pending.time = time;
                pending.stop[index(axis)] = true;
            }
            Request::AxisDiscrete {
                time,
                axis,
                value,
                discrete,
            } => {
                let Some(axis) = axis_of(axis) else {
                    resource.post_error(zwlr_virtual_pointer_v1::Error::InvalidAxis, "bad axis");
                    return;
                };
                pending.time = time;
                pending.value[index(axis)] = Some(value);
                pending.discrete[index(axis)] = Some(discrete);
            }
            Request::Frame => {
                let done = std::mem::take(&mut *pending);
                drop(pending);
                state.virtual_scroll(done);
            }
            _ => {}
        }
    }
}

impl Slipstream {
    /// A virtual wheel's events for one `frame`, sent as a real wheel's are: held back from
    /// the app under an overlay (which may scroll its own list), otherwise to the app.
    fn virtual_scroll(&mut self, done: Pending) {
        if !done.any() {
            return;
        }
        self.keys.super_tap.disarm();
        self.wake_ui();
        let amount = |axis: usize| {
            done.value[axis].unwrap_or_else(|| f64::from(done.discrete[axis].unwrap_or(0)) * NOTCH)
        };
        if self.pointer_blocked() || self.over_chrome(self.pointer_location()) {
            self.overlay_wheel(amount(1), NOTCH);
            return;
        }
        let source = done.source.unwrap_or(AxisSource::Wheel);
        let mut frame = AxisFrame::new(InputTime::from_millis(done.time)).source(source);
        for (axis, i) in [(Axis::Horizontal, 0), (Axis::Vertical, 1)] {
            if done.stop[i] {
                frame = frame.stop(axis);
            }
            if done.value[i].is_some() || done.discrete[i].is_some() {
                frame = frame.value(axis, amount(i));
            }
            if let Some(discrete) = done.discrete[i] {
                frame = frame.v120(axis, discrete * 120);
            }
        }
        let pointer = self.seat.get_pointer().unwrap();
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

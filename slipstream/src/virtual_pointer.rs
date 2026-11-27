//! `wlr-virtual-pointer-unstable-v1`: a mouse made of software.
//!
//! This is how wayvnc, wlrctl and other remote-control tools move the pointer, click and scroll.
//! Without it a VNC viewer sees the screen and can type (`ime.rs` offers the virtual keyboard)
//! but its mouse does nothing.
//!
//! What a virtual pointer sends goes through the same paths as a real mouse: relative motion is
//! `pointer_moved_by` (so a locked or confined pointer holds it too), buttons are `button_event`,
//! and the wheel is `wheel`. The buttons it holds down are let go of when it goes, so a dropped
//! connection never leaves a click or a drag held.
//!
//! **Unsandboxed clients only** (`ime::may_type`): the global is shown only to a client whose
//! process was proven, when it connected, to be outside any sandbox. That matters more here than
//! for the virtual keyboard. Virtual keys only ever reach the app with the keyboard, but a
//! virtual click lands wherever a real one would, Slipstream's own cards included: the share
//! picker, the password prompt and the way out.

use std::sync::Mutex;

use smithay::{
    backend::input::{AxisSource, ButtonState, InputTime},
    output::Output,
    reexports::{
        wayland_protocols_wlr::virtual_pointer::v1::server::{
            zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
            zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::ClientId,
            protocol::{wl_output::WlOutput, wl_pointer},
        },
    },
    utils::{Logical, Point},
};

use crate::{input::Wheel, state::Slipstream};

/// Which screen absolute motion is spread over, the axis events waiting for their `frame`, and
/// the buttons held down.
#[derive(Debug, Default)]
pub struct VirtualPointer {
    /// The screen the client named, if it did. Without one, absolute motion goes to the same
    /// screen a touchscreen would.
    output: Option<Output>,
    pending: Mutex<Pending>,
    held: Mutex<Held>,
}

#[derive(Debug, Default)]
struct Pending {
    source: Option<AxisSource>,
    time: u32,
    /// Horizontal, then vertical.
    value: [Option<f64>; 2],
    /// In notches, as the client sends them.
    discrete: [Option<i32>; 2],
    stop: [bool; 2],
}

impl Pending {
    /// The frame of scrolling these events make, if they make one. A notch is 120 in v120 units,
    /// counted in `f64` so that no number a client sends can overflow.
    fn wheel(self) -> Option<Wheel> {
        let any = self.value.iter().any(Option::is_some)
            || self.discrete.iter().any(Option::is_some)
            || self.stop.iter().any(|stop| *stop);
        any.then(|| Wheel {
            source: self.source.unwrap_or(AxisSource::Wheel),
            time: InputTime::from_millis(self.time),
            amount: self.value,
            v120: self
                .discrete
                .map(|notches| notches.map(|n| f64::from(n) * 120.0)),
            stop: self.stop,
        })
    }
}

/// The buttons a virtual pointer holds down.
#[derive(Debug, Default)]
struct Held(Vec<u32>);

impl Held {
    fn note(&mut self, button: u32, state: ButtonState) {
        match state {
            ButtonState::Pressed if !self.0.contains(&button) => self.0.push(button),
            ButtonState::Pressed => {}
            ButtonState::Released => self.0.retain(|held| *held != button),
        }
    }

    /// Every button still held, to let go of.
    fn take(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.0)
    }
}

/// Which of `Pending`'s pairs an axis is.
fn index(axis: WEnum<wl_pointer::Axis>) -> Option<usize> {
    match axis {
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(0),
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(1),
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
                ..VirtualPointer::default()
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
        let bad_axis =
            || resource.post_error(zwlr_virtual_pointer_v1::Error::InvalidAxis, "bad axis");
        match request {
            Request::Motion { time, dx, dy } => {
                // No acceleration of its own: what it sends is how far it moved.
                let delta = Point::from((dx, dy));
                state.pointer_moved_by(delta, delta, InputTime::from_millis(time));
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
                data.held.lock().unwrap().note(button, button_state);
                state.button_event(button, button_state, InputTime::from_millis(time));
            }
            Request::Axis { time, axis, value } => {
                let Some(i) = index(axis) else {
                    return bad_axis();
                };
                pending.time = time;
                pending.value[i] = Some(value);
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
                let Some(i) = index(axis) else {
                    return bad_axis();
                };
                pending.time = time;
                pending.stop[i] = true;
            }
            Request::AxisDiscrete {
                time,
                axis,
                value,
                discrete,
            } => {
                let Some(i) = index(axis) else {
                    return bad_axis();
                };
                pending.time = time;
                pending.value[i] = Some(value);
                pending.discrete[i] = Some(discrete);
            }
            Request::Frame => {
                let done = std::mem::take(&mut *pending);
                drop(pending);
                if let Some(wheel) = done.wheel() {
                    state.wheel(wheel);
                }
            }
            _ => {}
        }
    }

    /// Gone, by its own request or with its client: whatever it held down is let go of, so a
    /// remote viewer that drops mid-drag doesn't leave the drag going.
    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &ZwlrVirtualPointerV1,
        data: &VirtualPointer,
    ) {
        let held = data.held.lock().unwrap().take();
        for button in held {
            state.button_event(button, ButtonState::Released, InputTime::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixStream, sync::Arc};

    use smithay::reexports::wayland_server::{Display, backend::ClientData};

    use super::*;
    use crate::{capture, state::ClientState};

    /// Whether the virtual pointer's global is shown to a client made from `stream` with `data`,
    /// as the listener would make it.
    fn shown(stream: UnixStream, data: Arc<dyn ClientData>) -> bool {
        let display = Display::<Slipstream>::new().expect("a display");
        let client = display
            .handle()
            .insert_client(stream, data)
            .expect("the client inserts");
        <Slipstream as GlobalDispatch<ZwlrVirtualPointerManagerV1, ()>>::can_view(client, &())
    }

    #[test]
    fn only_programs_outside_a_sandbox_can_use_the_virtual_pointer() {
        // Both ends of a pair are this process: outside any sandbox, and still running.
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        let state = ClientState::for_connection(&ours);
        // Unless this process is confined itself, as in some containers, so rightly sandboxed.
        assert_eq!(
            shown(ours, Arc::new(state)),
            capture::this_process_is_unconfined()
        );

        // A process that has gone can't be proven to be outside one.
        for reap in [false, true] {
            let state =
                capture::with_a_client_that_has_gone("pointer", reap, ClientState::for_connection);
            let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
            assert!(!shown(ours, Arc::new(state)), "gone, reaped: {reap}");
        }

        // Nor can a client that has no state of ours.
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        assert!(!shown(ours, Arc::new(())));
    }

    #[test]
    fn the_buttons_it_holds_are_let_go_of_when_it_goes() {
        let mut held = Held::default();
        held.note(272, ButtonState::Pressed);
        held.note(273, ButtonState::Pressed);
        held.note(273, ButtonState::Pressed);
        held.note(272, ButtonState::Released);
        // A release of a button it never pressed changes nothing.
        held.note(274, ButtonState::Released);
        assert_eq!(held.take(), [273]);
        assert!(held.take().is_empty(), "let go of once");
    }

    #[test]
    fn any_number_of_notches_scrolls_without_overflowing() {
        let wheel = |discrete| {
            Pending {
                discrete,
                ..Pending::default()
            }
            .wheel()
            .unwrap()
            .frame()
        };
        let frame = wheel([Some(i32::MIN), Some(i32::MAX)]);
        assert_eq!(frame.v120, Some((i32::MIN, i32::MAX)));
        // Notches alone are a wheel's: 15 of scroll each.
        let frame = wheel([None, Some(-2)]);
        assert_eq!(frame.axis, (0.0, -30.0));
        assert_eq!(frame.v120, Some((0, -240)));
        assert!(Pending::default().wheel().is_none(), "nothing to send");
    }
}

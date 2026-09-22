//! `wlr-gamma-control-unstable-v1`: colour ramps set by a program of the user's own.
//!
//! This is what wlsunset, gammastep and redshift speak. Slipstream has its own night light, which
//! writes the same ramps (`nightlight.rs`), so the two have to take turns: while a client holds
//! an output's gamma, that output is left out of the night light, and when the client lets go —
//! or goes away — the night light is put back over the screen it was managing.
//!
//! **One client per output.** The protocol says a second one is told `failed`, which is how these
//! programs learn to stop. Ours is kept by the output's name, so a screen unplugged and plugged
//! back in is not still claimed by a control nobody holds.
//!
//! Offered to every client, as wlroots compositors do. A gamma ramp can make a screen unreadable,
//! which is a nuisance, but it reveals nothing and it is undone the moment the client lets go.
//! Screen *capture* is the one that gives a client something it can keep, and that is behind a
//! consent card (`screencopy.rs`) rather than a filter.

use std::{
    collections::HashMap,
    io::Read,
    os::fd::{AsRawFd, OwnedFd},
};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::gamma_control::v1::server::{
            zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1},
            zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
        },
    },
};

use crate::state::Slipstream;

/// Ramps are three channels of `u16`, so a screen's whole table is this many bytes.
fn table_bytes(size: usize) -> usize {
    size * 3 * std::mem::size_of::<u16>()
}

/// Which output a gamma control object drives, and the size its client was told. A control that
/// was told `failed` drives nothing: it holds no output and no size, and ignores what it is sent.
#[derive(Debug, Clone, Default)]
pub struct ControlData {
    pub output: Option<Output>,
    pub size: usize,
}

#[derive(Debug, Default)]
pub struct Gamma {
    /// The control holding each output, by the output's name. One at a time, as the protocol says.
    held: HashMap<String, ZwlrGammaControlV1>,
}

impl Gamma {
    /// Whether a client is driving this output's ramps, so the night light leaves it alone.
    pub fn is_held(&self, output: &Output) -> bool {
        self.held.contains_key(&output.name())
    }

    /// Every output a client is driving.
    pub fn held_outputs(&self) -> Vec<String> {
        self.held.keys().cloned().collect()
    }
}

pub fn state(display: &DisplayHandle) -> GlobalId {
    display.create_global::<Slipstream, ZwlrGammaControlManagerV1, _>(1, ())
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for Slipstream {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Slipstream {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } => {
                let Some(output) = Output::from_resource(&output) else {
                    // The output has gone since the client bound it. An inert object that is
                    // told `failed` is what the protocol asks for.
                    data_init.init(id, ControlData::default()).failed();
                    return;
                };
                let name = output.name();
                // One client per output: whoever asks second is told so and gives up.
                if state.gamma.held.contains_key(&name) {
                    tracing::info!(screen = name, "a second gamma control was refused");
                    data_init.init(id, ControlData::default()).failed();
                    return;
                }
                let Some(size) = state.gamma_size(&output) else {
                    tracing::info!(screen = name, "this screen has no gamma ramp to give away");
                    data_init.init(id, ControlData::default()).failed();
                    return;
                };
                let control = data_init.init(
                    id,
                    ControlData {
                        output: Some(output.clone()),
                        size,
                    },
                );
                control.gamma_size(size as u32);
                state.gamma.held.insert(name.clone(), control);
                tracing::info!(screen = name, size, "a program took this screen's gamma");
            }
            zwlr_gamma_control_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ControlData> for Slipstream {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        data: &ControlData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_gamma_control_v1::Request::SetGamma { fd } => {
                // An object already told `failed` drives nothing and ignores what it is sent.
                let (Some(output), 1..) = (data.output.as_ref(), data.size) else {
                    return;
                };
                let table = read_table(fd, data.size);
                let set = table.is_some_and(|(red, green, blue)| {
                    state.set_output_gamma(output, &red, &green, &blue)
                });
                if !set {
                    tracing::warn!(
                        screen = output.name(),
                        "couldn't use the gamma a program asked for"
                    );
                    resource.failed();
                    state.release_gamma(output);
                }
            }
            zwlr_gamma_control_v1::Request::Destroy => {}
            _ => {}
        }
    }

    /// The control is gone, by `destroy` or with its client: the screen goes back to whatever the
    /// night light says it should be.
    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrGammaControlV1,
        data: &ControlData,
    ) {
        let Some(name) = data.output.as_ref().map(Output::name) else {
            return;
        };
        // Only if this is still the control that holds it: a refused second control is destroyed
        // too, and must not take the first one's place away.
        if state.gamma.held.get(&name).map(Resource::id) == Some(resource.id()) {
            state.gamma.held.remove(&name);
            tracing::info!(screen = name, "a program gave this screen's gamma back");
            state.restore_night_light();
        }
    }
}

impl Slipstream {
    /// Stops holding an output's gamma after a table we couldn't use, and puts the night light
    /// back over it.
    fn release_gamma(&mut self, output: &Output) {
        if self.gamma.held.remove(&output.name()).is_some() {
            self.restore_night_light();
        }
    }
}

/// Reads a client's gamma table: `size` red, then green, then blue, as little-endian `u16`.
///
/// **Never blocks.** The fd is whatever the client passed, which may be a pipe it never writes
/// to, and a compositor that waits on one has stopped. It is read non-blocking, once: a table
/// that isn't all there is no table.
fn read_table(fd: OwnedFd, size: usize) -> Option<(Vec<u16>, Vec<u16>, Vec<u16>)> {
    // SAFETY: fcntl with F_SETFL on an owned descriptor.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return None;
        }
    }
    let wanted = table_bytes(size);
    let mut bytes = vec![0u8; wanted];
    let mut file = std::fs::File::from(fd);
    let mut filled = 0;
    while filled < wanted {
        match file.read(&mut bytes[filled..]) {
            Ok(0) => return None,
            Ok(read) => filled += read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let channel = |start: usize| {
        bytes[start..start + size * 2]
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect::<Vec<u16>>()
    };
    Some((channel(0), channel(size * 2), channel(size * 4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `name` keeps each test's file its own: these run on threads of their own, and one path
    /// shared between them is one test truncating another's table halfway through its read.
    fn table_in_a_file(name: &str, values: &[u16]) -> OwnedFd {
        let dir = crate::files::test_scratch(name);
        let path = dir.join("table");
        let mut file = std::fs::File::create(&path).unwrap();
        for value in values {
            file.write_all(&value.to_ne_bytes()).unwrap();
        }
        file.sync_all().unwrap();
        std::fs::File::open(&path).unwrap().into()
    }

    #[test]
    fn a_table_is_read_as_three_channels_in_order() {
        let fd = table_in_a_file("gamma-whole", &[1, 2, 10, 20, 100, 200]);
        let (red, green, blue) = read_table(fd, 2).unwrap();
        assert_eq!(
            (red, green, blue),
            (vec![1, 2], vec![10, 20], vec![100, 200])
        );
    }

    #[test]
    fn a_table_that_is_too_short_is_no_table() {
        // Two entries offered where three channels of two were asked for.
        let fd = table_in_a_file("gamma-short", &[1, 2]);
        assert!(read_table(fd, 2).is_none());
    }

    /// A client can pass any descriptor, including one that will never carry a table. Reading it
    /// must not stop the compositor.
    #[test]
    fn a_pipe_that_never_fills_does_not_block() {
        let (read, _write) = std::io::pipe().unwrap();
        assert!(read_table(OwnedFd::from(read), 256).is_none());
    }
}

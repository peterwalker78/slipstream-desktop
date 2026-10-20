//! Slipstream's own portal backend on the session bus.
//!
//! xdg-desktop-portal is what apps, sandboxed or not, ask when they want something from the
//! desktop. `session/slipstream.portal` points it at the bus name below for the interfaces
//! Slipstream answers itself — dark or light (`appearance.rs`) and shortcuts apps keep while
//! another app has the keyboard (`shortcuts/`) — and `slipstream-portals.conf` routes them here.
//! Both are served on one connection, at the path every portal backend uses.

use std::sync::OnceLock;

use slipstream_config::ColourScheme;
use smithay::reexports::calloop::channel::Sender;

use crate::{appearance, shortcuts};

pub const NAME: &str = "org.freedesktop.impl.portal.desktop.slipstream";
pub const PATH: &str = "/org/freedesktop/portal/desktop";

/// The connection, once the name is ours, for sending signals.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Takes the backend's name on the session bus and serves it. If something else has the name
/// there is another Slipstream running, and this one leaves the portal to it.
pub fn serve(scheme: ColourScheme, shortcut_events: Sender<shortcuts::portal::Event>) {
    appearance::start_with(scheme);
    std::thread::spawn(move || {
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(NAME))
            .and_then(|builder| builder.serve_at(PATH, appearance::Server))
            .and_then(|builder| {
                builder.serve_at(
                    PATH,
                    shortcuts::portal::Backend {
                        events: shortcut_events,
                    },
                )
            })
            .and_then(|builder| builder.build());
        match connection {
            Ok(connection) => {
                tracing::info!("serving the desktop portal backend on the session bus");
                let _ = CONNECTION.set(connection);
            }
            Err(err) => tracing::warn!("couldn't serve the desktop portal backend: {err}"),
        }
    });
}

/// The connection, once it is up.
pub fn connection() -> Option<zbus::blocking::Connection> {
    CONNECTION.get().cloned()
}

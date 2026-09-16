//! What apps are told to colour themselves: Slipstream answers the settings portal itself.
//!
//! `org.freedesktop.impl.portal.Settings` is the one channel every toolkit uses to ask the
//! desktop whether to be dark or light. GTK, Qt through its xdgdesktopportal platform theme,
//! Firefox, Chromium and anything packaged as a Flatpak all read `color-scheme` from it, and a
//! sandboxed app has no other way to find out at all.
//!
//! Slipstream names only itself in `XDG_CURRENT_DESKTOP`, so no other desktop's backend answers
//! for it. Without this one nothing would, and every app would fall back to light.
//!
//! `session/slipstream.portal` points xdg-desktop-portal at the bus name below and
//! `slipstream-portals.conf` asks for it first; the rest of the namespace (accent colour, fonts,
//! contrast) still goes on to whatever backend the machine has.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use slipstream_config::ColourScheme;
use zbus::{
    interface,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

const NAME: &str = "org.freedesktop.impl.portal.desktop.slipstream";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.impl.portal.Settings";

/// The cross-desktop namespace, and the only one that carries a preference rather than a theme's
/// name.
const APPEARANCE: &str = "org.freedesktop.appearance";
/// GNOME's namespace. GTK reads the theme's name from here, so apps that follow a theme rather
/// than a preference still turn dark.
const GNOME: &str = "org.gnome.desktop.interface";

/// What the settings file says now. Read by the bus thread, written by the main loop.
static SCHEME: Mutex<ColourScheme> = Mutex::new(ColourScheme::Dark);
/// The connection, once the name is ours, for sending `SettingChanged`.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Every namespace and key Slipstream answers for, as they stand.
fn readings() -> Vec<(&'static str, Vec<(&'static str, OwnedValue)>)> {
    let scheme = *SCHEME.lock().unwrap();
    let text = |value: &str| OwnedValue::from(zbus::zvariant::Str::from(value.to_string()));
    vec![
        (
            APPEARANCE,
            vec![("color-scheme", OwnedValue::from(scheme.portal_value()))],
        ),
        (
            GNOME,
            vec![
                ("color-scheme", text(scheme.gnome_value())),
                ("gtk-theme", text(scheme.gtk_theme())),
            ],
        ),
    ]
}

/// Whether a namespace is one of those asked for. No namespaces at all means all of them, and a
/// name ending in `*` matches everything that starts with the rest of it, as the portal's
/// interface describes.
fn asked_for(namespace: &str, filters: &[String]) -> bool {
    filters.is_empty()
        || filters.iter().any(|filter| match filter.strip_suffix('*') {
            Some(prefix) => namespace.starts_with(prefix),
            None => namespace == filter,
        })
}

fn reading(namespace: &str, key: &str) -> Option<OwnedValue> {
    readings()
        .into_iter()
        .filter(|(held, _)| *held == namespace)
        .flat_map(|(_, keys)| keys)
        .find(|(held, _)| *held == key)
        .map(|(_, value)| value)
}

struct Server;

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl Server {
    /// Everything Slipstream answers for, in the namespaces asked for.
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> HashMap<String, HashMap<String, OwnedValue>> {
        readings()
            .into_iter()
            .filter(|(namespace, _)| asked_for(namespace, &namespaces))
            .map(|(namespace, keys)| {
                let keys = keys
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value))
                    .collect();
                (namespace.to_string(), keys)
            })
            .collect()
    }

    /// One value. `ReadOne` is what a portal that knows this interface's second version calls;
    /// `Read` is the same thing under the name the first version used, and the portal still calls
    /// it once at startup.
    async fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        self.read_one(namespace, key).await
    }

    async fn read_one(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        // An error rather than a value is how a backend says "not mine": the portal moves on to
        // the next one, which is what should answer for fonts, contrast and the accent colour.
        reading(namespace, key).ok_or_else(|| {
            zbus::fdo::Error::UnknownProperty(format!("Slipstream doesn't set {namespace} {key}"))
        })
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;
}

/// Takes the appearance backend's name on the session bus and serves it. If something else has
/// the name there is another Slipstream running, and this one leaves the setting to it.
pub fn serve(scheme: ColourScheme) {
    *SCHEME.lock().unwrap() = scheme;
    std::thread::spawn(move || {
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(NAME))
            .and_then(|builder| builder.serve_at(PATH, Server))
            .and_then(|builder| builder.build());
        match connection {
            Ok(connection) => {
                tracing::info!("serving the appearance settings portal on the session bus");
                let _ = CONNECTION.set(connection);
            }
            Err(err) => tracing::warn!("couldn't serve the appearance settings portal: {err}"),
        }
    });
}

/// The setting changed. Apps are told one key at a time, as the interface asks, and take it
/// without restarting.
pub fn set(scheme: ColourScheme) {
    {
        let mut held = SCHEME.lock().unwrap();
        if *held == scheme {
            return;
        }
        *held = scheme;
    }
    let Some(connection) = CONNECTION.get().cloned() else {
        return;
    };
    std::thread::spawn(move || {
        for (namespace, keys) in readings() {
            for (key, value) in keys {
                let body = (namespace, key, Value::from(value));
                if let Err(err) =
                    connection.emit_signal(None::<&str>, PATH, INTERFACE, "SettingChanged", &body)
                {
                    tracing::warn!(namespace, key, "couldn't send SettingChanged: {err}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_filter_by_name_and_by_prefix() {
        let all: Vec<String> = Vec::new();
        assert!(asked_for(APPEARANCE, &all));
        assert!(asked_for(APPEARANCE, &[APPEARANCE.to_string()]));
        assert!(asked_for(APPEARANCE, &["org.freedesktop.*".to_string()]));
        assert!(!asked_for(APPEARANCE, &[GNOME.to_string()]));
        assert!(!asked_for(APPEARANCE, &["org.gnome.*".to_string()]));
    }

    #[test]
    fn dark_is_the_portals_first_preference_and_light_its_second() {
        assert_eq!(ColourScheme::Dark.portal_value(), 1);
        assert_eq!(ColourScheme::Light.portal_value(), 2);
    }

    #[test]
    fn keys_outside_slipstreams_own_are_left_to_another_backend() {
        assert!(reading(APPEARANCE, "color-scheme").is_some());
        assert!(reading(APPEARANCE, "accent-color").is_none());
        assert!(reading("org.gnome.desktop.a11y", "always-show-text-caret").is_none());
    }
}

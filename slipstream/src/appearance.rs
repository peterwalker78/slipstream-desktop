//! Apps are told to be dark, and to take Slipstream's primary colour as their accent: Slipstream
//! answers the settings portal itself.
//!
//! `org.freedesktop.impl.portal.Settings` is the one channel every toolkit uses to ask the
//! desktop whether to be dark or light. GTK, Qt through its xdgdesktopportal platform theme,
//! Firefox, Chromium and anything packaged as a Flatpak all read `color-scheme` from it, and a
//! sandboxed app has no other way to find out at all.
//!
//! Slipstream names only itself in `XDG_CURRENT_DESKTOP`, so no other desktop's backend answers
//! for it. Without this one nothing would, and every app would fall back to light.
//!
//! `accent-color` is the primary colour from the settings, the one the focus ring and the
//! workspace in front are drawn in, and apps are told when it changes. Apps that read the key
//! themselves follow it exactly; libadwaita apps follow it too, taking whichever of their own
//! accent colours is nearest.
//!
//! `session/slipstream.portal` points xdg-desktop-portal at Slipstream's backend (`portal.rs`) and
//! `slipstream-portals.conf` asks for it first; the rest of the namespace (fonts, contrast) still
//! goes on to whatever backend the machine has.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, mpsc},
};

use zbus::{
    interface,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

/// The cross-desktop namespace, and the only one that carries a preference rather than a theme's
/// name.
const APPEARANCE: &str = "org.freedesktop.appearance";
/// GNOME's namespace. GTK reads the theme's name from here, so apps that follow a theme rather
/// than a preference still turn dark.
const GNOME: &str = "org.gnome.desktop.interface";

const ACCENT: &str = "accent-color";

/// The primary colour as the settings have it now, each channel 0 to 1, once the compositor has
/// said. The portal answers on a thread of its own, so it is kept here rather than in the
/// compositor's state.
static PRIMARY: Mutex<Option<[f32; 3]>> = Mutex::new(None);

/// The primary colour apps are given: the settings' own, or the default until they are read.
fn primary() -> [f32; 3] {
    let held = *PRIMARY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    held.unwrap_or_else(|| slipstream_config::Borders::default().selected_tile_rgb())
}

/// An sRGB colour as the portal spells one: three doubles, each 0 to 1.
fn accent(rgb: [f32; 3]) -> Value<'static> {
    let [r, g, b] = rgb.map(|channel| f64::from(channel.clamp(0.0, 1.0)));
    Value::from((r, g, b))
}

/// Takes the primary colour from the settings, at start and whenever they change. When it is a
/// new one, apps already running are told through the portal.
pub fn set_primary(rgb: [f32; 3]) {
    {
        let mut held = PRIMARY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = held.replace(rgb);
        if before.is_none_or(|before| before == rgb) {
            return;
        }
    }
    // Sent off the event loop, one after another, so the last colour told is the last one set.
    static OUTBOX: OnceLock<mpsc::Sender<[f32; 3]>> = OnceLock::new();
    let Some(connection) = crate::portal::connection() else {
        return;
    };
    let outbox = OUTBOX.get_or_init(|| {
        let (outbox, colours) = mpsc::channel::<[f32; 3]>();
        std::thread::spawn(move || {
            for rgb in colours {
                let body = (APPEARANCE, ACCENT, accent(rgb));
                let sent = connection.emit_signal(
                    None::<&str>,
                    crate::portal::PATH,
                    "org.freedesktop.impl.portal.Settings",
                    "SettingChanged",
                    &body,
                );
                if let Err(err) = sent {
                    tracing::warn!("couldn't tell apps the accent colour changed: {err}");
                }
            }
        });
        outbox
    });
    let _ = outbox.send(rgb);
}

/// Every namespace and key Slipstream answers for. Apps are always told dark, to match
/// Slipstream's own bar, panels and windows, and given the primary colour as their accent.
fn readings() -> Vec<(&'static str, Vec<(&'static str, OwnedValue)>)> {
    let text = |value: &str| OwnedValue::from(zbus::zvariant::Str::from(value.to_string()));
    vec![
        (
            APPEARANCE,
            vec![
                // The portal's own spelling: 1 is dark, 2 light, 0 no preference.
                ("color-scheme", OwnedValue::from(1u32)),
                (
                    ACCENT,
                    OwnedValue::try_from(accent(primary())).expect("doubles hold no file"),
                ),
            ],
        ),
        (
            GNOME,
            vec![
                // GNOME's, which GTK reads when it doesn't know the portal's key.
                ("color-scheme", text("prefer-dark")),
                // A theme's name, for apps that follow one rather than a preference. Adwaita
                // ships with GTK itself, so it is on every machine GTK apps run on.
                ("gtk-theme", text("Adwaita-dark")),
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

pub struct Server;

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
        // the next one, which is what should answer for fonts and contrast.
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
    fn apps_are_told_dark() {
        let dark = reading(APPEARANCE, "color-scheme").unwrap();
        assert_eq!(u32::try_from(dark).unwrap(), 1);
    }

    #[test]
    fn apps_are_given_the_primary_colour_as_their_accent() {
        let triple = |value: OwnedValue| <(f64, f64, f64)>::try_from(value).unwrap();
        // The default primary, #42d3ff, until the settings say otherwise.
        let (r, g, b) = triple(reading(APPEARANCE, ACCENT).unwrap());
        let near = |got: f64, byte: u8| (got - f64::from(byte) / 255.0).abs() < 1e-6;
        assert!(
            near(r, 0x42) && near(g, 0xd3) && near(b, 0xff),
            "{r} {g} {b}"
        );
        // A colour the settings give is the one read back, in the portal's 0 to 1.
        set_primary([1.0, 0.5, 0.0]);
        assert_eq!(
            triple(reading(APPEARANCE, ACCENT).unwrap()),
            (1.0, 0.5, 0.0)
        );
        assert_eq!(
            reading(APPEARANCE, ACCENT)
                .unwrap()
                .value_signature()
                .to_string(),
            "(ddd)"
        );
        set_primary(slipstream_config::Borders::default().selected_tile_rgb());
    }

    #[test]
    fn keys_outside_slipstreams_own_are_left_to_another_backend() {
        assert!(reading(APPEARANCE, "color-scheme").is_some());
        assert!(reading(APPEARANCE, "contrast").is_none());
        assert!(reading("org.gnome.desktop.a11y", "always-show-text-caret").is_none());
    }
}

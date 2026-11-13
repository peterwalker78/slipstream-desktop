//! Keeping the theme settings that ask the portal away from the portal backends.
//!
//! Apps D-Bus and systemd start on another app's behalf (a file manager asked to show a download)
//! should look like the rest of the session, so the session shares how apps find their theme with
//! the user manager. But the portal backends are started the same way, and a backend told to ask
//! the portal for its own theme calls xdg-desktop-portal while xdg-desktop-portal is still waiting
//! for that backend to start: each waits on the other until D-Bus gives up. So each backend that
//! systemd starts gets a drop-in taking those settings back out, found from the installed
//! `.portal` files, whichever desktop they come from. Outside this session nothing sets them, and
//! the drop-ins change nothing.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// The settings that make an app ask the portal for its theme.
pub const THEME_VARS: [&str; 2] = ["QT_QPA_PLATFORMTHEME", "GTK_USE_PORTAL"];

/// The drop-in's file name, the same in every unit's folder.
const DROP_IN: &str = "slipstream-portal-theme.conf";

/// The drop-in that takes `THEME_VARS` out of a backend's environment.
fn drop_in() -> String {
    format!(
        "# Written by Slipstream at login. This unit is a portal backend: told to ask the portal\n\
         # for its own theme, it would wait on xdg-desktop-portal while xdg-desktop-portal waits\n\
         # on it. Outside a Slipstream session nothing sets these, and this changes nothing.\n\
         [Service]\n\
         UnsetEnvironment={}\n",
        THEME_VARS.join(" ")
    )
}

/// The `key=` value in an INI-style file's text, from the first line that sets it.
fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix(key)?
                .trim_start()
                .strip_prefix('=')
        })
        .map(str::trim)
        .find(|value| !value.is_empty())
}

/// The systemd units that start the portal backends installed under `data_dirs`, each found
/// through the backend's `.portal` file and the D-Bus service file for its name.
fn backend_units(data_dirs: &[PathBuf]) -> Vec<String> {
    let mut units = Vec::new();
    for dir in data_dirs {
        let Ok(entries) = fs::read_dir(dir.join("xdg-desktop-portal/portals")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "portal") {
                continue;
            }
            let Some(name) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| value(&text, "DBusName").map(str::to_string))
            else {
                continue;
            };
            let unit = data_dirs.iter().find_map(|dir| {
                let service = dir.join("dbus-1/services").join(format!("{name}.service"));
                let text = fs::read_to_string(service).ok()?;
                value(&text, "SystemdService").map(str::to_string)
            });
            if let Some(unit) = unit
                && unit.ends_with(".service")
                && !unit.contains('/')
                && !units.contains(&unit)
            {
                units.push(unit);
            }
        }
    }
    units.sort();
    units
}

/// Writes the drop-in for every portal backend systemd starts, under `units_dir` (the user's
/// `systemd/user` folder), and says whether any was new or changed, so the user manager needs a
/// reload before it starts them.
fn write_drop_ins(data_dirs: &[PathBuf], units_dir: &Path) -> bool {
    let wanted = drop_in();
    let mut changed = false;
    for unit in backend_units(data_dirs) {
        let dir = units_dir.join(format!("{unit}.d"));
        let path = dir.join(DROP_IN);
        if fs::read_to_string(&path).is_ok_and(|text| text == wanted) {
            continue;
        }
        match fs::create_dir_all(&dir).and_then(|()| fs::write(&path, &wanted)) {
            Ok(()) => changed = true,
            Err(err) => {
                tracing::warn!(unit, "couldn't keep the theme from a portal backend: {err}")
            }
        }
    }
    changed
}

/// Puts the drop-ins in place for this session's portal backends, and reloads the user manager if
/// any changed. Before the theme settings are shared, and before anything starts a backend.
pub fn keep_theme_from_backends() {
    let mut data_dirs = vec![slipstream_config::xdg_home("XDG_DATA_HOME", ".local/share")];
    data_dirs.extend(slipstream_config::xdg_data_dirs());
    let units_dir = slipstream_config::xdg_home("XDG_CONFIG_HOME", ".config").join("systemd/user");
    if write_drop_ins(&data_dirs, &units_dir) {
        let _ = crate::launch::command("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("portal-units-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn portal_backends_started_by_systemd_get_the_theme_taken_back_out() {
        let root = scratch("backends");
        let share = root.join("share");
        put(
            share.join("xdg-desktop-portal/portals/toolkit.portal"),
            "[portal]\nDBusName=org.example.impl.portal.toolkit\nInterfaces=a;\n",
        );
        put(
            share.join("dbus-1/services/org.example.impl.portal.toolkit.service"),
            "[D-BUS Service]\nName=org.example.impl.portal.toolkit\nExec=/bin/true\n\
             SystemdService=example-portal-toolkit.service\n",
        );
        // Started by D-Bus itself, so no unit to give a drop-in to.
        put(
            share.join("xdg-desktop-portal/portals/wallet.portal"),
            "[portal]\nDBusName=org.example.impl.portal.wallet\n",
        );
        put(
            share.join("dbus-1/services/org.example.impl.portal.wallet.service"),
            "[D-BUS Service]\nName=org.example.impl.portal.wallet\nExec=/bin/true\n",
        );
        let units = root.join("units");
        assert!(write_drop_ins(std::slice::from_ref(&share), &units));
        let written =
            fs::read_to_string(units.join("example-portal-toolkit.service.d").join(DROP_IN))
                .unwrap();
        assert!(written.contains("UnsetEnvironment=QT_QPA_PLATFORMTHEME GTK_USE_PORTAL"));
        assert_eq!(fs::read_dir(&units).unwrap().count(), 1);
        // Already in place: nothing to reload.
        assert!(!write_drop_ins(&[share], &units));
        let _ = fs::remove_dir_all(root);
    }
}

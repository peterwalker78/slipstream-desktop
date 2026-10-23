//! Which app a tray item belongs to: one of the Wayland clients, so a client that has hidden its
//! windows and the icon it keeps in the tray show as one app, and an app whose window is up isn't
//! listed as running in the background; or an installed app's desktop entry, for its name and
//! icon.
//!
//! The process behind the item's bus connection decides first, then its names. A Flatpak app's
//! bus connection is usually its sandbox's D-Bus proxy, a cousin of the app's processes under the
//! sandbox's outermost bwrap, so that is looked for too.

use std::path::Path;

use crate::apps::App;

/// How far up a client's ancestry an item's process may be and still be its app's: an app's
/// own launcher keeps the tray icon in some, a step or two above the process with the windows.
const ANCESTRY: usize = 3;

/// The process behind an item's bus connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Process {
    /// Its pid, then its parent's, and so on up, short of init.
    pub lineage: Vec<u32>,
    /// When it is a Flatpak sandbox's bus proxy, the sandbox's outermost bwrap, which the app's
    /// own processes run under too.
    pub sandbox: Option<u32>,
    /// Its name, as the kernel keeps it (at most 15 characters).
    pub name: String,
}

impl Process {
    /// Reads `pid`'s ancestry and name from `proc` (normally `/proc`).
    pub fn read(proc: &Path, pid: u32) -> Option<Self> {
        let lineage = crate::notify::lineage(proc, pid);
        if lineage.is_empty() {
            return None;
        }
        let name_of = |pid: u32| {
            crate::files::read_small_text(&proc.join(pid.to_string()).join("comm"), 64)
                .map(|name| name.trim_end().to_string())
        };
        Some(Self {
            sandbox: sandbox_root(&lineage, name_of),
            name: name_of(pid).unwrap_or_default(),
            lineage,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.lineage.first().copied()
    }
}

/// The outermost bwrap above a Flatpak sandbox's bus proxy: walking up from `xdg-dbus-proxy`
/// through the bwraps it runs under, the last one before something else.
fn sandbox_root(lineage: &[u32], name_of: impl Fn(u32) -> Option<String>) -> Option<u32> {
    let (first, above) = lineage.split_first()?;
    if name_of(*first).as_deref() != Some("xdg-dbus-proxy") {
        return None;
    }
    above
        .iter()
        .take_while(|pid| name_of(**pid).as_deref() == Some("bwrap"))
        .last()
        .copied()
}

/// A Wayland client, as far as matching goes.
#[derive(Debug, Clone, Copy)]
pub struct Client<'a> {
    /// Its pid, then its ancestors'.
    pub lineage: &'a [u32],
    pub app_id: &'a str,
}

/// The client an item belongs to, if any can be told: the one whose process is the item's, the
/// one in the item's Flatpak sandbox, the nearest one the item's process started, else the one
/// whose app ID is the item's ID or title. `names` are the item's ID and title.
pub fn owner(process: Option<&Process>, names: &[&str], clients: &[Client]) -> Option<usize> {
    if let Some((process, pid)) = process.and_then(|process| Some((process, process.pid()?))) {
        if let Some(at) = clients
            .iter()
            .position(|client| client.lineage.first() == Some(&pid))
        {
            return Some(at);
        }
        let nearest = |ancestor: u32, depth: usize| {
            clients
                .iter()
                .enumerate()
                .filter_map(|(at, client)| {
                    let up = client
                        .lineage
                        .iter()
                        .take(depth + 1)
                        .position(|p| *p == ancestor)?;
                    Some((up, at))
                })
                .min()
                .map(|(_, at)| at)
        };
        if let Some(root) = process.sandbox
            && let Some(at) = nearest(root, usize::MAX - 1)
        {
            return Some(at);
        }
        if let Some(at) = nearest(pid, ANCESTRY) {
            return Some(at);
        }
    }
    clients
        .iter()
        .position(|client| names.iter().any(|name| same_app(client.app_id, name)))
}

/// Whether an app ID and an item's ID or title name the same app: equal but for case and a
/// `.desktop` ending, or once a reverse-DNS prefix is taken off either.
pub fn same_app(app_id: &str, name: &str) -> bool {
    let plain = |id: &str| id.trim().trim_end_matches(".desktop").to_lowercase();
    let (app_id, name) = (plain(app_id), plain(name));
    if app_id.is_empty() || name.is_empty() {
        return false;
    }
    let last = |id: &str| id.rsplit('.').next().unwrap_or_default().to_string();
    app_id == name || last(&app_id) == name || app_id == last(&name)
}

/// The desktop entry for an item with no client to go by: by its ID or title as an app ID, then
/// by the program its process runs.
pub fn entry<'a>(apps: &'a [App], names: &[&str], process: Option<&Process>) -> Option<&'a App> {
    let by_name = names.iter().find_map(|name| {
        apps.iter().find(|app| {
            same_app(&app.id, name)
                || app
                    .wm_class
                    .as_deref()
                    .is_some_and(|class| class.eq_ignore_ascii_case(name))
        })
    });
    by_name.or_else(|| {
        let running = process
            .map(|process| process.name.as_str())
            .filter(|name| !name.is_empty())?;
        apps.iter().find(|app| {
            let program = app
                .exec
                .first()
                .map(|exec| exec.rsplit('/').next().unwrap_or(exec));
            // The kernel keeps 15 characters of a name.
            program.is_some_and(|program| program.chars().take(15).eq(running.chars()))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(lineage: &[u32]) -> Process {
        Process {
            lineage: lineage.to_vec(),
            ..Process::default()
        }
    }

    fn client<'a>(lineage: &'a [u32], app_id: &'a str) -> Client<'a> {
        Client { lineage, app_id }
    }

    #[test]
    fn an_item_belongs_to_the_client_with_its_process() {
        let clients = [
            client(&[900, 10], "org.example.Terminal"),
            client(&[500, 10], "org.example.Chat"),
        ];
        assert_eq!(
            owner(
                Some(&process(&[500, 10])),
                &["chrome_status_icon_1"],
                &clients
            ),
            Some(1)
        );
    }

    #[test]
    fn a_flatpaks_bus_proxy_finds_the_app_in_its_sandbox() {
        // flatpak run became the outer bwrap (300); it started the proxy's own bwrap (310) and the
        // app's inner one (320).
        let proxy = Process {
            lineage: vec![311, 310, 300, 10],
            sandbox: Some(300),
            name: "xdg-dbus-proxy".into(),
        };
        let clients = [
            client(&[700, 10], "org.example.Editor"),
            client(&[330, 320, 300, 10], "com.example.Chat"),
        ];
        assert_eq!(owner(Some(&proxy), &["chat"], &clients), Some(1));
    }

    #[test]
    fn the_sandbox_root_is_the_last_bwrap_above_the_proxy() {
        let names = |pid: u32| {
            Some(String::from(match pid {
                311 => "xdg-dbus-proxy",
                310 | 300 => "bwrap",
                _ => "systemd",
            }))
        };
        assert_eq!(sandbox_root(&[311, 310, 300, 10], names), Some(300));
        assert_eq!(sandbox_root(&[310, 300, 10], names), None, "not a proxy");
    }

    #[test]
    fn an_item_kept_by_the_apps_launcher_finds_the_process_with_the_window() {
        // A launcher (400) holds the tray icon; its helper two steps down has the window.
        let clients = [client(&[420, 410, 400, 10], "org.example.Store")];
        assert_eq!(
            owner(Some(&process(&[400, 10])), &["store-tray"], &clients),
            Some(0)
        );
    }

    #[test]
    fn an_app_started_from_a_terminal_isnt_taken_for_the_terminal() {
        // The terminal (200) started a shell (210) that started the app (220), which has no
        // window and so no client to match.
        let clients = [client(&[200, 10], "org.example.Terminal")];
        assert_eq!(
            owner(Some(&process(&[220, 210, 200, 10])), &["vault"], &clients),
            None
        );
    }

    #[test]
    fn with_no_process_to_go_by_the_names_decide() {
        let clients = [
            client(&[1], "org.example.Terminal"),
            client(&[2], "org.keepassxc.KeePassXC"),
        ];
        assert_eq!(owner(None, &["keepassxc"], &clients), Some(1));
        assert_eq!(owner(None, &["KeePassXC.desktop"], &clients), Some(1));
        assert_eq!(owner(None, &["chrome_status_icon_1"], &clients), None);
        assert_eq!(owner(None, &[""], &clients), None);
    }

    #[test]
    fn the_nearest_client_wins_when_several_descend_from_the_item() {
        let clients = [
            client(&[640, 630, 620, 600], "org.example.Game"),
            client(&[610, 600], "org.example.Launcher"),
        ];
        assert_eq!(
            owner(Some(&process(&[600, 10])), &["launcher"], &clients),
            Some(1)
        );
    }

    #[test]
    fn a_desktop_entry_is_found_by_name_then_by_program() {
        let app = |id: &str, exec: &str| App {
            id: id.into(),
            name: id.into(),
            generic_name: String::new(),
            keywords: Vec::new(),
            exec: vec![exec.into()],
            icon: None,
            terminal: false,
            wm_class: None,
            emulates_terminal: false,
        };
        let apps = [
            app(
                "com.example.desktopclient.Sync",
                "/usr/bin/examplesync-desktop",
            ),
            app("org.example.Vault", "vault"),
        ];
        let found = |names: &[&str], running: &str| {
            let process = Process {
                name: running.into(),
                ..Process::default()
            };
            entry(&apps, names, Some(&process)).map(|app| app.id.as_str())
        };
        assert_eq!(found(&["vault"], ""), Some("org.example.Vault"));
        assert_eq!(
            found(&["tray-1"], "examplesync-des"),
            Some("com.example.desktopclient.Sync")
        );
        assert_eq!(found(&["tray-1"], "other"), None);
    }
}

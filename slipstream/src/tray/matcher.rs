//! Which app a tray item belongs to: one of the Wayland clients, so a client that has hidden its
//! windows and the icon it keeps in the tray show as one app, and an app whose window is up isn't
//! listed as running in the background; or an installed app's desktop entry, for its name and
//! icon.
//!
//! The process behind the item's bus connection decides first, then, where the process allows
//! it, its names (`owner`). A Flatpak app's bus connection is usually its sandbox's D-Bus proxy,
//! a cousin of the app's processes under the sandbox's outermost bwrap, so that is looked for
//! too. The proxy's bwrap is often orphaned to init, though, which leaves the two with no
//! ancestor in common; then the Flatpak each runs in, which the sandbox's `.flatpak-info` names
//! for the proxy and the app alike, says which app it is.

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
    /// The app ID of the Flatpak it runs in (`flatpak_of`).
    pub flatpak: Option<String>,
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
            flatpak: flatpak_of(proc, pid),
            lineage,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.lineage.first().copied()
    }
}

/// The app ID of the Flatpak `pid` runs in, read as the desktop portal reads it: from the
/// `.flatpak-info` Flatpak puts at the root of every sandbox, its bus proxy's included, seen
/// through `proc`'s `<pid>/root`. Not from the process's scope, whose name any process can pick
/// for itself. The process is held by a pidfd while it is looked at, so a pid handed to another
/// process meanwhile reads as no Flatpak, as does anything that can't be read.
pub fn flatpak_of(proc: &Path, pid: u32) -> Option<String> {
    use smithay::reexports::rustix::{
        self,
        fs::{Mode, OFlags},
    };
    let pidfd = crate::capture::pidfd_open(pid)?;
    let root = rustix::fs::open(
        proc.join(pid.to_string()).join("root"),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let info = rustix::fs::openat(
        &root,
        ".flatpak-info",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let mut file = std::fs::File::from(info);
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(&mut file, 64 << 10), &mut text).ok()?;
    if !crate::capture::still_running(&pidfd) {
        return None;
    }
    flatpak_app(&text).map(str::to_string)
}

/// The app ID in the text of a `.flatpak-info`: `name` in its `[Application]` group.
fn flatpak_app(text: &str) -> Option<&str> {
    let mut group = "";
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            group = name;
        } else if group == "Application"
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            let app = value.trim();
            let valid = !app.is_empty()
                && app
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            return valid.then_some(app);
        }
    }
    None
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
    /// The app ID of the Flatpak its process runs in.
    pub flatpak: Option<&'a str>,
}

/// The client an item belongs to, if any can be told: the one whose process is the item's, the
/// one in the item's Flatpak sandbox, the nearest one the item's process started, the one from
/// the same Flatpak, else the one whose app ID is the item's ID or title. `names` are the item's
/// ID and title.
///
/// A name is a claim any app can make, so it counts only when nothing says otherwise: the item
/// has no process to go by, or the client descends from the item's process, however far down.
/// Nothing is matched across a Flatpak's edge, by name or by process: an item from one Flatpak is
/// never another's, and a host process is never a Flatpak app's.
pub fn owner(process: Option<&Process>, names: &[&str], clients: &[Client]) -> Option<usize> {
    let flatpak = process.and_then(|process| process.flatpak.as_deref());
    // A client whose Flatpak can't be read may still be in the item's: the sandbox and the
    // Flatpak's own app ID decide that below.
    let apart = |client: &Client| match (flatpak, client.flatpak) {
        (Some(ours), Some(theirs)) => ours != theirs,
        (None, Some(_)) => true,
        _ => false,
    };
    let pid = process.and_then(Process::pid);
    if let (Some(process), Some(pid)) = (process, pid) {
        if let Some(at) = clients
            .iter()
            .position(|client| !apart(client) && client.lineage.first() == Some(&pid))
        {
            return Some(at);
        }
        let nearest = |ancestor: u32, depth: usize| {
            clients
                .iter()
                .enumerate()
                .filter(|(_, client)| !apart(client))
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
    if let Some(flatpak) = flatpak {
        // A client in the same Flatpak first; else one whose own Flatpak can't be read but whose
        // app ID is this one's.
        return clients
            .iter()
            .position(|client| client.flatpak == Some(flatpak))
            .or_else(|| {
                clients
                    .iter()
                    .position(|client| client.flatpak.is_none() && same_app(client.app_id, flatpak))
            });
    }
    clients.iter().position(|client| {
        let related = pid.is_none_or(|pid| client.lineage.contains(&pid));
        related
            && client.flatpak.is_none()
            && names.iter().any(|name| same_app(client.app_id, name))
    })
}

/// Whether an app ID and an item's ID or title name the same app: equal but for case and a
/// `.desktop` ending, or once a reverse-DNS prefix is taken off either. Only a well-formed
/// reverse-DNS ID loses its prefix, so `.chat` or `chat.` is not `chat`.
pub fn same_app(app_id: &str, name: &str) -> bool {
    let plain = |id: &str| id.trim().trim_end_matches(".desktop").to_lowercase();
    let (app_id, name) = (plain(app_id), plain(name));
    if app_id.is_empty() || name.is_empty() {
        return false;
    }
    let last = |id: &str| {
        let reverse_dns = id.contains('.') && id.split('.').all(|part| !part.is_empty());
        reverse_dns
            .then(|| id.rsplit('.').next())
            .flatten()
            .map(str::to_string)
    };
    app_id == name || last(&app_id).as_ref() == Some(&name) || Some(&app_id) == last(&name).as_ref()
}

/// The desktop entry for an item with no client to go by: by its Flatpak's app ID, by its ID or
/// title as an app ID, then by the program its process runs.
pub fn entry<'a>(apps: &'a [App], names: &[&str], process: Option<&Process>) -> Option<&'a App> {
    let flatpak = process.and_then(|process| process.flatpak.as_deref());
    if let Some(app) = flatpak.and_then(|id| apps.iter().find(|app| same_app(&app.id, id))) {
        return Some(app);
    }
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
        Client {
            lineage,
            app_id,
            flatpak: None,
        }
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
            flatpak: None,
        };
        let clients = [
            client(&[700, 10], "org.example.Editor"),
            client(&[330, 320, 300, 10], "com.example.Chat"),
        ];
        assert_eq!(owner(Some(&proxy), &["chat"], &clients), Some(1));
    }

    #[test]
    fn an_orphaned_bus_proxy_finds_the_app_by_its_flatpak() {
        // The proxy's bwrap (2210) was reparented to init, so it shares no ancestor with the app
        // (2120, under bwraps 2105 and 2100, started by flatpak run's parent 2000). Only the Flatpak
        // they both run in says they are the same app's.
        let proxy = Process {
            lineage: vec![2211, 2210],
            sandbox: Some(2210),
            name: "xdg-dbus-proxy".into(),
            flatpak: Some("com.example.Chat".into()),
        };
        let lineage = [2120, 2115, 2105, 2100, 2000, 10];
        let terminal = [900, 10];
        let mut by_flatpak = [
            client(&terminal, "org.example.Terminal"),
            client(&lineage, "chat"),
        ];
        // The app's window gives an app ID of its own, not the Flatpak's.
        assert_eq!(
            owner(Some(&proxy), &["chat_status_icon_1", ""], &by_flatpak),
            Some(1),
            "by the client's app ID"
        );
        by_flatpak[1].app_id = "unrelated";
        by_flatpak[1].flatpak = Some("com.example.Chat");
        assert_eq!(
            owner(Some(&proxy), &["chat_status_icon_1", ""], &by_flatpak),
            Some(1),
            "by the client's own Flatpak"
        );
        by_flatpak[1].flatpak = Some("com.example.Other");
        assert_eq!(
            owner(Some(&proxy), &["chat_status_icon_1", ""], &by_flatpak),
            None,
            "another Flatpak's client"
        );
    }

    /// A made-up `/proc` under `target/test-scratch`, where this test process's own pid has
    /// `cgroup` and `root` as given, so it can be held by a pidfd like a real one.
    fn fake_proc(name: &str, cgroup: &str, flatpak_info: Option<&str>) -> std::path::PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let proc = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("matcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proc);
        let pid = proc.join(std::process::id().to_string());
        std::fs::create_dir_all(pid.join("root")).unwrap();
        std::fs::write(pid.join("cgroup"), cgroup).unwrap();
        if let Some(info) = flatpak_info {
            std::fs::write(pid.join("root/.flatpak-info"), info).unwrap();
        }
        proc
    }

    #[test]
    fn a_flatpaks_app_id_is_read_from_its_sandbox() {
        let proc = fake_proc(
            "sandboxed",
            "0::/\n",
            Some("[Application]\nname=com.example.Chat\nruntime=runtime/x\n"),
        );
        assert_eq!(
            flatpak_of(&proc, std::process::id()).as_deref(),
            Some("com.example.Chat")
        );
        assert_eq!(
            flatpak_app("[Instance]\nname=x\n[Application]\nname = org.example.two-words\n"),
            Some("org.example.two-words")
        );
        assert_eq!(flatpak_app("[Runtime]\nname=org.example.Platform\n"), None);
        assert_eq!(flatpak_app("[Application]\nname=../x\n"), None);
    }

    /// Any process can start itself in a scope named like a Flatpak's; that alone makes it no
    /// Flatpak.
    #[test]
    fn a_host_process_in_a_flatpak_looking_scope_isnt_that_flatpak() {
        let proc = fake_proc(
            "posing",
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
             app-flatpak-com.example.Chat-1234567890.scope\n",
            None,
        );
        assert_eq!(flatpak_of(&proc, std::process::id()), None);
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

    fn in_flatpak<'a>(lineage: &'a [u32], app_id: &'a str, flatpak: &'a str) -> Client<'a> {
        Client {
            lineage,
            app_id,
            flatpak: Some(flatpak),
        }
    }

    #[test]
    fn a_tray_item_cant_pose_as_another_app_by_its_name() {
        // An unrelated process names its item after a windowless app, and after an app with a
        // window up.
        let clients = [
            client(&[2, 10], "org.example.Vault"),
            client(&[3, 10], "org.example.Mail"),
        ];
        let hostile = process(&[666, 10]);
        assert_eq!(owner(Some(&hostile), &["vault", "Vault"], &clients), None);
        assert_eq!(
            owner(Some(&hostile), &["org.example.Mail", ""], &clients),
            None
        );
        // From inside a Flatpak, a name reaches no host app either.
        let sandboxed = Process {
            lineage: vec![700, 10],
            flatpak: Some("com.example.Game".into()),
            ..Process::default()
        };
        assert_eq!(owner(Some(&sandboxed), &["vault"], &clients), None);
        // And a host process is never a Flatpak app's, by name or by having started it.
        let flatpaks = [in_flatpak(&[820, 810, 800, 10], "chat", "com.example.Chat")];
        assert_eq!(
            owner(Some(&process(&[666, 10])), &["chat"], &flatpaks),
            None
        );
        assert_eq!(
            owner(Some(&process(&[800, 10])), &["chat"], &flatpaks),
            None
        );
        // Nor is one Flatpak's item another's.
        let other = Process {
            lineage: vec![900, 10],
            flatpak: Some("com.example.Other".into()),
            ..Process::default()
        };
        assert_eq!(owner(Some(&other), &["chat"], &flatpaks), None);
    }

    #[test]
    fn a_steam_shaped_tray_finds_its_web_helpers_window_however_deep() {
        // The client (steam) keeps the tray icon; the window is steamwebhelper's, several
        // processes down through the runtime's wrappers.
        let windows = [
            client(&[900, 10], "org.example.Terminal"),
            client(&[1460, 1450, 1440, 1430, 1420, 1400, 10], "steam"),
        ];
        assert_eq!(
            owner(Some(&process(&[1400, 10])), &["steam", "Steam"], &windows),
            Some(1)
        );
        // A process named the same that didn't start it doesn't.
        assert_eq!(
            owner(Some(&process(&[1500, 10])), &["steam", "Steam"], &windows),
            None
        );
    }

    #[test]
    fn a_discord_shaped_flatpak_finds_its_x11_window_by_flatpak() {
        // The item comes from the sandbox's bus proxy, orphaned to init; the window is the X11
        // client's, whose real pid runs in the same Flatpak.
        let proxy = Process {
            lineage: vec![3011, 3010],
            sandbox: Some(3010),
            name: "xdg-dbus-proxy".into(),
            flatpak: Some("com.example.Discord".into()),
        };
        let windows = [
            client(&[900, 10], "discord"),
            in_flatpak(
                &[3120, 3105, 3100, 3000, 10],
                "discord",
                "com.example.Discord",
            ),
        ];
        assert_eq!(
            owner(Some(&proxy), &["chrome_status_icon_1", ""], &windows),
            Some(1)
        );
    }

    #[test]
    fn a_chrome_shaped_flatpak_finds_its_wayland_window_by_flatpak() {
        let proxy = Process {
            lineage: vec![4011, 4010],
            sandbox: Some(4010),
            name: "xdg-dbus-proxy".into(),
            flatpak: Some("com.example.Chrome".into()),
        };
        let windows = [in_flatpak(
            &[4120, 4110, 4100, 3990, 10],
            "com.example.chrome",
            "com.example.Chrome",
        )];
        assert_eq!(
            owner(Some(&proxy), &["chrome_status_icon_1", ""], &windows),
            Some(0)
        );
    }

    #[test]
    fn an_xwaylandvideobridge_shaped_item_is_its_own_clients() {
        // A host service whose X11 client and tray item are one process, started by the
        // service manager.
        let clients = [
            client(&[900, 10], "org.example.Terminal"),
            client(&[5200, 1900], "xwaylandvideobridge"),
        ];
        let bridge = Process {
            lineage: vec![5200, 1900],
            name: "xwaylandvideobr".into(),
            ..Process::default()
        };
        assert_eq!(
            owner(
                Some(&bridge),
                &["Xwayland Video Bridge", "Wayland to X11 Video Bridge"],
                &clients
            ),
            Some(1)
        );
    }

    #[test]
    fn only_a_well_formed_app_id_loses_its_prefix() {
        assert!(same_app("com.example.Discord", "discord"));
        assert!(same_app("steam", "com.example.Steam.desktop"));
        assert!(!same_app(".discord", "discord"));
        assert!(!same_app("discord.", "discord"));
        assert!(!same_app("com..discord", "discord"));
        assert!(!same_app("com.example.Discord", "org.other.Discord"));
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
            app("com.example.Chat", "/usr/bin/flatpak"),
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
        let in_flatpak = Process {
            name: "xdg-dbus-proxy".into(),
            flatpak: Some("com.example.Chat".into()),
            ..Process::default()
        };
        assert_eq!(
            entry(&apps, &["chat_status_icon_1"], Some(&in_flatpak)).map(|app| app.id.as_str()),
            Some("com.example.Chat")
        );
    }
}

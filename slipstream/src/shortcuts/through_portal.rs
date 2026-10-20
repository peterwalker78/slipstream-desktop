//! An app binding a shortcut through the real xdg-desktop-portal, on a bus of the test's own.
//!
//! It starts a private `dbus-daemon` (with no services to activate, so nothing else on the machine
//! is started or touched), serves the backend on it with the portal configuration Slipstream
//! installs, starts xdg-desktop-portal against that, and then talks to the portal as an app does.
//! The person's answer and the key going down and up stand in for the main loop.
//!
//! Run with `cargo test -- --ignored through_xdg_desktop_portal`; it needs dbus-daemon and
//! xdg-desktop-portal installed.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keycode, Keysym},
    reexports::calloop::{EventLoop, channel},
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{
    Pending, Shortcuts,
    portal::{self, Backend, Event, Signal},
};
use crate::keys::Mods;

const CTRL_ALT: Mods = Mods {
    ctrl: true,
    alt: true,
    shift: false,
    logo: false,
};

/// Children killed however the test ends.
struct Kill(Vec<Child>, PathBuf);

impl Drop for Kill {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn portal_program() -> Option<PathBuf> {
    ["/usr/libexec", "/usr/lib", "/usr/lib/xdg-desktop-portal"]
        .iter()
        .map(|dir| Path::new(dir).join("xdg-desktop-portal"))
        .find(|path| path.is_file())
}

/// What a portal request came back with, waiting on its `Response` signal.
fn request(
    connection: &zbus::blocking::Connection,
    token: &str,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> (u32, HashMap<String, OwnedValue>) {
    let sender = connection
        .unique_name()
        .unwrap()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let response = zbus::blocking::Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .unwrap();
    let mut answers = response.receive_signal("Response").unwrap();
    let handle: OwnedObjectPath = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.GlobalShortcuts"),
            method,
            body,
        )
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(handle.as_str(), path);
    answers.next().unwrap().body().deserialize().unwrap()
}

fn text(value: &str) -> Value<'_> {
    Value::from(value)
}

/// The app's side: open a session, bind, list, then hear the shortcut go down and up.
fn app(address: String, done: mpsc::Sender<Vec<(String, String, u64)>>) {
    let connection = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .build()
        .unwrap();
    // The portal takes a moment to start.
    let bus = zbus::blocking::fdo::DBusProxy::new(&connection).unwrap();
    let desktop = zbus::names::BusName::try_from("org.freedesktop.portal.Desktop").unwrap();
    let started = Instant::now();
    while !bus.name_has_owner(desktop.clone()).unwrap() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the portal didn't start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let portal = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.GlobalShortcuts",
    )
    .unwrap();
    // An app that isn't sandboxed says who it is first, or the portal turns it away.
    connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &("org.example.Talk", HashMap::<&str, Value>::new()),
        )
        .unwrap();
    let mut activated = portal.receive_signal("Activated").unwrap();
    let mut deactivated = portal.receive_signal("Deactivated").unwrap();

    let options = HashMap::from([
        ("handle_token", text("open")),
        ("session_handle_token", text("talk")),
    ]);
    let (response, results) = request(&connection, "open", "CreateSession", &(options,));
    assert_eq!(response, 0, "CreateSession");
    // A string in the portal's documentation, an object path from some of its versions.
    let session = match &*results["session_handle"] {
        Value::Str(path) => path.to_string(),
        Value::ObjectPath(path) => path.to_string(),
        other => panic!("session_handle was {other:?}"),
    };
    let session = OwnedObjectPath::try_from(session).unwrap();

    let wanted = vec![(
        "talk",
        HashMap::from([
            ("description", text("Push to talk")),
            ("preferred_trigger", text("CTRL+ALT+t")),
        ]),
    )];
    let options = HashMap::from([("handle_token", text("bind"))]);
    let (response, results) = request(
        &connection,
        "bind",
        "BindShortcuts",
        &(&session, wanted, "", options),
    );
    assert_eq!(response, 0, "BindShortcuts");
    let bound: Vec<(String, HashMap<String, OwnedValue>)> = results["shortcuts"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(bound[0].0, "talk");
    let keys: String = bound[0].1["trigger_description"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(keys, "Ctrl+Alt+T");

    let options = HashMap::from([("handle_token", text("list"))]);
    let (response, results) = request(&connection, "list", "ListShortcuts", &(&session, options));
    assert_eq!(response, 0, "ListShortcuts");
    let listed: Vec<(String, HashMap<String, OwnedValue>)> = results["shortcuts"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(listed.len(), 1);

    let mut heard = Vec::new();
    for (name, signals) in [
        ("Activated", &mut activated),
        ("Deactivated", &mut deactivated),
    ] {
        let message = signals.next().unwrap();
        let (from, id, time, _): (OwnedObjectPath, String, u64, HashMap<String, OwnedValue>) =
            message.body().deserialize().unwrap();
        assert_eq!(from, session);
        heard.push((name.to_string(), id, time));
    }
    let _ = done.send(heard);
}

#[test]
#[ignore = "starts dbus-daemon and xdg-desktop-portal"]
fn through_xdg_desktop_portal() {
    let Some(xdp) = portal_program() else {
        panic!("xdg-desktop-portal isn't installed");
    };
    let dir = std::env::temp_dir().join(format!("slipstream-portal-test-{}", std::process::id()));
    let portals = dir.join("portals");
    let config = dir.join("config/xdg-desktop-portal");
    std::fs::create_dir_all(&portals).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        portals.join("slipstream.portal"),
        include_str!("../../../session/slipstream.portal"),
    )
    .unwrap();
    std::fs::write(
        config.join("slipstream-portals.conf"),
        include_str!("../../../session/slipstream-portals.conf"),
    )
    .unwrap();
    // The portal only takes an app ID it can find a desktop entry for.
    let applications = dir.join("data/applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(
        applications.join("org.example.Talk.desktop"),
        "[Desktop Entry]\nType=Application\nName=Talk\nExec=true\n",
    )
    .unwrap();
    // A bus of its own, with nothing to activate.
    let bus_config = dir.join("bus.conf");
    std::fs::write(
        &bus_config,
        format!(
            "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN\" \
             \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\
             <busconfig><type>session</type><listen>unix:path={}/bus</listen>\
             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
            dir.display()
        ),
    )
    .unwrap();
    let mut children = Kill(Vec::new(), dir.clone());
    let mut bus = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", bus_config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon isn't installed");
    let mut address = String::new();
    BufReader::new(bus.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_string();
    children.0.push(bus);

    // The backend, as the compositor serves it.
    let (events, received) = channel::channel::<Event>();
    let backend = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .name(crate::portal::NAME)
        .unwrap()
        .serve_at(crate::portal::PATH, Backend { events })
        .unwrap()
        .build()
        .unwrap();

    children.0.push(
        Command::new(xdp)
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("XDG_CURRENT_DESKTOP", "Slipstream")
            .env("XDG_DESKTOP_PORTAL_DIR", &portals)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_DATA_HOME", dir.join("data"))
            // The system's own, for GLib's settings schemas.
            .env(
                "XDG_DATA_DIRS",
                format!("{}:/usr/local/share:/usr/share", dir.join("data").display()),
            )
            .env("XDG_RUNTIME_DIR", &dir)
            .env("HOME", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let (done, heard) = mpsc::channel();
    let client = std::thread::spawn(move || app(address, done));

    // The main loop's side: the person allows the shortcut, then holds it down for a moment.
    struct Desk {
        shortcuts: Shortcuts,
        allowed: Option<Instant>,
    }
    let mut desk = Desk {
        shortcuts: Shortcuts::default(),
        allowed: None,
    };
    let mut event_loop: EventLoop<Desk> = EventLoop::try_new().unwrap();
    event_loop
        .handle()
        .insert_source(received, |event, _, desk| {
            let channel::Event::Msg(event) = event else {
                return;
            };
            match event {
                Event::Opened { session, app } => {
                    assert_eq!(app, "org.example.Talk");
                    desk.shortcuts.opened(session, app.clone(), app);
                }
                Event::Bind {
                    session,
                    wanted,
                    reply,
                } => {
                    assert!(
                        desk.shortcuts
                            .bind(Pending::new(session, wanted, reply), 0.0, true)
                    );
                    desk.shortcuts
                        .key(Keysym::Return, Mods::default(), 5.0, true);
                    desk.allowed = Some(Instant::now());
                }
                Event::List { session, reply } => {
                    let _ = reply.try_send(desk.shortcuts.list(&session));
                }
                Event::Closed { session } => {
                    desk.shortcuts.closed(&session);
                }
            }
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let key = Keycode::new(28);
    let (mut pressed, mut released) = (false, false);
    let heard = loop {
        assert!(Instant::now() < deadline, "the portal never answered");
        event_loop
            .dispatch(Some(Duration::from_millis(20)), &mut desk)
            .unwrap();
        if let Some(at) = desk.allowed {
            let send = |signal: Signal| portal::send(&backend, signal).unwrap();
            if !pressed && at.elapsed() > Duration::from_millis(500) {
                let signals = desk.shortcuts.press(key, CTRL_ALT, Keysym::t, 1000);
                assert_eq!(signals.len(), 1);
                signals.into_iter().for_each(send);
                pressed = true;
            } else if pressed && !released && at.elapsed() > Duration::from_millis(700) {
                desk.shortcuts.release(key, 1550).into_iter().for_each(send);
                released = true;
            }
        }
        if let Ok(heard) = heard.try_recv() {
            break heard;
        }
        assert!(!client.is_finished(), "the app's side failed");
    };
    assert_eq!(
        heard,
        vec![
            ("Activated".to_string(), "talk".to_string(), 1000),
            ("Deactivated".to_string(), "talk".to_string(), 1550),
        ]
    );
}

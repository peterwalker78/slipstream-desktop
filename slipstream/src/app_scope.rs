//! Each app in a systemd scope of its own, named `app-slipstream-<desktop ID>-<pid>.scope` as
//! desktops name them, so that anything telling apps apart by cgroup can: xdg-desktop-portal
//! reads an app's ID from it (the global shortcuts portal needs one), and the bar's meter counts
//! an app's helpers with it.
//!
//! The child is held between fork and exec until systemd has moved it, so nothing it starts is
//! left behind in the compositor's own cgroup. Without a systemd user manager on the session bus,
//! or when asking it fails, apps start exactly as they would without one.

use std::{
    fmt::Write as _,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::Command,
    sync::OnceLock,
    time::Duration,
};

use zbus::{
    Proxy,
    proxy::CacheProperties,
    zvariant::{OwnedObjectPath, Value},
};

/// Who started the app, the part of the name before its ID.
const LAUNCHER: &str = "slipstream";
/// The longest unit name systemd takes.
const UNIT_NAME_MAX: usize = 255;
/// How long a child waits to be moved before it runs anyway.
const HOLD: Duration = Duration::from_millis(250);

/// `id` escaped as systemd escapes a part of a unit name: anything but ASCII letters, digits, `:`,
/// `_` and a `.` that doesn't lead becomes `\xNN`. A `-` is escaped too, so it can't be taken for
/// the one before the pid.
pub fn escape(id: &str) -> String {
    let mut escaped = String::with_capacity(id.len());
    for (at, byte) in id.bytes().enumerate() {
        if byte.is_ascii_alphanumeric() || byte == b':' || byte == b'_' || (byte == b'.' && at > 0)
        {
            escaped.push(byte as char);
        } else {
            let _ = write!(escaped, "\\x{byte:02x}");
        }
    }
    escaped
}

/// The scope for app `id` started as process `pid`, or `None` without an ID or when the name
/// would be too long for systemd.
pub fn unit_name(id: &str, pid: u32) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    let name = format!("app-{LAUNCHER}-{}-{pid}.scope", escape(id));
    (name.len() <= UNIT_NAME_MAX).then_some(name)
}

/// The ID for a command that has no desktop entry: its program's file name.
pub fn command_id(command: &[String]) -> &str {
    command
        .first()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// What `Command::spawn` needs kept open until it returns: the child's ends of the two pipes.
pub struct Hold {
    _ends: [OwnedFd; 2],
}

/// Sets `process` up to wait, before it runs, until systemd has moved it into a scope for `id`.
/// `None` leaves it as it was: no ID, or no systemd user manager to ask. Keep the hold until
/// `spawn` has returned. `spawn` returns only once the child runs, so call it off the event loop.
pub fn hold(process: &mut Command, id: &str) -> Option<Hold> {
    // Before anything else: an ID too long for a unit name needs no bus.
    unit_name(id, u32::MAX)?;
    let manager = manager()?;
    hold_with(process, id, move |unit, pid| {
        zbus::block_on(futures_lite::future::or(
            start_scope(manager, unit, pid),
            async {
                async_io::Timer::after(HOLD).await;
                Err(zbus::Error::Failure("no answer in time".into()))
            },
        ))
        .map_err(|err| err.to_string())
    })
}

/// `hold`, with `mover` asking for the scope. The child runs once `mover` has answered, whatever
/// the answer, or once `HOLD` is up.
fn hold_with(
    process: &mut Command,
    id: &str,
    mover: impl FnOnce(&str, u32) -> Result<(), String> + Send + 'static,
) -> Option<Hold> {
    let (pid_from_child, pid_to_parent) = pipe()?;
    let (go_from_parent, go_to_child) = pipe()?;
    let (tell, wait) = (pid_to_parent.as_raw_fd(), go_from_parent.as_raw_fd());
    let timeout = HOLD.as_millis() as libc::c_int;
    // SAFETY: `getpid`, `write` and `poll` are async-signal-safe, which is all `pre_exec` asks.
    // Both pipes close on exec, so the app never sees them.
    unsafe {
        process.pre_exec(move || {
            let pid = (libc::getpid() as u32).to_ne_bytes();
            if libc::write(tell, pid.as_ptr().cast(), pid.len()) == pid.len() as isize {
                let mut go = libc::pollfd {
                    fd: wait,
                    events: libc::POLLIN,
                    revents: 0,
                };
                libc::poll(&mut go, 1, timeout);
            }
            Ok(())
        });
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        let mut pid = [0; 4];
        // Nothing to read means the child never got this far: the fork failed.
        if File::from(pid_from_child).read_exact(&mut pid).is_err() {
            return;
        }
        let pid = u32::from_ne_bytes(pid);
        let moved = match unit_name(&id, pid) {
            Some(unit) => mover(&unit, pid).map(|()| unit),
            None => Err("no unit name".to_string()),
        };
        let _ = File::from(go_to_child).write_all(&[1]);
        match moved {
            Ok(unit) => tracing::debug!(unit, "started in a scope of its own"),
            Err(err) => tracing::debug!(id, "started without a scope of its own: {err}"),
        }
    });
    Some(Hold {
        _ends: [pid_to_parent, go_from_parent],
    })
}

/// A pipe whose ends close on exec: (read, write).
fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe2` writes, which are then ours alone.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return None;
        }
        Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

/// The systemd user manager, asked once whether it is there.
fn manager() -> Option<&'static Proxy<'static>> {
    static MANAGER: OnceLock<Option<Proxy<'static>>> = OnceLock::new();
    MANAGER
        .get_or_init(|| match zbus::block_on(connect()) {
            Ok(manager) => Some(manager),
            Err(err) => {
                tracing::info!("apps start without scopes of their own: {err}");
                None
            }
        })
        .as_ref()
}

const SYSTEMD: &str = "org.freedesktop.systemd1";

async fn connect() -> zbus::Result<Proxy<'static>> {
    let connection = zbus::Connection::session().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    if !bus.name_has_owner(SYSTEMD.try_into()?).await? {
        return Err(zbus::Error::Failure(
            "no systemd user manager on the session bus".into(),
        ));
    }
    let manager = zbus::proxy::Builder::<Proxy<'static>>::new(&connection)
        .destination(SYSTEMD)?
        .path("/org/freedesktop/systemd1")?
        .interface("org.freedesktop.systemd1.Manager")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    // The manager sends `JobRemoved` only once somebody has asked for its signals.
    manager.call_method("Subscribe", &()).await?;
    Ok(manager)
}

/// Asks for a scope named `unit` holding `pid`, and waits for systemd to have started it.
async fn start_scope(manager: &Proxy<'static>, unit: &str, pid: u32) -> zbus::Result<()> {
    use futures_lite::StreamExt;
    // Listening before asking, so the answer can't come first.
    let mut removed = manager.receive_signal("JobRemoved").await?;
    let properties: Vec<(&str, Value)> = vec![
        ("PIDs", Value::from(vec![pid])),
        ("Slice", Value::from("app.slice")),
        ("CollectMode", Value::from("inactive-or-failed")),
    ];
    let auxiliary: Vec<(&str, Vec<(&str, Value)>)> = Vec::new();
    let job: OwnedObjectPath = manager
        .call("StartTransientUnit", &(unit, "fail", properties, auxiliary))
        .await?;
    while let Some(signal) = removed.next().await {
        let (_, path, _, result): (u32, OwnedObjectPath, String, String) =
            signal.body().deserialize()?;
        if path == job {
            return match result.as_str() {
                "done" => Ok(()),
                other => Err(zbus::Error::Failure(format!(
                    "the scope's job ended {other}"
                ))),
            };
        }
    }
    Err(zbus::Error::Failure("the bus went away".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The app ID back from a unit name, as xdg-desktop-portal reads it:
    /// `app-[<launcher>-]<ID>-<random>.scope`, with `\xNN` escapes undone.
    fn portal_app_id(unit: &str) -> Option<String> {
        let inner = unit.strip_prefix("app-")?.strip_suffix(".scope")?;
        let (named, random) = inner.rsplit_once('-')?;
        assert!(random.chars().all(|c| c.is_ascii_alphanumeric()));
        let id = match named.split_once('-') {
            Some((launcher, id)) if launcher.chars().all(|c| c.is_ascii_alphanumeric()) => id,
            _ => named,
        };
        let mut bytes = Vec::new();
        let mut rest = id.as_bytes();
        while let Some((&byte, after)) = rest.split_first() {
            if byte == b'\\' && after.first() == Some(&b'x') {
                let hex = std::str::from_utf8(after.get(1..3)?).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                rest = &after[3..];
            } else {
                bytes.push(byte);
                rest = after;
            }
        }
        String::from_utf8(bytes).ok()
    }

    #[test]
    fn desktop_ids_are_escaped_as_systemd_escapes_unit_names() {
        assert_eq!(escape("org.example.Notes"), "org.example.Notes");
        assert_eq!(escape("notes-editor"), "notes\\x2deditor");
        assert_eq!(escape(".hidden"), "\\x2ehidden");
        assert_eq!(escape("a b/c"), "a\\x20b\\x2fc");
        assert_eq!(escape("caf\u{e9}"), "caf\\xc3\\xa9");
    }

    #[test]
    fn the_portal_reads_back_the_desktop_id_from_the_scope() {
        for id in [
            "org.example.Notes",
            "notes-editor",
            "foot",
            "my app",
            ".x-y",
        ] {
            let unit = unit_name(id, 4242).unwrap();
            assert!(unit.starts_with("app-slipstream-"), "{unit}");
            assert_eq!(portal_app_id(&unit).as_deref(), Some(id), "{unit}");
        }
    }

    #[test]
    fn the_meter_counts_the_scope_as_the_apps_own() {
        let unit = unit_name("org.example.Notes", 77).unwrap();
        let path = format!("/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}");
        assert!(crate::usage::is_app_cgroup(&path));
    }

    #[test]
    fn no_scope_without_an_id_or_with_one_too_long() {
        assert_eq!(unit_name("", 1), None);
        assert_eq!(unit_name(&"x".repeat(300), 1), None);
        let mut process = Command::new("true");
        assert!(hold(&mut process, "").is_none());
        assert!(hold(&mut process, &"x".repeat(300)).is_none());
        assert!(process.status().unwrap().success());
    }

    #[test]
    fn a_bare_command_is_known_by_its_program() {
        let command = ["/usr/bin/notes-editor".to_string(), "--new".to_string()];
        assert_eq!(command_id(&command), "notes-editor");
        assert_eq!(command_id(&[]), "");
    }

    #[test]
    fn the_child_waits_to_be_moved_and_runs_after() {
        let asked = Arc::new(Mutex::new(None));
        let record = asked.clone();
        let mut process = Command::new("true");
        let hold = hold_with(&mut process, "org.example.Notes", move |unit, pid| {
            *record.lock().unwrap() = Some((unit.to_string(), pid));
            Ok(())
        });
        let mut child = process.spawn().unwrap();
        drop(hold);
        assert!(child.wait().unwrap().success());
        let (unit, pid) = asked.lock().unwrap().clone().unwrap();
        assert_eq!(pid, child.id());
        assert_eq!(
            unit,
            format!("app-slipstream-org.example.Notes-{pid}.scope")
        );
    }

    #[test]
    fn the_child_runs_anyway_when_systemd_says_no() {
        let mut process = Command::new("true");
        let hold = hold_with(&mut process, "org.example.Notes", |_, _| {
            Err("refused".to_string())
        });
        let mut child = process.spawn().unwrap();
        drop(hold);
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn the_child_runs_anyway_when_systemd_never_answers() {
        let mut process = Command::new("true");
        let hold = hold_with(&mut process, "org.example.Notes", |_, _| {
            std::thread::sleep(Duration::from_secs(5));
            Ok(())
        });
        let started = std::time::Instant::now();
        let mut child = process.spawn().unwrap();
        drop(hold);
        assert!(child.wait().unwrap().success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

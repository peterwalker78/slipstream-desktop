//! Follows changes on disk. The explorer's app list keeps in step with what's installed, as KDE's
//! menus do: every applications folder (Flatpak exports included) is watched, and any change
//! there, an app installed, removed or updated, reads the apps and their icons again once the
//! changes settle. The recent files list is read again when an app rewrites it. Settings apply as
//! soon as the Settings app saves them.

use std::{cell::Cell, mem::MaybeUninit, path::PathBuf, rc::Rc, time::Duration};

use smithay::reexports::{
    calloop::{
        Interest, LoopHandle, Mode, PostAction,
        generic::Generic,
        timer::{TimeoutAction, Timer},
    },
    rustix::{
        fs::inotify::{self, CreateFlags, Event, WatchFlags},
        io::Errno,
    },
};

use crate::{Slipstream, apps, settings};

/// Package managers write many files in a burst; read the apps once they've finished.
const APPS_SETTLE: Duration = Duration::from_millis(500);
/// Apps write the recent files list once per file opened, sometimes twice in a row.
const RECENT_SETTLE: Duration = Duration::from_millis(200);
/// The Settings app renames a finished file into place, but editors may take a few writes.
const SETTINGS_SETTLE: Duration = Duration::from_millis(100);

pub fn watch_apps(handle: &LoopHandle<'static, Slipstream>) {
    watch(
        handle,
        "apps being installed or removed",
        apps::application_dirs(),
        APPS_SETTLE,
        |_| true,
        |state| {
            tracing::info!("apps changed; reading them again");
            state.explorer.rescan_now();
        },
    );
}

/// Recently used files: GTK and KDE apps rewrite the list as files are opened.
pub fn watch_recent_files(handle: &LoopHandle<'static, Slipstream>) {
    watch(
        handle,
        "recently used files",
        vec![apps::data_home()],
        RECENT_SETTLE,
        |event| {
            event
                .file_name()
                .is_some_and(|name| name.to_bytes() == apps::RECENT_LIST.as_bytes())
        },
        |state| state.explorer.read_recent(),
    );
}

pub fn watch_settings(handle: &LoopHandle<'static, Slipstream>) {
    let path = slipstream_config::path();
    let Some(dir) = path.parent() else {
        return;
    };
    // Only a folder that exists can be watched, and it's where the file will go.
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), "can't watch for settings changes: {err}");
        return;
    }
    watch(
        handle,
        "settings changes",
        vec![dir.to_path_buf()],
        SETTINGS_SETTLE,
        |event| {
            event
                .file_name()
                .is_some_and(|name| name.to_bytes() == slipstream_config::FILE_NAME.as_bytes())
        },
        |state| {
            if let Some(settings) = settings::reload() {
                state.apply_settings(settings);
            }
        },
    );
}

/// Calls `changed` once the events in `dirs` that `wanted` picks out have settled for `settle`.
fn watch(
    handle: &LoopHandle<'static, Slipstream>,
    what: &'static str,
    dirs: Vec<PathBuf>,
    settle: Duration,
    wanted: fn(&Event<'_>) -> bool,
    changed: fn(&mut Slipstream),
) {
    let fd = match inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK) {
        Ok(fd) => fd,
        Err(err) => {
            tracing::warn!("can't watch for {what}: {err}");
            return;
        }
    };
    let changes = WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::ATTRIB;
    let folders = dirs
        .into_iter()
        .filter(|dir| inotify::add_watch(&fd, dir.as_path(), changes).is_ok())
        .count();
    tracing::info!(folders, "watching for {what}");

    let pending = Rc::new(Cell::new(false));
    let inserted = handle.insert_source(
        Generic::new(fd, Interest::READ, Mode::Level),
        move |_, fd, state| {
            let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
            let mut reader = inotify::Reader::new(&**fd, &mut buffer);
            let mut relevant = false;
            loop {
                match reader.next() {
                    Ok(event) => relevant |= wanted(&event),
                    Err(Errno::INTR) => continue,
                    Err(Errno::AGAIN) => break,
                    Err(err) => {
                        tracing::warn!("the watch for {what} failed: {err}");
                        break;
                    }
                }
            }
            if relevant && !pending.get() {
                pending.set(true);
                let reset = pending.clone();
                let settled = state.loop_handle.insert_source(
                    Timer::from_duration(settle),
                    move |_, _, state| {
                        reset.set(false);
                        changed(state);
                        TimeoutAction::Drop
                    },
                );
                if settled.is_err() {
                    pending.set(false);
                }
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(err) = inserted {
        tracing::warn!("can't watch for {what}: {}", err.error);
    }
}

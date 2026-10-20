//! The memory the desktop session is using now: the proportional share (PSS, swapped-out pages
//! included) of the session's own processes, the way other desktops' session figures are taken.

use std::{fs, os::unix::fs::MetadataExt, path::Path};

/// The session's processes, by the name the kernel gives each one.
const SESSION: [&str; 11] = [
    "slipstream",
    "Xwayland",
    "xdg-desktop-portal",
    "xdg-desktop-portal-wlr",
    "pipewire",
    "wireplumber",
    "pipewire-pulse",
    "dbus-broker",
    "dbus-daemon",
    "at-spi2-registryd",
    "at-spi-bus-launcher",
];

/// The kernel keeps only this much of a process's name.
const COMM_LEN: usize = 15;

/// Megabytes, or `None` when Slipstream itself isn't among this user's processes.
pub fn measure() -> Option<f64> {
    let uid = fs::metadata("/proc/self").ok()?.uid();
    let mut kib = 0;
    let mut compositor = false;
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let path = entry.path();
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
            || fs::metadata(&path).map(|m| m.uid()).ok() != Some(uid)
        {
            continue;
        }
        let Some(name) = session_name(&path) else {
            continue;
        };
        let Some(pss) = fs::read_to_string(path.join("smaps_rollup"))
            .ok()
            .and_then(|rollup| proportional_kib(&rollup))
        else {
            continue;
        };
        compositor |= name == "slipstream";
        kib += pss;
    }
    compositor.then(|| kib as f64 / 1024.0)
}

/// Which session process `proc` (a `/proc/PID` folder) is. A name the kernel has cut short is
/// told apart by the file its program was started from.
fn session_name(proc: &Path) -> Option<&'static str> {
    let comm = fs::read_to_string(proc.join("comm")).ok()?;
    let comm = comm.trim_end();
    let mut long = None;
    for name in SESSION {
        if name.len() <= COMM_LEN {
            if name == comm {
                return Some(name);
            }
        } else if name.get(..COMM_LEN) == Some(comm) {
            let exe = long.get_or_insert_with(|| fs::read_link(proc.join("exe")).ok());
            if exe
                .as_ref()
                .and_then(|exe| exe.file_name())
                .is_some_and(|file| file == name)
            {
                return Some(name);
            }
        }
    }
    None
}

/// `Pss` plus `SwapPss` from a process's `smaps_rollup`, in KiB.
fn proportional_kib(rollup: &str) -> Option<u64> {
    let field = |key: &str| {
        rollup.lines().find_map(|line| {
            line.strip_prefix(key)?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<u64>()
                .ok()
        })
    };
    Some(field("Pss:")? + field("SwapPss:").unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_proportional_share_and_its_swapped_part() {
        let rollup = "55d0-7ffd ---p 00000000 00:00 0  [rollup]\n\
                      Rss:              120000 kB\n\
                      Pss:               90000 kB\n\
                      Pss_Anon:          60000 kB\n\
                      Swap:              10000 kB\n\
                      SwapPss:            8000 kB\n";
        assert_eq!(proportional_kib(rollup), Some(98000));
        assert_eq!(proportional_kib("Rss: 1 kB\n"), None);
    }
}

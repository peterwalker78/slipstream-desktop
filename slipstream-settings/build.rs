//! Counts the source lines of each part of Slipstream for the About page's map of where its code
//! goes, and the bytes of source it carries. Every compositor module is placed in a part below; a
//! test fails on one that isn't, and on a name that no longer matches a module.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

/// The parts, each with the compositor modules it holds. A module is named by its path under
/// `slipstream/src` without `.rs`; a file in a folder is placed by its own path if that's listed,
/// otherwise by its folder's.
const PARTS: &[(&str, &[&str])] = &[
    (
        "Windows and tiling",
        &[
            "main",
            "layout",
            "floating",
            "focus",
            "grabs",
            "arrange",
            "gravity",
            "motion",
            "layers",
            "handlers",
            "xwayland",
            "workspace",
            "dock",
            "slot",
            "ghost",
            "fx",
            "live",
            "debug",
            "state",
            "render",
        ],
    ),
    (
        // The faded desktop's traces count with the wallpaper they're drawn on.
        "Living wallpaper and screensaver",
        &[
            "saver",
            "glmatrix",
            "glass",
            "idle",
            "screensaver",
            "scope",
            "working",
        ],
    ),
    (
        "Streams and rain",
        &[
            "rain",
            "streaks",
            "stream_tip",
            "usage",
            "tcp",
            "state/streams",
        ],
    ),
    (
        "Bullet time and Alt+Tab",
        &[
            "bullet",
            "overview",
            "tilt",
            "sound",
            "deck",
            "switcher",
            "pane",
            "state/bullet_time",
            "render/deck",
        ],
    ),
    (
        "Drawing and text",
        &[
            "paint",
            "text",
            "fallback",
            "icons",
            "anim",
            "panel",
            "card",
            "render/chrome",
            "state/ui",
        ],
    ),
    (
        "App explorer",
        &["explorer", "apps", "calc", "emoji", "launch", "watch"],
    ),
    (
        "Bar, quick settings and notifications",
        &[
            "bar",
            "quick",
            "status",
            "centre",
            "calendar",
            "notify",
            "notices",
            "alert",
            "meter",
            "osd",
            "toast",
            "history",
            "clipboard",
            "edit",
            "media",
            "battery",
            "files",
            "concentration",
        ],
    ),
    (
        "Lock, login and the way out",
        &[
            "lock",
            "auth",
            "exit",
            "offer",
            "power",
            "logind",
            "session",
            "restore",
            "resume",
            "polkit",
            "state/lifecycle",
            "render/lock",
        ],
    ),
    (
        "Keys, tour and help",
        &[
            "keys",
            "keynames",
            "tour",
            "miniature",
            "sheet",
            "awake",
            "shortcuts",
            "takeback",
            "state/app_shortcuts",
        ],
    ),
    (
        "Screens, sharing and screenshots",
        &[
            "screen",
            "connect",
            "known",
            "capture",
            "share",
            "screencopy",
            "screenshot",
            "snip",
            "zoom",
            "scanout",
            "vrr",
            "gamma",
            "nightlight",
            "inhibit",
            "state/screens",
            "render/png_file",
        ],
    ),
    (
        "Hardware, input and settings",
        &[
            "udev",
            "winit",
            "input",
            "devices",
            "layouts",
            "ime",
            "cursor",
            "settings",
            "appearance",
            "portal",
        ],
    ),
];

/// The Settings app and the settings file it shares with the compositor.
const SETTINGS_APP: &str = "The Settings app";

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let compositor = manifest.join("../slipstream/src");
    let config = manifest.join("../slipstream-config/src");
    let own = manifest.join("src");
    for dir in [&compositor, &config, &own] {
        println!("cargo::rerun-if-changed={}", dir.display());
    }

    let mut lines = vec![0u64; PARTS.len()];
    let mut used = vec![false; PARTS.iter().map(|(_, modules)| modules.len()).sum()];
    let mut unplaced = Vec::new();
    let mut bytes = 0;
    for file in sources(&compositor) {
        let text = fs::read_to_string(&file).unwrap_or_default();
        bytes += text.len() as u64;
        let module = file
            .strip_prefix(&compositor)
            .unwrap()
            .with_extension("")
            .to_string_lossy()
            .into_owned();
        let folder = module.split('/').next().unwrap_or_default();
        let found = [module.as_str(), folder].into_iter().find_map(|name| {
            PARTS
                .iter()
                .flat_map(|(_, modules)| modules.iter())
                .position(|listed| *listed == name)
        });
        match found {
            Some(at) => {
                used[at] = true;
                lines[part_of(at)] += text.lines().count() as u64;
            }
            None => unplaced.push(module),
        }
    }
    let mut settings_lines = 0;
    for file in sources(&config).into_iter().chain(sources(&own)) {
        let text = fs::read_to_string(&file).unwrap_or_default();
        bytes += text.len() as u64;
        settings_lines += text.lines().count() as u64;
    }

    let mut parts: Vec<(&str, u64)> = PARTS.iter().map(|(name, _)| *name).zip(lines).collect();
    parts.push((SETTINGS_APP, settings_lines));
    parts.sort_by_key(|part| std::cmp::Reverse(part.1));
    let stale: Vec<&str> = PARTS
        .iter()
        .flat_map(|(_, modules)| modules.iter().copied())
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(name, _)| name)
        .collect();

    let mut out = String::new();
    writeln!(out, "pub const CODE_PARTS: &[(&str, u64)] = &{parts:?};").unwrap();
    writeln!(out, "pub const CODE_BYTES: u64 = {bytes};").unwrap();
    writeln!(
        out,
        "#[cfg(test)]\npub const UNPLACED: &[&str] = &{unplaced:?};"
    )
    .unwrap();
    writeln!(out, "#[cfg(test)]\npub const STALE: &[&str] = &{stale:?};").unwrap();
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("code_parts.rs"), out).unwrap();
}

/// Which part the `at`th listed module belongs to.
fn part_of(mut at: usize) -> usize {
    for (part, (_, modules)) in PARTS.iter().enumerate() {
        if at < modules.len() {
            return part;
        }
        at -= modules.len();
    }
    unreachable!()
}

/// Every `.rs` file under `dir`, in a steady order.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

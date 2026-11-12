//! Slipstream's fonts, handed to Pango from the files installed beside the programs, so the app
//! matches the desktop without installing fonts system-wide.

use std::{fs, path::PathBuf};

use gtk::prelude::*;
use gtk4 as gtk;
use pango::prelude::*;

const FONTS: [&str; 5] = [
    "AtkinsonHyperlegible-Regular.ttf",
    "AtkinsonHyperlegible-Bold.ttf",
    "ChakraPetch-Bold.ttf",
    "JetBrainsMono-Regular.ttf",
    "JetBrainsMono-Bold.ttf",
];

/// Where the fonts are: `share/slipstream/fonts` beside the `bin` folder this program is in, as
/// installed, or a source tree's `assets/fonts` for a build run from its `target` folder.
fn fonts_dir() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let bin = program.parent()?;
    let installed = bin.parent()?.join("share").join("slipstream").join("fonts");
    let built = bin.parent()?.parent()?.join("assets").join("fonts");
    [installed, built]
        .into_iter()
        .find(|dir| dir.join(FONTS[0]).is_file())
}

/// Makes the fonts available to this app. Without them, the stylesheet's fallbacks apply.
pub fn register() {
    // Earlier versions unpacked a copy of the fonts here.
    let unpacked = slipstream_config::xdg_home("XDG_CACHE_HOME", ".cache")
        .join("slipstream")
        .join("fonts");
    let _ = fs::remove_dir_all(unpacked);
    let Some(dir) = fonts_dir() else {
        eprintln!("slipstream-settings: Slipstream's fonts aren't installed beside it");
        return;
    };
    let Some(font_map) = gtk::Label::new(None).pango_context().font_map() else {
        return;
    };
    for name in FONTS {
        let path = dir.join(name);
        if let Err(err) = font_map.add_font_file(&path) {
            eprintln!("slipstream-settings: can't load {}: {err}", path.display());
        }
    }
}

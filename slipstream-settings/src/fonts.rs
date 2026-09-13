//! The mockup's fonts, embedded as the compositor embeds them, and handed to Pango from files in
//! the cache folder, so the app looks like the mockup without installing fonts system-wide.

use std::{fs, path::PathBuf};

use gtk::prelude::*;
use gtk4 as gtk;
use pango::prelude::*;

const FONTS: [(&str, &[u8]); 5] = [
    (
        "AtkinsonHyperlegible-Regular.ttf",
        include_bytes!("../../assets/fonts/AtkinsonHyperlegible-Regular.ttf"),
    ),
    (
        "AtkinsonHyperlegible-Bold.ttf",
        include_bytes!("../../assets/fonts/AtkinsonHyperlegible-Bold.ttf"),
    ),
    (
        "ChakraPetch-Bold.ttf",
        include_bytes!("../../assets/fonts/ChakraPetch-Bold.ttf"),
    ),
    (
        "JetBrainsMono-Regular.ttf",
        include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    ),
    (
        "JetBrainsMono-Bold.ttf",
        include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
    ),
];

/// Makes the fonts available to this app. Without them, the stylesheet's fallbacks apply.
pub fn register() {
    let dir = cache_dir().join("slipstream").join("fonts");
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!(
            "slipstream-settings: can't unpack fonts into {}: {err}",
            dir.display()
        );
        return;
    }
    let Some(font_map) = gtk::Label::new(None).pango_context().font_map() else {
        return;
    };
    for (name, bytes) in FONTS {
        let path = dir.join(name);
        let unpacked = fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64);
        if !unpacked {
            let partial = dir.join(format!("{name}.partial"));
            if let Err(err) = fs::write(&partial, bytes).and_then(|()| fs::rename(&partial, &path))
            {
                eprintln!(
                    "slipstream-settings: can't unpack {}: {err}",
                    path.display()
                );
                continue;
            }
        }
        if let Err(err) = font_map.add_font_file(&path) {
            eprintln!("slipstream-settings: can't load {}: {err}", path.display());
        }
    }
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
}

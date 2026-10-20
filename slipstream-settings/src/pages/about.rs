//! About: which Slipstream this is, on what, and how much it weighs.

use super::*;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .css_classes(["main"])
        .build();
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 2);
    hero.append(&label("Slipstream", "hero"));
    hero.append(&label(&format!("Desktop on {}", os_name()), "page-sub"));
    page.append(&hero);

    let software = group(&page, "Software");
    row(
        &software,
        "Version",
        None,
        &label(env!("CARGO_PKG_VERSION"), "kv"),
    );
    let file = label(&home_relative(store.path()), "kv");
    file.set_selectable(true);
    row(
        &software,
        "Settings file",
        Some("Hand edits apply as soon as they're saved."),
        &file,
    );
    weight::section(&page);
    page.upcast()
}

/// The distribution's name and version from `/etc/os-release`, such as "Bazzite 44".
fn os_name() -> String {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| {
        release.lines().find_map(|line| {
            line.strip_prefix(key)?
                .strip_prefix('=')
                .map(|value| value.trim_matches('"').to_string())
        })
    };
    match (field("NAME"), field("VERSION_ID")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        _ => "Linux".to_string(),
    }
}

fn home_relative(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

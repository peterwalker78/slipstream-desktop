//! Shortcuts: whose habits the desktop's keys follow.

use slipstream_config::KeyStyle;

use super::*;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Shortcuts",
        "The shortcuts your hands already know. The shortcut sheet and the tour show the keys of \
         the style chosen here.",
    );
    let styles = group(&page, "Shortcuts");
    let keys = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.append(&keys);
    fill_keys(&keys, store);
    let refill = refiller(&keys, store, fill_keys);
    let chosen = store.get().keys.style;
    let mut first: Option<gtk::CheckButton> = None;
    for style in KeyStyle::ALL {
        let pick = gtk::CheckButton::new();
        if let Some(first) = &first {
            pick.set_group(Some(first));
        }
        pick.set_active(style == chosen);
        let (store, refill) = (store.clone(), refill.clone());
        pick.connect_toggled(move |pick| {
            if pick.is_active() {
                store.change(move |settings| settings.keys.style = style);
                refill();
            }
        });
        row(&styles, style.label(), Some(style.summary()), &pick);
        first.get_or_insert(pick);
    }
    fill_app_shortcuts(&group(&page, "Apps' own shortcuts"), store);
    page.upcast()
}

/// Shortcuts apps were allowed to keep, which work whichever window has the keyboard, each of
/// which can be taken away. Built again from the file whenever one is.
fn fill_app_shortcuts(card: &gtk::Box, store: &Store) {
    use slipstream_config::shortcuts;

    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let granted = shortcuts::read(&shortcuts::path());
    let given: Vec<_> = granted
        .apps
        .iter()
        .flat_map(|app| app.shortcuts.iter().map(move |s| (app, s)))
        .collect();
    if given.is_empty() {
        row(
            card,
            "No app has shortcuts of its own",
            Some(
                "An app such as a voice chat can ask for keys that work while you're in another \
                 window. Slipstream asks you first, and what you allow appears here.",
            ),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    let refill = refiller(card, store, fill_app_shortcuts);
    for (app, shortcut) in given {
        let what = if shortcut.description.is_empty() {
            &shortcut.id
        } else {
            &shortcut.description
        };
        let keys = if shortcut.keys.is_empty() {
            "Left out"
        } else {
            &shortcut.keys
        };
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        controls.append(&label(keys, "kv"));
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Take it away, and ask again next time"));
        delete.add_css_class("flat");
        {
            let refill = refill.clone();
            let (app, id) = (app.id.clone(), shortcut.id.clone());
            delete.connect_clicked(move |_| {
                let path = shortcuts::path();
                let mut granted = shortcuts::read(&path);
                if granted.forget(&app, &id)
                    && let Err(err) = shortcuts::write(&path, &granted)
                {
                    eprintln!("slipstream-settings: couldn't take the shortcut away: {err}");
                }
                refill();
            });
        }
        controls.append(&delete);
        row(card, &sentence(what), Some(&app.name), &controls);
    }
}

/// Every key of the chosen style, in the shortcut sheet's groups, each beside what it does. The
/// compositor beside this app says what they are, so the list is the sheet's own.
fn fill_keys(sheet: &gtk::Box, store: &Store) {
    while let Some(child) = sheet.first_child() {
        sheet.remove(&child);
    }
    let style = store.get().keys.style;
    let listing = crate::previews::compositor()
        .and_then(|compositor| {
            std::process::Command::new(compositor)
                .args(["--list-keys", style.id()])
                .output()
                .ok()
        })
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let groups = parse_listing(&listing);
    if groups.is_empty() {
        let card = group(sheet, "Keys in this style");
        row(
            &card,
            "The list of keys isn't available",
            Some("The shortcut sheet still shows every key."),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    for (title, rows) in groups {
        let card = group(sheet, title);
        for (does, keys) in rows {
            let keys: Vec<&str> = keys
                .iter()
                .map(|&key| if key == "Super" { "Super, tapped" } else { key })
                .collect();
            let sub = does
                .starts_with("zoom the screen")
                .then_some("Super and the scroll wheel zoom too.");
            row(&card, &sentence(does), sub, &label(&keys.join("   "), "kv"));
        }
    }
}

/// A group of the list of keys: its title, and what each of its rows does beside that row's keys.
type KeyGroup<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>);

/// The groups in `slipstream --list-keys`'s output.
fn parse_listing(listing: &str) -> Vec<KeyGroup<'_>> {
    let mut groups: Vec<KeyGroup> = Vec::new();
    for line in listing.lines() {
        match line.strip_prefix('\t') {
            Some(row) => {
                let mut fields = row.split('\t');
                if let (Some(does), Some((_, rows))) = (fields.next(), groups.last_mut()) {
                    rows.push((does, fields.collect()));
                }
            }
            None if !line.is_empty() => groups.push((line, Vec::new())),
            None => {}
        }
    }
    groups
}

/// `does` as a sentence's start: its first letter a capital.
fn sentence(does: &str) -> String {
    let mut chars = does.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_is_read_as_groups_of_rows() {
        let listing = "Apps and windows\n\tsearch apps and files\tSuper\tSuper+S\n\
                       \tclose window\tAlt+F4\n\nBullet time\n\topen\tSuper+B\n";
        assert_eq!(
            parse_listing(listing),
            [
                (
                    "Apps and windows",
                    vec![
                        ("search apps and files", vec!["Super", "Super+S"]),
                        ("close window", vec!["Alt+F4"]),
                    ]
                ),
                ("Bullet time", vec![("open", vec!["Super+B"])]),
            ]
        );
        assert!(parse_listing("").is_empty());
    }
}

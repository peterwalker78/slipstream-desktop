//! Shortcuts: whose habits the desktop's keys follow.

use slipstream_config::{KeyStyle, plugins};

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
    // Built here so a change of style can flag any that clash; placed beside the apps' own.
    let own_group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let own = group(&own_group, "Your own shortcuts");
    fill_own(&own, store);
    let refill_own = refiller(&own, store, fill_own);
    let plugin_group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let plugin_keys = group(&plugin_group, "Plugins' actions");
    fill_plugin_keys(&plugin_keys, store);
    let refill_plugins = refiller(&plugin_keys, store, fill_plugin_keys);
    let chosen = store.get().keys.style;
    let mut first: Option<gtk::CheckButton> = None;
    for style in KeyStyle::ALL {
        let pick = gtk::CheckButton::new();
        if let Some(first) = &first {
            pick.set_group(Some(first));
        }
        pick.set_active(style == chosen);
        let (store, refill, refill_own) = (store.clone(), refill.clone(), refill_own.clone());
        let refill_plugins = refill_plugins.clone();
        pick.connect_toggled(move |pick| {
            if pick.is_active() {
                store.change(move |settings| settings.keys.style = style);
                refill();
                refill_own();
                refill_plugins();
            }
        });
        row(&styles, style.label(), Some(style.summary()), &pick);
        first.get_or_insert(pick);
    }
    page.append(&own_group);
    page.append(&plugin_group);
    fill_app_shortcuts(&group(&page, "Apps' own shortcuts"), store);
    page.upcast()
}

/// Keys as the shortcuts specification writes them, as a person reads them, and why they can't
/// open an app, empty when they can: the compositor beside this app knows its keys, so it says.
struct Checked {
    spec: String,
    label: String,
    why: String,
}

/// `specs` checked against `style`, one for each, or `None` without the compositor to ask.
fn check_keys(style: KeyStyle, specs: &[String]) -> Option<Vec<Checked>> {
    let output = std::process::Command::new(crate::previews::compositor()?)
        .arg("--check-keys")
        .arg(style.id())
        .args(specs)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let checked: Vec<Checked> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let mut fields = line.splitn(3, '\t').map(String::from);
            let mut next = || fields.next().unwrap_or_default();
            Checked {
                spec: next(),
                label: next(),
                why: next(),
            }
        })
        .collect();
    (checked.len() == specs.len()).then_some(checked)
}

/// The installed apps a menu would show, by desktop file ID without `.desktop`, and name, in
/// order of name.
fn installed_apps() -> Vec<(String, String)> {
    use gtk::gio::prelude::*;

    let mut apps: Vec<(String, String)> = gtk::gio::AppInfo::all()
        .into_iter()
        .filter(|app| app.should_show())
        .filter_map(|app| {
            let id = app.id()?;
            let id = id.strip_suffix(".desktop").unwrap_or(&id).to_string();
            Some((id, app.display_name().to_string()))
        })
        .collect();
    apps.sort_by_cached_key(|(_, name)| name.to_lowercase());
    apps
}

/// The person's own shortcuts, each opening an app and each removable, and a row adding one: an
/// app is chosen, then its keys pressed. One the chosen style uses for something else stays, and
/// says why it does nothing.
fn fill_own(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let keys = store.get().keys;
    let apps = Rc::new(installed_apps());
    let name_of = {
        let apps = apps.clone();
        move |id: &str| {
            apps.iter()
                .find(|(app, _)| app == id)
                .map_or_else(|| id.to_string(), |(_, name)| name.clone())
        }
    };
    let specs: Vec<String> = keys.apps.iter().map(|key| key.keys.clone()).collect();
    let checked = check_keys(keys.style, &specs);
    let refill = refiller(card, store, fill_own);
    for (index, key) in keys.apps.iter().enumerate() {
        let (shown, why) = match checked.as_ref().map(|checked| &checked[index]) {
            Some(it) => {
                // Of two on the same keys, the first works.
                let earlier = checked
                    .as_ref()
                    .and_then(|checked| (0..index).find(|&before| checked[before].spec == it.spec));
                let why = match earlier {
                    Some(before) if it.why.is_empty() => format!(
                        "{} already opens {}.",
                        it.label,
                        name_of(&keys.apps[before].app)
                    ),
                    _ => it.why.clone(),
                };
                (it.label.clone(), why)
            }
            None => (key.keys.clone(), String::new()),
        };
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        controls.append(&label(&shown, "kv"));
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Take it away"));
        delete.add_css_class("flat");
        {
            let (store, refill, spec) = (store.clone(), refill.clone(), key.keys.clone());
            delete.connect_clicked(move |_| {
                store.change(|settings| settings.keys.remove_app_key(&spec));
                refill();
            });
        }
        controls.append(&delete);
        let sub = (!why.is_empty()).then(|| format!("Does nothing in this style. {why}"));
        row(
            card,
            &format!("Open {}", name_of(&key.app)),
            sub.as_deref(),
            &controls,
        );
    }
    add_own(card, store, apps, refill);
}

/// The row that adds a shortcut. Choosing an app turns it to taking keys: the window holds every
/// key meanwhile, so Slipstream's own reach it to be named as taken, and Esc stops.
fn add_own(
    card: &gtk::Box,
    store: &Store,
    apps: Rc<Vec<(String, String)>>,
    refill: impl Fn() + Clone + 'static,
) {
    const ASK: &str = "Choose an app, then press the keys that open it.";
    let mut names = vec!["Choose an app"];
    names.extend(apps.iter().map(|(_, name)| name.as_str()));
    let pick = gtk::DropDown::new(
        Some(gtk::StringList::new(&names)),
        Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )),
    );
    pick.set_enable_search(true);
    let listen = gtk::Button::with_label("Stop");
    listen.set_tooltip_text(Some("Stop, without adding a shortcut"));
    listen.set_visible(false);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    controls.append(&pick);
    controls.append(&listen);
    let Some(sub) = row(card, "Add a shortcut", Some(ASK), &controls) else {
        return;
    };
    let toplevel = {
        let card = card.downgrade();
        move || {
            card.upgrade()
                .and_then(|card| card.root())
                .and_then(|root| root.surface())
                .and_then(|surface| surface.downcast::<gtk::gdk::Toplevel>().ok())
        }
    };
    let chosen: Rc<Cell<Option<usize>>> = Rc::default();
    let stop = {
        let (toplevel, refill) = (toplevel.clone(), refill.clone());
        move || {
            if let Some(toplevel) = toplevel() {
                toplevel.restore_system_shortcuts();
            }
            refill();
        }
    };
    {
        let (apps, listen, sub, chosen) =
            (apps.clone(), listen.clone(), sub.clone(), chosen.clone());
        let toplevel = toplevel.clone();
        pick.connect_selected_notify(move |pick| {
            let Some(index) = (pick.selected() as usize).checked_sub(1) else {
                return;
            };
            chosen.set(Some(index));
            pick.set_visible(false);
            listen.set_visible(true);
            listen.grab_focus();
            sub.set_text(&format!(
                "Press the keys that open {}. Esc stops.",
                apps[index].1
            ));
            if let Some(toplevel) = toplevel() {
                toplevel.inhibit_system_shortcuts(None::<&gtk::gdk::Event>);
            }
        });
    }
    {
        let stop = stop.clone();
        listen.connect_clicked(move |_| stop());
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let store = store.clone();
    keys.connect_key_pressed(move |controller, key, keycode, state| {
        let Some(index) = chosen.get() else {
            return gtk::glib::Propagation::Proceed;
        };
        let held = gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::SHIFT_MASK;
        if key == gtk::gdk::Key::Escape && !state.intersects(held) {
            stop();
            return gtk::glib::Propagation::Stop;
        }
        let Some(spec) = pressed(controller, key, keycode, state) else {
            return gtk::glib::Propagation::Stop;
        };
        // The keys pressed are checked together with the ones already kept, to find any already
        // opening an app.
        let settings = store.get();
        let mut specs = vec![spec];
        specs.extend(settings.keys.apps.iter().map(|own| own.keys.clone()));
        let Some(mut checked) = check_keys(settings.keys.style, &specs) else {
            sub.set_text("Slipstream isn't installed beside Settings, so keys can't be checked.");
            return gtk::glib::Propagation::Stop;
        };
        let kept = checked.split_off(1);
        let checked = checked.remove(0);
        let opens = kept
            .iter()
            .position(|it| it.spec == checked.spec)
            .map(|index| &settings.keys.apps[index]);
        let does = plugin_action_on(&settings, &checked.spec);
        let why = match (opens, does) {
            (Some(own), _) if checked.why.is_empty() => format!(
                "{} already opens {}.",
                checked.label,
                apps.iter()
                    .find(|(id, _)| *id == own.app)
                    .map_or(own.app.as_str(), |(_, name)| name.as_str())
            ),
            (None, Some(does)) if checked.why.is_empty() => {
                format!("{} already does {does}.", checked.label)
            }
            _ => checked.why,
        };
        if !why.is_empty() {
            sub.set_text(&format!("{why} Try other keys, or Esc to stop."));
            return gtk::glib::Propagation::Stop;
        }
        let app = apps[index].0.clone();
        store.change(|settings| settings.keys.add_app_key(&checked.spec, &app));
        stop();
        gtk::glib::Propagation::Stop
    });
    listen.add_controller(keys);
}

/// The keys pressed, as the shortcuts specification writes them, or none for a modifier alone.
fn pressed(
    controller: &gtk::EventControllerKey,
    key: gtk::gdk::Key,
    keycode: u32,
    state: gtk::gdk::ModifierType,
) -> Option<String> {
    // The key's own symbol, as it reads without Shift: Shift+1 is 1 held with Shift.
    let plain = controller
        .widget()
        .and_then(|widget| {
            widget.display().translate_key(
                keycode,
                gtk::gdk::ModifierType::empty(),
                controller.group() as i32,
            )
        })
        .map_or(key, |(plain, ..)| plain);
    let name = plain.name().filter(|name| !is_modifier(name))?;
    let mut spec = String::new();
    for (mask, part) in [
        (gtk::gdk::ModifierType::SUPER_MASK, "LOGO+"),
        (gtk::gdk::ModifierType::CONTROL_MASK, "CTRL+"),
        (gtk::gdk::ModifierType::ALT_MASK, "ALT+"),
        (gtk::gdk::ModifierType::SHIFT_MASK, "SHIFT+"),
    ] {
        if state.contains(mask) {
            spec.push_str(part);
        }
    }
    spec.push_str(&name);
    Some(spec)
}

/// A plugin that's on and offers actions: its id, its name and the actions.
type Offered = (String, String, Vec<plugins::surfaces::Action>);

/// The plugins that are on and offer actions.
fn plugin_actions(settings: &Settings) -> Vec<Offered> {
    plugins::find()
        .0
        .into_iter()
        .filter(|plugin| !plugin.manifest.actions.is_empty() && settings.plugins.approved(plugin))
        .map(|plugin| (plugin.id, plugin.manifest.name, plugin.manifest.actions))
        .collect()
}

/// What the plugin action on `spec` does, as "Start the timer in Timer", if one is.
fn plugin_action_on(settings: &Settings, spec: &str) -> Option<String> {
    let key = settings.keys.plugins.iter().find(|key| key.keys == spec)?;
    let offered = plugin_actions(settings);
    let (_, name, actions) = offered.iter().find(|(id, ..)| *id == key.plugin)?;
    let action = actions.iter().find(|action| action.id == key.action)?;
    Some(format!("{} in {name}", action.label))
}

/// The actions the plugins that are on offer, each with the keys you gave it, which can be taken
/// away, or a button to give it some. The group is hidden while no plugin offers any.
fn fill_plugin_keys(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let settings = store.get();
    let offered = plugin_actions(&settings);
    if let Some(group) = card.parent() {
        group.set_visible(!offered.is_empty());
    }
    let refill = refiller(card, store, fill_plugin_keys);
    let given: Vec<String> = settings
        .keys
        .plugins
        .iter()
        .map(|key| key.keys.clone())
        .collect();
    let checked = check_keys(settings.keys.style, &given);
    for (plugin, name, actions) in &offered {
        for action in actions {
            let at = settings
                .keys
                .plugins
                .iter()
                .position(|key| key.plugin == *plugin && key.action == action.id);
            let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let Some(at) = at else {
                let choose = gtk::Button::with_label("Choose keys");
                controls.append(&choose);
                if let Some(line) = row(card, &action.label, Some(name), &controls) {
                    let target = (plugin.clone(), action.clone());
                    listen_for_keys(&choose, &line, store, target, refill.clone());
                }
                continue;
            };
            let spec = &settings.keys.plugins[at].keys;
            let (shown, why) = match checked.as_ref().map(|checked| &checked[at]) {
                Some(it) => {
                    // An app's own shortcut on the same keys works first.
                    let app = settings.keys.apps.iter().find(|own| own.keys == it.spec);
                    let why = match app {
                        Some(own) if it.why.is_empty() => {
                            format!("{} already opens {}.", it.label, own.app)
                        }
                        _ => it.why.clone(),
                    };
                    (it.label.clone(), why)
                }
                None => (spec.clone(), String::new()),
            };
            let sub = if why.is_empty() {
                name.clone()
            } else {
                format!("{name}. Does nothing in this style. {why}")
            };
            controls.append(&label(&shown, "kv"));
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.set_tooltip_text(Some("Take its keys away"));
            delete.add_css_class("flat");
            {
                let (store, refill) = (store.clone(), refill.clone());
                let (plugin, action) = (plugin.clone(), action.id.clone());
                delete.connect_clicked(move |_| {
                    store.change(|settings| settings.keys.remove_plugin_key(&plugin, &action));
                    refill();
                });
            }
            controls.append(&delete);
            row(card, &action.label, Some(&sub), &controls);
        }
    }
}

/// `choose` takes keys for a plugin's action once pressed: the window holds every key
/// meanwhile, so Slipstream's own reach it to be named as taken, and Esc stops. Keys something
/// else has are refused, saying why on `line`.
fn listen_for_keys(
    choose: &gtk::Button,
    line: &gtk::Label,
    store: &Store,
    (plugin, action): (String, plugins::surfaces::Action),
    refill: impl Fn() + Clone + 'static,
) {
    let listening: Rc<Cell<bool>> = Rc::default();
    let toplevel = {
        let choose = choose.downgrade();
        move || {
            choose
                .upgrade()
                .and_then(|choose| choose.root())
                .and_then(|root| root.surface())
                .and_then(|surface| surface.downcast::<gtk::gdk::Toplevel>().ok())
        }
    };
    let stop = {
        let toplevel = toplevel.clone();
        move || {
            if let Some(toplevel) = toplevel() {
                toplevel.restore_system_shortcuts();
            }
            refill();
        }
    };
    {
        let (listening, line, stop) = (listening.clone(), line.clone(), stop.clone());
        let label = action.label.clone();
        choose.connect_clicked(move |choose| {
            if listening.replace(true) {
                stop();
                return;
            }
            choose.set_label("Stop");
            line.set_text(&format!("Press the keys for {label}. Esc stops."));
            if let Some(toplevel) = toplevel() {
                toplevel.inhibit_system_shortcuts(None::<&gtk::gdk::Event>);
            }
        });
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (store, line) = (store.clone(), line.clone());
    keys.connect_key_pressed(move |controller, key, keycode, state| {
        if !listening.get() {
            return gtk::glib::Propagation::Proceed;
        }
        let held = gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::SHIFT_MASK;
        if key == gtk::gdk::Key::Escape && !state.intersects(held) {
            stop();
            return gtk::glib::Propagation::Stop;
        }
        let Some(spec) = pressed(controller, key, keycode, state) else {
            return gtk::glib::Propagation::Stop;
        };
        let settings = store.get();
        let Some(mut checked) = check_keys(settings.keys.style, &[spec]) else {
            line.set_text("Slipstream isn't installed beside Settings, so keys can't be checked.");
            return gtk::glib::Propagation::Stop;
        };
        let checked = checked.remove(0);
        let app = settings
            .keys
            .apps
            .iter()
            .find(|own| own.keys == checked.spec);
        let why = if !checked.why.is_empty() {
            checked.why
        } else if let Some(own) = app {
            format!("{} already opens {}.", checked.label, own.app)
        } else if let Some(does) = plugin_action_on(&settings, &checked.spec) {
            format!("{} already does {does}.", checked.label)
        } else {
            String::new()
        };
        if !why.is_empty() {
            line.set_text(&format!("{why} Try other keys, or Esc to stop."));
            return gtk::glib::Propagation::Stop;
        }
        store.change(|settings| {
            settings
                .keys
                .add_plugin_key(&checked.spec, &plugin, &action.id)
        });
        stop();
        gtk::glib::Propagation::Stop
    });
    choose.add_controller(keys);
}

/// Whether a key's symbol name is a modifier's, which only ever goes with another key.
fn is_modifier(name: &str) -> bool {
    [
        "Shift",
        "Control",
        "Alt",
        "Super",
        "Meta",
        "Hyper",
        "ISO_Level",
        "Num_Lock",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
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

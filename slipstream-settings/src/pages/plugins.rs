//! Plugins: little programs that each add a card to the panel beside the clock. This page lists
//! them, turns them on and off, chooses the one whose headline shows on the chip, and adds them
//! from a folder or a git address. Adding one never runs anything of it, and it starts off.

use std::path::{Path, PathBuf};

use slipstream_config::plugins::{self, Plugin, approval, store};

use super::*;

/// The guide to writing a plugin.
const HOW_TO: &str =
    "https://github.com/peterwalker78/slipstream-desktop/blob/main/guide/plugins.md";

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Plugins",
        "Little apps that add a card to the panel beside the clock. Each one is a program of its \
         own and can do anything you can, so only add plugins you trust.",
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.append(&list);
    fill(&list, store);
    page.upcast()
}

/// The page's groups, built again whenever a plugin is added, removed or turned on or off.
fn fill(list: &gtk::Box, store: &Store) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let settings = store.get();
    let refill = refiller(list, store, fill);

    let add = group(list, "Add a plugin");
    add_rows(&add, refill.clone());

    let yours = group(list, "Your plugins");
    // Folders that aren't plugins go here, between the plugins and what a plugin can do.
    let skipped = gtk::Box::new(gtk::Orientation::Vertical, 18);
    list.append(&skipped);

    let about = group(list, "What a plugin can do");
    let link = gtk::LinkButton::with_label(HOW_TO, "How to write one");
    row(
        &about,
        "Anything you can",
        Some(
            "A plugin's program runs as you, from a copy of its folder taken when you turn it \
             on, without your session's keys or sockets. Its card is only ever shown, and its \
             buttons only run its own program again. A plugin that changes after you turn it on \
             waits until you turn it on again.",
        ),
        &link,
    );

    // Plugin folders are read and fingerprinted off the GTK thread, since a big one takes a
    // while; the rows go in once that's done, unless the page has been built again meanwhile.
    let (list, store) = (list.downgrade(), store.clone());
    let approved = settings.plugins.clone();
    gtk::glib::spawn_future_local(async move {
        let looked = gtk::gio::spawn_blocking(move || look(&approved))
            .await
            .unwrap_or_default();
        let Some(list) = list.upgrade() else {
            return;
        };
        if !yours.is_ancestor(&list) {
            return;
        }
        if looked.plugins.is_empty() {
            row(
                &yours,
                "No plugins yet",
                Some(
                    "Add one above. It starts off, so you can read its files before turning it on.",
                ),
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
            );
        }
        for (plugin, explained) in &looked.plugins {
            plugin_row(&yours, &store, plugin, explained, &settings, refill.clone());
        }
        if !looked.problems.is_empty() {
            let group = group(&skipped, "Folders that aren't plugins");
            for problem in &looked.problems {
                let (path, why) = problem.split_once(": ").unwrap_or((problem, ""));
                row(
                    &group,
                    path,
                    Some(why),
                    &gtk::Box::new(gtk::Orientation::Horizontal, 0),
                );
            }
        }
    });
}

/// Every plugin, each held one with what its update means, and the folders that aren't plugins.
#[derive(Default)]
struct Looked {
    plugins: Vec<(Plugin, Vec<String>)>,
    problems: Vec<String>,
}

/// Looks at every plugin folder, as `approved` says each was turned on.
fn look(approved: &plugins::Plugins) -> Looked {
    let (found, problems) = plugins::find();
    let plugins = found
        .into_iter()
        .map(|plugin| {
            let held = approved.is_on(&plugin.id) && !approved.approved(&plugin);
            let explained = if held && plugin.too_big.is_none() {
                // Compared with what it asked for when it was turned on, if that was kept for the
                // approval the settings hold.
                let change = approval::load(&plugin.id)
                    .filter(|kept| approved.fingerprints.get(&plugin.id) == Some(&kept.fingerprint))
                    .map(|kept| approval::compare(&kept, &approval::Approval::of(&plugin)));
                approval::explain(&plugin, change.as_ref(), &approval::source(&plugin))
            } else {
                Vec::new()
            };
            (plugin, explained)
        })
        .collect();
    Looked { plugins, problems }
}

/// Adding a plugin: from a git address, or a folder.
fn add_rows(card: &gtk::Box, refill: impl Fn() + Clone + 'static) {
    let address = gtk::Entry::builder()
        .placeholder_text("https://github.com/someone/weather-plugin")
        .width_chars(30)
        .build();
    let clone = gtk::Button::with_label("Add");
    let controls = gtk::Box::builder().spacing(8).build();
    controls.append(&address);
    controls.append(&clone);
    let said = row(
        card,
        "From a git address",
        Some("Copied as it is now. Nothing in it runs until you turn it on."),
        &controls,
    )
    .expect("the row has a line of explanation");
    {
        let (field, said, refill) = (address.clone(), said.clone(), refill.clone());
        let add = move |button: &gtk::Button| {
            let url = field.text().trim().to_string();
            button.set_sensitive(false);
            said.set_label("Copying…");
            let (button, said, address, refill) =
                (button.clone(), said.clone(), field.clone(), refill.clone());
            gtk::glib::spawn_future_local(async move {
                let added = gtk::gio::spawn_blocking(move || add_from_git(&url))
                    .await
                    .unwrap_or_else(|_| Err("It couldn't be copied.".into()));
                button.set_sensitive(true);
                match added {
                    Ok(name) => {
                        address.set_text("");
                        said.set_label(&format!("Added {name}. Turn it on below."));
                        refill();
                    }
                    Err(why) => said.set_label(&why),
                }
            });
        };
        clone.connect_clicked(add.clone());
        let button = clone.clone();
        address.connect_activate(move |_| add(&button));
    }

    let choose = gtk::Button::with_label("Choose a folder…");
    let said = row(
        card,
        "From a folder",
        Some("A folder holding a plugin.toml. It's copied, so the original can go."),
        &choose,
    )
    .expect("the row has a line of explanation");
    choose.connect_clicked(move |button| {
        let window = button.root().and_downcast::<gtk::Window>();
        let (said, refill) = (said.clone(), refill.clone());
        gtk::glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Choose a plugin's folder")
                .modal(true)
                .build();
            let Ok(folder) = dialog.select_folder_future(window.as_ref()).await else {
                return;
            };
            let Some(path) = folder.path() else {
                said.set_label("That folder isn't on this computer.");
                return;
            };
            let added = gtk::gio::spawn_blocking(move || add_from_folder(&path))
                .await
                .unwrap_or_else(|_| Err("It couldn't be copied.".into()));
            match added {
                Ok(name) => {
                    said.set_label(&format!("Added {name}. Turn it on below."));
                    refill();
                }
                Err(why) => said.set_label(&why),
            }
        });
    });
}

/// A plugin's row: what it is and what it runs, then where its headline goes, removing it, and
/// its switch. `explained` says what a held plugin's update means.
fn plugin_row(
    card: &gtk::Box,
    store: &Store,
    plugin: &Plugin,
    explained: &[String],
    settings: &Settings,
    refill: impl Fn() + Clone + 'static,
) {
    let on = settings.plugins.is_on(&plugin.id);
    let held = on && !settings.plugins.approved(plugin);
    let mut about = Vec::new();
    if !plugin.manifest.description.trim().is_empty() {
        about.push(plugin.manifest.description.trim().to_string());
    }
    // What it runs, and what a held update means, go under the rest in smaller print.
    let mut details = vec![format!("Runs {}", plugin.manifest.exec.join(" "))];
    details.extend(explained.iter().cloned());
    if about.is_empty() {
        about.push(details.remove(0));
    }
    if !plugin.own {
        about.push(format!(
            "Installed for everyone in {}",
            plugin.dir.display()
        ));
    }
    if let Some(why) = &plugin.too_big {
        about.push(format!(
            "Too big to check: {why}. It can't be turned on until it holds less."
        ));
    }
    let controls = gtk::Box::builder().spacing(10).build();
    if on && !held {
        controls.append(&glance_choice(store, &plugin.id, settings, refill.clone()));
    }
    if plugin.own {
        let remove = remove_button();
        let (store_c, refill_c, id, dir) = (
            store.clone(),
            refill.clone(),
            plugin.id.clone(),
            plugin.dir.clone(),
        );
        remove.connect_clicked(move |button| {
            if confirmed(button) {
                store_c.change(|settings| settings.plugins.set_on(&id, None));
                approval::forget(&id);
                let removed = std::fs::remove_dir_all(&dir).and_then(|()| store::remove(&id));
                if let Err(err) = removed {
                    button.set_tooltip_text(Some(&format!("Couldn't remove it: {err}")));
                }
                refill_c();
            }
        });
        controls.append(&remove);
    }
    // Centred, as every control in the row is: the Remove button beside it is taller, and a
    // switch left to fill the row's height is drawn stretched.
    let switch = gtk::Switch::builder()
        .active(on && !held)
        .sensitive(on || plugin.too_big.is_none())
        .valign(gtk::Align::Center)
        .build();
    let (store_c, shown) = (store.clone(), plugin.clone());
    switch.connect_state_set(move |_, turned_on| {
        // What is approved is what this row showed: its fingerprint, and what it asks for, kept
        // to compare an update with. If the folder has changed since, the compositor holds it.
        if turned_on {
            if shown.too_big.is_some() {
                return gtk::glib::Propagation::Stop;
            }
            let _ = approval::save(&shown);
        } else {
            approval::forget(&shown.id);
        }
        let fingerprint = turned_on.then(|| shown.fingerprint.clone());
        store_c.change(|settings| settings.plugins.set_on(&shown.id, fingerprint));
        refill();
        gtk::glib::Propagation::Proceed
    });
    controls.append(&switch);
    let sub = row(
        card,
        &plugin.manifest.name,
        Some(&about.join("\n")),
        &controls,
    )
    .expect("the row has a line of explanation");
    let Some(text) = sub.parent().and_downcast::<gtk::Box>() else {
        return;
    };
    if held {
        let updated = label(
            "Updated, so it's off until you switch it on again.",
            "row-sub",
        );
        updated.add_css_class("needed");
        text.append(&updated);
    }
    if !details.is_empty() {
        let small = label(&details.join("\n"), "row-sub");
        small.add_css_class("detail");
        text.append(&small);
    }
}

/// The box that puts plugin `id`'s headline on the chip, or takes it off. Only one plugin's can
/// be there, so the page is built again after a change, untick the one that was.
fn glance_choice(
    store: &Store,
    id: &str,
    settings: &Settings,
    refill: impl Fn() + 'static,
) -> gtk::CheckButton {
    let shown = settings.plugins.glance() == Some(id);
    let choice = gtk::CheckButton::builder()
        .label("On the chip")
        .active(shown)
        .valign(gtk::Align::Center)
        .build();
    let (store, id) = (store.clone(), id.to_string());
    choice.connect_toggled(move |choice| {
        let on = choice.is_active();
        store.change(|settings| {
            if on {
                settings.plugins.glance = Some(id.clone());
            } else if settings.plugins.glance() == Some(id.as_str()) {
                settings.plugins.glance = None;
            }
        });
        refill();
    });
    choice
}

/// A Remove button, centred in its row.
fn remove_button() -> gtk::Button {
    gtk::Button::builder()
        .label("Remove")
        .valign(gtk::Align::Center)
        .build()
}

/// Whether a Remove button's press is the second within a few seconds: the first only asks.
fn confirmed(button: &gtk::Button) -> bool {
    if button.label().as_deref() == Some("Remove for good?") {
        return true;
    }
    button.set_label("Remove for good?");
    let button = button.downgrade();
    gtk::glib::timeout_add_local_once(Duration::from_secs(4), move || {
        if let Some(button) = button.upgrade() {
            button.set_label("Remove");
        }
    });
    false
}

/// Copies the plugin in `source` into the person's plugins folder. Returns its name.
fn add_from_folder(source: &Path) -> Result<String, String> {
    let plugin = plugins::read(source).map_err(|why| format!("That isn't a plugin: {why}."))?;
    let target = plugins::own_dir().join(&plugin.id);
    if target.exists() {
        return Err(format!(
            "There's a plugin called {} already. Remove it first to add this one.",
            plugin.id
        ));
    }
    copy_folder(source, &target).map_err(|err| format!("It couldn't be copied: {err}."))?;
    Ok(plugin.manifest.name)
}

/// Clones `url` (an https address, nothing else) into a staging folder, checks it's a plugin, and
/// moves it into the person's plugins folder without git's own files. Returns its name.
fn add_from_git(url: &str) -> Result<String, String> {
    let id = id_from_url(url).ok_or(
        "That isn't an https:// address of a git repository. Other kinds aren't taken, since \
         git can run commands for some of them.",
    )?;
    let target = plugins::own_dir().join(&id);
    if target.exists() {
        return Err(format!(
            "There's a plugin called {id} already. Remove it first to add this one."
        ));
    }
    let staging = slipstream_config::xdg_home("XDG_CACHE_HOME", ".cache")
        .join("slipstream")
        .join("plugin-staging")
        .join(&id);
    let _ = std::fs::remove_dir_all(&staging);
    let mut git = std::process::Command::new("git");
    git.args([
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
    ])
    .args([
        "clone",
        "--depth",
        "1",
        "--no-recurse-submodules",
        "--",
        url,
    ])
    .arg(&staging)
    .env("GIT_TERMINAL_PROMPT", "0");
    match slipstream_config::command::run(git, Duration::from_secs(120)) {
        slipstream_config::command::Run::Printed(_) => {}
        slipstream_config::command::Run::CouldntStart(_) => {
            return Err("Adding from a git address needs git, which isn't installed.".into());
        }
        failed => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(git_failed(&failed));
        }
    }
    let _ = std::fs::remove_dir_all(staging.join(".git"));
    let added = add_from_folder(&staging);
    let _ = std::fs::remove_dir_all(&staging);
    added
}

/// Why git couldn't copy a plugin, in git's own last words where it gave some.
fn git_failed(run: &slipstream_config::command::Run) -> String {
    use slipstream_config::command::Run;
    match run {
        Run::Failed { error, .. } if !error.is_empty() => {
            let error = error.strip_prefix("fatal: ").unwrap_or(error);
            format!("git couldn't copy it: {error}")
        }
        Run::TimedOut => "git took more than two minutes, so it was stopped.".into(),
        _ => "git couldn't copy it.".into(),
    }
}

/// A plugin's id from its repository's address: the last part of the path, without `.git`.
fn id_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let name = rest
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .trim_end_matches(".git");
    (rest.contains('/') && plugins::valid_id(name)).then(|| name.to_string())
}

/// Copies the folder `from` to `to`, files and folders; anything else, such as a link, isn't
/// copied (a plugin can't hold one, and `plugins::read` has already said so).
fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target: PathBuf = to.join(entry.file_name());
        if kind.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_s_failure_reads_as_its_last_line() {
        use slipstream_config::command::Run;
        let failed = Run::Failed {
            code: Some(128),
            error: "fatal: repository 'https://example.org/a/timer/' not found".into(),
        };
        assert_eq!(
            git_failed(&failed),
            "git couldn't copy it: repository 'https://example.org/a/timer/' not found"
        );
    }

    #[test]
    fn only_an_https_address_names_a_plugin() {
        assert_eq!(
            id_from_url("https://github.com/someone/weather-plugin.git").as_deref(),
            Some("weather-plugin")
        );
        assert_eq!(
            id_from_url("https://example.org/plugins/timer/").as_deref(),
            Some("timer")
        );
        for refused in [
            "http://example.org/a/timer",
            "ext::sh -c touch% /tmp/x",
            "file:///home/someone/timer",
            "git@github.com:someone/timer.git",
            "https://example.org",
        ] {
            assert_eq!(id_from_url(refused), None, "{refused}");
        }
    }
}

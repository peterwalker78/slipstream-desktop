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
        "Little apps that add a card to the panel beside the clock. Each one says what it can do \
         before you switch it on, and can't do anything else.",
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

    let yours = cards(list, "Your plugins");
    // Folders that aren't plugins go here, between the plugins and what a plugin can do.
    let skipped = gtk::Box::new(gtk::Orientation::Vertical, 18);
    list.append(&skipped);

    let about = group(list, "What a plugin can do");
    let link = gtk::LinkButton::with_label(HOW_TO, "How to write one");
    row(
        &about,
        "Only what it lists",
        Some("Each plugin runs in a sandbox, and can do only what's listed under its name."),
        &link,
    );
    // What each verdict beside a plugin's name means, a row each, in its own colour.
    for (care, means) in [
        (
            plugins::Care::Safe,
            "It can't see or send anything of yours that matters.",
        ),
        (
            plugins::Care::Thought,
            "It can see something private, or reach beyond itself. Think about who made it.",
        ),
        (
            plugins::Care::Full,
            "It runs outside the sandbox, as a program you install yourself does. Switch it on \
             only if you trust where it came from.",
        ),
    ] {
        let line = gtk::Box::builder().spacing(14).css_classes(["row"]).build();
        // A column as wide as the widest, so the meanings line up.
        let column = gtk::Box::builder().width_request(130).build();
        let verdict = verdict_label(care);
        verdict.set_valign(gtk::Align::Center);
        column.append(&verdict);
        line.append(&column);
        let says = label(means, "row-sub");
        says.set_hexpand(true);
        says.set_valign(gtk::Align::Center);
        line.append(&says);
        about.append(&line);
    }
    row(
        &about,
        "Updates wait for you",
        Some(
            "A plugin that changes after you switch it on stays off until you switch it on again.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
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
            let card = card(&yours);
            row(
                &card,
                "No plugins yet",
                Some(
                    "Add one above. It starts off, so you can read its files before turning it on.",
                ),
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
            );
        }
        for (plugin, explained) in &looked.plugins {
            plugin_card(&yours, &store, plugin, explained, &settings, refill.clone());
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
                approval::explain(change.as_ref(), &approval::source(&plugin))
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
        .width_chars(16)
        .hexpand(true)
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

/// A titled column of cards in `list`, one per plugin, so what belongs to each stays together.
fn cards(list: &gtk::Box, title: &str) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 7);
    group.append(&label(&title.to_uppercase(), "group-title"));
    let cards = gtk::Box::new(gtk::Orientation::Vertical, 12);
    group.append(&cards);
    list.append(&group);
    cards
}

/// A card of its own in `cards`, its rows divided as a group's are.
fn card(cards: &gtk::Box) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["card"])
        .build();
    cards.append(&card);
    card
}

/// A plugin's card. On top, its name, how safe it is and its switch; under that, anything that
/// stops the switch, then what it can do, then where its headline goes and removing it.
/// `explained` says what a held plugin's update means.
fn plugin_card(
    cards: &gtk::Box,
    store: &Store,
    plugin: &Plugin,
    explained: &[String],
    settings: &Settings,
    refill: impl Fn() + Clone + 'static,
) {
    let card = card(cards);
    let on = settings.plugins.is_on(&plugin.id);
    let held = on && !settings.plugins.approved(plugin);
    // What it can do, each with a picture and how much thought it deserves: the commands it
    // adds run as you whenever they're run, so they count too.
    let mut abilities = plugin.manifest.abilities();
    let commands = commands_of(&plugin.id);
    if !commands.is_empty() {
        let one = commands.len() == 1;
        abilities.push(plugins::Ability {
            what: plugins::What::Everything,
            says: format!(
                "Adds {} {}, which {} outside the sandbox whenever you or your tools run {}, \
                 with everything you can reach:",
                if one { "the command" } else { "the commands" },
                plugins::listed(&commands),
                if one { "runs" } else { "run" },
                if one { "it" } else { "them" },
            ),
            care: plugins::Care::Full,
        });
    }

    // Centred, as every control in the row is: a switch left to fill the row's height is drawn
    // stretched.
    let switch = gtk::Switch::builder()
        .active(on && !held)
        .sensitive(on || plugin.too_big.is_none())
        .valign(gtk::Align::Center)
        .build();
    {
        let (store, shown, refill) = (store.clone(), plugin.clone(), refill.clone());
        switch.connect_state_set(move |_, turned_on| {
            // What is approved is what this card showed: its fingerprint, and what it asks for,
            // kept to compare an update with. If the folder has changed since, the compositor
            // holds it.
            if turned_on {
                if shown.too_big.is_some() {
                    return gtk::glib::Propagation::Stop;
                }
                let _ = approval::save(&shown);
            } else {
                approval::forget(&shown.id);
            }
            let fingerprint = turned_on.then(|| shown.fingerprint.clone());
            store.change(|settings| settings.plugins.set_on(&shown.id, fingerprint));
            refill();
            gtk::glib::Propagation::Proceed
        });
    }
    let description = plugin.manifest.description.trim();
    let sub = row(&card, &plugin.manifest.name, Some(description), &switch)
        .expect("the row has a line of explanation");
    // A plugin with nothing to say about itself has only its name.
    sub.set_visible(!description.is_empty());
    if let Some(text) = sub.parent().and_downcast::<gtk::Box>() {
        // The program it runs, for anyone who wants to look it up.
        text.set_tooltip_text(Some(&format!("Runs {}", plugin.manifest.exec.join(" "))));
        // The verdict beside the name: the most thought anything it can do deserves.
        let care = abilities
            .iter()
            .map(|ability| ability.care)
            .max()
            .unwrap_or(plugins::Care::Safe);
        if let Some(title) = text.first_child() {
            let named = gtk::Box::builder().spacing(10).build();
            text.remove(&title);
            named.append(&title);
            let verdict = verdict_label(care);
            verdict.set_valign(gtk::Align::Center);
            named.append(&verdict);
            text.prepend(&named);
        }
    }

    // Why the switch is off, or can't stay on, straight under it.
    if held {
        notice(
            &card,
            "Updated, so it's off until you switch it on again.",
            explained,
        );
    }
    if let Some(why) = &plugin.too_big {
        notice(
            &card,
            &format!("Too big to check: {why}. It can't be switched on until it holds less."),
            &[],
        );
    }
    if !plugin.manifest.unconfined()
        && let Err(why) = slipstream_config::plugins::sandbox::bwrap()
    {
        notice(&card, &format!("{why}."), &[]);
    }

    let can = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .css_classes(["row", "abilities"])
        .build();
    can.append(&label("What it can do", "row-sub"));
    for ability in &abilities {
        let line = gtk::Box::builder().spacing(8).build();
        let icon = gtk::Image::from_icon_name(match ability.what {
            plugins::What::Nothing => "security-high-symbolic",
            plugins::What::Files => "folder-symbolic",
            plugins::What::Notifications => "mail-unread-symbolic",
            plugins::What::Online => "network-transmit-receive-symbolic",
            plugins::What::Windows => "window-new-symbolic",
            plugins::What::Notify => "preferences-system-notifications-symbolic",
            plugins::What::Changes => "document-edit-symbolic",
            plugins::What::Hardware => "computer-symbolic",
            plugins::What::Media => "multimedia-player-symbolic",
            plugins::What::Sound => "audio-volume-high-symbolic",
            plugins::What::Links => "web-browser-symbolic",
            plugins::What::OpenWindows => "focus-windows-symbolic",
            plugins::What::Clipboard => "edit-paste-symbolic",
            plugins::What::Everything => "dialog-warning-symbolic",
        });
        icon.add_css_class(care_class(ability.care));
        icon.set_valign(gtk::Align::Start);
        line.append(&icon);
        let says = label(&ability.says, "row-sub");
        says.set_hexpand(true);
        says.add_css_class(care_class(ability.care));
        line.append(&says);
        can.append(&line);
        // Outside the sandbox, what that means, plainly, under the line that says so.
        if ability.what == plugins::What::Everything {
            let means = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(3)
                .css_classes(["means"])
                .build();
            for power in plugins::OUTSIDE_THE_SANDBOX {
                let point = gtk::Box::builder().spacing(6).build();
                let bullet = label("•", "row-sub");
                bullet.set_valign(gtk::Align::Start);
                point.append(&bullet);
                let says = label(power, "row-sub");
                says.set_hexpand(true);
                point.append(&says);
                means.append(&point);
            }
            means.append(&label(plugins::NOT_AS_ROOT, "row-sub"));
            can.append(&means);
        }
    }
    card.append(&can);

    // Where its headline goes, where it's installed and removing it, at the foot.
    let foot = gtk::Box::builder().spacing(10).css_classes(["row"]).build();
    if on && !held {
        foot.append(&glance_choice(store, &plugin.id, settings, refill.clone()));
    }
    if !plugin.own {
        let shared = label(
            &format!("Installed for everyone in {}", plugin.dir.display()),
            "row-sub",
        );
        shared.set_valign(gtk::Align::Center);
        foot.append(&shared);
    }
    foot.append(&gtk::Box::builder().hexpand(true).build());
    if plugin.own {
        let remove = remove_button();
        let (store, id, dir) = (store.clone(), plugin.id.clone(), plugin.dir.clone());
        remove.connect_clicked(move |button| {
            if confirmed(button) {
                store.change(|settings| settings.plugins.set_on(&id, None));
                approval::forget(&id);
                let removed = std::fs::remove_dir_all(&dir).and_then(|()| store::remove(&id));
                if let Err(err) = removed {
                    button.set_tooltip_text(Some(&format!("Couldn't remove it: {err}")));
                }
                refill();
            }
        });
        foot.append(&remove);
    }
    card.append(&foot);
}

/// A line in the plugin's card, in the colour of what wants attention, saying why its switch is
/// off; `details` go under it in smaller print.
fn notice(card: &gtk::Box, says: &str, details: &[String]) {
    let notice = gtk::Box::builder()
        .spacing(8)
        .css_classes(["row", "notice"])
        .build();
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("needed");
    icon.set_valign(gtk::Align::Start);
    notice.append(&icon);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .build();
    let said = label(says, "row-sub");
    said.add_css_class("needed");
    text.append(&said);
    if !details.is_empty() {
        text.append(&label(&details.join("\n"), "row-sub"));
    }
    notice.append(&text);
    card.append(&notice);
}

/// The verdict a plugin comes to, as a pill on one line.
fn verdict_label(care: plugins::Care) -> gtk::Label {
    let verdict = label(care.verdict(), "verdict");
    verdict.set_wrap(false);
    verdict.set_halign(gtk::Align::Start);
    verdict.add_css_class(care_class(care));
    verdict
}

/// The style for how much thought something deserves: as quiet as the rest, the colour of what
/// wants attention, or the critical orange.
fn care_class(care: plugins::Care) -> &'static str {
    match care {
        plugins::Care::Safe => "safe",
        plugins::Care::Thought => "needed",
        plugins::Care::Full => "hot",
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

/// The commands Slipstream's installer linked into `~/.local/bin` from plugin `id`: they lead to
/// its approved copy, and run as you, outside the sandbox, whenever you or your tools run them.
fn commands_of(id: &str) -> Vec<String> {
    let current = store::of(id).join("current");
    let bin = slipstream_config::xdg_home("HOME", "").join(".local/bin");
    let Ok(entries) = std::fs::read_dir(bin) else {
        return Vec::new();
    };
    let mut commands: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            std::fs::read_link(entry.path()).is_ok_and(|target| target.starts_with(&current))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    commands.sort();
    commands
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

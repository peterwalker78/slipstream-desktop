//! The workspaces' names and order.

use super::*;

/// The workspaces: how many there are, what each is called, and their order.
pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Workspaces",
        "Up to twenty, named or not. Super and a number goes to one of the first nine. Delete \
         one and its windows move to the one before it, or to the first. One without a name \
         shows its number.",
    );
    let card = group(&page, "Workspaces");
    fill_workspaces(&card, store);
    page.upcast()
}

/// The list's rows, built again from the file whenever its shape changes.
fn fill_workspaces(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let list = store.get().workspaces.entries();
    let count = list.len();
    // Structure changes rebuild the list, after the click that caused them has finished.
    let refill = refiller(card, store, fill_workspaces);
    for (index, entry) in list.iter().enumerate() {
        let name = gtk::Entry::builder()
            .text(&entry.name)
            .placeholder_text(format!("Workspace {}", index + 1))
            .width_chars(18)
            .max_length(40)
            .build();
        let id = entry.id;
        let rename_store = store.clone();
        name.connect_changed(move |name| {
            let text = name.text().trim().to_string();
            rename_store.change(|settings| {
                settings.workspaces.list = settings.workspaces.entries();
                if let Some(entry) = settings.workspaces.list.iter_mut().find(|e| e.id == id) {
                    entry.name = text;
                }
            });
        });
        let button = |icon: &str, tip: &str, enabled: bool| {
            let button = gtk::Button::from_icon_name(icon);
            button.set_tooltip_text(Some(tip));
            button.set_sensitive(enabled);
            button.add_css_class("flat");
            button
        };
        let up = button("go-up-symbolic", "Move up", index > 0);
        let down = button("go-down-symbolic", "Move down", index + 1 < count);
        let delete = button("user-trash-symbolic", "Delete this workspace", count > 1);
        for (control, step) in [(&up, -1isize), (&down, 1)] {
            let store = store.clone();
            let refill = refill.clone();
            control.connect_clicked(move |_| {
                store.change(|settings| {
                    let mut list = settings.workspaces.entries();
                    let to = index as isize + step;
                    if to >= 0 && (to as usize) < list.len() {
                        list.swap(index, to as usize);
                    }
                    settings.workspaces.list = list;
                });
                refill();
            });
        }
        {
            let store = store.clone();
            let refill = refill.clone();
            delete.connect_clicked(move |_| {
                store.change(|settings| {
                    let mut list = settings.workspaces.entries();
                    // The last one stays: a desktop needs somewhere to put a window.
                    if list.len() > 1 {
                        list.retain(|entry| entry.id != id);
                    }
                    settings.workspaces.list = list;
                });
                refill();
            });
        }
        let controls = gtk::Box::builder().spacing(4).build();
        controls.append(&name);
        controls.append(&up);
        controls.append(&down);
        controls.append(&delete);
        let title = format!("{}", index + 1);
        row(card, &title, None, &controls);
    }
    let add = gtk::Button::with_label("Add a workspace");
    add.set_sensitive(count < slipstream_config::MAX_WORKSPACES);
    let add_store = store.clone();
    add.connect_clicked(move |_| {
        add_store.change(|settings| {
            let mut list = settings.workspaces.entries();
            if list.len() < slipstream_config::MAX_WORKSPACES {
                let id = slipstream_config::Workspaces { list: list.clone() }.fresh_id();
                list.push(slipstream_config::WorkspaceEntry {
                    id,
                    name: String::new(),
                });
            }
            settings.workspaces.list = list;
        });
        refill();
    });
    let plural = if count == 1 {
        "workspace"
    } else {
        "workspaces"
    };
    row(
        card,
        &format!("{count} {plural}"),
        Some("A new one goes at the end, empty."),
        &add,
    );
}

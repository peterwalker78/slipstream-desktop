//! The system tray: apps' StatusNotifierItems, shown in the notification centre's Background Apps
//! beside the apps whose windows have all closed, and their menus (dbusmenu), which Slipstream
//! draws itself (`tray_menu.rs`).
//!
//! The session serves `org.kde.StatusNotifierWatcher`, and the same under its freedesktop name,
//! unless another watcher already has it; either way it is a host. Everything on the bus happens
//! on the tray's own thread (`host.rs`), where every call to an app has a time limit, since apps
//! hang. The main loop hears what changed through a channel and asks for things with `send`,
//! which never waits.
//!
//! What an app says about itself is only ever drawn. Choosing a menu entry sends that app its own
//! `Event` for the entry and nothing else: no label, icon name or property is run, opened or
//! followed, and pictures and menus are cut to size before they reach the main loop.

mod host;
pub mod item;
pub mod matcher;
pub mod menu;
mod watcher;

use std::sync::OnceLock;

use smithay::reexports::calloop::channel::Sender;

/// The most items kept at once. A desktop has a handful; this only stops a runaway app.
pub const MAX_ITEMS: usize = 64;

/// What the tray's thread tells the main loop.
#[derive(Debug)]
pub enum Event {
    /// An item appeared, or something about it changed.
    Item(Box<Snapshot>),
    /// An item went: its app quit or took it back.
    Gone(String),
    /// An open menu's entries: when it is first read, and again whenever the app changes them.
    Menu { item: String, layout: menu::Layout },
    /// The menu couldn't be read: the app didn't answer, or answered nonsense.
    MenuFailed(String),
    /// The app answered an `Activate`. `refused` when it has none, as some menu-only apps don't;
    /// a left click then opens the menu instead.
    Activated { item: String, refused: bool },
}

/// What the main loop asks of an item. Each names the item by its key, and the tray's thread
/// sends the call to that item's own bus name and objects, whatever the item has said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// A left click: `x` and `y` are where on the screens, which some apps place a window by.
    /// `token` is an activation token for the window it brings forward.
    Activate {
        item: String,
        x: i32,
        y: i32,
        token: Option<String>,
    },
    /// A middle click.
    SecondaryActivate {
        item: String,
        x: i32,
        y: i32,
    },
    /// A right click on an item with no menu of its own to draw: the app shows one.
    ContextMenu {
        item: String,
        x: i32,
        y: i32,
    },
    /// The menu is about to show: read it, and follow its changes until `CloseMenu`.
    OpenMenu {
        item: String,
    },
    /// A submenu, entry `id`, opened or closed.
    OpenSubmenu {
        item: String,
        id: i32,
    },
    CloseSubmenu {
        item: String,
        id: i32,
    },
    /// Entry `id` was chosen.
    Click {
        item: String,
        id: i32,
    },
    CloseMenu {
        item: String,
    },
}

/// One item, as the main loop keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The bus name and object path it registered, written together: how it is told apart.
    pub key: String,
    pub props: item::Props,
    /// The process behind its bus connection, when the bus would say.
    pub process: Option<matcher::Process>,
}

impl Snapshot {
    /// Its ID and title, the names it goes by.
    pub fn names(&self) -> [&str; 2] {
        [self.props.id.as_str(), self.props.title.as_str()]
    }
}

/// Kept with the activation token a chip's click hands an item: the window the token brings
/// forward has to be the item's own app's, told the same way as its chip's (`matcher::owner`).
#[derive(Debug, Clone, Default)]
pub struct TokenFor {
    process: Option<matcher::Process>,
    names: [String; 2],
}

impl TokenFor {
    /// For `item`; `None` for an item no longer there, whose token brings nothing forward.
    pub fn new(item: Option<&Snapshot>) -> Self {
        item.map_or_else(Self::default, |item| Self {
            process: item.process.clone(),
            names: item.names().map(str::to_string),
        })
    }

    /// Whether the token may bring forward a window of `client`'s.
    pub fn reaches(&self, client: matcher::Client) -> bool {
        let names = [self.names[0].as_str(), self.names[1].as_str()];
        matcher::owner(self.process.as_ref(), &names, &[client]) == Some(0)
    }
}

/// Where commands go, once the tray's thread is running.
static COMMANDS: OnceLock<async_channel::Sender<Command>> = OnceLock::new();

/// Starts the tray's thread, which takes the watcher's name if it is free and hosts the items,
/// sending what changes to `events`.
pub fn serve(events: Sender<Event>) {
    let (commands, inbox) = async_channel::unbounded();
    if COMMANDS.set(commands).is_ok() {
        host::start(events, inbox);
    }
}

/// Asks the tray's thread for something. Without a tray (nested, unless asked for) it goes
/// nowhere.
pub fn send(command: Command) {
    if let Some(commands) = COMMANDS.get() {
        let _ = commands.try_send(command);
    }
}

/// The items the main loop knows of, in the order they arrived.
#[derive(Debug, Default)]
pub struct Tray {
    items: Vec<Snapshot>,
    /// Goes up whenever an item arrives, changes or goes.
    revision: u64,
}

impl Tray {
    pub fn items(&self) -> &[Snapshot] {
        &self.items
    }

    pub fn get(&self, key: &str) -> Option<&Snapshot> {
        self.items.iter().find(|item| item.key == key)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// An item arrived or changed. Returns whether anything shown can have changed.
    pub fn update(&mut self, snapshot: Snapshot) -> bool {
        match self.items.iter().position(|item| item.key == snapshot.key) {
            Some(at) if self.items[at] == snapshot => return false,
            Some(at) => self.items[at] = snapshot,
            None if self.items.len() >= MAX_ITEMS => return false,
            None => self.items.push(snapshot),
        }
        self.revision += 1;
        true
    }

    /// Returns whether the item was there.
    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.key != key);
        let gone = self.items.len() != before;
        if gone {
            self.revision += 1;
        }
        gone
    }
}

/// Text from an app, made safe to draw on one line: control characters become spaces, the
/// invisible characters that reorder or hide text are dropped, runs of space close up, and it is
/// cut to `chars` characters.
pub fn clean(text: &str, chars: usize) -> String {
    let mut out = String::new();
    let (mut kept, mut space) = (0, false);
    for ch in text.chars() {
        if is_hidden(ch) {
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            space = kept > 0;
            continue;
        }
        if kept + usize::from(space) >= chars {
            break;
        }
        if std::mem::take(&mut space) {
            out.push(' ');
            kept += 1;
        }
        out.push(ch);
        kept += 1;
    }
    out
}

/// Bidirectional overrides and isolates, zero-width characters and the byte order mark: none
/// belongs in a label, and each can make one read as something else.
fn is_hidden(ch: char) -> bool {
    matches!(ch, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(key: &str, title: &str) -> Snapshot {
        Snapshot {
            key: key.into(),
            props: item::Props {
                title: title.into(),
                ..item::Props::default()
            },
            process: None,
        }
    }

    #[test]
    fn a_chips_activation_token_brings_forward_only_its_own_apps_window() {
        let item = Snapshot {
            process: Some(matcher::Process {
                lineage: vec![500, 10],
                ..matcher::Process::default()
            }),
            ..snapshot(":1.5/StatusNotifierItem", "Chat")
        };
        let token = TokenFor::new(Some(&item));
        let client = |lineage: &'static [u32], app_id: &'static str| matcher::Client {
            lineage,
            app_id,
            flatpak: None,
        };
        assert!(token.reaches(client(&[500, 10], "org.example.Chat")));
        assert!(token.reaches(client(&[520, 500, 10], "org.example.Chat")));
        assert!(!token.reaches(client(&[600, 10], "org.example.Mail")));
        assert!(
            !token.reaches(client(&[600, 10], "chat")),
            "not by name alone"
        );
        assert!(!TokenFor::new(None).reaches(client(&[500, 10], "org.example.Chat")));
    }

    #[test]
    fn labels_lose_control_and_reordering_characters() {
        assert_eq!(clean("  Sync\n\tnow  ", 64), "Sync now");
        assert_eq!(clean("pay\u{202e}lmth.exe", 64), "paylmth.exe");
        assert_eq!(clean("a\u{200b}b\u{feff}", 64), "ab");
        assert_eq!(clean("Nextcloud", 4), "Next");
        assert_eq!(clean("\u{7}\u{1b}[31m", 64), "[31m");
    }

    #[test]
    fn an_unchanged_item_changes_nothing_and_a_gone_one_goes() {
        let mut tray = Tray::default();
        assert!(tray.update(snapshot(":1.4/StatusNotifierItem", "Chat")));
        assert!(!tray.update(snapshot(":1.4/StatusNotifierItem", "Chat")));
        assert!(tray.update(snapshot(":1.4/StatusNotifierItem", "Chat (2)")));
        assert_eq!(tray.items().len(), 1);
        assert!(tray.remove(":1.4/StatusNotifierItem"));
        assert!(!tray.remove(":1.4/StatusNotifierItem"));
        assert!(tray.items().is_empty());
    }

    #[test]
    fn no_more_than_the_most_items_are_kept() {
        let mut tray = Tray::default();
        for n in 0..MAX_ITEMS + 5 {
            tray.update(snapshot(&format!(":1.{n}/StatusNotifierItem"), "Item"));
        }
        assert_eq!(tray.items().len(), MAX_ITEMS);
    }
}

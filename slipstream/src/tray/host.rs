//! The tray's own thread: one connection to the session bus, the watcher when its name is free,
//! and the host, which follows each item and the menu that is open. Calls to apps run side by
//! side on a small executor, each within the connection's time limit, so an app that hangs holds
//! up nothing but its own chip.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_executor::{Executor, Task};
use futures_lite::{StreamExt, future};
use smithay::reexports::calloop::channel::Sender;
use zbus::{
    Connection, MatchRule, Message, MessageStream,
    fdo::{DBusProxy, RequestNameFlags, RequestNameReply},
    message::Type as MessageType,
    names::BusName,
    zvariant::Value,
};

use super::{
    Command, Event, Snapshot, item,
    matcher::Process,
    menu,
    watcher::{self, Address, Change, FreedesktopWatcher, KdeWatcher, Registry, Shared},
};

/// An app answers locally, at once; one that doesn't is given up on.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// Changes that come in a burst (a new icon and a new status, say) are read once, this long
/// after the first.
const SETTLE: Duration = Duration::from_millis(60);
/// The biggest reply read at all, and the biggest read value by value, which costs many times
/// its size in memory.
const MAX_REPLY: usize = 8 << 20;
const LENIENT_REPLY: usize = 256 << 10;
/// A menu asked only for its labels and states has no business being bigger than this.
const MAX_MENU_REPLY: usize = 512 << 10;

const ITEM_KDE: &str = "org.kde.StatusNotifierItem";
const ITEM_FREEDESKTOP: &str = "org.freedesktop.StatusNotifierItem";
const DBUSMENU: &str = "com.canonical.dbusmenu";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// What the host's loop hears from the watcher and the tasks it starts.
#[derive(Debug)]
pub enum Internal {
    /// An item registered, with the watcher here or the one elsewhere.
    Registered(Address),
    /// The watcher elsewhere says an item went.
    Unregistered(String),
    /// An item's properties were read: where it and its menu live.
    Routed { key: String, route: Route },
    /// An item's name had no owner when it was followed.
    Vanished(String),
}

/// Where calls to an item go: always its own connection, whatever name it registered by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The connection's unique name.
    pub owner: String,
    pub path: String,
    /// `org.kde.StatusNotifierItem`, or the freedesktop name for the few apps that use it.
    pub interface: &'static str,
    /// Its dbusmenu object's path.
    pub menu: Option<String>,
}

/// One of dbusmenu's events, as sent to an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEvent {
    pub destination: String,
    pub path: String,
    pub id: i32,
    pub event: &'static str,
}

impl MenuEvent {
    /// `event` on entry `id` of the item's own menu, on the item's own connection: the only
    /// call a menu entry ever makes.
    pub fn new(route: &Route, id: i32, event: &'static str) -> Option<Self> {
        Some(Self {
            destination: route.owner.clone(),
            path: route.menu.clone()?,
            id,
            event,
        })
    }
}

/// What an open menu's task does next, in the order asked.
#[derive(Debug)]
enum MenuOp {
    /// A submenu is about to show: the app may fill it in first.
    AboutToShow(i32),
    Event(i32, &'static str),
}

struct Followed {
    address: Address,
    route: Option<Route>,
    _task: Task<()>,
}

/// The menu being shown: whose, what it is to do next, and the task doing it.
struct OpenMenu {
    item: String,
    ops: async_channel::Sender<MenuOp>,
    task: Task<()>,
}

struct Host {
    conn: Connection,
    executor: Arc<Executor<'static>>,
    events: Sender<Event>,
    inbox: async_channel::Sender<Internal>,
    /// The watcher's registry, when the watcher is ours.
    registry: Option<Arc<Mutex<Registry>>>,
    /// Following the watcher elsewhere, when it isn't.
    elsewhere: Option<Task<()>>,
    items: HashMap<String, Followed>,
    menu: Option<OpenMenu>,
}

/// Starts the tray's thread.
pub fn start(events: Sender<Event>, commands: async_channel::Receiver<Command>) {
    let spawned = std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            let executor = Arc::new(Executor::new());
            let served =
                async_io::block_on(executor.run(serve(executor.clone(), events, commands)));
            if let Err(err) = served {
                tracing::warn!("the tray stopped: {err}");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("couldn't start the tray: {err}");
    }
}

/// What wakes the host's loop.
enum Wake {
    /// A bus name changed hands: who, and whether it now has none.
    Owner(String, bool),
    Command(Command),
    Internal(Internal),
    Stop,
}

async fn serve(
    executor: Arc<Executor<'static>>,
    events: Sender<Event>,
    commands: async_channel::Receiver<Command>,
) -> zbus::Result<()> {
    let conn = zbus::connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .build()
        .await?;
    let (inbox, internal) = async_channel::unbounded();
    let mut owners = DBusProxy::new(&conn)
        .await?
        .receive_name_owner_changed()
        .await?;
    let mut host = Host {
        conn,
        executor,
        events,
        inbox,
        registry: None,
        elsewhere: None,
        items: HashMap::new(),
        menu: None,
    };
    // The spec's host name, which some apps look for before they register.
    let host_name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let _ = host
        .conn
        .request_name_with_flags(host_name.as_str(), RequestNameFlags::DoNotQueue.into())
        .await;
    if !host.take_watcher().await {
        host.join_watcher(&host_name).await;
    }
    loop {
        let owner = async {
            match owners.next().await {
                Some(changed) => match changed.args() {
                    Ok(args) => Wake::Owner(args.name.to_string(), args.new_owner.is_none()),
                    Err(_) => Wake::Owner(String::new(), false),
                },
                None => Wake::Stop,
            }
        };
        let command = async { commands.recv().await.map_or(Wake::Stop, Wake::Command) };
        let internal = async { internal.recv().await.map_or(Wake::Stop, Wake::Internal) };
        match future::or(owner, future::or(command, internal)).await {
            Wake::Owner(name, true) => host.name_lost(&name, &host_name).await,
            Wake::Owner(..) => {}
            Wake::Command(command) => host.command(command),
            Wake::Internal(internal) => host.internal(internal),
            Wake::Stop => return Ok(()),
        }
    }
}

impl Host {
    /// Serves the watcher and takes its names, if nobody has them. Returns whether it did.
    async fn take_watcher(&mut self) -> bool {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let shared = Shared {
            registry: registry.clone(),
            inbox: self.inbox.clone(),
        };
        let server = self.conn.object_server();
        let served = server
            .at(watcher::PATH, KdeWatcher(shared.clone()))
            .await
            .is_ok()
            && server
                .at(watcher::PATH, FreedesktopWatcher(shared))
                .await
                .is_ok();
        let flags = RequestNameFlags::DoNotQueue.into();
        let owned = served
            && matches!(
                self.conn.request_name_with_flags(watcher::KDE, flags).await,
                Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
            );
        if !owned {
            let _ = server.remove::<KdeWatcher, _>(watcher::PATH).await;
            let _ = server.remove::<FreedesktopWatcher, _>(watcher::PATH).await;
            return false;
        }
        // The freedesktop name is a courtesy: another program having it changes nothing.
        let _ = self
            .conn
            .request_name_with_flags(watcher::FREEDESKTOP, flags)
            .await;
        if let Some(own) = self.conn.unique_name() {
            registry.lock().unwrap().add_host(own.as_str());
        }
        self.registry = Some(registry);
        self.elsewhere = None;
        watcher::announce(&self.conn, Change::HostRegistered).await;
        tracing::info!("serving the tray's watcher on the session bus");
        true
    }

    /// Hosts for the watcher another program serves: registers with it, takes the items it
    /// lists, and follows its signals for the rest.
    async fn join_watcher(&mut self, host_name: &str) {
        tracing::info!("another program serves the tray's watcher; hosting its items");
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(watcher::KDE)
            .and_then(|rule| rule.interface(watcher::KDE))
            .and_then(|rule| rule.path(watcher::PATH))
            .map(|rule| rule.build());
        let stream = match rule {
            Ok(rule) => MessageStream::for_match_rule(rule, &self.conn, Some(64)).await,
            Err(err) => Err(err),
        };
        match stream {
            Ok(stream) => {
                let inbox = self.inbox.clone();
                self.elsewhere = Some(self.executor.spawn(follow_watcher(stream, inbox)));
            }
            Err(err) => tracing::warn!("couldn't follow the tray's watcher: {err}"),
        }
        let registered = self
            .conn
            .call_method(
                Some(watcher::KDE),
                watcher::PATH,
                Some(watcher::KDE),
                "RegisterStatusNotifierHost",
                &(host_name,),
            )
            .await;
        if let Err(err) = registered {
            tracing::warn!("the tray's watcher refused this host: {err}");
        }
        let listed = self
            .conn
            .call_method(
                Some(watcher::KDE),
                watcher::PATH,
                Some(PROPERTIES),
                "Get",
                &(watcher::KDE, "RegisteredStatusNotifierItems"),
            )
            .await;
        let items: Vec<String> = listed
            .ok()
            .and_then(|reply| reply.body().deserialize::<Value>().ok()?.try_into().ok())
            .unwrap_or_default();
        for key in items.iter().take(super::MAX_ITEMS) {
            if let Some(address) = Address::parse(key, None) {
                self.follow(address);
            }
        }
    }

    fn internal(&mut self, internal: Internal) {
        match internal {
            Internal::Registered(address) => self.follow(address),
            Internal::Unregistered(key) => self.forget(&key),
            Internal::Routed { key, route } => {
                if let Some(followed) = self.items.get_mut(&key) {
                    followed.route = Some(route);
                }
            }
            Internal::Vanished(key) => {
                if let Some(registry) = &self.registry
                    && let Some(followed) = self.items.get(&key)
                {
                    let (gone, _) = registry.lock().unwrap().name_lost(&followed.address.bus);
                    self.announce_gone(gone);
                }
                self.forget(&key);
            }
        }
    }

    /// Starts following an item, unless it already is or the tray is full.
    fn follow(&mut self, address: Address) {
        let key = address.key();
        if self.items.contains_key(&key) || self.items.len() >= super::MAX_ITEMS {
            return;
        }
        let task = self.executor.spawn(follow_item(
            self.conn.clone(),
            address.clone(),
            self.events.clone(),
            self.inbox.clone(),
        ));
        self.items.insert(
            key,
            Followed {
                address,
                route: None,
                _task: task,
            },
        );
    }

    /// Stops following an item and tells the main loop it went.
    fn forget(&mut self, key: &str) {
        if self.items.remove(key).is_none() {
            return;
        }
        if self.menu.as_ref().is_some_and(|menu| menu.item == key) {
            self.menu = None;
        }
        let _ = self.events.send(Event::Gone(key.to_string()));
    }

    /// `name` left the bus: whatever was registered by it or lives on it goes. The watcher's
    /// own name going free means the program serving it quit, so it is served here instead, and
    /// the apps register again.
    async fn name_lost(&mut self, name: &str, host_name: &str) {
        if self.registry.is_none() && name == watcher::KDE {
            if self.take_watcher().await {
                return;
            }
            self.join_watcher(host_name).await;
        }
        if let Some(registry) = &self.registry {
            let (gone, host_gone) = registry.lock().unwrap().name_lost(name);
            self.announce_gone(gone);
            if host_gone {
                let conn = self.conn.clone();
                self.executor
                    .spawn(async move { watcher::announce(&conn, Change::HostUnregistered).await })
                    .detach();
            }
        }
        let gone: Vec<String> = self
            .items
            .iter()
            .filter(|(_, followed)| {
                followed.address.bus == name
                    || followed
                        .route
                        .as_ref()
                        .is_some_and(|route| route.owner == name)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.forget(&key);
        }
    }

    fn announce_gone(&self, gone: Vec<Address>) {
        for address in gone {
            let conn = self.conn.clone();
            self.executor
                .spawn(async move {
                    watcher::announce(&conn, Change::ItemUnregistered(&address.key())).await;
                })
                .detach();
        }
    }

    fn route(&self, key: &str) -> Option<Route> {
        self.items.get(key)?.route.clone()
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Activate { item, x, y, token } => {
                let Some(route) = self.route(&item) else {
                    return;
                };
                let (conn, events) = (self.conn.clone(), self.events.clone());
                self.spawn(async move {
                    if let Some(token) = token {
                        // KDE's addition: the window the app brings forward may take the
                        // keyboard. An app without it simply refuses.
                        let _ = call(&conn, &route, "ProvideXdgActivationToken", &(token,)).await;
                    }
                    let refused = match call(&conn, &route, "Activate", &(x, y)).await {
                        Ok(_) => false,
                        Err(err) => is_refusal(&err),
                    };
                    let _ = events.send(Event::Activated { item, refused });
                });
            }
            Command::SecondaryActivate { item, x, y } => {
                self.call_item(&item, "SecondaryActivate", x, y)
            }
            Command::ContextMenu { item, x, y } => self.call_item(&item, "ContextMenu", x, y),
            Command::OpenMenu { item } => self.open_menu(item),
            Command::OpenSubmenu { item, id } => {
                self.menu_op(&item, MenuOp::AboutToShow(id));
                self.menu_op(&item, MenuOp::Event(id, "opened"));
            }
            Command::CloseSubmenu { item, id } => self.menu_op(&item, MenuOp::Event(id, "closed")),
            Command::Click { item, id } => self.menu_op(&item, MenuOp::Event(id, "clicked")),
            Command::CloseMenu { item } => {
                if self.menu.as_ref().is_some_and(|menu| menu.item == item) {
                    self.menu_op(&item, MenuOp::Event(0, "closed"));
                    // Its task sends what it was asked, then finishes on its own.
                    if let Some(menu) = self.menu.take() {
                        menu.task.detach();
                    }
                }
            }
        }
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.executor.spawn(task).detach();
    }

    fn call_item(&self, item: &str, method: &'static str, x: i32, y: i32) {
        let Some(route) = self.route(item) else {
            return;
        };
        let conn = self.conn.clone();
        self.spawn(async move {
            if let Err(err) = call(&conn, &route, method, &(x, y)).await {
                tracing::debug!(method, "a tray item didn't take it: {err}");
            }
        });
    }

    fn open_menu(&mut self, item: String) {
        let route = self.route(&item).filter(|route| route.menu.is_some());
        let Some(route) = route else {
            let _ = self.events.send(Event::MenuFailed(item));
            return;
        };
        let (ops, queued) = async_channel::unbounded();
        let task = self.executor.spawn(follow_menu(
            self.conn.clone(),
            route,
            item.clone(),
            self.events.clone(),
            queued,
        ));
        self.menu = Some(OpenMenu { item, ops, task });
    }

    /// Hands `op` to the open menu's task, so its events go in the order asked. An event for a
    /// menu already closed still goes, straight to the item.
    fn menu_op(&self, item: &str, op: MenuOp) {
        if let Some(menu) = self.menu.as_ref().filter(|menu| menu.item == item) {
            let _ = menu.ops.try_send(op);
        } else if let (MenuOp::Event(id, event), Some(route)) = (op, self.route(item)) {
            let conn = self.conn.clone();
            self.spawn(async move { send_event(&conn, MenuEvent::new(&route, id, event)).await });
        }
    }
}

/// Hears the watcher elsewhere announce items coming and going.
async fn follow_watcher(mut stream: MessageStream, inbox: async_channel::Sender<Internal>) {
    while let Some(Ok(message)) = stream.next().await {
        let header = message.header();
        let Some(member) = header.member() else {
            continue;
        };
        let Ok(key) = message.body().deserialize::<String>() else {
            continue;
        };
        let internal = match member.as_str() {
            "StatusNotifierItemRegistered" => match Address::parse(&key, None) {
                Some(address) => Internal::Registered(address),
                None => continue,
            },
            "StatusNotifierItemUnregistered" => Internal::Unregistered(key),
            _ => continue,
        };
        let _ = inbox.try_send(internal);
    }
}

/// Follows one item for as long as it is there: reads its properties, then again whenever it
/// says something changed.
async fn follow_item(
    conn: Connection,
    address: Address,
    events: Sender<Event>,
    inbox: async_channel::Sender<Internal>,
) {
    let key = address.key();
    let Ok(dbus) = DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(bus) = BusName::try_from(address.bus.as_str()) else {
        return;
    };
    let owner = match dbus.get_name_owner(bus.clone()).await {
        Ok(owner) => owner.to_string(),
        Err(_) => {
            let _ = inbox.try_send(Internal::Vanished(key));
            return;
        }
    };
    let process = match dbus.get_connection_unix_process_id(bus).await {
        Ok(pid) => Process::read(Path::new("/proc"), pid),
        Err(_) => None,
    };
    // Subscribed before the first read, so a change in between isn't missed.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(owner.as_str())
        .and_then(|rule| rule.path(address.path.as_str()))
        .map(|rule| rule.build());
    let signals = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, &conn, Some(64))
            .await
            .ok(),
        Err(_) => None,
    };
    let route = Route {
        owner,
        path: address.path.clone(),
        interface: ITEM_KDE,
        menu: None,
    };
    let item = Item {
        key,
        process,
        events,
        inbox,
    };
    keep_up(&conn, item, route, signals).await;
}

/// What an item's task tells the rest of the tray.
struct Item {
    key: String,
    process: Option<Process>,
    events: Sender<Event>,
    inbox: async_channel::Sender<Internal>,
}

/// Reads an item's properties, then again whenever its signals say something changed.
async fn keep_up(conn: &Connection, item: Item, mut route: Route, signals: Option<MessageStream>) {
    let (dirty, changed) = async_channel::bounded(1);
    let follow = async {
        loop {
            if let Some(props) = read_props(conn, &mut route).await {
                route.menu = props.menu.clone();
                let _ = item.inbox.try_send(Internal::Routed {
                    key: item.key.clone(),
                    route: route.clone(),
                });
                let snapshot = Snapshot {
                    key: item.key.clone(),
                    props,
                    process: item.process.clone(),
                };
                let _ = item.events.send(Event::Item(Box::new(snapshot)));
            }
            if !next_change(&changed).await {
                return;
            }
        }
    };
    draining(signals, is_item_change, dirty, follow).await;
}

/// Runs `task` with `signals` read beside it the whole time. Every message is taken off the
/// stream as it arrives, whatever `task` is waiting on, so an app that signals faster than its
/// calls are answered never fills the stream's queue: a full queue would stop the connection
/// reading anything at all, every other app's replies included. A message `wanted` says is a
/// change only marks `dirty`, which holds one mark at most. Once the signals end, `dirty`
/// closes.
async fn draining<T>(
    signals: Option<MessageStream>,
    wanted: fn(&Message) -> bool,
    dirty: async_channel::Sender<()>,
    task: impl Future<Output = T>,
) -> T {
    let drain = async move {
        if let Some(mut signals) = signals {
            while let Some(message) = signals.next().await {
                if message.is_ok_and(|message| wanted(&message)) {
                    let _ = dirty.try_send(());
                }
            }
        }
        drop(dirty);
        future::pending().await
    };
    future::or(task, drain).await
}

/// Waits for a change to be marked, then for the rest of its burst. Returns false once the
/// signals have ended.
async fn next_change(changed: &async_channel::Receiver<()>) -> bool {
    if changed.recv().await.is_err() {
        return false;
    }
    async_io::Timer::after(SETTLE).await;
    let _ = changed.try_recv();
    true
}

/// An item's signals that change what its chip shows. Tooltips and overlays aren't drawn.
fn is_item_change(message: &Message) -> bool {
    let header = message.header();
    let member = header
        .member()
        .map(|member| member.as_str())
        .unwrap_or_default();
    (member.starts_with("New") && !matches!(member, "NewToolTip" | "NewOverlayIcon"))
        || member == "PropertiesChanged"
}

/// A menu's signals that change its entries.
fn is_menu_change(message: &Message) -> bool {
    let header = message.header();
    matches!(
        header.member().map(|member| member.as_str()),
        Some("LayoutUpdated" | "ItemsPropertiesUpdated")
    )
}

/// The item's properties. A few apps serve them under the freedesktop interface name: an item
/// that refuses the KDE one, or has nothing in it, is asked again by that name, which `route`
/// then keeps.
async fn read_props(conn: &Connection, route: &mut Route) -> Option<item::Props> {
    for interface in [route.interface, ITEM_FREEDESKTOP] {
        let reply = conn
            .call_method(
                Some(route.owner.as_str()),
                route.path.as_str(),
                Some(PROPERTIES),
                "GetAll",
                &(interface,),
            )
            .await;
        let props = match reply {
            Ok(reply) => props_from(&reply),
            Err(err) if is_refusal(&err) => None,
            Err(err) => {
                tracing::debug!("a tray item didn't answer: {err}");
                return None;
            }
        };
        if let Some(props) = props.filter(|props| !is_blank(props)) {
            route.interface = interface;
            return Some(props);
        }
        if interface == ITEM_FREEDESKTOP {
            break;
        }
    }
    None
}

fn props_from(reply: &Message) -> Option<item::Props> {
    let body = reply.body();
    if body.len() > MAX_REPLY {
        tracing::warn!(
            bytes = body.len(),
            "a tray item's properties were too big to read"
        );
        return None;
    }
    let wire = item::read(&body, LENIENT_REPLY)?;
    Some(wire.props(
        |dir, name| crate::apps::Icons::in_theme_path(dir, name, item::PICTURE_PX),
        |path| crate::paint::load_image(path, item::PICTURE_PX),
    ))
}

/// Properties that say nothing at all: what an app answers for an interface it doesn't have.
fn is_blank(props: &item::Props) -> bool {
    props.id.is_empty() && props.title.is_empty() && props.icon.is_empty() && props.menu.is_none()
}

/// Whether the app answered with an error, as against not answering at all.
fn is_refusal(err: &zbus::Error) -> bool {
    matches!(err, zbus::Error::MethodError(..) | zbus::Error::FDO(_))
}

/// Calls one of the item's own methods, on its own connection.
async fn call<B>(
    conn: &Connection,
    route: &Route,
    method: &'static str,
    body: &B,
) -> zbus::Result<Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(
        Some(route.owner.as_str()),
        route.path.as_str(),
        Some(route.interface),
        method,
        body,
    )
    .await
}

/// Shows an item's menu for as long as it is open: reads it, tells the app it opened, reads it
/// again whenever the app changes it, and sends the events asked of it, in order. Ends once
/// the menu is closed and everything asked has been sent.
async fn follow_menu(
    conn: Connection,
    route: Route,
    item: String,
    events: Sender<Event>,
    ops: async_channel::Receiver<MenuOp>,
) {
    let Some(path) = route.menu.clone() else {
        return;
    };
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(route.owner.as_str())
        .and_then(|rule| rule.path(path.as_str()))
        .and_then(|rule| rule.interface(DBUSMENU))
        .map(|rule| rule.build());
    let signals = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, &conn, Some(64))
            .await
            .ok(),
        Err(_) => None,
    };
    let (dirty, marked) = async_channel::bounded(1);
    let path = path.as_str();
    let show = async {
        // Whether it needs filling in or not, it is read next.
        let _ = about_to_show(&conn, &route, 0).await;
        match layout(&conn, &route.owner, path).await {
            Some(layout) => {
                let _ = events.send(Event::Menu {
                    item: item.clone(),
                    layout,
                });
            }
            None => {
                let _ = events.send(Event::MenuFailed(item.clone()));
            }
        }
        send_event(&conn, MenuEvent::new(&route, 0, "opened")).await;
        enum Next {
            Op(MenuOp),
            Changed,
            Done,
        }
        loop {
            let op = async { ops.recv().await.map_or(Next::Done, Next::Op) };
            let changed = async {
                // Without its signals the menu still works; it just isn't read again.
                if next_change(&marked).await {
                    Next::Changed
                } else {
                    future::pending().await
                }
            };
            let fresh = match future::or(op, changed).await {
                Next::Op(MenuOp::AboutToShow(id)) => {
                    about_to_show(&conn, &route, id).await == Some(true)
                }
                Next::Op(MenuOp::Event(id, event)) => {
                    send_event(&conn, MenuEvent::new(&route, id, event)).await;
                    false
                }
                Next::Changed => true,
                Next::Done => return,
            };
            if fresh && let Some(layout) = layout(&conn, &route.owner, path).await {
                let _ = events.send(Event::Menu {
                    item: item.clone(),
                    layout,
                });
            }
        }
    };
    draining(signals, is_menu_change, dirty, show).await;
}

/// Tells the app a menu or submenu is about to show. Returns whether it changed its entries.
async fn about_to_show(conn: &Connection, route: &Route, id: i32) -> Option<bool> {
    let path = route.menu.as_deref()?;
    let reply = conn
        .call_method(
            Some(route.owner.as_str()),
            path,
            Some(DBUSMENU),
            "AboutToShow",
            &(id,),
        )
        .await
        .ok()?;
    reply.body().deserialize::<bool>().ok()
}

/// The whole menu, asked for with only the properties Slipstream draws.
async fn layout(conn: &Connection, owner: &str, path: &str) -> Option<menu::Layout> {
    let properties: Vec<&str> = menu::PROPERTIES.to_vec();
    let reply = conn
        .call_method(
            Some(owner),
            path,
            Some(DBUSMENU),
            "GetLayout",
            &(0i32, -1i32, properties),
        )
        .await
        .map_err(|err| tracing::debug!("a tray menu didn't answer: {err}"))
        .ok()?;
    let body = reply.body();
    if body.len() > MAX_MENU_REPLY {
        tracing::warn!(bytes = body.len(), "a tray menu was too big to read");
        return None;
    }
    let (revision, root): (u32, menu::Node) = body.deserialize().ok()?;
    Some(menu::Layout::parse(revision, &root))
}

async fn send_event(conn: &Connection, event: Option<MenuEvent>) {
    let Some(event) = event else {
        return;
    };
    let body = (event.id, event.event, Value::from(0i32), timestamp());
    if let Err(err) = conn
        .call_method(
            Some(event.destination.as_str()),
            event.path.as_str(),
            Some(DBUSMENU),
            "Event",
            &body,
        )
        .await
    {
        tracing::debug!(event = event.event, "a tray menu didn't take it: {err}");
    }
}

/// Milliseconds since the tray started, for dbusmenu's event times.
fn timestamp() -> u32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flooding_item_with_a_slow_reply_doesnt_hold_up_another_items_menu() {
        async_io::block_on(async {
            let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
            let (tray, app) = future::try_zip(
                zbus::connection::Builder::async_io_unix_stream(ours)
                    .p2p()
                    .method_timeout(TIMEOUT)
                    .build(),
                zbus::connection::Builder::async_io_unix_stream(theirs)
                    .server(zbus::Guid::generate())
                    .unwrap()
                    .p2p()
                    .build(),
            )
            .await
            .unwrap();
            // One item never answers for its properties and signals over and over meanwhile;
            // another's menu answers at once.
            let app_side = async {
                let mut calls = MessageStream::from(&app);
                while let Some(Ok(call)) = calls.next().await {
                    let header = call.header();
                    match header.member().map(|member| member.as_str()) {
                        Some("GetAll") => {
                            for _ in 0..500 {
                                let _ = app
                                    .emit_signal(None::<&str>, "/Flood", ITEM_KDE, "NewIcon", &())
                                    .await;
                            }
                        }
                        Some("GetLayout") => {
                            let root: menu::Node = (0, HashMap::new(), Vec::new());
                            let _ = app.reply(&header, &(1u32, root)).await;
                        }
                        _ => {}
                    }
                }
            };
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .path("/Flood")
                .unwrap()
                .build();
            let signals = MessageStream::for_match_rule(rule, &tray, Some(64))
                .await
                .ok();
            let (events, _heard) = smithay::reexports::calloop::channel::channel();
            let (inbox, _routed) = async_channel::unbounded();
            let flooding = Item {
                key: ":1.1/Flood".into(),
                process: None,
                events,
                inbox,
            };
            let route = Route {
                owner: ":1.1".into(),
                path: "/Flood".into(),
                interface: ITEM_KDE,
                menu: None,
            };
            let other = async {
                async_io::Timer::after(Duration::from_millis(200)).await;
                let asked = Instant::now();
                let menu = layout(&tray, ":1.1", "/Other/Menu").await;
                (menu.is_some(), asked.elapsed())
            };
            let rest = async {
                future::zip(app_side, keep_up(&tray, flooding, route, signals)).await;
                (false, Duration::MAX)
            };
            let (answered, took) = future::or(other, rest).await;
            assert!(answered, "the other menu was read");
            assert!(took < Duration::from_millis(500), "it took {took:?}");
        });
    }

    #[test]
    fn a_menu_event_goes_only_to_the_items_own_menu_on_its_own_connection() {
        let route = Route {
            owner: ":1.42".into(),
            path: "/StatusNotifierItem".into(),
            interface: ITEM_KDE,
            menu: Some("/MenuBar".into()),
        };
        assert_eq!(
            MenuEvent::new(&route, 7, "clicked"),
            Some(MenuEvent {
                destination: ":1.42".into(),
                path: "/MenuBar".into(),
                id: 7,
                event: "clicked",
            })
        );
        let no_menu = Route {
            menu: None,
            ..route
        };
        assert_eq!(MenuEvent::new(&no_menu, 7, "clicked"), None);
    }

    #[test]
    fn properties_that_say_nothing_are_blank() {
        assert!(is_blank(&item::Props::default()));
        let titled = item::Props {
            title: "Chat".into(),
            ..item::Props::default()
        };
        assert!(!is_blank(&titled));
    }
}

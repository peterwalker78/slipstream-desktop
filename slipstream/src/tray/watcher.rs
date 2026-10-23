//! The StatusNotifierWatcher: where apps register their tray items and hosts register
//! themselves. Served as `org.kde.StatusNotifierWatcher`, the name apps look for, and as
//! `org.freedesktop.StatusNotifierWatcher` too, both on one registry.

use std::sync::{Arc, Mutex};

use zbus::{
    message::Header,
    names::{BusName, UniqueName},
    object_server::SignalEmitter,
    zvariant::ObjectPath,
};

use super::host::Internal;

pub const KDE: &str = "org.kde.StatusNotifierWatcher";
pub const FREEDESKTOP: &str = "org.freedesktop.StatusNotifierWatcher";
pub const PATH: &str = "/StatusNotifierWatcher";
/// Where an item lives when it registers by bus name alone.
const ITEM_PATH: &str = "/StatusNotifierItem";
/// The most items one connection may register, and hosts in all.
const PER_SENDER: usize = 8;
const MAX_HOSTS: usize = 16;

/// Where an item lives on the bus.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Address {
    pub bus: String,
    pub path: String,
}

impl Address {
    /// From a registration: a bus name, an object path on the sender's own connection, or both
    /// written together, as watchers list them. Anything that isn't a valid name and path is
    /// refused.
    pub fn parse(service: &str, sender: Option<&str>) -> Option<Self> {
        let (bus, path) = match service.find('/') {
            Some(0) => (sender?, service),
            Some(at) => service.split_at(at),
            None => (service, ITEM_PATH),
        };
        BusName::try_from(bus).ok()?;
        ObjectPath::try_from(path).ok()?;
        Some(Self {
            bus: bus.to_string(),
            path: path.to_string(),
        })
    }

    /// The bus name and path together: how items are told apart, and how watchers list them.
    pub fn key(&self) -> String {
        format!("{}{}", self.bus, self.path)
    }
}

/// Who has registered.
#[derive(Debug, Default)]
pub struct Registry {
    /// Each item, with the connection that registered it.
    items: Vec<(Address, String)>,
    hosts: Vec<String>,
}

impl Registry {
    /// Adds an item from `sender`. Returns whether it is new.
    fn add_item(&mut self, address: &Address, sender: &str) -> bool {
        let from_sender = self.items.iter().filter(|(_, by)| by == sender).count();
        if self.items.iter().any(|(known, _)| known == address)
            || from_sender >= PER_SENDER
            || self.items.len() >= super::MAX_ITEMS
        {
            return false;
        }
        self.items.push((address.clone(), sender.to_string()));
        true
    }

    /// Takes out what `name` registered, or what lives on it, now that it has left the bus.
    /// Returns the items' addresses, and whether a host went.
    pub fn name_lost(&mut self, name: &str) -> (Vec<Address>, bool) {
        let mut gone = Vec::new();
        self.items.retain(|(address, sender)| {
            let lost = address.bus == name || sender == name;
            if lost {
                gone.push(address.clone());
            }
            !lost
        });
        let hosts = self.hosts.len();
        self.hosts.retain(|host| host != name);
        (gone, self.hosts.len() != hosts)
    }

    pub fn add_host(&mut self, host: &str) -> bool {
        if self.hosts.iter().any(|known| known == host) || self.hosts.len() >= MAX_HOSTS {
            return false;
        }
        self.hosts.push(host.to_string());
        true
    }

    fn keys(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|(address, _)| address.key())
            .collect()
    }

    fn has_host(&self) -> bool {
        !self.hosts.is_empty()
    }
}

/// What both of the watcher's names share.
#[derive(Clone)]
pub struct Shared {
    pub registry: Arc<Mutex<Registry>>,
    pub inbox: async_channel::Sender<Internal>,
}

impl Shared {
    async fn register_item(
        &self,
        service: &str,
        header: &Header<'_>,
        connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(UniqueName::as_str);
        let address = Address::parse(service, sender)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("not an item: {service}")))?;
        let added = self
            .registry
            .lock()
            .unwrap()
            .add_item(&address, sender.unwrap_or_default());
        if added {
            announce(connection, Change::ItemRegistered(&address.key())).await;
            let _ = self.inbox.try_send(Internal::Registered(address));
        }
        Ok(())
    }

    async fn register_host(
        &self,
        service: &str,
        header: &Header<'_>,
        connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        // A host is known by its connection: that is what leaves the bus when it goes.
        let host = header.sender().map_or(service, UniqueName::as_str);
        let added = self.registry.lock().unwrap().add_host(host);
        if added {
            announce(connection, Change::HostRegistered).await;
        }
        Ok(())
    }

    fn items(&self) -> Vec<String> {
        self.registry.lock().unwrap().keys()
    }

    fn has_host(&self) -> bool {
        self.registry.lock().unwrap().has_host()
    }
}

/// A change every host should hear of, on both names.
pub enum Change<'a> {
    ItemRegistered(&'a str),
    ItemUnregistered(&'a str),
    HostRegistered,
    HostUnregistered,
}

/// Sends `change` as both interfaces' signal.
pub async fn announce(connection: &zbus::Connection, change: Change<'_>) {
    let Ok(emitter) = SignalEmitter::new(connection, PATH) else {
        return;
    };
    let sent = match change {
        Change::ItemRegistered(key) => KdeWatcher::status_notifier_item_registered(&emitter, key)
            .await
            .and(FreedesktopWatcher::status_notifier_item_registered(&emitter, key).await),
        Change::ItemUnregistered(key) => {
            KdeWatcher::status_notifier_item_unregistered(&emitter, key)
                .await
                .and(FreedesktopWatcher::status_notifier_item_unregistered(&emitter, key).await)
        }
        Change::HostRegistered => KdeWatcher::status_notifier_host_registered(&emitter)
            .await
            .and(FreedesktopWatcher::status_notifier_host_registered(&emitter).await),
        Change::HostUnregistered => KdeWatcher::status_notifier_host_unregistered(&emitter)
            .await
            .and(FreedesktopWatcher::status_notifier_host_unregistered(&emitter).await),
    };
    if let Err(err) = sent {
        tracing::warn!("couldn't tell hosts about the tray: {err}");
    }
}

/// `org.kde.StatusNotifierWatcher`.
pub struct KdeWatcher(pub Shared);

/// `org.freedesktop.StatusNotifierWatcher`: the same, under the name a few apps use.
pub struct FreedesktopWatcher(pub Shared);

/// Both interfaces are the same methods, properties and signals under two names.
macro_rules! watcher_interface {
    ($watcher:ident, $name:tt) => {
        #[zbus::interface(name = $name)]
        impl $watcher {
            async fn register_status_notifier_item(
                &self,
                service: &str,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] connection: &zbus::Connection,
            ) -> zbus::fdo::Result<()> {
                self.0.register_item(service, &header, connection).await
            }

            async fn register_status_notifier_host(
                &self,
                service: &str,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] connection: &zbus::Connection,
            ) -> zbus::fdo::Result<()> {
                self.0.register_host(service, &header, connection).await
            }

            #[zbus(property)]
            async fn registered_status_notifier_items(&self) -> Vec<String> {
                self.0.items()
            }

            #[zbus(property)]
            async fn is_status_notifier_host_registered(&self) -> bool {
                self.0.has_host()
            }

            #[zbus(property)]
            async fn protocol_version(&self) -> i32 {
                0
            }

            #[zbus(signal)]
            async fn status_notifier_item_registered(
                emitter: &SignalEmitter<'_>,
                service: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_item_unregistered(
                emitter: &SignalEmitter<'_>,
                service: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_host_registered(
                emitter: &SignalEmitter<'_>,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_host_unregistered(
                emitter: &SignalEmitter<'_>,
            ) -> zbus::Result<()>;
        }
    };
}

watcher_interface!(KdeWatcher, "org.kde.StatusNotifierWatcher");
watcher_interface!(FreedesktopWatcher, "org.freedesktop.StatusNotifierWatcher");

#[cfg(test)]
mod tests {
    use super::*;

    fn address(bus: &str, path: &str) -> Address {
        Address {
            bus: bus.into(),
            path: path.into(),
        }
    }

    #[test]
    fn an_item_registers_by_name_by_path_or_by_both() {
        assert_eq!(
            Address::parse("org.kde.StatusNotifierItem-4242-1", Some(":1.7")),
            Some(address(
                "org.kde.StatusNotifierItem-4242-1",
                "/StatusNotifierItem"
            ))
        );
        assert_eq!(
            Address::parse("/org/ayatana/NotificationItem/chat", Some(":1.7")),
            Some(address(":1.7", "/org/ayatana/NotificationItem/chat"))
        );
        assert_eq!(
            Address::parse(":1.9/StatusNotifierItem", None),
            Some(address(":1.9", "/StatusNotifierItem"))
        );
        assert_eq!(
            address(":1.9", "/StatusNotifierItem").key(),
            ":1.9/StatusNotifierItem"
        );
    }

    #[test]
    fn a_registration_that_names_nothing_is_refused() {
        assert_eq!(Address::parse("", Some(":1.7")), None);
        assert_eq!(Address::parse("/path/only", None), None);
        assert_eq!(Address::parse("not a name/x", None), None);
        assert_eq!(Address::parse(":1.7/bad//path", None), None);
    }

    #[test]
    fn items_go_when_their_name_or_the_connection_that_registered_them_leaves() {
        let mut registry = Registry::default();
        let named = address("org.kde.StatusNotifierItem-10-1", "/StatusNotifierItem");
        let unique = address(":1.20", "/StatusNotifierItem");
        assert!(registry.add_item(&named, ":1.10"));
        assert!(!registry.add_item(&named, ":1.10"), "only once");
        assert!(registry.add_item(&unique, ":1.20"));
        assert!(registry.add_host(":1.30"));
        assert_eq!(registry.name_lost(":1.10"), (vec![named], false));
        assert_eq!(registry.name_lost(":1.20"), (vec![unique], false));
        assert_eq!(registry.name_lost(":1.30"), (Vec::new(), true));
        assert!(registry.keys().is_empty());
    }

    #[test]
    fn one_connection_cant_fill_the_tray() {
        let mut registry = Registry::default();
        let added = (0..20)
            .filter(|n| registry.add_item(&address(":1.5", &format!("/Item{n}")), ":1.5"))
            .count();
        assert_eq!(added, PER_SENDER);
    }
}

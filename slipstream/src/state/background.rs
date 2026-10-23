//! Apps running with no window up, for the notification centre's Background Apps: Wayland
//! clients whose windows have all closed, and apps' tray items, put together so that an app
//! that hid its window and kept an icon in the tray shows once. What a press on one does is here
//! too: a tray item is asked to come forward itself, so a hidden app isn't started a second time.

use std::{path::Path, time::Instant};

use resvg::tiny_skia::Pixmap;
use smithay::{
    desktop::Window,
    reexports::wayland_server::{Resource, backend::ClientId, protocol::wl_surface::WlSurface},
    wayland::xdg_activation::XdgActivationTokenData,
};

use super::*;
use crate::{
    chips::{self, Act, Chip, Press},
    tray::{
        self, Command, Snapshot,
        item::{Picture, Status},
        matcher,
    },
};

/// How long a press waits for an app to answer `Activate` before the centre closes anyway.
const ACTIVATION_WAIT: f64 = 0.8;

/// A client's process and the ones above it, its app ID and its Flatpak, for `matcher::Client`
/// to borrow.
pub(super) type ClientFacts = (Vec<u32>, String, Option<String>);

/// What matching needs to know of a client with process `pid` and `app_id`.
pub(super) fn client_facts(pid: Option<u32>, app_id: String) -> ClientFacts {
    let proc = Path::new("/proc");
    let lineage = pid.map_or_else(Vec::new, |pid| crate::notify::lineage(proc, pid));
    let flatpak = pid.and_then(|pid| matcher::flatpak_of(proc, pid));
    (lineage, app_id, flatpak)
}

/// An app with no visible window: it closed its last window but its Wayland client is still
/// running. Shown in the notification centre so the user knows it's still there.
pub struct BackgroundApp {
    pub name: String,
    pub app_id: String,
    pub client_id: ClientId,
    /// The client's process, when the socket says.
    pub pid: Option<u32>,
}

#[derive(Default)]
pub struct Background {
    /// Clients whose windows have all closed, in the order they went.
    pub apps: Vec<BackgroundApp>,
    /// Each Wayland window mapped, with its client, app ID and process as they were then.
    mapped: Vec<(Window, ClientId, Option<String>, Option<u32>)>,
    pub tray: tray::Tray,
    /// The chips as last put together, and what from.
    chips: Vec<Chip>,
    built_from: Option<Inputs>,
    /// A tray item asked to come forward from its chip, and its answer once it has come: whether
    /// it refused.
    activation: Option<(String, Option<bool>)>,
}

/// What the chips are put together from: when none of it has changed, neither have they.
#[derive(PartialEq)]
struct Inputs {
    tray: u64,
    apps: Vec<ClientId>,
    windows: Vec<Window>,
    px: u32,
    catalog: u64,
}

impl Slipstream {
    /// A Wayland window has mapped: its client and app ID are noted while they can still be
    /// read. Some clients destroy a window's surface before the window itself, which takes both
    /// with it.
    pub(crate) fn note_mapped(&mut self, window: &Window) {
        let Some(surface) = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
        else {
            return;
        };
        let Some(client) = surface.client() else {
            return;
        };
        let pid = client
            .get_credentials(&self.display_handle)
            .ok()
            .and_then(|credentials| u32::try_from(credentials.pid).ok());
        let app_id = toplevel_data(&surface, |data| data.app_id.clone());
        self.background
            .mapped
            .retain(|(known, ..)| known != window && known.alive());
        self.background
            .mapped
            .push((window.clone(), client.id(), app_id, pid));
    }

    /// A window closed. If it was its client's last, the client is now running invisibly in
    /// the background: record it for the notification centre. Only apps from the apps list
    /// count: a portal's file chooser or a password prompt closing leaves a system service
    /// running, not an app. Called after `remove_window`, so the closing window is already gone
    /// from `all_open_windows`.
    pub(crate) fn track_closed_app(&mut self, window: &Window) {
        let Some(at) = self
            .background
            .mapped
            .iter()
            .position(|(known, ..)| known == window)
        else {
            return;
        };
        let (_, client_id, noted, pid) = self.background.mapped.remove(at);
        let live = window
            .toplevel()
            .and_then(|toplevel| toplevel_data(toplevel.wl_surface(), |data| data.app_id.clone()));
        let Some(app_id) = live.or(noted) else {
            return;
        };
        let Some(name) = self.explorer.app_name(&app_id) else {
            return;
        };
        let handle = self.display_handle.backend_handle();
        if handle.get_client_data(client_id.clone()).is_err() {
            return;
        }
        let still_open = self.all_open_windows().into_iter().any(|w| {
            w.toplevel()
                .and_then(|t| t.wl_surface().client())
                .is_some_and(|c| c.id() == client_id)
        });
        if !still_open
            && !self
                .background
                .apps
                .iter()
                .any(|a| a.client_id == client_id)
        {
            self.background.apps.push(BackgroundApp {
                name,
                app_id,
                client_id,
                pid,
            });
        }
    }

    /// A client that was only running in the background has opened a new window; remove it from
    /// the background list.
    pub(crate) fn untrack_background_app(&mut self, wl_surface: &WlSurface) {
        if let Some(client) = wl_surface.client() {
            let id = client.id();
            self.background.apps.retain(|app| app.client_id != id);
        }
    }

    /// Activates a background app: focuses its window if it opened one, otherwise runs its
    /// desktop entry (the process usually intercepts it to show its window again).
    pub fn activate_background_app(&mut self, app_id: &str) {
        let window = self.all_open_windows().into_iter().find(|w| {
            w.alive() && window_app_id(w).is_some_and(|id| id.eq_ignore_ascii_case(app_id))
        });
        if let Some(window) = window {
            if self.rain.contains(&window) {
                self.restore(&window);
            } else {
                self.activate_window(&window);
            }
        } else if let Some(app) = self.explorer.app_for(app_id) {
            self.start_app(&app);
        }
        self.background
            .apps
            .retain(|a| !a.app_id.eq_ignore_ascii_case(app_id));
    }

    /// What the tray's thread says.
    pub fn tray_event(&mut self, event: tray::Event) {
        let now = self.clock.now();
        match event {
            tray::Event::Item(snapshot) => {
                self.background.tray.update(*snapshot);
            }
            tray::Event::Gone(key) => {
                self.background.tray.remove(&key);
                if self.centre.menu_item() == Some(key.as_str()) {
                    self.centre.close_menu(now);
                }
            }
            tray::Event::Menu { item, layout } => self.centre.menu_layout(&item, layout, now),
            tray::Event::MenuFailed(item) => self.centre.menu_failed(&item),
            tray::Event::Activated { item, refused } => {
                if let Some((asked, answer)) = self.background.activation.as_mut()
                    && *asked == item
                {
                    *answer = Some(refused);
                }
            }
        }
    }

    /// The chips for the centre at `scale`, put together again only when something they're made
    /// from has changed.
    pub fn background_chips(&mut self, scale: f64) -> Vec<Chip> {
        let handle = self.display_handle.backend_handle();
        self.background
            .apps
            .retain(|app| handle.get_client_data(app.client_id.clone()).is_ok());
        let inputs = Inputs {
            tray: self.background.tray.revision(),
            apps: self
                .background
                .apps
                .iter()
                .map(|app| app.client_id.clone())
                .collect(),
            windows: self.all_open_windows(),
            px: (crate::chips::ICON * crate::panel::DESIGN_PX * scale as f32)
                .round()
                .max(1.0) as u32,
            catalog: self.explorer.catalog_version(),
        };
        if self.background.built_from.as_ref() != Some(&inputs) {
            self.background.chips = self.build_chips(&inputs.windows, inputs.px);
            self.background.built_from = Some(inputs);
        }
        self.background.chips.clone()
    }

    /// A chip for each windowless client, with the tray item that is the same app's, then one
    /// for each tray item with no client of its own. An item whose app has a window up isn't
    /// running in the background, and a passive one, which says it has nothing to say, is left
    /// out unless its app is already listed.
    fn build_chips(&mut self, windows: &[Window], px: u32) -> Vec<Chip> {
        let mut clients: Vec<ClientFacts> = self
            .background
            .apps
            .iter()
            .map(|app| client_facts(app.pid, app.app_id.clone()))
            .collect();
        let windowless = clients.len();
        for window in windows {
            let pid = self.window_pid(window);
            if pid.is_some()
                && !clients
                    .iter()
                    .any(|(known, ..)| known.first() == pid.as_ref())
            {
                clients.push(client_facts(pid, window_app_id(window).unwrap_or_default()));
            }
        }
        let views: Vec<matcher::Client> = clients
            .iter()
            .map(|(lineage, app_id, flatpak)| matcher::Client {
                lineage,
                app_id,
                flatpak: flatpak.as_deref(),
            })
            .collect();

        let mut chips: Vec<Chip> = self
            .background
            .apps
            .iter()
            .map(|app| Chip {
                name: app.name.clone(),
                icon: None,
                app_id: Some(app.app_id.clone()),
                item: None,
                attention: false,
                item_is_menu: false,
                menu: false,
            })
            .collect();
        let mut merged: Vec<Option<Snapshot>> = vec![None; chips.len()];
        let mut alone = Vec::new();
        for item in self.background.tray.items() {
            match matcher::owner(item.process.as_ref(), &item.names(), &views) {
                Some(at) if at < windowless && merged[at].is_none() => {
                    merged[at] = Some(item.clone())
                }
                Some(at) if at < windowless => alone.push(item.clone()),
                Some(_) => {}
                None if item.props.status == Status::Passive => {}
                None => alone.push(item.clone()),
            }
        }
        for (chip, item) in chips.iter_mut().zip(merged) {
            let app_id = chip.app_id.clone().unwrap_or_default();
            let own = self.explorer.app_icon(&app_id, px);
            chip.icon = match &item {
                Some(item) => self.item_icon(item, own, px),
                None => own,
            };
            if let Some(item) = item {
                attach(chip, &item);
            }
        }
        for item in alone {
            let names = [item.props.id.as_str(), item.props.title.as_str()];
            let app = self.explorer.app_for_item(&names, item.process.as_ref());
            let own = app
                .as_ref()
                .and_then(|app| self.explorer.app_icon(&app.id, px));
            let name = app
                .map(|app| app.name)
                .or_else(|| Some(item.props.title.clone()).filter(|title| !title.is_empty()))
                .or_else(|| Some(item.props.id.clone()).filter(|id| !id.is_empty()))
                .unwrap_or_else(|| "App".into());
            let mut chip = Chip {
                name,
                icon: self.item_icon(&item, own, px),
                app_id: None,
                item: None,
                attention: false,
                item_is_menu: false,
                menu: false,
            };
            attach(&mut chip, &item);
            chips.push(chip);
        }
        chips
    }

    /// The picture for a tray item's chip: what it shows while it needs attention, else its
    /// app's own icon (`own`), the same as its window had, else the item's own.
    fn item_icon(&mut self, item: &Snapshot, own: Option<Pixmap>, px: u32) -> Option<Pixmap> {
        let attention = (item.props.status == Status::NeedsAttention)
            .then(|| self.picture(&item.props.attention, px))
            .flatten();
        attention
            .or(own)
            .or_else(|| self.picture(&item.props.icon, px))
    }

    /// A tray picture at `px`: its name from the icon themes, else its pixels.
    fn picture(&mut self, picture: &Picture, px: u32) -> Option<Pixmap> {
        picture
            .name
            .as_deref()
            .and_then(|name| self.explorer.named_icon(name, px))
            .or_else(|| {
                picture
                    .pixels
                    .as_ref()
                    .and_then(|pixels| crate::paint::fit(pixels, px))
            })
    }

    /// Where chip `index` is, on the screens, for the apps that place what they show by it.
    fn chip_point(&self, index: usize) -> (i32, i32) {
        let screen = self
            .focused_screen_geometry()
            .map(|screen| screen.loc)
            .unwrap_or_default();
        self.centre
            .chip_box(index)
            .map_or((screen.x, screen.y), |area| {
                (
                    screen.x + (area.loc.x + area.size.w / 2.0) as i32,
                    screen.y + (area.loc.y + area.size.h) as i32,
                )
            })
    }

    /// A chip pressed: a left click asks a tray item to come forward (or shows its menu, if
    /// that's all it has) and brings a plain windowless app back; a middle click is a tray item's
    /// second action; a right click shows its menu.
    pub(crate) fn press_chip(&mut self, index: usize, press: Press) {
        let Some(chip) = self.centre.chips().get(index).cloned() else {
            return;
        };
        let now = self.clock.now();
        let (x, y) = self.chip_point(index);
        match chips::act(&chip, press) {
            Act::Menu { item, keyboard } => {
                self.centre.open_menu(item.clone(), index, keyboard, now);
                if self.centre.menu_open() {
                    tray::send(Command::OpenMenu { item });
                }
            }
            Act::AppMenu(item) => {
                self.concentration.asked(Instant::now());
                tray::send(Command::ContextMenu { item, x, y });
                self.centre.close();
            }
            Act::Activate(item) => {
                // A token for the window it brings forward: a press here asked for it, and it
                // brings forward only the item's own app's.
                let data = XdgActivationTokenData::default();
                let token_for = tray::TokenFor::new(self.background.tray.get(&item));
                data.user_data.insert_if_missing(|| token_for);
                let token = self
                    .protocols
                    .xdg_activation_state
                    .create_external_token(data)
                    .0
                    .to_string();
                self.concentration.asked(Instant::now());
                tray::send(Command::Activate {
                    item: item.clone(),
                    x,
                    y,
                    token: Some(token),
                });
                self.background.activation = Some((item, None));
                self.centre.begin_launch(chip.id(), now);
            }
            Act::Launch(_) => self.centre.begin_launch(chip.id(), now),
            Act::Secondary(item) => tray::send(Command::SecondaryActivate { item, x, y }),
            Act::Nothing => {}
        }
    }

    /// Once a chip's launch has played: the centre closes and the app comes forward. A tray
    /// item is given a moment to answer first, and one that has no way to come forward shows
    /// its menu instead.
    pub fn finish_chip_launch(&mut self, now: f64) {
        let Some((chip, since)) = self.centre.launching(now) else {
            return;
        };
        if since < crate::centre::LAUNCH_DUR {
            return;
        }
        let Some(item) = chip.item.clone() else {
            self.centre.close();
            if let Some(app_id) = chip.app_id {
                self.activate_background_app(&app_id);
            }
            return;
        };
        let answer = self
            .background
            .activation
            .as_ref()
            .filter(|(asked, _)| *asked == item)
            .and_then(|(_, answer)| *answer);
        match answer {
            Some(true) => {
                self.background.activation = None;
                self.centre.cancel_launch();
                if let Some(index) = self
                    .centre
                    .chips()
                    .iter()
                    .position(|shown| shown.id() == chip)
                {
                    self.press_chip(index, Press::Menu { keyboard: false });
                }
            }
            Some(false) => {
                self.background.activation = None;
                self.centre.close();
            }
            None if since >= ACTIVATION_WAIT => {
                self.background.activation = None;
                self.centre.close();
            }
            None => {}
        }
    }
}

/// Gives `chip` what its tray item can do.
fn attach(chip: &mut Chip, item: &Snapshot) {
    chip.item = Some(item.key.clone());
    chip.attention = item.props.status == Status::NeedsAttention;
    chip.item_is_menu = item.props.item_is_menu;
    chip.menu = item.props.menu.is_some();
}

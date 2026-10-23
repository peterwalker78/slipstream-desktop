//! X11 apps (Steam, and Flatpaks without Wayland access) through XWayland. Their windows tile
//! like any other; menus and tooltips (override-redirect windows) sit where the app puts them.
//!
//! The windows that place themselves stack as KWin stacks them. Docks and notifications go over
//! the tiles but under a fullscreen window on their screen, and never take the keyboard. Menus,
//! tooltips and override-redirect windows go over a fullscreen window only when they are its own
//! app's; another app's stay under it, so nothing can be drawn over a game or a video by an app
//! that merely says it is a menu.

use std::{os::unix::io::OwnedFd, process::Stdio};

use smithay::{
    desktop::{
        Window,
        space::{RenderZindex, SpaceElement},
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::CompositorHandler,
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmId},
    },
};

use crate::{
    Slipstream,
    cursor::Cursor,
    focus::KeyboardFocus,
    launch,
    layout::Rect,
    screenshot::Selection,
    state::{PlacesItself, places_itself, x11_client_pid},
};

impl Slipstream {
    /// Starts XWayland. X11 apps can connect once it reports ready, a moment later.
    pub fn start_xwayland(&mut self) {
        let spawned = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            std::iter::empty::<String>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        );
        let (xwayland, client) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                tracing::warn!("couldn't start XWayland, so X11 apps won't open: {err}");
                crate::start_autostart(None);
                return;
            }
        };
        let display_handle = self.display_handle.clone();
        let inserted =
            self.loop_handle
                .insert_source(xwayland, move |event, _, state| match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => {
                        // X11 apps draw at 1× and are scaled up with the screen.
                        state.client_compositor_state(&client).set_client_scale(1.0);
                        let mut wm = match X11Wm::start_wm(
                            state.loop_handle.clone(),
                            &display_handle,
                            x11_socket,
                            client.clone(),
                        ) {
                            Ok(wm) => wm,
                            Err(err) => {
                                tracing::warn!("couldn't start the X11 window manager: {err}");
                                crate::start_autostart(None);
                                return;
                            }
                        };
                        if let Some(image) = Cursor::load().x11_image() {
                            let _ = wm.set_cursor(
                                &image.pixels_rgba,
                                (image.width as u16, image.height as u16).into(),
                                (image.xhot as u16, image.yhot as u16).into(),
                            );
                        }
                        state.xwm = Some(wm);
                        let x_display = format!(":{display_number}");
                        tracing::info!(x_display, "XWayland ready");
                        launch::set_x_display(x_display.clone());
                        // DISPLAY isn't in the session's first import: XWayland only picks
                        // its number now. The autostart apps wait for it.
                        crate::start_autostart(Some(x_display));
                    }
                    XWaylandEvent::Error => {
                        tracing::warn!("XWayland exited while starting, so X11 apps won't open");
                        crate::start_autostart(None);
                    }
                });
        if let Err(err) = inserted {
            tracing::warn!("couldn't watch XWayland: {}", err.error);
        }
    }
}

impl XWaylandShellHandler for Slipstream {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.protocols.xwayland_shell_state
    }
}

impl XwmHandler for Slipstream {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("X11 events only arrive while the window manager runs")
    }

    fn new_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_mapped(true) {
            tracing::warn!("couldn't map an X11 window: {err}");
            return;
        }
        // A dock, a desktop widget or a notification keeps the place it picked, above or below
        // the tiles, and doesn't take the keyboard by appearing. A dock along no screen's edge is
        // no panel, and is an ordinary floating window.
        let screens: Vec<Rect> = (0..self.screens.len())
            .filter_map(|index| self.screen_rect(index))
            .collect();
        let kind = own_kind(&surface)
            .filter(|kind| *kind != Own::Dock || touches_edge(surface.geometry(), &screens));
        if kind.is_some() {
            let geometry = surface.geometry();
            let _ = surface.configure(geometry);
            let window = Window::new_x11_window(surface);
            window.user_data().insert_if_missing(|| PlacesItself);
            self.place_x11(window, geometry.loc, false);
            return;
        }
        // Games often ask for fullscreen before their window appears.
        let fullscreen = surface.is_fullscreen();
        let window = Window::new_x11_window(surface);
        self.add_window(window.clone());
        if fullscreen {
            self.set_fullscreen(&window, true);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let location = surface.last_configure().loc;
        self.place_x11(Window::new_x11_window(surface), location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            if places_itself(&window) {
                self.space.unmap_elem(&window);
            } else {
                self.remove_window(&window);
            }
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // A tiled window keeps its tile. Anything else gets the size it asked for, and a window
        // that places itself the place too.
        let tiled = self
            .workspaces
            .all_windows()
            .iter()
            .any(|window| window.x11_surface() == Some(&surface));
        let own_place = self.x11_window(&surface).filter(places_itself);
        let mut geometry = surface.last_configure();
        if !tiled {
            if let Some(width) = width {
                geometry.size.w = width as i32;
            }
            if let Some(height) = height {
                geometry.size.h = height as i32;
            }
        }
        if own_place.is_some() {
            if let Some(x) = x {
                geometry.loc.x = x;
            }
            if let Some(y) = y {
                geometry.loc.y = y;
            }
        }
        let _ = surface.configure(geometry);
        if let Some(window) = own_place {
            self.place_x11(window, geometry.loc, false);
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, _surface: X11Surface, property: WmWindowProperty) {
        // A new opacity changes nothing the window draws, so nothing else would show it.
        if matches!(property, WmWindowProperty::Opacity) {
            self.something_changed();
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Only menus and tooltips place themselves.
        if surface.is_override_redirect() {
            if let Some(window) = self.x11_window(&surface) {
                self.place_x11(window, geometry.loc, false);
            }
        }
    }

    /// An X11 app's own title bar or edge, on a floating window: it moves or resizes with the
    /// pointer, as a Wayland app's does. A tiled window stays in its tile.
    fn resize_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32, edges: ResizeEdge) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as Edge;
        let Some((window, location)) = self.x11_floating_press(&surface) else {
            return;
        };
        let edges = match edges {
            ResizeEdge::Top => Edge::Top,
            ResizeEdge::Bottom => Edge::Bottom,
            ResizeEdge::Left => Edge::Left,
            ResizeEdge::TopLeft => Edge::TopLeft,
            ResizeEdge::BottomLeft => Edge::BottomLeft,
            ResizeEdge::Right => Edge::Right,
            ResizeEdge::TopRight => Edge::TopRight,
            ResizeEdge::BottomRight => Edge::BottomRight,
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        self.start_floating_resize(window, edges, location, button, serial);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32) {
        let Some((window, location)) = self.x11_floating_press(&surface) else {
            return;
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        self.start_floating_move(window, location, button, serial);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.set_fullscreen(&window, true);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.set_fullscreen(&window, false);
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the focused X11 app may read the clipboard.
        matches!(
            self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus()),
            Some(KeyboardFocus::X11(surface)) if surface.xwm_id() == Some(xwm)
        )
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        // A screenshot the compositor put on the clipboard is written out here, not asked of a
        // client.
        let ours = match selection {
            SelectionTarget::Clipboard => current_data_device_selection_userdata(&self.seat)
                .and_then(|selection| match &*selection {
                    Selection::X11 => None,
                    other => Some(other.clone()),
                }),
            SelectionTarget::Primary => None,
        };
        match ours {
            Some(Selection::Image(png)) => {
                if mime_type == crate::screenshot::PNG {
                    crate::screenshot::send_image(png, fd);
                }
                return;
            }
            Some(Selection::Text(text)) => {
                if crate::history::TEXT_TYPES.contains(&mime_type.as_str()) {
                    crate::screenshot::send_bytes(move || text.as_bytes().to_vec(), fd);
                }
                return;
            }
            _ => {}
        }
        let failed = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|err| format!("{err:?}"))
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|err| format!("{err:?}")),
        };
        if let Err(err) = failed {
            tracing::warn!(err, "couldn't pass the clipboard to an X11 app");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types.clone(),
                    Selection::X11,
                );
                self.loop_handle
                    .insert_idle(move |state| state.clipboard_changed(mime_types, true));
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, Selection::X11)
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            // Only a selection an X11 app made: a screenshot on the clipboard stays.
            SelectionTarget::Clipboard => {
                let theirs = current_data_device_selection_userdata(&self.seat)
                    .is_some_and(|selection| matches!(*selection, Selection::X11));
                if theirs {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                let theirs = current_primary_selection_userdata(&self.seat)
                    .is_some_and(|selection| matches!(*selection, Selection::X11));
                if theirs {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::warn!("XWayland's window manager disconnected");
        self.xwm = None;
    }
}

impl Slipstream {
    /// For an X11 app's move or resize: its floating window and where the pointer is, while a
    /// button is held and nothing else has the pointer.
    fn x11_floating_press(
        &self,
        surface: &X11Surface,
    ) -> Option<(
        smithay::desktop::Window,
        smithay::utils::Point<f64, smithay::utils::Logical>,
    )> {
        let window = self.x11_window(surface)?;
        if !self.workspaces.is_floating(&window) || self.lock.is_some() {
            return None;
        }
        let pointer = self.seat.get_pointer()?;
        // Its click grab is the only one expected; a drag of ours is already under way.
        if pointer.is_grabbed() && self.drag.is_some() {
            return None;
        }
        Some((window, pointer.current_location()))
    }
}

impl Slipstream {
    /// Puts an X11 window that places itself at `at`, stacked for what it is and what is
    /// fullscreen on its screen (`layer_for`).
    fn place_x11(&mut self, window: Window, at: Point<i32, Logical>, activate: bool) {
        if let Some(layer) = self.x11_layer(&window) {
            window.override_z_index(layer as u8);
        }
        self.space.map_element(window, at, activate);
    }

    /// Stacks the X11 windows that place themselves again, now that what is fullscreen may have
    /// changed.
    pub(crate) fn restack_x11(&mut self) {
        let moved: Vec<Window> = self
            .space
            .elements()
            .filter(|window| places_itself(window))
            .filter_map(|window| {
                let layer = self.x11_layer(window)? as u8;
                (window.z_index() != layer).then(|| {
                    window.override_z_index(layer);
                    window.clone()
                })
            })
            .collect();
        // The space puts its windows in order of their layers as one is raised.
        for window in moved {
            self.space.raise_element(&window, false);
        }
    }

    /// Where `window`, an X11 window that places itself, stacks.
    fn x11_layer(&self, window: &Window) -> Option<RenderZindex> {
        let surface = window.x11_surface()?;
        let kind = own_kind(surface)?;
        let screens: Vec<(Rect, Option<Window>)> = (0..self.screens.len())
            .filter_map(|index| {
                let rect = self.screen_rect(index)?;
                Some((rect, self.fullscreen_at(self.screens.get(index)?.workspace)))
            })
            .collect();
        let covered = fullscreen_over(surface.geometry(), screens, |full| {
            self.is_fullscreen_apps(surface, full)
        });
        Some(layer_for(kind, covered))
    }

    /// Whether `surface` is the fullscreen window's app's: the same X client's, or a process it
    /// started, as the X server knows the client's process. An app's own claim isn't asked.
    fn is_fullscreen_apps(&self, surface: &X11Surface, full: &Window) -> bool {
        match (x11_client_pid(surface), self.window_pid(full)) {
            (Some(pid), Some(full_pid)) => crate::concentration::descends_from(pid, full_pid),
            _ => false,
        }
    }
}

/// Whether a fullscreen window covers any screen `geometry` reaches, from each screen's rectangle
/// and the window filling it, if one is: by its app only when every one it reaches is its app's
/// (`its_app`). Any part on a screen counts, so a window can't slip out from under by putting its
/// middle off it.
fn fullscreen_over<W>(
    geometry: Rectangle<i32, Logical>,
    screens: impl IntoIterator<Item = (Rect, Option<W>)>,
    its_app: impl Fn(&W) -> bool,
) -> Covered {
    let (x, y) = (geometry.loc.x, geometry.loc.y);
    let (w, h) = (geometry.size.w.max(1), geometry.size.h.max(1));
    let mut covered = Covered::No;
    for (r, full) in screens {
        let reaches = x < r.x + r.w && r.x < x + w && y < r.y + r.h && r.y < y + h;
        match full {
            Some(full) if reaches && !its_app(&full) => return Covered::ByOther,
            Some(_) if reaches => covered = Covered::ByItsApp,
            _ => {}
        }
    }
    covered
}

/// What an X11 window that places itself is, for where it stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Own {
    /// A desktop widget, or a helper window kept below the others and out of every list.
    Desktop,
    Dock,
    Notification,
    /// A menu, a tooltip, a drag image, or any override-redirect window.
    Popup,
}

/// Whether a fullscreen window covers the screen a window is on, and whose it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covered {
    No,
    ByItsApp,
    ByOther,
}

/// What an X11 window that keeps the place it picks is, from what it says it is (EWMH's window
/// types and states). `None` for an app's own window, which is tiled or floats.
fn own_kind(surface: &X11Surface) -> Option<Own> {
    kind_for(
        surface.window_type(),
        surface.is_below() && surface.is_skip_taskbar() && surface.is_skip_pager(),
        surface.is_override_redirect(),
    )
}

/// `own_kind`'s rule. `kept_out` is a window kept below the others and left out of the taskbar
/// and the pager: a desktop widget, or a helper window that isn't meant to be seen.
fn kind_for(window_type: Option<WmWindowType>, kept_out: bool, unmanaged: bool) -> Option<Own> {
    use WmWindowType::*;
    if unmanaged {
        return Some(Own::Popup);
    }
    match window_type {
        Some(Desktop) => Some(Own::Desktop),
        Some(Dock) => Some(Own::Dock),
        Some(Notification) => Some(Own::Notification),
        // Menus and tooltips are nearly always override-redirect; the few that aren't go here.
        Some(Menu | DropdownMenu | PopupMenu | Tooltip | Combo | Dnd) => Some(Own::Popup),
        _ => kept_out.then_some(Own::Desktop),
    }
}

/// Where a window of `kind` stacks. Over the tiles, except for desktop widgets; a fullscreen
/// window is in the space's own layer above the tiles, so under one means beneath that layer.
fn layer_for(kind: Own, covered: Covered) -> RenderZindex {
    match (kind, covered) {
        (Own::Desktop, _) => RenderZindex::Bottom,
        (Own::Dock | Own::Notification, Covered::No) => RenderZindex::Top,
        (Own::Dock | Own::Notification, _) => RenderZindex::Bottom,
        (Own::Popup, Covered::No | Covered::ByItsApp) => RenderZindex::Top,
        (Own::Popup, Covered::ByOther) => RenderZindex::Bottom,
    }
}

/// Whether an X11 window that places itself never takes the keyboard: a dock or a notification.
/// A click on one still reaches it.
pub(crate) fn never_takes_keyboard(window: &Window) -> bool {
    places_itself(window)
        && window.x11_surface().is_some_and(|surface| {
            matches!(
                kind_for(surface.window_type(), false, surface.is_override_redirect()),
                Some(Own::Dock | Own::Notification)
            )
        })
}

/// Whether `window` lies along an edge of one of the `screens`, as a panel does.
fn touches_edge(window: Rectangle<i32, Logical>, screens: &[Rect]) -> bool {
    let (x, y, w, h) = (window.loc.x, window.loc.y, window.size.w, window.size.h);
    screens.iter().any(|s| {
        let overlaps = x < s.x + s.w && s.x < x + w && y < s.y + s.h && s.y < y + h;
        overlaps && (x == s.x || y == s.y || x + w == s.x + s.w || y + h == s.y + s.h)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fullscreen window's layer: the space's own, which tiles share.
    const FULLSCREEN: RenderZindex = RenderZindex::Shell;

    #[test]
    fn app_windows_are_left_to_the_layout() {
        for window_type in [None, Some(WmWindowType::Normal), Some(WmWindowType::Dialog)] {
            assert_eq!(kind_for(window_type, false, false), None);
        }
    }

    #[test]
    fn a_window_kept_out_of_every_list_goes_under_the_tiles() {
        // What KDE's XWayland video bridge and a conky desktop widget both ask for.
        for kept_out in [
            kind_for(Some(WmWindowType::Normal), true, false),
            kind_for(Some(WmWindowType::Desktop), false, false),
        ] {
            assert_eq!(kept_out, Some(Own::Desktop));
            for covered in [Covered::No, Covered::ByItsApp, Covered::ByOther] {
                assert_eq!(layer_for(Own::Desktop, covered), RenderZindex::Bottom);
            }
        }
    }

    #[test]
    fn docks_and_notifications_go_over_the_tiles_but_under_a_fullscreen_window() {
        for kind in [Own::Dock, Own::Notification] {
            assert_eq!(layer_for(kind, Covered::No), RenderZindex::Top);
            assert!((layer_for(kind, Covered::No) as u8) > FULLSCREEN as u8);
            // Even the fullscreen app's own, as on KWin.
            for covered in [Covered::ByItsApp, Covered::ByOther] {
                assert!((layer_for(kind, covered) as u8) < FULLSCREEN as u8);
            }
        }
    }

    #[test]
    fn menus_and_tooltips_go_over_a_fullscreen_window_only_if_they_are_its_apps() {
        for window_type in [
            WmWindowType::Menu,
            WmWindowType::DropdownMenu,
            WmWindowType::PopupMenu,
            WmWindowType::Tooltip,
            WmWindowType::Combo,
            WmWindowType::Dnd,
        ] {
            let kind = kind_for(Some(window_type), false, false).unwrap();
            assert_eq!(kind, Own::Popup);
            assert_eq!(layer_for(kind, Covered::No), RenderZindex::Top);
            assert!((layer_for(kind, Covered::ByItsApp) as u8) > FULLSCREEN as u8);
            assert!((layer_for(kind, Covered::ByOther) as u8) < FULLSCREEN as u8);
        }
    }

    #[test]
    fn an_override_redirect_window_from_another_app_stays_below_fullscreen() {
        // Whatever type it claims, or none: skipping the dock type gains nothing.
        for window_type in [
            None,
            Some(WmWindowType::Normal),
            Some(WmWindowType::Dock),
            Some(WmWindowType::Notification),
        ] {
            let kind = kind_for(window_type, false, true).unwrap();
            assert!((layer_for(kind, Covered::ByOther) as u8) < FULLSCREEN as u8);
        }
    }

    /// Two screens side by side, the left one filled by the app `"game"` and the right by
    /// nothing, or by `right`.
    fn side_by_side(right: Option<&'static str>) -> [(Rect, Option<&'static str>); 2] {
        [
            (
                Rect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                Some("game"),
            ),
            (
                Rect {
                    x: 1920,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                right,
            ),
        ]
    }

    #[test]
    fn another_app_s_window_reaching_a_fullscreen_screen_is_covered() {
        let at = |x, y, w, h| Rectangle::<i32, Logical>::new((x, y).into(), (w, h).into());
        let its_app = |app: &&str| *app == "game";
        let screens = side_by_side(None);
        assert_eq!(
            fullscreen_over(at(-3000, 0, 4000, 1080), screens, |_| false),
            Covered::ByOther,
            "its middle off every screen"
        );
        assert_eq!(
            fullscreen_over(at(0, 0, 4000, 1080), screens, |_| false),
            Covered::ByOther,
            "its middle on the screen beside"
        );
        assert_eq!(
            fullscreen_over(at(2200, 100, 300, 200), screens, |_| false),
            Covered::No,
            "wholly on the screen beside"
        );
        assert_eq!(
            fullscreen_over(at(1800, 100, 300, 200), screens, its_app),
            Covered::ByItsApp,
            "the fullscreen app's own popup stays on top"
        );
        assert_eq!(
            fullscreen_over(at(1800, 100, 300, 200), side_by_side(Some("chat")), its_app),
            Covered::ByOther,
            "across its app's screen and another app's"
        );
    }

    #[test]
    fn a_dock_is_a_panel_only_along_a_screen_edge() {
        let screens = [
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            Rect {
                x: 1920,
                y: 0,
                w: 1280,
                h: 800,
            },
        ];
        let at = |x, y, w, h| Rectangle::<i32, Logical>::new((x, y).into(), (w, h).into());
        assert!(
            touches_edge(at(0, 1040, 1920, 40), &screens),
            "along the bottom"
        );
        assert!(
            touches_edge(at(1920, 300, 48, 200), &screens),
            "down the side"
        );
        assert!(
            touches_edge(at(3152, 0, 48, 800), &screens),
            "the second screen's right"
        );
        assert!(
            !touches_edge(at(600, 400, 300, 64), &screens),
            "floating mid-screen"
        );
        assert!(
            !touches_edge(at(5000, 0, 48, 800), &screens),
            "on no screen"
        );
    }
}

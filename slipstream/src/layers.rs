//! Other programs' panels, launchers, docks and overlays: `zwlr_layer_shell_v1`.
//!
//! Launchers such as fuzzel and wofi, region pickers such as slurp, colour pickers, on-screen
//! displays and wallpaper setters all draw through layer shell. Each surface sits on one of four
//! layers of its screen: the overlay and top layers in front of windows, the bottom and background
//! layers behind them. A window filling the screen covers everything but the overlay. A surface
//! that reserves an exclusive zone (a dock along an edge) takes that strip out of the tiling
//! area. Layer surfaces fade with the rest of the desktop, and bullet time and the lock hide them.
//!
//! A surface that asks for the keyboard outright takes it while it's up, and gives it back to the
//! workspace's window when it goes. One that takes it on demand gets it when clicked.
//!
//! **Unsandboxed clients only.** A layer surface can cover the whole screen and every window,
//! which is exactly what a fake password prompt needs, so the global is filtered as the clipboard's
//! is: a client sees it only if, when it connected, its process was proven to be outside any
//! sandbox.

use std::time::Duration;

use smithay::{
    backend::renderer::{
        element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
        gles::GlesRenderer,
    },
    desktop::{LayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::{Client, DisplayHandle, protocol::wl_output::WlOutput},
    utils::{Logical, Point, Rectangle, Scale},
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::{
    Slipstream, capture::Peer, focus::KeyboardFocus, layout::Rect, render::OutputElement,
    state::ClientState,
};

/// Whether a connecting client may draw layer surfaces, from `capture::unsandboxed_peer`'s look at
/// its process: only with proof that it is outside any sandbox.
pub fn client_may_draw(peer: Option<&Peer>) -> bool {
    peer.is_some()
}

fn may_draw(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.may_draw_layers)
}

pub fn state(display: &DisplayHandle) -> WlrLayerShellState {
    WlrLayerShellState::new_with_filter::<Slipstream, _>(display, may_draw)
}

/// `area` with the strips layer surfaces reserve along `screen`'s edges taken out. `zone` is what
/// the layer map left, in the screen's own coordinates.
pub fn within_zone(area: Rect, screen: Rect, zone: Rectangle<i32, Logical>) -> Rect {
    let left = area.x.max(screen.x + zone.loc.x);
    let top = area.y.max(screen.y + zone.loc.y);
    let right = (area.x + area.w).min(screen.x + zone.loc.x + zone.size.w);
    let bottom = (area.y + area.h).min(screen.y + zone.loc.y + zone.size.h);
    Rect {
        x: left,
        y: top,
        w: (right - left).max(1),
        h: (bottom - top).max(1),
    }
}

impl WlrLayerShellHandler for Slipstream {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // No screen named: the one the keyboard is on, as for a new window.
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.screens.focused_output())
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            surface.send_close();
            return;
        };
        tracing::info!(namespace, screen = output.name(), "a layer surface opened");
        let layer = LayerSurface::new(surface, namespace);
        if let Err(err) = layer_map_for_output(&output).map_layer(&layer) {
            tracing::warn!("couldn't place a layer surface: {err}");
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_layer_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let layer = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                tracing::info!(namespace = layer.namespace(), "a layer surface closed");
                map.unmap_layer(&layer);
            }
        }
        self.layers_changed();
    }
}

impl Slipstream {
    /// A commit on a layer surface: its screen's layers are placed again, a first commit is
    /// answered with its size, and the tiling and the keyboard follow any change.
    pub fn layer_commit(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let Some(output) = self
            .space
            .outputs()
            .find(|output| {
                layer_map_for_output(output)
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
        else {
            return;
        };
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
        });
        {
            let mut map = layer_map_for_output(&output);
            map.arrange();
            if !initial_configure_sent {
                if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL) {
                    layer.layer_surface().send_configure();
                }
            }
        }
        self.layers_changed();
    }

    /// After a layer surface came, went or changed: retile if a reserved strip moved, and hand
    /// the keyboard to a surface that asks for it outright, or back from one that's gone.
    pub fn layers_changed(&mut self) {
        let zones: Vec<(String, Rectangle<i32, Logical>)> = self
            .space
            .outputs()
            .map(|output| {
                (
                    output.name(),
                    layer_map_for_output(output).non_exclusive_zone(),
                )
            })
            .collect();
        if zones != self.layer_zones {
            self.layer_zones = zones;
            self.retile();
        }
        self.update_layer_focus();
    }

    /// The front-most mapped surface on the overlay or top layer that wants the keyboard outright.
    fn exclusive_layer(&self) -> Option<LayerSurface> {
        self.space.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| {
                map.layers_on(layer)
                    .rev()
                    .find(|surface| {
                        surface.cached_state().keyboard_interactivity
                            == KeyboardInteractivity::Exclusive
                            && is_mapped(surface)
                    })
                    .cloned()
            })
        })
    }

    /// Whether the keyboard is on a layer surface, and which.
    pub fn keyboard_layer(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        let KeyboardFocus::Wayland(surface) = focus else {
            return None;
        };
        self.space
            .outputs()
            .any(|output| {
                layer_map_for_output(output)
                    .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .then_some(surface)
    }

    fn update_layer_focus(&mut self) {
        if self.lock.is_some() {
            return;
        }
        if let Some(layer) = self.exclusive_layer() {
            let surface = layer.wl_surface().clone();
            if self.keyboard_layer().as_ref() != Some(&surface) {
                self.focus_layer(&layer);
            }
            return;
        }
        // The keyboard was on a layer surface that has gone or no longer takes it.
        let focus = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let stranded = match focus {
            Some(KeyboardFocus::Wayland(surface)) => {
                let layer = self.space.outputs().find_map(|output| {
                    layer_map_for_output(output)
                        .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                        .cloned()
                });
                let is_window = self
                    .space
                    .elements()
                    .any(|window| KeyboardFocus::Wayland(surface.clone()).is_window(window));
                match layer {
                    Some(layer) => !layer.can_receive_keyboard_focus() || !is_mapped(&layer),
                    // Neither a window nor a layer surface: a layer surface that has closed, or
                    // a popup, which popup grabs look after.
                    None => !is_window && self.popups.find_popup(&surface).is_none(),
                }
            }
            _ => false,
        };
        if stranded {
            self.restore_focus();
        }
    }

    /// Gives the keyboard to a layer surface.
    pub fn focus_layer(&mut self, layer: &LayerSurface) {
        if let Some(window) = self.focused_window() {
            crate::state::deactivate(&window);
        }
        tracing::info!(
            namespace = layer.namespace(),
            "keyboard focus on a layer surface"
        );
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        let focus = KeyboardFocus::Wayland(layer.wl_surface().clone());
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Some(focus), serial);
    }

    /// A click at `pos` on a layer surface that takes the keyboard on demand gives it the keyboard.
    /// True when the click landed on a layer surface at all, so no window beneath takes focus.
    pub fn layer_clicked(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some((layer, _)) = self.layer_under(pos, &LAYERS) else {
            return false;
        };
        if layer.can_receive_keyboard_focus()
            && self.keyboard_layer().as_ref() != Some(layer.wl_surface())
        {
            self.focus_layer(&layer);
        }
        true
    }

    /// The layer surface drawn at `pos` on the given layers, front to back, and where its surface
    /// starts in global coordinates.
    pub fn layer_under(
        &self,
        pos: Point<f64, Logical>,
        layers: &[Layer],
    ) -> Option<(LayerSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(pos).next()?.clone();
        let origin = self.space.output_geometry(&output)?.loc;
        if !self.layers_shown_on(&output) {
            return None;
        }
        let covered = self.fullscreen_on_output(&output);
        let map = layer_map_for_output(&output);
        let local = pos - origin.to_f64();
        layers
            .iter()
            .filter(|layer| !covered || **layer == Layer::Overlay)
            .find_map(|layer| {
                let surface = map.layer_under(*layer, local)?;
                let at = map.layer_geometry(surface)?.loc + origin;
                Some((surface.clone(), at.to_f64()))
            })
    }

    /// The surface under `pos` on the given layers, as the pointer aims at it.
    pub fn layer_surface_under(
        &self,
        pos: Point<f64, Logical>,
        layers: &[Layer],
    ) -> Option<(KeyboardFocus, Point<f64, Logical>)> {
        let (layer, at) = self.layer_under(pos, layers)?;
        let (surface, offset) = layer.surface_under(pos - at, WindowSurfaceType::ALL)?;
        Some((KeyboardFocus::Wayland(surface), at + offset.to_f64()))
    }

    /// Layer surfaces are hidden while bullet time is out or the desktop has faded to the
    /// wallpaper. The lock draws nothing of anyone's.
    fn layers_shown_on(&self, _output: &Output) -> bool {
        self.lock.is_none() && self.bullet.is_none() && !self.idle.is_faded()
    }

    fn fullscreen_on_output(&self, output: &Output) -> bool {
        self.screens
            .index_of(output)
            .and_then(|index| self.screens.get(index))
            .is_some_and(|screen| self.fullscreen_on(screen.workspace))
    }

    /// What `layers` draw on `output`, front to back, at `alpha`.
    pub fn layer_elements(
        &self,
        renderer: &mut GlesRenderer,
        output: &Output,
        layers: &[Layer],
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<OutputElement> {
        let covered = self.fullscreen_on_output(output);
        let map = layer_map_for_output(output);
        let mut elements = Vec::new();
        for layer in layers {
            if covered && *layer != Layer::Overlay {
                continue;
            }
            for surface in map.layers_on(*layer).rev() {
                let Some(geo) = map.layer_geometry(surface) else {
                    continue;
                };
                let location = geo.loc.to_physical_precise_round(scale);
                let drawn: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    surface.render_elements(renderer, location, scale, alpha);
                elements.extend(drawn.into_iter().map(OutputElement::Surface));
            }
        }
        elements
    }

    /// Frame callbacks to every layer surface on `output`.
    pub fn send_layer_frames(&self, output: &Output, now: Duration) {
        use smithay::desktop::utils::surface_primary_scanout_output;
        let map = layer_map_for_output(output);
        // Paced by the screen each surface is really on, and once a second otherwise, as windows
        // are (`render::send_frames`).
        let throttle = Some(Duration::from_secs(1));
        for layer in map.layers() {
            layer.send_frame(output, now, throttle, surface_primary_scanout_output);
        }
    }

    /// Places a layer surface's menu so it stays on its screen.
    fn unconstrain_layer_popup(&self, popup: &PopupSurface) {
        let Ok(root) = smithay::desktop::find_popup_root_surface(&PopupKind::Xdg(popup.clone()))
        else {
            return;
        };
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            let Some(layer) = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL) else {
                continue;
            };
            let Some(layer_geo) = map.layer_geometry(layer) else {
                return;
            };
            let Some(output_geo) = self.space.output_geometry(output) else {
                return;
            };
            // Relative to the popup's parent, as the positioner reads it.
            let mut target = Rectangle::from_size(output_geo.size);
            target.loc -= layer_geo.loc;
            target.loc -=
                smithay::desktop::get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
            popup.with_pending_state(|state| {
                state.geometry = state.positioner.get_unconstrained_geometry(target);
            });
            return;
        }
    }
}

/// Every layer, front to back.
pub const LAYERS: [Layer; 4] = [Layer::Overlay, Layer::Top, Layer::Bottom, Layer::Background];
/// The layers in front of windows, front to back.
pub const FRONT: [Layer; 2] = [Layer::Overlay, Layer::Top];
/// The layers behind windows, front to back.
pub const BACK: [Layer; 2] = [Layer::Bottom, Layer::Background];

/// Whether a layer surface has something to show yet.
fn is_mapped(layer: &LayerSurface) -> bool {
    smithay::backend::renderer::utils::with_renderer_surface_state(layer.wl_surface(), |state| {
        state.buffer().is_some()
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 1536,
        y: 0,
        w: 1536,
        h: 960,
    };

    #[test]
    fn a_zone_the_size_of_the_screen_leaves_the_area_alone() {
        let area = Rect {
            y: 32,
            h: 928,
            ..SCREEN
        };
        let zone = Rectangle::new((0, 0).into(), (1536, 960).into());
        assert_eq!(within_zone(area, SCREEN, zone), area);
    }

    #[test]
    fn a_dock_along_the_bottom_shortens_the_area() {
        let area = Rect {
            y: 32,
            h: 928,
            ..SCREEN
        };
        let zone = Rectangle::new((0, 0).into(), (1536, 900).into());
        assert_eq!(within_zone(area, SCREEN, zone), Rect { h: 868, ..area });
    }

    #[test]
    fn a_panel_down_the_left_narrows_the_area_on_its_own_screen() {
        let area = Rect {
            y: 32,
            h: 928,
            w: 1400,
            ..SCREEN
        };
        let zone = Rectangle::new((48, 0).into(), (1488, 960).into());
        let inside = within_zone(area, SCREEN, zone);
        assert_eq!(inside.x, 1536 + 48);
        assert_eq!(inside.w, 1400 - 48);
    }

    #[test]
    fn a_bar_along_the_top_no_taller_than_ours_changes_nothing() {
        let area = Rect {
            y: 32,
            h: 928,
            ..SCREEN
        };
        let zone = Rectangle::new((0, 30).into(), (1536, 930).into());
        assert_eq!(within_zone(area, SCREEN, zone), area);
    }
}

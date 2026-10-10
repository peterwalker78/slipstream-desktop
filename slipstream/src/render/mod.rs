//! Builds each frame: windows, the focus ring and, on real hardware, the pointer. Both backends
//! draw through here, so the nested window and the real screen look the same.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, ExportMem, ImportMem, Offscreen, Renderer,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind, RenderElementStates,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                texture::TextureRenderElement,
                utils::RescaleRenderElement,
            },
            gles::{GlesRenderer, GlesTexture, element::TextureShaderElement},
        },
    },
    desktop::{
        Window,
        utils::{OutputPresentationFeedback, surface_presentation_feedback_flags_from_states},
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::with_states,
};

use std::{collections::HashMap, time::Duration};

use resvg::tiny_skia::Pixmap;

use crate::{
    Slipstream,
    bar::{self, Bar},
    bullet::{self, Target},
    cursor::Cursor,
    gravity::Rung,
    layout::Rect,
    motion,
    overview::{self, Overview},
    paint::{self, Painter},
    panel::{self, DESIGN_PX},
    rain::Rain,
    text::{self, Face, Style},
    tilt::{self, Tilt},
};

mod chrome;
mod deck;
mod frames;
mod lock;
mod png_file;

pub use chrome::Chrome;
use chrome::*;
use deck::*;
use lock::*;
pub use png_file::{copy_frame, encode_png, save_png};

/// The desktop's backdrop colour, behind every window.
pub const BACKGROUND: Color32F = Color32F::new(0.043, 0.055, 0.075, 1.0);
/// The focused window's ring, until the settings are read: `borders.selected-tile` chooses it,
/// and the buffers take the chosen colour on the first frame they are drawn.
const FOCUS: Color32F = Color32F::new(0.259, 0.827, 1.0, 1.0);
/// The ring is drawn a fifth more transparent than the colour itself, so it marks the window
/// without ringing it in solid paint.
const RING_ALPHA: f32 = 0.8;
/// How strong the fine light edge round the docked pane's glass is.
const DOCK_RIM: f32 = 0.16;
/// The inside of a bay in the bar's material before its pane has landed in it: dark glass.
const DOCK_BAY: Color32F = Color32F::new(0.02, 0.03, 0.05, 0.55);
/// Focus ring width in logical pixels. It sits in the gap, outside the window.
const RING: i32 = 2;
/// Over distant windows: dims them to 70% brightness.
const DIM: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.3);
/// The fade at the end of a session.
const BLACK: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);
const WHITE: Color32F = Color32F::new(1.0, 1.0, 1.0, 1.0);
/// How long a tier tag shows.
const TAG_SHOWN: f64 = 1.8;
/// A tier tag's fade in and out with reduced motion, which shows it in place.
const TAG_REDUCED_FADE: f64 = 0.08;

// Both backends draw with GLES, which bullet time's 3D shader needs.
smithay::backend::renderer::element::render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    /// A window stretched or shrunk while it animates.
    Rescaled=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    /// A texture shown through one of Slipstream's shaders: bullet time's tilted overview, or the
    /// UI's glass fade.
    Shaded=TextureShaderElement,
    /// The desktop drawn into a texture, coming forward as the lock goes.
    Texture=TextureRenderElement<GlesTexture>,
}

impl std::fmt::Debug for OutputElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(e) => f.debug_tuple("Surface").field(e).finish(),
            Self::Rescaled(e) => f.debug_tuple("Rescaled").field(e).finish(),
            Self::Memory(e) => f.debug_tuple("Memory").field(e).finish(),
            Self::Solid(e) => f.debug_tuple("Solid").field(e).finish(),
            Self::Shaded(e) => f.debug_tuple("Shaded").field(e).finish(),
            Self::Texture(e) => f.debug_tuple("Texture").field(e).finish(),
            Self::_GenericCatcher(e) => f.debug_tuple("_GenericCatcher").field(e).finish(),
        }
    }
}

pub fn output_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    cursor: Option<&mut Cursor>,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    // A dragged gap's tiles follow it once a frame, before anything is drawn from them.
    state.retile_for_drag();
    let Some(output_geo) = state.space.output_geometry(output) else {
        return elements;
    };
    // This screen's own drawing buffers and wallpaper, put back at the end. Taking them out
    // keeps every use below a plain local, and keeps the screens from sharing a buffer sized for
    // one of them.
    let name = output.name();
    let mut chrome = state.chromes.remove(&name).unwrap_or_default();
    let mut saver = state
        .savers
        .remove(&name)
        .unwrap_or_else(|| state.new_saver());
    let screen_index = state.screens.index_of(output);
    let scale = Scale::from(output.current_scale().fractional_scale());
    let now = state.clock.frame();
    // While locked, the lock is all there is: nothing below reads a window.
    if state.lock.is_some() {
        let elements = lock_elements(
            state,
            renderer,
            output,
            output_geo,
            &mut chrome,
            &mut saver,
            cursor,
            scale,
        );
        chrome.unlock = None;
        state.chromes.insert(name.clone(), chrome);
        state.savers.insert(name, saver);
        return elements;
    }
    if state
        .unlocking
        .as_ref()
        .is_some_and(|unlocking| unlocking.done(state.wall()))
    {
        state.unlocking = None;
    }
    if state.unlocking.is_none() {
        chrome.unlock = None;
    }
    // How visible the UI is: it fades after a while with no input, leaving the wallpaper.
    let ui = state.ui_opacity(now);

    // Bullet time's overview: 0 normally, 1 zoomed right out. It runs on wall time, so opening
    // and closing it is never slowed.
    let wall = state.wall();
    let zoomed_out = state.overview.value(wall)[0].max(0.0);
    let zoom = 1.0 - 0.32 * zoomed_out;
    let centre = (
        output_geo.size.w as f64 / 2.0,
        output_geo.size.h as f64 / 2.0,
    );
    let to_overview = move |r: Rectangle<f64, Logical>| {
        Rectangle::<f64, Logical>::new(
            (
                centre.0 + (r.loc.x - centre.0) * zoom,
                centre.1 + (r.loc.y - centre.1) * zoom,
            )
                .into(),
            (r.size.w * zoom, r.size.h * zoom).into(),
        )
    };
    let area_local = state.screen_area(screen_index.unwrap_or(0)).map(|area| {
        Rectangle::<f64, Logical>::new(
            (
                (area.x - output_geo.loc.x) as f64,
                (area.y - output_geo.loc.y) as f64,
            )
                .into(),
            (area.w as f64, area.h as f64).into(),
        )
    });
    // Where the tilted overview's reflection starts, the bottom of the workspace frames, and how
    // far below that it shows.
    let mirror_line = area_local.map(|area| {
        let bottom = centre.1 + (area.loc.y + area.size.h + 10.0 - centre.1) * zoom;
        (bottom as f32, (area.size.h * zoom * 0.35) as f32)
    });
    let viewed_index = state
        .bullet
        .as_ref()
        .map_or(state.active_workspace(), |mode| mode.view);
    let selected = state.bullet.as_ref().and_then(|mode| mode.selected.clone());
    let labels = state
        .bullet
        .as_ref()
        .map(|mode| mode.labels.clone())
        .unwrap_or_default();
    // Bullet time, the panels and the cards are drawn where the keyboard is.
    let first_output = state.screens.focused_output().as_ref() == Some(output);
    chrome.overview.begin();
    chrome.panes.begin();
    chrome.overview.set_accent(state.bullet_rgb);
    if first_output && state.bullet.is_some() {
        state.overview_hits.clear();
        state.frame_hits.clear();
    }

    if let Some(cursor) = cursor.filter(|_| ui > 0.5) {
        let pointer = state.pointer_location();
        if output_geo.to_f64().contains(pointer) {
            let pos = pointer - output_geo.loc.to_f64();
            if let CursorImageStatus::Surface(surface) = &state.cursor_status {
                if !surface.alive() {
                    state.cursor_status = CursorImageStatus::default_named();
                }
            }
            // Slipstream's own cursor over a gap or during a drag, never over an overlay or the
            // lock.
            let status = match state.cursor_override {
                Some(icon) if state.drag.is_some() || !state.pointer_blocked() => {
                    CursorImageStatus::Named(icon)
                }
                _ => state.cursor_status.clone(),
            };
            match &status {
                CursorImageStatus::Hidden => {}
                CursorImageStatus::Surface(surface) => {
                    let hotspot = with_states(surface, |states| {
                        states
                            .data_map
                            .get::<CursorImageSurfaceData>()
                            .map(|data| data.lock().unwrap().hotspot)
                            .unwrap_or_else(|| (0, 0).into())
                    });
                    let location: Point<i32, Physical> =
                        (pos - hotspot.to_f64()).to_physical(scale).to_i32_round();
                    let tree: Vec<OutputElement> = render_elements_from_surface_tree(
                        renderer,
                        surface,
                        location,
                        scale,
                        1.0,
                        Kind::Cursor,
                    );
                    elements.extend(tree);
                }
                CursorImageStatus::Named(icon) => {
                    let (buffer, hotspot) =
                        cursor.image(icon.name(), scale.x, state.start_time.elapsed());
                    let location = (pos - hotspot).to_physical(scale);
                    match MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        location,
                        &buffer,
                        None,
                        None,
                        None,
                        Kind::Cursor,
                    ) {
                        Ok(element) => elements.push(OutputElement::Memory(element)),
                        Err(err) => tracing::warn!("couldn't draw the pointer: {err:?}"),
                    }
                }
            }
        }
    }

    // The workspace this screen is showing, which is not the same as the one the keyboard is on.
    let active = screen_index
        .and_then(|index| state.screens.get(index))
        .map(|screen| screen.workspace)
        .unwrap_or_else(|| state.active_workspace());
    // Workspaces sit side by side, a screen and a gap apart, and the view slides between them.
    let step = (output_geo.size.w + motion::WORKSPACE_GAP) as f64;
    let camera = state.motion.camera(&name, wall);
    let active_dx = (active as f64 - camera) * step;
    let focused = state.focused_window();
    let used: Vec<bool> = (0..state.workspaces.count())
        .map(|i| !state.workspaces.get(i).is_empty())
        .collect();

    // Every screen tiles windows, draws a bar and runs the wallpaper; the code rain stays on
    // the leftmost screen, where the tiling area gives up the width for it.
    let tiling_output = screen_index.is_some();
    let rain_output = screen_index == Some(0);

    // Fading between the UI and the wallpaper, everything from here down to the wallpaper is drawn
    // at full strength into a pane of glass that passes through the wallpaper (`glass.rs`). The
    // pointer, already added, stays in front. Reduced motion keeps a plain fade.
    let glass = tiling_output
        && ui > 0.0
        && ui < 1.0
        && !state.clock.reduced_motion
        && chrome.glass.ready(renderer);
    let ui_alpha = if glass { 1.0 } else { ui };
    let pane_start = elements.len();
    // The bar is nearer the eye than the windows once a pane hangs from it in front of them, so
    // while the two fade it and its pane are drawn into a glass of their own, which trails the
    // windows' on the way out and leads it back.
    let bar_apart = glass && rain_output && zoomed_out <= 0.0 && state.docked_on_show().is_some();
    let mut bar_pane: Vec<OutputElement> = Vec::new();
    // Other programs' launchers, pickers and panels in front of everything of the desktop's, and
    // their docks and wallpapers behind the windows, all inside the pane that fades. Not in
    // bullet time, which is the desktop's own.
    // A snip being chosen is in front of everything but the pointer; fragments of one just taken
    // fly in front of that.
    elements.extend(state.snip_flight_elements(renderer, output));
    let before_snip = elements.len();
    let mut snip = state.snip_elements(renderer, output, &mut chrome.overview, scale.x);
    elements.append(&mut snip);
    let snipping = elements.len() > before_snip;
    let layers_shown = tiling_output && ui > 0.0 && zoomed_out <= 0.0 && !snipping;
    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::FRONT,
            scale,
            ui_alpha,
        ));
    }

    // Bullet time leans the overview back like a floor, in a near perspective, and turns it a
    // little while gliding between workspaces.
    let velocity = if state.clock.reduced_motion {
        0.0
    } else {
        (camera - state.motion.camera(&name, wall - 1.0 / 60.0)) * 60.0
    };
    let tilting = first_output && zoomed_out > 0.0 && chrome.stage.ready(renderer);
    let physical = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_else(|| output_geo.size.to_f64().to_physical(scale).to_i32_round());
    // Tilted, the overview is drawn into a texture denser than the screen and sampled back down,
    // which is what keeps a zoomed-out window's text sharp. Everything inside the tilt is built at
    // that density; the bar, the labels in front and the rest of the screen keep the screen's own.
    let dense = if tilting {
        tilt::supersample(physical)
    } else {
        1.0
    };
    let world_scale = Scale::from((scale.x * dense, scale.y * dense));
    let tilt = if tilting {
        Tilt {
            pitch: (15.0 * zoomed_out).to_radians(),
            yaw: (velocity * 2.0).clamp(-6.0, 6.0).to_radians() * zoomed_out.min(1.0),
            distance: 1.15 * output_geo.size.w as f64,
            centre,
            lift: 27.0 * zoomed_out,
            // A slight fish-eye lens over the tilted overview.
            lens: 0.05 * zoomed_out.min(1.0),
        }
    } else {
        Tilt {
            centre,
            ..Tilt::default()
        }
    };
    if first_output {
        state.overview_tilt = tilt;
    }
    // What the overview tilts, and its upright labels in front.
    let mut world = Vec::new();
    // A window dragged in bullet time: where its ghost goes, in front of the rest of the world.
    let mut front_ghost: Option<Rectangle<f64, Logical>> = None;
    let mut front = Vec::new();
    if first_output {
        // Ready before the explorer first opens.
        state.explorer.warm_icons(scale.x);
    }
    // Alt+Tab's deck of glass panes, drawn with the windows below; the flat card stands in where
    // panes can't be drawn.
    let deck_shown = first_output
        && state.deck.is_some()
        && !state.clock.reduced_motion
        && chrome.panes.ready(renderer);
    if first_output && !deck_shown {
        state.deck = None;
    }
    if first_output {
        elements.extend(card_elements(
            state,
            renderer,
            output_geo.size,
            scale.x,
            now,
            wall,
            deck_shown,
        ));
    }
    // Where the pop-ups, the toast and the display below are drawn, for the pointer.
    let mut over_windows = Vec::new();
    // Notifications pop up at the top right, unless a window fills the screen.
    if first_output && ui > 0.0 && !state.fullscreen_on(active) {
        // Under the docked pane, when it hangs in their corner of this screen.
        state.notices.popup_drop = state
            .docked_on_show()
            .filter(|_| rain_output)
            .and_then(|_| state.docked_rect())
            .map_or(0.0, |pane| crate::dock::frame(pane).h as f64);
        let (popups, expired) =
            state
                .notices
                .popup_elements(renderer, output_geo.size.w, scale.x, now, ui_alpha);
        over_windows.extend(popups.iter().map(|popup| popup.geometry(scale)));
        elements.extend(popups.into_iter().map(OutputElement::Memory));
        crate::notify::closed(expired, crate::notify::Reason::Expired);
    }
    if first_output && ui > 0.0 {
        let toast = state.toast.element(renderer, output_geo.size, scale.x, now);
        over_windows.extend(toast.iter().map(|toast| toast.geometry(scale)));
        elements.extend(toast.map(OutputElement::Memory));
        // The volume and brightness display sits low on the screen, over a fullscreen window as
        // well: a film is exactly when the volume keys are used.
        let ring = state.panel_ring();
        let osd = state
            .osd
            .element(renderer, output_geo.size, scale.x, ring, now);
        over_windows.extend(osd.iter().map(|osd| osd.geometry(scale)));
        elements.extend(osd.map(OutputElement::Memory));
    }
    if first_output {
        state.chrome_areas = over_windows
            .into_iter()
            .map(|area| {
                let mut area = area.to_f64().to_logical(scale);
                area.loc += output_geo.loc.to_f64();
                area
            })
            .collect();
    }
    // Bullet time's amber border round the whole screen, in front of the bar: the bar is part of
    // the slowed world while it's open, not something outside the frame.
    if first_output && zoomed_out > 0.0 && ui > 0.0 {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        let (w, h) = (output_geo.size.w as f64, output_geo.size.h as f64);
        let edge = Rectangle::new((2.0, 2.0).into(), (w - 4.0, h - 4.0).into());
        elements.extend(
            chrome
                .overview
                .outline(
                    edge,
                    0.0,
                    2,
                    overview::border(state.bullet_rgb),
                    scale,
                    fade,
                )
                .into_iter()
                .map(OutputElement::Solid),
        );
    }
    // Bullet time's keys, along the foot of the screen, fading with the overview.
    if first_output && zoomed_out > 0.0 && ui > 0.0 {
        if chrome
            .legend
            .as_ref()
            .is_none_or(|legend| legend.scale != scale.x)
        {
            chrome.legend = paint_legend(scale.x);
        }
        if let Some(legend) = chrome.legend.as_ref() {
            let at = Point::<f64, Logical>::from((
                ((output_geo.size.w - legend.logical.w) / 2) as f64,
                output_geo.size.h as f64 - 28.0 * 0.8 - legend.logical.h as f64,
            ));
            let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
            elements.extend(
                legend
                    .element(renderer, at, fade)
                    .map(OutputElement::Memory),
            );
        }
    }
    // The bar hides when a window on screen fills it. On the screen bullet time is drawn on, it
    // follows the overview: the workspace being looked at is highlighted, the one it started on is
    // ringed, and the title names what's chosen.
    if tiling_output && ui > 0.0 && !state.fullscreen_on(active) {
        let content = bar_content(state, active, screen_index, first_output, &used);
        let docked = content.dock.is_some();
        let bar = chrome
            .bar
            .element(renderer, content, output_geo.size.w, scale.x, ui_alpha);
        let (ring, still, demand) = (
            state.ring_rgb,
            state.clock.reduced_motion,
            state.rain.demand(),
        );
        // The docked window's slot, alive on top of the bar where the bar kept room for it. It
        // opens, lights and flashes in step with its pane's flight (`dock::SlotLook`).
        if docked
            && let Some((x, width)) = chrome.bar.dock_slot()
            && let Some(window) = state.docked_on_show()
        {
            chrome.slot_at = Some((x, width));
            let keyboard = state.focused_window().as_ref() == Some(&window);
            let look = state
                .dock
                .as_ref()
                .map_or(crate::dock::SlotLook::AT_REST, |docked| {
                    crate::dock::SlotLook::docked(
                        docked.flight.as_ref(),
                        docked.lit_from,
                        docked.seated,
                        now,
                    )
                });
            let light = state.rain.light();
            if let Some(slot) = state.dock_slot.as_mut() {
                bar_pane.extend(
                    slot.element(
                        renderer,
                        Point::from((x, crate::slot::TOP as f64)),
                        width.round() as i32,
                        scale.x,
                        now,
                        ui_alpha * look.card,
                        keyboard,
                        ring,
                        still,
                        demand,
                        light,
                        (look.lit, look.flash),
                    )
                    .map(OutputElement::Memory),
                );
            }
        } else if rain_output
            && let Some((x, width)) = chrome.slot_at
            && let Some((_, flight)) = state.dock_leaving.as_ref()
        {
            // The slot of a pane on its way down: its light runs out and it closes behind it.
            let look = crate::dock::SlotLook::left(flight, now);
            let light = state.rain.light();
            if let Some(slot) = state.dock_slot_leaving.as_mut() {
                bar_pane.extend(
                    slot.element(
                        renderer,
                        Point::from((x, crate::slot::TOP as f64)),
                        width.round() as i32,
                        scale.x,
                        now,
                        ui_alpha * look.card,
                        false,
                        ring,
                        still,
                        demand,
                        light,
                        (look.lit, look.flash),
                    )
                    .map(OutputElement::Memory),
                );
            }
        }
        bar_pane.extend(bar.map(OutputElement::Memory));
        // Fading with a window docked, the bar and what hangs from it are a pane of their own.
        if !bar_apart {
            elements.append(&mut bar_pane);
        }
    }
    // The deck goes just behind the bar, in front of everything else of the desktop's.
    let behind_bar = elements.len();
    // Bullet time's hints over the code rain's streams, on the screen the rain is drawn on, and
    // the vignette, behind the bar.
    if (first_output || rain_output) && zoomed_out > 0.0 && ui > 0.0 {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        if rain_output && state.bullet.is_some() {
            let screen = Rect {
                x: 0,
                y: 0,
                w: output_geo.size.w,
                h: output_geo.size.h,
            };
            let streams: Vec<(Window, Rect)> = state
                .rain
                .streams
                .iter()
                .enumerate()
                .map(|(index, stream)| {
                    (
                        stream.window.clone(),
                        Rain::column(index, screen, bar::HEIGHT),
                    )
                })
                .collect();
            for (window, column) in streams {
                let middle = Point::from((
                    column.x as f64 + column.w as f64 / 2.0,
                    column.y as f64 + 200.0,
                ));
                if let Some((_, label)) =
                    labels.iter().find(|(target, _)| *target.window() == window)
                {
                    elements.extend(
                        chrome
                            .overview
                            .hint(renderer, label, None, middle, scale.x, fade)
                            .map(OutputElement::Memory),
                    );
                }
                if matches!(&selected, Some(Target::Stream(chosen)) if *chosen == window) {
                    let around = Rectangle::new(
                        (column.x as f64, column.y as f64 + 4.0).into(),
                        (column.w as f64, column.h as f64 - 8.0).into(),
                    );
                    elements.extend(
                        chrome
                            .overview
                            .outline(
                                around,
                                3.0,
                                3,
                                overview::ring(state.bullet_rgb),
                                scale,
                                fade,
                            )
                            .into_iter()
                            .map(OutputElement::Solid),
                    );
                }
            }
        }
        elements.extend(
            chrome
                .overview
                .vignette(renderer, output_geo.size, scale.x, fade)
                .map(OutputElement::Memory),
        );
    }

    let docked: Option<Window> = state.dock.as_ref().map(|docked| docked.window.clone());
    // A window on its way down from the bar is drawn as its pane until it lands.
    let coming_down: Option<Window> = state
        .dock_leaving
        .as_ref()
        .filter(|(_, flight)| {
            rain_output && zoomed_out <= 0.0 && !flight.done(now) && chrome.panes.ready(renderer)
        })
        .map(|(window, _)| window.clone());

    // The code rain, down the right-hand side.
    if rain_output && ui > 0.0 && !state.rain.is_empty() && !state.fullscreen_on(active) {
        elements.extend(
            state
                .rain
                .elements(
                    renderer,
                    output_geo.size,
                    bar::HEIGHT,
                    scale.x,
                    now,
                    ui_alpha,
                )
                .into_iter()
                .map(OutputElement::Memory),
        );
    }

    // Front to back. The space lists the workspace on screen bottom to top. During a switch,
    // the workspace sliding away is drawn too: its windows are unmapped but still alive.
    // A window's place is on the screen its workspace is laid out for; it's drawn where it sits on
    // that screen, as far from this screen's view as its workspace is. The ones on a workspace
    // another screen is showing land off the side of this one, which is where they belong.
    let offset = |index: usize| {
        let origin = state
            .screen_rect_for_workspace(index)
            .map_or(output_geo.loc.x, |rect| rect.x);
        shift_for_workspace(index, camera, step, output_geo.loc.x, origin)
    };
    // The pane docked to the bar is no workspace's: it is drawn on the screen the streams are on
    // wherever the workspaces slide to, in front of every window. While it flies up from its
    // tile, between sizes or back down, it is one rigid pane, drawn after the rest (`dock.rs`).
    let dock_flying = rain_output
        && zoomed_out <= 0.0
        && state
            .dock
            .as_ref()
            .and_then(|docked| docked.flight)
            .is_some_and(|flight| !flight.done(now))
        && chrome.panes.ready(renderer);
    let mut windows: Vec<(Window, Option<usize>)> = state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            if docked.as_ref() == Some(window) || coming_down.as_ref() == Some(window) {
                return None;
            }
            let index = state.workspaces.find(window).unwrap_or(active);
            // Outside bullet time another screen's windows are that screen's alone: a wider
            // screen's workspace is wider than the gap between this screen's workspaces, so its
            // edge would reach in here.
            let elsewhere = state
                .screens
                .showing(index)
                .is_some_and(|showing| Some(showing) != screen_index);
            (!elsewhere || zoomed_out > 0.0).then(|| (window.clone(), Some(index)))
        })
        .collect();
    for index in
        (0..state.workspaces.count()).filter(|index| state.screens.showing(*index).is_none())
    {
        // Zoomed out, neighbouring workspaces come into view.
        if ((index as f64 - camera) * step).abs() * zoom < step {
            // Front to back: the floating windows, front-most first, then the tiles.
            let ws = state.workspaces.get(index);
            let floating = ws.floating.windows().into_iter().rev();
            windows.extend(
                floating
                    .chain(ws.layout.windows())
                    .map(|window| (window, Some(index))),
            );
        }
    }
    // Windows just minimised, still pouring into their streams.
    windows.extend(
        state
            .rain
            .arriving(now)
            .into_iter()
            .map(|window| (window, None)),
    );
    // Fully faded, only the wallpaper is drawn.
    if ui <= 0.0 {
        windows.clear();
    }
    // How far along its workspace each window is drawn.
    let mut windows: Vec<(Window, f64)> = windows
        .into_iter()
        .map(|(window, index)| {
            let dx = index.map_or(active_dx, offset);
            (window, dx)
        })
        .collect();
    if rain_output
        && ui > 0.0
        && !dock_flying
        && let Some(window) = docked
            .as_ref()
            .filter(|window| state.space.element_geometry(window).is_some())
    {
        windows.insert(0, (window.clone(), 0.0));
    }
    state
        .tags
        .retain(|(window, _, at)| now - at < TAG_SHOWN && window.alive());
    let mut dimmed = 0;
    // Windows drawn here that the space doesn't hold get frame callbacks too (`drawn_off_space`).
    for (window, ..) in &windows {
        if state.space.element_geometry(window).is_none() && !state.drawn_off_space.contains(window)
        {
            state.drawn_off_space.push(window.clone());
        }
    }
    // Two tiles passing through each other, drawn as panes after the rest (`pane::Pass`).
    let passing = state
        .pass
        .as_ref()
        .filter(|pass| !pass.done(now) && zoomed_out <= 0.0 && !state.clock.reduced_motion)
        .filter(|pass| state.workspaces.find(&pass.front) == Some(active))
        .map(|pass| (pass.front.clone(), pass.back.clone()));
    let passing = passing.filter(|_| chrome.panes.ready(renderer));
    let in_deck: Vec<Window> = if deck_shown {
        state
            .deck
            .as_ref()
            .filter(|deck| deck.visible(wall))
            .map(|deck| deck.windows.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // The deck's windows are drawn wherever they live, so they keep drawing too.
    for window in &in_deck {
        if state.space.element_geometry(window).is_none() && !state.drawn_off_space.contains(window)
        {
            state.drawn_off_space.push(window.clone());
        }
    }
    // While the docked pane is out in the deck its bay stays under the bar, for the pane to
    // leave and come home to.
    if rain_output
        && ui > 0.0
        && zoomed_out <= 0.0
        && let Some(pane) = docked
            .as_ref()
            .filter(|window| in_deck.contains(window))
            .and_then(|_| state.docked_rect())
    {
        let at = Point::from((
            (pane.x - output_geo.loc.x) as f64,
            (pane.y - output_geo.loc.y) as f64,
        ));
        world.extend(
            chrome
                .dock_frame(renderer, (pane.w, pane.h), scale.x, at, ui_alpha)
                .map(OutputElement::Memory),
        );
        let inside = Rectangle::<i32, Logical>::new(
            (at.x as i32, at.y as i32).into(),
            (pane.w, pane.h).into(),
        );
        world.push(OutputElement::Solid(
            chrome.overview.solid(inside, DOCK_BAY, scale, ui_alpha),
        ));
    }
    // How much of the front of `world` is the docked pane's, bay and flight included: whatever
    // else is put in front of the windows goes in behind that.
    let mut pane_front = world.len();
    // The docked pane's own elements while the overview is open: upright, just behind the bar.
    let mut pane_layer: Vec<OutputElement> = Vec::new();
    for (window, dx) in windows {
        if in_deck.contains(&window)
            || passing
                .as_ref()
                .is_some_and(|(front, back)| *front == window || *back == window)
        {
            continue;
        }
        let own = window.geometry();
        // A window is kept inside the tiling area of the screen its own workspace is on, which
        // is not this screen's when it belongs to the one next door.
        let home_index = state.workspaces.find(&window);
        let is_docked = docked.as_ref() == Some(&window);
        // The docked pane hangs from the bar, which stays upright in bullet time: it is drawn
        // on the screen itself, at the screen's own density, not in the overview's world.
        let mark = world.len();
        let world_scale = if is_docked { scale } else { world_scale };
        let area_global = if is_docked {
            state.screen_area(0)
        } else {
            home_index
                .and_then(|index| state.area_for_workspace(index))
                .or_else(|| state.output_area())
        }
        .map(|area| [area.x as f64, area.y as f64, area.w as f64, area.h as f64]);
        let frame = match state.motion.frame(&window, now) {
            Some(frame) => frame,
            // X11 menus and tooltips place themselves and don't animate.
            None => match state.space.element_geometry(&window) {
                Some(geo) => motion::Frame::still([
                    geo.loc.x as f64,
                    geo.loc.y as f64,
                    geo.size.w as f64,
                    geo.size.h as f64,
                ]),
                None => continue,
            },
        };
        // At rest a window is drawn at its own size. While its tile moves it's stretched to the
        // tile's animated size, since the client redraws at the new size only a frame or two in.
        let [x, y, w, h] = frame.rect;
        // Set when the window is drawn at rest at other than its own size.
        let mut fitted = false;
        let [x, y, w, h] = if frame.moving || awaiting_size(&window) {
            // A client that hasn't yet answered the size it was last sent is still catching up
            // (a slow app, a gap being dragged, a hidden workspace coming back): it's stretched to
            // its tile, as while moving, rather than drawn at its old size with a band of the tile
            // empty or spilling over a neighbour.
            [x, y, w, h]
        } else {
            let at_rest = [x, y, own.size.w as f64, own.size.h as f64];
            let fullscreen = state.is_fullscreen(&window);
            match area_global {
                // Zoomed out, a window bigger than its tile (a client with a minimum size, or a
                // fullscreen one) shrinks to fit, so it stays inside its workspace's frame.
                Some(area) if zoomed_out > 0.0 => {
                    let fit = fit_within(at_rest, [x, y, w, h], area);
                    let t = zoomed_out.min(1.0);
                    std::array::from_fn(|i| at_rest[i] + (fit[i] - at_rest[i]) * t)
                }
                // A client that won't go as small as its tile — an app with a minimum width wider
                // than the tiling could give it, or one squeezed by the code rain taking width
                // from the right — would otherwise overlap its neighbour or be drawn under the
                // rain. It shrinks into the tile instead, so it and its focus ring stay inside.
                // A fullscreen window is not in the area at all.
                Some(area) if !fullscreen => {
                    let scaled = fit_within(at_rest, [x, y, w, h], area);
                    // Rounding at a fractional scale leaves a window a pixel or two over an edge.
                    // Rescaling the whole window for that is far more visible than the overhang
                    // — it flickers every time the client redraws at a size of its own — so only
                    // a real refusal to shrink counts.
                    if at_rest[2] - scaled[2] > SLACK || at_rest[3] - scaled[3] > SLACK {
                        tracing::debug!(
                            own = ?at_rest,
                            ?area,
                            drawn = ?scaled,
                            "a window too big for its tile is drawn scaled into it"
                        );
                        fitted = true;
                        scaled
                    } else {
                        at_rest
                    }
                }
                _ => at_rest,
            }
        };
        let s = frame.scale;
        let shown = Rectangle::<f64, Logical>::new(
            (
                x + dx - output_geo.loc.x as f64 + w * (1.0 - s) / 2.0,
                y - output_geo.loc.y as f64 + h * (1.0 - s) / 2.0,
            )
                .into(),
            (w * s, h * s).into(),
        );
        let alpha = frame.alpha as f32 * ui_alpha;
        // In bullet time windows are drawn smaller around the screen's centre, each also shrunk a
        // little about its own centre so the gaps between them open up inside their frame.
        let shown = if zoomed_out > 0.0 && !is_docked {
            let k = 0.06 * zoomed_out;
            to_overview(Rectangle::new(
                (
                    shown.loc.x + shown.size.w * k / 2.0,
                    shown.loc.y + shown.size.h * k / 2.0,
                )
                    .into(),
                (shown.size.w * (1.0 - k), shown.size.h * (1.0 - k)).into(),
            ))
        } else {
            shown
        };
        if zoomed_out > 0.0 && first_output && !is_docked {
            if state.bullet.is_some() {
                if let Some((_, label)) =
                    labels.iter().find(|(target, _)| *target.window() == window)
                {
                    let name = state.window_name(&window);
                    // A floating window's letter sits just above it, so a dialog centred over its
                    // parent doesn't cover the parent's.
                    let down = if state.workspaces.is_floating(&window) {
                        -30.0
                    } else {
                        shown.size.h / 2.0
                    };
                    let flat = (shown.loc.x + shown.size.w / 2.0, shown.loc.y + down);
                    let middle = Point::from(tilt.project(flat).unwrap_or(flat));
                    front.extend(
                        chrome
                            .overview
                            .hint(renderer, label, Some(&name), middle, scale.x, alpha)
                            .map(OutputElement::Memory),
                    );
                }
                // The window under the pointer shows its close button, in the flat overview so it
                // tilts with its window.
                let hovered = state
                    .bullet
                    .as_ref()
                    .filter(|mode| mode.pointer.drag.is_none())
                    .filter(|mode| mode.pointer.hover.as_ref() == Some(&window))
                    .map(|mode| mode.pointer.on_close);
                if let Some(hot) = hovered {
                    world.extend(
                        chrome
                            .overview
                            .close_button(
                                renderer,
                                bullet::close_button(shown),
                                hot,
                                world_scale.x,
                                alpha,
                            )
                            .map(OutputElement::Memory),
                    );
                }
                // A window being dragged leaves a ghost of itself, half its size, at the pointer.
                if let Some(drag) = state
                    .bullet
                    .as_ref()
                    .and_then(|mode| mode.pointer.drag.as_ref())
                    .filter(|drag| drag.window == window)
                {
                    let (w, h) = (shown.size.w / 2.0, shown.size.h / 2.0);
                    let ghost = Rectangle::new(
                        (drag.at.x - w / 2.0, drag.at.y - h / 2.0).into(),
                        (w, h).into(),
                    );
                    front_ghost = Some(ghost);
                }
                if matches!(&selected, Some(Target::Window(chosen)) if *chosen == window) {
                    // Heavier than the viewed frame's 3 px, with a faint second line further out
                    // for a glow, so the two never read as one double line.
                    let ring = overview::ring(state.bullet_rgb);
                    world.extend(
                        chrome
                            .overview
                            .outline(shown, 6.0, 6, ring, world_scale, alpha)
                            .into_iter()
                            .chain(chrome.overview.outline(
                                shown,
                                14.0 + 6.0,
                                1,
                                ring,
                                world_scale,
                                alpha * 0.25,
                            ))
                            .map(OutputElement::Solid),
                    );
                }
                state.overview_hits.push((window.clone(), shown));
            }
            // Workspaces not being looked at are dimmed.
            if home_index != Some(viewed_index) {
                let dim = alpha * 0.55 * zoomed_out.min(1.0) as f32;
                let shade =
                    chrome
                        .overview
                        .solid(shown.to_i32_round(), overview::SHADE, world_scale, dim);
                world.push(OutputElement::Solid(shade));
            }
        }
        // Gravity: the rung's tag in front, then the distant shade.
        if let Some((_, rung, at)) = state.tags.iter().find(|(w, ..)| *w == window).cloned() {
            world.extend(
                chrome
                    .tag(
                        renderer,
                        rung,
                        shown,
                        now - at,
                        world_scale,
                        state.clock.reduced_motion,
                    )
                    .map(OutputElement::Memory),
            );
        }
        let rung = home_index.map(|index| state.workspaces.get(index).gravity.rung(&window));
        if rung == Some(Rung::Distant) {
            let shade = chrome.dim(dimmed, shown.to_i32_round(), world_scale, alpha);
            world.push(OutputElement::Solid(shade));
            dimmed += 1;
        }
        // A window dragged with Super held outlines the tile it would swap with.
        if state.swap_target() == Some(&window) {
            let colour = overview::ring(state.ring_rgb);
            world.extend(
                chrome
                    .overview
                    .outline(shown, 0.0, 2, colour, world_scale, alpha * 0.6)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        // In bullet time the docked pane has its letter and, when chosen, the ring, as any
        // window does.
        if is_docked && zoomed_out > 0.0 && state.bullet.is_some() {
            let fade = alpha * zoomed_out.min(1.0) as f32;
            if let Some((_, label)) = labels.iter().find(|(target, _)| *target.window() == window) {
                let name = state.window_name(&window);
                let middle = Point::from((
                    shown.loc.x + shown.size.w / 2.0,
                    shown.loc.y + shown.size.h / 2.0,
                ));
                world.extend(
                    chrome
                        .overview
                        .hint(renderer, label, Some(&name), middle, scale.x, fade)
                        .map(OutputElement::Memory),
                );
            }
            if matches!(&selected, Some(Target::Window(chosen)) if *chosen == window) {
                let ring = overview::ring(state.bullet_rgb);
                world.extend(
                    chrome
                        .overview
                        .outline(shown, 0.0, 3, ring, world_scale, fade)
                        .into_iter()
                        .map(OutputElement::Solid),
                );
            }
        }
        // The docked pane's glass has a fine light edge, whoever has the keyboard, and for a
        // moment after it lands the seam where its frame meets the bar is lit.
        if is_docked {
            let seated = state.dock.as_ref().and_then(|docked| docked.seated);
            let seam = crate::dock::seam(seated, now);
            if seam > 0.0 {
                // White along the bar's edge, and round the glass as it locks in.
                let edge = crate::dock::FRAME;
                let line = Rectangle::<i32, Logical>::new(
                    (
                        shown.loc.x.round() as i32 - edge,
                        shown.loc.y.round() as i32 - edge,
                    )
                        .into(),
                    (shown.size.w.round() as i32 + 2 * edge, 2).into(),
                );
                world.push(OutputElement::Solid(chrome.overview.solid(
                    line,
                    WHITE,
                    world_scale,
                    alpha * seam,
                )));
                world.extend(
                    chrome
                        .overview
                        .outline(shown, 0.0, 2, WHITE, world_scale, alpha * seam * 0.8)
                        .into_iter()
                        .map(OutputElement::Solid),
                );
            }
            world.extend(
                chrome
                    .overview
                    .outline(shown, 0.0, 1, WHITE, world_scale, alpha * DOCK_RIM)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        // In bullet time the selection ring takes over from the focus ring.
        if focused.as_ref() == Some(&window) {
            let alpha = alpha * (1.0 - zoomed_out.min(1.0) as f32);
            let colour = overview::ring(state.ring_rgb);
            world.extend(
                chrome
                    .ring(shown.to_i32_round(), colour, world_scale, alpha)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        let location = (shown.loc - own.loc.to_f64())
            .to_physical(world_scale)
            .to_i32_round();
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            AsRenderElements::<GlesRenderer>::render_elements(
                &window,
                renderer,
                location,
                world_scale,
                alpha,
            );
        // A handle on what it was just drawn from, so it can fade out if it closes.
        if let Some(picture) = crate::ghost::Picture::of(&window, &renderer.context_id()) {
            match state
                .pictures
                .iter_mut()
                .find(|(drawn, _)| *drawn == window)
            {
                Some((_, kept)) => *kept = picture,
                None => state.pictures.push((window.clone(), picture)),
            }
        }
        if frame.waiting && !surfaces.is_empty() {
            state.motion.shown(&window, now);
        }
        // Only a window at rest on the screen it is on takes the pointer where it is drawn.
        if fitted && s == 1.0 && zoomed_out <= 0.0 && dx == 0.0 {
            let global = Rectangle::new(shown.loc + output_geo.loc.to_f64(), shown.size);
            match state.fitted.iter_mut().find(|(w, _)| *w == window) {
                Some((_, drawn)) => *drawn = global,
                None => state.fitted.push((window.clone(), global)),
            }
        } else if !fitted {
            state.fitted.retain(|(w, _)| *w != window);
        }
        if (frame.moving || fitted || s != 1.0 || zoom != 1.0) && own.size.w > 0 && own.size.h > 0 {
            let pivot = shown.loc.to_physical(world_scale).to_i32_round();
            let stretch = Scale::from((
                shown.size.w / own.size.w as f64,
                shown.size.h / own.size.h as f64,
            ));
            world.extend(surfaces.into_iter().map(|surface| {
                OutputElement::Rescaled(RescaleRenderElement::from_element(surface, pivot, stretch))
            }));
        } else {
            world.extend(surfaces.into_iter().map(OutputElement::Surface));
        }
        // And round it, the bar's material carried on down its sides and under its foot, with
        // the shadow the whole piece casts on the desktop.
        if is_docked {
            let size = (shown.size.w.round() as i32, shown.size.h.round() as i32);
            world.extend(
                chrome
                    .dock_frame(renderer, size, scale.x, shown.loc, alpha)
                    .map(OutputElement::Memory),
            );
            // With the overview open, everything of the pane's goes in front of it; fading,
            // it goes with the bar it hangs from.
            if zoomed_out > 0.0 {
                pane_layer.extend(world.drain(mark..));
            } else if bar_apart {
                bar_pane.extend(world.drain(mark..));
            }
            // The docked window is drawn first, so all of `world` so far is its pane.
            pane_front = world.len();
        }
    }

    // Tiles passing through each other, in front of the other windows.
    if let (Some((front_window, back_window)), Some(pass)) = (passing.as_ref(), state.pass.as_ref())
    {
        let screen = (output_geo.size.w as f64, output_geo.size.h as f64);
        let camera = crate::pane::Camera::for_screen(screen.0, screen.1);
        let origin = (output_geo.loc.x as f64, output_geo.loc.y as f64);
        let (front, back, deep) = pass.poses(now, screen.0, origin);
        let (axis, forward) = pass.leading();
        let end = if forward { 1.0 } else { 0.0 };
        let edge_at = match axis {
            crate::pane::Axis::Across => {
                crate::pane::project(&front, &camera, (end, 0.5)).map(|(x, _)| x)
            }
            crate::pane::Axis::Down => {
                crate::pane::project(&front, &camera, (0.5, end)).map(|(_, y)| y)
            }
        };
        let deep = deep as f32;
        let front_look = crate::pane::Look {
            alpha: ui_alpha,
            ring: Some((state.ring_rgb, RING_ALPHA)),
            edge: Some((axis, forward, 0.95 * deep)),
            ..Default::default()
        };
        let back_look = crate::pane::Look {
            alpha: ui_alpha,
            shade: 0.25 * deep,
            band: edge_at.map(|at| (axis, at, 150.0, 0.34 * deep)),
            ..Default::default()
        };
        let pieces: Vec<OutputElement> = [
            chrome.panes.element(
                renderer,
                front_window,
                &front,
                &camera,
                output_geo.size,
                scale.x,
                &front_look,
            ),
            chrome.panes.element(
                renderer,
                back_window,
                &back,
                &camera,
                output_geo.size,
                scale.x,
                &back_look,
            ),
        ]
        .into_iter()
        .flatten()
        .map(OutputElement::Shaded)
        .collect();
        world.splice(pane_front..pane_front, pieces);
    }
    if state.pass.as_ref().is_some_and(|pass| pass.done(now)) {
        state.pass = None;
    }

    // The docked pane in flight, and one just undocked on its way down, in front of every window.
    if rain_output && ui > 0.0 && zoomed_out <= 0.0 && chrome.panes.ready(renderer) {
        let arriving = state
            .dock
            .as_ref()
            .and_then(|docked| Some((docked.window.clone(), docked.flight?)));
        let flights: Vec<(Window, crate::dock::Flight)> = state
            .dock_leaving
            .clone()
            .into_iter()
            .chain(arriving)
            .filter(|(_, flight)| !flight.done(now))
            .collect();
        let origin = (output_geo.loc.x as f64, output_geo.loc.y as f64);
        let local =
            crate::pane::Camera::for_screen(output_geo.size.w as f64, output_geo.size.h as f64);
        // The stops are in the space's coordinates, so the eye is too, and the pane is brought
        // on to this screen afterwards.
        let eye = crate::pane::Camera {
            x: local.x + origin.0,
            y: local.y + origin.1,
            ..local
        };
        let mut pieces = Vec::new();
        for (window, flight) in flights {
            let (mut pose, lit) = flight.at(now, &eye);
            pose.x -= origin.0;
            pose.y -= origin.1;
            // The focus ring if the keyboard is in it, and the glass's own light edge if not.
            let ring = if focused.as_ref() == Some(&window) {
                (state.ring_rgb, RING_ALPHA)
            } else {
                ([1.0; 3], DOCK_RIM)
            };
            let (band_at, band_half, band_light) = lit.band;
            let look = crate::pane::Look {
                alpha: ui_alpha,
                ring: Some(ring),
                shade: 0.0,
                edge: Some(lit.edge),
                reach: lit.reach,
                band: Some((
                    crate::pane::Axis::Down,
                    band_at - origin.1,
                    band_half,
                    band_light,
                )),
            };
            pieces.extend(
                chrome
                    .panes
                    .element(
                        renderer,
                        &window,
                        &pose,
                        &local,
                        output_geo.size,
                        scale.x,
                        &look,
                    )
                    .map(OutputElement::Shaded),
            );
            // Behind the pane, the bay in the bar's material that it is heading for or has
            // left: the frame, and dark glass where the window will be.
            if let Some((bay, shown)) = flight.bay(now).filter(|(_, shown)| *shown > 0.0) {
                let at = Point::from((bay[0] - origin.0, bay[1] - origin.1));
                let size = (bay[2].round() as i32, bay[3].round() as i32);
                let alpha = ui_alpha * shown;
                pieces.extend(
                    chrome
                        .dock_frame(renderer, size, scale.x, at, alpha)
                        .map(OutputElement::Memory),
                );
                let inside = Rectangle::<i32, Logical>::new(
                    (at.x.round() as i32, at.y.round() as i32).into(),
                    size.into(),
                );
                pieces.push(OutputElement::Solid(
                    chrome.overview.solid(inside, DOCK_BAY, scale, alpha),
                ));
            }
        }
        pane_front += pieces.len();
        world.splice(0..0, pieces);
    }
    if let Some(docked) = state.dock.as_mut()
        && docked.flight.is_some_and(|flight| flight.done(now))
    {
        docked.flight = None;
    }
    if state
        .dock_leaving
        .as_ref()
        .is_some_and(|(_, flight)| flight.done(now))
    {
        state.dock_leaving = None;
        state.dock_slot_leaving = None;
    }

    // Alt+Tab's deck, just behind the bar.
    let mut deck_elements: Vec<OutputElement> = Vec::new();
    if deck_shown && let Some(mut deck) = state.deck.take() {
        if deck.visible(wall) {
            deck_elements = deck_elements_for(
                state,
                &mut chrome,
                renderer,
                &mut deck,
                output,
                output_geo,
                scale.x,
                now,
                wall,
                camera,
                step,
                rain_output,
                ui_alpha,
            );
        }
        if !deck.done(wall) {
            state.deck = Some(deck);
        }
    }
    elements.splice(behind_bar..behind_bar, deck_elements);
    elements.splice(behind_bar..behind_bar, pane_layer);

    // Bullet time: every workspace in view gets its frame, number and caption, behind its windows.
    if let (true, true, Some(area)) = (first_output && ui > 0.0, zoomed_out > 0.0, area_local) {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        let home = state.bullet.as_ref().map(|mode| mode.home);
        for index in 0..state.workspaces.count() {
            let dx = (index as f64 - camera) * step;
            if dx.abs() * zoom > output_geo.size.w as f64 {
                continue;
            }
            let frame = to_overview(Rectangle::new(
                (area.loc.x + dx - 10.0, area.loc.y - 10.0).into(),
                (area.size.w + 20.0, area.size.h + 20.0).into(),
            ));
            let viewed = index == viewed_index;
            // The frame a dragged window would land in lights up.
            let drop_target = state
                .bullet
                .as_ref()
                .and_then(|mode| mode.pointer.drag.as_ref())
                .is_some_and(|drag| drag.over == Some(index));
            let (colour, thickness) = if viewed || drop_target {
                (overview::frame_viewed(state.bullet_rgb), 3)
            } else {
                (overview::FRAME, 2)
            };
            world.extend(
                chrome
                    .overview
                    .outline(frame, 0.0, thickness, colour, world_scale, fade)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
            let count = state.workspaces.get(index).len();
            let mut caption = bullet::window_count(count);
            if home == Some(index) {
                caption.push_str(" · you started here");
            }
            let at = Point::from((frame.loc.x + 4.0, frame.loc.y - 46.0));
            world.extend(
                chrome
                    .overview
                    .workspace_label(
                        renderer,
                        &state.workspaces.label(index),
                        viewed,
                        &caption,
                        at,
                        world_scale.x,
                        fade,
                    )
                    .map(OutputElement::Memory),
            );
            if count == 0 {
                // High in the frame, clear of the wallpaper's logo in the middle of the screen.
                let middle = Point::from((
                    frame.loc.x + frame.size.w / 2.0,
                    frame.loc.y + frame.size.h * 0.15,
                ));
                world.extend(
                    chrome
                        .overview
                        .empty_note(renderer, index + 1, middle, world_scale.x, fade)
                        .map(OutputElement::Memory),
                );
            }
            if state.bullet.is_some() {
                state.frame_hits.push((index, frame));
            }
        }
    }

    // Closed windows fade in front of the windows retiling into their space, and behind the
    // docked pane as they were. Not in bullet time's overview, which draws windows where no ghost
    // was captured.
    if zoomed_out <= 0.0 {
        let context = renderer.context_id();
        let screen = output_geo.to_f64();
        let falling =
            state.ghosts.iter().any(|ghost| ghost.rain.is_some()) && chrome.fx.ready(renderer);
        let mut pieces: Vec<OutputElement> = Vec::new();
        for ghost in state
            .ghosts
            .iter_mut()
            .filter(|ghost| ghost.overlaps(screen))
        {
            // Falling away in the chosen effects, when closing into the rain is on.
            if let Some(effects) = ghost.rain
                && falling
            {
                let still: Vec<OutputElement> = ghost
                    .still_elements(&context, scale)
                    .into_iter()
                    .map(OutputElement::Texture)
                    .collect();
                let rect = Rectangle::new(ghost.rect.loc - screen.loc, ghost.rect.size);
                let t = now - ghost.started;
                if let Some(element) = chrome.fx.fall(
                    renderer,
                    effects,
                    &mut ghost.texture,
                    still,
                    rect,
                    screen.size.h,
                    scale.x,
                    t,
                ) {
                    pieces.push(OutputElement::Shaded(element));
                }
                continue;
            }
            pieces.extend(
                ghost
                    .elements(&context, screen.loc, scale, now)
                    .into_iter()
                    .map(OutputElement::Texture),
            );
        }
        world.splice(pane_front..pane_front, pieces);
    }
    state.ghosts.retain(|ghost| !ghost.done(now));
    state.pictures.retain(|(window, _)| window.alive());
    if let Some(ghost) = front_ghost {
        let alpha = 0.8 * zoomed_out.min(1.0) as f32 * ui_alpha;
        let edges = chrome.overview.outline(
            ghost,
            0.0,
            2,
            overview::ring(state.bullet_rgb),
            world_scale,
            alpha,
        );
        world.splice(0..0, edges.into_iter().map(OutputElement::Solid));
    }
    // The upright labels go in front, then the windows and frames: tilted while bullet time is
    // zoomed out, as they are otherwise.
    elements.append(&mut front);
    let tilted = if tilting && !world.is_empty() {
        let fade = zoomed_out.min(1.0) as f32;
        let look = tilt::Look {
            fog: 0.4 * fade,
            mirror: mirror_line.map_or((0.0, 0.0, 0.0), |(y, depth)| {
                (y, depth, 0.2 * fade * ui_alpha)
            }),
        };
        chrome.stage.element(
            renderer,
            &world,
            output_geo.size,
            physical,
            scale.x,
            dense,
            &tilt,
            &look,
        )
    } else {
        None
    };
    match tilted {
        Some(element) => elements.push(OutputElement::Shaded(element)),
        None => elements.append(&mut world),
    }

    // The keyboard-tips prompt, unless the explorer or bullet time is open over it.
    if tiling_output
        && ui > 0.0
        && zoomed_out <= 0.0
        && !used[active]
        && !state.explorer.is_open()
        && !state.sheet.is_open()
    {
        elements.extend(
            chrome
                .tips(
                    renderer,
                    output_geo.size,
                    active_dx,
                    scale,
                    ui_alpha,
                    (!state.clock.reduced_motion).then_some(wall),
                )
                .map(OutputElement::Memory),
        );
    }

    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::BACK,
            scale,
            ui_alpha,
        ));
    }

    // Where the turning page sits in the list, for the wallpaper to lie over it behind the glass.
    let mut fallen_at = None;
    // And where the bar's own pane sits, when it has one, with how far gone it is.
    let mut bar_fallen_at = None;
    let bar_gone = crate::glass::nearer(1.0 - ui);
    if glass {
        let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
        match chrome.glass.element(
            renderer,
            &pane,
            output_geo.size,
            physical,
            scale.x,
            1.0 - ui,
            0.0,
        ) {
            Some(element) => {
                // The bar's pane first: it is in front.
                if !bar_pane.is_empty() {
                    match chrome.glass_bar.element(
                        renderer,
                        &bar_pane,
                        output_geo.size,
                        physical,
                        scale.x,
                        bar_gone,
                        crate::glass::NEARER,
                    ) {
                        Some(bar) => {
                            bar_fallen_at = Some(elements.len());
                            elements.push(OutputElement::Shaded(bar));
                        }
                        None => elements.append(&mut bar_pane),
                    }
                }
                fallen_at = Some(elements.len());
                elements.push(OutputElement::Shaded(element));
            }
            // Drawn plainly this once; a glass that's broken stays off from now on.
            None => {
                elements.append(&mut bar_pane);
                elements.extend(pane);
            }
        }
    }

    // Coming back from the lock, the desktop is drawn whole into a texture that grows into place
    // and brightens, while the lock's card and veil fade in front of it.
    if tiling_output && !glass {
        if let Some((zoom, opacity)) = state.unlocking.as_ref().map(|u| u.desktop(wall)) {
            let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
            match unlock_zoom(
                renderer,
                &mut chrome,
                &pane,
                output_geo.size,
                physical,
                scale.x,
                zoom,
                opacity,
            ) {
                Some(element) => elements.push(OutputElement::Texture(element)),
                None => elements.extend(pane),
            }
        }
    }
    if let Some(unlocking) = state.unlocking.as_mut() {
        let (alpha, _) = unlocking.card_state(wall);
        let mut lock: Vec<OutputElement> = Vec::new();
        if first_output {
            lock.extend(
                unlocking
                    .elements(renderer, output_geo.size, scale.x, wall)
                    .into_iter()
                    .map(OutputElement::Memory),
            );
        }
        if tiling_output && alpha > 0.0 {
            lock.push(veil(&mut chrome, output_geo.size, scale, alpha));
        }
        elements.splice(pane_start..pane_start, lock);
    }

    // The living wallpaper, behind everything: dim while the UI is up, full once it fades. It
    // holds still under a window that fills the screen.
    if tiling_output {
        let paused = state.fullscreen_on(active) && ui >= 1.0;
        let glow = 0.45 + 0.55 * (1.0 - ui);
        // Slowed for bullet time, a wallpaper step lasts many frames, so it tweens between them.
        let tween = state.clock.in_bullet_time();
        // The clock, the date, the workspace and the battery, for variations that show them.
        let readings = {
            let status = state.status.lock().unwrap();
            crate::saver::Readings {
                time: status.time.clone(),
                date: status.date.clone(),
                place: state.workspaces.label(active),
                battery: status.battery,
            }
        };
        saver.set_readings(&readings);
        // A trace for each agent at work, over the wallpaper once the desktop has gone.
        elements.extend(
            state
                .scope
                .element(renderer, &name, output_geo.size, scale.x, now, ui)
                .map(OutputElement::Memory),
        );
        elements.extend(
            saver
                .element(renderer, output_geo.size, scale.x, now, glow, paused, tween)
                .map(OutputElement::Memory),
        );
        // The same wallpaper over the page, wherever the page has turned behind the glass.
        if let Some(at) = fallen_at {
            let through = saver
                .again(renderer, output_geo.size, glow)
                .and_then(|wallpaper| {
                    chrome.glass.through(
                        renderer,
                        wallpaper,
                        output_geo.size,
                        physical,
                        scale.x,
                        (1.0 - ui, 0.0),
                        None,
                        BACKGROUND,
                    )
                });
            if let Some(through) = through {
                elements.insert(at, OutputElement::Shaded(through));
            }
            // The bar's pane goes behind the wallpaper after the windows' has, so where it is
            // behind, the order is the wallpaper, the bar's pane, then the windows'.
            if let Some(bar_at) = bar_fallen_at {
                let windows =
                    chrome
                        .glass
                        .shown(renderer, output_geo.size, physical, 1.0 - ui, 0.0);
                let through = saver
                    .again(renderer, output_geo.size, glow)
                    .and_then(|wallpaper| {
                        chrome.glass_bar.through(
                            renderer,
                            wallpaper,
                            output_geo.size,
                            physical,
                            scale.x,
                            (bar_gone, crate::glass::NEARER),
                            windows,
                            BACKGROUND,
                        )
                    });
                if let Some(through) = through {
                    elements.insert(bar_at, OutputElement::Shaded(through));
                }
            }
        }
    }
    // A screenshot's flash, over the whole screen it was taken of.
    if let Some(alpha) = state.flash_alpha(&name) {
        chrome.flash.update(output_geo.size, WHITE);
        elements.insert(
            0,
            OutputElement::Solid(SolidColorRenderElement::from_buffer(
                &chrome.flash,
                (0, 0),
                scale,
                alpha,
                Kind::Unspecified,
            )),
        );
    }
    // The last thing on the way out, over everything the screen holds, the bar included.
    if tiling_output {
        let alpha = state.exit.as_ref().map_or(0.0, |exit| exit.blackout(wall));
        if alpha > 0.0 {
            chrome.blackout.update(output_geo.size, BLACK);
            elements.insert(
                0,
                OutputElement::Solid(SolidColorRenderElement::from_buffer(
                    &chrome.blackout,
                    (0, 0),
                    scale,
                    alpha,
                    Kind::Unspecified,
                )),
            );
        }
    }
    chrome.panes.finish();
    state.chromes.insert(name.clone(), chrome);
    state.savers.insert(name, saver);
    elements
}

/// How far a window may overhang the tiling area before it is scaled into it: enough for the
/// rounding at a fractional output scale, nowhere near a client that won't shrink.
const SLACK: f64 = 4.0;

/// What the bar on screen `screen_index` shows, with workspace `active` on screen and `used`
/// saying which workspaces hold windows. On the screen the keyboard is on (`first_output`) it
/// follows bullet time's overview while that's open, and names what has the keyboard; elsewhere
/// it names the window the keyboard would come back to there.
fn bar_content(
    state: &Slipstream,
    active: usize,
    screen_index: Option<usize>,
    first_output: bool,
    used: &[bool],
) -> bar::Content {
    let bullet = state.bullet.as_ref().filter(|_| first_output);
    let (highlighted, home) = bullet::bar_workspaces(active, bullet);
    let keyboard_here = first_output;
    let title = match bullet {
        Some(_) => state.bullet_bar_title(),
        None if keyboard_here => state.focused_title(),
        // Where the keyboard isn't, the window it would come back to on this screen.
        None => state
            .workspaces
            .get(active)
            .last_focus
            .as_ref()
            .filter(|window| state.workspaces.find(window) == Some(active))
            .map(crate::state::window_title)
            .unwrap_or_default(),
    };
    bar::Content {
        active: highlighted,
        home,
        occupied: used.to_vec(),
        elsewhere: (0..state.workspaces.count())
            .map(|index| {
                state
                    .screens
                    .showing(index)
                    .is_some_and(|showing| Some(showing) != screen_index)
            })
            .collect(),
        keyboard_here,
        labels: (0..state.workspaces.count())
            .map(|index| state.workspaces.label(index))
            .collect(),
        title,
        mode: state.bullet.as_ref().map(|_| {
            let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
            let [r, g, b] = state.bullet_rgb;
            (
                "BULLET TIME",
                (channel(r) << 24) | (channel(g) << 16) | (channel(b) << 8) | 0xff,
            )
        }),
        status: bar::Shown::of(&state.status.lock().unwrap()),
        do_not_disturb: state.settings.notifications.do_not_disturb,
        unread: state.notices.unread(),
        sharing: state.captures.sharing(),
        caps_lock: state
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().caps_lock),
        awake: state.awake,
        meter: state.meter.lock().unwrap().clone(),
        // The pane hangs from the bar of the screen the streams are on.
        dock: state
            .docked_on_show()
            .filter(|_| screen_index == Some(0))
            .and_then(|_| {
                let frame = crate::dock::frame(state.docked_rect()?);
                let left = frame.x - state.screen_rect(0)?.x;
                Some((left, left + frame.w))
            }),
    }
}

/// The panels and cards, on the screen the keyboard is on, front to back: the explorer, quick
/// settings, notifications, clipboard history, the key list, Alt+Tab's flat switcher (when the deck
/// isn't drawn), gravity's arrangements, the low battery card, the new screen card, the offer at
/// login, the share picker and the way out.
#[allow(clippy::too_many_arguments)]
fn card_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    size: Size<i32, Logical>,
    scale: f64,
    now: f64,
    wall: f64,
    deck_shown: bool,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    if state.explorer.is_open() {
        let running = state.running_apps();
        let ring = state.panel_ring();
        elements.extend(
            state
                .explorer
                .element(renderer, size.w, scale, now, running, ring)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    if state.quick.is_open() {
        let facts = state.quick_facts();
        elements.extend(
            state
                .quick
                .element(renderer, size.w, scale, now, facts)
                .map(OutputElement::Memory),
        );
    }
    if state.centre.is_open() {
        state.load_background_app_icons(scale);
        let facts = state.centre_facts();
        elements.extend(
            state
                .centre
                .element(renderer, size, scale, now, facts, &state.notices, &state.background_apps)
                .map(OutputElement::Memory),
        );
    }
    if state.history.is_open() {
        let ring = state.panel_ring();
        let now = state.wall();
        elements.extend(
            state
                .history
                .element(renderer, size, scale, now, ring)
                .map(OutputElement::Memory),
        );
    }
    if state.sheet.is_open() {
        let ring = state.panel_ring();
        let effects = state.settings.motion.effects;
        elements.extend(
            state
                .sheet
                .element(renderer, size, scale, now, &state.bindings, ring, effects)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    // Alt+Tab's switcher, on the screen the keyboard is on.
    if state.switcher.is_some() && !deck_shown {
        elements.extend(
            state
                .switcher_element(renderer, size, scale)
                .map(OutputElement::Memory),
        );
    }
    // Gravity's arrangements, while Super is held after Super+T.
    if state.arrange.is_some() {
        elements.extend(
            state
                .arrange_element(renderer, size, scale)
                .map(OutputElement::Memory),
        );
    }
    // The low battery card, above everything but the lock.
    if state.battery.card.is_some() {
        let mut card = state.battery.card.take();
        if let Some(card) = card.as_mut() {
            elements.extend(
                card.element(renderer, size, scale, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.battery.card = card;
    }
    // The card asking what a screen never seen here should show. Drawn where the keyboard is, as
    // the other cards are: that is the screen being looked at when the lead goes in.
    if state.connect.is_some() {
        let mut connect = state.connect.take();
        if let Some(connect) = connect.as_mut() {
            elements.extend(
                connect
                    .element(renderer, size, scale, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.connect = connect;
    }
    // The offer at login, above everything else it is asking about.
    if state.offer.is_some() {
        let mut offer = state.offer.take();
        if let Some(offer) = offer.as_mut() {
            elements.extend(
                offer
                    .element(renderer, size, scale, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.offer = offer;
    }
    // The share picker, on the screen the keyboard is on: the app waiting for it is usually the
    // one being looked at.
    if state.share.is_some() {
        let [r, g, b] = state
            .ring_rgb
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let ring = u32::from_be_bytes([r, g, b, 0xff]);
        let mut share = state.share.take();
        if let Some(share) = share.as_mut() {
            elements.extend(
                share
                    .element(renderer, size, scale, wall, ring)
                    .map(OutputElement::Memory),
            );
        }
        state.share = share;
    }
    // The way out: the ask card in the middle of the screen, or the slim one while the apps are
    // closing. It draws whatever else is up, and the fade to black goes over everything below.
    if state.exit.is_some() {
        let ring = state.panel_ring();
        let mut exit = state.exit.take();
        if let Some(exit) = exit.as_mut() {
            elements.extend(
                exit.element(renderer, size, scale, wall, ring)
                    .map(OutputElement::Memory),
            );
        }
        state.exit = exit;
    }
    elements
}

/// Whether `window` has been sent a size it hasn't acked and drawn yet.
fn awaiting_size(window: &Window) -> bool {
    use smithay::wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData};
    let Some(toplevel) = window.toplevel() else {
        return false;
    };
    let sent = with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .ok()
                    .map(|data| data.current_server_state().size)
            })
    })
    .flatten();
    let acked = toplevel.with_committed_state(|state| state.and_then(|state| state.size));
    sent.is_some() && sent != acked
}

/// Where a window drawn at `rect` goes to stay inside both its `tile` and the tiling `area`:
/// scaled down evenly (never up) and moved in from any edge it crosses. Rects are x, y, w, h.
fn fit_within(rect: [f64; 4], tile: [f64; 4], area: [f64; 4]) -> [f64; 4] {
    let (left, top) = (tile[0].max(area[0]), tile[1].max(area[1]));
    let right = (tile[0] + tile[2]).min(area[0] + area[2]);
    let bottom = (tile[1] + tile[3]).min(area[1] + area[3]);
    let (room_w, room_h) = ((right - left).max(1.0), (bottom - top).max(1.0));
    let k = (room_w / rect[2]).min(room_h / rect[3]).min(1.0);
    let (w, h) = (rect[2] * k, rect[3] * k);
    [
        rect[0].min(right - w).max(left),
        rect[1].min(bottom - h).max(top),
        w,
        h,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_screens_windows_are_drawn_off_this_one_whichever_side_its_workspace_is() {
        // The laptop (1536 wide, at 0) and a monitor (1920 wide, at 1536).
        let step = (1536 + motion::WORKSPACE_GAP) as f64;
        // A window at the monitor's left edge, drawn on the laptop.
        let drawn_at =
            |index: usize, camera: f64| 1536.0 + shift_for_workspace(index, camera, step, 0, 1536);
        // The laptop on workspace 2, the monitor on workspace 1: a lower workspace, so to the left.
        assert!(drawn_at(0, 1.0) < 0.0, "{}", drawn_at(0, 1.0));
        // The laptop on workspace 1, the monitor on workspace 3: off the right.
        assert!(drawn_at(2, 0.0) >= 1536.0, "{}", drawn_at(2, 0.0));
        // Its own workspace sits where the layout put it, on its own screen's edge.
        assert_eq!(0.0 + shift_for_workspace(1, 1.0, step, 0, 0), 0.0);
    }

    #[test]
    fn with_reduced_motion_a_tier_tag_appears_where_it_rests() {
        for age in [0.0, 0.02, 0.05, 0.1, 0.5, 1.0, 1.75] {
            assert_eq!(tag_motion(age, true).1, 0.0, "no drop at {age} s");
        }
        assert_eq!(
            tag_motion(0.1, true).0,
            1.0,
            "fully in after the short fade"
        );
        assert!(
            tag_motion(0.05, false).1 < -5.0,
            "without it, the tag still drops"
        );
        assert_eq!(tag_motion(0.5, false), (1.0, 0.0));
    }

    #[test]
    fn a_window_too_big_for_its_tile_shrinks_into_it_and_its_workspace() {
        let area = [0.0, 40.0, 1000.0, 600.0];
        // A client that won't go narrower than 700 in a 490-wide tile.
        let tile = [510.0, 40.0, 490.0, 600.0];
        let [x, y, w, h] = fit_within([510.0, 40.0, 700.0, 600.0], tile, area);
        assert!(x >= 510.0 && x + w <= 1000.0 + 1e-9, "inside the tile");
        assert!((w / h - 700.0 / 600.0).abs() < 1e-9, "it keeps its shape");
        assert_eq!(y, 40.0);

        // Fullscreen covers the bar above the area.
        let screen = [0.0, 0.0, 1000.0, 640.0];
        let [_, y, _, h] = fit_within(screen, screen, area);
        assert!(y >= 40.0 && y + h <= 640.0 + 1e-9, "below the bar");

        // The code rain takes width from the right: a client that won't shrink is brought inside
        // the area rather than drawn under the glyphs.
        let narrowed = [0.0, 40.0, 760.0, 600.0];
        let wide = [0.0, 40.0, 1000.0, 600.0];
        let [x, _, w, _] = fit_within(wide, wide, narrowed);
        assert!(x + w <= 760.0 + 1e-9, "clear of the streams");
        assert!(w < 1000.0, "scaled down, not cropped");

        assert_eq!(
            fit_within(tile, tile, area),
            tile,
            "a window that fits stays put"
        );
    }

    #[test]
    fn the_key_hint_is_painted_at_the_screen_scale() {
        let (pixmap, size) = paint_tips(1.25).unwrap();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            (
                (size.w as f64 * 1.25).round() as u32,
                (size.h as f64 * 1.25).round() as u32
            )
        );
        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] == 255));
    }
}

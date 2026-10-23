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
mod frame;
mod frames;
mod lock;
mod png_file;

pub use chrome::Chrome;
use chrome::*;
use deck::*;
use frame::View;
pub use frame::output_elements;
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

/// A finished frame's element as the screen shows it, magnified by the zoom.
pub type ZoomedElement<'a> = RescaleRenderElement<&'a OutputElement>;

/// `elements` as the screen shows them: each magnified about the zoom's centre, so the whole
/// frame is, whoever drew it. Without zoom the scale is 1 and nothing moves. Screenshots and
/// screen sharing take `elements` themselves, so they get the unmagnified screen.
pub fn zoomed(elements: &[OutputElement], view: crate::zoom::View) -> Vec<ZoomedElement<'_>> {
    elements
        .iter()
        .map(|element| RescaleRenderElement::from_element(element, view.centre, view.level))
        .collect()
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
        mode: state
            .bullet
            .as_ref()
            .map(|_| ("BULLET TIME", opaque(state.bullet_rgb))),
        status: bar::Shown::of(&crate::status::held(&state.status)),
        do_not_disturb: state.settings.notifications.do_not_disturb,
        unread: state.notices.unread(),
        sharing: state.captures.sharing(),
        caps_lock: state
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().caps_lock),
        awake: state.awake,
        layout: state.layout_label(),
        meter: crate::status::held(&state.meter)
            .as_ref()
            .map(|reading| crate::meter::shown(reading, &state.settings.meter)),
        modules: state.bar_modules.shown(),
        hovered_module: screen_index
            .and_then(|index| state.screens.get(index))
            .and_then(|screen| {
                let (name, id) = state.module_hover.over()?;
                (*name == screen.output.name()).then_some(*id)
            }),
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
fn card_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    size: Size<i32, Logical>,
    scale: f64,
    now: f64,
    wall: f64,
    deck_shown: bool,
) -> Vec<OutputElement> {
    let frame = |time| crate::panel::Frame { size, scale, time };
    let mut elements = Vec::new();
    if state.explorer.is_open() {
        let running = state.running_apps();
        let ring = state.panel_ring();
        let keys = crate::explorer::Keys::new(|action| state.key_label(action));
        elements.extend(
            state
                .explorer
                .element(renderer, frame(now), running, ring, keys)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    if state.quick.is_open() {
        let facts = state.quick_facts();
        elements.extend(
            state
                .quick
                .element(renderer, frame(now), facts)
                .map(OutputElement::Memory),
        );
    }
    if state.centre.is_open() {
        let chips = state.background_chips(scale);
        let facts = state.centre_facts();
        let explorer = &mut state.explorer;
        let mut icon = |name: &str, px: u32| explorer.named_icon(name, px);
        elements.extend(
            state
                .centre
                .elements(
                    renderer,
                    frame(now),
                    facts,
                    &state.notices,
                    chips,
                    &mut icon,
                )
                .into_iter()
                .map(OutputElement::Memory),
        );
        state.finish_chip_launch(now);
    }
    if state.history.is_open() {
        let ring = state.panel_ring();
        let now = state.wall();
        let key = state.key_label(crate::keys::Action::ClipboardHistory);
        elements.extend(
            state
                .history
                .element(renderer, frame(now), ring, key)
                .map(OutputElement::Memory),
        );
    }
    if state.sheet.is_open() {
        let ring = state.panel_ring();
        let effects = state.settings.motion.effects;
        elements.extend(
            state
                .sheet
                .element(
                    renderer,
                    frame(now),
                    crate::sheet::Listing {
                        bindings: &state.bindings,
                        style: state.settings.keys.style,
                        screens: state.space.outputs().count(),
                        own: state.shortcuts.own(),
                    },
                    ring,
                    effects,
                )
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
    if let Some(card) = state.battery.card.as_mut() {
        elements.extend(
            card.element(renderer, frame(wall))
                .map(OutputElement::Memory),
        );
    }
    // The card asking what a screen never seen here should show. Drawn where the keyboard is, as
    // the other cards are: that is the screen being looked at when the lead goes in.
    if let Some(connect) = state.connect.as_mut() {
        elements.extend(
            connect
                .element(renderer, frame(wall))
                .map(OutputElement::Memory),
        );
    }
    // The offer at login, above everything else it is asking about.
    if let Some(offer) = state.offer.as_mut() {
        elements.extend(
            offer
                .element(renderer, frame(wall))
                .map(OutputElement::Memory),
        );
    }
    // The share picker, on the screen the keyboard is on: the app waiting for it is usually the
    // one being looked at.
    let ring = opaque(state.ring_rgb);
    if let Some(share) = state.share.as_mut() {
        elements.extend(
            share
                .element(renderer, frame(wall), ring)
                .map(OutputElement::Memory),
        );
    }
    // The card asking about an app's shortcuts, where the keyboard is.
    if let Some(ask) = state.shortcuts.ask_mut() {
        elements.extend(
            ask.element(renderer, frame(wall), ring)
                .map(OutputElement::Memory),
        );
    }
    // The way out: the ask card in the middle of the screen, or the slim one while the apps are
    // closing. It draws whatever else is up, and the fade to black goes over everything below.
    let ring = state.panel_ring();
    if let Some(exit) = state.exit.as_mut() {
        elements.extend(
            exit.element(renderer, frame(wall), ring)
                .map(OutputElement::Memory),
        );
    }
    elements
}

/// The cursor theme's shape `name` with its hotspot at `pos`, `time` into its animation.
fn themed_pointer(
    renderer: &mut GlesRenderer,
    cursor: &mut Cursor,
    name: &str,
    pos: Point<f64, Logical>,
    scale: Scale<f64>,
    time: Duration,
) -> Option<OutputElement> {
    let (buffer, hotspot) = cursor.image(name, scale.x, time);
    match MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (pos - hotspot).to_physical(scale),
        &buffer,
        None,
        None,
        None,
        Kind::Cursor,
    ) {
        Ok(element) => Some(OutputElement::Memory(element)),
        Err(err) => {
            tracing::warn!("couldn't draw the pointer: {err:?}");
            None
        }
    }
}

/// The clock, the date and the battery for the wallpaper's variations that show them, with
/// `place` naming the workspace on screen.
fn saver_readings(state: &Slipstream, place: String) -> crate::saver::Readings {
    let status = crate::status::held(&state.status);
    crate::saver::Readings {
        time: status.time.clone(),
        date: status.date.clone(),
        place,
        battery: status.battery,
    }
}

/// `rgb`, opaque, packed as `0xrrggbbaa` for the painter.
fn opaque(rgb: [f32; 3]) -> u32 {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    u32::from_be_bytes([r, g, b, 0xff])
}

/// Whether `window` has been sent a size it hasn't acked and drawn yet.
fn awaiting_size(window: &Window) -> bool {
    use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
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

/// What the frames last drew: where things landed, for the pointer, the windows drawn from outside the space, and each screen's drawing buffers.
#[derive(Default)]
pub struct Frames {
    /// Where windows and workspace frames were drawn in the overview, for clicks.
    pub overview_hits: Vec<(Window, Rectangle<f64, Logical>)>,
    /// Where notification pop-ups, the toast and the volume display were drawn last frame, from
    /// the space's origin: the pointer over them is over glass, not the window beneath.
    pub chrome_areas: Vec<Rectangle<f64, Logical>>,
    /// Windows too big for their tiles, and where they were last drawn scaled into them, for
    /// the pointer. Global logical coordinates.
    pub fitted: Vec<(Window, Rectangle<f64, Logical>)>,
    /// What each window was last drawn from, for its fade when it closes.
    pub pictures: Vec<(Window, crate::ghost::Picture)>,
    /// Windows drawn in the last frame that aren't in the space (bullet time's other workspaces,
    /// a workspace sliding away), so they get frame callbacks and keep drawing.
    pub drawn_off_space: Vec<Window>,
    pub frame_hits: Vec<(usize, Rectangle<f64, Logical>)>,
    /// How the overview was last tilted on screen, to map clicks back onto it.
    pub overview_tilt: Tilt,
    /// The desktop's own drawing, one set per screen: every buffer in it is sized to the screen
    /// it is for, so sharing one between two screens of different sizes would repaint it twice a
    /// frame and, in the wallpaper's case, restart the animation.
    pub chromes: HashMap<String, Chrome>,
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
        let (pixmap, size) = paint_tips(1.25, "Super+/").unwrap();
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

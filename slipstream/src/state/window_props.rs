//! What a window's client has said about itself (its app id, title, minimum size and parent),
//! and the size and state it is told in return.

use smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes;

use super::*;

/// The most a client's minimum size is taken at, in logical pixels: wider and taller than any
/// tiling area, so a real minimum is never cut short.
const MIN_SIZE_LIMIT: i32 = 16384;

/// The smallest size a window's client will draw at, width and height, 0 where it sets none.
pub(crate) fn min_size(window: &Window) -> (i32, i32) {
    if let Some(surface) = window.x11_surface() {
        return clamp_min_size(surface.min_size().map_or((0, 0), |size| (size.w, size.h)));
    }
    window.toplevel().map_or((0, 0), |toplevel| {
        with_states(toplevel.wl_surface(), |states| {
            let size = states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .min_size;
            clamp_min_size((size.w, size.h))
        })
    })
}

/// A minimum size as a client sent it, brought into range. Neither `xdg_toplevel.set_min_size`
/// nor X11's size hints are checked before they arrive, and a width near `i32::MAX` would
/// overflow the tiling's arithmetic.
fn clamp_min_size((w, h): (i32, i32)) -> (i32, i32) {
    (w.clamp(0, MIN_SIZE_LIMIT), h.clamp(0, MIN_SIZE_LIMIT))
}

/// Reads an xdg toplevel's role data: its title and app id, as its client last set them.
pub(super) fn toplevel_data<T>(
    surface: &WlSurface,
    read: impl FnOnce(&XdgToplevelSurfaceRoleAttributes) -> Option<T>,
) -> Option<T> {
    with_states(surface, |states| {
        let data = states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .ok()?;
        read(&data)
    })
}

pub(crate) fn window_title(window: &Window) -> String {
    if let Some(surface) = window.x11_surface() {
        return surface.title();
    }
    window
        .toplevel()
        .and_then(|toplevel| toplevel_data(toplevel.wl_surface(), |data| data.title.clone()))
        .unwrap_or_default()
}

/// A window's Wayland app ID or X11 class, lowercased, to match it to its desktop entry.
pub(crate) fn window_app_id(window: &Window) -> Option<String> {
    if let Some(surface) = window.x11_surface() {
        return Some(surface.class().to_lowercase());
    }
    let toplevel = window.toplevel()?;
    toplevel_data(toplevel.wl_surface(), |data| data.app_id.clone()).map(|id| id.to_lowercase())
}

/// The last dot-separated segment of an app ID, with the first letter capitalised:
/// `com.discord.Discord` → `Discord`, `spotify` → `Spotify`.
pub(super) fn pretty_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// How the log names a window: by its app id, never by its title, which can hold a folder, a
/// document's name, a URL or who a message is from. `window_name` is for text on the screen.
pub(crate) fn logged_app(window: &Window) -> String {
    window_app_id(window).unwrap_or_else(|| "unknown app".to_string())
}

/// Whether `window` is `parent`'s own: an xdg toplevel whose parent is its surface, or an X11
/// window transient for it.
pub(crate) fn is_child_of(window: &Window, parent: &Window) -> bool {
    if let (Some(child), Some(parent)) = (window.toplevel(), parent.wl_surface()) {
        return child.parent().as_ref() == Some(&*parent);
    }
    if let (Some(child), Some(parent)) = (window.x11_surface(), parent.x11_surface()) {
        return child.is_transient_for() == Some(parent.window_id());
    }
    false
}

/// X11 windows that place themselves and are never tiled: menus, tooltips and drag images, and the
/// docks, desktop widgets and notifications `xwayland::own_layer` picks out when they map.
pub(crate) fn places_itself(window: &Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| surface.is_override_redirect())
        || window.user_data().get::<PlacesItself>().is_some()
}

/// Marks a managed X11 window that keeps the place it picks, in `places_itself`.
pub(crate) struct PlacesItself;

/// How opaque an X11 window asks to be drawn (`_NET_WM_WINDOW_OPACITY`, which compositing window
/// managers honour), from 0 to 1. A Wayland window, or one that doesn't say, is opaque.
pub(crate) fn own_opacity(window: &Window) -> f32 {
    window
        .x11_surface()
        .and_then(|surface| surface.opacity())
        .map_or(1.0, |opacity| (opacity as f64 / u32::MAX as f64) as f32)
}

/// An X11 window that takes no keyboard input at all: ICCCM's "no input" model, which neither
/// accepts focus nor asks for `WM_TAKE_FOCUS`.
pub(crate) fn takes_no_input(window: &Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| surface.input_model() == smithay::xwayland::xwm::WmInputModel::None)
}

/// Tells a window it isn't the active one any more, and sends that to its client.
pub(crate) fn deactivate(window: &Window) {
    window.set_activated(false);
    if let Some(toplevel) = window.toplevel()
        && toplevel.is_initial_configure_sent()
    {
        toplevel.send_pending_configure();
    }
}

/// Tells a window its place: the whole screen when `full`, else tile `r`, at a size it can draw
/// (`size_for_tile`), with its tiled edges marked.
pub(super) fn configure(window: &Window, r: Rect, full: bool) {
    let r = if full {
        r
    } else {
        size_for_tile(r, min_size(window))
    };
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            state.size = Some((r.w, r.h).into());
            // Tiled edges tell clients to drop rounded corners and shadows there.
            for edge in [
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ] {
                if full {
                    state.states.unset(edge);
                } else {
                    state.states.set(edge);
                }
            }
            if full {
                state.states.set(xdg_toplevel::State::Fullscreen);
            } else {
                state.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
        // Before a client's first commit, its initial configure carries this state instead.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    } else if let Some(surface) = window.x11_surface() {
        let _ = surface.configure(Rectangle::new((r.x, r.y).into(), (r.w, r.h).into()));
    }
}

/// The size to ask of a window for tile `r`, given its minimum size. The tile itself when the
/// window can go that small. When it can't, the window is drawn scaled down into its tile, so it
/// is asked for the tile's shape at the smallest size it accepts: scaled, it then fills the tile
/// exactly, instead of leaving a band of the tile empty below or beside it.
fn size_for_tile(r: Rect, (min_w, min_h): (i32, i32)) -> Rect {
    if r.w <= 0 || r.h <= 0 || (r.w >= min_w && r.h >= min_h) {
        return r;
    }
    let k = (min_w as f64 / r.w as f64).max(min_h as f64 / r.h as f64);
    Rect {
        w: ((r.w as f64 * k).ceil() as i32).max(min_w),
        h: ((r.h as f64 * k).ceil() as i32).max(min_h),
        ..r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_that_wont_fit_its_tile_is_asked_for_the_tile_s_shape() {
        let tile = Rect {
            x: 10,
            y: 20,
            w: 700,
            h: 900,
        };
        assert_eq!(
            size_for_tile(tile, (400, 300)),
            tile,
            "it fits: the tile itself"
        );
        let asked = size_for_tile(tile, (1034, 277));
        assert_eq!((asked.x, asked.y), (10, 20));
        assert!(asked.w >= 1034 && asked.h >= 277);
        let (shape, asked_shape) = (
            tile.w as f64 / tile.h as f64,
            asked.w as f64 / asked.h as f64,
        );
        assert!(
            (shape - asked_shape).abs() < 0.01,
            "{shape} against {asked_shape}"
        );
        let tall = size_for_tile(tile, (100, 1200));
        assert!(tall.h >= 1200 && (tall.w as f64 / tall.h as f64 - shape).abs() < 0.01);
    }

    #[test]
    fn a_minimum_size_from_a_client_is_brought_into_range() {
        assert_eq!(clamp_min_size((940, 0)), (940, 0), "a real minimum stands");
        assert_eq!(
            clamp_min_size((i32::MAX, i32::MIN)),
            (MIN_SIZE_LIMIT, 0),
            "nothing past the limit, nothing below zero"
        );
    }
}

//! Floating windows: the ones that sit above a workspace's tiling rather than in it.
//!
//! A new window floats when it has a parent (a dialog) or its height is fixed (a splash screen, a
//! picture-in-picture player). A dialog opens centred over its parent; anything else keeps the
//! size it asked for and opens centred in the tiling area. Floating windows are always above the
//! tiled ones on their workspace, with children above their parents. Each keeps its place as a
//! fraction of the tiling area, so a screen that changes size, or a workspace moved to another
//! screen, keeps it in the same part of the screen.
//!
//! Generic over the window type so it's unit-tested without Wayland.

use smithay::{
    desktop::Window,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::{
    layout::{Direction, Rect},
    state::{Slipstream, is_child_of, logged_app, min_size, window_app_id, window_title},
};

/// How far Super+Alt+arrow moves a floating window, in logical pixels.
pub const MOVE_STEP: i32 = 50;
/// How far a window moves off its tile when it's floated, so it's seen to come loose.
pub const LOOSEN: i32 = 50;

/// One floating window.
#[derive(Debug, Clone, PartialEq)]
pub struct Float<T> {
    pub window: T,
    /// The top-left corner, as a fraction of the tiling area's width and height from its corner.
    pub at: (f64, f64),
    /// A size asked for by the keys, until the window is floated again; its own size otherwise.
    pub asked: Option<(i32, i32)>,
}

/// A workspace's floating windows, back to front.
#[derive(Debug, Clone)]
pub struct Floating<T> {
    list: Vec<Float<T>>,
}

impl<T> Default for Floating<T> {
    fn default() -> Self {
        Self { list: Vec::new() }
    }
}

/// Where a `size` window goes centred in `area`, keeping its top left corner in the area when it
/// is bigger, so its title bar can be reached.
pub fn centred(area: Rect, (w, h): (i32, i32)) -> (i32, i32) {
    (
        area.x + ((area.w - w) / 2).max(0),
        area.y + ((area.h - h) / 2).max(0),
    )
}

/// `rect` moved inside `area`, preferring its top left corner where it doesn't fit.
pub fn clamped(area: Rect, rect: Rect) -> Rect {
    let x = rect.x.min(area.x + area.w - rect.w).max(area.x);
    let y = rect.y.min(area.y + area.h - rect.h).max(area.y);
    Rect { x, y, ..rect }
}

fn fraction(area: Rect, (x, y): (i32, i32)) -> (f64, f64) {
    (
        (x - area.x) as f64 / area.w.max(1) as f64,
        (y - area.y) as f64 / area.h.max(1) as f64,
    )
}

impl<T: Clone + PartialEq> Floating<T> {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn contains(&self, window: &T) -> bool {
        self.list.iter().any(|float| float.window == *window)
    }

    /// The windows, back to front.
    pub fn windows(&self) -> Vec<T> {
        self.list.iter().map(|float| float.window.clone()).collect()
    }

    /// The front-most window.
    pub fn top(&self) -> Option<T> {
        self.list.last().map(|float| float.window.clone())
    }

    pub fn get(&self, window: &T) -> Option<&Float<T>> {
        self.list.iter().find(|float| float.window == *window)
    }

    /// A new floating window `size` big, in front: centred over `over` (its parent, where it's
    /// drawn) when given, otherwise in `area`, and kept inside `area` either way.
    pub fn add(&mut self, window: T, size: (i32, i32), over: Option<Rect>, area: Rect) {
        let (x, y) = match over {
            Some(parent) => (
                parent.x + (parent.w - size.0) / 2,
                parent.y + (parent.h - size.1) / 2,
            ),
            None => centred(area, size),
        };
        self.add_at(window, (x, y), size, area);
    }

    /// A floating window in front with its top left corner at `at`, kept inside `area`.
    pub fn add_at(&mut self, window: T, at: (i32, i32), size: (i32, i32), area: Rect) {
        let rect = clamped(
            area,
            Rect {
                x: at.0,
                y: at.1,
                w: size.0,
                h: size.1,
            },
        );
        self.list.retain(|float| float.window != window);
        self.list.push(Float {
            window,
            at: fraction(area, (rect.x, rect.y)),
            asked: None,
        });
    }

    /// Replaces the entry for `float`'s window, keeping its place in the stack.
    pub fn update(&mut self, float: Float<T>) {
        if let Some(kept) = self
            .list
            .iter_mut()
            .find(|kept| kept.window == float.window)
        {
            *kept = float;
        }
    }

    /// Puts back a window taken off another workspace, keeping its place.
    pub fn put(&mut self, float: Float<T>) {
        self.list.retain(|kept| kept.window != float.window);
        self.list.push(float);
    }

    pub fn remove(&mut self, window: &T) -> Option<Float<T>> {
        let index = self.list.iter().position(|float| float.window == *window)?;
        Some(self.list.remove(index))
    }

    /// `window` to the front, and then any of its children in front of it, so a dialog is never
    /// lost behind the window it belongs to.
    pub fn raise(&mut self, window: &T, is_child_of: impl Fn(&T, &T) -> bool) {
        let Some(float) = self.remove(window) else {
            return;
        };
        self.list.push(float);
        let mut parents = vec![window.clone()];
        let mut index = 0;
        while index < self.list.len() - 1 {
            let child = &self.list[index].window;
            if parents.iter().any(|parent| is_child_of(child, parent)) {
                let float = self.list.remove(index);
                parents.push(float.window.clone());
                self.list.push(float);
            } else {
                index += 1;
            }
        }
    }

    /// Where each window is in `area`, back to front, at the size `size_of` says it is (or the
    /// size the keys asked for), kept inside `area`.
    pub fn rects(&self, area: Rect, size_of: &dyn Fn(&T) -> (i32, i32)) -> Vec<(T, Rect)> {
        self.list
            .iter()
            .map(|float| {
                let (w, h) = float.asked.unwrap_or_else(|| size_of(&float.window));
                let rect = Rect {
                    x: area.x + (float.at.0 * area.w as f64).round() as i32,
                    y: area.y + (float.at.1 * area.h as f64).round() as i32,
                    w: w.max(1),
                    h: h.max(1),
                };
                (float.window.clone(), clamped(area, rect))
            })
            .collect()
    }

    /// Moves `window` by `(dx, dy)` inside `area`, `size` being how big it is.
    pub fn move_by(&mut self, window: &T, (dx, dy): (i32, i32), size: (i32, i32), area: Rect) {
        let Some(float) = self.list.iter_mut().find(|float| float.window == *window) else {
            return;
        };
        let x = area.x + (float.at.0 * area.w as f64).round() as i32 + dx;
        let y = area.y + (float.at.1 * area.h as f64).round() as i32 + dy;
        let rect = clamped(
            area,
            Rect {
                x,
                y,
                w: size.0,
                h: size.1,
            },
        );
        float.at = fraction(area, (rect.x, rect.y));
    }

    /// Moves `window`'s top left corner to `at`, as a pointer drag does. Where it's drawn is still
    /// kept inside the area.
    pub fn move_to(&mut self, window: &T, at: (i32, i32), area: Rect) {
        if let Some(float) = self.list.iter_mut().find(|float| float.window == *window) {
            float.at = fraction(area, at);
        }
    }

    /// Asks `window` for `size`.
    pub fn ask(&mut self, window: &T, size: (i32, i32)) {
        if let Some(float) = self.list.iter_mut().find(|float| float.window == *window) {
            float.asked = Some(size);
        }
    }

    /// The floating window to focus from `from` going `direction`: the nearest by its centre along
    /// that way.
    pub fn neighbour(
        &self,
        from: &T,
        direction: Direction,
        area: Rect,
        size_of: &dyn Fn(&T) -> (i32, i32),
    ) -> Option<T> {
        let rects = self.rects(area, size_of);
        let centre = |rect: &Rect| {
            (
                rect.x as f64 + rect.w as f64 / 2.0,
                rect.y as f64 + rect.h as f64 / 2.0,
            )
        };
        let (_, from_rect) = rects.iter().find(|(window, _)| window == from)?;
        let (fx, fy) = centre(from_rect);
        rects
            .iter()
            .filter(|(window, _)| window != from)
            .filter_map(|(window, rect)| {
                let (x, y) = centre(rect);
                let distance = match direction {
                    Direction::Left => fx - x,
                    Direction::Right => x - fx,
                    Direction::Up => fy - y,
                    Direction::Down => y - fy,
                };
                (distance > 0.0).then_some((window, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(window, _)| window.clone())
    }
}

// The desktop's side of it: which windows float, and what the keys, the pointer and the
// compositor's own bookkeeping do with them.

/// The largest size a window's client will draw at, 0 where it sets none.
pub fn max_size(window: &Window) -> (i32, i32) {
    if let Some(surface) = window.x11_surface() {
        return surface.max_size().map_or((0, 0), |size| (size.w, size.h));
    }
    window.toplevel().map_or((0, 0), |toplevel| {
        with_states(toplevel.wl_surface(), |states| {
            let size = states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .max_size;
            (size.w, size.h)
        })
    })
}

/// Whether a new window opens floating: one with a parent (a dialog), or one whose height is
/// fixed (a splash screen). Firefox's picture-in-picture player floats too.
pub fn opens_floating(window: &Window) -> bool {
    let picture_in_picture = window_app_id(window).is_some_and(|id| id.ends_with("firefox"))
        && window_title(window) == "Picture-in-Picture";
    if picture_in_picture {
        return true;
    }
    let has_parent = match (window.toplevel(), window.x11_surface()) {
        (Some(toplevel), _) => toplevel.parent().is_some(),
        (None, Some(surface)) => surface.is_transient_for().is_some(),
        _ => false,
    };
    if has_parent {
        return true;
    }
    let (min, max) = (min_size(window).1, max_size(window).1);
    min > 0 && min == max
}

/// A window's own size, as it last drew itself.
pub fn own_size(window: &Window) -> (i32, i32) {
    let size = window.geometry().size;
    (size.w, size.h)
}

/// Tells a floating window where it is: no tiled edges and not fullscreen, at `asked` if the keys
/// set a size, otherwise at whatever size it chooses. X11 windows are told their place as well.
pub fn configure_floating(window: &Window, asked: Option<(i32, i32)>, rect: Rect) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            if let Some((w, h)) = asked {
                state.size = Some((w, h).into());
            }
            for edge in [
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
                xdg_toplevel::State::Fullscreen,
            ] {
                state.states.unset(edge);
            }
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    } else if let Some(surface) = window.x11_surface() {
        let _ = surface.configure(smithay::utils::Rectangle::new(
            (rect.x, rect.y).into(),
            (rect.w, rect.h).into(),
        ));
    }
}

impl Slipstream {
    /// A new window that floats: on its parent's workspace, centred over its parent, or on the
    /// workspace on screen, centred there.
    pub fn add_floating(&mut self, window: Window) {
        let parent = self
            .workspaces
            .all_windows()
            .into_iter()
            .find(|candidate| is_child_of(&window, candidate));
        let workspace = parent
            .as_ref()
            .and_then(|parent| self.workspaces.find(parent))
            .unwrap_or_else(|| self.active_workspace());
        let Some(area) = self.area_for_workspace(workspace) else {
            return;
        };
        let over = parent.as_ref().and_then(|parent| {
            self.space.element_geometry(parent).map(|geo| Rect {
                x: geo.loc.x,
                y: geo.loc.y,
                w: geo.size.w,
                h: geo.size.h,
            })
        });
        let (w, h) = own_size(&window);
        let sized = w > 0 && h > 0;
        let size = if sized { (w, h) } else { (640, 480) };
        self.workspaces
            .get_mut(workspace)
            .floating
            .add(window.clone(), size, over, area);
        // Many toolkits map before they know their size: it's centred again when it does.
        if !sized {
            self.centre_when_sized
                .push((window.clone(), parent.clone()));
        }
        self.retile();
        if self.may_take_keyboard(&window, None) {
            self.focus_window(&window);
        } else {
            self.next_in_line(&window);
        }
        tracing::info!(
            workspace = workspace + 1,
            app = logged_app(&window),
            dialog = parent.is_some(),
            "new window floating"
        );
    }

    /// A floating window that opened before it knew its size has one now: it's centred with it,
    /// over its parent or in its area, as it would have been.
    pub fn centre_now_sized(&mut self, window: &Window) {
        let Some(at) = self
            .centre_when_sized
            .iter()
            .position(|(waiting, _)| waiting == window)
        else {
            return;
        };
        let size = own_size(window);
        if size.0 <= 0 || size.1 <= 0 {
            return;
        }
        let (_, parent) = self.centre_when_sized.remove(at);
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let over = parent.and_then(|parent| {
            self.space.element_geometry(&parent).map(|geo| Rect {
                x: geo.loc.x,
                y: geo.loc.y,
                w: geo.size.w,
                h: geo.size.h,
            })
        });
        let floating = &mut self.workspaces.get_mut(index).floating;
        let Some(mut float) = floating.get(window).cloned() else {
            return;
        };
        let mut placed = Floating::default();
        placed.add(window.clone(), size, over, area);
        if let Some(fresh) = placed.get(window) {
            float.at = fresh.at;
        }
        floating.update(float);
    }

    /// The front-most window of a workspace is its floating stack, back to front, over the tiles.
    pub fn restack_floating(&mut self, workspace: usize) {
        for window in self.workspaces.get(workspace).floating.windows() {
            if self.space.element_location(&window).is_some() {
                self.space.raise_element(&window, false);
            }
        }
    }

    /// Super+Shift+V: the focused window floats, or goes back into the tiling.
    pub fn toggle_floating(&mut self) {
        let Some(window) = self.focused_window() else {
            return;
        };
        if self.fullscreen.as_ref() == Some(&window) {
            return;
        }
        let Some(index) = self.workspaces.find(&window) else {
            return;
        };
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            self.workspaces.get_mut(index).floating.remove(&window);
            let beside = self.most_recent_tiled(index);
            self.workspaces
                .insert(index, window.clone(), beside.as_ref(), area);
            tracing::info!(app = logged_app(&window), "window tiled");
        } else {
            let drawn = self.space.element_geometry(&window);
            self.take_off_workspace(&window);
            let (at, size) = match drawn {
                Some(geo) => (
                    (geo.loc.x + LOOSEN, geo.loc.y + LOOSEN),
                    (geo.size.w, geo.size.h),
                ),
                None => (centred(area, own_size(&window)), own_size(&window)),
            };
            let floating = &mut self.workspaces.get_mut(index).floating;
            floating.add_at(window.clone(), at, size, area);
            floating.ask(&window, size);
            tracing::info!(app = logged_app(&window), "window floating");
        }
        self.retile();
        self.focus_window(&window);
    }

    /// Super+Ctrl+V: the keyboard goes from a floating window to the tiles, or from the tiles to
    /// the front-most floating window.
    pub fn switch_floating_focus(&mut self) {
        let index = self.active_workspace();
        let focused = self.focused_window();
        let floating_focused = focused
            .as_ref()
            .is_some_and(|window| self.workspaces.is_floating(window));
        let next = if floating_focused {
            self.most_recent_tiled(index)
        } else {
            self.workspaces.get(index).floating.top()
        };
        if let Some(window) = next {
            self.focus_window(&window);
        }
    }

    /// The tiled window on workspace `index` used most recently.
    fn most_recent_tiled(&self, index: usize) -> Option<Window> {
        let tiled = self.workspaces.get(index).layout.windows();
        self.focus_history
            .iter()
            .find(|window| tiled.contains(window))
            .cloned()
            .or_else(|| tiled.last().cloned())
    }

    /// Super+arrow on a floating window: the nearest floating window that way.
    pub fn focus_floating_direction(&mut self, window: &Window, direction: Direction) {
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let next = self
            .workspaces
            .get(index)
            .floating
            .neighbour(window, direction, area, &own_size);
        if let Some(next) = next {
            self.focus_window(&next);
        }
    }

    /// Super+Alt+arrow on a floating window: it moves that way.
    pub fn move_floating(&mut self, window: &Window, direction: Direction) {
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let by = match direction {
            Direction::Left => (-MOVE_STEP, 0),
            Direction::Right => (MOVE_STEP, 0),
            Direction::Up => (0, -MOVE_STEP),
            Direction::Down => (0, MOVE_STEP),
        };
        let floating = &mut self.workspaces.get_mut(index).floating;
        let size = floating
            .get(window)
            .and_then(|float| float.asked)
            .unwrap_or_else(|| own_size(window));
        floating.move_by(window, by, size, area);
        self.retile();
    }

    /// Super+[ ] (with Shift for height) on a floating window: a twentieth of the area narrower or
    /// wider, never under its minimum or over the area.
    pub fn resize_floating(&mut self, window: &Window, how: crate::layout::Resize) {
        use crate::layout::Resize;
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let Some(area) = self.area_for_workspace(index) else {
            return;
        };
        let floating = &mut self.workspaces.get_mut(index).floating;
        let (w, h) = floating
            .get(window)
            .and_then(|float| float.asked)
            .unwrap_or_else(|| own_size(window));
        let (min_w, min_h) = min_size(window);
        let (step_w, step_h) = (area.w / 20, area.h / 20);
        let size = match how {
            Resize::Wider => ((w + step_w).min(area.w), h),
            Resize::Narrower => ((w - step_w).max(min_w.max(100)), h),
            Resize::Taller => (w, (h + step_h).min(area.h)),
            Resize::Shorter => (w, (h - step_h).max(min_h.max(60))),
        };
        floating.ask(window, size);
        self.retile();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 40,
        w: 1000,
        h: 600,
    };

    fn sizes(window: &&str) -> (i32, i32) {
        match *window {
            "big" => (1200, 800),
            _ => (200, 100),
        }
    }

    #[test]
    fn a_window_without_a_parent_opens_centred_in_the_area() {
        let mut floating = Floating::default();
        floating.add("splash", (200, 100), None, AREA);
        assert_eq!(
            floating.rects(AREA, &sizes),
            vec![(
                "splash",
                Rect {
                    x: 400,
                    y: 290,
                    w: 200,
                    h: 100
                }
            )]
        );
    }

    #[test]
    fn a_dialog_opens_centred_over_its_parent_and_stays_inside_the_area() {
        let mut floating = Floating::default();
        let parent = Rect {
            x: 500,
            y: 40,
            w: 500,
            h: 600,
        };
        floating.add("dialog", (200, 100), Some(parent), AREA);
        assert_eq!(
            floating.rects(AREA, &sizes)[0].1,
            Rect {
                x: 650,
                y: 290,
                w: 200,
                h: 100
            }
        );
        let near_the_edge = Rect {
            x: 900,
            y: 40,
            w: 100,
            h: 50,
        };
        floating.add("other", (200, 100), Some(near_the_edge), AREA);
        assert_eq!(
            floating.rects(AREA, &sizes)[1].1.x,
            800,
            "moved in from the right edge"
        );
    }

    #[test]
    fn a_window_bigger_than_the_area_keeps_its_top_left_corner_in_it() {
        let mut floating = Floating::default();
        floating.add("big", (1200, 800), None, AREA);
        let (_, rect) = floating.rects(AREA, &sizes)[0];
        assert_eq!((rect.x, rect.y), (AREA.x, AREA.y));
    }

    #[test]
    fn the_place_follows_the_area_when_it_changes_size() {
        let mut floating = Floating::default();
        floating.add_at("a", (500, 340), (200, 100), AREA);
        let wider = Rect {
            w: 2000,
            h: 1200,
            ..AREA
        };
        assert_eq!(floating.rects(wider, &sizes)[0].1.x, 1000);
    }

    #[test]
    fn raising_brings_a_windows_children_along_in_front() {
        let mut floating = Floating::default();
        for window in ["parent", "child", "other"] {
            floating.add(window, (200, 100), None, AREA);
        }
        floating.raise(&"parent", |child, parent| {
            *child == "child" && *parent == "parent"
        });
        assert_eq!(floating.windows(), vec!["other", "parent", "child"]);
    }

    #[test]
    fn focus_moves_to_the_nearest_centre_that_way() {
        let mut floating = Floating::default();
        floating.add_at("left", (0, 40), (200, 100), AREA);
        floating.add_at("middle", (400, 300), (200, 100), AREA);
        floating.add_at("far right", (800, 40), (200, 100), AREA);
        assert_eq!(
            floating.neighbour(&"middle", Direction::Left, AREA, &sizes),
            Some("left")
        );
        assert_eq!(
            floating.neighbour(&"left", Direction::Right, AREA, &sizes),
            Some("middle")
        );
        assert_eq!(
            floating.neighbour(&"far right", Direction::Right, AREA, &sizes),
            None
        );
    }

    #[test]
    fn the_keys_move_a_window_but_never_off_the_area() {
        let mut floating = Floating::default();
        floating.add_at("a", (10, 50), (200, 100), AREA);
        floating.move_by(&"a", (-MOVE_STEP, 0), (200, 100), AREA);
        assert_eq!(floating.rects(AREA, &sizes)[0].1.x, 0);
        floating.move_by(&"a", (MOVE_STEP, MOVE_STEP), (200, 100), AREA);
        assert_eq!(
            floating.rects(AREA, &sizes)[0].1,
            Rect {
                x: 50,
                y: 100,
                w: 200,
                h: 100
            }
        );
    }
}

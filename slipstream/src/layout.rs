//! Dwindle tiling: each new window splits the focused one along its longer side.
//!
//! Pure logic with no Wayland types, so it is unit-tested directly.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn inset(&self, by: i32) -> Rect {
        Rect {
            x: self.x + by,
            y: self.y + by,
            w: self.w.saturating_sub(by.saturating_mul(2)).max(1),
            h: self.h.saturating_sub(by.saturating_mul(2)).max(1),
        }
    }
}

/// Which way Super+[ and Super+] (with Shift for height) take the focused tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    Wider,
    Narrower,
    Taller,
    Shorter,
}

/// What a resize did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resized {
    /// The split moved to this ratio.
    Changed(f32),
    /// The tile is already as big or as small as it goes that way: the ratio is at its limit, or
    /// a neighbour's minimum size holds it.
    AtLimit,
    /// No split of that axis has the window on one side.
    NothingBeside,
}

/// How far a split may go towards either side.
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;
/// One press of a resize key, as a share of the split.
const RESIZE_STEP: f32 = 0.05;
/// A ratio this close to a third, a half or two thirds lands on it.
const SNAP: f32 = 0.02;

/// `ratio` within the limits, landing on ⅓, ½ or ⅔ when it's within `SNAP` of one.
pub fn snap_ratio(ratio: f32) -> f32 {
    let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
    [1.0 / 3.0, 0.5, 2.0 / 3.0]
        .into_iter()
        .find(|mark| (ratio - mark).abs() <= SNAP)
        .unwrap_or(ratio)
}

/// The gap between a split's two sides, for dragging with the pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    /// The gap itself: as wide as the inner gap, and as long as the split.
    pub strip: Rect,
    /// Whether the sides are side by side, so the strip is upright and moves left and right.
    pub vertical: bool,
    /// The split's whole rect, gaps included, which its ratio is a share of.
    pub split: Rect,
    /// The way down the tree to the split (`Dwindle::set_ratio`).
    pub path: Vec<bool>,
    /// The least each side can have along the axis, gaps included: its windows' minimum sizes.
    pub min: (i32, i32),
}

/// A divider's hit area is never narrower than this, in logical pixels.
const DIVIDER_HIT: i32 = 8;

impl Divider {
    /// Where the pointer can take hold of it: the strip, widened to `DIVIDER_HIT` about its
    /// middle when it's narrower.
    pub fn hit(&self) -> Rect {
        let mut hit = self.strip;
        if self.vertical && hit.w < DIVIDER_HIT {
            hit.x -= (DIVIDER_HIT - hit.w) / 2;
            hit.w = DIVIDER_HIT;
        } else if !self.vertical && hit.h < DIVIDER_HIT {
            hit.y -= (DIVIDER_HIT - hit.h) / 2;
            hit.h = DIVIDER_HIT;
        }
        hit
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let hit = self.hit();
        (hit.x as f64..(hit.x + hit.w) as f64).contains(&x)
            && (hit.y as f64..(hit.y + hit.h) as f64).contains(&y)
    }

    /// The ratio that puts the gap at the pointer's position along the axis (`at`): kept where
    /// both sides get their minimum and within the limits, and landing on ⅓, ½ or ⅔ when that
    /// is near and allowed.
    pub fn ratio_at(&self, at: f64) -> f32 {
        let (start, length) = if self.vertical {
            (self.split.x, self.split.w)
        } else {
            (self.split.y, self.split.h)
        };
        let length = length.max(1) as f32;
        let raw = ((at as f32) - start as f32) / length;
        let low = MIN_RATIO.max(self.min.0 as f32 / length);
        let high = MAX_RATIO.min(1.0 - self.min.1 as f32 / length);
        if low > high {
            // Both minimums can't fit: the limits alone, as the tiling does.
            return snap_ratio(raw);
        }
        let ratio = raw.clamp(low, high);
        let snapped = snap_ratio(ratio);
        if (low..=high).contains(&snapped) {
            snapped
        } else {
            ratio
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone)]
enum Node<T> {
    Leaf(T),
    /// `vertical`: `a` and `b` sit side by side; otherwise `a` is above `b`.
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Node<T>>,
        b: Box<Node<T>>,
    },
}

/// A tiling tree, copied out of `Dwindle` so it can be written down and read back. `Node` stays
/// private; this is the same shape with a public face.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape<T> {
    Leaf(T),
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Shape<T>>,
        b: Box<Shape<T>>,
    },
}

impl<T: Clone> Shape<T> {
    /// Every window in the tree, left/top first.
    pub fn leaves(&self) -> Vec<T> {
        match self {
            Shape::Leaf(id) => vec![id.clone()],
            Shape::Split { a, b, .. } => {
                let mut out = a.leaves();
                out.extend(b.leaves());
                out
            }
        }
    }
}

impl<T> Shape<T> {
    /// The same tree with every leaf translated, dropping the ones that come back `None`. A split
    /// left with one side collapses into it, as closing a window does — which is how a recorded
    /// window whose app never came back leaves no hole in the layout.
    pub fn map<U>(&self, leaf: &impl Fn(&T) -> Option<U>) -> Option<Shape<U>> {
        match self {
            Shape::Leaf(id) => leaf(id).map(Shape::Leaf),
            Shape::Split {
                vertical,
                ratio,
                a,
                b,
            } => match (a.map(leaf), b.map(leaf)) {
                (Some(a), Some(b)) => Some(Shape::Split {
                    vertical: *vertical,
                    ratio: *ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (some, None) | (None, some) => some,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dwindle<T> {
    root: Option<Node<T>>,
    /// Gap between windows and the edge of the output.
    pub outer_gap: i32,
    /// Gap between neighbouring windows.
    pub inner_gap: i32,
}

impl<T: Clone + PartialEq> Dwindle<T> {
    pub fn new(outer_gap: i32, inner_gap: i32) -> Self {
        Self {
            root: None,
            outer_gap,
            inner_gap,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn len(&self) -> usize {
        self.windows().len()
    }

    /// Windows in tree order (left/top first).
    pub fn windows(&self) -> Vec<T> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            Self::collect(
                root,
                Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                },
                &mut out,
            );
        }
        out.into_iter().map(|(t, _)| t).collect()
    }

    /// The tree as it stands: every split's axis and ratio, and the windows at the leaves, in
    /// tree order. `session.rs` writes this down so a layout can be rebuilt exactly, rather than
    /// replaying the inserts that built it — a tree that has been swapped or closed into no
    /// longer has an insert order to replay.
    pub fn shape(&self) -> Option<Shape<T>> {
        self.root.as_ref().map(Self::shape_of)
    }

    fn shape_of(node: &Node<T>) -> Shape<T> {
        match node {
            Node::Leaf(id) => Shape::Leaf(id.clone()),
            Node::Split {
                vertical,
                ratio,
                a,
                b,
            } => Shape::Split {
                vertical: *vertical,
                ratio: *ratio,
                a: Box::new(Self::shape_of(a)),
                b: Box::new(Self::shape_of(b)),
            },
        }
    }

    /// Puts a recorded tree back, splits and ratios and all. Replaces whatever was here, so the
    /// caller re-inserts any window the record didn't know about. Duplicate leaves are dropped,
    /// since a tree that named one window twice would make `remove` and `swap` ambiguous.
    pub fn rebuild(&mut self, shape: &Shape<T>) {
        let mut seen = Vec::new();
        self.root = Self::node_of(shape, &mut seen);
    }

    /// A shape as a tree, skipping leaves already used. A split with one live side collapses into
    /// it, which is how a window that never came back leaves no hole.
    fn node_of(shape: &Shape<T>, seen: &mut Vec<T>) -> Option<Node<T>> {
        match shape {
            Shape::Leaf(id) => {
                if seen.contains(id) {
                    return None;
                }
                seen.push(id.clone());
                Some(Node::Leaf(id.clone()))
            }
            Shape::Split {
                vertical,
                ratio,
                a,
                b,
            } => match (Self::node_of(a, seen), Self::node_of(b, seen)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    vertical: *vertical,
                    ratio: ratio.clamp(0.1, 0.9),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (only, None) | (None, only) => only,
            },
        }
    }

    /// Adds `id` by splitting `beside` (or the last window) along its longer side in `area`.
    pub fn insert(&mut self, id: T, beside: Option<&T>, area: Rect) {
        let Some(root) = &mut self.root else {
            self.root = Some(Node::Leaf(id));
            return;
        };
        let mut raw = Vec::new();
        Self::collect(root, area, &mut raw);
        let target = beside
            .and_then(|b| raw.iter().find(|(t, _)| t == b))
            .or(raw.last())
            .cloned();
        if let Some((target, rect)) = target {
            Self::split_leaf(root, &target, rect.w >= rect.h, &id);
        }
    }

    /// Swaps two windows' places in the tree, leaving every split and ratio alone, so the
    /// tiling keeps its shape and only those two windows change seats. Both must be here:
    /// otherwise a swap would quietly rename one window into another.
    pub fn swap(&mut self, a: &T, b: &T) -> bool {
        if a == b {
            return false;
        }
        let here = self.windows();
        if !here.contains(a) || !here.contains(b) {
            return false;
        }
        let Some(root) = &mut self.root else {
            return false;
        };
        Self::relabel(root, a, b);
        true
    }

    fn relabel(node: &mut Node<T>, a: &T, b: &T) {
        match node {
            // Each leaf is looked at once, so the one just given `b` isn't swapped back.
            Node::Leaf(t) => {
                if t == a {
                    *t = b.clone();
                } else if t == b {
                    *t = a.clone();
                }
            }
            Node::Split { a: x, b: y, .. } => {
                Self::relabel(x, a, b);
                Self::relabel(y, a, b);
            }
        }
    }

    /// Super+[ ] and Super+Shift+[ ]: moves the nearest split above `id` whose sides lie along
    /// that axis (side by side for wider and narrower, stacked for taller and shorter) one step
    /// towards the side that grows. When `id`'s tile in `area`, with minimum sizes, comes out the
    /// same, the ratio is put back and the tile is at its limit.
    pub fn resize(
        &mut self,
        id: &T,
        how: Resize,
        area: Rect,
        min: &dyn Fn(&T) -> (i32, i32),
    ) -> Resized {
        let side_by_side = matches!(how, Resize::Wider | Resize::Narrower);
        let grow = matches!(how, Resize::Wider | Resize::Taller);
        let Some(path) = self.path_to(id) else {
            return Resized::NothingBeside;
        };
        // The nearest ancestor of that axis: walk up from the leaf.
        let Some((depth, in_a)) = (0..path.len()).rev().find_map(|depth| {
            let vertical = self.split_at(&path[..depth])?.0;
            (vertical == side_by_side).then_some((depth, !path[depth]))
        }) else {
            return Resized::NothingBeside;
        };
        let split = &path[..depth];
        let Some((_, old)) = self.split_at(split) else {
            return Resized::NothingBeside;
        };
        // The first side grows as the ratio goes up.
        let step = if grow == in_a {
            RESIZE_STEP
        } else {
            -RESIZE_STEP
        };
        let new = snap_ratio(old + step);
        if new == old {
            return Resized::AtLimit;
        }
        let tile = |layout: &Self| {
            layout
                .rects_within(area, min)
                .into_iter()
                .find(|(t, _)| t == id)
                .map(|(_, rect)| rect)
        };
        let before = tile(self);
        self.set_ratio(split, new);
        if tile(self) == before {
            self.set_ratio(split, old);
            return Resized::AtLimit;
        }
        Resized::Changed(new)
    }

    /// Every split's gap in `area`, laid out as `rects_within` lays the windows out.
    pub fn dividers(&self, area: Rect, min: &dyn Fn(&T) -> (i32, i32)) -> Vec<Divider> {
        let half = self.inner_gap / 2;
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            let min = |t: &T| {
                let (w, h) = min(t);
                (
                    if w > 0 { w.saturating_add(2 * half) } else { 0 },
                    if h > 0 { h.saturating_add(2 * half) } else { 0 },
                )
            };
            let mut path = Vec::new();
            Self::dividers_within(
                root,
                area.inset(self.outer_gap - half),
                half,
                &min,
                &mut path,
                &mut out,
            );
        }
        out
    }

    fn dividers_within(
        node: &Node<T>,
        r: Rect,
        half: i32,
        min: &dyn Fn(&T) -> (i32, i32),
        path: &mut Vec<bool>,
        out: &mut Vec<Divider>,
    ) {
        let Node::Split {
            vertical,
            ratio,
            a,
            b,
        } = node
        else {
            return;
        };
        let (min_a, min_b) = (Self::min_of(a, min), Self::min_of(b, min));
        let (ra, rb, strip, along) = if *vertical {
            let wa = give_room(r.w, *ratio, min_a.0, min_b.0);
            (
                Rect { w: wa, ..r },
                Rect {
                    x: r.x + wa,
                    w: r.w - wa,
                    ..r
                },
                Rect {
                    x: r.x + wa - half,
                    y: r.y + half,
                    w: 2 * half,
                    h: r.h - 2 * half,
                },
                (min_a.0, min_b.0),
            )
        } else {
            let ha = give_room(r.h, *ratio, min_a.1, min_b.1);
            (
                Rect { h: ha, ..r },
                Rect {
                    y: r.y + ha,
                    h: r.h - ha,
                    ..r
                },
                Rect {
                    x: r.x + half,
                    y: r.y + ha - half,
                    w: r.w - 2 * half,
                    h: 2 * half,
                },
                (min_a.1, min_b.1),
            )
        };
        out.push(Divider {
            strip,
            vertical: *vertical,
            split: r,
            path: path.clone(),
            min: along,
        });
        for (second, child, rect) in [(false, a, ra), (true, b, rb)] {
            path.push(second);
            Self::dividers_within(child, rect, half, min, path, out);
            path.pop();
        }
    }

    /// The ratio of the split at `path`, if there is one.
    pub fn ratio_of(&self, path: &[bool]) -> Option<f32> {
        self.split_at(path).map(|(_, ratio)| ratio)
    }

    /// The way down to `id`'s leaf: at each split, `false` for its first side and `true` for its
    /// second.
    fn path_to(&self, id: &T) -> Option<Vec<bool>> {
        fn walk<T: PartialEq>(node: &Node<T>, id: &T, path: &mut Vec<bool>) -> bool {
            match node {
                Node::Leaf(t) => t == id,
                Node::Split { a, b, .. } => {
                    for (second, child) in [(false, a), (true, b)] {
                        path.push(second);
                        if walk(child, id, path) {
                            return true;
                        }
                        path.pop();
                    }
                    false
                }
            }
        }
        let mut path = Vec::new();
        walk(self.root.as_ref()?, id, &mut path).then_some(path)
    }

    fn node_at(&self, path: &[bool]) -> Option<&Node<T>> {
        let mut node = self.root.as_ref()?;
        for &second in path {
            match node {
                Node::Split { a, b, .. } => node = if second { b } else { a },
                Node::Leaf(_) => return None,
            }
        }
        Some(node)
    }

    /// The split at `path`: whether its sides are side by side, and its ratio.
    fn split_at(&self, path: &[bool]) -> Option<(bool, f32)> {
        match self.node_at(path)? {
            Node::Split {
                vertical, ratio, ..
            } => Some((*vertical, *ratio)),
            Node::Leaf(_) => None,
        }
    }

    /// Sets the ratio of the split at `path`, within the limits. Whether there was one.
    pub fn set_ratio(&mut self, path: &[bool], to: f32) -> bool {
        let mut node = match self.root.as_mut() {
            Some(root) => root,
            None => return false,
        };
        for &second in path {
            match node {
                Node::Split { a, b, .. } => node = if second { b } else { a },
                Node::Leaf(_) => return false,
            }
        }
        match node {
            Node::Split { ratio, .. } => {
                *ratio = to.clamp(MIN_RATIO, MAX_RATIO);
                true
            }
            Node::Leaf(_) => false,
        }
    }

    pub fn remove(&mut self, id: &T) -> bool {
        let Some(root) = self.root.take() else {
            return false;
        };
        let (root, removed) = Self::remove_from(root, id);
        self.root = root;
        removed
    }

    /// Where each window goes inside `area`, gaps included.
    pub fn rects(&self, area: Rect) -> Vec<(T, Rect)> {
        self.rects_within(area, &|_| (0, 0))
    }

    /// As `rects`, but a split moves off its ratio to give a window its minimum size (width,
    /// height; 0 for none), as long as both sides still get theirs. A client with a minimum
    /// width wider than half the screen would otherwise overhang its neighbour.
    pub fn rects_within(&self, area: Rect, min: &dyn Fn(&T) -> (i32, i32)) -> Vec<(T, Rect)> {
        let half = self.inner_gap / 2;
        let mut raw = Vec::new();
        if let Some(root) = &self.root {
            // Minimums are of the window; the tiles here are measured before their gaps come off.
            // Saturating throughout: a minimum is whatever the client sent.
            let min = |t: &T| {
                let (w, h) = min(t);
                (
                    if w > 0 { w.saturating_add(2 * half) } else { 0 },
                    if h > 0 { h.saturating_add(2 * half) } else { 0 },
                )
            };
            Self::collect_within(root, area.inset(self.outer_gap - half), &min, &mut raw);
        }
        raw.into_iter().map(|(t, r)| (t, r.inset(half))).collect()
    }

    fn collect(node: &Node<T>, r: Rect, out: &mut Vec<(T, Rect)>) {
        Self::collect_within(node, r, &|_| (0, 0), out);
    }

    fn collect_within(
        node: &Node<T>,
        r: Rect,
        min: &dyn Fn(&T) -> (i32, i32),
        out: &mut Vec<(T, Rect)>,
    ) {
        match node {
            Node::Leaf(t) => out.push((t.clone(), r)),
            Node::Split {
                vertical,
                ratio,
                a,
                b,
            } => {
                let (min_a, min_b) = (Self::min_of(a, min), Self::min_of(b, min));
                if *vertical {
                    let wa = give_room(r.w, *ratio, min_a.0, min_b.0);
                    Self::collect_within(a, Rect { w: wa, ..r }, min, out);
                    Self::collect_within(
                        b,
                        Rect {
                            x: r.x + wa,
                            w: r.w - wa,
                            ..r
                        },
                        min,
                        out,
                    );
                } else {
                    let ha = give_room(r.h, *ratio, min_a.1, min_b.1);
                    Self::collect_within(a, Rect { h: ha, ..r }, min, out);
                    Self::collect_within(
                        b,
                        Rect {
                            y: r.y + ha,
                            h: r.h - ha,
                            ..r
                        },
                        min,
                        out,
                    );
                }
            }
        }
    }

    /// The smallest a subtree can go: side by side, the widths add up and the tallest decides;
    /// stacked, the other way round.
    fn min_of(node: &Node<T>, min: &dyn Fn(&T) -> (i32, i32)) -> (i32, i32) {
        match node {
            Node::Leaf(t) => min(t),
            Node::Split { vertical, a, b, .. } => {
                let (a, b) = (Self::min_of(a, min), Self::min_of(b, min));
                if *vertical {
                    (a.0.saturating_add(b.0), a.1.max(b.1))
                } else {
                    (a.0.max(b.0), a.1.saturating_add(b.1))
                }
            }
        }
    }

    fn split_leaf(node: &mut Node<T>, target: &T, vertical: bool, id: &T) -> bool {
        match node {
            Node::Leaf(t) if t == target => {
                let old = Node::Leaf(t.clone());
                *node = Node::Split {
                    vertical,
                    ratio: 0.5,
                    a: Box::new(old),
                    b: Box::new(Node::Leaf(id.clone())),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                Self::split_leaf(a, target, vertical, id)
                    || Self::split_leaf(b, target, vertical, id)
            }
        }
    }

    fn remove_from(node: Node<T>, id: &T) -> (Option<Node<T>>, bool) {
        match node {
            Node::Leaf(t) if &t == id => (None, true),
            leaf @ Node::Leaf(_) => (Some(leaf), false),
            Node::Split {
                vertical,
                ratio,
                a,
                b,
            } => {
                let (a, removed_a) = Self::remove_from(*a, id);
                let (b, removed_b) = if removed_a {
                    (Some(*b), false)
                } else {
                    Self::remove_from(*b, id)
                };
                let node = match (a, b) {
                    (Some(a), Some(b)) => Some(Node::Split {
                        vertical,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                };
                (node, removed_a || removed_b)
            }
        }
    }
}

/// The window you reach by pressing `direction` from `from`.
///
/// Only windows entirely past that edge count. Among them, windows that line up with
/// `from` (overlap on the other axis) beat ones that don't, then the nearest wins, then
/// reading order (top, then left) breaks ties. Comparing centres instead lets a
/// one-pixel rounding difference pick the wrong window.
/// How much of `length` the first side of a split gets: its share by `ratio`, moved just far
/// enough that each side gets its minimum. When both minimums can't fit, the ratio stands and the
/// windows are drawn scaled into their tiles instead.
fn give_room(length: i32, ratio: f32, min_a: i32, min_b: i32) -> i32 {
    let share = (length as f32 * ratio).round() as i32;
    if min_a.saturating_add(min_b) > length {
        share
    } else {
        share.clamp(min_a, length - min_b)
    }
}

pub fn neighbour<T: Clone + PartialEq>(
    rects: &[(T, Rect)],
    from: &T,
    direction: Direction,
) -> Option<T> {
    /// Distance between two ranges on one axis; 0 when they overlap.
    fn gap(a: i32, a_len: i32, b: i32, b_len: i32) -> i32 {
        (b - (a + a_len)).max(a - (b + b_len)).max(0)
    }
    let (_, o) = rects.iter().find(|(t, _)| t == from)?;
    rects
        .iter()
        .filter(|(t, _)| t != from)
        .filter_map(|(t, r)| {
            let (along, off_axis, order) = match direction {
                Direction::Left => (o.x - (r.x + r.w), gap(o.y, o.h, r.y, r.h), r.y),
                Direction::Right => (r.x - (o.x + o.w), gap(o.y, o.h, r.y, r.h), r.y),
                Direction::Up => (o.y - (r.y + r.h), gap(o.x, o.w, r.x, r.w), r.x),
                Direction::Down => (r.y - (o.y + o.h), gap(o.x, o.w, r.x, r.w), r.x),
            };
            (along >= 0).then_some((t, (off_axis, along, order)))
        })
        .min_by_key(|(_, key)| *key)
        .map(|(t, _)| t.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1000,
        h: 600,
    };

    fn rect_of(layout: &Dwindle<&'static str>, id: &str) -> Rect {
        layout
            .rects(AREA)
            .into_iter()
            .find(|(t, _)| *t == id)
            .unwrap()
            .1
    }

    #[test]
    fn one_window_fills_the_area_inside_the_outer_gap() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        assert_eq!(
            rect_of(&l, "a"),
            Rect {
                x: 16,
                y: 16,
                w: 968,
                h: 568
            }
        );
    }

    #[test]
    fn second_window_goes_beside_the_first_on_a_wide_output() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        let (a, b) = (rect_of(&l, "a"), rect_of(&l, "b"));
        assert_eq!(a.y, b.y);
        assert_eq!(b.x - (a.x + a.w), 10, "inner gap between neighbours");
        assert_eq!(a.x, 16, "outer gap on the left");
        assert_eq!(b.x + b.w, 1000 - 16, "outer gap on the right");
    }

    #[test]
    fn a_window_with_a_minimum_width_gets_it_from_its_neighbour() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        // A chat app that won't go narrower than 600, beside one that goes to anything.
        let min = |t: &&str| if *t == "a" { (600, 400) } else { (0, 0) };
        let rects = l.rects_within(AREA, &min);
        let (a, b) = (rects[0].1, rects[1].1);
        assert_eq!(a.w, 600, "just its minimum, no more");
        assert_eq!(b.x - (a.x + a.w), 10, "the gap stays");
        assert_eq!(b.x + b.w, 1000 - 16);

        // On the right it takes the room from the left.
        l.swap(&"a", &"b");
        let rects = l.rects_within(AREA, &min);
        assert_eq!(rects[1].1.w, 600);
        assert_eq!(rects[1].1.x + 600, 1000 - 16);

        // A minimum the tile already meets changes nothing.
        let small = |_: &&str| (300, 200);
        assert_eq!(l.rects_within(AREA, &small), l.rects(AREA));

        // Two that can't both fit keep the ratio, to be scaled when drawn.
        let greedy = |_: &&str| (700, 0);
        assert_eq!(l.rects_within(AREA, &greedy), l.rects(AREA));
    }

    #[test]
    fn a_minimum_as_big_as_an_integer_goes_neither_past_the_area_nor_bang() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        l.insert("c", Some(&"b"), AREA);
        let huge = |t: &&str| {
            if *t == "c" {
                (0, 0)
            } else {
                (i32::MAX, i32::MAX)
            }
        };
        let rects = l.rects_within(AREA, &huge);
        assert_eq!(rects.len(), 3);
        assert_eq!(
            rects,
            l.rects(AREA),
            "minimums that can't fit leave the ratios alone"
        );
    }

    #[test]
    fn third_window_splits_the_tall_half_top_and_bottom() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        l.insert("c", Some(&"b"), AREA);
        let (b, c) = (rect_of(&l, "b"), rect_of(&l, "c"));
        assert_eq!(b.x, c.x);
        assert!(c.y > b.y);
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn removing_a_window_gives_its_space_back() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        assert!(l.remove(&"b"));
        assert_eq!(l.windows(), vec!["a"]);
        assert_eq!(rect_of(&l, "a").w, 968);
        assert!(!l.remove(&"missing"));
        assert!(l.remove(&"a"));
        assert!(l.is_empty());
    }

    #[test]
    fn swapping_two_windows_keeps_the_shape_of_the_tiling() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        l.insert("c", Some(&"b"), AREA);
        let before = l.rects(AREA);
        assert!(l.swap(&"a", &"c"));
        let after = l.rects(AREA);
        for ((_, was), (_, now)) in before.iter().zip(after.iter()) {
            assert_eq!(was, now, "the tiles themselves don't move");
        }
        assert_eq!(rect_of(&l, "c"), before[0].1, "c took a's tile");
        assert_eq!(rect_of(&l, "a"), before[2].1, "a took c's tile");
        assert_eq!(rect_of(&l, "b"), before[1].1, "b stayed put");
    }

    #[test]
    fn swapping_needs_two_different_windows_that_are_both_here() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        assert!(!l.swap(&"a", &"a"), "a window can't swap with itself");
        assert!(!l.swap(&"a", &"gone"), "and not with one that isn't here");
        assert_eq!(l.windows(), vec!["a", "b"]);
    }

    #[test]
    fn neighbour_follows_the_arrow_keys() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", Some(&"a"), AREA);
        l.insert("c", Some(&"b"), AREA);
        let rects = l.rects(AREA);
        assert_eq!(neighbour(&rects, &"a", Direction::Right), Some("b"));
        assert_eq!(neighbour(&rects, &"b", Direction::Down), Some("c"));
        assert_eq!(neighbour(&rects, &"c", Direction::Up), Some("b"));
        assert_eq!(neighbour(&rects, &"c", Direction::Left), Some("a"));
        assert_eq!(neighbour(&rects, &"a", Direction::Left), None);
    }

    #[test]
    fn the_shape_is_the_tree_the_inserts_built() {
        let mut l = Dwindle::new(16, 10);
        assert_eq!(l.shape(), None);
        l.insert("a", None, AREA);
        assert_eq!(l.shape(), Some(Shape::Leaf("a")));
        // Wider than it is tall, so the first split is a vertical one, side by side.
        l.insert("b", None, AREA);
        let Some(Shape::Split {
            vertical,
            ratio,
            a,
            b,
        }) = l.shape()
        else {
            panic!("two windows should be a split");
        };
        assert!(vertical);
        assert_eq!(ratio, 0.5);
        assert_eq!(*a, Shape::Leaf("a"));
        assert_eq!(*b, Shape::Leaf("b"));
    }

    #[test]
    fn a_recorded_tree_comes_back_with_its_splits_and_ratios() {
        let mut layout = Dwindle::new(16, 10);
        let shape = Shape::Split {
            vertical: true,
            ratio: 0.6,
            a: Box::new(Shape::Leaf("a")),
            b: Box::new(Shape::Split {
                vertical: false,
                ratio: 0.25,
                a: Box::new(Shape::Leaf("b")),
                b: Box::new(Shape::Leaf("c")),
            }),
        };
        layout.rebuild(&shape);
        assert_eq!(layout.shape().unwrap(), shape);
        assert_eq!(layout.windows(), ["a", "b", "c"]);
        assert_eq!(shape.leaves(), ["a", "b", "c"]);
        // The recorded ratio is honoured, not the even split a fresh insert would have made:
        // 1000 usable pixels at 0.6, less half the inner gap.
        let area = Rect {
            x: 0,
            y: 0,
            w: 1032,
            h: 616,
        };
        let rects = layout.rects(area);
        let a = rects.iter().find(|(id, _)| *id == "a").unwrap().1;
        assert_eq!(a.w, 596);
    }

    #[test]
    fn rebuilding_drops_a_window_named_twice() {
        let mut layout = Dwindle::new(16, 10);
        layout.rebuild(&Shape::Split {
            vertical: true,
            ratio: 0.5,
            a: Box::new(Shape::Leaf("a")),
            b: Box::new(Shape::Leaf("a")),
        });
        assert_eq!(layout.windows(), ["a"]);
    }

    #[test]
    fn a_shape_maps_onto_new_leaves_and_closes_up_around_the_missing() {
        let shape = Shape::Split {
            vertical: true,
            ratio: 0.5,
            a: Box::new(Shape::Leaf(0usize)),
            b: Box::new(Shape::Split {
                vertical: false,
                ratio: 0.5,
                a: Box::new(Shape::Leaf(1)),
                b: Box::new(Shape::Leaf(2)),
            }),
        };
        let kept = shape.map(&|slot: &usize| (*slot != 1).then(|| format!("w{slot}")));
        assert_eq!(
            kept.unwrap(),
            Shape::Split {
                vertical: true,
                ratio: 0.5,
                a: Box::new(Shape::Leaf("w0".to_string())),
                b: Box::new(Shape::Leaf("w2".to_string())),
            }
        );
        // Nothing left means no tree at all, rather than an empty one.
        assert_eq!(shape.map(&|_: &usize| None::<usize>), None);
    }

    fn ratio_of_root(l: &Dwindle<&'static str>) -> f32 {
        match l.shape() {
            Some(Shape::Split { ratio, .. }) => ratio,
            _ => panic!("a split at the root"),
        }
    }

    fn three() -> Dwindle<&'static str> {
        // a on the left; b above c on the right.
        let mut l = Dwindle::new(16, 10);
        for id in ["a", "b", "c"] {
            l.insert(id, None, AREA);
        }
        l
    }

    const NO_MIN: &dyn Fn(&&'static str) -> (i32, i32) = &|_| (0, 0);

    #[test]
    fn resize_moves_the_nearest_split_of_that_axis() {
        let mut l = three();
        // c is in the stacked split, under b: taller moves that split, not the root.
        assert_eq!(
            l.resize(&"c", Resize::Taller, AREA, NO_MIN),
            Resized::Changed(0.45)
        );
        let Some(Shape::Split { ratio, b, .. }) = l.shape() else {
            panic!("a split at the root");
        };
        assert_eq!(ratio, 0.5, "the side-by-side split is left alone");
        let Shape::Split {
            vertical, ratio, ..
        } = *b
        else {
            panic!("b and c are a split");
        };
        assert!(!vertical);
        assert_eq!(ratio, 0.45, "c, the second side, grows as the ratio falls");
        // Wider from c goes past its own split to the side-by-side one above, where c is on the
        // second side.
        assert_eq!(
            l.resize(&"c", Resize::Wider, AREA, NO_MIN),
            Resized::Changed(0.45)
        );
        assert_eq!(ratio_of_root(&l), 0.45);
        assert!(rect_of(&l, "c").w > rect_of(&l, "a").w);
        // From a, the first side, wider raises it.
        assert_eq!(
            l.resize(&"a", Resize::Wider, AREA, NO_MIN),
            Resized::Changed(0.5)
        );
    }

    #[test]
    fn snaps_to_thirds_and_half() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", None, AREA);
        let mut wider = || l.resize(&"a", Resize::Wider, AREA, NO_MIN);
        assert_eq!(wider(), Resized::Changed(0.55));
        assert_eq!(wider(), Resized::Changed(0.6));
        assert_eq!(
            wider(),
            Resized::Changed(2.0 / 3.0),
            "0.65 is near two thirds"
        );
        assert_eq!(snap_ratio(0.35), 1.0 / 3.0);
        assert_eq!(snap_ratio(0.51), 0.5);
        assert_eq!(snap_ratio(0.45), 0.45);
    }

    #[test]
    fn clamps_and_reports_the_limit() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", None, AREA);
        let mut last = Resized::NothingBeside;
        for _ in 0..20 {
            last = l.resize(&"a", Resize::Narrower, AREA, NO_MIN);
            if last == Resized::AtLimit {
                break;
            }
        }
        assert_eq!(last, Resized::AtLimit);
        assert_eq!(ratio_of_root(&l), MIN_RATIO);
        assert_eq!(
            l.resize(&"b", Resize::Wider, AREA, NO_MIN),
            Resized::AtLimit,
            "b is as wide as a's limit allows"
        );
        assert_eq!(
            l.resize(&"b", Resize::Narrower, AREA, NO_MIN),
            Resized::Changed(0.15)
        );
    }

    #[test]
    fn a_lone_window_has_nothing_to_resize() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        assert_eq!(
            l.resize(&"a", Resize::Wider, AREA, NO_MIN),
            Resized::NothingBeside
        );
        l.insert("b", None, AREA);
        assert_eq!(
            l.resize(&"a", Resize::Taller, AREA, NO_MIN),
            Resized::NothingBeside,
            "side by side, there's no stacked split"
        );
        assert_eq!(
            l.resize(&"gone", Resize::Wider, AREA, NO_MIN),
            Resized::NothingBeside
        );
    }

    #[test]
    fn a_neighbours_minimum_holds_it() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", None, AREA);
        // b won't go narrower than half the area, so a can't grow into it.
        let min = |t: &&str| if *t == "b" { (480, 0) } else { (0, 0) };
        assert_eq!(l.resize(&"a", Resize::Wider, AREA, &min), Resized::AtLimit);
        assert_eq!(ratio_of_root(&l), 0.5, "the ratio is put back");
    }

    #[test]
    fn dividers_lie_between_the_tiles() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", None, AREA);
        let dividers = l.dividers(AREA, NO_MIN);
        assert_eq!(dividers.len(), 1);
        let (a, b) = (rect_of(&l, "a"), rect_of(&l, "b"));
        let gap = &dividers[0];
        assert!(gap.vertical);
        assert_eq!(gap.path, Vec::<bool>::new());
        assert_eq!((gap.strip.x, gap.strip.w), (a.x + a.w, b.x - (a.x + a.w)));
        assert_eq!(
            (gap.strip.y, gap.strip.h),
            (a.y, a.h),
            "the split's full length"
        );
        assert!(gap.contains((a.x + a.w) as f64 + 5.0, 300.0));
        assert!(
            !gap.contains((a.x + a.w) as f64 - 5.0, 300.0),
            "not over a window"
        );

        // Three: the side-by-side gap, then the stacked one on the right.
        l.insert("c", Some(&"b"), AREA);
        let dividers = l.dividers(AREA, NO_MIN);
        assert_eq!(dividers.len(), 2);
        let (b, c) = (rect_of(&l, "b"), rect_of(&l, "c"));
        let stacked = &dividers[1];
        assert!(!stacked.vertical);
        assert_eq!(stacked.path, [true]);
        assert_eq!(
            (stacked.strip.y, stacked.strip.h),
            (b.y + b.h, c.y - (b.y + b.h))
        );
        assert_eq!((stacked.strip.x, stacked.strip.w), (b.x, b.w));
        // A narrow gap is widened to be caught.
        let mut thin = Dwindle::<&str>::new(16, 2);
        thin.insert("a", None, AREA);
        thin.insert("b", None, AREA);
        let hit = thin.dividers(AREA, NO_MIN)[0].hit();
        assert_eq!(hit.w, 8);
        // Dragging the gap to where the pointer is sets that ratio.
        let middle = gap_ratio_middle(&l);
        assert!((middle - 0.5).abs() < 0.01);
    }

    fn gap_ratio_middle(l: &Dwindle<&'static str>) -> f32 {
        let gap = &l.dividers(AREA, NO_MIN)[0];
        gap.ratio_at((gap.strip.x + gap.strip.w / 2) as f64)
    }

    #[test]
    fn a_drag_ratio_respects_minimums() {
        let mut l = Dwindle::new(16, 10);
        l.insert("a", None, AREA);
        l.insert("b", None, AREA);
        // b needs 590 of the 978 across; a can go down to the limit.
        let min = |t: &&str| if *t == "b" { (580, 0) } else { (0, 0) };
        let gap = &l.dividers(AREA, &min)[0];
        assert_eq!(gap.min, (0, 590));
        let right_edge = (gap.split.x + gap.split.w) as f64;
        let most = 1.0 - 590.0 / 978.0;
        assert!(
            (gap.ratio_at(right_edge) - most).abs() < 1e-4,
            "stops at b's minimum"
        );
        assert_eq!(
            gap.ratio_at(gap.split.x as f64),
            MIN_RATIO,
            "and at the limit"
        );
        l.set_ratio(&gap.path, gap.ratio_at(right_edge));
        let b = rect_of_within(&l, "b", &min);
        assert!(b.w >= 580, "b keeps its minimum: {}", b.w);
    }

    fn rect_of_within(
        layout: &Dwindle<&'static str>,
        id: &str,
        min: &dyn Fn(&&'static str) -> (i32, i32),
    ) -> Rect {
        layout
            .rects_within(AREA, min)
            .into_iter()
            .find(|(t, _)| *t == id)
            .unwrap()
            .1
    }

    #[test]
    fn closing_a_window_leaves_the_shape_of_what_is_left() {
        let mut l = Dwindle::new(16, 10);
        for id in ["a", "b", "c"] {
            l.insert(id, None, AREA);
        }
        l.remove(&"b");
        let Some(Shape::Split { a, b, .. }) = l.shape() else {
            panic!("two windows should be a split");
        };
        assert_eq!(*a, Shape::Leaf("a"));
        assert_eq!(*b, Shape::Leaf("c"));
    }
}

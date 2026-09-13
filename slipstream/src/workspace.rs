//! The workspaces: as many as the settings list, each with an id the list is followed by and a
//! name or none. Each keeps its own tiling layout and gravity weights, and remembers which
//! window had focus, so coming back to a workspace puts you where you left off.
//!
//! Which workspace is being looked at is not kept here: with more than one screen there is one
//! answer per screen, and `screen.rs` holds them. A workspace doesn't know or care where it is
//! drawn, which is what lets a screen go out without a single window moving.
//!
//! Generic over the window type so it's unit-tested without Wayland.

use crate::{
    gravity::Gravity,
    layout::{Dwindle, Rect},
};

#[derive(Debug)]
pub struct Workspace<T> {
    /// The settings file's id for it, which survives renaming and reordering.
    pub id: u32,
    /// Empty when it goes by its number.
    pub name: String,
    /// Which windows are here, and where they tile.
    pub layout: Dwindle<T>,
    /// Where they go instead while gravity is on.
    pub gravity: Gravity<T>,
    /// The window to focus when you come back to this workspace.
    pub last_focus: Option<T>,
    /// The window filling the tiling area above its neighbours (Super+F), which keep their tiles
    /// underneath. It lasts until something else on the workspace is wanted, so it isn't
    /// written into the session record.
    pub maximised: Option<T>,
}

impl<T: Clone + PartialEq> Workspace<T> {
    /// Where each window goes in `area`: by gravity when it's on, otherwise tiled.
    pub fn rects(&mut self, area: Rect) -> Vec<(T, Rect)> {
        self.rects_within(area, &|_| (0, 0))
    }

    /// As `rects`, with tiling making room for each window's minimum size (`Dwindle::rects_within`),
    /// and a maximised window given the whole area inside the outer gap.
    pub fn rects_within(&mut self, area: Rect, min: &dyn Fn(&T) -> (i32, i32)) -> Vec<(T, Rect)> {
        let mut rects = self.tiles_within(area, min);
        if let (Some(maximised), false) = (&self.maximised, self.gravity.is_on()) {
            let whole = area.inset(self.layout.outer_gap);
            for (window, rect) in rects.iter_mut() {
                if window == maximised {
                    *rect = whole;
                }
            }
        }
        rects
    }

    /// Super+F on `window`: it fills the area, or goes back to its tile. Whether it's maximised
    /// now.
    pub fn toggle_maximised(&mut self, window: &T) -> bool {
        if self.maximised.as_ref() == Some(window) {
            self.maximised = None;
            false
        } else if self.layout.windows().contains(window) {
            self.maximised = Some(window.clone());
            true
        } else {
            false
        }
    }

    /// Where each window's own tile is, ignoring a maximise: what the focus and move keys look
    /// for neighbours in.
    pub fn tiles_within(&mut self, area: Rect, min: &dyn Fn(&T) -> (i32, i32)) -> Vec<(T, Rect)> {
        if self.gravity.is_on() {
            let windows = self.layout.windows();
            let placed =
                self.gravity
                    .rects(&windows, area, self.layout.outer_gap, self.layout.inner_gap);
            if !placed.is_empty() {
                return placed;
            }
        }
        self.layout.rects_within(area, min)
    }
}

/// Where a removed window was, and the window that took over as gravity's centre, if any.
#[derive(Debug, PartialEq)]
pub struct Removed<T> {
    pub workspace: usize,
    pub new_centre: Option<T>,
}

#[derive(Debug)]
pub struct Workspaces<T> {
    list: Vec<Workspace<T>>,
    outer_gap: i32,
    inner_gap: i32,
}

impl<T: Clone + PartialEq> Workspaces<T> {
    /// One workspace per entry: an id and a name.
    pub fn new(entries: &[(u32, String)], outer_gap: i32, inner_gap: i32) -> Self {
        let mut workspaces = Self {
            list: Vec::new(),
            outer_gap,
            inner_gap,
        };
        workspaces.list = entries
            .iter()
            .map(|(id, name)| workspaces.fresh(*id, name.clone()))
            .collect();
        if workspaces.list.is_empty() {
            workspaces.list.push(workspaces.fresh(1, String::new()));
        }
        workspaces
    }

    fn fresh(&self, id: u32, name: String) -> Workspace<T> {
        Workspace {
            id,
            name,
            layout: Dwindle::new(self.outer_gap, self.inner_gap),
            gravity: Gravity::default(),
            last_focus: None,
            maximised: None,
        }
    }

    pub fn count(&self) -> usize {
        self.list.len()
    }

    /// What workspace `index` is called on screen: its name, or its number.
    pub fn label(&self, index: usize) -> String {
        slipstream_config::workspace_label(
            self.list.get(index).map_or("", |workspace| &workspace.name),
            index,
        )
    }

    /// The workspace with a name, by that name.
    pub fn named(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        (!name.is_empty())
            .then(|| {
                self.list
                    .iter()
                    .position(|workspace| workspace.name == name)
            })
            .flatten()
    }

    /// Makes the list the settings' list, following each workspace by its id: a renamed or moved
    /// workspace keeps its windows, a new one starts empty, and a deleted one's windows go to the
    /// nearest workspace before it that is still there (or the first). At least `min` workspaces
    /// are kept, unnamed ones added at the end if the list is shorter, so every screen can show
    /// one of its own.
    ///
    /// Returns where each old index went, for anything that remembers a workspace by index.
    pub fn reconcile(&mut self, entries: &[(u32, String)], min: usize, area: Rect) -> Vec<usize> {
        let old_ids: Vec<u32> = self.list.iter().map(|w| w.id).collect();
        let mut old: Vec<Option<Workspace<T>>> = std::mem::take(&mut self.list)
            .into_iter()
            .map(Some)
            .collect();
        let mut new: Vec<Workspace<T>> = Vec::new();
        for (id, name) in entries {
            let existing = old
                .iter_mut()
                .find(|slot| slot.as_ref().is_some_and(|w| w.id == *id))
                .and_then(Option::take);
            let mut workspace = existing.unwrap_or_else(|| self.fresh(*id, String::new()));
            workspace.name = name.clone();
            new.push(workspace);
        }
        while new.len() < min.max(1) {
            let taken: Vec<u32> = new.iter().map(|w| w.id).collect();
            new.push(self.fresh(slipstream_config::unused_id(&taken), String::new()));
        }
        let position = |id: u32| new.iter().position(|w| w.id == id);
        // A survivor goes where its id went; a deleted one where the nearest survivor before it
        // went, or to the first workspace.
        let map: Vec<usize> = (0..old_ids.len())
            .map(|index| {
                if old[index].is_none() {
                    return position(old_ids[index]).unwrap_or(0);
                }
                (0..index)
                    .rev()
                    .filter(|before| old[*before].is_none())
                    .find_map(|before| position(old_ids[before]))
                    .unwrap_or(0)
            })
            .collect();
        self.list = new;
        for (index, slot) in old.into_iter().enumerate() {
            if let Some(gone) = slot {
                let to = map[index];
                for window in gone.layout.windows() {
                    self.list[to].layout.insert(window, None, area);
                }
            }
        }
        map
    }

    pub fn get(&self, index: usize) -> &Workspace<T> {
        &self.list[index]
    }

    pub fn get_mut(&mut self, index: usize) -> &mut Workspace<T> {
        &mut self.list[index]
    }

    /// Every window on every workspace.
    pub fn all_windows(&self) -> Vec<T> {
        self.list.iter().flat_map(|w| w.layout.windows()).collect()
    }

    /// Which workspace holds `id`.
    pub fn find(&self, id: &T) -> Option<usize> {
        self.list
            .iter()
            .position(|w| w.layout.windows().contains(id))
    }

    /// Adds `id` to workspace `index`, splitting `beside` if it's there. A window maximised there
    /// goes back to its tile, so the new one is seen.
    pub fn insert(&mut self, index: usize, id: T, beside: Option<&T>, area: Rect) {
        let ws = &mut self.list[index];
        ws.maximised = None;
        ws.layout.insert(id, beside, area);
    }

    /// `window` is being focused: a different window maximised on its workspace goes back to its
    /// tile, since the one wanted is underneath it. Whether one did.
    pub fn focused(&mut self, window: &T) -> bool {
        let Some(index) = self.find(window) else {
            return false;
        };
        let ws = &mut self.list[index];
        if ws.maximised.as_ref().is_some_and(|max| max != window) {
            ws.maximised = None;
            return true;
        }
        false
    }

    /// Removes `id` from whichever workspace holds it. If it was that workspace's remembered
    /// focus, the most recently tiled remaining window takes over; if it was the centre of
    /// gravity, the most recently used one (by `recency`, lower is more recent) does.
    pub fn remove(&mut self, id: &T, recency: impl Fn(&T) -> usize) -> Option<Removed<T>> {
        let index = self.find(id)?;
        let ws = &mut self.list[index];
        ws.layout.remove(id);
        let mut remaining = ws.layout.windows();
        remaining.sort_by_key(|w| recency(w));
        let new_centre = ws.gravity.remove(id, &remaining);
        if ws.maximised.as_ref() == Some(id) {
            ws.maximised = None;
        }
        if ws.last_focus.as_ref() == Some(id) {
            ws.last_focus = ws.layout.windows().last().cloned();
        }
        Some(Removed {
            workspace: index,
            new_centre,
        })
    }
}

/// Super+D's workspace: the lowest-numbered one that is empty (`empty`, one flag a workspace)
/// and not on any screen (`shown`, the workspace each screen is showing).
pub fn lowest_empty(empty: &[bool], shown: &[usize]) -> Option<usize> {
    empty
        .iter()
        .enumerate()
        .position(|(index, &empty)| empty && !shown.contains(&index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gravity::Rung;

    #[test]
    fn super_d_picks_the_lowest_empty_workspace() {
        // 1 has windows and is on screen, 2 is on the other screen, 3 and 5 are empty.
        let empty = [false, true, true, false, true];
        assert_eq!(lowest_empty(&empty, &[0, 1]), Some(2));
        assert_eq!(lowest_empty(&empty, &[0]), Some(1));
        assert_eq!(lowest_empty(&empty, &[2, 1, 4]), None);
        assert_eq!(lowest_empty(&[false, false], &[0]), None);
    }

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1000,
        h: 600,
    };

    fn five() -> Vec<(u32, String)> {
        (1..=5).map(|id| (id, String::new())).collect()
    }

    #[test]
    fn an_edited_list_is_followed_by_id_and_a_deleted_workspace_s_windows_go_left() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "one", None, AREA);
        ws.insert(2, "three", None, AREA);
        ws.insert(3, "four", None, AREA);
        // Workspace 3 (id 3) is deleted, 4 is renamed and moved to the front, and a new one is
        // added at the end.
        let entries = vec![
            (4, "Mail".to_string()),
            (1, String::new()),
            (2, String::new()),
            (5, String::new()),
            (9, "New".to_string()),
        ];
        let map = ws.reconcile(&entries, 1, AREA);
        assert_eq!(ws.count(), 5);
        assert_eq!(
            map,
            vec![1, 2, 2, 0, 3],
            "id 3's windows went to id 2, before it"
        );
        assert_eq!(
            ws.find(&"four"),
            Some(0),
            "renamed and moved, windows and all"
        );
        assert_eq!(ws.label(0), "Mail");
        assert_eq!(
            ws.find(&"three"),
            Some(2),
            "the deleted workspace's window, one to the left"
        );
        assert_eq!(ws.find(&"one"), Some(1));
        assert_eq!(
            ws.label(1),
            "2",
            "an unnamed workspace goes by where it is now"
        );
        assert_eq!(ws.named("New"), Some(4));
        assert!(ws.get(4).layout.windows().is_empty());
    }

    #[test]
    fn deleting_the_first_workspace_sends_its_windows_to_the_new_first() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "a", None, AREA);
        let map = ws.reconcile(&[(2, String::new())], 1, AREA);
        assert_eq!(map[0], 0);
        assert_eq!(ws.find(&"a"), Some(0));
        assert_eq!(ws.count(), 1);
    }

    #[test]
    fn there_are_always_enough_workspaces_for_every_screen() {
        let mut ws: Workspaces<&str> = Workspaces::new(&five(), 16, 10);
        ws.reconcile(&[(1, String::new())], 2, AREA);
        assert_eq!(ws.count(), 2);
        assert_ne!(ws.get(0).id, ws.get(1).id);
    }

    #[test]
    fn a_workspace_added_after_an_id_at_the_top_of_its_range_still_gets_its_own() {
        let mut ws: Workspaces<&str> = Workspaces::new(&five(), 16, 10);
        ws.reconcile(&[(u32::MAX, "Mail".into())], 3, AREA);
        let ids: Vec<u32> = (0..ws.count()).map(|index| ws.get(index).id).collect();
        assert_eq!(ids, [u32::MAX, 1, 2]);
    }

    #[test]
    fn a_window_lives_on_exactly_one_workspace() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "a", None, AREA);
        ws.insert(2, "b", None, AREA);
        assert_eq!(ws.find(&"a"), Some(0));
        assert_eq!(ws.find(&"b"), Some(2));
        assert_eq!(ws.all_windows(), vec!["a", "b"]);
        assert_eq!(
            ws.remove(&"b", |_| 0),
            Some(Removed {
                workspace: 2,
                new_centre: None
            })
        );
        assert_eq!(ws.find(&"b"), None);
        assert_eq!(ws.remove(&"missing", |_| 0), None);
    }

    #[test]
    fn remembered_focus_moves_on_when_its_window_leaves() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "a", None, AREA);
        ws.insert(0, "b", Some(&"a"), AREA);
        ws.get_mut(0).last_focus = Some("b");
        ws.remove(&"b", |_| 0);
        assert_eq!(ws.get(0).last_focus, Some("a"));
        ws.remove(&"a", |_| 0);
        assert_eq!(ws.get(0).last_focus, None);
    }

    #[test]
    fn a_maximised_window_takes_the_area() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        for id in ["a", "b", "c"] {
            ws.insert(0, id, None, AREA);
        }
        let tiled = ws.get_mut(0).rects(AREA);
        assert!(ws.get_mut(0).toggle_maximised(&"b"));
        let maximised = ws.get_mut(0).rects(AREA);
        let rect = |rects: &[(&str, Rect)], id| rects.iter().find(|(w, _)| *w == id).unwrap().1;
        assert_eq!(rect(&maximised, "b"), AREA.inset(16));
        assert_eq!(
            rect(&maximised, "a"),
            rect(&tiled, "a"),
            "neighbours keep their tiles"
        );
        assert_eq!(
            ws.get_mut(0).tiles_within(AREA, &|_| (0, 0)),
            tiled,
            "the focus keys still see the tiles"
        );
        assert!(!ws.get_mut(0).toggle_maximised(&"b"));
        assert_eq!(ws.get_mut(0).rects(AREA), tiled, "back as before");

        ws.get_mut(0).toggle_maximised(&"c");
        ws.remove(&"c", |_| 0);
        assert_eq!(
            ws.get(0).maximised,
            None,
            "a window leaving takes its maximise"
        );
    }

    #[test]
    fn focus_elsewhere_ends_it() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "a", None, AREA);
        ws.insert(0, "b", None, AREA);
        ws.insert(1, "c", None, AREA);
        ws.get_mut(0).toggle_maximised(&"a");
        assert!(!ws.focused(&"a"), "focusing it again changes nothing");
        assert!(!ws.focused(&"c"), "another workspace's window leaves it be");
        assert_eq!(ws.get(0).maximised, Some("a"));
        assert!(ws.focused(&"b"));
        assert_eq!(ws.get(0).maximised, None);
    }

    #[test]
    fn a_new_window_ends_it() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        ws.insert(0, "a", None, AREA);
        ws.insert(0, "b", None, AREA);
        ws.get_mut(0).toggle_maximised(&"a");
        ws.insert(0, "c", Some(&"a"), AREA);
        assert_eq!(ws.get(0).maximised, None);
        assert_eq!(ws.get_mut(0).rects(AREA).len(), 3);
    }

    #[test]
    fn gravity_lays_out_until_it_turns_off_and_survives_its_centre_leaving() {
        let mut ws = Workspaces::new(&five(), 16, 10);
        for id in ["a", "b", "c"] {
            ws.insert(0, id, None, AREA);
        }
        let tiled = ws.get_mut(0).rects(AREA);
        ws.get_mut(0).gravity.step(&"a", true, &["b", "c"]);
        let heavy = ws.get_mut(0).rects(AREA);
        assert_ne!(tiled, heavy);

        // "c" was used most recently, so it takes the centre when "a" closes.
        let recency = |w: &&str| if *w == "c" { 0 } else { 1 };
        assert_eq!(ws.remove(&"a", recency).unwrap().new_centre, Some("c"));
        assert_eq!(ws.get(0).gravity.rung(&"c"), Rung::Centre);
        assert_eq!(
            ws.remove(&"b", recency).unwrap().new_centre,
            None,
            "an orbit window leaving hands nothing over"
        );

        ws.get_mut(0).gravity.off();
        let back = ws.get_mut(0).rects(AREA);
        assert_eq!(back, ws.get(0).layout.rects(AREA));
    }
}

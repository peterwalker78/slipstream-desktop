//! Putting the layout back.
//!
//! The record says which app was where. Launching them is the easy half; placing them is the
//! work, because apps start in whatever order they feel like and a new window would otherwise
//! tile beside whatever happens to have focus. So the plan holds every recorded window as an
//! unclaimed place, and each window that maps claims the first place its app is still waiting
//! for. Once a workspace's places are claimed, its recorded tree is rebuilt around them.
//!
//! **It reopens apps, not documents.** A `.desktop` entry's `Exec` line is the whole of what is
//! run. See the same section of the design for what that does and doesn't bring back.
//!
//! Places expire, because an app opened by hand half an hour later is a new window, not a
//! returning one. Pure logic, generic over the window type, so all of it is unit-tested.

use crate::{
    gravity::{Gravity, Rung},
    layout::Shape,
    session,
};

/// How long a recorded place waits for its window after the apps are launched. Long enough for a
/// browser or an Electron app to get going on a cold cache, short enough that it has run out
/// before anyone opens something by hand.
pub const EXPIRY: f64 = 30.0;

/// Which workspace a recorded one is now: the one with its name, if it had a name and that
/// workspace is still there; else the one at its number, if it had no name and there are that many;
/// else the first, since the workspace it was on has gone.
pub fn workspace_now(
    name: &str,
    index: usize,
    count: usize,
    named: impl Fn(&str) -> Option<usize>,
) -> usize {
    if !name.trim().is_empty() {
        return named(name).unwrap_or(0);
    }
    match index.checked_sub(1) {
        Some(at) if at < count => at,
        _ => 0,
    }
}

/// One recorded window, waiting for its app to open it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// The recorded `app_id`, lower-cased, as `window_app_id` reads it.
    pub app: String,
    /// The app's name from its desktop entry, for the offer card and the log.
    pub name: String,
    /// What to run. Never the app id, which any client can set to anything.
    pub exec: Vec<String>,
    pub in_terminal: bool,
    /// 0-based. `None` for a window that was in the code rain, which belongs to no workspace.
    pub workspace: Option<usize>,
    /// Its place among that workspace's windows: what the recorded tree's leaves refer to.
    pub slot: usize,
    pub focused: bool,
    /// Its rung on gravity's ladder, if gravity was on for its workspace.
    pub rung: Option<Rung>,
    pub pinned: bool,
}

/// Where a window that has just claimed its place belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    /// 0-based, or `None` for the code rain.
    pub workspace: Option<usize>,
    /// It had focus when the session ended, so it gets it back once everything has landed.
    pub focused: bool,
}

/// A recorded desktop, part-way back.
#[derive(Debug)]
pub struct Plan<W> {
    places: Vec<Place>,
    /// The window that claimed each place, by the same index.
    taken: Vec<Option<W>>,
    /// Each workspace's recorded tree, its leaves being slot numbers within that workspace.
    trees: Vec<Option<Shape<usize>>>,
    /// The workspace that was in front, 0-based.
    active: usize,
    /// Apps the record named that have no desktop entry here any more.
    lost: Vec<String>,
    /// Wall time after which an arriving window is just a new window.
    until: f64,
}

impl<W: Clone + PartialEq> Plan<W> {
    /// A plan from a record. `entry` looks an app id up in the desktop entries and gives back its
    /// name and how to run it; an app it doesn't know is counted as lost rather than guessed at,
    /// and **never** run as a command.
    ///
    /// `count` is how many workspaces there are now, and `place` says which of them a recorded
    /// workspace (its number counting from 1) has become — by name, or by number, or the first
    /// when it has gone. Two recorded workspaces that land on the same one share it: the first
    /// keeps its tiling tree, and the other's windows tile in beside them.
    pub fn new(
        record: &session::Record,
        now: f64,
        entry: impl Fn(&str) -> Option<(String, Vec<String>, bool)>,
        count: usize,
        place: impl Fn(usize) -> usize,
    ) -> Self {
        let count = count.max(1);
        let place = |index: usize| place(index).min(count - 1);
        // Which recorded workspace's tree each workspace now has.
        let mut owner: Vec<Option<usize>> = vec![None; count];
        let mut trees = vec![None; count];
        for tiling in &record.workspaces {
            let to = place(tiling.index);
            if owner[to].is_none() {
                owner[to] = Some(tiling.index);
                trees[to] = session::decode(&tiling.tiles);
            }
        }
        let mut places = Vec::new();
        let mut lost = Vec::new();
        for win in &record.windows {
            let Some((name, exec, in_terminal)) = entry(&win.app) else {
                if !lost.contains(&win.app) {
                    lost.push(win.app.clone());
                }
                continue;
            };
            // Counting from 1 in the file, 0-based here. A window in the rain has no workspace.
            let (workspace, slot) = match (win.in_rain, win.workspace) {
                (true, _) => (None, win.slot),
                (false, Some(index)) if index >= 1 => {
                    let to = place(index);
                    // A window whose recorded workspace doesn't own the tree here is kept clear
                    // of its slot numbers, so it tiles in beside them rather than taking a leaf.
                    let slot = match owner[to] {
                        // Saturating: both numbers come from the state file, and a hand edit
                        // can make them anything.
                        Some(own) if own != index => {
                            win.slot.saturating_add(index.saturating_mul(1000))
                        }
                        _ => win.slot,
                    };
                    (Some(to), slot)
                }
                // A record with neither is malformed; drop it rather than invent a workspace.
                (false, _) => continue,
            };
            places.push(Place {
                app: win.app.to_lowercase(),
                name,
                exec,
                in_terminal,
                workspace,
                slot,
                focused: win.focused,
                rung: win.gravity.as_deref().and_then(Rung::from_label),
                pinned: win.pinned,
            });
        }
        let taken = vec![None; places.len()];
        Self {
            places,
            taken,
            trees,
            active: place(record.active_workspace.max(1)),
            lost,
            until: now + EXPIRY,
        }
    }

    /// The wait starts again from here. The plan is built when the record is read, which may be
    /// a while before the offer is answered and anything is actually launched.
    pub fn restart(&mut self, now: f64) {
        self.until = now + EXPIRY;
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// What to launch, in the order the record named them.
    pub fn to_launch(&self) -> Vec<(Vec<String>, bool)> {
        self.places
            .iter()
            .map(|place| (place.exec.clone(), place.in_terminal))
            .collect()
    }

    /// The apps the offer card names, each once, in the order they were recorded.
    pub fn app_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for place in &self.places {
            if !names.contains(&place.name) {
                names.push(place.name.clone());
            }
        }
        names
    }

    /// Apps in the record with no desktop entry here. They aren't reopened, and the offer says so
    /// rather than pretending the layout came back whole.
    pub fn lost(&self) -> &[String] {
        &self.lost
    }

    /// A window has mapped. If a place of its app is still waiting, it takes the first one and
    /// this says where it belongs; otherwise it is an ordinary new window.
    pub fn claim(&mut self, app: &str, window: &W, now: f64) -> Option<Claim> {
        if now > self.until {
            return None;
        }
        let index = self.places.iter().enumerate().position(|(i, place)| {
            self.taken[i].is_none() && place.app.eq_ignore_ascii_case(app)
        })?;
        self.taken[index] = Some(window.clone());
        Some(Claim {
            workspace: self.places[index].workspace,
            focused: self.places[index].focused,
        })
    }

    /// Whether this window already holds a place, so it isn't offered one twice.
    pub fn holds(&self, window: &W) -> bool {
        self.taken
            .iter()
            .any(|taken| taken.as_ref() == Some(window))
    }

    /// A window that has gone again gives its place up, so nothing claimed by a window that
    /// closed while the rest were still starting holds a slot in the rebuilt tree.
    pub fn release(&mut self, window: &W) {
        for taken in &mut self.taken {
            if taken.as_ref() == Some(window) {
                *taken = None;
            }
        }
    }

    /// Workspace `index`'s recorded tree with the windows that claimed its slots in place of the
    /// slot numbers. Slots nothing claimed drop out, and the splits above them close up.
    pub fn tree(&self, index: usize) -> Option<Shape<W>> {
        self.trees.get(index)?.as_ref()?.map(&|slot: &usize| {
            self.places
                .iter()
                .enumerate()
                .find(|(_, place)| place.workspace == Some(index) && place.slot == *slot)
                .and_then(|(i, _)| self.taken[i].clone())
        })
    }

    /// Gravity as it was on workspace `index`, over the windows that came back.
    pub fn gravity(&self, index: usize) -> Gravity<W> {
        let mut centre = None;
        let mut distant = Vec::new();
        let mut pinned = Vec::new();
        let mut grid = false;
        for (i, place) in self.places.iter().enumerate() {
            let (Some(window), Some(rung)) = (self.taken[i].clone(), place.rung) else {
                continue;
            };
            if place.workspace != Some(index) {
                continue;
            }
            if place.pinned {
                pinned.push(window.clone());
            }
            match rung {
                Rung::Centre | Rung::Wide | Rung::Spotlight => centre = Some((window, rung)),
                Rung::Distant => distant.push(window),
                Rung::Grid => grid = true,
                Rung::Orbit | Rung::Tiling => {}
            }
        }
        Gravity::restore(centre, distant, pinned, grid)
    }

    /// Windows recorded as minimised that have come back, to pour into the code rain in the order
    /// their streams were in.
    pub fn to_minimise(&self) -> Vec<W> {
        let mut rain: Vec<(usize, W)> = self
            .places
            .iter()
            .enumerate()
            .filter(|(_, place)| place.workspace.is_none())
            .filter_map(|(i, place)| self.taken[i].clone().map(|window| (place.slot, window)))
            .collect();
        rain.sort_by_key(|(slot, _)| *slot);
        rain.into_iter().map(|(_, window)| window).collect()
    }

    /// The window that had focus, if it came back.
    pub fn focus(&self) -> Option<W> {
        self.places
            .iter()
            .enumerate()
            .find(|(_, place)| place.focused)
            .and_then(|(i, _)| self.taken[i].clone())
    }

    /// The workspace that was in front, 0-based.
    pub fn active(&self) -> usize {
        self.active
    }

    /// How many of the recorded windows have come back.
    pub fn claimed(&self) -> usize {
        self.taken.iter().filter(|taken| taken.is_some()).count()
    }

    /// Every workspace a recorded window landed on, so only those are rebuilt.
    pub fn touched(&self) -> Vec<usize> {
        let mut list = Vec::new();
        for (i, place) in self.places.iter().enumerate() {
            if let (Some(index), true) = (place.workspace, self.taken[i].is_some()) {
                if !list.contains(&index) {
                    list.push(index);
                }
            }
        }
        list.sort_unstable();
        list
    }

    /// Whether there is nothing more to wait for: every place claimed, or the time up.
    pub fn settled(&self, now: f64) -> bool {
        now > self.until || self.taken.iter().all(|taken| taken.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Record, Tiling, Win};

    fn entry(app: &str) -> Option<(String, Vec<String>, bool)> {
        match app {
            "org.kde.konsole" => Some(("Konsole".into(), vec!["konsole".into()], false)),
            "org.kde.dolphin" => Some(("Dolphin".into(), vec!["dolphin".into()], false)),
            "firefox" => Some(("Firefox".into(), vec!["firefox".into()], false)),
            _ => None,
        }
    }

    fn win(app: &str, workspace: usize, slot: usize) -> Win {
        Win {
            app: app.into(),
            workspace: Some(workspace),
            slot,
            ..Win::default()
        }
    }

    fn record(windows: Vec<Win>, workspaces: Vec<Tiling>) -> Record {
        Record::new(1, windows, workspaces)
    }

    fn plan(record: &Record) -> Plan<&'static str> {
        Plan::new(record, 0.0, entry, 5, |index| index - 1)
    }

    #[test]
    fn a_window_claims_the_first_place_its_app_is_waiting_for() {
        let record = record(
            vec![
                win("org.kde.konsole", 1, 0),
                win("org.kde.konsole", 2, 1),
                win("org.kde.dolphin", 1, 1),
            ],
            vec![],
        );
        let mut plan = plan(&record);
        assert_eq!(
            plan.claim("org.kde.konsole", &"a", 1.0),
            Some(Claim {
                workspace: Some(0),
                focused: false
            })
        );
        // The second Konsole window takes the second Konsole place, not the first again.
        assert_eq!(
            plan.claim("org.kde.konsole", &"b", 1.1),
            Some(Claim {
                workspace: Some(1),
                focused: false
            })
        );
        // A third is just a new window.
        assert_eq!(plan.claim("org.kde.konsole", &"c", 1.2), None);
        assert_eq!(plan.claimed(), 2);
    }

    #[test]
    fn a_workspace_number_as_big_as_an_integer_is_a_place_not_a_crash() {
        let record = record(
            vec![win("firefox", 5, 0), win("org.kde.konsole", usize::MAX, 3)],
            vec![Tiling {
                index: 5,
                name: String::new(),
                tiles: "0".into(),
            }],
        );
        let plan: Plan<&str> = Plan::new(&record, 0.0, entry, 5, |index| index.saturating_sub(1));
        assert_eq!(plan.len(), 2, "both land on the last workspace there is");
    }

    #[test]
    fn a_place_stops_waiting_after_half_a_minute() {
        let record = record(vec![win("firefox", 1, 0)], vec![]);
        let mut plan = plan(&record);
        assert_eq!(plan.claim("firefox", &"a", EXPIRY + 0.1), None);
        assert!(plan.settled(EXPIRY + 0.1));
    }

    #[test]
    fn an_app_with_no_desktop_entry_is_lost_never_run() {
        let record = record(
            vec![win("some-run-box-command", 1, 0), win("firefox", 1, 1)],
            vec![],
        );
        let plan = plan(&record);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.lost(), ["some-run-box-command"]);
        assert_eq!(plan.to_launch(), [(vec!["firefox".to_string()], false)]);
    }

    #[test]
    fn the_recorded_tree_comes_back_around_the_windows_that_claimed_it() {
        let record = record(
            vec![
                win("org.kde.konsole", 1, 0),
                win("org.kde.dolphin", 1, 1),
                win("firefox", 1, 2),
            ],
            vec![Tiling {
                index: 1,
                name: String::new(),
                tiles: "v0.5(0,h0.4(1,2))".into(),
            }],
        );
        let mut plan = plan(&record);
        plan.claim("org.kde.konsole", &"konsole", 1.0);
        plan.claim("org.kde.dolphin", &"dolphin", 1.1);
        plan.claim("firefox", &"firefox", 1.2);
        let tree = plan.tree(0).unwrap();
        assert_eq!(
            tree,
            Shape::Split {
                vertical: true,
                ratio: 0.5,
                a: Box::new(Shape::Leaf("konsole")),
                b: Box::new(Shape::Split {
                    vertical: false,
                    ratio: 0.4,
                    a: Box::new(Shape::Leaf("dolphin")),
                    b: Box::new(Shape::Leaf("firefox")),
                }),
            }
        );
        assert_eq!(plan.touched(), [0]);
    }

    #[test]
    fn a_slot_nothing_claimed_closes_up() {
        let record = record(
            vec![
                win("org.kde.konsole", 1, 0),
                win("org.kde.dolphin", 1, 1),
                win("firefox", 1, 2),
            ],
            vec![Tiling {
                index: 1,
                name: String::new(),
                tiles: "v0.5(0,h0.4(1,2))".into(),
            }],
        );
        let mut plan = plan(&record);
        plan.claim("org.kde.konsole", &"konsole", 1.0);
        // Dolphin never starts; the split above it collapses into Firefox.
        plan.claim("firefox", &"firefox", 1.2);
        assert_eq!(
            plan.tree(0).unwrap(),
            Shape::Split {
                vertical: true,
                ratio: 0.5,
                a: Box::new(Shape::Leaf("konsole")),
                b: Box::new(Shape::Leaf("firefox")),
            }
        );
    }

    #[test]
    fn a_window_that_closes_again_gives_its_place_back() {
        let record = record(
            vec![win("firefox", 1, 0)],
            vec![Tiling {
                index: 1,
                name: String::new(),
                tiles: "0".into(),
            }],
        );
        let mut plan = plan(&record);
        plan.claim("firefox", &"a", 1.0);
        plan.release(&"a");
        assert_eq!(plan.tree(0), None);
        assert_eq!(plan.claimed(), 0);
    }

    #[test]
    fn gravity_comes_back_over_the_windows_that_did() {
        let record = record(
            vec![
                Win {
                    gravity: Some("centre".into()),
                    ..win("org.kde.konsole", 2, 0)
                },
                Win {
                    gravity: Some("distant".into()),
                    pinned: true,
                    ..win("org.kde.dolphin", 2, 1)
                },
                Win {
                    gravity: Some("orbit".into()),
                    ..win("firefox", 2, 2)
                },
            ],
            vec![],
        );
        let mut plan = plan(&record);
        plan.claim("org.kde.konsole", &"konsole", 1.0);
        plan.claim("org.kde.dolphin", &"dolphin", 1.1);
        plan.claim("firefox", &"firefox", 1.2);
        let gravity = plan.gravity(1);
        assert_eq!(gravity.centre(), Some(&"konsole"));
        assert_eq!(gravity.rung(&"dolphin"), Rung::Distant);
        assert_eq!(gravity.rung(&"firefox"), Rung::Orbit);
        assert!(gravity.is_pinned(&"dolphin"));
        // Workspace 1 had no gravity, so it stays tiled.
        assert!(!plan.gravity(0).is_on());
    }

    #[test]
    fn a_centre_that_never_came_back_leaves_gravity_off() {
        let record = record(
            vec![
                Win {
                    gravity: Some("centre".into()),
                    ..win("org.kde.konsole", 1, 0)
                },
                Win {
                    gravity: Some("orbit".into()),
                    ..win("firefox", 1, 1)
                },
            ],
            vec![],
        );
        let mut plan = plan(&record);
        plan.claim("firefox", &"firefox", 1.0);
        assert!(!plan.gravity(0).is_on());
    }

    #[test]
    fn the_code_rain_comes_back_in_its_own_order() {
        let record = record(
            vec![
                Win {
                    in_rain: true,
                    workspace: None,
                    slot: 1,
                    ..win("firefox", 1, 1)
                },
                Win {
                    in_rain: true,
                    workspace: None,
                    slot: 0,
                    ..win("org.kde.konsole", 1, 0)
                },
            ],
            vec![],
        );
        let mut plan = plan(&record);
        assert_eq!(
            plan.claim("firefox", &"firefox", 1.0),
            Some(Claim {
                workspace: None,
                focused: false
            })
        );
        plan.claim("org.kde.konsole", &"konsole", 1.1);
        assert_eq!(plan.to_minimise(), ["konsole", "firefox"]);
        assert!(plan.touched().is_empty());
    }

    #[test]
    fn focus_and_the_workspace_in_front_come_back_last() {
        let mut record = record(
            vec![
                win("org.kde.konsole", 1, 0),
                Win {
                    focused: true,
                    ..win("firefox", 3, 0)
                },
            ],
            vec![],
        );
        record.active_workspace = 3;
        let mut plan = Plan::new(&record, 0.0, entry, 5, |index| index - 1);
        assert_eq!(plan.active(), 2);
        assert_eq!(plan.focus(), None);
        plan.claim("firefox", &"firefox", 1.0);
        assert_eq!(plan.focus(), Some("firefox"));
        assert!(!plan.settled(1.0));
        plan.claim("org.kde.konsole", &"konsole", 1.1);
        assert!(plan.settled(1.1));
    }

    #[test]
    fn a_recorded_workspace_is_found_by_name_then_number_else_the_first() {
        let named = |name: &str| (name == "Mail").then_some(3);
        assert_eq!(
            workspace_now("Mail", 1, 5, named),
            3,
            "moved, but still called Mail"
        );
        assert_eq!(
            workspace_now("Gone", 2, 5, named),
            0,
            "its name is nowhere now"
        );
        assert_eq!(workspace_now("", 2, 5, named), 1, "no name: by its number");
        assert_eq!(workspace_now("", 7, 5, named), 0, "a number past the end");
    }

    #[test]
    fn two_recorded_workspaces_landing_on_one_share_it_without_mixing_their_slots() {
        let record = record(
            vec![win("org.kde.konsole", 1, 0), win("org.kde.dolphin", 3, 0)],
            vec![
                Tiling {
                    index: 1,
                    name: String::new(),
                    tiles: "0".into(),
                },
                Tiling {
                    index: 3,
                    name: String::new(),
                    tiles: "0".into(),
                },
            ],
        );
        // Three workspaces became one.
        let mut plan: Plan<&str> = Plan::new(&record, 0.0, entry, 1, |_| 0);
        plan.claim("org.kde.konsole", &"k", 0.0);
        plan.claim("org.kde.dolphin", &"f", 0.0);
        assert_eq!(
            plan.tree(0).map(|shape| shape.leaves()),
            Some(vec!["k"]),
            "the first keeps its tree; the other tiles in beside it"
        );
    }

    #[test]
    fn app_names_are_listed_once_each_for_the_offer() {
        let record = record(
            vec![
                win("org.kde.konsole", 1, 0),
                win("org.kde.konsole", 1, 1),
                win("firefox", 2, 0),
            ],
            vec![],
        );
        assert_eq!(plan(&record).app_names(), ["Konsole", "Firefox"]);
    }
}

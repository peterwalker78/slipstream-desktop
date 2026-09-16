//! The screens the desktop is spread across, and which workspace each one shows.
//!
//! Workspaces stay global (`workspace.rs`): a window lives on exactly one of them, wherever it is
//! drawn. A screen is a view on to one workspace at a time, and no workspace is on two screens at
//! once.
//!
//! What a screen shows changes only through what is done on that screen. Going to a workspace
//! another screen is showing moves the keyboard there rather than trading the two screens'
//! workspaces; trading is its own command. Each workspace remembers the
//! screen it was last shown on, so a hidden one is laid out for the screen it will come back to,
//! and a screen that goes out and comes back (a monitor unplugged, the lid shut) gets back what
//! it was showing.
//!
//! Generic over the output type so the rules are unit-tested without Wayland.

/// What a screen is known by from one connection to the next: its connector's name.
pub trait Named {
    fn name(&self) -> String;
}

impl Named for smithay::output::Output {
    fn name(&self) -> String {
        smithay::output::Output::name(self)
    }
}

impl Named for &str {
    fn name(&self) -> String {
        self.to_string()
    }
}

/// One screen: an output, and the workspace it is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen<O> {
    pub output: O,
    /// Where its top left corner sits in the layout of screens, in logical pixels.
    pub x: i32,
    pub y: i32,
    /// 0-based.
    pub workspace: usize,
    /// The workspace it was showing before it took over one from a screen that went out, to go
    /// back to when that screen returns. Cleared by anything else it's asked to show.
    carried_from: Option<usize>,
}

/// Every lit screen, left to right, and which one the keyboard is on.
///
/// How many workspaces there are is not kept here: it's the workspace list's to say, and a second
/// copy of it once went stale and hid every workspace past the fifth. Anything that picks or
/// clamps a workspace is told the count by its caller.
#[derive(Debug)]
pub struct Screens<O> {
    list: Vec<Screen<O>>,
    focused: usize,
    /// Per workspace, the name of the screen that last showed it.
    homes: Vec<Option<String>>,
    /// Screens that went out, by name, with the workspace each was showing.
    gone: Vec<(String, usize)>,
}

impl<O> Default for Screens<O> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            focused: 0,
            homes: Vec::new(),
            gone: Vec::new(),
        }
    }
}

impl<O: Clone + PartialEq + Named> Screens<O> {
    /// Lights `output` at `x`. A screen that was here before takes back the workspace it was
    /// showing, and a screen that carried it on while it was gone goes back to its own. Failing
    /// that it takes `wanted`, the workspace the caller has set aside for it — a scratch one of
    /// its own, or the one it was remembered as showing in an earlier session. Failing that, the
    /// lowest-numbered of the `count` workspaces no other screen has, so a second screen comes up
    /// on workspace 2 rather than looking at the same desktop twice. The first screen to arrive
    /// takes the keyboard.
    pub fn add(&mut self, output: O, x: i32, y: i32, count: usize, wanted: Option<usize>) -> usize {
        if let Some(index) = self.index_of(&output) {
            let focused = self.focused_output();
            self.list[index].x = x;
            self.list[index].y = y;
            self.sort();
            // Moving a screen along doesn't move the keyboard off the one it's on.
            self.focused = focused
                .and_then(|output| self.index_of(&output))
                .unwrap_or(0);
            return self.index_of(&output).unwrap_or(index);
        }
        let count = count.max(1);
        let name = output.name();
        let remembered = self
            .gone
            .iter()
            .position(|(gone, _)| *gone == name)
            .map(|at| self.gone.remove(at).1)
            .filter(|workspace| *workspace < count);
        if let Some(wanted) = remembered
            && let Some(carrier) = self.showing(wanted)
            && let Some(own) = self.list[carrier].carried_from
            && own < count
            && self.showing(own).is_none()
        {
            // The screen that carried it on goes back to what it was showing.
            let carrier_name = self.list[carrier].output.name();
            let screen = &mut self.list[carrier];
            screen.workspace = own;
            screen.carried_from = None;
            self.set_home(own, carrier_name);
        }
        let workspace = remembered
            .filter(|wanted| self.showing(*wanted).is_none())
            .or_else(|| wanted.filter(|index| *index < count && self.showing(*index).is_none()))
            .or_else(|| (0..count).find(|index| self.showing(*index).is_none()))
            .unwrap_or(0);
        let focused = self.focused_output();
        self.list.push(Screen {
            output: output.clone(),
            x,
            y,
            workspace,
            carried_from: None,
        });
        self.set_home(workspace, name);
        self.sort();
        // Sorting moves the others about; the keyboard stays where it was.
        self.focused = focused
            .and_then(|output| self.index_of(&output))
            .unwrap_or(0);
        self.index_of(&output).unwrap_or(0)
    }

    /// Puts a screen out (unplugged, or the lid shut on it), remembering what it was showing for
    /// when it comes back. The keyboard moves to the nearest screen left of it. Returns the
    /// workspace it was showing, which nothing else is now.
    pub fn remove(&mut self, output: &O) -> Option<usize> {
        let index = self.index_of(output)?;
        let screen = self.list.remove(index);
        let name = screen.output.name();
        self.gone.retain(|(gone, _)| *gone != name);
        self.gone.push((name, screen.workspace));
        if self.focused >= index {
            self.focused = self.focused.saturating_sub(1);
        }
        self.focused = self.focused.min(self.list.len().saturating_sub(1));
        Some(screen.workspace)
    }

    /// The focused screen carries on workspace `index` for a screen that went out, and goes back
    /// to what it was showing when that screen returns.
    pub fn carry(&mut self, index: usize) -> bool {
        if self.showing(index).is_some() {
            return false;
        }
        let focused = self.focused;
        let Some(screen) = self.list.get_mut(focused) else {
            return false;
        };
        screen.carried_from.get_or_insert(screen.workspace);
        screen.workspace = index;
        let name = screen.output.name();
        self.set_home(index, name);
        true
    }

    /// The workspace list changed: `map` says where each old index went, and `count` is how many
    /// there are now. Every screen follows its workspace; if two would end up on the same one (a
    /// deleted workspace's screen landing where another already looks), the later takes the
    /// lowest one nothing is showing. What each workspace remembers follows it too.
    pub fn remap(&mut self, map: &[usize], count: usize) {
        let count = count.max(1);
        let moved = |index: usize| map.get(index).copied().unwrap_or(0).min(count - 1);
        let mut taken: Vec<usize> = Vec::new();
        for screen in &mut self.list {
            let mut to = moved(screen.workspace);
            if taken.contains(&to) {
                to = (0..count).find(|free| !taken.contains(free)).unwrap_or(0);
            }
            taken.push(to);
            screen.workspace = to;
            screen.carried_from = screen.carried_from.map(moved);
        }
        let mut homes = vec![None; count];
        for (index, home) in std::mem::take(&mut self.homes).into_iter().enumerate() {
            let to = moved(index);
            if homes[to].is_none() {
                homes[to] = home;
            }
        }
        self.homes = homes;
        for (_, workspace) in &mut self.gone {
            *workspace = moved(*workspace);
        }
        for index in 0..self.list.len() {
            let (workspace, name) = (self.list[index].workspace, self.list[index].output.name());
            self.set_home(workspace, name);
        }
    }

    /// Workspaces past `count` have gone. Forgets the homes and the remembered workspaces that
    /// pointed at them, so nothing comes back to a workspace that isn't there.
    pub fn trim_homes(&mut self, count: usize) {
        self.homes.truncate(count);
        self.gone.retain(|(_, workspace)| *workspace < count);
        for screen in &mut self.list {
            if screen.carried_from.is_some_and(|own| own >= count) {
                screen.carried_from = None;
            }
        }
    }

    fn set_home(&mut self, workspace: usize, name: String) {
        if self.homes.len() <= workspace {
            self.homes.resize(workspace + 1, None);
        }
        self.homes[workspace] = Some(name);
    }

    /// Left to right, then top to bottom, so "the next screen" walks a row the way it reads and
    /// a stack from the top down.
    fn sort(&mut self) {
        self.list.sort_by_key(|screen| (screen.x, screen.y));
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Screen<O>> {
        self.list.iter()
    }

    pub fn index_of(&self, output: &O) -> Option<usize> {
        self.list.iter().position(|screen| &screen.output == output)
    }

    pub fn get(&self, index: usize) -> Option<&Screen<O>> {
        self.list.get(index)
    }

    pub fn focused_index(&self) -> usize {
        self.focused
    }

    pub fn focused(&self) -> Option<&Screen<O>> {
        self.list.get(self.focused)
    }

    pub fn focused_output(&self) -> Option<O> {
        self.focused().map(|screen| screen.output.clone())
    }

    /// The workspace the keyboard is looking at. With no screens at all — which only happens
    /// between a screen going out and the next one lighting up — it is the first.
    pub fn workspace(&self) -> usize {
        self.focused().map_or(0, |screen| screen.workspace)
    }

    /// Which screen is showing workspace `index`, if any.
    pub fn showing(&self, index: usize) -> Option<usize> {
        self.list
            .iter()
            .position(|screen| screen.workspace == index)
    }

    /// The workspace `output` is showing.
    pub fn workspace_of(&self, output: &O) -> Option<usize> {
        self.index_of(output)
            .map(|index| self.list[index].workspace)
    }

    /// Where a workspace is laid out and drawn: the screen showing it; else the screen that last
    /// showed it, if that one is lit; else the focused one, where it would appear if switched to.
    pub fn screen_for_workspace(&self, index: usize) -> usize {
        self.showing(index)
            .or_else(|| {
                let home = self.homes.get(index)?.as_ref()?;
                self.list
                    .iter()
                    .position(|screen| screen.output.name() == *home)
            })
            .unwrap_or(self.focused)
    }

    /// Points the keyboard at `index`. Returns whether it moved.
    pub fn focus(&mut self, index: usize) -> bool {
        if index >= self.list.len() || index == self.focused {
            return false;
        }
        self.focused = index;
        true
    }

    /// Goes to workspace `index` of `count`. If another screen is showing it, the keyboard moves
    /// to that screen and nothing else changes; otherwise the focused screen shows it.
    pub fn show(&mut self, index: usize, count: usize) -> Show {
        let index = index.min(count.saturating_sub(1));
        let Some(from) = self.focused().map(|screen| screen.workspace) else {
            return Show::default();
        };
        if from == index {
            return Show::default();
        }
        if let Some(other) = self.showing(index) {
            self.focused = other;
            return Show {
                changed: true,
                from,
                moved_to: Some(other),
            };
        }
        let focused = self.focused;
        let screen = &mut self.list[focused];
        screen.workspace = index;
        screen.carried_from = None;
        let name = screen.output.name();
        self.set_home(index, name);
        Show {
            changed: true,
            from,
            moved_to: None,
        }
    }

    /// The focused screen and screen `other` trade workspaces. The keyboard stays on the focused
    /// screen, now showing what `other` was.
    pub fn swap_with(&mut self, other: usize) -> bool {
        let focused = self.focused;
        if other == focused || other >= self.list.len() || focused >= self.list.len() {
            return false;
        }
        let (mine, theirs) = (self.list[focused].workspace, self.list[other].workspace);
        self.list[focused].workspace = theirs;
        self.list[other].workspace = mine;
        for index in [focused, other] {
            self.list[index].carried_from = None;
            let (workspace, name) = (self.list[index].workspace, self.list[index].output.name());
            self.set_home(workspace, name);
        }
        true
    }

    /// The screen beside the focused one to the left (`-1`) or right (`1`), if there is one.
    pub fn beside(&self, side: i32) -> Option<usize> {
        let index = self.focused as i32 + side.signum();
        (index >= 0 && (index as usize) < self.list.len()).then_some(index as usize)
    }
}

/// The screen `direction` of the one at `from`, by where the screens actually sit: the nearest on
/// that side, and among equals the one that lines up with it best across the other axis.
///
/// Screens used to be a row, so "the next one left" was the previous index. They can be stacked
/// now, so it has to be worked out from the rectangles.
pub fn towards(
    rects: &[crate::layout::Rect],
    from: usize,
    direction: crate::layout::Direction,
) -> Option<usize> {
    use crate::layout::Direction;
    let here = *rects.get(from)?;
    let (here_near, here_far) = match direction {
        Direction::Left | Direction::Right => (here.x, here.x + here.w),
        Direction::Up | Direction::Down => (here.y, here.y + here.h),
    };
    rects
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != from)
        .filter_map(|(index, there)| {
            let (near, far) = match direction {
                Direction::Left | Direction::Right => (there.x, there.x + there.w),
                Direction::Up | Direction::Down => (there.y, there.y + there.h),
            };
            // Wholly on the side asked for, and how far away it is along that axis.
            let gap = match direction {
                Direction::Left | Direction::Up => (far <= here_near).then(|| here_near - far),
                Direction::Right | Direction::Down => (near >= here_far).then(|| near - here_far),
            }?;
            // How much of the other axis they share: a screen straight ahead beats one off to a
            // corner, however close that corner is.
            let overlap = match direction {
                Direction::Left | Direction::Right => {
                    (here.y + here.h).min(there.y + there.h) - here.y.max(there.y)
                }
                Direction::Up | Direction::Down => {
                    (here.x + here.w).min(there.x + there.w) - here.x.max(there.x)
                }
            };
            Some((index, gap, overlap.max(0)))
        })
        .min_by_key(|(_, gap, overlap)| (*gap, -*overlap))
        .map(|(index, _, _)| index)
}

/// What going to a workspace did.
#[derive(Debug, Default, PartialEq)]
pub struct Show {
    /// Whether anything changed at all.
    pub changed: bool,
    /// The workspace the focused screen was showing.
    pub from: usize,
    /// The screen the keyboard moved to, already showing the workspace; `None` when the focused
    /// screen shows it instead.
    pub moved_to: Option<usize>,
}

/// What is drawn over the focused screen when a press lands on another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// The explorer, quick settings, the notification centre or the shortcut sheet.
    Panel,
    /// Something that asks or takes every click: the share picker, the login offer, the way
    /// out's card, Alt+Tab's switcher, or bullet time.
    Card,
}

/// What a press on a screen other than the focused one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressElsewhere {
    MoveKeyboard,
    /// As a click outside the panel would: it closes, and the keyboard stays.
    ClosePanels,
    /// Nothing at all: the card is still up, on the screen the keyboard is on.
    Ignore,
}

pub fn press_elsewhere(overlay: Overlay) -> PressElsewhere {
    match overlay {
        Overlay::None => PressElsewhere::MoveKeyboard,
        Overlay::Panel => PressElsewhere::ClosePanels,
        Overlay::Card => PressElsewhere::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Screens<&'static str> {
        let mut screens = Screens::default();
        screens.add("eDP-1", 0, 0, 5, None);
        screens.add("DP-2", 1920, 0, 5, None);
        screens
    }

    fn shown(screens: &Screens<&str>) -> Vec<usize> {
        screens.iter().map(|screen| screen.workspace).collect()
    }

    #[test]
    fn a_new_screen_takes_a_workspace_nothing_else_is_showing() {
        let screens = two();
        assert_eq!(shown(&screens), vec![0, 1]);
        assert_eq!(screens.focused_index(), 0, "the keyboard stays put");
    }

    #[test]
    fn screens_are_kept_in_the_order_they_sit_in() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 1920, 0, 5, None);
        screens.add("DP-2", 0, 0, 5, None);
        assert_eq!(screens.get(0).map(|s| s.output), Some("DP-2"));
        assert_eq!(
            screens.focused_output(),
            Some("eDP-1"),
            "a screen appearing to the left doesn't steal the keyboard"
        );
    }

    #[test]
    fn going_to_a_workspace_another_screen_has_moves_the_keyboard_there() {
        let mut screens = two();
        let show = screens.show(1, 5);
        assert_eq!(
            show,
            Show {
                changed: true,
                from: 0,
                moved_to: Some(1)
            }
        );
        assert_eq!(shown(&screens), vec![0, 1], "neither screen changed");
        assert_eq!(screens.focused_index(), 1);
        assert!(!screens.show(1, 5).changed, "already there");
    }

    #[test]
    fn going_to_a_workspace_no_one_has_shows_it_here() {
        let mut screens = two();
        let show = screens.show(3, 5);
        assert_eq!(show.moved_to, None);
        assert_eq!(shown(&screens), vec![3, 1], "the other screen untouched");
        assert_eq!(screens.focused_index(), 0);
    }

    #[test]
    fn swapping_trades_workspaces_and_keeps_the_keyboard_where_it_is() {
        let mut screens = two();
        assert!(screens.swap_with(1));
        assert_eq!(shown(&screens), vec![1, 0]);
        assert_eq!(screens.focused_index(), 0);
        assert!(!screens.swap_with(0), "not with itself");
        assert!(!screens.swap_with(5), "no such screen");
    }

    #[test]
    fn a_hidden_workspace_belongs_to_the_screen_that_last_showed_it() {
        let mut screens = two();
        // Workspace 3 on the right-hand screen, then that screen moves on to 4.
        screens.focus(1);
        screens.show(2, 5);
        screens.show(3, 5);
        screens.focus(0);
        assert_eq!(screens.screen_for_workspace(2), 1, "laid out for the right");
        assert_eq!(
            screens.screen_for_workspace(4),
            0,
            "never shown: the focused"
        );
        assert_eq!(screens.screen_for_workspace(0), 0, "on show");
    }

    #[test]
    fn the_keyboard_is_pointed_at_a_screen_that_is_there() {
        let mut screens = two();
        assert!(screens.focus(1));
        assert!(!screens.focus(1), "already there");
        assert!(!screens.focus(7), "no such screen");
        assert_eq!(screens.focused_index(), 1);
        assert_eq!(screens.beside(1), None, "nothing right of the last");
        assert_eq!(screens.beside(-1), Some(0));
    }

    #[test]
    fn a_screen_going_out_leaves_the_keyboard_on_one_that_is_still_there() {
        let mut screens = two();
        screens.focus(1);
        assert_eq!(screens.remove(&"DP-2"), Some(1));
        assert_eq!(screens.len(), 1);
        assert_eq!(screens.focused_output(), Some("eDP-1"));
        assert_eq!(screens.workspace(), 0);
        // The lid shuts on the last screen: nothing left to focus, and no panic.
        assert_eq!(screens.remove(&"eDP-1"), Some(0));
        assert!(screens.is_empty());
        assert_eq!(screens.workspace(), 0);
        assert_eq!(screens.focused_output(), None);
    }

    #[test]
    fn a_monitor_plugged_back_in_gets_its_workspace_back() {
        let mut screens = two();
        screens.focus(1);
        screens.show(3, 5);
        screens.focus(0);
        screens.remove(&"DP-2");
        // Meanwhile the laptop goes to workspace 2, which the monitor started on.
        screens.show(1, 5);
        screens.add("DP-2", 1920, 0, 5, None);
        assert_eq!(shown(&screens), vec![1, 3], "the monitor is back on 4");
    }

    #[test]
    fn the_lid_opening_puts_both_screens_back_as_they_were() {
        let mut screens = two();
        // The keyboard is on the laptop when its lid shuts: the monitor carries on its workspace.
        assert_eq!(screens.remove(&"eDP-1"), Some(0));
        assert!(screens.carry(0));
        assert_eq!(shown(&screens), vec![0]);
        screens.add("eDP-1", 0, 0, 5, None);
        assert_eq!(shown(&screens), vec![0, 1], "each back on its own");
        assert_eq!(
            screens.focused_output(),
            Some("DP-2"),
            "the lid opening doesn't move the keyboard off the screen in use"
        );
    }

    #[test]
    fn a_screen_that_moved_on_after_carrying_keeps_what_it_shows() {
        let mut screens = two();
        screens.remove(&"eDP-1");
        screens.carry(0);
        screens.show(4, 5);
        screens.add("eDP-1", 0, 0, 5, None);
        assert_eq!(shown(&screens), vec![0, 4]);
    }

    #[test]
    fn a_remembered_workspace_another_screen_has_taken_means_a_free_one() {
        let mut screens = two();
        screens.remove(&"DP-2");
        screens.show(1, 5);
        screens.add("DP-2", 1920, 0, 5, None);
        assert_eq!(shown(&screens), vec![1, 0]);
    }

    #[test]
    fn screens_follow_their_workspaces_through_an_edit_and_never_share_one() {
        let mut screens = two();
        // Workspace 2 (index 1) was deleted and its screen sent to index 0, which the first
        // screen is already showing.
        screens.remap(&[0, 0, 1, 2, 3], 4);
        assert_eq!(shown(&screens), vec![0, 1], "a free one instead");
        screens.show(9, 4);
        assert_eq!(
            screens.workspace(),
            3,
            "a workspace past the end is the last one"
        );
    }

    #[test]
    fn a_screen_can_show_every_workspace_there_is_past_the_fifth() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, 7, None);
        let show = screens.show(6, 7);
        assert!(show.changed);
        assert_eq!(screens.workspace(), 6, "the seventh, not the fifth");
        screens.add("DP-2", 1920, 0, 7, None);
        assert_eq!(
            screens.get(1).map(|s| s.workspace),
            Some(0),
            "the next screen takes the lowest free one"
        );
    }

    #[test]
    fn lighting_the_same_output_twice_moves_it_rather_than_doubling_it() {
        let mut screens = two();
        screens.add("DP-2", 3840, 0, 5, None);
        assert_eq!(screens.len(), 2);
        assert_eq!(screens.get(1).map(|s| s.x), Some(3840));
        // Moved to the other side, it keeps the keyboard it had.
        screens.focus(1);
        screens.add("DP-2", -1920, 0, 5, None);
        assert_eq!(screens.get(0).map(|s| s.output), Some("DP-2"));
        assert_eq!(screens.focused_output(), Some("DP-2"));
    }

    #[test]
    fn a_press_elsewhere_never_answers_a_card_on_the_focused_screen() {
        assert_eq!(press_elsewhere(Overlay::Card), PressElsewhere::Ignore);
        assert_eq!(press_elsewhere(Overlay::Panel), PressElsewhere::ClosePanels);
        assert_eq!(press_elsewhere(Overlay::None), PressElsewhere::MoveKeyboard);
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> crate::layout::Rect {
        crate::layout::Rect { x, y, w, h }
    }

    #[test]
    fn the_screen_beside_one_is_found_by_where_it_sits() {
        use crate::layout::Direction;
        // A panel with a monitor to its right, tops level.
        let row = [rect(0, 0, 1536, 960), rect(1536, 0, 1920, 1080)];
        assert_eq!(towards(&row, 0, Direction::Right), Some(1));
        assert_eq!(towards(&row, 1, Direction::Left), Some(0));
        assert_eq!(towards(&row, 0, Direction::Up), None);
        assert_eq!(towards(&row, 0, Direction::Down), None);
    }

    #[test]
    fn a_screen_above_the_panel_is_reached_by_going_up() {
        use crate::layout::Direction;
        // A monitor on a stand, the laptop below it.
        let stack = [rect(0, 1080, 1536, 960), rect(0, 0, 1920, 1080)];
        assert_eq!(towards(&stack, 0, Direction::Up), Some(1));
        assert_eq!(towards(&stack, 1, Direction::Down), Some(0));
        assert_eq!(towards(&stack, 0, Direction::Left), None);
        assert_eq!(towards(&stack, 0, Direction::Right), None);
    }

    #[test]
    fn a_screen_straight_ahead_beats_one_off_to_a_corner() {
        use crate::layout::Direction;
        // Two to the right: one level with the panel, one far above it and slightly nearer.
        let screens = [
            rect(0, 1000, 1536, 960),
            rect(1536, 1000, 1920, 1080),
            rect(1500, -2000, 800, 600),
        ];
        assert_eq!(towards(&screens, 0, Direction::Right), Some(1));
    }

    #[test]
    fn a_screen_takes_the_workspace_set_aside_for_it() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, 5, None);
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(0));
        // Six workspaces now, the sixth made for this screen: it takes that one rather than
        // workspace 2, which the laptop's Super+2 still wants.
        screens.add("DP-2", 1920, 0, 6, Some(5));
        let monitor = screens.index_of(&"DP-2").unwrap();
        assert_eq!(screens.get(monitor).map(|s| s.workspace), Some(5));
        assert_eq!(screens.showing(1), None, "workspace 2 is still nobody's");
    }

    #[test]
    fn what_a_screen_showed_before_beats_the_workspace_set_aside_for_it() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, 6, None);
        screens.add("DP-2", 1920, 0, 6, Some(5));
        screens.focus(screens.index_of(&"DP-2").unwrap());
        screens.show(3, 6);
        screens.remove(&"DP-2");
        // Plugged back in, it comes back to what it was showing, not to a new one.
        screens.add("DP-2", 1920, 0, 6, Some(5));
        let monitor = screens.index_of(&"DP-2").unwrap();
        assert_eq!(screens.get(monitor).map(|s| s.workspace), Some(3));
    }

    #[test]
    fn a_screen_falls_back_to_the_lowest_free_workspace() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, 5, None);
        // The workspace set aside for it is gone, or taken: the old rule still applies.
        screens.add("DP-2", 1920, 0, 5, Some(9));
        let monitor = screens.index_of(&"DP-2").unwrap();
        assert_eq!(screens.get(monitor).map(|s| s.workspace), Some(1));
    }

    #[test]
    fn trimming_forgets_workspaces_that_have_gone() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, 6, None);
        screens.add("DP-2", 1920, 0, 6, Some(5));
        screens.remove(&"DP-2");
        // Its workspace went with it, so nothing should bring the screen back to a sixth that
        // isn't there any more.
        screens.trim_homes(5);
        screens.add("DP-2", 1920, 0, 5, None);
        let monitor = screens.index_of(&"DP-2").unwrap();
        let workspace = screens.get(monitor).map(|s| s.workspace).unwrap();
        assert!(workspace < 5, "came back to workspace {workspace}");
    }

    #[test]
    fn workspaces_and_screens_agree_after_add_and_remove() {
        let count = 4;
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, 0, count, None);
        screens.add("DP-2", 1536, 0, count, None);
        // Every screen shows a workspace that exists, and no two show the same one.
        let check = |screens: &Screens<&str>| {
            let list = shown(screens);
            assert!(list.iter().all(|&workspace| workspace < count), "{list:?}");
            let mut unique = list.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), list.len(), "{list:?}");
        };
        check(&screens);
        // Each workspace can be gone to, from either screen.
        for from in 0..2 {
            for index in 0..count {
                screens.focus(from);
                screens.show(index, count);
                assert_eq!(screens.workspace(), index);
                check(&screens);
            }
        }
        // The focused screen going out: the other keeps its workspace and takes the keyboard.
        screens.focus(1);
        let other = screens.get(0).map(|screen| screen.workspace);
        assert!(screens.remove(&"DP-2").is_some());
        assert_eq!(screens.focused_output(), Some("eDP-1"));
        assert_eq!(screens.get(0).map(|screen| screen.workspace), other);
        // A screen that isn't focused going out leaves the focused one as it was.
        screens.add("DP-2", 1536, 0, count, None);
        check(&screens);
        let before = screens.workspace();
        screens.remove(&"DP-2");
        assert_eq!(screens.workspace(), before);
        // Fewer workspaces than screens after an edit: every screen still shows one that exists.
        screens.add("DP-2", 1536, 0, count, None);
        screens.remap(&[0, 0, 0, 0], 1);
        assert!(shown(&screens).iter().all(|&workspace| workspace == 0));
    }
}

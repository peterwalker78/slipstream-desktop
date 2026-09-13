//! The screens the desktop is spread across, and which workspace each one shows.
//!
//! Workspaces stay global (`workspace.rs`): a window lives on exactly one of them, wherever it is
//! drawn. A screen is a view on to one workspace at a time, so two screens are two workspaces
//! side by side, and no window has to be moved when a screen comes or goes — the workspace it is
//! on simply stops being looked at.
//!
//! Generic over the output type so the rules are unit-tested without Wayland.

/// One screen: an output, and the workspace it is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen<O> {
    pub output: O,
    /// Where it sits in the layout of screens, left edge first, in logical pixels.
    pub x: i32,
    /// 0-based.
    pub workspace: usize,
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
}

impl<O> Default for Screens<O> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            focused: 0,
        }
    }
}

impl<O: Clone + PartialEq> Screens<O> {
    /// Lights `output` at `x`, showing the lowest-numbered of the `count` workspaces that no other
    /// screen has. A second screen therefore comes up on workspace 2 rather than looking at the
    /// same desktop twice. The first screen to arrive takes the keyboard.
    pub fn add(&mut self, output: O, x: i32, count: usize) -> usize {
        if let Some(index) = self.index_of(&output) {
            let focused = self.focused_output();
            self.list[index].x = x;
            self.sort();
            // Moving a screen along doesn't move the keyboard off the one it's on.
            self.focused = focused
                .and_then(|output| self.index_of(&output))
                .unwrap_or(0);
            return self.index_of(&output).unwrap_or(index);
        }
        let workspace = (0..count.max(1))
            .find(|index| !self.list.iter().any(|screen| screen.workspace == *index))
            .unwrap_or(0);
        let focused = self.focused_output();
        self.list.push(Screen {
            output: output.clone(),
            x,
            workspace,
        });
        self.sort();
        // Sorting moves the others about; the keyboard stays where it was.
        self.focused = focused
            .and_then(|output| self.index_of(&output))
            .unwrap_or(0);
        self.index_of(&output).unwrap_or(0)
    }

    /// Puts a screen out (unplugged, or the lid shut on it). The keyboard moves to the nearest
    /// screen left of it. Returns the workspace it was showing, which nothing else is now.
    pub fn remove(&mut self, output: &O) -> Option<usize> {
        let index = self.index_of(output)?;
        let screen = self.list.remove(index);
        if self.focused >= index {
            self.focused = self.focused.saturating_sub(1);
        }
        self.focused = self.focused.min(self.list.len().saturating_sub(1));
        Some(screen.workspace)
    }

    /// The workspace list changed: `map` says where each old index went, and `count` is how many
    /// there are now. Every screen follows its workspace; if two would end up on the same one (a
    /// deleted workspace's screen landing where another already looks), the later takes the
    /// lowest one nothing is showing.
    pub fn remap(&mut self, map: &[usize], count: usize) {
        let count = count.max(1);
        let mut taken: Vec<usize> = Vec::new();
        for screen in &mut self.list {
            let mut to = map
                .get(screen.workspace)
                .copied()
                .unwrap_or(0)
                .min(count - 1);
            if taken.contains(&to) {
                to = (0..count).find(|free| !taken.contains(free)).unwrap_or(0);
            }
            taken.push(to);
            screen.workspace = to;
        }
    }

    fn sort(&mut self) {
        self.list.sort_by_key(|screen| screen.x);
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

    /// Where a workspace is drawn: the screen showing it, or the focused one when it is nowhere,
    /// which is where it would appear if it were switched to.
    pub fn screen_for_workspace(&self, index: usize) -> usize {
        self.showing(index).unwrap_or(self.focused)
    }

    /// Points the keyboard at `index`. Returns whether it moved.
    pub fn focus(&mut self, index: usize) -> bool {
        if index >= self.list.len() || index == self.focused {
            return false;
        }
        self.focused = index;
        true
    }

    /// Shows workspace `index` of `count` on the focused screen.
    ///
    /// A workspace is only ever on one screen, so if another one is showing it the two trade:
    /// the screen that had it takes the one being left behind. Nothing moves between workspaces,
    /// so no window changes hands — the two screens simply swap what they are looking at.
    pub fn show(&mut self, index: usize, count: usize) -> Show {
        let index = index.min(count.saturating_sub(1));
        let Some(from) = self.focused().map(|screen| screen.workspace) else {
            return Show::default();
        };
        if from == index {
            return Show::default();
        }
        let swapped = self.showing(index).filter(|other| *other != self.focused);
        if let Some(other) = swapped {
            self.list[other].workspace = from;
        }
        let focused = self.focused;
        self.list[focused].workspace = index;
        Show {
            changed: true,
            from,
            swapped,
        }
    }

    /// Shows `index` on the screen that already has it, moving the keyboard there instead of
    /// dragging the workspace across. Used by anything that goes to a window rather than to a
    /// workspace — Alt+Tab, the explorer, a click on a stream in the code rain.
    pub fn go_to(&mut self, index: usize, count: usize) -> Show {
        if let Some(other) = self.showing(index) {
            let moved = self.focus(other);
            return Show {
                changed: moved,
                from: self.workspace(),
                swapped: None,
            };
        }
        self.show(index, count)
    }
}

/// What a switch did.
#[derive(Debug, Default, PartialEq)]
pub struct Show {
    /// Whether anything changed at all.
    pub changed: bool,
    /// The workspace the focused screen was showing.
    pub from: usize,
    /// The other screen that traded workspaces with it, if there was one.
    pub swapped: Option<usize>,
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
        screens.add("eDP-1", 0, 5);
        screens.add("DP-2", 1920, 5);
        screens
    }

    #[test]
    fn a_new_screen_takes_a_workspace_nothing_else_is_showing() {
        let screens = two();
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(0));
        assert_eq!(screens.get(1).map(|s| s.workspace), Some(1));
        assert_eq!(screens.focused_index(), 0, "the keyboard stays put");
    }

    #[test]
    fn screens_are_kept_in_the_order_they_sit_in() {
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 1920, 5);
        screens.add("DP-2", 0, 5);
        assert_eq!(screens.get(0).map(|s| s.output), Some("DP-2"));
        assert_eq!(
            screens.focused_output(),
            Some("eDP-1"),
            "a screen appearing to the left doesn't steal the keyboard"
        );
    }

    #[test]
    fn switching_to_a_workspace_another_screen_has_trades_them() {
        let mut screens = two();
        let show = screens.show(1, 5);
        assert_eq!(
            show,
            Show {
                changed: true,
                from: 0,
                swapped: Some(1)
            }
        );
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(1));
        assert_eq!(screens.get(1).map(|s| s.workspace), Some(0));
        assert!(!screens.show(1, 5).changed, "already there");
    }

    #[test]
    fn switching_to_a_workspace_no_one_has_just_shows_it() {
        let mut screens = two();
        let show = screens.show(3, 5);
        assert_eq!(show.swapped, None);
        assert_eq!(screens.workspace(), 3);
        assert_eq!(screens.get(1).map(|s| s.workspace), Some(1), "untouched");
    }

    #[test]
    fn going_to_a_window_moves_the_keyboard_rather_than_the_workspace() {
        let mut screens = two();
        // Workspace 2 is on the right-hand screen: go there instead of dragging it left.
        assert!(screens.go_to(1, 5).changed);
        assert_eq!(screens.focused_index(), 1);
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(0), "untouched");
        assert_eq!(screens.get(1).map(|s| s.workspace), Some(1));
    }

    #[test]
    fn the_keyboard_is_pointed_at_a_screen_that_is_there() {
        let mut screens = two();
        assert!(screens.focus(1));
        assert!(!screens.focus(1), "already there");
        assert!(!screens.focus(7), "no such screen");
        assert_eq!(screens.focused_index(), 1);
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
    fn a_screen_that_comes_back_takes_a_free_workspace_again() {
        let mut screens = two();
        screens.remove(&"eDP-1");
        assert_eq!(screens.len(), 1);
        assert_eq!(
            screens.workspace(),
            1,
            "the external screen's own workspace"
        );
        screens.add("eDP-1", 0, 5);
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(0));
        assert_eq!(
            screens.focused_output(),
            Some("DP-2"),
            "the lid opening doesn't move the keyboard off the screen in use"
        );
    }

    #[test]
    fn screens_follow_their_workspaces_through_an_edit_and_never_share_one() {
        let mut screens = two();
        // Workspace 2 (index 1) was deleted and its screen sent to index 0, which the first
        // screen is already showing.
        screens.remap(&[0, 0, 1, 2, 3], 4);
        assert_eq!(screens.get(0).map(|s| s.workspace), Some(0));
        assert_eq!(
            screens.get(1).map(|s| s.workspace),
            Some(1),
            "a free one instead"
        );
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
        screens.add("eDP-1", 0, 7);
        let show = screens.show(6, 7);
        assert!(show.changed);
        assert_eq!(screens.workspace(), 6, "the seventh, not the fifth");
        screens.add("DP-2", 1920, 7);
        assert_eq!(
            screens.get(1).map(|s| s.workspace),
            Some(0),
            "the next screen takes the lowest free one"
        );
    }

    #[test]
    fn lighting_the_same_output_twice_moves_it_rather_than_doubling_it() {
        let mut screens = two();
        screens.add("DP-2", 3840, 5);
        assert_eq!(screens.len(), 2);
        assert_eq!(screens.get(1).map(|s| s.x), Some(3840));
        // Moved to the other side, it keeps the keyboard it had.
        screens.focus(1);
        screens.add("DP-2", -1920, 5);
        assert_eq!(screens.get(0).map(|s| s.output), Some("DP-2"));
        assert_eq!(screens.focused_output(), Some("DP-2"));
    }

    #[test]
    fn a_press_elsewhere_never_answers_a_card_on_the_focused_screen() {
        assert_eq!(press_elsewhere(Overlay::Card), PressElsewhere::Ignore);
        assert_eq!(press_elsewhere(Overlay::Panel), PressElsewhere::ClosePanels);
        assert_eq!(press_elsewhere(Overlay::None), PressElsewhere::MoveKeyboard);
    }

    #[test]
    fn workspaces_and_screens_agree_after_add_and_remove() {
        let count = 4;
        let mut screens: Screens<&str> = Screens::default();
        screens.add("eDP-1", 0, count);
        screens.add("DP-2", 1536, count);
        let shown = |screens: &Screens<&str>| -> Vec<usize> {
            screens.iter().map(|screen| screen.workspace).collect()
        };
        // Every screen shows a workspace that exists, and no two show the same one.
        let check = |screens: &Screens<&str>| {
            let list = shown(screens);
            assert!(list.iter().all(|&workspace| workspace < count), "{list:?}");
            let mut unique = list.clone();
            unique.dedup();
            assert_eq!(unique.len(), list.len(), "{list:?}");
        };
        check(&screens);
        // Each workspace can be shown on the focused screen.
        for index in 0..count {
            screens.show(index, count);
            assert_eq!(screens.workspace(), index);
            check(&screens);
        }
        // The focused screen going out: the other keeps its workspace and takes the keyboard.
        screens.focus(1);
        let other = screens.get(0).map(|screen| screen.workspace);
        assert!(screens.remove(&"DP-2").is_some());
        assert_eq!(screens.focused_output(), Some("eDP-1"));
        assert_eq!(screens.get(0).map(|screen| screen.workspace), other);
        // A screen that isn't focused going out leaves the focused one as it was.
        screens.add("DP-2", 1536, count);
        let before = screens.workspace();
        screens.remove(&"DP-2");
        assert_eq!(screens.workspace(), before);
        check(&screens);
        // Fewer workspaces than screens after an edit: every screen still shows one that exists.
        screens.add("DP-2", 1536, count);
        screens.remap(&[0, 0, 0, 0], 1);
        assert!(shown(&screens).iter().all(|&workspace| workspace == 0));
    }
}

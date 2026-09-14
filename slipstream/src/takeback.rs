//! Apps that take the mouse or the keyboard shortcuts, and the one key that takes them back.
//!
//! Games lock the pointer in place and read relative motion for mouse look, and some confine it
//! to their window (`zwp_pointer_constraints_v1`). Virtual machines and remote desktops ask for
//! every key, Super and Alt+Tab included, to reach them (`zwp_keyboard_shortcuts_inhibit`). Both
//! are granted only to the window with the keyboard, and only while nothing of Slipstream's own
//! (a panel, bullet time, the lock) is up. The first time a window takes either, a toast says so
//! and names the way back.
//!
//! Super+Esc takes both back from the focused window: the pointer moves freely and bindings work
//! again. The window stays refused until Super+Esc is pressed on it again or it loses the
//! keyboard, so a game can't grab the pointer straight back on its next frame.

use smithay::{
    desktop::Window,
    reexports::wayland_server::{Resource, Weak, protocol::wl_surface::WlSurface},
    utils::{Logical, Point},
    wayland::{
        compositor::get_parent, keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::with_pointer_constraint, seat::WaylandFocus,
    },
};

use crate::{
    focus::KeyboardFocus,
    keys::{self, Action},
    state::Slipstream,
};

/// What a window has taken, for its toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Pointer,
    Shortcuts,
}

/// Which windows have been told about, and the window whose grabs were taken back.
#[derive(Debug, Default)]
pub struct TakeBack {
    told: Vec<(Weak<WlSurface>, Taken)>,
    refused: Option<Weak<WlSurface>>,
}

impl TakeBack {
    /// Whether `taken` from the window whose surface is `root` still needs its toast, marking it
    /// told either way.
    pub fn first_time(&mut self, root: &WlSurface, taken: Taken) -> bool {
        self.told.retain(|(surface, _)| surface.upgrade().is_ok());
        let seen = self
            .told
            .iter()
            .any(|(surface, kind)| *kind == taken && surface.upgrade().ok().as_ref() == Some(root));
        if !seen {
            self.told.push((root.downgrade(), taken));
        }
        !seen
    }

    pub fn is_refused(&self, root: &WlSurface) -> bool {
        self.refused
            .as_ref()
            .and_then(|surface| surface.upgrade().ok())
            .is_some_and(|surface| surface == *root)
    }

    pub fn refuse(&mut self, root: Option<&WlSurface>) {
        self.refused = root.map(|root| root.downgrade());
    }
}

/// The window's own surface for any surface in it: subsurfaces belong to their parent.
pub fn root_of(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl Slipstream {
    /// The focused window's own surface, if it may hold grabs at all right now.
    fn grab_holder(&self) -> Option<(Window, WlSurface)> {
        if self.pointer_blocked() || self.lock.is_some() {
            return None;
        }
        let window = self.focused_window()?;
        let root = window.wl_surface()?.into_owned();
        (!self.takeback.is_refused(&root)).then_some((window, root))
    }

    /// Where the pointer is aimed, and the origin of that surface, when it is over the window
    /// allowed to hold grabs.
    fn constrainable(&self) -> Option<(WlSurface, Point<f64, Logical>, Window)> {
        let (window, root) = self.grab_holder()?;
        let pointer = self.seat.get_pointer()?;
        let (focus, origin) = self.pointer_target(pointer.current_location())?;
        let surface = focus.wl_surface()?.into_owned();
        (root_of(&surface) == root).then_some((surface, origin, window))
    }

    /// The constraint on the surface under the pointer, if one applies: whether it locks, and
    /// the confining region's test, in the surface's own coordinates. Motion reads this before
    /// it moves the pointer.
    pub fn active_constraint(&self) -> Option<Constraint> {
        let (surface, origin, _) = self.constrainable()?;
        let pointer = self.seat.get_pointer()?;
        let here = pointer.current_location();
        with_pointer_constraint(&surface, &pointer, |constraint| {
            let constraint = constraint.filter(|constraint| constraint.is_active())?;
            let inside = |point: Point<f64, Logical>| {
                constraint
                    .region()
                    .is_none_or(|region| region.contains((point - origin).to_i32_round()))
            };
            // A constraint only holds while the pointer is inside its region.
            if !inside(here) {
                return None;
            }
            Some(match &*constraint {
                smithay::wayland::pointer_constraints::PointerConstraint::Locked(_) => {
                    Constraint::Locked
                }
                smithay::wayland::pointer_constraints::PointerConstraint::Confined(confined) => {
                    Constraint::Confined {
                        region: confined.region().cloned(),
                        origin,
                        surface: surface.clone(),
                    }
                }
            })
        })
    }

    /// Turns on a waiting constraint under the pointer once it may hold, and turns off one that
    /// no longer may. Runs after every pointer motion, focus change and new constraint.
    pub fn update_pointer_constraint(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let here = pointer.current_location();
        let Some((surface, origin, window)) = self.constrainable() else {
            // Whatever the pointer is over may not hold it: let go of anything held there.
            if let Some(surface) = pointer
                .current_focus()
                .and_then(|f| f.wl_surface().map(|s| s.into_owned()))
            {
                with_pointer_constraint(&surface, &pointer, |constraint| {
                    if let Some(constraint) = constraint.filter(|c| c.is_active()) {
                        constraint.deactivate();
                    }
                });
            }
            return;
        };
        let activated = with_pointer_constraint(&surface, &pointer, |constraint| {
            let Some(constraint) = constraint.filter(|c| !c.is_active()) else {
                return false;
            };
            let point = (here - origin).to_i32_round();
            if constraint
                .region()
                .is_none_or(|region| region.contains(point))
            {
                constraint.activate();
                return true;
            }
            false
        });
        if activated {
            tracing::info!(
                app = self.window_name(&window),
                "pointer held by the window"
            );
            self.tell_taken(&window, &root_of(&surface), Taken::Pointer);
        }
    }

    /// The focused window's shortcut inhibitor, on if it may hold one and off otherwise.
    pub fn update_shortcuts_inhibitor(&mut self) {
        let holder = self.grab_holder();
        let focus = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()));
        let Some(surface) = focus else {
            return;
        };
        let Some(inhibitor) = self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface) else {
            return;
        };
        match holder {
            Some((window, root)) if root == surface => {
                if !inhibitor.is_active() {
                    inhibitor.activate();
                    tracing::info!(
                        app = self.window_name(&window),
                        "shortcuts held by the window"
                    );
                    self.tell_taken(&window, &root, Taken::Shortcuts);
                }
            }
            _ => {
                if inhibitor.is_active() {
                    inhibitor.inactivate();
                }
            }
        }
    }

    /// Whether the keys should go straight to the focused app, bindings and all.
    pub fn shortcuts_inhibited(&self) -> bool {
        self.seat.keyboard_shortcuts_inhibited()
    }

    fn tell_taken(&mut self, window: &Window, root: &WlSurface, taken: Taken) {
        if !self.takeback.first_time(root, taken) {
            return;
        }
        let name = self.window_name(window);
        let key = self.take_back_label();
        let (title, body) = match taken {
            Taken::Pointer => (format!("{name} has the mouse"), format!("{key} frees it")),
            Taken::Shortcuts => (
                format!("{name} has the keyboard shortcuts"),
                format!("{key} takes them back"),
            ),
        };
        self.show_toast(&title, &body);
    }

    /// The key that takes grabs back, as the bindings have it.
    pub fn take_back_label(&self) -> String {
        self.bindings
            .iter()
            .find(|binding| binding.action == Action::TakeBack)
            .map(|binding| keys::label(binding.mods, binding.key))
            .unwrap_or_else(|| "Super+Esc".to_string())
    }

    /// Super+Esc: takes the pointer and the shortcuts back from the focused window, or, pressed
    /// on a window they were taken from, lets it have them again.
    pub fn take_back(&mut self) {
        let Some(window) = self.focused_window() else {
            return;
        };
        let Some(root) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return;
        };
        let name = self.window_name(&window);
        if self.takeback.is_refused(&root) {
            self.takeback.refuse(None);
            self.update_shortcuts_inhibitor();
            self.update_pointer_constraint();
            self.show_toast(&format!("{name} may take the mouse and keys again"), "");
            return;
        }
        let pointer = self.seat.get_pointer().unwrap();
        let mut held = false;
        if let Some(focus) = pointer.current_focus() {
            if let Some(surface) = focus.wl_surface() {
                if root_of(&surface) == root {
                    with_pointer_constraint(&surface, &pointer, |constraint| {
                        if let Some(constraint) = constraint.filter(|c| c.is_active()) {
                            held = true;
                            constraint.deactivate();
                        }
                    });
                }
            }
        }
        if let Some(inhibitor) = self.seat.keyboard_shortcuts_inhibitor_for_surface(&root) {
            if inhibitor.is_active() {
                held = true;
                inhibitor.inactivate();
            }
        }
        if held {
            self.takeback.refuse(Some(&root));
            let key = self.take_back_label();
            self.show_toast(
                "Mouse and keys are back",
                &format!("{key} again gives them to {name}"),
            );
        } else {
            self.show_toast(&format!("{name} isn't holding the mouse or keys"), "");
        }
    }

    /// The keyboard moved to another window: grabs follow the rules for the new one, and a
    /// window that was refused may ask again once it has the keyboard back.
    pub fn grabs_after_focus_change(&mut self, focused: Option<&KeyboardFocus>) {
        let root = focused.and_then(|focus| focus.wl_surface().map(|s| s.into_owned()));
        let still_refused = root
            .as_ref()
            .is_some_and(|root| self.takeback.is_refused(root));
        if !still_refused {
            self.takeback.refuse(None);
        }
        self.update_shortcuts_inhibitor();
        self.update_pointer_constraint();
    }
}

/// How the pointer is held over the window under it.
pub enum Constraint {
    /// It doesn't move; only relative motion reaches the app.
    Locked,
    /// It moves, but not out of `region` (the whole surface when `None`).
    Confined {
        region: Option<smithay::wayland::compositor::RegionAttributes>,
        origin: Point<f64, Logical>,
        surface: WlSurface,
    },
}

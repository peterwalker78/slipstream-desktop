//! Looking at the window that holds a process (`look.rs` has the bus side and the rules): the
//! window comes into view, raised and ringed, while the keyboard stays with the window that
//! asked, and then either the view goes back or the looked-at window takes the keyboard.

use std::{path::Path, time::Instant};

use super::*;
use crate::look::{self, Event, Facts, Refusal};

/// A look in progress.
#[derive(Debug, Clone)]
pub struct Look {
    /// The window that asked, which keeps the keyboard throughout.
    pub caller: Window,
    /// The window being shown, drawn with a ring.
    pub target: Window,
    /// The workspace the focused screen showed before the view moved, to go back to; `None`
    /// when the target was already in view.
    pub from: Option<usize>,
    /// The target was in the code rain, and goes back into it when the view does.
    pub from_rain: bool,
    pub started: Instant,
}

impl Slipstream {
    pub fn look_event(&mut self, event: Event) {
        match event {
            Event::Start { caller, pid, reply } => {
                let answer = self.start_look(&caller, pid);
                let _ = reply.try_send(answer);
            }
            Event::End {
                caller,
                stay,
                reply,
            } => {
                let allowed = self.look.is_some() && self.owns_keyboard_window(&caller);
                let _ = reply.try_send(allowed && self.end_look(stay));
            }
        }
    }

    /// The window holding the keyboard, wherever it is: on screen, or out of view while it
    /// looks at another.
    pub fn keyboard_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.all_open_windows()
            .into_iter()
            .find(|window| focus.is_window(window))
    }

    /// The window holding the keyboard's process and its ancestors, nearest first.
    fn keyboard_lineage(&self) -> Vec<u32> {
        self.keyboard_window()
            .and_then(|window| self.window_pid(&window))
            .map(|pid| crate::notify::lineage(Path::new("/proc"), pid))
            .unwrap_or_default()
    }

    fn owns_keyboard_window(&self, caller: &[u32]) -> bool {
        let compositor = crate::notify::lineage(Path::new("/proc"), std::process::id());
        look::owns(caller, &self.keyboard_lineage(), &compositor)
    }

    /// Shows the window above process `pid`, for `caller` (the asking process and its
    /// ancestors). A look already going is replaced, and the view still goes back to where the
    /// first one started.
    pub fn start_look(&mut self, caller: &[u32], pid: u32) -> Result<(), Refusal> {
        let compositor = crate::notify::lineage(Path::new("/proc"), std::process::id());
        let holder = self.keyboard_lineage();
        let checked = look::may_start(self.lock.is_some(), caller, &holder, &compositor)
            .and_then(|()| self.keyboard_window().ok_or(Refusal::NotInUse))
            .and_then(|caller_window| {
                let target = self
                    .window_for_pids(&crate::notify::lineage(Path::new("/proc"), pid))
                    .ok_or(Refusal::NoWindow)?;
                Ok((caller_window, target))
            });
        let (caller_window, target) = match checked {
            Ok(found) => found,
            Err(refusal) => {
                tracing::info!(pid, ?refusal, "a look was refused");
                return Err(refusal);
            }
        };
        // Set before anything moves, so the asking window stays active while it is out of view.
        let earlier = self.look.replace(Look {
            caller: caller_window,
            target: target.clone(),
            from: None,
            from_rain: false,
            started: Instant::now(),
        });
        let mut from = earlier.as_ref().and_then(|earlier| earlier.from);
        let mut from_rain = false;
        if let Some(earlier) = earlier {
            if earlier.target == target {
                from_rain = earlier.from_rain;
            } else {
                self.back_to_rain(&earlier);
            }
        }
        if self.rain.contains(&target) {
            from_rain = self.condense(std::slice::from_ref(&target)).is_some();
        } else if let Some(index) = self.workspaces.find(&target)
            && self.screens.showing(index).is_none()
        {
            let was = self.active_workspace();
            if self.show_workspace(index).is_some() {
                from = from.or(Some(was));
            }
        }
        self.raise_looked_at(&target);
        if let Some(look) = self.look.as_mut() {
            look.from = from;
            look.from_rain = from_rain;
        }
        tracing::info!(
            window = logged_app(&target),
            from_rain,
            "looking at a window"
        );
        Ok(())
    }

    /// In front of its neighbours, without the keyboard.
    fn raise_looked_at(&mut self, target: &Window) {
        if let Some(index) = self.workspaces.find(target) {
            self.workspaces
                .get_mut(index)
                .floating
                .raise(target, is_child_of);
            self.restack_floating(index);
        }
        if self.space.element_location(target).is_some() {
            self.space.raise_element(target, false);
        }
    }

    /// Ends the look: with `stay` the looked-at window takes the keyboard, as activating it does;
    /// otherwise the view goes back to where it was. False when no look was going.
    pub fn end_look(&mut self, stay: bool) -> bool {
        let Some(look) = self.look.take() else {
            return false;
        };
        if stay && look.target.alive() {
            self.activate_window(&look.target);
            tracing::info!(
                window = logged_app(&look.target),
                "stayed with the window looked at"
            );
        } else {
            self.go_back(&look);
        }
        self.finish_look(&look, stay);
        true
    }

    /// The view goes back to where the look started, and a window brought out of the rain for
    /// it goes back in.
    fn go_back(&mut self, look: &Look) {
        self.back_to_rain(look);
        if let Some(from) = look.from
            && from < self.workspaces.count()
            && self.active_workspace() != from
        {
            self.show_workspace(from);
        }
        tracing::info!("back from looking at a window");
    }

    fn back_to_rain(&mut self, look: &Look) {
        if look.from_rain && look.target.alive() && self.take_into_rain(&look.target) {
            self.pour_into_stream(&look.target, POUR);
            self.retile();
        }
    }

    /// What follows every end: the ring goes with `self.look`, the asking window is told it
    /// isn't active if the keyboard has left it, and the bus hears.
    fn finish_look(&mut self, look: &Look, stayed: bool) {
        if self.keyboard_window().as_ref() != Some(&look.caller) {
            deactivate(&look.caller);
        }
        look::send_ended(stayed);
    }

    /// Once a pass: a look ends by itself when the keyboard leaves the window that asked, the
    /// window looked at closes, the screen locks, or it has gone on too long.
    pub fn tick_look(&mut self) {
        let Some(look) = self.look.as_ref() else {
            return;
        };
        let facts = Facts {
            caller_has_keyboard: look.caller.alive()
                && self.keyboard_window().as_ref() == Some(&look.caller),
            target_open: look.target.alive() && self.all_open_windows().contains(&look.target),
            locked: self.lock.is_some(),
            elapsed: look.started.elapsed(),
        };
        let Some(why) = look::ended(facts) else {
            return;
        };
        let Some(look) = self.look.take() else {
            return;
        };
        tracing::info!(?why, "a look ended by itself");
        if why.goes_back() {
            self.go_back(&look);
        }
        self.finish_look(&look, false);
    }

    /// The window being looked at, for its ring.
    pub fn looked_at(&self) -> Option<&Window> {
        self.look.as_ref().map(|look| &look.target)
    }
}

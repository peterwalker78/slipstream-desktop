//! Variable refresh rate: a screen that waits for the next frame instead of refreshing on a fixed
//! beat. A game that misses a refresh is shown the moment its frame is ready, rather than a whole
//! refresh late. The desktop itself stays on the fixed beat by default, since a rate that follows
//! whatever happens to be drawing makes the pointer and animations judder.

use slipstream_config::VariableRefresh;
use smithay::{
    backend::renderer::element::{Id, RenderElementPresentationState, RenderElementStates},
    desktop::Window,
};

/// Frames a fullscreen game may be drawn through the compositor, rather than scanned out, before
/// the screen goes back to its fixed rate. A notification or the volume popup over the game for a
/// moment shouldn't switch the rate back and forth, which some screens show as a flicker in
/// brightness.
const GRACE_FRAMES: u32 = 60;

/// One screen's variable refresh, frame to frame.
#[derive(Debug, Default)]
pub struct Adaptive {
    on: bool,
    /// Frames in a row the fullscreen window has been composited while it was on.
    composited: u32,
}

impl Adaptive {
    /// Whether the next frame should vary its refresh. `capable`: the screen supports it and it
    /// can be switched without a modeset. `fullscreen`: a window fills the screen. `scanned_out`:
    /// the last frame put that window's buffer straight on the screen.
    pub fn next(
        &mut self,
        setting: VariableRefresh,
        capable: bool,
        fullscreen: bool,
        scanned_out: bool,
    ) -> bool {
        let on = match setting {
            _ if !capable => false,
            VariableRefresh::Off => false,
            VariableRefresh::Always => true,
            VariableRefresh::Games if fullscreen && scanned_out => {
                self.composited = 0;
                true
            }
            VariableRefresh::Games => {
                fullscreen && self.on && {
                    self.composited += 1;
                    self.composited <= GRACE_FRAMES
                }
            }
        };
        if !on {
            self.composited = 0;
        }
        self.on = on;
        on
    }
}

/// Whether the frame just drawn put `window`'s buffer straight on the screen, skipping the
/// compositor's own drawing.
pub fn scanned_out(window: &Window, states: &RenderElementStates) -> bool {
    let mut direct = false;
    window.with_surfaces(|surface, _| {
        direct |= states
            .element_render_state(Id::from_wayland_resource(surface))
            .is_some_and(|state| {
                matches!(
                    state.presentation_state,
                    RenderElementPresentationState::ZeroCopy
                )
            });
    });
    direct
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fullscreen_game_scanned_out_turns_it_on() {
        let mut vrr = Adaptive::default();
        assert!(vrr.next(VariableRefresh::Games, true, true, true));
    }

    #[test]
    fn the_desktop_keeps_a_fixed_rate_for_games() {
        let mut vrr = Adaptive::default();
        assert!(!vrr.next(VariableRefresh::Games, true, false, false));
        // A fullscreen window the compositor has to draw itself (a video under a translucent
        // popup, say) doesn't turn it on.
        assert!(!vrr.next(VariableRefresh::Games, true, true, false));
    }

    #[test]
    fn a_screen_that_cant_never_turns_it_on() {
        let mut vrr = Adaptive::default();
        assert!(!vrr.next(VariableRefresh::Always, false, true, true));
        assert!(!vrr.next(VariableRefresh::Games, false, true, true));
    }

    #[test]
    fn off_and_always_ignore_what_is_on_screen() {
        let mut vrr = Adaptive::default();
        assert!(!vrr.next(VariableRefresh::Off, true, true, true));
        assert!(vrr.next(VariableRefresh::Always, true, false, false));
    }

    #[test]
    fn a_moment_drawn_over_the_game_keeps_it_on() {
        let mut vrr = Adaptive::default();
        assert!(vrr.next(VariableRefresh::Games, true, true, true));
        for _ in 0..GRACE_FRAMES {
            assert!(vrr.next(VariableRefresh::Games, true, true, false));
        }
        assert!(vrr.next(VariableRefresh::Games, true, true, true));
        // The count starts again once the game is scanned out again.
        for _ in 0..GRACE_FRAMES {
            assert!(vrr.next(VariableRefresh::Games, true, true, false));
        }
        assert!(!vrr.next(VariableRefresh::Games, true, true, false));
        // And once off, compositing alone doesn't bring it back.
        assert!(!vrr.next(VariableRefresh::Games, true, true, false));
    }

    #[test]
    fn leaving_fullscreen_turns_it_off_at_once() {
        let mut vrr = Adaptive::default();
        assert!(vrr.next(VariableRefresh::Games, true, true, true));
        assert!(!vrr.next(VariableRefresh::Games, true, false, false));
    }
}

//! Telling clients about frames: which were shown, how, and when to draw the next.

use super::*;

/// Everything on `output`, front to back. `cursor` is `None` when a host desktop draws the
/// pointer (nested).
impl Slipstream {
    /// Frame callbacks after `output` has drawn: to every window in the space, and to the windows
    /// drawn outside it in that frame, which bullet time shows live. Nothing else hidden gets
    /// them, so windows on workspaces out of sight stay cheap.
    /// Who wants to hear when this frame reaches `output`: every window on it whose surfaces
    /// were drawn, per `states`.
    pub fn presentation_feedback(
        &self,
        output: &Output,
        states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        let mut feedback = OutputPresentationFeedback::new(output);
        for window in self.space.elements() {
            if !self.space.outputs_for_element(window).contains(output) {
                continue;
            }
            window.take_presentation_feedback(
                &mut feedback,
                |_, _| Some(output.clone()),
                |surface, _| surface_presentation_feedback_flags_from_states(surface, None, states),
            );
        }
        for layer in smithay::desktop::layer_map_for_output(output).layers() {
            layer.take_presentation_feedback(
                &mut feedback,
                |_, _| Some(output.clone()),
                |surface, _| surface_presentation_feedback_flags_from_states(surface, None, states),
            );
        }
        feedback
    }

    /// After drawing a screen: notes, per surface, which screen it is really being shown on.
    /// `send_frames` then paces each window by that screen's refresh rate rather than by whichever
    /// screen happened to draw last. Without this a window on a 60 Hz panel is woken at 60 plus
    /// 144 Hz when a 144 Hz monitor is plugged in, and a client that draws on every callback runs
    /// at the sum of every screen's rate.
    pub fn update_scanout_outputs(&mut self, output: &Output, states: &RenderElementStates) {
        use smithay::{
            backend::renderer::element::default_primary_scanout_output_compare,
            desktop::utils::update_surface_primary_scanout_output,
        };
        // Windows drawn from workspaces no screen is showing (bullet time, a window being shared)
        // count too: they are on this screen's frame, so this screen should pace them.
        let off_space = self.drawn_off_space.clone();
        for window in self.space.elements().chain(off_space.iter()) {
            window.with_surfaces(|surface, surface_states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    surface_states,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        for layer in smithay::desktop::layer_map_for_output(output).layers() {
            layer.with_surfaces(|surface, surface_states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    surface_states,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
        }
    }

    pub fn send_frames(&mut self, output: &Output) {
        use smithay::desktop::utils::surface_primary_scanout_output;
        let now = self.start_time.elapsed();
        let off_space = std::mem::take(&mut self.drawn_off_space);
        // A surface this screen isn't showing still gets a callback this often, so a window that
        // is hidden or fully covered carries on rather than freezing until it is looked at again.
        let throttle = Some(Duration::from_secs(1));
        let mut sent: Vec<&Window> = Vec::new();
        for window in self.space.elements().chain(off_space.iter()) {
            if sent.contains(&window) {
                continue;
            }
            window.send_frame(output, now, throttle, surface_primary_scanout_output);
            sent.push(window);
        }
        self.send_layer_frames(output, now);
    }
}

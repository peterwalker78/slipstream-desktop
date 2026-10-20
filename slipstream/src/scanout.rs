//! Telling a fullscreen window which buffers its screen can show directly. A game that allocates
//! from the scanout tranche gets its frames put straight on the screen, skipping the compositor's
//! own drawing, which is also what lets variable refresh follow it.

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::{
    backend::{
        allocator::format::FormatSet,
        renderer::element::{Id, RenderElementStates, utils::select_dmabuf_feedback},
    },
    desktop::{Window, utils::surface_primary_scanout_output},
    output::Output,
    reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags,
    wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder},
};

use crate::state::Slipstream;

/// The two feedbacks a screen hands out: the renderer's formats for everything it composites,
/// and the same with the screen's own formats preferred for a fullscreen window.
#[derive(Debug)]
pub struct Feedback {
    render: DmabufFeedback,
    scanout: DmabufFeedback,
}

impl Feedback {
    /// `main_device` and `render_formats` are the renderer's, as in the default feedback.
    /// `screen_device` is the DRM device lighting the screen and `plane_formats` its primary
    /// plane's. The scanout tranche keeps only formats the renderer can also read, so a buffer
    /// that can't go on the plane after all can still be drawn.
    pub fn new(
        main_device: libc::dev_t,
        render_formats: FormatSet,
        screen_device: libc::dev_t,
        plane_formats: &FormatSet,
    ) -> std::io::Result<Self> {
        let both: FormatSet = plane_formats
            .intersection(&render_formats)
            .copied()
            .collect();
        let builder = DmabufFeedbackBuilder::new(main_device, render_formats);
        Ok(Self {
            render: builder.clone().build()?,
            scanout: builder
                .add_preference_tranche(screen_device, TrancheFlags::Scanout, both, 4u32..=6)
                .build()?,
        })
    }

    /// The feedback for one surface, from how the last frame showed it. Only a fullscreen
    /// window's surfaces are ever offered the screen's formats: that is the only place a
    /// window's buffer can stand in for the whole frame.
    pub fn pick(
        &self,
        fullscreen: bool,
        surface: Id,
        states: &RenderElementStates,
    ) -> &DmabufFeedback {
        match fullscreen {
            true => select_dmabuf_feedback(surface, states, &self.render, &self.scanout),
            false => &self.render,
        }
    }

    fn is_scanout(&self, feedback: &DmabufFeedback) -> bool {
        std::ptr::eq(feedback, &self.scanout)
    }
}

/// Marks a surface that has been offered the scanout tranche, so it's logged once.
#[derive(Default)]
struct Offered(AtomicBool);

impl Slipstream {
    /// After `output` has drawn a frame: updates the feedback of every window on it that asked
    /// for some.
    pub fn send_dmabuf_feedback(
        &self,
        output: &Output,
        fullscreen: Option<&Window>,
        states: &RenderElementStates,
        feedback: &Feedback,
    ) {
        for window in self.space.elements() {
            let filling = fullscreen == Some(window);
            window.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, data| {
                let chosen = feedback.pick(filling, Id::from_wayland_resource(surface), states);
                if feedback.is_scanout(chosen) {
                    data.data_map.insert_if_missing_threadsafe(Offered::default);
                    let offered = data.data_map.get::<Offered>().expect("just inserted");
                    if !offered.0.swap(true, Ordering::Relaxed) {
                        tracing::info!(
                            output = output.name(),
                            "scanout: offered a fullscreen window buffers the screen can show directly"
                        );
                    }
                }
                chosen
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use smithay::backend::{
        allocator::{Format, Fourcc, Modifier},
        renderer::element::{RenderElementPresentationState, RenderElementState, RenderingReason},
    };

    use super::*;

    fn formats() -> FormatSet {
        [Fourcc::Argb8888, Fourcc::Xrgb8888]
            .into_iter()
            .map(|code| Format {
                code,
                modifier: Modifier::Linear,
            })
            .collect()
    }

    fn feedback() -> Feedback {
        Feedback::new(1, formats(), 2, &formats()).expect("a format table")
    }

    fn shown(presentation_state: RenderElementPresentationState) -> (Id, RenderElementStates) {
        let id = Id::new();
        let state = RenderElementState {
            visible_area: 100,
            presentation_state,
            needs_capture: false,
        };
        let states = RenderElementStates {
            states: HashMap::from([(id.clone(), state)]),
        };
        (id, states)
    }

    #[test]
    fn a_fullscreen_window_on_the_plane_keeps_the_screens_formats() {
        let feedback = feedback();
        let (id, states) = shown(RenderElementPresentationState::ZeroCopy);
        assert!(feedback.is_scanout(feedback.pick(true, id, &states)));
    }

    #[test]
    fn a_fullscreen_window_in_the_wrong_format_is_offered_the_screens() {
        let feedback = feedback();
        for reason in [
            RenderingReason::FormatUnsupported,
            RenderingReason::ScanoutFailed,
        ] {
            let (id, states) = shown(RenderElementPresentationState::Rendering {
                reason: Some(reason),
            });
            assert!(feedback.is_scanout(feedback.pick(true, id, &states)));
        }
    }

    #[test]
    fn composited_on_purpose_gets_the_renderers_formats() {
        // Recording, or something drawn over it: the compositor chose to draw it.
        let feedback = feedback();
        let (id, states) = shown(RenderElementPresentationState::Rendering { reason: None });
        assert!(!feedback.is_scanout(feedback.pick(true, id, &states)));
        let (_, states) = shown(RenderElementPresentationState::ZeroCopy);
        assert!(!feedback.is_scanout(feedback.pick(true, Id::new(), &states)));
    }

    #[test]
    fn only_fullscreen_windows_are_offered_the_screens_formats() {
        let feedback = feedback();
        let (id, states) = shown(RenderElementPresentationState::ZeroCopy);
        assert!(!feedback.is_scanout(feedback.pick(false, id, &states)));
    }
}

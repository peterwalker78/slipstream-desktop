//! The floating strip leans into a slide: a rigid pane that turns a little about its upright
//! middle line in the direction the workspaces' camera is travelling, by as much as the camera's
//! speed asks, and lies flat again once the camera stops. Fingers moving the camera move it the
//! same way, since only the camera is read.

use smithay::{
    backend::renderer::gles::{GlesRenderer, element::TextureShaderElement},
    utils::{Logical, Physical, Size},
};

use crate::{
    pane::{Camera, Look, Panes, Pose},
    render::OutputElement,
    tilt::{Canvas, Sizes},
};

/// The furthest it turns, in radians: 2.5°.
const MOST: f64 = 2.5 * std::f64::consts::PI / 180.0;
/// How far it turns for each workspace a second the camera travels, in radians: 0.35°.
const PER_SPEED: f64 = 0.35 * std::f64::consts::PI / 180.0;
/// How quickly the speed it follows catches up with the camera's, in seconds: enough to smooth
/// the steps of fingers on a touchpad without lagging a slide.
const SMOOTHING: f64 = 0.05;
/// Below this it is drawn flat, as the plain element it is at rest.
const AT_REST: f64 = 0.0005;

#[derive(Default)]
pub struct Lean {
    /// The camera's place, in workspaces, and the wall time it was there.
    last: Option<(f64, f64)>,
    /// Its speed, smoothed, in workspaces a second.
    speed: f64,
    canvas: Option<Canvas>,
    broken: bool,
}

impl Lean {
    /// Follows the camera, `camera` workspaces along at `wall` seconds, and says how far the strip
    /// turns: positive sends its right-hand end away.
    pub fn follow(&mut self, camera: f64, wall: f64) -> f64 {
        if let Some((was, then)) = self.last {
            let dt = wall - then;
            if dt > 0.0 {
                let raw = (camera - was) / dt;
                let k = 1.0 - (-dt / SMOOTHING).exp();
                self.speed += (raw - self.speed) * k;
                self.last = Some((camera, wall));
            } else if dt < 0.0 {
                self.last = Some((camera, wall));
            }
        } else {
            self.last = Some((camera, wall));
        }
        let yaw = (self.speed * PER_SPEED).clamp(-MOST, MOST);
        if yaw.abs() < AT_REST { 0.0 } else { yaw }
    }

    /// Stands it straight with nothing remembered: it isn't leaning this frame.
    pub fn rest(&mut self) {
        self.last = None;
        self.speed = 0.0;
        self.canvas = None;
    }

    /// The strip's elements, drawn as one pane turned by `yaw` about its middle on a screen
    /// `width` wide: they are drawn into a picture of the top `foot` pixels of the screen first,
    /// which the pane shader then shows turned. `None` where the shader can't be had.
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        panes: &mut Panes,
        elements: &[OutputElement],
        (width, foot): (i32, i32),
        middle: f64,
        scale: f64,
        yaw: f64,
        screen: Sizes,
    ) -> Option<TextureShaderElement> {
        if self.broken || elements.is_empty() {
            return None;
        }
        let own = Size::<i32, Logical>::from((width, foot));
        let physical = Size::<i32, Physical>::from((
            (width as f64 * scale).round() as i32,
            (foot as f64 * scale).round() as i32,
        ));
        let pose = Pose {
            yaw,
            ..Pose::flat([0.0, 0.0, width as f64, foot as f64])
        };
        // Seen straight on at the strip's own height, so its ends grow and shrink evenly and its
        // middle line stays level: it turns, it never rolls.
        let camera = Camera {
            y: middle,
            ..Camera::for_screen(screen.logical.w as f64, screen.logical.h as f64)
        };
        let look = Look {
            alpha: 1.0,
            // The glass's own corner light would come and go with the lean.
            matte: 1.0,
            ..Look::default()
        };
        panes.picture(
            renderer,
            (&mut self.canvas, &mut self.broken),
            elements,
            (own, physical, scale),
            &pose,
            &camera,
            screen,
            &look,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_leans_with_the_slide_and_settles_flat() {
        let mut lean = Lean::default();
        assert_eq!(lean.follow(0.0, 0.0), 0.0);
        // A slide to the right at a few workspaces a second: right-hand end away, within bounds.
        let mut yaw = 0.0;
        for frame in 1..=10 {
            yaw = lean.follow(frame as f64 * 0.05, frame as f64 / 60.0);
        }
        assert!(yaw > 0.0 && yaw <= MOST, "{yaw}");
        // Very fast, it stops at the most it turns.
        for frame in 11..=20 {
            yaw = lean.follow(frame as f64 * 0.5, frame as f64 / 60.0);
        }
        assert_eq!(yaw, MOST);
        // Back the other way, the other way round.
        for frame in 21..=40 {
            yaw = lean.follow(10.0 - (frame - 20) as f64 * 0.05, frame as f64 / 60.0);
        }
        assert!(yaw < 0.0, "{yaw}");
        // The camera stops: flat again.
        for frame in 41..=80 {
            yaw = lean.follow(9.0, frame as f64 / 60.0);
        }
        assert_eq!(yaw, 0.0);
    }
}

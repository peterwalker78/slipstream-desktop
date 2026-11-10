//! The bar as it is holds its breath while you type: what's on it dims, and comes back once the
//! typing is over. Its fill and the line under it stay as they are, and whatever wants attention
//! (a mode's chip, the sharing dot, the unread badge) never dims.
//!
//! The bar's picture is left alone: the dim is the bar's own fill laid over its contents at a
//! little under half strength, as solid boxes that leave out what wants attention.

use smithay::{
    backend::renderer::element::{
        Kind,
        solid::{SolidColorBuffer, SolidColorRenderElement},
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::motion::REDUCED_FADE;

/// How far the contents dim: to 55% of their strength.
pub const DIM: f32 = 0.45;
/// How long the dim takes to come on once typing starts, and to go once it ends, in seconds.
const IN: f64 = 0.6;
const OUT: f64 = 0.4;

#[derive(Default)]
pub struct Breath {
    /// How far through the dim it is, 0 to 1.
    level: f64,
    /// When it was last stepped.
    last: Option<f64>,
    /// One box for each stretch of the bar between things that want attention.
    boxes: Vec<SolidColorBuffer>,
}

impl Breath {
    /// Steps towards dimmed while `typing`, and back otherwise, to `now` seconds. Returns how
    /// strongly the contents are covered, 0 to `DIM`. With reduced motion it takes only the
    /// short fade every effect becomes.
    pub fn step(&mut self, typing: bool, now: f64, reduced: bool) -> f32 {
        let dt = self.last.map_or(0.0, |last| (now - last).max(0.0));
        self.last = Some(now);
        let (into, out) = if reduced {
            (REDUCED_FADE, REDUCED_FADE)
        } else {
            (IN, OUT)
        };
        self.level = if typing {
            (self.level + dt / into).min(1.0)
        } else {
            (self.level - dt / out).max(0.0)
        };
        let t = self.level;
        DIM * (t * t * (3.0 - 2.0 * t)) as f32
    }

    /// The dim over a bar at `bar` (logical pixels from the screen's corner), leaving out the
    /// spans of it in `attention` (left and right edges, the same way), at `strength`: the bar's
    /// own `fill` (0xRRGGBBAA), laid on whole.
    pub fn elements(
        &mut self,
        bar: Rectangle<f64, Logical>,
        attention: &[(f64, f64)],
        scale: f64,
        strength: f32,
        fill: u32,
    ) -> Vec<SolidColorRenderElement> {
        if strength <= 0.0 {
            return Vec::new();
        }
        let spans = calm_spans(bar.loc.x, bar.loc.x + bar.size.w, attention);
        let [r, g, b, _] = fill.to_be_bytes();
        let colour = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
        self.boxes
            .resize_with(spans.len(), SolidColorBuffer::default);
        // Short of the foot by a pixel, so the line under the bar keeps its strength.
        let height = (bar.size.h.round() as i32 - 1).max(1);
        spans
            .iter()
            .zip(self.boxes.iter_mut())
            .map(|(&(left, right), buffer)| {
                let left = left.round();
                let width = (right.round() - left).max(1.0) as i32;
                buffer.update(Size::<i32, Logical>::from((width, height)), colour);
                let at = Point::<f64, Logical>::from((left, bar.loc.y))
                    .to_physical(scale)
                    .to_i32_round::<i32>();
                SolidColorRenderElement::from_buffer(buffer, at, scale, strength, Kind::Unspecified)
            })
            .collect()
    }
}

/// The stretches from `left` to `right` that don't cross any span in `attention`.
fn calm_spans(left: f64, right: f64, attention: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut held: Vec<(f64, f64)> = attention.to_vec();
    held.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut spans = Vec::new();
    let mut from = left;
    for (start, end) in held {
        if start > from {
            spans.push((from, start.min(right)));
        }
        from = from.max(end);
    }
    if from < right {
        spans.push((from, right));
    }
    spans.retain(|(a, b)| b - a >= 1.0);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concentration::Concentration;
    use std::time::{Duration, Instant};

    #[test]
    fn the_bar_dims_while_you_type_and_comes_back_after() {
        let mut typing = Concentration::default();
        let mut breath = Breath::default();
        let start = Instant::now();
        let at = |secs: f64| start + Duration::from_secs_f64(secs);
        assert_eq!(breath.step(typing.typing(at(0.0)), 0.0, false), 0.0);
        typing.key(at(0.1), false);
        let mut dim = 0.0;
        for frame in 1..=40 {
            let now = 0.1 + frame as f64 / 60.0;
            dim = breath.step(typing.typing(at(now)), now, false);
        }
        assert_eq!(dim, DIM, "fully dimmed 0.6 s in");
        assert_eq!(breath.step(typing.typing(at(0.99)), 0.99, false), DIM);
        // A shortcut ends the typing, and the bar comes back over 0.4 s.
        typing.key(at(1.0), true);
        let half = breath.step(typing.typing(at(1.2)), 1.2, false);
        assert!(half > 0.0 && half < DIM, "{half}");
        assert_eq!(breath.step(typing.typing(at(1.45)), 1.45, false), 0.0);
    }

    #[test]
    fn reduced_motion_dims_in_the_short_fade() {
        let mut breath = Breath::default();
        breath.step(true, 0.0, true);
        assert_eq!(breath.step(true, REDUCED_FADE, true), DIM);
        assert_eq!(breath.step(false, 2.0 * REDUCED_FADE, true), 0.0);
    }

    #[test]
    fn what_wants_attention_is_left_out_of_the_dim() {
        let spans = calm_spans(0.0, 1000.0, &[(900.0, 930.0), (300.0, 380.0)]);
        assert_eq!(spans, [(0.0, 300.0), (380.0, 900.0), (930.0, 1000.0)]);
        assert_eq!(calm_spans(0.0, 100.0, &[]), [(0.0, 100.0)]);
    }
}

//! Short messages near the top of the screen, as the mockup's toast: an amber title over a line
//! or two of text. It slides in over 250 ms and goes after 2.6 s; a new one replaces it.

use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    motion::HYPR,
    paint::{self, Painter},
    panel,
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
const WIDTH: f32 = 440.0;
const TOP: f32 = 58.0;
/// Logical pixels per mockup pixel.
const MOCKUP_PX: f32 = 0.8;
const SHOWN: f64 = 2.6;
const FADE: f64 = 0.25;
const REDUCED_FADE: f64 = 0.08;

struct Painted {
    scale: f64,
    buffer: MemoryRenderBuffer,
    logical: Size<i32, Logical>,
    device: (i32, i32),
}

pub struct Toast {
    /// Title, body, and when it appeared on the animation clock.
    message: Option<(String, String, f64)>,
    painted: Option<Painted>,
    pub reduced_motion: bool,
}

impl Toast {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.painted = None;
    }

    pub fn new(reduced_motion: bool) -> Self {
        Self {
            message: None,
            painted: None,
            reduced_motion,
        }
    }

    pub fn show(&mut self, title: &str, body: &str, now: f64) {
        // The title only. Titles are fixed wording and counts; a body can name a window, and for
        // an app without a desktop entry that name is its title: a folder, a document, a URL.
        tracing::info!(title, "toast");
        self.message = Some((title.to_string(), body.to_string(), now));
        self.painted = None;
    }

    /// The toast for a screen `width` logical pixels wide, while one is showing.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (title, body, at) = self.message.as_ref()?;
        let age = now - at;
        if !(0.0..SHOWN).contains(&age) {
            self.message = None;
            self.painted = None;
            return None;
        }
        if self
            .painted
            .as_ref()
            .is_none_or(|painted| painted.scale != scale)
        {
            self.painted = paint(title, body, scale);
        }
        let painted = self.painted.as_ref()?;
        let fade = if self.reduced_motion {
            REDUCED_FADE
        } else {
            FADE
        };
        let alpha = (age / fade).min((SHOWN - age) / fade).clamp(0.0, 1.0);
        let rise = if self.reduced_motion {
            0.0
        } else {
            -8.0 * (1.0 - HYPR.at(age / FADE))
        };
        // The painted area holds the shadow too; the card itself keeps its place.
        let margin = (panel::NOTICE_MARGIN * MOCKUP_PX) as f64;
        let location = Point::<f64, Logical>::from((
            ((width - painted.logical.w) / 2) as f64,
            (TOP as f64 + rise) * MOCKUP_PX as f64 - margin,
        ))
        .to_physical(scale)
        .to_i32_round::<i32>()
        .to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            &painted.buffer,
            Some(alpha as f32),
            Some(Rectangle::from_size(
                (painted.device.0 as f64, painted.device.1 as f64).into(),
            )),
            Some(painted.logical),
            Kind::Unspecified,
        )
        .ok()
    }
}

fn paint(title: &str, body: &str, scale: f64) -> Option<Painted> {
    const AMBER: u32 = 0xffb547ff;
    let heading = Style {
        tracking: 0.1,
        ..Style::new(Face::BodyBold, 13.0, AMBER)
    };
    let prose = Style::new(Face::Body, 16.0, 0xdfe5eeff);
    let lines = text::wrap(body, &prose, WIDTH - 22.0 - 18.0);
    let height = 16.0 + 20.0 + 4.0 + lines.len() as f32 * 24.0 + 16.0;
    let m = panel::NOTICE_MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((WIDTH + 2.0 * m) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * m) * MOCKUP_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * scale).round() as i32,
        (logical.h as f64 * scale).round() as i32,
    );
    let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * MOCKUP_PX)?;
    // The notice's shadow, the amber left edge, then the card over the rest of it.
    let radius = panel::NOTICE_RADIUS;
    let (dy, blur, shadow) = panel::NOTICE_SHADOW;
    p.shadow(m, m, WIDTH, height, radius, dy, blur, shadow);
    p.fill(m, m, WIDTH, height, radius, AMBER);
    p.fill(m + 4.0, m, WIDTH - 4.0, height, radius - 3.0, panel::NOTICE);
    p.border(m, m, WIDTH, height, radius, 1.0, panel::NOTICE_EDGE);
    p.text(&title.to_uppercase(), m + 22.0, m + 16.0 + 10.0, &heading);
    for (i, line) in lines.iter().enumerate() {
        p.text(
            line,
            m + 22.0,
            m + 16.0 + 24.0 + 12.0 + i as f32 * 24.0,
            &prose,
        );
    }
    Some(Painted {
        scale,
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_message_grows_the_card_rather_than_overflowing() {
        let short = paint("Gravity", "On.", 1.25).unwrap();
        let long = paint(
            "Already the centre",
            "Konsole can’t get any heavier, and this sentence goes on long enough to wrap twice \
             over in a card this narrow. PgDn makes it lighter.",
            1.25,
        )
        .unwrap();
        assert_eq!(short.logical.w, long.logical.w);
        assert!(long.logical.h > short.logical.h);
    }
}

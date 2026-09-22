//! Builds each frame: windows, the focus ring and, on real hardware, the pointer. Both backends
//! draw through here, so the nested window and the real screen look the same.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, ExportMem, ImportMem, Offscreen, Renderer,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind, RenderElementStates,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                texture::TextureRenderElement,
                utils::RescaleRenderElement,
            },
            gles::{GlesRenderer, GlesTexture, element::TextureShaderElement},
        },
    },
    desktop::{
        Window,
        utils::{OutputPresentationFeedback, surface_presentation_feedback_flags_from_states},
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::with_states,
};

use std::{collections::HashMap, time::Duration};

use resvg::tiny_skia::Pixmap;

use crate::{
    Slipstream,
    bar::{self, Bar},
    bullet::{self, Target},
    cursor::Cursor,
    gravity::Rung,
    layout::Rect,
    motion,
    overview::{self, Overview},
    paint::{self, Painter},
    rain::Rain,
    text::{self, Face, Style},
    tilt::{self, Tilt},
};

/// The mockup's page colour, behind every window.
pub const BACKGROUND: Color32F = Color32F::new(0.043, 0.055, 0.075, 1.0);
/// The focused window's ring, until the settings are read: `borders.selected-tile` chooses it,
/// and the buffers take the chosen colour on the first frame they are drawn.
const FOCUS: Color32F = Color32F::new(0.259, 0.827, 1.0, 1.0);
/// The ring is drawn a fifth more transparent than the colour itself, so it marks the window
/// without ringing it in solid paint.
const RING_ALPHA: f32 = 0.8;
/// Focus ring width in logical pixels. It sits in the gap, outside the window.
const RING: i32 = 2;
/// Over distant windows: the mockup dims them to 70% brightness.
const DIM: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.3);
/// The fade at the end of a session.
const BLACK: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);
const WHITE: Color32F = Color32F::new(1.0, 1.0, 1.0, 1.0);
/// How long a tier tag shows.
const TAG_SHOWN: f64 = 1.8;
/// A tier tag's fade in and out with reduced motion, which shows it in place.
const TAG_REDUCED_FADE: f64 = 0.08;

// Both backends draw with GLES, which bullet time's 3D shader needs.
smithay::backend::renderer::element::render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    /// A window stretched or shrunk while it animates.
    Rescaled=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    /// A texture shown through one of Slipstream's shaders: bullet time's tilted overview, or the
    /// UI's glass fade.
    Shaded=TextureShaderElement,
    /// The desktop drawn into a texture, coming forward as the lock goes.
    Texture=TextureRenderElement<GlesTexture>,
}

impl std::fmt::Debug for OutputElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(e) => f.debug_tuple("Surface").field(e).finish(),
            Self::Rescaled(e) => f.debug_tuple("Rescaled").field(e).finish(),
            Self::Memory(e) => f.debug_tuple("Memory").field(e).finish(),
            Self::Solid(e) => f.debug_tuple("Solid").field(e).finish(),
            Self::Shaded(e) => f.debug_tuple("Shaded").field(e).finish(),
            Self::Texture(e) => f.debug_tuple("Texture").field(e).finish(),
            Self::_GenericCatcher(e) => f.debug_tuple("_GenericCatcher").field(e).finish(),
        }
    }
}

/// A painted surface: the scale it was painted at, its buffer, and its size in logical pixels.
type Painted = (f64, MemoryRenderBuffer, Size<i32, Logical>);

/// What Slipstream draws itself: the bar, the focus ring, and the key hint on an empty
/// workspace. The buffers live between frames, so damage tracking redraws only changes.
pub struct Chrome {
    pub bar: Bar,
    ring: [SolidColorBuffer; 4],
    /// One per distant window drawn this frame.
    dims: Vec<SolidColorBuffer>,
    tags: HashMap<Rung, Painted>,
    /// The keyboard-tips prompt on an empty workspace.
    tips: Option<Painted>,
    /// Bullet time's key legend along the foot of the screen.
    legend: Option<paint::Painted>,
    /// Bullet time's overlays.
    pub overview: Overview,
    /// Bullet time's 3D.
    stage: tilt::Stage,
    /// The UI's glass fade to the wallpaper and back.
    glass: crate::glass::Glass,
    /// The fade to black on the way out.
    blackout: SolidColorBuffer,
    /// The white flash when a screenshot is taken.
    flash: SolidColorBuffer,
    /// The lock's veil over the wallpaper.
    veil: SolidColorBuffer,
    /// The desktop as it comes back from behind the lock, and whether that texture failed.
    unlock: Option<(GlesTexture, Size<i32, Physical>)>,
    unlock_broken: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            bar: Bar::default(),
            ring: std::array::from_fn(|_| SolidColorBuffer::new((0, 0), FOCUS)),
            dims: Vec::new(),
            tags: HashMap::new(),
            tips: None,
            legend: None,
            overview: Overview::default(),
            stage: tilt::Stage::default(),
            glass: crate::glass::Glass::default(),
            blackout: SolidColorBuffer::new((0, 0), BLACK),
            flash: SolidColorBuffer::new((0, 0), WHITE),
            veil: SolidColorBuffer::new((0, 0), veil_colour()),
            unlock: None,
            unlock_broken: false,
        }
    }
}

/// A tier tag's opacity and how far above its resting place it is, in mockup pixels, `age`
/// seconds after it appeared: it drops 8 px into place while fading in, holds, and fades out.
/// Reduced motion leaves out the drop and shortens both fades.
fn tag_motion(age: f64, reduced_motion: bool) -> (f64, f64) {
    if reduced_motion {
        let alpha = (age / TAG_REDUCED_FADE).min((TAG_SHOWN - age) / TAG_REDUCED_FADE);
        return (alpha.clamp(0.0, 1.0), 0.0);
    }
    let progress = age / TAG_SHOWN;
    if progress < 0.12 {
        (progress / 0.12, -8.0 * (1.0 - progress / 0.12))
    } else if progress < 0.8 {
        (1.0, 0.0)
    } else {
        ((1.0 - progress) / 0.2, 0.0)
    }
}

/// The lock's veil, premultiplied as the renderer takes colours.
fn veil_colour() -> Color32F {
    let [r, g, b, a] = crate::lock::VEIL;
    Color32F::new(r * a, g * a, b * a, a)
}

/// A rung's tag: its name in amber, and the ladder beside it as a row of pips with this rung's
/// filled, so where the window is on the ladder reads at a glance. Laid out in mockup pixels.
fn paint_tag(rung: Rung, scale: f64) -> Option<(Pixmap, Size<i32, Logical>)> {
    const MOCKUP_PX: f32 = 0.8;
    const PIP: f32 = 7.0;
    const PIP_GAP: f32 = 4.0;
    const INK: u32 = 0x1a1206ff;
    let style = Style {
        tracking: 0.08,
        ..Style::new(Face::MonoBold, 16.0, INK)
    };
    let label = rung.label().to_uppercase();
    let rungs = Rung::LADDER.len() as f32;
    let pips_w = rungs * PIP + (rungs - 1.0) * PIP_GAP;
    let text_w = text::width(&label, &style);
    let (w, h) = (12.0 + text_w + 12.0 + pips_w + 12.0, 30.0);
    let size =
        Size::<i32, Logical>::from(((w * MOCKUP_PX).ceil() as i32, (h * MOCKUP_PX).ceil() as i32));
    let mut p = Painter::new(
        (size.w as f64 * scale).round() as u32,
        (size.h as f64 * scale).round() as u32,
        scale as f32 * MOCKUP_PX,
    )?;
    p.fill(0.0, 0.0, w, h, 6.0, 0xffb547ff);
    p.text(&label, 12.0, h / 2.0, &style);
    let mut x = 12.0 + text_w + 12.0;
    for step in Rung::LADDER {
        let (y, radius) = ((h - PIP) / 2.0, 2.0);
        if step == rung {
            p.fill(x, y, PIP, PIP, radius, INK);
        } else {
            p.border(x, y, PIP, PIP, radius, 1.5, 0x1a120680);
        }
        x += PIP + PIP_GAP;
    }
    Some((p.pixmap, size))
}

/// How far a window on workspace `index` is shifted from where it sits in the layout of screens,
/// on a screen at `here` whose view is at `camera` with workspaces `step` apart. `origin` is the
/// left edge of the screen the workspace is laid out for: its windows sit that far along, and are
/// drawn from this screen's edge instead, a workspace's distance from the view to the side.
fn shift_for_workspace(index: usize, camera: f64, step: f64, here: i32, origin: i32) -> f64 {
    (index as f64 - camera) * step + (here - origin) as f64
}

/// The prompt on an empty workspace: where to find every key, and nothing else. It used to be
/// the whole key table, which covered most of the wallpaper on an empty workspace — the one
/// moment the wallpaper is all there is to look at. Super+/ already lists every key, so the
/// empty workspace only has to say so.
const TIPS_TEXT: &str = "Keyboard tips";
const TIPS_KEY: &str = "Super+/";
/// Where it floats: this far down the screen, so it sits in the lower third clear of the
/// wallpaper's logo, and drifts this far either side of that over `TIPS_DRIFT` seconds.
const TIPS_DOWN: f64 = 0.72;
const TIPS_FLOAT_PX: f64 = 4.0;
const TIPS_DRIFT: f64 = 5.0;
/// How far its glow breathes, and over how long.
const TIPS_DIM: f32 = 0.15;
const TIPS_BREATH: f64 = 4.0;
/// The pill: its height, the padding at its ends, and the gap between the words and the key.
const TIPS_H: f32 = 34.0;
const TIPS_PAD: f32 = 16.0;
const TIPS_GAP: f32 = 12.0;
/// The glow: how many rings are drawn around the pill, how far apart, and the colour they fade
/// out from. Painted once into the buffer; only its opacity moves.
const TIPS_RINGS: i32 = 7;
const TIPS_RING_STEP: f32 = 1.6;
const TIPS_GLOW: u32 = 0x3cf0c0ff;

/// Bullet time's key legend, a pill of its keys laid out in mockup pixels, painted at `scale`.
fn paint_legend(scale: f64) -> Option<paint::Painted> {
    const MOCKUP_PX: f32 = 0.8;
    let style = Style::new(Face::Mono, 13.0, 0x8f98a8ff);
    let legend = bullet::legend();
    let (w, h) = (text::width(&legend, &style) + 32.0, 13.0 * 1.2 + 16.0);
    let logical =
        Size::<i32, Logical>::from(((w * MOCKUP_PX).ceil() as i32, (h * MOCKUP_PX).ceil() as i32));
    let device = (
        (logical.w as f64 * scale).round().max(1.0) as i32,
        (logical.h as f64 * scale).round().max(1.0) as i32,
    );
    let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * MOCKUP_PX)?;
    p.fill(0.0, 0.0, w, h, 10.0, 0x0a0c11cc);
    p.text(&legend, 16.0, h / 2.0, &style);
    Some(paint::Painted {
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
        scale,
    })
}

/// The keyboard-tips prompt, laid out in logical pixels and painted at `scale`: a glass pill
/// saying what to press, inside a soft glow drawn as rings fading outwards. The glow is baked in
/// and the whole thing is faded in and out as it floats, so nothing repaints per frame.
fn paint_tips(scale: f64) -> Option<(Pixmap, Size<i32, Logical>)> {
    let words = Style::new(Face::Mono, 14.0, 0xdce3ecff);
    let pill_w = (TIPS_PAD
        + text::width(TIPS_TEXT, &words)
        + TIPS_GAP
        + paint::keycap_width(TIPS_KEY)
        + TIPS_PAD)
        .ceil();
    // Room around the pill for the glow to fade out into.
    let halo = TIPS_RINGS as f32 * TIPS_RING_STEP;
    let size = Size::<i32, Logical>::from((
        (pill_w + 2.0 * halo).ceil() as i32,
        (TIPS_H + 2.0 * halo).ceil() as i32,
    ));
    let mut p = Painter::new(
        (size.w as f64 * scale).round() as u32,
        (size.h as f64 * scale).round() as u32,
        scale as f32,
    )?;
    // Rings outwards from the pill's edge, each fainter than the last.
    for ring in (1..=TIPS_RINGS).rev() {
        let grow = ring as f32 * TIPS_RING_STEP;
        let alpha = 0x30 / (ring as u32 + 1);
        let radius = (TIPS_H + 2.0 * grow) / 2.0;
        p.border(
            halo - grow,
            halo - grow,
            pill_w + 2.0 * grow,
            TIPS_H + 2.0 * grow,
            radius,
            TIPS_RING_STEP,
            (TIPS_GLOW & 0xffffff00) | alpha,
        );
    }
    p.fill(halo, halo, pill_w, TIPS_H, TIPS_H / 2.0, 0x0b0d12e0);
    p.border(
        halo,
        halo,
        pill_w,
        TIPS_H,
        TIPS_H / 2.0,
        1.0,
        (TIPS_GLOW & 0xffffff00) | 0x66,
    );
    let centre = halo + TIPS_H / 2.0;
    p.text(TIPS_TEXT, halo + TIPS_PAD, centre, &words);
    p.keycap(
        TIPS_KEY,
        halo + TIPS_PAD + text::width(TIPS_TEXT, &words) + TIPS_GAP,
        centre,
    );
    Some((p.pixmap, size))
}

impl Chrome {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.bar.forget_painted_text();
        self.overview.forget_painted_text();
        self.tags.clear();
        self.tips = None;
        self.legend = None;
    }

    /// A shade over a distant window. `index` keeps each distant window's shade separate.
    fn dim(
        &mut self,
        index: usize,
        over: Rectangle<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> SolidColorRenderElement {
        while self.dims.len() <= index {
            self.dims.push(SolidColorBuffer::new((0, 0), DIM));
        }
        let buffer = &mut self.dims[index];
        buffer.update(over.size, DIM);
        SolidColorRenderElement::from_buffer(
            buffer,
            over.loc.to_physical_precise_round(scale),
            scale,
            alpha,
            Kind::Unspecified,
        )
    }

    /// The tag for a window's new rung, near the top of it: it drops in, holds, and fades, over
    /// 1.8 s. With reduced motion it appears where it rests, with a short fade either end.
    fn tag<R>(
        &mut self,
        renderer: &mut R,
        rung: Rung,
        window: Rectangle<f64, Logical>,
        age: f64,
        scale: Scale<f64>,
        reduced_motion: bool,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (alpha, drop) = tag_motion(age, reduced_motion);
        if self
            .tags
            .get(&rung)
            .is_none_or(|(painted_at, ..)| *painted_at != scale.x)
        {
            let (pixmap, size) = paint_tag(rung, scale.x)?;
            self.tags
                .insert(rung, (scale.x, paint::buffer(&pixmap), size));
        }
        let (_, buffer, size) = self.tags.get(&rung)?;
        let device = (
            (size.w as f64 * scale.x).round(),
            (size.h as f64 * scale.x).round(),
        );
        let location = Point::<f64, Logical>::from((
            window.loc.x + (window.size.w - size.w as f64) / 2.0,
            window.loc.y + (22.0 + drop) * 0.8,
        ))
        .to_physical(scale)
        .to_i32_round::<i32>()
        .to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            buffer,
            Some(alpha.clamp(0.0, 1.0) as f32),
            Some(Rectangle::from_size(device.into())),
            Some(*size),
            Kind::Unspecified,
        )
        .ok()
    }

    fn ring(
        &mut self,
        around: Rectangle<i32, Logical>,
        colour: Color32F,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<SolidColorRenderElement> {
        let Rectangle { loc, size } = around;
        let edges = [
            (loc.x - RING, loc.y - RING, size.w + 2 * RING, RING),
            (loc.x - RING, loc.y + size.h, size.w + 2 * RING, RING),
            (loc.x - RING, loc.y, RING, size.h),
            (loc.x + size.w, loc.y, RING, size.h),
        ];
        self.ring
            .iter_mut()
            .zip(edges)
            .map(|(buffer, (x, y, w, h))| {
                buffer.update((w, h), colour);
                let location: Point<i32, Physical> =
                    Point::<i32, Logical>::from((x, y)).to_physical_precise_round(scale);
                SolidColorRenderElement::from_buffer(
                    buffer,
                    location,
                    scale,
                    alpha * RING_ALPHA,
                    Kind::Unspecified,
                )
            })
            .collect()
    }

    /// The keyboard-tips prompt, floating in the lower third of an empty workspace, which is `dx`
    /// from the screen while sliding. `drifting` is wall time, to float and breathe by, and
    /// `None` under reduced motion, which holds it still at full strength: it drifts a few pixels
    /// up and down and its glow breathes, so it reads as hovering rather than stuck to the
    /// wallpaper. Painted again whenever the scale changes.
    fn tips<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        dx: f64,
        scale: Scale<f64>,
        alpha: f32,
        drifting: Option<f64>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if self
            .tips
            .as_ref()
            .is_none_or(|(painted_at, ..)| *painted_at != scale.x)
        {
            let (pixmap, size) = paint_tips(scale.x)?;
            self.tips = Some((scale.x, paint::buffer(&pixmap), size));
        }
        let (_, buffer, size) = self.tips.as_ref()?;
        let (float, breath) = match drifting {
            None => (0.0, 1.0),
            Some(now) => {
                let turn = std::f64::consts::TAU;
                (
                    (now / TIPS_DRIFT * turn).sin() * TIPS_FLOAT_PX,
                    1.0 - TIPS_DIM as f64 * (0.5 - 0.5 * (now / TIPS_BREATH * turn).cos()),
                )
            }
        };
        let top = (screen.h as f64 * TIPS_DOWN - size.h as f64 / 2.0 + float)
            .clamp(bar::HEIGHT as f64, (screen.h - size.h).max(0) as f64);
        let device = (
            (size.w as f64 * scale.x).round(),
            (size.h as f64 * scale.x).round(),
        );
        // Whole screen pixels, so the text stays sharp.
        let location = Point::<f64, Logical>::from((((screen.w - size.w) / 2) as f64 + dx, top))
            .to_physical(scale)
            .to_i32_round::<i32>()
            .to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            buffer,
            Some(alpha * breath as f32),
            Some(Rectangle::from_size(device.into())),
            Some(*size),
            Kind::Unspecified,
        )
        .ok()
    }
}

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

pub fn output_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    cursor: Option<&mut Cursor>,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    // A dragged gap's tiles follow it once a frame, before anything is drawn from them.
    state.retile_for_drag();
    let Some(output_geo) = state.space.output_geometry(output) else {
        return elements;
    };
    // This screen's own drawing buffers and wallpaper, put back at the end. Taking them out
    // keeps every use below a plain local, and keeps the screens from sharing a buffer sized for
    // one of them.
    let name = output.name();
    let mut chrome = state.chromes.remove(&name).unwrap_or_default();
    let mut saver = state
        .savers
        .remove(&name)
        .unwrap_or_else(|| state.new_saver());
    let screen_index = state.screens.index_of(output);
    let scale = Scale::from(output.current_scale().fractional_scale());
    let now = state.clock.frame();
    // While locked, the lock is all there is: nothing below reads a window.
    if state.lock.is_some() {
        let elements = lock_elements(
            state,
            renderer,
            output,
            output_geo,
            &mut chrome,
            &mut saver,
            cursor,
            scale,
        );
        chrome.unlock = None;
        state.chromes.insert(name.clone(), chrome);
        state.savers.insert(name, saver);
        return elements;
    }
    if state
        .unlocking
        .as_ref()
        .is_some_and(|unlocking| unlocking.done(state.wall()))
    {
        state.unlocking = None;
    }
    if state.unlocking.is_none() {
        chrome.unlock = None;
    }
    // How visible the UI is: it fades after a while with no input, leaving the wallpaper.
    let ui = state.ui_opacity(now);

    // Bullet time's overview: 0 normally, 1 zoomed right out. It runs on wall time, so opening
    // and closing it is never slowed.
    let wall = state.wall();
    let zoomed_out = state.overview.value(wall)[0].max(0.0);
    let zoom = 1.0 - 0.32 * zoomed_out;
    let centre = (
        output_geo.size.w as f64 / 2.0,
        output_geo.size.h as f64 / 2.0,
    );
    let to_overview = move |r: Rectangle<f64, Logical>| {
        Rectangle::<f64, Logical>::new(
            (
                centre.0 + (r.loc.x - centre.0) * zoom,
                centre.1 + (r.loc.y - centre.1) * zoom,
            )
                .into(),
            (r.size.w * zoom, r.size.h * zoom).into(),
        )
    };
    let area_local = state.screen_area(screen_index.unwrap_or(0)).map(|area| {
        Rectangle::<f64, Logical>::new(
            (
                (area.x - output_geo.loc.x) as f64,
                (area.y - output_geo.loc.y) as f64,
            )
                .into(),
            (area.w as f64, area.h as f64).into(),
        )
    });
    // Where the tilted overview's reflection starts, the bottom of the workspace frames, and how
    // far below that it shows.
    let mirror_line = area_local.map(|area| {
        let bottom = centre.1 + (area.loc.y + area.size.h + 10.0 - centre.1) * zoom;
        (bottom as f32, (area.size.h * zoom * 0.35) as f32)
    });
    let viewed_index = state
        .bullet
        .as_ref()
        .map_or(state.active_workspace(), |mode| mode.view);
    let selected = state.bullet.as_ref().and_then(|mode| mode.selected.clone());
    let labels = state
        .bullet
        .as_ref()
        .map(|mode| mode.labels.clone())
        .unwrap_or_default();
    // Bullet time, the panels and the cards are drawn where the keyboard is.
    let first_output = state.screens.focused_output().as_ref() == Some(output);
    chrome.overview.begin();
    chrome.overview.set_accent(state.bullet_rgb);
    if first_output && state.bullet.is_some() {
        state.overview_hits.clear();
        state.frame_hits.clear();
    }

    if let Some(cursor) = cursor.filter(|_| ui > 0.5) {
        let pointer = state.pointer_location();
        if output_geo.to_f64().contains(pointer) {
            let pos = pointer - output_geo.loc.to_f64();
            if let CursorImageStatus::Surface(surface) = &state.cursor_status {
                if !surface.alive() {
                    state.cursor_status = CursorImageStatus::default_named();
                }
            }
            // Slipstream's own cursor over a gap or during a drag, never over an overlay or the
            // lock.
            let status = match state.cursor_override {
                Some(icon) if state.drag.is_some() || !state.pointer_blocked() => {
                    CursorImageStatus::Named(icon)
                }
                _ => state.cursor_status.clone(),
            };
            match &status {
                CursorImageStatus::Hidden => {}
                CursorImageStatus::Surface(surface) => {
                    let hotspot = with_states(surface, |states| {
                        states
                            .data_map
                            .get::<CursorImageSurfaceData>()
                            .map(|data| data.lock().unwrap().hotspot)
                            .unwrap_or_else(|| (0, 0).into())
                    });
                    let location: Point<i32, Physical> =
                        (pos - hotspot.to_f64()).to_physical(scale).to_i32_round();
                    let tree: Vec<OutputElement> = render_elements_from_surface_tree(
                        renderer,
                        surface,
                        location,
                        scale,
                        1.0,
                        Kind::Cursor,
                    );
                    elements.extend(tree);
                }
                CursorImageStatus::Named(icon) => {
                    let (buffer, hotspot) =
                        cursor.image(icon.name(), scale.x, state.start_time.elapsed());
                    let location = (pos - hotspot).to_physical(scale);
                    match MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        location,
                        &buffer,
                        None,
                        None,
                        None,
                        Kind::Cursor,
                    ) {
                        Ok(element) => elements.push(OutputElement::Memory(element)),
                        Err(err) => tracing::warn!("couldn't draw the pointer: {err:?}"),
                    }
                }
            }
        }
    }

    // The workspace this screen is showing, which is not the same as the one the keyboard is on.
    let active = screen_index
        .and_then(|index| state.screens.get(index))
        .map(|screen| screen.workspace)
        .unwrap_or_else(|| state.active_workspace());
    // Workspaces sit side by side, a screen and a gap apart, and the view slides between them.
    let step = (output_geo.size.w + motion::WORKSPACE_GAP) as f64;
    let camera = state.motion.camera(&name, wall);
    let active_dx = (active as f64 - camera) * step;
    let focused = state.focused_window();
    let used: Vec<bool> = (0..state.workspaces.count())
        .map(|i| !state.workspaces.get(i).is_empty())
        .collect();

    // Every screen tiles windows, draws a bar and runs the wallpaper; the code rain stays on
    // the leftmost screen, where the tiling area gives up the width for it.
    let tiling_output = screen_index.is_some();
    let rain_output = screen_index == Some(0);

    // Fading between the UI and the wallpaper, everything from here down to the wallpaper is drawn
    // at full strength into a pane of glass that passes through the wallpaper (`glass.rs`). The
    // pointer, already added, stays in front. Reduced motion keeps a plain fade.
    let glass = tiling_output
        && ui > 0.0
        && ui < 1.0
        && !state.clock.reduced_motion
        && chrome.glass.ready(renderer);
    let ui_alpha = if glass { 1.0 } else { ui };
    let pane_start = elements.len();
    // Other programs' launchers, pickers and panels in front of everything of the desktop's, and
    // their docks and wallpapers behind the windows, all inside the pane that fades. Not in
    // bullet time, which is the desktop's own.
    // A snip being chosen is in front of everything but the pointer; fragments of one just taken
    // fly in front of that.
    elements.extend(state.snip_flight_elements(renderer, output));
    let before_snip = elements.len();
    let mut snip = state.snip_elements(renderer, output, &mut chrome.overview, scale.x);
    elements.append(&mut snip);
    let snipping = elements.len() > before_snip;
    let layers_shown = tiling_output && ui > 0.0 && zoomed_out <= 0.0 && !snipping;
    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::FRONT,
            scale,
            ui_alpha,
        ));
    }

    // Bullet time leans the overview back like a floor, further than the mockup's 9° and in a
    // nearer perspective, and turns it a little while gliding between workspaces.
    let velocity = if state.clock.reduced_motion {
        0.0
    } else {
        (camera - state.motion.camera(&name, wall - 1.0 / 60.0)) * 60.0
    };
    let tilting = first_output && zoomed_out > 0.0 && chrome.stage.ready(renderer);
    let physical = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_else(|| output_geo.size.to_f64().to_physical(scale).to_i32_round());
    // Tilted, the overview is drawn into a texture denser than the screen and sampled back down,
    // which is what keeps a zoomed-out window's text sharp. Everything inside the tilt is built at
    // that density; the bar, the labels in front and the rest of the screen keep the screen's own.
    let dense = if tilting {
        tilt::supersample(physical)
    } else {
        1.0
    };
    let world_scale = Scale::from((scale.x * dense, scale.y * dense));
    let tilt = if tilting {
        Tilt {
            pitch: (15.0 * zoomed_out).to_radians(),
            yaw: (velocity * 2.0).clamp(-6.0, 6.0).to_radians() * zoomed_out.min(1.0),
            distance: 1.15 * output_geo.size.w as f64,
            centre,
            lift: 27.0 * zoomed_out,
            // A slight fish-eye lens over the tilted overview.
            lens: 0.05 * zoomed_out.min(1.0),
        }
    } else {
        Tilt {
            centre,
            ..Tilt::default()
        }
    };
    if first_output {
        state.overview_tilt = tilt;
    }
    // What the overview tilts, and its upright labels in front.
    let mut world = Vec::new();
    // A window dragged in bullet time: where its ghost goes, in front of the rest of the world.
    let mut front_ghost: Option<Rectangle<f64, Logical>> = None;
    let mut front = Vec::new();
    if first_output {
        // Ready before the explorer first opens.
        state.explorer.warm_icons(scale.x);
    }
    if first_output && state.explorer.is_open() {
        let running = state.running_apps();
        let ring = state.panel_ring();
        elements.extend(
            state
                .explorer
                .element(renderer, output_geo.size.w, scale.x, now, running, ring)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    if first_output && state.quick.is_open() {
        let facts = state.quick_facts();
        elements.extend(
            state
                .quick
                .element(renderer, output_geo.size.w, scale.x, now, facts)
                .map(OutputElement::Memory),
        );
    }
    if first_output && state.centre.is_open() {
        let facts = state.centre_facts();
        elements.extend(
            state
                .centre
                .element(
                    renderer,
                    output_geo.size,
                    scale.x,
                    now,
                    facts,
                    &state.notices,
                )
                .map(OutputElement::Memory),
        );
    }
    if first_output && state.history.is_open() {
        let ring = state.panel_ring();
        let now = state.wall();
        elements.extend(
            state
                .history
                .element(renderer, output_geo.size, scale.x, now, ring)
                .map(OutputElement::Memory),
        );
    }
    if first_output && state.sheet.is_open() {
        elements.extend(
            state
                .sheet
                .element(renderer, output_geo.size, scale.x, now, &state.bindings)
                .map(OutputElement::Memory),
        );
    }
    // Alt+Tab's switcher, on the screen the keyboard is on.
    if first_output && state.switcher.is_some() {
        elements.extend(
            state
                .switcher_element(renderer, output_geo.size, scale.x)
                .map(OutputElement::Memory),
        );
    }
    // The low battery card, above everything but the lock.
    if first_output && state.battery.card.is_some() {
        let mut card = state.battery.card.take();
        if let Some(card) = card.as_mut() {
            elements.extend(
                card.element(renderer, output_geo.size, scale.x, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.battery.card = card;
    }
    // The card asking what a screen never seen here should show. Drawn where the keyboard is, as
    // the other cards are: that is the screen being looked at when the lead goes in.
    if first_output && state.connect.is_some() {
        let mut connect = state.connect.take();
        if let Some(connect) = connect.as_mut() {
            elements.extend(
                connect
                    .element(renderer, output_geo.size, scale.x, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.connect = connect;
    }
    // The offer at login, above everything else it is asking about.
    if first_output && state.offer.is_some() {
        let mut offer = state.offer.take();
        if let Some(offer) = offer.as_mut() {
            elements.extend(
                offer
                    .element(renderer, output_geo.size, scale.x, wall)
                    .map(OutputElement::Memory),
            );
        }
        state.offer = offer;
    }
    // The share picker, on the screen the keyboard is on: the app waiting for it is usually the
    // one being looked at.
    if first_output && state.share.is_some() {
        let [r, g, b] = state
            .ring_rgb
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let ring = u32::from_be_bytes([r, g, b, 0xff]);
        let mut share = state.share.take();
        if let Some(share) = share.as_mut() {
            elements.extend(
                share
                    .element(renderer, output_geo.size, scale.x, wall, ring)
                    .map(OutputElement::Memory),
            );
        }
        state.share = share;
    }
    // The way out: the ask card in the middle of the screen, or the slim one while the apps are
    // closing. It draws whatever else is up, and the fade to black goes over everything below.
    if first_output && state.exit.is_some() {
        let ring = state.panel_ring();
        let mut exit = state.exit.take();
        if let Some(exit) = exit.as_mut() {
            elements.extend(
                exit.element(renderer, output_geo.size, scale.x, wall, ring)
                    .map(OutputElement::Memory),
            );
        }
        state.exit = exit;
    }
    // Where the pop-ups, the toast and the display below are drawn, for the pointer.
    let mut over_windows = Vec::new();
    // Notifications pop up at the top right, unless a window fills the screen.
    if first_output && ui > 0.0 && !state.fullscreen_on(active) {
        let (popups, expired) =
            state
                .notices
                .popup_elements(renderer, output_geo.size.w, scale.x, now, ui_alpha);
        over_windows.extend(popups.iter().map(|popup| popup.geometry(scale)));
        elements.extend(popups.into_iter().map(OutputElement::Memory));
        crate::notify::closed(expired, crate::notify::Reason::Expired);
    }
    if first_output && ui > 0.0 {
        // A gravity tag names the rung a window has just reached; the toast explaining that
        // rung drops below it rather than over it.
        let under_tag = state.tags.iter().any(|(_, _, at)| now - at < TAG_SHOWN);
        let toast = state
            .toast
            .element(renderer, output_geo.size.w, scale.x, now, under_tag);
        over_windows.extend(toast.iter().map(|toast| toast.geometry(scale)));
        elements.extend(toast.map(OutputElement::Memory));
        // The volume and brightness display sits low on the screen, over a fullscreen window as
        // well: a film is exactly when the volume keys are used.
        let ring = state.panel_ring();
        let osd = state
            .osd
            .element(renderer, output_geo.size, scale.x, ring, now);
        over_windows.extend(osd.iter().map(|osd| osd.geometry(scale)));
        elements.extend(osd.map(OutputElement::Memory));
    }
    if first_output {
        state.chrome_areas = over_windows
            .into_iter()
            .map(|area| {
                let mut area = area.to_f64().to_logical(scale);
                area.loc += output_geo.loc.to_f64();
                area
            })
            .collect();
    }
    // Bullet time's amber border round the whole screen, in front of the bar: the bar is part of
    // the slowed world while it's open, not something outside the frame.
    if first_output && zoomed_out > 0.0 && ui > 0.0 {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        let (w, h) = (output_geo.size.w as f64, output_geo.size.h as f64);
        let edge = Rectangle::new((2.0, 2.0).into(), (w - 4.0, h - 4.0).into());
        elements.extend(
            chrome
                .overview
                .outline(
                    edge,
                    0.0,
                    2,
                    overview::border(state.bullet_rgb),
                    scale,
                    fade,
                )
                .into_iter()
                .map(OutputElement::Solid),
        );
    }
    // Bullet time's keys, along the foot of the screen, fading with the overview.
    if first_output && zoomed_out > 0.0 && ui > 0.0 {
        if chrome
            .legend
            .as_ref()
            .is_none_or(|legend| legend.scale != scale.x)
        {
            chrome.legend = paint_legend(scale.x);
        }
        if let Some(legend) = chrome.legend.as_ref() {
            let at = Point::<f64, Logical>::from((
                ((output_geo.size.w - legend.logical.w) / 2) as f64,
                output_geo.size.h as f64 - 28.0 * 0.8 - legend.logical.h as f64,
            ));
            let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
            elements.extend(
                legend
                    .element(renderer, at, fade)
                    .map(OutputElement::Memory),
            );
        }
    }
    // The bar hides when a window on screen fills it. On the screen bullet time is drawn on, it
    // follows the overview: the workspace being looked at is highlighted, the one it started on is
    // ringed, and the title names what's chosen.
    if tiling_output && ui > 0.0 && !state.fullscreen_on(active) {
        let bullet = state.bullet.as_ref().filter(|_| first_output);
        let (highlighted, home) = bullet::bar_workspaces(active, bullet);
        let keyboard_here = first_output;
        let title = match bullet {
            Some(_) => state.bullet_bar_title(),
            None if keyboard_here => state.focused_title(),
            // Where the keyboard isn't, the window it would come back to on this screen.
            None => state
                .workspaces
                .get(active)
                .last_focus
                .as_ref()
                .filter(|window| state.workspaces.find(window) == Some(active))
                .map(crate::state::window_title)
                .unwrap_or_default(),
        };
        let content = bar::Content {
            active: highlighted,
            home,
            occupied: used.clone(),
            elsewhere: (0..state.workspaces.count())
                .map(|index| {
                    state
                        .screens
                        .showing(index)
                        .is_some_and(|showing| Some(showing) != screen_index)
                })
                .collect(),
            keyboard_here,
            labels: (0..state.workspaces.count())
                .map(|index| state.workspaces.label(index))
                .collect(),
            title,
            mode: state.bullet.as_ref().map(|_| {
                let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
                let [r, g, b] = state.bullet_rgb;
                (
                    "BULLET TIME",
                    (channel(r) << 24) | (channel(g) << 16) | (channel(b) << 8) | 0xff,
                )
            }),
            status: state.status.lock().unwrap().clone(),
            do_not_disturb: state.settings.notifications.do_not_disturb,
            unread: state.notices.unread(),
            sharing: state.captures.sharing(),
            caps_lock: state
                .seat
                .get_keyboard()
                .is_some_and(|keyboard| keyboard.modifier_state().caps_lock),
            meter: state.meter.lock().unwrap().clone(),
        };
        elements.extend(
            chrome
                .bar
                .element(renderer, content, output_geo.size.w, scale.x, ui_alpha)
                .map(OutputElement::Memory),
        );
    }
    // Bullet time's hints over the code rain's streams, on the screen the rain is drawn on, and
    // the vignette, behind the bar.
    if (first_output || rain_output) && zoomed_out > 0.0 && ui > 0.0 {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        if rain_output && state.bullet.is_some() {
            let screen = Rect {
                x: 0,
                y: 0,
                w: output_geo.size.w,
                h: output_geo.size.h,
            };
            let streams: Vec<(Window, Rect)> = state
                .rain
                .streams
                .iter()
                .enumerate()
                .map(|(index, stream)| {
                    (
                        stream.window.clone(),
                        Rain::column(index, screen, bar::HEIGHT),
                    )
                })
                .collect();
            for (window, column) in streams {
                let middle = Point::from((
                    column.x as f64 + column.w as f64 / 2.0,
                    column.y as f64 + 200.0,
                ));
                if let Some((_, label)) =
                    labels.iter().find(|(target, _)| *target.window() == window)
                {
                    elements.extend(
                        chrome
                            .overview
                            .hint(renderer, label, None, middle, scale.x, fade)
                            .map(OutputElement::Memory),
                    );
                }
                if matches!(&selected, Some(Target::Stream(chosen)) if *chosen == window) {
                    let around = Rectangle::new(
                        (column.x as f64, column.y as f64 + 4.0).into(),
                        (column.w as f64, column.h as f64 - 8.0).into(),
                    );
                    elements.extend(
                        chrome
                            .overview
                            .outline(
                                around,
                                3.0,
                                3,
                                overview::ring(state.bullet_rgb),
                                scale,
                                fade,
                            )
                            .into_iter()
                            .map(OutputElement::Solid),
                    );
                }
            }
        }
        elements.extend(
            chrome
                .overview
                .vignette(renderer, output_geo.size, scale.x, fade)
                .map(OutputElement::Memory),
        );
    }

    // The code rain, down the right-hand side.
    if rain_output && ui > 0.0 && !state.rain.is_empty() && !state.fullscreen_on(active) {
        elements.extend(
            state
                .rain
                .elements(
                    renderer,
                    output_geo.size,
                    bar::HEIGHT,
                    scale.x,
                    now,
                    ui_alpha,
                )
                .into_iter()
                .map(OutputElement::Memory),
        );
    }

    // Front to back. The space lists the workspace on screen bottom to top. During a switch,
    // the workspace sliding away is drawn too: its windows are unmapped but still alive.
    // A window's place is on the screen its workspace is laid out for; it's drawn where it sits on
    // that screen, as far from this screen's view as its workspace is. The ones on a workspace
    // another screen is showing land off the side of this one, which is where they belong.
    let offset = |index: usize| {
        let origin = state
            .screen_rect_for_workspace(index)
            .map_or(output_geo.loc.x, |rect| rect.x);
        shift_for_workspace(index, camera, step, output_geo.loc.x, origin)
    };
    let mut windows: Vec<(Window, f64)> = state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            let index = state.workspaces.find(window).unwrap_or(active);
            // Outside bullet time another screen's windows are that screen's alone: a wider
            // screen's workspace is wider than the gap between this screen's workspaces, so its
            // edge would reach in here.
            let elsewhere = state
                .screens
                .showing(index)
                .is_some_and(|showing| Some(showing) != screen_index);
            (!elsewhere || zoomed_out > 0.0).then(|| (window.clone(), offset(index)))
        })
        .collect();
    for index in
        (0..state.workspaces.count()).filter(|index| state.screens.showing(*index).is_none())
    {
        let dx = offset(index);
        // Zoomed out, neighbouring workspaces come into view.
        if ((index as f64 - camera) * step).abs() * zoom < step {
            // Front to back: the floating windows, front-most first, then the tiles.
            let ws = state.workspaces.get(index);
            let floating = ws.floating.windows().into_iter().rev();
            windows.extend(
                floating
                    .chain(ws.layout.windows())
                    .map(|window| (window, dx)),
            );
        }
    }
    // Windows just minimised, still pouring into their streams.
    windows.extend(
        state
            .rain
            .arriving(now)
            .into_iter()
            .map(|window| (window, active_dx)),
    );
    // Fully faded, only the wallpaper is drawn.
    if ui <= 0.0 {
        windows.clear();
    }
    state
        .tags
        .retain(|(window, _, at)| now - at < TAG_SHOWN && window.alive());
    let mut dimmed = 0;
    // Windows drawn here that the space doesn't hold get frame callbacks too (`drawn_off_space`).
    for (window, _) in &windows {
        if state.space.element_geometry(window).is_none() && !state.drawn_off_space.contains(window)
        {
            state.drawn_off_space.push(window.clone());
        }
    }
    for (window, dx) in windows {
        let own = window.geometry();
        // A window is kept inside the tiling area of the screen its own workspace is on, which
        // is not this screen's when it belongs to the one next door.
        let home_index = state.workspaces.find(&window);
        let area_global = home_index
            .and_then(|index| state.area_for_workspace(index))
            .or_else(|| state.output_area())
            .map(|area| [area.x as f64, area.y as f64, area.w as f64, area.h as f64]);
        let frame = match state.motion.frame(&window, now) {
            Some(frame) => frame,
            // X11 menus and tooltips place themselves and don't animate.
            None => match state.space.element_geometry(&window) {
                Some(geo) => motion::Frame::still([
                    geo.loc.x as f64,
                    geo.loc.y as f64,
                    geo.size.w as f64,
                    geo.size.h as f64,
                ]),
                None => continue,
            },
        };
        // At rest a window is drawn at its own size. While its tile moves it's stretched to the
        // tile's animated size, since the client redraws at the new size only a frame or two in.
        let [x, y, w, h] = frame.rect;
        // Set when the window is drawn at rest at other than its own size.
        let mut fitted = false;
        let [x, y, w, h] = if frame.moving || awaiting_size(&window) {
            // A client that hasn't yet answered the size it was last sent is still catching up
            // (a slow app, a gap being dragged, a hidden workspace coming back): it's stretched to
            // its tile, as while moving, rather than drawn at its old size with a band of the tile
            // empty or spilling over a neighbour.
            [x, y, w, h]
        } else {
            let at_rest = [x, y, own.size.w as f64, own.size.h as f64];
            let fullscreen = state.is_fullscreen(&window);
            match area_global {
                // Zoomed out, a window bigger than its tile (a client with a minimum size, or a
                // fullscreen one) shrinks to fit, so it stays inside its workspace's frame.
                Some(area) if zoomed_out > 0.0 => {
                    let fit = fit_within(at_rest, [x, y, w, h], area);
                    let t = zoomed_out.min(1.0);
                    std::array::from_fn(|i| at_rest[i] + (fit[i] - at_rest[i]) * t)
                }
                // A client that won't go as small as its tile — an app with a minimum width wider
                // than the tiling could give it, or one squeezed by the code rain taking width
                // from the right — would otherwise overlap its neighbour or be drawn under the
                // rain. It shrinks into the tile instead, so it and its focus ring stay inside.
                // A fullscreen window is not in the area at all.
                Some(area) if !fullscreen => {
                    let scaled = fit_within(at_rest, [x, y, w, h], area);
                    // Rounding at a fractional scale leaves a window a pixel or two over an edge.
                    // Rescaling the whole window for that is far more visible than the overhang
                    // — it flickers every time the client redraws at a size of its own — so only
                    // a real refusal to shrink counts.
                    if at_rest[2] - scaled[2] > SLACK || at_rest[3] - scaled[3] > SLACK {
                        tracing::debug!(
                            own = ?at_rest,
                            ?area,
                            drawn = ?scaled,
                            "a window too big for its tile is drawn scaled into it"
                        );
                        fitted = true;
                        scaled
                    } else {
                        at_rest
                    }
                }
                _ => at_rest,
            }
        };
        let s = frame.scale;
        let shown = Rectangle::<f64, Logical>::new(
            (
                x + dx - output_geo.loc.x as f64 + w * (1.0 - s) / 2.0,
                y - output_geo.loc.y as f64 + h * (1.0 - s) / 2.0,
            )
                .into(),
            (w * s, h * s).into(),
        );
        let alpha = frame.alpha as f32 * ui_alpha;
        // In bullet time windows are drawn smaller around the screen's centre, each also shrunk a
        // little about its own centre so the gaps between them open up inside their frame.
        let shown = if zoomed_out > 0.0 {
            let k = 0.06 * zoomed_out;
            to_overview(Rectangle::new(
                (
                    shown.loc.x + shown.size.w * k / 2.0,
                    shown.loc.y + shown.size.h * k / 2.0,
                )
                    .into(),
                (shown.size.w * (1.0 - k), shown.size.h * (1.0 - k)).into(),
            ))
        } else {
            shown
        };
        if zoomed_out > 0.0 && first_output {
            if state.bullet.is_some() {
                if let Some((_, label)) =
                    labels.iter().find(|(target, _)| *target.window() == window)
                {
                    let name = state.window_name(&window);
                    // A floating window's letter sits just above it, so a dialog centred over its
                    // parent doesn't cover the parent's.
                    let down = if state.workspaces.is_floating(&window) {
                        -30.0
                    } else {
                        shown.size.h / 2.0
                    };
                    let flat = (shown.loc.x + shown.size.w / 2.0, shown.loc.y + down);
                    let middle = Point::from(tilt.project(flat).unwrap_or(flat));
                    front.extend(
                        chrome
                            .overview
                            .hint(renderer, label, Some(&name), middle, scale.x, alpha)
                            .map(OutputElement::Memory),
                    );
                }
                // The window under the pointer shows its close button, in the flat overview so it
                // tilts with its window.
                let hovered = state
                    .bullet
                    .as_ref()
                    .filter(|mode| mode.pointer.drag.is_none())
                    .filter(|mode| mode.pointer.hover.as_ref() == Some(&window))
                    .map(|mode| mode.pointer.on_close);
                if let Some(hot) = hovered {
                    world.extend(
                        chrome
                            .overview
                            .close_button(
                                renderer,
                                bullet::close_button(shown),
                                hot,
                                world_scale.x,
                                alpha,
                            )
                            .map(OutputElement::Memory),
                    );
                }
                // A window being dragged leaves a ghost of itself, half its size, at the pointer.
                if let Some(drag) = state
                    .bullet
                    .as_ref()
                    .and_then(|mode| mode.pointer.drag.as_ref())
                    .filter(|drag| drag.window == window)
                {
                    let (w, h) = (shown.size.w / 2.0, shown.size.h / 2.0);
                    let ghost = Rectangle::new(
                        (drag.at.x - w / 2.0, drag.at.y - h / 2.0).into(),
                        (w, h).into(),
                    );
                    front_ghost = Some(ghost);
                }
                if matches!(&selected, Some(Target::Window(chosen)) if *chosen == window) {
                    // Heavier than the viewed frame's 3 px, with a faint second line further out
                    // for a glow, so the two never read as one double line.
                    let ring = overview::ring(state.bullet_rgb);
                    world.extend(
                        chrome
                            .overview
                            .outline(shown, 6.0, 6, ring, world_scale, alpha)
                            .into_iter()
                            .chain(chrome.overview.outline(
                                shown,
                                14.0 + 6.0,
                                1,
                                ring,
                                world_scale,
                                alpha * 0.25,
                            ))
                            .map(OutputElement::Solid),
                    );
                }
                state.overview_hits.push((window.clone(), shown));
            }
            // Workspaces not being looked at are dimmed.
            if home_index != Some(viewed_index) {
                let dim = alpha * 0.55 * zoomed_out.min(1.0) as f32;
                let shade =
                    chrome
                        .overview
                        .solid(shown.to_i32_round(), overview::SHADE, world_scale, dim);
                world.push(OutputElement::Solid(shade));
            }
        }
        // Gravity: the rung's tag in front, then the distant shade.
        if let Some((_, rung, at)) = state.tags.iter().find(|(w, ..)| *w == window).cloned() {
            world.extend(
                chrome
                    .tag(
                        renderer,
                        rung,
                        shown,
                        now - at,
                        world_scale,
                        state.clock.reduced_motion,
                    )
                    .map(OutputElement::Memory),
            );
        }
        let rung = home_index.map(|index| state.workspaces.get(index).gravity.rung(&window));
        if rung == Some(Rung::Distant) {
            let shade = chrome.dim(dimmed, shown.to_i32_round(), world_scale, alpha);
            world.push(OutputElement::Solid(shade));
            dimmed += 1;
        }
        // A window dragged with Super held outlines the tile it would swap with.
        if state.swap_target() == Some(&window) {
            let colour = overview::ring(state.ring_rgb);
            world.extend(
                chrome
                    .overview
                    .outline(shown, 0.0, 2, colour, world_scale, alpha * 0.6)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        // In bullet time the selection ring takes over from the focus ring.
        if focused.as_ref() == Some(&window) {
            let alpha = alpha * (1.0 - zoomed_out.min(1.0) as f32);
            let colour = overview::ring(state.ring_rgb);
            world.extend(
                chrome
                    .ring(shown.to_i32_round(), colour, world_scale, alpha)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        let location = (shown.loc - own.loc.to_f64())
            .to_physical(world_scale)
            .to_i32_round();
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            AsRenderElements::<GlesRenderer>::render_elements(
                &window,
                renderer,
                location,
                world_scale,
                alpha,
            );
        // A handle on what it was just drawn from, so it can fade out if it closes.
        if let Some(picture) = crate::ghost::Picture::of(&window, &renderer.context_id()) {
            match state
                .pictures
                .iter_mut()
                .find(|(drawn, _)| *drawn == window)
            {
                Some((_, kept)) => *kept = picture,
                None => state.pictures.push((window.clone(), picture)),
            }
        }
        if frame.waiting && !surfaces.is_empty() {
            state.motion.shown(&window, now);
        }
        // Only a window at rest on the screen it is on takes the pointer where it is drawn.
        if fitted && s == 1.0 && zoomed_out <= 0.0 && dx == 0.0 {
            let global = Rectangle::new(shown.loc + output_geo.loc.to_f64(), shown.size);
            match state.fitted.iter_mut().find(|(w, _)| *w == window) {
                Some((_, drawn)) => *drawn = global,
                None => state.fitted.push((window.clone(), global)),
            }
        } else if !fitted {
            state.fitted.retain(|(w, _)| *w != window);
        }
        if (frame.moving || fitted || s != 1.0 || zoom != 1.0) && own.size.w > 0 && own.size.h > 0 {
            let pivot = shown.loc.to_physical(world_scale).to_i32_round();
            let stretch = Scale::from((
                shown.size.w / own.size.w as f64,
                shown.size.h / own.size.h as f64,
            ));
            world.extend(surfaces.into_iter().map(|surface| {
                OutputElement::Rescaled(RescaleRenderElement::from_element(surface, pivot, stretch))
            }));
        } else {
            world.extend(surfaces.into_iter().map(OutputElement::Surface));
        }
    }

    // Bullet time: every workspace in view gets its frame, number and caption, behind its windows.
    if let (true, true, Some(area)) = (first_output && ui > 0.0, zoomed_out > 0.0, area_local) {
        let fade = zoomed_out.min(1.0) as f32 * ui_alpha;
        let home = state.bullet.as_ref().map(|mode| mode.home);
        for index in 0..state.workspaces.count() {
            let dx = (index as f64 - camera) * step;
            if dx.abs() * zoom > output_geo.size.w as f64 {
                continue;
            }
            let frame = to_overview(Rectangle::new(
                (area.loc.x + dx - 10.0, area.loc.y - 10.0).into(),
                (area.size.w + 20.0, area.size.h + 20.0).into(),
            ));
            let viewed = index == viewed_index;
            // The frame a dragged window would land in lights up.
            let drop_target = state
                .bullet
                .as_ref()
                .and_then(|mode| mode.pointer.drag.as_ref())
                .is_some_and(|drag| drag.over == Some(index));
            let (colour, thickness) = if viewed || drop_target {
                (overview::frame_viewed(state.bullet_rgb), 3)
            } else {
                (overview::FRAME, 2)
            };
            world.extend(
                chrome
                    .overview
                    .outline(frame, 0.0, thickness, colour, world_scale, fade)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
            let count = state.workspaces.get(index).len();
            let mut caption = bullet::window_count(count);
            if home == Some(index) {
                caption.push_str(" · you started here");
            }
            let at = Point::from((frame.loc.x + 4.0, frame.loc.y - 46.0));
            world.extend(
                chrome
                    .overview
                    .workspace_label(
                        renderer,
                        &state.workspaces.label(index),
                        viewed,
                        &caption,
                        at,
                        world_scale.x,
                        fade,
                    )
                    .map(OutputElement::Memory),
            );
            if count == 0 {
                // High in the frame, clear of the wallpaper's logo in the middle of the screen.
                let middle = Point::from((
                    frame.loc.x + frame.size.w / 2.0,
                    frame.loc.y + frame.size.h * 0.15,
                ));
                world.extend(
                    chrome
                        .overview
                        .empty_note(renderer, index + 1, middle, world_scale.x, fade)
                        .map(OutputElement::Memory),
                );
            }
            if state.bullet.is_some() {
                state.frame_hits.push((index, frame));
            }
        }
    }

    // Closed windows fade in front of the windows retiling into their space. Not in bullet
    // time's overview, which draws windows where no ghost was captured.
    if zoomed_out <= 0.0 {
        let context = renderer.context_id();
        let screen = output_geo.to_f64();
        let pieces: Vec<OutputElement> = state
            .ghosts
            .iter()
            .filter(|ghost| ghost.overlaps(screen))
            .flat_map(|ghost| ghost.elements(&context, screen.loc, scale, now))
            .map(OutputElement::Texture)
            .collect();
        world.splice(0..0, pieces);
    }
    state.ghosts.retain(|ghost| !ghost.done(now));
    state.pictures.retain(|(window, _)| window.alive());
    if let Some(ghost) = front_ghost {
        let alpha = 0.8 * zoomed_out.min(1.0) as f32 * ui_alpha;
        let edges = chrome.overview.outline(
            ghost,
            0.0,
            2,
            overview::ring(state.bullet_rgb),
            world_scale,
            alpha,
        );
        world.splice(0..0, edges.into_iter().map(OutputElement::Solid));
    }
    // The upright labels go in front, then the windows and frames: tilted while bullet time is
    // zoomed out, as they are otherwise.
    elements.append(&mut front);
    let tilted = if tilting && !world.is_empty() {
        let fade = zoomed_out.min(1.0) as f32;
        let look = tilt::Look {
            fog: 0.4 * fade,
            mirror: mirror_line.map_or((0.0, 0.0, 0.0), |(y, depth)| {
                (y, depth, 0.2 * fade * ui_alpha)
            }),
        };
        chrome.stage.element(
            renderer,
            &world,
            output_geo.size,
            physical,
            scale.x,
            dense,
            &tilt,
            &look,
        )
    } else {
        None
    };
    match tilted {
        Some(element) => elements.push(OutputElement::Shaded(element)),
        None => elements.append(&mut world),
    }

    // The keyboard-tips prompt, unless the explorer or bullet time is open over it.
    if tiling_output
        && ui > 0.0
        && zoomed_out <= 0.0
        && !used[active]
        && !state.explorer.is_open()
        && !state.sheet.is_open()
    {
        elements.extend(
            chrome
                .tips(
                    renderer,
                    output_geo.size,
                    active_dx,
                    scale,
                    ui_alpha,
                    (!state.clock.reduced_motion).then_some(wall),
                )
                .map(OutputElement::Memory),
        );
    }

    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::BACK,
            scale,
            ui_alpha,
        ));
    }

    // Where the turning page sits in the list, for the wallpaper to lie over it behind the glass.
    let mut fallen_at = None;
    if glass {
        let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
        match chrome.glass.element(
            renderer,
            &pane,
            output_geo.size,
            physical,
            scale.x,
            1.0 - ui,
        ) {
            Some(element) => {
                fallen_at = Some(elements.len());
                elements.push(OutputElement::Shaded(element));
            }
            // Drawn plainly this once; a glass that's broken stays off from now on.
            None => elements.extend(pane),
        }
    }

    // Coming back from the lock, the desktop is drawn whole into a texture that grows into place
    // and brightens, while the lock's card and veil fade in front of it.
    if tiling_output && !glass {
        if let Some((zoom, opacity)) = state.unlocking.as_ref().map(|u| u.desktop(wall)) {
            let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
            match unlock_zoom(
                renderer,
                &mut chrome,
                &pane,
                output_geo.size,
                physical,
                scale.x,
                zoom,
                opacity,
            ) {
                Some(element) => elements.push(OutputElement::Texture(element)),
                None => elements.extend(pane),
            }
        }
    }
    if let Some(unlocking) = state.unlocking.as_mut() {
        let (alpha, _) = unlocking.card_state(wall);
        let mut lock: Vec<OutputElement> = Vec::new();
        if first_output {
            lock.extend(
                unlocking
                    .elements(renderer, output_geo.size, scale.x, wall)
                    .into_iter()
                    .map(OutputElement::Memory),
            );
        }
        if tiling_output && alpha > 0.0 {
            lock.push(veil(&mut chrome, output_geo.size, scale, alpha));
        }
        elements.splice(pane_start..pane_start, lock);
    }

    // The living wallpaper, behind everything: dim while the UI is up, full once it fades. It
    // holds still under a window that fills the screen.
    if tiling_output {
        let paused = state.fullscreen_on(active) && ui >= 1.0;
        let glow = 0.45 + 0.55 * (1.0 - ui);
        // Slowed for bullet time, a wallpaper step lasts many frames, so it tweens between them.
        let tween = state.clock.in_bullet_time();
        // The clock, the date, the workspace and the battery, for variations that show them.
        let readings = {
            let status = state.status.lock().unwrap();
            crate::saver::Readings {
                time: status.time.clone(),
                date: status.date.clone(),
                place: state.workspaces.label(active),
                battery: status.battery,
            }
        };
        saver.set_readings(&readings);
        elements.extend(
            saver
                .element(renderer, output_geo.size, scale.x, now, glow, paused, tween)
                .map(OutputElement::Memory),
        );
        // The same wallpaper over the page, wherever the page has turned behind the glass.
        if let Some(at) = fallen_at {
            let through = saver
                .again(renderer, output_geo.size, glow)
                .and_then(|wallpaper| {
                    chrome.glass.through(
                        renderer,
                        wallpaper,
                        output_geo.size,
                        physical,
                        scale.x,
                        1.0 - ui,
                        BACKGROUND,
                    )
                });
            if let Some(through) = through {
                elements.insert(at, OutputElement::Shaded(through));
            }
        }
    }
    // A screenshot's flash, over the whole screen it was taken of.
    if let Some(alpha) = state.flash_alpha(&name) {
        chrome.flash.update(output_geo.size, WHITE);
        elements.insert(
            0,
            OutputElement::Solid(SolidColorRenderElement::from_buffer(
                &chrome.flash,
                (0, 0),
                scale,
                alpha,
                Kind::Unspecified,
            )),
        );
    }
    // The last thing on the way out, over everything the screen holds, the bar included.
    if tiling_output {
        let alpha = state.exit.as_ref().map_or(0.0, |exit| exit.blackout(wall));
        if alpha > 0.0 {
            chrome.blackout.update(output_geo.size, BLACK);
            elements.insert(
                0,
                OutputElement::Solid(SolidColorRenderElement::from_buffer(
                    &chrome.blackout,
                    (0, 0),
                    scale,
                    alpha,
                    Kind::Unspecified,
                )),
            );
        }
    }
    state.chromes.insert(name.clone(), chrome);
    state.savers.insert(name, saver);
    elements
}

/// The lock's veil over a whole screen.
fn veil(
    chrome: &mut Chrome,
    size: Size<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> OutputElement {
    chrome.veil.update(size, veil_colour());
    OutputElement::Solid(SolidColorRenderElement::from_buffer(
        &chrome.veil,
        (0, 0),
        scale,
        alpha,
        Kind::Unspecified,
    ))
}

/// Everything on a screen while it's locked, front to back: the plain pointer, then on the
/// focused screen the volume and brightness display and the lock's card, then the veil and the
/// living wallpaper. No window, pop-up, toast, bar or client cursor is read.
#[allow(clippy::too_many_arguments)]
fn lock_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geo: Rectangle<i32, Logical>,
    chrome: &mut Chrome,
    saver: &mut crate::saver::Saver,
    cursor: Option<&mut Cursor>,
    scale: Scale<f64>,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    let now = state.clock.now();
    let wall = state.wall();
    if let Some(cursor) = cursor {
        let pointer = state.pointer_location();
        if output_geo.to_f64().contains(pointer) {
            let pos = pointer - output_geo.loc.to_f64();
            let (buffer, hotspot) = cursor.image(
                CursorIcon::Default.name(),
                scale.x,
                state.start_time.elapsed(),
            );
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (pos - hotspot).to_physical(scale),
                &buffer,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                Ok(element) => elements.push(OutputElement::Memory(element)),
                Err(err) => tracing::warn!("couldn't draw the pointer: {err:?}"),
            }
        }
    }
    if state.screens.focused_output().as_ref() == Some(output) {
        let ring = state.panel_ring();
        elements.extend(
            state
                .osd
                .element(renderer, output_geo.size, scale.x, ring, now)
                .map(OutputElement::Memory),
        );
        let facts = state.lock_facts();
        if let Some(lock) = state.lock.as_mut() {
            elements.extend(
                lock.elements(renderer, output_geo.size, scale.x, &facts, wall)
                    .into_iter()
                    .map(OutputElement::Memory),
            );
        }
    }
    let shown = state.lock.as_ref().map_or(1.0, |lock| lock.shown(wall));
    elements.push(veil(chrome, output_geo.size, scale, shown));
    if let Some(index) = state.screens.index_of(output) {
        let readings = {
            let status = state.status.lock().unwrap();
            crate::saver::Readings {
                time: status.time.clone(),
                date: status.date.clone(),
                place: state
                    .screens
                    .get(index)
                    .map(|screen| state.workspaces.label(screen.workspace))
                    .unwrap_or_default(),
                battery: status.battery,
            }
        };
        saver.set_readings(&readings);
        elements.extend(
            saver
                .element(
                    renderer,
                    output_geo.size,
                    scale.x,
                    now,
                    crate::lock::WALLPAPER_GLOW,
                    false,
                    false,
                )
                .map(OutputElement::Memory),
        );
    }
    elements
}

/// `pane`, the desktop, drawn into the screen's unlock texture and shown `zoom` of its size about
/// the screen's centre at `opacity`. `None` when the texture can't be made, and the desktop is
/// drawn plainly instead.
#[allow(clippy::too_many_arguments)]
fn unlock_zoom(
    renderer: &mut GlesRenderer,
    chrome: &mut Chrome,
    pane: &[OutputElement],
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
    scale: f64,
    zoom: f64,
    opacity: f32,
) -> Option<TextureRenderElement<GlesTexture>> {
    if chrome.unlock_broken {
        return None;
    }
    tilt::draw_offscreen(
        renderer,
        &mut chrome.unlock,
        &mut chrome.unlock_broken,
        pane,
        logical,
        physical,
        scale,
        "the unlock",
    )?;
    let (texture, _) = chrome.unlock.as_ref()?;
    let shown = Size::<i32, Logical>::from((
        (logical.w as f64 * zoom).round() as i32,
        (logical.h as f64 * zoom).round() as i32,
    ));
    let at = Point::<f64, Logical>::from((
        (logical.w - shown.w) as f64 / 2.0,
        (logical.h - shown.h) as f64 / 2.0,
    ))
    .to_physical(scale);
    Some(TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        at,
        texture.clone(),
        1,
        Transform::Normal,
        Some(opacity),
        Some(Rectangle::from_size(
            (physical.w as f64, physical.h as f64).into(),
        )),
        Some(shown),
        None,
        Kind::Unspecified,
    ))
}

/// How far a window may overhang the tiling area before it is scaled into it: enough for the
/// rounding at a fractional output scale, nowhere near a client that won't shrink.
const SLACK: f64 = 4.0;

/// Where a window drawn at `rect` goes to stay inside both its `tile` and the tiling `area`:
/// scaled down evenly (never up) and moved in from any edge it crosses. Rects are x, y, w, h.
/// Whether `window` has been sent a size it hasn't acked and drawn yet.
fn awaiting_size(window: &Window) -> bool {
    use smithay::wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData};
    let Some(toplevel) = window.toplevel() else {
        return false;
    };
    let sent = with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .ok()
                    .map(|data| data.current_server_state().size)
            })
    })
    .flatten();
    let acked = toplevel.with_committed_state(|state| state.and_then(|state| state.size));
    sent.is_some() && sent != acked
}

fn fit_within(rect: [f64; 4], tile: [f64; 4], area: [f64; 4]) -> [f64; 4] {
    let (left, top) = (tile[0].max(area[0]), tile[1].max(area[1]));
    let right = (tile[0] + tile[2]).min(area[0] + area[2]);
    let bottom = (tile[1] + tile[3]).min(area[1] + area[3]);
    let (room_w, room_h) = ((right - left).max(1.0), (bottom - top).max(1.0));
    let k = (room_w / rect[2]).min(room_h / rect[3]).min(1.0);
    let (w, h) = (rect[2] * k, rect[3] * k);
    [
        rect[0].min(right - w).max(left),
        rect[1].min(bottom - h).max(top),
        w,
        h,
    ]
}

/// Draws `elements` into an offscreen texture and saves it as a PNG, for `shot:` debug steps.
/// `scale` must be the output's: client surfaces size themselves by it when drawn.
pub fn save_png(
    renderer: &mut GlesRenderer,
    elements: &[OutputElement],
    size: Size<i32, Physical>,
    scale: f64,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = copy_frame(renderer, elements, size, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    encode_png(file, &pixels, size.w as u32, size.h as u32)?;
    Ok(())
}

/// Draws `elements` into an offscreen texture and reads it back as opaque RGBA bytes, a row at a
/// time from the top. `scale` must be the output's.
pub fn copy_frame<E: smithay::backend::renderer::element::RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    size: Size<i32, Physical>,
    scale: f64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let buffer_size = Size::<i32, Buffer>::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let mut target = renderer.bind(&mut texture)?;
    let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
    damage.render_output(renderer, &mut target, 0, elements, BACKGROUND)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    let mut pixels = renderer.map_texture(&mapping)?.to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    Ok(pixels)
}

/// RGBA bytes, `w` × `h`, as a PNG written to `out`.
pub fn encode_png(
    out: impl std::io::Write,
    pixels: &[u8],
    w: u32,
    h: u32,
) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_screens_windows_are_drawn_off_this_one_whichever_side_its_workspace_is() {
        // The laptop (1536 wide, at 0) and a monitor (1920 wide, at 1536).
        let step = (1536 + motion::WORKSPACE_GAP) as f64;
        // A window at the monitor's left edge, drawn on the laptop.
        let drawn_at =
            |index: usize, camera: f64| 1536.0 + shift_for_workspace(index, camera, step, 0, 1536);
        // The laptop on workspace 2, the monitor on workspace 1: a lower workspace, so to the left.
        assert!(drawn_at(0, 1.0) < 0.0, "{}", drawn_at(0, 1.0));
        // The laptop on workspace 1, the monitor on workspace 3: off the right.
        assert!(drawn_at(2, 0.0) >= 1536.0, "{}", drawn_at(2, 0.0));
        // Its own workspace sits where the layout put it, on its own screen's edge.
        assert_eq!(0.0 + shift_for_workspace(1, 1.0, step, 0, 0), 0.0);
    }

    #[test]
    fn with_reduced_motion_a_tier_tag_appears_where_it_rests() {
        for age in [0.0, 0.02, 0.05, 0.1, 0.5, 1.0, 1.75] {
            assert_eq!(tag_motion(age, true).1, 0.0, "no drop at {age} s");
        }
        assert_eq!(
            tag_motion(0.1, true).0,
            1.0,
            "fully in after the short fade"
        );
        assert!(
            tag_motion(0.05, false).1 < -5.0,
            "without it, the tag still drops"
        );
        assert_eq!(tag_motion(0.5, false), (1.0, 0.0));
    }

    #[test]
    fn a_window_too_big_for_its_tile_shrinks_into_it_and_its_workspace() {
        let area = [0.0, 40.0, 1000.0, 600.0];
        // A client that won't go narrower than 700 in a 490-wide tile.
        let tile = [510.0, 40.0, 490.0, 600.0];
        let [x, y, w, h] = fit_within([510.0, 40.0, 700.0, 600.0], tile, area);
        assert!(x >= 510.0 && x + w <= 1000.0 + 1e-9, "inside the tile");
        assert!((w / h - 700.0 / 600.0).abs() < 1e-9, "it keeps its shape");
        assert_eq!(y, 40.0);

        // Fullscreen covers the bar above the area.
        let screen = [0.0, 0.0, 1000.0, 640.0];
        let [_, y, _, h] = fit_within(screen, screen, area);
        assert!(y >= 40.0 && y + h <= 640.0 + 1e-9, "below the bar");

        // The code rain takes width from the right: a client that won't shrink is brought inside
        // the area rather than drawn under the glyphs.
        let narrowed = [0.0, 40.0, 760.0, 600.0];
        let wide = [0.0, 40.0, 1000.0, 600.0];
        let [x, _, w, _] = fit_within(wide, wide, narrowed);
        assert!(x + w <= 760.0 + 1e-9, "clear of the streams");
        assert!(w < 1000.0, "scaled down, not cropped");

        assert_eq!(
            fit_within(tile, tile, area),
            tile,
            "a window that fits stays put"
        );
    }

    #[test]
    fn the_key_hint_is_painted_at_the_screen_scale() {
        let (pixmap, size) = paint_tips(1.25).unwrap();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            (
                (size.w as f64 * 1.25).round() as u32,
                (size.h as f64 * 1.25).round() as u32
            )
        );
        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] == 255));
    }
}

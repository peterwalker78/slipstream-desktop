//! What each screen keeps between frames for drawing the desktop's own parts, and the pieces
//! it paints: the arrangement tag, bullet time's key legend and the keyboard-tips prompt.

use super::*;

/// A painted surface: the scale it was painted at, its buffer, and its size in logical pixels.
pub(super) type Painted = (f64, MemoryRenderBuffer, Size<i32, Logical>);

/// One screen's buffers for what Slipstream draws itself. They live between frames, so damage
/// tracking redraws only changes.
pub struct Chrome {
    pub bar: Bar,
    pub(super) ring: [SolidColorBuffer; 4],
    /// One per distant window drawn this frame.
    pub(super) dims: Vec<SolidColorBuffer>,
    pub(super) tags: HashMap<Rung, Painted>,
    /// The keyboard-tips prompt on an empty workspace, and the key it names.
    pub(super) tips: Option<Painted>,
    tips_key: String,
    /// Bullet time's key legend along the foot of the screen.
    pub(super) legend: Option<paint::Painted>,
    /// Bullet time's overlays.
    pub overview: Overview,
    /// Bullet time's 3D.
    pub(super) stage: tilt::Stage,
    /// The UI's glass fade to the wallpaper and back.
    pub(super) glass: crate::glass::Glass,
    /// The same for the bar and the pane docked to it, which fade as a nearer pane of their own.
    pub(super) glass_bar: crate::glass::Glass,
    /// Where the docked window's slot was last drawn in the bar, for the slot of a window on its
    /// way down: its left edge and width in logical pixels.
    pub(super) slot_at: Option<(f64, f64)>,
    /// The fade to black on the way out.
    pub(super) blackout: SolidColorBuffer,
    /// The white flash when a screenshot is taken.
    pub(super) flash: SolidColorBuffer,
    /// The lock's veil over the wallpaper.
    pub(super) veil: SolidColorBuffer,
    /// The desktop as it comes back from behind the lock, and whether that texture failed.
    pub(super) unlock: Option<tilt::Canvas>,
    pub(super) unlock_broken: bool,
    /// Windows drawn as panes of glass: Alt+Tab's deck and tiles passing through each other.
    pub(super) panes: crate::pane::Panes,
    /// The bar's material round the docked pane with the shadow the piece casts, for each pane
    /// size painted lately: the one at rest, and the one a pane changing size is heading for.
    pub(super) dock_frames: Vec<((i32, i32), paint::Painted)>,
    /// A closed window falling away as code.
    pub(super) fx: crate::fx::Fx,
    /// The dark behind Alt+Tab's deck.
    pub(super) deck_dim: SolidColorBuffer,
    /// The name under the deck's front pane, and what it says.
    pub(super) deck_label: Option<(String, paint::Painted)>,
    /// The shadow floating windows cast, painted once round a small square and cut up to fit
    /// each window (`window_shadow`).
    pub(super) window_shadow: Option<paint::Painted>,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            bar: Bar::default(),
            ring: std::array::from_fn(|_| SolidColorBuffer::new((0, 0), FOCUS)),
            dims: Vec::new(),
            tags: HashMap::new(),
            tips: None,
            tips_key: String::new(),
            legend: None,
            overview: Overview::default(),
            stage: tilt::Stage::default(),
            glass: crate::glass::Glass::default(),
            glass_bar: crate::glass::Glass::default(),
            slot_at: None,
            blackout: SolidColorBuffer::new((0, 0), BLACK),
            panes: crate::pane::Panes::default(),
            dock_frames: Vec::new(),
            fx: crate::fx::Fx::default(),
            deck_dim: SolidColorBuffer::new((0, 0), BLACK),
            deck_label: None,
            flash: SolidColorBuffer::new((0, 0), WHITE),
            veil: SolidColorBuffer::new((0, 0), veil_colour()),
            unlock: None,
            unlock_broken: false,
            window_shadow: None,
        }
    }
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
    pub(super) fn dim(
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
    pub(super) fn tag<R>(
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

    pub(super) fn ring(
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

    /// The keyboard-tips prompt, floating in the lower third of `view`'s empty workspace, which
    /// slides with it. `drifting` is wall time, to float and breathe by, and
    /// `None` under reduced motion, which holds it still at full strength: it drifts a few pixels
    /// up and down and its glow breathes, so it reads as hovering rather than stuck to the
    /// wallpaper. Painted again whenever the scale changes.
    pub(super) fn tips<R>(
        &mut self,
        renderer: &mut R,
        view: &View,
        drifting: Option<f64>,
        key: &str,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (screen, dx, scale, alpha) = (view.geo.size, view.active_dx, view.scale, view.ui_alpha);
        let stale = self
            .tips
            .as_ref()
            .is_none_or(|(painted_at, ..)| *painted_at != scale.x);
        if stale || self.tips_key != key {
            let (pixmap, size) = paint_tips(scale.x, key)?;
            self.tips = Some((scale.x, paint::buffer(&pixmap), size));
            self.tips_key = key.to_string();
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

/// A tier tag's opacity and how far above its resting place it is, in design pixels, `age`
/// seconds after it appeared: it drops 8 px into place while fading in, holds, and fades out.
/// Reduced motion leaves out the drop and shortens both fades.
pub(super) fn tag_motion(age: f64, reduced_motion: bool) -> (f64, f64) {
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

/// A rung's tag: its name in amber, and the ladder beside it as a row of pips with this rung's
/// filled, so where the window is on the ladder reads at a glance. Laid out in design pixels.
fn paint_tag(rung: Rung, scale: f64) -> Option<(Pixmap, Size<i32, Logical>)> {
    const PIP: f32 = 7.0;
    const PIP_GAP: f32 = 4.0;
    const INK: u32 = panel::DARK;
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
        Size::<i32, Logical>::from(((w * DESIGN_PX).ceil() as i32, (h * DESIGN_PX).ceil() as i32));
    let mut p = Painter::new(
        (size.w as f64 * scale).round() as u32,
        (size.h as f64 * scale).round() as u32,
        scale as f32 * DESIGN_PX,
    )?;
    p.fill(0.0, 0.0, w, h, 6.0, panel::AMBER);
    p.text(&label, 12.0, h / 2.0, &style);
    let mut x = 12.0 + text_w + 12.0;
    for step in Rung::LADDER {
        let (y, radius) = ((h - PIP) / 2.0, 2.0);
        if step == rung {
            p.fill(x, y, PIP, PIP, radius, INK);
        } else {
            p.border(x, y, PIP, PIP, radius, 1.5, (INK & 0xffffff00) | 0x80);
        }
        x += PIP + PIP_GAP;
    }
    Some((p.pixmap, size))
}

/// How far a window on workspace `index` is shifted from where it sits in the layout of screens,
/// on a screen at `here` whose view is at `camera` with workspaces `step` apart. `origin` is the
/// left edge of the screen the workspace is laid out for: its windows sit that far along, and are
/// drawn from this screen's edge instead, a workspace's distance from the view to the side.
pub(super) fn shift_for_workspace(
    index: usize,
    camera: f64,
    step: f64,
    here: i32,
    origin: i32,
) -> f64 {
    (index as f64 - camera) * step + (here - origin) as f64
}

/// The prompt on an empty workspace: where to find every key, and nothing else, so the wallpaper
/// stays clear at the one moment it is all there is to look at. The shortcut sheet lists every
/// key, so the empty workspace only has to name the sheet's key.
const TIPS_TEXT: &str = "Keyboard tips";

/// Under the pill: how to get the tour, which nothing else says. Quieter than the pill, because
/// it is the second thing to read, not a second pill. Its words either side of its keycap.
const TIPS_UNDER: (&str, &str) = ("then", "for the tour");

const TIPS_UNDER_KEY: &str = "⏎";

const TIPS_UNDER_GAP: f32 = 10.0;

const TIPS_UNDER_H: f32 = paint::KEYCAP_H;

const TIPS_UNDER_SPACE: f32 = 6.0;

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

const TIPS_GLOW: u32 = panel::MINT;

/// How far a floating window's shadow reaches beyond it, in logical pixels.
const SHADOW_MARGIN: i32 = 36;
/// Half the square the shadow is painted round, past the shadow's rounded corners, so the middle
/// of each side is straight.
const SHADOW_HALF: i32 = 12;
/// The shadow floating windows and the docked pane cast: its corners, and how far down it falls
/// and how soft it is, as CSS's `box-shadow: 0 10px 40px`.
const SHADOW_RADIUS: f32 = 7.0;
const SHADOW_DROP: f32 = 10.0;
const SHADOW_BLUR: f32 = 40.0;
const SHADOW_COLOUR: u32 = 0x00000040;

/// A floating window's shadow round a square `2 * SHADOW_HALF + 1` logical pixels across, with
/// `SHADOW_MARGIN` of room all round. The square itself is left clear, so a see-through window
/// isn't darkened.
fn paint_window_shadow(scale: f64) -> Option<paint::Painted> {
    let side = 2 * (SHADOW_MARGIN + SHADOW_HALF) + 1;
    paint::Painted::new(Size::from((side, side)), scale, |p| {
        let (m, core) = (SHADOW_MARGIN as f32, (2 * SHADOW_HALF + 1) as f32);
        p.shadow(
            m,
            m,
            core,
            core,
            SHADOW_RADIUS,
            SHADOW_DROP,
            SHADOW_BLUR,
            SHADOW_COLOUR,
        );
        p.clear(m, m, core, core);
    })
}

/// Room around the docked pane for its frame and the shadow, in logical pixels.
const DOCK_MARGIN: i32 = 64;
/// How round the frame's two lower corners are.
const DOCK_RADIUS: f32 = 7.0;

/// The frame of a docked pane `size` logical pixels big, with `DOCK_MARGIN` of room all round:
/// the bar's own material round the pane, edged with the bar's line down its sides and under
/// its foot, and the shadow the piece casts. Nothing is painted where the window goes, nor above
/// the frame's top, which is the bar's lower edge.
fn paint_dock_frame(size: (i32, i32), scale: f64) -> Option<paint::Painted> {
    let logical = Size::<i32, Logical>::from((size.0 + 2 * DOCK_MARGIN, size.1 + 2 * DOCK_MARGIN));
    paint::Painted::new(logical, scale, |p| {
        let m = DOCK_MARGIN as f32;
        let edge = crate::dock::FRAME as f32;
        let (w, h) = (size.0 as f32, size.1 as f32);
        // Drawn from well above the pane, so the corners rounded off are the lower two.
        let (x, y) = (m - edge, m - 2.0 * DOCK_RADIUS);
        let (wide, tall) = (w + 2.0 * edge, h + edge + 2.0 * DOCK_RADIUS);
        p.shadow(
            x,
            y,
            wide,
            tall,
            DOCK_RADIUS,
            SHADOW_DROP,
            SHADOW_BLUR,
            SHADOW_COLOUR,
        );
        p.fill(x, y, wide, tall, DOCK_RADIUS, panel::CHIP | 0xe6);
        p.border(x, y, wide, tall, DOCK_RADIUS, 1.0, panel::DIVIDER);
        p.clear(0.0, 0.0, logical.w as f32, m - edge);
        p.clear(m, m, w, h);
    })
}

impl Chrome {
    /// The shadow of a floating window drawn at `window`, `zoom` times its size (smaller in bullet
    /// time's overview): the painted shadow's corners as they are, and a sliver from the middle of
    /// each side stretched along the window's edges. Each piece starts on the screen pixel where
    /// the one before it ended, so they meet without a seam at any scale.
    pub(super) fn window_shadow<R>(
        &mut self,
        renderer: &mut R,
        window: Rectangle<f64, Logical>,
        (scale, zoom): (f64, f64),
        alpha: f32,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if self
            .window_shadow
            .as_ref()
            .is_none_or(|painted| painted.scale != scale)
        {
            self.window_shadow = paint_window_shadow(scale);
        }
        let Some(painted) = self.window_shadow.as_ref() else {
            return Vec::new();
        };
        let reach = SHADOW_MARGIN + SHADOW_HALF;
        // A corner's size on screen, and the shadow's outer edge.
        let corner = ((reach as f64 * zoom).round() as i32).max(1);
        let margin = SHADOW_MARGIN as f64 * zoom;
        let outer = Rectangle::<f64, Logical>::new(
            (window.loc.x - margin, window.loc.y - margin).into(),
            (window.size.w + 2.0 * margin, window.size.h + 2.0 * margin).into(),
        );
        // Buffer pixels to a logical one, and the middle sliver: one buffer pixel wide.
        let k = painted.device.0 as f64 / painted.logical.w as f64;
        let (cut, far) = ((reach as f64 * k).round(), ((reach + 1) as f64 * k).round());
        let end = painted.device.0 as f64;
        // Along one axis: each piece's source span in the buffer and its length on screen.
        let pieces = |length: f64| {
            let middle = (length.round() as i32 - 2 * corner).max(0);
            [
                (0.0, cut, corner),
                (cut, far - cut, middle),
                (far, end - far, corner),
            ]
        };
        let (across, down) = (pieces(outer.size.w), pieces(outer.size.h));
        let start = outer.loc.to_physical(scale).to_i32_round::<i32>();
        let mut elements = Vec::with_capacity(8);
        let mut y = start.y as f64;
        for (row, &(src_y, src_h, h)) in down.iter().enumerate() {
            let mut x = start.x as f64;
            for (column, &(src_x, src_w, w)) in across.iter().enumerate() {
                // The middle is the window's own: nothing is painted there.
                if (row, column) != (1, 1) && w > 0 && h > 0 {
                    let src = Rectangle::new((src_x, src_y).into(), (src_w, src_h).into());
                    elements.extend(
                        MemoryRenderBufferRenderElement::from_buffer(
                            renderer,
                            (x, y),
                            &painted.buffer,
                            Some(alpha),
                            Some(src),
                            Some(Size::from((w, h))),
                            Kind::Unspecified,
                        )
                        .ok(),
                    );
                }
                x = (x + w as f64 * scale).round();
            }
            y = (y + h as f64 * scale).round();
        }
        elements
    }

    /// The frame for a docked pane `size` big whose top left corner is at `at`, painted once for
    /// each size and kept.
    pub(super) fn dock_frame<R>(
        &mut self,
        renderer: &mut R,
        size: (i32, i32),
        scale: f64,
        at: Point<f64, Logical>,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let kept = self
            .dock_frames
            .iter()
            .position(|(painted, frame)| *painted == size && frame.scale == scale);
        let index = match kept {
            Some(index) => index,
            None => {
                // Two are ever wanted at once: where a pane is and where it is going.
                if self.dock_frames.len() >= 2 {
                    self.dock_frames.remove(0);
                }
                self.dock_frames
                    .push((size, paint_dock_frame(size, scale)?));
                self.dock_frames.len() - 1
            }
        };
        let margin = DOCK_MARGIN as f64;
        self.dock_frames[index].1.element(
            renderer,
            Point::from((at.x - margin, at.y - margin)),
            alpha,
        )
    }
}

/// Bullet time's key legend, a pill of its keys laid out in design pixels, painted at `scale`.
pub(super) fn paint_legend(scale: f64) -> Option<paint::Painted> {
    let style = Style::new(Face::Mono, 13.0, panel::HINT);
    let legend = bullet::legend();
    let (w, h) = (text::width(&legend, &style) + 32.0, 13.0 * 1.2 + 16.0);
    let logical =
        Size::<i32, Logical>::from(((w * DESIGN_PX).ceil() as i32, (h * DESIGN_PX).ceil() as i32));
    paint::Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        p.fill(0.0, 0.0, w, h, 10.0, panel::CHIP | 0xcc);
        p.text(&legend, 16.0, h / 2.0, &style);
    })
}

/// The keyboard-tips prompt, laid out in design pixels and painted at `scale`: a glass pill
/// saying what to press, inside a soft glow drawn as rings fading outwards. The glow is baked in
/// and the whole thing is faded in and out as it floats, so nothing repaints per frame.
pub(super) fn paint_tips(scale: f64, key: &str) -> Option<(Pixmap, Size<i32, Logical>)> {
    let words = Style::new(Face::Mono, 14.0, panel::PROSE);
    let pill_w = (TIPS_PAD
        + text::width(TIPS_TEXT, &words)
        + TIPS_GAP
        + paint::keycap_width(key)
        + TIPS_PAD)
        .ceil();
    // Room around the pill for the glow to fade out into.
    let halo = TIPS_RINGS as f32 * TIPS_RING_STEP;
    let under = Style::new(Face::Body, 13.0, panel::PLACEHOLDER);
    let under_w = text::width(TIPS_UNDER.0, &under)
        + 2.0 * TIPS_UNDER_SPACE
        + paint::keycap_width(TIPS_UNDER_KEY)
        + text::width(TIPS_UNDER.1, &under);
    // The line underneath may be wider than the pill; the pill then sits in the middle of it.
    let widest = pill_w.max(under_w);
    let pill_x = halo + (widest - pill_w) / 2.0;
    let size = Size::<i32, Logical>::from((
        ((widest + 2.0 * halo) * DESIGN_PX).ceil() as i32,
        ((TIPS_H + TIPS_UNDER_GAP + TIPS_UNDER_H + 2.0 * halo) * DESIGN_PX).ceil() as i32,
    ));
    let mut p = Painter::new(
        (size.w as f64 * scale).round() as u32,
        (size.h as f64 * scale).round() as u32,
        scale as f32 * DESIGN_PX,
    )?;
    // Rings outwards from the pill's edge, each fainter than the last.
    for ring in (1..=TIPS_RINGS).rev() {
        let grow = ring as f32 * TIPS_RING_STEP;
        let alpha = 0x30 / (ring as u32 + 1);
        let radius = (TIPS_H + 2.0 * grow) / 2.0;
        p.border(
            pill_x - grow,
            halo - grow,
            pill_w + 2.0 * grow,
            TIPS_H + 2.0 * grow,
            radius,
            TIPS_RING_STEP,
            (TIPS_GLOW & 0xffffff00) | alpha,
        );
    }
    p.fill(
        pill_x,
        halo,
        pill_w,
        TIPS_H,
        TIPS_H / 2.0,
        panel::CHIP | 0xe0,
    );
    p.border(
        pill_x,
        halo,
        pill_w,
        TIPS_H,
        TIPS_H / 2.0,
        1.0,
        (TIPS_GLOW & 0xffffff00) | 0x66,
    );
    let centre = halo + TIPS_H / 2.0;
    p.text(TIPS_TEXT, pill_x + TIPS_PAD, centre, &words);
    p.keycap(
        key,
        pill_x + TIPS_PAD + text::width(TIPS_TEXT, &words) + TIPS_GAP,
        centre,
    );
    let under_centre = halo + TIPS_H + TIPS_UNDER_GAP + TIPS_UNDER_H / 2.0;
    let mut x = halo + (widest - under_w) / 2.0;
    x += p.text(TIPS_UNDER.0, x, under_centre, &under) + TIPS_UNDER_SPACE;
    x += p.keycap(TIPS_UNDER_KEY, x, under_centre) + TIPS_UNDER_SPACE;
    p.text(TIPS_UNDER.1, x, under_centre, &under);
    Some((p.pixmap, size))
}

//! Windows as rigid panes of glass in 3D: turned about their own middle, pushed back or brought
//! forward, and seen in perspective. Alt+Tab's deck and the pass-through swap draw windows this
//! way.
//!
//! A pane is a window drawn flat into a texture of its own, then shown through a small shader
//! that maps every screen pixel back onto that flat picture. The glass is rigid: it turns and
//! moves in depth, it never bends. A window opening arrives as a pane from behind the screen.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{
        Renderer, Texture,
        element::{AsRenderElements, Id, Kind, texture::TextureRenderElement},
        gles::{
            GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType, UniformValue,
            element::TextureShaderElement,
        },
        utils::DamageBag,
    },
    desktop::Window,
    utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Size, Transform},
};

use crate::{
    render::OutputElement,
    tilt::{self, draw_offscreen},
};

/// How far the eye is from the screen, in screen widths.
pub const EYE: f64 = 1.1;
/// How far behind the screen an opening window of half the screen or more sets off, in screen
/// widths: 260 pixels of a 1280-wide screen.
const ARRIVE_DEPTH: f64 = 260.0 / 1280.0;
/// How far back it leans about its bottom edge as it sets off, in radians: 9°.
const ARRIVE_LEAN: f64 = 9.0 * std::f64::consts::PI / 180.0;
/// The share of the screen's area from which an opening window gets the whole depth and lean: a
/// tile of half the screen, less its gaps and the bar.
const ARRIVE_FULL_FROM: f64 = 0.45;
/// When, in its way forward, the band of light starts down an opening window; it reaches the foot
/// as the window lands.
const ARRIVE_BAND_FROM: f64 = 0.45;
/// The band's half height, as a share of the window's, and its strength.
const ARRIVE_BAND_HALF: f64 = 0.2;
const ARRIVE_BAND_STRENGTH: f32 = 0.16;

/// Where a pane is and how it's turned. `x` and `y` are its middle on the screen and `z` its
/// depth, all in logical pixels; positive `z` is further away. `yaw` turns it about its upright
/// middle line (positive sends the right-hand edge away) and `pitch` about its level one
/// (positive sends the bottom edge away), in radians. `w` and `h` are its size at depth 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub w: f64,
    pub h: f64,
}

impl Pose {
    /// Lying flat on the screen over `rect` (x, y, w, h), exactly where a window at rest is drawn.
    pub fn flat(rect: [f64; 4]) -> Self {
        let [x, y, w, h] = rect;
        Self {
            x: x + w / 2.0,
            y: y + h / 2.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            w,
            h,
        }
    }

    /// A window opening over `rect` on a `screen` (width, height), `arrival` of its way forward:
    /// 0 is behind the screen and leaning back about its bottom edge, so its top is furthest
    /// away, and 1 is flat over `rect`. A little over 1 brings it a hair nearer than the screen
    /// as it overshoots. How far back and how far over it starts depends on the tile's size
    /// (`arrival_reach`).
    pub fn arriving(rect: [f64; 4], arrival: f64, screen: (f64, f64)) -> Self {
        let reach = arrival_reach(rect, screen);
        let left = 1.0 - arrival;
        let lean = ARRIVE_LEAN * reach * left;
        let flat = Self::flat(rect);
        let half = flat.h / 2.0;
        // Leaning back is a negative pitch. The middle rises and goes back with it, so the
        // bottom edge stays at the pane's depth, under where the window's foot will be.
        Self {
            y: rect[1] + rect[3] - half * lean.cos(),
            z: ARRIVE_DEPTH * screen.0 * reach * left + half * lean.sin(),
            pitch: -lean,
            ..flat
        }
    }

    /// Part way from `self` to `to`.
    pub fn mix(&self, to: &Pose, t: f64) -> Pose {
        // Exactly at either end, so a pane that has landed lies exactly where its window is.
        if t >= 1.0 {
            return *to;
        }
        if t <= 0.0 {
            return *self;
        }
        let l = |a: f64, b: f64| a + (b - a) * t;
        Pose {
            x: l(self.x, to.x),
            y: l(self.y, to.y),
            z: l(self.z, to.z),
            yaw: l(self.yaw, to.yaw),
            pitch: l(self.pitch, to.pitch),
            w: l(self.w, to.w),
            h: l(self.h, to.h),
        }
    }
}

/// How much of the whole depth and lean a window opening over `rect` on a `screen` (width,
/// height) sets off with: all of it for a tile of about half the screen or more, and less for a
/// smaller one, in step with the tile's size, so a small tile comes forward as a half-screen one
/// does scaled down to it, rather than being thrown back by a depth sized for the whole screen.
fn arrival_reach(rect: [f64; 4], (width, height): (f64, f64)) -> f64 {
    let share = (rect[2] * rect[3]).max(0.0) / (width * height).max(1.0);
    (share / ARRIVE_FULL_FROM).sqrt().min(1.0)
}

/// How big a window opening over `rect` on a `screen` looks about its centre, `arrival` of its
/// way forward (`Pose::arriving`), for drawing it flat where it can't be drawn as a pane: exactly
/// 1 once it has landed.
pub fn arriving_scale(rect: [f64; 4], arrival: f64, screen: (f64, f64)) -> f64 {
    let depth = ARRIVE_DEPTH * arrival_reach(rect, screen) * (1.0 - arrival);
    EYE / (EYE + depth)
}

/// The eye looking at the panes: straight at (`x`, `y`) on the screen, `distance` in front of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub distance: f64,
}

impl Camera {
    /// Looking at the middle of a screen `w` × `h`, from a little more than its width away: the
    /// same strength of perspective bullet time uses.
    pub fn for_screen(w: f64, h: f64) -> Self {
        Self {
            x: w / 2.0,
            y: h / 2.0,
            distance: EYE * w,
        }
    }

    /// Looking straight at the middle of a window opening over `rect`, from as far away as
    /// `for_screen` on a screen `width` wide: every tile comes forward towards its own middle,
    /// as a full-screen window does, rather than sliding in from the screen's middle and
    /// skewing on the way.
    pub fn for_arrival(rect: [f64; 4], width: f64) -> Self {
        Self {
            x: rect[0] + rect[2] / 2.0,
            y: rect[1] + rect[3] / 2.0,
            distance: EYE * width,
        }
    }
}

/// The homography, row by row, taking a point in the pane's own pixels (0..w, 0..h) to the
/// screen, in homogeneous coordinates.
pub fn matrix(pose: &Pose, camera: &Camera) -> [f64; 9] {
    let (sy, cy) = pose.yaw.sin_cos();
    let (sp, cp) = pose.pitch.sin_cos();
    let f = camera.distance;
    let (ex, ey) = (camera.x, camera.y);
    let depth = f + pose.z;
    // A point (a, b) from the pane's middle, turned by yaw then pitch, lands at
    // (a·cy, b·cp − a·sy·sp, b·sp + a·sy·cp) about the pane's middle; the eye divides by its depth.
    let d = [sy * cp, sp, depth];
    let sx = [
        ex * sy * cp + f * cy,
        ex * sp,
        ex * depth + f * (pose.x - ex),
    ];
    let syr = [
        ey * sy * cp - f * sy * sp,
        ey * sp + f * cp,
        ey * depth + f * (pose.y - ey),
    ];
    // Then from the pane's own pixels to its middle: a = lx − w/2, b = ly − h/2.
    let (hw, hh) = (pose.w / 2.0, pose.h / 2.0);
    let shift = |row: [f64; 3]| [row[0], row[1], row[2] - row[0] * hw - row[1] * hh];
    let [a, b, c] = shift(sx);
    let [d0, e, g] = shift(syr);
    let [h, i, j] = shift(d);
    [a, b, c, d0, e, g, h, i, j]
}

/// Where the pane's point (`u`, `v`), each 0 to 1 across it, shows on the screen. `None` behind
/// the eye.
pub fn project(pose: &Pose, camera: &Camera, (u, v): (f64, f64)) -> Option<(f64, f64)> {
    tilt::apply(&matrix(pose, camera), u * pose.w, v * pose.h)
}

/// The smallest rectangle on the screen holding the pane and `margin` around it.
pub fn bounds(pose: &Pose, camera: &Camera, margin: f64) -> Option<Rectangle<f64, Logical>> {
    let (mu, mv) = (margin / pose.w.max(1.0), margin / pose.h.max(1.0));
    let corners = [
        (-mu, -mv),
        (1.0 + mu, -mv),
        (1.0 + mu, 1.0 + mv),
        (-mu, 1.0 + mv),
    ];
    let mut points = Vec::with_capacity(4);
    for corner in corners {
        points.push(project(pose, camera, corner)?);
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Some(Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into()))
}

/// How a pane is lit, beyond its picture.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Look {
    pub alpha: f32,
    /// The focus ring just outside the pane: colour and strength.
    pub ring: Option<([f32; 3], f32)>,
    /// How much darker it is, 0 to 1: panes further back in a deck sit in shadow.
    pub shade: f32,
    /// A band of light across the pane at a line on the screen: whether the line runs down the
    /// screen (at an x) or across it (at a y), where, half its width and its strength. Where one
    /// pane passes in front of another, the one behind is lit along the other's edge.
    pub band: Option<(Axis, f64, f64, f32)>,
    /// One of the pane's own edges lit, and how strongly: the edge leading a pane through another.
    pub edge: Option<(Axis, bool, f32)>,
    /// How far the lit edge's glow reaches into the pane, in the pane's own pixels, or 0 for the
    /// usual few. A pane drawn far away needs more of its own pixels to show any.
    pub reach: f32,
    /// How much of the glass's own light off its top left corner it has lost, 0 to 1: a pane
    /// settling into a window's place loses it as it lands, so the window takes over unchanged.
    pub matte: f32,
}

impl Look {
    /// An opening window `arrival` of its way forward (`Pose::arriving`), seen on the screen at
    /// `pose`: a band of light crosses it from top to bottom over the last of the way, and the
    /// glass's own light goes with it.
    pub fn arriving(self, pose: &Pose, camera: &Camera, arrival: f64) -> Self {
        let crossed = ((arrival - ARRIVE_BAND_FROM) / (1.0 - ARRIVE_BAND_FROM)).clamp(0.0, 1.0);
        // From just above the top to just below the foot, so it comes in and goes out of sight.
        let along = -0.06 + 1.12 * crossed;
        let band = (crossed > 0.0 && crossed < 1.0)
            .then(|| project(pose, camera, (0.5, along)))
            .flatten()
            .map(|(_, y)| {
                (
                    Axis::Down,
                    y,
                    ARRIVE_BAND_HALF * pose.h,
                    ARRIVE_BAND_STRENGTH,
                )
            });
        Self {
            band,
            matte: crossed as f32,
            ..self
        }
    }
}

/// Along which screen direction something runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Left and right: an edge or band that is a vertical line.
    Across,
    /// Up and down: one that is a horizontal line.
    Down,
}

impl Axis {
    fn flag(self) -> f32 {
        match self {
            Axis::Across => 0.0,
            Axis::Down => 1.0,
        }
    }
}

/// A window's picture for its pane, and whether it was drawn this frame. The pane keeps one
/// element id for as long as it is drawn, so the damage tracker sees the same element move rather
/// than a new one turning up each frame, and its damage records each time the picture is drawn
/// again.
struct Kept {
    window: Window,
    texture: Option<crate::tilt::Canvas>,
    id: Id,
    damage: DamageBag<i32, Buffer>,
    used: bool,
}

/// How long the pane shader is kept after the last pane was drawn. Compiled, it holds a good deal
/// of memory in the graphics driver; compiled again, it comes from the driver's shader cache.
const KEEP_SHADER: Duration = Duration::from_secs(10);

/// Draws windows as panes, keeping a texture for each window drawn this way lately.
#[derive(Default)]
pub struct Panes {
    program: Option<GlesTexProgram>,
    broken: bool,
    textures: Vec<Kept>,
    /// Whether the shader has been compiled once, which fills the driver's shader cache.
    warmed: bool,
    /// Since when no pane has been drawn.
    idle_since: Option<Instant>,
}

impl Panes {
    /// Compiles the shader the first time it's called, where a pause goes unseen, so the driver's
    /// shader cache has it before the first pane is drawn. It's let go again once idle.
    pub fn warm(&mut self, renderer: &mut GlesRenderer) {
        if !self.warmed {
            self.warmed = true;
            self.ready(renderer);
        }
    }

    /// Whether panes can be drawn, compiling the shader the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.program.is_some() {
            return !self.broken;
        }
        let uniforms = [
            UniformName::new("box", UniformType::_4f),
            UniformName::new("unproject", UniformType::Matrix3x3),
            UniformName::new("pane", UniformType::_2f),
            UniformName::new("pixel", UniformType::_1f),
            UniformName::new("ring", UniformType::_4f),
            UniformName::new("shade", UniformType::_1f),
            UniformName::new("band", UniformType::_4f),
            UniformName::new("edge", UniformType::_3f),
            UniformName::new("reach", UniformType::_1f),
            UniformName::new("matte", UniformType::_1f),
        ];
        match renderer.compile_custom_texture_shader(SHADER, &uniforms) {
            Ok(program) => self.program = Some(program),
            Err(err) => {
                tracing::warn!("the glass pane shader didn't compile, so windows stay flat: {err}");
                self.broken = true;
            }
        }
        !self.broken
    }

    /// Call once a frame before drawing panes; `finish` then lets go of textures not used since.
    pub fn begin(&mut self) {
        for kept in &mut self.textures {
            kept.used = false;
        }
    }

    pub fn finish(&mut self) {
        self.textures
            .retain(|kept| kept.used && kept.window.alive());
        if !self.textures.is_empty() {
            self.idle_since = None;
        } else if self
            .idle_since
            .get_or_insert_with(Instant::now)
            .elapsed()
            >= KEEP_SHADER
        {
            self.program = None;
        }
    }

    /// `window` as a pane at `pose`, seen by `camera` on `screen`.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        pose: &Pose,
        camera: &Camera,
        screen: crate::tilt::Sizes,
        look: &Look,
    ) -> Option<TextureShaderElement> {
        let (scale, screen) = (screen.scale, screen.logical);
        if !self.ready(renderer) || look.alpha <= 0.0 {
            return None;
        }
        let own = window.geometry();
        if own.size.w <= 0 || own.size.h <= 0 {
            return None;
        }
        // Room for the ring and the edge's glow.
        let shown = bounds(pose, camera, 8.0)?;
        let visible = Rectangle::<f64, Logical>::from_size(screen.to_f64());
        let shown = shown.intersection(visible)?;
        let physical = Size::<i32, Physical>::from((
            (own.size.w as f64 * scale).round().max(1.0) as i32,
            (own.size.h as f64 * scale).round().max(1.0) as i32,
        ));
        let location = (Point::<i32, Logical>::default() - own.loc)
            .to_f64()
            .to_physical(scale)
            .to_i32_round();
        let surfaces: Vec<OutputElement> = AsRenderElements::<GlesRenderer>::render_elements(
            window,
            renderer,
            location,
            scale.into(),
            1.0,
        )
        .into_iter()
        .map(OutputElement::Surface)
        .collect();
        if surfaces.is_empty() {
            return None;
        }
        let at = match self.textures.iter().position(|kept| kept.window == *window) {
            Some(at) => at,
            None => {
                self.textures.push(Kept {
                    window: window.clone(),
                    texture: None,
                    id: Id::new(),
                    damage: DamageBag::new(4),
                    used: true,
                });
                self.textures.len() - 1
            }
        };
        let kept = &mut self.textures[at];
        kept.used = true;
        let texture = &mut kept.texture;
        draw_offscreen(
            renderer,
            texture,
            &mut self.broken,
            &surfaces,
            crate::tilt::Sizes {
                logical: own.size,
                physical,
                scale,
            },
            "a glass pane",
        )?;
        let texture = &texture.as_ref()?.texture;
        // Redrawn every frame it is shown, and seen through a shader whose look moves on with it.
        kept.damage.add([Rectangle::from_size(texture.size())]);
        let (id, damage) = (kept.id.clone(), kept.damage.snapshot());
        // The picture is laid out in the window's own logical pixels, which the pose may show
        // bigger or smaller: from those to the pose's, then on to the screen.
        let stretch = [
            pose.w / own.size.w as f64,
            0.0,
            0.0,
            0.0,
            pose.h / own.size.h as f64,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        let m = multiply(&matrix(pose, camera), &stretch);
        let m = tilt::invert(&m)?.map(|v| v as f32);
        let columns = [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]];
        // On whole screen pixels, so the element's own edges don't blur the picture.
        let corner = shown.loc.to_physical(scale).to_i32_floor::<i32>();
        let far = (shown.loc + shown.size.to_point())
            .to_physical(scale)
            .to_i32_ceil::<i32>();
        let size_px = Size::<i32, Physical>::from((far.x - corner.x, far.y - corner.y));
        if size_px.w <= 0 || size_px.h <= 0 {
            return None;
        }
        let place =
            Rectangle::<f64, Physical>::new(corner.to_f64(), size_px.to_f64()).to_logical(scale);
        let size = Size::<i32, Logical>::from((
            place.size.w.round().max(1.0) as i32,
            place.size.h.round().max(1.0) as i32,
        ));
        let inner = TextureRenderElement::from_texture_with_damage(
            id,
            renderer.context_id(),
            corner.to_f64(),
            texture.clone(),
            1,
            Transform::Normal,
            Some(look.alpha),
            Some(Rectangle::from_size(
                (physical.w as f64, physical.h as f64).into(),
            )),
            Some(size),
            None,
            damage,
            Kind::Unspecified,
        );
        let ring = look.ring.map_or([0.0; 4], |([r, g, b], a)| [r, g, b, a]);
        let band = look.band.map_or([0.0; 4], |(axis, at, half, strength)| {
            [axis.flag(), at as f32, half.max(1.0) as f32, strength]
        });
        let edge = look.edge.map_or([0.0; 3], |(axis, far, strength)| {
            [axis.flag(), if far { 1.0 } else { 0.0 }, strength]
        });
        let uniforms = vec![
            Uniform::new(
                "box",
                (
                    place.loc.x as f32,
                    place.loc.y as f32,
                    place.size.w as f32,
                    place.size.h as f32,
                ),
            ),
            Uniform::new(
                "unproject",
                UniformValue::Matrix3x3 {
                    matrices: vec![columns],
                    transpose: false,
                },
            ),
            Uniform::new("pane", (own.size.w as f32, own.size.h as f32)),
            Uniform::new("pixel", (1.0 / scale) as f32),
            Uniform::new("ring", (ring[0], ring[1], ring[2], ring[3])),
            Uniform::new("shade", look.shade),
            Uniform::new("band", (band[0], band[1], band[2], band[3])),
            Uniform::new("edge", (edge[0], edge[1], edge[2])),
            Uniform::new("reach", if look.reach > 0.0 { look.reach } else { 5.0 }),
            Uniform::new("matte", look.matte),
        ];
        Some(TextureShaderElement::new(
            inner,
            self.program.clone()?,
            uniforms,
        ))
    }
}

/// How long two tiles take to pass through each other, in animation seconds.
pub const PASS: f64 = 0.5 / 3.0;

/// Two tiles trading places by passing through each other: the one being moved comes forward and
/// the one it swaps with falls back, each turning a little on the way, so they cross as two
/// panes of glass rather than sliding over one another.
#[derive(Debug, Clone)]
pub struct Pass<W> {
    /// The window being moved, which passes in front.
    pub front: W,
    pub back: W,
    /// Each one's rect (x, y, w, h, in the space's pixels) before and after.
    from: [[f64; 4]; 2],
    to: [[f64; 4]; 2],
    start: f64,
    axis: Axis,
    /// The front one heads right, or down.
    forward: bool,
}

impl<W> Pass<W> {
    pub fn new(front: W, back: W, from: [[f64; 4]; 2], to: [[f64; 4]; 2], start: f64) -> Self {
        let dx = to[0][0] - from[0][0];
        let dy = to[0][1] - from[0][1];
        let axis = if dx.abs() >= dy.abs() {
            Axis::Across
        } else {
            Axis::Down
        };
        let forward = if axis == Axis::Across {
            dx > 0.0
        } else {
            dy > 0.0
        };
        Self {
            front,
            back,
            from,
            to,
            start,
            axis,
            forward,
        }
    }

    pub fn done(&self, now: f64) -> bool {
        now - self.start >= PASS
    }

    /// Both panes at `now`, front then back, on a screen `width` wide whose corner is at
    /// `origin` in the space, and how deep into the pass they are (0 at either end, 1 halfway).
    pub fn poses(&self, now: f64, width: f64, origin: (f64, f64)) -> (Pose, Pose, f64) {
        let t = ((now - self.start) / PASS).clamp(0.0, 1.0);
        let p = crate::anim::Easing::InOutCubic.at(t);
        let deep = (std::f64::consts::PI * p).sin();
        let sign = if self.forward { 1.0 } else { -1.0 };
        let at = |i: usize| {
            let r: [f64; 4] =
                std::array::from_fn(|k| self.from[i][k] + (self.to[i][k] - self.from[i][k]) * p);
            Pose::flat([r[0] - origin.0, r[1] - origin.1, r[2], r[3]])
        };
        let turn = 0.1 * deep * sign;
        let (mut front, mut back) = (at(0), at(1));
        front.z = -0.05 * width * deep;
        back.z = 0.08 * width * deep;
        match self.axis {
            Axis::Across => {
                front.yaw = turn;
                back.yaw = -turn;
            }
            Axis::Down => {
                front.pitch = turn;
                back.pitch = -turn;
            }
        }
        (front, back, deep)
    }

    /// Which way the pass runs, and whether the front pane's leading edge is its far one (right or
    /// bottom).
    pub fn leading(&self) -> (Axis, bool) {
        (self.axis, self.forward)
    }
}

fn multiply(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    std::array::from_fn(|k| {
        let (row, col) = (k / 3, k % 3);
        (0..3).map(|i| a[row * 3 + i] * b[i * 3 + col]).sum()
    })
}

/// Smithay's texture shader with the sampling replaced: each screen pixel is mapped back onto the
/// pane's flat picture, four times a quarter-pixel apart so slanted edges come out smooth. Just
/// outside the picture is the focus ring; over it, light from the top left, and any band or edge
/// the pane has been given.
const SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec4 box;
uniform mat3 unproject;
uniform vec2 pane;
uniform float pixel;
uniform vec4 ring;
uniform float shade;
uniform vec4 band;
uniform vec3 edge;
uniform float reach;
uniform float matte;

const float RING_GAP = 0.5;
const float RING_WIDTH = 2.5;

vec4 seen_at(vec2 screen) {
    vec3 p = unproject * vec3(screen, 1.0);
    if (p.z <= 0.0) {
        return vec4(0.0);
    }
    vec2 own = p.xy / p.z;
    vec2 beyond = max(-own, own - pane);
    float outside = max(beyond.x, beyond.y);
    if (outside > 0.0) {
        if (ring.a > 0.0 && outside > RING_GAP && outside < RING_GAP + RING_WIDTH) {
            return vec4(ring.rgb, 1.0) * ring.a;
        }
        return vec4(0.0);
    }
    vec4 colour = texture2D(tex, own / pane);
    colour.rgb *= 1.0 - shade;
    // Light off the glass, strongest at the top left corner.
    float d = 0.5 * (own.x / pane.x + own.y / pane.y);
    colour.rgb += vec3(0.07) * (1.0 - matte) * (1.0 - smoothstep(0.0, 0.45, d)) * colour.a;
    if (band.w > 0.0) {
        float along = band.x < 0.5 ? screen.x : screen.y;
        float k = 1.0 - clamp(abs(along - band.y) / band.z, 0.0, 1.0);
        colour.rgb += vec3(0.67, 0.94, 1.0) * band.w * k * k * colour.a;
    }
    if (edge.z > 0.0) {
        float u = edge.x < 0.5 ? own.x : own.y;
        float span = edge.x < 0.5 ? pane.x : pane.y;
        float from = edge.y > 0.5 ? span - u : u;
        float k = exp(-from / reach);
        colour += vec4(0.84, 0.98, 1.0, 1.0) * edge.z * k * (1.0 - colour.a * 0.3);
    }
    return colour;
}

void main() {
    vec2 screen = box.xy + v_coords * box.zw;
    vec4 color = 0.25 * (
        seen_at(screen + pixel * vec2(0.125, 0.375)) +
        seen_at(screen + pixel * vec2(-0.375, 0.125)) +
        seen_at(screen + pixel * vec2(-0.125, -0.375)) +
        seen_at(screen + pixel * vec2(0.375, -0.125)));

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f64, f64) = (1536.0, 960.0);
    const CAMERA: Camera = Camera {
        x: 768.0,
        y: 480.0,
        distance: 1700.0,
    };

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn an_opening_window_starts_behind_the_screen_leaning_back_and_lands_flat() {
        let rect = [200.0, 100.0, 600.0, 400.0];
        let start = Pose::arriving(rect, 0.0, SCREEN);
        let top = project(&start, &CAMERA, (0.0, 0.0)).unwrap();
        let foot = project(&start, &CAMERA, (0.0, 1.0)).unwrap();
        let foot_right = project(&start, &CAMERA, (1.0, 1.0)).unwrap();
        let top_right = project(&start, &CAMERA, (1.0, 0.0)).unwrap();
        assert!(
            foot_right.0 - foot.0 < 600.0,
            "pushed back, it looks smaller"
        );
        assert!(
            top_right.0 - top.0 < foot_right.0 - foot.0,
            "the top is further away than the foot"
        );
        assert_eq!(Pose::arriving(rect, 1.0, SCREEN), Pose::flat(rect));
    }

    /// How a window opening over `rect` sets off: how big its foot looks against its own width,
    /// and how far it leans back.
    fn setting_off(rect: [f64; 4]) -> (f64, f64) {
        let start = Pose::arriving(rect, 0.0, SCREEN);
        let foot = project(&start, &CAMERA, (0.0, 1.0)).unwrap();
        let foot_right = project(&start, &CAMERA, (1.0, 1.0)).unwrap();
        ((foot_right.0 - foot.0) / rect[2], -start.pitch)
    }

    #[test]
    fn a_smaller_tile_opens_from_nearer_and_leans_less() {
        let whole = setting_off([0.0, 0.0, 1536.0, 960.0]);
        let half = setting_off([0.0, 0.0, 768.0, 960.0]);
        let quarter = setting_off([768.0, 480.0, 768.0, 480.0]);
        let eighth = setting_off([1152.0, 480.0, 384.0, 480.0]);
        // From half the screen up, every tile sets off the same way.
        assert!((whole.0 - half.0).abs() < 1e-9 && whole.1 == half.1);
        // Below it, the smaller the tile the less far back it starts and the less it leans.
        assert!(half.0 < quarter.0 && quarter.0 < eighth.0 && eighth.0 < 1.0);
        assert!(half.1 > quarter.1 && quarter.1 > eighth.1 && eighth.1 > 0.0);
    }

    #[test]
    fn a_tile_off_to_one_side_comes_forward_square_on_to_its_own_middle() {
        let corner = [1152.0, 720.0, 384.0, 240.0];
        let start = Pose::arriving(corner, 0.0, SCREEN);
        let camera = Camera::for_arrival(corner, SCREEN.0);
        let middle = corner[0] + corner[2] / 2.0;
        for v in [0.0, 1.0] {
            let (left, _) = project(&start, &camera, (0.0, v)).unwrap();
            let (right, _) = project(&start, &camera, (1.0, v)).unwrap();
            assert!(((middle - left) - (right - middle)).abs() < 1e-6);
        }
    }

    #[test]
    fn drawn_flat_a_smaller_tile_opens_closer_to_its_own_size_and_lands_exactly() {
        let half = [0.0, 0.0, 768.0, 960.0];
        let quarter = [768.0, 480.0, 768.0, 480.0];
        assert!(arriving_scale(half, 0.0, SCREEN) < arriving_scale(quarter, 0.0, SCREEN));
        assert!(arriving_scale(quarter, 0.0, SCREEN) < 1.0);
        assert_eq!(arriving_scale(half, 1.0, SCREEN), 1.0);
        assert_eq!(arriving_scale(quarter, 1.0, SCREEN), 1.0);
    }

    #[test]
    fn a_flat_pane_lies_exactly_over_its_rect() {
        let pose = Pose::flat([100.0, 60.0, 700.0, 400.0]);
        assert!(close(
            project(&pose, &CAMERA, (0.0, 0.0)).unwrap(),
            (100.0, 60.0)
        ));
        assert!(close(
            project(&pose, &CAMERA, (1.0, 1.0)).unwrap(),
            (800.0, 460.0)
        ));
    }

    #[test]
    fn a_pane_pushed_back_shrinks_towards_the_eye_line() {
        let pose = Pose {
            z: 1700.0,
            ..Pose::flat([168.0, 80.0, 1200.0, 800.0])
        };
        // Twice as far away: half the size, about the point looked at.
        let a = project(&pose, &CAMERA, (0.0, 0.0)).unwrap();
        let b = project(&pose, &CAMERA, (1.0, 1.0)).unwrap();
        assert!(((b.0 - a.0) - 600.0).abs() < 1e-6);
        assert!(((b.1 - a.1) - 400.0).abs() < 1e-6);
    }

    #[test]
    fn turned_away_the_far_edge_is_shorter() {
        let pose = Pose {
            yaw: 0.5,
            ..Pose::flat([168.0, 80.0, 1200.0, 800.0])
        };
        let left = project(&pose, &CAMERA, (0.0, 1.0)).unwrap().1
            - project(&pose, &CAMERA, (0.0, 0.0)).unwrap().1;
        let right = project(&pose, &CAMERA, (1.0, 1.0)).unwrap().1
            - project(&pose, &CAMERA, (1.0, 0.0)).unwrap().1;
        assert!(right < left, "the right-hand edge has gone away");
    }

    #[test]
    fn passing_panes_start_and_end_flat_in_each_others_places() {
        let left = [16.0, 50.0, 760.0, 900.0];
        let right = [786.0, 50.0, 760.0, 900.0];
        let pass = Pass::new("a", "b", [left, right], [right, left], 1.0);
        let (front, back, deep) = pass.poses(1.0, 1536.0, (0.0, 0.0));
        assert_eq!(
            (front, back, deep),
            (Pose::flat(left), Pose::flat(right), 0.0)
        );
        let (front, back, deep) = pass.poses(1.0 + PASS / 2.0, 1536.0, (0.0, 0.0));
        assert!(
            deep > 0.99 && front.z < 0.0 && back.z > 0.0,
            "one forward, one back"
        );
        let (front, back, _) = pass.poses(1.0 + PASS, 1536.0, (0.0, 0.0));
        assert!(
            (front.x - Pose::flat(right).x).abs() < 1e-9
                && (back.x - Pose::flat(left).x).abs() < 1e-9
        );
        assert!(pass.done(1.0 + PASS + 1e-9));
        assert_eq!(pass.leading(), (Axis::Across, true));
    }

    #[test]
    fn the_shader_mapping_undoes_the_projection() {
        let pose = Pose {
            yaw: 0.4,
            pitch: -0.2,
            z: 300.0,
            ..Pose::flat([300.0, 100.0, 640.0, 480.0])
        };
        let m = matrix(&pose, &CAMERA);
        let back = tilt::invert(&m).unwrap();
        for (u, v) in [(0.1, 0.2), (0.9, 0.5), (0.5, 0.95)] {
            let seen = project(&pose, &CAMERA, (u, v)).unwrap();
            let own = tilt::apply(&back, seen.0, seen.1).unwrap();
            assert!(close(own, (u * 640.0, v * 480.0)));
        }
    }
}

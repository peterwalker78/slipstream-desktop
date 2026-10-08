//! The traces the faded desktop shows for AI agents at work: one small oscilloscope readout for
//! each, stacked down the right edge of the wallpaper.
//!
//! A trace is a wave in cyan while its agent works, and settles into a flat amber line when the
//! agent stops. The difference is one of kind, moving against still, so it reads from across a
//! room without comparing anything. Each agent's wave has a shape of its own, under what the
//! agent says to call it.
//!
//! They are only on screen once the desktop has faded to the wallpaper, and only for agents that
//! have worked since the desk was last touched: one that was already idle has no trace, and a
//! flat line is cleared by the key that brings the desktop back, having said what it had to.
//! Which agents are at work comes from the notes they leave (`working.rs`); with none, nothing
//! here draws.
//!
//! Reduced motion keeps the same picture without the movement: a standing wave for an agent at
//! work, a flat line for one that has stopped.

use std::collections::HashMap;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Buffer, Logical, Point, Rectangle, Size, Transform},
};

use crate::{
    paint::Painter,
    text::{self, Face, Style},
    working::Agent,
};

// Sizes in logical pixels.
/// A readout's width as a share of the screen's, and the limits it is held to.
const WIDTH_OF_SCREEN: f32 = 0.19;
const NARROWEST: i32 = 120;
const WIDEST: i32 = 300;
/// The gap to the screen's top and right edges.
const MARGIN: i32 = 18;
/// The space between a readout's dark backing and what is drawn on it.
const PAD: i32 = 9;
/// The line the agent's name is written on.
const LABEL: i32 = 15;
/// A trace's height, and the least it is squeezed to before readouts are left off instead.
const TRACE: i32 = 44;
const SHORTEST: i32 = 15;
const GAP: i32 = 8;
/// A trace is drawn as dots this big, this far apart.
const DOT: f64 = 2.0;
const PITCH: f64 = 3.0;

/// The backing behind a readout: the desktop's backdrop colour, letting some wallpaper through.
const BACKING: u32 = 0x0b0e13b8;
const BACKING_RADIUS: f32 = 7.0;
const NAME: u32 = 0xaab2c0ff;
/// At work, and stopped.
const CYAN: [f32; 3] = [51.0, 204.0, 255.0];
const AMBER: [f32; 3] = [255.0, 181.0, 71.0];

/// How fast a wave settles flat or rises again, as a rate a second.
const SETTLE: f32 = 3.0;
/// The readouts come in once the desktop has gone, and leave ahead of it coming back.
const APPEAR: f64 = 0.6;
const LEAVE: f64 = 0.15;
/// Traces are redrawn 30 times a second.
const STEP: f64 = 1.0 / 30.0;
/// How many times a second the bright spot crosses a trace.
const SWEEP: f64 = 0.6;
/// How bright a flat line is, and a wave where the spot passed longest ago.
const FLAT: f32 = 0.85;
const DIMMEST: f32 = 0.25;

/// One agent's readout.
#[derive(Debug, Clone, PartialEq)]
struct Trace {
    pid: u32,
    name: String,
    working: bool,
    /// Stopped, and the desk has been touched since: it goes once it is out of sight.
    seen: bool,
    /// 1 as a full wave, 0 flat.
    level: f32,
}

impl Trace {
    /// How many waves cross the readout in each of its two parts, and where they start: steady
    /// for as long as the agent runs, and different from one agent to the next.
    fn shape(&self) -> (f32, f32, f32) {
        let hash = self.pid.wrapping_mul(2_654_435_761);
        (
            2.0 + (hash % 3) as f32,
            5.0 + ((hash >> 8) % 4) as f32,
            ((hash >> 16) & 0xff) as f32 / 255.0 * std::f32::consts::TAU,
        )
    }

    /// The wave's height, -1 to 1, `across` the readout from 0 to 1 at time `t`.
    fn wave(&self, across: f32, t: f32) -> f32 {
        let (slow, quick, phase) = self.shape();
        let turn = std::f32::consts::TAU * across;
        0.6 * (turn * slow + t * 1.4 + phase).sin()
            + 0.4 * (turn * quick - t * 1.9 + phase * 2.0).sin()
    }
}

/// How the readouts are laid out on a screen, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fit {
    width: i32,
    /// The height of each trace.
    trace: i32,
    /// How many readouts there is room for.
    count: usize,
}

impl Fit {
    /// Room for `agents` readouts down a `screen`: traces shrink before any is left off.
    fn new(screen: Size<i32, Logical>, agents: usize) -> Option<Self> {
        let width = ((screen.w as f32 * WIDTH_OF_SCREEN) as i32).clamp(NARROWEST, WIDEST);
        let room = screen.h - 2 * MARGIN + GAP;
        let fixed = 2 * PAD + LABEL + GAP;
        if agents == 0 || width + 2 * MARGIN > screen.w || room < fixed + SHORTEST {
            return None;
        }
        let trace = (room / agents as i32 - fixed).clamp(SHORTEST, TRACE);
        let count = agents.min((room / (fixed + trace)) as usize);
        Some(Self {
            width,
            trace,
            count,
        })
    }

    fn pitch(&self) -> i32 {
        2 * PAD + LABEL + self.trace + GAP
    }

    fn size(&self) -> Size<i32, Logical> {
        (self.width, self.pitch() * self.count as i32 - GAP).into()
    }
}

/// One screen's readouts as last painted.
struct Canvas {
    buffer: MemoryRenderBuffer,
    device: (i32, i32),
    fit: Fit,
    scale: f64,
    /// The backings and names, which the traces are drawn over afresh each time.
    base: Vec<u8>,
    names: Vec<String>,
    /// Every trace's level when it was last painted, and when that was.
    painted: Vec<u32>,
    painted_at: f64,
}

pub struct Scope {
    traces: Vec<Trace>,
    pub reduced_motion: bool,
    /// How far in the readouts are, 0 to 1.
    shown: f32,
    last: Option<f64>,
    canvases: HashMap<String, Canvas>,
}

impl Scope {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            traces: Vec::new(),
            reduced_motion,
            shown: 0.0,
            last: None,
            canvases: HashMap::new(),
        }
    }

    /// The agents at work now. One not seen before gets a trace; one no longer among them has
    /// stopped, and its trace stays to say so.
    pub fn follow(&mut self, at_work: &[Agent]) {
        for trace in &mut self.traces {
            let agent = at_work.iter().find(|agent| agent.pid == trace.pid);
            trace.working = agent.is_some();
            // What it is called can change as its work does. A stopped one keeps its last name.
            if let Some(agent) = agent {
                trace.seen = false;
                trace.name.clone_from(&agent.name);
            }
        }
        for agent in at_work {
            if !self.traces.iter().any(|trace| trace.pid == agent.pid) {
                self.traces.push(Trace {
                    pid: agent.pid,
                    name: agent.name.clone(),
                    working: true,
                    seen: false,
                    level: 1.0,
                });
            }
        }
    }

    /// The desk was touched: whoever is there has seen which agents stopped.
    pub fn touched(&mut self) {
        for trace in &mut self.traces {
            trace.seen = !trace.working;
        }
        self.clear_seen();
    }

    /// Drops the stopped traces that have been seen, once nothing of them is on screen.
    fn clear_seen(&mut self) {
        if self.shown <= 0.0 {
            self.traces.retain(|trace| !trace.seen);
        }
    }

    /// Moves everything on to `now`, with the UI `ui` visible, and says how far in the readouts
    /// are. They come in only once the desktop has gone altogether.
    fn advance(&mut self, now: f64, ui: f32) -> f32 {
        let dt = self.last.map_or(0.0, |last| (now - last).clamp(0.0, 0.1));
        self.last = Some(now);
        if self.reduced_motion {
            self.shown = if ui <= 0.0 { 1.0 } else { 0.0 };
        } else if ui <= 0.0 {
            self.shown = (self.shown + (dt / APPEAR) as f32).min(1.0);
        } else {
            self.shown = (self.shown - (dt / LEAVE) as f32).max(0.0);
        }
        for trace in &mut self.traces {
            let target = if trace.working { 1.0 } else { 0.0 };
            if self.reduced_motion {
                trace.level = target;
            } else {
                trace.level += (target - trace.level) * (dt as f32 * SETTLE).min(1.0);
                if (target - trace.level).abs() < 0.002 {
                    trace.level = target;
                }
            }
        }
        self.clear_seen();
        self.shown
    }

    /// The readouts for the screen called `output`, `screen` logical pixels big, or nothing while
    /// the desktop is up or no agent has a trace.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        output: &str,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        ui: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let shown = self.advance(now, ui);
        if shown <= 0.0 {
            return None;
        }
        let fit = Fit::new(screen, self.traces.len())?;
        let traces = &self.traces[..fit.count];
        let logical = fit.size();
        let device = (
            (logical.w as f64 * scale).round().max(1.0) as i32,
            (logical.h as f64 * scale).round().max(1.0) as i32,
        );
        let fresh = self.canvases.get(output).is_none_or(|canvas| {
            canvas.device != device
                || canvas.fit != fit
                || canvas.scale != scale
                || !canvas
                    .names
                    .iter()
                    .eq(traces.iter().map(|trace| &trace.name))
        });
        if fresh {
            let base = base(traces, fit, device, scale)?;
            let buffer = MemoryRenderBuffer::from_slice(
                &base,
                Fourcc::Abgr8888,
                device,
                1,
                Transform::Normal,
                None,
            );
            self.canvases.insert(
                output.to_string(),
                Canvas {
                    buffer,
                    device,
                    fit,
                    scale,
                    base,
                    names: traces.iter().map(|trace| trace.name.clone()).collect(),
                    painted: Vec::new(),
                    painted_at: f64::NEG_INFINITY,
                },
            );
        }
        let canvas = self.canvases.get_mut(output)?;
        let levels: Vec<u32> = traces.iter().map(|trace| trace.level.to_bits()).collect();
        let moving = !self.reduced_motion && traces.iter().any(|trace| trace.level > 0.0);
        if levels != canvas.painted || (moving && now - canvas.painted_at >= STEP) {
            let Canvas { buffer, base, .. } = canvas;
            let size = (device.0 as usize, device.1 as usize);
            let t = (!self.reduced_motion).then_some(now);
            let mut context = buffer.render();
            let _ = context.draw(|pixels| {
                pixels.copy_from_slice(base);
                plot(pixels, size, traces, fit, scale, t);
                Ok::<_, ()>(vec![Rectangle::<i32, Buffer>::from_size(device.into())])
            });
            drop(context);
            canvas.painted = levels;
            canvas.painted_at = now;
        }
        let at: Point<f64, Logical> =
            ((screen.w - MARGIN - logical.w) as f64, MARGIN as f64).into();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            &canvas.buffer,
            Some(shown),
            Some(Rectangle::from_size(
                (device.0 as f64, device.1 as f64).into(),
            )),
            Some(logical),
            Kind::Unspecified,
        )
        .ok()
    }
}

/// The readouts' backings and names, as premultiplied pixels `device` big.
fn base(traces: &[Trace], fit: Fit, device: (i32, i32), scale: f64) -> Option<Vec<u8>> {
    let mut painter = Painter::new(device.0 as u32, device.1 as u32, scale as f32)?;
    let style = Style::new(Face::Mono, 11.0, NAME);
    let (width, height) = (fit.width as f32, (fit.pitch() - GAP) as f32);
    for (i, trace) in traces.iter().enumerate() {
        let top = (i as i32 * fit.pitch()) as f32;
        painter.fill(0.0, top, width, height, BACKING_RADIUS, BACKING);
        let name = text::ellipsize(&trace.name, &style, width - 2.0 * PAD as f32);
        painter.text(
            &name,
            PAD as f32,
            top + PAD as f32 + LABEL as f32 / 2.0 - 1.0,
            &style,
        );
    }
    Some(painter.pixmap.data().to_vec())
}

/// Draws every trace over `pixels`, a premultiplied picture `width` by `height`, as it stands at
/// time `t`. With no time there is no movement to show: the wave stands and nothing sweeps it.
fn plot(
    pixels: &mut [u8],
    (width, height): (usize, usize),
    traces: &[Trace],
    fit: Fit,
    scale: f64,
    t: Option<f64>,
) {
    let px = |logical: i32| (logical as f64 * scale).round() as i32;
    let pitch = (PITCH * scale).round().max(2.0) as i32;
    let dot = (DOT * scale).round().max(1.0) as i32;
    let (left, right) = (px(PAD), width as i32 - px(PAD) - dot);
    let span = (right - left).max(1) as f32;
    for (i, trace) in traces.iter().enumerate() {
        let top = px(i as i32 * fit.pitch() + PAD + LABEL);
        let middle = top + px(fit.trace) / 2;
        let swing = (px(fit.trace) / 2 - pitch).max(0) as f32;
        let rgb: [f32; 3] = std::array::from_fn(|c| AMBER[c] + (CYAN[c] - AMBER[c]) * trace.level);
        let time = t.unwrap_or(0.0);
        let spot = (time * SWEEP + trace.pid as f64 * 0.37).fract() as f32;
        let mut before = None;
        let mut x = left;
        while x <= right {
            let across = (x - left) as f32 / span;
            let lift = trace.wave(across, time as f32) * trace.level * swing;
            let y = middle + (lift / pitch as f32).round() as i32 * pitch;
            // Brightest where the spot is, fading back along the way it came.
            let lit = match t {
                Some(_) => {
                    let behind = (spot - across).rem_euclid(1.0);
                    DIMMEST + (1.0 - DIMMEST) * (-behind * 3.5).exp()
                }
                None => 0.85,
            };
            let alpha = FLAT + (lit - FLAT) * trace.level;
            // Dots fill the climb from the last column, so a steep wave stays one line.
            let from = before.unwrap_or(y);
            let mut at = from.min(y);
            while at <= from.max(y) {
                put(pixels, (width, height), (x, at), dot, rgb, alpha);
                at += pitch;
            }
            before = Some(y);
            x += pitch;
        }
    }
}

/// One square dot, `dot` pixels across with its corner at (`x`, `y`), in place of what was there.
fn put(
    pixels: &mut [u8],
    (width, height): (usize, usize),
    (x, y): (i32, i32),
    dot: i32,
    rgb: [f32; 3],
    alpha: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    let ink = [
        (rgb[0] * alpha).round() as u8,
        (rgb[1] * alpha).round() as u8,
        (rgb[2] * alpha).round() as u8,
        (alpha * 255.0).round() as u8,
    ];
    for row in y.max(0)..(y + dot).min(height as i32) {
        for col in x.max(0)..(x + dot).min(width as i32) {
            let i = (row as usize * width + col as usize) * 4;
            pixels[i..i + 4].copy_from_slice(&ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (1536, 960);

    fn agent(pid: u32, name: &str) -> Agent {
        Agent {
            pid,
            name: name.to_string(),
        }
    }

    /// One trace plotted on its own at `level`, and the rows of the picture that have a dot.
    fn rows(level: f32, t: Option<f64>) -> (Vec<u8>, Vec<usize>) {
        let fit = Fit::new(SCREEN.into(), 1).unwrap();
        let size = fit.size();
        let (width, height) = (size.w as usize, size.h as usize);
        let trace = Trace {
            pid: 300,
            name: String::new(),
            working: level > 0.0,
            seen: false,
            level,
        };
        let mut pixels = vec![0u8; width * height * 4];
        plot(&mut pixels, (width, height), &[trace], fit, 1.0, t);
        let lit = (0..height)
            .filter(|row| {
                pixels[row * width * 4..(row + 1) * width * 4]
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0)
            })
            .collect();
        (pixels, lit)
    }

    #[test]
    fn a_working_agent_s_trace_moves_and_a_stopped_one_lies_flat() {
        let (early, wave) = rows(1.0, Some(0.0));
        let (later, _) = rows(1.0, Some(0.5));
        assert!(wave.len() > 3 * DOT as usize, "a wave covers many rows");
        assert_ne!(early, later, "and moves");
        let (flat_early, flat) = rows(0.0, Some(0.0));
        let (flat_later, _) = rows(0.0, Some(0.5));
        assert_eq!(flat.len(), DOT as usize, "a flat line is one dot high");
        assert_eq!(flat_early, flat_later, "and still");
    }

    #[test]
    fn a_stopped_trace_is_amber_and_a_working_one_cyan() {
        let colour = |pixels: &[u8]| {
            let pixel = pixels.chunks_exact(4).find(|pixel| pixel[3] > 0).unwrap();
            (pixel[0] > pixel[2], pixel[2] > pixel[0])
        };
        assert_eq!(colour(&rows(0.0, Some(0.0)).0), (true, false));
        assert_eq!(colour(&rows(1.0, Some(0.0)).0), (false, true));
    }

    #[test]
    fn reduced_motion_keeps_the_trace_s_shape_without_moving_it() {
        let (standing, wave) = rows(1.0, None);
        assert!(wave.len() > 3 * DOT as usize);
        assert_eq!(standing, rows(1.0, None).0);
        let mut scope = Scope::new(true);
        scope.follow(&[agent(300, "kiln")]);
        scope.follow(&[]);
        scope.advance(0.0, 0.0);
        assert_eq!(
            scope.traces[0].level, 0.0,
            "a stop shows at once, with no settling"
        );
        assert_eq!(scope.shown, 1.0);
    }

    #[test]
    fn traces_show_only_once_the_desktop_has_faded() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln")]);
        assert_eq!(scope.advance(0.0, 1.0), 0.0, "the desktop is up");
        assert_eq!(scope.advance(0.1, 0.5), 0.0, "and still on its way out");
        scope.advance(0.2, 0.0);
        assert!(scope.advance(0.3, 0.0) > 0.0, "gone: they come in");
        let mut now = 0.3;
        while now < 2.0 {
            now += 0.05;
            scope.advance(now, 0.0);
        }
        assert_eq!(scope.shown, 1.0);
        while now < 3.0 {
            now += 0.05;
            scope.advance(now, 0.2);
        }
        assert_eq!(scope.shown, 0.0, "and leave as it comes back");
    }

    #[test]
    fn an_agent_that_was_not_working_has_no_trace() {
        let mut scope = Scope::new(false);
        scope.follow(&[]);
        assert!(scope.traces.is_empty());
    }

    #[test]
    fn a_stopped_trace_stays_until_the_desk_is_touched() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln"), agent(301, "ledger")]);
        scope.follow(&[agent(301, "ledger")]);
        assert_eq!(scope.traces.len(), 2);
        assert!(!scope.traces[0].working);
        scope.touched();
        assert_eq!(scope.traces.len(), 1, "the one still working stays");
        assert_eq!(scope.traces[0].name, "ledger");
    }

    #[test]
    fn a_seen_trace_leaves_only_once_it_is_out_of_sight() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln")]);
        scope.follow(&[]);
        let mut now = 0.0;
        while now < 1.0 {
            now += 0.05;
            scope.advance(now, 0.0);
        }
        scope.touched();
        assert_eq!(scope.traces.len(), 1, "still fading out with the rest");
        while now < 2.0 {
            now += 0.05;
            scope.advance(now, 1.0);
        }
        assert!(scope.traces.is_empty());
    }

    #[test]
    fn an_agent_that_starts_again_gets_its_wave_back() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln")]);
        scope.follow(&[]);
        scope.follow(&[agent(300, "kiln")]);
        assert_eq!(scope.traces.len(), 1);
        assert!(scope.traces[0].working);
    }

    #[test]
    fn a_trace_takes_its_agent_s_new_name() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "robin: reading the brief")]);
        scope.follow(&[agent(300, "robin: mending the kiln door")]);
        assert_eq!(scope.traces.len(), 1);
        assert_eq!(scope.traces[0].name, "robin: mending the kiln door");
        scope.follow(&[]);
        assert_eq!(scope.traces[0].name, "robin: mending the kiln door");
    }

    #[test]
    fn however_many_agents_the_readouts_stay_on_the_screen() {
        for agents in 1..40 {
            let fit = Fit::new(SCREEN.into(), agents).unwrap();
            assert!(fit.size().h <= SCREEN.1 - 2 * MARGIN, "{agents} agents");
            assert!(fit.count >= agents.min(12), "{agents} agents");
        }
        assert_eq!(Fit::new(SCREEN.into(), 0), None);
    }

    #[test]
    fn each_agent_has_a_wave_of_its_own() {
        let trace = |pid| Trace {
            pid,
            name: String::new(),
            working: true,
            seen: false,
            level: 1.0,
        };
        assert_ne!(trace(300).shape(), trace(301).shape());
        assert_eq!(trace(300).shape(), trace(300).shape());
    }
}

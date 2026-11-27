//! The readouts the faded desktop shows for AI agent sessions: one small oscilloscope for each,
//! stacked down the right edge of the wallpaper.
//!
//! Two states, each with its own look:
//! - **Active** (cyan wave): the agent is working, itself or through its helpers.
//! - **Idle** (flat amber line): the agent is open but between tasks.
//!
//! The agent's process ID sits in the readout's top right corner, so its session can be found.
//!
//! Under the trace, each helper the agent has working gets a row: a small wave of its own beside
//! what it is doing. Each task it has running in the background gets a row after those, the same
//! but in the tertiary colour, lightened where it needs to be to read on the backing, and its wave
//! moves for as long as the task runs, idle or not.
//!
//! A readout appears for every open session, not only ones that are currently working. When a
//! session's process exits its readout settles amber and clears at the next key or pointer input.
//! Which sessions are open comes from the notes they leave (`working.rs`); with none, nothing
//! here draws.
//!
//! Reduced motion keeps the same picture without the movement: standing waves for an active
//! agent and its helpers and tasks, and a flat line for idle or stopped.

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
    working::{Agent, AgentStatus},
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
/// The space kept between the name and the process ID beside it.
const PID_GAP: i32 = 8;
/// A trace's height, and the least it is squeezed to before readouts are left off instead.
const TRACE: i32 = 44;
const SHORTEST: i32 = 15;
const GAP: i32 = 8;
/// A helper's or task's row under the trace, the small wave at its start, and the space after
/// that wave.
const HELPER: i32 = 14;
const HELPER_WAVE: i32 = 24;
const HELPER_GAP: i32 = 6;
/// The most rows listed under a readout, helpers and tasks together; past this the last row says
/// how many more there are.
const MOST_ROWS: usize = 4;
/// A trace is drawn as dots this big, this far apart.
const DOT: f64 = 2.0;
const PITCH: f64 = 3.0;

/// The backing behind a readout: the desktop's backdrop colour, letting some wallpaper through.
const BACKING: u32 = 0x0b0e13b8;
const BACKING_RADIUS: f32 = 7.0;
const NAME: u32 = 0xaab2c0ff;

/// The backing as it shows over `behind`: it lets some of the wallpaper through.
fn backing_over(behind: [f32; 3]) -> [f32; 3] {
    let [r, g, b, a] = BACKING.to_be_bytes().map(|c| c as f32 / 255.0);
    std::array::from_fn(|c| [r, g, b][c] * a + behind[c] * (1.0 - a))
}

/// A task's wave colour from the tertiary, each channel 0–255: as light as it needs to be for
/// marks that carry meaning to read on the backing, and no lighter. It is worked out against the
/// backing over white, the brightest anything behind it can be: the readouts show over the
/// living wallpaper, which draws marks close to white. A colour light enough for that is lighter
/// than the backing over anything darker, and so reads wherever it sits.
fn task_ink(tertiary: [f32; 3]) -> [f32; 3] {
    use slipstream_config::colour::{NON_TEXT, legible};
    legible(tertiary, backing_over([1.0; 3]), NON_TEXT).map(|c| c * 255.0)
}
const HELPER_NAME: u32 = 0x7f8898ff;

/// How fast a wave settles flat or rises again, as a rate a second.
const SETTLE: f32 = 3.0;
/// The readouts come in once the desktop has gone, and leave ahead of it coming back.
const APPEAR: f64 = 0.6;
const LEAVE: f64 = 0.15;
/// Traces are redrawn 30 times a second when a wave is moving.
const STEP: f64 = 1.0 / 30.0;
/// How many times a second the bright spot crosses a trace.
const SWEEP: f64 = 0.6;
/// How bright a flat line is, a wave where the spot passed longest ago, and a helper's wave.
const FLAT: f32 = 0.85;
const DIMMEST: f32 = 0.25;
const HELPER_LIT: f32 = 0.7;

/// One agent's readout.
#[derive(Debug, Clone, PartialEq)]
struct Trace {
    pid: u32,
    name: String,
    /// What each of its helpers is doing.
    helpers: Vec<String>,
    /// What each of its background tasks is.
    tasks: Vec<String>,
    /// The agent's session is still open (its note file exists and its process is running).
    alive: bool,
    status: AgentStatus,
    /// Session has ended and the desk has been touched since: clears once out of sight.
    seen: bool,
    /// Colour level, animating 0 (amber) to 1 (cyan): cyan while active.
    colour: f32,
    /// Wave amplitude, animating 0 (flat) to 1 (full wave): wave only while active.
    amp: f32,
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

    /// The `nth` row's small wave, -1 to 1: a slice of the agent's own, offset so that no two
    /// rows move together.
    fn helper_wave(&self, nth: usize, across: f32, t: f32) -> f32 {
        self.wave(
            0.35 * across + 0.29 * nth as f32,
            1.3 * t + 1.7 * nth as f32,
        )
    }

    /// Whether anything on the readout moves: its wave while it has one, and its tasks' for as
    /// long as they run.
    fn moving(&self) -> bool {
        self.amp > 0.0 || !self.tasks.is_empty()
    }

    /// The rows listed under the trace: what each helper is doing, then each background task,
    /// with what each row is.
    fn rows(&self) -> Rows {
        rows(&self.helpers, &self.tasks)
    }
}

/// What a row under a trace stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Helper,
    Task,
    /// The last row, saying how many more there are than there is room for.
    More,
}

/// The rows under a trace, each with what it stands for.
type Rows = Vec<(String, Row)>;

/// The rows listed under a readout for `helpers` and `tasks`, as `Trace::rows`.
fn rows(helpers: &[String], tasks: &[String]) -> Rows {
    let all = helpers
        .iter()
        .map(|h| (h.clone(), Row::Helper))
        .chain(tasks.iter().map(|t| (t.clone(), Row::Task)));
    let count = helpers.len() + tasks.len();
    if count <= MOST_ROWS {
        return all.collect();
    }
    let mut rows: Vec<_> = all.take(MOST_ROWS - 1).collect();
    rows.push((format!("+{} more", count - rows.len()), Row::More));
    rows
}

/// How the readouts are laid out on a screen, in logical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fit {
    width: i32,
    /// The height of each trace.
    trace: i32,
    /// The top of each readout there is room for, and how many helper rows it lists.
    readouts: Vec<(i32, usize)>,
}

impl Fit {
    /// Room down a `screen` for readouts listing `rows` helper rows each: traces shrink before
    /// any readout is left off.
    fn new(screen: Size<i32, Logical>, rows: &[usize]) -> Option<Self> {
        let width = ((screen.w as f32 * WIDTH_OF_SCREEN) as i32).clamp(NARROWEST, WIDEST);
        let room = screen.h - 2 * MARGIN + GAP;
        let fixed = |rows: usize| 2 * PAD + LABEL + rows as i32 * HELPER + GAP;
        let all: i32 = rows.iter().map(|&r| fixed(r)).sum();
        if rows.is_empty() || width + 2 * MARGIN > screen.w || room < fixed(rows[0]) + SHORTEST {
            return None;
        }
        let trace = ((room - all) / rows.len() as i32).clamp(SHORTEST, TRACE);
        let mut readouts = Vec::new();
        let mut top = 0;
        for &r in rows {
            let next = top + fixed(r) + trace;
            if next > room {
                break;
            }
            readouts.push((top, r));
            top = next;
        }
        Some(Self {
            width,
            trace,
            readouts,
        })
    }

    fn count(&self) -> usize {
        self.readouts.len()
    }

    /// How tall the readout listing `rows` helper rows is.
    fn height(&self, rows: usize) -> i32 {
        2 * PAD + LABEL + self.trace + rows as i32 * HELPER
    }

    fn size(&self) -> Size<i32, Logical> {
        let bottom = self
            .readouts
            .last()
            .map_or(0, |&(top, rows)| top + self.height(rows));
        (self.width, bottom).into()
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
    names: Vec<(u32, String, Rows)>,
    /// Every trace's colour and amp packed together when last painted, and when that was.
    painted: Vec<u64>,
    painted_at: f64,
}

pub struct Scope {
    traces: Vec<Trace>,
    pub reduced_motion: bool,
    /// How far in the readouts are, 0 to 1.
    shown: f32,
    last: Option<f64>,
    canvases: HashMap<String, Canvas>,
    /// A wave at work, a line that has stopped, and a background task's wave, each channel
    /// 0–255: the primary and secondary colours as the settings have them, and the tertiary as
    /// `task_ink` makes it.
    working: [f32; 3],
    stopped: [f32; 3],
    task: [f32; 3],
}

impl Scope {
    pub fn new(reduced_motion: bool) -> Self {
        let channels = |rgb: [f32; 3]| rgb.map(|c| c * 255.0);
        let borders = slipstream_config::Borders::default();
        Self {
            traces: Vec::new(),
            reduced_motion,
            shown: 0.0,
            last: None,
            canvases: HashMap::new(),
            working: channels(borders.selected_tile_rgb()),
            stopped: channels(borders.bullet_time_rgb()),
            task: task_ink(borders.accent_rgb()),
        }
    }

    /// The colours from the settings, each channel 0–1: the primary for at work, the secondary
    /// for stopped and the tertiary for background tasks. Readouts on screen repaint in them.
    pub fn set_colours(&mut self, working: [f32; 3], stopped: [f32; 3], task: [f32; 3]) {
        let channels = |rgb: [f32; 3]| rgb.map(|c| c * 255.0);
        let colours = (channels(working), channels(stopped), task_ink(task));
        if colours == (self.working, self.stopped, self.task) {
            return;
        }
        (self.working, self.stopped, self.task) = colours;
        for canvas in self.canvases.values_mut() {
            canvas.painted.clear();
        }
    }

    /// Updates which agents have open sessions. One not seen before gets a readout; one no longer
    /// among them has gone, and its readout stays amber until the desk is touched.
    pub fn follow(&mut self, at_work: &[Agent]) {
        for trace in &mut self.traces {
            let agent = at_work.iter().find(|agent| agent.pid == trace.pid);
            trace.alive = agent.is_some();
            match agent {
                Some(agent) => {
                    trace.seen = false;
                    trace.status = agent.status;
                    trace.helpers.clone_from(&agent.helpers);
                    trace.tasks.clone_from(&agent.tasks);
                    // Keep the name from before going idle; only refresh while working.
                    if agent.status == AgentStatus::Active {
                        trace.name.clone_from(&agent.name);
                    }
                }
                // An agent that has gone has no helpers or tasks working for it either.
                None => {
                    trace.helpers.clear();
                    trace.tasks.clear();
                }
            }
        }
        for agent in at_work {
            if !self.traces.iter().any(|trace| trace.pid == agent.pid) {
                let active = agent.status == AgentStatus::Active;
                self.traces.push(Trace {
                    pid: agent.pid,
                    name: agent.name.clone(),
                    helpers: agent.helpers.clone(),
                    tasks: agent.tasks.clone(),
                    alive: true,
                    status: agent.status,
                    seen: false,
                    colour: if active { 1.0 } else { 0.0 },
                    amp: if active { 1.0 } else { 0.0 },
                });
            }
        }
    }

    /// The desk was touched: readouts for sessions that have ended are cleared.
    pub fn touched(&mut self) {
        for trace in &mut self.traces {
            // Only remove readouts for sessions whose process has gone. Idle sessions (alive but
            // between tasks) stay: the agent is still running and may start working again.
            trace.seen = !trace.alive;
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
            let target = if trace.alive && trace.status == AgentStatus::Active {
                1.0
            } else {
                0.0
            };
            let (colour_target, amp_target) = (target, target);
            if self.reduced_motion {
                trace.colour = colour_target;
                trace.amp = amp_target;
            } else {
                trace.colour += (colour_target - trace.colour) * (dt as f32 * SETTLE).min(1.0);
                if (colour_target - trace.colour).abs() < 0.002 {
                    trace.colour = colour_target;
                }
                trace.amp += (amp_target - trace.amp) * (dt as f32 * SETTLE).min(1.0);
                if (amp_target - trace.amp).abs() < 0.002 {
                    trace.amp = amp_target;
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
            // Out of sight, the readouts are painted afresh when they next come in.
            self.canvases.clear();
            return None;
        }
        let rows: Vec<usize> = self.traces.iter().map(|t| t.rows().len()).collect();
        let fit = Fit::new(screen, &rows)?;
        let traces = &self.traces[..fit.count()];
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
                    .map(|(pid, name, rows)| (*pid, name.clone(), rows.clone()))
                    .eq(traces
                        .iter()
                        .map(|trace| (trace.pid, trace.name.clone(), trace.rows())))
        });
        if fresh {
            let base = base(traces, &fit, device, scale)?;
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
                    fit: fit.clone(),
                    scale,
                    base,
                    names: traces
                        .iter()
                        .map(|trace| (trace.pid, trace.name.clone(), trace.rows()))
                        .collect(),
                    painted: Vec::new(),
                    painted_at: f64::NEG_INFINITY,
                },
            );
        }
        let canvas = self.canvases.get_mut(output)?;
        let levels: Vec<u64> = traces
            .iter()
            .map(|t| ((t.colour.to_bits() as u64) << 32) | t.amp.to_bits() as u64)
            .collect();
        // A wave needs 30 fps; a change of colour repaints through `levels`.
        let waving = !self.reduced_motion && traces.iter().any(Trace::moving);
        if levels != canvas.painted || (waving && now - canvas.painted_at >= STEP) {
            let Canvas { buffer, base, .. } = canvas;
            let size = (device.0 as usize, device.1 as usize);
            let t = (!self.reduced_motion).then_some(now);
            let mut context = buffer.render();
            let _ = context.draw(|pixels| {
                pixels.copy_from_slice(base);
                plot(
                    pixels,
                    size,
                    traces,
                    (self.working, self.stopped, self.task),
                    &fit,
                    scale,
                    t,
                );
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

/// The readouts' backings, names and rows, as premultiplied pixels `device` big.
fn base(traces: &[Trace], fit: &Fit, device: (i32, i32), scale: f64) -> Option<Vec<u8>> {
    let mut painter = Painter::new(device.0 as u32, device.1 as u32, scale as f32)?;
    let style = Style::new(Face::Mono, 11.0, NAME);
    let helper_style = Style::new(Face::Mono, 10.0, HELPER_NAME);
    let width = fit.width as f32;
    for (trace, &(top, count)) in traces.iter().zip(&fit.readouts) {
        let top = top as f32;
        let height = fit.height(count) as f32;
        painter.fill(0.0, top, width, height, BACKING_RADIUS, BACKING);
        let line = top + PAD as f32 + LABEL as f32 / 2.0 - 1.0;
        let pid = format!("PID: {}", trace.pid);
        let pid_width = text::width(&pid, &helper_style);
        painter.text(&pid, width - PAD as f32 - pid_width, line, &helper_style);
        let room = width - 2.0 * PAD as f32 - pid_width - PID_GAP as f32;
        let name = text::ellipsize(&trace.name, &style, room);
        painter.text(&name, PAD as f32, line, &style);
        let left = (PAD + HELPER_WAVE + HELPER_GAP) as f32;
        let first = top + (PAD + LABEL + fit.trace) as f32;
        for (i, (row, _)) in trace.rows().iter().enumerate() {
            let row = text::ellipsize(row, &helper_style, width - left - PAD as f32);
            let middle = first + (i as i32 * HELPER) as f32 + HELPER as f32 / 2.0 - 1.0;
            painter.text(&row, left, middle, &helper_style);
        }
    }
    Some(painter.pixmap.data().to_vec())
}

/// Draws every trace over `pixels`, a premultiplied picture `width` by `height`, as it stands at
/// time `t`. With no time there is no movement to show: the wave stands and nothing sweeps it.
fn plot(
    pixels: &mut [u8],
    (width, height): (usize, usize),
    traces: &[Trace],
    (working, stopped, task): ([f32; 3], [f32; 3], [f32; 3]),
    fit: &Fit,
    scale: f64,
    t: Option<f64>,
) {
    let px = |logical: i32| (logical as f64 * scale).round() as i32;
    let pitch = (PITCH * scale).round().max(2.0) as i32;
    let dot = (DOT * scale).round().max(1.0) as i32;
    let size = (width, height);
    let (left, right) = (px(PAD), width as i32 - px(PAD) - dot);
    let time = t.unwrap_or(0.0);
    for (trace, &(top, _)) in traces.iter().zip(&fit.readouts) {
        let rgb: [f32; 3] =
            std::array::from_fn(|c| stopped[c] + (working[c] - stopped[c]) * trace.colour);
        let spot = (time * SWEEP + trace.pid as f64 * 0.37).fract() as f32;
        let trace_top = top + PAD + LABEL;
        let pen = Pen { pitch, dot, rgb };
        pen.line(
            pixels,
            size,
            (left, right),
            (px(trace_top) + px(fit.trace) / 2, px(fit.trace)),
            |across| {
                let lift = trace.wave(across, time as f32) * trace.amp;
                // Brightest where the spot is, fading back along the way it came.
                let lit = match t {
                    Some(_) => {
                        let behind = (spot - across).rem_euclid(1.0);
                        DIMMEST + (1.0 - DIMMEST) * (-behind * 3.5).exp()
                    }
                    None => 0.85,
                };
                // Stopped: a steady amber line. Working: the swept cyan wave. Blends between.
                (lift, FLAT + (lit - FLAT) * trace.amp)
            },
        );
        let first = trace_top + fit.trace;
        let wave_right = left + px(HELPER_WAVE) - dot;
        for (i, (_, row)) in trace.rows().iter().enumerate() {
            // A helper's wave is in the trace's own colour and settles with it. A task's is in its
            // own colour, drawn whole so it keeps its contrast, and moves while the task runs,
            // whether the agent is working or idle.
            let (rgb, amp, lit) = match row {
                Row::Helper => (pen.rgb, trace.amp, HELPER_LIT),
                Row::Task => (task, 1.0, 1.0),
                Row::More => continue,
            };
            Pen { rgb, ..pen }.line(
                pixels,
                size,
                (left, wave_right),
                (px(first + i as i32 * HELPER) + px(HELPER) / 2, px(HELPER)),
                |across| {
                    let lift = trace.helper_wave(i, across, time as f32) * amp;
                    (lift, lit)
                },
            );
        }
    }
}

/// How a line of dots is drawn: their spacing, size and colour.
struct Pen {
    pitch: i32,
    dot: i32,
    rgb: [f32; 3],
}

impl Pen {
    /// A line of dots across `from` to `to`, about the row at `middle` in a band `band` pixels
    /// high. `shape` gives the line's lift, -1 to 1 of the band, and its brightness at each point
    /// `across` the line from 0 to 1.
    fn line(
        &self,
        pixels: &mut [u8],
        size: (usize, usize),
        (from, to): (i32, i32),
        (middle, band): (i32, i32),
        shape: impl Fn(f32) -> (f32, f32),
    ) {
        let Self { pitch, dot, rgb } = *self;
        let span = (to - from).max(1) as f32;
        let swing = (band / 2 - pitch).max(0) as f32;
        let mut before = None;
        let mut x = from;
        while x <= to {
            let (lift, alpha) = shape((x - from) as f32 / span);
            let y = middle + (lift * swing / pitch as f32).round() as i32 * pitch;
            // Dots fill the climb from the last column, so a steep wave stays one line.
            let last = before.unwrap_or(y);
            let mut at = last.min(y);
            while at <= last.max(y) {
                put(pixels, size, (x, at), dot, rgb, alpha);
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
            status: AgentStatus::Active,
            helpers: Vec::new(),
            tasks: Vec::new(),
        }
    }

    fn idle_agent(pid: u32, name: &str) -> Agent {
        Agent {
            status: AgentStatus::Idle,
            ..agent(pid, name)
        }
    }

    fn helped_agent(pid: u32, name: &str, helpers: &[&str]) -> Agent {
        Agent {
            helpers: helpers.iter().map(|h| h.to_string()).collect(),
            ..agent(pid, name)
        }
    }

    fn trace(pid: u32, helpers: &[&str], level: f32) -> Trace {
        Trace {
            pid,
            name: String::new(),
            helpers: helpers.iter().map(|h| h.to_string()).collect(),
            tasks: Vec::new(),
            alive: true,
            status: AgentStatus::Active,
            seen: false,
            colour: level,
            amp: level,
        }
    }

    /// `trace` plotted on its own, and the picture's width and height.
    fn plotted(trace: Trace, t: Option<f64>) -> (Vec<u8>, usize, usize) {
        let fit = Fit::new(SCREEN.into(), &[trace.rows().len()]).unwrap();
        let size = fit.size();
        let (width, height) = (size.w as usize, size.h as usize);
        let mut pixels = vec![0u8; width * height * 4];
        let scope = Scope::new(false);
        let colours = (scope.working, scope.stopped, scope.task);
        plot(
            &mut pixels,
            (width, height),
            &[trace],
            colours,
            &fit,
            1.0,
            t,
        );
        (pixels, width, height)
    }

    #[test]
    fn the_process_id_sits_in_the_top_right_corner_however_long_the_name() {
        let fit = Fit::new(SCREEN.into(), &[0]).unwrap();
        let size = fit.size();
        let (width, height) = (size.w as usize, size.h as usize);
        let named = |name: &str| {
            let readout = Trace {
                name: name.to_string(),
                ..trace(1444309, &[], 1.0)
            };
            base(&[readout], &fit, (size.w, size.h), 1.0).unwrap()
        };
        let unnamed = named("");
        let backing = unnamed[(height / 2 * width + width / 2) * 4..][..4].to_vec();
        let band = PAD as usize..(PAD + LABEL) as usize;
        let lit: Vec<usize> = band
            .clone()
            .flat_map(|row| (0..width).map(move |col| (row, col)))
            .filter(|&(row, col)| unnamed[(row * width + col) * 4..][..4] != backing[..])
            .map(|(_, col)| col)
            .collect();
        assert!(!lit.is_empty(), "the PID is drawn");
        assert!(lit.iter().all(|&col| col > width / 2), "on the right");
        assert!(
            *lit.iter().max().unwrap() <= width - PAD as usize,
            "inside the padding"
        );
        // A name too long to fit gives way to it rather than writing over it.
        let crowded = named(&"kiln ".repeat(40));
        let first = *lit.iter().min().unwrap();
        for row in band {
            let at = (row * width + first) * 4;
            assert_eq!(
                crowded[at..][..(width - first) * 4],
                unnamed[at..][..(width - first) * 4]
            );
        }
    }

    /// One trace plotted on its own at `colour`/`amp`, and the rows of the picture that have a dot.
    fn rows(colour: f32, amp: f32, t: Option<f64>) -> (Vec<u8>, Vec<usize>) {
        let trace = Trace {
            colour,
            amp,
            ..trace(300, &[], 0.0)
        };
        let (pixels, width, height) = plotted(trace, t);
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
        let (early, wave) = rows(1.0, 1.0, Some(0.0));
        let (later, _) = rows(1.0, 1.0, Some(0.5));
        assert!(wave.len() > 3 * DOT as usize, "a wave covers many rows");
        assert_ne!(early, later, "and moves");
        let (flat_early, flat) = rows(0.0, 0.0, Some(0.0));
        let (flat_later, _) = rows(0.0, 0.0, Some(0.5));
        assert_eq!(flat.len(), DOT as usize, "a flat line is one dot high");
        assert_eq!(flat_early, flat_later, "and still");
    }

    #[test]
    fn each_helper_gets_a_moving_wave_of_its_own_under_the_trace() {
        let below = |pixels: &[u8], width: usize, from: usize| {
            pixels[from * width * 4..]
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 0)
                .count()
        };
        let under = (PAD + LABEL + TRACE) as usize;
        let (alone, width, _) = plotted(trace(300, &[], 1.0), Some(0.0));
        assert_eq!(below(&alone, width, under), 0, "no helpers, nothing under");
        let (early, width, height) = plotted(trace(300, &["glazing", "sweeping"], 1.0), Some(0.0));
        let (later, ..) = plotted(trace(300, &["glazing", "sweeping"], 1.0), Some(0.5));
        assert_eq!(height, under + (PAD + 2 * HELPER) as usize);
        let second = under + HELPER as usize;
        assert!(
            below(&early, width, under) > below(&early, width, second),
            "a row each"
        );
        assert!(below(&early, width, second) > 0);
        assert_ne!(
            early[under * width * 4..],
            later[under * width * 4..],
            "and they move"
        );
    }

    #[test]
    fn past_the_room_for_rows_the_last_says_how_many_more() {
        let few = trace(300, &["a", "b"], 1.0);
        assert_eq!(
            few.rows(),
            [("a".into(), Row::Helper), ("b".into(), Row::Helper)]
        );
        let many = trace(300, &["a", "b", "c", "d", "e", "f"], 1.0);
        let rows = many.rows();
        assert_eq!(rows.len(), MOST_ROWS);
        assert_eq!(rows[MOST_ROWS - 1], ("+3 more".to_string(), Row::More));
    }

    /// A trace whose agent has `helpers` and background `tasks`.
    fn tasked(helpers: &[&str], tasks: &[&str], level: f32) -> Trace {
        Trace {
            tasks: tasks.iter().map(|t| t.to_string()).collect(),
            ..trace(300, helpers, level)
        }
    }

    #[test]
    fn tasks_are_listed_after_helpers_and_share_their_room() {
        let both = tasked(&["glazing"], &["serving"], 1.0);
        assert_eq!(
            both.rows(),
            [
                ("glazing".into(), Row::Helper),
                ("serving".into(), Row::Task)
            ]
        );
        let crowded = tasked(&["a", "b"], &["c", "d", "e"], 1.0);
        assert_eq!(
            crowded.rows(),
            [
                ("a".into(), Row::Helper),
                ("b".into(), Row::Helper),
                ("c".into(), Row::Task),
                ("+2 more".into(), Row::More),
            ],
            "helpers first, then tasks, in one count"
        );
        let busy = tasked(&["a", "b", "c", "d"], &["e"], 1.0);
        assert_eq!(busy.rows()[MOST_ROWS - 1], ("+2 more".into(), Row::More));
    }

    #[test]
    fn a_task_s_wave_is_in_the_tertiary_colour_and_a_helper_s_in_the_trace_s() {
        let under = (PAD + LABEL + TRACE) as usize;
        let scope = Scope::new(false);
        let ink = |rgb: [f32; 3]| rgb.map(|c| (c * HELPER_LIT).round() as u8);
        let whole = |rgb: [f32; 3]| rgb.map(|c| c.round() as u8);
        let row_colours = |pixels: &[u8], width: usize, row: usize| {
            let from = (under + row * HELPER as usize) * width * 4;
            let to = from + HELPER as usize * width * 4;
            let mut seen: Vec<[u8; 3]> = pixels[from..to]
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 0)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            seen.dedup();
            seen
        };
        for level in [1.0, 0.0] {
            let (pixels, width, _) = plotted(tasked(&["glazing"], &["serving"], level), Some(0.0));
            let trace_ink = if level > 0.0 {
                scope.working
            } else {
                scope.stopped
            };
            assert_eq!(row_colours(&pixels, width, 0), [ink(trace_ink)]);
            assert_eq!(row_colours(&pixels, width, 1), [whole(scope.task)]);
        }
    }

    /// The promise in the guide: a task's wave reads on the trace's backing, whatever wallpaper
    /// shows through it, from the darkest to the brightest.
    #[test]
    fn a_task_s_wave_reads_on_the_backing_in_any_colour_set_over_any_wallpaper() {
        use slipstream_config::colour::{NON_TEXT, contrast, oklch};
        let drawn = |ink: [f32; 3]| ink.map(|c| c.round() / 255.0);
        let behind = [
            [0.0; 3],
            [1.0; 3],
            [1.0, 0.9, 0.2],
            [0.2, 0.9, 1.0],
            [0.5; 3],
        ];
        let reads = |ink: [f32; 3]| {
            behind
                .iter()
                .all(|&wallpaper| contrast(ink, backing_over(wallpaper)) >= NON_TEXT)
        };
        // The default deep teal is too dark on the backing as it is, so it is lifted, keeping its
        // hue.
        let teal = slipstream_config::Borders::default().accent_rgb();
        assert!(!reads(teal));
        let lifted = drawn(Scope::new(false).task);
        assert!(reads(lifted), "{lifted:?}");
        assert!((oklch(lifted).2 - oklch(teal).2).abs() < 1.0);
        let mut scope = Scope::new(false);
        let set = |scope: &mut Scope, tertiary| {
            scope.set_colours(scope.working.map(|c| c / 255.0), [1.0, 0.7, 0.3], tertiary);
            scope.task
        };
        for dark in [
            [0.0; 3],
            [0.4, 0.0, 0.0],
            [0.05, 0.05, 0.3],
            [0.3, 0.2, 0.5],
        ] {
            assert!(reads(drawn(set(&mut scope, dark))), "{dark:?}");
        }
        // A tertiary that already reads is drawn as it is.
        let pale = [0.4, 0.8, 0.6];
        assert_eq!(set(&mut scope, pale), pale.map(|c| c * 255.0));
    }

    #[test]
    fn an_idle_trace_lies_flat_while_its_tasks_waves_move() {
        let idle = tasked(&[], &["serving"], 0.0);
        assert!(idle.moving(), "redrawn while a task runs");
        assert!(!trace(300, &[], 0.0).moving());
        let (early, width, _) = plotted(idle.clone(), Some(0.0));
        let (later, ..) = plotted(idle, Some(0.5));
        let under = (PAD + LABEL + TRACE) as usize * width * 4;
        assert_eq!(early[..under], later[..under], "the trace stays flat");
        assert_ne!(early[under..], later[under..], "the task's wave moves");
    }

    #[test]
    fn an_idle_trace_keeps_its_tasks_and_a_gone_one_drops_them() {
        let mut scope = Scope::new(false);
        let serving = Agent {
            tasks: vec!["serving".to_string()],
            ..idle_agent(300, "kiln")
        };
        scope.follow(std::slice::from_ref(&serving));
        assert_eq!(scope.traces[0].status, AgentStatus::Idle);
        assert_eq!(scope.traces[0].tasks, ["serving"]);
        scope.follow(&[]);
        assert!(scope.traces[0].tasks.is_empty());
    }

    #[test]
    fn reduced_motion_stands_task_waves_still_as_it_does_helpers() {
        let under = (PAD + LABEL + TRACE) as usize;
        let lit_rows = |pixels: &[u8], width: usize| {
            (under..under + HELPER as usize)
                .filter(|row| {
                    pixels[row * width * 4..(row + 1) * width * 4]
                        .chunks_exact(4)
                        .any(|pixel| pixel[3] > 0)
                })
                .count()
        };
        for level in [1.0, 0.0] {
            let (standing, width, _) = plotted(tasked(&[], &["serving"], level), None);
            assert!(
                lit_rows(&standing, width) > DOT as usize,
                "a standing wave, not a line, idle or not"
            );
            let (again, ..) = plotted(tasked(&[], &["serving"], level), None);
            assert_eq!(standing, again);
        }
        let (early, ..) = plotted(tasked(&[], &["serving"], 1.0), Some(0.0));
        let (later, ..) = plotted(tasked(&[], &["serving"], 1.0), Some(0.5));
        assert_ne!(early, later, "with motion, a task's wave moves");
    }

    #[test]
    fn a_trace_lists_its_agent_s_helpers_until_the_agent_goes() {
        let mut scope = Scope::new(false);
        scope.follow(&[helped_agent(300, "kiln", &["glazing"])]);
        assert_eq!(scope.traces[0].helpers, ["glazing"]);
        scope.follow(&[helped_agent(300, "kiln", &["glazing", "sweeping"])]);
        assert_eq!(scope.traces[0].helpers, ["glazing", "sweeping"]);
        scope.follow(&[]);
        assert!(scope.traces[0].helpers.is_empty());
    }

    #[test]
    fn a_stopped_trace_is_amber_and_a_working_one_cyan() {
        let colour = |pixels: &[u8]| {
            let pixel = pixels.chunks_exact(4).find(|pixel| pixel[3] > 0).unwrap();
            (pixel[0] > pixel[2], pixel[2] > pixel[0])
        };
        assert_eq!(colour(&rows(0.0, 0.0, Some(0.0)).0), (true, false));
        assert_eq!(colour(&rows(1.0, 1.0, Some(0.0)).0), (false, true));
    }

    #[test]
    fn reduced_motion_keeps_the_trace_s_shape_without_moving_it() {
        let (standing, wave) = rows(1.0, 1.0, None);
        assert!(wave.len() > 3 * DOT as usize);
        assert_eq!(standing, rows(1.0, 1.0, None).0);
        let mut scope = Scope::new(true);
        scope.follow(&[agent(300, "kiln")]);
        scope.follow(&[idle_agent(300, "kiln")]);
        scope.advance(0.0, 0.0);
        assert_eq!(
            scope.traces[0].amp, 0.0,
            "idle shows flat at once, with no settling"
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
    fn no_notes_means_no_traces() {
        let mut scope = Scope::new(false);
        scope.follow(&[]);
        assert!(scope.traces.is_empty());
    }

    #[test]
    fn an_idle_session_shows_a_trace_from_the_start() {
        let mut scope = Scope::new(false);
        scope.follow(&[idle_agent(300, "kiln")]);
        assert_eq!(
            scope.traces.len(),
            1,
            "idle session gets a trace immediately"
        );
        assert_eq!(scope.traces[0].status, AgentStatus::Idle);
        assert_eq!(scope.traces[0].colour, 0.0, "starts amber");
    }

    #[test]
    fn a_dead_trace_stays_until_the_desk_is_touched_but_an_idle_one_persists() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln"), agent(301, "ledger")]);
        // Agent 300 goes idle (still alive), agent 301 disappears (process gone).
        scope.follow(&[idle_agent(300, "kiln")]);
        assert_eq!(scope.traces.len(), 2);
        assert!(scope.traces[0].alive, "idle agent is still alive");
        assert!(!scope.traces[1].alive, "gone agent is dead");
        scope.touched();
        assert_eq!(scope.traces.len(), 1, "dead trace cleared, idle one kept");
        assert_eq!(scope.traces[0].name, "kiln");
        assert_eq!(scope.traces[0].status, AgentStatus::Idle);
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
    fn an_idle_agent_that_starts_working_gets_its_wave_back() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "kiln")]);
        scope.follow(&[idle_agent(300, "kiln")]);
        scope.follow(&[agent(300, "kiln")]);
        assert_eq!(scope.traces.len(), 1);
        assert!(scope.traces[0].alive);
        assert_eq!(scope.traces[0].status, AgentStatus::Active);
    }

    #[test]
    fn a_trace_takes_its_agent_s_new_name_only_while_active() {
        let mut scope = Scope::new(false);
        scope.follow(&[agent(300, "robin: reading the brief")]);
        scope.follow(&[agent(300, "robin: mending the kiln door")]);
        assert_eq!(scope.traces.len(), 1);
        assert_eq!(scope.traces[0].name, "robin: mending the kiln door");
        // Idle: keeps name from before.
        scope.follow(&[idle_agent(300, "robin: done")]);
        assert_eq!(
            scope.traces[0].name, "robin: mending the kiln door",
            "name stays frozen while idle"
        );
        // Active again: name updates.
        scope.follow(&[agent(300, "robin: back to work")]);
        assert_eq!(
            scope.traces[0].name, "robin: back to work",
            "name resumes updating"
        );
    }

    #[test]
    fn however_many_agents_the_readouts_stay_on_the_screen() {
        for agents in 1..40 {
            for helpers in [0, 1, MOST_ROWS] {
                let fit = Fit::new(SCREEN.into(), &vec![helpers; agents]).unwrap();
                let said = format!("{agents} agents with {helpers} rows each");
                assert!(fit.size().h <= SCREEN.1 - 2 * MARGIN, "{said}");
                assert!(fit.count() >= agents.min(6), "{said}");
                assert!(fit.trace >= SHORTEST, "{said}");
            }
        }
        assert_eq!(Fit::new(SCREEN.into(), &[]), None);
    }

    #[test]
    fn each_agent_has_a_wave_of_its_own() {
        assert_ne!(trace(300, &[], 1.0).shape(), trace(301, &[], 1.0).shape());
        assert_eq!(trace(300, &[], 1.0).shape(), trace(300, &[], 1.0).shape());
    }
}

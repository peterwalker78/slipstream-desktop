//! One variation drawn off screen, with no display and no GPU, for the Settings app's preview:
//! `slipstream --wallpaper-preview ID --size WxH [--still]`.
//!
//! Frames go to standard output as they would show on screen, each one `W × H` pixels of RGBA,
//! until whoever reads them stops. With `--still` there is one frame, the picture reduced motion
//! shows. A variation slower than 20 steps a second fades from one step to the next, as on the
//! desktop.
//!
//! The picture is composed on a grid laid out for a screen several times the size asked for, as
//! the saver lays one out for a real screen, so the logo and the cells have their proper
//! proportions; each frame is then shrunk to the size asked for by averaging blocks of pixels.

use std::io::Write;
use std::time::{Duration, Instant};

use super::{
    BACKGROUND, BLANK, Frame, GlyphCache, Grid, Layout, Look, Readings, TWEEN_LEVELS, epoch_now,
    layout, make, paint_cell, step_hz,
};

/// The width a preview is drawn at before it is shrunk: about a laptop screen's worth of cells,
/// small enough that each frame paints in a few milliseconds.
const DRAWN_WIDTH: usize = 1100;

/// How often a frame is sent while the picture is changing.
const FRAME: Duration = Duration::from_micros(33_333);

/// What to draw: which variation, how big, and whether only its still picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub size: (usize, usize),
    pub still: bool,
}

impl Request {
    /// Reads the arguments after `--wallpaper-preview`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut args = args.iter();
        let id = args.next().ok_or("which variation?")?.clone();
        let mut request = Request {
            id,
            size: (192, 120),
            still: false,
        };
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--still" => request.still = true,
                "--size" => {
                    let value = args.next().ok_or("--size needs a value")?;
                    let bad = || format!("--size {value} isn't understood");
                    let (w, h) = value.split_once('x').ok_or_else(bad)?;
                    request.size = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
                }
                _ => return Err(format!("{flag} isn't an option")),
            }
        }
        if request.size.0 < 16
            || request.size.1 < 10
            || request.size.0 > 3840
            || request.size.1 > 2160
        {
            return Err("the size must be between 16x10 and 3840x2160".into());
        }
        Ok(request)
    }
}

/// Sends the frames the request asks for to `out`. Returns once the still picture is sent, or
/// when `out` stops taking frames.
pub fn run(request: &Request, out: &mut impl Write) -> Result<(), String> {
    let Some(mut variation) = make(&request.id) else {
        return Err(format!("{} isn't a variation", request.id));
    };
    let (w, h) = request.size;
    let factor = DRAWN_WIDTH.div_ceil(w).max(1);
    let device = (w * factor, h * factor);
    let layout = layout(device.0, device.1).ok_or("that size is too small to lay out")?;

    let readings = local_readings();
    let offset = readings.utc_offset(epoch_now());
    let started = epoch_now();
    let mut canvas = Canvas::new(device, factor);
    let mut before = Grid::new(layout.cols, layout.rows);
    let mut latest = Grid::new(layout.cols, layout.rows);

    variation.reset(&layout, 1);
    if request.still {
        let frame = Frame {
            layout: &layout,
            elapsed: 0.0,
            dt: 0.0,
            still: true,
            readings: &readings,
            local: None,
        };
        compose(&mut *variation, &mut latest, frame);
        canvas.paint(&layout, &latest, &latest, 1.0);
        return send(out, &canvas.small);
    }

    let step = 1.0 / step_hz(variation.step_hz(), &layout);
    let fades = step > 1.0 / 20.0;
    // The preview opens on the moment the variation chooses, so it is run on to there unseen.
    let from = variation.preview_at();
    let mut stepped_to = 0.0;
    let mut step_to = |grid: &mut Grid, elapsed: f64, dt: f64| {
        let local = offset.map(|minutes| started + elapsed + minutes as f64 * 60.0);
        let moment = Frame {
            layout: &layout,
            elapsed,
            dt: dt as f32,
            still: false,
            readings: &readings,
            local,
        };
        compose(&mut *variation, grid, moment);
    };
    step_to(&mut latest, 0.0, 0.0);
    let clock = Instant::now();
    let mut sent: Option<(u64, u8)> = None;
    let mut steps = 0u64;
    let mut due = clock;
    loop {
        let now = from + clock.elapsed().as_secs_f64();
        while stepped_to + step <= now {
            std::mem::swap(&mut before, &mut latest);
            stepped_to += step;
            step_to(&mut latest, stepped_to, step);
            steps += 1;
        }
        let along = if fades {
            ((now - stepped_to) / step).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Nothing is sent while the picture hasn't changed.
        let key = (steps, (along * TWEEN_LEVELS as f64).round() as u8);
        if sent != Some(key) {
            canvas.paint(&layout, &before, &latest, along);
            if send(out, &canvas.small).is_err() {
                return Ok(());
            }
            sent = Some(key);
        }
        // Frames keep to their times however long each takes to paint, and one that runs late
        // doesn't make the next hurry.
        due += FRAME;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        } else {
            due = now;
        }
    }
}

fn send(out: &mut impl Write, pixels: &[u8]) -> Result<(), String> {
    out.write_all(pixels)
        .and_then(|()| out.flush())
        .map_err(|err| err.to_string())
}

fn compose(variation: &mut dyn super::Variation, grid: &mut Grid, frame: Frame) {
    grid.blank(frame.layout.cols, frame.layout.rows);
    variation.compose(frame, grid);
}

/// The time, the date and the first workspace, as the bar would give them now. No battery: a
/// preview has no business reading one.
fn local_readings() -> Readings {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // SAFETY: `time` and `localtime_r` write only into the values handed to them.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return Readings::default();
        }
        tm
    };
    Readings {
        time: format!("{:02}:{:02}", tm.tm_hour, tm.tm_min),
        date: format!(
            "{} {} {}",
            DAYS[tm.tm_wday.clamp(0, 6) as usize],
            tm.tm_mday,
            MONTHS[tm.tm_mon.clamp(0, 11) as usize]
        ),
        place: slipstream_config::workspace_label("", 0),
        battery: None,
    }
}

/// The pixels a grid is painted into, and the smaller picture they are shrunk to.
struct Canvas {
    device: (usize, usize),
    factor: usize,
    pixels: Vec<u8>,
    small: Vec<u8>,
    glyphs: GlyphCache,
    /// How each cell was last painted.
    shown: Vec<Look>,
}

impl Canvas {
    fn new(device: (usize, usize), factor: usize) -> Self {
        Self {
            device,
            factor,
            pixels: vec![0; device.0 * device.1 * 4],
            small: vec![0; (device.0 / factor) * (device.1 / factor) * 4],
            glyphs: GlyphCache::default(),
            shown: Vec::new(),
        }
    }

    /// Paints `along` the way from `before` to `latest`, as the saver does between steps, and
    /// shrinks it into `small`. Only the cells whose look changed since the last frame are
    /// painted again.
    fn paint(&mut self, layout: &Layout, before: &Grid, latest: &Grid, along: f64) {
        let (width, _) = self.device;
        let full = self.shown.len() != latest.cells.len();
        if full {
            for pixel in self.pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
            }
            self.shown = vec![(BLANK, BLANK, TWEEN_LEVELS); latest.cells.len()];
        }
        let level = (along * TWEEN_LEVELS as f64).round() as u8;
        for row in 0..layout.rows.min(latest.rows) {
            for col in 0..layout.cols.min(latest.cols) {
                let i = (row * latest.cols + col) as usize;
                let target = latest.cells[i];
                let start = before.cells.get(i).copied().unwrap_or(target);
                let look = if start == target || level >= TWEEN_LEVELS {
                    (target, target, TWEEN_LEVELS)
                } else if level == 0 {
                    (start, start, TWEEN_LEVELS)
                } else {
                    (start, target, level)
                };
                if look != self.shown[i] {
                    self.shown[i] = look;
                    paint_cell(
                        &mut self.pixels,
                        width,
                        layout,
                        &mut self.glyphs,
                        col,
                        row,
                        look,
                    );
                }
            }
        }
        let (w, h) = (self.device.0 / self.factor, self.device.1 / self.factor);
        shrink(&self.pixels, width, self.factor, &mut self.small, w, h);
    }
}

/// `pixels`, `width` wide, shrunk by `factor` into `small` (`w` × `h`), each pixel the average of
/// the block it stands for.
fn shrink(pixels: &[u8], width: usize, factor: usize, small: &mut [u8], w: usize, h: usize) {
    let area = (factor * factor) as u32;
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            for dy in 0..factor {
                let start = ((y * factor + dy) * width + x * factor) * 4;
                for pixel in pixels[start..start + factor * 4].chunks_exact(4) {
                    for (total, &channel) in sum.iter_mut().zip(pixel) {
                        *total += channel as u32;
                    }
                }
            }
            let at = (y * w + x) * 4;
            small[at..at + 4].copy_from_slice(&[
                (sum[0] / area) as u8,
                (sum[1] / area) as u8,
                (sum[2] / area) as u8,
                255,
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_arguments_are_read_and_checked() {
        let request = Request::parse(&args("tide --size 160x100 --still")).unwrap();
        assert_eq!(request.id, "tide");
        assert_eq!(request.size, (160, 100));
        assert!(request.still);
        assert!(!Request::parse(&args("tide")).unwrap().still);
        assert!(Request::parse(&args("tide --size 160")).is_err());
        assert!(Request::parse(&args("tide --size")).is_err());
        assert!(Request::parse(&args("tide --size 4x4")).is_err());
        assert!(Request::parse(&args("tide --frames 3")).is_err());
        assert!(Request::parse(&args("")).is_err());
    }

    #[test]
    fn a_block_shrinks_to_its_average() {
        let pixels = [
            0, 0, 0, 255, 100, 100, 100, 255, //
            200, 200, 200, 255, 100, 60, 20, 255,
        ];
        let mut small = [0u8; 4];
        shrink(&pixels, 2, 2, &mut small, 1, 1);
        assert_eq!(small, [100, 90, 80, 255]);
    }

    /// Takes `limit` bytes, then refuses more, as a reader that has gone away does.
    struct Taker {
        taken: Vec<u8>,
        limit: usize,
    }

    impl Write for Taker {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.taken.len() >= self.limit {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.taken.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn frames_are_sent_until_the_reader_goes_and_a_still_is_one_frame() {
        let frame = 64 * 40 * 4;
        let mut still = Taker {
            taken: Vec::new(),
            limit: usize::MAX,
        };
        let request = Request {
            id: "sonar".into(),
            size: (64, 40),
            still: true,
        };
        run(&request, &mut still).unwrap();
        assert_eq!(still.taken.len(), frame, "one still frame");

        let mut moving = Taker {
            taken: Vec::new(),
            limit: frame * 2,
        };
        let request = Request {
            still: false,
            ..request
        };
        run(&request, &mut moving).unwrap();
        assert_eq!(
            moving.taken.len(),
            frame * 2,
            "it stops when they're refused"
        );
        assert!(
            run(
                &Request {
                    id: "nope".into(),
                    ..request
                },
                &mut moving
            )
            .is_err()
        );
    }
}

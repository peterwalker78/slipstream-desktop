//! Frames of one variation drawn off screen, with no display and no GPU, for the Settings app's
//! previews: `slipstream --wallpaper-preview ID --size WxH --frames N --out DIR`.
//!
//! The picture is composed on a grid laid out for a screen several times the size asked for, as
//! the saver lays one out for a real screen, so the logo and the cells have their proper
//! proportions; each frame is then shrunk to the size asked for by averaging blocks of pixels.

use std::path::{Path, PathBuf};

use super::{
    BACKGROUND, BLANK, Frame, GlyphCache, Grid, Layout, Readings, TWEEN_LEVELS, epoch_now, layout,
    make, paint_cell, step_hz,
};

/// The width a preview is drawn at before it is shrunk: about a laptop screen's worth of cells,
/// small enough that each frame paints in a few milliseconds.
const DRAWN_WIDTH: usize = 1100;

/// Seconds between frames when the caller doesn't say.
const EVERY: f64 = 0.2;

/// What to draw: which variation, how big, how many frames from when, and where to put them.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub size: (usize, usize),
    pub frames: usize,
    /// Seconds into the variation of the first frame; the variation's own choice when `None`.
    pub from: Option<f64>,
    pub every: f64,
    pub out: PathBuf,
}

impl Request {
    /// Reads the arguments after `--wallpaper-preview`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut args = args.iter();
        let id = args.next().ok_or("which variation?")?.clone();
        let mut request = Request {
            id,
            size: (192, 120),
            frames: 16,
            from: None,
            every: EVERY,
            out: PathBuf::from("."),
        };
        while let Some(flag) = args.next() {
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            let bad = || format!("{flag} {value} isn't understood");
            match flag.as_str() {
                "--size" => {
                    let (w, h) = value.split_once('x').ok_or_else(bad)?;
                    request.size = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
                }
                "--frames" => request.frames = value.parse().map_err(|_| bad())?,
                "--from" => request.from = Some(value.parse().map_err(|_| bad())?),
                "--every" => request.every = value.parse().map_err(|_| bad())?,
                "--out" => request.out = PathBuf::from(value),
                _ => return Err(format!("{flag} isn't an option")),
            }
        }
        if request.size.0 < 16 || request.size.1 < 10 || request.size.0 > 3840 {
            return Err("the size must be between 16x10 and 3840 wide".into());
        }
        if request.frames > 240 || !(0.0..=10.0).contains(&request.every) {
            return Err("at most 240 frames, at most 10 s apart".into());
        }
        Ok(request)
    }
}

/// Draws the frames the request asks for, `frame-00.png` onwards, and `still.png`, the picture
/// reduced motion shows.
pub fn run(request: &Request) -> Result<(), String> {
    let Some(mut variation) = make(&request.id) else {
        return Err(format!("{} isn't a variation", request.id));
    };
    let (w, h) = request.size;
    let factor = DRAWN_WIDTH.div_ceil(w).max(1);
    let device = (w * factor, h * factor);
    let layout = layout(device.0, device.1).ok_or("that size is too small to lay out")?;
    std::fs::create_dir_all(&request.out).map_err(|err| err.to_string())?;

    let readings = local_readings();
    let offset = readings.utc_offset(epoch_now());
    let started = epoch_now();
    let mut canvas = Canvas::new(device, factor);
    let mut grid = Grid::new(layout.cols, layout.rows);

    // The still picture, from a variation of its own so it starts from nothing.
    if let Some(mut still) = make(&request.id) {
        still.reset(&layout, 1);
        compose(
            &mut *still,
            &layout,
            &mut grid,
            0.0,
            0.0,
            true,
            &readings,
            None,
        );
        canvas.save(&layout, &grid, &request.out.join("still.png"))?;
    }

    variation.reset(&layout, 1);
    let from = request.from.unwrap_or_else(|| variation.preview_at());
    let mut elapsed = 0.0;
    let mut dt = 0.0;
    let mut frame = 0;
    while frame < request.frames {
        let local = offset.map(|minutes| started + elapsed + minutes as f64 * 60.0);
        compose(
            &mut *variation,
            &layout,
            &mut grid,
            elapsed,
            dt as f32,
            false,
            &readings,
            local,
        );
        let step = 1.0 / step_hz(variation.step_hz(), &layout);
        // Every frame due before the next step shows this one, as the screen would.
        while frame < request.frames && from + frame as f64 * request.every < elapsed + step {
            canvas.save(
                &layout,
                &grid,
                &request.out.join(format!("frame-{frame:02}.png")),
            )?;
            frame += 1;
        }
        elapsed += step;
        dt = step;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compose(
    variation: &mut dyn super::Variation,
    layout: &Layout,
    grid: &mut Grid,
    elapsed: f64,
    dt: f32,
    still: bool,
    readings: &Readings,
    local: Option<f64>,
) {
    grid.blank(layout.cols, layout.rows);
    variation.compose(
        Frame {
            layout,
            elapsed,
            dt,
            still,
            readings,
            local,
        },
        grid,
    );
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
}

impl Canvas {
    fn new(device: (usize, usize), factor: usize) -> Self {
        Self {
            device,
            factor,
            pixels: vec![0; device.0 * device.1 * 4],
            small: vec![0; (device.0 / factor) * (device.1 / factor) * 4],
            glyphs: GlyphCache::default(),
        }
    }

    fn save(&mut self, layout: &Layout, grid: &Grid, path: &Path) -> Result<(), String> {
        let (width, _) = self.device;
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
        }
        for row in 0..layout.rows.min(grid.rows) {
            for col in 0..layout.cols.min(grid.cols) {
                let cell = grid.cells[(row * grid.cols + col) as usize];
                if cell != BLANK {
                    paint_cell(
                        &mut self.pixels,
                        width,
                        layout,
                        &mut self.glyphs,
                        col,
                        row,
                        (cell, cell, TWEEN_LEVELS),
                    );
                }
            }
        }
        let (w, h) = (self.device.0 / self.factor, self.device.1 / self.factor);
        shrink(&self.pixels, width, self.factor, &mut self.small, w, h);
        let file =
            std::fs::File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
        crate::render::encode_png(
            std::io::BufWriter::new(file),
            &self.small,
            w as u32,
            h as u32,
        )
        .map_err(|err| err.to_string())
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
        let request = Request::parse(&args(
            "tide --size 160x100 --frames 12 --out /x/y --every 0.25",
        ))
        .unwrap();
        assert_eq!(request.id, "tide");
        assert_eq!(request.size, (160, 100));
        assert_eq!(request.frames, 12);
        assert_eq!(request.every, 0.25);
        assert_eq!(request.out, PathBuf::from("/x/y"));
        assert!(Request::parse(&args("tide --size 160")).is_err());
        assert!(Request::parse(&args("tide --frames")).is_err());
        assert!(Request::parse(&args("tide --size 4x4")).is_err());
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

    #[test]
    fn frames_and_a_still_are_written_for_a_variation() {
        let out = crate::files::test_scratch("wallpaper-preview");
        let request = Request {
            id: "sonar".into(),
            size: (64, 40),
            frames: 3,
            from: Some(5.0),
            every: 0.5,
            out: out.clone(),
        };
        run(&request).unwrap();
        for name in ["still.png", "frame-00.png", "frame-02.png"] {
            let data = std::fs::read(out.join(name)).unwrap();
            assert_eq!(&data[1..4], b"PNG", "{name}");
        }
        let _ = std::fs::remove_dir_all(&out);
        assert!(
            run(&Request {
                id: "nope".into(),
                ..request
            })
            .is_err()
        );
    }
}

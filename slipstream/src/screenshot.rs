//! Print: the focused screen saved as a PNG and put on the clipboard. Super+Shift+S's snips
//! (`snip.rs`) are saved the same way.
//!
//! A request names the screen it's for, and only that screen's frame serves it. The pixels are
//! copied just after the frame is drawn, on the render path; cropping, encoding and writing the
//! file happen on a thread of their own, which hands the PNG back to the event loop. The
//! compositor then offers it as the clipboard itself, rather than through `wl-copy`, which would
//! need a window of its own on any compositor without data control.

use std::{
    fs,
    io::{self, Write},
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::Arc,
};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    output::Output,
    utils::{Logical, Physical, Rectangle, Size},
    wayland::selection::{SelectionTarget, data_device::set_data_device_selection},
};

use crate::{
    Slipstream,
    render::{self, OutputElement},
};

/// The clipboard type a screenshot is offered as.
pub const PNG: &str = "image/png";
/// The white flash when the picture is taken: its opacity at first, and how long it fades for.
pub const FLASH_ALPHA: f32 = 0.25;
pub const FLASH: f64 = 0.12;

/// A screenshot waiting for its screen's next frame.
pub struct Request {
    /// The output whose frame serves it.
    pub output: String,
    /// The part of the screen to keep, in logical pixels from its corner; the whole screen when
    /// `None`.
    pub crop: Option<Rectangle<f64, Logical>>,
}

/// A screenshot's outcome, back on the event loop.
pub enum Saved {
    Done { folder: PathBuf, png: Arc<Vec<u8>> },
    Failed(String),
}

/// What Smithay keeps with a selection the compositor offers: one an X11 app made, passed on to
/// Wayland apps, or a screenshot's PNG.
#[derive(Debug, Clone)]
pub enum Selection {
    X11,
    Image(Arc<Vec<u8>>),
    /// Text pasted back from the clipboard history.
    Text(Arc<str>),
}

/// `Screenshot_YYYY-MM-DD_HH-MM-SS.png`, from a local date and time.
pub fn file_name([year, month, day, hour, minute, second]: [i32; 6]) -> String {
    format!("Screenshot_{year:04}-{month:02}-{day:02}_{hour:02}-{minute:02}-{second:02}.png")
}

/// The local date and time now, as `file_name` takes it.
fn local_now() -> [i32; 6] {
    // SAFETY: `time` and `localtime_r` write only into the values handed to them.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return [1970, 1, 1, 0, 0, 0];
        }
        [
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        ]
    }
}

/// The Screenshots folder inside the pictures folder `user-dirs.dirs` names (`user_dirs`, the
/// file's text), or inside `~/Pictures` when it names none.
pub fn folder_in(user_dirs: Option<&str>, home: &Path) -> PathBuf {
    let pictures = user_dirs.and_then(|text| {
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix("XDG_PICTURES_DIR=")?;
            let value = value.trim().trim_matches('"');
            let path = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None if value.starts_with('/') => PathBuf::from(value),
                None => return None,
            };
            // A pictures folder set to the home folder itself would scatter screenshots there.
            (path != home).then_some(path)
        })
    });
    pictures
        .unwrap_or_else(|| home.join("Pictures"))
        .join("Screenshots")
}

/// Where screenshots go: `SLIPSTREAM_SCREENSHOT_DIR` when set, otherwise the pictures folder's
/// Screenshots.
fn folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("SLIPSTREAM_SCREENSHOT_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let user_dirs = crate::files::read_small(&config.join("user-dirs.dirs"), 64 << 10)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    folder_in(user_dirs.as_deref(), &home)
}

/// The part of an RGBA picture `size` big inside `crop`, in its own pixels, clipped to the
/// picture. `None` if nothing of it is inside.
pub(crate) fn cropped(
    pixels: &[u8],
    size: Size<i32, Physical>,
    crop: Rectangle<i32, Physical>,
) -> Option<(Vec<u8>, u32, u32)> {
    let x0 = crop.loc.x.clamp(0, size.w) as usize;
    let y0 = crop.loc.y.clamp(0, size.h) as usize;
    let x1 = (crop.loc.x.saturating_add(crop.size.w)).clamp(0, size.w) as usize;
    let y1 = (crop.loc.y.saturating_add(crop.size.h)).clamp(0, size.h) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let width = size.w as usize;
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for row in y0..y1 {
        out.extend_from_slice(&pixels[(row * width + x0) * 4..(row * width + x1) * 4]);
    }
    Some((out, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// A logical rectangle on a screen at `scale`, in the screen's pixels.
pub(crate) fn to_pixels(rect: Rectangle<f64, Logical>, scale: f64) -> Rectangle<i32, Physical> {
    let x0 = (rect.loc.x * scale).round() as i32;
    let y0 = (rect.loc.y * scale).round() as i32;
    let x1 = ((rect.loc.x + rect.size.w) * scale).round() as i32;
    let y1 = ((rect.loc.y + rect.size.h) * scale).round() as i32;
    Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
}

/// Crops, encodes and writes one screenshot, never over an earlier one taken the same second.
fn save(
    pixels: &[u8],
    size: Size<i32, Physical>,
    crop: Option<Rectangle<i32, Physical>>,
    folder: &Path,
    name: &str,
) -> Result<Vec<u8>, String> {
    let (picture, w, h) = match crop {
        Some(crop) => cropped(pixels, size, crop).ok_or("the window isn't on the screen")?,
        None => (pixels.to_vec(), size.w as u32, size.h as u32),
    };
    let mut png = Vec::new();
    render::encode_png(&mut png, &picture, w, h).map_err(|err| err.to_string())?;
    fs::create_dir_all(folder).map_err(|err| err.to_string())?;
    let stem = name.trim_end_matches(".png");
    for n in 1..100 {
        let file = if n == 1 {
            folder.join(name)
        } else {
            folder.join(format!("{stem}_{n}.png"))
        };
        match fs::File::options().write(true).create_new(true).open(&file) {
            Ok(mut out) => {
                out.write_all(&png).map_err(|err| err.to_string())?;
                tracing::info!(path = %file.display(), "screenshot saved");
                return Ok(png);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("too many screenshots in one second".into())
}

/// Hands a screenshot's PNG to a client pasting it, on a thread of its own so a slow reader can't
/// hold up the event loop.
pub fn send_image(png: Arc<Vec<u8>>, fd: OwnedFd) {
    send_bytes(move || png.as_slice().to_vec(), fd);
}

/// Hands a clip's bytes to a client pasting it, on a thread of its own.
pub fn send_bytes(bytes: impl FnOnce() -> Vec<u8> + Send + 'static, fd: OwnedFd) {
    std::thread::spawn(move || {
        // Written in one go: a reader's pipe may have come non-blocking.
        // SAFETY: the flags of a descriptor this thread owns are read and set, nothing else.
        unsafe {
            let raw = fd.as_raw_fd();
            let flags = libc::fcntl(raw, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(raw, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
        if let Err(err) = fs::File::from(fd).write_all(&bytes()) {
            tracing::debug!("a paste stopped reading the screenshot: {err}");
        }
    });
}

impl Slipstream {
    /// Print (`window` false) or Super+Shift+S (`window` true): asks the focused screen's next
    /// frame for a picture, cropped to the focused window for the second.
    pub fn take_screenshot(&mut self, window: bool) {
        // Nothing is pictured while locked.
        if self.lock.is_some() {
            return;
        }
        let Some(output) = self.screens.focused_output() else {
            return;
        };
        // Super+Shift+S snips: a region, a window or the whole screen, chosen on a frozen frame.
        if window {
            self.start_snip();
            return;
        }
        let crop = None;
        tracing::info!(window, "screenshot asked for");
        self.screenshot_requests.push(Request {
            output: output.name(),
            crop,
        });
    }

    /// Serves the screenshots waiting on `output` from the frame it has just drawn, from
    /// `elements`, and starts the flash there.
    pub fn serve_screenshots(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        elements: &[OutputElement],
        size: Size<i32, Physical>,
        scale: f64,
    ) {
        let name = output.name();
        if self.lock.is_some() {
            self.screenshot_requests.clear();
            return;
        }
        self.serve_snip(renderer, output, elements, size, scale);
        if !self
            .screenshot_requests
            .iter()
            .any(|request| request.output == name)
        {
            return;
        }
        let (mine, others): (Vec<Request>, Vec<Request>) =
            std::mem::take(&mut self.screenshot_requests)
                .into_iter()
                .partition(|request| request.output == name);
        self.screenshot_requests = others;
        let pixels = match render::copy_frame(renderer, elements, size, scale) {
            Ok(pixels) => Arc::new(pixels),
            Err(err) => {
                tracing::warn!(output = name, "screenshot failed: {err}");
                self.show_toast("Screenshot not saved", "The screen couldn’t be read back.");
                return;
            }
        };
        // After the copy, so the flash is never in the picture.
        if !self.clock.reduced_motion {
            self.flash = Some((name, self.wall()));
        }
        for request in mine {
            self.save_screenshot(pixels.clone(), size, request.crop, scale);
        }
    }

    /// Crops, encodes and writes a screenshot on a thread of its own, from a frame's `pixels`
    /// (`size` of them, at `scale`); `crop` is in logical pixels from the screen's corner. The
    /// outcome comes back to `screenshot_saved`.
    pub fn save_screenshot(
        &self,
        pixels: Arc<Vec<u8>>,
        size: Size<i32, Physical>,
        crop: Option<Rectangle<f64, Logical>>,
        scale: f64,
    ) {
        let crop = crop.map(|rect| to_pixels(rect, scale));
        let file = file_name(local_now());
        let answers = self.screenshot_answers.clone();
        std::thread::spawn(move || {
            // Read here, off the event loop: `user-dirs.dirs` may be on a slow home folder.
            let folder = folder();
            let saved = match save(&pixels, size, crop, &folder, &file) {
                Ok(png) => Saved::Done {
                    folder,
                    png: Arc::new(png),
                },
                Err(err) => Saved::Failed(err),
            };
            let _ = answers.send(saved);
        });
    }

    /// The flash's opacity on `output` now, while there is one. None for a frame that will serve
    /// a screenshot, so a second picture taken while the first one's flash fades never has it in:
    /// the flash skips that frame, and starts again once the picture is copied.
    pub fn flash_alpha(&mut self, output: &str) -> Option<f32> {
        if self
            .screenshot_requests
            .iter()
            .any(|request| request.output == output)
        {
            return None;
        }
        let (on, since) = self.flash.as_ref()?;
        let age = self.wall() - since;
        if age >= FLASH {
            self.flash = None;
            return None;
        }
        (on == output).then(|| FLASH_ALPHA * (1.0 - (age / FLASH) as f32))
    }

    /// A screenshot's file is written: its PNG becomes the clipboard, and a toast says where it
    /// went.
    pub fn screenshot_saved(&mut self, saved: Saved) {
        let (folder, png) = match saved {
            Saved::Done { folder, png } => (folder, png),
            Saved::Failed(err) => {
                tracing::warn!("screenshot not saved: {err}");
                self.show_toast("Screenshot not saved", &err);
                return;
            }
        };
        // One taken just before the lock came up keeps its file, but the clipboard and the
        // toast wait for nobody: nothing changes behind the lock.
        if self.lock.is_some() {
            tracing::info!("screenshot saved while locked; not copied");
            return;
        }
        set_data_device_selection(
            &self.display_handle,
            &self.seat,
            vec![PNG.to_string()],
            Selection::Image(png.clone()),
        );
        if self.settings.clipboard.history {
            let (png, answers) = (png.clone(), self.clip_answers.clone());
            std::thread::spawn(move || {
                if let Some(clip) = crate::history::image_clip(png) {
                    let _ = answers.send(clip);
                }
            });
        }
        // X11 apps are told too; their request comes back through the window manager.
        if let Some(xwm) = self.xwm.as_mut()
            && let Err(err) = xwm.new_selection(SelectionTarget::Clipboard, Some(vec![PNG.into()]))
        {
            tracing::warn!(?err, "couldn't offer the screenshot to X11 apps");
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let shown = folder
            .strip_prefix(&home)
            .ok()
            .filter(|inside| !home.as_os_str().is_empty() && !inside.as_os_str().is_empty())
            .map_or_else(
                || folder.display().to_string(),
                |inside| inside.display().to_string(),
            );
        self.show_toast(
            "Screenshot saved",
            &format!("{shown} · copied to the clipboard"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_dated() {
        assert_eq!(
            file_name([2026, 3, 7, 9, 5, 2]),
            "Screenshot_2026-03-07_09-05-02.png"
        );
    }

    #[test]
    fn the_folder_comes_from_user_dirs() {
        let home = Path::new("/home/alex");
        let dirs = "# written by xdg-user-dirs-update\n\
                    XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n\
                    XDG_PICTURES_DIR=\"$HOME/Bilder\"\n";
        assert_eq!(
            folder_in(Some(dirs), home),
            Path::new("/home/alex/Bilder/Screenshots")
        );
        assert_eq!(
            folder_in(Some("XDG_PICTURES_DIR=\"/srv/photos\"\n"), home),
            Path::new("/srv/photos/Screenshots")
        );
        assert_eq!(
            folder_in(Some("XDG_PICTURES_DIR=\"$HOME/\"\n"), home),
            Path::new("/home/alex/Pictures/Screenshots"),
            "not the home folder itself"
        );
        assert_eq!(
            folder_in(None, home),
            Path::new("/home/alex/Pictures/Screenshots")
        );
    }

    #[test]
    fn a_window_is_cut_from_its_screen_and_clipped_to_it() {
        let size = Size::from((4, 3));
        let pixels: Vec<u8> = (0..12u8).flat_map(|i| [i, i, i, 255]).collect();
        let (out, w, h) =
            cropped(&pixels, size, Rectangle::new((1, 1).into(), (2, 2).into())).expect("inside");
        assert_eq!((w, h), (2, 2));
        assert_eq!(
            out.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [5, 6, 9, 10]
        );
        let (_, w, h) = cropped(&pixels, size, Rectangle::new((3, -1).into(), (5, 2).into()))
            .expect("partly inside");
        assert_eq!((w, h), (1, 1));
        assert!(cropped(&pixels, size, Rectangle::new((9, 9).into(), (2, 2).into())).is_none());
        let rect = Rectangle::new((16.0, 42.0).into(), (100.0, 50.0).into());
        assert_eq!(
            to_pixels(rect, 1.25),
            Rectangle::new((20, 53).into(), (125, 62).into())
        );
    }
}

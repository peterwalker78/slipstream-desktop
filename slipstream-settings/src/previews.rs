//! Small moving previews of the living wallpaper's variations.
//!
//! The wallpaper is drawn on the CPU by the compositor, so the compositor draws the previews too:
//! `slipstream --wallpaper-preview ID` composes a variation off screen, with no display and no
//! GPU, and writes a few small frames. They are made once for each build of the compositor, on a
//! thread of their own, and kept in the cache folder; the page's one preview turns over the
//! frames of the variation it shows, or holds its still picture when motion is reduced.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use gtk::{gdk, gio, glib, prelude::*};
use gtk4 as gtk;

/// The compositor's program, which sits beside this app when installed.
const COMPOSITOR: &str = "slipstream";

/// A preview's size in logical pixels, the size its frames are drawn at (a little larger, for
/// screens scaled above 1), how many frames it has, and the seconds between them.
pub const SHOWN: (i32, i32) = (256, 160);
const DRAWN: (u32, u32) = (320, 200);
const FRAMES: usize = 16;
const EVERY: f64 = 0.2;

thread_local! {
    /// A screenshot is being taken: previews are put in place at once and hold still.
    static HELD: Cell<bool> = const { Cell::new(false) };
}

/// Makes previews for a screenshot: in place before the window draws, and still.
pub fn hold() {
    HELD.with(|held| held.set(true));
}

/// The compositor installed beside this app. Only that one: a compositor from somewhere else on
/// `PATH` might be an older build that doesn't know how to draw a preview, and would start a whole
/// session instead.
pub fn compositor() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(COMPOSITOR))
        .filter(|path| path.is_file())
}

/// Where previews are kept: `slipstream/wallpaper-previews` in the cache folder.
fn cache_root() -> PathBuf {
    slipstream_config::xdg_home("XDG_CACHE_HOME", ".cache")
        .join("slipstream")
        .join("wallpaper-previews")
}

/// A name for this build of the compositor, from its size and when it was written, so previews
/// are made again whenever it changes.
fn build_name(compositor: &Path) -> Option<String> {
    let meta = std::fs::metadata(compositor).ok()?;
    let written = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Some(format!("{:x}-{:x}", meta.len(), written.as_secs()))
}

/// The frames of one variation, loaded.
pub struct Frames {
    pub still: gdk::Texture,
    pub moving: Vec<gdk::Texture>,
}

/// Whether a folder holds a finished preview.
fn complete(dir: &Path) -> bool {
    dir.join("still.png").is_file() && dir.join(format!("frame-{:02}.png", FRAMES - 1)).is_file()
}

/// Makes the preview of `id` in `dir` if it isn't there yet, and loads it. Blocks: run it off the
/// main thread.
fn make(compositor: &Path, id: &str, dir: &Path) -> Result<Frames, String> {
    if !complete(dir) {
        // Drawn into a folder of its own and moved into place, so a preview half made by an app
        // that was closed meanwhile is never taken for a finished one.
        let partial = dir.with_extension(format!("partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&partial);
        let status = Command::new(compositor)
            .arg("--wallpaper-preview")
            .arg(id)
            .args(["--size", &format!("{}x{}", DRAWN.0, DRAWN.1)])
            .args(["--frames", &FRAMES.to_string()])
            .args(["--every", &EVERY.to_string()])
            .arg("--out")
            .arg(&partial)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| err.to_string())?;
        if !status.success() || !complete(&partial) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(format!("the compositor couldn't draw {id}"));
        }
        let _ = std::fs::remove_dir_all(dir);
        std::fs::rename(&partial, dir).map_err(|err| err.to_string())?;
    }
    let load =
        |name: &str| gdk::Texture::from_filename(dir.join(name)).map_err(|err| err.to_string());
    Ok(Frames {
        still: load("still.png")?,
        moving: (0..FRAMES)
            .map(|frame| load(&format!("frame-{frame:02}.png")))
            .collect::<Result<_, _>>()?,
    })
}

/// The picture a preview is shown in, waiting for its frames.
pub fn picture() -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .width_request(SHOWN.0)
        .height_request(SHOWN.1)
        .valign(gtk::Align::Center)
        .css_classes(["preview"])
        .build();
    picture.set_overflow(gtk::Overflow::Hidden);
    picture
}

/// The frames made so far, each beside its variation's id.
type Made = Rc<RefCell<Vec<(&'static str, Frames)>>>;

/// One picture for many variations: it shows whichever it was last asked to, turning over while
/// it is on screen, or holding still when motion is reduced.
pub struct Preview {
    picture: gtk::Picture,
    showing: Rc<Cell<&'static str>>,
    made: Made,
    reduced: bool,
}

impl Preview {
    /// Makes the previews of `ids` for `picture`, one at a time off the main thread, starting
    /// with whichever it is showing, and shows `first`.
    pub fn new(
        picture: &gtk::Picture,
        ids: Vec<&'static str>,
        first: &'static str,
        reduced: bool,
    ) -> Preview {
        let preview = Preview {
            picture: picture.clone(),
            showing: Rc::new(Cell::new(first)),
            made: Rc::default(),
            reduced,
        };
        let Some(compositor) = compositor() else {
            return preview;
        };
        let Some(build) = build_name(&compositor) else {
            return preview;
        };
        let root = cache_root();
        let folder = root.join(&build);
        if HELD.with(Cell::get) {
            // A screenshot: the picture in place before the window first draws, since a picture
            // that changes afterwards can't be drawn into one. Waiting here is fine for that.
            forget_other_builds(&root, &build);
            if let Ok(frames) = make(&compositor, first, &folder.join(first)) {
                preview.made.borrow_mut().push((first, frames));
            }
            preview.show(first);
            return preview;
        }
        if !reduced {
            animate(&preview);
        }
        let (picture, showing, made) = (
            picture.downgrade(),
            Rc::clone(&preview.showing),
            Rc::clone(&preview.made),
        );
        glib::spawn_future_local(async move {
            // Previews of earlier builds are no use any more.
            let _ = gio::spawn_blocking({
                let (root, build) = (root.clone(), build.clone());
                move || forget_other_builds(&root, &build)
            })
            .await;
            let mut left = ids;
            while !left.is_empty() && picture.upgrade().is_some() {
                let at = left.iter().position(|id| *id == showing.get()).unwrap_or(0);
                let id = left.remove(at);
                let (compositor, dir) = (compositor.clone(), folder.join(id));
                let made_now = gio::spawn_blocking(move || make(&compositor, id, &dir)).await;
                let Ok(Ok(frames)) = made_now else {
                    continue;
                };
                if let Some(picture) = picture.upgrade()
                    && showing.get() == id
                {
                    picture.set_paintable(Some(&frames.still));
                }
                made.borrow_mut().push((id, frames));
            }
        });
        preview
    }

    /// Shows `id`'s preview, or an empty picture until it is made.
    pub fn show(&self, id: &'static str) {
        self.showing.set(id);
        let made = self.made.borrow();
        match made.iter().find(|(made, _)| *made == id) {
            Some((_, frames)) => match frames.moving.get(FRAMES / 2) {
                Some(frame) if !self.reduced => self.picture.set_paintable(Some(frame)),
                _ => self.picture.set_paintable(Some(&frames.still)),
            },
            None => self.picture.set_paintable(None::<&gdk::Paintable>),
        }
    }
}

/// Turns the preview over to its next frame while it is on screen, until the page is gone.
fn animate(preview: &Preview) {
    let (picture, showing, made) = (
        preview.picture.downgrade(),
        Rc::clone(&preview.showing),
        Rc::clone(&preview.made),
    );
    let mut frame = 0usize;
    glib::timeout_add_local(Duration::from_secs_f64(EVERY), move || {
        let Some(picture) = picture.upgrade() else {
            return glib::ControlFlow::Break;
        };
        frame = frame.wrapping_add(1);
        let made = made.borrow();
        if let Some((_, frames)) = made.iter().find(|(id, _)| *id == showing.get())
            && !frames.moving.is_empty()
            && picture.is_mapped()
        {
            picture.set_paintable(Some(&frames.moving[frame % frames.moving.len()]));
        }
        glib::ControlFlow::Continue
    });
}

/// Removes the previews of every build but `build`.
fn forget_other_builds(root: &Path, build: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != build {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/test-scratch")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_build_is_named_by_its_size_and_time() {
        let dir = scratch("preview-build");
        let binary = dir.join("slipstream");
        std::fs::write(&binary, b"one").unwrap();
        let first = build_name(&binary).unwrap();
        assert!(first.starts_with("3-"), "{first}");
        std::fs::write(&binary, b"longer").unwrap();
        assert_ne!(
            build_name(&binary).unwrap(),
            first,
            "a new build, a new name"
        );
        assert!(build_name(&dir.join("missing")).is_none());
    }

    #[test]
    fn only_the_current_build_s_previews_are_kept() {
        let root = scratch("preview-cache");
        for build in ["old-1", "new-2"] {
            std::fs::create_dir_all(root.join(build).join("tide")).unwrap();
        }
        forget_other_builds(&root, "new-2");
        assert!(!root.join("old-1").exists());
        assert!(root.join("new-2/tide").exists());
    }

    #[test]
    fn a_half_made_preview_is_not_complete() {
        let dir = scratch("preview-complete");
        std::fs::write(dir.join("still.png"), b"").unwrap();
        assert!(!complete(&dir));
        std::fs::write(dir.join(format!("frame-{:02}.png", FRAMES - 1)), b"").unwrap();
        assert!(complete(&dir));
    }
}

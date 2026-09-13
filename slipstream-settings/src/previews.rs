//! Small moving previews of the living wallpaper's variations.
//!
//! The wallpaper is drawn on the CPU by the compositor, so the compositor draws the previews too:
//! `slipstream --wallpaper-preview ID` composes a variation off screen, with no display and no
//! GPU, and writes a few small frames. They are made once for each build of the compositor, on a
//! thread of their own, and kept in the cache folder; the page shows each row's frames in turn,
//! or only the still picture when motion is reduced.

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
pub const SHOWN: (i32, i32) = (160, 100);
const DRAWN: (u32, u32) = (200, 125);
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
fn compositor() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(COMPOSITOR))
        .filter(|path| path.is_file())
}

/// Where previews are kept: `slipstream/wallpaper-previews` in the cache folder.
fn cache_root() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    cache.join("slipstream").join("wallpaper-previews")
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

/// A preview's place in its row, waiting for its frames.
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

/// Fills `pictures` (a variation id and its picture each) with their previews as they are made,
/// one at a time off the main thread, and keeps the moving ones turning over while they are on
/// screen. With `reduced` motion each shows its still picture instead.
pub fn fill(pictures: Vec<(&'static str, gtk::Picture)>, reduced: bool) {
    let Some(compositor) = compositor() else {
        return;
    };
    let Some(build) = build_name(&compositor) else {
        return;
    };
    let root = cache_root();
    let folder = root.join(&build);
    if HELD.with(Cell::get) {
        // A screenshot: every preview in place before the window first draws, since a picture
        // that changes afterwards can't be drawn into one. Waiting here is fine for that.
        forget_other_builds(&root, &build);
        for (id, picture) in &pictures {
            if let Ok(frames) = make(&compositor, id, &folder.join(id)) {
                match frames.moving.get(FRAMES / 2) {
                    Some(frame) if !reduced => picture.set_paintable(Some(frame)),
                    _ => picture.set_paintable(Some(&frames.still)),
                }
            }
        }
        return;
    }
    // Each picture's frames once they are loaded. Held weakly, so the page can go and take its
    // previews with it.
    let shown: Shown = Rc::new(
        pictures
            .iter()
            .map(|(_, picture)| (picture.downgrade(), RefCell::new(Vec::new())))
            .collect(),
    );
    if !reduced {
        animate(Rc::clone(&shown));
    }
    glib::spawn_future_local(async move {
        // Previews of earlier builds are no use any more.
        let _ = gio::spawn_blocking({
            let (root, build) = (root.clone(), build.clone());
            move || forget_other_builds(&root, &build)
        })
        .await;
        for (index, (id, picture)) in pictures.into_iter().enumerate() {
            let (compositor, dir) = (compositor.clone(), folder.join(id));
            let made = gio::spawn_blocking(move || make(&compositor, id, &dir)).await;
            let Ok(Ok(frames)) = made else {
                continue;
            };
            picture.set_paintable(Some(&frames.still));
            if !reduced {
                *shown[index].1.borrow_mut() = frames.moving;
            }
        }
    });
}

/// The pictures being animated, and the frames each turns through.
type Shown = Rc<Vec<(glib::WeakRef<gtk::Picture>, RefCell<Vec<gdk::Texture>>)>>;

/// Turns every preview over to its next frame while it is on screen, until the page is gone.
fn animate(shown: Shown) {
    let mut frame = 0usize;
    glib::timeout_add_local(Duration::from_secs_f64(EVERY), move || {
        let mut alive = false;
        frame = frame.wrapping_add(1);
        for (picture, frames) in shown.iter() {
            let Some(picture) = picture.upgrade() else {
                continue;
            };
            alive = true;
            let frames = frames.borrow();
            if !frames.is_empty() && picture.is_mapped() {
                picture.set_paintable(Some(&frames[frame % frames.len()]));
            }
        }
        if alive {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
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

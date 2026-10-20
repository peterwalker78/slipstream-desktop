//! The living wallpaper's preview.
//!
//! The wallpaper is drawn on the CPU by the compositor, so the compositor draws the preview too:
//! `slipstream --wallpaper-preview ID --size WxH` composes a variation off screen, with no display
//! and no GPU, and sends its frames as the desktop would show them. The page shows each as it
//! arrives while the preview is on screen, and stops the drawing when it isn't. With reduced
//! motion it asks for the still picture alone.

use std::{
    cell::{Cell, RefCell},
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex},
};

use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;

/// The compositor's program, which sits beside this app when installed.
const COMPOSITOR: &str = "slipstream";

/// The preview's size in logical pixels, and the size its frames are drawn at: one and a half
/// times as big, so it stays sharp on a screen scaled up.
pub const SHOWN: (i32, i32) = (480, 300);
const DRAWN: (usize, usize) = (720, 450);

thread_local! {
    /// A screenshot is being taken: the preview is put in place at once.
    static HELD: Cell<bool> = const { Cell::new(false) };
}

/// Makes the preview ready for a screenshot: in place before the window draws.
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

/// The picture the preview is shown in.
pub fn picture() -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .width_request(SHOWN.0)
        .height_request(SHOWN.1)
        .css_classes(["preview"])
        .build();
    picture.set_overflow(gtk::Overflow::Hidden);
    picture
}

/// One variation being drawn by the compositor, and the latest frame it has sent.
struct Drawing {
    child: Child,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
}

impl Drawing {
    /// Starts the compositor drawing `id`: its still picture alone when `still`.
    fn start(id: &str, still: bool) -> Option<Drawing> {
        let mut command = Command::new(compositor()?);
        command
            .args(["--wallpaper-preview", id])
            .args(["--size", &format!("{}x{}", DRAWN.0, DRAWN.1)]);
        if still {
            command.arg("--still");
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut frames = child.stdout.take()?;
        let latest: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
        let into = Arc::clone(&latest);
        std::thread::spawn(move || {
            loop {
                let mut frame = vec![0; DRAWN.0 * DRAWN.1 * 4];
                if frames.read_exact(&mut frame).is_err() {
                    break;
                }
                if let Ok(mut latest) = into.lock() {
                    *latest = Some(frame);
                }
            }
        });
        Some(Drawing { child, latest })
    }

    /// The frame sent since the last one taken, if there is one.
    fn take(&self) -> Option<gdk::MemoryTexture> {
        let frame = self.latest.lock().ok()?.take()?;
        Some(gdk::MemoryTexture::new(
            DRAWN.0 as i32,
            DRAWN.1 as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(frame),
            DRAWN.0 * 4,
        ))
    }
}

impl Drop for Drawing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Shared {
    picture: glib::WeakRef<gtk::Picture>,
    showing: Cell<Option<&'static str>>,
    drawing: RefCell<Option<Drawing>>,
    reduced: bool,
}

impl Shared {
    /// Draws the variation being shown, in place of whatever was drawing before.
    fn restart(&self) {
        self.drawing.replace(None);
        if let Some(id) = self.showing.get() {
            self.drawing.replace(Drawing::start(id, self.reduced));
        }
    }
}

/// The one preview on the Wallpaper page: it shows whichever variation it was last asked to, as
/// it moves on the desktop, or still when motion is reduced.
pub struct Preview {
    shared: Rc<Shared>,
}

impl Preview {
    pub fn new(picture: &gtk::Picture, reduced: bool) -> Preview {
        let shared = Rc::new(Shared {
            picture: picture.downgrade(),
            showing: Cell::new(None),
            drawing: RefCell::new(None),
            reduced,
        });
        {
            let shared = Rc::clone(&shared);
            picture.connect_map(move |_| shared.restart());
        }
        {
            let shared = Rc::clone(&shared);
            picture.connect_unmap(move |_| {
                shared.drawing.replace(None);
            });
        }
        {
            let shared = Rc::clone(&shared);
            picture.add_tick_callback(move |picture, _| {
                if let Some(frame) = shared.drawing.borrow().as_ref().and_then(Drawing::take) {
                    picture.set_paintable(Some(&frame));
                }
                glib::ControlFlow::Continue
            });
        }
        Preview { shared }
    }

    /// Shows `id`.
    pub fn show(&self, id: &'static str) {
        if self.shared.showing.replace(Some(id)) == Some(id) {
            return;
        }
        let Some(picture) = self.shared.picture.upgrade() else {
            return;
        };
        picture.set_paintable(None::<&gdk::Paintable>);
        if HELD.with(Cell::get) {
            // A screenshot: the first frame in place before the window first draws, since a
            // picture that changes afterwards can't be drawn into one. Waiting here is fine.
            let Some(drawing) = Drawing::start(id, self.shared.reduced) else {
                return;
            };
            for _ in 0..200 {
                if let Some(frame) = drawing.take() {
                    picture.set_paintable(Some(&frame));
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        } else if picture.is_mapped() {
            self.shared.restart();
        }
    }
}

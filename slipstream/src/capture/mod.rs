//! Screen sharing: the compositor half of it.
//!
//! Slipstream serves `ext-image-capture-source-v1` and `ext-image-copy-capture-v1`, which is what
//! the packaged `xdg-desktop-portal-wlr` talks to. The portal does the D-Bus conversation with
//! Discord, Chrome or Meet, hands the frames to PipeWire, and asks us only for pixels — so there
//! is no portal of our own to install.
//!
//! **Only the portal gets the globals.** A Wayland global is visible to every client, and there
//! is no `wp_security_context_v1` here, so advertising these to all comers would let any app
//! record the screen silently, or read every window's title, and make the picker's consent
//! decorative. The globals are created with a filter (`may_capture`) that lets through only
//! `xdg-desktop-portal-wlr`, known by its executable (`peer.rs`): a process outside any sandbox,
//! still running, whose executable is a root-owned file of that name that nobody else can write.
//! That stops ordinary apps capturing behind the picker's back. It is not a wall against code
//! already running as the user, which could start the real portal binary under its own control.
//!
//! Frames are not rendered inside the protocol handler: the renderer belongs to whichever backend
//! is drawing, so a requested frame waits in `pending` and is filled from the element list of the
//! next frame that output draws (`serve_captures` below, called from `udev.rs` and `winit.rs`).
//! That also means a capture costs one extra draw of a screen that was being drawn anyway, and
//! nothing at all while nothing is being shared.
//!
//! **Windows as well as screens.** Every window is announced on `ext-foreign-toplevel-list-v1`
//! (to the same clients, through the same filter), which is how the portal lists them for the
//! picker and names the one to capture. A window is drawn on its own into the capture, at its own
//! size, whichever workspace it is on — minimised into the code rain included.
//!
//! Constraints offer shared memory only, so the path is the proven `copy_framebuffer`/`map_texture`
//! one with no modifier negotiation. dmabuf is still to come, and is where the speed is.

mod peer;

#[cfg(test)]
pub use peer::with_a_client_that_has_gone;
pub use peer::{Peer, pidfd_open, still_running, unsandboxed_peer};

use std::{
    ffi::OsStr,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use smithay::{
    backend::renderer::element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
    desktop::Window,
    utils::{IsAlive, Logical, Point, Scale},
    wayland::{
        foreign_toplevel_list::{
            ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState,
        },
        image_capture_source::{ToplevelCaptureSourceHandler, ToplevelCaptureSourceState},
    },
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            gles::{GlesMapping, GlesRenderer, GlesTexture},
        },
    },
    output::Output,
    reexports::wayland_server::{Client, protocol::wl_buffer::WlBuffer, protocol::wl_shm},
    utils::{Buffer as BufferCoords, Physical, Rectangle, Size, Transform},
    wayland::{
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler, ImageCaptureSourceState,
            OutputCaptureSourceHandler, OutputCaptureSourceState,
        },
        image_copy_capture::{
            BufferConstraints, CaptureFailureReason, Frame, FrameRef, ImageCopyCaptureHandler,
            ImageCopyCaptureState, Session, SessionRef,
        },
        shm::with_buffer_contents_mut,
    },
};

use crate::{Slipstream, render::OutputElement};

/// The shm formats offered. Both are four bytes of blue, green, red, then alpha or padding, which
/// is what the renderer hands back for `Fourcc::Argb8888`, so filling a buffer is a straight copy.
const FORMATS: [wl_shm::Format; 2] = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888];

/// What a capture follows.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    Screen(Output),
    Window(Window),
}

/// A window announced on the toplevel list, with what it was last announced as, so a change of
/// title is sent once rather than every pass.
struct Listed {
    window: Window,
    handle: ForeignToplevelHandle,
    title: String,
    app_id: String,
}

/// Everything being captured, and everything waiting to be.
#[derive(Default)]
pub struct Captures {
    /// Open sessions, with the size each was last told to allocate for. Dropping one tells its
    /// client the session has stopped, so they are held here for as long as the client keeps
    /// them.
    sessions: Vec<(Session, Size<i32, Physical>)>,
    /// Frames a client has asked for, each waiting for the next frame drawn.
    pending: Vec<(Target, Frame)>,
    /// Every window, as the toplevel list announces it.
    listed: Vec<Listed>,
}

impl Captures {
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.pending.is_empty()
    }

    /// Whether something is being shared right now, for the bar's red dot. A screenshot's
    /// session is here for a moment too, and the dot showing for that moment is honest.
    pub fn sharing(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// The identifier a window is announced under.
    pub fn identifier_of(&self, window: &Window) -> Option<String> {
        self.listed
            .iter()
            .find(|listed| listed.window == *window)
            .map(|listed| listed.handle.identifier())
    }

    /// The window a toplevel-list identifier names.
    pub fn window_for(&self, identifier: &str) -> Option<Window> {
        self.listed
            .iter()
            .find(|listed| listed.handle.identifier() == identifier)
            .map(|listed| listed.window.clone())
    }
}

/// Whether any unsandboxed client may capture, for checks of the capture protocols with tools
/// such as `grim` in a nested run. Off unless the nested backend turns it on.
static OPEN_TO_ANYONE: AtomicBool = AtomicBool::new(false);

/// Whether a compositor lets any unsandboxed client capture: only nested, and only when
/// `SLIPSTREAM_CAPTURE_ANYONE=1` asks for it.
pub fn opens_to_anyone(nested: bool, capture_anyone: Option<&OsStr>) -> bool {
    nested && capture_anyone == Some(OsStr::new("1"))
}

/// Set once, when the backend is chosen, before any client connects.
pub fn open_to_anyone(open: bool) {
    OPEN_TO_ANYONE.store(open, Ordering::Relaxed);
}

fn is_open_to_anyone() -> bool {
    OPEN_TO_ANYONE.load(Ordering::Relaxed)
}

/// Whether a connecting client may see the capture globals and the toplevel list, worked out
/// once, when it connects, from `unsandboxed_peer`'s look at its process: `None` there, which is
/// anything short of proof that the client is outside a sandbox, is a refusal.
pub fn client_may_capture(peer: Option<&Peer>) -> bool {
    let Some(peer) = peer else {
        tracing::debug!("capture refused: a sandboxed client, or one that couldn't be looked at");
        return false;
    };
    let name = peer
        .exe
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(|| "unknown".into(), OsStr::to_string_lossy);
    if peer.portal {
        tracing::info!(pid = peer.pid, exe = %name, "capture allowed");
        true
    } else if is_open_to_anyone() {
        tracing::info!(pid = peer.pid, exe = %name, "capture allowed (nested test opt-in)");
        true
    } else {
        tracing::debug!(exe = %name, "capture refused");
        false
    }
}

/// Which clients may see the capture globals and the toplevel list: the portal, as
/// `client_may_capture` judged when the client connected. A client with no state of ours is
/// refused.
pub fn may_capture(client: &Client) -> bool {
    client
        .get_data::<crate::state::ClientState>()
        .is_some_and(|state| state.may_capture)
}

/// What a capture source stands for, if it is still there.
fn source_target(source: &ImageCaptureSource) -> Option<Target> {
    let data = source.user_data();
    if let Some(output) = data
        .get::<smithay::output::WeakOutput>()
        .and_then(|weak| weak.upgrade())
    {
        return Some(Target::Screen(output));
    }
    data.get::<Window>()
        .filter(|window| window.alive())
        .map(|window| Target::Window(window.clone()))
}

fn source_output(source: &ImageCaptureSource) -> Option<Output> {
    match source_target(source)? {
        Target::Screen(output) => Some(output),
        Target::Window(_) => None,
    }
}

/// The constraints for a size: shared memory only; dmabuf is still to come.
fn constraints(size: Size<i32, Physical>) -> BufferConstraints {
    BufferConstraints {
        size: Size::<i32, BufferCoords>::from((size.w, size.h)),
        shm: FORMATS.to_vec(),
        dma: None,
    }
}

impl ImageCaptureSourceHandler for Slipstream {}

impl OutputCaptureSourceHandler for Slipstream {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.output_capture_source
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}

impl ForeignToplevelListHandler for Slipstream {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevel_list
    }
}

impl ToplevelCaptureSourceHandler for Slipstream {
    fn toplevel_capture_source_state(&mut self) -> &mut ToplevelCaptureSourceState {
        &mut self.toplevel_capture_source
    }

    fn toplevel_source_created(
        &mut self,
        source: ImageCaptureSource,
        toplevel: ForeignToplevelHandle,
    ) {
        if let Some(window) = self.captures.window_for(&toplevel.identifier()) {
            source.user_data().insert_if_missing(|| window);
        }
    }
}

impl ImageCopyCaptureHandler for Slipstream {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.image_copy_capture
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        let target = source_target(source)?;
        Some(constraints(self.capture_size(&target)?))
    }

    fn new_session(&mut self, session: Session) {
        let target = source_target(&session.source());
        let size = target
            .as_ref()
            .and_then(|target| self.capture_size(target))
            .unwrap_or_default();
        match &target {
            Some(Target::Screen(output)) => {
                tracing::info!(output = output.name(), "a client is capturing a screen")
            }
            Some(Target::Window(window)) => {
                tracing::info!(
                    app = crate::state::logged_app(window),
                    "a client is capturing a window"
                )
            }
            None => {}
        }
        self.captures.sessions.push((session, size));
    }

    fn frame(&mut self, session: &SessionRef, frame: Frame) {
        let Some(target) = source_target(&session.source()) else {
            // The screen or window it was following has gone.
            frame.fail(CaptureFailureReason::Stopped);
            return;
        };
        // A window that has been resized since the client allocated: tell it the new size, and
        // fail this frame so it asks again with a buffer that fits.
        if let Some(size) = self.capture_size(&target) {
            if let Some((open, told)) = self
                .captures
                .sessions
                .iter_mut()
                .find(|(open, _)| **open == *session)
            {
                if *told != size {
                    *told = size;
                    open.update_constraints(constraints(size));
                    frame.fail(CaptureFailureReason::BufferConstraints);
                    return;
                }
            }
        }
        self.captures.pending.push((target, frame));
        // Nothing is asked of the backend here: both of them come round again within a frame's
        // time whether anything changed or not, and `serve_captures` fills this in when they do.
    }

    fn frame_aborted(&mut self, frame: FrameRef) {
        // The client took its buffer back. Filling it after that would be writing into memory
        // nobody owns any more.
        self.captures
            .pending
            .retain(|(_, waiting)| *waiting != frame);
    }

    fn session_destroyed(&mut self, session: SessionRef) {
        self.captures.sessions.retain(|(open, _)| **open != session);
        if self.captures.is_empty() {
            tracing::info!("nothing is being captured any more");
        }
    }
}

impl Slipstream {
    /// Fills every frame waiting on `output` from the element list just drawn. Called by the
    /// backends after a frame, with the same elements the screen itself was given, so what a
    /// client records is what was on the screen.
    pub fn serve_captures(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        elements: &[OutputElement],
        size: Size<i32, Physical>,
        scale: f64,
    ) {
        // Either protocol's frames are filled from the same draw, so a screen being recorded
        // twice over still costs one read-back.
        if self.captures.pending.is_empty() && !self.screencopy.wants(output) {
            return;
        }
        let presented = self.start_time.elapsed();
        let mut waiting: Vec<Frame> = Vec::new();
        let mut windows: Vec<(Window, Frame)> = Vec::new();
        let mut elsewhere = Vec::new();
        for (target, frame) in std::mem::take(&mut self.captures.pending) {
            match target {
                Target::Screen(screen) if &screen == output => waiting.push(frame),
                // A window isn't drawn from the screen's elements, so any screen's frame will do.
                Target::Window(window) => windows.push((window, frame)),
                // Another screen's: it waits for that screen to draw.
                other => elsewhere.push((other, frame)),
            }
        }
        self.captures.pending = elsewhere;
        for (window, frame) in windows {
            self.serve_window(renderer, output, &window, frame, presented);
        }
        let screencopy = self.screencopy.wants(output);
        if waiting.is_empty() && !screencopy {
            return;
        }
        // While the password card is up a screen's capture is black: the card says who is being
        // asked and shows how long the password is, and nothing recording the screen sees that.
        if self.polkit.is_up() {
            let black = [0, 0, 0, 0xff].repeat((size.w * size.h).max(0) as usize);
            if screencopy {
                self.serve_screencopy(output, &black, Some(&black), size, presented);
            }
            for frame in waiting {
                match copy_into(&frame.buffer(), &black, size) {
                    Ok(()) => frame.success(Transform::Normal, None, presented),
                    Err(CopyError::Constraints) => {
                        frame.fail(CaptureFailureReason::BufferConstraints)
                    }
                    Err(CopyError::NotShm) => frame.fail(CaptureFailureReason::Unknown),
                }
            }
            return;
        }
        // One draw fills every frame waiting on this screen, however many clients are recording.
        let transform = output.current_transform();
        let drawn = draw_offscreen(renderer, elements, size, scale, transform);
        let pixels = match read_back(renderer, &drawn) {
            Ok(pixels) => pixels,
            Err(err) => {
                tracing::warn!(output = output.name(), "capture failed: {err}");
                for frame in waiting {
                    frame.fail(CaptureFailureReason::Unknown);
                }
                self.fail_screencopy_on(output);
                return;
            }
        };
        if screencopy {
            // Programs that asked for no pointer get a second draw with the cursor left out —
            // `grim` asks for none by default, and a screenshot with an uninvited pointer in it
            // is a bug. At most two draws, however many programs are recording.
            let plain = self.screencopy.wants_without_cursor(output).then(|| {
                let bare: Vec<&OutputElement> = elements
                    .iter()
                    .filter(|element| {
                        smithay::backend::renderer::element::Element::kind(*element)
                            != smithay::backend::renderer::element::Kind::Cursor
                    })
                    .collect();
                draw_offscreen(renderer, &bare, size, scale, transform)
            });
            let plain_pixels = plain
                .as_ref()
                .and_then(|drawn| read_back(renderer, drawn).ok());
            self.serve_screencopy(output, pixels, plain_pixels, size, presented);
        }
        for frame in waiting {
            match copy_into(&frame.buffer(), pixels, size) {
                Ok(()) => frame.success(Transform::Normal, None, presented),
                Err(CopyError::Constraints) => frame.fail(CaptureFailureReason::BufferConstraints),
                Err(CopyError::NotShm) => frame.fail(CaptureFailureReason::Unknown),
            }
        }
    }
}

impl Slipstream {
    /// How big a capture of `target` is, in the pixels a client allocates.
    fn capture_size(&self, target: &Target) -> Option<Size<i32, Physical>> {
        match target {
            Target::Screen(output) => output.current_mode().map(|mode| mode.size),
            Target::Window(window) => {
                let size = window
                    .geometry()
                    .size
                    .to_f64()
                    .to_physical(self.window_scale(window))
                    .to_i32_round();
                (size.w > 0 && size.h > 0).then_some(size)
            }
        }
    }

    /// The scale a window is captured at: the one of the screen it is on, so text is as sharp
    /// in the capture as on the screen, else the focused screen's.
    fn window_scale(&self, window: &Window) -> f64 {
        self.space
            .outputs_for_element(window)
            .first()
            .map(|output| output.current_scale().fractional_scale())
            .or_else(|| self.output_scale())
            .unwrap_or(1.0)
    }

    /// Draws one window on its own into a client's buffer: its surfaces, at its own size, with
    /// the edge of its geometry at the buffer's corner, so client-side shadows are left out.
    fn serve_window(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        window: &Window,
        frame: Frame,
        presented: Duration,
    ) {
        let Some(size) = self.capture_size(&Target::Window(window.clone())) else {
            frame.fail(CaptureFailureReason::Unknown);
            return;
        };
        // While locked a window's capture is black, at its size: nothing of it is read.
        if self.lock.is_some() {
            let black = [0, 0, 0, 0xff].repeat((size.w * size.h).max(0) as usize);
            match copy_into(&frame.buffer(), &black, size) {
                Ok(()) => frame.success(Transform::Normal, None, presented),
                Err(CopyError::Constraints) => frame.fail(CaptureFailureReason::BufferConstraints),
                Err(CopyError::NotShm) => frame.fail(CaptureFailureReason::Unknown),
            }
            return;
        }
        let scale = self.window_scale(window);
        let location = (Point::<i32, Logical>::default() - window.geometry().loc)
            .to_f64()
            .to_physical(scale)
            .to_i32_round();
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            AsRenderElements::<GlesRenderer>::render_elements(
                window,
                renderer,
                location,
                Scale::from(scale),
                1.0,
            );
        let elements: Vec<OutputElement> =
            surfaces.into_iter().map(OutputElement::Surface).collect();
        let drawn = draw_offscreen(renderer, &elements, size, scale, Transform::Normal);
        match read_back(renderer, &drawn) {
            Ok(pixels) => match copy_into(&frame.buffer(), pixels, size) {
                Ok(()) => {
                    frame.success(Transform::Normal, None, presented);
                    // A shared window off screen gets no frames from any screen; this capture is
                    // its frame, so it draws the next one.
                    window.send_frame(output, presented, Some(Duration::ZERO), |_, _| {
                        Some(output.clone())
                    });
                }
                Err(CopyError::Constraints) => frame.fail(CaptureFailureReason::BufferConstraints),
                Err(CopyError::NotShm) => frame.fail(CaptureFailureReason::Unknown),
            },
            Err(err) => {
                tracing::warn!("window capture failed: {err}");
                frame.fail(CaptureFailureReason::Unknown);
            }
        }
    }

    /// Brings the toplevel list up to date with the windows there are: new ones announced, closed
    /// ones withdrawn, and a changed title or app id sent. Called once a pass of the event loop;
    /// comparing a few strings per window is cheaper than hooking every place a window comes,
    /// goes, or retitles itself.
    pub fn sync_toplevels(&mut self, windows: Vec<Window>) {
        let windows: Vec<Window> = windows.into_iter().filter(|w| w.alive()).collect();
        let list = &mut self.foreign_toplevel_list;
        self.captures.listed.retain(|listed| {
            let open = windows.contains(&listed.window);
            if !open {
                list.remove_toplevel(&listed.handle);
            }
            open
        });
        for window in windows {
            let title = crate::state::window_title(&window);
            let app_id = crate::state::window_app_id(&window).unwrap_or_default();
            match self
                .captures
                .listed
                .iter_mut()
                .find(|listed| listed.window == window)
            {
                Some(listed) => {
                    if listed.title != title || listed.app_id != app_id {
                        if listed.title != title {
                            listed.handle.send_title(&title);
                        }
                        if listed.app_id != app_id {
                            listed.handle.send_app_id(&app_id);
                        }
                        listed.handle.send_done();
                        listed.title = title;
                        listed.app_id = app_id;
                    }
                }
                None => {
                    let handle = self
                        .foreign_toplevel_list
                        .new_toplevel::<Slipstream>(title.clone(), app_id.clone());
                    self.captures.listed.push(Listed {
                        window,
                        handle,
                        title,
                        app_id,
                    });
                }
            }
        }
    }

    /// The red dot on the bar was clicked: every capture stops. Each client is told its session
    /// has ended, which the portal passes on to the app as the end of the share.
    pub fn stop_sharing(&mut self) {
        let count = self.captures.sessions.len();
        for (session, _) in self.captures.sessions.drain(..) {
            session.stop();
        }
        for (_, frame) in self.captures.pending.drain(..) {
            frame.fail(CaptureFailureReason::Stopped);
        }
        tracing::info!(count, "screen sharing stopped from the bar");
    }
}

/// The screen drawn again into an offscreen texture and copied out of it, to be read with
/// `read_back`. The same path as the `shot:` debug step.
fn draw_offscreen<E>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    size: Size<i32, Physical>,
    scale: f64,
    transform: Transform,
) -> Result<GlesMapping, Box<dyn std::error::Error>>
where
    E: smithay::backend::renderer::element::Element
        + smithay::backend::renderer::element::RenderElement<GlesRenderer>,
{
    let buffer_size = Size::<i32, BufferCoords>::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Argb8888, buffer_size)?;
    let mut target = renderer.bind(&mut texture)?;
    // The screen's own transform, not `Normal`: a capture is handed over in the orientation the
    // screen is in, and the program reading it turns that back using `wl_output.transform`. A
    // buffer drawn upright for a screen that says it is flipped comes out upside down.
    let mut damage = OutputDamageTracker::new(size, scale, transform);
    damage.render_output(
        renderer,
        &mut target,
        0,
        elements,
        crate::render::BACKGROUND,
    )?;
    Ok(renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Argb8888)?)
}

/// The pixels of a `draw_offscreen`, read in place rather than copied: a screen being shared is
/// read back every frame.
fn read_back<'a>(
    renderer: &mut GlesRenderer,
    drawn: &'a Result<GlesMapping, Box<dyn std::error::Error>>,
) -> Result<&'a [u8], String> {
    match drawn {
        Ok(mapping) => renderer.map_texture(mapping).map_err(|err| err.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

/// Why a client's buffer couldn't be filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyError {
    /// Not a shared-memory buffer at all, or its pool has gone.
    NotShm,
    /// The wrong format, or too small for what was asked for.
    Constraints,
}

/// Whether a client's buffer is one that can be filled at all: shared memory, in a format the
/// renderer's pixels go straight into. Checked before anything is promised about a capture, so a
/// buffer of the wrong kind is answered as the protocol error it is rather than a late failure.
pub fn buffer_is_usable(buffer: &WlBuffer) -> Result<(), CopyError> {
    smithay::wayland::shm::with_buffer_contents(buffer, |_, _, data| {
        if FORMATS.contains(&data.format) {
            Ok(())
        } else {
            Err(CopyError::Constraints)
        }
    })
    .unwrap_or(Err(CopyError::NotShm))
}

/// `pixels` in full, into a client's buffer.
fn copy_into(buffer: &WlBuffer, pixels: &[u8], size: Size<i32, Physical>) -> Result<(), CopyError> {
    copy_region_into(buffer, pixels, size, Rectangle::from_size(size))
}

/// Part of `pixels` into a client's buffer: the rectangle `region` out of a picture `src` across,
/// row by row.
///
/// `pixels` is what the renderer handed back — four bytes a pixel, tightly packed, top row first.
/// The client's buffer may be wider than the rows going into it (a stride of its own choosing),
/// so each row is placed on its own rather than the whole thing copied at once.
pub fn copy_region_into(
    buffer: &WlBuffer,
    pixels: &[u8],
    src: Size<i32, Physical>,
    region: Rectangle<i32, Physical>,
) -> Result<(), CopyError> {
    // The region has to lie inside the picture, and the picture has to be all there.
    let inside = region.size.w > 0
        && region.size.h > 0
        && region.loc.x >= 0
        && region.loc.y >= 0
        && region
            .loc
            .x
            .checked_add(region.size.w)
            .is_some_and(|end| end <= src.w)
        && region
            .loc
            .y
            .checked_add(region.size.h)
            .is_some_and(|end| end <= src.h)
        && pixels.len() >= src.w as usize * src.h as usize * 4;
    if !inside {
        return Err(CopyError::Constraints);
    }
    with_buffer_contents_mut(buffer, |ptr, len, data| {
        if !FORMATS.contains(&data.format)
            || data.width < region.size.w
            || data.height < region.size.h
            || data.stride < 0
            || data.offset < 0
        {
            return Err(CopyError::Constraints);
        }
        let row = region.size.w as usize * 4;
        let src_row = src.w as usize * 4;
        let stride = data.stride as usize;
        let offset = data.offset as usize;
        let rows = region.size.h as usize;
        // Every row has to fit the client's stride and lie inside its pool.
        let end = stride
            .checked_mul(rows)
            .and_then(|size| size.checked_add(offset));
        if stride < row || end.is_none_or(|end| end > len) {
            return Err(CopyError::Constraints);
        }
        for y in 0..rows {
            let from = (region.loc.y as usize + y) * src_row + region.loc.x as usize * 4;
            // SAFETY: the checks above put every row inside the picture and inside the pool, and
            // the pool is mapped for as long as this closure runs.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels[from..].as_ptr(),
                    ptr.add(offset + y * stride),
                    row,
                );
            }
        }
        Ok(())
    })
    .unwrap_or(Err(CopyError::NotShm))
}

/// The globals, made once at startup. `new_with_filter` rather than `new`, so they are offered
/// only to the clients `may_capture` allows. The toplevel list goes through the same filter: a
/// list of every window's title is only the portal's business.
pub struct Globals {
    pub source: ImageCaptureSourceState,
    pub output_source: OutputCaptureSourceState,
    pub toplevel_source: ToplevelCaptureSourceState,
    pub copy: ImageCopyCaptureState,
    pub toplevel_list: ForeignToplevelListState,
}

pub fn state(display: &smithay::reexports::wayland_server::DisplayHandle) -> Globals {
    Globals {
        source: ImageCaptureSourceState::new(),
        output_source: OutputCaptureSourceState::new_with_filter::<Slipstream, _>(
            display,
            may_capture,
        ),
        toplevel_source: ToplevelCaptureSourceState::new_with_filter::<Slipstream, _>(
            display,
            may_capture,
        ),
        copy: ImageCopyCaptureState::new_with_filter::<Slipstream, _>(display, may_capture),
        toplevel_list: ForeignToplevelListState::new_with_filter::<Slipstream>(
            display,
            may_capture,
        ),
    }
}

impl Slipstream {
    /// The size a screen is captured at, for a session already following it. A screen that
    /// changes mode mid-share would otherwise go on offering the size it had when the share
    /// started, and every frame after it would fail on the constraints.
    pub fn capture_size_changed(&mut self, output: &Output) {
        // A screencopy frame was told a size when its buffer was described, and there is no way
        // to tell it a new one: it is failed, and the program asks again.
        self.fail_screencopy_on(output);
        let Some(size) = output.current_mode().map(|mode| mode.size) else {
            return;
        };
        for (session, told) in &mut self.captures.sessions {
            if source_output(&session.source()).as_ref() == Some(output) {
                *told = size;
                session.update_constraints(constraints(size));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixStream, path::PathBuf};

    use super::*;

    /// Whether a client connecting from a child that exits at once may capture.
    fn connect_from_a_child_that_exits(reap: bool) -> bool {
        with_a_client_that_has_gone("cap", reap, |stream| {
            client_may_capture(unsandboxed_peer(stream).as_ref())
        })
    }

    #[test]
    fn a_client_whose_connecting_process_has_gone_is_refused() {
        assert!(
            !connect_from_a_child_that_exits(false),
            "a zombie has no root to look in"
        );
        assert!(
            !connect_from_a_child_that_exits(true),
            "nor does a process that has been reaped"
        );
    }

    #[test]
    fn a_client_of_ours_cannot_capture() {
        // Both ends of a pair are this process: not in a Flatpak, still running, and not the
        // portal.
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        let peer = unsandboxed_peer(&ours).expect("this process can be looked at");
        assert!(!peer.portal);
        assert!(!client_may_capture(Some(&peer)));
    }

    #[test]
    fn only_the_portal_captures() {
        let peer = |portal| Peer {
            pid: 4242,
            exe: Some(PathBuf::from("/usr/libexec/example")),
            portal,
            settled: true,
        };
        assert!(client_may_capture(Some(&peer(true))));
        assert!(!client_may_capture(Some(&peer(false))));
        assert!(!client_may_capture(None), "no proof, no capture");
    }

    #[test]
    fn capture_is_never_open_to_anyone_by_default() {
        // No test turns the switch on, so this is how the login session starts.
        assert!(!is_open_to_anyone());
        assert!(
            !opens_to_anyone(false, Some(OsStr::new("1"))),
            "never in the session"
        );
        assert!(!opens_to_anyone(false, None));
        assert!(!opens_to_anyone(true, None));
        assert!(!opens_to_anyone(true, Some(OsStr::new("yes"))));
        assert!(opens_to_anyone(true, Some(OsStr::new("1"))));
    }
}

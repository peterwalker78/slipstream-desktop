//! Screen sharing: the compositor half of it.
//!
//! Slipstream serves `ext-image-capture-source-v1` and `ext-image-copy-capture-v1`, which is what
//! the packaged `xdg-desktop-portal-wlr` talks to. The portal does the D-Bus conversation with
//! Discord, Chrome or Meet, hands the frames to PipeWire, and asks us only for pixels — so there
//! is no portal of our own to install, which on this read-only `/usr` there would be nowhere to
//! put anyway.
//!
//! **Only the portal gets the globals.** A Wayland global is visible to every client, and there
//! is no `wp_security_context_v1` here, so advertising these to all comers would let any app
//! record the screen silently, or read every window's title, and make the picker's consent
//! decorative. The globals are created with a filter (`may_capture`) that lets through only
//! `xdg-desktop-portal-wlr`, known by its executable: a process outside any sandbox, still
//! running, whose executable is a root-owned file of that name that nobody else can write. That
//! stops ordinary apps capturing behind the picker's back. It is not a wall against code already
//! running as the user, which could start the real portal binary under its own control.
//!
//! Frames are not rendered inside the protocol handler: the renderer belongs to whichever backend
//! is drawing, so a requested frame waits in `pending` and is filled from the element list of the
//! next frame that output draws (`serve` below, called from `udev.rs` and `winit.rs`). That also
//! means a capture costs one extra draw of a screen that was being drawn anyway, and nothing at
//! all while nothing is being shared.
//!
//! **Windows as well as screens.** Every window is announced on `ext-foreign-toplevel-list-v1`
//! (to the same clients, through the same filter), which is how the portal lists them for the
//! picker and names the one to capture. A window is drawn on its own into the capture, at its own
//! size, whichever workspace it is on — minimised into the code rain included.
//!
//! Constraints offer shared memory only, so the path is the proven `copy_framebuffer`/`map_texture`
//! one with no modifier negotiation. dmabuf is still to come, and is where the speed is.

use std::{
    ffi::OsStr,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
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
            gles::{GlesRenderer, GlesTexture},
        },
    },
    output::Output,
    reexports::{
        rustix,
        wayland_server::{Client, protocol::wl_shm},
    },
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

/// The executable the portal's backend is installed as.
const PORTAL: &str = "xdg-desktop-portal-wlr";

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

/// The process at the other end of a connection, once it's proven to be outside any sandbox.
pub struct Peer {
    pid: i32,
    /// What `/proc/<pid>/exe` names, if it could be read.
    exe: Option<PathBuf>,
    /// Its executable is the portal's (`is_portal_executable`).
    portal: bool,
}

/// The connecting process, only when it is still running, its root filesystem could be opened,
/// and that root definitely has no `/.flatpak-info` — the file a Flatpak sees in its own root,
/// and the same check the portal makes. Read through `/proc`, so it is the kernel's view of the
/// process and not anything the client said. One look serves every decision made about a new
/// client: the capture globals here, and the clipboard's data control.
///
/// **Anything short of proof is `None`.** The connection's credentials name the process that
/// called `connect`, and that process may be gone by the time the connection is accepted: a
/// sandboxed app can connect from a child that exits at once and go on using the socket itself.
/// A test that treated "couldn't look" as "not sandboxed" would hand that app whatever the
/// unsandboxed get.
pub fn unsandboxed_peer(stream: &UnixStream) -> Option<Peer> {
    use rustix::fs::{Mode, OFlags};
    let peer = rustix::net::sockopt::socket_peercred(stream).ok()?;
    let pid = peer.pid.as_raw_nonzero().get();
    // Held from here to the end, so the process looked at can be asked afterwards whether it is
    // still the one that connected.
    let pidfd = peer_pidfd(stream, pid)?;
    // A process that has exited, zombie or reaped, has no root to open.
    let root = rustix::fs::open(
        format!("/proc/{pid}/root"),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    if !root_is_unsandboxed(&root) {
        return None;
    }
    // The executable itself, opened by path only, so what's looked at is the file's inode and
    // not a name that could be swapped afterwards; and the name it was started from.
    let exe = format!("/proc/{pid}/exe");
    let owner = rustix::fs::open(&exe, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
        .ok()
        .and_then(|file| rustix::fs::fstat(&file).ok())
        .map(|stat| (stat.st_uid, stat.st_mode));
    let path = rustix::fs::readlink(&exe, Vec::new())
        .ok()
        .map(|link| PathBuf::from(OsStr::from_bytes(link.as_bytes())));
    // Still running now, so the pid read above was still its own the whole time and was never
    // handed to another process.
    if !still_running(&pidfd) {
        return None;
    }
    let portal = match (&path, owner) {
        (Some(path), Some((uid, mode))) => is_portal_executable(path, uid, mode),
        _ => false,
    };
    Some(Peer {
        pid,
        exe: path,
        portal,
    })
}

/// Whether a process's root directory definitely has no `.flatpak-info`. A file, a directory or
/// a link of that name is there, and a root that can't be looked in is unknowable: either way,
/// not proof of anything but a sandbox.
fn root_is_unsandboxed(root: &OwnedFd) -> bool {
    use rustix::{fs::AtFlags, io::Errno};
    matches!(
        rustix::fs::statat(root, ".flatpak-info", AtFlags::SYMLINK_NOFOLLOW),
        Err(Errno::NOENT)
    )
}

/// Whether an executable, by the path its process was started from and its file's owner and
/// mode, is the portal's: a regular file named `xdg-desktop-portal-wlr`, owned by root and not
/// writable by group or others, so nobody but root can have put it there. A binary replaced since
/// it started reads as `… (deleted)`, which still counts.
fn is_portal_executable(path: &Path, uid: u32, mode: u32) -> bool {
    let bytes = path.as_os_str().as_bytes();
    let path = Path::new(OsStr::from_bytes(
        bytes.strip_suffix(b" (deleted)").unwrap_or(bytes),
    ));
    let regular = mode & libc::S_IFMT == libc::S_IFREG;
    let writable_by_others = mode & 0o022 != 0;
    regular && uid == 0 && !writable_by_others && path.file_name() == Some(OsStr::new(PORTAL))
}

/// A pidfd for the process at the other end of `stream`: `SO_PEERPIDFD`, which names exactly the
/// process that connected, or on kernels older than 6.5, a pidfd opened from the pid its
/// credentials gave.
fn peer_pidfd(stream: &UnixStream, pid: i32) -> Option<OwnedFd> {
    let mut fd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `fd` and `len` are valid for writes of the sizes given.
    let got = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERPIDFD,
            (&raw mut fd).cast(),
            &mut len,
        )
    };
    if got == 0 && fd >= 0 {
        // SAFETY: the kernel has just given us this descriptor to own.
        return Some(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOPROTOOPT) {
        return None;
    }
    pidfd_open(pid as u32)
}

/// A pidfd for whatever process has `pid` now.
pub fn pidfd_open(pid: u32) -> Option<OwnedFd> {
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    // SAFETY: as above, a descriptor that is ours to own.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) })
}

/// Whether the process behind `pidfd` still exists: signal 0 is only the permission and
/// existence check, and nothing is delivered.
pub fn still_running(pidfd: impl std::os::fd::AsFd) -> bool {
    // SAFETY: a valid pidfd, signal 0, no siginfo, no flags.
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_fd().as_raw_fd(),
            0,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        ) == 0
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
        if self.captures.pending.is_empty() {
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
        if waiting.is_empty() {
            return;
        }
        // One draw fills every frame waiting on this screen, however many clients are recording.
        let pixels = match draw_offscreen(renderer, elements, size, scale) {
            Ok(pixels) => pixels,
            Err(err) => {
                tracing::warn!(output = output.name(), "capture failed: {err}");
                for frame in waiting {
                    frame.fail(CaptureFailureReason::Unknown);
                }
                return;
            }
        };
        for frame in waiting {
            match copy_into(&frame, &pixels, size) {
                Ok(()) => frame.success(Transform::Normal, None, presented),
                Err(reason) => frame.fail(reason),
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
            match copy_into(&frame, &black, size) {
                Ok(()) => frame.success(Transform::Normal, None, presented),
                Err(reason) => frame.fail(reason),
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
        match draw_offscreen(renderer, &elements, size, scale) {
            Ok(pixels) => match copy_into(&frame, &pixels, size) {
                Ok(()) => {
                    frame.success(Transform::Normal, None, presented);
                    // A shared window off screen gets no frames from any screen; this capture is
                    // its frame, so it draws the next one.
                    window.send_frame(output, presented, Some(Duration::ZERO), |_, _| {
                        Some(output.clone())
                    });
                }
                Err(reason) => frame.fail(reason),
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

/// The screen drawn again into an offscreen texture, as bytes. The same path as the `shot:` debug
/// step, which is what makes this the milestone it is: nothing new has to be right for it to work.
fn draw_offscreen(
    renderer: &mut GlesRenderer,
    elements: &[OutputElement],
    size: Size<i32, Physical>,
    scale: f64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let buffer_size = Size::<i32, BufferCoords>::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Argb8888, buffer_size)?;
    let mut target = renderer.bind(&mut texture)?;
    let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
    damage.render_output(
        renderer,
        &mut target,
        0,
        elements,
        crate::render::BACKGROUND,
    )?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Argb8888)?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

/// Copies the screen into one client's buffer, a row at a time: the client's stride is its own
/// business and is often wider than the picture.
fn copy_into(
    frame: &Frame,
    pixels: &[u8],
    size: Size<i32, Physical>,
) -> Result<(), CaptureFailureReason> {
    let buffer = frame.buffer();
    with_buffer_contents_mut(&buffer, |ptr, len, data| {
        if !FORMATS.contains(&data.format) || data.width < size.w || data.height < size.h {
            return Err(CaptureFailureReason::BufferConstraints);
        }
        let row = size.w as usize * 4;
        let stride = data.stride as usize;
        let offset = data.offset as usize;
        let rows = size.h as usize;
        if stride < row || offset + stride * rows > len || pixels.len() < row * rows {
            return Err(CaptureFailureReason::BufferConstraints);
        }
        for y in 0..rows {
            // Safety: the bounds above put every row inside the pool, and the pool is mapped for
            // as long as this closure runs.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels[y * row..].as_ptr(),
                    ptr.add(offset + y * stride),
                    row,
                );
            }
        }
        Ok(())
    })
    .unwrap_or(Err(CaptureFailureReason::Unknown))
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

/// Runs `look` on the accepted end of a connection made from a child process that left at once,
/// while the parent keeps the connected socket: the process the connection's credentials name
/// has gone, as a zombie, or with `reap`, reaped. `name` keeps each caller's socket apart.
#[cfg(test)]
pub fn with_a_client_that_has_gone<T>(
    name: &str,
    reap: bool,
    look: impl FnOnce(&UnixStream) -> T,
) -> T {
    let dir = crate::files::test_scratch(&format!(
        "{name}-{}",
        if reap { "reaped" } else { "zombie" }
    ));
    let path = dir.join("socket");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    // Everything the child needs is made before the fork, so the child calls nothing but
    // connect and _exit, which are safe after forking a threaded process.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    assert!(
        bytes.len() < address.sun_path.len(),
        "{path:?} is too long for a socket"
    );
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    let socket = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    assert!(socket >= 0);
    let child = unsafe { libc::fork() };
    if child == 0 {
        unsafe {
            libc::connect(
                socket,
                (&raw const address).cast(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            );
            libc::_exit(0);
        }
    }
    assert!(child > 0);
    let (accepted, _) = listener.accept().unwrap();
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // Waits for the child to have exited; without WNOWAIT that also reaps it.
    let flags = libc::WEXITED | if reap { 0 } else { libc::WNOWAIT };
    assert_eq!(
        unsafe { libc::waitid(libc::P_PID, child as libc::id_t, &mut info, flags) },
        0
    );
    let answer = look(&accepted);
    if !reap {
        unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
    }
    unsafe { libc::close(socket) };
    std::fs::remove_dir_all(&dir).unwrap();
    answer
}

#[cfg(test)]
mod tests {
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
        };
        assert!(client_may_capture(Some(&peer(true))));
        assert!(!client_may_capture(Some(&peer(false))));
        assert!(!client_may_capture(None), "no proof, no capture");
    }

    #[test]
    fn a_root_with_flatpak_info_is_a_sandbox() {
        use rustix::fs::{Mode, OFlags};
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::files::test_scratch("cap-roots");
        let root = |name: &str| {
            let path = dir.join(name);
            std::fs::create_dir(&path).unwrap();
            path
        };
        let open = |path: &Path| {
            rustix::fs::open(
                path,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap()
        };
        let plain = root("plain");
        assert!(root_is_unsandboxed(&open(&plain)));
        let file = root("file");
        std::fs::write(
            file.join(".flatpak-info"),
            "[Application]\nname=org.example.App\n",
        )
        .unwrap();
        assert!(!root_is_unsandboxed(&open(&file)));
        let directory = root("directory");
        std::fs::create_dir(directory.join(".flatpak-info")).unwrap();
        assert!(!root_is_unsandboxed(&open(&directory)));
        let link = root("link");
        std::os::unix::fs::symlink("/nowhere/at/all", link.join(".flatpak-info")).unwrap();
        assert!(
            !root_is_unsandboxed(&open(&link)),
            "a link is there, wherever it points"
        );
        // A root that can't be searched can't be looked in. Root can search anything, so that
        // case only means something for anyone else.
        let shut = root("shut");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(!root_is_unsandboxed(&open(&shut)), "unknowable is refused");
        }
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_portal_is_known_by_its_executable() {
        let file = libc::S_IFREG;
        let portal = |path: &str, uid, mode| is_portal_executable(Path::new(path), uid, mode);
        assert!(portal(
            "/usr/libexec/xdg-desktop-portal-wlr",
            0,
            file | 0o755
        ));
        assert!(portal("/usr/lib/xdg-desktop-portal-wlr", 0, 0o100755));
        assert!(portal(
            "/usr/libexec/xdg-desktop-portal-wlr (deleted)",
            0,
            0o100755
        ));
        assert!(!portal(
            "/usr/libexec/xdg-desktop-portal-wlr",
            1000,
            0o100755
        ));
        assert!(!portal("/usr/libexec/xdg-desktop-portal-wlr", 0, 0o100775));
        assert!(!portal("/usr/libexec/xdg-desktop-portal-wlr", 0, 0o100757));
        assert!(!portal("/usr/bin/grim", 0, 0o100755));
        assert!(!portal(
            "/home/alex/bin/xdg-desktop-portal-wlr-x",
            0,
            0o100755
        ));
        assert!(
            !portal(
                "/usr/libexec/xdg-desktop-portal-wlr",
                0,
                libc::S_IFDIR | 0o755
            ),
            "not a regular file"
        );
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

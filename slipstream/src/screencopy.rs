//! `wlr-screencopy-unstable-v1`: the screen, to a program that asks for it.
//!
//! This is what `grim`, `wf-recorder` and most of the screenshot scripts people already have
//! speak. The portal path (`capture`) serves `ext-image-copy-capture-v1` and is hidden from
//! everything but the portal itself, which is right for a protocol the portal's own consent
//! covers — but it leaves every ordinary tool with nothing to talk to, because this is the
//! protocol they were written against.
//!
//! **So the consent is ours to ask for.** A global filter can't work here: the whole point is
//! that ordinary programs reach it. Instead every program is judged by the executable it was
//! started from, and the screen is given up only to one the user has allowed. Until then a
//! request is refused outright — never quietly granted.
//!
//! Three rules hold that line:
//!
//! - **A program that can't be named can't be allowed.** `unsandboxed_peer` returns nothing for a
//!   sandboxed client, or one `/proc` wouldn't answer for, and `settled_exe` returns nothing for
//!   an executable a user could rewrite. Either way there is no name to put on a card and nothing
//!   safe to remember, so the answer is no.
//! - **Nothing is captured while the screen is locked.** Not refused late — refused on arrival,
//!   and every frame already waiting is failed the moment the lock goes up.
//! - **Nothing is rendered in a protocol handler.** A `copy` request only checks and waits; the
//!   pixels come from the next frame the screen draws anyway, which is the same arrangement
//!   `capture` uses and for the same reason.
//!
//! Shared memory only. `wl-screenrec` wants dmabuf and will refuse; `grim` and `wf-recorder`
//! work, the latter more slowly than it would with dmabuf. That path is still to come, and it is
//! the one both protocols would share.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use smithay::{
    output::{Output, WeakOutput},
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Physical, Rectangle, Size},
};

use crate::{
    capture::{CopyError, copy_region_into},
    state::Slipstream,
};

/// The version served. Three brings `buffer_done`, which every current tool waits for.
const VERSION: u32 = 3;
/// How many frames one client may have waiting at once. A program in a loop can make them faster
/// than screens draw; past this the rest are refused rather than queued for ever.
const PENDING_PER_CLIENT: usize = 8;

/// What a frame object is for, kept on the object itself so a client that never asks for a copy
/// costs nothing at all.
#[derive(Debug)]
pub struct FrameData {
    /// `None` on a frame that was refused as it was made, which is inert from then on.
    output: Option<WeakOutput>,
    /// What to copy, in the screen's own pixels, already clipped to it.
    region: Rectangle<i32, Physical>,
    /// The screen's size when the buffer was described. A screen that changes mode after that
    /// makes the description a lie, and the frame is failed rather than filled from the new one.
    described: Size<i32, Physical>,
    with_cursor: bool,
    /// Whether the program may have this frame: set when its buffer is described, which happens
    /// only once it is allowed. A copy asked for before then, while a card is still asking about
    /// it, is refused rather than filled.
    allowed: AtomicBool,
    /// Whether a copy has been asked for already: the protocol says asking twice is an error.
    /// Atomic because a client's own data has to be shareable between threads.
    used: AtomicBool,
}

/// What to do about a program that has asked for the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Allowed,
    /// Put this program on a card and hold its frames until it is answered.
    Ask {
        program: String,
        exe: PathBuf,
    },
    No,
}

/// Allows a frame and tells its client what buffer to make. Sent once the program is allowed,
/// which may be now or once a card has been answered.
fn describe_buffer(frame: &ZwlrScreencopyFrameV1) {
    let Some(data) = frame.data::<FrameData>() else {
        frame.failed();
        return;
    };
    data.allowed.store(true, Ordering::Relaxed);
    let region = data.region;
    // Four bytes a pixel, packed, which is what the renderer hands back and what
    // `copy_region_into` writes. A client may choose a wider stride of its own.
    frame.buffer(
        wl_shm::Format::Xrgb8888,
        region.size.w as u32,
        region.size.h as u32,
        region.size.w as u32 * 4,
    );
    // Only version three knows this, and it is what says "that is every kind we have".
    if frame.version() >= 3 {
        frame.buffer_done();
    }
}

/// A frame waiting for the screen it follows to be drawn.
struct Waiting {
    frame: ZwlrScreencopyFrameV1,
    /// The buffer the client gave, held only until the next draw fills it.
    buffer: WlBuffer,
    /// The client that asked, so one client can't fill the queue.
    client: Option<ClientId>,
    with_damage: bool,
}

#[derive(Default)]
pub struct Screencopy {
    waiting: Vec<Waiting>,
    /// Programs allowed for this session only, by the "Until you log out" answer. The settings file
    /// holds the ones allowed for good.
    once: Vec<PathBuf>,
    /// Frames whose program is being asked about, held until the card is answered. Their clients
    /// have been told nothing yet, so they are simply waiting.
    asked_about: Vec<(PathBuf, ZwlrScreencopyFrameV1)>,
}

impl Screencopy {
    /// Whether anything is waiting on this screen, so a draw knows to read itself back.
    pub fn wants(&self, output: &Output) -> bool {
        self.waiting.iter().any(|it| {
            it.frame
                .data::<FrameData>()
                .and_then(|data| data.output.as_ref()?.upgrade())
                .is_some_and(|waiting_on| waiting_on == *output)
        })
    }

    /// Whether anything waiting on this screen asked for the pointer to be left out, which needs
    /// a draw of its own.
    pub fn wants_without_cursor(&self, output: &Output) -> bool {
        self.waiting.iter().any(|it| {
            it.frame.data::<FrameData>().is_some_and(|data| {
                !data.with_cursor
                    && data.output.as_ref().and_then(WeakOutput::upgrade).as_ref() == Some(output)
            })
        })
    }

    /// Allows a program for the rest of this session, without writing anything down.
    pub fn allow_once(&mut self, exe: PathBuf) {
        if !self.once.contains(&exe) {
            self.once.push(exe);
        }
    }

    /// Whether a program is already being asked about, so a second frame from it joins the queue
    /// rather than raising a second card.
    fn already_asking(&self, exe: &Path) -> bool {
        self.asked_about.iter().any(|(waiting, _)| waiting == exe)
    }

    /// How many frames are held for a program while it is asked about.
    fn held_for(&self, exe: &Path) -> usize {
        self.asked_about
            .iter()
            .filter(|(waiting, _)| waiting == exe)
            .count()
    }

    /// The frames held for `exe`, taken off the queue.
    fn answered(&mut self, exe: &Path) -> Vec<ZwlrScreencopyFrameV1> {
        let (mine, rest) = std::mem::take(&mut self.asked_about)
            .into_iter()
            .partition::<Vec<_>, _>(|(waiting, _)| waiting == exe);
        self.asked_about = rest;
        mine.into_iter().map(|(_, frame)| frame).collect()
    }

    fn allowed_once(&self, exe: &Path) -> bool {
        self.once.iter().any(|it| it == exe)
    }
}

pub fn state(display: &DisplayHandle) -> GlobalId {
    display.create_global::<Slipstream, ZwlrScreencopyManagerV1, _>(VERSION, ())
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Slipstream {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Slipstream {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, with_cursor, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor != 0, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (
                frame,
                overlay_cursor != 0,
                output,
                Some(Rectangle::new((x, y).into(), (width, height).into())),
            ),
            _ => return,
        };
        state.begin_screencopy(client, data_init, frame, with_cursor, &output, region);
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Slipstream {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        if data.used.swap(true, Ordering::Relaxed) {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "this frame has been copied already",
            );
            return;
        }
        // A frame whose buffer was never described has not been allowed: its program is still
        // being asked about, or was refused. The client could guess the buffer from the screen's
        // size, so the copy itself is what has to be refused.
        if !data.allowed.load(Ordering::Relaxed) {
            state
                .screencopy
                .asked_about
                .retain(|(_, held)| held != resource);
            resource.failed();
            return;
        }
        // Nothing is captured behind the lock, and a frame that arrives while it is up is
        // refused now rather than held until it comes down.
        if state.lock.is_some() {
            resource.failed();
            return;
        }
        let Some(output) = data.output.as_ref().and_then(WeakOutput::upgrade) else {
            resource.failed();
            return;
        };
        // A screen that changed mode since the buffer was described would be copied into a
        // buffer of the wrong size.
        if screen_size(&output) != Some(data.described) {
            resource.failed();
            return;
        }
        // The buffer has to be one we can write into before anything is promised about it. This
        // is the one check that has to happen now: it is the client's buffer, not ours, and the
        // protocol wants a protocol error rather than a failure for the wrong kind.
        if let Err(CopyError::NotShm) = crate::capture::buffer_is_usable(&buffer) {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "only shared-memory buffers are supported",
            );
            return;
        }
        let id = client.id();
        let queued = state
            .screencopy
            .waiting
            .iter()
            .filter(|it| it.client.as_ref() == Some(&id))
            .count();
        if queued >= PENDING_PER_CLIENT {
            tracing::warn!("a program has too many screen captures waiting; refusing this one");
            resource.failed();
            return;
        }
        state.screencopy.waiting.push(Waiting {
            frame: resource.clone(),
            buffer,
            client: Some(id),
            with_damage,
        });
    }

    /// The frame is gone, by `destroy` or with its client. Anything still waiting on it is
    /// dropped: filling a buffer whose owner has let go of it is writing into memory that is not
    /// ours any more.
    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrScreencopyFrameV1,
        _data: &FrameData,
    ) {
        state.screencopy.waiting.retain(|it| it.frame != *resource);
        state
            .screencopy
            .asked_about
            .retain(|(_, held)| held != resource);
    }
}

impl Slipstream {
    /// A program has asked for a screen. Judges it first and makes the frame object once, then
    /// either describes the buffer to make, holds the frame while the card asks, or refuses.
    fn begin_screencopy(
        &mut self,
        client: &Client,
        data_init: &mut DataInit<'_, Self>,
        frame: New<ZwlrScreencopyFrameV1>,
        with_cursor: bool,
        output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
        region: Option<Rectangle<i32, smithay::utils::Logical>>,
    ) {
        let (verdict, place) = self.judge_screencopy(client, output, region);
        let Some((output, region, size)) = place else {
            // Refused outright: an inert frame, told so at once. It is never allowed, so a copy
            // on it is failed rather than reaching for a screen it was never given.
            let frame = data_init.init(
                frame,
                FrameData {
                    output: None,
                    region: Rectangle::default(),
                    described: Size::default(),
                    with_cursor,
                    allowed: AtomicBool::new(false),
                    used: AtomicBool::new(false),
                },
            );
            frame.failed();
            return;
        };
        let frame = data_init.init(
            frame,
            FrameData {
                output: Some(output.downgrade()),
                region,
                described: size,
                with_cursor,
                allowed: AtomicBool::new(false),
                used: AtomicBool::new(false),
            },
        );
        match verdict {
            Verdict::Allowed => describe_buffer(&frame),
            // Held: the client is told nothing until the card is answered, so it simply waits.
            Verdict::Ask { program, exe } => {
                if !self.screencopy.already_asking(&exe) {
                    self.ask_about_capture(program, exe.clone());
                } else if self.screencopy.held_for(&exe) >= PENDING_PER_CLIENT {
                    frame.failed();
                    return;
                }
                self.screencopy.asked_about.push((exe, frame));
            }
            Verdict::No => frame.failed(),
        }
    }

    /// Raises the card that asks whether a program may record the screen.
    fn ask_about_capture(&mut self, program: String, exe: PathBuf) {
        let now = self.wall();
        self.close_panels();
        self.share = Some(crate::share::Picker::for_capture(program.clone(), exe, now));
        self.wake_ui();
        tracing::info!(program, "asking whether this program may record the screen");
    }

    /// The card was answered. `remember` writes the program down; otherwise it is allowed for
    /// this session only. Every frame held for it is then described, and its client goes on.
    pub fn allow_capture(&mut self, exe: PathBuf, remember: bool) {
        if remember {
            let path = exe.to_string_lossy().into_owned();
            self.change_settings(move |file| file.privacy.allow_capture(path.clone()));
        } else {
            self.screencopy.allow_once(exe.clone());
        }
        let held = self.screencopy.answered(&exe);
        tracing::info!(
            frames = held.len(),
            remember,
            "a program may record the screen"
        );
        for frame in held {
            describe_buffer(&frame);
        }
    }

    /// The card was refused, or went away: every frame held for that program is failed, and its
    /// client learns it got nothing.
    pub fn refuse_capture(&mut self, exe: PathBuf) {
        let held = self.screencopy.answered(&exe);
        if !held.is_empty() {
            tracing::info!(frames = held.len(), "a program may not record the screen");
        }
        for frame in held {
            frame.failed();
        }
    }

    /// What to do about this request, and what it would capture: the screen, the region in its
    /// own pixels, and the size that region was worked out against. A place of `None` is a
    /// refusal there is nothing to ask about.
    #[allow(clippy::type_complexity)]
    fn judge_screencopy(
        &mut self,
        client: &Client,
        output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
        region: Option<Rectangle<i32, smithay::utils::Logical>>,
    ) -> (
        Verdict,
        Option<(Output, Rectangle<i32, Physical>, Size<i32, Physical>)>,
    ) {
        let nowhere = (Verdict::No, None);
        let verdict = self.may_capture_screen(client);
        if verdict == Verdict::No {
            return nowhere;
        }
        let Some(output) = Output::from_resource(output) else {
            return nowhere;
        };
        let Some(size) = screen_size(&output) else {
            return nowhere;
        };
        // The region arrives in the screen's logical pixels and may be anywhere; the protocol
        // says to clip it to the screen rather than complain about it. Clipped again in the
        // screen's own pixels, since rounding at a fractional scale can reach one past its edge.
        let scale = output.current_scale().fractional_scale();
        let screen = Rectangle::from_size(size);
        let region = match region {
            None => screen,
            Some(asked) => {
                let whole = Rectangle::from_size(size.to_f64().to_logical(scale).to_i32_round());
                let clipped = asked.intersection(whole).and_then(|clipped| {
                    clipped
                        .to_f64()
                        .to_physical(scale)
                        .to_i32_round()
                        .intersection(screen)
                });
                match clipped {
                    Some(clipped) => clipped,
                    None => return nowhere,
                }
            }
        };
        match region.size.w > 0 && region.size.h > 0 {
            true => (verdict, Some((output, region, size))),
            false => nowhere,
        }
    }

    /// What to do about this client: allowed for good in the settings, allowed for this session,
    /// the portal (which asks its own way), or a program to put on the card.
    fn may_capture_screen(&mut self, client: &Client) -> Verdict {
        // Behind the lock there is nothing to give and nothing to ask: a card there would take
        // the first Enter after unlocking.
        if self.lock.is_some() {
            tracing::info!("a screen capture was refused: the screen is locked");
            return Verdict::No;
        }
        let named = client
            .get_data::<crate::state::ClientState>()
            .and_then(|state| state.asks_as.clone());
        let Some((program, exe)) = named else {
            // Nothing that can be named can be asked about, and nothing unnamed is allowed.
            tracing::info!("a screen capture was refused: the program couldn't be identified");
            return Verdict::No;
        };
        let path = exe.to_string_lossy().into_owned();
        if self.settings.privacy.allows_capture(&path) || self.screencopy.allowed_once(&exe) {
            return Verdict::Allowed;
        }
        // The portal has its own consent, through the share picker.
        if crate::capture::may_capture(client) {
            return Verdict::Allowed;
        }
        // One card at a time. A second program asking while one is up is refused rather than
        // queued: a queue of consent cards is a thing people click through.
        if self.share.is_some() && !self.screencopy.already_asking(&exe) {
            tracing::info!(
                program,
                "a screen capture was refused: already asking about another"
            );
            return Verdict::No;
        }
        Verdict::Ask { program, exe }
    }

    /// Fills every frame waiting on this screen from the pixels just drawn for it.
    /// `pixels` has the pointer drawn on it where the backend draws one; `without_cursor` is the
    /// same screen drawn again without it, for the programs that asked for none.
    pub fn serve_screencopy(
        &mut self,
        output: &Output,
        pixels: &[u8],
        without_cursor: Option<&[u8]>,
        size: Size<i32, Physical>,
        presented: std::time::Duration,
    ) {
        let mut left = Vec::new();
        for waiting in std::mem::take(&mut self.screencopy.waiting) {
            let Some(data) = waiting.frame.data::<FrameData>() else {
                continue;
            };
            if data.output.as_ref().and_then(WeakOutput::upgrade).as_ref() != Some(output) {
                left.push(waiting);
                continue;
            }
            // A program that asked for no pointer gets the bare draw where there is one; where
            // there isn't, the backend drew no pointer anyway.
            let from = match data.with_cursor {
                true => pixels,
                false => without_cursor.unwrap_or(pixels),
            };
            match copy_region_into(&waiting.buffer, from, size, data.region) {
                Ok(()) => {
                    // y_invert is not set: the renderer hands back the top row first.
                    waiting
                        .frame
                        .flags(zwlr_screencopy_frame_v1::Flags::empty());
                    if waiting.with_damage {
                        waiting.frame.damage(
                            0,
                            0,
                            data.region.size.w as u32,
                            data.region.size.h as u32,
                        );
                    }
                    let secs = presented.as_secs();
                    waiting
                        .frame
                        .ready((secs >> 32) as u32, secs as u32, presented.subsec_nanos());
                }
                Err(_) => waiting.frame.failed(),
            }
        }
        self.screencopy.waiting = left;
    }

    /// The screen locked: nothing waiting may be filled from a frame drawn before it went up.
    pub fn fail_screencopy_for_the_lock(&mut self) {
        let count = self.screencopy.waiting.len();
        for waiting in self.screencopy.waiting.drain(..) {
            waiting.frame.failed();
        }
        if count > 0 {
            tracing::info!(count, "screen captures failed: the screen locked");
        }
    }

    /// A screen has gone, or changed mode: anything waiting on it can't be filled.
    pub fn fail_screencopy_on(&mut self, output: &Output) {
        let mut left = Vec::new();
        for waiting in std::mem::take(&mut self.screencopy.waiting) {
            let follows = waiting
                .frame
                .data::<FrameData>()
                .and_then(|data| data.output.as_ref()?.upgrade());
            if follows.as_ref() == Some(output) || follows.is_none() {
                waiting.frame.failed();
            } else {
                left.push(waiting);
            }
        }
        self.screencopy.waiting = left;
    }
}

/// A screen's size in its own pixels, or `None` for one with no mode yet.
fn screen_size(output: &Output) -> Option<Size<i32, Physical>> {
    output.current_mode().map(|mode| mode.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowing_a_program_once_lists_it_once() {
        let mut screencopy = Screencopy::default();
        let grim = PathBuf::from("/usr/bin/grim");
        assert!(!screencopy.allowed_once(&grim));
        screencopy.allow_once(grim.clone());
        screencopy.allow_once(grim.clone());
        assert_eq!(screencopy.once, std::slice::from_ref(&grim));
        assert!(screencopy.allowed_once(&grim));
        // A program of the same name somewhere else is a different program.
        assert!(!screencopy.allowed_once(Path::new("/home/someone/bin/grim")));
    }
}

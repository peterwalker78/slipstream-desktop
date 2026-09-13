//! The screen-sharing picker: what an app sees when it asks to share the screen.
//!
//! The portal (`xdg-desktop-portal-wlr`) decides nothing itself. It runs a chooser program with
//! the sources the app will accept on its standard input, one per line — `Monitor: eDP-1 …` and
//! `Window: Title (identifier)` — and shares whichever line comes back, or nothing if none does.
//! The chooser is this binary run as `slipstream --choose-share`: it hands the lines to the
//! running compositor over a socket in the runtime directory and prints the answer, so the
//! question is asked on the desktop's own glass card, keyboard first, like everything else.
//!
//! **This card is the consent.** Nothing is shared until a row is chosen and Enter (or the
//! button) says so; Esc, or the card going away any other way, shares nothing. The portal's own
//! fallbacks either fail here (`slurp` needs a layer shell) or share a screen without asking, so
//! the config `scripts/update-session` writes always names this chooser.

use std::{
    cell::RefCell,
    io::{ErrorKind, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    reexports::calloop::{
        Interest, LoopHandle, Mode, PostAction,
        generic::Generic,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Size},
};

use crate::{
    Slipstream,
    card::{self, Btn, Button, Card, Hit, Row},
    motion::HYPR,
    paint::Painted,
    panel::MOCKUP_PX,
};

const OPEN: f64 = 0.18;
const REDUCED_FADE: f64 = 0.08;
/// Rows shown at once; the list scrolls with the selection past that.
const VISIBLE: usize = 9;
/// How long a chooser gets to send its whole list, counted from when it connects. It sends it all
/// at once before it waits, so only something that isn't the chooser takes this long.
const REQUEST_DEADLINE: Duration = Duration::from_millis(500);
/// The most a request may hold. The portal's list is a line per screen and window.
const REQUEST_LIMIT: usize = 1 << 20;

/// Something that can be shared, as the portal named it.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A whole screen, by its output name.
    Screen { name: String },
    /// A window, by its `ext-foreign-toplevel-list-v1` identifier.
    Window { identifier: String },
}

/// One of the portal's lines, understood.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The line exactly as the portal sent it: the answer has to match it byte for byte.
    pub line: String,
    pub source: Source,
}

/// Reads the portal's list. Lines that are neither a screen nor a window are left out rather
/// than offered as something nobody can name.
pub fn parse(list: &str) -> Vec<Choice> {
    list.lines()
        .filter_map(|line| {
            let source = if let Some(rest) = line.strip_prefix("Monitor: ") {
                let name = rest.split_whitespace().next()?.to_string();
                Source::Screen { name }
            } else {
                let rest = line.strip_prefix("Window: ")?;
                // The title can hold brackets of its own; the identifier is in the last pair.
                let open = rest.rfind('(')?;
                let identifier = rest[open + 1..].strip_suffix(')')?.to_string();
                if identifier.is_empty() {
                    return None;
                }
                Source::Window { identifier }
            };
            Some(Choice {
                line: line.to_string(),
                source,
            })
        })
        .collect()
}

/// What the answer came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Share the selected row.
    Share,
    /// Share nothing.
    Cancel,
}

/// The card, while an app waits for an answer.
pub struct Picker {
    choices: Vec<Choice>,
    /// What each row says: the name, and what it is.
    rows: Vec<Row>,
    selected: usize,
    /// The first row in view.
    top: usize,
    /// The chooser waiting on the other end, which the answer goes back down.
    reply: Option<UnixStream>,
    since: f64,
    pub reduced_motion: bool,
    hovered: Option<Button>,
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Card, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
}

impl Picker {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn new(
        choices: Vec<Choice>,
        rows: Vec<Row>,
        reply: Option<UnixStream>,
        now: f64,
        reduced_motion: bool,
    ) -> Self {
        Self {
            choices,
            rows,
            selected: 0,
            top: 0,
            reply,
            since: now,
            reduced_motion,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }

    pub fn key(&mut self, sym: Keysym, shift: bool) -> Act {
        match sym {
            // Enter only, never Space: the picker appears by itself, and a space typed into a chat
            // just as it does must not share the screen.
            Keysym::Return | Keysym::KP_Enter => Act::Share,
            Keysym::Escape => Act::Cancel,
            Keysym::Down => self.step(1),
            Keysym::Up => self.step(-1),
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                self.step(if shift || sym == Keysym::ISO_Left_Tab {
                    -1
                } else {
                    1
                })
            }
            Keysym::Home => self.select(0),
            Keysym::End => self.select(self.choices.len().saturating_sub(1)),
            _ => Act::Nothing,
        }
    }

    fn step(&mut self, by: isize) -> Act {
        let count = self.choices.len() as isize;
        if count > 0 {
            self.select((self.selected as isize + by).rem_euclid(count) as usize);
        }
        Act::Nothing
    }

    fn select(&mut self, index: usize) -> Act {
        self.selected = index.min(self.choices.len().saturating_sub(1));
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + VISIBLE {
            self.top = self.selected + 1 - VISIBLE;
        }
        Act::Nothing
    }

    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        let over = self.hit(x, y);
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// A click on a row chooses it; a click on a chosen row, or on Share, shares it. Beside the
    /// card nothing happens, as on the other cards that ask a question.
    pub fn click(&mut self, x: f64, y: f64) -> Act {
        match self.hit(x, y) {
            Some(Button::Go) => Act::Share,
            Some(Button::Stay) => Act::Cancel,
            Some(Button::Row(row)) => {
                let index = self.top + row;
                if index == self.selected {
                    Act::Share
                } else {
                    self.select(index)
                }
            }
            _ => Act::Nothing,
        }
    }

    fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let px = MOCKUP_PX as f64;
        let (lx, ly) = (((x - self.at.x) / px) as f32, ((y - self.at.y) / px) as f32);
        self.hits
            .iter()
            .find(|hit| hit.contains(lx, ly))
            .map(|hit| hit.which)
    }

    /// The line the portal will recognise for the selected row.
    pub fn chosen(&self) -> Option<&Choice> {
        self.choices.get(self.selected)
    }

    /// Sends the answer down to the chooser, or nothing, which the portal reads as "declined".
    pub fn answer(mut self, share: bool) {
        let line = if share {
            self.chosen().map(|choice| format!("{}\n", choice.line))
        } else {
            None
        };
        if let Some(mut reply) = self.reply.take() {
            if let Some(line) = line {
                // A chooser that has gone away can't be told; the portal it belonged to has
                // already given up on it.
                let _ = reply.write_all(line.as_bytes());
            }
        }
    }

    fn card(&self, ring: u32) -> Card {
        let windows = self
            .choices
            .iter()
            .any(|choice| matches!(choice.source, Source::Window { .. }));
        let screens = self
            .choices
            .iter()
            .any(|choice| matches!(choice.source, Source::Screen { .. }));
        let what = match (screens, windows) {
            (true, true) => "a screen or a window",
            (false, true) => "a window",
            _ => "your screen",
        };
        let end = (self.top + VISIBLE).min(self.rows.len());
        let hidden = self.rows.len() - (end - self.top);
        Card {
            title: "Share your screen".to_string(),
            note: format!(
                "An app is asking to see {what}. Choose what it sees: nothing is shared until you \
                 do, and the red dot on the bar stops it."
            ),
            rows: self.rows[self.top..end].to_vec(),
            more: (hidden > 0).then(|| format!("{hidden} more — arrow keys scroll")),
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Cancel".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: "Share".to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.hovered,
            selected: Some((self.selected - self.top, ring)),
        }
    }

    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        ring: u32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let card = self.card(ring);
        let fresh = self
            .shown
            .as_ref()
            .is_some_and(|(shown, size, at)| *shown == card && *size == screen && *at == scale);
        if !fresh {
            let (painted, hits) = card::paint(&card, scale);
            self.hits = hits;
            self.painted = painted;
            self.shown = Some((card, screen, scale));
        }
        let painted = self.painted.as_ref()?;
        self.at = Point::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        let since = now - self.since;
        let (alpha, rise) = if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
        } else {
            let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
            (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
        };
        let at = self.at + Point::from((0.0, rise * MOCKUP_PX as f64));
        painted.element(renderer, at, alpha)
    }
}

/// Where the compositor listens for the chooser: one socket per Wayland display, so a nested
/// Slipstream never answers for the desktop it runs in.
fn socket_path(display: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("slipstream");
    Some(dir.join(format!("share-{display}")))
}

/// Starts listening for the chooser.
pub fn listen(handle: &LoopHandle<'static, Slipstream>, display: &str) {
    let Some(path) = socket_path(display) else {
        tracing::warn!("no XDG_RUNTIME_DIR, so screen sharing can't ask which screen");
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // A socket left by a Slipstream that died on this display: nothing is listening on it.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::warn!(path = %path.display(), "can't listen for the share picker: {err}");
            return;
        }
    };
    if let Err(err) = listener.set_nonblocking(true) {
        tracing::warn!("can't listen for the share picker: {err}");
        return;
    }
    let inserted = handle.insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        |_, listener, state| {
            while let Ok((stream, _)) = listener.accept() {
                state.share_asked(stream);
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(err) = inserted {
        tracing::warn!("can't listen for the share picker: {}", err.error);
    }
}

/// How far a request has got.
#[derive(Debug, PartialEq)]
enum Progress {
    /// More is to come.
    Waiting,
    /// The chooser shut its side down: this is the whole list.
    Complete(String),
    Refused(String),
}

/// Reads whatever `stream` has for now onto `bytes`, without waiting for more.
fn read_available(mut stream: impl Read, bytes: &mut Vec<u8>) -> Progress {
    let mut chunk = [0; 16 * 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Progress::Complete(String::from_utf8_lossy(bytes).into_owned()),
            Ok(read) if bytes.len() + read > REQUEST_LIMIT => {
                return Progress::Refused("the list is bigger than 1 MiB".into());
            }
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Progress::Waiting,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Progress::Refused(err.to_string()),
        }
    }
}

/// What a chooser's request came to: its list and the stream to answer on, or why it was refused.
pub type Request = Result<(String, UnixStream), String>;

/// Reads a chooser's request on the event loop as it arrives, never waiting on the socket, and
/// calls `done` once: with the whole list when the chooser shuts its side down, or with a refusal
/// when it sends more than `REQUEST_LIMIT` or is still sending at `REQUEST_DEADLINE`. A refused
/// chooser's socket closes, which its portal reads as nothing chosen.
pub fn read_request<D: 'static>(
    handle: &LoopHandle<'static, D>,
    stream: UnixStream,
    done: impl FnOnce(&mut D, Request) + 'static,
) {
    let reply = match stream
        .set_nonblocking(true)
        .and_then(|()| stream.try_clone())
    {
        Ok(reply) => reply,
        Err(err) => {
            tracing::warn!("the share picker's request couldn't be read: {err}");
            return;
        }
    };
    // Whichever comes first, the end of the list or the deadline, answers; the other finds
    // nothing left to do.
    let waiting = Rc::new(RefCell::new(Some((done, reply))));
    let reader = waiting.clone();
    let mut bytes = Vec::new();
    let inserted = handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, data| {
            let result = match read_available(&**stream, &mut bytes) {
                Progress::Waiting => return Ok(PostAction::Continue),
                Progress::Complete(list) => Ok(list),
                Progress::Refused(why) => Err(why),
            };
            let taken = reader.borrow_mut().take();
            if let Some((done, reply)) = taken {
                done(data, result.map(|list| (list, reply)));
            }
            Ok(PostAction::Remove)
        },
    );
    let token = match inserted {
        Ok(token) => token,
        Err(err) => {
            tracing::warn!("the share picker's request couldn't be read: {}", err.error);
            return;
        }
    };
    let loop_handle = handle.clone();
    let timer = handle.insert_source(Timer::from_duration(REQUEST_DEADLINE), move |_, _, data| {
        let taken = waiting.borrow_mut().take();
        if let Some((done, _reply)) = taken {
            loop_handle.remove(token);
            done(
                data,
                Err(format!(
                    "still sending after {} ms",
                    REQUEST_DEADLINE.as_millis()
                )),
            );
        }
        TimeoutAction::Drop
    });
    if timer.is_err() {
        // Without a deadline a silent client would hold its socket open for good.
        handle.remove(token);
    }
}

/// `slipstream --choose-share`: the portal's chooser. Passes the portal's list to the running
/// compositor and prints what was chosen. Printing nothing is a refusal, which is also the answer
/// when there is no Slipstream to ask.
pub fn choose() -> i32 {
    let mut list = String::new();
    let _ = std::io::stdin().read_to_string(&mut list);
    let Some(path) = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .and_then(|display| socket_path(&display))
    else {
        eprintln!("slipstream --choose-share: no WAYLAND_DISPLAY or XDG_RUNTIME_DIR");
        return 0;
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("slipstream --choose-share: {}: {err}", path.display());
            return 0;
        }
    };
    let sent = stream
        .write_all(list.as_bytes())
        .and_then(|()| stream.shutdown(std::net::Shutdown::Write));
    if let Err(err) = sent {
        eprintln!("slipstream --choose-share: {err}");
        return 0;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    print!("{answer}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(count: usize) -> Picker {
        let choices = (0..count)
            .map(|n| Choice {
                line: format!("Window: Title {n} (id{n})"),
                source: Source::Window {
                    identifier: format!("id{n}"),
                },
            })
            .collect::<Vec<_>>();
        let rows = choices
            .iter()
            .map(|choice| Row {
                name: choice.line.clone(),
                title: String::new(),
                note: None,
            })
            .collect();
        Picker::new(choices, rows, None, 0.0, false)
    }

    #[test]
    fn the_portals_lines_are_understood() {
        let choices = parse(
            "Monitor: eDP-1 BOE 0x0CB4 (eDP-1)\nWindow: notes (draft) - Kate (a1B2c3)\nnonsense\n",
        );
        assert_eq!(
            choices[0].source,
            Source::Screen {
                name: "eDP-1".into()
            }
        );
        // Brackets in a title don't fool it: the identifier is in the last pair.
        assert_eq!(
            choices[1].source,
            Source::Window {
                identifier: "a1B2c3".into()
            }
        );
        assert_eq!(choices[1].line, "Window: notes (draft) - Kate (a1B2c3)");
        assert_eq!(choices.len(), 2, "a line that names nothing isn't offered");
    }

    #[test]
    fn arrows_wrap_and_the_list_scrolls_with_them() {
        let mut picker = picker(12);
        assert_eq!(picker.key(Keysym::Up, false), Act::Nothing);
        assert_eq!(picker.selected, 11, "up from the first wraps to the last");
        assert_eq!(picker.top, 12 - VISIBLE, "and the last is in view");
        picker.key(Keysym::Down, false);
        assert_eq!((picker.selected, picker.top), (0, 0));
        assert_eq!(picker.key(Keysym::Return, false), Act::Share);
        assert_eq!(picker.key(Keysym::Escape, false), Act::Cancel);
    }

    #[test]
    fn the_answer_is_the_portals_own_line_or_nothing() {
        for (share, expected) in [(true, "Window: Title 1 (id1)\n"), (false, "")] {
            let (ours, mut theirs) = UnixStream::pair().expect("a socket pair");
            let mut picker = picker(3);
            picker.reply = Some(ours);
            picker.key(Keysym::Down, false);
            picker.answer(share);
            let mut got = String::new();
            theirs.read_to_string(&mut got).unwrap();
            assert_eq!(got, expected);
        }
    }

    /// Runs `read_request` on an event loop of its own until it answers, and says when it did.
    fn request_from(client: impl FnOnce(UnixStream) + Send + 'static) -> (Request, Duration) {
        use smithay::reexports::calloop::EventLoop;
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        let writer = std::thread::spawn(move || client(theirs));
        let mut event_loop: EventLoop<'static, Option<Request>> = EventLoop::try_new().unwrap();
        let started = std::time::Instant::now();
        read_request(&event_loop.handle(), ours, |answer, request| {
            *answer = Some(request)
        });
        let mut answer = None;
        while answer.is_none() && started.elapsed() < Duration::from_secs(3) {
            event_loop
                .dispatch(Some(Duration::from_millis(10)), &mut answer)
                .unwrap();
        }
        let took = started.elapsed();
        drop(event_loop);
        writer.join().unwrap();
        (answer.expect("an answer within 3 s"), took)
    }

    #[test]
    fn a_slow_writer_is_dropped() {
        let (request, took) = request_from(|mut stream| {
            // A byte every 99 ms, until the socket is closed on it.
            while stream.write_all(b"M").is_ok() {
                std::thread::sleep(Duration::from_millis(99));
            }
        });
        assert!(request.is_err(), "refused");
        assert!(
            (Duration::from_millis(450)..=Duration::from_millis(550)).contains(&took),
            "refused after {took:?}"
        );

        let list = "Monitor: eDP-1 BOE 0x0CB4 (eDP-1)\nWindow: notes - Kate (a1B2c3)\n";
        let (request, took) = request_from(move |mut stream| {
            stream.write_all(list.as_bytes()).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
        });
        let (got, _reply) = request.expect("the whole list is read");
        assert_eq!(got, list);
        assert!(took < Duration::from_millis(400), "{took:?}");

        let (request, _) = request_from(|mut stream| {
            let _ = stream.write_all(&vec![b'x'; REQUEST_LIMIT + 1]);
            let _ = stream.shutdown(std::net::Shutdown::Write);
        });
        assert!(request.is_err(), "more than a MiB is refused");
    }

    #[test]
    fn a_click_beside_the_card_shares_nothing() {
        assert_eq!(picker(2).click(3.0, 3.0), Act::Nothing);
    }
}

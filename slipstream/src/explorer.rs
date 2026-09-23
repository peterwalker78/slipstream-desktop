//! The app explorer on a tapped Super. Type to search the installed apps, arrow keys to choose, Enter
//! to open, Esc to close. While it's open it takes every key, so nothing reaches the focused app.
//!
//! Its side column lists recent files and a log-out action. With a query that matches no app,
//! Enter runs the query as a command, as Windows' Run dialog does.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    apps::{self, App, Icons},
    icons,
    keys::Mods,
    motion::HYPR,
    paint::{self, Painter},
    panel,
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
const WIDTH: f32 = 1040.0;
const TOP: f32 = 130.0;
const SEARCH_H: f32 = 85.0;
const BODY_H: f32 = 440.0;
const FOOT_H: f32 = 44.0;
const HEIGHT: f32 = SEARCH_H + 1.0 + BODY_H + FOOT_H;
/// Room around the frame for its shadow.
const MARGIN: f32 = 64.0;
const COLUMNS: usize = 4;
const ROWS: usize = 3;
/// How many rows PgUp and PgDn move.
const PAGE_ROWS: usize = 3;
const TILE_H: f32 = 128.0;
/// The most icons kept rasterised. Notifications can name any icon they like, so the cache starts
/// again when it fills rather than keeping every name ever sent.
const ICON_CACHE: usize = 256;
const TILE_GAP: f32 = 6.0;
const ICON: f32 = 66.0;
const ROW_H: f32 = 42.0;
/// Logical pixels per mockup pixel: the mockup is drawn for the laptop's 1.25×.
const MOCKUP_PX: f32 = 0.8;
/// Opening: 180 ms from 10 px higher and transparent. Closing is instant.
const OPEN: f64 = 0.18;
const REDUCED_FADE: f64 = 0.08;

/// Something the explorer can open.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    App(App),
    File(PathBuf),
    /// A command typed into the search box.
    Run(String),
    LogOut,
    /// The shortcut sheet.
    Shortcuts,
    /// A sum or a conversion worked out from the query: Enter copies it.
    Answer(crate::calc::Answer),
    /// An emoji found by a `:` query, and its name: Enter types it.
    Emoji(&'static str, &'static str),
}

pub enum Outcome {
    Nothing,
    Close,
    /// Open this, and whether to start a new instance of an app even if it's already open.
    Activate(Item, bool),
}

#[derive(Default)]
struct Catalog {
    apps: Vec<App>,
    icons: Icons,
    /// Recently used files that still exist, newest first.
    recent: Vec<PathBuf>,
    /// The recent files are being read on a thread of their own.
    reading_recent: bool,
    /// They were asked for again while being read, so that thread reads them once more.
    recent_stale: bool,
    /// Goes up with every scan, and whenever the recent files change, so the explorer repaints.
    version: u64,
    /// The apps' icons, rasterised on a thread of their own at the grid's size, by icon name.
    /// `None` for a name no theme has.
    tile_icons: HashMap<String, Option<Pixmap>>,
    /// The size they're rasterised at, in device pixels.
    tile_px: u32,
    /// Goes up as they land, so an open explorer repaints with them.
    tile_icons_landed: u64,
    /// The icons are being rasterised, and were asked for again meanwhile.
    warming: bool,
    warm_stale: bool,
    /// A Run query that starts with a path, and whether that path is a program: looked at on a
    /// thread, since the path can be on a mount that doesn't answer.
    run_program: Option<(String, bool)>,
}

/// How many emoji a `:` query lists.
const EMOJI_SHOWN: usize = 6;
/// How long an answer takes to decode when it changes.
const ANSWER_DECODE: f64 = 0.32;

/// How many recent files are kept for the side column and the search.
const RECENT: usize = 24;

/// Reads the recent files: the list on disk in the session, a stand-in in tests.
type RecentReader = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

struct Results {
    apps: Vec<App>,
    answer: Option<crate::calc::Answer>,
    emoji: Vec<(&'static str, &'static str)>,
    run: Option<String>,
    files: Vec<PathBuf>,
    log_out: bool,
    shortcuts: bool,
}

impl Results {
    /// Everything selectable, in selection order.
    fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self.apps.iter().cloned().map(Item::App).collect();
        items.extend(self.answer.clone().map(Item::Answer));
        items.extend(
            self.emoji
                .iter()
                .map(|(emoji, name)| Item::Emoji(emoji, name)),
        );
        items.extend(self.run.clone().map(Item::Run));
        items.extend(self.files.iter().cloned().map(Item::File));
        if self.log_out {
            items.push(Item::LogOut);
        }
        if self.shortcuts {
            items.push(Item::Shortcuts);
        }
        items
    }
}

/// The painted explorer, its size in logical pixels, and its size in buffer pixels.
type Painted = (MemoryRenderBuffer, Size<i32, Logical>, (i32, i32));

/// The caret's size in mockup pixels. It's an element of its own, so blinking never repaints the
/// explorer.
const CARET: (f32, f32) = (3.0, 29.7);
const CARET_COLOUR: u32 = 0xffb547ff;

/// Everything the painted explorer depends on.
#[derive(Clone, PartialEq)]
struct Look {
    query: String,
    selected: usize,
    scroll: usize,
    version: u64,
    icons_landed: u64,
    running: Vec<String>,
    /// The keyboard's ring colour, which marks the selection.
    ring: u32,
    width: i32,
    scale: f64,
    /// How far the answer's decode is, in thirtieths of a second, while it runs.
    decode_step: Option<u64>,
}

pub struct Explorer {
    open: bool,
    query: String,
    selected: usize,
    /// The first row of apps showing.
    scroll: usize,
    /// When it opened, on the animation clock.
    opened_at: f64,
    catalog: Arc<Mutex<Catalog>>,
    scanned: Option<Instant>,
    read_recent_files: RecentReader,
    /// Icons rasterised so far, by name and size in device pixels.
    icon_cache: HashMap<(String, u32), Option<Pixmap>>,
    shown: Option<Look>,
    buffer: Option<Painted>,
    /// The caret, painted for a scale, and its corner in logical pixels from the painted area's.
    caret: Option<(f64, MemoryRenderBuffer, Size<i32, Logical>)>,
    caret_at: Point<f64, Logical>,
    /// The catalog version and icon size last asked to be warmed.
    warmed: Option<(u64, u32)>,
    /// The last Run query whose first word was sent to be looked at.
    run_asked: Option<String>,
    /// The painted area's corner and the frame, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Item, Rectangle<f64, Logical>)>,
    pub reduced_motion: bool,
    /// The answer last shown, and when it changed, so a new one decodes into place.
    answer_since: Option<(String, f64)>,
    /// How long the shown answer has been decoding, for the paint.
    answer_age: f64,
}

impl Explorer {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn new(reduced_motion: bool) -> Self {
        let mut explorer = Self::idle(reduced_motion);
        // Read the apps and the recent files now, so the first tap of Super finds them.
        explorer.rescan();
        explorer.read_recent();
        explorer
    }

    /// Closed, and with no apps read yet.
    fn idle(reduced_motion: bool) -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            scroll: 0,
            opened_at: 0.0,
            catalog: Arc::default(),
            scanned: None,
            read_recent_files: Arc::new(|| apps::recent_files(RECENT, |path| path.is_file())),
            icon_cache: HashMap::new(),
            shown: None,
            buffer: None,
            caret: None,
            caret_at: Point::default(),
            warmed: None,
            run_asked: None,
            origin: Point::default(),
            frame: Rectangle::from_size((0.0, 0.0).into()),
            targets: Vec::new(),
            reduced_motion,
            answer_since: None,
            answer_age: f64::MAX,
        }
    }

    /// Whether the installed apps have been read yet. They are scanned on another thread at
    /// startup, so anything that needs to match an app id waits for this.
    pub fn apps_read(&self) -> bool {
        self.catalog.lock().unwrap().version > 0
    }

    /// The desktop entry for this app ID or X11 class. The whole entry, because putting a layout
    /// back needs its `Exec` line as well as its name.
    pub fn app_for(&self, id: &str) -> Option<App> {
        self.catalog
            .lock()
            .unwrap()
            .apps
            .iter()
            .find(|app| {
                app.id.eq_ignore_ascii_case(id)
                    || app
                        .wm_class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(id))
            })
            .cloned()
    }

    /// The name of the app with this app ID or X11 class, from its desktop entry.
    pub fn app_name(&self, id: &str) -> Option<String> {
        self.app_for(id).map(|app| app.name)
    }

    /// The icon of the app with this app ID or X11 class, `px` pixels square.
    pub fn app_icon(&mut self, id: &str, px: u32) -> Option<Pixmap> {
        let app = self
            .catalog
            .lock()
            .unwrap()
            .apps
            .iter()
            .find(|app| {
                app.id.eq_ignore_ascii_case(id)
                    || app
                        .wm_class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(id))
            })
            .cloned()?;
        self.icon(&app, px)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, now: f64) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.scroll = 0;
        self.opened_at = now;
        self.rescan();
        // What was last read shows now. A file deleted since goes from the list once this read
        // is back, and a hung mount keeps the last list up rather than the explorer shut.
        self.read_recent();
    }

    /// Reads the recent files again on a thread of their own, never waiting for it: checking
    /// that each still exists can take as long as a hung network mount. A read already under way
    /// goes round once more when it finishes, so the newest list always lands last.
    pub fn read_recent(&self) {
        {
            let mut catalog = self.catalog.lock().unwrap();
            if catalog.reading_recent {
                catalog.recent_stale = true;
                return;
            }
            catalog.reading_recent = true;
        }
        let catalog = self.catalog.clone();
        let read = self.read_recent_files.clone();
        std::thread::spawn(move || {
            loop {
                let files = read();
                let mut catalog = catalog.lock().unwrap();
                if catalog.recent != files {
                    catalog.recent = files;
                    catalog.version += 1;
                }
                if !std::mem::take(&mut catalog.recent_stale) {
                    catalog.reading_recent = false;
                    return;
                }
            }
        });
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = None;
        self.buffer = None;
        self.targets.clear();
    }

    /// Apps were installed, removed or updated: read them again now, with fresh icons.
    pub fn rescan_now(&mut self) {
        self.scanned = None;
        self.icon_cache.clear();
        {
            let mut catalog = self.catalog.lock().unwrap();
            catalog.tile_icons.clear();
            catalog.tile_icons_landed += 1;
        }
        self.warmed = None;
        self.rescan();
    }

    /// Rasterises every app's icon at the grid's size for a screen at `scale`, on a thread of its
    /// own, so neither opening the explorer nor typing into it waits for SVGs. Cheap to call every
    /// frame: it only starts work when the apps or the size have changed.
    pub fn warm_icons(&mut self, scale: f64) {
        let px = (ICON * scale as f32 * MOCKUP_PX).round() as u32;
        let version = self.catalog.lock().unwrap().version;
        if version == 0 || self.warmed == Some((version, px)) {
            return;
        }
        self.warmed = Some((version, px));
        {
            let mut catalog = self.catalog.lock().unwrap();
            if catalog.tile_px != px {
                catalog.tile_icons.clear();
                catalog.tile_px = px;
            }
            if catalog.warming {
                catalog.warm_stale = true;
                return;
            }
            catalog.warming = true;
        }
        let catalog = self.catalog.clone();
        std::thread::spawn(move || {
            loop {
                // What's still to do, in the order the grid shows the apps, so the first page
                // lands first. The paths are found under the lock, the pixels drawn outside it.
                let (px, todo): (u32, Vec<(String, Option<PathBuf>)>) = {
                    let catalog = catalog.lock().unwrap();
                    let mut seen = std::collections::HashSet::new();
                    let todo = apps::search(&catalog.apps, "")
                        .into_iter()
                        .filter_map(|app| app.icon.clone())
                        .filter(|name| !catalog.tile_icons.contains_key(name))
                        .filter(|name| seen.insert(name.clone()))
                        .map(|name| {
                            let path = catalog.icons.find(&name, catalog.tile_px);
                            (name, path)
                        })
                        .collect();
                    (catalog.tile_px, todo)
                };
                for batch in todo.chunks(ROWS * COLUMNS) {
                    let drawn: Vec<(String, Option<Pixmap>)> = batch
                        .iter()
                        .map(|(name, path)| {
                            let pixmap = path.as_ref().and_then(|path| paint::load_image(path, px));
                            (name.clone(), pixmap)
                        })
                        .collect();
                    let mut catalog = catalog.lock().unwrap();
                    if catalog.tile_px != px {
                        break;
                    }
                    catalog.tile_icons.extend(drawn);
                    catalog.tile_icons_landed += 1;
                }
                let mut catalog = catalog.lock().unwrap();
                if !std::mem::take(&mut catalog.warm_stale) {
                    catalog.warming = false;
                    return;
                }
            }
        });
    }

    /// An app's icon for the grid: drawn, known to be missing (`Some(None)`), or not rasterised
    /// yet (`None`).
    fn tile_icon(&self, app: &App, px: u32) -> Option<Option<Pixmap>> {
        let Some(name) = &app.icon else {
            return Some(None);
        };
        let catalog = self.catalog.lock().unwrap();
        if catalog.tile_px != px {
            return None;
        }
        catalog.tile_icons.get(name).cloned()
    }

    /// Reads the installed apps and icon themes again in the background, at most every 30 s.
    fn rescan(&mut self) {
        if self
            .scanned
            .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        self.scanned = Some(Instant::now());
        let catalog = self.catalog.clone();
        std::thread::spawn(move || {
            let (apps, icons) = (apps::scan(), Icons::scan());
            tracing::info!(apps = apps.len(), "read the installed apps");
            let mut catalog = catalog.lock().unwrap();
            catalog.apps = apps;
            catalog.icons = icons;
            catalog.version += 1;
        });
    }

    fn results(&self) -> Results {
        // `:` and a word looks for emoji and nothing else.
        if let Some(words) = self.query.trim_start().strip_prefix(':') {
            return Results {
                apps: Vec::new(),
                answer: None,
                emoji: crate::emoji::search(words, EMOJI_SHOWN),
                run: None,
                files: Vec::new(),
                log_out: false,
                shortcuts: false,
            };
        }
        let answer = crate::calc::answer(&self.query);
        let query = self.query.trim().to_lowercase();
        let catalog = self.catalog.lock().unwrap();
        let apps: Vec<App> = apps::search(&catalog.apps, &query)
            .into_iter()
            .cloned()
            .collect();
        // What is typed is offered as a command only as the last resort, once no app and no sum
        // has matched.
        let run = (!query.is_empty() && apps.is_empty() && answer.is_none())
            .then(|| self.query.trim().to_string());
        let files = if query.is_empty() {
            catalog.recent.iter().take(3).cloned().collect()
        } else {
            catalog
                .recent
                .iter()
                .filter(|path| file_name(path).to_lowercase().contains(&query))
                .take(4)
                .cloned()
                .collect()
        };
        Results {
            apps,
            answer,
            emoji: Vec::new(),
            run,
            files,
            log_out: "log out".contains(&query) || "logout".contains(&query),
            // Asked for by name, never shown on an empty query: the hint card teaches Super+/.
            shortcuts: !query.is_empty()
                && ["keyboard shortcuts", "keys", "help"]
                    .iter()
                    .any(|words| words.contains(&query)),
        }
    }

    /// Back to the row Enter lands on for what is typed now: the first one.
    fn reset_selection(&mut self) {
        self.selected = 0;
    }

    /// A key while the explorer is open, with the modifiers held. `ch` is the character it types,
    /// if any; nothing types while a chord is held.
    pub fn key(&mut self, sym: Keysym, ch: Option<char>, mods: Mods) -> Outcome {
        let results = self.results();
        let apps = results.apps.len();
        let total = results.items().len();
        let at = self.selected;
        match sym {
            // Esc backs out one level: a query first, then the explorer.
            Keysym::Escape if !self.query.is_empty() => {
                self.query.clear();
                self.reset_selection();
                self.scroll = 0;
                return Outcome::Nothing;
            }
            Keysym::Escape => return Outcome::Close,
            Keysym::Return | Keysym::KP_Enter => {
                return results
                    .items()
                    .into_iter()
                    .nth(self.selected)
                    .map_or(Outcome::Nothing, |item| Outcome::Activate(item, mods.shift));
            }
            // Ctrl+Backspace and Ctrl+U clear the query, as in a shell or a browser's address bar.
            Keysym::BackSpace | Keysym::u if mods.ctrl => {
                self.query.clear();
                self.reset_selection();
            }
            Keysym::BackSpace => {
                self.query.pop();
                self.reset_selection();
            }
            Keysym::Left => self.selected = at.saturating_sub(1),
            Keysym::Right => self.selected += 1,
            // Tab walks everything in order, as → and ← do, and wraps.
            Keysym::Tab | Keysym::ISO_Left_Tab if total > 0 => {
                self.selected = if mods.shift || sym == Keysym::ISO_Left_Tab {
                    (at + total - 1) % total
                } else {
                    (at + 1) % total
                };
            }
            Keysym::Home => self.selected = 0,
            Keysym::End => self.selected = total.saturating_sub(1),
            // The mockup's grid moves: down a row, or from the last row to the side column. With
            // nothing below, the selection stays rather than jumping sideways.
            Keysym::Down => {
                let to = if at < apps && at + COLUMNS < apps {
                    at + COLUMNS
                } else if at < apps {
                    apps
                } else {
                    at + 1
                };
                if to < total {
                    self.selected = to;
                }
            }
            Keysym::Up => {
                let step = if at < apps { COLUMNS } else { 1 };
                self.selected = at.saturating_sub(step);
            }
            // PgUp and PgDn move three rows, within the grid or within the side column.
            Keysym::Prior | Keysym::Next => {
                let down = sym == Keysym::Next;
                self.selected = if at < apps {
                    let step = PAGE_ROWS * COLUMNS;
                    if down {
                        (at + step).min(apps - 1)
                    } else {
                        at.saturating_sub(step)
                    }
                } else if down {
                    (at + PAGE_ROWS).min(total.saturating_sub(1))
                } else {
                    at.saturating_sub(PAGE_ROWS).max(apps)
                };
            }
            _ => match ch {
                Some(ch) if !ch.is_control() => {
                    self.query.push(ch);
                    self.reset_selection();
                }
                _ => return Outcome::Nothing,
            },
        }
        let apps = self.results().apps.len();
        let total = if matches!(sym, Keysym::BackSpace | Keysym::u) || ch.is_some() {
            self.results().items().len()
        } else {
            total
        };
        self.selected = self.selected.min(total.saturating_sub(1));
        // Keep the selected app's row in view.
        if self.selected < apps {
            let row = self.selected / COLUMNS;
            if row < self.scroll {
                self.scroll = row;
            } else if row >= self.scroll + ROWS {
                self.scroll = row + 1 - ROWS;
            }
        }
        self.scroll = self.scroll.min(apps.div_ceil(COLUMNS).saturating_sub(ROWS));
        Outcome::Nothing
    }

    /// A click, in logical pixels from the output's corner. A middle click starts a new instance
    /// of an app that's already open.
    pub fn click(&self, x: f64, y: f64, middle: bool) -> Outcome {
        if let Some((item, _)) = self.targets.iter().find(|(_, area)| area.contains((x, y))) {
            Outcome::Activate(item.clone(), middle)
        } else if self.frame.contains((x, y)) {
            Outcome::Nothing
        } else {
            Outcome::Close
        }
    }

    /// The explorer for a screen `width` logical pixels wide at `scale`, repainted when what it
    /// shows changes. `running` holds the app IDs and X11 classes of open windows, lowercased.
    /// The explorer's elements, front first: the caret, then the painted explorer.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
        running: Vec<String>,
        ring: u32,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if !self.open {
            return Vec::new();
        }
        self.warm_icons(scale);
        let since = now - self.opened_at;
        // A new answer decodes out of rain glyphs, on wall time.
        let answer = self.results().answer.map(|answer| answer.value);
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        match (&answer, &self.answer_since) {
            (Some(value), Some((shown, _))) if value == shown => {}
            (Some(value), _) => self.answer_since = Some((value.clone(), wall)),
            (None, _) => self.answer_since = None,
        }
        self.answer_age = self
            .answer_since
            .as_ref()
            .map_or(f64::MAX, |(_, at)| wall - at);
        let decode_step = (!self.reduced_motion && self.answer_age < ANSWER_DECODE)
            .then_some((self.answer_age * 30.0) as u64);
        let look = {
            let catalog = self.catalog.lock().unwrap();
            Look {
                query: self.query.clone(),
                selected: self.selected,
                scroll: self.scroll,
                version: catalog.version,
                icons_landed: catalog.tile_icons_landed,
                running,
                ring,
                width,
                scale,
                decode_step,
            }
        };
        if self.shown.as_ref() != Some(&look) {
            if self.repaint(&look).is_none() {
                return Vec::new();
            }
            self.shown = Some(look);
        }
        let (alpha, rise) = if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0), 0.0)
        } else {
            let eased = HYPR.at(since / OPEN);
            (eased.clamp(0.0, 1.0), -10.0 * (1.0 - eased))
        };
        let Some((buffer, logical, device)) = self.buffer.as_ref() else {
            return Vec::new();
        };
        // Whole screen pixels, or the text blurs.
        let location =
            Point::<f64, Logical>::from((self.origin.x, self.origin.y + rise * MOCKUP_PX as f64))
                .to_physical(scale)
                .to_i32_round::<i32>();
        let mut elements = Vec::with_capacity(2);
        // The caret blinks, half a second on and half off, unless motion is reduced: then it
        // stays on.
        let caret_on = self.reduced_motion || (since * 2.0).rem_euclid(2.0) < 1.0;
        if caret_on {
            if self.caret.as_ref().is_none_or(|(at, _, _)| *at != scale) {
                self.caret = caret_buffer(scale);
            }
            if let Some((_, caret, size)) = &self.caret {
                let at = location + self.caret_at.to_physical(scale).to_i32_round::<i32>();
                elements.extend(
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        at.to_f64(),
                        caret,
                        Some(alpha as f32),
                        None,
                        Some(*size),
                        Kind::Unspecified,
                    )
                    .ok(),
                );
            }
        }
        elements.extend(
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                buffer,
                Some(alpha as f32),
                Some(Rectangle::from_size(
                    (device.0 as f64, device.1 as f64).into(),
                )),
                Some(*logical),
                Kind::Unspecified,
            )
            .ok(),
        );
        elements
    }

    fn icon(&mut self, app: &App, px: u32) -> Option<Pixmap> {
        let name = app.icon.clone()?;
        self.named_icon(&name, px)
    }

    /// An icon from the icon themes by its name (or a file's path), `px` pixels square.
    pub fn named_icon(&mut self, name: &str, px: u32) -> Option<Pixmap> {
        let key = (name.to_string(), px);
        if let Some(cached) = self.icon_cache.get(&key) {
            return cached.clone();
        }
        let path = self.catalog.lock().unwrap().icons.find(name, px);
        let pixmap = path.and_then(|path| paint::load_image(&path, px));
        if self.icon_cache.len() >= ICON_CACHE {
            self.icon_cache.clear();
        }
        self.icon_cache.insert(key, pixmap.clone());
        pixmap
    }

    /// Whether a Run query's first word is a program file, as far as is known yet: a path is
    /// looked at on a thread, and the row says Open until the answer lands and repaints it.
    fn run_is_program(&mut self, command: &str) -> bool {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        if !matches!(
            apps::run_query(command, &home, |_| false),
            apps::Run::Path(_)
        ) {
            return false;
        }
        if let Some((query, program)) = &self.catalog.lock().unwrap().run_program
            && query == command
        {
            return *program;
        }
        if self.run_asked.as_deref() != Some(command) {
            self.run_asked = Some(command.to_string());
            let (catalog, command) = (self.catalog.clone(), command.to_string());
            std::thread::spawn(move || {
                let program = matches!(
                    apps::run_query(&command, &home, crate::launch::executable),
                    apps::Run::Command(_)
                );
                let mut catalog = catalog.lock().unwrap();
                catalog.run_program = Some((command, program));
                catalog.version += 1;
            });
        }
        false
    }

    fn repaint(&mut self, look: &Look) -> Option<()> {
        let results = self.results();
        let screen_w = look.width as f32 / MOCKUP_PX;
        let width = WIDTH.min(screen_w - 40.0).max(480.0);
        let logical = Size::<i32, Logical>::from((
            ((width + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
            ((HEIGHT + 2.0 * MARGIN + 40.0) * MOCKUP_PX).ceil() as i32,
        ));
        let device = (
            (logical.w as f64 * look.scale).round() as i32,
            (logical.h as f64 * look.scale).round() as i32,
        );
        let f = look.scale as f32 * MOCKUP_PX;
        let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;

        // The frame's corner inside the pixmap, and its left edge on screen.
        let (fx, fy) = (MARGIN, MARGIN);
        let left = (screen_w - width) / 2.0;
        let on_screen = |x: f32, y: f32, w: f32, h: f32| {
            Rectangle::<f64, Logical>::new(
                (
                    ((left + x - fx) * MOCKUP_PX) as f64,
                    ((TOP + y - fy) * MOCKUP_PX) as f64,
                )
                    .into(),
                ((w * MOCKUP_PX) as f64, (h * MOCKUP_PX) as f64).into(),
            )
        };
        let mut targets = Vec::new();

        // Glass. The mockup's is 92% opaque over a 26 px blur; with no blur behind it, text
        // under it showed through, so it's nearly solid.
        p.card(
            fx,
            fy,
            width,
            HEIGHT,
            18.0,
            (40.0, 110.0, 0x000000b0),
            panel::GLASS,
            panel::GLASS_EDGE,
        );

        // The search row.
        let centre = fy + SEARCH_H / 2.0;
        p.icon(
            icons::SEARCH,
            fx + 26.0,
            centre - 14.0,
            28.0,
            Some(0x8f99aaff),
        );
        let mut x = fx + 26.0 + 28.0 + 16.0;
        x += p.text(
            &look.query,
            x,
            centre,
            &Style::new(Face::Body, 27.0, 0xf2f4f8ff),
        );
        // In logical pixels from the painted area's corner.
        self.caret_at = Point::from((
            ((x + 2.0) * MOCKUP_PX) as f64,
            ((centre - 13.0) * MOCKUP_PX) as f64,
        ));
        if look.query.is_empty() {
            p.text(
                "Apps, files, sums, :emoji",
                x + 9.0,
                centre,
                &Style::new(Face::Body, 27.0, panel::PLACEHOLDER),
            );
        }
        panel::key_hint(&mut p, fx + width - 26.0, centre, &["Super"], "");
        p.fill(fx, fy + SEARCH_H, width, 1.0, 0.0, 0xffffff12);

        // The apps grid.
        let body_y = fy + SEARCH_H + 1.0;
        let apps_w = width * 1.6 / 2.6;
        let tile_w = (apps_w - 40.0 - TILE_GAP * (COLUMNS - 1) as f32) / COLUMNS as f32;
        let answering = results.answer.is_some() || look.query.trim_start().starts_with(':');
        if results.apps.is_empty() && !answering {
            let message = if look.query.trim().is_empty() {
                "Looking for apps…".to_string()
            } else {
                format!("No apps match “{}”.", look.query.trim())
            };
            p.text(
                &message,
                fx + 30.0,
                body_y + 62.0,
                &Style::new(Face::Body, 16.0, 0x8b93a3ff),
            );
        }
        let icon_px = (ICON * f).round() as u32;
        let first = look.scroll * COLUMNS;
        for (index, app) in results
            .apps
            .iter()
            .enumerate()
            .skip(first)
            .take(ROWS * COLUMNS)
        {
            let (row, column) = ((index - first) / COLUMNS, index % COLUMNS);
            let x = fx + 20.0 + column as f32 * (tile_w + TILE_GAP);
            let y = body_y + 20.0 + row as f32 * (TILE_H + TILE_GAP);
            let selected = index == look.selected;
            if selected {
                p.fill(x, y, tile_w, TILE_H, 12.0, selection_fill(look.ring));
                p.border(x, y, tile_w, TILE_H, 12.0, 2.0, look.ring);
            }
            let icon_x = x + (tile_w - ICON) / 2.0;
            match self.tile_icon(app, icon_px) {
                Some(Some(icon)) => p.image(&icon, icon_x, y + 16.0),
                Some(None) => placeholder(&mut p, &app.name, icon_x, y + 16.0),
                // Still being rasterised: the tile repaints when it lands.
                None => {}
            }
            let style = Style::new(
                Face::Body,
                15.0,
                if selected { 0xffffffff } else { 0xe6e9efff },
            );
            let name = text::ellipsize(&app.name, &style, tile_w - 12.0);
            let name_w = text::width(&name, &style);
            p.text(
                &name,
                x + (tile_w - name_w) / 2.0,
                y + 16.0 + ICON + 10.0 + 11.0,
                &style,
            );
            let running = [Some(&app.id), app.wm_class.as_ref()]
                .into_iter()
                .flatten()
                .any(|name| look.running.contains(&name.to_lowercase()));
            if running {
                p.fill(
                    x + tile_w / 2.0 - 2.5,
                    y + TILE_H - 10.0,
                    5.0,
                    5.0,
                    2.5,
                    0x7fe3ffff,
                );
            }
            targets.push((Item::App(app.clone()), on_screen(x, y, tile_w, TILE_H)));
        }
        let rows = results.apps.len().div_ceil(COLUMNS);
        if rows > ROWS {
            let track = ROWS as f32 * (TILE_H + TILE_GAP) - TILE_GAP;
            let thumb = track * ROWS as f32 / rows as f32;
            let top = body_y + 20.0 + track * look.scroll as f32 / rows as f32;
            p.fill(fx + apps_w - 9.0, top, 3.0, thumb, 1.5, 0xffffff26);
        }

        // The side column: run, files, system.
        let side_x = fx + apps_w;
        p.fill(side_x, body_y, 1.0, BODY_H, 0.0, 0xffffff10);
        let (row_x, row_w) = (side_x + 17.0, width - apps_w - 33.0);
        let mut y = body_y + 14.0;
        let mut index = results.apps.len();
        if let Some(answer) = &results.answer {
            y += header(&mut p, row_x, y, "Answer");
            let selected = index == look.selected;
            answer_row(
                &mut p,
                row_x,
                y,
                row_w,
                answer,
                selected,
                look.ring,
                look.decode_step.map(|_| self.answer_age),
            );
            targets.push((
                Item::Answer(answer.clone()),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_H + 3.0;
            index += 1;
        }
        if !results.emoji.is_empty() {
            y += header(&mut p, row_x, y, "Emoji");
        } else if look.query.trim_start().starts_with(':') {
            y += header(&mut p, row_x, y, "Emoji");
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::SEARCH,
                    label: if look.query.trim().len() > 1 {
                        "No emoji match".into()
                    } else {
                        "Type a word: :fire, :thumbs, :party".into()
                    },
                    trailing: None,
                    keycap: false,
                    selected: false,
                    dim: true,
                    glyph: None,
                },
                look.ring,
            );
            y += ROW_H + 3.0;
        }
        for (emoji, name) in &results.emoji {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: "",
                    label: name.to_string(),
                    trailing: (index == look.selected).then(|| "⏎".to_string()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: Some(emoji),
                },
                look.ring,
            );
            targets.push((Item::Emoji(emoji, name), on_screen(row_x, y, row_w, ROW_H)));
            y += ROW_H + 3.0;
            index += 1;
        }
        if let Some(command) = &results.run {
            y += header(&mut p, row_x, y, "Run");
            let (opens, label) = run_label(command, self.run_is_program(command));
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: if opens { icons::FILE } else { icons::RUN },
                    label,
                    trailing: Some("⏎".into()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((
                Item::Run(command.clone()),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_H + 3.0;
            index += 1;
        }
        let files_title = if look.query.trim().is_empty() {
            "Recent files"
        } else {
            "Files"
        };
        // A sum or an emoji search has no use for an empty files list.
        let files_shown = !results.files.is_empty() || !answering;
        if files_shown {
            y += header(&mut p, row_x, y, files_title);
        }
        if results.files.is_empty() && files_shown {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::FILE,
                    label: "No files".into(),
                    trailing: None,
                    keycap: false,
                    selected: false,
                    dim: true,
                    glyph: None,
                },
                look.ring,
            );
            y += ROW_H + 3.0;
        }
        for file in &results.files {
            let folder = file
                .parent()
                .and_then(|dir| dir.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::FILE,
                    label: file_name(file),
                    trailing: folder,
                    keycap: false,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((Item::File(file.clone()), on_screen(row_x, y, row_w, ROW_H)));
            y += ROW_H + 3.0;
            index += 1;
        }
        if results.log_out || results.shortcuts {
            y += header(&mut p, row_x, y, "System");
        }
        if results.log_out {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::POWER,
                    label: "Log out".into(),
                    trailing: Some("Ctrl+Alt+Del".into()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((Item::LogOut, on_screen(row_x, y, row_w, ROW_H)));
            y += ROW_H + 3.0;
            index += 1;
        }
        if results.shortcuts {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::KEYBOARD,
                    label: "Keyboard shortcuts".into(),
                    trailing: Some("Super+/".into()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((Item::Shortcuts, on_screen(row_x, y, row_w, ROW_H)));
        }

        // The footer.
        let foot_y = body_y + BODY_H;
        p.fill(fx, foot_y, width, 1.0, 0.0, 0xffffff10);
        let mut right = fx + width - 22.0;
        let keys: &[(&[&str], &str)] = &[
            (&["Esc"], "clear, close"),
            (&["Shift+Enter"], "new window"),
            (&["Enter"], "open"),
            (&["Tab"], "next"),
            (&["↑ ↓ ← →"], "choose"),
        ];
        for (keys, label) in keys {
            right = panel::key_hint(&mut p, right, foot_y + FOOT_H / 2.0, keys, label) - 20.0;
        }

        self.origin = on_screen(0.0, 0.0, 0.0, 0.0).loc;
        self.frame = on_screen(fx, fy, width, HEIGHT);
        self.targets = targets;
        self.buffer = Some((paint::buffer(&p.pixmap), logical, device));
        Some(())
    }
}

/// The Run row for `command`: whether it opens rather than runs, and its label. URLs and paths
/// open; a path whose first word is a program (`program`) runs, with the rest as arguments.
fn run_label(command: &str, program: bool) -> (bool, String) {
    let opens = !program
        && !matches!(
            apps::run_query(command, std::path::Path::new("/home"), |_| false),
            apps::Run::Command(_)
        );
    let label = if opens {
        format!("Open “{command}”")
    } else {
        format!("Run “{command}”")
    };
    (opens, label)
}

/// The caret painted at `scale`, with its size in logical pixels. Sized as the painted explorer
/// is, in whole logical pixels first, so the renderer scales it by the same amount.
fn caret_buffer(scale: f64) -> Option<(f64, MemoryRenderBuffer, Size<i32, Logical>)> {
    let size = Size::<i32, Logical>::from((
        ((CARET.0 * MOCKUP_PX).round() as i32).max(1),
        ((CARET.1 * MOCKUP_PX).round() as i32).max(1),
    ));
    let mut pixmap = Pixmap::new(
        ((size.w as f64 * scale).round() as u32).max(1),
        ((size.h as f64 * scale).round() as u32).max(1),
    )?;
    let [r, g, b, a] = CARET_COLOUR.to_be_bytes();
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(r, g, b, a));
    Some((scale, paint::buffer(&pixmap), size))
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// An app with no icon gets a coloured square with its initial, as the mockup draws every app.
fn placeholder(p: &mut Painter, name: &str, x: f32, y: f32) {
    const COLOURS: [u32; 6] = [
        0x3cf0c0ff, 0x33ccffff, 0xffb547ff, 0xff7a93ff, 0xa78bfaff, 0x7fe3ffff,
    ];
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    p.fill(
        x + 4.0,
        y + 4.0,
        ICON - 8.0,
        ICON - 8.0,
        15.0,
        COLOURS[hash as usize % COLOURS.len()],
    );
    let initial: String = name.chars().next().into_iter().collect();
    let style = Style::new(Face::MonoBold, 30.0, 0x10131aff);
    let w = text::width(&initial, &style);
    p.text(&initial, x + (ICON - w) / 2.0, y + ICON / 2.0, &style);
}

/// A section title in the side column. Returns the height it takes.
fn header(p: &mut Painter, x: f32, y: f32, title: &str) -> f32 {
    let style = Style {
        tracking: 0.12,
        ..Style::new(Face::Body, 12.0, 0x7d8697ff)
    };
    p.text(&title.to_uppercase(), x + 10.0, y + 19.0, &style);
    32.0
}

struct Row<'a> {
    icon: &'a str,
    label: String,
    /// A folder name, or a key shown as a keycap.
    trailing: Option<String>,
    keycap: bool,
    selected: bool,
    dim: bool,
    /// Text drawn in the icon's place: an emoji.
    glyph: Option<&'a str>,
}

/// The selection's fill: the ring colour, faint.
fn selection_fill(ring: u32) -> u32 {
    (ring & 0xffffff00) | 0x1c
}

/// The answer to a sum or a conversion: what was understood, then the value, which decodes out of
/// rain glyphs for `decoding` seconds after it changes. Enter copies it.
#[allow(clippy::too_many_arguments)]
fn answer_row(
    p: &mut Painter,
    x: f32,
    y: f32,
    w: f32,
    answer: &crate::calc::Answer,
    selected: bool,
    ring: u32,
    decoding: Option<f64>,
) {
    if selected {
        p.fill(x, y, w, ROW_H, 9.0, selection_fill(ring));
        p.border(x, y, w, ROW_H, 9.0, 2.0, ring);
    }
    let centre = y + ROW_H / 2.0;
    p.icon(icons::RUN, x + 12.0, centre - 11.0, 22.0, Some(0x9aa4b6ff));
    let right = x + w - 12.0;
    let hint = "copy";
    let hint_style = Style::new(Face::Body, 12.0, panel::HINT);
    let hint_w = text::width(hint, &hint_style);
    p.text(hint, right - hint_w, centre, &hint_style);
    let room = w - 46.0 - hint_w - 24.0;
    let colour = if selected { 0xffffffff } else { 0xe6e9efff };
    let style = Style::new(Face::Body, 15.0, colour);
    // What was understood, the value, and any unit after it: `5 km = ` `3.107` ` mi`.
    let (lead, value, tail) = match answer.shown.rfind(&answer.value) {
        Some(at) => (
            &answer.shown[..at],
            answer.value.as_str(),
            &answer.shown[at + answer.value.len()..],
        ),
        None => (answer.shown.as_str(), "", ""),
    };
    let value_style = Style::new(Face::BodyBold, 15.0, colour);
    let full = text::width(&answer.shown, &style) + 2.0;
    if full > room || decoding.is_none() {
        let mut at = x + 46.0;
        at += p.text(
            &text::ellipsize(lead, &style, room * 0.6),
            at,
            centre,
            &style,
        );
        let left = (x + 46.0 + room - at).max(0.0);
        at += p.text(
            &text::ellipsize(value, &value_style, left),
            at,
            centre,
            &value_style,
        );
        let left = (x + 46.0 + room - at).max(0.0);
        p.text(&text::ellipsize(tail, &style, left), at, centre, &style);
        return;
    }
    let mut at = x + 46.0;
    at += p.text(lead, at, centre, &style);
    let [r, g, b, _] = ring.to_be_bytes();
    let glyph_style = Style::new(Face::MonoBold, 15.0, u32::from_be_bytes([r, g, b, 0xff]));
    let age = decoding.unwrap_or(f64::MAX);
    let masks = crate::history::decoded(value.chars().count(), age, value.len() as u64);
    for (ch, mask) in value.chars().zip(masks) {
        let shown = mask.unwrap_or(ch).to_string();
        let style = if mask.is_some() {
            &glyph_style
        } else {
            &value_style
        };
        at += p.text(&shown, at, centre, style);
    }
    p.text(tail, at, centre, &style);
}

fn side_row(p: &mut Painter, x: f32, y: f32, w: f32, row: &Row, ring: u32) {
    if row.selected {
        p.fill(x, y, w, ROW_H, 9.0, selection_fill(ring));
        p.border(x, y, w, ROW_H, 9.0, 2.0, ring);
    }
    let centre = y + ROW_H / 2.0;
    let ink = if row.dim { panel::HINT } else { 0x9aa4b6ff };
    match row.glyph {
        Some(glyph) => {
            let style = Style::new(Face::Body, 20.0, 0xf2f4f8ff);
            let glyph_w = text::width(glyph, &style);
            p.text(glyph, x + 12.0 + (22.0 - glyph_w) / 2.0, centre, &style);
        }
        None => p.icon(row.icon, x + 12.0, centre - 11.0, 22.0, Some(ink)),
    }
    let mut room = w - 12.0 - 22.0 - 12.0 - 12.0;
    if let Some(trailing) = &row.trailing {
        let right = x + w - 12.0;
        if row.keycap {
            let tw = paint::keycap_width(trailing);
            p.keycap(trailing, right - tw, centre);
            room -= tw + 12.0;
        } else {
            let style = Style::new(Face::Body, 12.0, panel::HINT);
            let tw = text::width(trailing, &style);
            p.text(trailing, right - tw, centre, &style);
            room -= tw + 12.0;
        }
    }
    let colour = if row.dim {
        panel::HINT
    } else if row.selected {
        0xffffffff
    } else {
        0xe6e9efff
    };
    let style = Style::new(Face::Body, 15.0, colour);
    p.text(
        &text::ellipsize(&row.label, &style, room.max(0.0)),
        x + 46.0,
        centre,
        &style,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_apps(names: &[&str]) -> Explorer {
        // Never this machine's recent files.
        let explorer = Explorer {
            scanned: Some(Instant::now()),
            read_recent_files: Arc::new(Vec::new),
            ..Explorer::idle(true)
        };
        explorer.catalog.lock().unwrap().apps = names
            .iter()
            .map(|name| App {
                id: name.to_lowercase(),
                name: name.to_string(),
                generic_name: String::new(),
                keywords: Vec::new(),
                exec: vec![name.to_lowercase()],
                icon: None,
                terminal: false,
                wm_class: None,
            })
            .collect();
        explorer
    }

    /// Types a query into an open explorer, a character at a time, as a person would.
    fn type_in(explorer: &mut Explorer, query: &str) {
        for ch in query.chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
    }

    #[test]
    fn a_command_is_offered_only_when_no_app_matches() {
        let mut explorer = with_apps(&["Files"]);
        explorer.open(0.0);
        type_in(&mut explorer, "files");
        // An app matched, so nothing is offered to run.
        assert!(explorer.results().run.is_none());
    }

    fn selected(explorer: &Explorer) -> Option<Item> {
        explorer
            .results()
            .items()
            .into_iter()
            .nth(explorer.selected)
    }

    #[test]
    fn open_never_waits_for_recent_files() {
        const XBEL: &str = r#"<xbel>
  <bookmark href="file:///srv/share/notes.txt" modified="2026-09-10T10:00:00Z"></bookmark>
  <bookmark href="file:///srv/share/plan.odt" modified="2026-09-11T10:00:00Z"></bookmark>
</xbel>"#;
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let mut explorer = Explorer {
            // As a file on a hung mount would: each check waits, here until the test lets go.
            read_recent_files: Arc::new(move || {
                apps::recent_in(XBEL, RECENT, |_| {
                    let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
                    true
                })
            }),
            ..with_apps(&["Firefox"])
        };
        explorer.read_recent();
        let started = Instant::now();
        explorer.open(0.0);
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(explorer.results().files.is_empty(), "nothing read yet");
        let version = explorer.catalog.lock().unwrap().version;
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while explorer.results().files.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            explorer.results().files,
            [
                PathBuf::from("/srv/share/plan.odt"),
                PathBuf::from("/srv/share/notes.txt")
            ]
        );
        assert!(
            explorer.catalog.lock().unwrap().version > version,
            "it repaints"
        );
    }

    #[test]
    fn icon_names_from_notifications_never_pile_up() {
        let mut explorer = with_apps(&[]);
        for n in 0..1000 {
            assert!(
                explorer
                    .named_icon(&format!("made-up-icon-{n}"), 48)
                    .is_none()
            );
            assert!(explorer.icon_cache.len() <= ICON_CACHE);
        }
    }

    #[test]
    fn typing_filters_and_enter_opens_the_best_match() {
        let mut explorer = with_apps(&["Dolphin", "Firefox", "Konsole"]);
        explorer.open(0.0);
        for ch in "fire".chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
        match explorer.key(Keysym::Return, None, Mods::default()) {
            Outcome::Activate(Item::App(app), false) => assert_eq!(app.name, "Firefox"),
            _ => panic!("Enter should open Firefox"),
        }
    }

    #[test]
    fn shift_enter_asks_for_a_new_window() {
        let mut explorer = with_apps(&["Foot"]);
        explorer.open(0.0);
        explorer.key(Keysym::NoSymbol, Some('f'), Mods::default());
        assert!(matches!(
            explorer.key(
                Keysym::Return,
                None,
                Mods {
                    shift: true,
                    ..Mods::default()
                }
            ),
            Outcome::Activate(Item::App(_), true)
        ));
    }

    #[test]
    fn a_query_no_app_matches_can_run_as_a_command() {
        let mut explorer = with_apps(&["Firefox"]);
        explorer.open(0.0);
        for ch in "htop -d 5".chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
        assert_eq!(selected(&explorer), Some(Item::Run("htop -d 5".into())));
    }

    #[test]
    fn arrows_move_through_the_grid_and_on_to_the_side_column() {
        let names: Vec<String> = (0..14).map(|i| format!("App {i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut explorer = with_apps(&names);
        explorer.open(0.0);
        explorer.key(Keysym::Right, None, Mods::default());
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 5);
        explorer.key(Keysym::Down, None, Mods::default());
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 13, "the fourth row");
        assert_eq!(explorer.scroll, 1, "scrolled to keep it in view");
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 14, "past the apps: the log-out row");
        assert_eq!(selected(&explorer), Some(Item::LogOut));
        explorer.key(Keysym::Up, None, Mods::default());
        assert_eq!(explorer.selected, 13);
    }

    const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        logo: false,
    };

    fn typed(explorer: &mut Explorer, text: &str) {
        for ch in text.chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), NONE);
        }
    }

    #[test]
    fn escape_clears_the_query_before_it_closes() {
        let mut explorer = with_apps(&["Foot", "Firefox"]);
        explorer.open(0.0);
        typed(&mut explorer, "fi");
        explorer.key(Keysym::Down, None, NONE);
        assert!(matches!(
            explorer.key(Keysym::Escape, None, NONE),
            Outcome::Nothing
        ));
        assert_eq!(
            (explorer.query.as_str(), explorer.selected, explorer.scroll),
            ("", 0, 0)
        );
        assert!(matches!(
            explorer.key(Keysym::Escape, None, NONE),
            Outcome::Close
        ));
    }

    #[test]
    fn tab_moves_on_and_wraps() {
        let mut explorer = with_apps(&["A", "B", "C"]);
        explorer.open(0.0);
        // Three apps and the log-out row.
        explorer.key(Keysym::Tab, None, NONE);
        assert_eq!(explorer.selected, 1);
        explorer.key(Keysym::ISO_Left_Tab, None, NONE);
        explorer.key(Keysym::ISO_Left_Tab, None, NONE);
        assert_eq!(explorer.selected, 3, "back round to the last");
        explorer.key(Keysym::Tab, None, NONE);
        assert_eq!(explorer.selected, 0);
    }

    #[test]
    fn down_with_nothing_below_stays_put() {
        let mut explorer = with_apps(&["Foot", "Foot Client", "Foot Server", "Kate"]);
        explorer.open(0.0);
        typed(&mut explorer, "fo");
        assert_eq!(
            explorer.results().items().len(),
            3,
            "one row, no side column"
        );
        explorer.key(Keysym::Down, None, NONE);
        explorer.key(Keysym::Down, None, NONE);
        assert_eq!(explorer.selected, 0);
    }

    #[test]
    fn home_end_and_page_keys_jump() {
        let names: Vec<String> = (0..30).map(|i| format!("App {i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut explorer = with_apps(&names);
        explorer.open(0.0);
        explorer.key(Keysym::End, None, NONE);
        assert_eq!(selected(&explorer), Some(Item::LogOut));
        explorer.key(Keysym::Home, None, NONE);
        assert_eq!(explorer.selected, 0);
        explorer.key(Keysym::Right, None, NONE);
        explorer.key(Keysym::Next, None, NONE);
        assert_eq!(explorer.selected, 13, "three rows down");
        assert_eq!(explorer.scroll, 1);
        explorer.key(Keysym::Next, None, NONE);
        explorer.key(Keysym::Next, None, NONE);
        assert_eq!(explorer.selected, 29, "no further than the last app");
        explorer.key(Keysym::Prior, None, NONE);
        assert_eq!(explorer.selected, 17);
    }

    #[test]
    fn chords_never_type_and_ctrl_clears() {
        let mut explorer = with_apps(&["Foot"]);
        explorer.open(0.0);
        typed(&mut explorer, "foo");
        let ctrl = Mods { ctrl: true, ..NONE };
        explorer.key(Keysym::v, None, ctrl);
        assert_eq!(explorer.query, "foo", "Ctrl+V types nothing");
        explorer.key(Keysym::BackSpace, None, ctrl);
        assert_eq!(explorer.query, "");
        typed(&mut explorer, "fo");
        explorer.key(Keysym::u, None, ctrl);
        assert_eq!(explorer.query, "");
    }

    #[test]
    fn icons_are_rasterised_off_the_event_loop_before_the_explorer_opens() {
        let mut explorer = with_apps(&["Foot", "Kate"]);
        {
            let mut catalog = explorer.catalog.lock().unwrap();
            for app in &mut catalog.apps {
                app.icon = Some(format!("made-up-{}", app.id));
            }
            catalog.version = 1;
        }
        let foot = explorer.catalog.lock().unwrap().apps[0].clone();
        let px = (ICON * 1.25 * MOCKUP_PX).round() as u32;
        assert_eq!(
            explorer.tile_icon(&foot, px),
            None,
            "nothing rasterised yet"
        );
        explorer.warm_icons(1.25);
        let deadline = Instant::now() + Duration::from_secs(5);
        while explorer.catalog.lock().unwrap().warming && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        // No theme has it, so the tile gets its lettered square, and it never looks again.
        assert_eq!(explorer.tile_icon(&foot, px), Some(None));
        let landed = explorer.catalog.lock().unwrap().tile_icons_landed;
        explorer.warm_icons(1.25);
        assert!(
            !explorer.catalog.lock().unwrap().warming,
            "nothing new to do"
        );
        assert_eq!(explorer.catalog.lock().unwrap().tile_icons_landed, landed);
        // Another scale needs another size.
        assert_eq!(explorer.tile_icon(&foot, px + 1), None);
    }

    #[test]
    fn a_run_path_that_starts_with_a_program_says_run() {
        assert_eq!(
            run_label("~/bin/tool --flag", true),
            (false, "Run “~/bin/tool --flag”".into())
        );
        assert_eq!(
            run_label("~/Documents", false),
            (true, "Open “~/Documents”".into())
        );
        assert_eq!(run_label("htop -d 5", false).0, false);
        assert_eq!(run_label("https://example.org", false).0, true);
        // The look-up itself: a program file in a made-up folder.
        let dir = crate::files::test_scratch("run-label");
        let tool = dir.join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let query = format!("{} --flag", tool.display());
        let mut explorer = with_apps(&[]);
        assert!(!explorer.run_is_program(&query), "not known yet");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !explorer.run_is_program(&query) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(explorer.run_is_program(&query));
        assert!(
            !explorer.run_is_program(&dir.display().to_string()),
            "a folder opens"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

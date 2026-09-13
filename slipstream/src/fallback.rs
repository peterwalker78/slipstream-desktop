//! Characters the embedded fonts don't have: Cyrillic and Greek in body text, CJK, emoji, symbols.
//! Window titles, notifications and file names carry them, and without a face that has them they
//! drew as empty boxes.
//!
//! The faces come from the fonts already installed, looked through in order: Noto Sans (Cyrillic,
//! Greek and more Latin), Noto Sans CJK, Noto Emoji, then Symbola. Emoji draw in one colour, like
//! the text around them.
//!
//! **Memory is the budget.** A face is read with ttf-parser, which parses nothing up front and
//! reads the file only where a glyph needs it, and each glyph is rasterised when it's first drawn,
//! into a cache bounded by count and bytes. A system font file is mapped rather than read, so its
//! pages are the page cache's, shared and given back under pressure; a CJK collection is 19 MB of
//! that, not the hundreds of megabytes a full parse costs. A file the user could rewrite in place
//! is read into memory instead, since a mapped file that shrinks under a reader kills it.
//!
//! **Nothing waits.** Finding the files and loading a face happen on a thread of their own, the
//! first time a character none of the loaded faces has is asked for, never at startup. Until the
//! face lands the character takes up its usual space and draws nothing; when it lands, the event
//! loop is woken, takes it in, and everything with text on it is painted again.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, OnceLock},
};

use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};
use smithay::reexports::calloop::ping::Ping;
use ttf_parser::{Face, GlyphId};

/// The fallback faces, in the order a character looks through them, each by the file names it's
/// installed under, most likely first.
const FACES: [&[&str]; 4] = [
    &[
        "NotoSans-Regular.ttf",
        "NotoSans[wght].ttf",
        "NotoSans[wdth,wght].ttf",
    ],
    &[
        "NotoSansCJK-Regular.ttc",
        "NotoSansCJK-VF.ttc",
        "NotoSansCJKsc-Regular.otf",
        "NotoSansCJKjp-Regular.otf",
    ],
    &["NotoEmoji-Regular.ttf", "NotoEmoji[wght].ttf"],
    &["Symbola.ttf", "Symbola.otf"],
];

/// How many rasterised glyphs are kept, and how many bytes of coverage, whichever fills first.
const GLYPHS: usize = 2048;
const GLYPH_BYTES: usize = 16 << 20;
/// A font read into memory rather than mapped: the largest collections are about 20 MB.
const FONT_LIMIT: u64 = 64 << 20;
/// Characters no face has, remembered so they aren't looked up again; forgotten when a face lands.
const ABSENT: usize = 1024;
/// How deep a fonts folder is looked through: `/usr/share/fonts/<family>/<file>` is two.
const DEPTH: usize = 4;

/// What a character draws from, when the embedded face doesn't have it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Found {
    /// A loaded face's glyph, and how far it advances at the size asked for.
    Glyph {
        slot: usize,
        id: GlyphId,
        advance: f32,
    },
    /// A face that may have it is still being found or loaded.
    Pending,
    /// No installed face has it.
    Absent,
}

/// A rasterised glyph: coverage from 0 to 255, `width` by `height`, with its bottom-left corner
/// `xmin` right of the pen and `ymin` above the baseline, as fontdue's metrics measure it.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
}

/// Work for a thread of its own.
#[derive(Debug, Clone, PartialEq)]
enum Job {
    /// Look through the fonts folders for every face's file.
    Find,
    /// Load one face from its file.
    Load(usize, PathBuf),
}

/// What a job came back with.
enum Landed {
    Files(Vec<Option<PathBuf>>),
    Face(usize, Option<Box<Face<'static>>>),
}

enum Slot {
    Unloaded,
    Loading,
    /// Boxed: a parsed face's tables are a few kilobytes of pointers.
    Ready(Box<Face<'static>>),
    /// Not installed, or its file couldn't be read.
    Missing,
}

struct Fallbacks {
    /// Each face's file, once the fonts folders have been looked through.
    files: Option<Vec<Option<PathBuf>>>,
    finding: bool,
    slots: Vec<Slot>,
    absent: HashSet<char>,
    glyphs: GlyphCache,
}

impl Fallbacks {
    fn new(faces: usize) -> Self {
        Self {
            files: None,
            finding: false,
            slots: (0..faces).map(|_| Slot::Unloaded).collect(),
            absent: HashSet::new(),
            glyphs: GlyphCache::new(GLYPHS, GLYPH_BYTES),
        }
    }

    /// Where `ch` draws from at `px` pixels. A character that no loaded face has sets the next
    /// step going with `start`: finding the files, or loading the next face in order.
    fn find(&mut self, ch: char, px: f32, start: &mut dyn FnMut(Job)) -> Found {
        for (slot, state) in self.slots.iter().enumerate() {
            if let Slot::Ready(face) = state
                && let Some(id) = face.glyph_index(ch).filter(|id| id.0 != 0)
            {
                let scale = px / face.units_per_em().max(1) as f32;
                let advance = face.glyph_hor_advance(id).unwrap_or(0) as f32 * scale;
                return Found::Glyph { slot, id, advance };
            }
        }
        if self.absent.contains(&ch) {
            return Found::Absent;
        }
        let Some(files) = &self.files else {
            if !self.finding {
                self.finding = true;
                start(Job::Find);
            }
            return Found::Pending;
        };
        for (slot, state) in self.slots.iter_mut().enumerate() {
            match state {
                Slot::Loading => return Found::Pending,
                Slot::Unloaded => match files.get(slot).cloned().flatten() {
                    Some(path) => {
                        *state = Slot::Loading;
                        start(Job::Load(slot, path));
                        return Found::Pending;
                    }
                    None => *state = Slot::Missing,
                },
                Slot::Ready(_) | Slot::Missing => {}
            }
        }
        if self.absent.len() >= ABSENT {
            self.absent.clear();
        }
        self.absent.insert(ch);
        Found::Absent
    }

    /// Takes in what a job found.
    fn land(&mut self, landed: Landed) {
        match landed {
            Landed::Files(files) => {
                self.files = Some(files);
                self.finding = false;
            }
            Landed::Face(slot, face) => {
                if let Some(state) = self.slots.get_mut(slot) {
                    *state = face.map_or(Slot::Missing, Slot::Ready);
                }
            }
        }
        self.absent.clear();
    }

    /// Calls `draw` with `slot`'s glyph `id` rasterised at `px` pixels, from the cache if it's
    /// there.
    fn with_raster<T>(
        &mut self,
        slot: usize,
        id: GlyphId,
        px: f32,
        draw: impl FnOnce(&Raster) -> T,
    ) -> Option<T> {
        let Some(Slot::Ready(face)) = self.slots.get(slot) else {
            return None;
        };
        let key = (slot, id.0, (px * 8.0).round() as u32);
        if self.glyphs.get(&key).is_none() {
            let raster = rasterise(face, id, px);
            self.glyphs.insert(key, raster);
        }
        self.glyphs.get(&key).map(draw)
    }
}

/// Rasterised glyphs, least recently used out once either bound is reached.
struct GlyphCache {
    most: usize,
    most_bytes: usize,
    bytes: usize,
    clock: u64,
    entries: HashMap<(usize, u16, u32), (Raster, u64)>,
}

impl GlyphCache {
    fn new(most: usize, most_bytes: usize) -> Self {
        Self {
            most,
            most_bytes,
            bytes: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn get(&mut self, key: &(usize, u16, u32)) -> Option<&Raster> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|(raster, used)| {
            *used = clock;
            &*raster
        })
    }

    fn insert(&mut self, key: (usize, u16, u32), raster: Raster) {
        let size = raster.coverage.len();
        while !self.entries.is_empty()
            && (self.entries.len() >= self.most || self.bytes + size > self.most_bytes)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= gone.coverage.len();
            }
        }
        self.clock += 1;
        self.bytes += size;
        if let Some((replaced, _)) = self.entries.insert(key, (raster, self.clock)) {
            self.bytes -= replaced.coverage.len();
        }
    }
}

/// A glyph's outline filled at `px` pixels. A glyph with no outline (a space) or an absurd box is
/// empty.
fn rasterise(face: &Face, id: GlyphId, px: f32) -> Raster {
    let empty = Raster {
        xmin: 0,
        ymin: 0,
        width: 0,
        height: 0,
        coverage: Vec::new(),
    };
    let Some(bbox) = face.glyph_bounding_box(id) else {
        return empty;
    };
    let scale = px / face.units_per_em().max(1) as f32;
    let xmin = (bbox.x_min as f32 * scale).floor() as i32;
    let ymin = (bbox.y_min as f32 * scale).floor() as i32;
    let xmax = (bbox.x_max as f32 * scale).ceil() as i32;
    let ymax = (bbox.y_max as f32 * scale).ceil() as i32;
    let (width, height) = (xmax - xmin, ymax - ymin);
    if !(1..=1024).contains(&width) || !(1..=1024).contains(&height) {
        return empty;
    }
    let mut outline = Outline {
        path: PathBuilder::new(),
        scale,
        left: xmin as f32,
        top: ymax as f32,
    };
    if face.outline_glyph(id, &mut outline).is_none() {
        return empty;
    }
    let (Some(path), Some(mut pixmap)) = (
        outline.path.finish(),
        Pixmap::new(width as u32, height as u32),
    ) else {
        return empty;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    Raster {
        xmin,
        ymin,
        width: width as usize,
        height: height as usize,
        coverage: pixmap
            .data()
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect(),
    }
}

/// A glyph's outline in pixels, with y pointing down from the top of its box.
struct Outline {
    path: PathBuilder,
    scale: f32,
    left: f32,
    top: f32,
}

impl Outline {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale - self.left, self.top - y * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let ((x1, y1), (x, y)) = (self.at(x1, y1), self.at(x, y));
        self.path.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let ((x1, y1), (x2, y2), (x, y)) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.path.close();
    }
}

static FALLBACKS: LazyLock<Mutex<Fallbacks>> =
    LazyLock::new(|| Mutex::new(Fallbacks::new(FACES.len())));
/// What jobs found, until the event loop takes it in.
static LANDED: Mutex<Vec<Landed>> = Mutex::new(Vec::new());
/// Wakes the event loop when a job has finished.
static WAKER: OnceLock<Ping> = OnceLock::new();

/// Where the event loop is woken when a face lands.
pub fn set_waker(ping: Ping) {
    let _ = WAKER.set(ping);
}

/// Where `ch` draws from at `px` pixels. Called while painting, on the event loop.
pub fn find(ch: char, px: f32) -> Found {
    FALLBACKS.lock().unwrap().find(ch, px, &mut |job| {
        std::thread::spawn(move || run(job));
    })
}

/// Calls `draw` with a fallback glyph rasterised at `px` pixels.
pub fn with_raster<T>(
    slot: usize,
    id: GlyphId,
    px: f32,
    draw: impl FnOnce(&Raster) -> T,
) -> Option<T> {
    FALLBACKS.lock().unwrap().with_raster(slot, id, px, draw)
}

/// Takes in whatever jobs have found since the last call, on the event loop, so a face never
/// appears half-way through painting something. Returns whether anything changed, in which case
/// text painted before now may be missing glyphs that can be drawn.
pub fn take_landed() -> bool {
    let landed = std::mem::take(&mut *LANDED.lock().unwrap());
    if landed.is_empty() {
        return false;
    }
    let mut fallbacks = FALLBACKS.lock().unwrap();
    for landed in landed {
        fallbacks.land(landed);
    }
    true
}

fn run(job: Job) {
    let landed = match job {
        Job::Find => Landed::Files(find_files(&font_dirs())),
        Job::Load(slot, path) => Landed::Face(slot, load(&path).map(Box::new)),
    };
    LANDED.lock().unwrap().push(landed);
    if let Some(waker) = WAKER.get() {
        waker.ping();
    }
}

/// The user's fonts folder, then the system's.
fn font_dirs() -> Vec<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    vec![data_home.join("fonts"), PathBuf::from("/usr/share/fonts")]
}

/// Each face's file: for each of its names in turn, the first folder that has it.
fn find_files(dirs: &[PathBuf]) -> Vec<Option<PathBuf>> {
    let mut named: HashMap<String, PathBuf> = HashMap::new();
    for dir in dirs {
        look_in(dir, DEPTH, &mut named);
    }
    FACES
        .iter()
        .map(|names| names.iter().find_map(|name| named.get(*name).cloned()))
        .collect()
}

/// Remembers every wanted file name under `dir`, keeping the first found. Folders are looked in
/// only if they are real ones, so a link can't lead round in a circle.
fn look_in(dir: &Path, depth: usize, named: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if depth > 1 {
                look_in(&entry.path(), depth - 1, named);
            }
        } else if FACES.iter().any(|names| names.contains(&name.as_str())) {
            named.entry(name).or_insert_with(|| entry.path());
        }
    }
}

/// A face from its file, for the rest of the compositor's life: mapped when nobody but its owner,
/// who isn't this user, could change it in place, and read into memory otherwise.
fn load(path: &Path) -> Option<Face<'static>> {
    let before = memory();
    let data = match map(path) {
        Some(mapped) => mapped,
        None => {
            let bytes = crate::files::read_small(path, FONT_LIMIT)
                .inspect_err(|err| {
                    tracing::warn!(path = %path.display(), "couldn't read a fallback font: {err}")
                })
                .ok()?;
            Box::leak(bytes.into_boxed_slice())
        }
    };
    let face = Face::parse(data, 0)
        .inspect_err(
            |err| tracing::warn!(path = %path.display(), "couldn't read a fallback font: {err}"),
        )
        .ok()?;
    let after = memory();
    tracing::info!(
        file = %path.display(),
        rss_before_kb = before.0,
        rss_after_kb = after.0,
        anon_before_kb = before.1,
        anon_after_kb = after.1,
        "loaded fallback face"
    );
    Some(face)
}

/// `path` mapped read-only, if it's a regular file this user doesn't own and can't write: then
/// only another user could shrink it under the mapping, and package updates replace a file rather
/// than rewrite it. The mapping is never undone, since the face lives as long as the compositor.
fn map(path: &Path) -> Option<&'static [u8]> {
    use std::os::unix::fs::MetadataExt;
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    // SAFETY: getuid has no failure mode.
    let ours = metadata.uid() == unsafe { libc::getuid() };
    let writable = {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        // SAFETY: a valid C string and a mode; faccessat only reads them.
        unsafe { libc::faccessat(libc::AT_FDCWD, name.as_ptr(), libc::W_OK, libc::AT_EACCESS) == 0 }
    };
    let len = usize::try_from(metadata.len()).ok()?;
    if !metadata.is_file() || ours || writable || len == 0 {
        return None;
    }
    // SAFETY: a new read-only shared mapping of an open file, `len` bytes long; the pointer is
    // checked before use, and the mapping outlives the descriptor, which may close.
    let at = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if at == libc::MAP_FAILED {
        return None;
    }
    // SAFETY: the mapping is `len` readable bytes and is never unmapped.
    Some(unsafe { std::slice::from_raw_parts(at.cast::<u8>(), len) })
}

/// This process's resident memory and the anonymous part of it, in kilobytes.
fn memory() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:"), field("RssAnon:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JetBrains Mono has Cyrillic, and the body face doesn't: a made-up fallback for tests.
    fn mono() -> Face<'static> {
        Face::parse(
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
            0,
        )
        .unwrap()
    }

    /// The body face itself, which lacks Cyrillic: a fallback that doesn't help.
    fn body() -> Face<'static> {
        Face::parse(
            include_bytes!("../../assets/fonts/AtkinsonHyperlegible-Regular.ttf"),
            0,
        )
        .unwrap()
    }

    fn no_jobs(job: Job) {
        panic!("{job:?} started with every face loaded");
    }

    #[test]
    fn a_character_the_body_face_lacks_draws_from_a_fallback() {
        assert!(body().glyph_index('Ж').is_none(), "the test needs a gap");
        let mut fallbacks = Fallbacks::new(1);
        fallbacks.files = Some(vec![None]);
        fallbacks.slots[0] = Slot::Ready(Box::new(mono()));
        let Found::Glyph { slot, id, advance } = fallbacks.find('Ж', 20.0, &mut no_jobs) else {
            panic!("JetBrains Mono has Ж");
        };
        assert_eq!(slot, 0);
        assert!(
            (advance - 12.0).abs() < 0.5,
            "a mono em is 600 units: {advance}"
        );
        let inked = fallbacks
            .with_raster(slot, id, 20.0, |raster| {
                assert_eq!(raster.coverage.len(), raster.width * raster.height);
                assert!(raster.height > 8 && raster.ymin >= -1, "{raster:?}");
                raster.coverage.iter().filter(|cover| **cover > 128).count()
            })
            .unwrap();
        assert!(inked > 20, "the glyph is drawn");
    }

    #[test]
    fn faces_are_found_then_loaded_in_order_and_nothing_waits() {
        let mut fallbacks = Fallbacks::new(2);
        let mut jobs = Vec::new();
        let mut start = |job| jobs.push(job);
        assert_eq!(fallbacks.find('Ж', 16.0, &mut start), Found::Pending);
        assert_eq!(fallbacks.find('Ж', 16.0, &mut start), Found::Pending);
        assert_eq!(jobs, [Job::Find], "the folders are looked through once");

        fallbacks.land(Landed::Files(vec![
            Some("/fonts/first.ttf".into()),
            Some("/fonts/second.ttf".into()),
        ]));
        let mut jobs = Vec::new();
        let mut start = |job| jobs.push(job);
        assert_eq!(fallbacks.find('Ж', 16.0, &mut start), Found::Pending);
        assert_eq!(fallbacks.find('Ж', 16.0, &mut start), Found::Pending);
        assert_eq!(jobs, [Job::Load(0, "/fonts/first.ttf".into())]);

        // The first face turns out not to have it, so the next one loads.
        fallbacks.land(Landed::Face(0, Some(Box::new(body()))));
        let mut jobs = Vec::new();
        assert_eq!(
            fallbacks.find('Ж', 16.0, &mut |job| jobs.push(job)),
            Found::Pending
        );
        assert_eq!(jobs, [Job::Load(1, "/fonts/second.ttf".into())]);
        fallbacks.land(Landed::Face(1, Some(Box::new(mono()))));
        assert!(matches!(
            fallbacks.find('Ж', 16.0, &mut no_jobs),
            Found::Glyph { slot: 1, .. }
        ));
        // A character the body face has is the first face's.
        assert!(matches!(
            fallbacks.find('A', 16.0, &mut no_jobs),
            Found::Glyph { slot: 0, .. }
        ));
        // One that nothing installed has is absent, and not looked for again.
        assert_eq!(
            fallbacks.find('\u{10FFFD}', 16.0, &mut no_jobs),
            Found::Absent
        );
        assert!(fallbacks.absent.contains(&'\u{10FFFD}'));
    }

    #[test]
    fn a_face_that_is_not_installed_or_will_not_load_is_skipped() {
        let mut fallbacks = Fallbacks::new(2);
        fallbacks.land(Landed::Files(vec![None, Some("/fonts/broken.ttf".into())]));
        let mut jobs = Vec::new();
        assert_eq!(
            fallbacks.find('Ж', 16.0, &mut |job| jobs.push(job)),
            Found::Pending
        );
        assert_eq!(jobs, [Job::Load(1, "/fonts/broken.ttf".into())]);
        fallbacks.land(Landed::Face(1, None));
        assert_eq!(fallbacks.find('Ж', 16.0, &mut no_jobs), Found::Absent);
    }

    #[test]
    fn the_glyph_cache_stays_within_its_bound() {
        let raster = |bytes: usize| Raster {
            xmin: 0,
            ymin: 0,
            width: bytes,
            height: 1,
            coverage: vec![255; bytes],
        };
        let mut cache = GlyphCache::new(8, 100);
        for id in 0..50u16 {
            cache.insert((0, id, 160), raster(10));
            // The first glyph is used all the time, so it's never the one to go.
            assert!(cache.get(&(0, 0, 160)).is_some(), "at {id}");
            assert!(cache.entries.len() <= 8 && cache.bytes <= 100);
        }
        cache.insert((0, 999, 160), raster(95));
        assert!(cache.bytes <= 100, "{}", cache.bytes);
        assert!(cache.get(&(0, 999, 160)).is_some());
        assert_eq!(
            cache.bytes,
            cache
                .entries
                .values()
                .map(|(raster, _)| raster.coverage.len())
                .sum::<usize>()
        );
    }

    #[test]
    fn a_cjk_character_measures_with_a_fallback() {
        // Only where Noto Sans CJK is installed; a machine without it has nothing to check.
        let files = find_files(&font_dirs());
        let Some(path) = files[1].clone() else {
            return;
        };
        let face = load(&path).expect("the installed CJK face loads");
        let mut fallbacks = Fallbacks::new(2);
        fallbacks.files = Some(files);
        fallbacks.slots[0] = Slot::Missing;
        fallbacks.slots[1] = Slot::Ready(Box::new(face));
        let Found::Glyph { advance, .. } = fallbacks.find('日', 20.0, &mut no_jobs) else {
            panic!("Noto Sans CJK has 日");
        };
        assert!(
            (advance - 20.0).abs() < 0.5,
            "a CJK character is an em wide"
        );
    }
}

//! What was open when the session ended, written to `~/.local/state/slipstream/session.toml`. Off
//! unless `session.remember` is set, and with it off nothing is written at all — there is no record
//! of what was open sitting in the state directory.
//!
//! Apps and places only. **No window titles**, because a title is often a document name, a URL or
//! a correspondent, and this file goes to disk without being asked about each time.
//!
//! ```toml
//! version = 1
//! saved-at = "2026-09-12T19:04:11Z"
//! active-workspace = 2
//!
//! [[window]]
//! app = "org.kde.konsole"
//! workspace = 1
//! slot = 0
//! focused = true
//!
//! [[workspace]]
//! index = 1
//! tiles = "v0.5(0,1)"
//! ```
//!
//! Pure: no Wayland types, no clock but the system one, so all of it is unit-tested.
//!
//! Written twice over: whenever the desktop has been still for a couple of seconds
//! (`Slipstream::tick_record`), so a crash or a power cut still leaves a layout to come back to,
//! and once more when the way out's card goes up, from which point the windows are closing and
//! the desktop is no record of anything. `restore.rs` reads it back at the next login.
#![allow(dead_code)]

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::layout::Shape;

/// The file's name inside Slipstream's state folder.
pub const FILE_NAME: &str = "session.toml";

/// Bumped when the format changes in a way an older Slipstream would read wrongly. A file from a
/// newer version is ignored rather than half-understood.
pub const VERSION: u32 = 1;

/// More than this many windows and the rest are left out, so a runaway can't write a huge file.
const MAX_WINDOWS: usize = 60;

const HEADER: &str = "\
# What Slipstream had open when the session ended, so the layout can be put back. Written only
# while `remember` is on under [session] in settings.toml; turning it off deletes this file.
# Apps and places, never window titles.

";

/// A whole desktop, as it was.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Record {
    pub version: u32,
    /// UTC, so a file that moves between machines still sorts.
    pub saved_at: String,
    /// The workspace that was in front, counting from 1.
    pub active_workspace: usize,
    #[serde(rename = "window")]
    pub windows: Vec<Win>,
    #[serde(rename = "workspace")]
    pub workspaces: Vec<Tiling>,
}

/// One window: which app, and where it was.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Win {
    /// The `app_id`, lower-cased, as `window_app_id` reads it. Matched to a `.desktop` entry when
    /// the layout is put back; never run as a command, because any client can set any `app_id`.
    pub app: String,
    /// Its workspace's number counting from 1, or absent for a window in the code rain, which
    /// belongs to no workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<usize>,
    /// Its place among that workspace's windows, or among the rain's: what `tiles` refers to.
    pub slot: usize,
    #[serde(skip_serializing_if = "is_false")]
    pub focused: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub in_rain: bool,
    /// "centre", "orbit" or "distant", when gravity was on for its workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gravity: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub pinned: bool,
}

/// One workspace's tiling tree, as the string grammar below.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Tiling {
    /// Its number when it was written, counting from 1.
    pub index: usize,
    /// Its name, if it had one: a named workspace is found again by name, wherever the list has
    /// moved it since.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub tiles: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Record {
    /// A record of this moment, with `windows` and `workspaces` filled in by the caller.
    pub fn new(active_workspace: usize, windows: Vec<Win>, workspaces: Vec<Tiling>) -> Self {
        Self {
            version: VERSION,
            saved_at: stamp(SystemTime::now()),
            active_workspace,
            windows: windows.into_iter().take(MAX_WINDOWS).collect(),
            workspaces,
        }
    }

    /// Whether this file came from a Slipstream that knew something this one doesn’t.
    pub fn is_too_new(&self) -> bool {
        self.version > VERSION
    }

    /// The same desktop as `other`, the moment it was taken aside. The save while the desktop is
    /// in use compares with what's already on disk, so a still desktop isn't written over and
    /// over with nothing but a new timestamp.
    pub fn same_desktop(&self, other: &Record) -> bool {
        self.version == other.version
            && self.active_workspace == other.active_workspace
            && self.windows == other.windows
            && self.workspaces == other.workspaces
    }
}

/// The tiling tree as one line: `0` is a leaf, `v0.5(a,b)` a side-by-side split and `h0.5(a,b)` a
/// stacked one. Leaves are slot numbers within the workspace.
pub fn encode(shape: &Shape<usize>) -> String {
    match shape {
        Shape::Leaf(slot) => slot.to_string(),
        Shape::Split {
            vertical,
            ratio,
            a,
            b,
        } => format!(
            "{}{ratio}({},{})",
            if *vertical { 'v' } else { 'h' },
            encode(a),
            encode(b)
        ),
    }
}

/// `encode` read back. `None` for anything malformed: a hand-edited or truncated file leaves the
/// desktop empty rather than half-built. So is a tree no record could hold, deeper than
/// `MAX_WINDOWS` or with a slot past it: the file is read at every login, and a corrupt one
/// nested thousands deep would otherwise overflow the stack every time.
pub fn decode(text: &str) -> Option<Shape<usize>> {
    let (shape, rest) = take(text.trim(), 0)?;
    rest.is_empty().then_some(shape)
}

/// One shape off the front of `text`, `depth` splits down, and whatever follows it.
fn take(text: &str, depth: usize) -> Option<(Shape<usize>, &str)> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if first != 'v' && first != 'h' {
        let end = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        let slot: usize = text.get(..end)?.parse().ok()?;
        if slot >= MAX_WINDOWS {
            return None;
        }
        return Some((Shape::Leaf(slot), &text[end..]));
    }
    if depth >= MAX_WINDOWS {
        return None;
    }
    let open = text.find('(')?;
    let ratio: f32 = text.get(1..open)?.parse().ok()?;
    if !ratio.is_finite() {
        return None;
    }
    let (a, rest) = take(text.get(open + 1..)?, depth + 1)?;
    let (b, rest) = take(rest.strip_prefix(',')?, depth + 1)?;
    Some((
        Shape::Split {
            vertical: first == 'v',
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        },
        rest.strip_prefix(')')?,
    ))
}

/// `$XDG_STATE_HOME/slipstream`, else `~/.local/state/slipstream`. The session script already
/// makes this folder for the log.
pub fn state_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local")
                .join("state")
        });
    state.join("slipstream")
}

pub fn path() -> PathBuf {
    state_dir().join(FILE_NAME)
}

/// Reads the record at `path`. A missing file, or one from a newer Slipstream, means nothing.
pub fn read(path: &Path) -> io::Result<Option<Record>> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map(|record| record.filter(|r| !r.is_too_new())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// A record from its text. One whose tiling can't be read is unreadable as a whole, so nothing
/// half-understood is offered.
pub fn parse(text: &str) -> io::Result<Option<Record>> {
    let record: Record =
        toml::from_str(text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if let Some(tiling) = record
        .workspaces
        .iter()
        .find(|tiling| decode(&tiling.tiles).is_none())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workspace {}'s tiling can't be read", tiling.index),
        ));
    }
    Ok(Some(record))
}

/// Writes `record` over the file at `path`, so that nothing ever reads half of it and a power cut
/// leaves the old record or the new one, never neither.
pub fn write(path: &Path, record: &Record) -> io::Result<()> {
    let text =
        toml::to_string(record).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    slipstream_config::replace(path, format!("{HEADER}{text}").as_bytes())
}

/// Deletes the record, for when remembering is turned off. A file that isn't there is a success.
pub fn forget(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `time` as `YYYY-MM-DDTHH:MM:SSZ`. std knows only seconds since the epoch and there's no date
/// crate in the tree, so the date comes from `slipstream_config::civil`.
fn stamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = slipstream_config::civil(days);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(vertical: bool, a: Shape<usize>, b: Shape<usize>) -> Shape<usize> {
        Shape::Split {
            vertical,
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    #[test]
    fn a_tree_survives_the_round_trip() {
        let tree = split(
            true,
            Shape::Leaf(0),
            split(false, Shape::Leaf(1), Shape::Leaf(12)),
        );
        let text = encode(&tree);
        assert_eq!(text, "v0.5(0,h0.5(1,12))");
        assert_eq!(decode(&text), Some(tree));
        assert_eq!(decode("3"), Some(Shape::Leaf(3)));
    }

    #[test]
    fn a_tree_that_is_not_a_tree_is_nothing() {
        for text in [
            "",
            "v0.5(0)",
            "v0.5(0,1",
            "v(0,1)",
            "vx(0,1)",
            "0,1",
            "v0.5(0,1))",
            "q0.5(0,1)",
        ] {
            assert_eq!(decode(text), None, "{text}");
        }
    }

    #[test]
    fn a_deep_tree_is_refused() {
        let deep = format!("{}0{}", "v0.5(".repeat(1000), ",1)".repeat(1000));
        assert_eq!(decode(&deep), None);
        assert_eq!(decode("v0.5(0"), None);
        assert_eq!(decode("v0.5(0,60)"), None, "no workspace has a 61st slot");
        // Sixty windows in a chain is as deep as a real record gets.
        let chain = format!("{}0{}", "v0.5(0,".repeat(59), ")".repeat(59));
        assert!(decode(&chain).is_some(), "{chain}");
        // A ratio out of range still reads, and the layout keeps it within bounds.
        let wide = decode("v2(0,1)").expect("a finite ratio reads");
        let mut layout = crate::layout::Dwindle::new(16, 10);
        layout.rebuild(&wide);
        let Some(Shape::Split { ratio, .. }) = layout.shape() else {
            panic!("the split is rebuilt");
        };
        assert_eq!(ratio, 0.9);
    }

    #[test]
    fn a_record_whose_tree_cannot_be_read_is_unreadable() {
        let text = format!(
            "version = 1\n[[window]]\napp = \"foot\"\nworkspace = 1\n[[workspace]]\nindex = 1\ntiles = \"{}0{}\"\n",
            "v0.5(".repeat(1000),
            ",1)".repeat(1000)
        );
        let err = parse(&text).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(parse("version = 1\n[[workspace]]\nindex = 1\ntiles = \"v0.5(0,1)\"\n").is_ok());
    }

    #[test]
    fn a_record_survives_the_round_trip_through_toml() {
        let record = Record::new(
            2,
            vec![
                Win {
                    app: "org.kde.konsole".into(),
                    workspace: Some(1),
                    slot: 0,
                    focused: true,
                    ..Win::default()
                },
                Win {
                    app: "com.discordapp.discord".into(),
                    slot: 0,
                    in_rain: true,
                    ..Win::default()
                },
            ],
            vec![Tiling {
                index: 1,
                name: String::new(),
                tiles: "0".into(),
            }],
        );
        let text = toml::to_string(&record).unwrap();
        // No title is ever written down, whatever the windows were called.
        assert!(!text.contains("title"), "{text}");
        assert_eq!(parse(&text).unwrap(), Some(record));
    }

    #[test]
    fn the_same_desktop_a_moment_later_is_the_same_desktop() {
        let windows = vec![Win {
            app: "org.kde.konsole".into(),
            workspace: Some(1),
            slot: 0,
            focused: true,
            ..Win::default()
        }];
        let before = Record::new(1, windows.clone(), Vec::new());
        let mut after = Record::new(1, windows, Vec::new());
        after.saved_at = "2031-01-01T00:00:00Z".into();
        assert!(before.same_desktop(&after));
        // Anything that would put a window somewhere else is a different desktop.
        let mut moved = after.clone();
        moved.windows[0].workspace = Some(2);
        assert!(!before.same_desktop(&moved));
        let mut elsewhere = after.clone();
        elsewhere.active_workspace = 3;
        assert!(!before.same_desktop(&elsewhere));
        let mut untiled = after;
        untiled.workspaces.push(Tiling {
            index: 1,
            name: String::new(),
            tiles: "0".into(),
        });
        assert!(!before.same_desktop(&untiled));
    }

    #[test]
    fn a_missing_key_takes_its_default_and_an_unknown_one_is_ignored() {
        let record = parse("version = 1\nfuture-key = 7\n").unwrap().unwrap();
        assert_eq!(record.version, 1);
        assert!(record.windows.is_empty());
        assert!(!record.is_too_new());
    }

    #[test]
    fn a_file_from_a_newer_slipstream_is_left_alone() {
        let record = parse("version = 99\n").unwrap().unwrap();
        assert!(record.is_too_new());
    }

    #[test]
    fn a_runaway_cannot_write_more_than_sixty_windows() {
        let windows = vec![Win::default(); MAX_WINDOWS + 20];
        assert_eq!(
            Record::new(1, windows, Vec::new()).windows.len(),
            MAX_WINDOWS
        );
    }

    #[test]
    fn the_stamp_is_utc_to_the_second() {
        assert_eq!(
            stamp(UNIX_EPOCH + std::time::Duration::from_secs(1_757_703_851)),
            "2025-09-12T19:04:11Z"
        );
        assert_eq!(stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // A leap day, which is what the era shift is for.
        assert_eq!(
            stamp(UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800)),
            "2024-02-29T00:00:00Z"
        );
    }

    #[test]
    fn writing_and_reading_it_back_gives_the_same_record() {
        let dir = crate::files::test_scratch("session");
        let path = dir.join(FILE_NAME);
        let record = Record::new(1, vec![Win::default()], Vec::new());
        write(&path, &record).unwrap();
        assert_eq!(read(&path).unwrap(), Some(record));
        forget(&path).unwrap();
        assert_eq!(read(&path).unwrap(), None);
        // Forgetting what isn't there is a success, so unticking twice isn't an error.
        forget(&path).unwrap();
        let _ = fs::remove_dir(&dir);
    }
}

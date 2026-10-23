//! Where the settings file lives, and reading and replacing it safely.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::Settings;

/// The file's name inside Slipstream's config folder.
pub const FILE_NAME: &str = "settings.toml";

/// Written at the top of the file.
pub(crate) const HEADER: &str = "\
# Slipstream's settings. The Settings app and quick settings write this file,
# and Slipstream applies each change as soon as it's saved. Hand edits work too, but the apps
# don't keep comments.

";

/// An XDG base folder: `$var` when it is set to an absolute path, else `fallback` under `$HOME`.
/// `xdg_home("XDG_STATE_HOME", ".local/state")` is `~/.local/state` unless set otherwise.
pub fn xdg_home(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback)
        })
}

/// `$XDG_CONFIG_HOME/slipstream/settings.toml`, else `~/.config/slipstream/settings.toml`.
pub fn path() -> PathBuf {
    xdg_home("XDG_CONFIG_HOME", ".config")
        .join("slipstream")
        .join(FILE_NAME)
}

/// Reads the settings at `path`. A missing file means every default. The first read of the
/// person's own file moves a `[meter]` from before plugins out of it (`plugins::move_meter_once`),
/// before anything that writes the file back without it can.
pub fn read(path: &Path) -> io::Result<Settings> {
    if path == self::path() {
        crate::plugins::move_meter_once();
    }
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(err) => Err(err),
    }
}

pub(crate) fn parse(text: &str) -> io::Result<Settings> {
    toml::from_str(text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Writes `settings` to `path`, creating its folder, as `replace` does.
pub fn write(path: &Path, settings: &Settings) -> io::Result<()> {
    let text =
        toml::to_string(settings).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    replace(path, format!("{HEADER}{text}").as_bytes())
}

/// Partial files left this long ago were left by a writer that died part-way.
const ABANDONED: Duration = Duration::from_secs(60);

/// Tells apart two partial files one process starts in the same nanosecond.
static PARTIALS: AtomicU64 = AtomicU64::new(0);

/// Replaces the file at `path` with `bytes`, creating its folder, so that nothing ever reads half
/// a file and a power cut leaves either the old file or the new one. The bytes go to a partial
/// file of this writer's own (`<name>.<pid>.<nanos>.<n>.partial`: the compositor and the Settings
/// app can save at the same moment), which is synced, renamed over `path`, and the folder synced
/// so the rename lasts too. Partial files of `path`'s that a dead writer left behind go.
///
/// A `path` that is a symlink (a dotfiles manager's, say) has its target replaced rather than the
/// link, and the file keeps the permissions it had.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, None)
}

/// `replace` for a file only its owner may read, whatever it allowed before: one that holds
/// commands to run.
pub fn replace_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, Some(fs::Permissions::from_mode(0o600)))
}

fn replace_with(path: &Path, bytes: &[u8], permissions: Option<fs::Permissions>) -> io::Result<()> {
    let linked = fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let target = if linked {
        fs::canonicalize(path).ok()
    } else {
        None
    };
    let path = target.as_deref().unwrap_or(path);
    let permissions =
        permissions.or_else(|| fs::metadata(path).ok().map(|meta| meta.permissions()));
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    fs::create_dir_all(dir)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut partial_name = name.to_owned();
    partial_name.push(format!(
        ".{}.{nanos}.{}.partial",
        std::process::id(),
        PARTIALS.fetch_add(1, Ordering::Relaxed)
    ));
    let partial = dir.join(&partial_name);
    let written = File::options()
        .write(true)
        .create_new(true)
        .open(&partial)
        .and_then(|mut file| {
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&partial, path));
    if let Err(err) = written {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    File::open(dir)?.sync_all()?;
    remove_abandoned_partials(dir, name);
    Ok(())
}

/// Removes `name`'s partial files in `dir` that are older than `ABANDONED`. A writer still
/// working on its own is younger than that, so it's left alone.
fn remove_abandoned_partials(dir: &Path, name: &std::ffi::OsStr) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut prefix = name.to_owned();
    prefix.push(".");
    let prefix = prefix.as_encoded_bytes();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let bytes = file_name.as_encoded_bytes();
        if !bytes.starts_with(prefix) || !bytes.ends_with(b".partial") {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ABANDONED);
        if abandoned {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// For a settings file that can't be read: renames it to `settings.toml.broken-YYYYmmdd-HHMMSS`
/// (UTC) beside itself, so none of it is lost, and writes the defaults in its place. Returns
/// where the old file went.
pub fn move_aside(path: &Path) -> io::Result<PathBuf> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    let stamp = format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    let mut aside: OsString = path.as_os_str().to_owned();
    aside.push(format!(".broken-{stamp}"));
    // A second move within the same second keeps the first one's file too.
    let mut target = PathBuf::from(&aside);
    let mut n = 1;
    while target.exists() {
        n += 1;
        let mut numbered = aside.clone();
        numbered.push(format!("-{n}"));
        target = PathBuf::from(numbered);
    }
    fs::rename(path, &target)?;
    write(path, &Settings::default())?;
    Ok(target)
}

/// The civil date of a day counted from 1970-01-01. Howard Hinnant's days-to-civil algorithm,
/// which is public domain.
pub fn civil(days: i64) -> (i64, u32, u32) {
    // Shift to an era starting on 1 March 0000, so a leap day falls at the end of a year.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_file_is_an_error_not_defaults() {
        let err = parse("[wallpaper]\nfade-after-secs = \"soon\"\n").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// A folder of its own for one test, removed when it's dropped: under the workspace's
    /// `target/test-scratch`, not the system's temporary folder, which is memory on some machines.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            let dir = manifest
                .parent()
                .unwrap_or(manifest)
                .join("target/test-scratch")
                .join(format!("config-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn two_writers_never_share_a_temp_name() {
        let scratch = Scratch::new("writers");
        let path = scratch.0.join(FILE_NAME);
        write(&path, &Settings::default()).unwrap();
        let writer = |fade_after_secs: u64| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut settings = Settings::default();
                settings.wallpaper.fade_after_secs = fade_after_secs;
                for _ in 0..200 {
                    write(&path, &settings).unwrap();
                }
            })
        };
        let (a, b) = (writer(30), writer(600));
        let mut reads = 0;
        while !(a.is_finished() && b.is_finished()) {
            let settings = read(&path).expect("every read in between parses");
            assert!([120, 30, 600].contains(&settings.wallpaper.fade_after_secs));
            reads += 1;
        }
        a.join().unwrap();
        b.join().unwrap();
        assert!(reads > 0);
        assert_eq!(scratch.names(), [FILE_NAME]);
    }

    #[test]
    fn a_write_leaves_no_partial_behind() {
        let scratch = Scratch::new("partial");
        let path = scratch.0.join(FILE_NAME);
        // One a writer that died left a while ago, one another writer is working on right now,
        // and another program's file that merely looks like one.
        let abandoned = scratch.0.join("settings.toml.4242.1.0.partial");
        let working = scratch.0.join("settings.toml.4343.2.0.partial");
        let unrelated = scratch.0.join("notes.txt.partial");
        for file in [&abandoned, &working, &unrelated] {
            fs::write(file, "half").unwrap();
        }
        let old = SystemTime::now() - Duration::from_secs(600);
        for file in [&abandoned, &unrelated] {
            File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        write(&path, &Settings::default()).unwrap();
        assert_eq!(
            scratch.names(),
            [
                "notes.txt.partial",
                FILE_NAME,
                "settings.toml.4343.2.0.partial"
            ]
        );
        assert_eq!(read(&path).unwrap(), Settings::default());
    }

    #[test]
    fn move_aside_keeps_the_bytes() {
        let scratch = Scratch::new("aside");
        let path = scratch.0.join(FILE_NAME);
        let broken = "[wallpaper]\nfade-after-secs = \"soon\"\n[[workspaces.list]]\nid = 1\nname = \"Mail\"\n";
        fs::write(&path, broken).unwrap();
        assert!(read(&path).is_err());
        let aside = move_aside(&path).unwrap();
        let aside_name = aside.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            aside_name.starts_with("settings.toml.broken-") && aside_name.len() == 36,
            "{aside_name}"
        );
        assert_eq!(fs::read_to_string(&aside).unwrap(), broken);
        assert_eq!(read(&path).unwrap(), Settings::default());
        // Twice in a second loses neither.
        fs::write(&path, "second = [").unwrap();
        let again = move_aside(&path).unwrap();
        assert_ne!(again, aside);
        assert_eq!(fs::read_to_string(&again).unwrap(), "second = [");
        assert_eq!(fs::read_to_string(&aside).unwrap(), broken);
    }

    #[test]
    fn a_linked_file_keeps_its_link_and_permissions() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new("linked");
        let real = scratch.0.join("dotfiles-settings.toml");
        fs::write(&real, "").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let path = scratch.0.join(FILE_NAME);
        symlink(&real, &path).unwrap();
        let mut settings = Settings::default();
        settings.wallpaper.fade_after_secs = 30;
        write(&path, &settings).unwrap();
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(read(&real).unwrap(), settings);
        let mode = fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_private_file_is_the_owner_s_alone() {
        let scratch = Scratch::new("private");
        let path = scratch.0.join("session.toml");
        fs::write(&path, "").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        replace_private(&path, b"x = 1\n").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(20_708), (2026, 9, 12));
    }
}

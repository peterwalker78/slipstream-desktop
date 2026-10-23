//! What a plugin's folder holds, and its fingerprint: the SHA-256 of every file's path, whether it
//! is a program, and its contents.
//!
//! The folder is walked from its metadata alone first, so one that holds too many files, folders
//! too deep or too many bytes is turned away before anything in it is read. Files are then read
//! a chunk at a time through a descriptor that refuses links and anything but a regular file, so
//! a file swapped for a link or a pipe after the walk is never followed or waited on.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// The most files and folders a plugin's folder may hold.
pub const MOST_ENTRIES: usize = 4096;
/// The deepest its folders may go.
pub const MOST_DEPTH: usize = 16;
/// The most its files may hold, all together, in bytes.
pub const MOST_BYTES: u64 = 64 << 20;

/// Why a folder has no fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unchecked {
    /// It's past one of the limits, found from its metadata before anything was read.
    TooBig(String),
    /// It holds something a plugin can't: a link, or something that is neither a file nor a
    /// folder; or it couldn't be read.
    NotAPlugin(String),
}

impl Unchecked {
    pub fn why(&self) -> &str {
        match self {
            Unchecked::TooBig(why) | Unchecked::NotAPlugin(why) => why,
        }
    }
}

/// A folder's fingerprint, and each file's own, by its path inside the folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents {
    pub fingerprint: String,
    /// Each file's SHA-256, with ` program` after it for one that can be run.
    pub files: BTreeMap<String, String>,
}

/// A file found by the walk.
pub(super) struct Listed {
    /// Its path inside the folder, with `/` between its parts.
    pub name: String,
    pub path: PathBuf,
}

/// Every file in `dir`, from metadata alone, in order of its path inside `dir`.
pub(super) fn list(dir: &Path) -> Result<Vec<Listed>, Unchecked> {
    let not = |err: std::io::Error| Unchecked::NotAPlugin(err.to_string());
    let mut files = Vec::new();
    let (mut entries, mut bytes) = (0usize, 0u64);
    let mut folders = vec![(dir.to_path_buf(), 0usize)];
    while let Some((folder, depth)) = folders.pop() {
        for entry in fs::read_dir(&folder).map_err(not)? {
            let entry = entry.map_err(not)?;
            entries += 1;
            if entries > MOST_ENTRIES {
                return Err(Unchecked::TooBig(format!(
                    "it holds more than {MOST_ENTRIES} files and folders"
                )));
            }
            let path = entry.path();
            let inside = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            let Some(name) = inside.to_str().map(str::to_string) else {
                return Err(Unchecked::NotAPlugin(format!(
                    "{} has a name that isn't text",
                    inside.display()
                )));
            };
            // Not followed: a link is itself what's looked at.
            let meta = entry.metadata().map_err(not)?;
            let kind = meta.file_type();
            if kind.is_symlink() {
                return Err(Unchecked::NotAPlugin(format!(
                    "it holds a link, {name}, and plugins can't"
                )));
            } else if kind.is_dir() {
                if depth + 1 > MOST_DEPTH {
                    return Err(Unchecked::TooBig(format!(
                        "its folders go more than {MOST_DEPTH} deep"
                    )));
                }
                folders.push((path, depth + 1));
            } else if kind.is_file() {
                bytes += meta.len();
                if bytes > MOST_BYTES {
                    return Err(Unchecked::TooBig(format!(
                        "it holds more than {} MB",
                        MOST_BYTES >> 20
                    )));
                }
                files.push(Listed { name, path });
            } else {
                return Err(Unchecked::NotAPlugin(format!(
                    "it holds {name}, which is neither a file nor a folder"
                )));
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// Opens `path` to read, refusing a link, and anything but a regular file without waiting on it.
/// Says too whether it can be run.
pub(super) fn open_regular(path: &Path) -> Result<(File, bool), String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    let meta = file.metadata().map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err(format!("{} changed as it was read", path.display()));
    }
    Ok((file, meta.permissions().mode() & 0o100 != 0))
}

/// Reads `file` a chunk at a time into `each`, at most `left` bytes, taking what it read from
/// `left`.
pub(super) fn stream(
    file: File,
    left: &mut u64,
    mut each: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), Unchecked> {
    let mut reader = file.take(*left + 1);
    let mut chunk = vec![0u8; 64 << 10];
    let mut read = 0u64;
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|err| Unchecked::NotAPlugin(err.to_string()))?;
        if n == 0 {
            break;
        }
        read += n as u64;
        if read > *left {
            return Err(Unchecked::TooBig(format!(
                "it holds more than {} MB",
                MOST_BYTES >> 20
            )));
        }
        each(&chunk[..n]).map_err(Unchecked::NotAPlugin)?;
    }
    *left -= read;
    Ok(())
}

/// What `dir` holds, and its fingerprint.
pub fn contents(dir: &Path) -> Result<Contents, Unchecked> {
    let files = list(dir)?;
    let mut left = MOST_BYTES;
    let mut whole = Sha256::new();
    let mut each = BTreeMap::new();
    for file in files {
        let (opened, program) = open_regular(&file.path).map_err(Unchecked::NotAPlugin)?;
        let mut hash = Sha256::new();
        stream(opened, &mut left, |bytes| {
            hash.update(bytes);
            Ok(())
        })?;
        let digest = hex(&hash.finalize());
        whole.update((file.name.len() as u64).to_le_bytes());
        whole.update(file.name.as_bytes());
        whole.update([u8::from(program)]);
        whole.update(digest.as_bytes());
        let recorded = if program {
            format!("{digest} program")
        } else {
            digest
        };
        each.insert(file.name, recorded);
    }
    Ok(Contents {
        fingerprint: hex(&whole.finalize()),
        files: each,
    })
}

/// `dir`'s fingerprint, or why it has none.
pub fn fingerprint(dir: &Path) -> Result<String, String> {
    contents(dir)
        .map(|contents| contents.fingerprint)
        .map_err(|unchecked| unchecked.why().to_string())
}

/// Whether `name` could be a fingerprint: 64 lower-case hex digits.
pub fn is_fingerprint(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A folder of its own for one test, under the workspace's `target/test-scratch`.
    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("contents-{name}-{}", std::process::id()));
        let _ = super::super::store::remove_tree(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn making_a_file_a_program_changes_the_fingerprint() {
        let dir = scratch("mode");
        fs::write(dir.join("run"), "#!/bin/sh\necho {}\n").unwrap();
        let before = contents(&dir).unwrap();
        fs::set_permissions(dir.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
        let after = contents(&dir).unwrap();
        assert_ne!(before.fingerprint, after.fingerprint);
        assert!(after.files["run"].ends_with(" program"));
        // Only whether it can be run counts, not who else may read it.
        fs::set_permissions(dir.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(contents(&dir).unwrap(), after);
    }

    /// The promise in the guide: a folder past the limits is turned away from its metadata,
    /// before anything in it is read.
    #[test]
    fn a_folder_too_big_to_check_is_refused_without_reading() {
        let dir = scratch("big");
        // A hundred gigabytes that take no room: reading them would take minutes.
        fs::File::create(dir.join("huge"))
            .unwrap()
            .set_len(100 << 30)
            .unwrap();
        let started = Instant::now();
        assert!(matches!(contents(&dir), Err(Unchecked::TooBig(_))));
        assert!(started.elapsed() < Duration::from_secs(1));

        let dir = scratch("many");
        for n in 0..=MOST_ENTRIES {
            fs::write(dir.join(n.to_string()), "").unwrap();
        }
        assert!(matches!(contents(&dir), Err(Unchecked::TooBig(_))));

        let dir = scratch("deep");
        let mut deep = dir.clone();
        for _ in 0..=MOST_DEPTH {
            deep.push("d");
        }
        fs::create_dir_all(&deep).unwrap();
        assert!(matches!(contents(&dir), Err(Unchecked::TooBig(_))));
    }

    #[test]
    fn a_pipe_in_a_plugin_s_folder_is_refused_and_never_waited_on() {
        let dir = scratch("pipe");
        let path = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
        // SAFETY: a valid C string for the path.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(matches!(contents(&dir), Err(Unchecked::NotAPlugin(_))));
        assert!(
            open_regular(&dir.join("pipe")).is_err(),
            "opened without waiting, and refused"
        );
    }
}

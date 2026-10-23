//! Files: this tool's private state folder, and writing files whole.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::env::{Env, uid};

/// Makes the state folder, private to this user, and returns it. Without a runtime folder its
/// parent is a fixed name in /tmp that anyone could have made first, and files written into
/// someone else's folder can be pointed anywhere, so a parent that isn't this user's own is
/// refused.
pub fn make_state(env: &Env) -> Option<PathBuf> {
    let state = env.state_dir();
    if env.runtime_dir().is_none() {
        let parent = state.parent()?;
        let _ = fs::DirBuilder::new().mode(0o700).create(parent);
        let meta = fs::symlink_metadata(parent).ok()?;
        if meta.file_type().is_symlink() || meta.uid() != uid() {
            return None;
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok()?;
    }
    fs::create_dir_all(&state).ok()?;
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).ok()?;
    Some(state)
}

/// The first line of a file, without the spaces and tabs around it, as `read` gives it.
pub fn first_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.split('\n').next().unwrap_or_default();
    Some(line.trim_matches([' ', '\t']).to_owned())
}

/// The first line of a file, if it is a whole number.
pub fn number_in(path: &Path) -> Option<u64> {
    first_line(path)
        .filter(|line| crate::text::is_number(line))?
        .parse()
        .ok()
}

/// `line` and a newline, as the whole of the file.
pub fn write_line(path: &Path, line: impl std::fmt::Display) -> io::Result<()> {
    fs::write(path, format!("{line}\n"))
}

/// Writes a file whole: beside it under a name of its own, then moved over it in one step, so a
/// reader never sees half of it. The new file is readable by this user alone.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_whole(path, contents, 0o600)
}

/// The same with a given mode, less the umask.
pub fn write_whole(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    write_atomically(path, contents, mode, false)
}

/// The same with exactly the given mode, as a file being replaced had.
pub fn replace_with_mode(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    write_atomically(path, contents, mode, true)
}

fn write_atomically(path: &Path, contents: &[u8], mode: u32, exact: bool) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".{}", std::process::id()));
    let temp = path.with_file_name(temp_name);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&temp)?;
        if exact {
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.write_all(contents)?;
        file.sync_data()?;
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_line_is_read_without_the_blanks_around_it() {
        let dir = std::env::temp_dir().join(format!("agent-notify-state-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("started");
        fs::write(&file, "  1234\t\nlater\n").unwrap();
        assert_eq!(number_in(&file), Some(1234));
        fs::write(&file, "12a\n").unwrap();
        assert_eq!(number_in(&file), None);
        write_private(&file, b"whole\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "whole\n");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}

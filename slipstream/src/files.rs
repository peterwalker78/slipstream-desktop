//! Reading files whose names come from outside the compositor: a notification's icon or image
//! path, a desktop entry's `Icon=`, a desktop entry itself, a cursor theme's shapes.
//!
//! Such a name can be anything. `/dev/zero` never ends, a FIFO blocks the first `open` until a
//! writer turns up, and a regular file can be gigabytes, while these reads happen on the event
//! loop (or on a scan the explorer waits for). So only a regular file no bigger than the caller's
//! limit is read, and nothing on the way can block.
//!
//! Symbolic links are followed: icon themes and Flatpak's exported icons and desktop entries are
//! mostly links. What makes a link safe is the check on the file actually opened, which is a
//! regular file or nothing.

use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

/// An icon or picture: the biggest in the installed themes is well under a megabyte.
pub const IMAGE_LIMIT: u64 = 8 << 20;
/// An XCursor file holds every size and animation frame of one shape; the largest installed ones
/// are about 5 MB.
pub const CURSOR_LIMIT: u64 = 16 << 20;
/// A `.desktop` file is text, a few kilobytes even with every translation.
pub const DESKTOP_ENTRY_LIMIT: u64 = 1 << 20;

/// The whole of `path`, if it is a regular file of at most `limit` bytes.
pub fn read_small(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    // Looked at before it's opened: opening a device node can do something by itself (a serial
    // line, a watchdog), so anything but a regular file is refused untouched.
    let named = fs::metadata(path)?;
    check(&named, limit)?;
    // Non-blocking, so a FIFO swapped in since the look can't hold the open up, and never a
    // controlling terminal.
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)?;
    // What was opened has to be what was looked at, and still a regular file within the limit.
    let opened = file.metadata()?;
    check(&opened, limit)?;
    if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
        return Err(refused("the file changed while it was being opened"));
    }
    let mut data = Vec::with_capacity(opened.len() as usize);
    // One byte past the limit, to tell a file that grew from one that fits: a size reported as
    // zero (as the kernel's pseudo-files do) still can't make this read run on.
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(refused("the file is bigger than allowed"));
    }
    Ok(data)
}

fn check(metadata: &Metadata, limit: u64) -> io::Result<()> {
    if !metadata.file_type().is_file() {
        return Err(refused("not a regular file"));
    }
    if metadata.len() > limit {
        return Err(refused("the file is bigger than allowed"));
    }
    Ok(())
}

fn refused(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why)
}

/// An empty folder for one test's files, under the workspace's `target/test-scratch` rather than
/// the system's temporary folder, which is memory on some machines.
#[cfg(test)]
pub fn test_scratch(name: &str) -> std::path::PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest
        .parent()
        .unwrap_or(manifest)
        .join("target")
        .join("test-scratch")
        .join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, sync::mpsc, time::Duration};

    use super::*;

    /// Runs `read` on a thread and waits a moment for it, so a read that blocks fails the test
    /// instead of hanging the suite.
    fn at_once<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(read());
        });
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the read should return at once")
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        test_scratch(&format!("files-{name}"))
    }

    #[test]
    fn a_fifo_is_refused_without_waiting_for_a_writer() {
        let dir = scratch("fifo");
        let fifo = dir.join("icon.png");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let result = at_once(move || read_small(&fifo, IMAGE_LIMIT).map(|data| data.len()));
        fs::remove_dir_all(&dir).unwrap();
        assert!(result.is_err(), "a FIFO is not an icon");
    }

    #[test]
    fn a_device_that_never_ends_is_refused() {
        let result = at_once(|| read_small(Path::new("/dev/zero"), IMAGE_LIMIT).map(|d| d.len()));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let image = at_once(|| crate::paint::load_image(Path::new("/dev/zero"), 16).is_some());
        assert!(!image);
    }

    #[test]
    fn a_small_file_reads_whole_and_a_big_one_not_at_all() {
        let dir = scratch("sizes");
        let small = dir.join("small.svg");
        fs::write(&small, b"<svg/>").unwrap();
        assert_eq!(read_small(&small, 16).unwrap(), b"<svg/>");
        let big = dir.join("big.png");
        fs::write(&big, [0u8; 64]).unwrap();
        let err = read_small(&big, 16).unwrap_err();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_link_to_a_regular_file_is_followed_and_a_link_to_a_device_is_not() {
        let dir = scratch("links");
        let target = dir.join("real.png");
        fs::write(&target, b"icon").unwrap();
        let link = dir.join("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let to_device = dir.join("zero.png");
        std::os::unix::fs::symlink("/dev/zero", &to_device).unwrap();
        let followed = read_small(&link, IMAGE_LIMIT).unwrap();
        let refused = at_once(move || read_small(&to_device, IMAGE_LIMIT).is_err());
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(followed, b"icon");
        assert!(refused);
    }
}

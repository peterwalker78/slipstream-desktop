//! Who is at the other end of a Wayland connection: the process that connected, looked at
//! through `/proc` once, when it connects. Every privilege a client may be given (the capture
//! globals, data control, input methods, layer surfaces, and a card that asks about it by name)
//! is decided from this one look.

use std::{
    ffi::OsStr,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
};

use smithay::reexports::rustix;

/// The executable the portal's backend is installed as.
const PORTAL: &str = "xdg-desktop-portal-wlr";

/// The process at the other end of a connection, once it's proven to be outside any sandbox.
pub struct Peer {
    pub(super) pid: i32,
    /// What `/proc/<pid>/exe` names, if it could be read.
    pub(super) exe: Option<PathBuf>,
    /// Its executable is the portal's (`is_portal_executable`).
    pub(super) portal: bool,
    /// Its executable is a regular file owned by root that nobody else can write
    /// (`is_settled_executable`), so a path remembered for it means the same program tomorrow.
    pub(super) settled: bool,
}

impl Peer {
    /// The path its executable was started from, for naming it on a card and for remembering an
    /// answer about it. `None` where `/proc` couldn't be read, and **`None` for anything a user
    /// could rewrite**: an answer remembered against a path anyone can overwrite is a standing
    /// permission for whatever is put there next.
    pub fn settled_exe(&self) -> Option<&Path> {
        self.settled.then_some(self.exe.as_deref()).flatten()
    }

    /// What to call it on a card: the executable's own name, never the whole path.
    pub fn name(&self) -> Option<String> {
        self.exe
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }
}

/// The connecting process, only when it is still running, its root filesystem could be opened,
/// and that root definitely has no `/.flatpak-info` — the file a Flatpak sees in its own root,
/// and the same check the portal makes — nor the file a plugin's sandbox has in its root
/// (`sandbox::MARKER`). Read through `/proc`, so it is the kernel's view of the
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
    let settled = owner.is_some_and(|(uid, mode)| is_settled_executable(uid, mode));
    // Without the marker, so a program upgraded since it started isn't a different one.
    let exe = path.map(|path| without_deleted(&path).to_path_buf());
    Some(Peer {
        pid,
        exe,
        portal,
        settled,
    })
}

/// Whether a process's root directory definitely has no `.flatpak-info`, nor a plugin sandbox's
/// marker. A file, a directory or
/// a link of that name is there, and a root that can't be looked in is unknowable: either way,
/// not proof of anything but a sandbox.
fn root_is_unsandboxed(root: &OwnedFd) -> bool {
    use rustix::{fs::AtFlags, io::Errno};
    // Flatpak's apps, and Slipstream's own plugins in theirs.
    [".flatpak-info", slipstream_config::plugins::sandbox::MARKER]
        .into_iter()
        .all(|marker| {
            matches!(
                rustix::fs::statat(root, marker, AtFlags::SYMLINK_NOFOLLOW),
                Err(Errno::NOENT)
            )
        })
}

/// Whether an executable, by the path its process was started from and its file's owner and
/// mode, is the portal's: a regular file named `xdg-desktop-portal-wlr`, owned by root and not
/// writable by group or others, so nobody but root can have put it there. A binary replaced since
/// it started reads as `… (deleted)`, which still counts.
fn is_portal_executable(path: &Path, uid: u32, mode: u32) -> bool {
    is_settled_executable(uid, mode)
        && without_deleted(path).file_name() == Some(OsStr::new(PORTAL))
}

/// Whether an executable is one whose path can be written down and trusted again later: a regular
/// file, owned by root, that group and others can't write.
///
/// The test `is_portal_executable` makes about the portal, without the name. An answer remembered
/// against a path anyone can overwrite — anything under a home directory, say — would be a
/// standing permission for whatever program is put there next, which is the whole value of the
/// answer given away.
fn is_settled_executable(uid: u32, mode: u32) -> bool {
    let regular = mode & libc::S_IFMT == libc::S_IFREG;
    let writable_by_others = mode & 0o022 != 0;
    regular && uid == 0 && !writable_by_others
}

/// A path with `/proc`'s ` (deleted)` marker taken off: a binary replaced since it started still
/// names the program it is.
fn without_deleted(path: &Path) -> &Path {
    let bytes = path.as_os_str().as_bytes();
    Path::new(OsStr::from_bytes(
        bytes.strip_suffix(b" (deleted)").unwrap_or(bytes),
    ))
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

/// Runs `look` on the accepted end of a connection made from a child process that left at once,
/// while the parent keeps the connected socket: the process the connection's credentials name
/// has gone, as a zombie, or with `reap`, reaped. `name` keeps each caller's socket apart.
#[cfg(test)]
pub fn with_a_client_that_has_gone<T>(
    name: &str,
    reap: bool,
    look: impl FnOnce(&UnixStream) -> T,
) -> T {
    // An abstract socket has no file, so its name's length doesn't depend on where the
    // repository is checked out.
    let label = format!(
        "slipstream-test-{name}-{}-{}",
        if reap { "reaped" } else { "zombie" },
        std::process::id()
    );
    let bytes = label.as_bytes();
    let listener = std::os::unix::net::UnixListener::bind_addr(
        &<std::os::unix::net::SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(
            bytes,
        )
        .unwrap(),
    )
    .unwrap();
    // Everything the child needs is made before the fork, so the child calls nothing but
    // connect and _exit, which are safe after forking a threaded process.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    // An abstract name starts with a zero byte and runs to the given length.
    for (to, from) in address.sun_path[1..].iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    let length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + bytes.len()) as libc::socklen_t;
    let socket = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    assert!(socket >= 0);
    let child = unsafe { libc::fork() };
    if child == 0 {
        unsafe {
            libc::connect(socket, (&raw const address).cast(), length);
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
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer is remembered against a path, so the path has to mean the same program later.
    #[test]
    fn only_an_executable_nobody_else_can_write_is_worth_remembering() {
        let root_only = libc::S_IFREG | 0o755;
        assert!(is_settled_executable(0, root_only));
        // Owned by the user, so the user can put anything there tomorrow.
        assert!(!is_settled_executable(1000, root_only));
        // Root's, but group or world writable.
        assert!(!is_settled_executable(0, libc::S_IFREG | 0o775));
        assert!(!is_settled_executable(0, libc::S_IFREG | 0o777));
        // Not a regular file at all.
        assert!(!is_settled_executable(0, libc::S_IFLNK | 0o755));

        let settled = |settled| Peer {
            pid: 7,
            exe: Some(PathBuf::from("/usr/bin/example")),
            portal: false,
            settled,
        };
        assert_eq!(
            settled(true).settled_exe(),
            Some(Path::new("/usr/bin/example"))
        );
        assert_eq!(settled(false).settled_exe(), None, "nothing to remember");
        // It can still be named on a card either way.
        assert_eq!(settled(false).name().as_deref(), Some("example"));
    }

    #[test]
    fn a_program_replaced_since_it_started_is_still_the_same_program() {
        assert_eq!(
            without_deleted(Path::new("/usr/bin/grim (deleted)")),
            Path::new("/usr/bin/grim")
        );
        assert_eq!(
            without_deleted(Path::new("/usr/bin/grim")),
            Path::new("/usr/bin/grim")
        );
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
        let plugin = root("plugin");
        std::fs::write(plugin.join(slipstream_config::plugins::sandbox::MARKER), "").unwrap();
        assert!(
            !root_is_unsandboxed(&open(&plugin)),
            "a plugin with windows gets what a Flatpak gets, and no more"
        );
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
}

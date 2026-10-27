//! The sandbox a plugin's program runs in, unless it runs as the person: bubblewrap (`bwrap`), the
//! same tool Flatpak uses, which nearly every distribution packages.
//!
//! Inside, the program has the system's programs, libraries and settings, read-only, with a
//! private `/tmp` and an empty home folder. Of the person's own files it sees only its read-only
//! copy, its own folder for what it keeps, and the folders its manifest says it reads, read-only.
//! It has its own process, IPC and host-name namespaces, so it can't see or signal anything
//! outside, and no network unless it needs `network`. It has no session bus and no display;
//! one that needs `windows` is given the Wayland socket alone, and `MARKER` in its root tells the
//! compositor it is sandboxed, so it gets nothing kept for unsandboxed programs (typing as the
//! keyboard, the clipboard at any time, layers over the desktop). It is ended with whatever
//! started it, and it can't reach the terminal of anything that started it.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use super::{Manifest, Need, ReadFolder};

/// Where `bwrap` is, or why a plugin can't run in its sandbox here: it isn't installed, or it is
/// older than 0.8, which can't stop a program inside making namespaces of its own.
pub fn bwrap() -> Result<&'static Path, &'static str> {
    static FOUND: OnceLock<Result<PathBuf, &'static str>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let path = std::env::var_os("PATH").ok_or(NO_BWRAP)?;
            let bwrap = std::env::split_paths(&path)
                .map(|dir| dir.join("bwrap"))
                .find(|bwrap| is_program(bwrap))
                .ok_or(NO_BWRAP)?;
            let said = std::process::Command::new(&bwrap)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            match version(&said) {
                Some(found) if found < (0, 8) => Err(OLD_BWRAP),
                _ => Ok(bwrap),
            }
        })
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(|why| *why)
}

/// The version `bwrap --version` says, as "bubblewrap 0.12.0".
fn version(said: &str) -> Option<(u32, u32)> {
    let mut parts = said.split_whitespace().nth(1)?.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// A file at the root of every plugin's sandbox, by which the compositor knows a client that
/// connects from one is sandboxed, as Flatpak's `/.flatpak-info` says so of its apps.
pub const MARKER: &str = ".slipstream-plugin";

/// Why a plugin that runs in the sandbox can't run here, when it can't.
pub const NO_BWRAP: &str = "It runs in a sandbox, which needs bubblewrap (bwrap), and that isn't \
                            installed";
pub const OLD_BWRAP: &str = "It runs in a sandbox, which needs bubblewrap 0.8 or newer, and this \
                             one is older";

/// The person's session, as far as the sandbox takes from it.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub home: PathBuf,
    pub runtime: Option<PathBuf>,
    /// `WAYLAND_DISPLAY`: a socket's name in `runtime`, or its path.
    pub wayland: Option<OsString>,
}

impl Session {
    pub fn now() -> Session {
        Session {
            home: crate::xdg_home("HOME", ""),
            runtime: std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute()),
            wayland: std::env::var_os("WAYLAND_DISPLAY").filter(|name| !name.is_empty()),
        }
    }

    /// The Wayland socket's path.
    fn wayland_socket(&self) -> Option<PathBuf> {
        let name = Path::new(self.wayland.as_ref()?);
        if name.is_absolute() {
            Some(name.to_path_buf())
        } else {
            Some(self.runtime.as_ref()?.join(name))
        }
    }
}

/// The folders of the system's own a program needs to run, read-only.
const SYSTEM: &[&str] = &["/usr", "/etc", "/opt"];
/// The top-level folders that are links into `/usr` on most systems now, and real folders on the
/// rest.
const MERGED: &[&str] = &["/bin", "/sbin", "/lib", "/lib64", "/lib32"];

/// `bwrap`'s words for running `manifest`'s program from `dir`, keeping what it keeps in `state`
/// and reading `reads`, each where the plugin looks for it. The program and its own words follow
/// them, after `--`.
pub fn args(
    manifest: &Manifest,
    dir: &Path,
    state: &Path,
    reads: &[ReadFolder],
    session: &Session,
    seccomp: Option<i32>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut add = |words: &[&std::ffi::OsStr]| args.extend(words.iter().map(OsString::from));
    // No namespaces of its own inside: in one it could make a root without the marker, and
    // connect from there as though it weren't sandboxed.
    add(&[
        "--unshare-all".as_ref(),
        "--unshare-user".as_ref(),
        "--disable-userns".as_ref(),
        "--die-with-parent".as_ref(),
        "--new-session".as_ref(),
    ]);
    let marker = Path::new("/").join(MARKER);
    add(&[
        "--ro-bind".as_ref(),
        "/dev/null".as_ref(),
        marker.as_os_str(),
    ]);
    if manifest.needs(Need::Network) {
        add(&["--share-net".as_ref()]);
    }
    for folder in SYSTEM {
        add(&["--ro-bind-try".as_ref(), folder.as_ref(), folder.as_ref()]);
    }
    // Where `/usr/local` is a link out of `/usr`, as it is to `/var/usrlocal` on Fedora's atomic
    // systems, the programs installed there are where it leads.
    if let Ok(local) = std::fs::canonicalize("/usr/local")
        && !local.starts_with("/usr")
    {
        add(&[
            "--ro-bind-try".as_ref(),
            local.as_os_str(),
            local.as_os_str(),
        ]);
    }
    for folder in MERGED {
        match std::fs::read_link(folder) {
            Ok(target) => add(&["--symlink".as_ref(), target.as_os_str(), folder.as_ref()]),
            Err(_) => add(&["--ro-bind-try".as_ref(), folder.as_ref(), folder.as_ref()]),
        }
    }
    add(&[
        "--proc".as_ref(),
        "/proc".as_ref(),
        "--dev".as_ref(),
        "/dev".as_ref(),
    ]);
    add(&[
        "--tmpfs".as_ref(),
        "/tmp".as_ref(),
        "--tmpfs".as_ref(),
        "/run".as_ref(),
    ]);
    if manifest.needs(Need::Network) {
        // Where `/etc/resolv.conf` leads on systems whose resolver keeps it in `/run`.
        for folder in ["/run/systemd/resolve", "/run/NetworkManager"] {
            add(&["--ro-bind-try".as_ref(), folder.as_ref(), folder.as_ref()]);
        }
    }
    if session.home.is_absolute() && session.home != Path::new("/") {
        add(&["--tmpfs".as_ref(), session.home.as_os_str()]);
    }
    if manifest.needs(Need::Windows)
        && let (Some(runtime), Some(socket)) = (&session.runtime, session.wayland_socket())
    {
        add(&["--dir".as_ref(), runtime.as_os_str()]);
        add(&[
            "--ro-bind-try".as_ref(),
            socket.as_os_str(),
            socket.as_os_str(),
        ]);
        add(&[
            "--dev-bind-try".as_ref(),
            "/dev/dri".as_ref(),
            "/dev/dri".as_ref(),
        ]);
    }
    for folder in reads {
        add(&[
            "--ro-bind-try".as_ref(),
            folder.real.as_os_str(),
            folder.named.as_os_str(),
        ]);
    }
    // Last, so that nothing it reads covers them.
    add(&["--ro-bind".as_ref(), dir.as_os_str(), dir.as_os_str()]);
    add(&["--bind".as_ref(), state.as_os_str(), state.as_os_str()]);
    // The system calls it's refused (`seccomp`), read by bubblewrap from this open file.
    if let Some(fd) = seccomp {
        let fd = fd.to_string();
        add(&["--seccomp".as_ref(), fd.as_ref()]);
    }
    add(&["--chdir".as_ref(), dir.as_os_str(), "--".as_ref()]);
    args
}

/// The environment a sandboxed program is given beyond `ENVIRONMENT`: the display, for one that
/// needs `windows`.
pub fn environment(manifest: &Manifest, session: &Session) -> Vec<(&'static str, OsString)> {
    let mut env = Vec::new();
    if manifest.needs(Need::Windows)
        && let (Some(runtime), Some(wayland)) = (&session.runtime, &session.wayland)
    {
        env.push(("XDG_RUNTIME_DIR", runtime.clone().into_os_string()));
        env.push(("WAYLAND_DISPLAY", wayland.clone()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(needs: &[Need]) -> Manifest {
        Manifest {
            name: "Ferry times".into(),
            description: String::new(),
            exec: vec!["./ferry".into()],
            every_secs: None,
            read: Vec::new(),
            needs: needs.to_vec(),
        }
    }

    fn session() -> Session {
        Session {
            home: "/home/ada".into(),
            runtime: Some("/run/user/1000".into()),
            wayland: Some("wayland-1".into()),
        }
    }

    fn words(args: &[OsString]) -> String {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The promise in the guide: unless it says it needs them, a sandboxed plugin has no network,
    /// no display and nothing of the person's home but its own folders and what it reads.
    #[test]
    fn bubblewrap_s_version_is_read_from_what_it_says() {
        assert_eq!(version("bubblewrap 0.12.0\n"), Some((0, 12)));
        assert_eq!(version("bubblewrap 0.6.1"), Some((0, 6)));
        assert!(version("bubblewrap 0.6.1").unwrap() < (0, 8));
        assert_eq!(version(""), None);
    }

    #[test]
    fn a_sandboxed_plugin_gets_only_what_it_needs() {
        let dir = Path::new("/home/ada/.local/state/slipstream/plugins/ferry/ab12");
        let state = Path::new("/home/ada/.local/state/slipstream/plugins/ferry/state");
        let reads = [ReadFolder {
            named: "/home/ada/.cache/ferry".into(),
            real: "/var/home/ada/.cache/ferry".into(),
        }];
        let bare = words(&args(
            &manifest(&[]),
            dir,
            state,
            &reads,
            &session(),
            Some(9),
        ));
        assert!(bare.contains("--seccomp 9 --chdir"), "{bare}");
        assert!(bare.starts_with(
            "--unshare-all --unshare-user --disable-userns --die-with-parent --new-session \
             --ro-bind /dev/null /.slipstream-plugin"
        ));
        assert!(!bare.contains("--share-net"), "{bare}");
        assert!(
            !bare.contains("wayland-1") && !bare.contains("/dev/dri"),
            "{bare}"
        );
        assert!(!bare.contains("/run/user/1000"), "{bare}");
        assert!(bare.contains("--tmpfs /home/ada"), "{bare}");
        assert!(bare.contains("--ro-bind-try /var/home/ada/.cache/ferry /home/ada/.cache/ferry"));
        let own = format!(
            "--ro-bind {0} {0} --bind {1} {1}",
            dir.display(),
            state.display()
        );
        assert!(bare.contains(&own), "its own folders come last: {bare}");
        assert!(bare.ends_with(&format!("--chdir {} --", dir.display())));
        assert!(environment(&manifest(&[]), &session()).is_empty());

        let all = manifest(&[Need::Network, Need::Windows]);
        let given = words(&args(&all, dir, state, &[], &session(), None));
        assert!(given.contains("--share-net"), "{given}");
        assert!(given.contains("--ro-bind-try /run/user/1000/wayland-1 /run/user/1000/wayland-1"));
        assert_eq!(environment(&all, &session()).len(), 2);
    }

    /// A folder it reads can't cover its own: `read = ["/"]` still leaves it its copy and state.
    #[test]
    fn what_it_reads_never_covers_its_own_folders() {
        let dir = Path::new("/home/ada/plugin");
        let state = Path::new("/home/ada/state");
        let everything = ReadFolder {
            named: "/".into(),
            real: "/".into(),
        };
        let given = words(&args(
            &manifest(&[]),
            dir,
            state,
            &[everything],
            &session(),
            None,
        ));
        let read = given.find("--ro-bind-try / /").unwrap();
        let own = given.find("--bind /home/ada/state").unwrap();
        assert!(read < own, "{given}");
    }
}

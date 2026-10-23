//! Where Slipstream keeps a plugin it has been told to run: `$XDG_STATE_HOME/slipstream/plugins/
//! <id>/`. Turning a plugin on copies its folder into `<fingerprint>/` there, read-only, and the
//! plugin only ever runs from that copy, so a change to its folder never runs until it is turned
//! on again. `current` is a link to the copy that was approved, which moves in one step, so
//! commands a plugin offers outside its card (linked from `current/…`) run what was approved too.
//! `state/` is the plugin's own, for what it keeps.

use std::{
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{
    Plugin,
    contents::{self, MOST_BYTES, is_fingerprint},
};

/// The link to the approved copy, in a plugin's store.
pub const CURRENT: &str = "current";
/// The plugin's own folder in its store, for what it keeps.
pub const STATE: &str = "state";
/// A copy still being written.
const PARTIAL: &str = ".new-";

/// Where every plugin's store is.
pub fn root() -> PathBuf {
    crate::xdg_home("XDG_STATE_HOME", ".local/state")
        .join("slipstream")
        .join("plugins")
}

/// Plugin `id`'s store.
pub fn of(id: &str) -> PathBuf {
    root().join(id)
}

/// A copy of `plugin`'s folder in `store` that holds exactly what its fingerprint says: the one
/// kept from before if it still does, else a new one, written, checked against the fingerprint
/// and made read-only. Altered copies are thrown away and made again.
pub fn snapshot(plugin: &Plugin, store: &Path) -> Result<PathBuf, String> {
    let fingerprint = &plugin.fingerprint;
    if plugin.too_big.is_some() || !is_fingerprint(fingerprint) {
        return Err("its folder couldn't be checked".into());
    }
    let kept = store.join(fingerprint);
    if kept.exists() {
        if verify(&kept, fingerprint).is_ok() {
            return Ok(kept);
        }
        remove_tree(&kept).map_err(|err| err.to_string())?;
    }
    fs::create_dir_all(store).map_err(|err| err.to_string())?;
    let new = store.join(format!("{PARTIAL}{fingerprint}"));
    remove_tree(&new).map_err(|err| err.to_string())?;
    if let Err(why) = copy(&plugin.dir, &new) {
        let _ = remove_tree(&new);
        return Err(why);
    }
    // What was written is what's checked, so a folder that changed while it was copied is caught.
    if contents::fingerprint(&new).as_ref() != Ok(fingerprint) {
        let _ = remove_tree(&new);
        return Err("its folder changed while it was being copied".into());
    }
    fs::rename(&new, &kept).map_err(|err| err.to_string())?;
    seal(&kept).map_err(|err| err.to_string())?;
    Ok(kept)
}

/// Whether `copy` still holds what `fingerprint` says.
pub fn verify(copy: &Path, fingerprint: &str) -> Result<(), String> {
    match contents::fingerprint(copy) {
        Ok(now) if now == fingerprint => Ok(()),
        Ok(_) => Err("its approved copy has been changed, so it doesn't run".into()),
        Err(why) => Err(format!("its approved copy can't be checked: {why}")),
    }
}

/// Copies the files in `from` into a new folder `to`, read-only, each a program if it was one.
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    let files = contents::list(from).map_err(|unchecked| unchecked.why().to_string())?;
    fs::create_dir(to).map_err(|err| err.to_string())?;
    let mut left = MOST_BYTES;
    for file in files {
        let target = to.join(&file.name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let (source, program) = contents::open_regular(&file.path)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)
            .map_err(|err| err.to_string())?;
        contents::stream(source, &mut left, |bytes| {
            io::Write::write_all(&mut out, bytes).map_err(|err| err.to_string())
        })
        .map_err(|unchecked| unchecked.why().to_string())?;
        let mode = if program { 0o555 } else { 0o444 };
        fs::set_permissions(&target, fs::Permissions::from_mode(mode))
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Makes every folder in `dir` read-only too, so nothing can be added to the copy.
fn seal(dir: &Path) -> io::Result<()> {
    let mut folders = vec![dir.to_path_buf()];
    let mut all = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                folders.push(entry.path());
            }
        }
        all.push(folder);
    }
    for folder in all {
        fs::set_permissions(folder, fs::Permissions::from_mode(0o555))?;
    }
    Ok(())
}

/// Points `store`'s `current` at its copy `fingerprint`, in one step: the new link is made under
/// another name and renamed over the old.
pub fn point_current(store: &Path, fingerprint: &str) -> io::Result<()> {
    if current(store).as_deref() == Some(fingerprint) {
        return Ok(());
    }
    let new = store.join(format!("{PARTIAL}{CURRENT}"));
    let _ = fs::remove_file(&new);
    std::os::unix::fs::symlink(fingerprint, &new)?;
    fs::rename(&new, store.join(CURRENT))
}

/// Takes `store`'s `current` away: the plugin is no longer approved.
pub fn clear_current(store: &Path) {
    let _ = fs::remove_file(store.join(CURRENT));
}

/// The copy `store`'s `current` points at.
pub fn current(store: &Path) -> Option<String> {
    let target = fs::read_link(store.join(CURRENT)).ok()?;
    target
        .to_str()
        .filter(|name| is_fingerprint(name))
        .map(str::to_string)
}

/// Removes the copies in `store` other than `keep`, the one `current` points at and those
/// `in_use` says something is still running from, and any copy left half written.
pub fn prune(store: &Path, keep: Option<&str>, in_use: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = fs::read_dir(store) else {
        return;
    };
    let current = current(store);
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let partial = name.starts_with(PARTIAL) && name != format!("{PARTIAL}{CURRENT}");
        if !(is_fingerprint(&name) || partial) {
            continue;
        }
        let path = entry.path();
        if keep == Some(name.as_str()) || current.as_deref() == Some(name.as_str()) || in_use(&path)
        {
            continue;
        }
        let _ = remove_tree(&path);
    }
}

/// Removes plugin `id`'s store: its copies, its `current` link and what it kept.
pub fn remove(id: &str) -> io::Result<()> {
    remove_tree(&of(id))
}

/// Removes `path` and everything in it, read-only folders included. A link is removed, never
/// followed.
pub fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
        Ok(meta) if !meta.is_dir() => return fs::remove_file(path),
        Ok(_) => {}
    }
    let mut folders = vec![path.to_path_buf()];
    while let Some(folder) = folders.pop() {
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                folders.push(entry.path());
            }
        }
    }
    fs::remove_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("store-{name}-{}", std::process::id()));
        let _ = remove_tree(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn timer(root: &Path, program: &str) -> Plugin {
        let dir = root.join("source").join("timer");
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(
            dir.join(super::super::MANIFEST),
            "name = \"Timer\"\nexec = [\"./timer\"]\n",
        )
        .unwrap();
        fs::write(dir.join("timer"), program).unwrap();
        fs::set_permissions(dir.join("timer"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("lib").join("words"), "tick tock").unwrap();
        super::super::read(&dir).unwrap()
    }

    /// The promise in the guide: a plugin runs from a read-only copy of what was approved.
    #[test]
    fn an_approved_copy_is_read_only_and_holds_what_was_approved() {
        let root = scratch("copy");
        let plugin = timer(&root, "#!/bin/sh\necho {}\n");
        let store = root.join("store");
        let copy = snapshot(&plugin, &store).unwrap();
        assert_eq!(copy, store.join(&plugin.fingerprint));
        verify(&copy, &plugin.fingerprint).unwrap();
        let program = fs::metadata(copy.join("timer"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(program & 0o777, 0o555);
        let words = fs::metadata(copy.join("lib/words")).unwrap().permissions();
        assert_eq!(words.mode() & 0o777, 0o444);
        for folder in [&copy, &copy.join("lib")] {
            let mode = fs::metadata(folder).unwrap().permissions().mode();
            assert_eq!(mode & 0o222, 0, "{} can be written to", folder.display());
        }
        assert!(fs::write(copy.join("added"), "x").is_err());

        // Altered all the same, it no longer runs, and is made again from the folder.
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(copy.join("added"), "x").unwrap();
        assert!(verify(&copy, &plugin.fingerprint).is_err());
        assert_eq!(snapshot(&plugin, &store).unwrap(), copy);
        verify(&copy, &plugin.fingerprint).unwrap();
    }

    #[test]
    fn a_folder_that_changed_since_it_was_read_isn_t_copied() {
        let root = scratch("raced");
        let plugin = timer(&root, "#!/bin/sh\necho {}\n");
        fs::write(plugin.dir.join("timer"), "#!/bin/sh\necho changed\n").unwrap();
        let store = root.join("store");
        assert!(snapshot(&plugin, &store).is_err());
        assert!(!store.join(&plugin.fingerprint).exists());
    }

    /// `current` moves to the copy approved in one step, and old copies go once nothing uses them.
    #[test]
    fn current_moves_to_the_approved_copy_and_old_copies_go() {
        let root = scratch("current");
        let store = root.join("store");
        let first = timer(&root, "#!/bin/sh\necho one\n");
        snapshot(&first, &store).unwrap();
        point_current(&store, &first.fingerprint).unwrap();
        assert_eq!(current(&store).as_deref(), Some(first.fingerprint.as_str()));
        let second = timer(&root, "#!/bin/sh\necho two\n");
        snapshot(&second, &store).unwrap();
        point_current(&store, &second.fingerprint).unwrap();
        assert_eq!(
            fs::read_to_string(store.join(CURRENT).join("timer")).unwrap(),
            "#!/bin/sh\necho two\n"
        );
        let old = store.join(&first.fingerprint);
        prune(&store, Some(&second.fingerprint), &|path| path == old);
        assert!(old.exists(), "still in use");
        fs::create_dir_all(store.join(STATE)).unwrap();
        prune(&store, Some(&second.fingerprint), &|_| false);
        assert!(!old.exists());
        assert!(store.join(&second.fingerprint).exists());
        assert!(
            store.join(STATE).exists(),
            "what the plugin keeps is its own"
        );
        clear_current(&store);
        assert_eq!(current(&store), None);
    }

    /// The promise in the guide: removing a plugin removes everything Slipstream kept for it.
    #[test]
    fn removing_a_store_takes_its_read_only_copies_too() {
        let root = scratch("remove");
        let store = root.join("store");
        let plugin = timer(&root, "#!/bin/sh\necho {}\n");
        snapshot(&plugin, &store).unwrap();
        point_current(&store, &plugin.fingerprint).unwrap();
        remove_tree(&store).unwrap();
        assert!(!store.exists());
    }
}

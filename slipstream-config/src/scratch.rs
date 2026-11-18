//! A folder of its own for one test, under the workspace's `target/test-scratch` rather than the
//! system's temporary folder, which is memory on some machines and which no plugin may read. It's
//! removed when the test ends, passed or failed, so runs don't pile up folders (some tests leave
//! files that claim to be 100 GB, which a backup would try to copy).
//!
//! Only for tests: the crates that use it ask for the `test-scratch` feature in their
//! dev-dependencies, so it's never in a real build.

use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use crate::plugins::store::remove_tree;

/// An empty folder named `name` and the test process's id, gone when this is dropped.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        // The workspace's own path, with no `..` in it: sandbox tests compare paths exactly.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("{name}-{}", std::process::id()));
        // Sealed plugin copies are read-only; `remove_tree` opens them up first.
        let _ = remove_tree(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = remove_tree(&self.0);
    }
}

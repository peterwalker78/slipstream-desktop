//! Scratch folders for tests. Reading files whose names come from outside is
//! `slipstream_config::files`.

/// An empty folder for one test's files, under the workspace's `target/test-scratch` rather than
/// the system's temporary folder, which is memory on some machines.
#[cfg(test)]
pub fn test_scratch(name: &str) -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest
        .parent()
        .unwrap_or(manifest)
        .join("target")
        .join("test-scratch")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::mpsc, time::Duration};

    #[test]
    fn a_device_that_never_ends_is_no_image() {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::paint::load_image(Path::new("/dev/zero"), 16).is_some());
        });
        let image = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the read should return at once");
        assert!(!image);
    }
}

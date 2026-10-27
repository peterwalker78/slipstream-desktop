//! Making the folders a plugin changes as it is switched on: in a test binary of its own, since it
//! points the home folder at a scratch folder.

use std::path::PathBuf;

use slipstream_config::plugins::{self, Manifest, Need};

fn writing(folders: &[&str]) -> Manifest {
    Manifest {
        name: "Log".into(),
        exec: vec!["./log".into()],
        needs: vec![Need::Write],
        write: folders.iter().map(|folder| folder.to_string()).collect(),
        ..Manifest::default()
    }
}

/// The promise in the guide: a folder a plugin changes that isn't there yet is made as it is
/// switched on, only in the home folder and only where a plugin could be given it.
#[test]
fn a_missing_folder_to_change_is_made_only_where_a_plugin_could_have_it() {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-folders");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: the only test in this binary, so no other thread reads the environment.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
        std::env::set_var("XDG_STATE_HOME", home.join(".local/state"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
    }

    let log = writing(&["$HOME/Documents/Log"]);
    assert_eq!(plugins::missing_write_folders(&log), ["~/Documents/Log"]);
    plugins::make_write_folders(&log).unwrap();
    assert!(home.join("Documents/Log").is_dir());
    assert!(plugins::missing_write_folders(&log).is_empty());
    assert_eq!(
        plugins::write_folders(&log).unwrap().len(),
        1,
        "and it's given"
    );

    // A link out of the home folder is followed before it's checked.
    std::os::unix::fs::symlink("/usr", home.join("System")).unwrap();
    for refused in [
        "$HOME/System/Log",
        "$HOME/.hidden/Log",
        "$HOME/bin/Log",
        "/opt/log",
        "$XDG_CACHE_HOME/log",
    ] {
        let asks = writing(&[refused]);
        assert!(plugins::make_write_folders(&asks).is_err(), "{refused}");
    }
    assert!(!home.join(".hidden").exists() && !home.join(".cache").exists());

    // Without `write` nothing is made.
    let mut quiet = writing(&["$HOME/Quiet"]);
    quiet.needs.clear();
    plugins::make_write_folders(&quiet).unwrap();
    assert!(!home.join("Quiet").exists());
}

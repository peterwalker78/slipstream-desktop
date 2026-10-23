//! A `[meter]` from before plugins, moved out when the settings file is first read: in a test
//! binary of its own, since it points the XDG folders at a scratch folder.

use std::path::PathBuf;

use slipstream_config::{Settings, path, plugins, read, write};

/// The promise in the guide: the meter's command becomes a plugin even when the Settings app,
/// which writes back only the keys it knows, saves the file before the compositor ever starts.
#[test]
fn a_meter_command_survives_the_settings_app_saving_first() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("meter-read");
    let _ = std::fs::remove_dir_all(&root);
    // SAFETY: the only test in this binary, so no other thread reads the environment.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_STATE_HOME", root.join("state"));
    }
    let file = path();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[meter]\ncommand = \"uptime\"\n").unwrap();

    // What the Settings app does on a change: read, then write back what it knows.
    let settings: Settings = read(&file).unwrap();
    write(&file, &settings).unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("uptime"));

    let meter = plugins::read(&plugins::own_dir().join("meter")).unwrap();
    assert_eq!(meter.manifest.exec[2], "uptime");
    assert!(root.join("state/slipstream/meter-moved").exists());
}

//! Reading and writing the real file, in a scratch folder under `target/`.

use std::path::PathBuf;

use slipstream_config::{
    Appearance, Borders, Clipboard, ColourScheme, Display, Lock, Meter, Motion, NightSchedule,
    Notifications, Session, Settings, Sound, Wallpaper, read, write,
};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_missing_file_means_every_default() {
    let dir = scratch("missing");
    assert_eq!(
        read(&dir.join("settings.toml")).unwrap(),
        Settings::default()
    );
}

#[test]
fn written_settings_read_back_and_leave_nothing_else_behind() {
    let dir = scratch("written");
    let path = dir.join("slipstream").join("settings.toml");
    let settings = Settings {
        appearance: Appearance {
            colour_scheme: ColourScheme::Light,
        },
        wallpaper: Wallpaper {
            fade_after_secs: 600,
            variations: vec!["decode".into()],
            change_every_mins: 30,
        },
        motion: Motion { reduced: true },
        display: Display {
            night_light: true,
            night_light_schedule: NightSchedule::Sunset,
            ..Default::default()
        },
        notifications: Notifications {
            do_not_disturb: true,
            wait_while_typing: false,
            longest_wait_mins: 5,
            sounds: false,
            sound_theme: "harbour".into(),
        },
        borders: Borders {
            selected_tile: "#3cf0c0".into(),
            bullet_time: "#ff9b3d".into(),
        },
        session: Session {
            remember: true,
            reopen_without_asking: true,
        },
        lock: Lock {
            after_idle_mins: 15,
            before_sleep: false,
        },
        sound: Sound { volume_blip: false },
        clipboard: Clipboard { history: false },
        meter: Meter {
            command: "quota --json".into(),
            every_secs: 120,
        },
        workspaces: Default::default(),
    };
    write(&path, &settings).unwrap();
    assert_eq!(read(&path).unwrap(), settings);

    write(&path, &Settings::default()).unwrap();
    assert_eq!(read(&path).unwrap(), Settings::default());
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["settings.toml"]);
}

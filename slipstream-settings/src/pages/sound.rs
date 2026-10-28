//! The volume, and the sounds Slipstream makes itself.

use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use slipstream_config::sounds::{self, Preset};

use super::*;

/// The speakers or headphones playing now, as PipeWire names them.
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Sound",
        "The volume, and the sounds Slipstream makes itself",
    );
    let keys = group(&page, "Volume");

    if let Some(reading) = volume() {
        let slider = volume_slider(reading, store);
        row(
            &keys,
            "Volume",
            Some("The speakers or headphones playing now."),
            &slider,
        );
    }

    let blip = gtk::CheckButton::new();
    blip.set_active(store.get().sound.volume_blip);
    row(
        &keys,
        "Click when the volume changes",
        Some(
            "A short click through the speakers at the new level, from the volume keys or a \
             volume slider, as macOS and Windows do, so the volume can be heard as well as seen. \
             Never on mute, and never on brightness.",
        ),
        &blip,
    );
    let blip_store = store.clone();
    blip.connect_toggled(move |blip| {
        let on = blip.is_active();
        blip_store.change(move |settings| settings.sound.volume_blip = on);
    });

    let current = store.get().sound.volume_step() as u64;
    let steps = choices(&STEP_CHOICES, current);
    let names: Vec<String> = steps.iter().map(|step| format!("{step}%")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let step = gtk::DropDown::from_strings(&names);
    step.set_selected(steps.iter().position(|&s| s == current).unwrap_or(0) as u32);
    let step_store = store.clone();
    step.connect_selected_notify(move |pick| {
        if let Some(&step) = steps.get(pick.selected() as usize) {
            step_store.change(|settings| settings.sound.volume_step = step as u8);
        }
    });
    row(
        &keys,
        "Volume key step",
        Some("How far each press of the volume keys turns the sound up or down."),
        &step,
    );

    let heard = group(&page, "Sounds");
    let click = sound_choice(
        store,
        sounds::CLICKS,
        false,
        |settings| settings.sound.click.clone(),
        |settings, choice| settings.sound.click = choice,
    );
    row(
        &heard,
        "Volume click",
        Some("A short sound works best: one plays for every step."),
        &click,
    );
    let ding = sound_choice(
        store,
        sounds::DINGS,
        true,
        |settings| settings.notifications.ding.clone(),
        |settings, choice| settings.notifications.ding = choice,
    );
    row(
        &heard,
        "Notification sound",
        Some(
            "Each app's own sound, or one for every notification. With one chosen, \
             notifications that would be silent make it too.",
        ),
        &ding,
    );

    let bullet = group(&page, "Bullet time");
    let muffle = gtk::CheckButton::new();
    muffle.set_active(store.get().sound.muffle_bullet_time);
    row(
        &bullet,
        "Muffle what's playing",
        Some(
            "Sound goes quieter and duller while bullet time is open, and comes back as you leave.",
        ),
        &muffle,
    );
    let store = store.clone();
    muffle.connect_toggled(move |muffle| {
        let on = muffle.is_active();
        store.change(move |settings| settings.sound.muffle_bullet_time = on);
    });
    page.upcast()
}

/// A slider for the volume, 0 to 100% as the volume keys go. It follows the volume wherever it's
/// changed, from the keys, quick settings or another app, as PipeWire reports each change. Moving
/// it sets the level once the slider pauses, so a drag doesn't start a program for every step on
/// the way, and clicks at the new level as the volume keys do.
fn volume_slider((level, muted): (u8, bool), store: &Store) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_round_digits(0);
    scale.set_draw_value(false);
    scale.set_width_request(200);
    scale.set_value(level as f64);
    // Set while the slider is being moved to match the speakers, so that isn't sent back to them.
    let reading = Rc::new(Cell::new(false));
    let muted = Rc::new(Cell::new(muted));
    // Until when the slider's own last move is still being made: the speakers' reports from in
    // between would pull it back to an older level.
    let moved_until = Rc::new(Cell::new(None::<std::time::Instant>));
    let catch_up = {
        let (scale, reading) = (scale.downgrade(), Rc::clone(&reading));
        let (muted, moved_until) = (Rc::clone(&muted), Rc::clone(&moved_until));
        move || {
            let moving = moved_until
                .get()
                .is_some_and(|until| std::time::Instant::now() < until);
            let Some(scale) = scale.upgrade().filter(|_| !moving) else {
                return;
            };
            if let Some((level, now_muted)) = volume() {
                muted.set(now_muted);
                reading.set(true);
                scale.set_value(level as f64);
                reading.set(false);
            }
        }
    };
    scale.connect_map({
        let catch_up = catch_up.clone();
        move |_| catch_up()
    });
    follow_changes(&scale, catch_up);

    let waiting = Rc::new(Cell::new(false));
    let store = store.clone();
    scale.connect_value_changed(move |scale| {
        if reading.get() {
            return;
        }
        moved_until.set(Some(std::time::Instant::now() + MOVE_SETTLES));
        if waiting.replace(true) {
            return;
        }
        let (scale, waiting) = (scale.downgrade(), Rc::clone(&waiting));
        let sound = store.get().sound;
        let click = (sound.volume_blip && !muted.get()).then_some(sound.click);
        gtk::glib::timeout_add_local_once(Duration::from_millis(60), move || {
            waiting.set(false);
            if let Some(scale) = scale.upgrade() {
                set_volume(scale.value().round() as u8, click);
            }
        });
    });
    scale
}

/// How long after the slider last moved the speakers' reports are left alone.
const MOVE_SETTLES: Duration = Duration::from_millis(800);

/// Calls `catch_up` soon after each change PipeWire reports to the speakers in use, or to which
/// speakers those are, for as long as `scale` is there. Without `pactl` the slider still reads
/// the volume each time the page is shown.
fn follow_changes(scale: &gtk::Scale, catch_up: impl Fn() + Clone + 'static) {
    let Ok(watch) = gtk::gio::Subprocess::newv(
        &[
            std::ffi::OsStr::new("pactl"),
            std::ffi::OsStr::new("subscribe"),
        ],
        gtk::gio::SubprocessFlags::STDOUT_PIPE | gtk::gio::SubprocessFlags::STDERR_SILENCE,
    ) else {
        return;
    };
    scale.connect_destroy({
        let watch = watch.clone();
        move |_| watch.force_exit()
    });
    let Some(events) = watch.stdout_pipe() else {
        return;
    };
    let lines = gtk::gio::DataInputStream::new(&events);
    gtk::glib::spawn_future_local(async move {
        // A burst of reports, as from a drag in quick settings, is read once.
        let due = Rc::new(Cell::new(false));
        while let Ok(Some(line)) = lines
            .read_line_utf8_future(gtk::glib::Priority::DEFAULT)
            .await
        {
            if !(line.contains(" on sink ") || line.contains(" on server")) || due.replace(true) {
                continue;
            }
            let (due, catch_up) = (Rc::clone(&due), catch_up.clone());
            gtk::glib::timeout_add_local_once(Duration::from_millis(50), move || {
                due.set(false);
                catch_up();
            });
        }
    });
}

/// The volume now, in percent, and whether it's muted, or `None` without PipeWire's `wpctl` to
/// ask.
fn volume() -> Option<(u8, bool)> {
    let output = Command::new("wpctl")
        .args(["get-volume", SINK])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    parse_volume(&String::from_utf8_lossy(&output.stdout))
}

/// `Volume: 0.75 [MUTED]` is 75%, muted.
fn parse_volume(text: &str) -> Option<(u8, bool)> {
    let level: f64 = text.split_whitespace().nth(1)?.parse().ok()?;
    let percent = (level * 100.0).round().clamp(0.0, 100.0) as u8;
    Some((percent, text.contains("[MUTED]")))
}

/// Sets the volume, on a thread of its own so the slider doesn't wait for `wpctl`, then plays
/// `click` if there is one, so it sounds at the level just set.
fn set_volume(percent: u8, click: Option<String>) {
    let level = format!("{:.2}", percent.min(100) as f64 / 100.0);
    std::thread::spawn(move || {
        let set = Command::new("wpctl")
            .args(["set-volume", "-l", "1.0", SINK, &level])
            .status();
        if let Err(err) = set {
            eprintln!("slipstream-settings: couldn't set the volume: {err}");
            return;
        }
        if let Some(click) = click
            && sounds::due()
        {
            play(&click, sounds::CLICKS, "");
        }
    });
}

/// Plays `choice`, one of `presets`, a sound from `theme` or a file, without waiting for it. Not
/// while the last one is still going, so clicking away at a slider or a list only ever has one
/// sound playing, and a stuck one is stopped soon after.
fn play(choice: &str, presets: &[Preset], theme: &str) {
    let path = match sounds::themed(choice) {
        Some(name) => sounds::find(name, theme, &sounds::theme_bases(), &read_index),
        None => sounds::file(choice, presets),
    };
    if let Some(path) = path {
        let mut command = Command::new("pw-play");
        command
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(err) = PLAYER.play(command) {
            eprintln!("slipstream-settings: couldn't play a sound: {err}");
        }
    }
}

/// Every sound Settings plays. As long as the desktop lets a notification sound go on.
static PLAYER: sounds::Player = sounds::Player::new(Duration::from_secs(8));

/// A sound theme's index, if it's a small regular file.
fn read_index(path: &Path) -> Option<String> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= 1 << 20)?;
    std::fs::read_to_string(path).ok()
}

/// What the list of sounds holds, row by row, as the settings file says each.
const PICK_A_FILE: &str = "";

/// A choice of sound: `presets` (after each app's own, with `apps_own`), a file of one's own, and a
/// button to hear it. Picking one plays it.
fn sound_choice(
    store: &Store,
    presets: &'static [Preset],
    apps_own: bool,
    get: fn(&Settings) -> String,
    set: fn(&mut Settings, String),
) -> gtk::Box {
    let names = gtk::StringList::new(&[]);
    let list = gtk::DropDown::new(Some(names.clone()), gtk::Expression::NONE);
    let hear = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Play")
        .valign(gtk::Align::Center)
        .build();
    // Each row's choice, as the settings file says it.
    let choices: Rc<RefCell<Vec<String>>> = Rc::default();
    // Set while the list is being changed to match, so its handler stays quiet.
    let syncing = Rc::new(Cell::new(false));
    let fill = {
        let (names, list, hear) = (names.clone(), list.clone(), hear.clone());
        let (choices, syncing) = (Rc::clone(&choices), Rc::clone(&syncing));
        move |current: &str| {
            let mut rows: Vec<(String, String)> = Vec::new();
            if apps_own {
                rows.push((sounds::APPS_OWN.into(), "Each app's own".into()));
                rows.extend(
                    sounds::THEMED
                        .iter()
                        .map(|(id, name, _)| (id.to_string(), name.to_string())),
                );
            }
            rows.extend(
                presets
                    .iter()
                    .map(|preset| (preset.id.to_string(), preset.name.to_string())),
            );
            if sounds::is_own_file(current) {
                let name = Path::new(current)
                    .file_name()
                    .map_or_else(|| current.to_string(), |name| name.to_string_lossy().into());
                rows.push((current.to_string(), name));
            }
            rows.push((PICK_A_FILE.into(), "Choose a file…".into()));
            syncing.set(true);
            let shown: Vec<&str> = rows.iter().map(|(_, name)| name.as_str()).collect();
            names.splice(0, names.n_items(), &shown);
            list.set_selected(
                rows.iter()
                    .position(|(choice, _)| choice == current)
                    .unwrap_or(0) as u32,
            );
            syncing.set(false);
            hear.set_sensitive(current != sounds::APPS_OWN);
            *choices.borrow_mut() = rows.into_iter().map(|(choice, _)| choice).collect();
        }
    };
    fill(&get(&store.get()));

    let hear_store = store.clone();
    hear.connect_clicked(move |_| {
        let settings = hear_store.get();
        let (choice, theme) = (get(&settings), settings.notifications.sound_theme);
        std::thread::spawn(move || play(&choice, presets, &theme));
    });
    let store = store.clone();
    list.connect_selected_notify(move |list| {
        if syncing.get() {
            return;
        }
        let Some(choice) = choices.borrow().get(list.selected() as usize).cloned() else {
            return;
        };
        if choice != PICK_A_FILE {
            fill(&choice);
            let saved = choice.clone();
            store.change(move |settings| set(settings, saved));
            let theme = store.get().notifications.sound_theme;
            std::thread::spawn(move || play(&choice, presets, &theme));
            return;
        }
        // A file of one's own. Turning away from the dialog leaves the sound as it was.
        let window = list.root().and_downcast::<gtk::Window>();
        let (store, fill) = (store.clone(), fill.clone());
        gtk::glib::spawn_future_local(async move {
            let audio = gtk::FileFilter::new();
            audio.set_name(Some("Sounds"));
            audio.add_mime_type("audio/*");
            let dialog = gtk::FileDialog::builder()
                .title("Choose a sound")
                .modal(true)
                .default_filter(&audio)
                .build();
            let path = dialog
                .open_future(window.as_ref())
                .await
                .ok()
                .and_then(|file| file.path());
            let Some(path) = path.map(|path| path.to_string_lossy().into_owned()) else {
                fill(&get(&store.get()));
                return;
            };
            fill(&path);
            let saved = path.clone();
            store.change(move |settings| set(settings, saved));
            std::thread::spawn(move || play(&path, presets, ""));
        });
    });

    let control = gtk::Box::builder().spacing(8).build();
    control.append(&list);
    control.append(&hear);
    control
}

/// The steps offered for the volume keys, in percent.
const STEP_CHOICES: [u64; 3] = [2, 5, 10];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_is_read_from_wpctl() {
        assert_eq!(parse_volume("Volume: 0.75\n"), Some((75, false)));
        assert_eq!(parse_volume("Volume: 0.40 [MUTED]\n"), Some((40, true)));
        assert_eq!(parse_volume("Volume: 1.30\n"), Some((100, false)));
        assert_eq!(parse_volume("nothing"), None);
    }
}

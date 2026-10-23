//! The volume, and the sounds Slipstream makes itself.

use std::process::Command;

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

/// A slider for the volume, 0 to 100% as the volume keys go. It reads the level again whenever the
/// page is shown, so a change made with the keys meanwhile is there. Moving it sets the level once
/// the slider pauses, so a drag doesn't start a program for every step on the way, and clicks at
/// the new level as the volume keys do.
fn volume_slider((level, muted): (u8, bool), store: &Store) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_round_digits(0);
    scale.set_draw_value(false);
    scale.set_width_request(200);
    scale.set_value(level as f64);
    // Set while the slider is being moved to match the speakers, so that isn't sent back to them.
    let reading = Rc::new(Cell::new(false));
    let muted = Rc::new(Cell::new(muted));
    scale.connect_map({
        let (reading, muted) = (Rc::clone(&reading), Rc::clone(&muted));
        move |scale| {
            if let Some((level, now_muted)) = volume() {
                muted.set(now_muted);
                reading.set(true);
                scale.set_value(level as f64);
                reading.set(false);
            }
        }
    });
    let waiting = Rc::new(Cell::new(false));
    let store = store.clone();
    scale.connect_value_changed(move |scale| {
        if reading.get() || waiting.replace(true) {
            return;
        }
        let (scale, waiting) = (scale.downgrade(), Rc::clone(&waiting));
        let click = store.get().sound.volume_blip && !muted.get();
        gtk::glib::timeout_add_local_once(Duration::from_millis(60), move || {
            waiting.set(false);
            if let Some(scale) = scale.upgrade() {
                set_volume(scale.value().round() as u8, click);
            }
        });
    });
    scale
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

/// Sets the volume, on a thread of its own so the slider doesn't wait for `wpctl`, then clicks if
/// `click`, so the click sounds at the level just set.
fn set_volume(percent: u8, click: bool) {
    let level = format!("{:.2}", percent.min(100) as f64 / 100.0);
    std::thread::spawn(move || {
        let set = Command::new("wpctl")
            .args(["set-volume", "-l", "1.0", SINK, &level])
            .status();
        if let Err(err) = set {
            eprintln!("slipstream-settings: couldn't set the volume: {err}");
            return;
        }
        if click
            && slipstream_config::click::due()
            && let Some(path) = slipstream_config::click::file()
        {
            let _ = Command::new("pw-play").arg(path).status();
        }
    });
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

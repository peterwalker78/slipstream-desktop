//! The sounds Slipstream makes itself.

use super::*;

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page("Sound", "The sounds Slipstream makes itself");
    let keys = group(&page, "Volume");

    let blip = gtk::CheckButton::new();
    blip.set_active(store.get().sound.volume_blip);
    row(
        &keys,
        "Click when the volume changes",
        Some(
            "A short click through the speakers at the new level, from the volume keys or quick \
             settings' slider, as macOS and Windows do, so the volume can be heard as well as \
             seen. Never on mute, and never on brightness.",
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

/// The steps offered for the volume keys, in percent.
const STEP_CHOICES: [u64; 3] = [2, 5, 10];

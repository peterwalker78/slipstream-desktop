//! Mouse & keyboard: how the pointer moves and scrolls, the touchpad, key repeat and keyboard
//! layouts.

use slipstream_config::{
    Acceleration, KeyboardLayout, LAYOUT_LISTS, LayoutChoice, Scrolling, layout_catalogue,
    system_layouts_from_env,
};

use super::*;

/// How long a key is held before it repeats, in milliseconds.
const DELAY_CHOICES: [u64; 7] = [150, 200, 250, 300, 400, 500, 700];
/// How many times a second a held key repeats.
const RATE_CHOICES: [u64; 7] = [10, 15, 20, 25, 30, 40, 50];

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Mouse & keyboard",
        "How the pointer moves and scrolls, and how the keys type",
    );
    mouse(&group(&page, "Mouse"), store);
    // A desktop with no touchpad has nothing to set here. Where it can't be told, it's shown.
    if touchpad_present(udev_input_records()) != Some(false) {
        touchpad(&group(&page, "Touchpad"), store);
    }
    typing(&group(&page, "Typing"), store);
    fill_layouts(&group(&page, "Layouts"), store);
    page.upcast()
}

fn mouse(card: &gtk::Box, store: &Store) {
    let mouse = store.get().mouse;
    let speed = speed_scale(mouse.speed(), store, |settings, speed| {
        settings.mouse.speed = speed
    });
    row(card, "Pointer speed", None, &speed);

    let names: Vec<&str> = Acceleration::ALL.iter().map(|it| it.label()).collect();
    let acceleration = gtk::DropDown::from_strings(&names);
    acceleration.set_selected(
        Acceleration::ALL
            .iter()
            .position(|&it| it == mouse.acceleration)
            .unwrap_or(0) as u32,
    );
    let acceleration_store = store.clone();
    acceleration.connect_selected_notify(move |pick| {
        if let Some(&chosen) = Acceleration::ALL.get(pick.selected() as usize) {
            acceleration_store.change(|settings| settings.mouse.acceleration = chosen);
        }
    });
    row(
        card,
        "Acceleration",
        Some(
            "Adaptive carries the pointer further the faster you move. Flat always moves it the \
             same distance for the same movement, as many games prefer.",
        ),
        &acceleration,
    );

    let natural = switch(mouse.natural_scrolling, store, |settings, on| {
        settings.mouse.natural_scrolling = on
    });
    row(
        card,
        "Natural scrolling",
        Some("Turning the wheel down moves the page down, rather than further down the page."),
        &natural,
    );
}

fn touchpad(card: &gtk::Box, store: &Store) {
    let pad = store.get().touchpad;
    let tap = switch(pad.tap_to_click, store, |settings, on| {
        settings.touchpad.tap_to_click = on
    });
    row(
        card,
        "Tap to click",
        Some("Tap with one finger to click, two to right-click, three for the middle button."),
        &tap,
    );
    let natural = switch(pad.natural_scrolling, store, |settings, on| {
        settings.touchpad.natural_scrolling = on
    });
    row(
        card,
        "Natural scrolling",
        Some("The page follows your fingers, as on a phone."),
        &natural,
    );
    let speed = speed_scale(pad.speed(), store, |settings, speed| {
        settings.touchpad.speed = speed
    });
    row(card, "Pointer speed", None, &speed);
    let typing = switch(pad.disable_while_typing, store, |settings, on| {
        settings.touchpad.disable_while_typing = on
    });
    row(
        card,
        "Ignore the touchpad while typing",
        Some("A palm resting on it can't move the pointer while you type."),
        &typing,
    );

    let names: Vec<&str> = Scrolling::ALL.iter().map(|it| it.label()).collect();
    let scrolling = gtk::DropDown::from_strings(&names);
    scrolling.set_selected(
        Scrolling::ALL
            .iter()
            .position(|&it| it == pad.scrolling)
            .unwrap_or(0) as u32,
    );
    let scrolling_store = store.clone();
    scrolling.connect_selected_notify(move |pick| {
        if let Some(&chosen) = Scrolling::ALL.get(pick.selected() as usize) {
            scrolling_store.change(|settings| settings.touchpad.scrolling = chosen);
        }
    });
    row(
        card,
        "Scrolling",
        Some("Slide two fingers anywhere, or one finger along the right or bottom edge."),
        &scrolling,
    );
}

fn typing(card: &gtk::Box, store: &Store) {
    let keyboard = store.get().keyboard;
    let delays = choices(&DELAY_CHOICES, keyboard.repeat_delay() as u64);
    let names: Vec<String> = delays.iter().map(|&ms| seconds(ms)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let delay = gtk::DropDown::from_strings(&names);
    delay.set_selected(
        delays
            .iter()
            .position(|&ms| ms == keyboard.repeat_delay() as u64)
            .unwrap_or(0) as u32,
    );
    let delay_store = store.clone();
    delay.connect_selected_notify(move |pick| {
        if let Some(&ms) = delays.get(pick.selected() as usize) {
            delay_store.change(|settings| settings.keyboard.repeat_delay_ms = ms as u32);
        }
    });
    row(
        card,
        "Repeat after",
        Some("How long a key is held before it starts repeating."),
        &delay,
    );

    let rates = choices(&RATE_CHOICES, keyboard.repeat_rate() as u64);
    let names: Vec<String> = rates
        .iter()
        .map(|rate| format!("{rate} a second"))
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let rate = gtk::DropDown::from_strings(&names);
    rate.set_selected(
        rates
            .iter()
            .position(|&it| it == keyboard.repeat_rate() as u64)
            .unwrap_or(0) as u32,
    );
    let rate_store = store.clone();
    rate.connect_selected_notify(move |pick| {
        if let Some(&it) = rates.get(pick.selected() as usize) {
            rate_store.change(|settings| settings.keyboard.repeat_rate = it as u32);
        }
    });
    row(
        card,
        "Repeat speed",
        Some("How fast a held key repeats."),
        &rate,
    );
}

/// The layouts in use, each with a way to take it out while there are others, the key that
/// switches between them, and a way to add one. Built again after every change.
fn fill_layouts(card: &gtk::Box, store: &Store) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let refill = refiller(card, store, fill_layouts);
    let system = system_layouts_from_env();
    let in_use = store.get().keyboard.layouts_or(&system);
    let catalogue = read_catalogue();
    for (index, layout) in in_use.iter().enumerate() {
        let sub = (index == 0).then_some("Used when you log in");
        let control: gtk::Widget = if in_use.len() > 1 {
            let remove = gtk::Button::from_icon_name("list-remove-symbolic");
            remove.set_tooltip_text(Some("Remove this layout"));
            remove.add_css_class("flat");
            let (store, refill, system) = (store.clone(), refill.clone(), system.clone());
            let mut rest = in_use.clone();
            rest.remove(index);
            remove.connect_clicked(move |_| {
                let (rest, system) = (rest.clone(), system.clone());
                store.change(move |settings| settings.keyboard.set_layouts(rest, &system));
                refill();
            });
            remove.upcast()
        } else {
            gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast()
        };
        row(card, &layout_name(&catalogue, layout), sub, &control);
    }
    if in_use.len() > 1 {
        let keys = label("Alt+Shift", "kv");
        keys.set_wrap(false);
        row(
            card,
            "Switch layout",
            Some(
                "Press Alt and Shift together and let go. The bar shows the layout in use; \
                 clicking it switches too.",
            ),
            &keys,
        );
    }
    // Without xkeyboard-config's list there are no names to choose from.
    if catalogue.is_empty() {
        return;
    }
    let mut names = vec!["Choose a layout"];
    names.extend(catalogue.iter().map(|choice| choice.name.as_str()));
    let list = gtk::StringList::new(&names);
    let pick = gtk::DropDown::new(
        Some(list),
        Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )),
    );
    pick.set_enable_search(true);
    let store = store.clone();
    pick.connect_selected_notify(move |pick| {
        let Some(choice) = (pick.selected() as usize)
            .checked_sub(1)
            .and_then(|index| catalogue.get(index))
        else {
            return;
        };
        if !in_use.contains(&choice.layout) {
            let mut layouts = in_use.clone();
            layouts.push(choice.layout.clone());
            let system = system.clone();
            store.change(move |settings| settings.keyboard.set_layouts(layouts, &system));
        }
        refill();
    });
    row(
        card,
        "Add a layout",
        Some("Type to search the list."),
        &pick,
    );
}

/// A switch in a row, saving `set` with its new state whenever it's flipped.
fn switch(on: bool, store: &Store, set: fn(&mut Settings, bool)) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(on);
    let store = store.clone();
    switch.connect_active_notify(move |switch| {
        let on = switch.is_active();
        store.change(move |settings| set(settings, on));
    });
    switch
}

/// A slider from slowest to fastest, its middle mark the device's usual speed, saving `set`
/// in tenths as it moves.
fn speed_scale(speed: f64, store: &Store, set: fn(&mut Settings, f64)) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.1);
    scale.set_round_digits(1);
    scale.set_draw_value(false);
    scale.set_width_request(200);
    scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    scale.set_value(speed);
    let store = store.clone();
    scale.connect_value_changed(move |scale| {
        let speed = (scale.value() * 10.0).round() / 10.0;
        store.change(move |settings| set(settings, speed));
    });
    scale
}

/// A wait in milliseconds as people say it: `0.2 seconds`, `1 second`.
fn seconds(ms: u64) -> String {
    if ms == 1000 {
        return "1 second".to_string();
    }
    let text = format!("{:.2}", ms as f64 / 1000.0);
    format!(
        "{} seconds",
        text.trim_end_matches('0').trim_end_matches('.')
    )
}

fn read_catalogue() -> Vec<LayoutChoice> {
    LAYOUT_LISTS
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| layout_catalogue(&text))
        .unwrap_or_default()
}

/// A layout's name as the catalogue has it, or its xkb names when it isn't listed.
fn layout_name(catalogue: &[LayoutChoice], layout: &KeyboardLayout) -> String {
    catalogue
        .iter()
        .find(|choice| choice.layout == *layout)
        .map(|choice| choice.name.clone())
        .unwrap_or_else(|| {
            if layout.variant.is_empty() {
                layout.layout.clone()
            } else {
                format!("{} ({})", layout.layout, layout.variant)
            }
        })
}

/// The input devices udev knows of, as the text of their records.
fn udev_input_records() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/run/udev/data") else {
        return Vec::new();
    };
    entries
        .flatten()
        // Character devices with the input subsystem's major number, 13.
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("c13:"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect()
}

/// Whether any of udev's input records is a touchpad, or `None` when there are no records to go
/// by (no udev, or none it lets this program read).
fn touchpad_present(records: Vec<String>) -> Option<bool> {
    if records.is_empty() {
        return None;
    }
    Some(
        records
            .iter()
            .any(|record| record.lines().any(|line| line == "E:ID_INPUT_TOUCHPAD=1")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_waits_read_as_seconds() {
        assert_eq!(seconds(200), "0.2 seconds");
        assert_eq!(seconds(150), "0.15 seconds");
        assert_eq!(seconds(1000), "1 second");
        assert_eq!(seconds(2000), "2 seconds");
    }

    #[test]
    fn the_touchpad_group_hides_only_when_udev_says_there_is_none() {
        assert_eq!(touchpad_present(Vec::new()), None);
        let mouse = "E:ID_INPUT=1\nE:ID_INPUT_MOUSE=1\n".to_string();
        let pad = "E:ID_INPUT=1\nE:ID_INPUT_TOUCHPAD=1\n".to_string();
        assert_eq!(touchpad_present(vec![mouse.clone()]), Some(false));
        assert_eq!(touchpad_present(vec![mouse, pad]), Some(true));
    }

    #[test]
    fn an_unlisted_layout_goes_by_its_xkb_names() {
        let catalogue = [LayoutChoice {
            layout: KeyboardLayout::new("gb", ""),
            name: "English (UK)".into(),
        }];
        assert_eq!(
            layout_name(&catalogue, &KeyboardLayout::new("gb", "")),
            "English (UK)"
        );
        assert_eq!(
            layout_name(&catalogue, &KeyboardLayout::new("xx", "yy")),
            "xx (yy)"
        );
    }
}

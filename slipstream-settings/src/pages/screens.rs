//! Screens: where each sits, and the mode and scale it runs at.

use super::*;

/// Where the screens sit. Each one after the first is placed against the one before it, which is
/// all the arranging two or three screens on a desk ever needs; the compositor moves them the
/// moment this is saved.
pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Screens",
        "Where your screens sit, so the pointer and Super+arrows cross between them the way they \
         are really arranged on your desk.",
    );
    displays(&page, store);
    refresh(&page, store);
    let card = group(&page, "Arrangement");
    fill_screens(&card, store);
    let note = group(&page, "About this list");
    row(
        &note,
        "Screens are remembered by what they are",
        Some(
            "A screen is known by what it reports about itself, so moving it to another port \
             keeps its place, and a different screen on the same port gets its own. Screens that \
             aren't plugged in now aren't shown.",
        ),
        &gtk::Box::new(gtk::Orientation::Horizontal, 0),
    );
    page.upcast()
}

/// One row per lit screen: what it is running, and what it could run. The modes come from
/// `screens.toml`, which the compositor writes — only it can read a connector.
fn displays(page: &gtk::Box, store: &Store) {
    use slipstream_config::screens as known;
    let remembered = known::read(&known::path());
    let lit: Vec<&known::Known> = remembered.lit().collect();
    if lit.is_empty() {
        let card = group(page, "Each screen");
        row(
            &card,
            "No screens yet",
            Some("This list fills in when Slipstream is running."),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    for screen in lit {
        // A group per screen, named after it, so which screen a resolution belongs to is never
        // in doubt on a desk with more than one.
        let card = group(page, &screen.label);
        let place = store.get().display.place(&screen.monitor);

        // Resolutions, biggest first, with the screen's own preferred one marked. "Automatic"
        // leaves it to the screen.
        let mut labels = vec!["Automatic".to_string()];
        let mut modes: Vec<String> = Vec::new();
        for mode in &screen.modes {
            let label = mode.label();
            labels.push(match mode.preferred {
                true => format!("{label}  (this screen's own)"),
                false => label.clone(),
            });
            modes.push(label);
        }
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let chosen = modes
            .iter()
            .position(|mode| !place.mode.is_empty() && *mode == place.mode)
            .map(|index| index + 1)
            .unwrap_or(0);
        let mode_pick = gtk::DropDown::from_strings(&refs);
        mode_pick.set_selected(chosen as u32);
        mode_pick.set_sensitive(!screen.modes.is_empty());
        {
            let store = store.clone();
            let monitor = screen.monitor.clone();
            let modes = modes.clone();
            mode_pick.connect_selected_notify(move |pick| {
                let picked = pick.selected() as usize;
                let mode = match picked {
                    0 => String::new(),
                    index => modes.get(index - 1).cloned().unwrap_or_default(),
                };
                let monitor = monitor.clone();
                store.change(move |settings| settings.display.set_mode(&monitor, &mode));
            });
        }
        row(
            &card,
            "Resolution",
            Some(match screen.running.is_empty() {
                true => "What this screen is set to.",
                false => "What this screen is set to. Automatic uses the one it asks for.",
            }),
            &mode_pick,
        );

        // Scale, in the steps people actually use.
        let steps: Vec<f64> = vec![1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
        let mut scale_labels = vec!["Automatic".to_string()];
        for step in &steps {
            scale_labels.push(format!("{}%", (step * 100.0).round()));
        }
        let scale_refs: Vec<&str> = scale_labels.iter().map(String::as_str).collect();
        let scale_at = steps
            .iter()
            .position(|step| (step - place.scale).abs() < 0.001)
            .map(|index| index + 1)
            .unwrap_or(0);
        let scale_pick = gtk::DropDown::from_strings(&scale_refs);
        scale_pick.set_selected(scale_at as u32);
        {
            let store = store.clone();
            let monitor = screen.monitor.clone();
            let steps = steps.clone();
            scale_pick.connect_selected_notify(move |pick| {
                let picked = pick.selected() as usize;
                let scale = match picked {
                    0 => 0.0,
                    index => steps.get(index - 1).copied().unwrap_or(0.0),
                };
                let monitor = monitor.clone();
                store.change(move |settings| settings.display.set_scale(&monitor, scale));
            });
        }
        let running = match screen.running.is_empty() {
            true => format!("It is {} × {} logical pixels.", screen.width, screen.height),
            false => format!(
                "How big everything is drawn. It is running {} at {:.0}%.",
                screen.running,
                if screen.scale > 0.0 {
                    screen.scale
                } else {
                    1.0
                } * 100.0
            ),
        };
        row(&card, "Scale", Some(&running), &scale_pick);
    }
}

/// Variable refresh, for every screen that supports it.
fn refresh(page: &gtk::Box, store: &Store) {
    use slipstream_config::VariableRefresh;
    let card = group(page, "Refresh rate");
    let names: Vec<&str> = VariableRefresh::ALL
        .iter()
        .map(|which| which.label())
        .collect();
    let pick = gtk::DropDown::from_strings(&names);
    let current = store.get().display.variable_refresh;
    pick.set_selected(
        VariableRefresh::ALL
            .iter()
            .position(|which| *which == current)
            .unwrap_or(0) as u32,
    );
    {
        let store = store.clone();
        pick.connect_selected_notify(move |pick| {
            let which = VariableRefresh::ALL
                .get(pick.selected() as usize)
                .copied()
                .unwrap_or_default();
            store.change(move |settings| settings.display.variable_refresh = which);
        });
    }
    row(
        &card,
        "Variable refresh rate",
        Some(
            "On a screen that supports it, each frame is shown as soon as it's ready, so a game \
             that can't keep up still moves smoothly.",
        ),
        &pick,
    );
}

/// The rows, rebuilt whenever the shape changes. Reads the screens the compositor says are lit
/// (`screens.toml`), so this is the same list, named the same way, that the compositor arranges.
fn fill_screens(card: &gtk::Box, store: &Store) {
    use slipstream_config::{Align, Position, screens as known};
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    let remembered = known::read(&known::path());
    let lit: Vec<&known::Known> = remembered.lit().collect();
    if lit.len() < 2 {
        let note = match lit.len() {
            0 => "No screens to arrange. This list fills in when Slipstream is running.",
            _ => "Only one screen. Plug another in and it can be placed against this one.",
        };
        row(
            card,
            "Nothing to arrange",
            Some(note),
            &gtk::Box::new(gtk::Orientation::Horizontal, 0),
        );
        return;
    }
    let refill = refiller(card, store, fill_screens);
    for (index, screen) in lit.iter().enumerate() {
        let size = format!("{}×{}", screen.width, screen.height);
        // The first screen is the one everything else is placed against, so it has nothing to set.
        let Some(before) = lit.get(index.wrapping_sub(1)) else {
            row(
                card,
                screen.name(),
                Some(&format!(
                    "{size} · {} · everything else is placed against this one",
                    screen.connector
                )),
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
            );
            continue;
        };
        let place = store.get().display.place(&screen.monitor);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let names: Vec<String> = Position::ALL
            .iter()
            .map(|position| position.label(before.name()))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let position = gtk::DropDown::from_strings(&refs);
        position.set_selected(
            Position::ALL
                .iter()
                .position(|which| *which == place.position)
                .unwrap_or(0) as u32,
        );

        let align_names: Vec<&str> = Align::ALL
            .iter()
            .map(|align| align.label(place.position.beside()))
            .collect();
        let align = gtk::DropDown::from_strings(&align_names);
        align.set_selected(
            Align::ALL
                .iter()
                .position(|which| *which == place.align)
                .unwrap_or(0) as u32,
        );
        controls.append(&position);
        controls.append(&align);
        row(
            card,
            screen.name(),
            Some(&format!("{size} · {}", screen.connector)),
            &controls,
        );

        let monitor = screen.monitor.clone();
        {
            // Each list reads the other, so they hold each other weakly.
            let (store, align, refill) = (store.clone(), align.downgrade(), refill.clone());
            let monitor = monitor.clone();
            position.connect_selected_notify(move |position| {
                let Some(which) = Position::ALL.get(position.selected() as usize).copied() else {
                    return;
                };
                let Some(align) = align.upgrade() else {
                    return;
                };
                let chosen = Align::ALL
                    .get(align.selected() as usize)
                    .copied()
                    .unwrap_or_default();
                let monitor = monitor.clone();
                store.change(move |settings| settings.display.set_place(&monitor, which, chosen));
                // Beside and stacked name different edges, so the other list has to be rebuilt.
                refill();
            });
        }
        {
            let (store, position) = (store.clone(), position.downgrade());
            align.connect_selected_notify(move |align| {
                let Some(chosen) = Align::ALL.get(align.selected() as usize).copied() else {
                    return;
                };
                let Some(position) = position.upgrade() else {
                    return;
                };
                let which = Position::ALL
                    .get(position.selected() as usize)
                    .copied()
                    .unwrap_or_default();
                let monitor = monitor.clone();
                store.change(move |settings| settings.display.set_place(&monitor, which, chosen));
            });
        }
    }
}

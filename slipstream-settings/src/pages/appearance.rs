//! Appearance: Slipstream's three colours, the bar's style and motion.

use super::*;
use crate::{harmony, picker::ColourButton};

pub(super) fn build(store: &Store) -> gtk::Widget {
    let page = page(
        "Appearance",
        "The colours Slipstream marks things with, the bar, and how much it moves",
    );
    let colours = group(&page, "Colours");
    self::colours(&colours, store);
    bar_style(&page, store);
    let motion = group(&page, "Motion");
    // The same name quick settings gives this switch.
    let reduced = gtk::Switch::new();
    reduced.set_active(store.get().motion.reduced);
    let reduced_store = store.clone();
    reduced.connect_active_notify(move |reduced| {
        let on = reduced.is_active();
        reduced_store.change(|settings| settings.motion.reduced = on);
    });
    row(
        &motion,
        "Reduced motion",
        Some("Windows jump into place and effects become short fades"),
        &reduced,
    );
    let styles = [
        (slipstream_config::Effects::Slipstream, "Slipstream"),
        (slipstream_config::Effects::Matrix, "Matrix rain"),
    ];
    let names: Vec<&str> = styles.iter().map(|(_, name)| *name).collect();
    let effects = gtk::DropDown::from_strings(&names);
    let current = store.get().motion.effects;
    effects.set_selected(
        styles
            .iter()
            .position(|(which, _)| *which == current)
            .unwrap_or(0) as u32,
    );
    let effects_store = store.clone();
    effects.connect_selected_notify(move |effects| {
        if let Some((which, _)) = styles.get(effects.selected() as usize) {
            let which = *which;
            effects_store.change(|settings| settings.motion.effects = which);
        }
    });
    row(
        &motion,
        "Effects",
        Some("What minimised apps turn into: falling code, or streaks of light"),
        &effects,
    );
    let rain = gtk::Switch::new();
    rain.set_active(store.get().motion.close_into_rain);
    let rain_store = store.clone();
    rain.connect_active_notify(move |rain| {
        let on = rain.is_active();
        rain_store.change(|settings| settings.motion.close_into_rain = on);
    });
    row(
        &motion,
        "Close into the rain",
        Some("A closed window falls away in the same effects instead of fading"),
        &rain,
    );

    page.upcast()
}

/// The bar's style: the focus bar, a lit edge, or a floating strip.
fn bar_style(page: &gtk::Box, store: &Store) {
    let bar = group(page, "Bar");
    let styles = slipstream_config::BarStyle::ALL;
    let names: Vec<&str> = styles.iter().map(|style| style.label()).collect();
    let choice = gtk::DropDown::from_strings(&names);
    let chosen = store.get().appearance.bar_style;
    choice.set_selected(styles.iter().position(|held| *held == chosen).unwrap_or(0) as u32);
    let style_store = store.clone();
    choice.connect_selected_notify(move |choice| {
        if let Some(chosen) = styles.get(choice.selected() as usize).copied() {
            style_store.change(move |settings| settings.appearance.bar_style = chosen);
        }
    });
    row(
        &bar,
        "Style",
        Some(
            "The focus bar quietens while you type, the lit edge carries a charge with calm \
             pulses and gusts that leave a curling wake, and the floating strip sits clear of \
             the screen's edges.",
        ),
        &choice,
    );
}

/// What the theme list says when the colours are none of its sets.
const YOUR_OWN: &str = "Your own";

/// The focus ring, bullet time's colour and the accent, each channel 0–1.
type Three = [[f32; 3]; 3];

/// The colour rows, kept in step with each other: whatever changes one shows the result on all.
struct Colours {
    store: Store,
    theme: gtk::DropDown,
    names: gtk::StringList,
    shown: Cell<Three>,
    wheel: gtk::Scale,
    focus: ColourButton,
    bullet: ColourButton,
    accent: ColourButton,
    /// Set while the rows are being changed to match, so their own handlers stay quiet.
    syncing: Cell<bool>,
    /// The question about matching the other two, waiting for the rainbow to stop moving.
    ask_after: RefCell<Option<gtk::glib::SourceId>>,
    /// Set while that question is open, so it's only asked once at a time.
    asking: Cell<bool>,
}

impl Colours {
    fn current_of(settings: &Settings) -> Three {
        let borders = &settings.borders;
        [
            borders.selected_tile_rgb(),
            borders.bullet_time_rgb(),
            borders.accent_rgb(),
        ]
    }

    /// Saves `three` and shows it on every row.
    fn choose(&self, three: Three) {
        let [focus, bullet, accent] = three.map(harmony::hex);
        self.store.change(|settings| {
            settings.borders.selected_tile = focus;
            settings.borders.bullet_time = bullet;
            settings.borders.accent = accent;
        });
        crate::restyle(three);
        self.show(three);
    }

    /// A focus ring of `focus`, leaving the other two as they are.
    fn choose_focus(&self, focus: [f32; 3]) {
        let mut three = self.shown.get();
        three[0] = focus;
        self.choose(three);
    }

    fn show(&self, three: Three) {
        self.syncing.set(true);
        self.shown.set(three);
        let sets = harmony::THEMES.len() as u32;
        match matching_theme(three) {
            Some(index) => {
                self.theme.set_selected(index as u32);
                if self.names.n_items() > sets {
                    self.names.remove(sets);
                }
            }
            None => {
                if self.names.n_items() == sets {
                    self.names.append(YOUR_OWN);
                }
                self.theme.set_selected(sets);
            }
        }
        // The rainbow moves to the focus ring's hue, unless it's already within a step of it.
        let hue = harmony::hue(three[0]);
        if (self.wheel.value() - hue).abs() >= 1.0 {
            self.wheel.set_value(hue.round());
        }
        for (button, rgb) in [&self.focus, &self.bullet, &self.accent].iter().zip(three) {
            button.set(rgb);
        }
        self.syncing.set(false);
    }
}

/// How long the rainbow has to rest before the question about the other two is asked, so a drag
/// asks once, at the end.
const RAINBOW_RESTS: Duration = Duration::from_millis(700);

/// Asks, `after` this long, whether the other two colours should change to go with the primary
/// colour just chosen. Asking again before then waits from the start.
fn offer_partners(colours: &Rc<Colours>, after: Duration) {
    if let Some(waiting) = colours.ask_after.take() {
        waiting.remove();
    }
    let weak = Rc::downgrade(colours);
    let waiting = gtk::glib::timeout_add_local_once(after, move || {
        let Some(colours) = weak.upgrade() else {
            return;
        };
        // It has run, so there's nothing left to take back.
        colours.ask_after.take();
        if colours.asking.replace(true) {
            return;
        }
        let dialog = gtk::AlertDialog::builder()
            .message("Match the other two colours?")
            .detail(
                "The secondary and tertiary colours can change to ones that go with your new \
                 primary colour.",
            )
            .buttons(["Keep them", "Match them"])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        let window = colours.wheel.root().and_downcast::<gtk::Window>();
        gtk::glib::spawn_future_local(async move {
            let answer = dialog.choose_future(window.as_ref()).await;
            colours.asking.set(false);
            if answer == Ok(1) {
                // The primary colour as it is now, in case it moved while the question was open.
                let focus = colours.shown.get()[0];
                let (bullet, accent) = harmony::partners(focus);
                colours.choose([focus, bullet, accent]);
            }
        });
    });
    colours.ask_after.replace(Some(waiting));
}

/// Which of the sets `three` is, if it's one of them.
fn matching_theme(three: Three) -> Option<usize> {
    harmony::THEMES.iter().position(|theme| {
        [theme.focus, theme.bullet, theme.accent]
            .map(slipstream_config::rgb)
            .iter()
            .zip(three)
            .all(|(held, rgb)| *held == Some(rgb))
    })
}

/// The theme list's entries: a set's three colours as dots, then its name.
fn theme_entries() -> gtk::SignalListItemFactory {
    const DOT: i32 = 14;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let dots = gtk::DrawingArea::new();
        dots.set_content_width(3 * DOT + 2 * 4);
        dots.set_content_height(DOT);
        dots.set_valign(gtk::Align::Center);
        let entry = gtk::Box::builder().spacing(10).build();
        entry.append(&dots);
        entry.append(&gtk::Label::new(None));
        item.set_child(Some(&entry));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let name = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .map(|name| name.string().to_string())
            .unwrap_or_default();
        let Some(entry) = item.child() else {
            return;
        };
        let dots = entry.first_child().and_downcast::<gtk::DrawingArea>();
        let label = entry.last_child().and_downcast::<gtk::Label>();
        let (Some(dots), Some(label)) = (dots, label) else {
            return;
        };
        label.set_label(&name);
        let set = harmony::THEMES.iter().find(|set| set.name == name);
        dots.set_visible(set.is_some());
        let three = set.map(|set| [set.focus, set.bullet, set.accent].map(slipstream_config::rgb));
        dots.set_draw_func(move |_, cairo, _, height| {
            let radius = height as f64 / 2.0 - 1.0;
            for (n, rgb) in three.iter().flatten().enumerate() {
                let Some([r, g, b]) = rgb else { continue };
                let x = height as f64 / 2.0 + n as f64 * (height as f64 + 4.0);
                cairo.arc(x, height as f64 / 2.0, radius, 0.0, std::f64::consts::TAU);
                cairo.set_source_rgb(*r as f64, *g as f64, *b as f64);
                let _ = cairo.fill();
            }
        });
    });
    factory
}

/// The theme, the focus ring on a rainbow with the other two following it, and each colour on
/// its own for a choice of one's own.
fn colours(card: &gtk::Box, store: &Store) {
    let names: Vec<&str> = harmony::THEMES.iter().map(|theme| theme.name).collect();
    let names = gtk::StringList::new(&names);
    let theme = gtk::DropDown::new(Some(names.clone()), gtk::Expression::NONE);
    theme.set_factory(Some(&theme_entries()));
    let wheel = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 360.0, 1.0);
    wheel.set_round_digits(0);
    wheel.set_draw_value(false);
    wheel.set_width_request(200);
    wheel.add_css_class("hue");
    // The handlers hold it; GTK lets them go with the widgets when the window closes.
    let colours = Rc::new(Colours {
        store: store.clone(),
        theme,
        names,
        shown: Cell::new(Colours::current_of(&store.get())),
        wheel,
        focus: ColourButton::new("Primary colour"),
        bullet: ColourButton::new("Secondary colour"),
        accent: ColourButton::new("Tertiary colour"),
        syncing: Cell::new(false),
        ask_after: RefCell::new(None),
        asking: Cell::new(false),
    });

    let chosen = Rc::clone(&colours);
    colours.theme.connect_selected_notify(move |theme| {
        if chosen.syncing.get() {
            return;
        }
        if let Some(set) = harmony::THEMES.get(theme.selected() as usize) {
            let three = [set.focus, set.bullet, set.accent]
                .map(|hex| slipstream_config::rgb(hex).expect("a valid theme"));
            chosen.choose(three);
        }
    });
    let turned = Rc::clone(&colours);
    colours.wheel.connect_value_changed(move |wheel| {
        if !turned.syncing.get() {
            turned.choose_focus(harmony::on_wheel(wheel.value()));
            offer_partners(&turned, RAINBOW_RESTS);
        }
    });
    // The handlers hold the colours weakly: the buttons belong to them, so a strong hold would
    // keep them alive for good.
    let picked = Rc::downgrade(&colours);
    colours.focus.connect_chosen(move |rgb| {
        if let Some(picked) = picked.upgrade() {
            picked.choose_focus(rgb);
            offer_partners(&picked, Duration::ZERO);
        }
    });
    for (which, button) in [(1, &colours.bullet), (2, &colours.accent)] {
        let picked = Rc::downgrade(&colours);
        button.connect_chosen(move |rgb| {
            if let Some(picked) = picked.upgrade() {
                let mut three = picked.shown.get();
                three[which] = rgb;
                picked.choose(three);
            }
        });
    }
    colours.show(colours.shown.get());

    row(
        card,
        "Theme",
        Some("Sets of three colours that go together. Change any of them below to make your own."),
        &colours.theme,
    );
    let focus_control = gtk::Box::builder().spacing(10).build();
    focus_control.append(&colours.wheel);
    focus_control.append(&colours.focus.button);
    row(
        card,
        "Primary colour",
        Some(
            "Where the keyboard is: the ring around the focused window and in the panels, and the \
             workspace you're on. Also a battery that's charging and plugins that are on. Pick \
             any colour, and you're asked whether the two below should change to go with it.",
        ),
        &focus_control,
    );
    row(
        card,
        "Secondary colour",
        Some(
            "Bullet time, and whatever wants your attention: the notification count, Caps Lock, \
             a low battery, and apps and plugins asking to be looked at.",
        ),
        &colours.bullet.button,
    );
    row(
        card,
        "Tertiary colour",
        Some(
            "What's on or chosen: switches, tiles and ticks, sliders, today in the calendar, the \
             text cursor, toasts and the button Enter presses.",
        ),
        &colours.accent.button,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_colours_are_the_first_theme() {
        assert_eq!(
            matching_theme(Colours::current_of(&Settings::default())),
            Some(0)
        );
    }

    #[test]
    fn colours_that_match_no_set_are_your_own() {
        let mut three = Colours::current_of(&Settings::default());
        three[2] = slipstream_config::rgb("#123456").unwrap();
        assert_eq!(matching_theme(three), None);
    }
}

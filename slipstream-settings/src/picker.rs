//! A colour of one's own: a button showing the colour, and the small window it opens to change it.
//! The window starts on the colour the button shows, with hue, saturation and lightness sliders
//! and the colour's hex code, and nothing is changed until Select.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;

use crate::harmony;

/// How wide the window is. Everything in it fills that width.
const WIDTH: i32 = 360;

/// What's called with a colour once it's picked, set after the button is made.
type Chosen = Rc<RefCell<Option<Box<dyn Fn([f32; 3])>>>>;

/// A button that shows a colour and opens the picker for it.
pub struct ColourButton {
    pub button: gtk::Button,
    colour: Rc<Cell<[f32; 3]>>,
    swatch: gtk::DrawingArea,
    chosen: Chosen,
}

impl ColourButton {
    /// A button whose picker is titled `title`.
    pub fn new(title: &'static str) -> ColourButton {
        let colour = Rc::new(Cell::new([1.0, 1.0, 1.0]));
        let swatch = swatch(&colour, 32, 18);
        let button = gtk::Button::builder()
            .child(&swatch)
            .tooltip_text(title)
            .valign(gtk::Align::Center)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(title)]);
        let chosen: Chosen = Rc::default();
        button.connect_clicked({
            let (colour, chosen) = (Rc::clone(&colour), Rc::clone(&chosen));
            move |button| {
                let chosen = Rc::clone(&chosen);
                open(button, title, colour.get(), move |rgb| {
                    if let Some(chosen) = chosen.borrow().as_ref() {
                        chosen(rgb);
                    }
                });
            }
        });
        ColourButton {
            button,
            colour,
            swatch,
            chosen,
        }
    }

    /// Shows `rgb`, without calling back.
    pub fn set(&self, rgb: [f32; 3]) {
        self.colour.set(rgb);
        self.swatch.queue_draw();
    }

    /// Calls `chosen` with the colour picked, when one is.
    pub fn connect_chosen(&self, chosen: impl Fn([f32; 3]) + 'static) {
        *self.chosen.borrow_mut() = Some(Box::new(chosen));
    }
}

/// A rounded patch of `colour`, `w` by `h`, wider if there's room.
fn swatch(colour: &Rc<Cell<[f32; 3]>>, w: i32, h: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(w)
        .content_height(h)
        .build();
    let colour = Rc::clone(colour);
    area.set_draw_func(move |_, cr, w, h| {
        let [r, g, b] = colour.get();
        let (w, h, radius) = (w as f64, h as f64, 5.0_f64.min(h as f64 / 2.0));
        cr.new_sub_path();
        for (x, y, from) in [
            (w - radius, radius, -90.0_f64),
            (w - radius, h - radius, 0.0),
            (radius, h - radius, 90.0),
            (radius, radius, 180.0),
        ] {
            cr.arc(x, y, radius, from.to_radians(), (from + 90.0).to_radians());
        }
        cr.close_path();
        cr.set_source_rgb(r as f64, g as f64, b as f64);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    area
}

/// Opens the picker over `button`'s window on `start`, calling `chosen` on Select.
fn open(button: &gtk::Button, title: &str, start: [f32; 3], chosen: impl Fn([f32; 3]) + 'static) {
    let working = Rc::new(Cell::new(start));
    let preview = swatch(&working, WIDTH, 64);
    preview.set_hexpand(true);

    let [hue, saturation, lightness] = to_hsl(start);
    let hue = slider(0.0, 360.0, hue, "spectrum");
    let saturation = slider(0.0, 100.0, saturation * 100.0, "saturation");
    let lightness = slider(0.0, 100.0, lightness * 100.0, "lightness");
    let hex = gtk::Entry::builder()
        .text(harmony::hex(start))
        .max_length(7)
        .hexpand(true)
        .build();

    let grid = gtk::Grid::builder()
        .row_spacing(12)
        .column_spacing(14)
        .css_classes(["row", "picker"])
        .build();
    grid.attach(&preview, 0, 0, 2, 1);
    for (at, (name, control)) in [
        ("Hue", hue.upcast_ref::<gtk::Widget>()),
        ("Saturation", saturation.upcast_ref()),
        ("Lightness", lightness.upcast_ref()),
        ("Hex", hex.upcast_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        let label = gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .css_classes(["row-title"])
            .build();
        control.update_relation(&[gtk::accessible::Relation::LabelledBy(&[label
            .upcast_ref::<gtk::Accessible>(
        )])]);
        grid.attach(&label, 0, at as i32 + 1, 1, 1);
        grid.attach(control, 1, at as i32 + 1, 1, 1);
    }

    let cancel = gtk::Button::with_label("Cancel");
    let select = gtk::Button::builder()
        .label("Select")
        .css_classes(["suggested-action"])
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(6)
        .build();
    buttons.append(&cancel);
    buttons.append(&select);
    grid.attach(&buttons, 0, 5, 2, 1);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .width_request(WIDTH)
        .build();
    content.append(&grid);

    let header = gtk::HeaderBar::new();
    header.set_decoration_layout(Some(":close"));
    header.set_title_widget(Some(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["title"])
            .build(),
    ));
    let window = gtk::Window::builder()
        .title(title)
        .modal(true)
        .resizable(false)
        .child(&content)
        .default_widget(&select)
        .build();
    window.add_css_class("slipstream-settings");
    window.set_titlebar(Some(&header));
    window.set_transient_for(button.root().and_downcast_ref::<gtk::Window>());

    // The saturation and lightness sliders show what they'd do at the hue they're on.
    let gradients = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &gradients,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
        );
    }
    let restyle = {
        let gradients = gradients.clone();
        move |[h, s, l]: [f64; 3]| gradients.load_from_string(&gradients_css(h, s, l))
    };
    restyle(to_hsl(start));

    // Set while the controls are being changed to match each other, so their handlers stay quiet.
    let syncing = Rc::new(Cell::new(false));
    let from_sliders = {
        let (hue, saturation, lightness) = (hue.clone(), saturation.clone(), lightness.clone());
        let (hex, working, preview) = (hex.clone(), Rc::clone(&working), preview.clone());
        let (syncing, restyle) = (Rc::clone(&syncing), restyle.clone());
        move || {
            if syncing.get() {
                return;
            }
            let hsl = [
                hue.value(),
                saturation.value() / 100.0,
                lightness.value() / 100.0,
            ];
            let rgb = from_hsl(hsl);
            working.set(rgb);
            preview.queue_draw();
            restyle(hsl);
            syncing.set(true);
            hex.set_text(&harmony::hex(rgb));
            syncing.set(false);
        }
    };
    for slider in [&hue, &saturation, &lightness] {
        let from_sliders = from_sliders.clone();
        slider.connect_value_changed(move |_| from_sliders());
    }
    hex.connect_changed({
        let (hue, saturation, lightness) = (hue.clone(), saturation.clone(), lightness.clone());
        let (working, preview) = (Rc::clone(&working), preview.clone());
        move |hex| {
            if syncing.get() {
                return;
            }
            let Some(rgb) = slipstream_config::rgb(hex.text().trim()) else {
                return;
            };
            working.set(rgb);
            preview.queue_draw();
            let hsl = to_hsl(rgb);
            restyle(hsl);
            syncing.set(true);
            hue.set_value(hsl[0]);
            saturation.set_value(hsl[1] * 100.0);
            lightness.set_value(hsl[2] * 100.0);
            syncing.set(false);
        }
    });

    cancel.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    select.connect_clicked({
        let window = window.downgrade();
        move |_| {
            chosen(working.get());
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    // Enter in the hex box selects, as it does anywhere else in the window.
    hex.set_activates_default(true);
    let escape = gtk::ShortcutController::new();
    escape.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(escape);
    window.connect_close_request(move |_| {
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_remove_provider_for_display(&display, &gradients);
        }
        glib::Propagation::Proceed
    });
    window.present();
    hue.grab_focus();
}

fn slider(from: f64, to: f64, value: f64, class: &str) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, from, to, 1.0);
    scale.set_round_digits(0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_value(value);
    scale.add_css_class(class);
    scale
}

/// The sliders' troughs: every hue, and the saturation and lightness the current hue can have.
fn gradients_css(hue: f64, saturation: f64, lightness: f64) -> String {
    let stops = |from: f64, to: f64, at: &dyn Fn(f64) -> [f64; 3]| {
        let stops: Vec<String> = (0..=6)
            .map(|step| harmony::hex(from_hsl(at(from + (to - from) * step as f64 / 6.0))))
            .collect();
        stops.join(", ")
    };
    let trough = |class: &str, stops: String| {
        format!(
            ".picker scale.{class} trough {{ min-height: 10px; border-radius: 5px; \
             background-image: linear-gradient(to right, {stops}); }} \
             .picker scale.{class} highlight {{ background: none; }} "
        )
    };
    let spectrum: Vec<String> = (0..=12)
        .map(|step| harmony::hex(from_hsl([step as f64 * 30.0, 1.0, 0.5])))
        .collect();
    trough("spectrum", spectrum.join(", "))
        + &trough(
            "saturation",
            stops(0.0, 1.0, &|s| [hue, s, lightness.clamp(0.2, 0.8)]),
        )
        + &trough("lightness", stops(0.0, 1.0, &|l| [hue, saturation, l]))
}

/// `rgb` as hue in degrees, saturation and lightness from 0 to 1.
fn to_hsl([r, g, b]: [f32; 3]) -> [f64; 3] {
    let [r, g, b] = [r, g, b].map(|c| c as f64);
    let (high, low) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (high + low) / 2.0;
    let span = high - low;
    if span < 1e-9 {
        return [0.0, 0.0, lightness];
    }
    let saturation = span / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if high == r {
        ((g - b) / span).rem_euclid(6.0)
    } else if high == g {
        (b - r) / span + 2.0
    } else {
        (r - g) / span + 4.0
    } * 60.0;
    [hue, saturation.min(1.0), lightness]
}

fn from_hsl([hue, saturation, lightness]: [f64; 3]) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r, g, b].map(|c| (c + m).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_goes_there_and_back() {
        for hex in [
            "#4fd6ff", "#ffb547", "#ff77b0", "#000000", "#ffffff", "#808080",
        ] {
            let rgb = slipstream_config::rgb(hex).unwrap();
            assert_eq!(harmony::hex(from_hsl(to_hsl(rgb))), hex);
        }
        assert_eq!(harmony::hex(from_hsl([0.0, 1.0, 0.5])), "#ff0000");
        assert_eq!(harmony::hex(from_hsl([240.0, 1.0, 0.5])), "#0000ff");
    }
}

//! Slipstream's Settings app (Super+I), laid out as the mockup's Settings window: pages in a
//! sidebar, and groups of rows on each page. A change is saved to the settings file straight
//! away, and Slipstream applies it as soon as the file changes. Only settings Slipstream actually
//! follows are shown.
//!
//! `--screenshot PATH` draws the window into a PNG and quits (`--page ID` picks the page), to
//! check how it looks without opening it on screen; GTK's Broadway backend works for that.

mod fonts;
mod pages;
mod previews;

use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use gtk::{gdk, gio, glib, gsk, prelude::*};
use gtk4 as gtk;
use slipstream_config::Settings;

const APP_ID: &str = "io.github.peterwalker78.SlipstreamSettings";

fn main() -> glib::ExitCode {
    let mut screenshot = None;
    let mut page = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--screenshot", Some(path)) => screenshot = Some(PathBuf::from(path)),
            ("--page", Some(id)) => page = Some(id),
            _ => {
                eprintln!(
                    "usage: slipstream-settings [--screenshot PATH] [--page {}]",
                    pages::ids().join("|")
                );
                return glib::ExitCode::FAILURE;
            }
        }
    }

    let app = gtk::Application::builder().application_id(APP_ID).build();
    if screenshot.is_some() {
        // A screenshot never hands over to a Settings window that's already open.
        app.set_flags(gio::ApplicationFlags::NON_UNIQUE);
    }
    app.connect_startup(|app| {
        fonts::register();
        style();
        app.set_accels_for_action("window.close", &["<Control>w", "<Control>q"]);
    });
    app.connect_activate(move |app| activate(app, page.as_deref(), screenshot.clone()));
    app.run_with_args::<&str>(&[])
}

/// The settings as shown, saved to the file after every change.
#[derive(Clone)]
pub struct Store {
    settings: Rc<RefCell<Settings>>,
    path: Rc<PathBuf>,
    /// Shown under the page when the file can't be read or saved.
    note: Note,
}

impl Store {
    pub fn get(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Changes the settings and saves them. Slipstream picks the change up from the file.
    ///
    /// The file is read again first, so a change made elsewhere meanwhile (in quick settings, say)
    /// isn't written back over. A file that can't be read isn't written at all, as quick settings
    /// refuses too: saving would replace everything in it with the defaults. The note says so,
    /// and goes as soon as a change finds the file readable again.
    pub fn change(&self, change: impl FnOnce(&mut Settings)) {
        let on_disk = match slipstream_config::read(&self.path) {
            Ok(on_disk) => on_disk,
            Err(err) => {
                self.note.unreadable(&self.path, &err);
                return;
            }
        };
        let mut settings = self.settings.borrow_mut();
        *settings = on_disk;
        let before = settings.clone();
        change(&mut settings);
        if *settings == before {
            self.note.hide();
            return;
        }
        match slipstream_config::write(&self.path, &settings) {
            Ok(()) => self.note.hide(),
            Err(err) => self.note.say(
                &format!("Couldn't save {}: {err}", self.path.display()),
                "error",
            ),
        }
    }
}

/// The note under the page: what's wrong with the settings file, and for a file that can't be
/// read, the way to start again without losing it.
#[derive(Clone)]
struct Note {
    bar: gtk::Box,
    text: gtk::Label,
    move_aside: gtk::Button,
}

impl Note {
    fn new() -> Self {
        let text = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let move_aside = gtk::Button::builder()
            .label("Move it aside")
            .valign(gtk::Align::Center)
            .tooltip_text("Keeps the file beside the new one, and starts again from the defaults")
            .visible(false)
            .build();
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .css_classes(["note"])
            .visible(false)
            .build();
        bar.append(&text);
        bar.append(&move_aside);
        Self {
            bar,
            text,
            move_aside,
        }
    }

    /// `message`, in the style `class` names (`error`, or `told` for news that isn't a problem).
    fn say(&self, message: &str, class: &str) {
        self.text.set_label(message);
        self.text.set_css_classes(&[class]);
        self.move_aside.set_visible(false);
        self.bar.set_visible(true);
    }

    fn unreadable(&self, path: &Path, err: &io::Error) {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        self.say(
            &format!(
                "{name} can't be read ({}). Slipstream is still using the settings it last \
                 read. Fix the file, or move it aside to start again.",
                brief(err)
            ),
            "error",
        );
        self.move_aside.set_visible(true);
    }

    fn hide(&self) {
        self.bar.set_visible(false);
    }
}

/// An error in a line: TOML's parse errors quote the offending line with markers under it, which
/// a note has no room for, so only the lines that say something are kept.
fn brief(err: &io::Error) -> String {
    err.to_string()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains('|'))
        .collect::<Vec<_>>()
        .join(": ")
}

/// The keyboard focus outlines in `rgb`, each channel 0–1.
fn focus_css(rgb: [f32; 3]) -> String {
    let [r, g, b] = rgb.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!(
        ".side > row:focus-visible, .row switch:focus-visible, .row checkbutton:focus-visible, \
         .row button:focus-visible, dropdown > button:focus-visible, \
         .note > button:focus-visible {{ outline-color: #{r:02x}{g:02x}{b:02x}; }}"
    )
}

/// The mockup's colours, spacing and fonts, over GTK's dark theme.
fn style() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    // The keyboard's outline is the focus ring's colour, as it is on the desktop.
    let ring = slipstream_config::read(&slipstream_config::path())
        .unwrap_or_default()
        .borders
        .selected_tile_rgb();
    let focus = gtk::CssProvider::new();
    focus.load_from_string(&focus_css(ring));
    gtk::style_context_add_provider_for_display(
        &display,
        &focus,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
}

fn activate(app: &gtk::Application, page: Option<&str>, screenshot: Option<PathBuf>) {
    // Super+I again brings back the window that's already open.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    if screenshot.is_some() {
        previews::hold();
    }
    let window = open_window(app, page, None);
    if let Some(path) = screenshot {
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(800), move || {
            match save_png(&window, &path) {
                Ok(()) => println!("saved {}", path.display()),
                Err(err) => eprintln!("slipstream-settings: screenshot failed: {err}"),
            }
            app.quit();
        });
    }
}

/// Builds and shows the window on `page`, with `told` in the note if there's news to give.
fn open_window(
    app: &gtk::Application,
    page: Option<&str>,
    told: Option<String>,
) -> gtk::ApplicationWindow {
    let path = slipstream_config::path();
    let note = Note::new();
    if let Some(told) = told {
        note.say(&told, "told");
    }
    let settings = slipstream_config::read(&path).unwrap_or_else(|err| {
        note.unreadable(&path, &err);
        Settings::default()
    });
    // The app keeps its own motion quiet too.
    if settings.motion.reduced
        && let Some(gtk_settings) = gtk::Settings::default()
    {
        gtk_settings.set_gtk_enable_animations(false);
    }
    let store = Store {
        settings: Rc::new(RefCell::new(settings)),
        path: Rc::new(path),
        note: note.clone(),
    };

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(120)
        .hexpand(true)
        .vexpand(true)
        .build();
    let side = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Browse)
        .width_request(230)
        .css_classes(["side"])
        .build();
    // Each page scrolls on its own, so a long page left scrolled down doesn't carry its offset to a
    // short one and show it blank.
    for page in &pages::PAGES {
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&(page.build)(&store))
            .build();
        stack.add_named(&scroller, Some(page.id));
        side.append(&sidebar_item(page));
    }
    side.connect_row_selected({
        let stack = stack.clone();
        move |_, row| {
            if let Some(page) = row.and_then(|row| pages::PAGES.get(row.index() as usize)) {
                stack.set_visible_child_name(page.id);
            }
        }
    });
    let first = page
        .and_then(|id| pages::PAGES.iter().position(|page| page.id == id))
        .unwrap_or(0);
    side.select_row(side.row_at_index(first as i32).as_ref());

    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.append(&stack);
    main.append(&note.bar);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&side);
    body.append(&main);

    // A tiling desktop has no use for minimise and maximise buttons; the mockup shows only ✕.
    let header = gtk::HeaderBar::new();
    header.set_decoration_layout(Some(":close"));
    header.set_title_widget(Some(
        &gtk::Label::builder()
            .label("Settings")
            .css_classes(["title"])
            .build(),
    ));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Settings")
        .default_width(900)
        .default_height(620)
        .child(&body)
        .build();
    window.add_css_class("slipstream-settings");
    window.set_titlebar(Some(&header));
    window.present();

    // The unreadable file goes beside itself and the defaults take its place. The window is built
    // again from them, on the same page, so every row shows what the file now says.
    note.move_aside.connect_clicked({
        let app = app.clone();
        let window = window.downgrade();
        let store = store.clone();
        let stack = stack.clone();
        move |_| match slipstream_config::move_aside(store.path()) {
            Ok(aside) => {
                let told = format!(
                    "The file that couldn't be read is kept as {}. These are the defaults.",
                    aside.file_name().map_or_else(
                        || aside.display().to_string(),
                        |name| { name.to_string_lossy().into_owned() }
                    )
                );
                let page = stack.visible_child_name().map(|name| name.to_string());
                let app = app.clone();
                let window = window.clone();
                glib::idle_add_local_once(move || {
                    // Held, so the app doesn't quit in the moment between the two windows.
                    let _hold = app.hold();
                    if let Some(window) = window.upgrade() {
                        window.destroy();
                    }
                    open_window(&app, page.as_deref(), Some(told));
                });
            }
            Err(err) => store.note.say(
                &format!("Couldn't move {} aside: {err}", store.path().display()),
                "error",
            ),
        }
    });
    window
}

fn sidebar_item(page: &pages::Page) -> gtk::Box {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 11);
    item.append(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(
        page.icons,
    )));
    item.append(&gtk::Label::new(Some(page.title)));
    item
}

/// Draws `window` with GTK's software renderer and saves it as a PNG.
fn save_png(window: &gtk::ApplicationWindow, path: &Path) -> Result<(), String> {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let node = snapshot.to_node().ok_or("the window drew nothing")?;
    let renderer = gsk::CairoRenderer::new();
    renderer
        .realize_for_display(&WidgetExt::display(window))
        .map_err(|err| err.to_string())?;
    let texture = renderer.render_texture(node, None);
    renderer.unrealize();
    texture.save_to_png(path).map_err(|err| err.to_string())
}

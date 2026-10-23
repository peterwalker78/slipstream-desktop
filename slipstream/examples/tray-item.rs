//! A tray item to try the tray with, with no real app: a StatusNotifierItem with a dbusmenu that
//! has a few entries, a checkbox, a pair of radio buttons in a submenu, a separator and an entry
//! that can't be chosen. It registers with the watcher on the session bus and prints every call
//! it gets, so a check can see that a chosen entry reached it.
//!
//! ```sh
//! dbus-run-session -- sh -c 'SLIPSTREAM_TRAY=1 slipstream & sleep 2; target/debug/examples/tray-item'
//! ```
//!
//! `--attention` starts it needing attention, `--name NAME` sets its ID and title, `--icon NAME`
//! gives it an icon name to look up instead of its own pixels, and `--menu-only` makes a left
//! click show its menu.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use zbus::{
    interface,
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, StructureBuilder, Value},
};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";

#[derive(Clone)]
struct Options {
    name: String,
    icon: String,
    attention: bool,
    menu_only: bool,
}

/// What the menu shows, which its checkbox and radio buttons change.
struct State {
    revision: u32,
    paused: bool,
    dark: bool,
    attention: bool,
}

type Shared = Arc<Mutex<State>>;

/// A square picture as ARGB32 in network byte order: a ring in `rgb` with a dot in the middle.
fn picture(side: i32, rgb: [u8; 3]) -> (i32, i32, Vec<u8>) {
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    let centre = (side as f32 - 1.0) / 2.0;
    for y in 0..side {
        for x in 0..side {
            let d = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt() / centre;
            let on = (0.62..0.92).contains(&d) || d < 0.3;
            let [r, g, b] = rgb;
            data.extend_from_slice(&if on { [255, r, g, b] } else { [0, 0, 0, 0] });
        }
    }
    (side, side, data)
}

struct Item {
    options: Options,
    state: Shared,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, x: i32, y: i32) {
        println!("Activate {x} {y}");
    }

    fn secondary_activate(&self, x: i32, y: i32) {
        println!("SecondaryActivate {x} {y}");
    }

    fn context_menu(&self, x: i32, y: i32) {
        println!("ContextMenu {x} {y}");
    }

    fn provide_xdg_activation_token(&self, token: &str) {
        println!("ProvideXdgActivationToken ({} characters)", token.len());
    }

    fn scroll(&self, delta: i32, orientation: &str) {
        println!("Scroll {delta} {orientation}");
    }

    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> String {
        self.options.name.to_lowercase()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.options.name.clone()
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        if self.state.lock().unwrap().attention {
            "NeedsAttention"
        } else {
            "Active"
        }
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.options.icon.clone()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![
            picture(22, [0x3c, 0xf0, 0xc0]),
            picture(64, [0x3c, 0xf0, 0xc0]),
        ]
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![picture(64, [0xff, 0xb5, 0x47])]
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        self.options.menu_only
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        ObjectPath::try_from(MENU_PATH).unwrap().into()
    }

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

/// One menu entry: its id, its properties, and its own entries.
fn entry(id: i32, props: &[(&str, Value<'static>)], children: Vec<OwnedValue>) -> OwnedValue {
    let props: HashMap<String, OwnedValue> = props
        .iter()
        .map(|(key, value)| (key.to_string(), value.try_to_owned().unwrap()))
        .collect();
    let structure = StructureBuilder::new()
        .add_field(id)
        .add_field(props)
        .add_field(children)
        .build()
        .unwrap();
    Value::from(structure).try_to_owned().unwrap()
}

fn label(text: &'static str) -> (&'static str, Value<'static>) {
    ("label", Value::from(text))
}

fn radio(on: bool) -> [(&'static str, Value<'static>); 2] {
    [
        ("toggle-type", Value::from("radio")),
        ("toggle-state", Value::from(i32::from(on))),
    ]
}

/// What `GetLayout` answers: the menu's revision, and its root with everything under it.
type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

struct Menu {
    state: Shared,
}

impl Menu {
    fn layout(&self) -> Layout {
        let state = self.state.lock().unwrap();
        let separator = |id| entry(id, &[("type", Value::from("separator"))], Vec::new());
        let [light_type, light_state] = radio(!state.dark);
        let [dark_type, dark_state] = radio(state.dark);
        let children = vec![
            entry(1, &[label("_Open Vault")], Vec::new()),
            entry(
                2,
                &[
                    label("_Lock Now"),
                    ("icon-name", Value::from("system-lock-screen")),
                ],
                Vec::new(),
            ),
            entry(
                3,
                &[
                    label("_Pause Syncing"),
                    ("toggle-type", Value::from("checkmark")),
                    ("toggle-state", Value::from(i32::from(state.paused))),
                ],
                Vec::new(),
            ),
            separator(4),
            entry(
                5,
                &[
                    label("_Theme"),
                    ("children-display", Value::from("submenu")),
                ],
                vec![
                    entry(6, &[label("_Light"), light_type, light_state], Vec::new()),
                    entry(7, &[label("_Dark"), dark_type, dark_state], Vec::new()),
                ],
            ),
            entry(
                8,
                &[label("Check for _Updates"), ("enabled", Value::from(false))],
                Vec::new(),
            ),
            separator(9),
            entry(10, &[label("_Quit")], Vec::new()),
        ];
        (state.revision, (0, HashMap::new(), children))
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, parent: i32, depth: i32, properties: Vec<String>) -> Layout {
        println!("GetLayout {parent} {depth} {properties:?}");
        self.layout()
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _properties: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        println!("GetGroupProperties {ids:?}");
        Vec::new()
    }

    fn get_property(&self, id: i32, name: &str) -> OwnedValue {
        println!("GetProperty {id} {name}");
        Value::from("").try_to_owned().unwrap()
    }

    async fn event(
        &self,
        id: i32,
        event: &str,
        _data: Value<'_>,
        _timestamp: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        println!("Event {id} {event}");
        if event != "clicked" {
            return;
        }
        let revision = {
            let mut state = self.state.lock().unwrap();
            match id {
                3 => state.paused = !state.paused,
                6 => state.dark = false,
                7 => state.dark = true,
                _ => return,
            }
            state.revision += 1;
            state.revision
        };
        let _ = Menu::layout_updated(&emitter, revision, 0).await;
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event, _, _) in &events {
            println!("Event {id} {event}");
        }
        Vec::new()
    }

    fn about_to_show(&self, id: i32) -> bool {
        println!("AboutToShow {id}");
        false
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        println!("AboutToShowGroup {ids:?}");
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

fn options() -> Options {
    let mut options = Options {
        name: "Test Item".into(),
        icon: String::new(),
        attention: false,
        menu_only: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--attention" => options.attention = true,
            "--menu-only" => options.menu_only = true,
            "--name" => options.name = args.next().unwrap_or_default(),
            "--icon" => options.icon = args.next().unwrap_or_default(),
            other => eprintln!("tray-item: unknown option {other}"),
        }
    }
    options
}

fn main() -> zbus::Result<()> {
    let options = options();
    let state = Arc::new(Mutex::new(State {
        revision: 1,
        paused: false,
        dark: true,
        attention: options.attention,
    }));
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let connection = zbus::blocking::connection::Builder::session()?
        .name(name.as_str())?
        .serve_at(
            ITEM_PATH,
            Item {
                options: options.clone(),
                state: state.clone(),
            },
        )?
        .serve_at(MENU_PATH, Menu { state })?
        .build()?;
    connection.call_method(
        Some("org.kde.StatusNotifierWatcher"),
        "/StatusNotifierWatcher",
        Some("org.kde.StatusNotifierWatcher"),
        "RegisterStatusNotifierItem",
        &(name.as_str(),),
    )?;
    println!("registered as {name}");
    loop {
        std::thread::park();
    }
}

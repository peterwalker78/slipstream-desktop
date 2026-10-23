//! A StatusNotifierItem's properties as Slipstream keeps them: read leniently, cut to size, and
//! good for nothing but drawing a chip and choosing what a click does.
//!
//! Pictures arrive two ways. `IconName` is looked up in the item's own `IconThemePath` here, on
//! the tray's thread, through the same size-limited loader as every other icon, and otherwise in
//! the icon themes when the chip is drawn. `IconPixmap` is pixels: ARGB32 in network byte order,
//! one picture per size, of which the one nearest the size wanted is kept, made smaller.

use std::{collections::HashMap, path::Path};

use resvg::tiny_skia::{IntSize, Pixmap};
use serde::Deserialize;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Type, Value, as_value};

use super::clean;

/// The pictures kept are at most this many pixels square, enough for a chip at three times.
pub const PICTURE_PX: u32 = 64;
/// A picture an item sends is read only up to this size on a side.
const PIXMAP_SIDE: i32 = 1024;
/// The longest title or ID kept, in characters.
const TEXT_CHARS: usize = 128;
/// The longest icon name looked up: a file name, no more.
const NAME_CHARS: usize = 255;

/// What the item says about the state of its app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing to say: the item may be hidden.
    Passive,
    #[default]
    Active,
    /// Something wants the person's attention, a message waiting, say.
    NeedsAttention,
}

impl Status {
    fn parse(status: &str) -> Self {
        match status {
            "Passive" => Status::Passive,
            "NeedsAttention" => Status::NeedsAttention,
            _ => Status::Active,
        }
    }
}

/// One of the item's pictures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picture {
    /// A name to look for in the icon themes.
    pub name: Option<String>,
    /// The picture itself, at most `PICTURE_PX` square: a file from the item's own theme folder,
    /// or the pixels it sent. Drawn when the name is missing or finds nothing.
    pub pixels: Option<Pixmap>,
}

impl Picture {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.pixels.is_none()
    }
}

/// What a chip needs to know about an item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub icon: Picture,
    /// What it shows while it needs attention, if anything different.
    pub attention: Picture,
    /// It only has a menu: a left click shows it rather than activating anything.
    pub item_is_menu: bool,
    /// Its dbusmenu object's path, when it has one.
    pub menu: Option<String>,
}

/// One picture as the bus carries it: width, height, and ARGB32 pixels in network byte order.
type WirePixmap = (i32, i32, Vec<u8>);

/// The properties as the bus carries them. Read straight into their types, so pixels stay bytes
/// rather than becoming a value apiece; properties Slipstream doesn't use are skipped.
#[derive(Debug, Default, Deserialize, Type)]
#[zvariant(signature = "a{sv}", crate = "zbus::zvariant")]
#[serde(default, rename_all = "PascalCase")]
pub struct Wire {
    #[serde(with = "as_value::optional")]
    id: Option<String>,
    #[serde(with = "as_value::optional")]
    title: Option<String>,
    #[serde(with = "as_value::optional")]
    status: Option<String>,
    #[serde(with = "as_value::optional")]
    icon_name: Option<String>,
    #[serde(with = "as_value::optional")]
    icon_pixmap: Option<Vec<WirePixmap>>,
    #[serde(with = "as_value::optional")]
    attention_icon_name: Option<String>,
    #[serde(with = "as_value::optional")]
    attention_icon_pixmap: Option<Vec<WirePixmap>>,
    #[serde(with = "as_value::optional")]
    icon_theme_path: Option<String>,
    #[serde(with = "as_value::optional")]
    item_is_menu: Option<bool>,
    #[serde(with = "as_value::optional")]
    menu: Option<OwnedObjectPath>,
}

impl Wire {
    /// The properties from a dictionary of values, for an item that gave one of them a type the
    /// specification doesn't: whatever has the right type is kept, the rest left out.
    pub fn from_values(values: &HashMap<String, OwnedValue>) -> Self {
        let text = |key: &str| {
            values
                .get(key)
                .and_then(|value| string(value))
                .map(String::from)
        };
        let pixmaps = |key: &str| values.get(key).map(|value| pixmaps(value));
        Wire {
            id: text("Id"),
            title: text("Title"),
            status: text("Status"),
            icon_name: text("IconName"),
            icon_pixmap: pixmaps("IconPixmap"),
            attention_icon_name: text("AttentionIconName"),
            attention_icon_pixmap: pixmaps("AttentionIconPixmap"),
            icon_theme_path: text("IconThemePath"),
            item_is_menu: values.get("ItemIsMenu").and_then(|value| match &**value {
                Value::Bool(on) => Some(*on),
                _ => None,
            }),
            menu: values
                .get("Menu")
                .and_then(|value| string(value))
                .and_then(|path| OwnedObjectPath::try_from(path.to_string()).ok()),
        }
    }

    /// What Slipstream keeps of it. `theme_file` finds an icon by name in the item's own theme
    /// folder, and `load` reads that file at `PICTURE_PX`: both run on the tray's thread.
    pub fn props(
        self,
        theme_file: impl Fn(&Path, &str) -> Option<std::path::PathBuf>,
        load: impl Fn(&Path) -> Option<Pixmap>,
    ) -> Props {
        let theme = self
            .icon_theme_path
            .as_deref()
            .map(Path::new)
            .filter(|dir| dir.is_absolute());
        let picture = |name: Option<String>, pixmaps: Option<Vec<WirePixmap>>| {
            let name = name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty() && name.len() <= NAME_CHARS);
            let from_pixmaps = || pixmaps.as_deref().and_then(best_pixmap);
            match name {
                // A file named outright: an app's own picture in its own folder.
                Some(path) if path.starts_with('/') => Picture {
                    name: None,
                    pixels: picture_file(Path::new(&path))
                        .and_then(&load)
                        .or_else(from_pixmaps),
                },
                Some(name) => match theme.and_then(|dir| theme_file(dir, &name)) {
                    Some(file) => Picture {
                        name: None,
                        pixels: load(&file).or_else(from_pixmaps),
                    },
                    None => Picture {
                        name: Some(name),
                        pixels: from_pixmaps(),
                    },
                },
                None => Picture {
                    name: None,
                    pixels: from_pixmaps(),
                },
            }
        };
        let menu = self
            .menu
            .map(|path| path.to_string())
            .filter(|path| path != "/" && path != "/NO_DBUSMENU");
        Props {
            id: clean(self.id.as_deref().unwrap_or_default(), TEXT_CHARS),
            title: clean(self.title.as_deref().unwrap_or_default(), TEXT_CHARS),
            status: Status::parse(self.status.as_deref().unwrap_or_default()),
            icon: picture(self.icon_name, self.icon_pixmap),
            attention: picture(self.attention_icon_name, self.attention_icon_pixmap),
            item_is_menu: self.item_is_menu.unwrap_or(false),
            menu,
        }
    }
}

/// `path` if it names a picture the icon loader reads.
fn picture_file(path: &Path) -> Option<&Path> {
    path.extension()
        .is_some_and(|ext| ext == "png" || ext == "svg")
        .then_some(path)
}

fn string<'a>(value: &'a Value<'a>) -> Option<&'a str> {
    match value {
        Value::Str(text) => Some(text.as_str()),
        Value::ObjectPath(path) => Some(path.as_str()),
        Value::Value(inner) => string(inner),
        _ => None,
    }
}

/// Pictures from an `a(iiay)` value, skipping any that isn't one.
fn pixmaps(value: &Value<'_>) -> Vec<WirePixmap> {
    let Value::Array(list) = value else {
        return Vec::new();
    };
    list.inner()
        .iter()
        .filter_map(|picture| {
            let Value::Structure(fields) = picture else {
                return None;
            };
            match fields.fields() {
                [Value::I32(w), Value::I32(h), Value::Array(bytes)] => {
                    let wanted = usize::try_from(i64::from(*w) * i64::from(*h) * 4).ok()?;
                    if !(1..=PIXMAP_SIDE).contains(w)
                        || !(1..=PIXMAP_SIDE).contains(h)
                        || bytes.len() < wanted
                    {
                        return None;
                    }
                    let data = bytes
                        .inner()
                        .iter()
                        .take(wanted)
                        .map(|byte| match byte {
                            Value::U8(byte) => Some(*byte),
                            _ => None,
                        })
                        .collect::<Option<Vec<u8>>>()?;
                    Some((*w, *h, data))
                }
                _ => None,
            }
        })
        .collect()
}

/// The picture nearest `PICTURE_PX` of those an item sent, as premultiplied RGBA no bigger than
/// that: the smallest at least that big, else the biggest. Pictures whose sizes don't add up are
/// passed over.
pub fn best_pixmap(list: &[WirePixmap]) -> Option<Pixmap> {
    let wanted = PICTURE_PX as i32;
    let fits = |(w, h, data): &&WirePixmap| {
        (1..=PIXMAP_SIDE).contains(w)
            && (1..=PIXMAP_SIDE).contains(h)
            && data.len() >= (*w as usize) * (*h as usize) * 4
    };
    let usable: Vec<&WirePixmap> = list.iter().filter(fits).collect();
    let chosen = usable
        .iter()
        .filter(|(w, h, _)| (*w).min(*h) >= wanted)
        .min_by_key(|(w, h, _)| w * h)
        .or_else(|| usable.iter().max_by_key(|(w, h, _)| w * h))?;
    let (w, h, data) = chosen;
    let pixmap = from_argb(*w as u32, *h as u32, data)?;
    if pixmap.width() > PICTURE_PX || pixmap.height() > PICTURE_PX {
        crate::paint::fit(&pixmap, PICTURE_PX)
    } else {
        Some(pixmap)
    }
}

/// ARGB32 in network byte order, `w` × `h`, as tiny-skia's premultiplied RGBA.
fn from_argb(w: u32, h: u32, argb: &[u8]) -> Option<Pixmap> {
    let size = IntSize::from_wh(w, h)?;
    let mut rgba = Vec::with_capacity(argb.len());
    for pixel in argb.chunks_exact(4).take((w * h) as usize) {
        let [a, r, g, b] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        let premultiply = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        rgba.extend_from_slice(&[premultiply(r), premultiply(g), premultiply(b), a]);
    }
    Pixmap::from_vec(rgba, size)
}

/// The properties from a `GetAll` reply's body: read into their types, or, if the item gave one
/// a type of its own, value by value. A reply bigger than `lenient` bytes is only read the first
/// way, since reading it value by value costs many times its size.
pub fn read(body: &zbus::message::Body, lenient: usize) -> Option<Wire> {
    match body.deserialize::<Wire>() {
        Ok(wire) => Some(wire),
        Err(_) if body.len() <= lenient => body
            .deserialize::<HashMap<String, OwnedValue>>()
            .ok()
            .map(|values| Wire::from_values(&values)),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant;

    /// A dictionary of values as the bus carries it.
    fn encode(values: HashMap<&str, Value<'_>>) -> zvariant::serialized::Data<'static, 'static> {
        let context = zvariant::serialized::Context::new_dbus(zvariant::LE, 0);
        zvariant::to_bytes(context, &values).unwrap()
    }

    fn no_theme(_: &Path, _: &str) -> Option<std::path::PathBuf> {
        None
    }

    fn no_file(_: &Path) -> Option<Pixmap> {
        None
    }

    #[test]
    fn pixels_arrive_as_argb_in_network_order() {
        // An opaque red pixel, then a half-transparent white one.
        let pixmap = from_argb(2, 1, &[255, 255, 0, 0, 128, 255, 255, 255]).unwrap();
        assert_eq!(pixmap.data(), &[255, 0, 0, 255, 128, 128, 128, 128]);
    }

    #[test]
    fn the_picture_nearest_the_size_wanted_is_kept_and_made_smaller() {
        let square = |side: i32| (side, side, vec![255u8; (side * side * 4) as usize]);
        let chosen = best_pixmap(&[square(16), square(128), square(256)]).unwrap();
        assert_eq!(chosen.width(), PICTURE_PX, "128 made smaller, not 256");
        let small = best_pixmap(&[square(16), square(22)]).unwrap();
        assert_eq!(small.width(), 22, "else the biggest there is");
    }

    #[test]
    fn pictures_whose_sizes_dont_add_up_are_refused() {
        assert!(
            best_pixmap(&[(4, 4, vec![0; 63])]).is_none(),
            "short of bytes"
        );
        assert!(best_pixmap(&[(0, 4, Vec::new())]).is_none());
        assert!(best_pixmap(&[(-4, -4, vec![0; 64])]).is_none());
        assert!(
            best_pixmap(&[(4096, 1, vec![0; 4096 * 4])]).is_none(),
            "too wide"
        );
        assert!(best_pixmap(&[]).is_none());
    }

    #[test]
    fn a_reply_is_read_into_its_types_and_unused_properties_are_skipped() {
        let tooltip = Value::from(("", Vec::<WirePixmap>::new(), "Synced", "All files"));
        let data = encode(HashMap::from([
            ("Id", Value::from("Nextcloud")),
            ("Status", Value::from("NeedsAttention")),
            ("IconName", Value::from("state-ok")),
            ("ToolTip", tooltip),
            ("ItemIsMenu", Value::from(true)),
            (
                "Menu",
                Value::from(zvariant::ObjectPath::try_from("/MenuBar").unwrap()),
            ),
            (
                "IconPixmap",
                Value::from(vec![(1i32, 1i32, vec![255u8, 0, 255, 0])]),
            ),
        ]));
        let wire: Wire = data.deserialize().unwrap().0;
        let props = wire.props(no_theme, no_file);
        assert_eq!(props.id, "Nextcloud");
        assert_eq!(props.status, Status::NeedsAttention);
        assert_eq!(props.icon.name.as_deref(), Some("state-ok"));
        assert_eq!(props.icon.pixels.as_ref().map(Pixmap::width), Some(1));
        assert!(props.item_is_menu);
        assert_eq!(props.menu.as_deref(), Some("/MenuBar"));
    }

    #[test]
    fn a_property_of_the_wrong_type_loses_only_itself() {
        let values: HashMap<String, OwnedValue> = HashMap::from([
            ("Title".into(), OwnedValue::from(7i32)),
            (
                "IconName".into(),
                Value::from("mail-unread").try_into().unwrap(),
            ),
            ("Menu".into(), Value::from("/Menu").try_into().unwrap()),
            ("ItemIsMenu".into(), Value::from("yes").try_into().unwrap()),
        ]);
        let props = Wire::from_values(&values).props(no_theme, no_file);
        assert_eq!(props.title, "");
        assert_eq!(props.icon.name.as_deref(), Some("mail-unread"));
        assert_eq!(
            props.menu.as_deref(),
            Some("/Menu"),
            "a path sent as a string"
        );
        assert!(!props.item_is_menu);
    }

    #[test]
    fn a_menu_that_is_no_menu_is_left_out() {
        for path in ["/", "/NO_DBUSMENU"] {
            let wire = Wire {
                menu: Some(OwnedObjectPath::try_from(path).unwrap()),
                ..Wire::default()
            };
            assert_eq!(wire.props(no_theme, no_file).menu, None);
        }
    }

    #[test]
    fn an_icon_in_the_items_own_theme_folder_is_read_there() {
        let wire = Wire {
            icon_name: Some("tray-1".into()),
            icon_theme_path: Some("/run/app/icons".into()),
            ..Wire::default()
        };
        let found = |dir: &Path, name: &str| Some(dir.join(format!("{name}.png")));
        let loaded = |path: &Path| {
            assert_eq!(path, Path::new("/run/app/icons/tray-1.png"));
            Pixmap::new(8, 8)
        };
        let props = wire.props(found, loaded);
        assert_eq!(props.icon.name, None, "not looked for again in the themes");
        assert!(props.icon.pixels.is_some());
    }

    #[test]
    fn an_icon_named_by_path_is_read_only_if_it_is_a_picture() {
        let named = |path: &str| Wire {
            icon_name: Some(path.into()),
            ..Wire::default()
        };
        let loaded = |_: &Path| Pixmap::new(8, 8);
        assert!(
            named("/tmp/app/tray.png")
                .props(no_theme, loaded)
                .icon
                .pixels
                .is_some()
        );
        let refused = named("/etc/passwd").props(no_theme, loaded);
        assert!(refused.icon.is_empty(), "neither read nor looked up");
    }
}

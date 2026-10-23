//! An app's menu (`com.canonical.dbusmenu`) as Slipstream draws it: entries with their labels,
//! checkmarks and radio buttons, separators and submenus, read from `GetLayout` and cut to a size
//! a menu on a screen can have. Nothing in it is ever run: an entry is only its id, which goes
//! back to the app that sent it.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

use super::clean;

/// The properties asked for. Apps that honour the list send nothing else, pictures included.
pub const PROPERTIES: [&str; 8] = [
    "type",
    "label",
    "enabled",
    "visible",
    "toggle-type",
    "toggle-state",
    "children-display",
    "icon-name",
];

/// How deep submenus go, the root's entries being the first level.
pub const MAX_DEPTH: usize = 5;
/// The most entries one level shows, and in the whole menu.
pub const MAX_ENTRIES: usize = 64;
pub const MAX_TOTAL: usize = 512;
/// The longest label kept, in characters.
const LABEL_CHARS: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    None,
    Check(bool),
    Radio(bool),
}

/// A label with its mnemonic: the letter after an underscore, which picks the entry from the
/// keyboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    /// Which character of `text` is the mnemonic, counting characters.
    pub mnemonic: Option<usize>,
}

impl Label {
    /// A label as dbusmenu writes it: `_` marks the next letter as the mnemonic and is dropped,
    /// and `__` is an underscore.
    pub fn parse(raw: &str) -> Self {
        let text = clean(raw, LABEL_CHARS * 2);
        let mut label = Label::default();
        let mut chars = text.chars();
        let mut count = 0;
        while let Some(ch) = chars.next() {
            if count >= LABEL_CHARS {
                break;
            }
            let ch = match ch {
                '_' => match chars.next() {
                    Some('_') => '_',
                    Some(next) => {
                        if label.mnemonic.is_none() && next.is_alphanumeric() {
                            label.mnemonic = Some(count);
                        }
                        next
                    }
                    None => break,
                },
                ch => ch,
            };
            label.text.push(ch);
            count += 1;
        }
        label
    }

    /// The mnemonic letter, in lower case.
    pub fn key(&self) -> Option<char> {
        let at = self.mnemonic?;
        self.text.chars().nth(at)?.to_lowercase().next()
    }
}

/// One entry, and its own entries when it opens a submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i32,
    pub separator: bool,
    pub label: Label,
    pub enabled: bool,
    pub toggle: Toggle,
    pub icon: Option<String>,
    /// Opens a submenu. Its entries may arrive only once it is about to show.
    pub submenu: bool,
    /// The visible entries under it, with no separator first, last or twice running.
    pub children: Vec<Entry>,
}

impl Entry {
    /// Whether the keyboard and the pointer can choose it.
    pub fn choosable(&self) -> bool {
        !self.separator && self.enabled
    }

    /// The entry with this id, here or anywhere under it.
    pub fn find(&self, id: i32) -> Option<&Entry> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }
}

/// A whole menu, from the root down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub revision: u32,
    pub root: Entry,
}

/// A layout node as `GetLayout` sends it: its id, its properties and its children, each a
/// variant holding another node.
pub type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// A node's properties, borrowed from the reply.
type Props<'a> = Vec<(&'a str, &'a Value<'a>)>;

impl Layout {
    /// The menu from a `GetLayout` reply. Entries past the depth or the size are left out, and
    /// invisible entries go; anything malformed is skipped rather than refused.
    pub fn parse(revision: u32, root: &Node) -> Self {
        let (id, props, children) = root;
        let props: Props = props
            .iter()
            .map(|(key, value)| (key.as_str(), plain(value)))
            .collect();
        let mut budget = MAX_TOTAL;
        let children = entries(children.iter().map(|child| &**child), 1, &mut budget);
        let mut root = entry(*id, &props, children);
        root.submenu = true;
        Layout { revision, root }
    }
}

fn entries<'a>(
    children: impl Iterator<Item = &'a Value<'a>>,
    depth: usize,
    budget: &mut usize,
) -> Vec<Entry> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut list = Vec::new();
    for child in children {
        if list.len() >= MAX_ENTRIES || *budget == 0 {
            break;
        }
        let Some((id, props, grandchildren)) = node(child) else {
            continue;
        };
        if !flag(&props, "visible", true) {
            continue;
        }
        *budget -= 1;
        let children = entries(grandchildren.iter(), depth + 1, budget);
        list.push(entry(id, &props, children));
    }
    tidy_separators(list)
}

/// A value with any variants around it taken off.
fn plain<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => plain(inner),
        other => other,
    }
}

/// A node, if `value` is one.
fn node<'a>(value: &'a Value<'a>) -> Option<(i32, Props<'a>, &'a [Value<'a>])> {
    let Value::Structure(structure) = plain(value) else {
        return None;
    };
    let [Value::I32(id), Value::Dict(props), Value::Array(children)] = structure.fields() else {
        return None;
    };
    let props = props
        .iter()
        .filter_map(|(key, value)| match key {
            Value::Str(key) => Some((key.as_str(), plain(value))),
            _ => None,
        })
        .collect();
    Some((*id, props, children.inner()))
}

fn entry(id: i32, props: &Props, children: Vec<Entry>) -> Entry {
    let separator = text(props, "type") == Some("separator");
    let toggle_on = matches!(get(props, "toggle-state"), Some(Value::I32(1)));
    let toggle = match text(props, "toggle-type") {
        Some("checkmark") => Toggle::Check(toggle_on),
        Some("radio") => Toggle::Radio(toggle_on),
        _ => Toggle::None,
    };
    let submenu =
        !separator && (text(props, "children-display") == Some("submenu") || !children.is_empty());
    Entry {
        id,
        separator,
        label: Label::parse(text(props, "label").unwrap_or_default()),
        enabled: flag(props, "enabled", true),
        toggle,
        icon: text(props, "icon-name")
            .map(|name| clean(name, 255))
            .filter(|name| !name.is_empty() && !name.contains('/')),
        submenu,
        children,
    }
}

fn get<'a>(props: &Props<'a>, key: &str) -> Option<&'a Value<'a>> {
    props
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| *value)
}

fn text<'a>(props: &Props<'a>, key: &str) -> Option<&'a str> {
    match get(props, key) {
        Some(Value::Str(text)) => Some(text.as_str()),
        _ => None,
    }
}

fn flag(props: &Props, key: &str, default: bool) -> bool {
    match get(props, key) {
        Some(Value::Bool(on)) => *on,
        _ => default,
    }
}

/// No separator at either end, and never two together.
fn tidy_separators(list: Vec<Entry>) -> Vec<Entry> {
    let mut tidy: Vec<Entry> = Vec::with_capacity(list.len());
    for entry in list {
        if entry.separator && tidy.last().is_none_or(|last| last.separator) {
            continue;
        }
        tidy.push(entry);
    }
    if tidy.last().is_some_and(|last| last.separator) {
        tidy.pop();
    }
    tidy
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use zbus::zvariant::{Array, Dict, StructureBuilder, Value};

    /// A layout node as an app would send it.
    pub fn node(
        id: i32,
        props: &[(&str, Value<'static>)],
        children: Vec<Value<'static>>,
    ) -> Value<'static> {
        let props: HashMap<String, Value<'static>> = props
            .iter()
            .map(|(key, value)| (key.to_string(), value.try_clone().unwrap()))
            .collect();
        let children: Vec<Value<'static>> = children
            .into_iter()
            .map(|child| Value::Value(Box::new(child)))
            .collect();
        Value::Structure(
            StructureBuilder::new()
                .add_field(id)
                .append_field(Value::Dict(Dict::from(props)))
                .append_field(Value::Array(Array::from(children)))
                .build()
                .unwrap(),
        )
    }

    /// A whole menu, `children` under the root, as `GetLayout` would read it.
    pub fn layout(children: Vec<Value<'static>>) -> Layout {
        let children = children
            .into_iter()
            .map(|child| OwnedValue::try_from(child).unwrap())
            .collect();
        Layout::parse(1, &(0, HashMap::new(), children))
    }

    pub fn labelled(id: i32, label: &'static str) -> Value<'static> {
        node(id, &[("label", Value::from(label))], Vec::new())
    }

    pub fn separator(id: i32) -> Value<'static> {
        node(id, &[("type", Value::from("separator"))], Vec::new())
    }

    #[test]
    fn mnemonics_drop_their_underscore_and_a_doubled_one_stays() {
        assert_eq!(
            Label::parse("_Open"),
            Label {
                text: "Open".into(),
                mnemonic: Some(0)
            }
        );
        assert_eq!(Label::parse("E_xit").key(), Some('x'));
        let both = Label::parse("Save __As _Copy");
        assert_eq!(both.text, "Save _As Copy");
        assert_eq!(both.key(), Some('c'));
        assert_eq!(Label::parse("trailing_").text, "trailing");
        assert_eq!(Label::parse("plain").mnemonic, None);
    }

    #[test]
    fn entries_read_their_kind_state_and_submenu() {
        let menu = layout(vec![
            labelled(1, "_Show"),
            node(
                2,
                &[
                    ("label", Value::from("Paused")),
                    ("toggle-type", Value::from("checkmark")),
                    ("toggle-state", Value::from(1i32)),
                ],
                Vec::new(),
            ),
            node(
                3,
                &[
                    ("label", Value::from("Off")),
                    ("toggle-type", Value::from("radio")),
                ],
                Vec::new(),
            ),
            node(
                4,
                &[
                    ("label", Value::from("Gone")),
                    ("enabled", Value::from(false)),
                ],
                Vec::new(),
            ),
            node(
                5,
                &[
                    ("label", Value::from("More")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![labelled(6, "Inside")],
            ),
        ]);
        let entries = &menu.root.children;
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].label.text, "Show");
        assert_eq!(entries[1].toggle, Toggle::Check(true));
        assert_eq!(entries[2].toggle, Toggle::Radio(false));
        assert!(!entries[3].choosable());
        assert!(entries[4].submenu);
        assert_eq!(menu.root.find(6).unwrap().label.text, "Inside");
    }

    #[test]
    fn hidden_entries_and_stray_separators_go() {
        let menu = layout(vec![
            separator(1),
            labelled(2, "One"),
            separator(3),
            separator(4),
            node(
                5,
                &[
                    ("label", Value::from("Hidden")),
                    ("visible", Value::from(false)),
                ],
                Vec::new(),
            ),
            labelled(6, "Two"),
            separator(7),
        ]);
        let ids: Vec<i32> = menu.root.children.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [2, 3, 6]);
    }

    #[test]
    fn a_huge_or_deep_menu_is_cut_to_size() {
        let wide = layout((0..500).map(|id| labelled(id, "Entry")).collect());
        assert_eq!(wide.root.children.len(), MAX_ENTRIES);
        let mut deep = labelled(100, "Bottom");
        for id in (0..20).rev() {
            deep = node(id, &[("label", Value::from("Level"))], vec![deep]);
        }
        let deep = layout(vec![deep]);
        let mut depth = 0;
        let mut at = &deep.root;
        while let Some(child) = at.children.first() {
            depth += 1;
            at = child;
        }
        assert_eq!(depth, MAX_DEPTH);
    }

    #[test]
    fn malformed_nodes_are_skipped_not_fatal() {
        let menu = layout(vec![
            Value::from("not a node"),
            Value::from((1i32, 2i32)),
            labelled(3, "Fine"),
            node(
                4,
                &[
                    ("label", Value::from(5u32)),
                    ("toggle-state", Value::from("on")),
                ],
                Vec::new(),
            ),
        ]);
        let ids: Vec<i32> = menu.root.children.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [3, 4]);
        assert_eq!(menu.root.children[1].label.text, "");
    }
}

//! Reading the JSON an agent hands its hooks, and the records agents keep.
//!
//! A value that is missing, `null` or `false`, or reached through something that isn't an object
//! or a list, counts as absent, so a list of alternatives takes the first that is there.

use serde_json::Value;

use crate::text;

/// One step into a value: a key of an object or an index into a list.
#[derive(Clone, Copy)]
pub enum Step {
    Key(&'static str),
    Index(usize),
}

pub use Step::{Index, Key};

/// The value at `path`, if every step leads somewhere and it is there.
pub fn at<'a>(value: &'a Value, path: &[Step]) -> Option<&'a Value> {
    let mut here = value;
    for step in path {
        here = match step {
            Key(key) => here.as_object()?.get(*key)?,
            Index(index) => here.as_array()?.get(*index)?,
        };
    }
    present(here)
}

/// The value itself, unless it is `null` or `false`.
pub fn present(value: &Value) -> Option<&Value> {
    (!matches!(value, Value::Null | Value::Bool(false))).then_some(value)
}

/// The first of `paths` that is there.
pub fn first<'a>(value: &'a Value, paths: &[&[Step]]) -> Option<&'a Value> {
    paths.iter().find_map(|path| at(value, path))
}

/// A value as text: a string as it is, anything else as JSON.
pub fn to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The hook's JSON, which may be missing or not JSON at all.
pub struct Payload(Option<Value>);

impl Payload {
    pub fn parse(text: &str) -> Payload {
        Payload(serde_json::from_str(text.trim()).ok())
    }

    pub fn is_object(&self) -> bool {
        matches!(self.0, Some(Value::Object(_)))
    }

    /// The first of `paths` that is there.
    pub fn get(&self, paths: &[&[Step]]) -> Option<&Value> {
        first(self.0.as_ref()?, paths)
    }

    /// The first of `paths` that is there, as text.
    pub fn text(&self, paths: &[&[Step]]) -> Option<String> {
        self.get(paths).map(to_text)
    }

    /// A value for a notification: one line of at most 160 characters, with the characters
    /// notification markup cares about escaped. A list is joined with spaces. Empty if absent.
    pub fn field(&self, paths: &[&[Step]]) -> String {
        let joined = match self.get(paths) {
            None => return String::new(),
            Some(Value::Array(items)) => items.iter().map(to_text).collect::<Vec<_>>().join(" "),
            Some(other) => to_text(other),
        };
        text::escape_markup(text::cut(&text::collapse_whitespace(&joined), 160))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_one_escaped_line_cut_to_length() {
        let payload = Payload::parse(r#"{"message": "Allow <rm>\n  &   go?", "long": "x"}"#);
        assert_eq!(
            payload.field(&[&[Key("message")]]),
            "Allow &lt;rm&gt; &amp; go?"
        );
        let long = Payload::parse(&format!(r#"{{"m": "{}"}}"#, "é".repeat(200)));
        assert_eq!(long.field(&[&[Key("m")]]).chars().count(), 160);
    }

    #[test]
    fn alternatives_skip_what_is_missing_null_false_or_the_wrong_shape() {
        let payload = Payload::parse(
            r#"{"cwd": null, "workspacePaths": "oops", "b": false, "c": ["x", 1, null]}"#,
        );
        let cwd: &[&[Step]] = &[&[Key("cwd")], &[Key("workspacePaths"), Index(0)]];
        assert_eq!(payload.field(cwd), "");
        assert_eq!(payload.field(&[&[Key("b")], &[Key("c")]]), "x 1 null");
    }

    #[test]
    fn a_payload_that_is_not_json_has_nothing_in_it() {
        let payload = Payload::parse("not json");
        assert!(!payload.is_object());
        assert_eq!(payload.field(&[&[Key("cwd")]]), "");
    }
}

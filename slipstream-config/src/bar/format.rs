//! A module's `format`, filled in from what its command said, as the plain text the bar shows.

use super::{Icons, Module, Output, Tint, output::plain};

/// `max-length` when the settings don't give one: enough for a short reading and a word or two.
pub const DEFAULT_MAX_LENGTH: usize = 32;
/// The longest `max-length` honoured. The bar has no room for more.
const LONGEST: usize = 120;
/// The most lines of a tooltip shown, and the most characters of each.
pub const TOOLTIP_LINES: usize = 16;
const TOOLTIP_LINE: usize = 120;

/// What a module shows: its text and tooltip as plain text, and its tint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    pub text: String,
    pub tooltip: Option<String>,
    pub tint: Tint,
}

/// What `module` shows for `output`, or nothing while it is hidden: Waybar hides a module whose
/// command printed no text. A module with no command shows its `format` as it is.
pub fn shown(module: &Module, output: &Output) -> Option<Shown> {
    let has_command = !module.exec.trim().is_empty();
    if has_command && output.text.trim().is_empty() {
        return None;
    }
    let template = module.format.as_deref().unwrap_or("{}");
    let filled = format(template, output, &module.format_icons);
    let text = shorten(
        plain(&filled, false).trim(),
        module.max_length.unwrap_or(DEFAULT_MAX_LENGTH).min(LONGEST),
    );
    if text.is_empty() {
        return None;
    }
    let tooltip = output
        .tooltip
        .as_deref()
        .filter(|_| module.tooltip)
        .map(|tooltip| {
            plain(tooltip, true)
                .lines()
                .map(|line| shorten(line.trim_end(), TOOLTIP_LINE))
                .take(TOOLTIP_LINES)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|tooltip| !tooltip.trim().is_empty());
    Some(Shown {
        text,
        tooltip,
        tint: output.tint,
    })
}

/// `text` cut to `most` characters, the last of them an ellipsis.
fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let kept: String = text.chars().take(most.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// `template` with each `{field}` filled in, as Waybar's fmt-style format: `{}` and `{text}` are
/// the text, `{alt}`, `{icon}` and `{percentage}` the rest, `{{` and `}}` are braces, and a field
/// may carry a width and alignment (`{percentage:>3}`). Anything else is left as it is.
pub fn format(template: &str, output: &Output, icons: &Icons) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        let field = tail
            .strip_prefix('{')
            .and_then(|inner| Some(&inner[..inner.find('}')?]));
        let Some(field) = field else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
            continue;
        };
        let (name, spec) = field.split_once(':').unwrap_or((field, ""));
        let value = match name {
            "" | "text" => Some(output.text.clone()),
            "alt" => Some(output.alt.clone()),
            "icon" => Some(icon(icons, output.percentage.unwrap_or(0), &output.alt)),
            "percentage" => Some(output.percentage.unwrap_or(0).to_string()),
            _ => None,
        };
        match value {
            Some(value) => out.push_str(&pad(&value, spec, name == "percentage")),
            None => out.push_str(&tail[..field.len() + 2]),
        }
        rest = &tail[field.len() + 2..];
    }
    out.push_str(rest);
    out
}

/// `value` padded as a fmt spec of `[[fill]align][width]` says. Numbers go right by default.
fn pad(value: &str, spec: &str, number: bool) -> String {
    let mut chars: Vec<char> = spec.chars().collect();
    let align_at = |c: &char| matches!(c, '<' | '>' | '^');
    let (fill, align) = match chars.as_slice() {
        [fill, align, ..] if align_at(align) => {
            let found = (*fill, *align);
            chars.drain(..2);
            found
        }
        [align, ..] if align_at(align) => {
            let found = (' ', *align);
            chars.drain(..1);
            found
        }
        _ => (' ', if number { '>' } else { '<' }),
    };
    let width: usize = chars
        .iter()
        .collect::<String>()
        .parse()
        .unwrap_or(0)
        .min(LONGEST);
    let short = width.saturating_sub(value.chars().count());
    let fill = |n: usize| fill.to_string().repeat(n);
    match align {
        '>' => format!("{}{value}", fill(short)),
        '^' => format!("{}{value}{}", fill(short / 2), fill(short - short / 2)),
        _ => format!("{value}{}", fill(short)),
    }
}

/// The icon for `percentage` and `alt`, as Waybar picks it: by `alt` from a table (else its
/// `default`), then from a ramp by `percentage`, the ramp's steps spread evenly over 0 to 100.
fn icon(icons: &Icons, percentage: u8, alt: &str) -> String {
    let icons = match icons {
        Icons::ByAlt(by_alt) => match by_alt.get(alt).filter(|_| !alt.is_empty()) {
            Some(icons) => icons,
            None => match by_alt.get("default") {
                Some(icons) => icons,
                None => return String::new(),
            },
        },
        icons => icons,
    };
    match icons {
        Icons::One(icon) => icon.clone(),
        Icons::Ramp(ramp) if !ramp.is_empty() => {
            let step = (100 / ramp.len()).max(1);
            ramp[(percentage as usize / step).min(ramp.len() - 1)].clone()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn said(text: &str) -> Output {
        Output {
            text: text.into(),
            ..Output::default()
        }
    }

    fn module(format: &str) -> Module {
        Module {
            exec: "true".into(),
            format: Some(format.into()),
            ..Module::default()
        }
    }

    #[test]
    fn fields_are_filled_in_and_the_rest_left_as_written() {
        let output = Output {
            text: "21°C".into(),
            alt: "sunny".into(),
            percentage: Some(7),
            ..Output::default()
        };
        let icons = Icons::None;
        assert_eq!(format("{}", &output, &icons), "21°C");
        assert_eq!(format("{text} ({alt})", &output, &icons), "21°C (sunny)");
        assert_eq!(format("{percentage:>3}%", &output, &icons), "  7%");
        assert_eq!(format("[{:*^8}]", &output, &icons), "[**21°C**]");
        assert_eq!(format("{{}} {nope} {", &output, &icons), "{} {nope} {");
        assert_eq!(format("}", &output, &icons), "}");
        assert_eq!(format("{percentage}", &said("x"), &icons), "0");
    }

    #[test]
    fn icons_are_picked_by_alt_then_by_percentage() {
        let ramp = Icons::Ramp(vec!["low".into(), "mid".into(), "high".into()]);
        let pick = |percentage, alt| icon(&ramp, percentage, alt);
        assert_eq!(pick(0, ""), "low");
        assert_eq!(pick(32, ""), "low");
        assert_eq!(pick(33, ""), "mid", "as Waybar steps it");
        assert_eq!(pick(100, ""), "high");
        let by_alt = Icons::ByAlt(BTreeMap::from([
            ("muted".to_string(), Icons::One("M".into())),
            ("default".to_string(), ramp.clone()),
        ]));
        assert_eq!(icon(&by_alt, 90, "muted"), "M");
        assert_eq!(icon(&by_alt, 90, "playing"), "high");
        assert_eq!(icon(&by_alt, 90, ""), "high");
        assert_eq!(icon(&Icons::ByAlt(BTreeMap::new()), 5, "x"), "");
        let many = Icons::Ramp((0..300).map(|n| n.to_string()).collect());
        assert_eq!(icon(&many, 100, ""), "100", "more steps than percent");
    }

    #[test]
    fn a_module_shows_plain_text_cut_to_its_length() {
        let shown = |module: &Module, output: &Output| shown(module, output).map(|s| s.text);
        assert_eq!(
            shown(&module("<b>{}</b> &amp; more"), &said("Mail")).as_deref(),
            Some("Mail & more")
        );
        let long = said(&"word ".repeat(40));
        let cut = shown(&module("{}"), &long).unwrap();
        assert_eq!(cut.chars().count(), DEFAULT_MAX_LENGTH);
        assert!(cut.ends_with("w…"), "{cut}");
        assert_eq!(shorten("ab cd", 4), "ab…", "no space before the ellipsis");
        let five = Module {
            max_length: Some(5),
            ..module("{}")
        };
        assert_eq!(shown(&five, &said("abcdefgh")).as_deref(), Some("abcd…"));
        let huge = Module {
            max_length: Some(100_000),
            ..module("{}")
        };
        assert_eq!(
            shown(&huge, &said(&"x".repeat(1000)))
                .unwrap()
                .chars()
                .count(),
            LONGEST
        );
    }

    #[test]
    fn a_module_is_hidden_while_its_command_says_nothing() {
        assert_eq!(shown(&module("Mail: {}"), &said("")), None);
        assert_eq!(shown(&module("{}"), &said("  ")), None);
        assert_eq!(shown(&module("{}"), &said("<b></b>")), None);
        // A button with no command shows its format as it is.
        let button = Module {
            format: Some("Power".into()),
            ..Module::default()
        };
        assert_eq!(shown(&button, &Output::default()).unwrap().text, "Power");
    }

    #[test]
    fn a_tooltip_is_plain_lines_unless_switched_off() {
        let output = Output {
            text: "3".into(),
            tooltip: Some("<i>From</i> Ada\nFrom Bo\n".into()),
            tint: Tint::Warning,
            ..Output::default()
        };
        let on = shown(&module("{}"), &output).unwrap();
        assert_eq!(on.tooltip.as_deref(), Some("From Ada\nFrom Bo"));
        assert_eq!(on.tint, Tint::Warning);
        let off = Module {
            tooltip: false,
            ..module("{}")
        };
        assert_eq!(shown(&off, &output).unwrap().tooltip, None);
        let tall = Output {
            text: "x".into(),
            tooltip: Some("line\n".repeat(100)),
            ..Output::default()
        };
        let lines = shown(&module("{}"), &tall).unwrap().tooltip.unwrap();
        assert_eq!(lines.lines().count(), TOOLTIP_LINES);
    }

    #[test]
    fn escaped_json_output_shows_its_characters_and_no_entities() {
        let output = Output::json(
            r#"{"text": "1 idle &amp; 1 &lt;waiting&gt;", "tooltip": "codex: Fish &amp; chips (idle)\nclaude: a &lt;b&gt; c", "class": "warning"}"#,
        )
        .unwrap();
        let shown = shown(&module("{}"), &output).unwrap();
        assert_eq!(shown.text, "1 idle & 1 <waiting>");
        assert_eq!(
            shown.tooltip.as_deref(),
            Some("codex: Fish & chips (idle)\nclaude: a <b> c")
        );
    }
}

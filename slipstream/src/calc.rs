//! Answers typed into the app explorer: arithmetic (`12*7`, `2^10`, `sqrt(2)`, `15% of 80`) and
//! unit conversions (`5 km in miles`, `100f to c`, `3.5 GiB in MB`).
//!
//! A query only counts as a sum if it has an operator or a function in it and every character is
//! accounted for, so an app's name or a bare number never turns into an answer.

/// An answer: what was understood, and the value to copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The answer as the row shows it, e.g. `12 × 7 = 84` or `5 km = 3.10686 mi`.
    pub shown: String,
    /// Just the value, as it's copied.
    pub value: String,
}

/// The answer to `query`, if it is a sum or a conversion.
pub fn answer(query: &str) -> Option<Answer> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    convert(query).or_else(|| calculate(query))
}

fn calculate(query: &str) -> Option<Answer> {
    let expression = query.strip_suffix('=').unwrap_or(query).trim();
    let tokens = tokenize(expression)?;
    let operated = tokens.iter().any(|token| {
        matches!(
            token,
            Token::Op(_) | Token::Function(_) | Token::Percent | Token::Of
        )
    }) && tokens.len() > 1;
    if !operated {
        return None;
    }
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.expression()?;
    if parser.at != parser.tokens.len() || !value.is_finite() {
        return None;
    }
    let value = format_number(value);
    Some(Answer {
        shown: format!("{} = {value}", pretty(expression)),
        value,
    })
}

/// The expression as the row shows it: `*` and `/` as × and ÷.
fn pretty(expression: &str) -> String {
    expression
        .replace('*', " × ")
        .replace('/', " ÷ ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A number with up to ten significant digits, no trailing zeros, and no exponent until it's
/// very large or very small.
pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        return format!("{value:.6e}");
    }
    let digits = (10 - magnitude.log10().floor() as i32 - 1).clamp(0, 12) as usize;
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    if text == "-0" { "0".into() } else { text }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Op(char),
    Open,
    Close,
    Function(Function),
    /// `%` after a number: a hundredth.
    Percent,
    /// `of`, as in `15% of 80`.
    Of,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Function {
    Sqrt,
    Sin,
    Cos,
    Tan,
    Ln,
    Log,
    Abs,
    Round,
    Floor,
    Ceil,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | ',' | '_' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                // Thousands separators inside a number (`1,000`, `1_000`) are part of it.
                let separator = |at: usize| {
                    matches!(chars.get(at), Some(',' | '_'))
                        && chars.get(at + 1).is_some_and(char::is_ascii_digit)
                };
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || chars[i] == '.' || separator(i))
                {
                    i += 1;
                }
                let number: String = chars[start..i]
                    .iter()
                    .filter(|c| !matches!(c, ',' | '_'))
                    .collect();
                tokens.push(Token::Number(number.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '^' => {
                tokens.push(Token::Op(c));
                i += 1;
            }
            '×' | 'x' | 'X' if matches!(tokens.last(), Some(Token::Number(_) | Token::Close)) => {
                tokens.push(Token::Op('*'));
                i += 1;
            }
            '÷' => {
                tokens.push(Token::Op('/'));
                i += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            'π' => {
                tokens.push(Token::Number(std::f64::consts::PI));
                i += 1;
            }
            c if c.is_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_alphanumeric() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
                tokens.push(match word.as_str() {
                    "pi" => Token::Number(std::f64::consts::PI),
                    "e" => Token::Number(std::f64::consts::E),
                    "of" => Token::Of,
                    "sqrt" => Token::Function(Function::Sqrt),
                    "sin" => Token::Function(Function::Sin),
                    "cos" => Token::Function(Function::Cos),
                    "tan" => Token::Function(Function::Tan),
                    "ln" => Token::Function(Function::Ln),
                    "log" => Token::Function(Function::Log),
                    "abs" => Token::Function(Function::Abs),
                    "round" => Token::Function(Function::Round),
                    "floor" => Token::Function(Function::Floor),
                    "ceil" => Token::Function(Function::Ceil),
                    _ => return None,
                });
            }
            _ => return None,
        }
    }
    Some(tokens)
}

/// Recursive descent: `expression := term (('+'|'-') term)*`, `term := power (('*'|'/'|'of')
/// power)*`, `power := unary ('^' power)?`, `unary := '-' unary | postfix`, `postfix := atom '%'?`.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.at += 1;
            // `80 + 15%` adds fifteen percent of 80, as a calculator's percent key does.
            let percent = self.is_percent_next();
            let rhs = self.term()?;
            let rhs = if percent { value * rhs } else { rhs };
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    /// Whether the next term is a bare percentage: a number followed by `%` and nothing binding
    /// tighter.
    fn is_percent_next(&self) -> bool {
        matches!(
            (
                self.tokens.get(self.at),
                self.tokens.get(self.at + 1),
                self.tokens.get(self.at + 2)
            ),
            (
                Some(Token::Number(_)),
                Some(Token::Percent),
                None | Some(Token::Op('+' | '-')) | Some(Token::Close)
            )
        )
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        loop {
            match self.peek() {
                Some(Token::Op('*')) | Some(Token::Of) => {
                    self.at += 1;
                    value *= self.power()?;
                }
                Some(Token::Op('/')) => {
                    self.at += 1;
                    value /= self.power()?;
                }
                // Implied multiplication: `2(3+4)`, `2pi`, `3 sqrt 4`.
                Some(Token::Open) | Some(Token::Function(_)) | Some(Token::Number(_)) => {
                    value *= self.power()?;
                }
                _ => return Some(value),
            }
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.at += 1;
            let exponent = self.power()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Token::Op('-')) => {
                self.at += 1;
                Some(-self.unary()?)
            }
            Some(Token::Op('+')) => {
                self.at += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Option<f64> {
        let mut value = self.atom()?;
        while let Some(Token::Percent) = self.peek() {
            self.at += 1;
            value /= 100.0;
        }
        Some(value)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.peek().cloned()? {
            Token::Number(value) => {
                self.at += 1;
                Some(value)
            }
            Token::Open => {
                self.at += 1;
                let value = self.expression()?;
                if self.peek() == Some(&Token::Close) {
                    self.at += 1;
                }
                Some(value)
            }
            Token::Function(function) => {
                self.at += 1;
                let x = self.power()?;
                Some(match function {
                    Function::Sqrt => x.sqrt(),
                    Function::Sin => x.to_radians().sin(),
                    Function::Cos => x.to_radians().cos(),
                    Function::Tan => x.to_radians().tan(),
                    Function::Ln => x.ln(),
                    Function::Log => x.log10(),
                    Function::Abs => x.abs(),
                    Function::Round => x.round(),
                    Function::Floor => x.floor(),
                    Function::Ceil => x.ceil(),
                })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Length,
    Mass,
    Temperature,
    Volume,
    Data,
    Time,
    Speed,
    Area,
}

/// A unit: its names, what it measures, how many base units one is, and the symbol shown.
struct Unit {
    names: &'static [&'static str],
    kind: Kind,
    factor: f64,
    symbol: &'static str,
}

const UNITS: &[Unit] = &[
    Unit {
        names: &["m", "metre", "metres", "meter", "meters"],
        kind: Kind::Length,
        factor: 1.0,
        symbol: "m",
    },
    Unit {
        names: &["km", "kilometre", "kilometres", "kilometer", "kilometers"],
        kind: Kind::Length,
        factor: 1000.0,
        symbol: "km",
    },
    Unit {
        names: &[
            "cm",
            "centimetre",
            "centimetres",
            "centimeter",
            "centimeters",
        ],
        kind: Kind::Length,
        factor: 0.01,
        symbol: "cm",
    },
    Unit {
        names: &[
            "mm",
            "millimetre",
            "millimetres",
            "millimeter",
            "millimeters",
        ],
        kind: Kind::Length,
        factor: 0.001,
        symbol: "mm",
    },
    Unit {
        names: &["mi", "mile", "miles"],
        kind: Kind::Length,
        factor: 1609.344,
        symbol: "mi",
    },
    Unit {
        names: &["yd", "yard", "yards"],
        kind: Kind::Length,
        factor: 0.9144,
        symbol: "yd",
    },
    Unit {
        names: &["ft", "foot", "feet"],
        kind: Kind::Length,
        factor: 0.3048,
        symbol: "ft",
    },
    Unit {
        names: &["in", "inch", "inches"],
        kind: Kind::Length,
        factor: 0.0254,
        symbol: "in",
    },
    Unit {
        names: &["nmi", "nautical mile", "nautical miles"],
        kind: Kind::Length,
        factor: 1852.0,
        symbol: "nmi",
    },
    Unit {
        names: &["kg", "kilo", "kilos", "kilogram", "kilograms"],
        kind: Kind::Mass,
        factor: 1.0,
        symbol: "kg",
    },
    Unit {
        names: &["g", "gram", "grams"],
        kind: Kind::Mass,
        factor: 0.001,
        symbol: "g",
    },
    Unit {
        names: &["mg", "milligram", "milligrams"],
        kind: Kind::Mass,
        factor: 1e-6,
        symbol: "mg",
    },
    Unit {
        names: &["t", "tonne", "tonnes"],
        kind: Kind::Mass,
        factor: 1000.0,
        symbol: "t",
    },
    Unit {
        names: &["lb", "lbs", "pound", "pounds"],
        kind: Kind::Mass,
        factor: 0.453_592_37,
        symbol: "lb",
    },
    Unit {
        names: &["oz", "ounce", "ounces"],
        kind: Kind::Mass,
        factor: 0.028_349_523_125,
        symbol: "oz",
    },
    Unit {
        names: &["st", "stone", "stones"],
        kind: Kind::Mass,
        factor: 6.350_293_18,
        symbol: "st",
    },
    Unit {
        names: &["c", "°c", "celsius", "centigrade"],
        kind: Kind::Temperature,
        factor: 0.0,
        symbol: "°C",
    },
    Unit {
        names: &["f", "°f", "fahrenheit"],
        kind: Kind::Temperature,
        factor: 1.0,
        symbol: "°F",
    },
    Unit {
        names: &["k", "kelvin"],
        kind: Kind::Temperature,
        factor: 2.0,
        symbol: "K",
    },
    Unit {
        names: &["l", "litre", "litres", "liter", "liters"],
        kind: Kind::Volume,
        factor: 1.0,
        symbol: "L",
    },
    Unit {
        names: &[
            "ml",
            "millilitre",
            "millilitres",
            "milliliter",
            "milliliters",
        ],
        kind: Kind::Volume,
        factor: 0.001,
        symbol: "mL",
    },
    Unit {
        names: &["pint", "pints", "pt"],
        kind: Kind::Volume,
        factor: 0.568_261_25,
        symbol: "pt",
    },
    Unit {
        names: &["gal", "gallon", "gallons"],
        kind: Kind::Volume,
        factor: 4.546_09,
        symbol: "gal",
    },
    Unit {
        names: &["us gal", "us gallon", "us gallons"],
        kind: Kind::Volume,
        factor: 3.785_411_784,
        symbol: "US gal",
    },
    Unit {
        names: &["cup", "cups"],
        kind: Kind::Volume,
        factor: 0.25,
        symbol: "cups",
    },
    Unit {
        names: &["b", "byte", "bytes"],
        kind: Kind::Data,
        factor: 1.0,
        symbol: "B",
    },
    Unit {
        names: &["kb", "kilobyte", "kilobytes"],
        kind: Kind::Data,
        factor: 1e3,
        symbol: "kB",
    },
    Unit {
        names: &["mb", "megabyte", "megabytes"],
        kind: Kind::Data,
        factor: 1e6,
        symbol: "MB",
    },
    Unit {
        names: &["gb", "gigabyte", "gigabytes"],
        kind: Kind::Data,
        factor: 1e9,
        symbol: "GB",
    },
    Unit {
        names: &["tb", "terabyte", "terabytes"],
        kind: Kind::Data,
        factor: 1e12,
        symbol: "TB",
    },
    Unit {
        names: &["kib"],
        kind: Kind::Data,
        factor: 1024.0,
        symbol: "KiB",
    },
    Unit {
        names: &["mib"],
        kind: Kind::Data,
        factor: 1_048_576.0,
        symbol: "MiB",
    },
    Unit {
        names: &["gib"],
        kind: Kind::Data,
        factor: 1_073_741_824.0,
        symbol: "GiB",
    },
    Unit {
        names: &["tib"],
        kind: Kind::Data,
        factor: 1_099_511_627_776.0,
        symbol: "TiB",
    },
    Unit {
        names: &["s", "sec", "secs", "second", "seconds"],
        kind: Kind::Time,
        factor: 1.0,
        symbol: "s",
    },
    Unit {
        names: &["min", "mins", "minute", "minutes"],
        kind: Kind::Time,
        factor: 60.0,
        symbol: "min",
    },
    Unit {
        names: &["h", "hr", "hrs", "hour", "hours"],
        kind: Kind::Time,
        factor: 3600.0,
        symbol: "h",
    },
    Unit {
        names: &["day", "days"],
        kind: Kind::Time,
        factor: 86_400.0,
        symbol: "days",
    },
    Unit {
        names: &["week", "weeks"],
        kind: Kind::Time,
        factor: 604_800.0,
        symbol: "weeks",
    },
    Unit {
        names: &["m/s", "mps"],
        kind: Kind::Speed,
        factor: 1.0,
        symbol: "m/s",
    },
    Unit {
        names: &["km/h", "kmh", "kph"],
        kind: Kind::Speed,
        factor: 1.0 / 3.6,
        symbol: "km/h",
    },
    Unit {
        names: &["mph"],
        kind: Kind::Speed,
        factor: 0.447_04,
        symbol: "mph",
    },
    Unit {
        names: &["knot", "knots", "kn"],
        kind: Kind::Speed,
        factor: 1852.0 / 3600.0,
        symbol: "kn",
    },
    Unit {
        names: &["m2", "m²", "square metre", "square metres", "square meters"],
        kind: Kind::Area,
        factor: 1.0,
        symbol: "m²",
    },
    Unit {
        names: &["ha", "hectare", "hectares"],
        kind: Kind::Area,
        factor: 10_000.0,
        symbol: "ha",
    },
    Unit {
        names: &["acre", "acres"],
        kind: Kind::Area,
        factor: 4_046.856_422_4,
        symbol: "acres",
    },
    Unit {
        names: &["ft2", "ft²", "sq ft", "square foot", "square feet"],
        kind: Kind::Area,
        factor: 0.092_903_04,
        symbol: "ft²",
    },
];

fn unit(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();
    UNITS
        .iter()
        .find(|unit| unit.names.contains(&name.as_str()))
}

fn to_base(value: f64, unit: &Unit) -> f64 {
    match (unit.kind, unit.symbol) {
        (Kind::Temperature, "°C") => value + 273.15,
        (Kind::Temperature, "°F") => (value - 32.0) * 5.0 / 9.0 + 273.15,
        (Kind::Temperature, _) => value,
        _ => value * unit.factor,
    }
}

fn from_base(value: f64, unit: &Unit) -> f64 {
    match (unit.kind, unit.symbol) {
        (Kind::Temperature, "°C") => value - 273.15,
        (Kind::Temperature, "°F") => (value - 273.15) * 9.0 / 5.0 + 32.0,
        (Kind::Temperature, _) => value,
        _ => value / unit.factor,
    }
}

/// `5 km in miles`, `100f to c`, `3.5 GiB as MB`.
fn convert(query: &str) -> Option<Answer> {
    let lower = query.to_lowercase();
    // The last separator, so `5 in in cm` reads as inches into centimetres.
    let (left, right) = [" in ", " to ", " as ", " into ", " = "]
        .iter()
        .filter_map(|separator| lower.rfind(separator).map(|at| (at, separator.len())))
        .max_by_key(|(at, _)| *at)
        .map(|(at, len)| (&lower[..at], &lower[at + len..]))?;
    let left = left.trim();
    let split = left
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-' || *c == ',' || *c == ' '))
        .map(|(at, _)| at)?;
    let number = left[..split].replace([',', ' '], "");
    let value: f64 = number.parse().ok()?;
    let from = unit(&left[split..])?;
    let to = unit(right)?;
    if from.kind != to.kind || std::ptr::eq(from, to) {
        return None;
    }
    let converted = from_base(to_base(value, from), to);
    let shown_value = format_number(converted);
    Some(Answer {
        shown: format!(
            "{} {} = {shown_value} {}",
            format_number(value),
            from.symbol,
            to.symbol
        ),
        value: shown_value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(query: &str) -> Option<String> {
        answer(query).map(|answer| answer.value)
    }

    #[test]
    fn sums_are_worked_out() {
        assert_eq!(value("12*7").as_deref(), Some("84"));
        assert_eq!(value("2^10").as_deref(), Some("1024"));
        assert_eq!(value("(1 + 2) * 3").as_deref(), Some("9"));
        assert_eq!(value("10 / 4").as_deref(), Some("2.5"));
        assert_eq!(value("-3 + 5").as_deref(), Some("2"));
        assert_eq!(value("2(3+4)").as_deref(), Some("14"));
        assert_eq!(value("sqrt 16").as_deref(), Some("4"));
        assert_eq!(value("3 × 4").as_deref(), Some("12"));
        assert_eq!(value("1,000 * 3").as_deref(), Some("3000"));
    }

    #[test]
    fn percentages_read_as_people_say_them() {
        assert_eq!(value("15% of 80").as_deref(), Some("12"));
        assert_eq!(value("80 + 15%").as_deref(), Some("92"));
        assert_eq!(value("80 - 25%").as_deref(), Some("60"));
        assert_eq!(value("50%*4").as_deref(), Some("2"));
    }

    #[test]
    fn names_and_bare_numbers_are_not_sums() {
        assert_eq!(answer("firefox"), None);
        assert_eq!(answer("42"), None);
        assert_eq!(answer("2048"), None);
        assert_eq!(answer("htop -d 5"), None);
        assert_eq!(answer("1/0"), None);
        assert_eq!(answer("x11"), None);
    }

    #[test]
    fn units_convert_within_a_kind() {
        assert_eq!(value("5 km in miles").as_deref(), Some("3.106855961"));
        assert_eq!(value("100f to c").as_deref(), Some("37.77777778"));
        assert_eq!(value("0 c in f").as_deref(), Some("32"));
        assert_eq!(value("1 GiB in MB").as_deref(), Some("1073.741824"));
        assert_eq!(value("5 in in cm").as_deref(), Some("12.7"));
        assert_eq!(value("70 kg in st").as_deref(), Some("11.02311311"));
        assert_eq!(answer("5 km in kg"), None);
    }

    #[test]
    fn a_conversion_shows_what_it_understood() {
        assert_eq!(
            answer("2 pints in ml").unwrap().shown,
            "2 pt = 1136.5225 mL"
        );
    }

    #[test]
    fn numbers_are_tidy() {
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(1.0 / 3.0), "0.3333333333");
        assert_eq!(format_number(123456789.0), "123456789");
        assert_eq!(format_number(-0.0), "0");
    }
}

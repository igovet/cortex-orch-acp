//! Python `json.loads` and `json.dumps(sort_keys=True, ensure_ascii=False,
//! separators=(",", ":"))` for owner files (the oracle is CPython 3.12 `_json`).
//!
//! Unlike serde_json this accepts exactly what Python accepts: `NaN`,
//! `Infinity`/`-Infinity`, integers of any size up to Python's 4300-digit
//! conversion limit (kept as exact decimal text), duplicate keys (the last value
//! at the first position) and lone UTF-16 surrogate escapes. Python decodes a
//! lone surrogate but cannot encode it as UTF-8, so such a document is readable
//! but never rewritten. Parsing, serialization and drop are iterative, so deep
//! owner data cannot exhaust a thread stack.
use std::collections::HashMap;

/// CPython's default `int` string-conversion limit (`sys.int_info`).
const MAX_INT_DIGITS: usize = 4300;
/// The container nesting CPython 3.12.13 decodes before `RecursionError`
/// (observed, `json.loads("[" * n + "]" * n)`); deeper documents refuse.
pub(crate) const MAX_DEPTH: usize = 57_956;

/// A decoded Python JSON value.
#[derive(Debug)]
pub(crate) enum Py {
    Null,
    Bool(bool),
    /// Exact decimal text of a Python `int`.
    Int(String),
    Float(f64),
    Str(String),
    Array(Vec<Py>),
    /// Python dict order: first-insertion position, last value.
    Object(Vec<(String, Py)>),
}

impl Py {
    pub(crate) fn get(&self, key: &str) -> Option<&Py> {
        match self {
            Py::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Python `value == 1` (`True == 1 == 1.0`).
    pub(crate) fn equals_one(&self) -> bool {
        match self {
            Py::Int(text) => text == "1",
            Py::Bool(value) => *value,
            Py::Float(value) => *value == 1.0,
            _ => false,
        }
    }
}

impl Drop for Py {
    fn drop(&mut self) {
        fn take(value: &mut Py, out: &mut Vec<Py>) {
            match value {
                Py::Array(items) => out.append(items),
                Py::Object(entries) => out.extend(entries.drain(..).map(|(_, v)| v)),
                _ => {}
            }
        }
        let mut pending = Vec::new();
        take(self, &mut pending);
        while let Some(mut item) = pending.pop() {
            take(&mut item, &mut pending);
        }
    }
}

/// A decoded document; `encodable` is false when Python could not re-encode it.
pub(crate) struct Document {
    pub value: Py,
    pub encodable: bool,
}

/// Python `json.loads(bytes.decode("utf-8"))`; `None` where Python raises.
pub(crate) fn loads(bytes: &[u8]) -> Option<Document> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.starts_with('\u{feff}') {
        return None; // "Unexpected UTF-8 BOM"
    }
    let mut parser = Parser {
        text,
        at: 0,
        encodable: true,
    };
    parser.whitespace();
    let value = parser.document()?;
    parser.whitespace();
    (parser.at == text.len()).then_some(Document {
        value,
        encodable: parser.encodable,
    })
}

enum Frame {
    Array(Vec<Py>),
    Object(Vec<(String, Py)>, HashMap<String, usize>, String),
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    encodable: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }
    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }
    fn literal(&mut self, word: &str, value: Py) -> Option<Py> {
        self.text[self.at..].starts_with(word).then(|| {
            self.at += word.len();
            value
        })
    }
    /// An object key, its colon and the whitespace before its value.
    fn key(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let key = self.string()?;
        self.whitespace();
        if !self.eat(b':') {
            return None;
        }
        self.whitespace();
        Some(key)
    }
    fn document(&mut self) -> Option<Py> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let mut current = match self.peek()? {
                open @ (b'[' | b'{') => {
                    if stack.len() + 1 > MAX_DEPTH {
                        return None;
                    }
                    self.at += 1;
                    self.whitespace();
                    if open == b'[' {
                        if self.eat(b']') {
                            Py::Array(Vec::new())
                        } else {
                            stack.push(Frame::Array(Vec::new()));
                            continue;
                        }
                    } else if self.eat(b'}') {
                        Py::Object(Vec::new())
                    } else {
                        let key = self.key()?;
                        stack.push(Frame::Object(Vec::new(), HashMap::new(), key));
                        continue;
                    }
                }
                _ => self.scalar()?,
            };
            loop {
                self.whitespace();
                match stack.last_mut() {
                    None => return Some(current),
                    Some(Frame::Array(items)) => {
                        items.push(current);
                        if self.eat(b',') {
                            self.whitespace();
                            break;
                        }
                        if !self.eat(b']') {
                            return None;
                        }
                        let Some(Frame::Array(items)) = stack.pop() else {
                            unreachable!()
                        };
                        current = Py::Array(items);
                    }
                    Some(Frame::Object(entries, index, pending)) => {
                        let key = std::mem::take(pending);
                        match index.get(&key) {
                            Some(&position) => entries[position].1 = current,
                            None => {
                                index.insert(key.clone(), entries.len());
                                entries.push((key, current));
                            }
                        }
                        if self.eat(b',') {
                            self.whitespace();
                            *pending = self.key()?;
                            break;
                        }
                        if !self.eat(b'}') {
                            return None;
                        }
                        let Some(Frame::Object(entries, _, _)) = stack.pop() else {
                            unreachable!()
                        };
                        current = Py::Object(entries);
                    }
                }
            }
        }
    }
    fn scalar(&mut self) -> Option<Py> {
        match self.peek()? {
            b'"' => {
                self.at += 1;
                self.string().map(Py::Str)
            }
            b'n' => self.literal("null", Py::Null),
            b't' => self.literal("true", Py::Bool(true)),
            b'f' => self.literal("false", Py::Bool(false)),
            b'N' => self.literal("NaN", Py::Float(f64::NAN)),
            b'I' => self.literal("Infinity", Py::Float(f64::INFINITY)),
            b'-' if self.text[self.at..].starts_with("-Infinity") => {
                self.literal("-Infinity", Py::Float(f64::NEG_INFINITY))
            }
            _ => self.number(),
        }
    }
    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        self.at - start
    }
    fn number(&mut self) -> Option<Py> {
        let start = self.at;
        self.eat(b'-');
        match self.peek()? {
            b'0' => self.at += 1,
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        let mut float = false;
        let bytes = self.text.as_bytes();
        if self.peek() == Some(b'.') && bytes.get(self.at + 1).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
            self.digits();
            float = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let exponent = self.at;
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                self.at = exponent; // Python backtracks to the mantissa
            } else {
                float = true;
            }
        }
        let text = &self.text[start..self.at];
        if float {
            return text.parse::<f64>().ok().map(Py::Float);
        }
        if text.trim_start_matches('-').len() > MAX_INT_DIGITS {
            return None; // Python: "Exceeds the limit (4300 digits)"
        }
        Some(Py::Int(if text == "-0" { "0" } else { text }.to_owned()))
    }
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.text.get(self.at..self.at + 4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.at += 4;
        u32::from_str_radix(digits, 16).ok()
    }
    /// A string body after its opening quote (strict: no raw control characters).
    fn string(&mut self) -> Option<String> {
        let mut out = String::new();
        loop {
            let start = self.at;
            while self
                .peek()
                .is_some_and(|b| b != b'"' && b != b'\\' && b >= 0x20)
            {
                self.at += 1;
            }
            out.push_str(&self.text[start..self.at]);
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(out);
                }
                b'\\' => self.at += 1,
                _ => return None, // "Invalid control character"
            }
            let escape = self.peek()?;
            self.at += 1;
            match escape {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let mut code = self.hex4()?;
                    if (0xD800..=0xDBFF).contains(&code) && self.text[self.at..].starts_with("\\u")
                    {
                        let resume = self.at;
                        self.at += 2;
                        let low = self.hex4()?;
                        if (0xDC00..=0xDFFF).contains(&low) {
                            code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        } else {
                            self.at = resume;
                        }
                    }
                    match char::from_u32(code) {
                        Some(c) => out.push(c),
                        None => {
                            // A lone surrogate: Python keeps it but cannot encode it.
                            self.encodable = false;
                            out.push('\u{fffd}');
                        }
                    }
                }
                _ => return None,
            }
        }
    }
}

/// Python `float.__repr__`, plus `json.dumps`' non-finite spellings.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    // Rust's `{:e}` is the shortest round-trip digit string, like Python's repr.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = exponent.parse::<i32>().unwrap() + 1;
    let count = digits.len() as i32;
    let body = if -4 < point && point <= 16 {
        if point <= 0 {
            format!("0.{}{digits}", "0".repeat(-point as usize))
        } else if point >= count {
            format!("{digits}{}.0", "0".repeat((point - count) as usize))
        } else {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        }
    } else {
        let mut mantissa = digits[..1].to_owned();
        if count > 1 {
            mantissa.push('.');
            mantissa.push_str(&digits[1..]);
        }
        let exponent = point - 1;
        format!(
            "{mantissa}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    };
    format!("{sign}{body}")
}

/// Python's quoting with `ensure_ascii=False` (only `"`, `\` and controls escape).
pub(crate) fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

/// Python `json.dumps(value, sort_keys=True, ensure_ascii=False,
/// separators=(",", ":"))`. A caller must refuse an unencodable document.
pub(crate) fn dumps(value: &Py, out: &mut String) {
    enum Step<'a> {
        Value(&'a Py),
        Text(&'static str),
        Key(&'a str),
    }
    let mut stack = vec![Step::Value(value)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Text(text) => out.push_str(text),
            Step::Key(key) => {
                out.push_str(&quote(key));
                out.push(':');
            }
            Step::Value(Py::Null) => out.push_str("null"),
            Step::Value(Py::Bool(true)) => out.push_str("true"),
            Step::Value(Py::Bool(false)) => out.push_str("false"),
            Step::Value(Py::Int(text)) => out.push_str(text),
            Step::Value(Py::Float(value)) => out.push_str(&float_repr(*value)),
            Step::Value(Py::Str(text)) => out.push_str(&quote(text)),
            Step::Value(Py::Array(items)) => {
                out.push('[');
                stack.push(Step::Text("]"));
                for (index, item) in items.iter().enumerate().rev() {
                    stack.push(Step::Value(item));
                    if index > 0 {
                        stack.push(Step::Text(","));
                    }
                }
            }
            Step::Value(Py::Object(entries)) => {
                out.push('{');
                stack.push(Step::Text("}"));
                let mut sorted: Vec<&(String, Py)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (index, (key, item)) in sorted.into_iter().enumerate().rev() {
                    stack.push(Step::Value(item));
                    stack.push(Step::Key(key));
                    if index > 0 {
                        stack.push(Step::Text(","));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str) -> Option<String> {
        let document = loads(text.as_bytes())?;
        let mut out = String::new();
        dumps(&document.value, &mut out);
        Some(out)
    }

    #[test]
    fn floats_spell_like_python_repr() {
        for (value, python) in [
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (1e-4, "0.0001"),
            (0.1, "0.1"),
            (-0.0, "-0.0"),
            (1.5e300, "1.5e+300"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (2.5, "2.5"),
            (100.0, "100.0"),
            (1e22, "1e+22"),
            (-12.75, "-12.75"),
        ] {
            assert_eq!(float_repr(value), python);
        }
    }

    #[test]
    fn python_only_shapes_decode_and_reencode() {
        assert_eq!(
            roundtrip("[1e400,-1e400,NaN,-0,10000000000000000000000000,1E2]").unwrap(),
            "[Infinity,-Infinity,NaN,0,10000000000000000000000000,100.0]"
        );
        // Duplicate keys keep the last value; output is sorted.
        assert_eq!(
            roundtrip(r#" {"b":1,"a":{"y":2,"x":3},"b":[true,null]} "#).unwrap(),
            r#"{"a":{"x":3,"y":2},"b":[true,null]}"#
        );
        assert_eq!(
            roundtrip(r#""\ud83d\ude00\u00e9\/\t""#).unwrap(),
            "\"\u{1f600}\u{e9}/\\t\""
        );
        let lone = loads(br#""\ud800\u0041\udc00x""#).unwrap();
        assert!(!lone.encodable);
        assert!(loads(b"[1]").unwrap().encodable);
        let big = "1".repeat(MAX_INT_DIGITS);
        assert!(loads(big.as_bytes()).is_some());
        assert!(loads(format!("-{big}").as_bytes()).is_some());
        assert!(loads(format!("{big}1").as_bytes()).is_none());
    }

    #[test]
    fn invalid_json_refuses_like_python() {
        for text in [
            "",
            " ",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "1e",
            "-",
            "nul",
            "nan",
            "[1 2]",
            "\"a\nb\"",
            "\"\\x\"",
            "\"\\u12\"",
            "\"\\ud800\\uzzzz\"",
            "{1:2}",
            "\u{feff}{}",
            "[]]",
            "{\"a\" 1}",
            "-NaN",
            "\"open",
        ] {
            assert!(loads(text.as_bytes()).is_none(), "{text:?}");
        }
        assert!(loads(b"\xff").is_none());
    }

    #[test]
    fn deep_nesting_is_iterative_and_bounded() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        let document = loads(deep.as_bytes()).unwrap();
        let mut out = String::new();
        dumps(&document.value, &mut out);
        assert_eq!(out, deep);
        let deeper = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert!(loads(deeper.as_bytes()).is_none());
    }
}

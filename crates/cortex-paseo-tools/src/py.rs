//! The CPython semantics the plugin's Python store readers rely on, so the
//! Rust readers print byte-identical JSON: `json.loads`, `json.dumps(...,
//! ensure_ascii=False)` with the default `", "`/`": "` separators and dict
//! insertion order, `str()`/`repr()`, truthiness, `==`, and the `str` methods
//! the readers call (`strip`, `split`, `isalpha`, `len`, slicing).
//!
//! The decoder follows `crates/cortex-acp/src/pyjson.rs` (CPython 3.12 `_json`):
//! `NaN`/`Infinity`, integers of any size (kept as exact decimal text),
//! duplicate keys (last value at the first position). Known differences: a
//! lone UTF-16 surrogate escape decodes to U+FFFD here, where Python keeps the
//! surrogate and would then fail to print it; and containers nested deeper
//! than `MAX_DEPTH` refuse as invalid JSON here, where CPython decodes them.
use std::collections::HashMap;

/// CPython's default `int` string-conversion limit (`sys.int_info`).
const MAX_INT_DIGITS: usize = 4300;
/// Nesting bound that keeps recursive drop/clone/dump within a thread stack.
const MAX_DEPTH: usize = 10_000;
/// `json.dumps` recursion bound (CPython's default recursion limit).
const MAX_DUMP_DEPTH: usize = 990;

/// A decoded Python value.
#[derive(Debug, Clone)]
pub enum Py {
    Null,
    Bool(bool),
    /// Exact decimal text of a Python `int`.
    Int(String),
    Float(f64),
    Str(String),
    List(Vec<Py>),
    /// Python dict order: first-insertion position, last value.
    Dict(Vec<(String, Py)>),
}

/// Python `None`, for `.get()` misses.
pub static NONE: Py = Py::Null;

/// An exception Python would raise and the reader would not catch.
#[derive(Debug)]
pub struct Crash;

impl Py {
    pub fn int(value: i64) -> Py {
        Py::Int(value.to_string())
    }
    pub fn str(value: impl Into<String>) -> Py {
        Py::Str(value.into())
    }
    pub fn dict() -> Py {
        Py::Dict(Vec::new())
    }
    pub fn is_dict(&self) -> bool {
        matches!(self, Py::Dict(_))
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Py::Null)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(s),
            _ => None,
        }
    }
    /// `dict.get(key)` with a present/absent distinction (`None` = absent).
    pub fn get_opt(&self, key: &str) -> Option<&Py> {
        match self {
            Py::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// `dict.get(key)`: Python `None` when absent. Callers check `is_dict`
    /// first where Python would raise on a non-dict.
    pub fn get(&self, key: &str) -> &Py {
        self.get_opt(key).unwrap_or(&NONE)
    }
    /// `dict.get(key)` that raises `AttributeError` on a non-dict, as Python does.
    pub fn get_or_crash(&self, key: &str) -> Result<&Py, Crash> {
        if self.is_dict() {
            Ok(self.get(key))
        } else {
            Err(Crash)
        }
    }
    pub fn contains(&self, key: &str) -> bool {
        self.get_opt(key).is_some()
    }
    /// `d[key] = value`: replaces in place, or appends.
    pub fn set(&mut self, key: &str, value: Py) {
        if let Py::Dict(entries) = self {
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key.to_owned(), value)),
            }
        }
    }
    /// `d.pop(key, None)` / `del d[key]`.
    pub fn remove(&mut self, key: &str) {
        if let Py::Dict(entries) = self {
            entries.retain(|(k, _)| k != key);
        }
    }
    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Py::Null => false,
            Py::Bool(b) => *b,
            Py::Int(text) => text != "0",
            Py::Float(f) => *f != 0.0,
            Py::Str(s) => !s.is_empty(),
            Py::List(items) => !items.is_empty(),
            Py::Dict(entries) => !entries.is_empty(),
        }
    }
    /// `isinstance(value, int)` (a `bool` is an `int`).
    pub fn is_int(&self) -> bool {
        matches!(self, Py::Int(_) | Py::Bool(_))
    }
    /// `isinstance(value, int) and not isinstance(value, bool)`.
    pub fn is_strict_int(&self) -> bool {
        matches!(self, Py::Int(_))
    }
    /// For an int-like value: `value >= 0`.
    pub fn int_non_negative(&self) -> bool {
        match self {
            Py::Int(text) => !text.starts_with('-'),
            Py::Bool(_) => true,
            _ => false,
        }
    }
    /// For an int-like value: `value > 0`.
    pub fn int_positive(&self) -> bool {
        match self {
            Py::Int(text) => !text.starts_with('-') && text != "0",
            Py::Bool(b) => *b,
            _ => false,
        }
    }
    /// `isinstance(value, (int, float)) and not isinstance(value, bool)`.
    pub fn is_number(&self) -> bool {
        matches!(self, Py::Int(_) | Py::Float(_))
    }
    /// `float(value)` for an int or float; `OverflowError` for a huge int.
    pub fn to_float(&self) -> Result<f64, Crash> {
        match self {
            Py::Float(f) => Ok(*f),
            Py::Int(text) => {
                let value: f64 = text.parse().map_err(|_| Crash)?;
                if value.is_finite() {
                    Ok(value)
                } else {
                    Err(Crash)
                }
            }
            Py::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            _ => Err(Crash),
        }
    }
    /// Python iteration of a value whose items must be strings
    /// (`[d for d in value if isinstance(d, str)]`): a list's string items,
    /// a dict's keys, a string's characters. `TypeError` otherwise.
    pub fn iter_strings(&self) -> Result<Vec<String>, Crash> {
        match self {
            Py::List(items) => Ok(items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()),
            Py::Dict(entries) => Ok(entries.iter().map(|(k, _)| k.clone()).collect()),
            Py::Str(s) => Ok(s.chars().map(String::from).collect()),
            _ => Err(Crash),
        }
    }
    /// Python iteration of a value (`for item in value`): a list's items, a
    /// dict's keys or a string's characters (both as strings). `TypeError`
    /// for any other value.
    pub fn iterate(&self) -> Result<Vec<Py>, Crash> {
        match self {
            Py::List(items) => Ok(items.clone()),
            Py::Dict(entries) => Ok(entries.iter().map(|(k, _)| Py::str(k.clone())).collect()),
            Py::Str(s) => Ok(s.chars().map(|c| Py::Str(c.to_string())).collect()),
            _ => Err(Crash),
        }
    }
}

/// Python `a == b`.
pub fn eq(a: &Py, b: &Py) -> bool {
    fn number(v: &Py) -> Option<NumberKey<'_>> {
        match v {
            Py::Bool(b) => Some(NumberKey::Int(if *b { "1" } else { "0" })),
            Py::Int(text) => Some(NumberKey::Int(text)),
            Py::Float(f) => Some(NumberKey::Float(*f)),
            _ => None,
        }
    }
    if let (Some(x), Some(y)) = (number(a), number(b)) {
        return x.equals(&y);
    }
    match (a, b) {
        (Py::Null, Py::Null) => true,
        (Py::Str(x), Py::Str(y)) => x == y,
        (Py::List(x), Py::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q)),
        (Py::Dict(x), Py::Dict(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, value)| {
                    y.iter()
                        .find(|(other, _)| other == key)
                        .is_some_and(|(_, other)| eq(value, other))
                })
        }
        _ => false,
    }
}

/// `a == "text"`.
pub fn eq_str(a: &Py, text: &str) -> bool {
    a.as_str() == Some(text)
}

enum NumberKey<'a> {
    Int(&'a str),
    Float(f64),
}

impl NumberKey<'_> {
    fn equals(&self, other: &NumberKey<'_>) -> bool {
        match (self, other) {
            (NumberKey::Int(x), NumberKey::Int(y)) => x == y,
            (NumberKey::Float(x), NumberKey::Float(y)) => x == y,
            (NumberKey::Int(i), NumberKey::Float(f)) | (NumberKey::Float(f), NumberKey::Int(i)) => {
                // Exact comparison: an integral float equals the int with the
                // same decimal value.
                if !f.is_finite() || f.fract() != 0.0 {
                    return false;
                }
                integral_float_text(*f) == *i
            }
        }
    }
}

/// The exact decimal text of an integral finite float.
fn integral_float_text(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    format!("{value:.0}")
}

/// Exact sum of two non-negative decimal integers.
pub fn add_decimal(a: &str, b: &str) -> String {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let (mut i, mut j, mut carry) = (a.len(), b.len(), 0u8);
    while i > 0 || j > 0 || carry > 0 {
        let x = if i > 0 {
            i -= 1;
            a[i] - b'0'
        } else {
            0
        };
        let y = if j > 0 {
            j -= 1;
            b[j] - b'0'
        } else {
            0
        };
        let sum = x + y + carry;
        out.push(b'0' + sum % 10);
        carry = sum / 10;
    }
    out.reverse();
    let text = String::from_utf8(out).unwrap_or_default();
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}

// --- str methods --------------------------------------------------------------

/// `str.isspace` for one character (includes the \x1c-\x1f separators).
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `' '.join(s.split())`.
pub fn normalize_spaces(s: &str) -> String {
    s.split(is_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `len(s)`: code points.
pub fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[:n]`.
pub fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((at, _)) => &s[..at],
        None => s,
    }
}

/// `value if len(value) <= limit else value[:limit - 1] + '…'`.
pub fn ellipsis(value: &str, limit: usize) -> String {
    if len(value) <= limit {
        value.to_owned()
    } else {
        format!("{}…", prefix(value, limit.saturating_sub(1)))
    }
}

/// `str.isalpha` for one character.
pub fn is_alpha(c: char) -> bool {
    c.is_alphabetic()
}

// --- json.loads -----------------------------------------------------------------

/// Python `json.loads(text)`; `None` where Python raises.
pub fn loads(text: &str) -> Option<Py> {
    if text.starts_with('\u{feff}') {
        return None; // "Unexpected UTF-8 BOM"
    }
    let mut parser = Parser { text, at: 0 };
    parser.whitespace();
    let value = parser.document()?;
    parser.whitespace();
    (parser.at == text.len()).then_some(value)
}

enum Frame {
    List(Vec<Py>),
    Dict(Vec<(String, Py)>, HashMap<String, usize>, String),
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
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
                            Py::List(Vec::new())
                        } else {
                            stack.push(Frame::List(Vec::new()));
                            continue;
                        }
                    } else if self.eat(b'}') {
                        Py::Dict(Vec::new())
                    } else {
                        let key = self.key()?;
                        stack.push(Frame::Dict(Vec::new(), HashMap::new(), key));
                        continue;
                    }
                }
                _ => self.scalar()?,
            };
            loop {
                self.whitespace();
                match stack.last_mut() {
                    None => return Some(current),
                    Some(Frame::List(items)) => {
                        items.push(current);
                        if self.eat(b',') {
                            self.whitespace();
                            break;
                        }
                        if !self.eat(b']') {
                            return None;
                        }
                        let Some(Frame::List(items)) = stack.pop() else {
                            return None;
                        };
                        current = Py::List(items);
                    }
                    Some(Frame::Dict(entries, index, pending)) => {
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
                        let Some(Frame::Dict(entries, _, _)) = stack.pop() else {
                            return None;
                        };
                        current = Py::Dict(entries);
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
            return None;
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
                _ => return None,
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
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                _ => return None,
            }
        }
    }
}

// --- json.dumps / repr ----------------------------------------------------------

/// Python `float.__repr__` digits, with `json.dumps`' or `repr`'s non-finite
/// spelling.
fn float_text(value: f64, json: bool) -> String {
    if value.is_nan() {
        return if json { "NaN" } else { "nan" }.into();
    }
    if value.is_infinite() {
        return match (value > 0.0, json) {
            (true, true) => "Infinity",
            (false, true) => "-Infinity",
            (true, false) => "inf",
            (false, false) => "-inf",
        }
        .into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    // Rust's `{:e}` is the shortest round-trip digit string, like Python's repr.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = exponent.parse::<i32>().unwrap_or(0) + 1;
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

/// `json.dumps` string quoting with `ensure_ascii=False`.
pub fn quote(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python `json.dumps(value, ensure_ascii=False)` (default separators).
pub fn dumps(value: &Py) -> Result<String, Crash> {
    let mut out = String::new();
    dump(value, &mut out, 0)?;
    Ok(out)
}

fn dump(value: &Py, out: &mut String, depth: usize) -> Result<(), Crash> {
    if depth > MAX_DUMP_DEPTH {
        return Err(Crash); // RecursionError
    }
    match value {
        Py::Null => out.push_str("null"),
        Py::Bool(true) => out.push_str("true"),
        Py::Bool(false) => out.push_str("false"),
        Py::Int(text) => out.push_str(text),
        Py::Float(f) => out.push_str(&float_text(*f, true)),
        Py::Str(s) => quote(s, out),
        Py::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                dump(item, out, depth + 1)?;
            }
            out.push(']');
        }
        Py::Dict(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                quote(key, out);
                out.push_str(": ");
                dump(item, out, depth + 1)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Python `str(value)`.
pub fn to_str(value: &Py) -> String {
    match value {
        Py::Str(s) => s.clone(),
        other => repr(other),
    }
}

/// Python `repr(value)`.
pub fn repr(value: &Py) -> String {
    let mut out = String::new();
    write_repr(value, &mut out, 0);
    out
}

fn write_repr(value: &Py, out: &mut String, depth: usize) {
    if depth > MAX_DUMP_DEPTH {
        out.push_str("...");
        return;
    }
    match value {
        Py::Null => out.push_str("None"),
        Py::Bool(true) => out.push_str("True"),
        Py::Bool(false) => out.push_str("False"),
        Py::Int(text) => out.push_str(text),
        Py::Float(f) => out.push_str(&float_text(*f, false)),
        Py::Str(s) => repr_str(s, out),
        Py::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_repr(item, out, depth + 1);
            }
            out.push(']');
        }
        Py::Dict(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                repr_str(key, out);
                out.push_str(": ");
                write_repr(item, out, depth + 1);
            }
            out.push('}');
        }
    }
}

/// `str.isprintable` for one character, approximated without the full
/// Unicode category tables: controls, format characters, separators other
/// than the ASCII space, surrogates and private-use characters are not printable.
fn printable(c: char) -> bool {
    let code = c as u32;
    if code < 0x20 || (0x7f..=0xa0).contains(&code) || code == 0xad {
        return false;
    }
    !matches!(code,
        0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x1680 | 0x180e
        | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064 | 0x2066..=0x206f
        | 0x3000 | 0xd800..=0xf8ff | 0xfeff | 0xfff9..=0xfffb
        | 0xe0001 | 0xe0020..=0xe007f | 0xf0000..=0x10ffff)
}

fn repr_str(s: &str, out: &mut String) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        match c {
            c if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if printable(c) => out.push(c),
            c if (c as u32) <= 0xff => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
}

/// Python `int(text)` for an argv value, bounded to SQLite's 64-bit range
/// (a larger Python int would overflow when bound).
pub fn parse_int(text: &str) -> Option<i64> {
    let text = strip(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let value: i128 = clean.parse().ok()?;
    let value = if negative { -value } else { value };
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str) -> String {
        dumps(&loads(text).unwrap()).unwrap()
    }

    #[test]
    fn dumps_like_python_default_separators_and_order() {
        assert_eq!(
            roundtrip(r#"{"b":1,"a":{"y":2.5,"x":[true,null]},"b":"é\u0001"}"#),
            "{\"b\": \"é\\u0001\", \"a\": {\"y\": 2.5, \"x\": [true, null]}}"
        );
        assert_eq!(
            roundtrip("[1e16,1E2,-0,NaN,1e400]"),
            "[1e+16, 100.0, 0, NaN, Infinity]"
        );
    }

    #[test]
    fn repr_and_str_follow_python() {
        let value = loads(r#"{"a":[1,2.0,null,true],"b":"it's"}"#).unwrap();
        assert_eq!(to_str(&value), "{'a': [1, 2.0, None, True], 'b': \"it's\"}");
        assert_eq!(to_str(&Py::Float(f64::NAN)), "nan");
        assert_eq!(
            repr(&Py::str("a\u{7f}\u{e9}\u{2028}")),
            "'a\\x7f\u{e9}\\u2028'"
        );
    }

    #[test]
    fn equality_crosses_numeric_types() {
        assert!(eq(&Py::int(1), &Py::Float(1.0)));
        assert!(eq(&Py::Bool(true), &Py::int(1)));
        assert!(!eq(&Py::int(1), &Py::str("1")));
        assert!(eq(
            &loads(r#"{"a":1,"b":2}"#).unwrap(),
            &loads(r#"{"b":2,"a":1.0}"#).unwrap()
        ));
    }

    #[test]
    fn integers_parse_like_python_int() {
        assert_eq!(parse_int(" +1_000 "), Some(1000));
        assert_eq!(parse_int("-5"), Some(-5));
        assert_eq!(parse_int("1__0"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("9223372036854775808"), None);
        assert_eq!(add_decimal("999", "1"), "1000");
        assert_eq!(add_decimal("0", "0"), "0");
    }

    #[test]
    fn string_helpers_count_code_points() {
        assert_eq!(ellipsis("абвгд", 3), "аб…");
        assert_eq!(ellipsis("abc", 3), "abc");
        assert_eq!(normalize_spaces("  a \u{1f} b\n"), "a b");
        assert_eq!(strip("\u{1c} x \u{3000}"), "x");
    }
}

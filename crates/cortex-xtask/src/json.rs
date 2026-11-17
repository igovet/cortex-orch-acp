//! Order-preserving JSON with Python `json.dumps` compatible serialization.
//!
//! The generated artifacts are byte-compared with output historically produced
//! by Python (`sort_keys`, `ensure_ascii`, `indent`, compact separators), so the
//! formatting rules are reproduced here instead of delegating to serde_json.

use std::fmt::Write as _;

pub type Res<T> = Result<T, String>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Raw numeric literal; only integers are supported on output.
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn obj() -> Value {
        Value::Obj(Vec::new())
    }

    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    pub fn int(n: i64) -> Value {
        Value::Num(n.to_string())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn req(&self, key: &str) -> Res<&Value> {
        self.get(key).ok_or_else(|| format!("missing key {key}"))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    /// Insert or replace a key, keeping the original position (Python dict semantics).
    pub fn set(&mut self, key: &str, value: Value) {
        if let Value::Obj(items) = self {
            if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                items.push((key.to_string(), value));
            }
        }
    }

    /// Python truthiness, as used by `spec.get("additionalProperties", True)`.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => n.parse::<f64>().map(|f| f != 0.0).unwrap_or(true),
            Value::Str(s) => !s.is_empty(),
            Value::Arr(a) => !a.is_empty(),
            Value::Obj(o) => !o.is_empty(),
        }
    }

    /// Replace `from` with `to` in every string and key (text-level equivalent of
    /// `json.loads(json.dumps(v).replace(from, to))` for ASCII `from`/`to`).
    pub fn replace_text(&self, from: &str, to: &str) -> Value {
        match self {
            Value::Str(s) => Value::Str(s.replace(from, to)),
            Value::Arr(a) => Value::Arr(a.iter().map(|v| v.replace_text(from, to)).collect()),
            Value::Obj(o) => {
                let mut out = Value::obj();
                for (k, v) in o {
                    out.set(&k.replace(from, to), v.replace_text(from, to));
                }
                out
            }
            other => other.clone(),
        }
    }
}

pub struct Format {
    pub sort_keys: bool,
    pub ensure_ascii: bool,
    pub indent: Option<usize>,
    /// Separators `(",", ":")` without indent; ignored when `indent` is set.
    pub compact: bool,
}

pub const CANONICAL: Format = Format { sort_keys: true, ensure_ascii: false, indent: None, compact: true };

pub fn dumps(value: &Value, format: &Format) -> Res<String> {
    let mut out = String::new();
    write_value(&mut out, value, format, 0)?;
    Ok(out)
}

/// `json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))`
pub fn canonical(value: &Value) -> Res<String> {
    dumps(value, &CANONICAL)
}

/// `json.dumps(value, ensure_ascii=False, indent=2) + "\n"`
pub fn pretty(value: &Value) -> Res<String> {
    let mut s = dumps(value, &Format { sort_keys: false, ensure_ascii: false, indent: Some(2), compact: false })?;
    s.push('\n');
    Ok(s)
}

/// `json.dumps(value, indent=2) + "\n"` (ASCII-escaping, insertion order).
pub fn pretty_ascii(value: &Value) -> Res<String> {
    let mut s = dumps(value, &Format { sort_keys: false, ensure_ascii: true, indent: Some(2), compact: false })?;
    s.push('\n');
    Ok(s)
}

pub fn quote(text: &str, ensure_ascii: bool) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && !(' '..='~').contains(&c) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{:04x}", unit);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_value(out: &mut String, value: &Value, f: &Format, depth: usize) -> Res<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Num(n) => {
            if n.contains(['.', 'e', 'E']) || n.parse::<i128>().is_err() {
                return Err(format!("unsupported non-integer number {n}"));
            }
            out.push_str(n);
        }
        Value::Str(s) => out.push_str(&quote(s, f.ensure_ascii)),
        Value::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                separator(out, f, depth + 1, i == 0);
                write_value(out, item, f, depth + 1)?;
            }
            close(out, f, depth, ']');
        }
        Value::Obj(items) => {
            if items.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            let mut ordered: Vec<&(String, Value)> = items.iter().collect();
            if f.sort_keys {
                // Python sorts str keys by code point, which equals UTF-8 byte order.
                ordered.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            }
            out.push('{');
            for (i, (key, item)) in ordered.into_iter().enumerate() {
                separator(out, f, depth + 1, i == 0);
                out.push_str(&quote(key, f.ensure_ascii));
                out.push_str(if f.indent.is_none() && f.compact { ":" } else { ": " });
                write_value(out, item, f, depth + 1)?;
            }
            close(out, f, depth, '}');
        }
    }
    Ok(())
}

fn separator(out: &mut String, f: &Format, depth: usize, first: bool) {
    match f.indent {
        Some(n) => {
            if !first {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(n * depth));
        }
        None => {
            if !first {
                out.push_str(if f.compact { "," } else { ", " });
            }
        }
    }
}

fn close(out: &mut String, f: &Format, depth: usize, ch: char) {
    if let Some(n) = f.indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * depth));
    }
    out.push(ch);
}

pub fn parse(text: &str) -> Res<Value> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err("trailing data in JSON".into());
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Res<Value> {
        if depth > 256 {
            return Err("JSON nesting too deep".into());
        }
        match self.s.get(self.i) {
            None => Err("unexpected end of JSON".into()),
            Some(b'{') => {
                self.i += 1;
                let mut obj = Value::obj();
                self.ws();
                if self.eat("}") {
                    return Ok(obj);
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return Err("expected ':' in JSON object".into());
                    }
                    self.ws();
                    let v = self.value(depth + 1)?;
                    obj.set(&key, v);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(obj);
                    }
                    return Err("expected ',' or '}' in JSON object".into());
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Value::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Value::Arr(items));
                    }
                    return Err("expected ',' or ']' in JSON array".into());
                }
            }
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len()
                    && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    self.i += 1;
                }
                Ok(Value::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
            Some(_) => Err("invalid JSON value".into()),
        }
    }

    fn hex4(&mut self) -> Res<u32> {
        let slice = self.s.get(self.i..self.i + 4).ok_or("short unicode escape")?;
        let text = std::str::from_utf8(slice).map_err(|_| "bad unicode escape")?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|_| "bad unicode escape".to_string())
    }

    fn string(&mut self) -> Res<String> {
        if !self.eat("\"") {
            return Err("expected JSON string".into());
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = *self.s.get(self.i).ok_or("unterminated JSON string")?;
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                if !self.eat("\\u") {
                                    return Err("lone surrogate in JSON string".into());
                                }
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return Err("lone surrogate in JSON string".into());
                                }
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            }
                            char::from_u32(code).ok_or("lone surrogate in JSON string")?
                        }
                        _ => return Err("bad JSON escape".into()),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return Err("control character in JSON string".into()),
                other => out.push(other),
            }
        }
        String::from_utf8(out).map_err(|_| "invalid UTF-8 in JSON string".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order_and_formats_like_python() {
        let v = parse(r#"{"b":[1,2],"a":{"x":"é\n","y":[]},"c":{}}"#).unwrap();
        assert_eq!(canonical(&v).unwrap(), "{\"a\":{\"x\":\"é\\n\",\"y\":[]},\"b\":[1,2],\"c\":{}}");
        assert_eq!(
            pretty(&v).unwrap(),
            "{\n  \"b\": [\n    1,\n    2\n  ],\n  \"a\": {\n    \"x\": \"é\\n\",\n    \"y\": []\n  },\n  \"c\": {}\n}\n"
        );
        assert_eq!(quote("é😀\u{7f}", true), "\"\\u00e9\\ud83d\\ude00\\u007f\"");
        assert_eq!(quote("\u{7f}\u{1}", false), "\"\u{7f}\\u0001\"");
    }

    #[test]
    fn duplicate_keys_keep_first_position_last_value() {
        let v = parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(canonical(&v).unwrap(), "{\"a\":3,\"b\":2}");
        assert_eq!(v.as_obj().unwrap()[0].0, "a");
    }

    #[test]
    fn rejects_floats_and_garbage() {
        assert!(dumps(&parse("1.5").unwrap(), &CANONICAL).is_err());
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
    }
}

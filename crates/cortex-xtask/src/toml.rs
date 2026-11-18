//! A deliberately small TOML reader for the manifests this checkout owns.
//!
//! It covers what `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` use: tables,
//! arrays of tables, dotted keys, basic/literal strings, integers, booleans, arrays
//! (also multi-line) and inline tables. Multi-line strings, dates and floats are
//! refused rather than guessed. The result reuses the order-preserving JSON value.

use crate::json::{Res, Value};

pub fn parse(text: &str) -> Res<Value> {
    let mut root = Value::obj();
    let mut path: Vec<String> = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((number, raw)) = lines.next() {
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = |message: &str| format!("TOML line {}: {message}", number + 1);
        if let Some(header) = line.strip_prefix("[[") {
            let name = header
                .strip_suffix("]]")
                .ok_or_else(|| at("unterminated table header"))?;
            path = split_key(name).map_err(|e| at(&e))?;
            let table = descend(&mut root, &path[..path.len() - 1], true).map_err(|e| at(&e))?;
            let last = &path[path.len() - 1];
            match table.get(last) {
                None => table.set(last, Value::Arr(vec![Value::obj()])),
                Some(Value::Arr(_)) => {
                    if let Some(Value::Arr(items)) = obj_slot(table, last) {
                        items.push(Value::obj());
                    }
                }
                Some(_) => return Err(at("array-of-tables name already used")),
            }
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let name = header
                .strip_suffix(']')
                .ok_or_else(|| at("unterminated table header"))?;
            path = split_key(name).map_err(|e| at(&e))?;
            descend(&mut root, &path, true).map_err(|e| at(&e))?;
            continue;
        }
        let (key, rest) = split_assignment(line).ok_or_else(|| at("expected key = value"))?;
        let mut value_text = rest.trim().to_string();
        // A multi-line array continues until its brackets balance.
        while depth(&value_text) > 0 {
            let (_, next) = lines.next().ok_or_else(|| at("unterminated array"))?;
            value_text.push(' ');
            value_text.push_str(strip_comment(next).trim());
        }
        let value = parse_value(&value_text).map_err(|e| at(&e))?;
        let keys = split_key(key).map_err(|e| at(&e))?;
        let table = descend(&mut root, &path, false).map_err(|e| at(&e))?;
        let table = descend(table, &keys[..keys.len() - 1], true).map_err(|e| at(&e))?;
        table.set(&keys[keys.len() - 1], value);
    }
    Ok(root)
}

fn obj_slot<'a>(table: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    match table {
        Value::Obj(items) => items.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// Walk (and with `create`, build) nested tables; an array of tables resolves to its last element.
fn descend<'a>(mut table: &'a mut Value, path: &[String], create: bool) -> Res<&'a mut Value> {
    for part in path {
        if table.get(part).is_none() {
            if !create {
                return Err(format!("missing table {part}"));
            }
            table.set(part, Value::obj());
        }
        let slot = obj_slot(table, part).ok_or("not a table")?;
        table = match slot {
            Value::Obj(_) => slot,
            Value::Arr(items) => items.last_mut().ok_or("empty array of tables")?,
            _ => return Err(format!("{part} is not a table")),
        };
    }
    Ok(table)
}

fn strip_comment(line: &str) -> &str {
    let (mut quote, mut escaped) = (None::<char>, false);
    for (index, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' => return &line[..index],
            None => {}
        }
    }
    line
}

fn depth(text: &str) -> i32 {
    let (mut quote, mut escaped, mut depth) = (None::<char>, false, 0);
    for c in text.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn split_key(text: &str) -> Res<Vec<String>> {
    let mut parts = Vec::new();
    let mut rest = text.trim();
    loop {
        let (part, tail) = if let Some(body) = rest.strip_prefix('"') {
            let end = body.find('"').ok_or("unterminated quoted key")?;
            (body[..end].to_string(), &body[end + 1..])
        } else if let Some(body) = rest.strip_prefix('\'') {
            let end = body.find('\'').ok_or("unterminated quoted key")?;
            (body[..end].to_string(), &body[end + 1..])
        } else {
            let end = rest.find(['.', ' ', '\t']).unwrap_or(rest.len());
            (rest[..end].to_string(), &rest[end..])
        };
        if part.is_empty() {
            return Err("empty key".into());
        }
        parts.push(part);
        let tail = tail.trim_start();
        match tail.strip_prefix('.') {
            Some(next) => rest = next.trim_start(),
            None if tail.is_empty() => return Ok(parts),
            None => return Err("unexpected text after key".into()),
        }
    }
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let (mut quote, mut escaped) = (None::<char>, false);
    for (index, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '=' => return Some((&line[..index], &line[index + 1..])),
            None => {}
        }
    }
    None
}

fn parse_value(text: &str) -> Res<Value> {
    let text = text.trim();
    let (value, rest) = value(text)?;
    if rest.trim().is_empty() {
        Ok(value)
    } else {
        Err("unexpected text after value".into())
    }
}

fn value(text: &str) -> Res<(Value, &str)> {
    let text = text.trim_start();
    if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return Err("multi-line strings are not supported".into());
    }
    if let Some(body) = text.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = body.char_indices();
        while let Some((index, c)) = chars.next() {
            match c {
                '"' => return Ok((Value::Str(out), &body[index + 1..])),
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    _ => return Err("unsupported string escape".into()),
                },
                c => out.push(c),
            }
        }
        return Err("unterminated string".into());
    }
    if let Some(body) = text.strip_prefix('\'') {
        let end = body.find('\'').ok_or("unterminated string")?;
        return Ok((Value::Str(body[..end].to_string()), &body[end + 1..]));
    }
    if let Some(mut rest) = text.strip_prefix('[') {
        let mut items = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix(']') {
                return Ok((Value::Arr(items), tail));
            }
            let (item, tail) = value(rest)?;
            items.push(item);
            rest = tail.trim_start();
            if let Some(tail) = rest.strip_prefix(',') {
                rest = tail;
            } else if !rest.starts_with(']') {
                return Err("expected , or ] in array".into());
            }
        }
    }
    if let Some(mut rest) = text.strip_prefix('{') {
        let mut table = Value::obj();
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix('}') {
                return Ok((table, tail));
            }
            let (key, tail) =
                split_assignment(rest).ok_or("expected key = value in inline table")?;
            let keys = split_key(key)?;
            let (item, tail) = value(tail)?;
            let target = descend(&mut table, &keys[..keys.len() - 1], true)?;
            target.set(&keys[keys.len() - 1], item);
            rest = tail.trim_start();
            if let Some(tail) = rest.strip_prefix(',') {
                rest = tail;
            } else if !rest.starts_with('}') {
                return Err("expected , or } in inline table".into());
            }
        }
    }
    let end = text.find([',', ']', '}', ' ', '\t']).unwrap_or(text.len());
    let (word, rest) = text.split_at(end);
    match word {
        "true" => Ok((Value::Bool(true), rest)),
        "false" => Ok((Value::Bool(false), rest)),
        w if !w.is_empty()
            && w.chars()
                .all(|c| c.is_ascii_digit() || c == '_' || c == '-' || c == '+') =>
        {
            Ok((Value::Num(w.replace('_', "")), rest))
        }
        _ => Err(format!("unsupported TOML value {word:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_manifest_shapes_cargo_uses() {
        let doc = parse(
            "[workspace]\nmembers = [\n  \"a\", # first\n  \"b\",\n]\n[workspace.metadata.cortex-pins]\nrust = \"1.2.3\"\n\
             [dependencies]\nx = \"=1.0.0\"\ny = { version = \"=2.0.0\", features = [\"f\"], package = 'z' }\nw.workspace = true\n\
             [target.'cfg(unix)'.dependencies]\nq = \"=3.0.0\"\n[[package]]\nname = \"n\"\n[[package]]\nname = \"m\"\n",
        )
        .unwrap();
        assert_eq!(
            doc.get("workspace")
                .unwrap()
                .get("members")
                .unwrap()
                .as_arr()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            doc.get("dependencies")
                .unwrap()
                .get("y")
                .unwrap()
                .get("package")
                .unwrap()
                .as_str(),
            Some("z")
        );
        assert_eq!(
            doc.get("dependencies")
                .unwrap()
                .get("w")
                .unwrap()
                .get("workspace"),
            Some(&Value::Bool(true))
        );
        let target = doc.get("target").unwrap().get("cfg(unix)").unwrap();
        assert_eq!(
            target
                .get("dependencies")
                .unwrap()
                .get("q")
                .unwrap()
                .as_str(),
            Some("=3.0.0")
        );
        assert_eq!(doc.get("package").unwrap().as_arr().unwrap().len(), 2);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(parse("a = \"\"\"x\"\"\"\n").is_err());
        assert!(parse("a = 1.5\n").is_err());
    }
}

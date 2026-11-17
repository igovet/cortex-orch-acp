//! Preserve Python's arbitrary JSON history without narrowing it to serde's numbers.
//! Numeric placeholders are private to this parse and cannot collide with input strings.
use super::*;
use std::collections::BTreeMap;

pub(in crate::machine) struct RegistryJson {
    pub value: Value,
    pub connection_order: Vec<String>,
    numbers: BTreeMap<String, String>,
}

fn invalid() -> Refused {
    refuse(
        "invalid_registry",
        "connections.json is not valid UTF-8 JSON.",
    )
}
pub(super) fn tokens(input: &str) -> Result<Vec<&str>> {
    let mut tokens = vec![];
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i] == b'"' {
            i += 1;
            loop {
                match bytes.get(i) {
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') => i += 2,
                    Some(_) => i += 1,
                    None => return Err(invalid()),
                }
            }
        } else if b" \t\r\n{}[],:".contains(&bytes[i]) {
            i += 1;
        } else {
            while i < bytes.len() && !b" \t\r\n{}[],:".contains(&bytes[i]) {
                i += 1;
            }
        }
        tokens.push(&input[start..i]);
    }
    Ok(tokens)
}
pub(super) fn number(token: &str) -> Result<String> {
    if ["NaN", "Infinity", "-Infinity"].contains(&token) {
        return Ok(token.into());
    }
    static NUMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$").unwrap()
    });
    if !NUMBER.is_match(token) {
        return Err(invalid());
    }
    if !token.contains(['.', 'e', 'E']) {
        return Ok(if token == "-0" { "0" } else { token }.into());
    }
    let n: f64 = token.parse().map_err(|_| invalid())?;
    Ok(if n.is_infinite() {
        if n.is_sign_negative() {
            "-Infinity".into()
        } else {
            "Infinity".into()
        }
    } else {
        crate::config_read_model::python_json(&json!(n))
    })
}
/// Replace each lone UTF-16 surrogate escape of one string token with U+FFFD;
/// a valid escaped pair and every other escape stay unchanged.
fn lossy_surrogates(token: &str) -> std::borrow::Cow<'_, str> {
    fn unit(t: &str, at: usize) -> Option<u32> {
        let hex = t.get(at..at + 6)?.strip_prefix("\\u")?;
        u32::from_str_radix(hex, 16).ok()
    }
    if !token.contains("\\u") {
        return token.into();
    }
    let (mut out, mut changed, mut i) = (String::with_capacity(token.len()), false, 0);
    while i < token.len() {
        if token[i..].starts_with('\\') {
            match unit(token, i) {
                Some(0xD800..=0xDBFF)
                    if unit(token, i + 6).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) =>
                {
                    out.push_str(&token[i..i + 12]);
                    i += 12;
                }
                Some(0xD800..=0xDFFF) => {
                    out.push_str("\\ufffd");
                    changed = true;
                    i += 6;
                }
                _ => {
                    // Copy the backslash and the escaped character whole.
                    let next = token[i + 1..].chars().next().map_or(0, char::len_utf8);
                    out.push_str(&token[i..i + 1 + next]);
                    i += 1 + next;
                }
            }
        } else {
            let c = token[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    if changed {
        out.into()
    } else {
        token.into()
    }
}
impl RegistryJson {
    pub(in crate::machine) fn parse(bytes: Option<&[u8]>) -> Result<Self> {
        Self::parse_with(bytes, false)
    }
    /// Read-only projections only (the default-profile seed captures ids and
    /// limits): Python decodes a lone surrogate escape it cannot re-encode, so
    /// such a registry is read with U+FFFD in its place and never rewritten.
    pub(in crate::machine) fn parse_lossy(bytes: Option<&[u8]>) -> Result<Self> {
        Self::parse_with(bytes, true)
    }
    fn parse_with(bytes: Option<&[u8]>, lossy: bool) -> Result<Self> {
        let input = std::str::from_utf8(bytes.unwrap_or_default()).map_err(|_| invalid())?;
        if input.is_empty() {
            return Ok(Self {
                value: json!({"schema_version":1,"connections":{},"override_history":{}}),
                connection_order: vec![],
                numbers: BTreeMap::new(),
            });
        }
        let raw = tokens(input)?;
        let tokens: Vec<std::borrow::Cow<'_, str>> = raw
            .into_iter()
            .map(|t| {
                if lossy && t.starts_with('"') {
                    lossy_surrogates(t)
                } else {
                    t.into()
                }
            })
            .collect();
        let tokens: Vec<&str> = tokens.iter().map(|t| t.as_ref()).collect();
        let strings: Vec<String> = tokens
            .iter()
            .filter(|t| t.starts_with('"'))
            .map(|t| serde_json::from_str(t).map_err(|_| invalid()))
            .collect::<Result<_>>()?;
        let base = "\0cortex-number-";
        let reserved: std::collections::BTreeSet<_> = strings
            .iter()
            .filter_map(|s| {
                s.strip_prefix(base)
                    .and_then(|s| s.split_once(':').map(|(label, _)| label))
            })
            .collect();
        // At most one namespace is reserved per input string; no growing-prefix scan.
        let namespace = (0..=strings.len())
            .find(|i| !reserved.contains(i.to_string().as_str()))
            .unwrap();
        let prefix = format!("{base}{namespace}:");
        let mut transformed = String::new();
        let mut numbers = BTreeMap::new();
        for token in tokens {
            if token.starts_with('"')
                || token.len() == 1 && b" \t\r\n{}[],:".contains(&token.as_bytes()[0])
                || ["true", "false", "null"].contains(&token)
            {
                transformed.push_str(token);
            } else {
                let key = format!("{prefix}{}", numbers.len());
                numbers.insert(key.clone(), number(token)?);
                transformed.push_str(&json!(key).to_string());
            }
        }
        let ordered: crate::config_read_model::ordered::Ordered =
            serde_json::from_str(&transformed).map_err(|_| invalid())?;
        let connection_order = ordered
            .member("connections")
            .map(|v| v.members.iter().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        let mut value = ordered.value;
        fn legal_keys(v: &Value, numbers: &BTreeMap<String, String>) -> bool {
            match v {
                Value::Object(m) => m
                    .iter()
                    .all(|(k, v)| !numbers.contains_key(k) && legal_keys(v, numbers)),
                Value::Array(a) => a.iter().all(|v| legal_keys(v, numbers)),
                _ => true,
            }
        }
        // Replacing numeric values must not make an illegal numeric object key valid.
        if !legal_keys(&value, &numbers) {
            return Err(invalid());
        }
        fn restore(v: &mut Value, numbers: &BTreeMap<String, String>) {
            match v {
                Value::String(s) => {
                    if let Some(n) = numbers.get(s) {
                        // Out-of-range metadata remains invalid, never rounded into a valid field.
                        *v = if !n.contains(['.', 'e', 'E'])
                            && !["NaN", "Infinity", "-Infinity"].contains(&n.as_str())
                        {
                            // Preserve integer type even beyond serde's fixed-width range.
                            n.parse::<i64>()
                                .map(|n| json!(n))
                                .or_else(|_| n.parse::<u64>().map(|n| json!(n)))
                                .unwrap_or_else(|_| {
                                    if n.starts_with('-') {
                                        json!(i64::MIN)
                                    } else {
                                        json!(u64::MAX)
                                    }
                                })
                        } else {
                            serde_json::from_str::<Value>(n)
                                .unwrap_or_else(|_| json!({"nonfinite":true}))
                        };
                    }
                }
                Value::Array(a) => a.iter_mut().for_each(|v| restore(v, numbers)),
                Value::Object(m) => m.values_mut().for_each(|v| restore(v, numbers)),
                _ => (),
            }
        }
        if let Some(m) = value.as_object_mut() {
            for (key, v) in m {
                if key != "override_history" {
                    restore(v, &numbers);
                }
            }
        }
        Ok(Self {
            value,
            numbers,
            connection_order,
        })
    }
    pub(in crate::machine) fn bytes(&self, value: &Value) -> Vec<u8> {
        fn encode(v: &Value, numbers: &BTreeMap<String, String>) -> String {
            match v {
                Value::String(s) if numbers.contains_key(s) => numbers[s].clone(),
                Value::Object(m) => format!(
                    "{{{}}}",
                    m.iter()
                        .map(|(k, v)| format!("{}:{}", json!(k), encode(v, numbers)))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Value::Array(a) => format!(
                    "[{}]",
                    a.iter()
                        .map(|v| encode(v, numbers))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                _ => crate::config_read_model::python_json(v),
            }
        }
        encode(value, &self.numbers).into_bytes()
    }
}

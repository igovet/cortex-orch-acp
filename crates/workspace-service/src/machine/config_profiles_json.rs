//! Ordered profile JSON with Python's wider numeric metadata, never rounded.
//! Wide atoms are admitted only in ignored top-level profile-document metadata.
use super::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[derive(Clone)]
pub(super) enum Ordered {
    Object(Vec<(String, Ordered)>),
    Array(Vec<Ordered>),
    Scalar(Value),
    Number(String),
}
impl Ordered {
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }
    pub fn from_value(v: &Value) -> Self {
        match v {
            Value::Object(m) => Self::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
            Value::Array(a) => Self::Array(a.iter().map(Self::from_value).collect()),
            v => Self::Scalar(v.clone()),
        }
    }
    fn from_ordered(v: write::Ordered) -> Self {
        match v {
            write::Ordered::Object(m) => Self::Object(
                m.into_iter()
                    .map(|(k, v)| (k, Self::from_ordered(v)))
                    .collect(),
            ),
            write::Ordered::Array(a) => {
                Self::Array(a.into_iter().map(Self::from_ordered).collect())
            }
            write::Ordered::Scalar(v) => Self::Scalar(v),
        }
    }
    pub fn pretty(&self) -> Vec<u8> {
        fn render(v: &Ordered, level: usize) -> String {
            let pad = "  ".repeat(level + 1);
            let end = "  ".repeat(level);
            match v {
                Ordered::Number(n) => n.clone(),
                Ordered::Scalar(v) => {
                    String::from_utf8(write::Ordered::from_value(v).pretty()).unwrap()
                }
                Ordered::Object(m) if !m.is_empty() => format!(
                    "{{\n{}\n{end}}}",
                    m.iter()
                        .map(|(k, v)| format!("{pad}{}: {}", json!(k), render(v, level + 1)))
                        .collect::<Vec<_>>()
                        .join(",\n")
                ),
                Ordered::Array(a) if !a.is_empty() => format!(
                    "[\n{}\n{end}]",
                    a.iter()
                        .map(|v| format!("{pad}{}", render(v, level + 1)))
                        .collect::<Vec<_>>()
                        .join(",\n")
                ),
                Ordered::Object(_) => "{}".into(),
                Ordered::Array(_) => "[]".into(),
            }
        }
        render(self, 0).into_bytes()
    }
    fn wide(&self) -> bool {
        match self {
            Self::Number(_) => true,
            Self::Object(m) => m.iter().any(|(_, v)| v.wide()),
            Self::Array(a) => a.iter().any(Self::wide),
            _ => false,
        }
    }
    pub fn qualified(&self) -> bool {
        if let Self::Object(m) = self {
            m.iter().all(|(k, v)| {
                !["format", "active", "applied", "pending", "profiles"].contains(&k.as_str())
                    || !v.wide()
            })
        } else {
            !self.wide()
        }
    }
}
impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Self::Number(_) => s.serialize_unit(), // ignored metadata only; never a modeled setting
            Self::Scalar(v) => v.serialize(s),
            Self::Object(m) => {
                let mut out = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    out.serialize_entry(k, v)?;
                }
                out.end()
            }
            Self::Array(a) => {
                let mut out = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    out.serialize_element(v)?;
                }
                out.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        write::Ordered::deserialize(d).map(Self::from_ordered)
    }
}
pub(super) fn parse(bytes: &[u8]) -> Result<Ordered> {
    let bad = || {
        invalid("routing-profiles.json is not valid UTF-8 JSON; repair it by hand. Nothing was changed.")
    };
    let input = std::str::from_utf8(bytes).map_err(|_| bad())?;
    let tokens = super::super::registry_json::tokens(input).map_err(|_| bad())?;
    let strings: Vec<String> = tokens
        .iter()
        .filter(|t| t.starts_with('"'))
        .map(|t| serde_json::from_str(t).map_err(|_| bad()))
        .collect::<Result<_>>()?;
    let reserved: std::collections::BTreeSet<_> = strings
        .iter()
        .filter_map(|s| {
            s.strip_prefix("\0profile-number-")
                .and_then(|s| s.split_once(':').map(|(n, _)| n))
        })
        .collect();
    let namespace = (0..=strings.len())
        .find(|n| !reserved.contains(n.to_string().as_str()))
        .unwrap();
    let prefix = format!("\0profile-number-{namespace}:");
    let mut numbers = std::collections::BTreeMap::new();
    let mut transformed = String::new();
    for t in tokens {
        let wide = matches!(t, "NaN" | "Infinity" | "-Infinity")
            || !t.starts_with('"')
                && t.as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_digit() || *b == b'-')
                && (write::user_defaults::exact_numbers(t.as_bytes()).is_err()
                    || t.parse::<f64>().is_ok_and(|n| !n.is_finite()));
        if wide {
            if t.len() > 4300 {
                return Err(refuse(
                    "native_config_unavailable",
                    "Numeric metadata exceeds the qualified Python integer bound.",
                ));
            }
            let n = super::super::registry_json::number(t).map_err(|_| bad())?;
            let key = format!("{prefix}{}", numbers.len());
            transformed.push_str(&json!(key).to_string());
            numbers.insert(key, n);
        } else {
            transformed.push_str(if t == "-0" { "0" } else { t });
        }
    }
    let mut out: Ordered = serde_json::from_str(&transformed).map_err(|_| bad())?;
    fn restore(
        v: &mut Ordered,
        numbers: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        match v {
            Ordered::Scalar(Value::String(s)) if numbers.contains_key(s) => {
                *v = Ordered::Number(numbers[s].clone())
            }
            Ordered::Array(a) => {
                for v in a {
                    restore(v, numbers)?;
                }
            }
            Ordered::Object(m) => {
                for (k, v) in m {
                    if numbers.contains_key(k) {
                        return Err(invalid("routing-profiles.json is not valid UTF-8 JSON; repair it by hand. Nothing was changed."));
                    }
                    restore(v, numbers)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    restore(&mut out, &numbers)?;
    if !out.qualified() {
        return Err(refuse(
            "native_config_unavailable",
            "Wide numbers in modeled profile fields retain Python.",
        ));
    }
    Ok(out)
}

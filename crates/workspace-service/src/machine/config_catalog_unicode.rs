//! Unicode 15 label normalization, generated from the Python oracle's Unicode database.
// Runtime uses only retained tables, never a Python process.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
struct Tables {
    decomposition: BTreeMap<u32, Vec<u32>>,
    combining: BTreeMap<u32, u8>,
    composition: BTreeMap<u64, u32>,
    categories: Vec<(u32, u32, u8)>,
}
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES
        .get_or_init(|| serde_json::from_str(include_str!("config_catalog_unicode.json")).unwrap())
}
fn category(c: char) -> u8 {
    let cp = c as u32;
    let ranges = &tables().categories;
    let index = ranges.partition_point(|(_, end, _)| *end < cp);
    ranges[index].2
}
fn combining(c: u32) -> u8 {
    tables().combining.get(&c).copied().unwrap_or(0)
}
fn compose(a: u32, b: u32) -> Option<u32> {
    // Unicode's algorithmic Hangul composition.
    if (0x1100..0x1113).contains(&a) && (0x1161..0x1176).contains(&b) {
        return Some(0xAC00 + (a - 0x1100) * 588 + (b - 0x1161) * 28);
    }
    if (0xAC00..=0xD7A3).contains(&a) && (a - 0xAC00) % 28 == 0 && (0x11A8..0x11C3).contains(&b) {
        return Some(a + b - 0x11A7);
    }
    tables()
        .composition
        .get(&(a as u64 * 0x110000 + b as u64))
        .copied()
}
pub(super) fn normalize(s: &str) -> String {
    let mut decomposed = Vec::new();
    for c in s.chars().map(u32::from) {
        if (0xAC00..=0xD7A3).contains(&c) {
            let offset = c - 0xAC00;
            decomposed.extend([0x1100 + offset / 588, 0x1161 + (offset % 588) / 28]);
            if offset % 28 != 0 {
                decomposed.push(0x11A7 + offset % 28);
            }
        } else if let Some(parts) = tables().decomposition.get(&c) {
            decomposed.extend(parts);
        } else {
            decomposed.push(c);
        }
    }
    // Stable canonical ordering; avoid quadratic insertion on untrusted mark runs.
    let mut start = 0;
    for i in 0..decomposed.len() {
        if combining(decomposed[i]) == 0 {
            decomposed[start..i].sort_by_key(|c| combining(*c));
            start = i + 1;
        }
    }
    decomposed[start..].sort_by_key(|c| combining(*c));
    let mut composed = Vec::new();
    let mut starter = 0;
    let mut last = 0;
    for c in decomposed {
        let class = combining(c);
        if !composed.is_empty() && (last == 0 || last < class) {
            if let Some(joined) = compose(composed[starter], c) {
                composed[starter] = joined;
                continue;
            }
        }
        if class == 0 {
            starter = composed.len();
        }
        composed.push(c);
        last = class;
    }
    composed
        .into_iter()
        .filter_map(char::from_u32)
        .filter(|c| !c.is_control() && category(*c) & 4 == 0)
        .collect()
}
pub(crate) fn safe_name(s: &str) -> bool {
    let s = normalize(s);
    let chars: Vec<_> = s.chars().collect();
    !s.trim().is_empty()
        && !s.to_ascii_lowercase().starts_with("xn--")
        && chars.iter().enumerate().all(|(i, &c)| {
            let cat = category(c);
            cat & 1 != 0
                || cat & 2 != 0 && i > 0 && !chars[i - 1].is_ascii()
                || c == ' '
                || "-_()#+&,'".contains(c)
                || c == ':' && chars.get(i + 1) == Some(&' ')
        })
}
pub(super) fn component(s: &str) -> Option<String> {
    let mut kept = String::new();
    let mut previous = None;
    for c in normalize(s).chars() {
        if category(c) & 2 != 0 && previous.is_none_or(|c: char| c.is_ascii()) {
            continue;
        }
        kept.push(c);
        previous = Some(c);
    }
    let clean = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        None
    } else if clean.chars().count() > 64 {
        Some(format!(
            "{}…",
            clean.chars().take(63).collect::<String>().trim_end()
        ))
    } else {
        Some(clean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_labels_match_the_oracle_scalar_and_composed_oracle() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/catalog-unicode-python.json"
        ))
        .unwrap();
        for row in corpus["vectors"].as_array().unwrap() {
            let text = row[0].as_str().unwrap();
            assert_eq!(normalize(text), row[1].as_str().unwrap(), "{text:?}");
            assert_eq!(component(text).as_deref(), row[2].as_str(), "{text:?}");
            assert_eq!(safe_name(text), row[3].as_bool().unwrap(), "{text:?}");
        }
    }
}

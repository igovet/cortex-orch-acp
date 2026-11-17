//! Bounded discovery-description projection; never a raw diagnostic sink.
use regex::{Captures, Regex};
use serde_json::Value;
use sha2::{Digest, Sha256};
fn hidden(s: &str) -> String {
    format!(
        "[HIDDEN_SECRET:{:.16}]",
        format!("{:x}", Sha256::digest(s.as_bytes()))
    )
}
fn re(pattern: &str) -> Regex {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, Regex>>> =
        std::sync::OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(pattern.to_owned())
        .or_insert_with(|| Regex::new(pattern).unwrap())
        .clone()
}
fn replace(s: &str, pattern: &str, f: impl Fn(&Captures<'_>) -> String) -> String {
    re(pattern).replace_all(s, f).into_owned()
}
fn marker(s: &str) -> bool {
    s.starts_with("[REDACTED") || s.starts_with("[HIDDEN_SECRET")
}
fn quoted(s: &str) -> Option<(usize, char)> {
    let q = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &s[1..];
    let end = rest.find(['"', '\'', '\r', '\n']).unwrap_or(rest.len());
    Some((1 + end + usize::from(rest[end..].starts_with(q)), q))
}
pub(super) fn pystr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", repr(&Value::String(k.clone())), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => v.to_string(),
    }
}
fn repr(v: &Value) -> String {
    if let Value::String(s) = v {
        let q = if s.contains('\'') && !s.contains('"') {
            '"'
        } else {
            '\''
        };
        format!(
            "{q}{}{q}",
            s.replace('\\', "\\\\")
                .replace(q, &format!("\\{q}"))
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        )
    } else {
        pystr(v)
    }
}
pub(super) fn scrub(input: &str) -> String {
    let original = input.chars().count();
    let s = redact(input);
    if s.chars().count() > 1000 {
        return format!(
            "{}...[truncated_from:{original}_chars]",
            s.chars().take(1000).collect::<String>()
        );
    }
    s
}
/// The credential redaction of `scrub` without its 1000-character display cap:
/// a caller that keeps its own bound (display activity) applies only this.
pub(crate) fn redact(input: &str) -> String {
    let mut s = replace(input, r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@\s]+@", |c| {
        format!("{}[REDACTED]@", &c[1])
    });
    // Python's header grammar has lookahead/backreferences. Scan the bounded
    // tail explicitly instead of weakening it to a bearer-only expression.
    let head = re(r#"(?i)\b[A-Za-z0-9_-]*authorization["']?[ \t]*[:=][ \t]*"#);
    let scheme = re(r"^[A-Za-z][A-Za-z0-9!#$%&*+.^_`|~-]*[ \t]+");
    let param = r#"[A-Za-z0-9_-]+=(?:"[^"\r\n]*"|[^\s,;"'{}\[\]]*)"#;
    let cred = re(&format!(
        r#"^(?:{param}|[^\s"',;{{}}\[\]]+)(?:[ \t]*,[ \t]*{param})*"#
    ));
    let mut out = String::new();
    let mut end = 0;
    for h in head.find_iter(&s) {
        if h.start() < end {
            continue;
        }
        let tail = &s[h.end()..];
        if marker(tail) {
            continue;
        }
        let parsed = if let Some((n, q)) = quoted(tail) {
            Some((n, format!("{q}[REDACTED]{q}")))
        } else {
            let skip = scheme
                .find(tail)
                .filter(|m| !marker(&tail[m.end()..]))
                .map(|m| m.end())
                .unwrap_or(0);
            cred.find(&tail[skip..])
                .map(|m| (skip + m.end(), "[REDACTED]".into()))
                .or_else(|| cred.find(tail).map(|m| (m.end(), "[REDACTED]".into())))
        };
        if let Some((n, value)) = parsed {
            out.push_str(&s[end..h.end()]);
            out.push_str(&value);
            end = h.end() + n;
        }
    }
    out.push_str(&s[end..]);
    s = out;
    s = replace(
        &s,
        r"(?i)\b(bearer|token)\s+([A-Za-z0-9._~+/=_-]{8,})",
        |c| {
            if c[1].eq_ignore_ascii_case("bearer") || c[2].bytes().any(|b| b.is_ascii_digit()) {
                "[REDACTED_TOKEN]".into()
            } else {
                c[0].into()
            }
        },
    );
    let key = r"[A-Za-z0-9_]*(?:api[_-]?key|token|secret|password|authorization)[A-Za-z0-9_]*";
    s = replace(&s, &format!(r"(?i)\b({key})\s*[=:]\s*(\S+)"), |c| {
        if marker(&c[2]) {
            c[0].into()
        } else {
            format!("{}=[REDACTED]", &c[1])
        }
    });
    let re = re(&format!(r#"(?i)\b({key})(["'])([ \t]*[:=][ \t]*)"#));
    let mut out = String::new();
    let mut end = 0;
    for c in re.captures_iter(&s) {
        let m = c.get(0).unwrap();
        if m.start() < end {
            continue;
        }
        let tail = &s[m.end()..];
        if marker(tail) {
            continue;
        }
        let parsed = quoted(tail).map(|(n, q)| (n, q.to_string())).or_else(|| {
            let n = tail
                .find(|c: char| c.is_whitespace() || ",;}]".contains(c))
                .unwrap_or(tail.len());
            (n > 0).then_some((n, String::new()))
        });
        if let Some((n, q)) = parsed {
            out.push_str(&s[end..m.end()]);
            out.push_str(&format!("{q}[REDACTED]{q}"));
            end = m.end() + n;
        }
    }
    out.push_str(&s[end..]);
    s = out;
    s = replace(
        &s,
        r"(?i)\b((?:api[_ -]?key|token|secret|password|authorization|credential)\s+(?:is|was))\s+\S+",
        |c| format!("{} [REDACTED]", &c[1]),
    );
    s = replace(
        &s,
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
        |c| {
            if c[0].starts_with("sk-") && !c[0].bytes().any(|b| b.is_ascii_digit()) {
                c[0].into()
            } else {
                hidden(&c[0])
            }
        },
    );
    s = replace(&s, r"\b[A-Za-z0-9]{24,}\b", |c| {
        if c[0].bytes().any(|b| b.is_ascii_digit()) {
            hidden(&c[0])
        } else {
            c[0].into()
        }
    });
    s
}

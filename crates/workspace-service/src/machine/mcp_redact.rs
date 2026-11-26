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
    let key = r"[A-Za-z0-9_]*(?:api[_-]?key|access[_-]?key|private[_-]?key|token|secret|password|passwd|credential|authorization)[A-Za-z0-9_]*";
    let value = r#""[^"\r\n]*"\S*|'[^'\r\n]*'\S*|[^\s'"]+|['"]\S*"#;
    // `NAME=value` / `NAME: value`: only the value is masked (the frozen discovery oracle fixes the `NAME=[REDACTED]` form).
    let assignment = |s: &str, pattern: String| {
        replace(s, &pattern, |c| {
            let raw = &c[3];
            let (_q, inner) = match raw.chars().next() {
                Some(q @ ('"' | '\'')) if raw.len() >= 2 && raw.ends_with(q) => {
                    (q.to_string(), &raw[1..raw.len() - 1])
                }
                Some(q @ ('"' | '\'')) => (q.to_string(), &raw[1..]),
                _ => (String::new(), raw),
            };
            let name = c[1].to_lowercase();
            // A token count (`max_tokens=100`) is a number, not a credential.
            let count = name.contains("token") && !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit());
            if (_q.is_empty() && marker(inner)) || count || name == "hidden_secret" {
                c[0].into()
            } else {
                format!("{}=[REDACTED]", &c[1])
            }
        })
    };
    s = assignment(&s, format!(r#"(?i)\b({key})(\s*[=:]\s*)({value})"#));
    // Environment-style names ending in KEY (`OPENAI_API_KEY`, `SSH_KEY`) are credentials too.
    s = assignment(&s, format!(r#"\b([A-Z][A-Z0-9_]*_KEYS?)(=)({value})"#));
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
            if c[0].starts_with("sk-") && !c[0].starts_with("sk-proj-") && !c[0].bytes().any(|b| b.is_ascii_digit()) {
                c[0].into()
            } else {
                hidden(&c[0])
            }
        },
    );
    // A bare `Basic <base64>` credential (after the assignment forms, which the frozen oracle fixes).
    s = replace(&s, r"(?i:(?:\b(Bearer|token|Basic)\s+)?)\b(Basic)\s+([A-Za-z0-9+/]{8,}=*)", |c| {
        if c.get(1).is_none() && c[3].bytes().any(|b| b.is_ascii_digit() || b == b'=' || b == b'+' || b == b'/') {
            format!("{} [REDACTED]", &c[2])
        } else {
            c[0].into()
        }
    });
    s = replace(&s, r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)", |_| "[REDACTED]".into());
    s = replace(&s, r"\b[A-Za-z0-9]{24,}\b", |c| {
        // A lowercase hex string of a digest length (MD5, SHA-1, SHA-256, SHA-512) is a digest or revision, as Git and
        // sha*sum print them; it is not a credential. Mixed or upper case stays a suspected secret.
        let hex = matches!(c[0].len(), 32 | 40 | 64 | 128) && c[0].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex && c[0].bytes().any(|b| b.is_ascii_digit()) {
            hidden(&c[0])
        } else {
            c[0].into()
        }
    });
    s
}
/// Whether an argument or field name denotes a credential. The name is split into
/// words (`apiKey`, `api_key`, `sudo-password` and `Authorization` alike) and matched
/// whole, so `graphRevision` or a token count (`input_tokens`, `tokenCount`) is not one.
pub(crate) fn secret_name(name: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    let has = |w: &str| words.iter().any(|x| x == w);
    let pair = |a: &str, b: &str| words.windows(2).any(|p| p[0] == a && p[1] == b);
    // A token is a credential unless the name counts or limits tokens.
    let counted = words.iter().any(|w| {
        matches!(w.as_str(), "count" | "counts" | "usage" | "limit" | "max" | "min" | "total" | "budget" | "used" | "input" | "output" | "cached" | "reasoning" | "prompt" | "completion")
    });
    ["password", "passwd", "passphrase", "secret", "secrets", "authorization", "credential", "credentials", "apikey", "apikeys"]
        .iter()
        .any(|w| has(w))
        || (has("token") && !counted)
        || ["api", "access", "private", "secret", "auth", "ssh", "signing"].iter().any(|a| pair(a, "key"))
        || pair("client", "secret")
        || pair("bearer", "token")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn value_formats_are_masked_wherever_they_appear() {
        for text in [
            "printf 'OPENAI_API_KEY=sk-qa0123456789abcdefghijklmnop\\n'",
            "OPENAI_API_KEY=sk-qa0123456789abcdefghijklmnop",
            "sk-proj-abcDEF_ghi-JKL0123456789xyz",
            "ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            "xoxb-123456789012-abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefgh12345678",
            "Authorization: Bearer abcdefgh12345678",
            "Basic dXNlcjpwYXNzd29yZA==",
            "DB_PASSWORD=hunter2hunter2",
            "MY_TOKEN='a b c'",
            "SSH_KEY=abcdef",
            "client_secret: abcdef12",
        ] {
            let out = redact(text);
            for leak in ["qa0123", "abcDEF", "0123456789abcdefghij", "11ABCDEFG", "123456789012", "IOSFODNN7", "abc\n", "hbGciOi", "abcdefgh12", "dXNlcjpw", "hunter2", "a b c", "abcdef12"] {
                assert!(!out.contains(leak), "{text:?} -> {out:?}");
            }
        }
        let out = redact("printf 'OPENAI_API_KEY=sk-qa0123456789abcdefghijklmnop\\n'");
        assert!(out.starts_with("printf 'OPENAI_API_KEY=[REDACTED]"), "{out}");
        assert_eq!(redact(&redact("API_TOKEN=abcdef12 x")), redact("API_TOKEN=abcdef12 x"));
    }
    #[test]
    fn harmless_text_is_kept() {
        let rev = "ab12".repeat(16);
        for text in [
            format!("{{\"graphRevision\":\"{rev}\"}}"),
            "max_tokens=100 input_tokens: 42 tokenCount=7".to_string(),
            "commit 0123456789abcdef0123456789abcdef01234567".to_string(),
            "sort_key=name Basic usage of the key".to_string(),
            "sk-learn is a package".to_string(),
        ] {
            assert_eq!(redact(&text), text);
        }
        let marker = redact("sk-qa0123456789abcdefghijklmnop");
        assert!(marker.starts_with("[HIDDEN_SECRET:"));
        assert_eq!(redact(&marker), marker);
    }
    #[test]
    fn secret_names_match_whole_words() {
        for name in [
            "api_key", "apiKey", "API_KEY", "apikey", "token", "authToken", "secret", "password", "passwd", "Authorization",
            "credential", "credentials", "private_key", "client_secret", "access_key", "sudo_password", "sudoPassword", "SECRET_TOKEN",
        ] {
            assert!(secret_name(name), "{name}");
        }
        for name in [
            "graphRevision", "revision", "tokens", "input_tokens", "output_tokens", "tokenCount", "token_count", "max_tokens", "usage",
            "keyboard", "key", "monkey", "authority", "passport", "secretary", "cwd", "command", "sort_key",
        ] {
            assert!(!secret_name(name), "{name}");
        }
    }
}

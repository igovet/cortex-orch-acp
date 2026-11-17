//! The store readers' display scrubber (`STORE_PYTHON_HELPERS.scrub` in
//! `integrations/paseo/server/worker-history.ts`): credential assignments and
//! private absolute paths never reach the client.
use regex::Regex;
use std::sync::OnceLock;

// Python's `\s`/`\S` for str patterns follow `str.isspace`, which also covers
// the \x1c-\x1f separators that the regex crate's `\s` leaves out.
const CREDENTIAL: &str = r"(?i)(?:bearer|token|password|secret|api[_ -]?key)[\s\x1c-\x1f]*[:=][\s\x1c-\x1f]*[^\s\x1c-\x1f]+";
const PRIVATE_PATH: &str = r"^/(?:home|tmp|private|Users)/[^\s\x1c-\x1f,;]+";

fn credential() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(CREDENTIAL).expect("credential pattern"))
}

fn private_path() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(PRIVATE_PATH).expect("private path pattern"))
}

/// `scrub(value)` for a string value.
pub fn scrub(value: &str) -> String {
    let redacted = credential().replace_all(value, "[redacted]");
    // `(?<![A-Za-z0-9])/(?:home|tmp|private|Users)/[^\s,;]+`: the regex crate
    // has no look-behind, so try each '/' that is not preceded by an ASCII
    // letter or digit, exactly where Python's scanner would.
    let text: &str = &redacted;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut at, mut copied) = (0, 0);
    while at < bytes.len() {
        if bytes[at] == b'/' && (at == 0 || !bytes[at - 1].is_ascii_alphanumeric()) {
            if let Some(found) = private_path().find(&text[at..]) {
                out.push_str(&text[copied..at]);
                out.push_str("[private path]");
                at += found.end();
                copied = at;
                continue;
            }
        }
        at += 1;
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn redacts_credentials_and_private_paths_like_python() {
        assert_eq!(scrub("token=abc def"), "[redacted] def");
        assert_eq!(scrub("API-key: x"), "[redacted]");
        assert_eq!(scrub("Api key = s3cr3t,"), "[redacted]");
        assert_eq!(scrub("see /home/u/a.txt, ok"), "see [private path], ok");
        assert_eq!(scrub("x/home/u"), "x/home/u");
        assert_eq!(scrub("x/home/-/tmp/z q"), "x/home/-[private path] q");
        assert_eq!(scrub("/tmp/"), "/tmp/");
        assert_eq!(scrub("é/tmp/a"), "é[private path]");
    }
}

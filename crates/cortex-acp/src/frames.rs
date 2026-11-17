//! Python `transport.StdioTransport` reader: one bounded inbound line per
//! frame. A malformed line becomes Python's JSON-RPC error and the connection
//! continues; only EOF or a stdin failure ends reading.
use cortex_engine::wire::{self, LineFault};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Python `MAX_FRAME_BYTES`: one line's bytes, excluding its newline.
pub(crate) const MAX_FRAME_BYTES: usize = 15 * 1024 * 1024;
pub(crate) const OVERSIZED_FRAME_MESSAGE: &str =
    "Request exceeds Cortex's 15 MiB message limit; attach fewer or smaller images.";
const ID_RECOVERY_PREFIX_BYTES: usize = 64 * 1024;
const MAX_JSON_DEPTH: usize = 64;

/// One malformed inbound line, answered with `id` (null when unknown).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrameError {
    pub code: i64,
    pub message: String,
    pub id: Value,
}
impl FrameError {
    fn new(code: i64, message: &str, id: Value) -> Self {
        Self {
            code,
            message: message.into(),
            id,
        }
    }
    pub(crate) fn reply(&self) -> Value {
        serde_json::json!({"jsonrpc":"2.0","id":self.id,"error":{"code":self.code,"message":self.message}})
    }
}

/// Python `_exceeds_json_depth`: bracket nesting outside string literals.
fn exceeds_depth(text: &[u8], max: usize) -> bool {
    let (mut depth, mut in_string, mut escape) = (0i64, false, false);
    for &b in text {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > max as i64 {
                    return true;
                }
            }
            b'}' | b']' => depth -= 1,
            _ => {}
        }
    }
    false
}
fn skip_ws(text: &str, mut at: usize) -> usize {
    let b = text.as_bytes();
    while at < b.len() && matches!(b[at], b' ' | b'\t' | b'\r' | b'\n') {
        at += 1;
    }
    at
}
/// Python `JSONDecoder.raw_decode`: one value at `at` and the offset after it.
fn raw_decode(text: &str, at: usize) -> Option<(Value, usize)> {
    let mut stream = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
    match stream.next() {
        Some(Ok(v)) => Some((v, at + stream.byte_offset())),
        _ => None,
    }
}
/// Python `recover_request_id`: the top-level integer or string `id` of an
/// oversized frame's head, read member by member; never guessed.
pub(crate) fn recover_request_id(head: &[u8]) -> Value {
    let head = &head[..head.len().min(ID_RECOVERY_PREFIX_BYTES)];
    // A multi-byte character cut by the head boundary is left undecoded.
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap(),
        Err(_) => return Value::Null,
    };
    let mut at = skip_ws(text, 0);
    if !text[at..].starts_with('{') {
        return Value::Null;
    }
    at += 1;
    loop {
        at = skip_ws(text, at);
        if !text[at..].starts_with('"') {
            return Value::Null;
        }
        let Some((key, next)) = raw_decode(text, at) else {
            return Value::Null;
        };
        at = skip_ws(text, next);
        if !text[at..].starts_with(':') {
            return Value::Null;
        }
        at = skip_ws(text, at + 1);
        if exceeds_depth(text[at..].as_bytes(), MAX_JSON_DEPTH) {
            return Value::Null;
        }
        let Some((value, next)) = raw_decode(text, at) else {
            return Value::Null;
        };
        at = next;
        if key == "id" {
            return if value.is_string() || value.is_i64() || value.is_u64() {
                value
            } else {
                Value::Null
            };
        }
        at = skip_ws(text, at);
        if !text[at..].starts_with(',') {
            return Value::Null;
        }
        at += 1;
    }
}
/// Python `parse_frame` for one newline-stripped line. Never echoes content.
pub(crate) fn parse_frame(line: &[u8]) -> Result<Value, FrameError> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(FrameError::new(
            -32700,
            OVERSIZED_FRAME_MESSAGE,
            recover_request_id(line),
        ));
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return Err(FrameError::new(
            -32700,
            "Frame is not valid UTF-8.",
            Value::Null,
        ));
    };
    if exceeds_depth(line, MAX_JSON_DEPTH) {
        return Err(FrameError::new(
            -32700,
            &format!("Frame exceeds the maximum nesting depth of {MAX_JSON_DEPTH}."),
            Value::Null,
        ));
    }
    let message = match wire::parse_line(text) {
        Ok(v) => v,
        Err(LineFault::DuplicateKey) => {
            return Err(FrameError::new(
                -32600,
                "Duplicate key in a JSON-RPC object.",
                Value::Null,
            ))
        }
        Err(LineFault::Syntax) => {
            return Err(FrameError::new(
                -32700,
                "Frame is not valid JSON.",
                Value::Null,
            ))
        }
    };
    let Value::Object(object) = &message else {
        return Err(FrameError::new(
            -32600,
            "Batches are not supported; send one JSON-RPC object per line.",
            Value::Null,
        ));
    };
    let id = || object.get("id").cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(FrameError::new(
            -32600,
            "Missing or invalid \"jsonrpc\": \"2.0\".",
            id(),
        ));
    }
    if object.get("method").is_some_and(|m| !m.is_string()) {
        return Err(FrameError::new(
            -32600,
            "\"method\" must be a string.",
            id(),
        ));
    }
    Ok(message)
}
/// One inbound line event: `None` at EOF.
pub(crate) type Inbound = Result<Map<String, Value>, FrameError>;
/// Python `_read_loop`: read one line of at most `MAX_FRAME_BYTES` bytes.
/// An oversized or final unterminated line keeps only its head for id
/// recovery and drains forward without accumulating the rest. Blank lines
/// are skipped. A stdin failure ends reading like EOF.
pub(crate) async fn read<R: AsyncBufRead + Unpin>(r: &mut R) -> Option<Inbound> {
    loop {
        let mut line = Vec::new();
        let mut oversized = false;
        let terminated = loop {
            let Ok(buf) = r.fill_buf().await else {
                return None;
            };
            if buf.is_empty() {
                break false;
            }
            let (n, done) = match buf.iter().position(|b| *b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            let content = if done { n - 1 } else { n };
            if !oversized {
                if line.len() + content > MAX_FRAME_BYTES {
                    oversized = true;
                    let keep = ID_RECOVERY_PREFIX_BYTES
                        .saturating_sub(line.len())
                        .min(content);
                    line.extend_from_slice(&buf[..keep]);
                    line.truncate(ID_RECOVERY_PREFIX_BYTES);
                } else {
                    line.extend_from_slice(&buf[..content]);
                }
            }
            r.consume(n);
            if done {
                break true;
            }
        };
        if !terminated && line.is_empty() && !oversized {
            return None;
        }
        if oversized || !terminated {
            return Some(Err(FrameError::new(
                -32700,
                OVERSIZED_FRAME_MESSAGE,
                recover_request_id(&line),
            )));
        }
        if line.is_empty() {
            continue;
        }
        return Some(parse_frame(&line).map(|v| match v {
            Value::Object(o) => o,
            _ => unreachable!("parse_frame admits only objects"),
        }));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fault(line: &str) -> (i64, String, Value) {
        let e = parse_frame(line.as_bytes()).unwrap_err();
        (e.code, e.message, e.id)
    }
    #[test]
    fn python_parse_frame_order_and_wording() {
        assert_eq!(fault("nope").0, -32700);
        assert_eq!(
            fault("[1]").1,
            "Batches are not supported; send one JSON-RPC object per line."
        );
        assert_eq!(fault("5").0, -32600);
        assert_eq!(
            fault(r#"{"a":1,"a":2}"#).1,
            "Duplicate key in a JSON-RPC object."
        );
        // The duplicate's object closes after the syntax error: Python reports JSON.
        assert_eq!(fault(r#"{"a":1,"a":2,"#).1, "Frame is not valid JSON.");
        assert_eq!(fault(r#"[{"a":1,"a":2}, x]"#).0, -32600);
        assert_eq!(fault(r#"{"jsonrpc":"1.0","id":{"x":1}}"#).2, json!({"x":1}));
        assert_eq!(
            fault(r#"{"jsonrpc":"2.0","id":3,"method":4}"#),
            (-32600, "\"method\" must be a string.".into(), json!(3))
        );
        assert_eq!(
            fault(&format!("{}{}", "[".repeat(65), "]".repeat(65))).1,
            "Frame exceeds the maximum nesting depth of 64."
        );
        assert_eq!(
            fault(&format!("{}{}", "[".repeat(64), "]".repeat(64))).0,
            -32600
        );
        assert_eq!(
            parse_frame(&[0xff]).unwrap_err().message,
            "Frame is not valid UTF-8."
        );
    }
    #[test]
    fn python_request_id_recovery() {
        let id = |s: &str| recover_request_id(s.as_bytes());
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":7,"params":{"x":"aaa"#),
            json!(7)
        );
        assert_eq!(id(r#" { "id" : "p-1" , "x""#), json!("p-1"));
        assert_eq!(id(r#"{"params":{"x":"aaa","id":9"#), Value::Null);
        assert_eq!(id(r#"{"id":1.5,"x""#), Value::Null);
        assert_eq!(id(r#"{"id":true}"#), Value::Null);
        assert_eq!(id(r#"[{"id":1}]"#), Value::Null);
        let mut cut = br#"{"id":4,"a":""#.to_vec();
        cut.push(0xc3);
        assert_eq!(recover_request_id(&cut), json!(4));
        cut.push(0x28);
        assert_eq!(recover_request_id(&cut), Value::Null);
    }
    #[tokio::test]
    async fn lines_are_bounded_drained_and_skipped_like_python() {
        let mut input = b"\n".to_vec();
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"m"}"#);
        input.push(b'\n');
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":2,"x":""#);
        input.extend(std::iter::repeat_n(b'a', MAX_FRAME_BYTES));
        input.extend_from_slice(b"\"}\n");
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":3}"#);
        let mut r = tokio::io::BufReader::new(&input[..]);
        assert_eq!(read(&mut r).await.unwrap().unwrap()["id"], 1);
        let e = read(&mut r).await.unwrap().unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str(), e.id),
            (-32700, OVERSIZED_FRAME_MESSAGE, json!(2))
        );
        // A final unterminated line is answered like an oversized one.
        let e = read(&mut r).await.unwrap().unwrap_err();
        assert_eq!(
            (e.message.as_str(), e.id),
            (OVERSIZED_FRAME_MESSAGE, json!(3))
        );
        assert!(read(&mut r).await.is_none());
    }
    #[tokio::test]
    async fn a_line_of_exactly_the_limit_is_admitted() {
        let head = br#"{"jsonrpc":"2.0","id":1,"x":""#;
        let mut input = head.to_vec();
        input.extend(std::iter::repeat_n(b'a', MAX_FRAME_BYTES - head.len() - 2));
        input.extend_from_slice(b"\"}");
        assert_eq!(input.len(), MAX_FRAME_BYTES);
        input.push(b'\n');
        let mut r = tokio::io::BufReader::new(&input[..]);
        assert_eq!(read(&mut r).await.unwrap().unwrap()["id"], 1);
    }
}

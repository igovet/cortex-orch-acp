//! Engine-facing prompt blocks for retained owner attachments. The blocks follow
//! the engine's advertised `promptCapabilities` and what the bundled adapters do
//! with them (claude-agent-acp 0.107.0 decodes text, PNG/JPEG/GIF/WebP and PDF
//! blobs; codex-acp 2.6.0 turns only text resources and images into model input
//! and would pass any other blob through as base64 text). An attachment is
//! delivered once per engine session; later prompts reference it by marker.
use base64::Engine as _;
use cortex_engine::{engine::PromptCapabilities, environment::EngineKind};
use serde_json::{json, Value};
use std::collections::HashSet;

/// One verified retained attachment body.
pub struct Stored {
    pub kind: String,
    pub mime: Option<String>,
    pub uri: Option<String>,
    pub name: Option<String>,
    pub sha256: String,
    /// Unique per retained body (the private body file name).
    pub key: String,
    pub bytes: Vec<u8>,
}
const TEXT_APPLICATIONS: [&str; 10] = [
    "application/json",
    "application/xml",
    "application/javascript",
    "application/typescript",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/x-sh",
    "application/sql",
    "application/graphql",
];
/// MIME types the engines' image path accepts.
pub fn supported_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}
pub fn base_mime(mime: Option<&str>) -> Option<String> {
    let base = mime?.split(';').next()?.trim().to_ascii_lowercase();
    match base.as_str() {
        "" => None,
        "image/jpg" => Some("image/jpeg".into()),
        _ => Some(base),
    }
}
fn declared_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || TEXT_APPLICATIONS.contains(&mime)
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}
fn undeclared(mime: &Option<String>) -> bool {
    mime.as_deref()
        .is_none_or(|m| m == "application/octet-stream" || m == "binary/octet-stream")
}
fn display_name(s: &Stored) -> String {
    let from_uri = || {
        s.uri
            .as_deref()
            .and_then(|u| u.split(['?', '#']).next())
            .and_then(|u| u.rsplit('/').next())
            .filter(|n| !n.is_empty())
    };
    s.name
        .as_deref()
        .filter(|n| !n.is_empty())
        .or_else(from_uri)
        .map(|n| n.chars().take(200).collect())
        .unwrap_or_else(|| format!("attachment-{}", &s.sha256[..12.min(s.sha256.len())]))
}
fn describe(s: &Stored) -> String {
    format!(
        "{} (mime {}, {} bytes, sha256 {})",
        display_name(s),
        base_mime(s.mime.as_deref()).unwrap_or_else(|| "unknown".into()),
        s.bytes.len(),
        s.sha256
    )
}
fn uri_of(s: &Stored) -> String {
    s.uri
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("cortex:attachment/{}", &s.sha256[..12.min(s.sha256.len())]))
}
fn marker(s: &Stored, why: &str) -> Value {
    json!({"type":"text","text":format!("[attachment not readable by the engine: {} - {why}]",describe(s))})
}
/// The block(s) for one attachment that has not yet been delivered to the session.
pub fn block_for(s: &Stored, caps: PromptCapabilities, engine: EngineKind) -> Value {
    let mime = base_mime(s.mime.as_deref());
    if s.kind == "image" || mime.as_deref().is_some_and(|m| m.starts_with("image/")) {
        return match mime.as_deref() {
            Some(m) if supported_image(m) && caps.image => json!({"type":"image","mimeType":m,
                "data":base64::engine::general_purpose::STANDARD.encode(&s.bytes)}),
            Some(m) if supported_image(m) => marker(s, "the engine does not accept image input"),
            _ => marker(
                s,
                "unsupported image type; only PNG, JPEG, GIF and WebP are sent",
            ),
        };
    }
    let text = || {
        std::str::from_utf8(&s.bytes)
            .ok()
            .filter(|t| !t.contains('\0'))
    };
    let is_text = match mime.as_deref() {
        Some(m) if declared_text(m) => text().is_some(),
        _ if undeclared(&mime) => text().is_some(),
        _ => false,
    };
    if is_text {
        let body = text().unwrap();
        return if caps.embedded_context {
            json!({"type":"resource","resource":{"uri":uri_of(s),
                "mimeType":mime.clone().filter(|m|declared_text(m)).unwrap_or_else(||"text/plain".into()),
                "text":body}})
        } else {
            json!({"type":"text","text":format!("[attachment: {}]\n{body}\n[end of attachment]",describe(s))})
        };
    }
    if mime.as_deref() == Some("application/pdf") {
        // Only claude-agent-acp decodes a PDF blob; codex-acp would put base64 text
        // into the model context.
        return if engine == EngineKind::Claude && caps.embedded_context {
            json!({"type":"resource","resource":{"uri":uri_of(s),"mimeType":"application/pdf",
                "blob":base64::engine::general_purpose::STANDARD.encode(&s.bytes)}})
        } else {
            marker(
                s,
                "this engine cannot read PDF attachments; the PDF is attached but not readable",
            )
        };
    }
    marker(s, "binary content is not sent to the engine")
}
/// Blocks for this prompt. `sent` holds the keys already delivered to the session;
/// they are referenced by one marker instead of being re-sent. The returned keys
/// are the attachments newly delivered by these blocks.
pub fn prompt_blocks(
    items: &[Stored],
    sent: &HashSet<String>,
    caps: PromptCapabilities,
    engine: EngineKind,
) -> (Vec<Value>, Vec<String>) {
    let mut blocks = Vec::new();
    let mut fresh = Vec::new();
    let mut earlier = Vec::new();
    for item in items {
        if sent.contains(&item.key) {
            earlier.push(describe(item));
        } else {
            blocks.push(block_for(item, caps, engine));
            fresh.push(item.key.clone());
        }
    }
    if !earlier.is_empty() {
        blocks.push(json!({"type":"text","text":format!(
            "[attachments already provided earlier in this session: {}]",earlier.join("; "))}));
    }
    (blocks, fresh)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(kind: &str, mime: Option<&str>, bytes: &[u8]) -> Stored {
        Stored {
            kind: kind.into(),
            mime: mime.map(Into::into),
            uri: Some("file:///a/notes.txt".into()),
            name: None,
            sha256: "ab".repeat(32),
            key: "k".into(),
            bytes: bytes.to_vec(),
        }
    }
    const ALL: PromptCapabilities = PromptCapabilities {
        image: true,
        embedded_context: true,
    };
    const NONE: PromptCapabilities = PromptCapabilities {
        image: false,
        embedded_context: false,
    };
    #[test]
    fn text_images_pdf_and_binary_follow_capabilities() {
        let t = block_for(
            &item("resource", Some("text/plain; charset=utf-8"), b"hello"),
            ALL,
            EngineKind::Codex,
        );
        assert_eq!(t["type"], "resource");
        assert_eq!(t["resource"]["text"], "hello");
        let t = block_for(
            &item("resource", None, b"fn main(){}"),
            NONE,
            EngineKind::Codex,
        );
        assert_eq!(t["type"], "text");
        assert!(
            t["text"].as_str().unwrap().contains("notes.txt")
                && t["text"].as_str().unwrap().contains("fn main")
        );
        assert_eq!(
            block_for(
                &item("image", Some("image/png"), b"x"),
                ALL,
                EngineKind::Codex
            )["type"],
            "image"
        );
        for m in ["image/svg+xml", "image/bmp"] {
            assert!(
                block_for(&item("image", Some(m), b"x"), ALL, EngineKind::Codex)["text"]
                    .as_str()
                    .unwrap()
                    .contains("unsupported image")
            );
        }
        assert!(block_for(
            &item("image", Some("image/png"), b"x"),
            NONE,
            EngineKind::Claude
        )["text"]
            .as_str()
            .is_some());
        let pdf = item("resource", Some("application/pdf"), b"%PDF-1.4\n\xff");
        assert_eq!(
            block_for(&pdf, ALL, EngineKind::Claude)["resource"]["mimeType"],
            "application/pdf"
        );
        let m = block_for(&pdf, ALL, EngineKind::Codex);
        assert!(
            m["text"].as_str().unwrap().contains("not readable")
                && !m.to_string().contains("JVBER")
        );
        let bin = block_for(
            &item("resource", Some("application/zip"), b"PK\0\x01"),
            ALL,
            EngineKind::Claude,
        );
        assert!(bin["text"].as_str().unwrap().contains("sha256"));
        let invalid = block_for(
            &item("resource", None, &[0xff, 0xfe, 0x00]),
            ALL,
            EngineKind::Claude,
        );
        assert_eq!(invalid["type"], "text");
    }
    #[test]
    fn an_attachment_is_sent_once_per_session() {
        let items = [item("resource", Some("text/plain"), b"hello")];
        let (first, fresh) = prompt_blocks(&items, &HashSet::new(), ALL, EngineKind::Claude);
        assert_eq!(first[0]["type"], "resource");
        let sent: HashSet<_> = fresh.into_iter().collect();
        let (later, fresh) = prompt_blocks(&items, &sent, ALL, EngineKind::Claude);
        assert!(fresh.is_empty() && later.len() == 1 && later[0]["type"] == "text");
        assert!(!later[0].to_string().contains("hello"));
    }
}

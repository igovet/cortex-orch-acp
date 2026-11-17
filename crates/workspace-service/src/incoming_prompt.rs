//! Inbound owner content only. Prompt metadata never enlarges file-read authority.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cortex_core::input::{Block, Capture, MAX_BYTES};
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
fn invalid(code: &str) -> io::Error {
    io::Error::other(code.to_owned())
}
fn field(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}
fn unavailable(b: &mut Block, availability: &str, reason: &str) {
    b.availability = availability.into();
    b.reason = Some(reason.into());
    b.body = None;
}
fn retain(b: &mut Block, body: Vec<u8>, total: &mut usize) {
    if body.len() > MAX_BYTES {
        unavailable(b, "too_large", "block_exceeds_10mib_cap");
    } else if *total + body.len() > MAX_BYTES {
        unavailable(b, "too_large", "prompt_attachment_total_exceeds_10mib_cap");
    } else {
        *total += body.len();
        b.availability = "stored".into();
        b.body = Some(body);
    }
}
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}
fn decode(data: &str) -> Result<Vec<u8>, &'static str> {
    if data.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("block_exceeds_10mib_cap");
    }
    STANDARD.decode(data).map_err(|_| "invalid_blob_encoding")
}
fn percent_decode(raw: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bytes = raw.bytes();
    while let Some(c) = bytes.next() {
        if c == b'%' {
            let a = (bytes.next()? as char).to_digit(16)?;
            let b = (bytes.next()? as char).to_digit(16)?;
            out.push((a * 16 + b) as u8);
        } else {
            out.push(c);
        }
    }
    String::from_utf8(out).ok()
}
fn upload_root() -> Option<PathBuf> {
    let value = std::env::var_os("PASEO_HOME")?;
    let path = PathBuf::from(value);
    let path = if let Ok(s) = path.strip_prefix("~") {
        PathBuf::from(std::env::var_os("HOME")?).join(s)
    } else {
        path
    };
    if !path.is_absolute() {
        return None;
    }
    Some(path.canonicalize().ok()?.join("uploads"))
}
fn resource(b: &mut Block, root: &Path, total: &mut usize, upload: Option<&Path>) {
    let Some(uri) = b.uri.clone() else {
        unavailable(b, "inaccessible", "missing_uri");
        return;
    };
    let Some(mut path) = uri.strip_prefix("file:") else {
        b.availability = "reference_only".into();
        return;
    };
    if private_uri(&uri) {
        unavailable(b, "inaccessible", "private_path");
        return;
    }
    if let Some(rest) = path.strip_prefix("//") {
        let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if !["", "localhost"].contains(&authority) {
            unavailable(b, "inaccessible", "unsupported_file_authority");
            return;
        }
        path = &rest[authority.len()..];
        let _ = tail;
    }
    path = path.split(['?', '#']).next().unwrap_or("");
    let Some(raw) = percent_decode(path) else {
        unavailable(b, "inaccessible", "unresolvable_path");
        return;
    };
    let raw = PathBuf::from(raw);
    if !raw.is_absolute() {
        unavailable(b, "inaccessible", "non_absolute_file_uri");
        return;
    }
    let uploads = upload.filter(|p| raw.starts_with(p)).map(Path::to_owned);
    let allowed = uploads.as_deref().unwrap_or(root);
    if let Some(uploads) = &uploads {
        match (uploads.canonicalize(), raw.canonicalize()) {
            (Ok(u), Ok(p)) if u == *uploads && p.starts_with(uploads) => {}
            (_, Err(e)) if e.kind() == io::ErrorKind::NotFound => {
                unavailable(b, "inaccessible", "missing_upload");
                return;
            }
            _ => {
                unavailable(b, "inaccessible", "symlink_escape");
                return;
            }
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&raw)
    {
        Ok(f) => f,
        Err(e) => {
            unavailable(
                b,
                "inaccessible",
                if e.raw_os_error() == Some(libc::ELOOP) {
                    "symlink_path"
                } else {
                    "unreadable"
                },
            );
            return;
        }
    };
    let (info, real) = match (
        file.metadata(),
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())),
    ) {
        (Ok(i), Ok(r)) => (i, r),
        _ => {
            unavailable(b, "inaccessible", "unresolvable_path");
            return;
        }
    };
    if !real.starts_with(allowed) {
        unavailable(
            b,
            if uploads.is_some() {
                "inaccessible"
            } else {
                "reference_only"
            },
            if uploads.is_some() {
                "symlink_escape"
            } else {
                "outside_allowed_attachment_roots"
            },
        );
        return;
    }
    // The string filter cannot see a link inside the project that points at private
    // state; judge the resolved path too, with the same refusal.
    if uploads.is_none()
        && real.strip_prefix(allowed).is_ok_and(|rel| {
            rel.components()
                .any(|c| PRIVATE_COMPONENTS.iter().any(|p| c.as_os_str() == *p))
        })
    {
        unavailable(b, "inaccessible", "private_path");
        return;
    }
    if !info.is_file() {
        unavailable(b, "inaccessible", "not_a_regular_file");
        return;
    }
    if info.uid() != unsafe { libc::getuid() } {
        unavailable(b, "inaccessible", "not_owned");
        return;
    }
    if info.nlink() != 1 {
        unavailable(b, "inaccessible", "multiple_hardlinks");
        return;
    }
    let mut bytes = Vec::new();
    if file
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        unavailable(b, "inaccessible", "unreadable");
        return;
    }
    retain(b, bytes, total);
}
const PRIVATE_COMPONENTS: [&str; 5] = [
    ".git",
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
];
/// A file URI that names Git or Cortex private state is never read or retained.
fn private_uri(uri: &str) -> bool {
    let decoded = percent_decode(uri.split(['?', '#']).next().unwrap_or(uri)).unwrap_or_default();
    decoded.split('/').any(|p| PRIVATE_COMPONENTS.contains(&p))
}
pub(crate) fn observe_reference(root: &Path, uri: &str) -> (String, Option<String>) {
    // Re-resolution observes availability; it never upgrades captured user content.
    if private_uri(uri) {
        return ("inaccessible".into(), Some("private_path".into()));
    }
    let mut block = Block {
        index: 0,
        kind: "resource_link".into(),
        mime: None,
        name: None,
        uri: Some(uri.into()),
        availability: "reference_only".into(),
        reason: None,
        body: None,
    };
    resource(&mut block, root, &mut 0, upload_root().as_deref());
    (block.availability, block.reason)
}
pub fn capture(root: &Path, blocks: &Value) -> io::Result<(String, Capture)> {
    capture_with_upload(root, blocks, upload_root().as_deref())
}
fn capture_with_upload(
    root: &Path,
    blocks: &Value,
    upload: Option<&Path>,
) -> io::Result<(String, Capture)> {
    let blocks = blocks
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 256)
        .ok_or_else(|| invalid("invalid_prompt"))?;
    let mut text = String::new();
    let mut captured = Vec::new();
    let mut total = 0;
    for (index, value) in blocks.iter().enumerate() {
        let kind = value["type"]
            .as_str()
            .ok_or_else(|| invalid("invalid_prompt"))?;
        if kind == "text" {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(
                value["text"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid_prompt"))?,
            );
            continue;
        }
        let mut block = Block {
            index,
            kind: kind.into(),
            mime: field(value, "mimeType"),
            name: field(value, "name"),
            uri: field(value, "uri"),
            availability: "inaccessible".into(),
            reason: None,
            body: None,
        };
        match kind {
            "image" => {
                let supported = crate::native_attachments::base_mime(block.mime.as_deref())
                    .filter(|m| crate::native_attachments::supported_image(m));
                if !block.mime.as_ref().is_some_and(|s| s.starts_with("image/"))
                    || !value["data"].is_string()
                {
                    unavailable(&mut block, "inaccessible", "invalid_image");
                } else if supported.is_none() {
                    // SVG, BMP and the like are neither retained nor re-sent.
                    unavailable(&mut block, "inaccessible", "unsupported_image_type");
                    push_line(
                        &mut text,
                        &format!(
                            "[image: {} - unsupported type, not retained; only PNG, JPEG, GIF and WebP are accepted]",
                            block.mime.as_deref().unwrap_or("unknown")
                        ),
                    );
                    captured.push(block);
                    continue;
                } else {
                    match decode(value["data"].as_str().unwrap()) {
                        Ok(body) => retain(&mut block, body, &mut total),
                        Err(reason) => unavailable(
                            &mut block,
                            if reason == "block_exceeds_10mib_cap" {
                                "too_large"
                            } else {
                                "inaccessible"
                            },
                            reason,
                        ),
                    }
                }
                if block.availability != "stored" {
                    return Err(invalid(&format!(
                        "image_capture_rejected:{}",
                        block.reason.as_deref().unwrap_or("capture_error")
                    )));
                }
                push_line(
                    &mut text,
                    &format!("[image: {}]", block.mime.as_deref().unwrap()),
                );
            }
            "resource_link" => resource(&mut block, root, &mut total, upload),
            "resource" => {
                let r = &value["resource"];
                block.mime = field(r, "mimeType");
                block.uri = field(r, "uri");
                if let Some(s) = r["text"].as_str() {
                    retain(&mut block, s.as_bytes().to_vec(), &mut total);
                } else if let Some(s) = r["blob"].as_str() {
                    match decode(s) {
                        Ok(body) => retain(&mut block, body, &mut total),
                        Err(reason) => unavailable(
                            &mut block,
                            if reason == "block_exceeds_10mib_cap" {
                                "too_large"
                            } else {
                                "inaccessible"
                            },
                            reason,
                        ),
                    }
                } else {
                    unavailable(&mut block, "inaccessible", "no_embedded_content");
                }
            }
            _ => return Err(invalid("native_prompt_content_unavailable")),
        }
        // A reference without retained bytes is still named in the prompt.
        if block.kind != "image" && block.availability != "stored" {
            push_line(
                &mut text,
                &format!(
                    "[{}: {} ({}) - {}{}]",
                    block.kind,
                    block.name.as_deref().unwrap_or("unnamed"),
                    block.uri.as_deref().unwrap_or("no uri"),
                    block.availability,
                    block
                        .reason
                        .as_deref()
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default()
                ),
            );
        }
        captured.push(block);
    }
    if text.trim().is_empty() && !captured.is_empty() {
        text = "Attached resources".into();
    }
    if text.trim().is_empty() || text.chars().count() > 32_000 {
        return Err(invalid("invalid_prompt"));
    }
    Ok((text, Capture { blocks: captured }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cap_and_resource_refusals_are_exact() {
        let root = std::env::temp_dir().join(format!("incoming-capture-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let scope = root.join("scope");
        std::fs::create_dir(&scope).unwrap();
        std::fs::write(root.join("outside"), "outside").unwrap();
        std::fs::write(scope.join("file"), "snapshot").unwrap();
        std::os::unix::fs::symlink("file", scope.join("link")).unwrap();
        std::fs::hard_link(scope.join("file"), scope.join("hard")).unwrap();
        let uri = |name: &str| format!("file://{}", scope.join(name).display());
        let (_, capture) = capture_with_upload(
            &scope,
            &json!([
                {"type":"resource_link","uri":uri("link")},
                {"type":"resource_link","uri":uri("hard")},
                {"type":"resource_link","uri":"file://remote/file"},
                {"type":"resource_link","uri":"file:relative"},
                {"type":"resource_link","uri":format!("file://{}",root.join("outside").display())}
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            capture
                .blocks
                .iter()
                .map(|b| b.reason.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "symlink_path",
                "multiple_hardlinks",
                "unsupported_file_authority",
                "non_absolute_file_uri",
                "outside_allowed_attachment_roots"
            ]
        );
        let data = STANDARD.encode(vec![0u8; MAX_BYTES + 1]);
        assert!(super::capture_with_upload(
            &root,
            &json!([{"type":"image","mimeType":"image/png","data":data}]),
            None
        )
        .err()
        .unwrap()
        .to_string()
        .contains("block_exceeds_10mib_cap"));
        let body = vec![0u8; MAX_BYTES / 2 + 1];
        let data = STANDARD.encode(body);
        assert!(super::capture_with_upload(
            &root,
            &json!([{"type":"image","mimeType":"image/png","data":data},
            {"type":"image","mimeType":"image/png","data":data}]),
            None
        )
        .err()
        .unwrap()
        .to_string()
        .contains("prompt_attachment_total_exceeds_10mib_cap"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn links_without_bytes_are_named_and_private_or_unsupported_input_is_not_retained() {
        let root = std::env::temp_dir().join(format!("incoming-ki21-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".cortex")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".cortex/secret"), "s").unwrap();
        std::fs::write(root.join(".git/config"), "c").unwrap();
        std::fs::write(root.join("ok.txt"), "ok").unwrap();
        let uri = |n: &str| format!("file://{}", root.join(n).display());
        let (text, capture) = capture_with_upload(
            &root,
            &json!([
                {"type":"text","text":"look"},
                {"type":"resource_link","name":"s","uri":uri(".cortex/secret")},
                {"type":"resource_link","name":"g","uri":uri(".git/config")},
                {"type":"resource_link","name":"remote","uri":"https://example.invalid/x"},
                {"type":"resource_link","name":"ok","uri":uri("ok.txt")},
                {"type":"image","mimeType":"image/svg+xml","data":"PHN2Zy8+"},
                {"type":"image","mimeType":"image/bmp","data":"Qk0="},
                {"type":"image","mimeType":"image/png","data":"YWJj"}
            ]),
            None,
        )
        .unwrap();
        let reasons: Vec<_> = capture
            .blocks
            .iter()
            .map(|b| (b.availability.as_str(), b.reason.as_deref()))
            .collect();
        assert_eq!(
            reasons,
            [
                ("inaccessible", Some("private_path")),
                ("inaccessible", Some("private_path")),
                ("reference_only", None),
                ("stored", None),
                ("inaccessible", Some("unsupported_image_type")),
                ("inaccessible", Some("unsupported_image_type")),
                ("stored", None),
            ]
        );
        assert!(capture
            .blocks
            .iter()
            .filter(|b| b.availability != "stored")
            .all(|b| b.body.is_none()));
        assert!(
            text.contains("[resource_link: remote (https://example.invalid/x) - reference_only]"),
            "{text}"
        );
        assert!(
            text.contains("[resource_link: s (") && text.contains("inaccessible: private_path]"),
            "{text}"
        );
        assert!(
            text.contains("image/svg+xml - unsupported type")
                && text.contains("[image: image/png]"),
            "{text}"
        );
        assert!(
            !text.contains("(file://") || !text.contains("ok.txt"),
            "stored link is not a marker: {text}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlinked_directories_into_private_state_are_not_retained() {
        let root = std::env::temp_dir().join(format!("incoming-private-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".cortex")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".cortex/secret"), "s").unwrap();
        std::fs::write(root.join(".git/config"), "c").unwrap();
        std::os::unix::fs::symlink(".cortex", root.join("link")).unwrap();
        std::os::unix::fs::symlink(".git", root.join("glink")).unwrap();
        let uri = |n: &str| format!("file://{}", root.join(n).display());
        let (text, capture) = capture_with_upload(
            &root,
            &json!([
                {"type":"resource_link","name":"a","uri":uri("link/secret")},
                {"type":"resource_link","name":"b","uri":uri("glink/config")}
            ]),
            None,
        )
        .unwrap();
        for b in &capture.blocks {
            assert_eq!(
                (b.availability.as_str(), b.reason.as_deref(), b.body.is_none()),
                ("inaccessible", Some("private_path"), true)
            );
        }
        assert!(!text.contains("stored"));
        // Re-resolution refuses the same way.
        assert_eq!(
            observe_reference(&root, &uri("link/secret")),
            ("inaccessible".into(), Some("private_path".into()))
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

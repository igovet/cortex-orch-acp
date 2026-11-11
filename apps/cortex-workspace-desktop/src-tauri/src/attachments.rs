//! Byte snapshots of an actual native drop, never a webview-supplied path.
use base64::Engine;
use serde_json::{json, Value};
use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt};

/// A clipboard image, as the single file the page attaches.
pub fn clipboard_png(bytes: &[u8]) -> Result<Value, &'static str> {
    if bytes.len() > workspace_protocol::ATTACHMENT_BYTES { return Err("This clipboard image is over the 4 MiB limit for attachments."); }
    Ok(json!([{"name":"clipboard.png","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}]))
}

pub fn snapshot(uris: &[gtk::glib::GString]) -> Result<Value, String> {
    if uris.len() > workspace_protocol::ATTACHMENT_FILES { return Err("Attach at most 8 files.".into()); }
    let mut total = 0usize;
    let mut files = Vec::new();
    for uri in uris {
        let (path, host) = gtk::glib::filename_from_uri(uri).map_err(|_| "Drop local files only.")?;
        if host.is_some_and(|host| !host.is_empty() && host != "localhost") {
            return Err("Drop local files only.".into());
        }
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("This filename is unavailable.")?;
        if name.chars().count() > 255 || name.chars().any(|c| c.is_control() || c == '\\') {
            return Err("This filename is unavailable.".into());
        }
        // O_NOFOLLOW | O_NONBLOCK: do not follow a symlink or block opening a FIFO.
        let file = OpenOptions::new().read(true).custom_flags(0o400000 | 0o4000)
            .open(&path).map_err(|_| "This dropped file could not be opened.")?;
        if !file.metadata().map_err(|_| "This dropped file is unavailable.")?.is_file() {
            return Err("Drop regular files, not folders.".into());
        }
        let mut bytes = Vec::new();
        file.take((workspace_protocol::ATTACHMENT_BYTES - total + 1) as u64).read_to_end(&mut bytes)
            .map_err(|_| "This dropped file could not be read.")?;
        total += bytes.len();
        if total > workspace_protocol::ATTACHMENT_BYTES { return Err("Attachments must total 4 MiB or less.".into()); }
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" }
            else if bytes.starts_with(b"\xff\xd8\xff") { "image/jpeg" }
            else { "application/octet-stream" };
        files.push(json!({"name":name,"mimeType":mime,
                         "data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
    }
    Ok(json!(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_images_are_named_and_bounded() {
        assert_eq!(clipboard_png(b"png").unwrap()[0]["name"], "clipboard.png");
        assert!(clipboard_png(&vec![0; workspace_protocol::ATTACHMENT_BYTES + 1]).unwrap_err().contains("4 MiB"));
    }
    #[test]
    fn snapshots_exact_bytes_and_rejects_symlinks_directories_and_size_overflow() {
        let dir = std::env::temp_dir().join(format!("cw-drop-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, b"fixture").unwrap();
        let uri = |path: &std::path::Path| gtk::glib::filename_to_uri(path, None).unwrap();
        assert_eq!(snapshot(&[uri(&file)]).unwrap()[0]["data"], "Zml4dHVyZQ==");
        assert!(snapshot(&vec![uri(&file); 9]).is_err());
        assert!(snapshot(&[uri(&dir)]).is_err());
        let link = dir.join("linked");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(snapshot(&[uri(&link)]).is_err());
        std::fs::write(&file, vec![0u8; workspace_protocol::ATTACHMENT_BYTES]).unwrap();
        let files = snapshot(&[uri(&file)]).unwrap();
        let data = files[0]["data"].as_str().unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap().len(), workspace_protocol::ATTACHMENT_BYTES);
        std::fs::write(&file, vec![0u8; workspace_protocol::ATTACHMENT_BYTES + 1]).unwrap();
        assert!(snapshot(&[uri(&file)]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

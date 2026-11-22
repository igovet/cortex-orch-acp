//! Descriptor-relative editor writes. No symlink traversal, revision-free save or binary conversion.
use super::{native::*, settings::Lock};
use serde_json::{json, Value};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

pub const MAX_FILE: usize = 512 * 1024;
// SHA-256 is kept dependency-free here so Slice A does not change the shared lockfile.
pub fn revision(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(128);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, b) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(b.try_into().unwrap());
        }
        for i in 16..64 {
            let a = w[i - 15];
            let b = w[i - 2];
            w[i] = w[i - 16]
                .wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
                .wrapping_add(w[i - 7])
                .wrapping_add(b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10));
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut z] = h;
        for i in 0..64 {
            let t = z
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let u = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            z = g;
            g = f;
            f = e;
            e = d.wrapping_add(t);
            d = c;
            c = b;
            b = a;
            a = t.wrapping_add(u);
        }
        for (h, v) in h.iter_mut().zip([a, b, c, d, e, f, g, z]) {
            *h = h.wrapping_add(v);
        }
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}
fn cstring(name: &str) -> Result<CString> {
    CString::new(name).map_err(|_| refuse("invalid_path", "Invalid path."))
}
fn open_at(folder: &File, name: &str, flags: i32, mode: u32) -> Result<File> {
    let name = cstring(name)?;
    // The parent is held open; O_NOFOLLOW applies to the final component.
    let fd = unsafe {
        libc::openat(
            folder.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(match e.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => refuse(
                "symlink_refused",
                "This path goes through a link. Open the file at its real location.",
            ),
            Some(libc::ENOENT) => refuse("file_missing", "This file is not in the checkout."),
            _ => e.into(),
        });
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn parent(checkout: &Path, name: &str) -> Result<(File, String)> {
    relative(name)?;
    if name.split('/').any(|p| RESERVED.contains(&p)) {
        return Err(refuse("invalid_path", "Private helper path."));
    }
    let mut parts = name.split('/').collect::<Vec<_>>();
    let name = parts.pop().unwrap().to_owned();
    let mut folder = File::open(checkout)?;
    for part in parts {
        folder = open_at(&folder, part, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|e| {
            if e.code == "file_missing" {
                refuse(
                    "folder_missing",
                    "The folder of this file is gone. Your text is kept here.",
                )
            } else {
                e
            }
        })?;
    }
    Ok((folder, name))
}
/// One folder of the checkout (`""` is the checkout itself), held open by descriptor and
/// reached without following any link, with the same private-name refusal as reads and saves.
pub(super) fn open_folder(checkout: &Path, dir: &str) -> Result<File> {
    let mut folder = File::open(checkout)?;
    if dir.is_empty() {
        return Ok(folder);
    }
    relative(dir)?;
    if dir.split('/').any(|p| RESERVED.contains(&p)) {
        return Err(refuse("invalid_path", "Private helper path."));
    }
    for part in dir.split('/') {
        folder = open_at(&folder, part, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|e| {
            if e.code == "file_missing" {
                refuse(
                    "folder_missing",
                    "This folder is no longer in the checkout.",
                )
            } else {
                e
            }
        })?;
    }
    Ok(folder)
}
fn held(folder: &File, name: &str) -> Result<Option<(Vec<u8>, u32)>> {
    let file = match open_at(folder, name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
        Ok(f) => f,
        Err(e) if e.code == "file_missing" => return Ok(None),
        Err(e) => return Err(e),
    };
    let info = file.metadata()?;
    if !info.is_file() {
        return Err(refuse(
            "not_a_file",
            "Only regular files can be edited here.",
        ));
    }
    if info.len() > MAX_FILE as u64 {
        return Err(refuse(
            "file_too_large",
            "This file is too large to edit here.",
        ));
    }
    let mut data = Vec::new();
    file.take(MAX_FILE as u64 + 1).read_to_end(&mut data)?;
    if data.len() > MAX_FILE {
        return Err(refuse(
            "file_too_large",
            "This file is too large to edit here.",
        ));
    }
    Ok(Some((data, info.mode() & 0o7777)))
}
/// Bounded, descriptor-relative observation of the current working copy.
pub(super) fn checkout_digest(checkout: &Path, name: &str) -> Result<Option<String>> {
    let (folder, leaf) = match parent(checkout, name) {
        Err(e) if e.code == "folder_missing" => return Ok(None),
        other => other?,
    };
    let open = || open_at(&folder, &leaf, libc::O_RDONLY | libc::O_NONBLOCK, 0);
    let mut file = match open() {
        Err(e) if e.code == "file_missing" => return Ok(None),
        other => other?,
    };
    let info = file.metadata()?;
    if !info.is_file() || info.nlink() != 1 || info.len() > MAX_FILE as u64 {
        return Err(refuse(
            "evidence_unavailable",
            "Recorded file cannot be observed.",
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)?;
    let stamp = |m: &fs::Metadata| (m.dev(), m.ino(), m.ctime(), m.ctime_nsec(), m.len());
    if bytes.len() as u64 != info.len() || stamp(&open()?.metadata()?) != stamp(&info) {
        return Err(refuse(
            "evidence_unavailable",
            "Recorded file changed during observation.",
        ));
    }
    Ok(Some(revision(&bytes)))
}

fn inside(checkout: &Path, name: &str) -> Result<PathBuf> {
    relative(name)?;
    let base = fs::canonicalize(checkout)?;
    let path = checkout.join(name);
    let resolved = match fs::canonicalize(&path) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // An absent final file is still a valid read; canonicalize its existing parent.
            let parent = fs::canonicalize(path.parent().unwrap())?;
            parent.join(path.file_name().unwrap())
        }
        Err(e) => return Err(e.into()),
    };
    if resolved == base || !resolved.starts_with(&base) {
        return Err(refuse(
            "invalid_path",
            "That path is not inside this checkout.",
        ));
    }
    if resolved
        .strip_prefix(&base)
        .unwrap()
        .components()
        .any(|p| RESERVED.contains(&p.as_os_str().to_str().unwrap_or("")))
    {
        return Err(refuse("invalid_path", "Private helper path."));
    }
    Ok(resolved)
}
pub fn locate(checkout: &Path, p: &Value) -> Result<Value> {
    if p["file"].is_null() {
        return Ok(json!({"root":checkout,"path":null}));
    }
    let path = inside(checkout, string(p, "file")?)?;
    if !path.is_file() {
        return Err(refuse("file_missing", "This file is not in the checkout."));
    }
    Ok(json!({"root":checkout,"path":path}))
}
/// The largest picture, video or sound shown inline. Its base64 reply stays well inside one frame.
pub const MAX_MEDIA: usize = 4 * 1024 * 1024;
/// The only file types whose bytes are sent for inline viewing, chosen by name and never by content.
pub fn media_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        _ => return None,
    })
}
/// `media: true` reads one picture, video or sound as base64 for inline viewing. It is never edited or saved.
fn read_media(path: &Path, name: &str) -> Result<Value> {
    let mime = media_type(name)
        .ok_or_else(|| refuse("not_media", "This file type cannot be shown here."))?;
    let mut reply = json!({"file":name,"text":null,"revision":null});
    if fs::metadata(path)?.len() > MAX_MEDIA as u64 {
        reply["reason"] = json!("This file is too large to show here.");
        return Ok(reply);
    }
    let mut data = Vec::new();
    File::open(path)?
        .take(MAX_MEDIA as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > MAX_MEDIA {
        reply["reason"] = json!("This file is too large to show here.");
        return Ok(reply);
    }
    use base64::Engine;
    reply["revision"] = json!(revision(&data));
    reply["size"] = json!(data.len());
    reply["media"] =
        json!({"mime":mime,"data":base64::engine::general_purpose::STANDARD.encode(&data)});
    Ok(reply)
}
pub fn read(checkout: &Path, p: &Value) -> Result<Value> {
    let name = string(p, "file")?;
    let path = inside(checkout, name)?;
    if p["media"] == json!(true) && path.is_file() {
        return read_media(&path, name);
    }
    let mut reply = json!({"file":name,"text":null,"revision":null});
    let reason = if !path.is_file() {
        Some("This file is not in the checkout.")
    } else if fs::metadata(&path)?.len() > MAX_FILE as u64 {
        Some("This file is too large to show here.")
    } else {
        None
    };
    if let Some(reason) = reason {
        reply["reason"] = json!(reason);
        return Ok(reply);
    }
    let mut data = Vec::new();
    File::open(&path)?
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > MAX_FILE {
        reply["reason"] = json!("This file is too large to show here.");
        return Ok(reply);
    }
    if data.contains(&0) {
        reply["reason"] = json!("This is a binary file.");
        return Ok(reply);
    }
    let read_only = match parent(checkout, name).and_then(|(folder, name)| held(&folder, &name)) {
        Err(e) => Some(e.message),
        _ if std::str::from_utf8(&data).is_err() => {
            Some("This file is not UTF-8 text, so saving it here would change its bytes.".into())
        }
        _ => None,
    };
    Ok(
        json!({"file":name,"text":String::from_utf8_lossy(&data),"revision":revision(&data),
        "size":data.len(),"readOnly":read_only,"writer":null}),
    )
}
fn conflict(current: Option<&(Vec<u8>, u32)>) -> Value {
    json!({"outcome":"conflict","revision":current.map(|(d,_)|revision(d))})
}
pub fn save(checkout: &Path, state: &Path, p: &Value) -> Result<Value> {
    save_with(checkout, state, p, || Ok(()))
}
fn save_with(
    checkout: &Path,
    state: &Path,
    p: &Value,
    before_replace: impl FnOnce() -> std::io::Result<()>,
) -> Result<Value> {
    save_locked_with(checkout, p, before_replace, || {
        Lock::new(state, "machine.lock").map(Some)
    })
}
pub(super) fn save_locked(checkout: &Path, p: &Value, _lock: &Lock) -> Result<Value> {
    save_locked_with(checkout, p, || Ok(()), || Ok(None))
}
fn save_locked_with(
    checkout: &Path,
    p: &Value,
    before_replace: impl FnOnce() -> std::io::Result<()>,
    acquire_lock: impl FnOnce() -> Result<Option<Lock>>,
) -> Result<Value> {
    let text = string(p, "text")?;
    let base = p
        .get("revision")
        .filter(|v| v.is_null() || v.is_string())
        .ok_or_else(|| refuse("invalid_request", "This save has no base revision."))?;
    if text.len() > MAX_FILE {
        return Err(refuse(
            "file_too_large",
            "This file is too large to save here.",
        ));
    }
    if text.contains('\0') {
        return Err(refuse("binary_file", "This text holds a NUL byte."));
    }
    let _lock = acquire_lock()?;
    let (folder, name) = parent(checkout, string(p, "file")?)?;
    let current = held(&folder, &name)?;
    if let Some((data, _)) = &current {
        if data.contains(&0) {
            return Err(refuse("binary_file", "This is a binary file."));
        }
    }
    if json!(current.as_ref().map(|(d, _)| revision(d))) != *base {
        return Ok(conflict(current.as_ref()));
    }
    if let Some((data, _)) = &current {
        if std::str::from_utf8(data).is_err() {
            return Err(refuse(
                "not_text",
                "This file is not UTF-8 text, so saving it here would change its bytes.",
            ));
        }
        if data == text.as_bytes() {
            return Ok(json!({"outcome":"saved","revision":base,"size":data.len()}));
        }
    }
    let temporary = format!(".cw-save-{}.tmp", super::settings::unique());
    let temp_name = cstring(&temporary)?;
    let dest_name = cstring(&name)?;
    let result = (|| {
        let mut target = open_at(
            &folder,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?;
        target.write_all(text.as_bytes())?;
        target.set_permissions(fs::Permissions::from_mode(
            current.as_ref().map_or(0o644, |(_, m)| *m),
        ))?;
        target.sync_all()?;
        before_replace()?;
        let found = held(&folder, &name)?;
        if found.as_ref().map(|(d, _)| d) != current.as_ref().map(|(d, _)| d) {
            return Ok(conflict(found.as_ref()));
        }
        let rc = unsafe {
            if current.is_none() {
                libc::linkat(
                    folder.as_raw_fd(),
                    temp_name.as_ptr(),
                    folder.as_raw_fd(),
                    dest_name.as_ptr(),
                    0,
                )
            } else {
                libc::renameat(
                    folder.as_raw_fd(),
                    temp_name.as_ptr(),
                    folder.as_raw_fd(),
                    dest_name.as_ptr(),
                )
            }
        };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            if current.is_none() && e.kind() == std::io::ErrorKind::AlreadyExists {
                return Ok(conflict(held(&folder, &name)?.as_ref()));
            }
            return Err(e.into());
        }
        folder.sync_all()?;
        Ok(json!({"outcome":"saved","revision":revision(text.as_bytes()),"size":text.len()}))
    })();
    unsafe { libc::unlinkat(folder.as_raw_fd(), temp_name.as_ptr(), 0) };
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_vectors() {
        assert_eq!(
            revision(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            revision(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            revision(&vec![b'a'; 1000000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    #[test]
    fn media_reads_are_typed_bounded_and_confined() {
        use base64::Engine;
        let root = std::env::temp_dir().join(format!(
            "cw-media-test-{}",
            super::super::settings::unique()
        ));
        let outside =
            std::env::temp_dir().join(format!("cw-media-out-{}", super::super::settings::unique()));
        fs::create_dir_all(root.join("img")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let png = [0x89, b'P', b'N', b'G', 0, 0, 0, 13];
        fs::write(root.join("img/a.PNG"), png).unwrap();
        let reply = read(&root, &json!({"file":"img/a.PNG","media":true})).unwrap();
        assert_eq!(reply["media"]["mime"], "image/png");
        assert_eq!(reply["text"], Value::Null);
        assert_eq!(reply["size"], 8);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(reply["media"]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
        // The ordinary read is unchanged: no bytes, only why there is no text.
        let plain = read(&root, &json!({"file":"img/a.PNG"})).unwrap();
        assert!(plain.get("media").is_none());
        assert_eq!(plain["reason"], "This is a binary file.");
        // Only listed media types; a text or unknown file is refused, never sent as bytes.
        fs::write(root.join("notes.txt"), b"hi").unwrap();
        fs::write(root.join("doc.pdf"), b"%PDF").unwrap();
        for f in ["notes.txt", "doc.pdf"] {
            assert_eq!(
                read(&root, &json!({"file":f,"media":true}))
                    .unwrap_err()
                    .code,
                "not_media"
            );
        }
        // Bounded: one byte over the limit has no bytes; the limit itself fits one frame.
        let big = fs::File::create(root.join("v.mp4")).unwrap();
        big.set_len(MAX_MEDIA as u64 + 1).unwrap();
        let reply = read(&root, &json!({"file":"v.mp4","media":true})).unwrap();
        assert!(reply.get("media").is_none());
        assert_eq!(reply["reason"], "This file is too large to show here.");
        big.set_len(MAX_MEDIA as u64).unwrap();
        let reply = read(&root, &json!({"file":"v.mp4","media":true})).unwrap();
        assert_eq!(reply["media"]["mime"], "video/mp4");
        assert!(serde_json::to_vec(&reply).unwrap().len() < super::super::native::FRAME_BUDGET);
        // Confined to the checkout by the same guards as text reads.
        fs::write(outside.join("x.png"), png).unwrap();
        std::os::unix::fs::symlink(outside.join("x.png"), root.join("link.png")).unwrap();
        for f in ["link.png", "../x.png", "/etc/x.png", ".git/x.png"] {
            assert!(read(&root, &json!({"file":f,"media":true})).is_err(), "{f}");
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn interrupted_and_late_save_leave_current_bytes() {
        let root =
            std::env::temp_dir().join(format!("cw-file-test-{}", super::super::settings::unique()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let file = root.join("a");
        fs::write(&file, b"old").unwrap();
        let p = json!({"file":"a","text":"new","revision":revision(b"old")});
        assert!(save_with(&root, &root, &p, || Err(std::io::Error::other("injected"))).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"old");
        let reply = save_with(&root, &root, &p, || fs::write(&file, b"worker")).unwrap();
        assert_eq!(reply["outcome"], "conflict");
        assert_eq!(fs::read(&file).unwrap(), b"worker");
        assert!(!fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".cw-save-")));
        fs::remove_dir_all(root).unwrap();
    }
}

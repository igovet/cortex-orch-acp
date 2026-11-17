//! Compare the installed regular files, symlinks and directories with the archive itself;
//! no `md5sums` metadata is assumed. A small tar reader streams `dpkg-deb --fsys-tarfile`.

use crate::fsutil::sha256_hex;
use crate::json::Res;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Result {
    pub files: i64,
    pub links: i64,
    pub mismatches: Vec<String>,
}

fn octal(field: &[u8]) -> Res<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut v = (field[0] & 0x7f) as u64;
        for byte in &field[1..] {
            v = v.checked_mul(256).ok_or("tar size overflow")? + *byte as u64;
        }
        return Ok(v);
    }
    let text: String = field.iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| "bad tar number".to_string())
}

fn cstr(field: &[u8]) -> String {
    String::from_utf8_lossy(&field[..field.iter().position(|b| *b == 0).unwrap_or(field.len())]).into_owned()
}

fn pax(data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let Some(space) = data[at..].iter().position(|b| *b == b' ') else { break };
        let Ok(len) = String::from_utf8_lossy(&data[at..at + space]).parse::<usize>() else { break };
        if len == 0 || at + len > data.len() {
            break;
        }
        let record = String::from_utf8_lossy(&data[at + space + 1..at + len - 1]).into_owned();
        if let Some((k, v)) = record.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
        at += len;
    }
    out
}

fn safe(name: &str) -> Res<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("unsafe archive member".into());
    }
    Ok(path.to_path_buf())
}

pub fn verify(archive: &Path, root: &Path) -> Res<Result> {
    let mut child = Command::new("dpkg-deb")
        .arg("--fsys-tarfile")
        .arg(archive)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("dpkg-deb: {e}"))?;
    let mut stream = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut result = Result { files: 0, links: 0, mismatches: Vec::new() };
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    let mut pax_name: Option<String> = None;
    let mut pax_link: Option<String> = None;
    let outcome = (|| -> Res<()> {
        loop {
            let mut header = [0u8; 512];
            let mut got = 0;
            while got < 512 {
                let k = stream.read(&mut header[got..]).map_err(|e| e.to_string())?;
                if k == 0 {
                    break;
                }
                got += k;
            }
            if got == 0 || header.iter().all(|b| *b == 0) {
                return Ok(());
            }
            if got < 512 {
                return Err("truncated tar header".into());
            }
            let size = octal(&header[124..136])?;
            let kind = header[156];
            let padded = size.div_ceil(512) * 512;
            let ustar = &header[257..262] == b"ustar";
            let mut name = cstr(&header[0..100]);
            if ustar {
                let prefix = cstr(&header[345..500]);
                if !prefix.is_empty() {
                    name = format!("{prefix}/{name}");
                }
            }
            let mut link = cstr(&header[157..257]);
            if matches!(kind, b'L' | b'K' | b'x' | b'g') {
                let mut data = vec![0u8; size as usize];
                stream.read_exact(&mut data).map_err(|e| e.to_string())?;
                let mut skip = vec![0u8; (padded - size) as usize];
                stream.read_exact(&mut skip).map_err(|e| e.to_string())?;
                match kind {
                    b'L' => long_name = Some(cstr(&data)),
                    b'K' => long_link = Some(cstr(&data)),
                    b'x' => {
                        for (k, v) in pax(&data) {
                            match k.as_str() {
                                "path" => pax_name = Some(v),
                                "linkpath" => pax_link = Some(v),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if let Some(v) = long_name.take().or(pax_name.take()) {
                name = v;
            }
            if let Some(v) = long_link.take().or(pax_link.take()) {
                link = v;
            }
            let relative = safe(&name)?;
            let target = root.join(&relative);
            match kind {
                b'0' | 0 | b'7' => {
                    result.files += 1;
                    let mut data = vec![0u8; size as usize];
                    stream.read_exact(&mut data).map_err(|e| e.to_string())?;
                    let mut skip = vec![0u8; (padded - size) as usize];
                    stream.read_exact(&mut skip).map_err(|e| e.to_string())?;
                    let expected = sha256_hex(&data);
                    let meta = std::fs::symlink_metadata(&target);
                    let ok = meta.as_ref().is_ok_and(|m| m.file_type().is_file())
                        && std::fs::read(&target).map(|d| sha256_hex(&d) == expected).unwrap_or(false);
                    if !ok {
                        result.mismatches.push(name.clone());
                    }
                }
                other => {
                    let mut skip = vec![0u8; padded as usize];
                    stream.read_exact(&mut skip).map_err(|e| e.to_string())?;
                    match other {
                        b'2' => {
                            result.links += 1;
                            if std::fs::read_link(&target).map(|t| t.to_string_lossy() != link).unwrap_or(true) {
                                result.mismatches.push(name.clone());
                            }
                        }
                        b'5' => {
                            if !target.is_dir() {
                                result.mismatches.push(name.clone());
                            }
                        }
                        b'1' => {
                            result.links += 1;
                            let other_path = root.join(safe(&link)?);
                            let same = match (std::fs::metadata(&target), std::fs::metadata(&other_path)) {
                                (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
                                _ => false,
                            };
                            if !same {
                                result.mismatches.push(name.clone());
                            }
                        }
                        _ => return Err("unsupported archive member".into()),
                    }
                }
            }
        }
    })();
    drop(stream);
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    outcome?;
    if !status.success() {
        return Err(format!("archive reader failed: {stderr}"));
    }
    Ok(result)
}

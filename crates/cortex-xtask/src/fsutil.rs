//! File and text helpers that reproduce the Python semantics the generators relied on.

use crate::json::Res;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub fn read_bytes(path: &Path) -> Res<Vec<u8>> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// `Path.read_text()`: UTF-8 with universal newlines (`\r\n` and lone `\r` become `\n`).
pub fn read_text(path: &Path) -> Res<String> {
    let text = String::from_utf8(read_bytes(path)?)
        .map_err(|_| format!("not UTF-8: {}", path.display()))?;
    Ok(universal_newlines(&text))
}

pub fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn write_text(path: &Path, content: &str) -> Res<()> {
    write_bytes(path, content.as_bytes())
}

pub fn write_bytes(path: &Path, content: &[u8]) -> Res<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::contract::hex(&Sha256::digest(bytes))
}

pub fn file_digest(path: &Path) -> Res<String> {
    Ok(sha256_hex(&read_bytes(path)?))
}

/// Python `str.isspace` for the characters that matter in prose sources.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

pub fn py_rstrip(text: &str) -> &str {
    text.trim_end_matches(is_py_space)
}

pub fn py_split_ws(text: &str) -> Vec<&str> {
    text.split(is_py_space).filter(|s| !s.is_empty()).collect()
}

/// Python `str.splitlines()` (no line ends kept).
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < bytes.len() {
        let (pos, c) = bytes[i];
        let is_break = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_break {
            lines.push(&text[start..pos]);
            if c == '\r' && i + 1 < bytes.len() && bytes[i + 1].1 == '\n' {
                i += 1;
            }
            start = if i + 1 < bytes.len() {
                bytes[i + 1].0
            } else {
                text.len()
            };
        }
        i += 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Regular files and symlinks below `root` in `os.walk(topdown, followlinks=False)` order,
/// pruning directories whose name is in `excluded`. Symlinked directories are not entered
/// and not yielded (they appear in `dirs`, never in `files`).
pub fn walk_files(root: &Path, excluded: &[&str]) -> Res<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut names: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let points_to_dir = meta.file_type().is_symlink()
                && fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            if meta.is_dir() {
                if !excluded.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if !points_to_dir {
                out.push(path);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            py_splitlines("a\nb\r\nc\rd\u{2028}e"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(py_splitlines("a\n\nb\n"), vec!["a", "", "b"]);
        assert_eq!(py_splitlines(""), Vec::<&str>::new());
        assert_eq!(py_splitlines("\n"), vec![""]);
    }

    #[test]
    fn strip_and_split() {
        assert_eq!(py_strip(" \t x \n"), "x");
        assert_eq!(py_split_ws(" a  b\tc "), vec!["a", "b", "c"]);
    }
}

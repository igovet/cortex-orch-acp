//! `project-root <directory>`: the runtime's logical project root for one
//! workspace directory, ported from `PROJECT_ROOT_PYTHON` in
//! `integrations/paseo/server/worker-history.ts`. A verified linked Git
//! worktree resolves to its main repository (which owns the one `.cortex`
//! store); any ambiguous, forged or unsafe layout exits 1 instead of guessing.
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Any refusal: Python's `Conflict`, `OSError`, `RuntimeError` or `ValueError`.
struct Conflict;

impl From<std::io::Error> for Conflict {
    fn from(_: std::io::Error) -> Self {
        Conflict
    }
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `str(Path(value))`: collapse repeated and trailing slashes and `.` parts;
/// POSIX keeps exactly two leading slashes.
fn normalize(value: &str) -> String {
    let parts: Vec<&str> = value
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let prefix = if value.starts_with("//") && !value.starts_with("///") {
        "//"
    } else if value.starts_with('/') {
        "/"
    } else {
        ""
    };
    let joined = parts.join("/");
    if prefix.is_empty() && joined.is_empty() {
        ".".into()
    } else {
        format!("{prefix}{joined}")
    }
}

fn canonical(value: &str) -> Result<PathBuf, Conflict> {
    let text = normalize(value);
    let project = PathBuf::from(&text);
    if !project.is_absolute() || !project.is_dir() {
        return Err(Conflict);
    }
    if fs::canonicalize(&project)?.as_os_str() != text.as_str() {
        return Err(Conflict);
    }
    Ok(project)
}

/// `str.splitlines()` boundaries.
fn line_break(c: char) -> bool {
    matches!(
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
    )
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if line_break(c) {
            lines.push(&text[start..at]);
            let mut end = at + c.len_utf8();
            if c == '\r' {
                if let Some(&(next, '\n')) = chars.peek() {
                    chars.next();
                    end = next + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// One small owner-private regular file, never following a symlink.
fn read_line(path: &Path) -> Result<String, Conflict> {
    let info = fs::symlink_metadata(path)?;
    if info.file_type().is_symlink() || !info.is_file() || info.uid() != uid() || info.nlink() != 1
    {
        return Err(Conflict);
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut data = Vec::new();
    file.by_ref().take(4097).read_to_end(&mut data)?;
    let text = String::from_utf8(data).map_err(|_| Conflict)?;
    let lines = split_lines(&text);
    if text.len() > 4096 || lines.len() != 1 {
        return Err(Conflict);
    }
    Ok(lines[0].to_owned())
}

fn within_uid(base: &Path, raw: &str) -> Result<PathBuf, Conflict> {
    let candidate = PathBuf::from(normalize(raw));
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    };
    let resolved = fs::canonicalize(&candidate)?;
    let mut current = PathBuf::from("/");
    let parts: Vec<_> = resolved.components().skip(1).collect();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        current.push(part);
        let info = fs::symlink_metadata(&current)?;
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(Conflict);
        }
    }
    let info = fs::symlink_metadata(&resolved)?;
    if info.file_type().is_symlink() || info.uid() != uid() {
        return Err(Conflict);
    }
    Ok(resolved)
}

fn text(path: &Path) -> Result<&str, Conflict> {
    path.to_str().ok_or(Conflict)
}

fn logical_root(value: &str) -> Result<PathBuf, Conflict> {
    let candidate = canonical(value)?;
    let marker = candidate.join(".git");
    let info = match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
        Err(_) => return Err(Conflict),
        Ok(info) => info,
    };
    if info.file_type().is_symlink() {
        return Err(Conflict);
    }
    if info.is_dir() {
        return if info.uid() == uid() {
            Ok(candidate)
        } else {
            Err(Conflict)
        };
    }
    let line = read_line(&marker)?;
    let target = line
        .strip_prefix("gitdir: ")
        .filter(|t| !t.is_empty())
        .ok_or(Conflict)?;
    let gitdir = within_uid(&candidate, target)?;
    if !gitdir.is_dir() {
        return Err(Conflict);
    }
    let common = within_uid(&gitdir, &read_line(&gitdir.join("commondir"))?)?;
    if !common.is_dir() || !common.join("HEAD").exists() || !common.join("worktrees").is_dir() {
        return Err(Conflict);
    }
    // The worktree metadata must point back to this exact ".git" file and
    // live directly under the common directory's own "worktrees/".
    if within_uid(&gitdir, &read_line(&gitdir.join("gitdir"))?)? != marker {
        return Err(Conflict);
    }
    let parent = gitdir.parent().ok_or(Conflict)?;
    if parent.file_name().and_then(|n| n.to_str()) != Some("worktrees")
        || parent.parent() != Some(common.as_path())
    {
        return Err(Conflict);
    }
    let project = within_uid(&common, "..")?;
    if within_uid(&project, ".git")? != common {
        return Err(Conflict);
    }
    canonical(text(&project)?)
}

/// Run the reader for its argv (after the subcommand): `(stdout, exit code)`.
pub fn run(argv: &[String]) -> (String, i32) {
    let resolved = argv
        .first()
        .ok_or(Conflict)
        .and_then(|value| logical_root(value))
        .and_then(|root| text(&root).map(|s| format!("{s}\n")));
    match resolved {
        Ok(line) => (line, 0),
        Err(Conflict) => (String::new(), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_pathlib() {
        assert_eq!(normalize("/a//b/./c/"), "/a/b/c");
        assert_eq!(normalize("//a"), "//a");
        assert_eq!(normalize("///a"), "/a");
        assert_eq!(normalize("a/.."), "a/..");
        assert_eq!(normalize(""), ".");
    }

    #[test]
    fn splits_lines_like_python() {
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines("\n"), vec![""]);
        assert!(split_lines("").is_empty());
        assert_eq!(split_lines("a\u{1c}b\u{2028}"), vec!["a", "b"]);
    }
}

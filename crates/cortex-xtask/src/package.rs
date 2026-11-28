//! Runtime payload enumeration, content hash, stamping and validation (`package`, `runtime-payload`).
//!
//! The content-hash definition is unchanged from the former Python implementation:
//! SHA-256 over, for each payload file in sorted relative-path order,
//! `relative \0 decimal(len(body)) \0 body`, where `package-identity.json` contributes its
//! canonical (sorted keys, compact separators, ASCII) form with `version` set to the base version.
//! What changed is the payload membership: the Python `src/` tree and the Python Workspace
//! helpers are no longer part of the shipped runtime.

use crate::fsutil::{file_digest, read_bytes, read_text, walk_files, write_text};
use crate::json::{dumps, parse, pretty_ascii, Format, Res, Value};
use crate::profiles;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const BASE: &str = env!("CARGO_PKG_VERSION");

/// What actually ships. Named explicitly: walking the repository root would fold tests, docs
/// and development scripts into the package identity.
pub const PAYLOAD_ENTRIES: &[&str] = &[
    "skills",
    "agent-sources",
    "assets",
    "policy",
    "report-templates",
    "integrations",
    "package-identity.json",
    "pinned-dependencies.json",
    "profiles.json",
    "runtime-payload.json",
    "packaging/engines",
    "packaging/voice",
    "packaging/update-signing/release-ed25519.pub",
];

const PAYLOAD_EXCLUDED_DIRS: &[&str] = &[
    ".cache",
    ".git",
    ".next",
    ".turbo",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
];
const PASEO_EXCLUDED_DIRS: &[&str] = &[
    ".cache",
    ".git",
    ".next",
    ".tmp",
    ".turbo",
    "__pycache__",
    "build",
    "cache",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "temp",
    "tmp",
];
const UNSAFE_PARTS: &[&str] = &[".git", "tests", "prototypes", "node_modules"];

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every payload file in a stable order, as (relative, path).
pub fn payload_paths(root: &Path) -> Res<Vec<(String, PathBuf)>> {
    let mut entries = Vec::new();
    for name in PAYLOAD_ENTRIES {
        let entry = root.join(name);
        if !entry.exists() {
            continue;
        }
        if entry.is_file() {
            entries.push((name.to_string(), entry));
            continue;
        }
        for path in walk_files(&entry, PAYLOAD_EXCLUDED_DIRS)? {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "pyc" || ext == "pyo" {
                continue;
            }
            if path.is_file() || is_symlink(&path) {
                entries.push((rel(root, &path), path));
            }
        }
    }
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(entries)
}

fn assert_no_symlink_tree(path: &Path) -> Res<()> {
    if !path.is_absolute() {
        return Err("payload path must be absolute".into());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if is_symlink(&current) {
            return Err("unsafe symlink in payload path".into());
        }
        if current.exists() && !current.is_dir() && current != path {
            return Err("unsafe non-directory in payload path".into());
        }
    }
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err("payload root must be a real directory".into());
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
            let dir_like = meta.is_dir() || (meta.file_type().is_symlink() && p.is_dir());
            if dir_like && PAYLOAD_EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            if meta.file_type().is_symlink() {
                return Err("unsafe symlink in payload tree".into());
            }
            if meta.is_dir() {
                stack.push(p);
            }
        }
    }
    Ok(())
}

pub fn absolute_root(root: &Path) -> Res<PathBuf> {
    std::path::absolute(root).map_err(|e| e.to_string())
}

pub fn payload_digest(root: &Path) -> Res<String> {
    let root = absolute_root(root)?;
    for name in PAYLOAD_ENTRIES {
        let entry = root.join(name);
        if entry.is_dir() {
            assert_no_symlink_tree(&entry)?;
        }
    }
    let mut digest = Sha256::new();
    for (relative, path) in payload_paths(&root)? {
        if is_symlink(&path) {
            return Err("symlink in payload".into());
        }
        let mut body = read_bytes(&path)?;
        if relative == "package-identity.json" {
            let mut value = parse(&String::from_utf8(body).map_err(|_| "identity not UTF-8")?)?;
            value.set("version", Value::str(BASE));
            body = dumps(
                &value,
                &Format {
                    sort_keys: true,
                    ensure_ascii: true,
                    indent: None,
                    compact: true,
                },
            )?
            .into_bytes();
        }
        digest.update(relative.as_bytes());
        digest.update([0u8]);
        digest.update(body.len().to_string().as_bytes());
        digest.update([0u8]);
        digest.update(&body);
    }
    Ok(crate::contract::hex(&digest.finalize()))
}

pub fn paseo_payload_digest(root: &Path) -> Res<String> {
    let mut files: Vec<(String, PathBuf)> = walk_files(root, PASEO_EXCLUDED_DIRS)?
        .into_iter()
        .map(|p| (rel(root, &p), p))
        .collect();
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        if path.file_name().and_then(|n| n.to_str()) == Some("plugin-integrity.json") {
            continue;
        }
        let body = read_bytes(&path)?;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((body.len() as u64).to_be_bytes());
        digest.update(&body);
    }
    Ok(crate::contract::hex(&digest.finalize()))
}

fn stamp_paseo_plugin(root: &Path) -> Res<()> {
    let plugin = root.join("integrations/paseo");
    if !plugin.is_dir() {
        return Ok(());
    }
    let digest = paseo_payload_digest(&plugin)?;
    let mut value = Value::obj();
    value.set("algorithm", Value::str("sha256"));
    value.set("payload_sha256", Value::str(digest));
    write_text(
        &plugin.join("plugin-integrity.json"),
        &pretty_ascii(&value)?,
    )
}

pub fn stamp(root: &Path) -> Res<String> {
    let path = root.join("package-identity.json");
    let mut value = parse(&read_text(&path)?)?;
    stamp_paseo_plugin(root)?;
    let version = format!("{BASE}+sha256.{}", &payload_digest(root)?[..16]);
    value.set("version", Value::str(version.clone()));
    write_text(&path, &pretty_ascii(&value)?)?;
    Ok(version)
}

pub fn validate(root: &Path) -> Res<String> {
    profiles::check(root).map_err(|e| format!("Agent profile generation failed: {e}"))?;
    let identity = parse(&read_text(&root.join("package-identity.json"))?)?;
    let expected = format!("{BASE}+sha256.{}", &payload_digest(root)?[..16]);
    if identity.get("name").and_then(Value::as_str) != Some("cortex")
        || identity.get("version").and_then(Value::as_str) != Some(expected.as_str())
    {
        return Err("invalid package identity or stale cache stamp".into());
    }
    let paseo = root.join("integrations/paseo");
    if paseo.is_dir() {
        let integrity = paseo.join("plugin-integrity.json");
        if !integrity.is_file() {
            return Err("missing Paseo plugin integrity stamp".into());
        }
        let value = parse(&read_text(&integrity)?)?;
        if value.get("payload_sha256").and_then(Value::as_str)
            != Some(paseo_payload_digest(&paseo)?.as_str())
        {
            return Err("stale Paseo plugin integrity stamp".into());
        }
    }
    let catalogue = parse(&read_text(&root.join("profiles.json"))?)?;
    let profile_list = catalogue
        .req("profiles")?
        .as_arr()
        .ok_or("invalid profiles")?;
    let mut skill_count = 0;
    for entry in std::fs::read_dir(root.join("skills"))
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("worker-")
            && entry.path().join("SKILL.md").exists()
        {
            skill_count += 1;
        }
    }
    let names: BTreeSet<&str> = profile_list
        .iter()
        .filter_map(|p| p.get("name").and_then(Value::as_str))
        .collect();
    let canonical: BTreeSet<_> = crate::profiles::BASE_ROLES.iter().copied().collect();
    if profile_list.len() != canonical.len() || skill_count != canonical.len() || names != canonical {
        return Err("expected the five canonical worker profiles".into());
    }
    for profile in profile_list {
        let source = profile.req("source")?.as_str().ok_or("invalid source")?;
        let stem = Path::new(source)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid source")?;
        let text = read_text(
            &root
                .join("skills")
                .join(format!("worker-{stem}"))
                .join("SKILL.md"),
        )?;
        if text.contains("## Attached worker guidance")
            || text.contains("../skills/")
            || text.contains(".codex/plugins/")
        {
            return Err(
                "profiles must not contain copied skill bundles or installation paths".into(),
            );
        }
        if !text.contains("`context-compaction`") {
            return Err("missing recovery skill requirement".into());
        }
        if ![
            "## Role and responsibility",
            "## Specialist workflow",
            "## Report class selection",
        ]
        .iter()
        .all(|s| text.contains(s))
        {
            return Err("incomplete specialist structure".into());
        }
    }
    let declared = parse(&read_text(&root.join("runtime-payload.json"))?)?;
    if !declared
        .req("files")?
        .as_arr()
        .ok_or("invalid manifest")?
        .is_empty()
    {
        return Err("runtime payload manifest must not declare Python modules".into());
    }
    forbid_python(&payload_paths(root)?)?;
    let mut templates = BTreeSet::new();
    for entry in std::fs::read_dir(root.join("report-templates"))
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("md") {
            templates.insert(p.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    let expected_templates: BTreeSet<String> = [
        "general",
        "planning",
        "investigation",
        "implementation",
        "verification",
        "documentation",
        "synthesis",
        "pipeline",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if templates != expected_templates {
        return Err("draft template set".into());
    }
    Ok(identity.req("version")?.as_str().unwrap_or("").to_string())
}

/// The shipped runtime contains no Python and no `src/` tree.
pub fn forbid_python(files: &[(String, PathBuf)]) -> Res<()> {
    let bad: Vec<&str> = files
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| {
            name.starts_with("src/")
                || name.ends_with(".py")
                || name.ends_with(".pyc")
                || name.ends_with(".pyo")
        })
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "runtime payload must contain no Python: {}",
            bad.join(", ")
        ))
    }
}

/// The complete shipped runtime file set, validated.
pub fn payload_files(root: &Path) -> Res<Vec<(String, PathBuf)>> {
    let root = absolute_root(root)?;
    for entry in PAYLOAD_ENTRIES {
        if !root.join(entry).exists() {
            return Err(format!("missing runtime payload entry: {entry}"));
        }
    }
    let files = payload_paths(&root)?;
    let identity = parse(&read_text(&root.join("package-identity.json"))?)?;
    let expected = format!("{BASE}+sha256.{}", &payload_digest(&root)?[..16]);
    if identity.get("version").and_then(Value::as_str) != Some(expected.as_str()) {
        return Err("runtime content stamp mismatch".into());
    }
    let declared = parse(&read_text(&root.join("runtime-payload.json"))?)?;
    for name in declared.req("files")?.as_arr().ok_or("invalid manifest")? {
        let name = name.as_str().ok_or("invalid manifest")?;
        if !files.iter().any(|(n, _)| n == name) {
            return Err(format!("missing runtime module: {name}"));
        }
    }
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    for (name, path) in &files {
        let resolved = path
            .canonicalize()
            .map_err(|_| format!("unsafe or missing runtime file: {name}"))?;
        if is_symlink(path)
            || !path.is_file()
            || !resolved.starts_with(&canonical_root)
            || Path::new(name)
                .components()
                .any(|c| UNSAFE_PARTS.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            return Err(format!("unsafe or missing runtime file: {name}"));
        }
    }
    forbid_python(&files)?;
    Ok(files)
}

pub fn run_stamp(root: &Path) -> Res<()> {
    println!("{}", stamp(root)?);
    Ok(())
}

pub fn run_check(root: &Path) -> Res<()> {
    println!("{}", validate(root)?);
    Ok(())
}

pub fn run_payload_list(root: &Path) -> Res<()> {
    for (name, _) in payload_files(root)? {
        println!("{name}");
    }
    Ok(())
}

#[allow(dead_code)]
pub fn digest_of(path: &Path) -> Res<String> {
    file_digest(path)
}

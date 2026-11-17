//! Offline engine and voice trees from reviewed npm locks (`bundle-engines`, `bundle-voice`).

use crate::fsutil::{file_digest, read_text, walk_files, write_text};
use crate::json::{parse, pretty_ascii, Res, Value};
use crate::proc::{chmod, mode, output, run, which};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const PACKAGES: &[(&str, &str)] = &[
    ("codex-acp", "@agentclientprotocol/codex-acp"),
    ("claude-agent-acp", "@agentclientprotocol/claude-agent-acp"),
];

fn unlocked(lock: &Value) -> bool {
    lock.req("packages").ok().and_then(Value::as_obj).map(|packages| {
        packages.iter().any(|(name, entry)| {
            !name.is_empty()
                && (!entry.get("integrity").and_then(Value::as_str).unwrap_or("").starts_with("sha512-")
                    || !entry.get("resolved").and_then(Value::as_str).unwrap_or("").starts_with("https://registry.npmjs.org/"))
        })
    }).unwrap_or(true)
}

fn dependencies_equal(actual: Option<&Value>, expected: &Value) -> bool {
    // Python dict equality: same key/value set regardless of order.
    match (actual.and_then(Value::as_obj), expected.as_obj()) {
        (Some(a), Some(e)) => a.len() == e.len() && e.iter().all(|(k, v)| a.iter().any(|(ak, av)| ak == k && av == v)),
        _ => false,
    }
}

pub fn validate_lock(root: &Path) -> Res<Value> {
    let source = root.join("packaging/engines");
    let lock = parse(&read_text(&source.join("package-lock.json"))?)?;
    let package = parse(&read_text(&source.join("package.json"))?)?;
    let pins = parse(&read_text(&root.join("pinned-dependencies.json"))?)?;
    let pins = pins.req("engines")?;
    let mut expected = Value::obj();
    for (engine, name) in PACKAGES {
        expected.set(name, pins.req(engine)?.req("pinned_package_version")?.clone());
    }
    expected.set("@openai/codex", pins.req("codex-acp")?.req("bundled_dependency_version")?.clone());
    let root_package = lock.req("packages")?.req("")?;
    if !dependencies_equal(package.get("dependencies"), &expected) || !dependencies_equal(root_package.get("dependencies"), &expected) {
        return Err("engine lock differs from pinned-dependencies.json".into());
    }
    for (name, version) in expected.as_obj().unwrap() {
        let entry = lock.req("packages")?.get(&format!("node_modules/{name}"));
        if entry.and_then(|e| e.get("version")) != Some(version) {
            return Err("engine lock version drift".into());
        }
    }
    if unlocked(&lock) {
        return Err("unlocked engine dependency".into());
    }
    Ok(lock)
}

pub fn validate_tree(tree: &Path) -> Res<Value> {
    let manifest = parse(&read_text(&tree.join("bundle-manifest.json"))?)?;
    for (relative, expected) in manifest.req("files")?.as_obj().ok_or("invalid manifest")? {
        let path = tree.join(relative);
        if !path.is_file() || Some(file_digest(&path)?.as_str()) != expected.as_str() {
            return Err(format!("incomplete engine bundle: {relative}"));
        }
    }
    for relative in manifest.req("required")?.as_arr().ok_or("invalid manifest")? {
        let relative = relative.as_str().ok_or("invalid manifest")?;
        if !tree.join(relative).is_file() {
            return Err(format!("missing required engine: {relative}"));
        }
    }
    if mode(&tree.join("node/bin/node")) & 0o111 == 0 {
        return Err("bundled Node is not executable".into());
    }
    Ok(manifest)
}

fn tree_files(output: &Path, follow_symlinked_files: bool) -> Res<Value> {
    let mut files: Vec<(Vec<String>, PathBuf)> = Vec::new();
    for path in walk_files(output, &[])? {
        let is_link = fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if (is_link && !follow_symlinked_files) || !path.is_file() {
            continue;
        }
        let parts = path.strip_prefix(output).unwrap().components().map(|c: Component| c.as_os_str().to_string_lossy().into_owned()).collect();
        files.push((parts, path));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Value::obj();
    for (parts, path) in files {
        out.set(&parts.join("/"), Value::str(file_digest(&path)?));
    }
    Ok(out)
}

fn normalize_join(base: &Path, entry: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in entry.split('/').filter(|p| !p.is_empty() && *p != ".") {
        path.push(part);
    }
    path
}

fn relative(output: &Path, path: &Path) -> String {
    path.strip_prefix(output).unwrap().to_string_lossy().into_owned()
}

fn npm_ci(output: &Path, seconds: u64) -> Res<()> {
    run(
        Command::new("npm").args(["ci", "--prefix"]).arg(output).args(["--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund"]),
        Duration::from_secs(seconds),
    )
}

pub fn build_engines(root: &Path, output: &Path) -> Res<()> {
    let lock = validate_lock(root)?;
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    for name in ["package.json", "package-lock.json"] {
        fs::copy(root.join("packaging/engines").join(name), output.join(name)).map_err(|e| e.to_string())?;
    }
    npm_ci(output, 600)?;
    let spec = parse(&read_text(&root.join("packaging/engines/node-checksums.json"))?)?;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => return Err(format!("unsupported architecture {other}")),
    };
    let version = spec.req("version")?.as_str().ok_or("invalid node spec")?.to_string();
    if output_trim(Command::new("node").arg("--version"))? != format!("v{version}") {
        return Err("builder Node differs from bundled Node".into());
    }
    let archive_name = format!("node-v{version}-linux-{arch}");
    let scratch = crate::proc::TempDir::new("node-bundle-")?;
    let archive = scratch.0.join("node.tar.xz");
    run(
        Command::new("curl").args(["-fsSL", "--proto", "=https", "--tlsv1.2", "-o"]).arg(&archive)
            .arg(format!("https://nodejs.org/dist/v{version}/{archive_name}.tar.xz")),
        Duration::from_secs(600),
    )?;
    if Some(file_digest(&archive)?.as_str()) != spec.req("sha256")?.get(arch).and_then(Value::as_str) {
        return Err("Node checksum mismatch".into());
    }
    let extracted = scratch.0.join("x");
    fs::create_dir(&extracted).map_err(|e| e.to_string())?;
    run(
        Command::new("tar").arg("-xJf").arg(&archive).arg("-C").arg(&extracted)
            .arg(format!("{archive_name}/bin/node")).arg(format!("{archive_name}/LICENSE")),
        Duration::from_secs(120),
    )?;
    for rel in ["bin/node", "LICENSE"] {
        let destination = output.join("node").join(rel);
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(extracted.join(&archive_name).join(rel), &destination).map_err(|e| e.to_string())?;
    }
    chmod(&output.join("node/bin/node"), 0o755)?;
    let builder_node = which("node").and_then(|p| p.canonicalize().ok()).ok_or("node not found")?;
    if file_digest(&output.join("node/bin/node"))? != file_digest(&builder_node)? {
        return Err("builder Node bytes differ from verified distribution".into());
    }
    let mut required = vec!["node/bin/node".to_string(), "node/LICENSE".to_string()];
    for (_, name) in PACKAGES {
        let package_dir = output.join("node_modules").join(name);
        let data = parse(&read_text(&package_dir.join("package.json"))?)?;
        let entry = match data.req("bin")? {
            Value::Str(s) => s.clone(),
            Value::Obj(o) => o.first().and_then(|(_, v)| v.as_str()).ok_or("invalid bin")?.to_string(),
            _ => return Err("invalid bin".into()),
        };
        required.push(relative(output, &package_dir.join("package.json")));
        required.push(relative(output, &normalize_join(&package_dir, &entry)));
    }
    let triple = format!("{}-unknown-linux-musl", if arch == "x64" { "x86_64" } else { "aarch64" });
    required.push(format!("node_modules/@openai/codex-linux-{arch}/vendor/{triple}/bin/codex"));
    required.push(format!("node_modules/@anthropic-ai/claude-agent-sdk-linux-{arch}/claude"));
    let mut installed = Vec::new();
    for (name, entry) in lock.req("packages")?.as_obj().ok_or("invalid lock")? {
        if name.is_empty() {
            continue;
        }
        let package_json = output.join(name).join("package.json");
        if !package_json.exists() {
            if entry.get("optional").map(Value::truthy).unwrap_or(false) {
                continue;
            }
            return Err(format!("missing dependency: {name}"));
        }
        let data = parse(&read_text(&package_json)?)?;
        if data.get("version") != entry.get("version") {
            return Err(format!("dependency version drift: {name}"));
        }
        let mut item = Value::obj();
        item.set("name", Value::str(name.rsplit("node_modules/").next().unwrap_or(name)));
        item.set("version", entry.req("version")?.clone());
        item.set("license", entry.get("license").cloned().unwrap_or(Value::str("UNDECLARED")));
        installed.push(item);
    }
    let mut manifest = Value::obj();
    manifest.set("node_version", Value::str(version));
    manifest.set("architecture", Value::str(arch));
    manifest.set("required", Value::Arr(required.into_iter().map(Value::Str).collect()));
    manifest.set("packages", Value::Arr(installed));
    manifest.set("lock_sha256", Value::str(file_digest(&output.join("package-lock.json"))?));
    manifest.set("files", tree_files(output, false)?);
    write_text(&output.join("bundle-manifest.json"), &pretty_ascii(&manifest)?)?;
    validate_tree(output)?;
    Ok(())
}

fn output_trim(command: &mut Command) -> Res<String> {
    Ok(output(command)?.trim().to_string())
}

pub fn validate_voice(tree: &Path, source: &Path) -> Res<Value> {
    let manifest = parse(&read_text(&tree.join("bundle-manifest.json"))?)?;
    if manifest.get("lock_sha256").and_then(Value::as_str) != Some(file_digest(&source.join("package-lock.json"))?.as_str()) {
        return Err("voice lock drift".into());
    }
    let files = manifest.req("files")?.as_obj().ok_or("invalid manifest")?;
    for (name, expected) in files {
        let file = tree.join(name);
        let linked = fs::symlink_metadata(&file).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if linked || !file.is_file() || Some(file_digest(&file)?.as_str()) != expected.as_str() {
            return Err("incomplete voice bundle".into());
        }
    }
    let digest_of = |name: &str| files.iter().find(|(k, _)| k == name).and_then(|(_, v)| v.as_str());
    for name in ["voice.cjs", "model.json", "package.json"] {
        if digest_of(name) != Some(file_digest(&source.join(name))?.as_str()) {
            return Err("staged voice source drift".into());
        }
    }
    for name in ["voice.cjs", "model.json", "node_modules/sherpa-onnx-node/sherpa-onnx.js"] {
        if digest_of(name).is_none() {
            return Err("missing voice runtime".into());
        }
    }
    for name in ["sherpa-onnx.node", "libonnxruntime.so", "libsherpa-onnx-c-api.so"] {
        if !files.iter().any(|(p, _)| p.ends_with(&format!("/{name}"))) {
            return Err("missing native voice runtime".into());
        }
    }
    Ok(manifest)
}

pub fn build_voice(root: &Path, output: &Path) -> Res<()> {
    let source = root.join("packaging/voice");
    let lock = parse(&read_text(&source.join("package-lock.json"))?)?;
    let mut expected = Value::obj();
    expected.set("sherpa-onnx-node", Value::str("2.0.1"));
    if !dependencies_equal(lock.req("packages")?.req("")?.get("dependencies"), &expected) {
        return Err("voice version drift".into());
    }
    if unlocked(&lock) {
        return Err("unlocked voice dependency".into());
    }
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    for name in ["package.json", "package-lock.json", "voice.cjs", "model.json", "NOTICE.md", "LICENSE-APACHE-2.0", "LICENSE-ONNXRUNTIME"] {
        fs::copy(source.join(name), output.join(name)).map_err(|e| format!("copy {name}: {e}"))?;
    }
    npm_ci(output, 180)?;
    run(Command::new("node").args(["-e", "require('sherpa-onnx-node')"]).current_dir(output), Duration::from_secs(30))?;
    let mut manifest = Value::obj();
    manifest.set("lock_sha256", Value::str(file_digest(&source.join("package-lock.json"))?));
    manifest.set("files", tree_files(output, true)?);
    write_text(&output.join("bundle-manifest.json"), &pretty_ascii(&manifest)?)?;
    validate_voice(output, &source)?;
    Ok(())
}

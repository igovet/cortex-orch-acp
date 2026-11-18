//! `check-pins`: reviewed Rust/CLI inputs, checked without building native code or using the network.
//!
//! After an intentional Cargo update, review `Cargo.lock` and update its SHA-256 in
//! `workspace.metadata.cortex-pins` alongside any changed direct dependency pin.

use crate::fsutil::{read_bytes, read_text, sha256_hex};
use crate::json::{parse as parse_json, Res, Value};
use crate::toml;
use regex::Regex;
use std::collections::BTreeSet;
use std::path::Path;

const DESKTOP: &str = "apps/cortex-workspace-desktop";
const APP: &str = "apps/cortex-workspace-desktop/src-tauri";
const DESKTOPS: [&str; 2] = [DESKTOP, "prototypes/cortex-workspace/desktop"];
const EDITOR: &str = "packages/workspace-ui";
const EDITOR_SCOPES: [&str; 2] = ["@codemirror/", "@lezer/"];

fn version_pin(text: &str) -> bool {
    Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$")
        .unwrap()
        .is_match(text)
}

fn read_toml(path: &Path) -> Res<Value> {
    toml::parse(&read_text(path)?)
}

fn read_json(path: &Path) -> Res<Value> {
    parse_json(&read_text(path)?)
}

fn dependency_tables<'a>(manifest: &'a Value, out: &mut Vec<&'a Value>) {
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(key) {
            out.push(table);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(Value::as_obj) {
        for (_, target) in targets {
            dependency_tables(target, out);
        }
    }
}

/// The editor's direct packages are exactly the reviewed versions, in the manifest and in the lock.
fn check_editor(root: &Path, errors: &mut Vec<String>) -> Res<()> {
    let reviewed = read_json(&root.join("pinned-dependencies.json"))?
        .req("workspace_editor")?
        .clone();
    let package = read_json(&root.join(EDITOR).join("package.json"))?;
    let package = package.req("dependencies")?;
    let lock = read_json(&root.join(EDITOR).join("package-lock.json"))?;
    let lock = lock.req("packages")?;
    let mut direct = Value::obj();
    for (name, version) in package.as_obj().ok_or("dependencies")? {
        if EDITOR_SCOPES.iter().any(|scope| name.starts_with(scope)) {
            direct.set(name, version.clone());
        }
    }
    let reviewed_empty = !reviewed.truthy();
    if reviewed_empty || !same_object(&direct, &reviewed) {
        errors.push(format!(
            "{EDITOR}: editor packages differ from the reviewed exact pins"
        ));
    }
    for (name, version) in direct.as_obj().unwrap_or(&[]) {
        let version = version.as_str().unwrap_or("");
        if !version_pin(version) {
            errors.push(format!(
                "{name}: editor package requires an exact x.y.z pin"
            ));
        }
        let declared = lock
            .req("")?
            .req("dependencies")?
            .get(name)
            .and_then(Value::as_str);
        let locked = lock
            .get(&format!("node_modules/{name}"))
            .and_then(|p| p.get("version"))
            .and_then(Value::as_str);
        if declared != Some(version) || locked != Some(version) {
            errors.push(format!("{name}: editor package-lock drift"));
        }
    }
    Ok(())
}

/// Python dict equality: same keys and values, order irrelevant.
fn same_object(a: &Value, b: &Value) -> bool {
    match (a.as_obj(), b.as_obj()) {
        (Some(x), Some(y)) => x.len() == y.len() && x.iter().all(|(k, v)| b.get(k) == Some(v)),
        _ => false,
    }
}

fn run_checks(root: &Path, errors: &mut Vec<String>) -> Res<()> {
    let manifest = read_toml(&root.join("Cargo.toml"))?;
    let workspace = manifest.req("workspace")?;
    let pins = workspace.req("metadata")?.req("cortex-pins")?;
    let channel = read_toml(&root.join("rust-toolchain.toml"))?
        .req("toolchain")?
        .req("channel")?
        .as_str()
        .ok_or("channel")?
        .to_string();
    if !version_pin(&channel) || Some(channel.as_str()) != pins.req("rust")?.as_str() {
        errors.push("Rust channel must be the reviewed exact stable version".into());
    }
    if workspace.req("package")?.req("rust-version")?.as_str() != Some(channel.as_str()) {
        errors.push("workspace rust-version differs from the toolchain".into());
    }
    let lock_bytes = read_bytes(&root.join("Cargo.lock"))?;
    let lock = toml::parse(
        &String::from_utf8(lock_bytes.clone()).map_err(|_| "Cargo.lock is not UTF-8")?,
    )?;
    if Some(sha256_hex(&lock_bytes).as_str()) != pins.req("lock-sha256")?.as_str() {
        errors.push("Cargo.lock digest drift; review and refresh cortex-pins.lock-sha256".into());
    }
    let mut locked = BTreeSet::new();
    for package in lock.req("package")?.as_arr().ok_or("package")? {
        locked.insert((
            package.req("name")?.as_str().ok_or("name")?.to_string(),
            package
                .req("version")?
                .as_str()
                .ok_or("version")?
                .to_string(),
        ));
    }
    let members: Vec<&str> = workspace
        .req("members")?
        .as_arr()
        .ok_or("members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let defaults: Vec<&str> = workspace
        .req("default-members")?
        .as_arr()
        .ok_or("default-members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if !members.contains(&APP) || defaults.is_empty() || defaults.contains(&APP) {
        errors.push("Tauri must be a workspace member but not a default member".into());
    }
    if defaults
        .iter()
        .any(|member| !members.contains(member) || !member.starts_with("crates/"))
    {
        errors.push("default members must explicitly select headless crates".into());
    }
    let mut manifests: Vec<(String, Value)> = Vec::new();
    let mut root_dependencies = Value::obj();
    root_dependencies.set(
        "dependencies",
        workspace
            .get("dependencies")
            .cloned()
            .unwrap_or_else(Value::obj),
    );
    manifests.push(("Cargo.toml".into(), root_dependencies));
    for member in &members {
        manifests.push((
            format!("{member}/Cargo.toml"),
            read_toml(&root.join(member).join("Cargo.toml"))?,
        ));
    }
    for (path, manifest) in &manifests {
        let mut tables = Vec::new();
        dependency_tables(manifest, &mut tables);
        for table in tables {
            for (name, spec) in table.as_obj().unwrap_or(&[]) {
                if matches!(spec, Value::Obj(_))
                    && (spec.get("workspace").is_some_and(Value::truthy)
                        || spec.get("path").is_some())
                {
                    continue;
                }
                let version = match spec {
                    Value::Str(text) => text.as_str(),
                    other => other.get("version").and_then(Value::as_str).unwrap_or(""),
                };
                let exact = version.strip_prefix('=').filter(|v| version_pin(v));
                let Some(exact) = exact else {
                    errors.push(format!("{path}: {name} requires an exact =x.y.z crate pin"));
                    continue;
                };
                let package = match spec {
                    Value::Obj(_) => spec.get("package").and_then(Value::as_str).unwrap_or(name),
                    _ => name,
                };
                if !locked.contains(&(package.to_string(), exact.to_string())) {
                    errors.push(format!(
                        "{package}: exact manifest pin missing from Cargo.lock"
                    ));
                }
                if let Some(reviewed) = pins.get(package).and_then(Value::as_str) {
                    if exact != reviewed {
                        errors.push(format!("{package}: manifest differs from reviewed pin"));
                    }
                }
            }
        }
    }
    for desktop in DESKTOPS {
        let package = read_json(&root.join(desktop).join("package.json"))?;
        let npm_lock = read_json(&root.join(desktop).join("package-lock.json"))?;
        let npm_lock = npm_lock.req("packages")?;
        let cli = package
            .req("devDependencies")?
            .req("@tauri-apps/cli")?
            .as_str()
            .ok_or("cli")?;
        if !version_pin(cli) || Some(cli) != pins.req("tauri-cli")?.as_str() {
            errors.push(format!(
                "{desktop}: Tauri CLI differs from the reviewed exact pin"
            ));
        }
        let declared = npm_lock
            .req("")?
            .req("devDependencies")?
            .req("@tauri-apps/cli")?
            .as_str();
        let installed = npm_lock
            .req("node_modules/@tauri-apps/cli")?
            .req("version")?
            .as_str();
        if declared != Some(cli) || installed != Some(cli) {
            errors.push(format!("{desktop}: Tauri CLI package-lock drift"));
        }
    }
    for package in ["tauri", "tauri-build"] {
        let reviewed = pins.req(package)?.as_str().ok_or("pin")?;
        if !locked.contains(&(package.to_string(), reviewed.to_string())) {
            errors.push(format!("{package}: reviewed pin missing from Cargo.lock"));
        }
    }
    check_editor(root, errors)
}

/// Actionable, value-free diagnostics for missing or drifting inputs.
pub fn check(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    if run_checks(root, &mut errors).is_err() {
        errors.push("missing or malformed pin input".into());
    }
    errors
}

pub fn run(root: &Path) -> Res<i32> {
    let errors = check(root);
    for error in &errors {
        eprintln!("{error}");
    }
    if errors.is_empty() {
        println!("Workspace pins OK");
    }
    Ok(i32::from(!errors.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn checkout() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    /// A complete, otherwise valid copy of every pin input.
    fn fixture() -> crate::proc::TempDir {
        let root = checkout();
        let dir = crate::proc::TempDir::new("cw-pin-check-").unwrap();
        let workspace = read_toml(&root.join("Cargo.toml")).unwrap();
        let mut files: Vec<String> = [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "pinned-dependencies.json",
        ]
        .map(String::from)
        .to_vec();
        for member in workspace
            .req("workspace")
            .unwrap()
            .req("members")
            .unwrap()
            .as_arr()
            .unwrap()
        {
            files.push(format!("{}/Cargo.toml", member.as_str().unwrap()));
        }
        for desktop in DESKTOPS {
            files.extend(["package.json", "package-lock.json"].map(|n| format!("{desktop}/{n}")));
        }
        files.extend(["package.json", "package-lock.json"].map(|n| format!("{EDITOR}/{n}")));
        for name in files {
            let out = dir.0.join(&name);
            fs::create_dir_all(out.parent().unwrap()).unwrap();
            fs::copy(root.join(&name), out).unwrap();
        }
        dir
    }

    fn drift(dir: &Path, name: &str, old: &str, new: &str, diagnostic: &str) {
        let path = dir.join(name);
        let original = fs::read_to_string(&path).unwrap();
        assert!(original.contains(old), "{name} lacks {old}");
        fs::write(&path, original.replacen(old, new, 1)).unwrap();
        assert!(
            check(dir).iter().any(|e| e.contains(diagnostic)),
            "{name}: no {diagnostic}"
        );
        fs::write(&path, original).unwrap();
        assert_eq!(check(dir), Vec::<String>::new());
    }

    #[test]
    fn this_checkout_pins_are_consistent() {
        assert_eq!(check(&checkout()), Vec::<String>::new());
    }

    #[test]
    fn production_pin_drift_is_detected_in_a_complete_fixture() {
        let dir = fixture();
        assert_eq!(check(&dir.0), Vec::<String>::new());
        drift(
            &dir.0,
            &format!("{APP}/Cargo.toml"),
            "=2.10.1",
            "^2.10.1",
            "exact =x.y.z",
        );
        drift(
            &dir.0,
            &format!("{DESKTOP}/package.json"),
            "\"@tauri-apps/cli\": \"2.10.1\"",
            "\"@tauri-apps/cli\": \"2.10.2\"",
            "reviewed exact pin",
        );
        let lock = dir.0.join(DESKTOP).join("package-lock.json");
        let original = fs::read_to_string(&lock).unwrap();
        let start = original.find("\"node_modules/@tauri-apps/cli\"").unwrap();
        let version = original[start..].find("\"version\": \"2.10.1\"").unwrap() + start;
        let mut drifted = original.clone();
        drifted.replace_range(version..version + 19, "\"version\": \"2.10.0\"");
        fs::write(&lock, drifted).unwrap();
        assert!(check(&dir.0)
            .iter()
            .any(|e| e.contains("package-lock drift")));
        fs::write(&lock, original).unwrap();
        assert_eq!(check(&dir.0), Vec::<String>::new());
        drift(
            &dir.0,
            "Cargo.toml",
            "default-members = [\"crates/workspace-protocol\"]",
            &format!("default-members = [\"{APP}\"]"),
            "not a default member",
        );
    }
}

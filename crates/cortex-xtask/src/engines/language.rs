//! Reviewed upstream content for local Code; no package lifecycle scripts.
use super::*;

pub(super) const PINS: &str = "packaging/engines/lsp-pins.json";
const LICENSES: &[&str] = &["LICENSE-MIT", "LICENSE-APACHE"];
const NPM: &[(&str, &str, &str, &str)] = &[
    (
        "typescript-language-server",
        "lib/cli.mjs",
        "LICENSE",
        "Apache-2.0",
    ),
    ("typescript", "lib/tsserver.js", "LICENSE.txt", "Apache-2.0"),
];
const RUST: &str = "lsp/rust-analyzer";

fn pin<'a>(entry: &'a Value, key: &str) -> Res<&'a str> {
    entry
        .req(key)?
        .as_str()
        .ok_or_else(|| format!("invalid Code language pin: {key}"))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn spec(root: &Path) -> Res<Value> {
    let spec = parse(&read_text(&root.join(PINS))?)?;
    let rust = spec.req("rustAnalyzer")?;
    if !regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
        .unwrap()
        .is_match(pin(rust, "version")?)
        || !["x64", "arm64"]
            .into_iter()
            .all(|arch| pin(rust.get("sha256").unwrap_or(&Value::Null), arch).is_ok_and(digest))
        || !LICENSES
            .iter()
            .all(|name| pin(rust.get("licenses").unwrap_or(&Value::Null), name).is_ok_and(digest))
    {
        return Err("invalid rust-analyzer pin".into());
    }
    let version = regex::Regex::new(r"^\d+\.\d+\.\d+$").unwrap();
    for (name, _, _, _) in NPM {
        let npm = spec.req("npm")?.req(name)?;
        if !version.is_match(pin(npm, "version")?) || !digest(pin(npm, "sha256")?) {
            return Err(format!("invalid Code language pin: {name}"));
        }
    }
    Ok(spec)
}

fn fetch(url: &str, path: &Path, expected: &str) -> Res<()> {
    run(
        Command::new("curl")
            .args([
                "-fsSL",
                "--proto",
                "=https",
                "--tlsv1.2",
                "--max-filesize",
                "67108864",
                "-o",
            ])
            .arg(path)
            .arg(url),
        Duration::from_secs(180),
    )?;
    if file_digest(path)? != expected {
        return Err(format!("Code language archive checksum mismatch: {url}"));
    }
    Ok(())
}

fn required() -> Vec<String> {
    let mut files = vec![RUST.into()];
    files.extend(
        LICENSES
            .iter()
            .map(|name| format!("lsp/rust-analyzer-licenses/{name}")),
    );
    for (name, entry, license, _) in NPM {
        files.extend([
            format!("lsp/{name}/{entry}"),
            format!("lsp/{name}/package.json"),
            format!("lsp/{name}/{license}"),
        ]);
    }
    files
}

pub(super) fn stage(root: &Path, output: &Path, arch: &str) -> Res<(Vec<String>, Vec<Value>)> {
    let spec = spec(root)?;
    let temp = crate::proc::TempDir::new("code-language-")?;
    let archive = temp.0.join("archive");
    let license_dir = output.join("lsp/rust-analyzer-licenses");
    fs::create_dir_all(&license_dir).map_err(|e| e.to_string())?;
    let rust = spec.req("rustAnalyzer")?;
    let version = pin(rust, "version")?;
    let triple = match arch {
        "x64" => "x86_64-unknown-linux-gnu",
        "arm64" => "aarch64-unknown-linux-gnu",
        _ => return Err("unsupported Code language architecture".into()),
    };
    fetch(
        &format!("https://github.com/rust-lang/rust-analyzer/releases/download/{version}/rust-analyzer-{triple}.gz"),
        &archive, pin(rust.req("sha256")?, arch)?,
    )?;
    let binary = output.join(RUST);
    let stdout = fs::File::create(&binary).map_err(|e| e.to_string())?;
    run(
        Command::new("gzip").arg("-dc").arg(&archive).stdout(stdout),
        Duration::from_secs(60),
    )?;
    chmod(&binary, 0o755)?;
    for name in LICENSES {
        fetch(
            &format!("https://raw.githubusercontent.com/rust-lang/rust-analyzer/{version}/{name}"),
            &license_dir.join(name),
            pin(rust.req("licenses")?, name)?,
        )?;
    }
    let mut packages = vec![package("rust-analyzer", version, "MIT OR Apache-2.0")];
    for (name, _, _, license) in NPM {
        let entry = spec.req("npm")?.req(name)?;
        let version = pin(entry, "version")?;
        fetch(
            &format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz"),
            &archive,
            pin(entry, "sha256")?,
        )?;
        let destination = output.join("lsp").join(name);
        fs::create_dir(&destination).map_err(|e| e.to_string())?;
        // Only hash-verified upstream tarballs reach extraction.
        run(
            Command::new("tar")
                .args(["-xzf"])
                .arg(&archive)
                .args([
                    "--no-same-owner",
                    "--no-same-permissions",
                    "--strip-components=1",
                    "-C",
                ])
                .arg(&destination),
            Duration::from_secs(60),
        )?;
        packages.push(package(name, version, license));
    }
    verify_files(output, &spec)?;
    Ok((required(), packages))
}

fn package(name: &str, version: &str, license: &str) -> Value {
    let mut item = Value::obj();
    item.set("name", Value::str(name));
    item.set("version", Value::str(version));
    item.set("license", Value::str(license));
    item
}

fn verify_files(tree: &Path, spec: &Value) -> Res<()> {
    for entry in required() {
        if !tree
            .join(&entry)
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_file())
        {
            return Err(format!("missing Code language artifact: {entry}"));
        }
    }
    if mode(&tree.join(RUST)) & 0o111 == 0 {
        return Err("bundled rust-analyzer is not executable".into());
    }
    for name in LICENSES {
        if file_digest(&tree.join("lsp/rust-analyzer-licenses").join(name))?
            != pin(spec.req("rustAnalyzer")?.req("licenses")?, name)?
        {
            return Err("bundled rust-analyzer license drift".into());
        }
    }
    for (name, _, _, _) in NPM {
        let pkg = parse(&read_text(
            &tree.join("lsp").join(name).join("package.json"),
        )?)?;
        let pinned = spec.req("npm")?.req(name)?;
        if pin(&pkg, "name")? != *name || pin(&pkg, "version")? != pin(pinned, "version")? {
            return Err(format!("bundled Code language version drift: {name}"));
        }
    }
    Ok(())
}

pub(crate) fn validate(root: &Path, tree: &Path, manifest: &Value) -> Res<()> {
    if pin(manifest, "lsp_pins_sha256")? != file_digest(&root.join(PINS))? {
        return Err("staged Code language pin drift".into());
    }
    let files = manifest.req("files")?;
    if !required()
        .iter()
        .all(|name| pin(files, name).is_ok_and(digest))
    {
        return Err("Code language artifacts missing from bundle integrity manifest".into());
    }
    verify_files(tree, &spec(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_language_pins_require_all_platforms_and_bundle_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pins = spec(&root).unwrap();
        assert!(digest(
            pin(
                pins.req("rustAnalyzer").unwrap().req("sha256").unwrap(),
                "x64"
            )
            .unwrap()
        ));
        assert!(required().contains(&"lsp/typescript/lib/tsserver.js".to_string()));
        let empty = crate::proc::TempDir::new("missing-code-bundle-").unwrap();
        let mut manifest = Value::obj();
        manifest.set(
            "lsp_pins_sha256",
            Value::str(file_digest(&root.join(PINS)).unwrap()),
        );
        manifest.set("files", Value::obj());
        assert!(validate(&root, &empty.0, &manifest)
            .unwrap_err()
            .contains("missing from bundle"));
    }
}

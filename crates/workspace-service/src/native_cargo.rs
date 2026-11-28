//! Evidence for a single Cargo test target, observed from Cargo's own build output.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Ordinary,
    Observe,
    Rebuild,
}

#[derive(Clone)]
pub(crate) struct Selection {
    package: String,
    target: Option<String>,
    filter: Option<String>,
    separator: usize,
}

pub(crate) fn selection(argv: &[String]) -> Option<Selection> {
    let [program, command, rest @ ..] = argv else {
        return None;
    };
    let pinned = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo/bin/cargo"));
    if program != "cargo" && pinned.as_deref() != Some(Path::new(program)) || command != "test" {
        return None;
    }
    let name = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    };
    let mut locked = false;
    let mut offline = false;
    let mut package = None;
    let mut target = None;
    let mut filter = None;
    let mut words = rest.iter().peekable();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--" => break,
            "--locked" if !locked => locked = true,
            "--offline" if !offline => offline = true,
            "-p" | "--package" if package.is_none() => package = Some(words.next()?.as_str()),
            "--lib" if target.is_none() => target = Some(None),
            "--test" if target.is_none() => target = Some(Some(words.next()?.as_str())),
            other if name(other) && filter.is_none() => filter = Some(other.to_owned()),
            _ => return None,
        }
    }
    let package = package.filter(|package| name(package))?;
    let target = target?;
    if !locked || !offline || target.is_some_and(|name_value| !name(name_value)) {
        return None;
    }
    let separator = argv
        .iter()
        .position(|word| word == "--")
        .unwrap_or(argv.len());
    Some(Selection {
        package: package.to_owned(),
        target: target.map(str::to_owned),
        filter,
        separator,
    })
}

impl Selection {
    pub(crate) fn instrumented(&self, original: &[String]) -> Vec<String> {
        let mut argv = original.to_vec();
        argv.insert(self.separator, "--message-format=json".to_owned());
        argv.insert(self.separator, "--verbose".to_owned());
        argv
    }
    pub(crate) fn build_only(&self, original: &[String]) -> Vec<String> {
        let mut argv = original[..self.separator].to_vec();
        argv.push("--message-format=json".into());
        argv.push("--no-run".into());
        argv
    }
}

pub(crate) fn artifact(
    receipt: &Value,
    checkout: &Path,
    target_dir: &Path,
    selection: &Selection,
) -> Option<Value> {
    let observed = &receipt["observation"];
    if observed["complete"] != true
        || observed["output_truncated"] != false
        || !observed["failure"].is_null()
    {
        return None;
    }
    let mut records = observed["stdout"]
        .as_str()?
        .lines()
        .map(serde_json::from_str::<Value>);
    let mut found = None;
    let mut finished = false;
    for raw in records.by_ref() {
        let record = raw.ok()?;
        match record["reason"].as_str() {
            Some("build-finished") => {
                finished = record["success"] == true;
                break;
            }
            Some("compiler-artifact") if record["profile"]["test"] == true => {
                let kinds = record["target"]["kind"].as_array()?;
                let matches_target = match selection.target.as_deref() {
                    Some(name) => {
                        record["target"]["name"] == name && kinds.iter().any(|kind| kind == "test")
                    }
                    None => kinds.iter().any(|kind| kind == "lib"),
                };
                if matches_target {
                    let manifest = Path::new(record["manifest_path"].as_str()?);
                    let executable = Path::new(record["executable"].as_str()?);
                    let (package_path, fragment) = record["package_id"]
                        .as_str()?
                        .strip_prefix("path+file://")?
                        .rsplit_once('#')?;
                    let package_name =
                        fragment.split_once('@').map(|(name, _)| name).or_else(|| {
                            Path::new(package_path)
                                .file_name()
                                .and_then(|part| part.to_str())
                        })?;
                    if found.is_some()
                        || package_name != selection.package
                        || !manifest.is_absolute()
                        || !manifest.starts_with(checkout)
                        || manifest.parent()? != Path::new(package_path)
                    {
                        return None;
                    }
                    let (binary_sha256, file_identity) = binary_identity(target_dir, executable)?;
                    found = Some(json!({
                        "fresh":record["fresh"].as_bool()?,
                        "package":selection.package,
                        "target":selection.target,
                        "binary_path":executable.strip_prefix(target_dir).ok()?.to_str()?,
                        "binary_sha256":binary_sha256,
                        "file_identity":file_identity,
                    }));
                }
            }
            _ => {}
        }
    }
    finished.then_some(found).flatten()
}

// Cargo's default harness launch identifies the selected artifact. A custom
// runner or truncated output is not a source-to-executable observation.
pub(crate) fn launched_harness(
    receipt: &Value,
    target: &Path,
    artifact: &Value,
    argv: &[String],
    selection: &Selection,
) -> bool {
    let Some(relative) = artifact["binary_path"].as_str() else {
        return false;
    };
    let path = target.join(relative);
    let mut expected = path.to_string_lossy().into_owned();
    for argument in selection
        .filter
        .iter()
        .chain(argv.get(selection.separator + 1..).unwrap_or(&[]).iter())
    {
        if argument.is_empty()
            || !argument
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:=/".contains(&byte))
        {
            return false;
        }
        expected.push(' ');
        expected.push_str(argument);
    }
    let stderr = receipt["observation"]["stderr"].as_str().unwrap_or("");
    let mut running = stderr
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("Running "));
    running
        .next()
        .and_then(|line| line.strip_prefix("Running `"))
        .and_then(|line| line.strip_suffix('`'))
        == Some(expected.as_str())
        && running.next().is_none()
}

/// A finite, stable read of the exact artifact Cargo reported.
fn binary_identity(directory: &Path, executable: &Path) -> Option<(String, String)> {
    const LIMIT: u64 = 256 * 1024 * 1024;
    let relative = executable.strip_prefix(directory).ok()?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    let root = crate::native_project::open_project_root(directory.parent()?).ok()?;
    let targets = crate::native_project::openat(
        &root,
        directory.file_name()?.to_str()?,
        libc::O_RDONLY | libc::O_DIRECTORY,
    )
    .ok()?;
    let fd = crate::native_project::openat(&targets, relative.to_str()?, libc::O_RDONLY).ok()?;
    let before = fd.metadata().ok()?;
    if !before.is_file() || before.len() > LIMIT {
        return None;
    }
    let mut hash = Sha256::new();
    let mut file = (&fd).take(LIMIT + 1);
    let mut buffer = [0; 32 * 1024];
    let mut read = 0;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        read += count as u64;
    }
    let after = fd.metadata().ok()?;
    let identity = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let previous = identity(&before);
    (read == before.len() && previous == identity(&after)).then(|| {
        (
            format!("{:x}", hash.finalize()),
            format!("{:x}", Sha256::digest(format!("{previous:?}").as_bytes())),
        )
    })
}
pub(crate) fn is_pinned_cargo(command: &std::process::Command) -> bool {
    use std::ffi::OsStr;
    let Some(pinned) =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo/bin/cargo"))
    else {
        return false;
    };
    if command.get_program() != OsStr::new("cargo") {
        return command.get_program() == pinned.as_os_str() && pinned.is_file();
    }
    let search = command
        .get_envs()
        .find_map(|(key, value)| (key == "PATH").then_some(value).flatten());
    search
        .and_then(|path| {
            std::env::split_paths(path)
                .map(|folder| folder.join("cargo"))
                .find(|candidate| {
                    candidate
                        .metadata()
                        .is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
                })
        })
        .is_some_and(|resolved| resolved == pinned)
}

/// A fingerprint of the selected build only. Unknown or changed artifact bytes
/// cannot be used to claim that Cargo selected a binary from an older checkout.
pub(crate) fn stale(current: &Value, previous: &Value) -> bool {
    current["fresh"] == true
        && previous["fresh"] == false
        && same_artifact(current, previous)
        && current["source_digest"]
            .as_str()
            .zip(previous["source_digest"].as_str())
            .is_some_and(|(now, then)| now.len() == 64 && then.len() == 64 && now != then)
}

pub(crate) fn same_artifact(current: &Value, previous: &Value) -> bool {
    current["package"] == previous["package"]
        && current["target"] == previous["target"]
        && current["binary_path"] == previous["binary_path"]
        && current["file_identity"].is_string()
        && current["file_identity"] == previous["file_identity"]
        && current["binary_sha256"]
            .as_str()
            .is_some_and(|s| s.len() == 64 && previous["binary_sha256"] == s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[tokio::test]
    async fn real_cargo_reuses_a_bound_stale_binary_and_a_private_rebuild_reads_dirty_source() {
        fixture(false).await;
    }

    #[tokio::test]
    #[ignore = "requires the owner's pinned Cargo installation and real verifier sandbox"]
    async fn verifier_sandbox_checks_the_same_artifact_and_uses_a_private_target() {
        fixture(true).await;
    }

    async fn fixture(check_sandbox: bool) {
        let root = std::env::temp_dir().join(format!(
            "cortex-cargo-provenance-{}",
            crate::native_effects::identity().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("/usr/bin/git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(output.status.success());
            output.stdout
        };
        git(&["init", "-q"]);
        fs::write(root.join(".gitignore"), "/target/\n/private-target/\n").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let source = root.join("src/lib.rs");
        fs::write(&source, "#[test] fn case() { assert_eq!(1, 2); }\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "fixture",
        ]);
        let head = git(&["rev-parse", "HEAD"]);
        let argv: Vec<String> = [
            "cargo",
            "test",
            "--locked",
            "--offline",
            "-p",
            "fixture",
            "--lib",
            "case",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let selection = selection(&argv).unwrap();
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let run = |directory: &str| {
            let args = selection.instrumented(&argv);
            let target = root.join(directory);
            let output = Command::new(&cargo)
                .current_dir(&root)
                .args(&args[1..])
                .env("CARGO_TARGET_DIR", &target)
                .output()
                .unwrap();
            let receipt = json!({"observation":{"complete":true,"exit_code":output.status.code(),
                "output_truncated":false,"failure":null,"stdout":String::from_utf8(output.stdout).unwrap()}});
            (
                output.status.success(),
                artifact(&receipt, &root, &target, &selection).unwrap(),
            )
        };
        let old_digest = crate::native_effects::observe_digest(root.clone())
            .await
            .unwrap();
        let (passed, mut prior) = run("target");
        assert!(!passed);
        assert_eq!(prior["fresh"], false);
        prior["source_digest"] = json!(old_digest);
        let (_, mut current) = run("target");
        current["source_digest"] = prior["source_digest"].clone();
        assert_eq!(current["fresh"], true);
        assert!(!stale(&current, &prior));
        let mtime = source.metadata().unwrap().modified().unwrap();
        fs::write(&source, "#[test] fn case() { assert_eq!(2, 2); }\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        assert_eq!(git(&["rev-parse", "HEAD"]), head);
        let dirty_digest = crate::native_effects::observe_digest(root.clone())
            .await
            .unwrap();
        assert_ne!(dirty_digest, old_digest);
        let (passed, mut reused) = run("target");
        assert!(!passed, "the shared target kept the old failing executable");
        reused["source_digest"] = json!(dirty_digest);
        assert!(stale(&reused, &prior));
        assert!(!stale(&reused, &Value::Null));
        if check_sandbox {
            let mut pinned = argv.clone();
            let home = PathBuf::from(std::env::var_os("HOME").unwrap());
            assert!(home.parent() == Some(std::env::temp_dir().as_path()));
            assert!(
                home.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("cw-test-home-")
            );
            let cargo_path = home.join(".cargo/bin/cargo");
            fs::create_dir_all(cargo_path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&cargo, &cargo_path).unwrap();
            pinned[0] = cargo_path.to_str().unwrap().into();
            let configured = super::selection(&pinned).unwrap();
            let (_cancel, receiver) = tokio::sync::watch::channel(false);
            let retained = crate::native_effects::verify_cargo(
                root.clone(),
                None,
                &pinned,
                &configured,
                Mode::Observe,
                receiver.clone(),
            )
            .await
            .unwrap();
            assert_eq!(retained["observation"]["exit_code"], 101, "{retained}");
            assert_eq!(
                retained["cargo_artifact"]["binary_sha256"], reused["binary_sha256"],
                "{retained}"
            );
            assert_eq!(retained["cargo_artifact"]["fresh"], true, "{retained}");
            let rebuilt = crate::native_effects::verify_cargo(
                root.clone(),
                None,
                &pinned,
                &configured,
                Mode::Rebuild,
                receiver,
            )
            .await
            .unwrap();
            assert_eq!(rebuilt["observation"]["exit_code"], 0, "{rebuilt}");
            assert_eq!(rebuilt["observation"]["complete"], true, "{rebuilt}");
            assert_eq!(
                rebuilt["cargo_artifact"]["private_target"], true,
                "{rebuilt}"
            );
            assert_eq!(rebuilt["cargo_artifact"]["fresh"], false, "{rebuilt}");
        }
        let (passed, rebuilt) = run("private-target");
        assert!(passed);
        assert_eq!(rebuilt["fresh"], false);
        assert_ne!(rebuilt["binary_sha256"], reused["binary_sha256"]);
        fs::remove_dir_all(root).unwrap();
    }
}

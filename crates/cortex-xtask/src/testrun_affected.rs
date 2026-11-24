//! Opt-in Cargo package selection. An uncertain input must not reduce coverage.
use super::Step;
use crate::json::{self, Res};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Package {
    name: String,
    dir: PathBuf,
    path_deps: Vec<PathBuf>,
}

pub(super) struct Selection {
    direct: BTreeSet<String>,
    affected: BTreeSet<String>,
    tests_only: bool,
}

fn output(root: &Path, timeout: &Path, program: &Path, args: &[&str]) -> Res<String> {
    let out = Command::new(timeout)
        .args(["--kill-after=5", "30"])
        .arg(program)
        .args(args)
        .current_dir(root)
        // A partial clone with missing objects must fall back, not fetch.
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|_| "could not start change discovery")?;
    if !out.status.success() {
        return Err("Git or Cargo metadata command failed".into());
    }
    String::from_utf8(out.stdout).map_err(|_| "non-UTF-8 change discovery output".into())
}

pub(super) fn discover(root: &Path, timeout: &Path, base: &str) -> Res<Selection> {
    let root = root
        .canonicalize()
        .map_err(|_| "could not canonicalize suite root")?;
    let git = |args: &[&str]| output(&root, timeout, Path::new("git"), args);
    let git_root = git(&["rev-parse", "--show-toplevel"])?;
    if Path::new(git_root.trim_end_matches('\n')) != root {
        return Err("suite root is not the Git checkout root".into());
    }
    let commit = git(&[
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{base}^{{commit}}"),
    ])?;
    let merge_base = git(&["merge-base", "--all", commit.trim(), "HEAD"])?;
    let merge_base = merge_base.trim();
    if merge_base.is_empty() || merge_base.lines().count() != 1 {
        return Err("expected exactly one merge base".into());
    }
    let tracked = git(&[
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        merge_base,
        "--",
    ])?;
    let untracked = git(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    let paths: Vec<_> = tracked
        .split_terminator('\0')
        .chain(untracked.split_terminator('\0'))
        .collect();
    if paths.is_empty() {
        return Err("no changes found".into());
    }
    let cargo =
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME not set")?).join(".cargo/bin/cargo");
    let metadata = output(
        &root,
        timeout,
        &cargo,
        &[
            "metadata",
            "--format-version=1",
            "--locked",
            "--offline",
            "--no-deps",
        ],
    )?;
    select(&root, &packages(&metadata)?, &paths)
}

fn packages(metadata: &str) -> Res<Vec<Package>> {
    let json = json::parse(metadata).map_err(|_| "unreadable Cargo metadata")?;
    let members = json
        .req("workspace_members")?
        .as_arr()
        .ok_or("invalid workspace members")?;
    let mut packages = Vec::new();
    for p in json
        .req("packages")?
        .as_arr()
        .ok_or("invalid Cargo packages")?
    {
        if !members.contains(p.req("id")?) {
            continue;
        }
        let manifest = Path::new(
            p.req("manifest_path")?
                .as_str()
                .ok_or("invalid manifest path")?,
        );
        let mut path_deps = Vec::new();
        for dep in p
            .req("dependencies")?
            .as_arr()
            .ok_or("invalid dependencies")?
        {
            if let Some(path) = dep.get("path").and_then(json::Value::as_str) {
                path_deps.push(PathBuf::from(path));
            }
        }
        packages.push(Package {
            name: p
                .req("name")?
                .as_str()
                .ok_or("invalid package name")?
                .into(),
            dir: manifest.parent().ok_or("invalid package directory")?.into(),
            path_deps,
        });
    }
    if packages.len() != members.len() {
        return Err("incomplete Cargo workspace metadata".into());
    }
    Ok(packages)
}

fn select(root: &Path, packages: &[Package], paths: &[&str]) -> Res<Selection> {
    let mut direct = BTreeSet::new();
    let mut tests_only = true;
    for path in paths {
        let path = Path::new(path);
        let Some((pkg, relative)) = packages.iter().find_map(|p| {
            let dir = p.dir.strip_prefix(root).ok()?;
            if dir.parent()? != Path::new("crates") {
                return None;
            }
            path.strip_prefix(dir).ok().map(|relative| (p, relative))
        }) else {
            return Err("shared or unclassified change".into());
        };
        if relative.extension().and_then(|ext| ext.to_str()) != Some("rs")
            || !(relative.starts_with("src") || relative.starts_with("tests"))
        {
            return Err("shared or unclassified crate input".into());
        }
        // xtask generates and checks shared project inputs.
        if pkg.name == "cortex-xtask" && relative.starts_with("src") {
            return Err("shared verification tool changed".into());
        }
        tests_only &= relative.starts_with("tests");
        direct.insert(pkg.name.clone());
    }
    let mut affected = direct.clone();
    loop {
        let previous = affected.len();
        for pkg in packages {
            if packages
                .iter()
                .any(|dep| affected.contains(&dep.name) && pkg.path_deps.contains(&dep.dir))
            {
                affected.insert(pkg.name.clone());
            }
        }
        if affected.len() == previous {
            break;
        }
    }
    Ok(Selection {
        direct,
        affected,
        tests_only,
    })
}

fn test_packages(step: &Step) -> impl Iterator<Item = &str> {
    step.argv
        .windows(2)
        .filter(|pair| pair[0] == "-p")
        .map(|pair| pair[1].as_str())
}

pub(super) fn apply(steps: &mut Vec<Step>, selection: &Selection) {
    let cargo_test =
        |s: &Step| s.argv.get(1).is_some_and(|arg| arg == "test") && s.argv[0] == "cargo";
    let tested: BTreeSet<_> = steps
        .iter()
        .filter(|s| cargo_test(s))
        .flat_map(test_packages)
        .map(String::from)
        .collect();
    if !selection.direct.is_subset(&tested) {
        eprintln!("test-fast: affected package not covered by default runner; running full suite");
        return;
    }
    steps.retain_mut(|s| {
        if !cargo_test(s) {
            // Preserve the binary build for Rust fixture consumers. For source changes
            // keep every JS check, including the plugin's Rust-backed parity tests.
            return !(selection.tests_only && s.needs_modules);
        }
        let mut argv = Vec::new();
        let mut iter = s.argv.iter();
        let mut selected = false;
        while let Some(arg) = iter.next() {
            if arg == "-p" {
                let package = iter.next().expect("runner supplies a package after -p");
                if selection.affected.contains(package) {
                    argv.extend([arg.clone(), package.clone()]);
                    selected = true;
                }
            } else {
                argv.push(arg.clone());
            }
        }
        s.argv = argv;
        selected
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, dependencies: &[&str]) -> Package {
        Package {
            name: name.into(),
            dir: Path::new("/checkout/crates").join(name),
            path_deps: dependencies
                .iter()
                .map(|dep| Path::new("/checkout/crates").join(dep))
                .collect(),
        }
    }

    #[test]
    fn transitive_dependents_are_selected_without_unrelated_packages() {
        let pkgs = [
            pkg("outer", &["middle"]),
            pkg("middle", &["inner"]),
            pkg("inner", &[]),
            pkg("unrelated", &[]),
        ];
        let root = Path::new("/checkout");
        let changes = select(root, &pkgs, &["crates/inner/tests/case.rs"]).unwrap();
        assert_eq!(
            changes.affected,
            ["inner", "middle", "outer"].map(String::from).into()
        );
        let mut steps = vec![
            super::super::cargo_step("cargo", &["inner", "unrelated", "middle"], &[]),
            super::super::cargo_step("remote", &["outer"], &["--no-default-features"]),
            super::super::cargo_step("other", &["unrelated"], &[]),
            super::super::npm_step("ui", "ui", "test", 1),
        ];
        apply(&mut steps, &changes);
        assert_eq!(
            steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["cargo", "remote"]
        );
        assert_eq!(
            test_packages(&steps[0]).collect::<Vec<_>>(),
            ["inner", "middle"]
        );
        assert_eq!(steps[1].argv.last().unwrap(), "--no-default-features");
    }

    #[test]
    fn source_keeps_non_cargo_checks_and_shared_inputs_refuse_narrowing() {
        let pkgs = [pkg("inner", &[]), pkg("cortex-xtask", &[])];
        let root = Path::new("/checkout");
        let mut steps = vec![
            super::super::cargo_step("cargo", &["inner"], &[]),
            super::super::npm_step("ui", "ui", "test", 1),
        ];
        let plan = select(root, &pkgs, &["crates/inner/src/lib.rs"]).unwrap();
        apply(&mut steps, &plan);
        assert_eq!(steps.len(), 2);
        for path in [
            "Cargo.toml",
            "crates/inner/Cargo.toml",
            "shared/fixture.rs",
            "crates/missing/src/lib.rs",
            "crates/cortex-xtask/src/main.rs",
        ] {
            assert!(
                select(root, &pkgs, &["crates/inner/tests/case.rs", path]).is_err(),
                "{path}"
            );
        }
        let plan = select(root, &pkgs, &["crates/cortex-xtask/tests/test.rs"]).unwrap();
        apply(&mut steps, &plan);
        assert_eq!(
            steps.len(),
            2,
            "an unrunnable direct package must not narrow"
        );
    }
}

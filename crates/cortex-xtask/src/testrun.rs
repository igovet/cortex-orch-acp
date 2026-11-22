//! `test-fast`: the bounded, no-paid default suite.
//!
//! Every step is one bounded subprocess under GNU `timeout --kill-after=5 <seconds>`;
//! steps run one at a time, never in parallel, in the same checkout. No step reaches
//! a paid engine or the network: cargo runs `--locked --offline`, and the account
//! roots (`HOME`, `XDG_*`) point to a fresh scratch directory outside Git. Cargo and
//! rustup keep their real homes (`CARGO_HOME`, `RUSTUP_HOME`) so the pinned toolchain
//! stays usable. A missing prerequisite (cargo, npm, installed `node_modules`, GNU
//! `timeout`) is an explicit failure (exit 2), never a silent skip.

use crate::json::Res;
use crate::proc::{which, TempDir};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Per-test deadline for the Vitest suites, in milliseconds.
const TEST_MS: &str = "60000";
const BUILD_SECONDS: u64 = 900;
const CARGO_SECONDS: u64 = 900;
const NPM_SECONDS: u64 = 600;
const PASEO_SCRIPT_SECONDS: u64 = 180;

/// Headless crates; the Tauri crates need GTK and run in the `desktop` step.
const CARGO_PACKAGES: [&str; 7] = [
    "workspace-protocol",
    "cortex-core",
    "cortex-engine",
    "workspace-service",
    "cortex-acp",
    "cortex-paseo-tools",
    "cortex-xtask",
];

struct Step {
    name: String,
    dir: &'static str,
    argv: Vec<String>,
    seconds: u64,
    /// `node_modules` must exist below `dir` (an npm step).
    needs_modules: bool,
}

fn cargo_step(name: &str, packages: &[&str], extra: &[&str]) -> Step {
    let mut argv = vec![
        "cargo".to_string(),
        "test".into(),
        "--locked".into(),
        "--offline".into(),
    ];
    for package in packages {
        argv.extend(["-p".to_string(), (*package).to_string()]);
    }
    argv.extend(extra.iter().map(|s| (*s).to_string()));
    Step {
        name: name.into(),
        dir: ".",
        argv,
        seconds: CARGO_SECONDS,
        needs_modules: false,
    }
}

fn npm_step(name: &str, dir: &'static str, script: &str, seconds: u64) -> Step {
    let argv = ["npm", "--prefix", ".", "run", script, "--silent"]
        .map(String::from)
        .to_vec();
    Step {
        name: name.into(),
        dir,
        argv,
        seconds,
        needs_modules: true,
    }
}

/// These read an installed Paseo Metro bundle whose path the owner discovers and supplies
/// (docs/project/test-matrix.md); an unsupplied path is a blocker, so they are never default steps.
const INSTALLED_BUNDLE_SCRIPTS: &[&str] = &["test:installed-questions", "test:native-queue"];

fn paseo_scripts(root: &Path) -> Res<Vec<String>> {
    let text = fs::read_to_string(root.join("integrations/paseo/package.json"))
        .map_err(|e| e.to_string())?;
    let package = crate::json::parse(&text)?;
    let scripts = package.req("scripts")?.as_obj().ok_or("scripts")?;
    Ok(scripts
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| {
            name.starts_with("test:") && !INSTALLED_BUNDLE_SCRIPTS.contains(&name.as_str())
        })
        .collect())
}

fn steps(root: &Path) -> Res<Vec<Step>> {
    let mut steps = vec![
        Step {
            name: "build".into(),
            dir: ".",
            argv: [
                "cargo",
                "build",
                "--locked",
                "--offline",
                "-p",
                "cortex-acp",
                "-p",
                "cortex-paseo-tools",
                "-p",
                "workspace-service",
            ]
            .map(String::from)
            .to_vec(),
            seconds: BUILD_SECONDS,
            needs_modules: false,
        },
        cargo_step("cargo", &CARGO_PACKAGES, &[]),
        cargo_step(
            "desktop",
            &["cortex-workspace-desktop"],
            &["--no-default-features"],
        ),
        // Remote access and the phone shell's native side: loopback only, no relay and no web view.
        cargo_step(
            "remote",
            &[
                "workspace-remote",
                "workspace-client",
                "cortex-workspace-android",
            ],
            &[],
        ),
    ];
    steps.push(npm_step("ui", "packages/workspace-ui", "test", NPM_SECONDS));
    let mut ui = steps.pop().unwrap();
    // Vitest has no per-test default deadline; pass one explicitly.
    ui.argv = [
        "npx",
        "--no-install",
        "vitest",
        "run",
        &format!("--testTimeout={TEST_MS}"),
    ]
    .map(String::from)
    .to_vec();
    steps.push(ui);
    let mut desktop_ui = npm_step(
        "desktop-ui",
        "apps/cortex-workspace-desktop",
        "test",
        NPM_SECONDS,
    );
    desktop_ui.argv = [
        "npx",
        "--no-install",
        "vitest",
        "run",
        &format!("--testTimeout={TEST_MS}"),
    ]
    .map(String::from)
    .to_vec();
    steps.push(desktop_ui);
    let mut phone_ui = npm_step(
        "phone-ui",
        "apps/cortex-workspace-android",
        "test",
        NPM_SECONDS,
    );
    phone_ui.argv = [
        "npx",
        "--no-install",
        "vitest",
        "run",
        &format!("--testTimeout={TEST_MS}"),
    ]
    .map(String::from)
    .to_vec();
    steps.push(phone_ui);
    steps.push(npm_step(
        "paseo-typecheck",
        "integrations/paseo",
        "typecheck",
        NPM_SECONDS,
    ));
    for script in paseo_scripts(root)? {
        steps.push(npm_step(
            &format!("paseo-{}", script.trim_start_matches("test:")),
            "integrations/paseo",
            &script,
            PASEO_SCRIPT_SECONDS,
        ));
    }
    // `npm run X --prefix .` keeps each step bound to its own directory.
    Ok(steps)
}

pub struct Options {
    pub list: bool,
    pub only: Vec<String>,
    pub skip: Vec<String>,
}

impl Options {
    pub fn parse(raw: &[String]) -> Res<Options> {
        let mut options = Options {
            list: false,
            only: Vec::new(),
            skip: Vec::new(),
        };
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--list" => options.list = true,
                "--only" | "--skip" => {
                    let value = iter
                        .next()
                        .ok_or(format!("{arg} needs a comma-separated list of step names"))?;
                    let names = value.split(',').filter(|n| !n.is_empty()).map(String::from);
                    if arg == "--only" {
                        options.only.extend(names);
                    } else {
                        options.skip.extend(names);
                    }
                }
                other => {
                    return Err(format!(
                        "unknown option {other}; use --list, --only NAMES or --skip NAMES"
                    ))
                }
            }
        }
        Ok(options)
    }
}

fn account_environment(scratch: &Path) -> Res<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    // Keep the real toolchain homes before the account roots move.
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    env.insert(
        "CARGO_HOME".into(),
        std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}/.cargo")),
    );
    env.insert(
        "RUSTUP_HOME".into(),
        std::env::var("RUSTUP_HOME").unwrap_or_else(|_| format!("{home}/.rustup")),
    );
    for (key, name) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        let path = scratch.join(name);
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        crate::proc::chmod(&path, 0o700)?;
        env.insert(key.into(), path.display().to_string());
    }
    Ok(env)
}

/// Resolve once against the invoking PATH, before changing the test cwd or
/// augmenting the child PATH with Cargo's directory. Some distributions ship
/// GNU coreutils under the prefixed name and a different implementation as timeout.
fn gnu_timeout(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| {
        Command::new(path)
            .arg("--version")
            .output()
            .is_ok_and(|out| {
                out.status.success() && out.stdout.starts_with(b"timeout (GNU coreutils) ")
            })
    })
}

pub fn run(root: &Path, options: &Options) -> Res<i32> {
    let all = steps(root)?;
    for name in options.only.iter().chain(&options.skip) {
        if !all.iter().any(|s| &s.name == name) {
            return Err(format!("unknown step {name}; run `test-fast --list`"));
        }
    }
    let selected: Vec<&Step> = all
        .iter()
        .filter(|s| {
            (options.only.is_empty() || options.only.contains(&s.name))
                && !options.skip.contains(&s.name)
        })
        .collect();
    if options.list {
        for step in &selected {
            println!(
                "{}: (cd {}) {} [{}s]",
                step.name,
                step.dir,
                step.argv.join(" "),
                step.seconds
            );
        }
        return Ok(0);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let candidates = ["timeout", "gnutimeout"]
        .into_iter()
        .filter_map(which)
        .map(|path| cwd.join(path));
    let Some(timeout) = gnu_timeout(candidates) else {
        eprintln!("Requires GNU timeout (timeout or gnutimeout on PATH).");
        return Ok(2);
    };
    println!("GNU timeout: {}", timeout.display());
    let cargo = which("cargo").or_else(|| {
        let home = std::env::var("CARGO_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".cargo"))
            })?;
        Some(home.join("bin/cargo")).filter(|p| p.is_file())
    });
    for step in &selected {
        if step.argv[0] == "cargo" && cargo.is_none() {
            eprintln!("Requires cargo for step {}.", step.name);
            return Ok(2);
        }
        if step.argv[0] != "cargo" {
            if which(&step.argv[0]).is_none() {
                eprintln!("Requires {} for step {}.", step.argv[0], step.name);
                return Ok(2);
            }
            if step.needs_modules && !root.join(step.dir).join("node_modules").is_dir() {
                eprintln!(
                    "Step {} needs installed dependencies: run `npm ci` in {} first.",
                    step.name, step.dir
                );
                return Ok(2);
            }
        }
    }
    let scratch = TempDir::new("cortex-fast-")?;
    let account = account_environment(&scratch.0)?;
    let target = root.join("target");
    let cargo_dir = cargo
        .as_ref()
        .and_then(|c| c.parent().map(Path::to_path_buf));
    let path = std::env::var("PATH").unwrap_or_default();
    let path = match &cargo_dir {
        Some(dir) => format!("{}:{path}", dir.display()),
        None => path,
    };
    let start = Instant::now();
    let mut failed = false;
    for (index, step) in selected.iter().enumerate() {
        println!(
            "Step {}: {} ({})",
            index + 1,
            step.name,
            step.argv.join(" ")
        );
        let tick = Instant::now();
        let mut command = Command::new(&timeout);
        command
            .args(["--kill-after=5", &step.seconds.to_string()])
            .args(&step.argv)
            .current_dir(root.join(step.dir));
        command
            .envs(&account)
            .env("PATH", &path)
            .env("CARGO_TARGET_DIR", &target)
            .env("CORTEX_WORKSPACE_CHANNEL", "dev");
        // Child fixtures use the same caller-selected temporary root as the scratch account.
        command.env_remove("CORTEX_WORKSPACE_RELEASE_AUTHORIZED");
        if step.name == "cargo" || step.name == "desktop" || step.name == "remote" {
            command.env(
                "WORKSPACE_TEST_SERVICE_BINARY",
                target.join("debug/workspace-service"),
            );
        }
        let code = command
            .status()
            .map_err(|e| format!("cannot start {}: {e}", step.name))?
            .code()
            .unwrap_or(124);
        println!(
            "Step {}: exit={code}, {:.2}s",
            index + 1,
            tick.elapsed().as_secs_f64()
        );
        failed |= code != 0;
    }
    println!(
        "Ran {} steps in {:.2}s",
        selected.len(),
        start.elapsed().as_secs_f64()
    );
    Ok(i32::from(failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnu_timeout_prefers_verified_default_and_rejects_non_gnu() {
        let scratch = TempDir::new("cortex-timeout-test-").unwrap();
        let fixture = |name: &str, version: &str, code: u8| {
            let path = scratch.0.join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s\\n' '{version}'\nexit {code}\n"),
            )
            .unwrap();
            crate::proc::chmod(&path, 0o700).unwrap();
            path
        };
        let gnu = fixture("timeout", "timeout (GNU coreutils) 9.7", 0);
        let prefixed = fixture("gnutimeout", "timeout (GNU coreutils) 9.7", 0);
        let uutils = fixture("uutils", "timeout (uutils coreutils) 0.10.0", 0);
        let failed = fixture("failed", "timeout (GNU coreutils) 9.7", 1);
        assert_eq!(gnu_timeout([gnu.clone(), prefixed.clone()]), Some(gnu));
        assert_eq!(
            gnu_timeout([uutils.clone(), prefixed.clone()]),
            Some(prefixed)
        );
        assert_eq!(gnu_timeout([uutils, failed]), None);
    }
}

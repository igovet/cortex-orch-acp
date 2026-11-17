//! `shlibdeps` and `npm-ci` build helpers.

use crate::fsutil::read_text;
use crate::json::{parse, Res, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LOADER: &str = "/lib64/ld-linux-x86-64.so.2";
const CANONICAL_LOADER: &str = "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2";

/// Remove the unconditional dpkg diversion notices of Ubuntu's merged-/usr libc6 alias.
pub fn diagnostics(stderr: &str, known_alias: bool) -> (String, usize) {
    let mut stderr = stderr.to_string();
    let mut count = 0;
    if known_alias {
        for (direction, path) in [("from", LOADER.to_string()), ("to", format!("{LOADER}.usr-is-merged"))] {
            let notice = format!(
                "dpkg-shlibdeps: warning: diversions involved - output may be incorrect\n diversion by libc6 {direction}: {path}\n"
            );
            count += stderr.matches(&notice).count();
            stderr = stderr.replace(&notice, "");
        }
    }
    (stderr, count)
}

pub fn shlibdeps(target: &Path) -> Res<i32> {
    let stage = Path::new("debian/cortex-workspace");
    fs::create_dir_all(stage.join("DEBIAN")).map_err(|e| e.to_string())?;
    let binaries = stage.join("usr/bin");
    fs::create_dir_all(&binaries).map_err(|e| e.to_string())?;
    let names = ["cortex-workspace-desktop", "workspace-service"];
    for name in names {
        fs::copy(target.join("release").join(name), binaries.join(name)).map_err(|e| format!("copy {name}: {e}"))?;
    }
    let result = Command::new("dpkg-shlibdeps")
        .arg("-O")
        .args(names.iter().map(|n| format!("-e{}", binaries.join(n).display())))
        .output()
        .map_err(|e| format!("cannot start dpkg-shlibdeps: {e}"))?;
    let loader = Path::new(LOADER);
    let alias = loader.is_symlink() && loader.canonicalize().ok().as_deref() == Some(Path::new(CANONICAL_LOADER));
    let code = result.status.code().unwrap_or(1);
    let (stderr, count) = diagnostics(&String::from_utf8_lossy(&result.stderr), alias && code == 0);
    print!("{}", String::from_utf8_lossy(&result.stdout));
    eprint!("{stderr}");
    if count > 0 {
        eprintln!("Ubuntu libc6 merged-/usr alias exceptions applied: {count} (see docs/project/verification.md).");
    }
    Ok(code)
}

/// React Native 0.81.5's toolchain requires these old APIs; no compatible updates are cached offline.
const EXCEPTIONS: [(&str, &str); 3] = [("inflight", "1.0.6"), ("rimraf", "3.0.2"), ("glob", "7.2.3")];

fn notices(root: &Path) -> Res<BTreeSet<String>> {
    let lock = parse(&read_text(&root.join("integrations/paseo/package-lock.json"))?)?;
    let packages = lock.req("packages")?;
    let mut out = BTreeSet::new();
    for (name, version) in EXCEPTIONS {
        let entry = packages.req(&format!("node_modules/{name}"))?;
        if entry.get("version").and_then(Value::as_str) == Some(version) {
            let deprecated = entry.req("deprecated")?.as_str().ok_or("invalid deprecated")?;
            out.insert(format!("npm warn deprecated {name}@{version}: {deprecated}\n"));
        }
    }
    Ok(out)
}

pub fn npm_ci(root: &Path) -> Res<i32> {
    let allowed = notices(root)?;
    let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut child = Command::new("npm")
        .args(["--prefix", "integrations/paseo", "ci", "--ignore-scripts", "--no-audit", "--no-fund"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(|e| e.to_string())?)
        .stderr(writer)
        .spawn()
        .map_err(|e| format!("cannot start npm: {e}"))?;
    let mut skipped = 0;
    let mut stream = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let n = stream.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if allowed.contains(&line) {
            skipped += 1;
        } else {
            print!("{line}");
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if skipped > 0 {
        println!("Upstream React Native deprecation exceptions applied: {skipped} (see docs/project/verification.md).");
    }
    Ok(status.code().unwrap_or(1))
}

pub fn default_target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/build/target"))
}

//! Debian ordering metadata and the channel-derived self-contained `.deb` (`debian-version`,
//! `package-channel`).

use crate::engines::{validate_lock, validate_tree, validate_voice};
use crate::fsutil::{file_digest, read_text, write_text};
use crate::json::{dumps, parse, Format, Res, Value};
use crate::package::{payload_files, BASE};
use crate::proc::{chmod, copy_tree, mode, output, run, TempDir};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const ICON_SIZES: [u32; 6] = [32, 48, 64, 128, 256, 512];

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// UTC `YYYYMMDDhhmmssffffff`, a real calendar instant.
pub fn build_stamp(value: &str) -> Res<&str> {
    if value.len() != 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("build stamp must be UTC YYYYMMDDhhmmssffffff".into());
    }
    let n = |range: std::ops::Range<usize>| value[range].parse::<u32>().unwrap();
    let (year, month, day, hour, minute, second) =
        (n(0..4), n(4..6), n(6..8), n(8..10), n(10..12), n(12..14));
    if year < 1
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err("invalid UTC build stamp".into());
    }
    Ok(value)
}

pub fn debian_version(identity: &str, stamp: &str) -> Res<String> {
    let pattern = Regex::new(&format!(
        r"^{}\+sha256\.([0-9a-f]{{16}})$",
        regex::escape(BASE)
    ))
    .unwrap();
    let hash = pattern
        .captures(identity)
        .ok_or("invalid package version")?[1]
        .to_string();
    // 'z' sorts after 's' in every legacy +sha256.<hash>, even f-prefixed ones.
    Ok(format!("{BASE}+z{}.sha256.{hash}", build_stamp(stamp)?))
}

fn channel(root: &Path, name: &str) -> Res<Value> {
    let all = parse(&read_text(
        &root.join("crates/workspace-protocol/channels.json"),
    )?)?;
    Ok(all.req(name)?.clone())
}

fn field(c: &Value, key: &str) -> Res<String> {
    Ok(c.req(key)?
        .as_str()
        .ok_or(format!("invalid channel field {key}"))?
        .to_string())
}

pub fn overlay(root: &Path, name: &str) -> Res<String> {
    let c = channel(root, name)?;
    let mut value = Value::obj();
    value.set("productName", c.req("title")?.clone());
    value.set("identifier", c.req("identifier")?.clone());
    dumps(
        &value,
        &Format {
            sort_keys: false,
            ensure_ascii: true,
            indent: None,
            compact: false,
        },
    )
}

fn package_version(identity: &str, stamp: &str, name: &str) -> Res<String> {
    let mut version = debian_version(identity, stamp)?;
    if name == "release" {
        match std::env::var("CORTEX_WORKSPACE_RELEASE_AUTHORIZED").as_deref() {
            Ok("github-main-clean") => {}
            Ok("local-owner-test") => version.push_str(".local"),
            _ => return Err("release package requires explicit authorization".into()),
        }
    }
    Ok(version)
}

fn mkdir(path: &Path) -> Res<()> {
    fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))
}

/// `shutil.copyfile`: content only; the new file gets the default mode under the current umask.
fn copy(from: &Path, to: &Path) -> Res<()> {
    let mut source = fs::File::open(from).map_err(|e| format!("open {}: {e}", from.display()))?;
    let mut target = fs::File::create(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    std::io::copy(&mut source, &mut target)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}

pub struct Package<'a> {
    pub root: &'a Path,
    pub target: &'a Path,
    pub output: &'a Path,
    pub channel: &'a str,
    pub depends: Option<String>,
    pub engines: Option<PathBuf>,
    pub stamp: &'a str,
}

pub fn package(p: &Package) -> Res<()> {
    // Installed modes must not follow the builder's umask: under 0077 dpkg-deb refuses the 0700
    // control directory, and root-owned 0700 trees would be unreadable anyway.
    let previous = unsafe { libc::umask(0o022) };
    let result = build_package(p);
    unsafe { libc::umask(previous) };
    result
}

fn build_package(p: &Package) -> Res<()> {
    let root = p.root;
    let c = channel(root, p.channel)?;
    let identity = parse(&read_text(&root.join("package-identity.json"))?)?;
    let version = package_version(
        identity
            .req("version")?
            .as_str()
            .ok_or("invalid identity")?,
        p.stamp,
        p.channel,
    )?;
    let dependencies = p
        .depends
        .clone()
        .unwrap_or_else(|| "libwebkit2gtk-4.1-0, libgtk-3-0, libc6, libgcc-s1".into());
    if dependencies.contains('\n')
        || Regex::new(r"\b[\w.+-]+-dev\b|\b(?:nodejs|npm)\b")
            .unwrap()
            .is_match(&dependencies)
    {
        return Err("invalid runtime dependencies".into());
    }
    let engines = p
        .engines
        .clone()
        .unwrap_or_else(|| p.target.parent().unwrap().join("engines"));
    validate_lock(root)?;
    let manifest = validate_tree(&engines)?;
    crate::engines::language::validate(root, &engines, &manifest)?;
    if manifest.get("lock_sha256").and_then(Value::as_str)
        != Some(file_digest(&root.join("packaging/engines/package-lock.json"))?.as_str())
    {
        return Err("staged engine lock drift".into());
    }
    let voice = engines.parent().unwrap().join("voice");
    validate_voice(&voice, &root.join("packaging/voice"))?;
    mkdir(p.output)?;
    let (desktop_binary, service_binary) =
        (field(&c, "desktop_binary")?, field(&c, "service_binary")?);
    for (source, binary) in [
        ("cortex-workspace-desktop", &desktop_binary),
        ("workspace-service", &service_binary),
    ] {
        copy(
            &p.target.join("release").join(source),
            &p.output.join(binary),
        )?;
        chmod(&p.output.join(binary), 0o755)?;
    }
    let package_name = field(&c, "package")?;
    let title = field(&c, "title")?;
    let unit = field(&c, "unit")?;
    let identifier = field(&c, "identifier")?;
    let scratch = TempDir::new("channel-deb-")?;
    let stage = &scratch.0;
    chmod(stage, 0o755)?;
    let binaries = stage.join("usr/bin");
    mkdir(&binaries)?;
    for name in [&desktop_binary, &service_binary] {
        copy(&p.output.join(name), &binaries.join(name))?;
        chmod(&binaries.join(name), 0o755)?;
    }
    let applications = stage.join("usr/share/applications");
    mkdir(&applications)?;
    // GNOME matches a Wayland window by its app id, which GTK takes from the identifier, so the
    // dock shows this icon and not a generic one. X11 matches the entry by the binary's name.
    write_text(
        &applications.join(format!("{package_name}.desktop")),
        &format!(
            "[Desktop Entry]\nType=Application\nName={title}\nExec={desktop_binary}\nIcon={package_name}\nStartupWMClass={identifier}\nTerminal=false\nCategories=Development;\n"
        ),
    )?;
    for size in ICON_SIZES {
        let icons = stage.join(format!("usr/share/icons/hicolor/{size}x{size}/apps"));
        mkdir(&icons)?;
        copy(
            &root.join(format!("packaging/icons/cortex-{size}.png")),
            &icons.join(format!("{package_name}.png")),
        )?;
    }
    let units = stage.join("usr/lib/systemd/user");
    mkdir(&units)?;
    copy(
        &root.join("packaging/systemd/user").join(&unit),
        &units.join(&unit),
    )?;
    let wants = units.join("default.target.wants");
    mkdir(&wants)?;
    std::os::unix::fs::symlink(format!("../{unit}"), wants.join(&unit))
        .map_err(|e| e.to_string())?;
    let runtime = stage.join("usr/lib").join(&package_name).join("runtime");
    for (relative, source) in payload_files(root)? {
        let destination = runtime.join(&relative);
        mkdir(destination.parent().unwrap())?;
        copy(&source, &destination)?;
        chmod(
            &destination,
            if mode(&source) & 0o111 != 0 {
                0o755
            } else {
                0o644
            },
        )?;
    }
    let voiced = runtime.parent().unwrap().join("voice");
    copy_tree(&voice, &voiced, false)?;
    let bundled = runtime.parent().unwrap().join("engines");
    copy_tree(&engines, &bundled, true)?;
    normalize_modes(&voiced)?;
    normalize_modes(&bundled)?;
    let data = stage.join("usr/share").join(&package_name);
    mkdir(&data)?;
    copy(
        &root.join(crate::contract::SCHEMA_PATH),
        &data.join("schema.json"),
    )?;
    copy(
        &root.join("package-identity.json"),
        &data.join("package-identity.json"),
    )?;
    // The fonts are compiled into the desktop binary; their OFL text ships beside it, readable.
    let notices = stage.join("usr/share/doc").join(&package_name);
    mkdir(&notices)?;
    copy(
        &root.join("apps/cortex-workspace-desktop/public/licenses/fonts.html"),
        &notices.join("fonts.html"),
    )?;
    let control = stage.join("DEBIAN");
    mkdir(&control)?;
    for script in ["postinst", "prerm"] {
        let body = read_text(&root.join("packaging/debian").join(script))?.replace("@UNIT@", &unit);
        write_text(&control.join(script), &body)?;
        chmod(&control.join(script), 0o755)?;
    }
    let architecture = output(Command::new("dpkg").arg("--print-architecture"))?
        .trim()
        .to_string();
    // No scripting runtime is declared: the package is native binaries, bundled Node engines and data.
    write_text(
        &control.join("control"),
        &format!(
            "Package: {package_name}\nVersion: {version}\nArchitecture: {architecture}\nSection: devel\nPriority: optional\nMaintainer: Cortex\nDepends: {dependencies}, git, ca-certificates, systemd, dbus-user-session, pulseaudio-utils, bzip2\nDescription: {title} local desktop\n"
        ),
    )?;
    run(
        Command::new("dpkg-deb")
            .args(["--root-owner-group", "--build"])
            .arg(stage)
            .arg(
                p.output
                    .join(format!("{package_name}_{version}_{architecture}.deb")),
            )
            .env("SOURCE_DATE_EPOCH", "0"),
        Duration::from_secs(900),
    )
}

fn normalize_modes(dir: &Path) -> Res<()> {
    chmod(dir, 0o755)?;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            normalize_modes(&path)?;
        } else {
            chmod(
                &path,
                if mode(&path) & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                },
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_match_python_contract() {
        assert_eq!(
            build_stamp("20261116123456123456"),
            Ok("20261116123456123456")
        );
        assert!(build_stamp("20261316123456123456").is_err());
        assert!(build_stamp("20260230123456123456").is_err());
        assert!(build_stamp("20240229123456123456").is_ok());
        assert!(build_stamp("2026111612345612345").is_err());
        assert!(build_stamp("20261116126056123456").is_err());
    }

    #[test]
    fn version_is_base_plus_z_stamp() {
        let identity = format!("{BASE}+sha256.0123456789abcdef");
        assert_eq!(
            debian_version(&identity, "20261116123456123456").unwrap(),
            format!("{BASE}+z20261116123456123456.sha256.0123456789abcdef")
        );
        assert!(debian_version("1.0.6", "20261116123456123456").is_err());
    }
}

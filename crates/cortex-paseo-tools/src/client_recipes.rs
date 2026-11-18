//! `print-client-recipes`: non-destructive Zed Custom Agent registration helper
//! for Cortex, plus the codeg recipe text.
//!
//! Zed registers Cortex as a separate, explicitly selected custom agent through its
//! `agent_servers` setting: never an automatic activation and never routed through
//! Zed's own provider credentials. By default nothing is written: the command prints
//! the exact JSON entry to add and, when `--settings PATH` is given, a one-line state
//! summary ("would add", "would replace", "already identical"), never a diff or any
//! other key's value, so an unrelated secret-looking setting is never echoed. A real
//! write needs `--settings PATH --apply --i-consent-to-write`, targets only that path
//! (never an implicit default location, a symlinked path or a path with a symlinked
//! parent), refuses JSONC (comments or trailing commas) rather than rewrite it, writes
//! a timestamped private backup first (never overwriting a same-second backup) and
//! replaces the file atomically with its mode preserved. It never writes codeg's
//! own configuration.
use crate::py::{self, Py};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const AGENT_NAME: &str = "Cortex";

const JSONC_MANUAL: &str = "{settings} does not parse as strict JSON (Zed settings files may use
JSONC -- comments and trailing commas -- which this helper deliberately
refuses to parse-and-rewrite, since doing so could silently drop comments
or reformat unrelated content). Add the entry manually instead:

1. Open {settings} in Zed's own settings editor (or any text editor).
2. Find (or create) the top-level \"agent_servers\" object.
3. Add this entry inside it, preserving every other key:

{entry}

4. Save the file. Zed will list \"Cortex\" as a separate custom agent
   the next time you open the Agent panel; it is never auto-activated.
";

fn codeg_recipe(binary: &Path) -> String {
    format!(
        "codeg 0.30.8 Custom Agent recipe (unverified -- see docs/features/codeg-agent/index.md)
====================================================================================

codeg's own custom-agent registration path (Settings -> Agents) is documented
upstream as supporting npx/uvx/binary distribution channels. Whether it accepts an
arbitrary local command (the native binary {binary}) is NOT verified by this tool
and stays an open question in docs/features/codeg-agent/index.md. Do not assume support.

Candidate manual steps (verify against your installed codeg before relying on them):

1. Open codeg -> Settings -> Agents -> Add custom agent.
2. If codeg offers a \"binary\"/local-command channel: point it at {binary}
   with no arguments and the environment variable CORTEX_ACP_RUNTIME_ROOT={root}.
3. If codeg only offers npx/uvx/registry channels: a local binary launch is not
   directly supported; packaging a wrapper is a possible future step, not
   something this tool does.
4. codeg injects its own \"codeg-mcp\" MCP server into every hosted agent's
   session; Cortex treats any injected MCP server as untrusted, inert metadata and
   never routes Cortex's own identity through it.

This tool never writes codeg's own configuration files.
",
        binary = binary.display(),
        root = checkout_root().display()
    )
}

fn checkout_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

// ------------------------------------------------------------ JSON (indent=2)

fn escape(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python `json.dumps(value, indent=2)` (ensure_ascii, insertion order).
fn pretty(value: &Py, level: usize, out: &mut String) {
    let pad = "  ".repeat(level + 1);
    let close = "  ".repeat(level);
    match value {
        Py::List(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&pad);
                pretty(item, level + 1, out);
                out.push_str(if index + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&close);
            out.push(']');
        }
        Py::Dict(entries) if !entries.is_empty() => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                out.push_str(&pad);
                escape(key, out);
                out.push_str(": ");
                pretty(item, level + 1, out);
                out.push_str(if index + 1 < entries.len() {
                    ",\n"
                } else {
                    "\n"
                });
            }
            out.push_str(&close);
            out.push('}');
        }
        Py::Str(text) => escape(text, out),
        Py::List(_) => out.push_str("[]"),
        Py::Dict(_) => out.push_str("{}"),
        other => out.push_str(&py::dumps(other).unwrap_or_default()),
    }
}

pub(crate) fn render(value: &Py) -> String {
    let mut out = String::new();
    pretty(value, 0, &mut out);
    out
}

fn entry(binary: &Path) -> Py {
    let mut env = Py::dict();
    env.set(
        "CORTEX_ACP_RUNTIME_ROOT",
        Py::str(checkout_root().display().to_string()),
    );
    let mut agent = Py::dict();
    agent.set("type", Py::str("custom"));
    agent.set("command", Py::str(binary.display().to_string()));
    agent.set("args", Py::List(Vec::new()));
    agent.set("env", env);
    let mut wrapper = Py::dict();
    wrapper.set(AGENT_NAME, agent);
    wrapper
}

// --------------------------------------------------------------- file safety

fn refuse_if_symlinked(path: &Path) -> Result<(), String> {
    if path.is_symlink() {
        return Err(format!(
            "{} is a symlink; refusing to read or write through it. Pass the real target path explicitly instead (e.g. $(realpath {})).",
            path.display(),
            path.display()
        ));
    }
    let mut current = PathBuf::new();
    let parts: Vec<_> = path.components().collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push(part.as_os_str());
        if current.is_symlink() {
            return Err(format!(
                "{} (a parent directory of {}) is a symlink; refusing to read or write through it. Pass the real target path explicitly instead.",
                current.display(),
                path.display()
            ));
        }
    }
    Ok(())
}

/// Civil UTC date and time `YYYYMMDDTHHMMSS` for a Unix time.
fn timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

fn write_backup(path: &Path, mode: u32) -> Result<PathBuf, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = timestamp(now);
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("bad settings file name")?;
    for suffix in 0u32.. {
        let backup = path.with_file_name(if suffix == 0 {
            format!("{name}.bak.{stamp}")
        } else {
            format!("{name}.bak.{stamp}.{suffix}")
        });
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&backup)
        {
            Ok(mut file) => {
                file.write_all(&data).map_err(|e| e.to_string())?;
                fs::set_permissions(&backup, fs::Permissions::from_mode(mode))
                    .map_err(|e| e.to_string())?;
                return Ok(backup);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot write the backup: {error}")),
        }
    }
    unreachable!()
}

fn atomic_write_with_backup(path: &Path, text: &str) -> Result<(), String> {
    refuse_if_symlinked(path)?;
    let mode = if path.exists() {
        let mode = fs::metadata(path)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o7777;
        write_backup(path, mode)?;
        Some(mode)
    } else {
        None
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("bad settings file name")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let token = fs::read_to_string("/proc/sys/kernel/random/uuid").map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".{name}.{}", token.trim().replace('-', "")));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())
                .and_then(|()| file.sync_all())
        })
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ------------------------------------------------------------------ the tool

fn existing(data: &Py) -> Option<&Py> {
    data.get_opt("agent_servers")
        .and_then(|servers| servers.get_opt(AGENT_NAME))
}

fn same(a: &Py, b: &Py) -> bool {
    py::eq(a, b)
}

struct Args {
    settings: Option<PathBuf>,
    apply: bool,
    consent: bool,
    replace: bool,
    binary: Option<PathBuf>,
    codeg_only: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args {
        settings: None,
        apply: false,
        consent: false,
        replace: false,
        binary: None,
        codeg_only: false,
    };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--apply" => args.apply = true,
            "--i-consent-to-write" => args.consent = true,
            "--replace" => args.replace = true,
            "--print-codeg-recipe" => args.codeg_only = true,
            "--settings" | "--rust-binary" => {
                let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
                if arg == "--settings" {
                    args.settings = Some(PathBuf::from(value));
                } else {
                    args.binary = Some(PathBuf::from(value));
                }
            }
            "--python" | "--python3" => return Err(
                "the Python entry was removed; Cortex registers only the native cortex-acp binary"
                    .into(),
            ),
            other => return Err(format!("unrecognized argument {other:?}")),
        }
    }
    Ok(args)
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Returns the text for stdout and the exit code; diagnostics go to stderr directly.
pub fn run(argv: &[String]) -> (String, i32) {
    let args = match parse(argv) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}");
            return (String::new(), 2);
        }
    };
    let binary = args
        .binary
        .clone()
        .unwrap_or_else(|| checkout_root().join("target/debug/cortex-acp"));
    if !binary.is_absolute() {
        eprintln!("error: Rust binary must be an absolute path");
        return (String::new(), 2);
    }
    if args.codeg_only {
        return (format!("{}\n", codeg_recipe(&binary)), 0);
    }
    let (mut stdout, code) = settings_flow(&args, &binary);
    stdout.push('\n');
    stdout.push_str(&codeg_recipe(&binary));
    stdout.push('\n');
    (stdout, code)
}

fn settings_flow(args: &Args, binary: &Path) -> (String, i32) {
    let entry = entry(binary);
    let entry_text = render(&entry);
    let unbuilt = !is_executable_file(binary);
    let unbuilt_note = format!(
        "{} is not an executable file. Build it with `cargo build --locked --offline -p cortex-acp` or pass --rust-binary PATH.",
        binary.display()
    );
    if let Some(settings) = &args.settings {
        if let Err(error) = refuse_if_symlinked(settings) {
            eprintln!("error: {error}");
            return (String::new(), 2);
        }
    }
    let read = |path: &Path| -> (Option<Py>, String) {
        match fs::read_to_string(path) {
            Ok(text) => (py::loads(&text), text),
            Err(_) => (None, String::new()),
        }
    };
    if !args.apply {
        let mut out = format!("Zed agent_servers entry to add for Cortex:\n\n{entry_text}\n");
        if unbuilt {
            eprintln!("\nnote: {unbuilt_note}");
        }
        if let Some(settings) = &args.settings {
            let (data, _) = read(settings);
            if data.is_none() && settings.exists() {
                out.push_str(&format!(
                    "\n{} is not strict JSON; showing manual instructions instead:\n\n",
                    settings.display()
                ));
                out.push_str(
                    &JSONC_MANUAL
                        .replace("{settings}", &settings.display().to_string())
                        .replace("{entry}", &entry_text),
                );
                out.push('\n');
                return (out, 0);
            }
            let state = match data.as_ref().and_then(existing) {
                Some(found) if same(found, entry.get(AGENT_NAME)) => "already identical; nothing would change.",
                None => "would add a new 'Cortex' agent_servers entry.",
                Some(_) => "would replace the existing, different 'Cortex' agent_servers entry (pass --replace together with --apply to actually do this).",
            };
            out.push_str(&format!("\n{}: {state}\n", settings.display()));
        }
        return (out, 0);
    }
    let Some(settings) = &args.settings else {
        eprintln!("error: --apply requires --settings PATH.");
        return (String::new(), 2);
    };
    if !args.consent {
        eprintln!(
            "error: --apply requires --i-consent-to-write (explicit consent for a real write)."
        );
        return (String::new(), 2);
    }
    if unbuilt {
        eprintln!("error: refusing to register a missing Rust entry: {unbuilt_note}");
        return (String::new(), 2);
    }
    let (data, raw) = read(settings);
    let mut data = match data {
        None if settings.exists() => {
            eprintln!("refusing to write: {} is not strict JSON (Zed allows JSONC). Manual instructions:\n", settings.display());
            return (
                JSONC_MANUAL
                    .replace("{settings}", &settings.display().to_string())
                    .replace("{entry}", &entry_text),
                2,
            );
        }
        None => Py::dict(),
        Some(data) => data,
    };
    if !data.is_dict() {
        data = Py::dict();
    }
    let proposed = entry.get(AGENT_NAME).clone();
    if let Some(found) = existing(&data) {
        if !found.is_null() && !same(found, &proposed) && !args.replace {
            eprintln!(
                "refusing to overwrite an existing, different 'Cortex' agent_servers entry (pass --replace to overwrite):\ncurrent: {}\nproposed: {}",
                render(found),
                render(&proposed)
            );
            return (String::new(), 2);
        }
    }
    let mut servers = match data.get_opt("agent_servers") {
        Some(found) if found.is_dict() => found.clone(),
        _ => Py::dict(),
    };
    servers.set(AGENT_NAME, proposed);
    data.set("agent_servers", servers);
    let text = format!("{}\n", render(&data));
    if text == raw {
        return (
            format!(
                "{} already contains the exact Cortex entry; nothing to write.\n",
                settings.display()
            ),
            0,
        );
    }
    match atomic_write_with_backup(settings, &text) {
        Ok(()) => (
            format!(
                "wrote {} (backup created alongside it).\n",
                settings.display()
            ),
            0,
        ),
        Err(error) => {
            eprintln!("error: {error}");
            (String::new(), 2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("client-recipes-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn binary(dir: &Path) -> String {
        let path = dir.join("cortex-acp");
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[test]
    fn timestamp_is_utc_civil_time() {
        assert_eq!(timestamp(0), "19700101T000000");
        assert_eq!(timestamp(1_700_000_000), "20231114T221320");
    }

    #[test]
    fn preview_never_writes_and_never_echoes_other_keys() {
        let dir = scratch("preview");
        let settings = dir.join("settings.json");
        fs::write(
            &settings,
            "{\"secret\": \"hunter2\", \"agent_servers\": {}}",
        )
        .unwrap();
        let before = fs::read(&settings).unwrap();
        let (out, code) = run(&args(&[
            "--settings",
            settings.to_str().unwrap(),
            "--rust-binary",
            &binary(&dir),
        ]));
        assert_eq!(code, 0);
        assert!(out.contains("would add a new 'Cortex'") && !out.contains("hunter2"));
        assert_eq!(fs::read(&settings).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn apply_needs_consent_then_writes_with_a_backup_and_preserves_other_keys_and_mode() {
        let dir = scratch("apply");
        let settings = dir.join("settings.json");
        fs::write(
            &settings,
            "{\"b\": 1, \"a\": \"é\", \"agent_servers\": {\"Other\": {}}}",
        )
        .unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o640)).unwrap();
        let bin = binary(&dir);
        let base = [
            "--settings",
            settings.to_str().unwrap(),
            "--rust-binary",
            &bin,
            "--apply",
        ];
        assert_eq!(run(&args(&base)).1, 2);
        let mut consented = base.to_vec();
        consented.push("--i-consent-to-write");
        assert_eq!(run(&args(&consented)).1, 0);
        let written = fs::read_to_string(&settings).unwrap();
        assert!(
            written.find("\"b\"").unwrap() < written.find("\"a\"").unwrap(),
            "key order preserved"
        );
        assert!(
            written.contains("\\u00e9")
                && written.contains("\"Other\": {}")
                && written.contains("\"Cortex\"")
        );
        assert_eq!(
            fs::metadata(&settings).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let backups = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.bak.")
            })
            .count();
        assert_eq!(backups, 1);
        // Idempotent, and a different existing entry needs --replace.
        assert!(run(&args(&consented)).0.contains("nothing to write"));
        fs::write(
            &settings,
            "{\"agent_servers\": {\"Cortex\": {\"command\": \"/other\"}}}",
        )
        .unwrap();
        assert_eq!(run(&args(&consented)).1, 2);
        consented.push("--replace");
        assert_eq!(run(&args(&consented)).1, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jsonc_symlinks_and_unbuilt_binaries_are_refused() {
        let dir = scratch("refuse");
        let bin = binary(&dir);
        let jsonc = dir.join("jsonc.json");
        fs::write(&jsonc, "{ // comment\n}").unwrap();
        let apply = |path: &Path, bin: &str| {
            run(&args(&[
                "--settings",
                path.to_str().unwrap(),
                "--rust-binary",
                bin,
                "--apply",
                "--i-consent-to-write",
            ]))
            .1
        };
        assert_eq!(apply(&jsonc, &bin), 2);
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&jsonc, &link).unwrap();
        assert_eq!(apply(&link, &bin), 2);
        let plain = dir.join("plain.json");
        assert_eq!(apply(&plain, "/nonexistent/cortex-acp"), 2);
        assert!(!plain.exists());
        assert_eq!(run(&args(&["--python"])).1, 2);
        fs::remove_dir_all(dir).unwrap();
    }
}

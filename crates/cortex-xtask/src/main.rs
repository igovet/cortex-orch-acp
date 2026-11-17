//! `cortex-xtask`: the Python-free build, generation and packaging toolchain.
//!
//! Every subcommand reads its inputs below `--root` (default: the nearest ancestor of the
//! working directory holding `package-identity.json`) and exits non-zero with a diagnostic on
//! stderr when an input is stale or inconsistent.

mod contract;
mod debian;
mod driver;
mod engines;
mod fsutil;
mod installed;
mod instructions;
mod json;
mod package;
mod pins;
mod prerequisites;
mod proc;
mod profiles;
mod rendered;
mod testrun;
mod toml;
mod tools;

use json::Res;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cortex-xtask <command> [options]

generation (check never writes; write regenerates):
  generate-contract     check|write   Workspace wire contract (schema -> generated artifacts)
  generate-instructions check|write   native instruction data from skills/
  profiles              check|write   worker skills from agent-sources/
  check-generated                     all three checks
verification:
  check-pins                          reviewed Rust/CLI dependency pins (no build, no network)
  prerequisite-probe                  read-only inventory of native-build prerequisites (inventory only)
  test-fast             [--list] [--only NAMES] [--skip NAMES]   bounded no-paid cargo + npm suites, sequential
package identity:
  package               stamp|check   content hash stamp / validation
  runtime-payload       list          the exact shipped runtime file set
build helpers:
  package-channel       overlay|package <dev|release> [--target D --output D --depends-file F --engines D --build-stamp S]
  bundle-engines        --output D
  bundle-voice          --output D
  debian-version        <identity-version> <build-stamp>
  build-stamp           <build-stamp>
  shlibdeps             [--target D]
  npm-ci
  build-deb             [--release|--local-release|--ci-dev] [--engine docker|podman] [--jobs N] [--build-stamp S]
                        [--test-workspace] [--check-install] [--check-installed --prior-archive P] [--resume ID]
  installed-gate        --archive A --prior-archive P --evidence D [--channel dev|release] [--native-engine F]
                        [--display both|x11|wayland] [--close-cycles N]   installed real-input acceptance (Docker)
  rendered-image        [--tag T]   the Python-free rendered-check image (WebKitGTK, Xvfb, Openbox, xdotool, session bus)
  check-rendered        --artifacts D [--display x11|wayland] [--only common|lifecycle|controls|wayland|terminal|voice|attachments] [--root DIR] [--image T] [--build-image no]
                        rendered owner journey and visual audit of the built desktop binary (Docker)
  journey-series        --artifacts D --logs D [--runs N] [--display x11|wayland] [--expect-sha256 H]
  rendered-scrollbar    <label> --artifacts D --checkout D --evidence D   scrollbar grab and window-edge input check (run inside the rendered image)
  new-build-dir         <parent>
  snapshot              <destination>   copy the container build inputs
common option: --root DIR";

struct Args {
    positional: Vec<String>,
    root: Option<PathBuf>,
    flags: Vec<(String, String)>,
}

impl Args {
    fn parse(raw: &[String]) -> Res<Args> {
        let mut args = Args { positional: Vec::new(), root: None, flags: Vec::new() };
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            if let Some(name) = arg.strip_prefix("--") {
                let (name, value) = match name.split_once('=') {
                    Some((n, v)) => (n.to_string(), v.to_string()),
                    None => (name.to_string(), iter.next().ok_or(format!("--{name} requires a value"))?.clone()),
                };
                if name == "root" {
                    args.root = Some(PathBuf::from(value));
                } else {
                    args.flags.push((name, value));
                }
            } else {
                args.positional.push(arg.clone());
            }
        }
        Ok(args)
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.flag(name).map(PathBuf::from)
    }

    fn root(&self) -> Res<PathBuf> {
        if let Some(root) = &self.root {
            return Ok(root.clone());
        }
        let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
        loop {
            if dir.join("package-identity.json").is_file() {
                return Ok(dir);
            }
            if !dir.pop() {
                return Err("cannot locate the repository root; pass --root".into());
            }
        }
    }

    fn action(&self, allowed: &[&str]) -> Res<&str> {
        match self.positional.get(1).map(String::as_str) {
            Some(a) if allowed.contains(&a) => Ok(a),
            _ => Err(format!("expected one of: {}", allowed.join(", "))),
        }
    }
}

fn run(raw: &[String]) -> Res<i32> {
    // Boolean flags make the generic `--name value` parser unsuitable for `build-deb`.
    if raw.first().map(String::as_str) == Some("build-deb") {
        if raw.iter().any(|a| a == "-h" || a == "--help") {
            println!("{USAGE}");
            return Ok(0);
        }
        driver::build_deb(&args_root_for_build(raw)?, &driver::Options::parse(&strip_root(&raw[1..]))?)?;
        return Ok(0);
    }
    if raw.first().map(String::as_str) == Some("test-fast") {
        // Boolean `--list` makes the generic `--name value` parser unsuitable here.
        let root = package::absolute_root(&args_root_for_build(raw)?)?;
        return testrun::run(&root, &testrun::Options::parse(&strip_root(&raw[1..]))?);
    }
    if let Some(command) = raw.first().filter(|c| installed::COMMANDS.contains(&c.as_str())) {
        return installed::run(command, &raw[1..]);
    }
    if let Some(command) = raw.first().filter(|c| rendered::COMMANDS.contains(&c.as_str())) {
        return rendered::run(command, &raw[1..]);
    }
    let args = Args::parse(raw)?;
    let command = args.positional.first().map(String::as_str).ok_or(USAGE)?;
    match command {
        "generate-contract" => contract::run(&args.root()?, args.action(&["check", "write"])? == "write")?,
        "generate-instructions" => instructions::run(&args.root()?, args.action(&["check", "write"])? == "write")?,
        "profiles" => profiles::run(&args.root()?, args.action(&["check", "write"])? == "write")?,
        "check-generated" => {
            let root = args.root()?;
            contract::run(&root, false)?;
            instructions::run(&root, false)?;
            profiles::run(&root, false)?;
        }
        "package" => match args.action(&["stamp", "check"])? {
            "stamp" => package::run_stamp(&args.root()?)?,
            _ => package::run_check(&args.root()?)?,
        },
        "runtime-payload" => {
            args.action(&["list"])?;
            package::run_payload_list(&args.root()?)?
        }
        "package-channel" => package_channel(&args)?,
        "bundle-engines" => {
            engines::build_engines(&args.root()?, &args.path("output").ok_or("--output is required")?)?
        }
        "bundle-voice" => engines::build_voice(&args.root()?, &args.path("output").ok_or("--output is required")?)?,
        "debian-version" => {
            let (Some(identity), Some(stamp)) = (args.positional.get(1), args.positional.get(2)) else {
                return Err("usage: debian-version <identity-version> <build-stamp>".into());
            };
            println!("{}", debian::debian_version(identity, stamp)?);
        }
        "build-stamp" => {
            let stamp = args.positional.get(1).ok_or("usage: build-stamp <build-stamp>")?;
            println!("{}", debian::build_stamp(stamp)?);
        }
        "snapshot" => driver::snapshot(&package::absolute_root(&args.root()?)?, &PathBuf::from(args.positional.get(1).ok_or("usage: snapshot <destination>")?))?,
        "new-build-dir" => driver::new_build_dir(std::path::Path::new(args.positional.get(1).ok_or("usage: new-build-dir <parent>")?))?,
        "shlibdeps" => {
            return tools::shlibdeps(&args.path("target").unwrap_or_else(tools::default_target));
        }
        "npm-ci" => return tools::npm_ci(&args.root()?),
        "check-pins" => return pins::run(&args.root()?),
        "prerequisite-probe" => return prerequisites::run(),
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => return Err(format!("unknown command {other}\n{USAGE}")),
    }
    Ok(0)
}

/// `build-deb` forwards its options verbatim; only `--root` is consumed here.
fn strip_root(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = raw.iter();
    while let Some(a) = iter.next() {
        if a == "--root" {
            iter.next();
        } else if !a.starts_with("--root=") {
            out.push(a.clone());
        }
    }
    out
}

fn args_root_for_build(raw: &[String]) -> Res<PathBuf> {
    let mut iter = raw.iter();
    while let Some(a) = iter.next() {
        if a == "--root" {
            return iter.next().map(PathBuf::from).ok_or_else(|| "--root requires a value".to_string());
        }
        if let Some(v) = a.strip_prefix("--root=") {
            return Ok(PathBuf::from(v));
        }
    }
    Args { positional: vec![], root: None, flags: vec![] }.root().and_then(|r| package::absolute_root(&r))
}

fn package_channel(args: &Args) -> Res<()> {
    let action = args.action(&["overlay", "package"])?;
    let channel = args.positional.get(2).map(String::as_str).filter(|c| matches!(*c, "dev" | "release")).ok_or("channel must be dev or release")?;
    let root = args.root()?;
    if action == "overlay" {
        println!("{}", debian::overlay(&root, channel)?);
        return Ok(());
    }
    let stamp = args.flag("build-stamp").ok_or("package requires --build-stamp")?;
    let depends = match args.path("depends-file") {
        Some(file) => {
            let text = fsutil::read_text(&file)?;
            Some(fsutil::py_strip(&text).trim_start_matches("shlibs:Depends=").to_string())
        }
        None => None,
    };
    let target = args.path("target").ok_or("package requires --target")?;
    let output = args.path("output").ok_or("package requires --output")?;
    let root = package::absolute_root(&root)?;
    debian::package(&debian::Package {
        root: &root,
        target: &target,
        output: &output,
        channel,
        depends,
        engines: args.path("engines"),
        stamp,
    })
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code.clamp(1, 255) as u8),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

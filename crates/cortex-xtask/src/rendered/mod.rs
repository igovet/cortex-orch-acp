//! No-image WebKitGTK owner journey and visual audit of the shipped desktop binary, its pointer/edge probe, the rendered
//! image's recipe and the series runner.
//!
//! The journey runs on a private display (X11 with a real window manager, or headless Mutter) with a session bus, the native
//! service and the no-paid native peer. The visual audit reads computed styles and rectangles from the real binary, unmodified:
//! an LD_PRELOAD probe compiled in the container (`probe.c`) adds a request file to its web view, so the product carries no
//! inspector or debug route. No screenshot is captured by the audit. Module map: `entry` (account, session bus, final
//! evidence), `session` (display, service, app, audits, window behaviour, scrollbar), `journey` (the page and accessibility
//! helpers), `flow` (the journey in order), `controls` (worker, command and guidance controls), `stages` (Wayland frame and
//! controls, terminal, voice), `attachments` (OS drop, clipboard image, file picker and delivery to the peer), `a11y` (the accessibility tree), `x11` (the window close request), `series`.

mod a11y;
mod attachments;
mod controls;
mod entry;
mod flow;
mod journey;
mod series;
mod session;
mod stages;
mod x11;

use crate::installed::util::*;
use crate::installed::Cli;
use crate::json::{Res, Value};
use std::path::PathBuf;
use std::process::Command;

pub const COMMANDS: &[&str] = &[
    "check-rendered",
    "check-rendered-inside",
    "rendered-scrollbar",
    "rendered-image",
    "journey-series",
];

pub const BUILDER: &str = "cortex-workspace-builder:ubuntu26.04-rust1.99.1-node24.14.1";
pub const IMAGE: &str = "cortex-workspace-rendered:ubuntu26.04";
pub const PROBE: &str = include_str!("probe.c");
pub const WEBKIT_HOST: &str =
    include_str!("../../../../packages/workspace-ui/test/rendered/webkit-host.c");
/// Where the image keeps the pieces it compiled at build time.
pub const PREBUILT: &str = "/opt/cortex-render";
pub const AUDIT: &str = include_str!("audit.js");
pub const POPOVERS: &str = include_str!("popovers.js");
pub const ENGINE: &str = include_str!("engine.js");

pub fn run(command: &str, raw: &[String]) -> Res<i32> {
    let cli = Cli::parse(raw)?;
    match command {
        "rendered-image" => image(cli.flag("tag").unwrap_or(IMAGE)).map(|_| 0),
        "check-rendered" => host(&cli),
        "check-rendered-inside" => entry::inside(&cli),
        "rendered-scrollbar" => session::scrollbar(&cli),
        "journey-series" => series::run(&cli),
        _ => Err(format!("unknown command {command}")),
    }
}

/// The rendered-check image: no Python, no compiler, no development libraries. The C pieces the checks need (the WebKitGTK window
/// the drivers speak to, and the request-file probe preloaded into the desktop binary) are compiled in the builder stage, which
/// has the WebKitGTK development files; the final stage is the plain Ubuntu base with the runtime libraries, a private X server,
/// a window manager, input, accessibility and audio tools, a session bus and the builder's Node binary. Ubuntu's openbox and
/// mutter packages only recommend interpreters for helper scripts the checks never run, so a files-free shim package provides
/// those names and apt is told never to install the real ones.
pub fn image(tag: &str) -> Res<()> {
    let tmp = crate::proc::TempDir::new("rendered-image-")?;
    let context = &tmp.0;
    let stage = context.join("shim");
    std::fs::create_dir_all(stage.join("DEBIAN")).map_err(|e| e.to_string())?;
    write(
        &stage.join("DEBIAN/control"),
        "Package: cortex-render-python-shim\nVersion: 1\nArchitecture: all\nMaintainer: Cortex\nSection: misc\nPriority: optional\nProvides: python3 (= 3.14.0), python3-minimal (= 3.14.0), python3-gi (= 3.56.0)\nDescription: rendered image shim, no files; the checks never run an interpreter\n",
    )?;
    run_ok(&[
        "dpkg-deb",
        "--build",
        &stage.to_string_lossy(),
        &context.join("shim.deb").to_string_lossy(),
    ])?;
    write(&context.join("probe.c"), PROBE)?;
    write(&context.join("webkit-host.c"), WEBKIT_HOST)?;
    let dockerfile = format!(
        "FROM {BUILDER} AS compile\nUSER root\nCOPY probe.c webkit-host.c /src/\n\
         RUN mkdir /out && gcc -shared -fPIC -O1 -w -o /out/probe.so /src/probe.c $(pkg-config --cflags --libs webkit2gtk-4.1 gtk+-3.0) \\\n \
         && cc -O1 -o /out/webkit-host /src/webkit-host.c $(pkg-config --cflags --libs gtk+-3.0 webkit2gtk-4.1)\n\
         FROM {BASE}\nENV DEBIAN_FRONTEND=noninteractive\nCOPY shim.deb /shim.deb\n\
         RUN dpkg -i /shim.deb && printf 'Package: python3 python3-minimal python3-gi python3.* libpython3*\\nPin: release *\\nPin-Priority: -1\\n' > /etc/apt/preferences.d/no-python\n\
         RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libwebkit2gtk-4.1-0 libgtk-3-0t64 libayatana-appindicator3-1 librsvg2-2 libssl3t64 libxdo3 fontconfig fonts-dejavu-core \\\n \
         xvfb xdotool x11-utils openbox dbus dbus-x11 at-spi2-core xdg-dbus-proxy mutter git pulseaudio pulseaudio-utils bzip2 sqlite3 xclip wl-clipboard && rm -rf /var/lib/apt/lists/*\n\
         COPY --from=compile /usr/local/bin/node /usr/local/bin/node\nCOPY --from=compile /out/ /opt/cortex-render/\nENTRYPOINT []\n"
    );
    write(&context.join("Dockerfile"), &dockerfile)?;
    run_ok(&[
        "docker",
        "build",
        "--quiet",
        "--tag",
        tag,
        &context.to_string_lossy(),
    ])
    .map(|_| ())
}

fn flag_path(cli: &Cli, name: &str) -> Res<PathBuf> {
    cli.path(name)
        .ok_or_else(|| format!("--{name} is required"))
}

/// Host side: a disposable no-network container, read-only sources, the exact built artifacts. The harness is this very binary,
/// copied in, so the container needs no interpreter.
fn host(cli: &Cli) -> Res<i32> {
    let artifacts = flag_path(cli, "artifacts")?
        .canonicalize()
        .map_err(|e| format!("artifacts: {e}"))?;
    let root = crate::installed::gate::find_root(cli)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let tag = cli.flag("image").unwrap_or(IMAGE).to_string();
    if cli.flag("build-image") != Some("no") {
        image(&tag)?;
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = crate::proc::TempDir::new("rendered-")?;
    std::fs::copy(&me, tmp.0.join("cortex-xtask")).map_err(|e| e.to_string())?;
    chmod(&tmp.0.join("cortex-xtask"), 0o755)?;
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let mut argv: Vec<String> = [
        "docker",
        "run",
        "--rm",
        "--network=none",
        "--user",
        &format!("{uid}:{gid}"),
        "--shm-size=512m",
        "--cap-drop=ALL",
        "--security-opt=seccomp=unconfined",
        "--security-opt=apparmor=unconfined",
        "--security-opt=systempaths=unconfined",
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    for (source, target) in [
        (&artifacts, "/artifacts"),
        (&root, "/checkout"),
        (&tmp.0, "/harness"),
    ] {
        argv.extend([
            "--mount".into(),
            format!(
                "type=bind,source={},target={target},readonly",
                source.display()
            ),
        ]);
    }
    argv.extend([
        "--env".into(),
        "HOME=/tmp".into(),
        tag,
        "/harness/cortex-xtask".into(),
        "check-rendered-inside".into(),
        "--artifacts".into(),
        "/artifacts".into(),
        "--checkout".into(),
        "/checkout".into(),
    ]);
    if let Some(only) = cli.flag("only") {
        argv.extend(["--only".into(), only.into()]);
    }
    let display = cli.flag("display").unwrap_or("x11");
    require(
        matches!(display, "x11" | "wayland"),
        "--display is x11 or wayland",
    )?;
    if display == "wayland" || cli.flag("journey-wayland").is_some() {
        argv.extend(["--journey-wayland".into(), "yes".into()]);
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    let status = command.status().map_err(|e| format!("docker: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

/// Python's `json.dumps(value, ensure_ascii=False)` for evidence: the page's own numbers (fractions, exponents) pass through as
/// written, which the build tools' strict writer refuses.
pub fn pretty_line(value: &Value) -> String {
    dump(value, None, 0)
}

/// The same with an indent, as `json.dumps(value, indent=2)`.
pub fn pretty_block(value: &Value, indent: usize) -> String {
    dump(value, Some(indent), 0)
}

fn dump(value: &Value, indent: Option<usize>, depth: usize) -> String {
    let nl =
        |level: usize| indent.map_or(String::new(), |i| format!("\n{}", " ".repeat(i * level)));
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Num(text) => text.clone(),
        Value::Str(text) => crate::json::quote(text, false),
        Value::Arr(items) if items.is_empty() => "[]".into(),
        Value::Arr(items) => {
            let sep = if indent.is_some() { "," } else { ", " };
            format!(
                "[{}{}{}]",
                nl(depth + 1),
                items
                    .iter()
                    .map(|v| dump(v, indent, depth + 1))
                    .collect::<Vec<_>>()
                    .join(&format!("{sep}{}", nl(depth + 1))),
                nl(depth)
            )
        }
        Value::Obj(pairs) if pairs.is_empty() => "{}".into(),
        Value::Obj(pairs) => {
            let sep = if indent.is_some() { "," } else { ", " };
            let body: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}: {}",
                        crate::json::quote(k, false),
                        dump(v, indent, depth + 1)
                    )
                })
                .collect();
            format!(
                "{{{}{}{}}}",
                nl(depth + 1),
                body.join(&format!("{sep}{}", nl(depth + 1))),
                nl(depth)
            )
        }
    }
}

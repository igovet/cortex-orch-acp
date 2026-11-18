//! Host side of the installed real-input gate: builds the disposable image, runs every
//! display/install scenario in a fresh privileged PID-1 systemd container and writes the
//! archive-bound `result.json`. Docker only; no host install, session, display or credentials.

use super::channel::{require_archives, Channel};
use super::config;
use super::util::*;
use super::Cli;
use crate::json::{parse, pretty_ascii, Res, Value};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn require_native_health(output: &str) -> Res<()> {
    // A disappeared window is not evidence of a clean exit. gio-launched children can abort
    // while the input driver and installed service still return success.
    let fatal = Regex::new(
        r"(?im)^(?:corrupted double-linked list|corrupted size vs[.] prev_size|double free or corruption|free[(][)]:|malloc[(][)]:|Segmentation fault|Aborted(?: [(]core dumped[)])?)",
    )
    .unwrap();
    require(
        !fatal.is_match(output),
        "native process failure diagnostic; archive is not qualified",
    )
}

fn require_native_journey(output: &str, backend: &str) -> Res<()> {
    let mut rows = Vec::new();
    for text in output.lines() {
        if text.contains("\"step\": \"native_task_journey\"") {
            rows.push(parse(text).map_err(|e| format!("malformed native journey evidence: {e}"))?);
        }
    }
    require(
        rows.len() == 2,
        "missing or duplicate native journey evidence",
    )?;
    let paused: Vec<Option<bool>> = rows
        .iter()
        .map(|r| match r.get("paused_resumed") {
            Some(Value::Bool(v)) => Some(*v),
            _ => None,
        })
        .collect();
    require(
        paused == [Some(false), Some(true)],
        "native journey did not run the plain then Stop/Resume scenarios",
    )?;
    let ids: BTreeSet<&str> = rows
        .iter()
        .filter_map(|r| r.get("task_id").and_then(Value::as_str))
        .filter(|t| !t.is_empty())
        .collect();
    require(
        ids.len() == 2,
        "exactly two distinct native task identities are required",
    )?;
    for r in &rows {
        let ok = r.get("backend").and_then(Value::as_str) == Some(backend)
            && r.get("state").and_then(Value::as_str) == Some("COMPLETED")
            && r.get("contained_checks")
                .and_then(Value::as_i64)
                .is_some_and(|c| c > 0);
        require(
            ok,
            "native journey row is incomplete or for another backend",
        )?;
    }
    Ok(())
}

/// The gate image carries no Python of its own. Only a candidate or prior archive that itself
/// depends on python3 (an old archive) brings it, and then the evidence says so.
fn require_python_policy(output: &str, allowed: bool) -> Res<()> {
    let mut rows = Vec::new();
    for text in output.lines() {
        if text.contains("\"step\": \"python_policy\"") {
            rows.push(parse(text).map_err(|e| format!("malformed python policy evidence: {e}"))?);
        }
    }
    let stages: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("stage").and_then(Value::as_str))
        .collect();
    require(
        stages == ["image", "after-install"],
        "missing or duplicate python policy evidence",
    )?;
    for r in &rows {
        let present = matches!(r.get("present"), Some(Value::Bool(true)));
        let said =
            matches!(r.get("allowed_by_archive_depends"), Some(Value::Bool(v)) if *v == allowed);
        require(
            said && (allowed || !present),
            "python3 is present in the gate container but no archive requires it",
        )?;
    }
    Ok(())
}

fn depends_of(archive: &Path) -> Res<BTreeSet<String>> {
    let fields = run_ok(&["dpkg-deb", "-f", &archive.to_string_lossy(), "Depends"])?.stdout;
    let name = Regex::new(r"^[a-z0-9.+-]+").unwrap();
    let mut set = BTreeSet::new();
    for part in fields.split(',') {
        let part = part.trim();
        let m = name
            .find(part)
            .ok_or_else(|| format!("unparsable Depends entry {part:?}"))?;
        set.insert(m.as_str().to_string());
    }
    Ok(set)
}

/// Run to completion with all output appended to `log`; returns the exit code.
fn run_logged(argv: &[&str], log: &Path, timeout: Duration) -> Res<i32> {
    let sink = std::fs::File::create(log).map_err(|e| e.to_string())?;
    let err = sink.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(sink)
        .stderr(err)
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.code().unwrap_or(-1));
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err("scenario deadline exceeded".into());
        }
        sleep_ms(200);
    }
}

pub fn find_root(cli: &Cli) -> Res<PathBuf> {
    if let Some(root) = cli.path("root") {
        return Ok(root);
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

pub fn run(cli: &Cli, channel: &Channel, close_cycles: u32) -> Res<i32> {
    let result = run_inner(cli, channel, close_cycles);
    match &result {
        Ok(_) => println!("PASS: exact installed archive; X11 and Wayland, clean and plain apt upgrade; all windows closed."),
        Err(_) => println!("FAIL: installed-package acceptance is incomplete; do not hand over this archive."),
    }
    result.map(|_| 0)
}

fn run_inner(cli: &Cli, channel: &Channel, close_cycles: u32) -> Res<()> {
    let (Some(archive), Some(prior), Some(evidence)) = (
        cli.path("archive"),
        cli.path("prior-archive"),
        cli.path("evidence"),
    ) else {
        return Err("--archive, --prior-archive and --evidence are required".into());
    };
    // A diagnostic single-phase run; never acceptance evidence.
    let only = cli.flag("only-phase").filter(|p| !p.is_empty());
    let display = cli.flag("display").unwrap_or("both");
    if !["both", "x11", "wayland"].contains(&display) {
        return Err("--display must be both, x11 or wayland".into());
    }
    let archive = archive
        .canonicalize()
        .map_err(|e| format!("archive: {e}"))?;
    let prior = prior
        .canonicalize()
        .map_err(|e| format!("prior archive: {e}"))?;
    find_root(cli)?;
    let evidence = if evidence.is_absolute() {
        evidence
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(evidence)
    };
    std::fs::create_dir_all(evidence.parent().ok_or("bad evidence path")?)
        .map_err(|e| e.to_string())?;
    std::fs::create_dir(&evidence).map_err(|e| format!("evidence directory must be new: {e}"))?;
    let archive_hash = sha_file(&archive)?;
    let prior_hash = sha_file(&prior)?;
    let native_engine = match cli.path("native-engine") {
        Some(p) => p,
        None => archive
            .parent()
            .ok_or("archive has no parent")?
            .join("native-fake-engine"),
    }
    .canonicalize()
    .map_err(|e| {
        format!("native fake engine peer is required beside the archive (or --native-engine): {e}")
    })?;
    let engine_hash = sha_file(&native_engine)?;
    require_archives(channel, &[&archive, &prior])?;
    let mut depends = depends_of(&archive)?;
    depends.extend(depends_of(&prior)?);
    let python_allowed = depends.iter().any(|d| d.starts_with("python3"));
    let image = format!("cortex-installed-check:{}", &archive_hash[..16]);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let harness = obj(vec![("cortex-xtask", s(sha_file(&me)?))]);
    {
        let tmp = crate::proc::TempDir::new("installed-")?;
        // The context lives under the system temp dir, never beside the evidence.
        let context = &tmp.0;
        std::fs::copy(&me, context.join("cortex-xtask")).map_err(|e| e.to_string())?;
        std::fs::copy(&native_engine, context.join("native-fake-engine"))
            .map_err(|e| e.to_string())?;
        for f in ["cortex-xtask", "native-fake-engine"] {
            chmod(&context.join(f), 0o755)?;
        }
        let packages: Vec<&str> = depends.iter().map(String::as_str).collect();
        // Ubuntu's openbox and mutter-common-bin depend on Python for helper scripts the gate never
        // runs. When no archive needs Python, a files-free shim package satisfies those names so the
        // image contains no interpreter at all; the in-container policy check proves it.
        let shim = if python_allowed {
            String::new()
        } else {
            let stage = context.join("shim");
            std::fs::create_dir_all(stage.join("DEBIAN")).map_err(|e| e.to_string())?;
            write(
                &stage.join("DEBIAN/control"),
                "Package: cortex-gate-python-shim\nVersion: 1\nArchitecture: all\nMaintainer: Cortex\nSection: misc\nPriority: optional\nProvides: python3 (= 3.14.0), python3-minimal (= 3.14.0), python3-gi (= 3.56.0)\nDescription: gate image shim, no files; image carries no Python\n",
            )?;
            run_ok(&[
                "dpkg-deb",
                "--build",
                &stage.to_string_lossy(),
                &context.join("shim.deb").to_string_lossy(),
            ])?;
            "COPY shim.deb /shim.deb\nRUN dpkg -i /shim.deb && printf 'Package: python3 python3-minimal python3-gi python3.* libpython3*\\nPin: release *\\nPin-Priority: -1\\n' > /etc/apt/preferences.d/no-python\n".to_string()
        };
        let dockerfile = format!(
            "FROM {BASE}\nENV container=docker\n{shim}RUN apt-get update && apt-get install -y --no-install-recommends {} systemd-sysv dbus-user-session xvfb openbox xdotool x11-utils mutter at-spi2-core git libglib2.0-bin xclip wl-clipboard sqlite3 && rm -rf /var/lib/apt/lists/*\nRUN echo 'path-include=/usr/share/doc/{}/*' >> /etc/dpkg/dpkg.cfg.d/excludes\nCOPY cortex-xtask native-fake-engine /harness/\nSTOPSIGNAL SIGRTMIN+3\nCMD [\"/sbin/init\"]\n",
            packages.join(" "),
            channel.package
        );
        write(&context.join("Dockerfile"), &dockerfile)?;
        let built = capture(
            Command::new("docker")
                .args(["build", "-t", &image])
                .arg(context),
            Duration::from_secs(1800),
        )?;
        write(
            &evidence.join("image-build.log"),
            &format!("{}{}", built.stdout, built.stderr),
        )?;
        print!("{}{}", built.stdout, built.stderr);
        require(built.code == 0, "docker build failed")?;
    }
    let image_id = run_ok(&["docker", "image", "inspect", &image, "--format", "{{.Id}}"])?
        .stdout
        .trim()
        .to_string();
    let mut scenarios: Vec<Value> = Vec::new();
    let mut result = obj(vec![
        ("channel", s(&channel.name)),
        ("package", s(&channel.package)),
        ("archive_sha256", s(&archive_hash)),
        ("prior_sha256", s(&prior_hash)),
        ("image", s(image_id)),
        ("harness_sha256", harness),
        ("native_engine_sha256", s(&engine_hash)),
        ("python_required_by_archive_depends", b(python_allowed)),
        ("scenarios", arr(vec![])),
        ("close_cycles", n(close_cycles as i64)),
        ("accepted", b(false)),
    ]);
    let outcome = (|| -> Res<()> {
        let backends: Vec<&str> = if display == "both" {
            vec!["x11", "wayland"]
        } else {
            vec![display]
        };
        for backend in backends {
            for mode in ["clean", "upgrade"] {
                let folder = evidence.join(format!("{backend}-{mode}"));
                std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
                chmod(&folder, 0o777)?; // Disposable container user writes only this evidence directory.
                let mount = |src: &Path, dst: &str, ro: bool| {
                    format!(
                        "type=bind,source={},target={dst}{}",
                        src.display(),
                        if ro { ",readonly" } else { "" }
                    )
                };
                let id = run_ok(&[
                    "docker",
                    "run",
                    "--detach",
                    "--privileged",
                    "--cgroupns=private",
                    "--network=none",
                    "--tmpfs",
                    "/run",
                    "--tmpfs",
                    "/run/lock",
                    "--tmpfs",
                    "/tmp",
                    "--mount",
                    &mount(&archive, "/archives/candidate.deb", true),
                    "--mount",
                    &mount(&prior, "/archives/prior.deb", true),
                    "--mount",
                    &mount(&folder, "/evidence", false),
                    &image,
                ])?
                .stdout
                .trim()
                .to_string();
                let scenario = (|| -> Res<()> {
                    let cycles = close_cycles.to_string();
                    let code = run_logged(
                        &[
                            "docker",
                            "exec",
                            &id,
                            "/harness/cortex-xtask",
                            "installed-inside",
                            "--channel",
                            &channel.name,
                            "--close-cycles",
                            &cycles,
                            "--python-allowed",
                            if python_allowed { "yes" } else { "no" },
                            "--only-phase",
                            only.unwrap_or(""),
                            backend,
                            mode,
                        ],
                        &folder.join("output.log"),
                        Duration::from_secs(1200),
                    );
                    // Complete fixture-only output; never an owner's log or environment.
                    let output =
                        std::fs::read_to_string(folder.join("output.log")).unwrap_or_default();
                    print!("{output}");
                    let code = code?;
                    scenarios.push(obj(vec![
                        ("backend", s(backend)),
                        ("mode", s(mode)),
                        ("exit_code", n(code as i64)),
                    ]));
                    require(code == 0, &format!("{backend} {mode} failed"))?;
                    require_native_health(&output)?;
                    require_python_policy(&output, python_allowed)?;
                    if only.is_some() {
                        return Ok(());
                    }
                    require_native_journey(&output, backend)?;
                    config::require_evidence(&output, channel, backend, mode)
                })();
                let removed = run_ok(&["docker", "rm", "--force", &id]);
                scenario?;
                removed?;
            }
        }
        require(
            sha_file(&archive)? == archive_hash && sha_file(&prior)? == prior_hash,
            "archive changed during check",
        )?;
        require(
            sha_file(&native_engine)? == engine_hash,
            "fake engine changed during check",
        )?;
        require(
            sha_file(&me)?
                == result
                    .get("harness_sha256")
                    .and_then(|h| h.get("cortex-xtask"))
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            "harness changed during check",
        )?;
        Ok(())
    })();
    let complete = outcome.is_ok() && only.is_none() && display == "both" && scenarios.len() == 4;
    result.set("scenarios", arr(scenarios));
    result.set("accepted", b(complete));
    write(&evidence.join("result.json"), &pretty_ascii(&result)?)?;
    outcome?;
    require(
        complete,
        "partial display run is diagnostic, not owner acceptance",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journey(backend: &str, task: &str, paused: bool, checks: i64) -> String {
        line(&obj(vec![
            ("step", s("native_task_journey")),
            ("backend", s(backend)),
            ("task_id", s(task)),
            ("state", s("COMPLETED")),
            ("contained_checks", n(checks)),
            ("paused_resumed", b(paused)),
        ]))
    }

    #[test]
    fn native_journey_needs_two_distinct_completed_tasks() {
        let ok = format!(
            "{}\nnoise\n{}\n",
            journey("x11", "t1", false, 2),
            journey("x11", "t2", true, 1)
        );
        assert!(require_native_journey(&ok, "x11").is_ok());
        assert!(require_native_journey(&ok, "wayland").is_err());
        let dup = format!(
            "{}\n{}\n",
            journey("x11", "t1", false, 2),
            journey("x11", "t1", true, 1)
        );
        assert!(require_native_journey(&dup, "x11").is_err());
        let one = journey("x11", "t1", false, 2);
        assert!(require_native_journey(&one, "x11").is_err());
        let none = format!(
            "{}\n{}\n",
            journey("x11", "t1", false, 0),
            journey("x11", "t2", true, 1)
        );
        assert!(require_native_journey(&none, "x11").is_err());
        let order = format!(
            "{}\n{}\n",
            journey("x11", "t1", true, 2),
            journey("x11", "t2", false, 1)
        );
        assert!(require_native_journey(&order, "x11").is_err());
    }

    #[test]
    fn native_failure_diagnostics_block_acceptance() {
        assert!(require_native_health("fine\nok").is_ok());
        assert!(require_native_health("x\nSegmentation fault (core dumped)\n").is_err());
        assert!(require_native_health("double free or corruption (out)").is_err());
    }

    #[test]
    fn config_evidence_is_exact() {
        let ch = Channel::new("release").unwrap();
        let home = std::path::Path::new("/nonexistent-home");
        let _ = home;
        let docs = config::documents();
        let mut files = obj(vec![]);
        for (name, value) in &docs {
            files.set(
                name,
                obj(vec![
                    (
                        "bytes_sha256",
                        s(sha_bytes(crate::json::pretty(value).unwrap().as_bytes())),
                    ),
                    ("semantic_sha256", s(sha_bytes(canon(value).as_bytes()))),
                ]),
            );
        }
        let row = |stage: &str, channel: &str| {
            line(&obj(vec![
                ("step", s("orchestration_config_preserved")),
                ("stage", s(stage)),
                ("channel", s(channel)),
                ("files", files.clone()),
            ]))
        };
        let shown = |phase: &str| {
            line(&obj(vec![
                ("step", s("orchestration_settings_visible")),
                ("backend", s("x11")),
                ("phase", s(phase)),
                ("channel", s("release")),
                ("active", s("default")),
                ("profiles", n(6)),
                ("coordinator", s(config::MODEL)),
                ("effort", s(config::EFFORT)),
            ]))
        };
        let mut lines = vec![row("seeded", "release"), shown("cold-start")];
        for stage in [
            "candidate-install",
            "first-launch",
            "unreadable-project-launch",
        ] {
            lines.push(row(stage, "release"));
        }
        lines.push(shown("incident"));
        let good = lines.join("\n");
        assert!(config::require_evidence(&good, &ch, "x11", "clean").is_ok());
        // Upgrade needs the prior stages; a duplicate or wrong-channel row fails closed.
        assert!(config::require_evidence(&good, &ch, "x11", "upgrade").is_err());
        let wrong = good.replacen("\"channel\": \"release\"", "\"channel\": \"dev\"", 1);
        assert!(config::require_evidence(&wrong, &ch, "x11", "clean").is_err());
        let dup = format!("{good}\n{}", row("first-launch", "release"));
        assert!(config::require_evidence(&dup, &ch, "x11", "clean").is_err());
    }

    fn policy(stage: &str, present: bool, allowed: bool) -> String {
        line(&obj(vec![
            ("step", s("python_policy")),
            ("stage", s(stage)),
            ("present", b(present)),
            ("allowed_by_archive_depends", b(allowed)),
        ]))
    }

    #[test]
    fn python_policy_is_fail_closed() {
        let clean = format!(
            "{}\n{}\n",
            policy("image", false, false),
            policy("after-install", false, false)
        );
        assert!(require_python_policy(&clean, false).is_ok());
        let present = format!(
            "{}\n{}\n",
            policy("image", false, false),
            policy("after-install", true, false)
        );
        assert!(require_python_policy(&present, false).is_err());
        // Python is tolerated only when the archive's own Depends require it, and the evidence says so.
        let old = format!(
            "{}\n{}\n",
            policy("image", true, true),
            policy("after-install", true, true)
        );
        assert!(require_python_policy(&old, true).is_ok());
        assert!(require_python_policy(&old, false).is_err());
        assert!(require_python_policy(&policy("image", false, false), false).is_err());
    }
}

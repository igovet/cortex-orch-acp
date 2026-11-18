//! No-paid native journey fixtures; only disposable installed systemd containers.

use super::channel::Channel;
use super::input::{Input, TASK_COMPOSER};
use super::util::*;
use crate::json::{dumps, Format, Res, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn digest(value: &Value) -> String {
    sha_bytes(canon(value).as_bytes())
}

fn write_json(path: &Path, value: &Value) -> Res<()> {
    let text = dumps(
        value,
        &Format {
            sort_keys: false,
            ensure_ascii: true,
            indent: None,
            compact: false,
        },
    )?;
    write(path, &text)?;
    chmod(path, 0o600)
}

pub fn prepare(home: &Path, channel: &Channel, name: &str, scenario: &str) -> Res<PathBuf> {
    super::require_container(false)?;
    prepare_with(home, channel, name, scenario, "/harness/native-fake-engine")
}

/// The same disposable account configuration for any container, with the no-paid peer at `engine`.
pub fn prepare_with(
    home: &Path,
    channel: &Channel,
    name: &str,
    scenario: &str,
    engine: &str,
) -> Res<PathBuf> {
    chmod(home, 0o700)?;
    let root = home.join(name);
    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
    write(&root.join("work.txt"), "before")?;
    write(&root.join("scenario.txt"), scenario)?;
    let config = channel.config(home);
    let catalog = channel.catalog(home).join("cn_00000000f1f0");
    for folder in [&config, &catalog] {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let mut parent = Some(folder.as_path());
        while let Some(p) = parent {
            if p == home {
                break;
            }
            chmod(p, 0o700)?;
            parent = p.parent();
        }
    }
    let launch = arr(vec![s(engine), s("desktop-auto")]);
    let mut record = obj(vec![
        ("id", s("cn_00000000f1f0")),
        ("name", s("Fixture engine")),
        ("key_kind", Value::Null),
        ("created_at", s("2026-01-01T00:00:00+00:00")),
        ("updated_at", s("2026-01-01T00:00:00+00:00")),
        ("engine", s("codex-acp")),
        ("provider", s("fixture")),
        ("endpoint", Value::Null),
        ("launch_command", launch.clone()),
        ("auth_mode", s("native_account")),
        ("qualified", b(true)),
        ("state", s("configured")),
        (
            "credential_ref",
            obj(vec![
                ("kind", s("engine_native_dir")),
                ("path", s(home.to_string_lossy())),
                ("owned", b(false)),
            ]),
        ),
        ("max_concurrent_sessions", n(4)),
    ]);
    let approved = obj(vec![
        ("engine", s("codex-acp")),
        ("provider", s("fixture")),
        ("endpoint", Value::Null),
        ("launch_command", launch),
    ]);
    record.set("approved_fingerprint", s(digest(&approved)));
    write_json(
        &config.join("connections.json"),
        &obj(vec![
            ("schema_version", n(1)),
            ("connections", obj(vec![("cn_00000000f1f0", record)])),
        ]),
    )?;
    write_json(
        &config.join("defaults.json"),
        &obj(vec![(
            "defaults",
            obj(vec![
                ("route_connection_id", s("cn_00000000f1f0")),
                ("route_model", s("fixture-model")),
                ("parameters", obj(vec![])),
            ]),
        )]),
    )?;
    write_json(
        &config.join("routing.json"),
        &obj(vec![
            ("model_policy_version", n(2)),
            (
                "enabled",
                obj(vec![("codex-acp", arr(vec![s("fixture-model")]))]),
            ),
            (
                "models",
                obj(vec![(
                    "codex-acp",
                    obj(vec![
                        ("auto", arr(vec![s("fixture-model")])),
                        (
                            "tiers",
                            obj(vec![(
                                "fixture-model",
                                arr(vec![s("light"), s("standard"), s("deep")]),
                            )]),
                        ),
                    ]),
                )]),
            ),
            ("pin", obj(vec![("*", s("codex-acp/fixture-model"))])),
        ]),
    )?;
    let mut route = obj(vec![
        ("engine", s("codex-acp")),
        ("connection_id", s("cn_00000000f1f0")),
        ("model", s("fixture-model")),
        ("supported_parameters", arr(vec![])),
    ]);
    let id = format!("xr_{}", &digest(&route)[..24]);
    route.set("id", s(id));
    write_json(
        &catalog.join("cs_fixture.json"),
        &obj(vec![
            ("snapshot_id", s("cs_fixture")),
            (
                "entries",
                arr(vec![obj(vec![
                    ("route", route),
                    ("connection_id", s("cn_00000000f1f0")),
                    ("catalog_snapshot_id", s("cs_fixture")),
                    (
                        "evidence",
                        obj(vec![(
                            "catalog_obtained",
                            obj(vec![("source", s("discovery"))]),
                        )]),
                    ),
                ])]),
            ),
        ]),
    )?;
    Ok(root)
}

/// One read-only query against a project's store; rows are `|`-separated.
pub fn query(db: &Path, sql: &str) -> Res<String> {
    let done = capture(
        std::process::Command::new("sqlite3")
            .args(["-readonly", "-batch", "-noheader", "-cmd", ".timeout 5000"])
            .arg(db)
            .arg(sql),
        Duration::from_secs(30),
    )?;
    require(
        done.code == 0,
        &format!("store query failed: {}", tail(&done.stderr)),
    )?;
    Ok(done.stdout.trim().to_string())
}

pub fn count(db: &Path, sql: &str) -> Res<i64> {
    query(db, sql)?
        .parse::<i64>()
        .map_err(|_| "store query returned no count".to_string())
}

pub fn state(root: &Path, channel: &Channel) -> Res<Option<String>> {
    let db = channel.store_db(root);
    if !db.exists() {
        return Ok(None);
    }
    let out = query(
        &db,
        "SELECT state FROM runtime_tasks ORDER BY rowid DESC LIMIT 1",
    )?;
    Ok((!out.is_empty()).then_some(out))
}

pub fn evidence(root: &Path, channel: &Channel, paused: bool) -> Res<Vec<(&'static str, Value)>> {
    let db = channel.store_db(root);
    let row = query(
        &db,
        "SELECT task_id||'|'||state FROM runtime_tasks ORDER BY rowid DESC LIMIT 1",
    )?;
    let (task, status) = row.split_once('|').ok_or("no native task row")?;
    require(status == "COMPLETED", "native task not completed")?;
    let q = |sql: String| count(&db, &sql);
    require(
        q(format!("SELECT count(*) FROM runtime_events WHERE task_id='{task}' AND type='TASK_CREATED' AND json_extract(payload_json,'$.execution_host')='rust'"))? == 1,
        "native task was not created by the Rust execution host exactly once",
    )?;
    require(q(format!("SELECT count(*) FROM delegation_provisioning WHERE task_id='{task}' AND status!='terminated'"))? == 0, "provisioning not terminated")?;
    require(q(format!("SELECT count(*) FROM capacity_reservations WHERE task_id='{task}' AND state!='released'"))? == 0, "capacity not released")?;
    require(q(format!("SELECT count(*) FROM runtime_events WHERE task_id='{task}' AND type='NATIVE_RECOVERY_REQUIRED'"))? == 0, "native recovery required")?;
    let checks = q(format!("SELECT count(*) FROM runtime_requests WHERE task_id='{task}' AND operation='native_command_output'"))?;
    require(checks > 0, "no contained verification")?;
    if paused {
        for event in ["OWNER_TASK_PAUSED", "OWNER_TASK_RESUMED"] {
            require(
                q(format!(
                    "SELECT count(*) FROM runtime_events WHERE task_id='{task}' AND type='{event}'"
                ))? == 1,
                "missing or duplicate pause/resume event",
            )?;
        }
    }
    Ok(vec![
        ("task_id", s(task)),
        ("state", s(status)),
        ("contained_checks", n(checks)),
        ("paused_resumed", b(paused)),
    ])
}

pub fn journey(input: &Input, backend: &str, channel: &Channel) -> Res<()> {
    let root = PathBuf::from("/home/acceptance/native-journey");
    input.click("Add project", &[], false)?;
    wait("native project picker", || {
        Ok(input
            .find("Search for folder", &["list item"], true)?
            .is_some())
    })?;
    input.click("Search for folder", &["list item"], true)?;
    input.fill("Search for folder", &root.to_string_lossy())?;
    wait("native project resolved", || {
        Ok(input
            .find("native-journey", &["list item"], true)?
            .is_some())
    })?;
    // The result list can re-render between locating the row and the pointer press; a press
    // that landed on a replaced row leaves the picker open, so press the live row again.
    let mut attempt = 0;
    loop {
        attempt += 1;
        input.click("native-journey", &["list item"], true)?;
        match wait_secs("native activation", 15, || input.has("Activate", &[])) {
            Ok(()) => break,
            Err(e)
                if attempt < 3
                    && input
                        .find("native-journey", &["list item"], true)?
                        .is_some() =>
            {
                eprintln!("{{\"step\": \"native_picker_press_retry\", \"attempt\": {attempt}, \"after\": {:?}}}", e);
            }
            Err(e) => return Err(e),
        }
    }
    input.click("Activate", &[], false)?;
    wait_secs("native project ready", 60, || {
        Ok(!input.has("Activate", &[])? && input.has("New task", &[])?)
    })?;
    for paused in [false, true] {
        let title = if paused {
            "Native resumable journey"
        } else {
            "Native completed journey"
        };
        write(&root.join("scenario.txt"), if paused { "hold" } else { "" })?;
        input.click("New task", &[], false)?;
        wait("native task editor", || input.has("Describe the task", &[]))?;
        input.fill("Describe the task", title)?;
        input.click("Project folder", &["radio button"], true)?;
        input.click("Save draft", &[], false)?;
        wait("native draft saved", || {
            Ok(!input.has("Save draft", &[])? && input.find(title, &["link"], true)?.is_some())
        })?;
        input.click(title, &["link"], true)?;
        wait("native task details", || {
            input.has("Task progress and details", &["panel"])
        })?;
        // Start, Stop and Resume are the task composer's one action button; the title band has none of them.
        input.click_in(TASK_COMPOSER, "Start")?;
        if paused {
            wait_secs("native worker live", 60, || {
                if state(&root, channel)?.as_deref() != Some("EXECUTING") {
                    return Ok(false);
                }
                Ok(count(&channel.store_db(&root), "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')")? == 1)
            })?;
            // The composer is empty here, so its action button is Stop.
            wait("native stop button", || input.has_in(TASK_COMPOSER, "Stop"))?;
            input.click_in(TASK_COMPOSER, "Stop")?;
            wait("native task paused", || {
                // Exact controls, not text: the opposite button must be shown and this one gone.
                Ok(input.has_in(TASK_COMPOSER, "Resume")?
                    && !input.any_button("Stop")?
                    && state(&root, channel)?.as_deref() == Some("PAUSED"))
            })?;
            write(&root.join("scenario.txt"), "")?;
            input.click_in(TASK_COMPOSER, "Resume")?;
        }
        wait_secs("native task completed", 90, || {
            Ok(state(&root, channel)?.as_deref() == Some("COMPLETED"))
        })?;
        wait_secs("native result rendered", 30, || {
            input.seen("Fixture result.")
        })?;
        require(
            !input.seen("Stale result")?,
            "unchanged native result was labelled stale",
        )?;
        let mut pairs = vec![("backend", s(backend))];
        pairs.extend(evidence(&root, channel, paused)?);
        emit("native_task_journey", pairs);
        input.click("native-journey", &["link"], false)?;
        wait("native return to project board", || {
            Ok(input
                .find("New task", &["button", "push button"], false)?
                .is_some())
        })?;
    }
    Ok(())
}

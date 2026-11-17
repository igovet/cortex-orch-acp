//! Synthetic owner-shaped orchestration config; never copies an owner's files or credentials.

use super::channel::Channel;
use super::input::Node;
use super::util::*;
use crate::json::{parse, pretty, Res, Value};
use std::path::Path;

pub const PROFILE_NAMES: [&str; 6] = ["claude-codex", "experiment", "default", "test", "claude-codex-opencode", "default-custom"];
pub const MODEL: &str = "qualification-coordinator";
pub const EFFORT: &str = "medium";
pub const FILES: [&str; 3] = ["routing.json", "defaults.json", "routing-profiles.json"];

fn null() -> Value {
    Value::Null
}

pub fn documents() -> Vec<(&'static str, Value)> {
    let routing = obj(vec![("model_policy_version", n(2))]);
    let defaults = obj(vec![
        (
            "defaults",
            obj(vec![
                ("route_id", s(format!("xr_{}", "1".repeat(24)))),
                ("route_connection_id", s(format!("cn_{}", "1".repeat(12)))),
                ("route_model", s(MODEL)),
                ("parameters", obj(vec![("thought_level", s(EFFORT))])),
                ("language", s("ru")),
            ]),
        ),
        (
            "ceiling",
            obj(vec![
                ("allowed_connections", null()),
                ("allowed_data_recipients", null()),
                ("allowed_routes", null()),
                (
                    "fallback_policy",
                    obj(vec![
                        ("allowed_failure_classes", null()),
                        ("allowed_target_routes", null()),
                        ("budget", null()),
                        ("data_recipient_consent", b(false)),
                    ]),
                ),
                ("limits", obj(vec![])),
                ("permission_level", null()),
                ("spend_limit", null()),
                ("spend_limit_enforceable", b(false)),
            ]),
        ),
    ]);
    let mut profiles = obj(vec![]);
    for name in PROFILE_NAMES {
        profiles.set(
            name,
            obj(vec![
                ("description", s(format!("Synthetic preservation fixture: {name}"))),
                ("created_at", s("2026-01-01T00:00:00Z")),
                ("updated_at", s("2026-01-01T00:00:00Z")),
                ("sections", arr(vec![s("routing"), s("user_defaults")])),
                ("settings", obj(vec![("routing", routing.clone()), ("user_defaults", defaults.clone())])),
            ]),
        );
    }
    let table = obj(vec![
        ("format", n(2)),
        ("active", s("default")),
        ("pending", null()),
        ("applied", obj(vec![("profile", s("default")), ("at", s("2026-01-01T00:00:00Z")), ("revisions", obj(vec![]))])),
        ("profiles", profiles),
    ]);
    vec![(FILES[0], routing), (FILES[1], defaults), (FILES[2], table)]
}

pub fn seed(home: &Path, channel: &Channel) -> Res<Vec<(String, Vec<u8>)>> {
    let folder = channel.config(home);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut parent = Some(folder.as_path());
    while let Some(p) = parent {
        if p == home {
            break;
        }
        chmod(p, 0o700)?;
        parent = p.parent();
    }
    let mut expected = Vec::new();
    for (name, value) in documents() {
        let data = pretty(&value)?.into_bytes();
        let path = folder.join(name);
        std::fs::write(&path, &data).map_err(|e| e.to_string())?;
        chmod(&path, 0o600)?;
        expected.push((name.to_string(), data));
    }
    Ok(expected)
}

fn semantic(data: &[u8]) -> Res<String> {
    Ok(canon(&parse(std::str::from_utf8(data).map_err(|e| e.to_string())?)?))
}

pub fn verify(home: &Path, channel: &Channel, expected: &[(String, Vec<u8>)], stage: &str) -> Res<Vec<(&'static str, Value)>> {
    let mut files = obj(vec![]);
    for name in FILES {
        let data = std::fs::read(channel.config(home).join(name)).map_err(|e| format!("{stage}: {name}: {e}"))?;
        let want = &expected.iter().find(|(k, _)| k == name).ok_or("missing expectation")?.1;
        if semantic(&data)? != semantic(want)? {
            return Err(format!("{stage}: orchestration config semantics changed: {name}"));
        }
        if &data != want {
            return Err(format!("{stage}: orchestration config bytes changed: {name}"));
        }
        files.set(name, obj(vec![("bytes_sha256", s(sha_bytes(&data))), ("semantic_sha256", s(sha_bytes(semantic(&data)?.as_bytes())))]));
    }
    Ok(vec![
        ("stage", s(stage)),
        ("channel", s(&channel.name)),
        ("files", files),
        ("active", s("default")),
        ("profiles", n(PROFILE_NAMES.len() as i64)),
        ("coordinator", s(MODEL)),
        ("effort", s(EFFORT)),
    ])
}

fn rows_of(output: &str, steps: &[&str]) -> Res<Vec<Value>> {
    let mut rows = Vec::new();
    for text in output.lines() {
        if steps.iter().any(|st| text.contains(&format!("\"step\": \"{st}\""))) {
            rows.push(parse(text).map_err(|e| format!("malformed evidence line: {e}"))?);
        }
    }
    Ok(rows)
}

/// Fail closed on missing, duplicate, wrong-channel or changed orchestration evidence.
pub fn require_evidence(output: &str, channel: &Channel, backend: &str, mode: &str) -> Res<()> {
    let rows = rows_of(output, &["orchestration_config_preserved", "orchestration_settings_visible"])?;
    let step = |r: &Value| r.get("step").and_then(Value::as_str).unwrap_or("").to_string();
    let preserved: Vec<&Value> = rows.iter().filter(|r| step(r) == "orchestration_config_preserved").collect();
    let mut stages: Vec<&str> = vec!["seeded"];
    if mode == "upgrade" {
        stages.extend(["prior-install", "before-upgrade"]);
    }
    stages.extend(["candidate-install", "first-launch", "unreadable-project-launch"]);
    let got: Vec<String> = preserved.iter().map(|r| r.get("stage").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    require(got == stages, "missing or duplicate config preservation evidence")?;
    let expected = documents();
    for row in &preserved {
        require(row.get("channel").and_then(Value::as_str) == Some(channel.name.as_str()), "wrong config preservation channel")?;
        let files = row.get("files").and_then(Value::as_obj).ok_or("incomplete config file evidence")?;
        let mut keys: Vec<&str> = files.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort();
        let mut want: Vec<&str> = FILES.to_vec();
        want.sort();
        require(keys == want, "incomplete config file evidence")?;
        for (name, value) in &expected {
            let byte_hash = sha_bytes(pretty(value)?.as_bytes());
            let semantic_hash = sha_bytes(canon(value).as_bytes());
            let have = row.get("files").and_then(|f| f.get(name)).ok_or("incomplete config file evidence")?;
            let ok = have.get("bytes_sha256").and_then(Value::as_str) == Some(byte_hash.as_str())
                && have.get("semantic_sha256").and_then(Value::as_str) == Some(semantic_hash.as_str())
                && have.as_obj().map(|o| o.len()) == Some(2);
            require(ok, "config preservation digest mismatch")?;
        }
    }
    let rendered: Vec<&Value> = rows.iter().filter(|r| step(r) == "orchestration_settings_visible").collect();
    let phases: Vec<String> = rendered.iter().map(|r| r.get("phase").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    require(phases == ["cold-start", "incident"], "missing or duplicate Settings evidence")?;
    for row in rendered {
        let text = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_string);
        let ok = text("channel").as_deref() == Some(channel.name.as_str())
            && text("backend").as_deref() == Some(backend)
            && text("active").as_deref() == Some("default")
            && row.get("profiles").and_then(Value::as_i64) == Some(6)
            && text("coordinator").as_deref() == Some(MODEL)
            && text("effort").as_deref() == Some(EFFORT);
        require(ok, "incorrect Settings evidence")?;
    }
    Ok(())
}

pub fn profiles_visible(nodes: &[Node]) -> bool {
    let visible: Vec<&Node> = nodes.iter().filter(|n| n.w > 0 && n.h > 0).collect();
    let mut names: Vec<(&str, &Node)> = Vec::new();
    for name in PROFILE_NAMES {
        let want = format!("Profile name: {name}");
        let found: Vec<&&Node> = visible.iter().filter(|n| n.name == want).collect();
        if found.len() != 1 {
            return false;
        }
        names.push((name, *found[0]));
    }
    let picker: Vec<&str> = visible.iter().filter(|n| n.name.starts_with("Profile: ")).map(|n| n.name.as_str()).collect();
    if picker != ["Profile: default"] {
        return false;
    }
    // WebKitGTK folds the plain "Active" label into a non-showing ancestor's text, so bind
    // activeness through row controls: only inactive rows offer "Activate…".
    let center = |n: &Node| n.y as f64 + n.h as f64 / 2.0;
    let mut rows: Vec<f64> = names.iter().map(|(_, n)| center(n)).collect();
    rows.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let reach = rows.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min) / 2.0;
    let row = |n: &Node| -> Option<&str> {
        let (near, at) = names
            .iter()
            .min_by(|a, b| (center(a.1) - center(n)).abs().partial_cmp(&(center(b.1) - center(n)).abs()).unwrap())
            .unwrap();
        ((center(at) - center(n)).abs() < reach).then_some(*near)
    };
    let mut activate = Vec::new();
    for n in visible.iter().filter(|n| n.name == "Activate…") {
        match row(n) {
            Some(r) => activate.push(r),
            None => return false,
        }
    }
    for name in PROFILE_NAMES {
        let want = format!("More for {name}");
        let found: Vec<Option<&str>> = visible.iter().filter(|n| n.name == want).map(|n| row(n)).collect();
        if found != [Some(name)] {
            return false;
        }
    }
    activate.sort();
    let mut others: Vec<&str> = PROFILE_NAMES.iter().copied().filter(|n| *n != "default").collect();
    others.sort();
    activate == others
}

pub fn coordinator_visible(nodes: &[Node]) -> bool {
    let visible: Vec<&Node> = nodes.iter().filter(|n| n.w > 0 && n.h > 0).collect();
    visible.iter().any(|n| n.role == "heading" && n.name == "Coordinator")
        && visible.iter().any(|n| format!("{} {}", n.name, n.text).contains(MODEL))
        && visible.iter().any(|n| n.name == "Route parameter value: thought_level" && n.text == EFFORT)
}

//! Retained-helper refusals answered natively: replies and state-file effects
//! compared against real `python3 -I -B machine_tools.py` captures
//! (tests/generate_machine_refusal_fixture.py), with no Python provider here.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::{Config, LaunchSpec, Service};

fn tree(state: &Path) -> BTreeSet<String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.insert(format!(
                "state/{}",
                path.strip_prefix(base).unwrap().to_str().unwrap()
            ));
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(state, state, &mut out);
    out
}
fn events(state: &Path) -> Vec<String> {
    fs::read_to_string(state.join("logs/service.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn retained_refusals_match_the_oracle_without_a_provider() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/machine-refusal-python.json")).unwrap();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 55);
    for case in cases {
        let base = std::env::temp_dir().join(format!(
            "cw-refuse-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let root = base.join("repo");
        fs::create_dir(&root).unwrap();
        let state = base.join("state");
        let service = Service::bind(Config {
            runtime_dir: base.join("run"),
            state_dir: state.clone(),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let id = service.register(&root, "Refusals").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = tree(&state);
        let logged = events(&state).len();
        let mut call = json!({"op":case["op"],"params":case["params"]});
        if case["project"] == true {
            call["projectId"] = json!(id);
        }
        let reply = service.machine(serde_json::from_value(call).unwrap()).await;
        let name = case["name"].as_str().unwrap();
        match &case["expected"] {
            // No JSON from the helper: the socket's ordinary RPC error.
            Value::Null => {
                let error = reply.expect_err(name);
                assert!(
                    error.to_string().contains("machine_tool_failed"),
                    "{name}: {error}"
                );
            }
            expected => assert_eq!(&reply.expect(name), expected, "{name}"),
        }
        assert!(
            !events(&state).is_empty() || case["expected"].is_null(),
            "{name}"
        );
        let after = tree(&state);
        let mut python: BTreeSet<String> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f[0].as_str())
            .filter(|f| f.starts_with("state/"))
            .map(str::to_owned)
            .collect();
        // The captures use the release channel. A dev-channel helper spawn
        // creates these private XDG roots before Python runs (child.rs).
        if workspace_protocol::channel::current().is_dev() {
            for leaf in ["cache", "config", "data", "state"] {
                if after.contains(&format!("state/{leaf}")) {
                    python.insert(format!("state/{leaf}"));
                }
            }
        }
        // Everything Python left exists natively; nothing else was created.
        assert!(python.is_subset(&after), "{name}: {python:?} {after:?}");
        assert!(
            after.difference(&before).all(|p| python.contains(p)),
            "{name}: {:?}",
            after.difference(&before).collect::<Vec<_>>()
        );
        let python_events: Vec<String> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f[0] == "state/logs/service.log")
            .map(|f| serde_json::from_value(f[1].clone()).unwrap())
            .unwrap_or_default();
        assert_eq!(events(&state)[logged..], python_events[..], "{name}");
        service.stop().await.unwrap();
        let _ = fs::remove_dir_all(&base);
    }
}

//! Real helper processes with deliberately slow inspections; no engines.
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use workspace_service::{Config, LaunchSpec, Service};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn inspections_cannot_starve_reads_or_owner_actions() {
    let root = std::env::temp_dir().join(format!("workspace-helper-load-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    let root = &fixture.0;
    let helper = root.join("helper.py");
    std::fs::write(
        &helper,
        r#"
import json, os, pathlib, sys, time
r=json.load(sys.stdin)
state=pathlib.Path(r["stateDir"])
if r["op"] == "repo.snapshot":
    (state / ("ready-" + str(os.getpid()))).touch()
    end=time.monotonic()+4
    while (state / "hold").exists() and time.monotonic()<end:
        time.sleep(.01)
print(json.dumps({"ok":True,"result":{}}))
"#,
    )
    .unwrap();
    let launch = LaunchSpec::new(
        PathBuf::from("/usr/bin/python3"),
        vec![Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fake_acp.py")
            .display()
            .to_string()],
    )
    .unwrap()
    .with_tools(helper)
    .unwrap();
    let config = Config {
        runtime_dir: root.join("runtime"),
        state_dir: root.join("state"),
        allowed_root: root.clone(),
        launch,
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(100),
        event_capacity: 16,
    };
    let service = Service::bind(config).await.unwrap();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let registered = service.register(&project, "Fixture").await.unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    std::fs::write(root.join("state/hold"), "").unwrap();
    let mut inspections = vec![];
    for _ in 0..4 {
        let service = service.clone();
        let id = id.clone();
        inspections.push(tokio::spawn(async move {
            service
                .machine(
                    serde_json::from_value(json!({"op":"repo.snapshot","projectId":id})).unwrap(),
                )
                .await
        }));
    }
    let end = Instant::now() + Duration::from_secs(2);
    while std::fs::read_dir(root.join("state"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("ready-"))
        .count()
        < 2
    {
        assert!(Instant::now() < end, "inspection fixture did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    let read = tokio::time::timeout(
        Duration::from_millis(700),
        service
            .machine(serde_json::from_value(json!({"op":"product.read","projectId":id})).unwrap()),
    )
    .await;
    let read_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let action = tokio::time::timeout(
        Duration::from_millis(700),
        service.machine(
            serde_json::from_value(json!({"op":"product.message","projectId":id})).unwrap(),
        ),
    )
    .await;
    let action_ms = started.elapsed().as_millis();
    std::fs::remove_file(root.join("state/hold")).unwrap();
    for inspection in inspections {
        inspection.await.unwrap().unwrap();
    }
    service.stop().await.unwrap();
    assert!(read.unwrap().unwrap()["ok"] == true);
    assert!(action.unwrap().unwrap()["ok"] == true);
    println!("saturated_inspections read_ms={read_ms} action_ms={action_ms}");
}

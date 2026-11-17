#[path = "support/hermetic.rs"]
mod hermetic;
use cortex_core::{OpenMode, ProjectKey, Store, StoreChannel};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
use workspace_service::{native_runtime::UnqualifiedRoutes, Config, LaunchSpec, Service};

/// The native host is the only execution host.
async fn bind(config: Config) -> std::io::Result<std::sync::Arc<Service>> {
    Service::bind_native(config, std::sync::Arc::new(UnqualifiedRoutes)).await
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "native-commands-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(p.join("project")).unwrap();
        Self(p)
    }
    fn config(&self, endpoint: &str) -> Config {
        Config {
            runtime_dir: self.0.join(endpoint),
            state_dir: self.0.join("state"),
            allowed_root: self.0.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 128,
        }
    }
    fn store(&self, id: &str) -> Store {
        let channel = if workspace_protocol::channel::current().is_dev() {
            StoreChannel::Dev
        } else {
            StoreChannel::Release
        };
        let root = self.0.join("project");
        let s = Store::open(
            ProjectKey::new(&root, channel).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        s.write(|db| {
            db.execute(
                "INSERT OR IGNORE INTO workspace_projects VALUES (?, ?, ?, 'now',1)",
                (
                    id,
                    root.to_str().unwrap(),
                    format!("uid:{}", unsafe { libc::getuid() }),
                ),
            )?;
            Ok(())
        })
        .unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn create(id: &str) -> Value {
    json!({"protocol":{"major":1,"minor":8},"projectId":id,"actorId":"forged","requestId":"draft",
        "operation":"tasks.create","target":id,"expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Native saved request","checkout":{"kind":"main"}}})
}
#[tokio::test]
async fn desktop_create_commits_natively_and_pushes_a_versioned_task() {
    let f = Fixture::new();
    let s = bind(f.config("runtime")).await.unwrap();
    let p = s.register(&f.0.join("project"), "Project").await.unwrap();
    let id = p["projectId"].as_str().unwrap();
    s.activate(id).await.unwrap();
    let store = f.store(id);
    let q = json!({"protocol":{"major":2,"mapping":2},"requestId":"snapshot","projectId":id,
        "operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let before = s.native_query(q.clone()).await.unwrap();
    assert_eq!(before["data"]["value"]["items"], json!([]));
    let mut events = s.subscribe();
    let runner = tokio::spawn(s.clone().serve());
    let receipt = s.workspace(create(id)).await.unwrap();
    assert_eq!(receipt["outcome"], "recorded");
    assert_eq!(s.workspace(create(id)).await.unwrap(), receipt);
    let delta = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = s.receive_event(&mut events).await.unwrap();
            if event["type"] == "native_delta" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(delta["batch"]["generation"], before["generation"]);
    assert!(delta["batch"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == receipt["target"] && c["operation"]["type"] == "upsert"));
    let after = s.native_query(q).await.unwrap();
    assert_eq!(after["data"]["value"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        after["data"]["value"]["items"][0]["taskId"],
        receipt["target"]
    );
    store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM runtime_tasks", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn second_service_cannot_open_the_same_project_channel() {
    let f = Fixture::new();
    let first = bind(f.config("first")).await.unwrap();
    let p = first
        .register(&f.0.join("project"), "Project")
        .await
        .unwrap();
    let id = p["projectId"].as_str().unwrap();
    first.activate(id).await.unwrap();
    let second = bind(f.config("second")).await.unwrap();
    assert_eq!(
        second.activate(id).await.err().unwrap().to_string(),
        "scheduler_owner_busy"
    );
    first.stop().await.unwrap();
    drop(first);
    second.activate(id).await.unwrap();
    second.stop().await.unwrap();
}
/// A native task's published report is readable through `reports.read` with bounded paging,
/// and only when the report belongs to a runtime task bound to a workspace task of this project.
#[tokio::test]
async fn native_reports_read_pages_a_bound_task_report_and_refuses_others() {
    use cortex_core::store::documents::{Publication, Publisher};
    let f = Fixture::new();
    let s = bind(f.config("runtime")).await.unwrap();
    let p = s.register(&f.0.join("project"), "Project").await.unwrap();
    let id = p["projectId"].as_str().unwrap();
    s.activate(id).await.unwrap();
    let store = f.store(id);
    let runner = tokio::spawn(s.clone().serve());
    let created = s.workspace(create(id)).await.unwrap();
    let task = created["target"].as_str().unwrap().to_owned();
    let machine = cortex_core::lifecycle::TaskMachine::new(&store);
    machine.create("bound-runtime", "Bound", "fixture").unwrap();
    machine.create("other-runtime", "Other", "fixture").unwrap();
    store
        .write(|db| {
            db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'bound-runtime','{}','{}','[]','now',NULL)", [f.0.join("project").to_str().unwrap()])?;
            db.execute("INSERT INTO workspace_executions VALUES ('e1',?,'bound-runtime','s1',1,1,'now')", [&task])?;
            db.execute("UPDATE workspace_tasks SET execution_id='e1' WHERE task_id=?", [&task])?;
            Ok(())
        })
        .unwrap();
    let api = Publisher::new(&store);
    let publish = |runtime: &str, key: &str| {
        let draft = api.create_draft(runtime, "owner", "general", key).unwrap();
        let marker = format!("Cortex draft ID: `{}`\n", draft.draft_id);
        let body = format!("{marker}\n# Report\n{}", "report body line\n".repeat(600));
        api.write_draft(runtime, "owner", &draft.draft_id, &body).unwrap();
        let published = api
            .publish(runtime, "owner", &draft.draft_id, key, Publication {
                title: "Worker report",
                summary: "summary",
                author: "worker",
                source_revision: 0,
                artifacts: json!([]),
            })
            .unwrap();
        (published.report_id, body.chars().count())
    };
    let (bound, total) = publish("bound-runtime", "k1");
    let (other, _) = publish("other-runtime", "k2");
    let read = |target: &str, cursor: Option<&str>| {
        let mut q = json!({"protocol":{"major":1,"minor":8},"requestId":"r","projectId":id,
            "operation":"reports.read","target":target,"limit":4000});
        if let Some(c) = cursor {
            q["cursor"] = json!(c);
        }
        s.workspace(q)
    };
    let first = read(&bound, None).await.unwrap();
    assert_eq!(first["reportId"], bound.as_str());
    assert_eq!(first["title"], "Worker report");
    assert_eq!(first["totalCharacters"], total);
    assert_eq!(first["markdown"].as_str().unwrap().chars().count(), 4000);
    assert_eq!(first["partial"], true);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = read(&bound, Some(&cursor)).await.unwrap();
    assert_eq!(second["reportId"], bound.as_str());
    assert_eq!(second["sha256"], first["sha256"]);
    assert!(second["markdown"].as_str().unwrap().chars().count() > 0);
    // A cursor is bound to its report; another task's report and unknown ids are not readable.
    assert!(read(&other, None).await.unwrap_err().to_string().contains("not_found"));
    assert!(read("r_missing", None).await.unwrap_err().to_string().contains("not_found"));
    assert!(read(&other, Some(&cursor)).await.is_err());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

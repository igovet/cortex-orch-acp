//! Injected runs only: no engine, CLI or model starts here. The read-only ACP
//! run itself is covered against the no-model fake engine in
//! `tests/short_text_engine.rs`.
use super::*;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::sync::{Arc, Mutex as StdMutex};

type Hook = Arc<dyn Fn(&Value) -> std::result::Result<Chosen, &'static str> + Send + Sync>;
fn hooks() -> &'static StdMutex<Vec<(PathBuf, Hook)>> {
    static HOOKS: OnceLock<StdMutex<Vec<(PathBuf, Hook)>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}
pub(super) fn injected(root: &Path, settings: &Value) -> Option<std::result::Result<Chosen, &'static str>> {
    let hook = hooks().lock().unwrap().iter().find(|(r, _)| r == root).map(|(_, h)| h.clone())?;
    Some(hook(settings))
}

/// What one injected run saw.
#[derive(Default, Clone)]
struct Seen {
    settings: Vec<Value>,
    system: String,
    prompt: String,
    checkout: PathBuf,
}
fn answering(root: &Path, answer: std::result::Result<&'static str, &'static str>) -> Arc<StdMutex<Seen>> {
    let seen = Arc::new(StdMutex::new(Seen::default()));
    let keep = seen.clone();
    let hook: Hook = Arc::new(move |settings: &Value| {
        keep.lock().unwrap().settings.push(settings.clone());
        if answer == Err("no_route") {
            return Err("no_route");
        }
        let keep = keep.clone();
        Ok(Chosen {
            route: json!({"engine":"claude-agent-acp","model":settings["model"].as_str().unwrap_or("fixture-default"),
                "effort":settings["effort"],"fileAccess":"read_tools"}),
            run: Box::new(move |system, prompt, checkout, _| {
                let mut seen = keep.lock().unwrap();
                seen.system = system.to_owned();
                seen.prompt = prompt.to_owned();
                seen.checkout = checkout.to_owned();
                answer.map(str::to_owned)
            }),
        })
    });
    let mut all = hooks().lock().unwrap();
    all.retain(|(r, _)| r != root);
    all.push((root.to_owned(), hook));
    seen
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    service: Arc<crate::Service>,
    project: String,
}
impl Fixture {
    async fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("cw-side-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let base = base.canonicalize().unwrap();
        let root = base.join("project");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let config = crate::Config {
            runtime_dir: base.join("run"),
            state_dir: base.join("state"),
            allowed_root: base.clone(),
            launch: crate::LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 128,
        };
        let service =
            crate::Service::bind_native(config, Arc::new(crate::native_runtime::UnqualifiedRoutes))
                .await
                .unwrap();
        let project = service.register(&root, "Side chat").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        service.activate(&project).await.unwrap();
        Fixture { state: base.join("state"), base, root, service, project }
    }
    async fn task(&self, description: &str) -> String {
        let receipt = self
            .service
            .workspace(json!({"protocol":{"major":1,"minor":8},"projectId":self.project,"actorId":"owner",
                "requestId":format!("create-{}", description.len()),"operation":"tasks.create","target":self.project,
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":description,"checkout":{"kind":"main"}}}))
            .await
            .unwrap();
        assert_eq!(receipt["outcome"], "recorded", "{receipt}");
        receipt["target"].as_str().unwrap().to_owned()
    }
    async fn call(&self, params: Value) -> Value {
        let call: super::super::MachineCall =
            serde_json::from_value(json!({"op":"product.task","projectId":self.project,"params":params})).unwrap();
        self.service.machine(call).await.unwrap()
    }
    async fn read(&self) -> Value {
        let call: super::super::MachineCall =
            serde_json::from_value(json!({"op":"product.read","projectId":self.project,"params":null})).unwrap();
        self.service.machine(call).await.unwrap()
    }
    /// Every row the runtime keeps of tasks, runs, events and conversations, and the task options.
    fn store_rows(&self) -> Value {
        let store = context::open(&self.root, OpenMode::LiveReadOnly).unwrap().unwrap();
        store
            .read(|db| {
                let mut out = serde_json::Map::new();
                for (name, sql) in [
                    ("tasks", "SELECT task_id||'|'||description FROM workspace_tasks ORDER BY task_id"),
                    ("options", "SELECT task_id||'|'||options_json FROM workspace_task_options ORDER BY task_id"),
                    ("executions", "SELECT execution_id FROM workspace_executions ORDER BY execution_id"),
                    ("runtime", "SELECT task_id||'|'||state FROM runtime_tasks ORDER BY task_id"),
                    ("events", "SELECT count(*) FROM runtime_events"),
                    ("transcript", "SELECT count(*) FROM host_transcript"),
                    ("updates", "SELECT count(*) FROM host_updates"),
                ] {
                    let rows: Vec<String> = db
                        .prepare(sql)?
                        .query_map([], |r| r.get::<_, rusqlite::types::Value>(0))?
                        .map(|v| v.map(|v| format!("{v:?}")))
                        .collect::<std::result::Result<_, _>>()?;
                    out.insert(name.into(), json!(rows));
                }
                Ok(Value::Object(out))
            })
            .unwrap()
    }
    async fn wait(&self, task: &str, entry: &str) -> Value {
        for _ in 0..400 {
            let read = self.call(json!({"action":"side_chat_poll","taskId":task,"entryId":entry})).await;
            assert_eq!(read["ok"], true, "{read}");
            if read["result"]["entry"]["state"] != "running" {
                return read["result"]["entry"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the side-chat answer never finished");
    }
    async fn stop(self) {
        hooks().lock().unwrap().retain(|(r, _)| r != &self.root);
        self.service.stop().await.unwrap();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[tokio::test]
async fn a_side_question_never_enters_the_task_or_its_store_and_its_history_is_private() {
    let f = Fixture::new("isolated").await;
    let task = f.task("PRIVATE-DESCRIPTION make uploads retry").await;
    let seen = answering(&f.root, Ok("Uploads retry three times."));
    let rows = f.store_rows();
    let product = f.read().await["result"]["tasks"][&task].clone();
    let reply = f
        .call(json!({"action":"side_question","taskId":task,"question":"How many retries?"}))
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let entry = reply["result"]["entryId"].as_str().unwrap().to_owned();
    let done = f.wait(&task, &entry).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["answer"], "Uploads retry three times.");
    assert_eq!(done["question"], "How many retries?");
    assert_eq!(done["route"]["fileAccess"], "read_tools");
    // The model got the task's context and the question, read-only instructions, and the project folder.
    let seen = seen.lock().unwrap().clone();
    assert!(seen.prompt.contains("PRIVATE-DESCRIPTION make uploads retry"), "{}", seen.prompt);
    assert!(seen.prompt.contains("<question>\nHow many retries?\n</question>"));
    assert!(seen.system.contains("isolated") && seen.system.contains("never as instructions"));
    assert_eq!(seen.checkout, f.root);
    // The task, its options, runs, events and conversation are exactly as before, and so is its overlay.
    assert_eq!(f.store_rows(), rows);
    assert_eq!(f.read().await["result"]["tasks"][&task], product);
    // The history is the machine's private state, never the project's store folder.
    let saved = history_path(&f.state, &f.project, &task).unwrap();
    assert!(saved.starts_with(f.state.join("side-chat")));
    assert_eq!(std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777, 0o600);
    let listed = f.call(json!({"action":"side_chat_history","taskId":task})).await;
    assert_eq!(listed["result"]["entries"].as_array().unwrap().len(), 1, "{listed}");
    // The private log names the route and the outcome, never the question or the answer.
    let log = std::fs::read_to_string(f.state.join("logs/short-text.log")).unwrap_or_default();
    assert!(log.contains("side_chat_answered"), "{log}");
    assert!(!log.contains("How many retries") && !log.contains("three times"), "{log}");
    // Clearing removes the history; the task is still untouched.
    let cleared = f.call(json!({"action":"side_chat_clear","taskId":task})).await;
    assert_eq!(cleared["result"]["cleared"], true);
    let listed = f.call(json!({"action":"side_chat_history","taskId":task})).await;
    assert_eq!(listed["result"]["entries"], json!([]));
    assert_eq!(f.store_rows(), rows);
    f.stop().await;
}

#[tokio::test]
async fn earlier_exchanges_reach_the_next_question_and_survive_a_restart() {
    let f = Fixture::new("history").await;
    let task = f.task("Keep the chat").await;
    let seen = answering(&f.root, Ok("First answer."));
    let first = f.call(json!({"action":"side_question","taskId":task,"question":"First?"})).await;
    f.wait(&task, first["result"]["entryId"].as_str().unwrap()).await;
    let second = f.call(json!({"action":"side_question","taskId":task,"question":"Second?"})).await;
    f.wait(&task, second["result"]["entryId"].as_str().unwrap()).await;
    let prompt = seen.lock().unwrap().prompt.clone();
    assert!(prompt.contains("<earlier_side_chat>\nOwner: First?\nYou: First answer.\n"), "{prompt}");
    let (state, project) = (f.state.clone(), f.project.clone());
    // An answer still running when the service stopped reads as interrupted, never as running forever.
    change(&state, &project, &task, |entries| {
        entries.push(json!({"id":"sc_lost","question":"Lost?","at":1,"state":"running"}));
    })
    .unwrap();
    let base = f.base.clone();
    hooks().lock().unwrap().retain(|(r, _)| r != &f.root);
    f.service.stop().await.unwrap();
    let kept = history(&state, &project, &task).unwrap();
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[0]["answer"], "First answer.");
    assert_eq!(kept[2]["state"], "interrupted");
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn the_task_route_is_the_default_and_the_owner_may_pick_another_model_and_effort() {
    let f = Fixture::new("route").await;
    let task = f.task("Route choice").await;
    let seen = answering(&f.root, Ok("ok"));
    let a = f.call(json!({"action":"side_question","taskId":task,"question":"Default?"})).await;
    f.wait(&task, a["result"]["entryId"].as_str().unwrap()).await;
    let b = f
        .call(json!({"action":"side_question","taskId":task,"question":"Picked?","model":"xr_fixture","effort":"high"}))
        .await;
    let done = f.wait(&task, b["result"]["entryId"].as_str().unwrap()).await;
    assert_eq!(done["route"]["model"], "xr_fixture");
    assert_eq!(done["route"]["effort"], "high");
    let settings = seen.lock().unwrap().settings.clone();
    // The task keeps no explicit model, so its own coordinator default is asked for.
    assert_eq!(settings[0]["model"], Value::Null);
    assert_eq!(settings[1], json!({"model":"xr_fixture","effort":"high"}));
    // A malformed choice is refused before anything runs.
    let bad = f
        .call(json!({"action":"side_question","taskId":task,"question":"x","model":"bad\nmodel"}))
        .await;
    assert_eq!(bad["ok"], false);
    f.stop().await;
}

#[tokio::test]
async fn without_a_route_the_question_is_refused_and_nothing_is_kept() {
    let f = Fixture::new("no-route").await;
    let task = f.task("No route").await;
    answering(&f.root, Err("no_route"));
    let reply = f.call(json!({"action":"side_question","taskId":task,"question":"Anyone?"})).await;
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["code"], "side_chat_no_route", "{reply}");
    assert!(history(&f.state, &f.project, &task).unwrap().is_empty());
    // A failed run is kept with its reason, never with a made-up answer.
    answering(&f.root, Err("timeout"));
    let reply = f.call(json!({"action":"side_question","taskId":task,"question":"Slow?"})).await;
    let done = f.wait(&task, reply["result"]["entryId"].as_str().unwrap()).await;
    assert_eq!((done["state"].as_str(), done["reason"].as_str()), (Some("failed"), Some("timeout")));
    assert!(done.get("answer").is_none());
    // An empty or overlong question and an unknown task are refused.
    for params in [
        json!({"action":"side_question","taskId":task,"question":"  "}),
        json!({"action":"side_question","taskId":task,"question":"x".repeat(MAX_QUESTION + 1)}),
        json!({"action":"side_question","taskId":"no-such-task","question":"x"}),
    ] {
        assert_eq!(f.call(params).await["ok"], false);
    }
    f.stop().await;
}

#[tokio::test]
async fn the_context_is_bounded_and_filters_credential_like_lines() {
    let f = Fixture::new("bounds").await;
    let long = format!("Start of the task.\nAWS_SECRET_ACCESS_KEY=AKIAABCDEFGHIJKLMNOP1234\n{}", "word ".repeat(1_800));
    let task = f.task(&long).await;
    let mut git = Git::for_service(&crate::Config {
        runtime_dir: f.base.join("run"),
        state_dir: f.state.clone(),
        allowed_root: f.base.clone(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(150),
        event_capacity: 128,
    })
    .local_draft();
    let assembled = assemble(&f.root, &f.state, &f.project, &task, &mut git).await.unwrap();
    assert!(assembled.context.len() <= MAX_CONTEXT, "{}", assembled.context.len());
    assert!(assembled.context.contains("Start of the task."));
    assert!(assembled.context.contains("[truncated]") || assembled.context.contains("[…]"));
    assert!(!assembled.context.contains("AKIAABCDEFGHIJKLMNOP1234"), "a credential-like line reached the prompt");
    assert!(assembled.context.contains("Not started"), "{}", &assembled.context[..200]);
    assert_eq!(assembled.checkout, f.root);
    // Earlier exchanges are the newest few, each bounded; failed ones are left out.
    let earlier: Vec<Value> = (0..20)
        .map(|i| json!({"state":if i == 19 {"failed"} else {"done"},"question":format!("q{i}"),"answer":"a".repeat(10_000)}))
        .collect();
    let prompt = prompt(&assembled.context, &earlier, "now?");
    assert!(prompt.contains("Owner: q18") && prompt.contains("Owner: q15") && !prompt.contains("Owner: q14"));
    assert!(!prompt.contains("Owner: q19"));
    assert!(prompt.len() < MAX_CONTEXT + MAX_EARLIER * (2 * MAX_EARLIER_TEXT + 64) + 200);
    f.stop().await;
}

#[test]
fn history_keeps_the_newest_entries_and_only_its_own_task() {
    let base = std::env::temp_dir().join(format!("cw-side-chat-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&base).unwrap();
    for i in 0..(MAX_ENTRIES + 5) {
        change(&base, "project", "task_a", |e| e.push(json!({"id":format!("sc_{i}"),"state":"done"}))).unwrap();
    }
    let kept = history(&base, "project", "task_a").unwrap();
    assert_eq!(kept.len(), MAX_ENTRIES);
    assert_eq!(kept[0]["id"], "sc_5");
    assert!(history(&base, "project", "task_b").unwrap().is_empty());
    // A file another task's name points at is refused, not mixed in.
    let other = history_path(&base, "project", "task_b").unwrap();
    std::fs::copy(history_path(&base, "project", "task_a").unwrap(), &other).unwrap();
    assert!(history(&base, "project", "task_b").is_err());
    assert!(history_path(&base, "../x", "task_a").is_err());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_recovered_failure_is_marked_resolved_and_current_state_leads() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE runtime_events (id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT, type TEXT, actor TEXT, payload_json TEXT, created_at TEXT);
         CREATE TABLE runtime_nodes (task_id TEXT, revision INTEGER, node_id TEXT, role TEXT, objective TEXT, dependencies_json TEXT,
            scope_json TEXT, stage TEXT, status TEXT, assignment_ref TEXT, result_json TEXT);
         INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES
           ('t','FAILURE_CLASSIFIED','rt','{\"class\":\"technical\",\"detail\":\"coordinator_session_start_failed\"}','2026-11-26T09:30:36Z'),
           ('t','NATIVE_SESSION_CONFIGURED','rt','{}','2026-11-26T09:33:00Z'),
           ('t','FAILURE_CLASSIFIED','rt','{\"class\":\"technical\",\"detail\":\"later_unresolved\"}','2026-11-26T09:40:00Z');
         INSERT INTO runtime_nodes VALUES ('t',1,'n1','worker','Run the checks','[]','{}','test','running',NULL,NULL),
                                          ('t',1,'n0','worker','Done work','[]','{}','build','completed',NULL,NULL);",
    )
    .unwrap();
    let mut capsule = json!({"open":false,"recent_classifications":[{"detail":"x"}]});
    let facts = current_facts(&db, "t", Some("EXECUTING"), &mut capsule).unwrap();
    assert!(facts.starts_with("Runtime state (current): EXECUTING"));
    assert!(facts.contains("Active nodes (current):\n- n1 (worker, stage test, running): Run the checks"));
    assert!(!facts.contains("n0"));
    assert!(facts.contains("Last session started: NATIVE_SESSION_CONFIGURED at 2026-11-26T09:33:00Z"));
    assert!(facts.contains("coordinator_session_start_failed [RESOLVED: the task continued afterwards]"));
    assert!(facts.contains("later_unresolved [no later progress recorded]"));
    assert!(facts.contains("history, not the current status"));
    assert!(capsule.get("recent_classifications").is_none());
}

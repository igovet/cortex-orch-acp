//! Custom harness: this executable is also the disposable ACP peer and relay.
#[path = "support/hermetic.rs"]
mod hermetic;
#[path = "support/command_acceptance.rs"]
mod command_acceptance;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;
fn verification_available(root: &std::path::Path) -> bool {
    match workspace_service::native_command_supervisor::observe_command_view(root, "read_only") {
        Ok(()) => true,
        Err(e) => {
            assert!(
                e.to_string()
                    .starts_with("native_command_sandbox_unavailable:")
                    || e.raw_os_error() == Some(libc::ENOTSUP),
                "{e}"
            );
            false
        }
    }
}
use cortex_core::{
    lifecycle::{domain, runtime::RuntimeAction},
    OpenMode, ProjectKey, Store, StoreChannel, TaskState,
};
use cortex_engine::environment::{CodexLauncher, EngineKind, LaunchSpec};
use serde_json::{json, Value};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use workspace_service::native_sessions::{NativeHost, SessionRole, SessionRoute};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        Self::new_at(
            &std::env::temp_dir(),
            "native-session-long-project-path-regression",
        )
    }
    fn new_at(parent: &std::path::Path, prefix: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = parent.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn key(&self) -> ProjectKey {
        ProjectKey::new(&self.0, StoreChannel::Dev).unwrap()
    }
    fn store(&self) -> Store {
        Store::open(self.key(), OpenMode::RuntimeValidate).unwrap()
    }
    async fn host(&self) -> NativeHost {
        let key = self.key();
        tokio::task::spawn_blocking(move || NativeHost::open(key))
            .await
            .unwrap()
            .unwrap()
    }
    // Native-write attribution fixtures need an engine that still permits native writes.
    fn native_writer_route(&self) -> SessionRoute {
        let mut route = self.route();
        route.launch.engine = EngineKind::OpenCode;
        let credentials = self.0.join(".cortex-dev/opencode");
        std::fs::create_dir(&credentials).unwrap();
        route.launch.credential_directory = credentials;
        route
    }
    fn legacy_route(&self) -> SessionRoute {
        let mut route = self.route();
        route.launch.engine = EngineKind::Claude;
        route
    }
    fn route(&self) -> SessionRoute {
        let executable = std::env::current_exe().unwrap();
        SessionRoute {
            launch: LaunchSpec {
                engine: EngineKind::Codex,
                executable: executable.clone(),
                args: vec!["native-host".into()],
                cwd: self.0.clone(),
                native_home: self.0.clone(),
                credential_directory: self.0.clone(),
                base_environment: BTreeMap::new(),
                codex_launcher: None,
                confinement: None,
            },
            relay: executable,
            broker_parent: self.0.join(".cortex-dev"),
            connection: "fake-only".into(),
            connection_limit: 4,
            model: "fixture-model".into(),
            parameters: BTreeMap::new(),
            instructions: "Fixture instructions; no model or credentials.".into(),
            deadline: Duration::from_secs(2),
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn call(name: &str, args: Value) -> String {
    format!("call:{}", json!({"name":name,"arguments":args}))
}
/// An inline fixture report completed with every required report section of every
/// template and of a review, so the presence check passes whatever the role.
fn with_sections(mut args: Value) -> Value {
    let Some(report) = args["report"]
        .as_str()
        .filter(|r| !r.starts_with("r_") || r.len() != 14)
    else {
        return args;
    };
    let mut report = report.to_owned();
    let mut seen = std::collections::BTreeSet::new();
    for (template, stage) in [
        ("general", "execution"),
        ("implementation", "execution"),
        ("investigation", "execution"),
        ("verification", "execution"),
        ("planning", "execution"),
        ("documentation", "execution"),
        ("general", "review"),
    ] {
        for s in cortex_core::assignment_brief::sections(template, stage)
            .iter()
            .filter(|s| s.required)
        {
            if seen.insert(s.heading) && !report.contains(&format!("## {}", s.heading)) {
                report.push_str(&format!("\n\n## {}\n\nFixture section.", s.heading));
            }
        }
    }
    args["report"] = json!(report);
    args
}
/// A reviewer finishes its own coverage: the journey's changed file and every
/// completed report of the current revision (implementation and earlier review) are read before the verdict, or
/// result_submit refuses it with exactly what is still unread.
async fn review_reads(
    p: &Project,
    review: &workspace_service::native_sessions::SessionDriver,
    task: &str,
    assignment: &str,
) {
    let (paths, reports): (Vec<String>, Vec<String>) = p
        .store()
        .read(|db| {
            let packet: String = db.query_row(
                "SELECT coalesce((SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET'
                 AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1),'{}')",
                (task, assignment),
                |r| r.get(0),
            )?;
            let packet: Value = serde_json::from_str(&packet)?;
            let paths = packet["packet"]["changed_files"]["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["current_sha256"].is_string())
                // Only the task's own changes are required reading.
                .filter(|f| f["required"] != false)
                .filter_map(|f| f["path"].as_str().map(str::to_owned))
                .collect();
            let reports = db
                .prepare(
                    "SELECT json_extract(n.result_json,'$.report_ref') FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                     WHERE n.task_id=? AND n.revision=t.revision
                     AND json_type(n.result_json,'$.report_ref')='text' ORDER BY n.rowid",
                )?
                .query_map([task], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((paths, reports))
        })
        .unwrap();
    for path in paths {
        review
            .prompt(&call("project_read", json!({"path":path})))
            .await
            .unwrap();
    }
    for report in reports {
        review
            .prompt(&call("read_report", json!({"report_id":report})))
            .await
            .unwrap();
    }
}
/// Terminal result revocation may close ACP before end_turn. Only an exact
/// durable host receipt confirms the result; transport loss alone never passes.
async fn submit_result(
    h: &NativeHost,
    d: &workspace_service::native_sessions::SessionDriver,
    task: &str,
    owner: &cortex_core::ownership::OwnerEpoch,
    assignment: &str,
    args: Value,
) {
    let key = args["request_key"].as_str().unwrap().to_owned();
    let args = with_sections(args);
    match d.prompt(&call("result_submit", args)).await {
        Ok(turn) => assert_eq!(turn.stop_reason, "end_turn"),
        Err(e) => assert_eq!(e.code(), "native_turn_outcome_unknown"),
    }
    let receipt = h
        .result_receipt(task.into(), owner.clone(), assignment.into(), key)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("exact result receipt missing for {assignment}"));
    assert_eq!(receipt["assignment_ref"], assignment);
    assert_eq!(receipt["outcome"], "recorded");
    assert_eq!(receipt["certified"], false);
}
async fn close(host: NativeHost) {
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
}
async fn document_tools_through_engine_and_relay() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Document evidence".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    async fn checked(
        d: &workspace_service::native_sessions::SessionDriver,
        name: &str,
        args: Value,
        expected: Value,
        stop: &str,
    ) {
        let prompt = format!(
            "call:{}",
            json!({"name":name,"arguments":args,"__fixture_expect":expected})
        );
        assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, stop);
    }
    let create = json!({"template":"implementation","request_key":"draft"});
    checked(
        &d,
        "create_draft",
        create.clone(),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "create_draft",
        create,
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    let draft = s
        .read(|db| Ok(db.query_row("SELECT id FROM drafts", [], |r| r.get::<_, String>(0))?))
        .unwrap();
    assert_eq!(
        s.read(|db| Ok(db.query_row("SELECT count(*) FROM drafts", [], |r| r.get::<_, i64>(0))?))
            .unwrap(),
        1
    );
    checked(
        &d,
        "create_draft",
        json!({"template":"general","request_key":"draft"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"payload_conflict","/result/structuredContent/field":"request_key"}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "create_draft",
        // The pipeline template is not offered: the schema refuses it before the store.
        json!({"template":"pipeline","request_key":"pipeline"}),
        json!({"/error/code":-32602,"/error/data/code":"invalid_arguments"}),
        "refusal",
    )
    .await;
    checked(
        &d,
        "list_reports",
        json!({}),
        json!({"/result/structuredContent/own_drafts/0/draft_id":draft,
        "/result/structuredContent/reports":[],"/result/structuredContent/next_cursor":null}),
        "end_turn",
    )
    .await;
    let body = format!(
        "Cortex draft ID: `{draft}`\n\n# Report\n\n{}",
        "évidence ".repeat(700)
    );
    let write = json!({"draft_id":draft,"content":body,"request_key":"write"});
    checked(
        &d,
        "draft_write",
        write.clone(),
        json!({"/result/structuredContent/outcome":"written"}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "draft_write",
        write,
        json!({"/result/structuredContent/outcome":"written"}),
        "end_turn",
    )
    .await;
    let newer = format!("{body}\nLater edit.");
    checked(
        &d,
        "draft_write",
        json!({"draft_id":draft,"content":newer,"request_key":"newer"}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    // An old acknowledged replacement must not overwrite a later edit.
    checked(
        &d,
        "draft_write",
        json!({"draft_id":draft,"content":body,"request_key":"write"}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "draft_write",
        json!({"draft_id":draft,"content":"changed","request_key":"write"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"payload_conflict"}),
        "end_turn",
    )
    .await;
    let body = newer;
    checked(
        &d,
        "read_draft",
        json!({"draft_id":draft,"limit":1}),
        json!({"/result/structuredContent/total_characters":body.chars().count()}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "read_draft",
        json!({"draft_id":draft,"limit":80}),
        json!({"/result/structuredContent/markdown":body.chars().take(80).collect::<String>()}),
        "end_turn",
    )
    .await;
    let publish = json!({"draft_id":draft,"title":"Fixture report","summary":"Fixture evidence",
        "author":"coordinator","request_key":"publish"});
    checked(
        &d,
        "write_report",
        publish.clone(),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "write_report",
        publish,
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    let report = s
        .read(|db| Ok(db.query_row("SELECT id FROM reports", [], |r| r.get::<_, String>(0))?))
        .unwrap();
    checked(&d,"read_report",json!({"report_id":report,"limit":80}),
        json!({"/result/structuredContent/markdown":body.chars().take(80).collect::<String>(),
            "/result/structuredContent/source_revision":0,"/result/structuredContent/title":"Fixture report"}),"end_turn").await;
    let cursor = cortex_core::store::documents::Publisher::new(&s)
        .read_report("t", &report, None, 80)
        .unwrap()
        .next_cursor
        .unwrap();
    checked(&d,"read_report",json!({"cursor":cursor,"limit":80}),
        json!({"/result/structuredContent/report_id":report,
            "/result/structuredContent/markdown":body.chars().skip(80).take(80).collect::<String>()}),"end_turn").await;
    checked(&d,"list_reports",json!({"limit":1}),json!({
        "/result/structuredContent/reports/0/report_id":report,"/result/structuredContent/own_drafts":[],
        "/result/structuredContent/changes/0/reference":report}),"end_turn").await;
    checked(
        &d,
        "tools_discover",
        json!({"name":"read_report"}),
        json!({"/result/structuredContent/tool/name":"read_report",
            "/result/structuredContent/tool/inputSchema/properties/limit/maximum":4000}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "tools_discover",
        json!({"name":"command_run"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"tool_not_available"}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "tools_discover",
        json!({"limit":1}),
        json!({"/result/structuredContent/tools/0/read_batch_supported":false}),
        "end_turn",
    )
    .await;
    // End this principal without changing task lifecycle. A worker receives only
    // its own drafts and cannot submit the coordinator's published report.
    d.shutdown().await.unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        Ok(())
    }).unwrap();
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    checked(
        &w,
        "read_draft",
        json!({"draft_id":draft}),
        json!({"/result/isError":true,"/result/structuredContent/field":"draft_id"}),
        "end_turn",
    )
    .await;
    // A foreign report is a correctable refusal: the worker reads why, nothing is recorded,
    // and the refusal is retained by its code for the owner.
    checked(
        &w,
        "result_submit",
        json!({"report":report,"summary":"Not mine","checks":[],"request_key":"foreign-result"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"result_report_not_owned",
            "/result/structuredContent/recorded":false}),
        "end_turn",
    )
    .await;
    assert_eq!(
        s.read(|db| Ok(db.query_row(
            "SELECT json_extract(payload_json,'$.code') FROM runtime_events WHERE type='WORKER_RESULT_REFUSED'",
            [],
            |r| r.get::<_, String>(0),
        )?))
        .unwrap(),
        "result_report_not_owned"
    );
    checked(
        &w,
        "read_report",
        json!({"report_id":report,"limit":80}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "create_draft",
        json!({"template":"implementation","request_key":"worker-draft"}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    let own = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM drafts WHERE published_report_id IS NULL",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    // A published report the result names is checked for its required sections too.
    checked(&w,"draft_write",json!({"draft_id":own,"content":format!("Cortex draft ID: `{own}`\n\n# Worker\n\nObserved.\n\n## Changes\n\nNone.\n\n## Verification\n\nNone run.\n\n## Risks and remaining work\n\nNone."),
        "request_key":"worker-write"}),json!({"/result/isError":false}),"end_turn").await;
    checked(
        &w,
        "write_report",
        json!({"draft_id":own,"title":"Worker report","summary":"Worker evidence",
        "author":"backend-dev","request_key":"worker-publish"}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    let own_report = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT published_report_id FROM drafts WHERE id=?",
                [&own],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    submit_result(
        &h,
        &w,
        "t",
        &o,
        "a",
        json!({"report":own_report,
        "summary":"Own published evidence","checks":[],"request_key":"own-result"}),
    )
    .await;
    w.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
async fn channel_source_project_and_commands() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    std::fs::create_dir(p.0.join("src")).unwrap();
    std::fs::write(p.0.join("src/value"), "one\ntwo\n").unwrap();
    std::fs::write(p.0.join("src/large"), "x".repeat(70000)).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "src/value"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&p.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success());
    }
    let h = p.host().await;
    let o = h
        .create("t".into(), "Tool parity fixture".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    let source = "r_111111111111";
    let source_dir = p.0.join(".cortex-dev/t");
    std::fs::create_dir_all(source_dir.join(".source_blocks")).unwrap();
    std::fs::set_permissions(&source_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let huge = "line\n".repeat(2_100_000);
    assert!(huge.len() > 10 * 1024 * 1024);
    let huge_path = source_dir.join("r_222222222222.md");
    std::fs::write(&huge_path, &huge).unwrap();
    std::fs::set_permissions(&huge_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(p.0.join("src/huge"), &huge).unwrap();
    let body = "Owner text αβ";
    let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
    let file = source_dir.join(format!("{source}.md"));
    std::fs::write(&file, body).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let image = b"\x89PNG\r\n\x1a\n";
    let image_file = source_dir.join(".source_blocks/image.bin");
    std::fs::write(&image_file, image).unwrap();
    std::fs::set_permissions(&image_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    s.write(|db|{
        db.execute("INSERT INTO reports VALUES (?,'t','source',?,'fixture')",(source,format!("{source}.md")))?;
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,'Input','Input','user','fixture',?,?)",rusqlite::params![source,hash,body.len() as i64])?;
        db.execute("INSERT INTO source_revisions VALUES ('t',1,'client','message',?,'[]')",[source])?;
        db.execute("INSERT INTO source_blocks VALUES ('t',?,0,'image','image/png','fixture',NULL,?,8,'stored',NULL,'user_direct','image.bin',NULL,'fixture')",(source,format!("{:x}",Sha256::digest(image))))?;
        db.execute("INSERT INTO pending_questions VALUES ('pq_fixture','t',NULL,'fixture','Choose','[]','answered','Exact answer','text','fixture','fixture')",[])?;
        db.execute("INSERT INTO pending_questions VALUES ('pq_unknown','t',NULL,'runtime:missing:fixture','Choose','[]','answered','Legacy answer','elicitation','fixture','fixture')",[])?;
        db.execute("INSERT INTO task_events(task_id,event_type,payload_json,dedup_key,created_at,origin) VALUES ('t','progress',?,'question_answered:pq_unknown','fixture','runtime')",
            [json!({"session_id":"legacy-answer-session"}).to_string()])?;
        db.execute("INSERT INTO task_events(id,task_id,event_type,payload_json,dedup_key,created_at,origin) VALUES (777001,'t','user_message',?,'constraint:fixture','fixture','owner')",
            [json!({"text":"Owner constraint ξ"}).to_string()])?;
        db.execute("INSERT INTO task_events(id,task_id,event_type,payload_json,dedup_key,created_at,origin) VALUES (777002,'t','progress',?,'progress:fixture','fixture','runtime')",
            [json!({"text":"not a constraint"}).to_string()])?;
        db.execute("INSERT INTO reports VALUES ('r_222222222222','t','source','r_222222222222.md','fixture')",[])?;
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES ('r_222222222222','Large','Large','user','fixture',?,?)",rusqlite::params![format!("{:x}",Sha256::digest(huge.as_bytes())),huge.len() as i64])?;
        db.execute("INSERT INTO source_revisions VALUES ('t',2,'client','large','r_222222222222','[]')",[])?;
        db.execute("INSERT INTO source_blocks VALUES ('t',?,1,'resource_link',NULL,'gone',?,NULL,NULL,'reference_only',NULL,'user_direct',NULL,NULL,'fixture')",
            (source,format!("file://{}",p.0.join("gone").display())))?;
        Ok(())
    }).unwrap();
    async fn check(
        d: &workspace_service::native_sessions::SessionDriver,
        n: &str,
        a: Value,
        e: Value,
    ) {
        let target = a["request_key"].as_str().or_else(|| a["path"].as_str()).unwrap_or("-");
        let prompt = format!(
            "call:{}",
            json!({"name":n,"arguments":a,"__fixture_expect":e})
        );
        match tokio::time::timeout(Duration::from_secs(20), d.prompt(&prompt)).await {
            Err(_) => { eprintln!("NOREPLY {n} {target}"); }
            Ok(v) => { v.unwrap_or_else(|e| panic!("{n} fixture action {target}: {e:?}")); }
        }
    }
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    check(
        &d,
        "list_sources",
        json!({"limit":1}),
        json!({"/result/structuredContent/entries/0/source_ref":format!("{source}:-1"),
        "/result/structuredContent/answered_questions/0/source_ref":"pq_fixture:answer"}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":format!("{source}:-1"),"offset":11,"limit":2}),
        json!({"/result/structuredContent/chunk":"α"}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":format!("{source}:0"),"offset":999,"limit":1}),
        json!({"/result/content/1/type":"image","/result/content/1/mimeType":"image/png"}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":"pq_fixture:answer"}),
        json!({"/result/structuredContent/chunk":"Exact answer"}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":"r_000000000000"}),
        json!({"/result/isError":true}),
    )
    .await;
    check(&d,"read_source",json!({"source_ref":"r_222222222222:-1","offset":huge.len()-5,"limit":5}),
        json!({"/result/structuredContent/chunk":"line\n","/result/structuredContent/total_size":huge.len(),"/result/structuredContent/next_offset":null})).await;
    check(&d,"read_source",json!({"source_ref":"pq_unknown:answer"}),
        json!({"/result/structuredContent/asked_by":"unknown","/result/structuredContent/session_id":"legacy-answer-session","/result/structuredContent/session_binding":"recorded"})).await;
    // A constraint dropped from a recovery capsule is read back in full by event id.
    check(&d,"read_source",json!({"source_ref":"777001:constraint"}),
        json!({"/result/structuredContent/chunk":"Owner constraint ξ","/result/structuredContent/trust_label":"user_direct"})).await;
    check(
        &d,
        "read_source",
        json!({"source_ref":"777001:constraint","offset":17,"limit":2}),
        json!({"/result/structuredContent/chunk":"ξ","/result/structuredContent/next_offset":null}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":"777002:constraint"}),
        json!({"/result/isError":true}),
    )
    .await;
    check(
        &d,
        "read_source",
        json!({"source_ref":"999:constraint"}),
        json!({"/result/isError":true}),
    )
    .await;
    check(&d,"read_source",json!({"source_ref":format!("{source}:1")}),
        json!({"/result/structuredContent/availability":"reference_only","/result/structuredContent/chunk":""})).await;
    s.read(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM source_block_reads WHERE availability='inaccessible'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT availability FROM source_blocks WHERE block_index=1",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "reference_only"
        );
        Ok(())
    })
    .unwrap();
    check(&d,"project_read",json!({"path":"src/huge","offset":2_100_000,"limit":1,"line_numbers":false}),
        json!({"/result/structuredContent/content":"line\n","/result/structuredContent/total_lines":2_100_000})).await;
    check(
        &d,
        "project_read",
        json!({"path":"src/value","limit":1}),
        json!({"/result/structuredContent/read_coverage_complete":false}),
    )
    .await;
    check(
        &d,
        "project_read",
        json!({"path":".cortex-dev/cortex.sqlite3"}),
        json!({"/result/isError":true}),
    )
    .await;
    // Oversized catalogues are retained whole, not clipped or silently omitted.
    s.write(|db| { for index in 0..80 {
        db.execute("INSERT INTO source_blocks VALUES ('t','r_222222222222',?,'resource_link',NULL,?,NULL,NULL,NULL,'reference_only',NULL,'user_direct',NULL,NULL,'fixture')",
            (index,"descriptor".repeat(100)))?;
    } Ok(()) }).unwrap();
    check(&d,"list_sources",json!({"after":1,"limit":1}),json!({"/result/structuredContent/deferred":true,"/result/structuredContent/complete":false})).await;
    let deferred:String=s.read(|db|Ok(db.query_row("SELECT id FROM large_outputs WHERE source_kind='tool_result' ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0))?)).unwrap();
    check(&d,"read_large_output",json!({"large_output_id":deferred,"limit":100}),json!({"/result/structuredContent/encoding":"utf-8","/result/structuredContent/integrity_verified":true})).await;
    // A real retained legacy-format row uses its own directory/status/byte contract.
    std::fs::create_dir(source_dir.join("large_outputs")).unwrap();
    std::fs::set_permissions(
        source_dir.join("large_outputs"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let legacy = source_dir.join("large_outputs/lo_fixture.bin");
    std::fs::write(&legacy, [255u8, 0]).unwrap();
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
    s.write(|db| {db.execute("INSERT INTO large_outputs VALUES ('lo_fixture','t','coordinator','terminal','legacy-ref','lo_fixture.bin',2,0,7,NULL,NULL,'fixture')",[])?;Ok(())}).unwrap();
    d.shutdown().await.unwrap();
    let replacement = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    check(
        &replacement,
        "read_large_output",
        json!({"large_output_id":deferred,"limit":10}),
        json!({"/result/structuredContent/integrity_verified":true}),
    )
    .await;
    check(&replacement,"read_large_output",json!({"large_output_id":"lo_fixture"}),json!({"/result/structuredContent/encoding":"base64","/result/structuredContent/chunk":"/wA=","/result/structuredContent/exit_status/exit_code":7,"/result/structuredContent/integrity_verified":false})).await;
    replacement.shutdown().await.unwrap();
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\",\".gitignore\"]','execution','running','a',NULL)",[])?;
        cortex_core::access::switch_tx(db,"t","development","fixture-development","acp_set_mode")
    }).unwrap();
    let mut route = p.legacy_route();
    // Unlike the old fake route, credentials must not alias the entire task tree.
    let account = p.0.join("fixture-account");
    std::fs::create_dir(&account).unwrap();
    route.launch.credential_directory = account;
    route.deadline = Duration::from_secs(5);
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap();
    let hash = format!("{:x}", Sha256::digest(b"one\ntwo\n"));
    // Knowing the exact digest is not authority to replace unread bytes.
    check(
        &w,
        "project_write",
        json!({"path":"src/value","content":"bad","expected_sha256":hash}),
        json!({"/result/structuredContent/error":"gateway_complete_read_required",
            "/result/structuredContent/write_state":"rejected"}),
    )
    .await;
    // Creation needs the absent precondition, not a text-read ledger entry.
    check(
        &w,
        "project_write",
        json!({"path":"fresh/parents/file","content":"created","expected_sha256":hash}),
        json!({"/result/structuredContent/error":"gateway_write_conflict"}),
    )
    .await;
    assert!(!p.0.join("fresh").exists());
    check(
        &w,
        "project_write",
        json!({"path":"fresh/parents/file","content":"created","expected_sha256":"absent"}),
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    // An owner `deny-write:` recorded while this worker runs refuses its next write
    // to that path at once, in the normalized form the message parser stores.
    s.write(|db| {
        let denied = cortex_core::lifecycle::steering::parse_denials("Stop there. deny-write: ./fresh//denied")?;
        cortex_core::lifecycle::event_tx(db, "t", "USER_STEERING",
            json!({"text":"Stop there. deny-write: ./fresh//denied","request":"deny","deny_write":denied,"delivery":"restart"}))?;
        Ok(())
    })
    .unwrap();
    check(
        &w,
        "project_write",
        json!({"path":"fresh/denied/file","content":"denied","expected_sha256":"absent"}),
        json!({"/result/isError":true}),
    )
    .await;
    assert!(
        !p.0.join("fresh/denied").exists(),
        "a write to a denied path happened"
    );
    check(
        &w,
        "project_write",
        json!({"path":"fresh/parents/file","content":"bad","expected_sha256":"absent"}),
        json!({"/result/structuredContent/error":"gateway_write_conflict"}),
    )
    .await;
    std::fs::write(p.0.join("fresh/parents/file"), "external").unwrap();
    check(&w,"project_write",
        json!({"path":"fresh/parents/file","content":"bad","expected_sha256":format!("{:x}",Sha256::digest(b"external"))}),
        json!({"/result/structuredContent/error":"gateway_stale_read","/result/structuredContent/write_state":"rejected"})).await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("fresh/parents/file")).unwrap(),
        "external"
    );
    check(
        &w,
        "project_read",
        json!({"path":"src/value","limit":1}),
        json!({"/result/structuredContent/read_coverage_complete":false}),
    )
    .await;
    check(
        &w,
        "project_write",
        json!({"path":"src/value","content":"bad","expected_sha256":hash,"request_key":"partial-write"}),
        json!({"/result/structuredContent/error":"gateway_complete_read_required","/result/structuredContent/write_state":"rejected"}),
    )
    .await;
    check(
        &w,
        "project_edit",
        json!({"path":"src/value","edits":[{"old_string":"absent text","new_string":"ONE"}]}),
        json!({"/result/structuredContent/code":"gateway_edit_not_found","/result/structuredContent/current_read/sha256":hash,"/result/structuredContent/field":"edits[0].old_string"}),
    )
    .await;
    check(&w,"project_edit",json!({"path":"src/value","edits":[{"old_string":"one","new_string":"ONE"}],"request_key":"edit"}),json!({"/result/structuredContent/outcome":"written"})).await;
    std::fs::write(p.0.join("src/value"), "later").unwrap();
    check(&w,"project_edit",json!({"path":"src/value","edits":[{"old_string":"one","new_string":"ONE"}],"request_key":"edit"}),json!({"/result/structuredContent/outcome":"written"})).await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("src/value")).unwrap(),
        "later"
    );
    check(
        &w,
        "project_edit",
        json!({"path":"src/value","edits":[{"old_string":"later","new_string":"stale"}]}),
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(p.0.join("src/value")).unwrap(), "stale");
    std::fs::write(p.0.join("src/value"), "later").unwrap();
    check(
        &w,
        "project_read",
        json!({"path":"nested/new"}),
        json!({"/result/structuredContent/sha256":"absent"}),
    )
    .await;
    let create = json!({"path":"nested/new","content":"created","expected_sha256":"absent"});
    check(
        &w,
        "project_write",
        create.clone(),
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    check(
        &w,
        "project_write",
        create,
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    check(
        &w,
        "project_read",
        json!({"path":"src/value"}),
        json!({"/result/structuredContent/read_coverage_complete":true}),
    )
    .await;
    // The failed explicit key retains its structured rejection even after a read.
    // A fresh keyless proposal can use the current complete read.
    check(&w,"project_write",json!({"path":"src/value","content":"bad","expected_sha256":hash,"request_key":"partial-write"}),
        json!({"/result/structuredContent/error":"gateway_complete_read_required","/result/structuredContent/write_state":"rejected"})).await;
    check(&w,"project_write",json!({"path":"src/value","content":"written","expected_sha256":format!("{:x}",Sha256::digest(b"later"))}),json!({"/result/structuredContent/outcome":"written"})).await;
    check(
        &w,
        "project_git",
        json!({"kind":"status","request_key":"git-status"}),
        json!({"/result/structuredContent/exit_code":0}),
    )
    .await;
    check(
        &w,
        "project_git",
        json!({"kind":"diff","request_key":"git-diff"}),
        json!({"/result/structuredContent/exit_code":0}),
    )
    .await;
    let diff=s.read(|db|Ok(db.query_row("SELECT json_extract(result_json,'$.output') FROM runtime_requests WHERE request_key='git-diff'",[],|r|r.get::<_,String>(0))?)).unwrap();
    assert!(diff.contains("-one") && diff.contains("+written"));
    // History reads need no host command: every kind runs for any worker on any engine.
    for (kind,extra,expected) in [
        ("log",json!({"max_count":5}),"fixture"),
        ("show",json!({"rev":"HEAD","path":"src/value"}),"+one"),
        ("cat_file",json!({"rev":"HEAD","path":"src/value"}),"one"),
        ("ls_files",json!({"path":"src"}),"src/value"),
        ("blame",json!({"path":"src/value","rev":"HEAD"}),"one"),
    ] {
        let key=format!("git-history-{kind}");
        let mut a=json!({"kind":kind,"request_key":key});
        a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        check(&w,"project_git",a,json!({"/result/structuredContent/exit_code":0})).await;
        let output=s.read(|db|Ok(db.query_row("SELECT json_extract(result_json,'$.output') FROM runtime_requests WHERE request_key=?",[&key],|r|r.get::<_,String>(0))?)).unwrap();
        assert!(output.contains(expected),"{kind}: {output}");
    }
    for bad in [json!({"kind":"log","rev":"--output=/tmp/x","request_key":"git-bad-rev"}),
        json!({"kind":"show","path":"../x","request_key":"git-bad-path"}),
        json!({"kind":"cat_file","request_key":"git-cat-missing"})] {
        check(&w,"project_git",bad,json!({"/result/isError":true})).await;
    }

    // This filter exists only through an include. The required but inert
    // false command would fail a worktree comparison if Git activated it.
    std::fs::write(p.0.join(".gitattributes"), "src/value filter=synthetic\n").unwrap();
    std::fs::write(
        p.0.join(".git/filter-fixture.conf"),
        "[filter \"synthetic\"]\n\tclean = /usr/bin/false\n\trequired = true\n",
    ).unwrap();
    use std::io::Write as _;
    writeln!(
        std::fs::OpenOptions::new().append(true).open(p.0.join(".git/config")).unwrap(),
        "[include]\n\tpath = filter-fixture.conf"
    ).unwrap();
    // Verify the include is effective in the fixture without running the
    // filter itself. The gateway disables the resolved filter drivers.
    let included = std::process::Command::new("/usr/bin/git")
        .args(["config", "--local", "--includes", "--get", "filter.synthetic.required"])
        .current_dir(&p.0)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(included.status.success(), "fixture include was not loaded");
    assert_eq!(included.stdout, b"true\n");
    for (kind, expected) in [("status", " M src/value"), ("diff", "+written")] {
        let key = format!("git-{kind}-included-filter");
        check(
            &w,
            "project_git",
            json!({"kind":kind,"request_key":key}),
            json!({"/result/structuredContent/exit_code":0}),
        ).await;
        let output = s.read(|db| Ok(db.query_row(
            "SELECT json_extract(result_json,'$.output') FROM runtime_requests WHERE request_key=?",
            [&key],
            |r| r.get::<_,String>(0),
        )?)).unwrap();
        assert!(output.contains(expected), "{kind}: {output}");
    }
    std::fs::write(p.0.join("src/paged"), "a\nb\n").unwrap();
    check(
        &w,
        "project_read",
        json!({"path":"src/paged","limit":1}),
        json!({"/result/structuredContent/read_coverage_complete":false}),
    )
    .await;
    check(
        &w,
        "project_read",
        json!({"path":"src/paged","offset":2,"limit":1}),
        json!({"/result/structuredContent/read_coverage_complete":true}),
    )
    .await;
    check(&w,"project_write",json!({"path":"src/paged","content":"combined","expected_sha256":format!("{:x}",Sha256::digest(b"a\nb\n"))}),json!({"/result/structuredContent/outcome":"written"})).await;
    check(
        &w,
        "project_read",
        json!({"path":"src/overlap"}),
        json!({"/result/structuredContent/exists":false}),
    )
    .await;
    check(
        &w,
        "project_write",
        json!({"path":"src/overlap","content":"aaa","expected_sha256":"absent"}),
        json!({"/result/structuredContent/byte_count":3}),
    )
    .await;
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[{"old_string":"aa","new_string":"b"}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_unique","/result/structuredContent/field":"edits[0].old_string"}),
    )
    .await;
    // Ordered replacements are atomic, including when a later anchor is absent.
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[
        {"old_string":"aaa","new_string":"first"},{"old_string":"missing","new_string":"second"}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_found","/result/structuredContent/field":"edits[1].old_string"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("src/overlap")).unwrap(),
        "aaa"
    );
    check(&w,"project_edit",json!({"path":"src/overlap","edits":[{"old_string":"aaa","new_string":"b"}]}),
        json!({"/result/structuredContent/edits_applied":1,"/result/structuredContent/byte_count":1})).await;
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[{"old_string":"b","new_string":"b"}]}),
        json!({"/result/isError":false,"/result/structuredContent/outcome":"unchanged"}),
    )
    .await;
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[{"old_string":"b","new_string":"ééé"}]}),
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[{"old_string":"éé","new_string":"x"}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_unique","/result/structuredContent/field":"edits[0].old_string"}),
    )
    .await;
    check(&w,"project_edit",json!({"path":"src/overlap","edits":[{"old_string":"éé","new_string":"x","replace_all":true}]}),
        json!({"/result/structuredContent/outcome":"written"})).await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("src/overlap")).unwrap(),
        "xé"
    );
    let ignore_hash = std::fs::read(p.0.join(".gitignore"))
        .map(|b| format!("{:x}", Sha256::digest(b)))
        .unwrap_or("absent".into());
    check(
        &w,
        "project_read",
        json!({"path":".gitignore"}),
        json!({"/result/isError":false}),
    )
    .await;
    check(&w,"project_write",json!({"path":".gitignore","content":"fixture\n","expected_sha256":ignore_hash}),
        json!({"/result/structuredContent/read_coverage_complete":false,"/result/structuredContent/full_content_sha256":null})).await;
    assert!(std::fs::read_to_string(p.0.join(".gitignore"))
        .unwrap()
        .contains("/.cortex-dev/"));
    // Lose only the receipt commit after an actual replacement. An exact retry
    // reconciles the committed intent's hashes, not the edit expression again.
    s.write(|db|{db.execute_batch("CREATE TRIGGER lose_file_ack BEFORE UPDATE ON runtime_requests
        WHEN OLD.operation='project_edit' AND OLD.request_key='lost-ack' AND json_extract(NEW.result_json,'$.complete')=1
        BEGIN SELECT RAISE(ABORT,'fixture lost file acknowledgement'); END;")?;Ok(())}).unwrap();
    let interrupted = json!({"path":"src/paged","edits":[{"old_string":"combined","new_string":"combined-once"}],"request_key":"lost-ack"});
    check(
        &w,
        "project_edit",
        interrupted.clone(),
        json!({"/result/isError":true}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("src/paged")).unwrap(),
        "combined-once"
    );
    s.write(|db| {
        db.execute_batch("DROP TRIGGER lose_file_ack")?;
        Ok(())
    })
    .unwrap();
    check(
        &w,
        "project_edit",
        interrupted.clone(),
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    check(
        &w,
        "project_edit",
        interrupted,
        json!({"/result/structuredContent/outcome":"written"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(p.0.join("src/paged")).unwrap(),
        "combined-once"
    );
    s.read(|db|{
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROJECT_RECONCILED' AND json_extract(payload_json,'$.request_key')='lost-ack'",[],|r|r.get::<_,i64>(0))?,1);
        Ok(())
    }).unwrap();
    check(&w,"project_read",json!({"path":"src/large"}),json!({"/result/structuredContent/truncated_lines/0":1,"/result/structuredContent/read_coverage_complete":false})).await;
    let large = std::fs::read(p.0.join("src/large")).unwrap();
    check(&w,"project_write",
        json!({"path":"src/large","content":"shrunk","expected_sha256":format!("{:x}",Sha256::digest(&large))}),
        json!({"/result/structuredContent/error":"gateway_complete_read_required",
            "/result/structuredContent/field":"path","/result/structuredContent/write_state":"rejected"})).await;
    assert_eq!(std::fs::read(p.0.join("src/large")).unwrap(), large);
    std::os::unix::fs::symlink(p.0.join("src/value"), p.0.join("alias")).unwrap();
    check(
        &w,
        "project_read",
        json!({"path":"alias"}),
        json!({"/result/isError":true}),
    )
    .await;
    // The alias-refusal scenario is complete; revision observation deliberately
    // rejects unresolved aliases, so remove only this disposable fixture link.
    std::fs::remove_file(p.0.join("alias")).unwrap();
    check(
        &w,
        "access_request",
        json!({"paths":["extra"],"reason":"Fixture reservation","request_key":"extra"}),
        json!({"/result/isError":false}),
    )
    .await;
    if workspace_service::native_command_supervisor::observe_command_view(&p.0, "development")
        .is_ok()
    {
        // Hold a real process until the same channel has read a file, replayed
        // its key and polled the command. A blocking MCP handler cannot pass.
        let args = json!({"argv":["/bin/sh","-c","while [ ! -f release-command ]; do sleep 0.1; done"],"timeout_seconds":20,"request_key":"background"});
        let pending = json!({"/result/structuredContent/state":"running","/result/structuredContent/settled":false});
        check(&w, "command_run", args.clone(), pending.clone()).await;
        let reference: String = s.read(|db| Ok(db.query_row("SELECT json_extract(result_json,'$.tool_call_ref') FROM runtime_requests WHERE request_key='background'",[],|r|r.get(0))?)).unwrap();
        check(&w, "command_run", args, pending.clone()).await;
        check(
            &w,
            "command_status",
            json!({"tool_call_ref":reference}),
            pending,
        )
        .await;
        check(
            &w,
            "project_read",
            json!({"path":"src/value"}),
            json!({"/result/isError":false}),
        )
        .await;
        std::fs::write(p.0.join("release-command"), "ready").unwrap();
        tokio::time::timeout(Duration::from_secs(8),async {
            loop {
                if s.read(|db| Ok(db.query_row("SELECT json_extract(result_json,'$.settled') IS 1 FROM runtime_requests WHERE request_key='background'",[],|r|r.get::<_,bool>(0))?)).unwrap() { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        check(&w,"command_status",json!({"tool_call_ref":reference}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/settled":true})).await;
        check(
            &w,
            "command_run",
            json!({"argv":["/bin/true"],"cwd":p.0.join("absent-cwd"),"request_key":"missing-cwd"}),
            json!({"/result/structuredContent/code":"invalid_cwd"}),
        )
        .await;
        // A writing role's declared reservation does not restrict project writes.
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","new-unreserved"],"request_key":"unreserved"}),
            json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"development"})).await;
        assert!(p.0.join("new-unreserved").exists());
        // The command's finish record attributes the file it created to the task,
        // so its review requires it like a gateway write.
        let record: Value = serde_json::from_str(&s.read(|db| Ok(db.query_row(
            "SELECT json_extract(e.payload_json,'$.receipt.observation') FROM runtime_events e JOIN runtime_requests r
             ON json_extract(r.result_json,'$.tool_call_ref')=json_extract(e.payload_json,'$.receipt.tool_call_ref')
             WHERE e.type='NATIVE_COMMAND_FINISHED' AND r.request_key='unreserved'",[],|r|r.get::<_,String>(0))?)).unwrap()).unwrap();
        assert_eq!(record["changed_paths_complete"], true, "{record}");
        assert!(
            record["changed_paths"]
                .as_array()
                .unwrap()
                .contains(&json!("new-unreserved")),
            "{record}"
        );
        // A sibling outside the project and the command's private scratch
        // distinguishes the host-writable route from the Guarded profile.
        let outside = Project::new();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch",outside.0.join("host-effect")],"request_key":"host-effect"}),json!({"/result/structuredContent/exit_code":0})).await;
        assert!(outside.0.join("host-effect").exists());
        std::fs::write(p.0.join("src/denied"), "owner-protected").unwrap();
        s.write(|db| {
            cortex_core::access::switch_tx(db, "t", "guarded", "fixture-guarded", "acp_set_mode")
        })
        .unwrap();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","guarded-unreserved"],"request_key":"guarded-write"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"guarded"})).await;
        check(&w,"command_run",json!({"argv":["/usr/bin/touch",outside.0.join("guarded-effect")],"request_key":"guarded-host"}),json!({"/result/structuredContent/exit_code":1})).await;
        assert!(!outside.0.join("guarded-effect").exists());
        check(
            &w,
            "command_run",
            json!({"argv":["/bin/pwd"],"cwd":outside.0,"request_key":"guarded-cwd"}),
            json!({"/error/code":-32001}),
        )
        .await;
        s.write(|db| {
            cortex_core::access::switch_tx(db, "t", "auto", "fixture-auto", "acp_set_mode")
        })
        .unwrap();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch",outside.0.join("auto-effect")],"request_key":"auto-effect"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"outside_confirmed"})).await;
        assert!(outside.0.join("auto-effect").exists());
        check(&w,"command_run",json!({"argv":["/bin/pwd"],"cwd":"/","request_key":"outside-cwd"}),
            json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/output":"/\n"})).await;
        // The rest of the journey exercises the sandboxed (Landlock) Development profile and its receipts.
        s.write(|db| {
            cortex_core::access::switch_tx(db, "t", "development", "fixture-development-2", "acp_set_mode")
        })
        .unwrap();
        check(&w,"command_run",json!({"argv":["/bin/cat","src/value"],"request_key":"cat"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/output":"written","/result/structuredContent/complete":true})).await;
        check(
            &w,
            "command_run",
            json!({"argv":["/bin/cat","src/value"],"request_key":"cat"}),
            json!({"/result/structuredContent/output":"written"}),
        )
        .await;
        check(
            &w,
            "command_run",
            json!({"argv":["/usr/bin/touch","src/conflict"],"request_key":"cat"}),
            json!({"/result/isError":true,"/result/structuredContent/code":"request_key_conflict","/result/structuredContent/field":"request_key"}),
        )
        .await;
        assert!(!p.0.join("src/conflict").exists());
        check(&w,"command_run",json!({"argv":["/bin/cat","src/large"],"request_key":"large"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/byte_count":70000,"/result/structuredContent/output_truncated":true,"/result/structuredContent/complete":true})).await;
        let id=s.read(|db|Ok(db.query_row("SELECT json_extract(result_json,'$.large_output_id') FROM runtime_requests WHERE request_key='large'",[],|r|r.get::<_,String>(0))?)).unwrap();
        check(&w,"read_large_output",json!({"large_output_id":id,"offset":69999,"limit":1}),json!({"/result/structuredContent/output":"x","/result/structuredContent/next_offset":null})).await;
        check(
            &w,
            "command_run",
            json!({"argv":["/bin/sh","-c","echo changed > src/effect"],"request_key":"effect"}),
            json!({"/result/structuredContent/exit_code":0}),
        )
        .await;
        assert!(p.0.join("src/effect").is_file());
        check(&w,"command_run",json!({"argv":["/bin/cat",".cortex-dev/cortex.sqlite3"],"request_key":"private"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/cortex_store/code":"cortex_store_read_only_for_commands"})).await;
        check(&w,"command_run",json!({"argv":["/bin/sleep","5"],"timeout_seconds":1,"request_key":"timeout"}),json!({"/result/structuredContent/complete":false,"/result/structuredContent/failure":"timeout"})).await;
        s.read(|db|{
            let process:String=db.query_row("SELECT p.payload_json FROM runtime_requests r JOIN runtime_events p ON p.task_id=r.task_id
                AND p.type='NATIVE_COMMAND_PROCESS' AND json_extract(p.payload_json,'$.ref')=json_extract(r.result_json,'$.tool_call_ref')
                WHERE r.request_key='timeout'",[],|r|r.get(0))?;
            let process:Value=serde_json::from_str(&process)?;
            assert!(process["process_identity"]["pid"].as_u64().unwrap()>0);
            assert!(process["process_identity"]["start_ticks"].as_u64().unwrap()>0);
            assert!(!process["process_identity"]["boot_id"].as_str().unwrap().is_empty());
            let digest:Option<String>=db.query_row("SELECT json_extract(result_json,'$.revision_digest') FROM runtime_requests WHERE request_key='cat'",[],|r|r.get(0))?;
            assert!(digest.is_none(),"trivial reads have no check revision binding");
            let changed:Option<String>=db.query_row("SELECT json_extract(result_json,'$.revision_digest') FROM runtime_requests WHERE request_key='effect'",[],|r|r.get(0))?;
            assert!(changed.is_none(),"a mutating command must not certify an unchanged revision");
            Ok(())
        }).unwrap();
        check(
            &w,
            "command_run",
            json!({"argv":["/bin/sleep","5"],"timeout_seconds":1,"request_key":"timeout"}),
            json!({"/result/structuredContent/failure":"timeout"}),
        )
        .await;
        let (file,reference)=s.read(|db|Ok(db.query_row("SELECT l.filename,json_extract(r.result_json,'$.tool_call_ref') FROM large_outputs l JOIN runtime_requests r ON json_extract(r.result_json,'$.large_output_id')=l.id WHERE r.request_key='large'",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?)).unwrap();
        std::fs::write(source_dir.join(".native-output").join(file), "tampered").unwrap();
        check(
            &w,
            "read_large_output",
            json!({"large_output_id":id}),
            json!({"/result/isError":true}),
        )
        .await;
        let receipt=s.read(|db|Ok(db.query_row("SELECT result_json FROM runtime_requests WHERE operation='command_run' AND request_key='cat'",[],|r|r.get::<_,String>(0))?)).unwrap();
        let receipt: Value = serde_json::from_str(&receipt).unwrap();
        std::fs::create_dir_all(p.0.join("src/nested/.cortex-dev")).unwrap();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","src/private-scope-effect"],"request_key":"private-scope"}),json!({"/result/structuredContent/exit_code":0})).await;
        assert!(p.0.join("src/private-scope-effect").exists());
        // Auto: the owner allowed model writes to Cortex store directories; only live store files stay sealed.
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","src/nested/.cortex-dev/allowed"],"request_key":"private-child"}),json!({"/result/structuredContent/exit_code":0})).await;
        assert!(p.0.join("src/nested/.cortex-dev/allowed").exists());
        // A nested store keeps the project unobservable for the result; the fixture removes it again.
        std::fs::remove_dir_all(p.0.join("src/nested/.cortex-dev")).unwrap();
        let trivial_ref = receipt["tool_call_ref"].clone();
        check(
            &w,
            "command_run",
            json!({"argv":["/usr/bin/cmp","src/value","src/value"],"request_key":"check-cmp"}),
            json!({"/result/structuredContent/exit_code":0}),
        )
        .await;
        let receipt=s.read(|db|Ok(db.query_row("SELECT result_json FROM runtime_requests WHERE operation='command_run' AND request_key='check-cmp'",[],|r|r.get::<_,String>(0))?)).unwrap();
        let receipt: Value = serde_json::from_str(&receipt).unwrap();
        // Owner write denials stay enforced by Landlock (Guarded; Auto and host commands are not sandboxed).
        s.write(|db| {
            cortex_core::access::switch_tx(db, "t", "guarded", "fixture-guarded-2", "acp_set_mode")
        })
        .unwrap();
        s.write(|db| {
            cortex_core::lifecycle::event_tx(
                db,
                "t",
                "USER_STEERING",
                json!({"deny_write":["src/denied"]}),
            )
        })
        .unwrap();
        check(
            &w,
            "command_run",
            // Landlock refuses the write (timestamps are not covered: touch would succeed).
            json!({"argv":["/usr/bin/tee","src/denied"],"request_key":"owner-denied"}),
            json!({"/result/structuredContent/exit_code":1}),
        )
        .await;
        s.write(|db| {
            cortex_core::access::switch_tx(db, "t", "development", "fixture-development-3", "acp_set_mode")
        })
        .unwrap();
        submit_result(&h,&w,"t",&o,"a",json!({"report":"# Tool evidence\nObserved native tools.","summary":"Native tools checked","request_key":"result","checks":[{"name":"cmp","outcome":"passed","command":["/usr/bin/cmp","src/value","src/value"],"tool_call_ref":receipt["tool_call_ref"]},{"name":"tampered","outcome":"passed","tool_call_ref":reference},{"name":"cat","outcome":"passed","tool_call_ref":trivial_ref},{"name":"unknown","outcome":"passed","tool_call_ref":"native-command:unknown"}]})).await;
        s.read(|db| {
            let result: String = db.query_row(
                "SELECT result_json FROM runtime_nodes WHERE assignment_ref='a'",
                [],
                |r| r.get(0),
            )?;
            let result: Value = serde_json::from_str(&result)?;
            assert_eq!(result["checks"][0]["evidence_strength"], "verified");
            for (index, gap) in [
                (1, "call_incomplete"),
                (2, "trivial_command"),
                (3, "tool_call_ref_unknown"),
            ] {
                assert_eq!(result["checks"][index]["outcome"], "unverified");
                assert_eq!(result["checks"][index]["claimed_outcome"], "passed");
                assert_eq!(result["checks"][index]["evidence_strength"], "none");
                assert_eq!(result["checks"][index]["evidence_gap"], gap);
            }
            assert_eq!(
                result["checks"][0]["revision_digest"],
                receipt["revision_digest"]
            );
            Ok(())
        })
        .unwrap();
    } else {
        println!("native command sandbox unavailable: positive command paths unverified");
    }
    w.shutdown().await.unwrap();
    s.write(|db|{db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'r','worker-explorer','Inspect','[]','[]','execution','running','b',NULL)",[])?;Ok(())}).unwrap();
    let r = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "b".into(),
            },
            p.legacy_route(),
        )
        .await
        .unwrap();
    check(
        &r,
        "project_write",
        json!({"path":"src/value","content":"forged","expected_sha256":"absent"}),
        json!({"/error/code":-32001}),
    )
    .await;
    if verification_available(&p.0) {
        check(&r,"command_run",json!({"argv":["/bin/cat","src/value"],"request_key":"ro-read"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"read_only"})).await;
        check(
            &r,
            "command_run",
            json!({"argv":["/usr/bin/touch","src/forbidden"],"request_key":"ro-write"}),
            json!({"/result/structuredContent/exit_code":1}),
        )
        .await;
        assert!(!p.0.join("src/forbidden").exists());
        let id=s.read(|db|Ok(db.query_row("SELECT json_extract(result_json,'$.large_output_id') FROM runtime_requests WHERE request_key='large'",[],|r|r.get::<_,String>(0))?)).unwrap();
        check(
            &r,
            "read_large_output",
            json!({"large_output_id":id}),
            json!({"/result/isError":true}),
        )
        .await;
        let other: String = s.read(|db| Ok(db.query_row("SELECT json_extract(result_json,'$.tool_call_ref') FROM runtime_requests WHERE request_key='background'",[],|row|row.get(0))?)).unwrap();
        check(
            &r,
            "command_status",
            json!({"tool_call_ref":other}),
            json!({"/result/structuredContent/code":"unknown_command_ref"}),
        )
        .await;
        // Fail only the final receipt transaction after the process has run.
        s.write(|db| { db.execute_batch("CREATE TRIGGER fixture_refuse_settlement BEFORE INSERT ON large_outputs BEGIN SELECT RAISE(ABORT,'fixture'); END")?; Ok(()) }).unwrap();
        let lost = json!({"argv":["/bin/true"],"request_key":"settlement-failure"});
        check(
            &r,
            "command_run",
            lost.clone(),
            json!({"/result/structuredContent/code":"command_outcome_unknown"}),
        )
        .await;
        s.write(|db| {
            db.execute_batch("DROP TRIGGER fixture_refuse_settlement")?;
            Ok(())
        })
        .unwrap();
        // The supervisor finished, so the receipt is settled at once: its outcome stays
        // unrecorded (never a success) and the same request is never run again.
        check(
            &r,
            "command_run",
            lost,
            json!({"/result/structuredContent/settled":true,"/result/structuredContent/complete":false,
                "/result/structuredContent/failure":"outcome_not_recorded","/result/structuredContent/exit_code":0}),
        )
        .await;
        s.read(|db|{
            let count:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_DIAGNOSTIC' AND json_extract(payload_json,'$.phase')='settlement' AND json_extract(payload_json,'$.stage')='after_process_registration'",[],|row|row.get(0))?;
            assert_eq!(count,1);
            Ok(())
        }).unwrap();
        // A launch that fails before its supervisor is bound (here: the handle write is
        // refused) never releases the payload; its receipt is settled as not started,
        // so no unknown outcome is left behind for close or repair.
        s.write(|db| { db.execute_batch("CREATE TRIGGER fixture_refuse_handle BEFORE UPDATE ON runtime_requests
            WHEN json_type(NEW.result_json,'$.handle')='object' BEGIN SELECT RAISE(ABORT,'fixture'); END")?; Ok(()) }).unwrap();
        check(
            &r,
            "command_run",
            json!({"argv":["/bin/cat","src/value"],"request_key":"launch-failure"}),
            json!({"/result/isError":true}),
        )
        .await;
        s.write(|db| {
            db.execute_batch("DROP TRIGGER fixture_refuse_handle")?;
            Ok(())
        })
        .unwrap();
        let receipt: Value = s
            .read(|db| {
                Ok(serde_json::from_str(&db.query_row(
                    "SELECT result_json FROM runtime_requests WHERE request_key='launch-failure'",
                    [],
                    |r| r.get::<_, String>(0),
                )?)?)
            })
            .unwrap();
        assert_eq!(receipt["settled"], true, "{receipt}");
        assert_eq!(receipt["complete"], false);
        assert_eq!(receipt["failure"], "command_not_started");
        assert_eq!(receipt["launched"], false);
        assert!(receipt["handle"].is_null());
        s.read(|db| {
            assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='command_run'
                AND json_extract(result_json,'$.settled') IS NOT 1 AND json_extract(result_json,'$.complete') IS NOT 1
                AND request_key IN ('launch-failure','settlement-failure'))",[],|r|r.get::<_,bool>(0))?);
            Ok(())
        }).unwrap();
    }
    r.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
async fn coordinator_tools_not_chat_and_lease() {
    let p = Project::new();
    let h = p.host().await;
    let key = p.key();
    assert!(tokio::task::spawn_blocking(move || NativeHost::open(key))
        .await
        .unwrap()
        .is_err());
    let o = h
        .create("t".into(), "Explain".into(), "owner".into())
        .await
        .unwrap();
    let dispatched = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await;
    if dispatched.is_err() {
        let diagnostic=p.store().read(|db|Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE type=\'NATIVE_SESSION_START_FAILED\' ORDER BY id DESC LIMIT 1",[],|r|r.get::<_,String>(0))?)).unwrap();
        panic!("native start diagnostic: {diagnostic}");
    }
    let d = dispatched.unwrap();
    assert!(h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .is_err());
    d.prompt(r#"{"name":"answer_submit","arguments":{"text":"Forged","request_key":"chat"}}"#)
        .await
        .unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    assert_eq!(
        d.prompt(&call("plan_submit", json!({})))
            .await
            .unwrap()
            .stop_reason,
        "refusal"
    );
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    // A verification that only prints a search (no `-q`: its status asserts nothing the
    // output does not) is a correctable content error with its exact code, never an
    // authority refusal; nothing is recorded and the coordinator may retry.
    let trivial = json!({"name":"plan_submit","arguments":{"request_key":"trivial","plan":{
        "tasks":[{"id":"w","role":"backend-dev","objective":"Write notes.txt","dependencies":[],
            "scope":["notes.txt"],"model":"auto"}],
        "verification":[["grep","-x","ok","notes.txt"]]}},
        "__fixture_expect":{"/result/isError":true,
            "/result/structuredContent/code":"verification_trivial_command",
            "/result/structuredContent/recorded":false}});
    assert_eq!(
        d.prompt(&format!("call:{trivial}"))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    let answer = call(
        "answer_submit",
        json!({"text":"Observed answer.","request_key":"answer"}),
    );
    assert_eq!(d.prompt(&answer).await.unwrap().stop_reason, "end_turn");
    assert_eq!(d.prompt(&answer).await.unwrap().stop_reason, "end_turn");
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::AnswerReady);
    d.shutdown().await.unwrap();
    let s = p.store();
    s.read(|db| {
        let count: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type='ANSWER_SUBMITTED'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1);
        let busy: i64 = db.query_row(
            "SELECT count(*) FROM capacity_reservations WHERE state!='released'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(busy, 0);
        Ok(())
    })
    .unwrap();
    drop(s);
    close(h).await;
}
/// sudo in a host command asks the owner through the askpass route, and a writing
/// worker's command_run that needs the host is rerun there by the runtime. A fake
/// `sudo` stands in for the real one; no real sudo runs and no real password exists.
/// A command's start never waits for a project walk or digest: the process event of
/// a supervised launch carries no digest or tree-walk stage, a writing command's
/// change record still names the file it changed, and its "before" observation is
/// either complete before the release or checked against it. With
/// `CORTEX_LATENCY_SOURCE` (a Git checkout cloned into the disposable project) and
/// `CORTEX_LATENCY_IGNORED` (Git-ignored files to add), it also prints the start
/// latency (NATIVE_COMMAND_STARTED to NATIVE_COMMAND_PROCESS) of 20-command series.
async fn command_start_never_waits_for_project_observation() {
    let p = Project::new_at(&std::env::temp_dir(), "command-start-latency");
    let git = |args: &[&str], at: &std::path::Path| {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(at)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
    };
    match std::env::var("CORTEX_LATENCY_SOURCE") {
        Ok(source) => git(&["clone", "-q", "--shared", &source, "."], &p.0),
        Err(_) => {
            std::fs::create_dir_all(p.0.join("src")).unwrap();
            for n in 0..200 {
                std::fs::write(p.0.join(format!("src/f{n}.rs")), format!("fn f{n}() {{}}\n").repeat(200)).unwrap();
            }
            git(&["init", "-q"], &p.0);
            git(&["add", "."], &p.0);
            git(&["commit", "-qm", "fixture"], &p.0);
        }
    }
    std::fs::write(p.0.join("edited-by-command"), "0\n").unwrap();
    git(&["add", "edited-by-command"], &p.0);
    git(&["commit", "-qm", "edit target"], &p.0);
    let ignored: usize = std::env::var("CORTEX_LATENCY_IGNORED").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    use std::io::Write as _;
    writeln!(std::fs::OpenOptions::new().append(true).open(p.0.join(".git/info/exclude")).unwrap(), "/bulk/").unwrap();
    for n in 0..ignored {
        let dir = p.0.join(format!("bulk/pkg{}/lib", n / 100));
        if n % 100 == 0 {
            std::fs::create_dir_all(&dir).unwrap();
        }
        std::fs::write(dir.join(format!("m{n}.js")), "module.exports = 1;\n").unwrap();
    }
    let h = p.host().await;
    let o = h.create("t".into(), "Command start latency".into(), "owner".into()).await.unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"**\"]','execution','running','a',NULL)", [])?;
        cortex_core::access::switch_tx(db, "t", "development", "fixture", "acp_set_mode")
    })
    .unwrap();
    let d = h
        .dispatch("t".into(), o, SessionRole::Worker { assignment: "a".into() }, p.legacy_route())
        .await
        .unwrap();
    let python = ["/usr/bin/python3", "/bin/python3"].into_iter().find(|p| std::path::Path::new(p).exists());
    let quick: Vec<String> = match python {
        Some(py) => vec![py.into(), "-c".into(), "pass".into()],
        None => vec!["/bin/sh".into(), "-c".into(), ":".into()],
    };
    let edit: Vec<String> = match python {
        Some(py) => vec![py.into(), "-c".into(), "open('edited-by-command','a').write('x\\n')".into()],
        None => vec!["/bin/sh".into(), "-c".into(), "echo x >> edited-by-command".into()],
    };
    let mut supervised = false;
    // Guarded hides the project's secret files (the whole-checkout grant needs none);
    // a Guarded command cannot run outside the checkout, so it runs the edit series only.
    for (index, mode) in ["development", "guarded", "auto"].into_iter().enumerate() {
        s.write(|db| cortex_core::access::switch_tx(db, "t", mode, &format!("fixture-{mode}"), "acp_set_mode")).unwrap();
        for (series, argv, cwd) in [("tmp", &quick, Some("/tmp")), ("edit", &edit, None)] {
            if mode == "guarded" && series == "tmp" {
                continue;
            }
            let first: i64 = s.read(|db| Ok(db.query_row("SELECT coalesce(max(id),0) FROM runtime_events", [], |r| r.get(0))?)).unwrap();
            let started = std::time::Instant::now();
            for n in 0..20 {
                let mut args = json!({"argv":argv,"request_key":format!("{mode}-{series}-{n}"),"timeout_seconds":60});
                if let Some(cwd) = cwd {
                    args["cwd"] = json!(cwd);
                }
                let prompt = format!("call:{}", json!({"name":"command_run","arguments":args,
                    "__fixture_expect":{"/result/structuredContent/exit_code":0}}));
                assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, "end_turn", "{mode} {series} {n}");
            }
            let total = started.elapsed().as_millis();
            let (mut latencies, processes): (Vec<f64>, Vec<Value>) = s
                .read(|db| {
                    let mut q = db.prepare(
                        "SELECT (julianday(p.created_at)-julianday(b.created_at))*86400000.0, p.payload_json
                         FROM runtime_events b JOIN runtime_events p ON p.type='NATIVE_COMMAND_PROCESS'
                         AND json_extract(p.payload_json,'$.ref')=json_extract(b.payload_json,'$.ref')
                         WHERE b.type='NATIVE_COMMAND_STARTED' AND b.id>? ORDER BY b.id",
                    )?;
                    let rows = q.query_map([first], |r| Ok((r.get::<_, f64>(0)?, r.get::<_, String>(1)?)))?;
                    let mut out = (Vec::new(), Vec::new());
                    for row in rows {
                        let (ms, payload) = row?;
                        out.0.push(ms);
                        out.1.push(serde_json::from_str(&payload).unwrap());
                    }
                    Ok(out)
                })
                .unwrap();
            assert_eq!(latencies.len(), 20, "{mode} {series}");
            for process in &processes {
                let timings = &process["timings"];
                for stage in ["digest_ms", "tree_walk_ms", "tree_entries"] {
                    assert!(timings.get(stage).is_none(), "{mode} {series}: {process}");
                }
                supervised |= process["supervised"] == true;
            }
            latencies.sort_by(f64::total_cmp);
            println!(
                "start latency {mode} {series}: median {:.1} ms, p95 {:.1} ms, max {:.1} ms; series of 20 took {total} ms",
                latencies[9] / 2.0 + latencies[10] / 2.0,
                latencies[18],
                latencies[19]
            );
        }
        // The edits are still attributed to the task after each command ended.
        let records: Vec<Value> = s
            .read(|db| {
                let mut q = db.prepare(
                    "SELECT json_extract(payload_json,'$.receipt.observation') FROM runtime_events
                     WHERE type='NATIVE_COMMAND_FINISHED' AND json_extract(payload_json,'$.receipt.argv') LIKE '%edited-by-command%'",
                )?;
                let rows = q.query_map([], |r| r.get::<_, Option<String>>(0))?;
                Ok(rows.filter_map(|r| r.ok().flatten()).map(|r| serde_json::from_str(&r).unwrap()).collect())
            })
            .unwrap();
        assert_eq!(records.len(), 20 * (index + 1), "{records:?}");
        {
            for record in &records {
                assert_eq!(record["changed_paths_complete"], true, "{record}");
                assert!(record["changed_paths"].as_array().unwrap().contains(&json!("edited-by-command")), "{record}");
                assert!(record["revision_digest"].is_null(), "a writing command binds no revision: {record}");
                assert!(record["before_observation"].is_string(), "{record}");
            }
        }
    }
    assert!(supervised, "no supervised launch was observed");
    d.shutdown().await.unwrap();
}

async fn sudo_prompts_and_host_reroute() {
    use std::os::unix::fs::PermissionsExt;
    use workspace_service::native_askpass as askpass;
    // The fake sudo is not root: accept its helper and its shared pipe (fixtures only).
    askpass::trust_any_caller_for_tests();
    const SECRET: &str = "fixture-sudo-secret-7Qx2";
    let digest = |text: &str| {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
    };
    let outside = Project::new();
    let bin = outside.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    // Asks for the password the way sudo does (argv `-A` first) and prints only its digest.
    std::fs::write(
        bin.join("sudo"),
        "#!/bin/sh\n[ \"$1\" = \"-A\" ] || { echo missing-askpass-flag; exit 3; }\n\
         pw=$(\"$SUDO_ASKPASS\" '[sudo] password for fixture: ') || { echo askpass-failed; exit 1; }\n\
         printf 'hash=%s\\n' \"$(printf %s \"$pw\" | sha256sum | cut -c1-64)\"\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("sudo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let fake = bin.join("sudo").to_str().unwrap().to_owned();
    async fn checked(
        d: &workspace_service::native_sessions::SessionDriver,
        name: &str,
        args: Value,
        expected: Value,
    ) {
        let prompt = format!(
            "call:{}",
            json!({"name":name,"arguments":args,"__fixture_expect":expected})
        );
        assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, "end_turn");
    }
    /// Answers the next prompt of task `t` once it opens, after checking what it names.
    fn answer_next(answer: Option<&'static str>) -> tokio::task::JoinHandle<Value> {
        tokio::spawn(async move {
            for _ in 0..500 {
                if let Some(prompt) = askpass::pending().into_iter().find(|p| p["taskId"] == "t") {
                    let id = prompt["promptId"].as_str().unwrap();
                    let reply = match answer {
                        Some(text) => askpass::Answer::Password(askpass::Secret::new(text.into())),
                        None => askpass::Answer::Cancel,
                    };
                    askpass::answer(id, reply).unwrap();
                    return prompt;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no sudo prompt opened");
        })
    }

    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Install the build tools".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        cortex_core::access::switch_tx(db, "t", "auto", "fixture", "acp_set_mode")
    })
    .unwrap();
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.legacy_route(),
        )
        .await
        .unwrap();
    // 1. A writing worker's command_run with sudo runs on the host at once (Auto
    // answers the consent card); sudo gets `-A` and the owner's answer, and the
    // receipt says why it was rerouted and how the prompt ended.
    let answered = answer_next(Some(SECRET));
    let run = json!({"argv":[fake,"install-fixture"],"request_key":"sudo-run"});
    checked(
        &d,
        "command_run",
        run.clone(),
        json!({
            "/result/isError":false,
            "/result/structuredContent/profile":"outside_confirmed",
            "/result/structuredContent/exit_code":0,
            "/result/structuredContent/output":format!("hash={}\n", digest(SECRET)),
            "/result/structuredContent/rerouted/from":"command_run",
            "/result/structuredContent/rerouted/reason":"privilege",
            "/result/structuredContent/sudo_prompts":["answered"],
        }),
    )
    .await;
    let prompt = answered.await.unwrap();
    assert_eq!(prompt["title"], "Password for sudo");
    assert_eq!(prompt["worker"], "worker-backend-dev");
    assert_eq!(prompt["task"], "Install the build tools");
    assert!(prompt["command"]
        .as_str()
        .unwrap()
        .ends_with("install-fixture"));
    assert!(!prompt.to_string().contains(SECRET));
    // 2. The same request again answers the same host receipt: nothing runs, nothing asks.
    checked(
        &d,
        "command_run",
        run,
        json!({
            "/result/structuredContent/rerouted/reason":"privilege",
            "/result/structuredContent/sudo_prompts":["answered"],
        }),
    )
    .await;
    assert!(!askpass::pending().iter().any(|p| p["taskId"] == "t"));
    // 3. A cancelled prompt fails sudo normally; the receipt says it was cancelled.
    let cancelled = answer_next(None);
    checked(
        &d,
        "outside_command",
        json!({"argv":[fake,"cancel-fixture"],"request_key":"sudo-cancel"}),
        json!({
            "/result/structuredContent/exit_code":1,
            "/result/structuredContent/output":"sudo password prompt: cancelled\naskpass-failed\n",
            "/result/structuredContent/sudo_prompts":["cancelled"],
        }),
    )
    .await;
    cancelled.await.unwrap();
    // 4. In Auto the payload runs once, unsandboxed, on the host; only the host receipt exists.
    let output = "unshare: unshare failed: Operation not permitted\nsudo: effective uid is not 0\n";
    // The sandbox may mount the project root read-only for a plain-folder fixture.
    // The separate disposable /tmp fixture is writable in both execution routes.
    let marker = outside.0.join("runs");
    let command = json!({
        "argv": [
            "/bin/sh", "-c",
            format!("echo ran >> \"$1\" || exit 10; printf '%s' '{output}'; exit 4"),
            "fixture", marker,
        ],
        "request_key": "nested",
    });
    let _ = d
        .prompt(&call("command_run", command.clone()))
        .await
        .unwrap();
    let receipt = |key: &str| -> Value {
        s.read(|db| {
            let raw: String = db.query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id='t' AND request_key=?",
                [key],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&raw)?)
        })
        .unwrap()
    };
    // Auto runs the command once, unsandboxed, on the host as the owner: only the
    // host receipt exists, no sandbox receipt and no nested-sandbox classification.
    assert!(s
        .read(|db| Ok(db
            .query_row(
                "SELECT count(*) FROM runtime_requests WHERE task_id='t' AND request_key='nested' AND operation='command_run'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
        .unwrap()
        == 0);
    let host = receipt("nested.host");
    assert_eq!(host["complete"], true);
    let host_reply = &host["reply"]["structuredContent"];
    assert_eq!(host_reply["profile"], "outside_confirmed");
    assert_eq!(host_reply["exit_code"], 4);
    assert_eq!(host_reply["output"], output);
    assert_eq!(host_reply["rerouted"]["reason"], "auto_host");
    let expected = json!({
        "/result/structuredContent/profile":"outside_confirmed",
        "/result/structuredContent/exit_code":4,
        "/result/structuredContent/tool_call_ref":host_reply["tool_call_ref"],
        "/result/structuredContent/rerouted/reason":"auto_host",
    });
    // The same request answers the same receipt: the payload is not run again.
    checked(&d, "command_run", command.clone(), expected).await;
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "ran\n");
    let outside_runs: i64 = s
        .read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_OUTSIDE_SETTLED' AND json_extract(payload_json,'$.decider')='mode:auto'",
            [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(outside_runs, 3);
    println!("auto host routing verified: one unsandboxed run, receipt not replayed");
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
    // 5. In Auto a read-only role is not sandboxed either: its sudo is rerouted to the host
    // through the same consent route and runs as the owner.
    // no consent card opens and nothing outside the project is written.
    let r = Project::new();
    // The marker is outside the read-only command's private scratch and checkout.
    let marker = std::env::temp_dir().join(format!(
        "cw-reader-sudo-{}-{}",
        std::process::id(),
        digest(&format!("{:?}", std::time::SystemTime::now()))[..12].to_owned()
    ));
    std::fs::write(
        bin.join("sudo-writer"),
        "#!/bin/sh\nfor a; do last=$a; done\n: > \"$last\"\n",
    )
    .unwrap();
    std::fs::set_permissions(
        bin.join("sudo-writer"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::create_dir(outside.0.join("as-sudo")).unwrap();
    std::os::unix::fs::symlink(bin.join("sudo-writer"), outside.0.join("as-sudo/sudo")).unwrap();
    let h = r.host().await;
    let o = h
        .create("t".into(), "Read only".into(), "owner".into())
        .await
        .unwrap();
    let s = r.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-explorer','Observe','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        cortex_core::access::switch_tx(db, "t", "auto", "fixture", "acp_set_mode")
    })
    .unwrap();
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            r.legacy_route(),
        )
        .await
        .unwrap();
    let _ = d
        .prompt(&call(
            "command_run",
            json!({"argv":[outside.0.join("as-sudo/sudo"),marker],"request_key":"reader-sudo"}),
        ))
        .await
        .unwrap();
    let leaked = marker.exists();
    let _ = std::fs::remove_file(&marker);
    // Auto: every role (read-only included) runs its command unsandboxed on the host as the owner.
    assert!(leaked, "an Auto read-only role command did not run on the host");
    let asked: i64 = s
        .read(|db| Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_OUTSIDE_REQUESTED'", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(asked, 1);
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;

    // The password reached only the fake sudo: no store row, event, receipt, output
    // file or other project file holds it.
    fn files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => files(&path, out),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    let mut all = Vec::new();
    for root in [&p.0, &r.0, &outside.0] {
        files(root, &mut all);
    }
    assert!(
        all.iter()
            .any(|f| f.extension().is_some_and(|e| e == "bin")),
        "no command output retained"
    );
    assert!(all
        .iter()
        .any(|f| f.file_name().is_some_and(|n| n == "cortex.sqlite3")));
    for file in all {
        let bytes = std::fs::read(&file).unwrap_or_default();
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "the sudo password appears in {}",
            file.display()
        );
    }
}
async fn access_modes_and_outside_routes() {
    // A writing worker keeps the same asking/workspace-write mode in every access
    // mode: no session runs in bypassPermissions or agent-full-access.
    for (engine, floor, automatic) in [
        (EngineKind::Codex, "workspace-write", "workspace-write"),
        (EngineKind::Claude, "default", "default"),
        (EngineKind::OpenCode, "cortex", "cortex"),
    ] {
        for mode in ["guarded", "development", "auto"] {
            let p = Project::new();
            let outside = Project::new();
            let h = p.host().await;
            let o = h
                .create("t".into(), "Work".into(), "owner".into())
                .await
                .unwrap();
            let s = p.store();
            s.write(|db|{
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
                cortex_core::access::switch_tx(db,"t",mode,"fixture","acp_set_mode")
            }).unwrap();
            let mut route = p.route();
            route.launch.engine = engine;
            if engine == EngineKind::OpenCode {
                std::fs::create_dir(p.0.join("opencode")).unwrap();
                route.launch.credential_directory = p.0.join("opencode");
            }
            // A caller-supplied mode cannot override the channel's policy floor.
            route
                .parameters
                .insert("mode".into(), "untrusted-mode".into());
            let d=h.dispatch("t".into(),o,SessionRole::Worker{assignment:"a".into()},route).await.unwrap_or_else(|e| {
                let why=s.read(|db|Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' ORDER BY id DESC LIMIT 1",[],|r|r.get::<_,String>(0))?)).unwrap();
                panic!("fixture engine {engine:?} access {mode}: {e:?}; {why}");
            });
            let confirmed=s.read(|db|Ok(db.query_row("SELECT json_extract(payload_json,'$.mode') FROM runtime_events WHERE type='NATIVE_ACCESS_MODE_CONFIRMED'",[],|r|r.get::<_,String>(0))?)).unwrap();
            assert_eq!(confirmed, if mode == "auto" { automatic } else { floor });
            async fn checked(
                d: &workspace_service::native_sessions::SessionDriver,
                name: &str,
                args: Value,
                expected: Value,
                stop: &str,
            ) {
                let prompt = format!(
                    "call:{}",
                    json!({"name":name,"arguments":args,"__fixture_expect":expected})
                );
                assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, stop);
            }
            d.prompt("permission").await.unwrap();
            // OpenCode keeps the client route outside Auto; the runtime answers
            // Claude and Codex itself: Development allows an uncontained request,
            // Guarded refuses it unless the request is provably contained.
            let expected =
                if mode == "auto" || engine != EngineKind::OpenCode && mode == "development" {
                    "NATIVE_PERMISSION_DECIDED"
                } else {
                    "NATIVE_PERMISSION_CANCELLED"
                };
            assert_eq!(
                s.read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type=?",
                    [expected],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
                1
            );
            let path = outside.0.join("output.txt");
            // A write outside the project asks the owner in Guarded and Development
            // (no owner surface here: refused) and is allowed in Auto.
            let write = json!({"path":path,"content":"first","request_key":"write"});
            checked(
                &d,
                "outside_write",
                write.clone(),
                json!({"/result/isError":mode!="auto"}),
                "end_turn",
            )
            .await;
            if mode != "auto" {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
            }
            if path.exists() {
                std::fs::write(&path, "newer").unwrap();
            }
            checked(
                &d,
                "outside_write",
                write,
                json!({"/result/isError":mode!="auto"}),
                "end_turn",
            )
            .await;
            if mode == "auto" {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
            } else {
                assert!(!path.exists());
            }
            checked(
                &d,
                "outside_write",
                json!({"path":path,"content":"conflict","request_key":"write"}),
                json!({"/result/isError":true,"/result/structuredContent/code":"request_key_conflict"}),
                "end_turn",
            )
            .await;
            let command = json!({"argv":["/bin/sh","-c","printf 'host output'; printf 'effect' >> count"],"cwd":outside.0,"request_key":"command"});
            checked(
                &d,
                "outside_command",
                command.clone(),
                json!({"/result/isError":mode!="auto"}),
                "end_turn",
            )
            .await;
            checked(
                &d,
                "outside_command",
                command,
                json!({"/result/isError":mode!="auto"}),
                "end_turn",
            )
            .await;
            if mode == "auto" {
                checked(&d,"outside_command",json!({"argv":["/usr/bin/head","-c","70000","/dev/zero"],"request_key":"bounded-output"}),
                    json!({"/result/structuredContent/complete":true,"/result/structuredContent/output_truncated":true,
                        "/result/structuredContent/byte_count":70000,"/result/structuredContent/exit_code":0}),"end_turn").await;
                checked(&d,"outside_command",json!({"argv":["/bin/sleep","5"],"timeout_seconds":1,"request_key":"timeout"}),
                    json!({"/result/structuredContent/complete":false,"/result/structuredContent/exit_code":null,
                        "/result/structuredContent/failure":"interrupted_or_timeout"}),"end_turn").await;
                assert_eq!(
                    std::fs::read_to_string(outside.0.join("count")).unwrap(),
                    "effect"
                );
            } else {
                assert!(!outside.0.join("count").exists());
            }
            // Another project's Cortex state is allowed on request (Auto allows; with
            // no owner client here an ask is refused); its live database never.
            for store in [".cortex", ".cortex-dev"] {
                std::fs::create_dir(outside.0.join(store)).unwrap();
                let target = outside.0.join(store).join("db");
                std::fs::write(&target, "fixture").unwrap();
                if mode == "auto" {
                    checked(&d,"outside_write",json!({"path":target,"content":"yes","request_key":store}),
                        json!({"/result/isError":false}),"end_turn").await;
                    assert_eq!(std::fs::read_to_string(&target).unwrap(), "yes");
                } else {
                    checked(&d,"outside_write",json!({"path":target,"content":"no","request_key":store}),
                        json!({"/result/isError":true,"/result/content/0/text":"outside_action_not_confirmed"}),"end_turn").await;
                    assert_eq!(std::fs::read_to_string(&target).unwrap(), "fixture");
                }
                let live = outside.0.join(store).join("cortex.sqlite3");
                std::fs::write(&live, "fixture").unwrap();
                checked(&d,"outside_write",json!({"path":live,"content":"no","request_key":format!("{store}-live")}),
                    json!({"/result/isError":true,"/result/structuredContent/code":"cortex_live_store"}),"end_turn").await;
                assert_eq!(std::fs::read_to_string(live).unwrap(), "fixture");
            }
            checked(
                &d,
                "outside_write",
                json!({"path":p.0.join("inside"),"content":"no","request_key":"inside"}),
                json!({"/result/isError":true,"/result/structuredContent/code":"outside_path_inside_project","/result/structuredContent/field":"path"}),
                "end_turn",
            )
            .await;
            std::os::unix::fs::symlink(&p.0, outside.0.join("alias")).unwrap();
            checked(
                &d,
                "outside_write",
                json!({"path":outside.0.join("alias/inside"),"content":"no","request_key":"alias"}),
                json!({"/result/isError":true,"/result/structuredContent/code":"outside_symlink_refused"}),
                "end_turn",
            )
            .await;
            d.prompt(&call(
                "ask_question",
                json!({"recommended":1,"question":"Wait?","options":["Yes","No"],"request_key":"wait"}),
            ))
            .await
            .unwrap();
            checked(
                &d,
                "outside_write",
                json!({"path":outside.0.join("waiting"),"content":"no","request_key":"waiting"}),
                json!({"/result/isError":true,"/result/structuredContent/code":"worker_waiting_for_answer"}),
                "end_turn",
            )
            .await;
            assert!(!outside.0.join("waiting").exists());
            d.shutdown().await.unwrap();
            drop(s);
            close(h).await;
        }
    }
    for role in ["worker-explorer", "worker-code-reviewer"] {
        let p = Project::new();
        let outside = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Read only".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db|{
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Observe','[]','[\"src\"]','execution','running','a',NULL)",[role])?;
            cortex_core::access::switch_tx(db,"t","auto","fixture","acp_set_mode")
        }).unwrap();
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                p.route(),
            )
            .await
            .unwrap();
        for (name, args) in [
            (
                "outside_write",
                json!({"path":outside.0.join("no"),"content":"no","request_key":"readonly-write"}),
            ),
            (
                "outside_command",
                json!({"argv":["/bin/touch",outside.0.join("no")],"request_key":"readonly-command"}),
            ),
        ] {
            // Auto runs every role's commands unsandboxed on the host as the owner; a
            // read-only role still cannot write through the outside_write route.
            let expected = if name == "outside_command" { "end_turn" } else { "refusal" };
            assert_eq!(
                d.prompt(&call(name, args)).await.unwrap().stop_reason,
                expected
            );
        }
        assert!(outside.0.join("no").exists());
        d.prompt("permission").await.unwrap();
        assert_eq!(
            s.read(|db| Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_CANCELLED'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
            1
        );
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
    // A non-confirming engine cannot prompt, and its owned process is reaped.
    let p = Project::new();
    std::fs::write(p.0.join("scenario.txt"), "mode-not-confirmed").unwrap();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    assert!(h
        .dispatch("t".into(), o, SessionRole::Coordinator, p.route())
        .await
        .is_err());
    assert_eq!(
        p.store()
            .read(|db| Ok(db.query_row(
                "SELECT count(*) FROM capacity_reservations WHERE state!='released'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        0
    );
    close(h).await;
}
/// claude-agent-acp asks for every tool in its `default` floor (coordinators
/// always; workers outside Auto). Bodies follow claude-agent-acp 0.100.0:
/// Cortex/external MCP tools carry `_meta.claudeCode.toolName`, native tools
/// only kind/locations/rawInput, with allow-once/allow-always/reject options.
async fn claude_permission_requests_follow_runtime_policy() {
    use workspace_service::native_sessions::SessionDriver;
    fn options() -> Value {
        json!([{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
            {"optionId":"allow-with-updates","name":"Yes, and don't ask again","kind":"allow_always"},
            {"optionId":"reject","name":"No","kind":"reject_once"}])
    }
    fn mcp(tool: &str) -> Value {
        json!({"toolCall":{"toolCallId":"toolu_mcp","title":tool,"kind":"other","rawInput":{"request_key":"k"},
            "_meta":{"claudeCode":{"toolName":format!("mcp__cortex__{tool}"),
                "mcpServer":{"name":"cortex","source":"dynamic"}}}},"options":options()})
    }
    fn native(kind: &str, raw: Value, location: Option<PathBuf>) -> Value {
        json!({"toolCall":{"toolCallId":"toolu_native","title":kind,"kind":kind,"rawInput":raw,
            "locations":location.iter().map(|p|json!({"path":p})).collect::<Vec<_>>()},"options":options()})
    }
    fn named(tool: &str) -> Value {
        json!({"toolCall":{"toolCallId":"toolu_named","title":tool,"kind":"other","rawInput":{},
            "_meta":{"claudeCode":{"toolName":tool}}},"options":options()})
    }
    async fn answer(d: &SessionDriver, s: &Store, request: &Value) -> Value {
        assert_eq!(
            d.prompt(&format!("permission-request:{request}"))
                .await
                .unwrap_or_else(|e| panic!("{request}: {e:?}"))
                .stop_reason,
            "end_turn"
        );
        let raw = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT payload_json FROM runtime_events WHERE type IN
            ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED','NATIVE_PERMISSION_AUTO_ALLOWED') ORDER BY id DESC LIMIT 1",
                    [],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        serde_json::from_str::<Value>(&raw).unwrap()["outcome"].clone()
    }
    let allow = json!({"outcome":"selected","optionId":"allow-once"});
    let reject = json!({"outcome":"selected","optionId":"reject"});
    // Coordinators are read-only: channel MCP and native reads pass, edits do not.
    for mode in ["guarded", "development", "auto"] {
        let p = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Plan".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| cortex_core::access::switch_tx(db, "t", mode, "fixture", "acp_set_mode"))
            .unwrap();
        let mut route = p.route();
        route.launch.engine = EngineKind::Claude;
        let d = h
            .dispatch("t".into(), o, SessionRole::Coordinator, route)
            .await
            .unwrap();
        let file = p.0.join("src/a.txt");
        for (request, expected) in [
            (mcp("plan_submit"), &allow),
            (mcp("project_read"), &allow),
            (mcp("user_question_request"), &allow),
            (
                native("read", json!({"file_path":file}), Some(file.clone())),
                &allow,
            ),
            (native("search", json!({"pattern":"x"}), None), &allow),
            (
                native("edit", json!({"file_path":file}), Some(file.clone())),
                &reject,
            ),
            (native("execute", json!({"command":"ls"}), None), &reject),
            (named("Agent"), &reject),
            (named("AskUserQuestion"), &reject),
        ] {
            assert_eq!(
                &answer(&d, &s, &request).await,
                expected,
                "coordinator {mode}: {request}"
            );
        }
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
    // Writers: Guarded keeps native effects inside the project, Development
    // lets them work on the host, and a deletion waits for the owner.
    for mode in ["guarded", "development", "auto"] {
        let p = Project::new();
        let outside = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db|{
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
            cortex_core::access::switch_tx(db,"t",mode,"fixture","acp_set_mode")
        }).unwrap();
        let mut route = p.route();
        route.launch.engine = EngineKind::Claude;
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                route,
            )
            .await
            .unwrap();
        let inside = p.0.join("src/a.txt");
        let host = outside.0.join("b.txt");
        let guarded = mode == "guarded";
        let host_effect = if guarded { &reject } else { &allow };
        let owner_confirmed = if mode == "auto" { &allow } else { &reject };
        let control = p.0.join(".cortex-dev/injected");
        for (request, expected) in [
            (mcp("result_submit"), &allow),
            (
                native("edit", json!({"file_path":inside}), Some(inside.clone())),
                &allow,
            ),
            // Outside the project: a read asks in Guarded (no client here: refused) and
            // is allowed in Development and Auto; a write asks in Guarded and Development.
            (
                native("read", json!({"file_path":inside}), Some(inside.clone())),
                &allow,
            ),
            (
                native("read", json!({"file_path":host}), Some(host.clone())),
                host_effect,
            ),
            (
                native("edit", json!({"file_path":host}), Some(host.clone())),
                owner_confirmed,
            ),
            (
                native(
                    "edit",
                    json!({"file_path":p.0.join("src/../../escape")}),
                    Some(p.0.join("src/../../escape")),
                ),
                owner_confirmed,
            ),
            // Cortex private state is allowed on request: Auto allows, otherwise it
            // asks (no client here: refused).
            (
                native("edit", json!({"file_path":control}), Some(control.clone())),
                owner_confirmed,
            ),
            (
                native("read", json!({"file_path":control}), Some(control.clone())),
                owner_confirmed,
            ),
            (
                native("execute", json!({"command":"ls"}), None),
                host_effect,
            ),
            (
                native("execute", json!({"command":"cd src && rm -rf build"}), None),
                owner_confirmed,
            ),
            (
                native("execute", json!({"command":"bash -c 'rm -f a'"}), None),
                owner_confirmed,
            ),
            (
                native("delete", json!({"path":inside}), Some(inside.clone())),
                owner_confirmed,
            ),
            (named("Task"), &reject),
            (named("request_user_input"), &reject),
        ] {
            assert_eq!(
                &answer(&d, &s, &request).await,
                expected,
                "writer {mode}: {request}"
            );
        }
        // An engine that offers no reject option is cancelled, never approved.
        let mut bare = native("execute", json!({"command":"rm a"}), None);
        bare["options"] = json!([{"optionId":"allow-once","name":"Yes","kind":"allow_once"}]);
        let bare_expected = if mode == "auto" {
            allow.clone()
        } else {
            json!({"outcome":"cancelled"})
        };
        assert_eq!(answer(&d, &s, &bare).await, bare_expected);
        let (requested, ruled): (i64, i64) = s.read(|db| Ok((
            db.query_row("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_PERMISSION_REQUESTED','NATIVE_PERMISSION_AUTO_ALLOWED')",[],|r|r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED','NATIVE_PERMISSION_AUTO_ALLOWED')
                AND json_extract(payload_json,'$.rule') IS NOT NULL",[],|r|r.get(0))?,
        ))).unwrap();
        assert_eq!((requested, ruled), (15, 15));
        // A channel tool the access mode allows leaves one combined record; every other
        // request keeps its request record and its separate decision record.
        let (combined, pairs, decisions): (i64, i64, i64) = s.read(|db| Ok((
            db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_AUTO_ALLOWED'
                AND json_extract(payload_json,'$.rule')='channel_mcp' AND json_extract(payload_json,'$.decider') LIKE 'mode:%'
                AND json_extract(payload_json,'$.tool') LIKE 'mcp__cortex__%'",[],|r|r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_REQUESTED'",[],|r|r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED')",[],|r|r.get(0))?,
        ))).unwrap();
        assert!(combined > 0, "{mode}");
        assert_eq!(pairs, decisions, "{mode}");
        assert_eq!(combined + pairs, 15, "{mode}");
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
}
/// Reviewers and read-only specialists run in the engine's read-only/asking mode
/// In a native Workspace task (no incoming ACP client) an "ask the owner" native
/// permission request becomes an owner decision of kind `permission` in the
/// task's decisions projection: it waits while open, Allow once and Deny reach the
/// engine as its own options with a permission receipt, a pause withdraws it and
/// cancels the request, a service restart withdraws what it left open, and
/// Cortex's own `outside_write` asks on the same card in Development.
async fn desktop_permission_decisions() {
    use cortex_core::workspace::control;
    async fn bound(
        mode: &str,
        engine: EngineKind,
    ) -> (
        Project,
        NativeHost,
        Store,
        workspace_service::native_sessions::SessionDriver,
        Project,
    ) {
        let p = Project::new();
        let outside = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        let root = p.0.to_str().unwrap().to_owned();
        let epoch = o.epoch;
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('host',?,'t','{\"name\":\"cortex-native\"}','{}','[]','2026-11-17T00:00:00Z',NULL)", [&root])?;
            db.execute("INSERT INTO workspace_projects VALUES ('wp',?,'actor','2026-11-17T00:00:00Z',1)", [&root])?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES ('wt','wp','Work','we','2026-11-17T00:00:00Z')", [])?;
            db.execute("INSERT INTO workspace_executions VALUES ('we','wt','t','host',?,1,'2026-11-17T00:00:00Z')", [epoch])?;
            cortex_core::access::switch_tx(db, "t", mode, "fixture", "acp_set_mode")
        })
        .unwrap();
        let mut route = p.route();
        route.launch.engine = engine;
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                route,
            )
            .await
            .unwrap();
        (p, h, s, d, outside)
    }
    let list = |s: &Store| -> Vec<Value> {
        control::decisions(
            s,
            "wp",
            "actor",
            &json!({"operation":"decisions.list","target":"we","limit":100}),
        )
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["kind"] == "permission")
            .cloned()
            .collect()
    };
    async fn pending(s: &Store, list: &dyn Fn(&Store) -> Vec<Value>, n: usize) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let items = list(s);
                if let Some(d) = items
                    .iter()
                    .filter(|d| d["status"] == "pending")
                    .nth(0)
                    .filter(|_| items.len() > n)
                {
                    return d.clone();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("no pending permission decision")
    }
    let answer = |s: &Store, d: &Value, label: &str, request: &str| {
        let option = d["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["label"] == label)
            .unwrap()["optionId"]
            .clone();
        control::answer(
            s,
            "wp",
            "actor",
            &json!({"protocol":{"major":1,"minor":8},"projectId":"wp","actorId":"actor",
            "requestId":request,"operation":"decisions.answer","target":d["decisionId"],"expectedVersion":d["version"],
            "ownerEpoch":d["ownerEpoch"],"payload":{"executionId":"we","acpSessionId":"host","batchId":null,"answers":[{
                "decisionId":d["decisionId"],"questionId":d["questionId"],"expectedVersion":d["version"],"optionId":option,"text":null}]}}),
        )
    };
    let last = |s: &Store, kind: &str| -> Value {
        serde_json::from_str(
            &s.read(|db| {
                Ok(db.query_row(
                    "SELECT payload_json FROM runtime_events WHERE type=? ORDER BY id DESC LIMIT 1",
                    [kind],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap(),
        )
        .unwrap()
    };
    let (p, h, s, d, outside) = bound("guarded", EngineKind::Claude).await;
    let host = outside.0.join("notes.txt");
    let request = json!({"toolCall":{"toolCallId":"toolu_read","title":"Read","kind":"read","rawInput":{"file_path":host},
        "locations":[{"path":host}]},"options":[{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
        {"optionId":"reject","name":"No","kind":"reject_once"}]});
    let text = format!("permission-request:{request}");
    for (n, (label, option, receipt)) in [
        ("Allow once", "allow-once", "allow_once"),
        ("Deny", "reject", "reject_once"),
    ]
    .into_iter()
    .enumerate()
    {
        let owner = async {
            let item = pending(&s, &list, n).await;
            // The decision names who asks, what, where, why and the access mode.
            assert_eq!(item["askedBy"], "backend-dev");
            assert_eq!(item["permission"]["operation"], "read");
            assert_eq!(item["permission"]["paths"], json!([host]));
            assert_eq!(item["permission"]["reason"], "outside_project");
            assert_eq!(item["permission"]["access"], "guarded");
            assert_eq!(item["allowedOperations"], json!(["decisions.answer"]));
            let labels: Vec<_> = item["options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["label"].clone())
                .collect();
            assert_eq!(labels, [json!("Allow once"), json!("Deny")]);
            workspace_protocol::validate(
                "DecisionPage",
                &control::decisions(
                    &s,
                    "wp",
                    "actor",
                    &json!({"operation":"decisions.list","target":"we","limit":100}),
                )
                .unwrap(),
            )
            .unwrap();
            // The notifier's selection of tasks with an open question includes it.
            let asking: i64 = s.read(|db| Ok(db.query_row("SELECT count(DISTINCT e.task_id) FROM pending_questions q JOIN workspace_executions e
                ON e.runtime_task_id=q.task_id WHERE q.status='open'", [], |r| r.get(0))?)).unwrap();
            assert_eq!(asking, 1);
            // Still pending: no decision recorded until the owner answers.
            let decided: i64 = s.read(|db| Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_DECIDED'",
                [], |r| r.get(0))?)).unwrap();
            assert_eq!(decided, n as i64);
            answer(&s, &item, label, &format!("answer-{n}")).unwrap();
        };
        let (turn, ()) = tokio::join!(d.prompt(&text), owner);
        assert_eq!(turn.unwrap().stop_reason, "end_turn");
        let decided = last(&s, "NATIVE_PERMISSION_DECIDED");
        assert_eq!(
            (
                decided["outcome"]["optionId"].as_str(),
                decided["decider"].as_str(),
                decided["rule"].as_str()
            ),
            (Some(option), Some("owner"), Some("outside_read"))
        );
        let receipts: Vec<String> = s
            .read(|db| {
                let mut q = db.prepare(
                    "SELECT host_outcome FROM permission_receipts WHERE task_id='t' ORDER BY rowid",
                )?;
                let rows = q
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(receipts.last().map(String::as_str), Some(receipt));
        let answered = list(&s).into_iter().nth(n).unwrap();
        assert_eq!(answered["status"], "answered");
        assert_eq!(
            answered["answerText"],
            if n == 0 { "Allowed once" } else { "Denied" }
        );
    }
    // A pause while the decision is open withdraws it and cancels the engine's request.
    let owner = async {
        let item = pending(&s, &list, 2).await;
        s.write(|db| {
            Ok(db.execute(
                "UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='t'",
                [],
            )?)
        })
        .unwrap();
        item
    };
    let (turn, item) = tokio::join!(d.prompt(&text), owner);
    let _ = turn;
    let withdrawn = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let x = list(&s)
                .into_iter()
                .find(|x| x["decisionId"] == item["decisionId"])
                .unwrap();
            if x["status"] != "pending" {
                return x;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("decision stayed pending after the pause");
    assert_eq!(withdrawn["status"], "superseded");
    assert_eq!(
        withdrawn["headline"],
        "The task was paused before you answered."
    );
    assert_eq!(withdrawn["allowedOperations"], json!([]));
    assert!(answer(&s, &withdrawn, "Allow once", "late").is_err());
    let _ = d.shutdown().await;
    drop(s);
    close(h).await;
    // A decision a stopped service left open is withdrawn when the service starts.
    let s = p.store();
    let id = s
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'",
                [],
            )?;
            cortex_core::permission_decisions::open_tx(
                db,
                "t",
                Some("a"),
                "gone",
                "r",
                "Owner decision: x",
                &json!({"operation":"read"}),
            )
        })
        .unwrap();
    drop(s);
    let h = p.host().await;
    let s = p.store();
    let status: String = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT status FROM pending_questions WHERE id=?",
                [&id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(status, "withdrawn");
    assert_eq!(
        last(&s, "NATIVE_PERMISSION_DECISION_WITHDRAWN")["reason"],
        "service_restarted"
    );
    drop(s);
    close(h).await;
    drop(outside);
    // Cortex's own outside_write asks the owner on the same card in Development.
    let (_p, h, s, d, outside) = bound("development", EngineKind::Codex).await;
    let target = outside.0.join("written.txt");
    let owner = async {
        let item = pending(&s, &list, 0).await;
        assert_eq!(item["permission"]["operation"], "write");
        assert_eq!(item["permission"]["access"], "development");
        answer(&s, &item, "Allow once", "outside-answer").unwrap();
    };
    let write = call(
        "outside_write",
        json!({"path":target,"content":"owner said yes","request_key":"w"}),
    );
    let (turn, ()) = tokio::join!(d.prompt(&write), owner);
    assert_eq!(turn.unwrap().stop_reason, "end_turn");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "owner said yes");
    let settled = last(&s, "NATIVE_OUTSIDE_SETTLED");
    assert_eq!(
        (
            settled["allowed"].as_bool(),
            settled["decider"].as_str(),
            settled["rule"].as_str()
        ),
        (Some(true), Some("owner"), Some("outside_write"))
    );
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
/// An "ask the owner" decision is a real, answerable card: it names the role,
/// operation, path and reason, offers only Allow once / Deny, keeps the engine's
/// request pending until the owner answers, and records the owner as decider.
async fn owner_cards_stay_pending_and_record_the_owner() {
    use cortex_engine::{engine::PermissionDecision, FutureResult};
    struct Owner {
        cards: std::sync::Mutex<Vec<Value>>,
        answers: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<String>>,
    }
    impl workspace_service::native_sessions::ClientPermissions for Owner {
        fn decide<'a>(
            &'a self,
            _session: &'a str,
            request: &'a Value,
        ) -> FutureResult<'a, PermissionDecision> {
            Box::pin(async move {
                self.cards.lock().unwrap().push(request.clone());
                Ok(match self.answers.lock().await.recv().await {
                    Some(option) => PermissionDecision::Selected(option),
                    None => PermissionDecision::Cancelled,
                })
            })
        }
    }
    let p = Project::new();
    let outside = Project::new();
    let h = p.host().await;
    let (answer, answers) = tokio::sync::mpsc::channel(4);
    let owner = std::sync::Arc::new(Owner {
        cards: Default::default(),
        answers: tokio::sync::Mutex::new(answers),
    });
    h.bind_permissions(owner.clone());
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    let root = p.0.to_str().unwrap().to_owned();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('host',?,'t','{\"name\":\"cortex-rust-acp\"}','{}','[]','2026-11-17T00:00:00Z',NULL)", [&root])?;
        cortex_core::access::switch_tx(db, "t", "guarded", "fixture", "acp_set_mode")
    })
    .unwrap();
    let mut route = p.route();
    route.launch.engine = EngineKind::Claude;
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap();
    let host = outside.0.join("notes.txt");
    let request = json!({"toolCall":{"toolCallId":"toolu_read","title":"Read","kind":"read","rawInput":{"file_path":host},
        "locations":[{"path":host}]},"options":[{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
        {"optionId":"allow-always","name":"Always","kind":"allow_always"},{"optionId":"reject","name":"No","kind":"reject_once"}]});
    let decided = |s: &Store| {
        s.read(|db| Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED')",
        [], |r| r.get::<_, i64>(0))?)).unwrap()
    };
    for (n, (choice, expected)) in [
        ("allow-once", "allow-once"),
        ("reject", "reject"),
        ("allow-always", "reject"),
    ]
    .into_iter()
    .enumerate()
    {
        let text = format!("permission-request:{request}");
        let prompt = d.prompt(&text);
        let owner_side = async {
            // The card is open and the engine's request is still pending.
            tokio::time::timeout(Duration::from_secs(5), async {
                while owner.cards.lock().unwrap().len() <= n {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("no owner card");
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(decided(&s), n as i64, "decided before the owner answered");
            answer.send(choice.to_owned()).await.unwrap();
        };
        let (turn, ()) = tokio::join!(prompt, owner_side);
        assert_eq!(turn.unwrap().stop_reason, "end_turn");
        let card = owner.cards.lock().unwrap()[n].clone();
        assert_eq!(
            card["toolCall"]["title"],
            format!("Owner decision: worker-backend-dev wants to read {} because it reads outside the project", host.display())
        );
        let kinds: Vec<&str> = card["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["allow_once", "reject_once"]);
        let payload: Value = serde_json::from_str(&s.read(|db| Ok(db.query_row(
            "SELECT payload_json FROM runtime_events WHERE type='NATIVE_PERMISSION_DECIDED' ORDER BY id DESC LIMIT 1",
            [], |r| r.get::<_, String>(0))?)).unwrap()).unwrap();
        // A denial (or an option the card did not offer) goes back as the engine's reject.
        assert_eq!(
            (
                payload["outcome"]["optionId"].as_str(),
                payload["rule"].as_str(),
                payload["decider"].as_str()
            ),
            (Some(expected), Some("outside_read"), Some("owner")),
            "{choice}"
        );
    }
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
/// in every access mode (never bypassPermissions/agent-full-access); a native
/// write or execute request from them is denied by the runtime and recorded; a
/// Claude session never carries native shell/edit tools; and an adapter without
/// a restricted mode refuses the start instead of running unrestricted.
async fn read_only_channels_start_restricted_in_every_access_mode() {
    fn options() -> Value {
        json!([{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
            {"optionId":"reject","name":"No","kind":"reject_once"}])
    }
    fn native(kind: &str, raw: Value, location: Option<PathBuf>) -> Value {
        json!({"toolCall":{"toolCallId":"toolu_native","title":kind,"kind":kind,"rawInput":raw,
            "locations":location.iter().map(|p|json!({"path":p})).collect::<Vec<_>>()},"options":options()})
    }
    fn last_decision(s: &Store) -> (String, Value) {
        let (kind, raw) = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT type,payload_json FROM runtime_events WHERE type IN
                    ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED') ORDER BY id DESC LIMIT 1",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )?)
            })
            .unwrap();
        (kind, serde_json::from_str(&raw).unwrap())
    }
    let claude_meta = json!({"/claudeCode/options/allowDangerouslySkipPermissions":false,
        "/claudeCode/options/strictMcpConfig":true,"/claudeCode/options/managedSettings/disableAllHooks":true,
        "/claudeCode/options/settings/autoMemoryEnabled":false,"/claudeCode/options/settingSources":["project"],
        "disallowed:Bash":true,"disallowed:Edit":true,"disallowed:Write":true,"disallowed:NotebookEdit":true,
        "disallowed:Agent":true,"disallowed:Read":false,"disallowed:Grep":false,"disallowed:Glob":false,
        "disallowed:WebFetch":false});
    for (engine, restricted) in [
        (EngineKind::Claude, "default"),
        (EngineKind::Codex, "read-only"),
    ] {
        for access in ["auto", "guarded", "development"] {
            for (profile, stage) in [
                ("worker-code-reviewer", "review"),
                ("worker-explorer", "execution"),
                ("worker-backend-dev", "review"),
            ] {
                let p = Project::new();
                let outside = Project::new();
                let h = p.host().await;
                let o = h
                    .create("t".into(), "Work".into(), "owner".into())
                    .await
                    .unwrap();
                let s = p.store();
                s.write(|db| {
                    let state = if stage == "review" { "REVIEWING" } else { "EXECUTING" };
                    db.execute("UPDATE runtime_tasks SET state=?,revision=1 WHERE task_id='t'", [state])?;
                    db.execute(
                        "INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Work','[]','[\"src\"]',?,'running','a',NULL)",
                        [profile, stage],
                    )?;
                    cortex_core::access::switch_tx(db, "t", access, "fixture", "acp_set_mode")
                })
                .unwrap();
                let mut route = p.route();
                route.launch.engine = engine;
                let d = h
                    .dispatch(
                        "t".into(),
                        o,
                        SessionRole::Worker {
                            assignment: "a".into(),
                        },
                        route,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("{engine:?} {access} {profile}@{stage}: {e:?}"));
                let confirmed = s.read(|db| Ok(db.query_row(
                    "SELECT json_extract(payload_json,'$.mode') FROM runtime_events WHERE type='NATIVE_ACCESS_MODE_CONFIRMED'",
                    [], |r| r.get::<_, String>(0))?)).unwrap();
                assert_eq!(
                    confirmed, restricted,
                    "{engine:?} {access} {profile}@{stage}"
                );
                let expected_meta = if engine == EngineKind::Claude {
                    claude_meta.clone()
                } else {
                    json!({"":null})
                };
                assert_eq!(
                    d.prompt(&format!("session-meta:{expected_meta}"))
                        .await
                        .unwrap()
                        .stop_reason,
                    "end_turn",
                    "{engine:?} session/new _meta"
                );
                let inside = p.0.join("src/a.txt");
                let host = outside.0.join("notes.txt");
                for (request, decided, rule) in [
                    (
                        native("execute", json!({"command":"git commit -am x"}), None),
                        false,
                        "read_only",
                    ),
                    (
                        native("edit", json!({"file_path":inside}), Some(inside.clone())),
                        false,
                        "read_only",
                    ),
                    (
                        native("delete", json!({"path":inside}), Some(inside.clone())),
                        false,
                        "read_only",
                    ),
                    (
                        native("read", json!({"file_path":host}), Some(host.clone())),
                        access != "guarded",
                        match access {
                            "auto" => "auto_outside_read",
                            _ => "outside_read",
                        },
                    ),
                    (
                        native("read", json!({"file_path":inside}), Some(inside.clone())),
                        true,
                        "",
                    ),
                ] {
                    assert_eq!(
                        d.prompt(&format!("permission-request:{request}"))
                            .await
                            .unwrap()
                            .stop_reason,
                        "end_turn"
                    );
                    let (kind, payload) = last_decision(&s);
                    let label = format!("{engine:?} {access} {profile}@{stage}: {request}");
                    // The runtime answers Claude and Codex itself, with the offered
                    // option, and records the decision with its rule.
                    assert_eq!(kind, "NATIVE_PERMISSION_DECIDED", "{label}");
                    if !rule.is_empty() {
                        assert_eq!(payload["rule"], rule, "{label}");
                    }
                    if decided {
                        assert_eq!(
                            payload["outcome"],
                            json!({"outcome":"selected","optionId":"allow-once"}),
                            "{label}"
                        );
                        assert_eq!(payload["decider"], format!("mode:{access}"), "{label}");
                    } else {
                        assert_eq!(payload["rule"], rule, "{label}");
                        assert_eq!(
                            payload["outcome"],
                            json!({"outcome":"selected","optionId":"reject"}),
                            "{label}"
                        );
                    }
                }
                d.shutdown().await.unwrap();
                drop(s);
                close(h).await;
            }
        }
    }
    // A writing worker's Claude session carries the same native tool restriction.
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        cortex_core::access::switch_tx(db, "t", "auto", "fixture", "acp_set_mode")
    })
    .unwrap();
    let mut route = p.route();
    route.launch.engine = EngineKind::Claude;
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap();
    assert_eq!(
        d.prompt(&format!("session-meta:{claude_meta}"))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
    // A writing Codex worker runs in the Cortex worker permission profile when its
    // engine offers it (the native Codex launcher defines it), in every access mode,
    // and its escalations (an apply_patch outside the project, into credentials, Git
    // controls or Cortex state) are answered by the runtime per the decision table.
    for access in ["auto", "guarded", "development"] {
        let p = Project::new();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&p.0)
            .status()
            .unwrap()
            .success());
        std::fs::write(p.0.join("codex-permission-profile"), "").unwrap();
        let outside = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\",\"nested/leaf\"]','execution','running','a',NULL)", [])?;
            cortex_core::access::switch_tx(db, "t", access, "fixture", "acp_set_mode")
        })
        .unwrap();
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                p.route(),
            )
            .await
            .unwrap();
        let confirmed = s.read(|db| Ok(db.query_row(
            "SELECT json_extract(payload_json,'$.mode') FROM runtime_events WHERE type='NATIVE_ACCESS_MODE_CONFIRMED'",
            [], |r| r.get::<_, String>(0))?)).unwrap();
        assert_eq!(confirmed, "permission-profile:cortex-workspace", "{access}");
        let inside = p.0.join("src/a.rs");
        for (target, outcome, rule) in [
            (
                inside.clone(),
                "allow-once",
                if access == "auto" {
                    "auto"
                } else if access == "development" {
                    "native_development"
                } else {
                    "native_project"
                },
            ),
            // Auto allows everything but Cortex private state (with a rule naming
            // what it allowed); outside Auto the rest asks, and with no owner client
            // connected here an ask is refused.
            (
                outside.0.join("b.rs"),
                if access == "auto" {
                    "allow-once"
                } else {
                    "reject"
                },
                if access == "auto" {
                    "auto_outside_write"
                } else {
                    "outside_write"
                },
            ),
            (
                p.0.join("deploy/server.pem"),
                if access == "auto" {
                    "allow-once"
                } else {
                    "reject"
                },
                if access == "auto" {
                    "auto_credential"
                } else {
                    "credential"
                },
            ),
            (
                p.0.join(".git/config"),
                if access == "auto" {
                    "allow-once"
                } else {
                    "reject"
                },
                if access == "auto" {
                    "auto_control"
                } else {
                    "control_target"
                },
            ),
            // Cortex private state is allowed on request like credentials; the live
            // store files are refused in every mode.
            (
                p.0.join(".cortex/injected"),
                if access == "auto" { "allow-once" } else { "reject" },
                if access == "auto" { "auto_private" } else { "cortex_private" },
            ),
            (
                p.0.join(".cortex-dev/injected"),
                if access == "auto" { "allow-once" } else { "reject" },
                if access == "auto" { "auto_private" } else { "cortex_private" },
            ),
            (p.0.join(".cortex/cortex.sqlite3-wal"), "reject", "cortex_live_store"),
        ] {
            let request = native("edit", json!({"path":target}), Some(target.clone()));
            assert_eq!(
                d.prompt(&format!("permission-request:{request}"))
                    .await
                    .unwrap()
                    .stop_reason,
                "end_turn"
            );
            let (kind, payload) = last_decision(&s);
            assert_eq!(kind, "NATIVE_PERMISSION_DECIDED", "{access} {target:?}");
            assert_eq!(
                (
                    payload["outcome"]["optionId"].as_str(),
                    payload["rule"].as_str()
                ),
                (Some(outcome), Some(rule)),
                "{access} {target:?}"
            );
            let decider = if outcome == "allow-once" {
                format!("mode:{access}")
            } else if rule == "cortex_live_store" {
                "runtime".into()
            } else {
                "no_owner_client".into()
            };
            assert_eq!(payload["decider"], decider, "{access} {target:?}");
        }
        // The sandbox itself (the fixture follows Codex 0.166 under the profile): an
        // ordinary project patch is written without a request; a patch into Cortex
        // state or Git controls fails the sandbox check, becomes an approval request
        // and is written only when the runtime allows it.
        let requested = |s: &Store| -> i64 {
            s.read(|db| Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_REQUESTED'",
                [], |r| r.get(0))?)).unwrap()
        };
        std::fs::create_dir_all(p.0.join("src")).unwrap();
        std::fs::create_dir_all(p.0.join(".cortex")).unwrap();
        let before = requested(&s);
        let patch = json!({"path":"src/plain.rs","content":"fn plain() {}\n"});
        assert_eq!(d.prompt(&format!("sandbox-patch:{patch}")).await.unwrap().stop_reason, "end_turn");
        assert_eq!(std::fs::read_to_string(p.0.join("src/plain.rs")).unwrap(), "fn plain() {}\n");
        assert_eq!(requested(&s), before, "{access}: an ordinary patch asks nothing");
        for (relative, rule) in [
            (".cortex/patched.md", if access == "auto" { "auto_private" } else { "cortex_private" }),
            (".git/patched", if access == "auto" { "auto_control" } else { "control_target" }),
            // A credential file the profile keeps read-only (the project root `.env`).
            (".env", if access == "auto" { "auto_credential" } else { "credential" }),
        ] {
            let patch = json!({"path":relative,"content":"patched"});
            assert_eq!(d.prompt(&format!("sandbox-patch:{patch}")).await.unwrap().stop_reason, "end_turn");
            let (kind, payload) = last_decision(&s);
            assert_eq!(kind, "NATIVE_PERMISSION_DECIDED", "{access} {relative}");
            assert_eq!(payload["rule"], rule, "{access} {relative}");
            let written = std::fs::read_to_string(p.0.join(relative)).ok();
            if access == "auto" {
                assert_eq!(payload["decider"], "mode:auto", "{relative}");
                assert_eq!(written.as_deref(), Some("patched"), "{relative}");
            } else {
                assert_eq!(payload["decider"], "no_owner_client", "{access} {relative}");
                assert_eq!(written, None, "{access} {relative}");
            }
        }
        // The gateway reaches the same paths only through a runtime decision: Cortex
        // state on request (missing parents included), Git reads without one, and
        // the live store files never.
        async fn call(d: &workspace_service::native_sessions::SessionDriver, name: &str, args: Value, expected: Value) {
            let prompt = format!("call:{}", json!({"name":name,"arguments":args,"__fixture_expect":expected}));
            assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, "end_turn", "{name} {args}");
        }
        let auto = access == "auto";
        let refused = json!({"/result/isError":true,"/result/structuredContent/code":"protected_path_not_confirmed"});
        call(&d, "project_write", json!({"path":".cortex/gateway/note.md","content":"note\n","expected_sha256":"absent","request_key":"gw-write"}),
            if auto { json!({"/result/isError":false}) } else { refused.clone() }).await;
        assert_eq!(std::fs::read_to_string(p.0.join(".cortex/gateway/note.md")).ok().as_deref(),
            auto.then_some("note\n"), "{access}");
        call(&d, "project_read", json!({"path":".cortex/gateway/note.md"}),
            if auto { json!({"/result/isError":false}) } else { refused.clone() }).await;
        call(&d, "project_read", json!({"path":".git/HEAD"}), json!({"/result/isError":false})).await;
        call(&d, "project_write", json!({"path":".cortex/cortex.sqlite3-journal","content":"x","expected_sha256":"absent","request_key":"gw-live"}),
            json!({"/result/isError":true,"/result/structuredContent/code":"cortex_live_store"})).await;
        assert!(!p.0.join(".cortex/cortex.sqlite3-journal").exists());
        // A Git control file and credential files (by name, in any directory) are
        // decided the same way for gateway writes; reading a credential file and
        // writing ordinary source that merely mentions a token need no decision.
        call(&d, "project_write", json!({"path":".git/qa-note","content":"1\n","expected_sha256":"absent","request_key":"gw-git"}),
            if auto { json!({"/result/isError":false}) } else { refused.clone() }).await;
        assert_eq!(std::fs::read_to_string(p.0.join(".git/qa-note")).ok().as_deref(), auto.then_some("1\n"), "{access}");
        call(&d, "project_write", json!({"path":"config/.env.production","content":"QA=1\n","expected_sha256":"absent","request_key":"gw-env"}),
            if auto { json!({"/result/isError":false}) } else { refused.clone() }).await;
        assert_eq!(std::fs::read_to_string(p.0.join("config/.env.production")).ok().as_deref(), auto.then_some("QA=1\n"), "{access}");
        std::fs::create_dir_all(p.0.join("app")).unwrap();
        std::fs::write(p.0.join("app/.env"), "SAMPLE=1\n").unwrap();
        call(&d, "project_read", json!({"path":"app/.env"}), json!({"/result/isError":false})).await;
        call(&d, "project_edit", json!({"path":"app/.env","edits":[{"old_string":"SAMPLE=1\n","new_string":"SAMPLE=1\nQA=2\n"}],"request_key":"gw-env-edit"}),
            if auto { json!({"/result/isError":false}) } else { refused.clone() }).await;
        assert_eq!(std::fs::read_to_string(p.0.join("app/.env")).unwrap(), if auto { "SAMPLE=1\nQA=2\n" } else { "SAMPLE=1\n" }, "{access}");
        call(&d, "project_write", json!({"path":"src/token.rs","content":"pub fn token() {}\n","expected_sha256":"absent","request_key":"gw-source"}),
            json!({"/result/isError":false})).await;
        let decided: Vec<(String, bool, String)> = s.read(|db| {
            let mut q = db.prepare("SELECT json_extract(payload_json,'$.rule'),json_extract(payload_json,'$.allowed'),json_extract(payload_json,'$.decider')
                FROM runtime_events WHERE type='NATIVE_PROTECTED_PATH_DECIDED' ORDER BY id")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }).unwrap();
        let asked = if auto { ("cortex_private".to_owned(), true, "mode:auto".to_owned()) }
            else { ("cortex_private".to_owned(), false, "no_owner_client".to_owned()) };
        let rule = |rule: &str| (rule.to_owned(), asked.1, asked.2.clone());
        assert_eq!(decided, vec![asked.clone(), asked.clone(), ("cortex_live_store".into(), false, "runtime".into()),
            rule("control_target"), rule("credential"), rule("credential")], "{access}");
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
    // No restricted mode offered: the read-only session does not start.
    for engine in [EngineKind::Codex, EngineKind::Claude] {
        let p = Project::new();
        std::fs::write(p.0.join("no-restricted-modes"), "").unwrap();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Review".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-code-reviewer','Review','[]','[\"src\"]','review','running','a',NULL)", [])?;
            cortex_core::access::switch_tx(db, "t", "auto", "fixture", "acp_set_mode")
        })
        .unwrap();
        let mut route = p.route();
        route.launch.engine = engine;
        assert!(h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into()
                },
                route
            )
            .await
            .is_err());
        let failure: Value = serde_json::from_str(&s.read(|db| Ok(db.query_row(
            "SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' ORDER BY id DESC LIMIT 1",
            [], |r| r.get::<_, String>(0))?)).unwrap()).unwrap();
        assert_eq!(
            (failure["phase"].as_str(), failure["error"].as_str()),
            (Some("access_mode"), Some("restricted_mode_unavailable")),
            "{engine:?}"
        );
        let configured: i64 = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_ACCESS_MODE_CONFIRMED'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(configured, 0);
        drop(s);
        close(h).await;
    }
}
/// The configured-session record carries the engine's own name for the model: an alias such as
/// `default` reads with the concrete model its entry describes.
async fn engine_model_name_is_recorded_with_the_configured_session() {
    for (model, label) in [
        ("default", Some("Default (Opus 5.5)")),
        ("fixture-model", Some("Fixture")),
    ] {
        let p = Project::new();
        std::fs::write(p.0.join("model-alias"), "").unwrap();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
            Ok(())
        })
        .unwrap();
        let mut route = p.route();
        route.model = model.into();
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                route,
            )
            .await
            .unwrap();
        let payload: Value = serde_json::from_str(&s.read(|db| Ok(db.query_row(
            "SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_CONFIGURED' ORDER BY id DESC LIMIT 1",
            [], |r| r.get::<_, String>(0))?)).unwrap()).unwrap();
        assert_eq!(payload["model"], model);
        assert_eq!(payload["model_label"].as_str(), label, "{model}: {payload}");
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
}
async fn worker_question_answer_exact_delivery_and_native_denial() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        Ok(())
    }).unwrap();
    let d = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    assert_eq!(
        d.prompt(&call(
            "answer_submit",
            json!({"text":"Not worker authority","request_key":"forged"})
        ))
        .await
        .unwrap()
        .stop_reason,
        "refusal"
    );
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Executing);
    d.prompt("permission").await.unwrap();
    s.read(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_CANCELLED'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        Ok(())
    })
    .unwrap();
    let invalid = format!("call:{}", json!({"name":"ask_question",
        "arguments":{"recommended":3,"question":"Which output?","options":["Detailed","Brief"],"request_key":"out-of-list"},
        "__fixture_expect":{"/result/isError":true,"/result/structuredContent/code":"question_recommendation_invalid"}
    }));
    assert_eq!(d.prompt(&invalid).await.unwrap().stop_reason, "end_turn");
    s.read(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM pending_questions", [], |row| row.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
    let q = call(
        "ask_question",
        json!({"recommended":2,"question":"Which output?","options":["Detailed","Brief"],"request_key":"question"}),
    );
    assert_eq!(d.prompt(&q).await.unwrap().stop_reason, "end_turn");
    assert_eq!(d.prompt(&q).await.unwrap().stop_reason, "end_turn");
    assert_eq!(d.prompt(&call("result_submit",json!({
        "report":"# Premature result","summary":"Still waiting","checks":[],"request_key":"waiting-result"
    }))).await.unwrap().stop_reason,"refusal");
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    let question = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE status='open'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    s.read(|db| {
        let stored: String = db.query_row(
            "SELECT options_json FROM pending_questions WHERE id=?", [&question], |row| row.get(0),
        )?;
        assert_eq!(serde_json::from_str::<Value>(&stored)?, json!([
            "Detailed", {"label":"Brief","recommended":true},
        ]));
        Ok(())
    }).unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Executing);
    assert_eq!(
        h.answer(
            "t".into(),
            o.clone(),
            "answer".into(),
            question.clone(),
            "custom free text".into()
        )
        .await
        .unwrap()["recorded"],
        true
    );
    assert_eq!(
        h.answer(
            "t".into(),
            o,
            "answer".into(),
            question,
            "custom free text".into()
        )
        .await
        .unwrap()["recorded"],
        true
    );
    assert!(d.continue_answer().await.unwrap());
    assert!(!d.continue_answer().await.unwrap());
    s.write(|db| {
        let waits = cortex_core::lifecycle::waits::projection_tx(db, "t")?;
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0]["status"], "resumed");
        Ok(())
    })
    .unwrap();
    let q = call(
        "ask_question",
        json!({"question":"Wait for which result?","recommended":1,"options":["First","Second"],"request_key":"question2"}),
    );
    d.prompt(&q).await.unwrap();
    assert_eq!(d.prompt(&call("result_submit",json!({
        "report":"# Premature result","summary":"Still waiting","checks":[],"request_key":"waiting-result"
    }))).await.unwrap().stop_reason,"refusal");
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    let question = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE status='open'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let owner = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT owner_ref,epoch FROM task_owners WHERE task_id='t'",
                [],
                |r| {
                    Ok(cortex_core::ownership::OwnerEpoch {
                        owner: r.get(0)?,
                        epoch: r.get(1)?,
                    })
                },
            )?)
        })
        .unwrap();
    h.answer("t".into(), owner, "answer2".into(), question, "hang".into())
        .await
        .unwrap();
    {
        let continuation = d.continue_answer();
        tokio::pin!(continuation);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(30))=>{},r=&mut continuation=>panic!("unexpected early continuation result: {r:?}")}
        d.cancel().await.unwrap();
        assert!(continuation.await.is_err());
    }
    assert!(!d.continue_answer().await.unwrap());
    s.write(|db| {
        let waits = cortex_core::lifecycle::waits::projection_tx(db, "t")?;
        assert_eq!(waits.last().unwrap()["status"], "orphaned");
        Ok(())
    })
    .unwrap();
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
/// The worker asks, the owner answers, and in the answer's own turn the worker requests
/// a plan change: the replan cancels its assignment, revokes its channel and closes the
/// resuming wait as `cancelled` before ACP replies. The engine acted on the answer, so
/// the delivery is settled, never reported as unconfirmed.
async fn answer_turn_replan_settles_the_answer_delivery() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        Ok(())
    }).unwrap();
    let d = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    let q = call(
        "ask_question",
        json!({"question":"Which route?","recommended":1,"options":["Light","Stop"],"request_key":"route-question"}),
    );
    assert_eq!(d.prompt(&q).await.unwrap().stop_reason, "end_turn");
    let question = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE status='open'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    // The fixture engine performs the call the answer text names, in the answer's turn.
    let answer = call(
        "replan_request",
        json!({"reason":"The owner's answer changes the plan","request_key":"answer-replan"}),
    );
    assert_eq!(
        h.answer("t".into(), o, "answer".into(), question, answer)
            .await
            .unwrap()["recorded"],
        true
    );
    assert!(d.continue_answer().await.unwrap());
    s.write(|db| {
        let waits = cortex_core::lifecycle::waits::projection_tx(db, "t")?;
        assert_eq!(waits.last().unwrap()["status"], "cancelled");
        assert_eq!(waits.last().unwrap()["reason"], "replanning");
        assert!(waits.iter().all(|w| w["status"] != "orphaned"));
        let replanned: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id='t' AND type='REPLAN_REQUEST_ACCEPTED')",
            [],
            |r| r.get(0),
        )?;
        assert!(replanned);
        Ok(())
    })
    .unwrap();
    d.shutdown().await.unwrap();
    s.read(|db| {
        let mut q = db.prepare(
            "SELECT engine_session_ref,call_sequence FROM usage_records
             WHERE task_id='t' ORDER BY call_sequence",
        )?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, rows[1].0);
        assert_eq!((rows[0].1, rows[1].1), (1, 2));
        Ok(())
    })
    .unwrap();
    drop(s);
    close(h).await;
}
async fn pause_resume_does_not_revive_old_coordinator_and_cancel_is_not_completion() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Goal".into(), "owner".into())
        .await
        .unwrap();
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    h.owner_action(
        "t".into(),
        o.clone(),
        "pause".into(),
        RuntimeAction::Pause {
            resume: false,
            expected_version: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Paused);
    assert!(d.prompt("must not run").await.is_err());
    let s = p.store();
    let version = s.write(|db| domain::pause_version_tx(db, "t")).unwrap();
    assert!(h
        .owner_action(
            "t".into(),
            o.clone(),
            "stale".into(),
            RuntimeAction::Pause {
                resume: true,
                expected_version: version + 1
            }
        )
        .await
        .is_err());
    h.owner_action(
        "t".into(),
        o.clone(),
        "resume".into(),
        RuntimeAction::Pause {
            resume: true,
            expected_version: version,
        },
    )
    .await
    .unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    assert!(d.prompt("still fenced").await.is_err());
    // Capacity remains held until the old process is actually reaped.
    assert!(h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .is_err());
    d.shutdown().await.unwrap();
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    h.owner_action(
        "t".into(),
        o.clone(),
        "pause".into(),
        RuntimeAction::Pause {
            resume: false,
            expected_version: 0,
        },
    )
    .await
    .unwrap();
    h.owner_action(
        "t".into(),
        o.clone(),
        "resume".into(),
        RuntimeAction::Pause {
            resume: true,
            expected_version: version,
        },
    )
    .await
    .unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    d.prompt("replayed controls must not revoke this session")
        .await
        .unwrap();
    s.write(|db|{
        db.execute_batch("CREATE TRIGGER fail_native_owner_receipt BEFORE INSERT ON runtime_requests WHEN NEW.operation='native_owner_action' BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;Ok(())
    }).unwrap();
    assert!(h
        .owner_action(
            "t".into(),
            o.clone(),
            "rollback-pause".into(),
            RuntimeAction::Pause {
                resume: false,
                expected_version: version
            }
        )
        .await
        .is_err());
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Planning);
    d.prompt("rolled back receipt must also roll back revocation")
        .await
        .unwrap();
    s.write(|db| {
        db.execute_batch("DROP TRIGGER fail_native_owner_receipt")?;
        Ok(())
    })
    .unwrap();
    h.owner_action(
        "t".into(),
        o,
        "stop".into(),
        RuntimeAction::Cancel {
            reason: "owner stopped".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Cancelled);
    assert!(d.prompt("must stay stopped").await.is_err());
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
async fn startup_failure_settles_only_owned_reaped_process() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Goal".into(), "owner".into())
        .await
        .unwrap();
    let mut route = p.route();
    route.model = "unobserved-model".into();
    assert!(h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, route)
        .await
        .is_err());
    let s = p.store();
    s.read(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM capacity_reservations WHERE state='released'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM trusted_channels WHERE revoked_at IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
    let d = h
        .dispatch("t".into(), o, SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
}
/// A refusal before the engine process exists (here a launcher that cannot be
/// prepared) must settle as a confirmed absence, never as an ambiguous worker
/// that blocks recovery with `lost_worker_termination_unconfirmed`.
async fn launch_refusal_before_spawn_is_a_confirmed_absence() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Goal".into(), "owner".into())
        .await
        .unwrap();
    let mut route = p.route();
    route.launch.codex_launcher = Some(CodexLauncher::new(|| Err(cortex_engine::Error::Invalid)));
    assert!(h
        .dispatch("t".into(), o, SessionRole::Coordinator, route)
        .await
        .is_err());
    let s = p.store();
    s.read(|db| {
        let why: String = db.query_row("SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' ORDER BY id DESC LIMIT 1",[],|r|r.get(0))?;
        assert!(why.contains("\"phase\":\"launch\"") && why.contains("invalid_arguments"), "{why}");
        let status: String = db.query_row("SELECT status FROM delegation_provisioning ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0))?;
        assert_eq!(status, "terminated");
        assert_eq!(db.query_row("SELECT count(*) FROM capacity_reservations WHERE state='released'",[],|r|r.get::<_,i64>(0))?, 1);
        Ok(())
    })
    .unwrap();
    drop(s);
    close(h).await;
}
/// Behind Cortex's native Codex launcher the session's relays travel with the launcher (App Server
/// defines them as config layers), not in ACP `session/new`, whose server table would replace the
/// layer that turns the owner's own Codex MCP servers off. The fixture engine refuses a session that
/// gets them both ways; the coordinator still reaches the runtime's tools through the relay.
async fn codex_launcher_sessions_hand_relays_to_app_server_not_acp() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Explain".into(), "owner".into())
        .await
        .unwrap();
    let mut route = p.route();
    let launcher = std::env::current_exe().unwrap();
    route.launch.codex_launcher = Some(CodexLauncher::new(move || Ok(launcher.clone())));
    let dispatched = h
        .dispatch("t".into(), o, SessionRole::Coordinator, route)
        .await;
    if dispatched.is_err() {
        let diagnostic=p.store().read(|db|Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' ORDER BY id DESC LIMIT 1",[],|r|r.get::<_,String>(0))?)).unwrap();
        panic!("native start diagnostic: {diagnostic}");
    }
    let d = dispatched.unwrap();
    // An empty plan is refused by the runtime: the call went through the handed-over relay.
    assert_eq!(
        d.prompt(&call("plan_submit", json!({})))
            .await
            .unwrap()
            .stop_reason,
        "refusal"
    );
    d.shutdown().await.unwrap();
    close(h).await;
}
/// claude-agent-acp 0.107.0 sends the new session's `available_commands_update` right
/// behind the `session/new` reply. Coordinator sessions with an effort parameter start
/// every time (they failed intermittently in `configure` with
/// `transport_closed_outcome_unknown` while the host closed on that update).
async fn claude_update_behind_session_new_reply_never_fails_configure() {
    let p = Project::new();
    std::fs::write(p.0.join("commands-after-new"), "").unwrap();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Explain".into(), "owner".into())
        .await
        .unwrap();
    for _ in 0..5 {
        let mut route = p.route();
        route.launch.engine = EngineKind::Claude;
        route
            .parameters
            .insert("thought_level".into(), "high".into());
        let d = h
            .dispatch("t".into(), o.clone(), SessionRole::Coordinator, route)
            .await
            .unwrap_or_else(|e| {
                let why = p.store().read(|db| Ok(db.query_row("SELECT coalesce(group_concat(payload_json),'') FROM runtime_events WHERE type LIKE 'NATIVE_SESSION_START_%'",[],|r|r.get::<_,String>(0))?)).unwrap();
                panic!("{e:?}; {why}");
            });
        d.shutdown().await.unwrap();
    }
    let s = p.store();
    s.read(|db| {
        let failed: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type LIKE 'NATIVE_SESSION_START_%'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(failed, 0);
        Ok(())
    })
    .unwrap();
    drop(s);
    close(h).await;
}
/// A transport that closes while an unbound session is configured, with the engine's
/// exit confirmed, starts the session once more under the same channel. Both attempts
/// are recorded; the failed one keeps its exit status, close reason and a private,
/// redacted stderr tail named by its digest.
async fn configure_transport_loss_starts_once_more_and_records_both_attempts() {
    let p = Project::new();
    std::fs::write(p.0.join("configure-crash-once"), "").unwrap();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Explain".into(), "owner".into())
        .await
        .unwrap();
    let d = h
        .dispatch("t".into(), o, SessionRole::Coordinator, p.route())
        .await
        .unwrap_or_else(|e| {
            let why = p.store().read(|db| Ok(db.query_row("SELECT coalesce(group_concat(payload_json),'') FROM runtime_events WHERE type LIKE 'NATIVE_SESSION_START_%'",[],|r|r.get::<_,String>(0))?)).unwrap();
            panic!("{e:?}; {why}");
        });
    assert!(p.0.join("configure-crashed").exists());
    let s = p.store();
    let (payload, channel, bound) = s
        .read(|db| {
            let rows = db.prepare("SELECT type,payload_json FROM runtime_events WHERE type LIKE 'NATIVE_SESSION_START_%' ORDER BY id")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].0, "NATIVE_SESSION_START_RETRIED");
            let payload: Value = serde_json::from_str(&rows[0].1).unwrap();
            let channel: String = db.query_row("SELECT channel_id FROM trusted_channels WHERE revoked_at IS NULL", [], |r| r.get(0))?;
            let bound: i64 = db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status='ready' AND engine_session_ref IS NOT NULL", [], |r| r.get(0))?;
            Ok((payload, channel, bound))
        })
        .unwrap();
    // The same channel carries the second attempt, bound once.
    assert_eq!(payload["channel_id"], channel);
    assert_eq!(bound, 1);
    assert_eq!(payload["phase"], "configure");
    assert_eq!(payload["error"], "transport_closed_outcome_unknown");
    assert_eq!(payload["attempt"], 1);
    assert_eq!(payload["role"], "coordinator");
    assert!(payload.get("blocker").is_none());
    let diagnostics = &payload["diagnostics"];
    assert_eq!(diagnostics["exit"], "exit:3", "{payload}");
    assert_eq!(diagnostics["exited_unprompted"], true, "{payload}");
    assert!(diagnostics["stderr_bytes"].as_u64().unwrap() > 0);
    let file =
        p.0.join(".cortex-dev/t/.native-diagnostics")
            .join(diagnostics["file"].as_str().unwrap());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let body = std::fs::read_to_string(&file).unwrap();
    use sha2::Digest;
    assert_eq!(
        diagnostics["sha256"],
        format!("{:x}", sha2::Sha256::digest(body.as_bytes()))
    );
    assert!(body.contains("configure crashed"), "{body}");
    assert!(body.contains("phase: configure"), "{body}");
    for secret in [
        "sk-fixture",
        "0123456789abcdefghijklmnop",
        "fixture-bearer-value",
    ] {
        assert!(!body.contains(secret), "{secret}: {body}");
    }
    drop(s);
    d.shutdown().await.unwrap();
    close(h).await;
}
/// A second transport loss stops the start: the final failure is recorded with the
/// blocker that names the role whose session could not start, the reservation is
/// released and no channel stays live.
async fn repeated_configure_transport_loss_stops_with_the_roles_blocker() {
    for worker in [false, true] {
        let p = Project::new();
        std::fs::write(p.0.join("configure-crash"), "").unwrap();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        let role = if worker {
            s.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
                Ok(())
            })
            .unwrap();
            SessionRole::Worker {
                assignment: "a".into(),
            }
        } else {
            SessionRole::Coordinator
        };
        let error = h
            .dispatch("t".into(), o, role, p.route())
            .await
            .err()
            .expect("a session whose adapter always dies never starts");
        let expected = if worker {
            "native_session_start_failed"
        } else {
            "coordinator_session_start_failed"
        };
        assert_eq!(error.code(), expected);
        s.read(|db| {
            let rows = db.prepare("SELECT type,payload_json FROM runtime_events WHERE type LIKE 'NATIVE_SESSION_START_%' ORDER BY id")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let kinds: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
            assert_eq!(kinds, ["NATIVE_SESSION_START_RETRIED", "NATIVE_SESSION_START_FAILED"]);
            let last: Value = serde_json::from_str(&rows[1].1).unwrap();
            assert_eq!(last["attempt"], 2);
            assert_eq!(last["phase"], "configure");
            assert_eq!(last["blocker"], expected);
            assert_eq!(last["role"], if worker { "worker" } else { "coordinator" });
            assert!(last["diagnostics"]["file"].as_str().unwrap().ends_with("-start-2.log"));
            assert_eq!(db.query_row("SELECT count(*) FROM capacity_reservations WHERE state!='released'",[],|r|r.get::<_,i64>(0))?, 0);
            assert_eq!(db.query_row("SELECT count(*) FROM trusted_channels WHERE revoked_at IS NULL",[],|r|r.get::<_,i64>(0))?, 0);
            Ok(())
        })
        .unwrap();
        let files = std::fs::read_dir(p.0.join(".cortex-dev/t/.native-diagnostics"))
            .unwrap()
            .count();
        assert_eq!(files, 2);
        drop(s);
        close(h).await;
    }
}
async fn inline_result_is_published_once_bound_to_attempt_and_reconciled_after_restart() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Work".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        Ok(())
    }).unwrap();
    let d = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    let args = with_sections(
        json!({"report":"# Bounded native result\n\nObserved fixture result, not task certification.",
        "summary":"x".repeat(4000),"checks":[{"name":"paid desktop check","outcome":"not_run",
            "blocker":"paid live check not requested by the owner"}],"request_key":"result"}),
    );
    let mut forged = args.clone();
    forged["checks"] = json!([{"name":"invented pass","outcome":"passed","command":["true"],
        "tool_call_ref":"gateway:00000000000000000000000000000000"}]);
    // A passed claim needs a project observation. With the folder unobservable (a
    // `.git` entry Git cannot use, in this disposable fixture only) the claim is refused
    // and nothing is published. (On an observable folder an invented pass is recorded
    // `unverified` with its evidence gap, never as passed: `worker_checks::evaluate`.)
    std::fs::write(p.0.join(".git"), "gitdir: /nonexistent/cortex-fixture\n").unwrap();
    let forged_stop = d
        .prompt(&call("result_submit", forged))
        .await
        .unwrap()
        .stop_reason;
    std::fs::remove_file(p.0.join(".git")).unwrap();
    assert_eq!(forged_stop, "refusal");
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    // Publication commits before the lifecycle transaction. Simulate failure of
    // the latter; exact retry must use the same immutable file, not publish again.
    s.write(|db| {
        db.execute_batch("CREATE TRIGGER refuse_native_result BEFORE INSERT ON runtime_events
            WHEN NEW.type='WORKER_COMPLETED' BEGIN SELECT RAISE(ABORT,'fixture result failure'); END;")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        d.prompt(&call("result_submit", args.clone()))
            .await
            .unwrap()
            .stop_reason,
        "refusal"
    );
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM task_events WHERE event_type='worker_result'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT status FROM runtime_nodes WHERE assignment_ref='a'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "running"
        );
        Ok(())
    })
    .unwrap();
    let mut conflict = args.clone();
    conflict["report"] = json!("# Changed after ambiguous delivery");
    let conflict = with_sections(conflict);
    assert_eq!(
        d.prompt(&call("result_submit", conflict))
            .await
            .unwrap()
            .stop_reason,
        "refusal"
    );
    s.write(|db| {
        db.execute_batch("DROP TRIGGER refuse_native_result")?;
        Ok(())
    })
    .unwrap();
    submit_result(&h, &d, "t", &o, "a", args.clone()).await;
    assert_eq!(h.state("t".into()).await.unwrap(), TaskState::Executing);
    let receipt = h
        .result_receipt("t".into(), o.clone(), "a".into(), "result".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt["certified"], false);
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM task_events WHERE event_type='worker_result'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        let result: String = db.query_row(
            "SELECT result_json FROM runtime_nodes WHERE assignment_ref='a'",
            [],
            |r| r.get(0),
        )?;
        let result: Value = serde_json::from_str(&result)?;
        assert_eq!(result["report_ref"], receipt["report_ref"]);
        assert_eq!(result["summary"].as_str().unwrap().len(), 4000);
        let mut expected = args["checks"].clone();
        expected[0]["evidence_strength"] = json!("none");
        assert_eq!(result["checks"], expected);
        // A reported node still owns its engine reservation until owned wait.
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM capacity_reservations WHERE state='reserved'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        Ok(())
    })
    .unwrap();
    assert!(d.prompt(&call("result_submit", args)).await.is_err());
    d.shutdown().await.unwrap();
    drop(s);
    close(h).await;
    let reopened = p.host().await;
    assert_eq!(
        reopened
            .result_receipt("t".into(), o.clone(), "a".into(), "result".into())
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(reopened
        .result_receipt(
            "t".into(),
            cortex_core::ownership::OwnerEpoch {
                owner: o.owner,
                epoch: o.epoch + 1
            },
            "a".into(),
            "result".into()
        )
        .await
        .is_err());
    close(reopened).await;
}
async fn prompt_usage_follows_real_native_dispatches() {
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Usage fixture".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    std::fs::write(p.0.join("record-prompts"), "").unwrap();
    s.write(|db| {
        db.execute_batch(
            "CREATE TRIGGER fixture_fail_usage BEFORE INSERT ON usage_records
             WHEN NEW.call_sequence=5
             BEGIN SELECT RAISE(ABORT,'fixture usage insert failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();

    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    let wire_session = d.session_id().to_owned();
    for text in [
        "fixture-usage-reported",
        "fixture-usage-zero",
        "fixture-usage-invalid",
    ] {
        assert_eq!(d.prompt(text).await.unwrap().stop_reason, "end_turn");
    }
    // A settled reply already has its usage row before any shutdown write.
    s.read(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM usage_records WHERE task_id='t'",
                [],
                |r| r.get::<_, i64>(0),
            )?,
            3,
        );
        Ok(())
    }).unwrap();
    assert_eq!(
        d.prompt("fixture-usage-refusal").await.unwrap().stop_reason,
        "refusal",
    );
    // The accounting trigger aborts this call's insert but not its own settle
    // event, and no replay is delivered to the fake engine.
    assert_eq!(
        d.prompt("fixture-usage-reported")
            .await
            .unwrap()
            .stop_reason,
        "end_turn",
    );
    let prompt_count = || {
        std::fs::read_to_string(p.0.join("prompt-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    {
        let turn = d.prompt("fixture-usage-hang");
        tokio::pin!(turn);
        tokio::select! {
            result = &mut turn => panic!("fixture must stay pending: {:?}", result.err()),
            _ = async {
                while prompt_count() != 6 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => {}
        }
        // Drop an already dispatched prompt, as owner/service interruption does.
    }
    assert_eq!(
        d.prompt("fixture-usage-zero").await.err().unwrap().code(),
        "native_turn_outcome_unknown",
    );
    assert_eq!(prompt_count(), 6);
    d.shutdown().await.unwrap();
    s.read(|db| {
        let (attempt, engine_ref): (String, String) = db.query_row(
            "SELECT c.attempt_id, json_extract(e.payload_json,'$.engine_session_ref')
             FROM trusted_channels c JOIN runtime_events e
             ON c.channel_id=json_extract(e.payload_json,'$.channel_id')
             WHERE c.role='coordinator' AND c.task_id='t' AND e.type='NATIVE_SESSION_CONFIGURED'
               AND json_extract(e.payload_json,'$.engine')='codex-acp'
               AND json_extract(e.payload_json,'$.model')='fixture-model'
               AND json_extract(e.payload_json,'$.session')=?",
            [&wire_session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(engine_ref, format!("native:{attempt}:{wire_session}"));
        let mut rows = db.prepare(
            "SELECT call_sequence,basis,tokens_json,stop_reason,failed
             FROM usage_records WHERE task_id='t' AND attempt_id=? AND engine_session_ref=?
             ORDER BY call_sequence",
        )?;
        let actual = rows
            .query_map((&attempt, &engine_ref), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let expected = [
            (
                1,
                "engine_reported",
                json!({"inputTokens":4,"outputTokens":2,"cachedReadTokens":9,"totalTokens":15}),
                Some("end_turn"),
                0,
            ),
            (
                2,
                "engine_reported",
                json!({"totalTokens":0}),
                Some("end_turn"),
                0,
            ),
            (3, "unknown", json!({}), Some("end_turn"), 0),
            (
                4,
                "engine_reported",
                json!({"outputTokens":3}),
                Some("refusal"),
                0,
            ),
            (6, "unknown", json!({}), None, 1),
        ];
        assert_eq!(actual.len(), expected.len());
        for ((seq, basis, tokens, reason, failed), expected) in actual.into_iter().zip(expected) {
            assert_eq!(
                (
                    seq,
                    basis.as_str(),
                    serde_json::from_str(&tokens)?,
                    reason.as_deref(),
                    failed
                ),
                expected,
            );
        }
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM runtime_events
                 WHERE task_id='t' AND type='NATIVE_TURN_SETTLED'",
                [],
                |r| r.get::<_, i64>(0),
            )?,
            5,
        );
        Ok(())
    })
    .unwrap();

    // A real worker result is retained even though the adapter exits without
    // returning the last response. A prompt refused before dispatch adds no row.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        Ok(())
    }).unwrap();
    std::fs::write(p.0.join("scenario.txt"), "usage-exit-after-result").unwrap();
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    let args = with_sections(json!({
        "report":"# Usage exit fixture","summary":"The result was accepted",
        "checks":[],"request_key":"usage-result"
    }));
    assert_eq!(
        w.prompt(&call("result_submit", args))
            .await
            .err()
            .unwrap()
            .code(),
        "native_turn_outcome_unknown",
    );
    assert_eq!(
        h.result_receipt("t".into(), o, "a".into(), "usage-result".into())
            .await
            .unwrap()
            .unwrap()["outcome"],
        "recorded",
    );
    assert!(w.prompt("fixture-usage-zero").await.is_err());
    let worker_session = w.session_id().to_owned();
    w.shutdown().await.unwrap();
    s.read(|db| {
        let expected = db.query_row(
            "SELECT attempt_id FROM trusted_channels WHERE task_id='t' AND role='worker'",
            [],
            |r| r.get::<_, String>(0),
        )?;
        let (attempt, session, seq, basis, tokens, stop, failed) = db.query_row(
            "SELECT attempt_id,engine_session_ref,call_sequence,basis,tokens_json,stop_reason,failed
             FROM usage_records WHERE task_id='t' AND attempt_id=?",
            [&expected],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?)),
        )?;
        assert_eq!(attempt, expected);
        assert_eq!(session, format!("native:{expected}:{worker_session}"));
        assert_eq!(
            (seq, basis.as_str(), tokens.as_str(), stop, failed),
            (1, "unknown", "{}", None, 1)
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM usage_records", [], |r| r
                .get::<_, i64>(0))?,
            6,
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(prompt_count(), 7);
    drop(s);
    close(h).await;
}

async fn result_failure_and_review_refusal_are_not_acceptance() {
    for stage in ["execution", "review"] {
        let p = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state=?,revision=1 WHERE task_id='t'",
                [if stage=="review" {"REVIEWING"} else {"EXECUTING"}])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Work','[]','[]',?,'running','a',NULL)",
                (if stage=="review" {"worker-code-reviewer"} else {"worker-backend-dev"}, stage))?;
            Ok(())
        }).unwrap();
        let d = h
            .dispatch(
                "t".into(),
                o.clone(),
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                p.route(),
            )
            .await
            .unwrap();
        let mut args = with_sections(
            json!({"report":"# Retained failure","summary":"Incomplete work","checks":[],
            "request_key":"failed-result","assignment_outcome":"failed"}),
        );
        if stage == "review" {
            args["verdict"] = json!("APPROVE");
        } else {
            // A failure is accepted only as a named dead end: without its blocker it
            // is refused, correctably, and the same assignment stays active.
            d.prompt(&call("result_submit", args.clone()))
                .await
                .unwrap();
            let refused: String = s
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT json_extract(payload_json,'$.code') FROM runtime_events
                         WHERE type='WORKER_RESULT_REFUSED' ORDER BY id DESC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(refused, "assignment_blocker_required");
        }
        args["blocker"] = json!({"kind":"owner_decision",
            "detail":"Which of the two retained formats to keep is the owner's decision."});
        if stage == "review" {
            assert_eq!(
                d.prompt(&call("result_submit", args))
                    .await
                    .unwrap()
                    .stop_reason,
                "refusal"
            );
        } else {
            submit_result(&h, &d, "t", &o, "a", args).await;
        }
        s.read(|db| {
            let status: String = db.query_row(
                "SELECT status FROM runtime_nodes WHERE assignment_ref='a'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                status,
                if stage == "review" {
                    "running"
                } else {
                    "failed"
                }
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, i64>(0))?,
                if stage == "review" { 0 } else { 1 }
            );
            Ok(())
        })
        .unwrap();
        assert_ne!(h.state("t".into()).await.unwrap(), TaskState::Completed);
        d.shutdown().await.unwrap();
        drop(s);
        close(h).await;
    }
}

async fn native_plan_review_verification_journey(
    restart: bool,
    failing: bool,
    fix: bool,
    truncated: bool,
    verifier: Option<&command_acceptance::VerifierInputs>,
) {
    let p = Project::new();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&p.0)
        .status()
        .unwrap()
        .success());
    std::fs::write(p.0.join("work.txt"), "before").unwrap();
    let mut h = p.host().await;
    let o = h
        .create("journey".into(), "Implement fixture".into(), "owner".into())
        .await
        .unwrap();
    if let Some(inputs) = verifier {
        inputs.prepare(&p.0);
    }
    let coordinator = h
        .dispatch(
            "journey".into(),
            o.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    let verification = if let Some(inputs) = verifier {
        json!([inputs.argv()])
    } else if truncated {
        json!([["/usr/bin/dd", "if=/dev/zero", "bs=1048577", "count=1", "status=none"]])
    } else if failing {
        json!([["/bin/false"]])
    } else {
        json!([["/usr/bin/cmp", "/dev/null", "/dev/null"]])
    };
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Change work.txt","dependencies":[],"scope":["work.txt"],"model":"auto"}],
        "verification":verification});
    // Invalid proposals cannot change the task, create nodes or consume the key; each is
    // a correctable content error (isError with its code), not an authority refusal.
    for bad in [
        json!({"tasks":[],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"code-reviewer","objective":"Reserved","dependencies":[],"scope":["."],"model":"auto"}],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Cycle","dependencies":["work"],"scope":["."],"model":"auto"}],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Escape","dependencies":[],"scope":["../private"],"model":"auto"}],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Shell","dependencies":[],"scope":["."],"model":"auto"}],"verification":[["true","&&","false"]]}),
    ] {
        let stop = coordinator
            .prompt(&call(
                "plan_submit",
                json!({"plan":bad,"request_key":"bad"}),
            ))
            .await
            .unwrap()
            .stop_reason;
        // A schema-invalid plan is an argument refusal; a schema-valid one the runtime
        // refuses is a correctable content error (end_turn with isError).
        assert!(stop == "refusal" || stop == "end_turn", "{bad}: {stop}");
        assert_eq!(
            h.state("journey".into()).await.unwrap(),
            TaskState::Planning
        );
    }
    if !verification_available(&p.0) {
        assert_eq!(
            coordinator
                .prompt(&call(
                    "plan_submit",
                    json!({"plan":plan,"request_key":"unavailable"})
                ))
                .await
                .unwrap()
                .stop_reason,
            "end_turn"
        );
        assert_eq!(
            h.state("journey".into()).await.unwrap(),
            TaskState::Planning
        );
        p.store()
            .read(|db| {
                let count: i64 = db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='PLAN_ACCEPTED'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
        coordinator.shutdown().await.unwrap();
        close(h).await;
        println!("native verification unavailable: plan refused without acceptance");
        assert!(
            verifier.is_none(),
            "required read-only verifier unavailable; no compiler payload was launched"
        );
        return;
    }
    let proposal = call("plan_submit", json!({"plan":plan,"request_key":"plan"}));
    assert_eq!(
        coordinator.prompt(&proposal).await.unwrap().stop_reason,
        "end_turn"
    );
    assert_eq!(
        coordinator.prompt(&proposal).await.unwrap().stop_reason,
        "end_turn"
    );
    coordinator.shutdown().await.unwrap();
    let step = h.advance("journey".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "EXECUTING", "admitted verifier plan: {step}");
    let a = step["assignments"][0]["assignment_ref"]
        .as_str()
        .expect("worker assignment missing after plan admission")
        .to_owned();
    let worker = h
        .dispatch(
            "journey".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: a.clone(),
            },
            p.native_writer_route(),
        )
        .await
        .unwrap();
    // The truncated case is about the verification output (dd, below). The changed
    // file stays readable: a single line past project_read's line bound can never be
    // read completely, which is a reviewer dead end, not an approval.
    // The worker's engine edits the file natively, inside its turn and without a
    // Cortex tool: the turn's change record attributes it to the task.
    let edit = worker
        .prompt(r#"native-write:{"path":"work.txt","content":"after"}"#)
        .await
        .unwrap();
    assert_eq!(edit.stop_reason, "end_turn");
    assert_eq!(
        std::fs::read_to_string(p.0.join("work.txt")).unwrap(),
        "after"
    );
    let _ = truncated;
    let result = json!({"report":"# Work\n\nFixture file changed.","summary":"Changed the fixture file.","checks":[],"request_key":"result"});
    submit_result(&h, &worker, "journey", &o, &a, result).await;
    // A report never stands in for a process wait or begins verification.
    assert_eq!(
        h.advance("journey".into(), o.clone()).await.unwrap()["state"],
        "EXECUTING"
    );
    worker.shutdown().await.unwrap();
    // Another actor's uncommitted file in the shared checkout, changed outside any
    // turn or command of this task: listed as context, never required or bound.
    std::fs::write(p.0.join("unrelated.txt"), "someone else's work").unwrap();
    let mut step = h.advance("journey".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "REVIEWING");
    assert_eq!(step["assignments"][0]["role"], "worker-code-reviewer");
    let listed = &step["assignments"][0]["review_packet"]["changed_files"];
    let required = |path: &str| {
        listed["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .map(|f| f["required"].clone())
    };
    assert_eq!(listed["attribution_complete"], true, "{listed}");
    assert_eq!(required("work.txt"), Some(json!(true)), "{listed}");
    assert_eq!(required("unrelated.txt"), Some(json!(false)), "{listed}");
    // The binding stays with the recorded packet, not in the reviewer's brief.
    assert!(step["assignments"][0]["review_packet"]["review_binding"].is_null());
    if !restart {
        // It keeps changing during the review: no re-binding, no refusal below.
        std::fs::write(p.0.join("unrelated.txt"), "changed again").unwrap();
    }
    if restart {
        let before = step["assignments"][0]["assignment_ref"].clone();
        close(h).await;
        h = p.host().await;
        step = h.advance("journey".into(), o.clone()).await.unwrap();
        assert_eq!(step["assignments"][0]["assignment_ref"], before);
    }
    let mut review = h
        .dispatch(
            "journey".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: step["assignments"][0]["assignment_ref"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    if restart {
        review
            .prompt("Read the fixture, then stop before reporting.")
            .await
            .unwrap();
        let old = step["assignments"][0]["assignment_ref"].clone();
        review.shutdown().await.unwrap();
        close(h).await;
        h = p.host().await;
        step = h.advance("journey".into(), o.clone()).await.unwrap();
        assert!(step["assignments"].as_array().unwrap().is_empty());
        tokio::time::sleep(Duration::from_millis(5100)).await;
        step = h.advance("journey".into(), o.clone()).await.unwrap();
        assert_ne!(step["assignments"][0]["assignment_ref"], old);
        review = h
            .dispatch(
                "journey".into(),
                o.clone(),
                SessionRole::Worker {
                    assignment: step["assignments"][0]["assignment_ref"]
                        .as_str()
                        .unwrap()
                        .into(),
                },
                p.route(),
            )
            .await
            .unwrap();
    }
    review_reads(
        &p,
        &review,
        "journey",
        step["assignments"][0]["assignment_ref"].as_str().unwrap(),
    )
    .await;
    let mut result = json!({"report":"# Independent review\n\nObserved fixture file and worker report.","summary":"Reviewed the fixture.",
        "checks":[],"verdict":if fix{"REQUEST_CHANGES"}else{"APPROVE"},"request_key":"review",
        "findings":if fix{json!([{"node":"work","files":["work.txt"],"fix":"Make the fixture correct.","cross_cutting":false}])}else{json!([])}});
    if restart {
        let assignment = step["assignments"][0]["assignment_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let refusal = || {
            p.store()
                .read(|db| {
                    let raw: String = db.query_row(
                        "SELECT payload_json FROM runtime_events WHERE task_id='journey' AND type='WORKER_RESULT_REFUSED'
                         AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                        [&assignment],
                        |r| r.get(0),
                    )?;
                    Ok(serde_json::from_str::<Value>(&raw)?)
                })
                .unwrap()
        };
        std::fs::write(p.0.join("work.txt"), "changed during review").unwrap();
        review
            .prompt(&call("result_submit", result.clone()))
            .await
            .unwrap();
        p.store().read(|db|{
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_nodes WHERE stage='review' AND status='running' AND result_json IS NULL",[],|r|r.get::<_,i64>(0))?,1);
            assert_eq!(db.query_row("SELECT count(*) FROM reports",[],|r|r.get::<_,i64>(0))?,1);
            Ok(())
        }).unwrap();
        assert_eq!(refusal()["code"], "review_project_revision_changed");
        // The review is rebound to the current state and its packet now names the
        // current bytes: the earlier read of the replaced bytes no longer covers the
        // file, so a verdict submitted without re-reading it is refused, unrecorded.
        result["request_key"] = json!("review-unread");
        review
            .prompt(&call("result_submit", with_sections(result.clone())))
            .await
            .unwrap();
        let stale = refusal();
        assert_eq!(stale["code"], "review_file_coverage_required", "{stale}");
        assert_eq!(
            stale["missing"],
            json!(["work.txt (current complete read is missing or stale)"])
        );
        assert!(h
            .result_receipt(
                "journey".into(),
                o.clone(),
                assignment.clone(),
                "review-unread".into()
            )
            .await
            .unwrap()
            .is_none());
        // The reviewer re-reads what changed, then submits with a fresh request_key.
        review_reads(&p, &review, "journey", &assignment).await;
        result["request_key"] = json!("review-current");
    }
    submit_result(
        &h,
        &review,
        "journey",
        &o,
        step["assignments"][0]["assignment_ref"].as_str().unwrap(),
        result,
    )
    .await;
    review.shutdown().await.unwrap();
    let mut final_step = h.advance("journey".into(), o.clone()).await.unwrap();
    if fix {
        assert_eq!(final_step["state"], "EXECUTING");
        let worker = h
            .dispatch(
                "journey".into(),
                o.clone(),
                SessionRole::Worker {
                    assignment: final_step["assignments"][0]["assignment_ref"]
                        .as_str()
                        .unwrap()
                        .into(),
                },
                p.route(),
            )
            .await
            .unwrap();
        std::fs::write(p.0.join("work.txt"), "fixed").unwrap();
        submit_result(&h,&worker,"journey",&o,final_step["assignments"][0]["assignment_ref"].as_str().unwrap(),json!({"report":"# Fix\n\nCorrected fixture.","summary":"Fixed the fixture.","checks":[],"request_key":"fix"})).await;
        worker.shutdown().await.unwrap();
        let step = h.advance("journey".into(), o.clone()).await.unwrap();
        assert_eq!(step["state"], "REVIEWING");
        let review = h
            .dispatch(
                "journey".into(),
                o.clone(),
                SessionRole::Worker {
                    assignment: step["assignments"][0]["assignment_ref"]
                        .as_str()
                        .unwrap()
                        .into(),
                },
                p.route(),
            )
            .await
            .unwrap();
        review_reads(
            &p,
            &review,
            "journey",
            step["assignments"][0]["assignment_ref"].as_str().unwrap(),
        )
        .await;
        submit_result(&h,&review,"journey",&o,step["assignments"][0]["assignment_ref"].as_str().unwrap(),json!({"report":"# Review fix\n\nFix verified.","summary":"Approved the fix.","checks":[],"verdict":"APPROVE","request_key":"approve-fix"})).await;
        review.shutdown().await.unwrap();
        final_step = h.advance("journey".into(), o.clone()).await.unwrap();
    }
    if final_step["state"] == "RECONCILING" {
        // Repair history deliberately disables runtime-composed synthesis.
        // Verification is not completion until a coordinator reconciles it.
        assert!(!failing && !truncated);
        assert_eq!(p.store().read(|db|Ok(db.query_row(
            "SELECT count(*) FROM runtime_tasks WHERE task_id='journey' AND synthesis IS NOT NULL",[],|r|r.get::<_,i64>(0))?)).unwrap(),0);
        let coordinator = h
            .dispatch(
                "journey".into(),
                o.clone(),
                SessionRole::Coordinator,
                p.route(),
            )
            .await
            .unwrap();
        coordinator
            .prompt(&call(
                "synthesis_submit",
                json!({"text":"Reviewed repair and runtime verification reconciled."}),
            ))
            .await
            .unwrap();
        coordinator.shutdown().await.unwrap();
        final_step = h.advance("journey".into(), o.clone()).await.unwrap();
    }
    assert_eq!(
        final_step["state"],
        if failing || truncated {
            "PLANNING"
        } else {
            "COMPLETED"
        }
    );
    p.store().read(|db|{
        let plans:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE type='PLAN_ACCEPTED'",[],|r|r.get(0))?;
        assert_eq!(plans,1);
        let receipts:i64=db.query_row("SELECT count(*) FROM runtime_requests WHERE operation='verification_command'",[],|r|r.get(0))?;
        assert_eq!(receipts,1);
        let output:String=db.query_row("SELECT result_json FROM runtime_requests WHERE operation='native_command_output'",[],|r|r.get(0))?;
        let output:Value=serde_json::from_str(&output)?;
        assert_eq!(output["profile"],"native-readonly-landlock");
        assert_eq!(output["observation"]["complete"],!truncated);
        assert_eq!(output["observation"]["output_truncated"],truncated);
        assert_eq!(output["observation"]["exit_code"],if failing{1}else{0});
        if verifier.is_some() {
            assert_eq!(output["observation"]["stdout"], command_acceptance::VERIFIER_OUTPUT);
        }
        let event:String=db.query_row("SELECT type FROM runtime_events WHERE type IN ('VERIFICATION_FAILED','VERIFICATION_PASSED')",[],|r|r.get(0))?;
        assert_eq!(event,if failing||truncated{"VERIFICATION_FAILED"}else{"VERIFICATION_PASSED"});
        Ok(())
    }).unwrap();
    close(h).await;
    if let Some(inputs) = verifier {
        inputs.assert_effects(&p.0);
        println!("verifier: host input visible, ignored build output retained, non-ignored write did not persist and private store input was masked");
    }
}

/// A task in a plain project folder (no Git repository anywhere above it) runs its
/// whole lifecycle: plan acceptance with an execution baseline, a worker that writes a
/// file, an independent review told exactly which file changed, runtime verification
/// and completion. The project observation never stops it for a missing repository.
async fn plain_folder_task_runs_its_whole_lifecycle() {
    let p = Project::new();
    assert!(std::fs::symlink_metadata(p.0.join(".git")).is_err());
    let outside = std::process::Command::new("/usr/bin/git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&p.0)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        !outside.status.success(),
        "the fixture folder must be in no Git repository"
    );
    std::fs::write(p.0.join("work.txt"), "before").unwrap();
    let h = p.host().await;
    let o = h
        .create("plain".into(), "Implement fixture".into(), "owner".into())
        .await
        .unwrap();
    let coordinator = h
        .dispatch(
            "plain".into(),
            o.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Change work.txt","dependencies":[],"scope":["work.txt"],"model":"auto"}],
        "verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
    if !verification_available(&p.0) {
        coordinator.shutdown().await.unwrap();
        close(h).await;
        println!("native verification unavailable: plain-folder lifecycle not run");
        return;
    }
    assert_eq!(
        coordinator
            .prompt(&call(
                "plan_submit",
                json!({"plan":plan,"request_key":"plan"})
            ))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    coordinator.shutdown().await.unwrap();
    let step = h.advance("plain".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "EXECUTING", "{step}");
    let a = step["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let worker = h
        .dispatch(
            "plain".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: a.clone(),
            },
            p.native_writer_route(),
        )
        .await
        .unwrap();
    // The worker's engine edits the file natively, inside its turn and without a
    // Cortex tool: the turn's change record attributes it to the task.
    let edit = worker
        .prompt(r#"native-write:{"path":"work.txt","content":"after"}"#)
        .await
        .unwrap();
    assert_eq!(edit.stop_reason, "end_turn");
    assert_eq!(
        std::fs::read_to_string(p.0.join("work.txt")).unwrap(),
        "after"
    );
    let result = json!({"report":"# Work\n\nFixture file changed.","summary":"Changed the fixture file.","checks":[],"request_key":"result"});
    submit_result(&h, &worker, "plain", &o, &a, result).await;
    worker.shutdown().await.unwrap();
    let step = h.advance("plain".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "REVIEWING", "{step}");
    assert_eq!(step["assignments"][0]["role"], "worker-code-reviewer");
    // The review packet compares the folder with the execution baseline completely
    // and names the file the worker changed.
    let changed = &step["assignments"][0]["review_packet"]["changed_files"];
    assert_eq!(changed["complete"], true, "{changed}");
    assert!(
        changed["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "work.txt" && f["change"] == "modified"),
        "{changed}"
    );
    let review_ref = step["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let review = h
        .dispatch(
            "plain".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: review_ref.clone(),
            },
            p.route(),
        )
        .await
        .unwrap();
    review_reads(&p, &review, "plain", &review_ref).await;
    submit_result(&h, &review, "plain", &o, &review_ref,
        json!({"report":"# Independent review\n\nObserved the fixture file and the worker report.","summary":"Reviewed the fixture.",
            "checks":[],"verdict":"APPROVE","request_key":"review","findings":[]})).await;
    review.shutdown().await.unwrap();
    let mut final_step = h.advance("plain".into(), o.clone()).await.unwrap();
    if final_step["state"] == "RECONCILING" {
        let coordinator = h
            .dispatch(
                "plain".into(),
                o.clone(),
                SessionRole::Coordinator,
                p.route(),
            )
            .await
            .unwrap();
        coordinator
            .prompt(&call(
                "synthesis_submit",
                json!({"text":"Reviewed work and runtime verification reconciled."}),
            ))
            .await
            .unwrap();
        coordinator.shutdown().await.unwrap();
        final_step = h.advance("plain".into(), o.clone()).await.unwrap();
    }
    assert_eq!(final_step["state"], "COMPLETED", "{final_step}");
    p.store().read(|db|{
        let passed:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='plain' AND type='VERIFICATION_PASSED'",[],|r|r.get(0))?;
        assert_eq!(passed,1);
        let stopped:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='plain' AND (type='NATIVE_DRIVER_STOPPED' OR payload_json LIKE '%project_revision_unavailable%')",[],|r|r.get(0))?;
        assert_eq!(stopped,0);
        let baseline:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='plain' AND type='EXECUTION_BASELINE'",[],|r|r.get(0))?;
        assert_eq!(baseline,1);
        Ok(())
    }).unwrap();
    // The runtime never created a repository in the folder.
    assert!(std::fs::symlink_metadata(p.0.join(".git")).is_err());
    close(h).await;
}

/// Two tasks sharing one checkout cannot let the second task mutate the tree while
/// the first task is moving from execution into independent review. Releasing the
/// sibling writer lets both tasks finish their own review and verification paths.
async fn shared_checkout_writer_waits_until_review_and_both_verify() {
    use cortex_core::ownership::OwnerEpoch;
    let p = Project::new();
    if !verification_available(&p.0) {
        println!("native verification unavailable: shared checkout journey not run");
        return;
    }
    let h = p.host().await;
    let plan = async |task: &str, extra: bool| {
        let owner = h
            .create(task.into(), "Shared checkout".into(), "owner".into())
            .await
            .unwrap();
        let coordinator = h
            .dispatch(
                task.into(),
                owner.clone(),
                SessionRole::Coordinator,
                p.route(),
            )
            .await
            .unwrap();
        let mut nodes = vec![
            json!({"id":"work","role":"backend-dev","objective":"Update a file","dependencies":[],"scope":[task],"model":"auto"}),
        ];
        if extra {
            nodes.push(json!({"id":"more","role":"backend-dev","objective":"Finish the file","dependencies":["work"],"scope":[task],"model":"auto"}));
        }
        coordinator.prompt(&call("plan_submit",json!({"request_key":"plan","plan":{"tasks":nodes,"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]}}))).await.unwrap();
        coordinator.shutdown().await.unwrap();
        let work = h.advance(task.into(), owner.clone()).await.unwrap();
        (owner, work)
    };
    let finish_step = async |task: &str, owner: &OwnerEpoch, step: &Value| {
        let node = &step["assignments"][0];
        let assignment = node["assignment_ref"].as_str().unwrap().to_owned();
        let driver = h
            .dispatch(
                task.into(),
                owner.clone(),
                SessionRole::Worker {
                    assignment: assignment.clone(),
                },
                p.route(),
            )
            .await
            .unwrap();
        let mut result = json!({"report":"# Report\n\nObserved the fixture change.","summary":"Observed the fixture change.","checks":[],"request_key":"submit"});
        if node["stage"] == "review" {
            result["verdict"] = json!("APPROVE");
            result["findings"] = json!([]);
            review_reads(&p, &driver, task, &assignment).await;
        } else {
            std::fs::write(p.0.join(task), node["node"].as_str().unwrap()).unwrap();
            // The runtime sees a worker write only through its recorded effect; a bare
            // file write is invisible, so a read-only review would not need the checkout.
            let (task_id, node_id) = (task.to_owned(), node["node"].as_str().unwrap().to_owned());
            p.store().write(|db| {
                cortex_core::lifecycle::event_tx(db, &task_id, "NATIVE_PROJECT_WRITTEN", json!({"path":task_id,"node":node_id}))?;
                Ok(())
            }).unwrap();
        }
        submit_result(&h, &driver, task, owner, &assignment, result).await;
        driver.shutdown().await.unwrap();
    };
    let complete = async |task: &str, owner: &OwnerEpoch| {
        let review = h.advance(task.into(), owner.clone()).await.unwrap();
        assert_eq!(review["assignments"][0]["stage"], "review", "{review}");
        finish_step(task, owner, &review).await;
        let mut step = h.advance(task.into(), owner.clone()).await.unwrap();
        if step["state"] == "RECONCILING" {
            let coord = h
                .dispatch(
                    task.into(),
                    owner.clone(),
                    SessionRole::Coordinator,
                    p.route(),
                )
                .await
                .unwrap();
            coord
                .prompt(&call(
                    "synthesis_submit",
                    json!({"text":"Reviewed work and runtime verification reconciled."}),
                ))
                .await
                .unwrap();
            coord.shutdown().await.unwrap();
            step = h.advance(task.into(), owner.clone()).await.unwrap();
        }
        assert_eq!(step["state"], "COMPLETED", "{step}");
    };
    let (a, work_a) = plan("a", false).await;
    let (b, work_b) = plan("b", true).await;
    finish_step("a", &a, &work_a).await;
    let waiting = h.advance("a".into(), a.clone()).await.unwrap();
    assert_eq!(waiting["writeConflict"], true, "{waiting}");
    finish_step("b", &b, &work_b).await;
    let queued = h.advance("b".into(), b.clone()).await.unwrap();
    assert_eq!(queued["writeConflict"], true, "{queued}");
    complete("a", &a).await;
    let more = h.advance("b".into(), b.clone()).await.unwrap();
    assert_eq!(more["assignments"][0]["node"], "more", "{more}");
    finish_step("b", &b, &more).await;
    complete("b", &b).await;
    p.store().read(|db| {
        for task in ["a", "b"] {
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", [task], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'", [task], |r| r.get::<_, i64>(0))?, 0);
        }
        Ok(())
    }).unwrap();
    close(h).await;
}

async fn legacy_task_admission_and_session_lifetime_fence() {
    let p = Project::new();
    let h = p.host().await;
    let owner = h
        .create("native".into(), "Fixture".into(), "owner".into())
        .await
        .unwrap();
    let driver = h
        .dispatch(
            "native".into(),
            owner.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    close(h).await;
    // The session still owns the executor even though its embedding host handle
    // was dropped. Both the lifetime and scheduler fences must remain held.
    let key = p.key();
    assert_eq!(
        tokio::task::spawn_blocking(move || NativeHost::open(key))
            .await
            .unwrap()
            .err()
            .unwrap()
            .code(),
        "scheduler_owner_busy"
    );
    driver.shutdown().await.unwrap();
    let h = p.host().await; // Native task marker survives reopening.
    h.owner_action(
        "native".into(),
        owner,
        "cancel".into(),
        RuntimeAction::Cancel {
            reason: "fixture".into(),
        },
    )
    .await
    .unwrap();
    close(h).await;

    // A terminated native task and an unfinished retained task can share the
    // store, but the latter can never be silently claimed by the native driver.
    cortex_core::lifecycle::TaskMachine::new(&p.store())
        .create("legacy", "Fixture", "legacy-owner")
        .unwrap();
    let key = p.key();
    assert_eq!(
        tokio::task::spawn_blocking(move || NativeHost::open(key))
            .await
            .unwrap()
            .err()
            .unwrap()
            .code(),
        "legacy_runtime_tasks_active"
    );
    p.store()
        .write(|db| domain::cancel_task_tx(db, "legacy", "fixture"))
        .unwrap();
    let h = p.host().await;
    close(h).await;
}

/// Project skills are read through Cortex MCP `read_skill` by name, directory or
/// file path; escapes and unknown names are precise tool errors, not authority refusals.
async fn project_skills_read_through_the_channel() {
    let p = Project::new();
    std::fs::create_dir_all(p.0.join(".agents/skills/topic-research")).unwrap();
    std::fs::write(
        p.0.join(".agents/skills/topic-research/SKILL.md"),
        "---\nname: topic-research\ndescription: Research a topic.\n---\n# Topic research fixture\n",
    )
    .unwrap();
    std::fs::create_dir_all(p.0.join(".agents/skills/tool-discipline")).unwrap();
    std::fs::write(
        p.0.join(".agents/skills/tool-discipline/SKILL.md"),
        "PROJECT COPY",
    )
    .unwrap();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Project skill fixture".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    async fn checked(
        d: &workspace_service::native_sessions::SessionDriver,
        skill: &str,
        expected: Value,
    ) {
        let prompt = format!(
            "call:{}",
            json!({"name":"read_skill","arguments":{"skill":skill},"__fixture_expect":expected})
        );
        assert_eq!(
            d.prompt(&prompt).await.unwrap().stop_reason,
            "end_turn",
            "{skill}"
        );
    }
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    checked(
        &d,
        "topic-research",
        json!({"/result/isError":false,"/result/structuredContent/source":"project",
            "/result/structuredContent/path":".agents/skills/topic-research/SKILL.md"}),
    )
    .await;
    d.shutdown().await.unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-general','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        Ok(())
    }).unwrap();
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    for form in [
        "topic-research",
        ".agents/skills/topic-research",
        ".agents/skills/topic-research/SKILL.md",
    ] {
        checked(
            &w,
            form,
            json!({"/result/isError":false,"/result/structuredContent/source":"project",
                "/result/structuredContent/path":".agents/skills/topic-research/SKILL.md",
                "/result/structuredContent/body":"---\nname: topic-research\ndescription: Research a topic.\n---\n# Topic research fixture\n"}),
        )
        .await;
    }
    // The Cortex id wins over the same-named project file; its path reads the file.
    let bundled: Value = serde_json::from_str(include_str!("../schema/instructions.json")).unwrap();
    checked(
        &w,
        "tool-discipline",
        json!({"/result/isError":false,
            "/result/structuredContent/body":bundled["skills"]["tool-discipline"]}),
    )
    .await;
    checked(
        &w,
        ".agents/skills/tool-discipline/SKILL.md",
        json!({"/result/isError":false,"/result/structuredContent/body":"PROJECT COPY"}),
    )
    .await;
    for (form, code) in [
        ("../outside/SKILL.md", "project_skill_path_refused"),
        (".cortex-dev/t/SKILL.md", "project_skill_path_refused"),
        ("/etc/passwd", "project_skill_outside_project"),
        ("unknown-skill", "skill_unknown"),
        ("coordinator-communication", "skill_not_for_role"),
    ] {
        checked(
            &w,
            form,
            json!({"/result/isError":true,"/result/structuredContent/code":code,
                "/result/structuredContent/skill":form}),
        )
        .await;
    }
    w.shutdown().await.unwrap();
    close(h).await;
}
async fn native_workspace_binding_projects_activity_and_scoped_skills() {
    use cortex_core::projections::ProjectionReader;
    use workspace_protocol::{
        core::{CoreQuery, Page, Target},
        read::QueryReply,
    };
    let p = Project::new();
    let h = p.host().await;
    let owner = h
        .create("runtime".into(), "Goal".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('project',?,'actor','now',1)",
            [p.0.to_str()],
        )?;
        Ok(())
    })
    .unwrap();
    let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft","projectId":"project",
        "actorId":"actor","operation":"tasks.create","target":"project","expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Native task"}});
    let draft = cortex_core::workspace::create_draft(&s, "project", "actor", &command)
        .unwrap()
        .receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let execution = h
        .bind_workspace(
            "project".into(),
            "actor".into(),
            draft.clone(),
            "runtime".into(),
            owner.clone(),
        )
        .await
        .unwrap();
    let reader = ProjectionReader::new(
        Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "project".into(),
        "actor".into(),
    )
    .unwrap();
    let snapshot = reader
        .query(CoreQuery::TaskGet(Target { id: draft.clone() }))
        .unwrap();
    let QueryReply::Task(t) = snapshot.data else {
        panic!()
    };
    assert_eq!(t.execution_id.as_deref(), Some(execution.as_str()));
    assert_eq!(t.phase, "planning");
    let subscription = reader
        .subscribe(&snapshot.generation, snapshot.high_watermark)
        .unwrap();
    let driver = h
        .dispatch(
            "runtime".into(),
            owner.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    assert_eq!(
        driver
            .prompt(&call("read_skill", json!({"skill":"tool-discipline"})))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    // A Cortex skill outside this role is a precise tool error, not an authority refusal.
    assert_eq!(
        driver
            .prompt(&format!(
                "call:{}",
                json!({"name":"read_skill","arguments":{"skill":"tool-discipline-worker"},
                    "__fixture_expect":{"/result/isError":true,
                        "/result/structuredContent/code":"skill_not_for_role"}})
            ))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    let turn = driver.prompt("native-activity").await.unwrap();
    assert_eq!(turn.updates, 4);
    reader.poll().unwrap();
    assert!(subscription.next().is_some());
    let query = || {
        CoreQuery::History(Page {
            target: draft.clone(),
            cursor: None,
            limit: 20,
        })
    };
    let QueryReply::History(page) = reader.query(query()).unwrap().data else {
        panic!()
    };
    assert_eq!(page.items.len(), 3);
    let encoded = serde_json::to_string(&page).unwrap();
    assert!(encoded.contains("Native public response."));
    assert!(encoded.contains("orchestrator: Fixture step"));
    assert!(!encoded.contains("private-fixture-value"), "a thought and a credential-named argument are never retained");
    assert!(encoded.contains("shown-fixture-input") && encoded.contains("shown-fixture-output"), "{encoded}");
    assert!(!encoded.contains("Skills already included"));
    assert!(page.next_cursor.is_none());
    // Cortex MCP calls in both engine shapes keep their real arguments and results; only the credential-named value is masked.
    assert_eq!(driver.prompt("native-cortex-tools").await.unwrap().updates, 5);
    let rows: Vec<String> = s
        .read(|db| {
            Ok(db
                .prepare("SELECT update_json FROM host_updates WHERE json_extract(update_json,'$.toolCallId') LIKE '%:claude-batch' OR json_extract(update_json,'$.toolCallId') LIKE '%:codex-report' ORDER BY seq")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap();
    let retained = rows.join("\n");
    for want in ["read_batch", "r_fixture", "batch result text", "report body text", "[REDACTED]"] {
        assert!(retained.contains(want), "{want}: {retained}");
    }
    assert!(!retained.contains("hidden-fixture-token"), "{retained}");
    let QueryReply::History(page) = reader.query(query()).unwrap().data else {
        panic!()
    };
    let steps: Vec<Value> = page
        .items
        .iter()
        .filter_map(|r| r.text.as_deref()?.strip_prefix("Workspace step: ").map(|s| serde_json::from_str(s).unwrap()))
        .collect();
    let step = |id: &str| steps.iter().rev().find(|s| s["callId"].as_str().is_some_and(|c| c.ends_with(id))).cloned().unwrap_or_else(|| panic!("{id}: {steps:?}"));
    assert!(step(":claude-batch")["input"].as_str().unwrap().contains("r_fixture"));
    assert_eq!(step(":claude-batch")["output"], "batch result text");
    assert_eq!(step(":codex-report")["output"], "report body text");
    assert!(step(":codex-report")["input"].as_str().unwrap().contains("r_fixture"));
    assert_eq!(
        h.state("runtime".into()).await.unwrap(),
        TaskState::Planning
    );
    // One streamed message is one stable identity, not one per engine chunk.
    let before: i64 = s
        .read(|db| Ok(db.query_row("SELECT coalesce(max(seq),-1) FROM (SELECT seq FROM host_transcript UNION ALL SELECT seq FROM host_updates)", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(driver.prompt("native-chunked").await.unwrap().updates, 6);
    let rows: Vec<(String, Option<String>)> = s
        .read(|db| {
            Ok(db
                .prepare("SELECT text,message_id FROM host_transcript WHERE kind='agent_message' AND seq>? ORDER BY seq")?
                .query_map([before], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })
        .unwrap();
    let texts: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(texts, ["Вы выб", "рали `div", "ide`.", "Review ", "text."]);
    let ids: Vec<_> = rows
        .iter()
        .map(|r| r.1.clone().expect("message id"))
        .collect();
    assert!(ids[0] == ids[1] && ids[1] == ids[2], "{ids:?}");
    assert!(ids[3] == ids[4] && ids[3] != ids[0], "{ids:?}");
    // A later turn of the same channel is a new message even with no tool row between.
    driver.prompt("native-chunked-next").await.unwrap();
    let next: Option<String> = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT message_id FROM host_transcript WHERE kind='agent_message' AND text='Next turn.' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_ne!(
        next.as_deref(),
        Some(ids[4].as_str()),
        "a new turn must not join the previous message"
    );
    // The coordinator asks mid-message: chunks streamed after the question commit
    // still belong to the Workspace conversation, in order, never cut short.
    let before: i64 = s
        .read(|db| Ok(db.query_row("SELECT coalesce(max(seq),-1) FROM (SELECT seq FROM host_transcript UNION ALL SELECT seq FROM host_updates)", [], |r| r.get(0))?))
        .unwrap();
    let ask = json!({"name":"user_question_request","arguments":{"question":"Which layout?",
        "recommended":1,"options":["Split","Single"],"request_key":"midstream"},
        "__fixture_before":["Findings: the layout ","has two "],
        "__fixture_after":["options and ","one risk."]});
    assert_eq!(
        driver
            .prompt(&format!("call:{ask}"))
            .await
            .unwrap()
            .stop_reason,
        "end_turn"
    );
    let streamed: Vec<(String, String)> = s
        .read(|db| {
            Ok(db
                .prepare("SELECT text,message_id FROM host_transcript WHERE kind='agent_message' AND seq>? ORDER BY seq")?
                .query_map([before], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })
        .unwrap();
    let texts: Vec<_> = streamed.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Findings: the layout ",
            "has two ",
            "options and ",
            "one risk.",
            "Post-question coordinator text"
        ]
    );
    assert!(
        streamed[..4]
            .iter()
            .all(|r| r.1.ends_with(":fixture-midstream")),
        "{streamed:?}"
    );
    let QueryReply::History(page) = reader.query(query()).unwrap().data else {
        panic!()
    };
    // The page lists one item per stored chunk; the conversation joins them in order.
    let joined: String = serde_json::to_value(&page).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "assistant")
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert!(
        joined.contains("Findings: the layout has two options and one risk."),
        "{joined}"
    );
    assert_eq!(
        h.state("runtime".into()).await.unwrap(),
        TaskState::WaitingUser
    );
    let question: String = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE task_id='runtime' AND status='open'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        h.answer(
            "runtime".into(),
            owner.clone(),
            "midstream-answer".into(),
            question,
            "Split".into()
        )
        .await
        .unwrap()["recorded"],
        true
    );
    driver.shutdown().await.unwrap();
    close(h).await;
    let h = p.host().await;
    assert_eq!(
        h.bind_workspace(
            "project".into(),
            "actor".into(),
            draft,
            "runtime".into(),
            owner
        )
        .await
        .unwrap(),
        execution
    );
    let retained: i64 = s
        .read(|db| Ok(db.query_row("SELECT count(*) FROM host_transcript", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(retained, 12, "the native-activity message, five chunked rows, the next turn and the five question-turn chunks");
    close(h).await;
}

async fn configured_external_worker_grants() {
    for (role, stage, engine, expectation) in [
        ("worker-explorer", "execution", EngineKind::Codex, "read"),
        // Same server and role on Claude: the offer never depends on the engine.
        ("worker-explorer", "execution", EngineKind::Claude, "read"),
        (
            "worker-backend-dev",
            "execution",
            EngineKind::Codex,
            "write",
        ),
        (
            "worker-senior-consultant",
            "execution",
            EngineKind::Codex,
            "none",
        ),
        (
            "worker-code-reviewer",
            "review",
            EngineKind::Claude,
            "review-read",
        ),
        (
            "worker-code-reviewer",
            "review",
            EngineKind::Claude,
            "review-none",
        ),
    ] {
        let p = Project::new();
        let config = Project::new(); // Separate from the project; never owner configuration.
        let registry_dir = config.0.join("cortex");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&registry_dir)
            .unwrap();
        let exe = std::env::current_exe().unwrap();
        let path = registry_dir.join("mcp-servers.json");
        std::fs::write(
            &path,
            json!({"version":1,"servers":{"fixture":{
                "command":exe,"args":["external-fixture"],"allowed_tools":["lookup","write"],
                "read_only_tools":["lookup"],"coordinator_tools":["lookup"],
                "read_paths":[exe],"runtime_dir_env":["FIXTURE_RUNTIME"],"roles":["worker"],
                "env":{"FIXTURE_SECRET":"memory-only-fixture"},
                "enabled": expectation != "review-none"
            }}})
            .to_string(),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let registry = workspace_service::native_mcp::Registry::load(
            &workspace_service::native_routes::RouteEnvironment {
                home: config.0.clone(),
                config: config.0.clone(),
                state: config.0.clone(),
                routing_override: None,
            },
        )
        .unwrap();
        if expectation != "review-none" {
            registry
                .validate_client(&json!([{"name":"fixture","command":"/untrusted/override"}]))
                .unwrap();
        }
        assert!(registry
            .validate_client(&json!([{"name":"unconfigured","command":exe}]))
            .is_err());
        let key = p.key();
        let h = tokio::task::spawn_blocking(move || NativeHost::open_with_mcp(key, registry))
            .await
            .unwrap()
            .unwrap();
        let o = h
            .create("t".into(), "MCP fixture".into(), "owner".into())
            .await
            .unwrap();
        std::fs::create_dir(p.0.join("src")).unwrap();
        let store = p.store();
        store.write(|db|{
            let state = if stage == "review" { "REVIEWING" } else { "EXECUTING" };
            db.execute("UPDATE runtime_tasks SET state=?,revision=1 WHERE task_id='t'",[state])?;
            // `docs/styleguide.md` does not exist yet: it is withheld, never refused.
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Work','[]',?,?,'running','a',NULL)",[role,r#"["src","docs/styleguide.md"]"#,stage])?;
            Ok(())
        }).unwrap();
        let mut route = p.route();
        route.launch.engine = engine;
        route.launch.args = vec!["mcp-host".into()];
        let d = h
            .dispatch(
                "t".into(),
                o,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                route,
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), d.prompt(expectation))
                .await
                .unwrap_or_else(|_| panic!("fixture {role}/{expectation}: prompt did not finish; {}", std::fs::read_to_string(p.0.join("mcp-probe")).unwrap_or_default()))
                .unwrap_or_else(|e| panic!(
                    "fixture {role}: {e}; {}",
                    std::fs::read_to_string(p.0.join("mcp-probe")).unwrap_or_default()
                ))
                .stop_reason,
            "end_turn"
        );
        if stage == "review" {
            let boundary = std::fs::read_to_string(p.0.join("review-mcp-boundary")).unwrap();
            assert!(
                boundary.contains("mcp__cortex__tools_discover"),
                "review discovery must identify the Cortex-only tool by its Claude name"
            );
            assert!(boundary.contains("exact name advertised by the engine"));
            assert!(boundary.contains("If the external tool is not advertised"));
            assert!(!boundary.contains("mcp__fixture__"));
            assert!(!boundary.contains("# Cortex tool cheat sheet"));
            // A plain string replaces the Claude Code preset (claude-agent-acp 0.107.0
            // `sessionSystemPrompt`); the tool-calls-only rule opens and closes it.
            let prompt: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(p.0.join("review-system-prompt")).unwrap(),
            )
            .unwrap();
            let prompt = prompt.as_str().expect("systemPrompt is a replacing string");
            assert!(prompt.starts_with("# Output rule: tool calls only\n"));
            assert!(prompt.ends_with(
                "After the final tool call, end without text, or with one short sentence at most.\n"
            ));
            assert_eq!(prompt.matches("# Output rule: tool calls only").count(), 2);
            for preset in ["give short updates", "Brief is good", "English: progress,"] {
                assert!(!prompt.contains(preset), "{preset}");
            }
        }
        assert_eq!(
            p.0.join("src/external-write").exists(),
            expectation == "write"
        );
        assert!(!p.0.join("outside-scope").exists());
        assert!(!p.0.join("docs").exists());
        store.read(|db|{
            let withheld:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_EXTERNAL_MCP_WITHHELD'",[],|r|r.get(0))?;
            assert_eq!(withheld,0,"a not-yet-created scope withholds nothing");
            let offered:Vec<String>=db.prepare("SELECT payload_json FROM runtime_events WHERE type='NATIVE_EXTERNAL_MCP_OFFERED' ORDER BY id")?.query_map([],|r|r.get(0))?.collect::<Result<_,_>>()?;
            assert_eq!(offered.is_empty(),matches!(expectation,"none"|"review-none"),"offered is recorded exactly when a server is attached");
            for body in &offered{
                let v:serde_json::Value=serde_json::from_str(body).unwrap();
                assert_eq!(v["servers"][0]["server"],"fixture");
                assert!((1..=2).contains(&v["servers"][0]["tools"].as_u64().unwrap()));
                assert!(!body.contains("FIXTURE_SECRET")&&!body.contains("memory-only-fixture"));
            }
            let count:i64=db.query_row("SELECT count(*) FROM runtime_requests WHERE operation='external_call'",[],|r|r.get(0))?;
            assert_eq!(count,match expectation {"write"=>2,"read" | "review-read"=>1,_=>0});
            let saved:String=db.query_row("SELECT coalesce(group_concat(result_json),'') FROM runtime_requests WHERE operation='external_call'",[],|r|r.get(0))?;
            assert!(!saved.contains("FIXTURE_SECRET"));
            Ok(())
        }).unwrap();
        d.shutdown().await.unwrap();
        assert!(std::fs::read_dir(p.0.join(".cortex-dev"))
            .unwrap()
            .all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("external-runtime-")));
        close(h).await;
    }
}

/// A worker whose scope names a file it has not created yet, an optional server
/// refused for a protected descendant and one that exits during its own startup
/// checks: the session starts with only the usable server and records the rest.
async fn optional_external_failures_withhold_only_that_server() {
    let p = Project::new();
    let config = Project::new(); // Separate from the project; never owner configuration.
    let registry_dir = config.0.join("cortex");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&registry_dir)
        .unwrap();
    std::fs::create_dir_all(config.0.join("guarded/.ssh")).unwrap();
    std::fs::create_dir(config.0.join("exit-grant")).unwrap();
    let marker = config.0.join("exit-grant/exited");
    let exe = std::env::current_exe().unwrap();
    let path = registry_dir.join("mcp-servers.json");
    std::fs::write(
        &path,
        json!({"version":1,"servers":{
            "fixture":{"command":exe,"args":["external-fixture"],"allowed_tools":["lookup","write"],
                "read_only_tools":["lookup"],"read_paths":[exe],"runtime_dir_env":["FIXTURE_RUNTIME"],
                "roles":["worker"]},
            "guarded":{"command":exe,"args":["external-fixture"],"allowed_tools":["lookup"],
                "read_only_tools":["lookup"],"read_paths":[config.0.join("guarded")],"roles":["worker"]},
            "exits":{"command":exe,"args":["external-exit"],"allowed_tools":["lookup"],
                "read_only_tools":["lookup"],"read_paths":[exe],"write_paths":[config.0.join("exit-grant")],
                "env":{"EXIT_MARKER":marker},"roles":["worker"]}
        }})
        .to_string(),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let registry = workspace_service::native_mcp::Registry::load(
        &workspace_service::native_routes::RouteEnvironment {
            home: config.0.clone(),
            config: config.0.clone(),
            state: config.0.clone(),
            routing_override: None,
        },
    )
    .unwrap();
    let key = p.key();
    let h = tokio::task::spawn_blocking(move || NativeHost::open_with_mcp(key, registry))
        .await
        .unwrap()
        .unwrap();
    let o = h
        .create("t".into(), "MCP withholding".into(), "owner".into())
        .await
        .unwrap();
    std::fs::create_dir(p.0.join("src")).unwrap();
    std::fs::write(p.0.join("initialize-after"), marker.to_str().unwrap()).unwrap();
    let store = p.store();
    let root = p.0.to_str().unwrap().to_owned();
    store.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        // `docs/` does not exist: the worker is about to create docs/styleguide.md.
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]',?,'execution','running','a',NULL)",
            [r#"["src","docs/styleguide.md"]"#])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('host',?,'t','{\"name\":\"cortex-native\"}','{}','[]','2026-11-17T00:00:00Z',NULL)", [&root])?;
        Ok(())
    }).unwrap();
    let mut route = p.legacy_route();
    route.launch.args = vec!["mcp-host".into()];
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap_or_else(|e| {
            let why = store.read(|db| Ok(db.query_row("SELECT coalesce(group_concat(payload_json),'') FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED'",[],|r|r.get::<_,String>(0))?)).unwrap();
            panic!("optional external servers stopped the worker: {e:?}; {why}");
        });
    assert!(
        marker.exists(),
        "the exiting server ran inside its containment"
    );
    // The usable server is the only external entry and keeps its write grant.
    assert_eq!(d.prompt("write").await.unwrap().stop_reason, "end_turn");
    assert!(p.0.join("src/external-write").exists());
    assert!(
        !p.0.join("docs").exists(),
        "a missing scope is never created or widened"
    );
    store.read(|db|{
        let mut rows = db.prepare("SELECT json_extract(payload_json,'$.server'),json_extract(payload_json,'$.reason')
            FROM runtime_events WHERE task_id='t' AND type='NATIVE_EXTERNAL_MCP_WITHHELD' ORDER BY id")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        rows.sort();
        assert_eq!(rows, vec![
            ("exits".to_owned(),"server_exited_before_session".to_owned()),
            ("guarded".to_owned(),"protected_path_refused".to_owned()),
        ]);
        let failed:i64 = db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED'",[],|r|r.get(0))?;
        assert_eq!(failed, 0);
        let notes = db.prepare("SELECT update_json FROM host_updates WHERE session_id='host' ORDER BY seq")?
            .query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        for server in ["guarded","exits"] {
            assert!(notes.iter().any(|n| {
                let v:Value = serde_json::from_str(n).unwrap();
                v["status"]=="failed" && v["title"].as_str().is_some_and(|t|
                    t.contains(&format!("External MCP server {server} was not attached")))
            }), "{server}: {notes:?}");
        }
        Ok(())
    }).unwrap();
    d.shutdown().await.unwrap();
    assert!(std::fs::read_dir(p.0.join(".cortex-dev"))
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("external-runtime-")));
    close(h).await;
}

/// `"containment": "none"` as the registry default: the owner-configured server
/// runs without Landlock/seccomp, with the owner's HOME, PATH and runtime
/// directory (no private TMPDIR or runtime directory variables), still only
/// through the channel-bound relay and its tool grants. Its path grants are not
/// evaluated (a missing one does not withhold it) and it is stopped at session end.
async fn uncontained_external_server_runs_with_owner_authority() {
    // The owner opt-out is offered identically to both engines; one engine per process
    // (modes `uncontained-external-codex` and `uncontained-external-claude`).
    let claude = std::env::args().nth(1).as_deref() == Some("uncontained-external-claude");
    uncontained_on(if claude { EngineKind::Claude } else { EngineKind::Codex }).await;
}

async fn uncontained_on(engine: EngineKind) {
    let p = Project::new();
    let config = Project::new(); // Separate from the project; never owner configuration.
    let registry_dir = config.0.join("cortex");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&registry_dir)
        .unwrap();
    let exe = std::env::current_exe().unwrap();
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = |key: &str| {
        status
            .lines()
            .find(|l| l.starts_with(&format!("{key}:")))
            .unwrap()
            .to_owned()
    };
    let path = registry_dir.join("mcp-servers.json");
    std::fs::write(
        &path,
        json!({"version":1,"containment":"none","servers":{"fixture":{
            "command":exe,"args":["external-fixture"],"allowed_tools":["lookup","write"],
            "read_only_tools":["lookup"],"runtime_dir_env":["FIXTURE_RUNTIME"],"roles":["worker"],
            "read_paths":[config.0.join("not-created")],
            "env":{"EXPECT_UNCONTAINED_HOME":config.0,"EXPECT_NoNewPrivs":line("NoNewPrivs"),
                "EXPECT_Seccomp":line("Seccomp"),
                "EXPECT_XDG_RUNTIME_DIR":std::env::var("XDG_RUNTIME_DIR").unwrap()}
        }}})
        .to_string(),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let registry = workspace_service::native_mcp::Registry::load(
        &workspace_service::native_routes::RouteEnvironment {
            home: config.0.clone(),
            config: config.0.clone(),
            state: config.0.clone(),
            routing_override: None,
        },
    )
    .unwrap();
    let key = p.key();
    let h = tokio::task::spawn_blocking(move || NativeHost::open_with_mcp(key, registry))
        .await
        .unwrap()
        .unwrap();
    let o = h
        .create("t".into(), "Uncontained MCP".into(), "owner".into())
        .await
        .unwrap();
    std::fs::create_dir(p.0.join("src")).unwrap();
    let store = p.store();
    store.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]',?,'execution','running','a',NULL)",[r#"["src"]"#])?;
        Ok(())
    }).unwrap();
    let mut route = p.legacy_route();
    route.launch.engine = engine;
    route.launch.args = vec!["mcp-host".into()];
    let d = h
        .dispatch(
            "t".into(),
            o,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap();
    assert_eq!(
        d.prompt("write")
            .await
            .unwrap_or_else(|e| panic!(
                "uncontained fixture: {e}; {}",
                std::fs::read_to_string(p.0.join("mcp-probe")).unwrap_or_default()
            ))
            .stop_reason,
        "end_turn"
    );
    assert!(p.0.join("src/external-write").exists());
    assert_eq!(
        std::fs::read_to_string(p.0.join("outside-scope")).unwrap(),
        "owner authority",
        "no Landlock: the server wrote outside every grant"
    );
    println!(
        "uncontained external MCP: owner HOME/PATH, no Landlock/seccomp, relay-bound tool grants"
    );
    store
        .read(|db| {
            let withheld: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_EXTERNAL_MCP_WITHHELD'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(withheld, 0, "an unenforced missing grant withholds nothing");
            let offered: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_EXTERNAL_MCP_OFFERED' AND payload_json LIKE '%\"fixture\"%'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(offered, 1, "the uncontained server is offered on every engine");
            let count: i64 = db.query_row(
                "SELECT count(*) FROM runtime_requests WHERE operation='external_call'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                count, 2,
                "tool grants and replay still pass through the relay"
            );
            Ok(())
        })
        .unwrap();
    d.shutdown().await.unwrap();
    assert!(std::fs::read_dir(p.0.join(".cortex-dev"))
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("external-runtime-")));
    close(h).await;
}

fn external_fixture() {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let r: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let result = match r["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":[{"name":"lookup","description":"Look up entries in the fixture index.","inputSchema":{"type":"object","additionalProperties":false}},{"name":"write","inputSchema":{"type":"object"}}]})
            }
            "tools/call" => {
                let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap();
                assert!(std::env::var("CORTEX_CHANNEL_SECRET").is_err());
                if let Ok(home) = std::env::var("EXPECT_UNCONTAINED_HOME") {
                    // The service's own runtime directory; the private runtime
                    // directory variables and TMPDIR are containment-only.
                    assert_eq!(runtime, std::env::var("EXPECT_XDG_RUNTIME_DIR").unwrap());
                    assert!(std::env::var("FIXTURE_RUNTIME").is_err());
                    assert!(std::env::var("TMPDIR").is_err());
                    // The owner's explicit opt-out: owner HOME and PATH, no
                    // no_new_privs/seccomp beyond the parent's, no Landlock, network.
                    assert_eq!(std::env::var("HOME").unwrap(), home);
                    let path = std::env::var("PATH").unwrap();
                    let exe = std::env::current_exe().unwrap();
                    let dirs: Vec<_> = path.split(':').collect();
                    assert_eq!(dirs[0], exe.parent().unwrap().to_str().unwrap());
                    assert!(dirs.contains(&format!("{home}/.local/bin").as_str()));
                    let status = std::fs::read_to_string("/proc/self/status").unwrap();
                    for key in ["NoNewPrivs", "Seccomp"] {
                        let line = status
                            .lines()
                            .find(|l| l.starts_with(&format!("{key}:")))
                            .unwrap();
                        assert_eq!(line, std::env::var(format!("EXPECT_{key}")).unwrap());
                    }
                    std::fs::write("outside-scope", "owner authority").unwrap();
                    assert!(std::fs::read(".cortex-dev/cortex.sqlite3").is_ok());
                    std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                } else {
                    assert_eq!(std::env::var("FIXTURE_RUNTIME").unwrap(), runtime);
                    assert!(std::fs::read(".cortex-dev/cortex.sqlite3").is_err());
                    assert!(std::fs::write("outside-scope", "denied").is_err());
                    // The network stays shared with the host (Landlock contains files only).
                    assert!(std::net::TcpListener::bind("127.0.0.1:0").is_ok());
                }
                let count = std::fs::read_to_string(format!("{runtime}/count"))
                    .unwrap_or_default()
                    .parse::<usize>()
                    .unwrap_or(0)
                    + 1;
                std::fs::write(format!("{runtime}/count"), count.to_string()).unwrap();
                if r["params"]["name"] == "write" {
                    std::fs::write("src/external-write", "ok").unwrap();
                }
                json!({"count":count})
            }
            _ => json!({}),
        };
        writeln!(
            stdout,
            "{}",
            json!({"jsonrpc":"2.0","id":r["id"],"result":result})
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}
/// Live D6 regression: the route's `thought_level` must reach each engine under
/// its own advertised option id, or the coordinator never starts (configure).
async fn route_effort_starts_codex_and_claude_coordinators() {
    for engine in [EngineKind::Codex, EngineKind::Claude] {
        let p = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        let mut route = p.route();
        route.launch.engine = engine;
        route
            .parameters
            .insert("thought_level".into(), "high".into());
        let d = h
            .dispatch("t".into(), o, SessionRole::Coordinator, route)
            .await
            .unwrap_or_else(|e| {
                let why = s.read(|db| Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' ORDER BY id DESC LIMIT 1",[],|r|r.get::<_,String>(0))?)).unwrap();
                panic!("{engine:?} effort configure: {e:?}; {why}");
            });
        drop(d);
        drop(s);
        close(h).await;
    }
}
/// A reopened no-change task can carry a renamed unresolved check. Permission
/// approval is not submission success; the gateway must explain the exact gap.
async fn reopened_synthesis_feedback() -> bool {
    for engine in [EngineKind::Claude, EngineKind::Codex] {
        let p = Project::new();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&p.0)
            .status()
            .unwrap()
            .success());
        std::fs::write(p.0.join("work.txt"), "unchanged").unwrap();
        let h = p.host().await;
        let o = h
            .create("retry".into(), "Recheck fixture".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        let route = || {
            let mut r = p.route();
            r.launch.engine = engine;
            r
        };
        if !verification_available(&p.0) {
            // Match the other native journeys: assert the production refusal,
            // but never present this negative path as positive reconciliation.
            let d = h
                .dispatch("retry".into(), o.clone(), SessionRole::Coordinator, route())
                .await
                .unwrap();
            assert_eq!(d.prompt(&call("plan_submit", json!({
                "request_key":"unavailable",
                "plan":{"tasks":[{"id":"recheck","role":"qa-engineer","objective":"Check work.txt",
                    "dependencies":[],"scope":["work.txt"],"model":"auto"}],
                    "verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]}
            }))).await.unwrap().stop_reason, "end_turn");
            assert_eq!(h.state("retry".into()).await.unwrap(), TaskState::Planning);
            s.read(|db| {
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='retry' AND type='PLAN_ACCEPTED'",
                    [], |r| r.get::<_,i64>(0))?, 0);
                Ok(())
            }).unwrap();
            d.shutdown().await.unwrap();
            drop(s);
            close(h).await;
            println!("namespace-unavailable plan refusal asserted; positive synthesis feedback journey NOT RUN");
            return false;
        }
        let old = "pytest run (assigned verification command)";
        let current = "pytest run (original assigned verification command)";
        for revision in 1..=2 {
            if revision == 2 {
                std::fs::write(p.0.join("work.txt"), "changed between turns").unwrap();
                s.write(|db| {
                    assert!(domain::reopen_tx(db, "retry")?);
                    domain::settle_recovery_tx(db, "retry", TaskState::Planning)?;
                    cortex_core::lifecycle::event_tx(
                        db,
                        "retry",
                        "USER_STEERING",
                        json!({"request":"follow-up","text":"Recheck the current file.","delivery":"boundary"}),
                    )?;
                    assert!(cortex_core::lifecycle::steering::apply_pending_tx(db, "retry")?);
                    Ok(())
                })
                .unwrap();
            }
            let d = h
                .dispatch("retry".into(), o.clone(), SessionRole::Coordinator, route())
                .await
                .unwrap();
            if revision == 2 {
                s.write(|db| {
                    assert_eq!(cortex_core::lifecycle::steering::deliver_tx(db, "retry")?.len(), 1);
                    Ok(())
                }).unwrap();
                let stale_answer = json!({"name":"answer_submit","arguments":{
                    "text":"The previous check still applies.","request_key":"stale-answer",
                    "not_verified":[{"item":old,"blocker":"pytest is not installed"}]
                },"__fixture_expect":{
                    "/result/isError":true,
                    "/result/structuredContent/code":"answer_project_revision_changed",
                    "/result/structuredContent/recorded":false
                }});
                assert_eq!(
                    d.prompt(&format!("call:{stale_answer}")).await.unwrap().stop_reason,
                    "end_turn"
                );
                assert_eq!(h.state("retry".into()).await.unwrap(), TaskState::Planning);
                s.write(|db| {
                    cortex_core::lifecycle::steering::confirm_tx(db, "retry")?;
                    Ok(())
                }).unwrap();
            }
            // Permission and a valid schema cannot bypass the task-state gate; the refusal
            // is a structured tool error naming synthesis_not_expected.
            assert_eq!(
                d.prompt(&call("synthesis_submit", json!({"text":"Too early."})))
                    .await
                    .unwrap()
                    .stop_reason,
                "end_turn"
            );
            let mut plan = json!({"tasks":[{"id":"recheck","role":"qa-engineer","objective":"Check unchanged work.txt",
                "dependencies":[],"scope":["work.txt"],"model":"auto"}],
                "verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
            if revision == 2 {
                // The continuation's coordinator sees the outcomes of the plan in force
                // verbatim, a plan that drops one is answered with the exact statement to
                // carry, and a plan that names it by id alone is accepted.
                let statement = "The requested change is made";
                d.prompt(&format!("call:{}", json!({"name":"task_get_context","arguments":{},
                    "__fixture_expect":{"/result/structuredContent/plan_summary/outcomes/0/id":"goal",
                        "/result/structuredContent/plan_summary/outcomes/0/statement":statement}})))
                    .await.unwrap();
                // A batched context read larger than one batch item is deferred, and the
                // deferred item still carries the outcomes inline (live: the coordinator
                // only got batch_result_limit and could not replan).
                d.prompt(&format!("call:{}", json!({"name":"read_batch",
                    "arguments":{"calls":[{"id":"capsule","tool":"read_capsule","arguments":{}},
                        {"id":"ctx","tool":"task_get_context","arguments":{}}]},
                    "__fixture_expect":{"/result/structuredContent/results/0/status":"deferred",
                        "/result/structuredContent/results/0/plan_outcomes/0/id":"goal",
                        "/result/structuredContent/results/0/plan_outcomes/0/statement":statement,
                        // A repeated, unchanged context read is compact and keeps them too.
                        "/result/structuredContent/results/1/result/unchanged_since_last_read":true,
                        "/result/structuredContent/results/1/result/plan_outcomes/0/statement":statement}})))
                    .await.unwrap();
                let mut dropped = plan.clone();
                dropped["outcomes"] = json!([{"id":"recheck","statement":"The file is rechecked"}]);
                dropped["tasks"][0]["delivers"] = json!(["recheck"]);
                d.prompt(&format!("call:{}", json!({"name":"plan_submit",
                    "arguments":{"request_key":"plan-dropped","plan":dropped},
                    "__fixture_expect":{"/result/isError":true,
                        "/result/structuredContent/code":"plan_outcome_dropped",
                        "/result/structuredContent/recorded":false,
                        "/result/structuredContent/missing_outcomes/0/id":"goal",
                        "/result/structuredContent/missing_outcomes/0/statement":statement}})))
                    .await.unwrap();
                // A plan whose delivers names an undefined id is answered with that id.
                let mut ghost = plan.clone();
                ghost["outcomes"] = json!([{"id":"goal"}]);
                ghost["tasks"][0]["delivers"] = json!(["goal", "ghost"]);
                d.prompt(&format!("call:{}", json!({"name":"plan_submit",
                    "arguments":{"request_key":"plan-ghost","plan":ghost},
                    "__fixture_expect":{"/result/isError":true,
                        "/result/structuredContent/code":"plan_outcome_unknown",
                        "/result/structuredContent/recorded":false,
                        "/result/structuredContent/unknown_delivers/0/id":"ghost",
                        "/result/structuredContent/unknown_delivers/0/task":"recheck",
                        "/result/structuredContent/defined_outcome_ids/0":"goal"}})))
                    .await.unwrap();
                assert_eq!(h.state("retry".into()).await.unwrap(), TaskState::Planning);
                plan["outcomes"] = json!([{"id":"goal"}]);
                plan["tasks"][0]["delivers"] = json!(["goal"]);
            }
            d.prompt(&call("plan_submit", json!({
                "request_key":format!("plan-{revision}"),
                "plan":plan
            }))).await.unwrap();
            if revision == 2 {
                let kept = s.read(|db| cortex_core::lifecycle::plan::task_outcomes(db, "retry")).unwrap().unwrap();
                assert_eq!(kept, vec![json!({"id":"goal","statement":"The requested change is made"})]);
            }
            d.shutdown().await.unwrap();
            let step = h.advance("retry".into(), o.clone()).await.unwrap();
            assert_eq!(step["state"], "EXECUTING");
            let assignment = step["assignments"][0]["assignment_ref"].as_str().unwrap();
            let worker = h
                .dispatch(
                    "retry".into(),
                    o.clone(),
                    SessionRole::Worker {
                        assignment: assignment.into(),
                    },
                    route(),
                )
                .await
                .unwrap();
            // Worker channels must not receive coordinator correction details.
            assert_eq!(
                worker
                    .prompt(&call(
                        "synthesis_submit",
                        json!({"text":"Not a coordinator."})
                    ))
                    .await
                    .unwrap()
                    .stop_reason,
                "refusal"
            );
            let gap = if revision == 1 { old } else { current };
            submit_result(
                &h,
                &worker,
                "retry",
                &o,
                assignment,
                json!({
                    "report":"# Recheck\n\nNo project changes. Optional pytest unavailable.",
                    "summary":"Rechecked unchanged fixture.",
                    "checks":[{"name":gap,"outcome":"not_run","blocker":"pytest is not installed"}],
                    "request_key":"recheck"
                }),
            )
            .await;
            worker.shutdown().await.unwrap();
            assert_eq!(
                h.advance("retry".into(), o.clone()).await.unwrap()["state"],
                "RECONCILING"
            );
            let d = h
                .dispatch("retry".into(), o.clone(), SessionRole::Coordinator, route())
                .await
                .unwrap();
            if engine == EngineKind::Claude {
                let request = json!({"toolCall":{"toolCallId":"synthesis","kind":"other",
                    "_meta":{"claudeCode":{"toolName":"mcp__cortex__synthesis_submit"}}},
                    "options":[{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
                        {"optionId":"reject","name":"No","kind":"reject_once"}]});
                d.prompt(&format!("permission-request:{request}"))
                    .await
                    .unwrap();
                s.read(|db| {
                    let raw: String = db.query_row(
                        "SELECT payload_json FROM runtime_events WHERE task_id='retry' AND type IN ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_AUTO_ALLOWED') ORDER BY id DESC LIMIT 1",
                        [], |r| r.get(0))?;
                    assert_eq!(serde_json::from_str::<Value>(&raw)?["outcome"]["optionId"], "allow-once");
                    Ok(())
                }).unwrap();
            }
            if revision == 2 {
                // The old name is valid JSON/schema but does not disclose this revision's gap.
                let stale = json!({"text":"Rechecked the fixture.",
                    "not_verified":[{"item":old,"blocker":"pytest is not installed"}]});
                s.write(|db| {
                    assert!(matches!(
                        cortex_core::lifecycle::reconciliation::submit_synthesis_tx(
                            db, "retry", &stale
                        ),
                        Err(cortex_core::CoreError::Refused(
                            "synthesis_gaps_undisclosed"
                        ))
                    ));
                    Ok(())
                })
                .unwrap();
                assert_eq!(
                    d.prompt(&call("synthesis_submit", stale.clone())).await.unwrap().stop_reason,
                    "end_turn",
                    "correctable synthesis gaps must be MCP tool feedback, not an authority RPC refusal"
                );
                for _ in 0..2 {
                    d.prompt(&format!(
                        "call:{}",
                        json!({"name":"synthesis_submit","arguments":stale,
                            "__fixture_expect":{"/result/isError":true,
                                "/result/structuredContent/code":"synthesis_gaps_undisclosed",
                                "/result/structuredContent/required_not_verified":[current],
                                "/result/structuredContent/recorded":false}
                        })
                    ))
                    .await
                    .unwrap();
                }
                // A duplicate authored disclosure is a different correction, not an access refusal.
                d.prompt(&format!(
                    "call:{}",
                    json!({"name":"synthesis_submit",
                        "arguments":{"text":"Rechecked.\n\n## Not verified\n\npytest",
                            "not_verified":[{"item":current,"blocker":"pytest is not installed"}]},
                        "__fixture_expect":{"/result/isError":true,
                            "/result/structuredContent/code":"synthesis_disclosure_in_text",
                            "/result/structuredContent/recorded":false}
                    })
                ))
                .await
                .unwrap();
                s.read(|db| {
                    assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='retry' AND type='SYNTHESIS_SUBMITTED'",
                        [], |r| r.get::<_,i64>(0))?, 1);
                    assert!(db.query_row("SELECT synthesis FROM runtime_tasks WHERE task_id='retry'",
                        [], |r| r.get::<_,Option<String>>(0))?.is_none());
                    Ok(())
                }).unwrap();
            }
            if revision == 2 {
                // An unmet goal outcome is never synthesized: the refusal records nothing
                // and tells the coordinator to continue the work instead.
                let continue_now = "Nothing was recorded: the original goal is not met, so the task cannot finish. Do not synthesize unfinished work. Continue now: call replan_request and then submit a plan whose tasks deliver every outcome that is not done (keep every outcome and the unresolved verification commands), or, when only the owner can decide (for example whether to close the task as is), ask with user_question_request.";
                for (arguments, code) in [
                    (json!({"text":"Rechecked, but the goal is not met.",
                        "not_verified":[{"item":gap,"blocker":"pytest is not installed"}],
                        "outcomes":[{"id":"goal","status":"not_done"}]}), "completion_goal_not_met"),
                    (json!({"text":"Rechecked.",
                        "not_verified":[{"item":gap,"blocker":"pytest is not installed"},
                            {"item":"Push the branch","blocker":"not done"}]}), "not_verified_not_a_check"),
                ] {
                    let mut expect = json!({"/result/isError":true,
                        "/result/structuredContent/code":code,
                        "/result/structuredContent/recorded":false});
                    if code == "completion_goal_not_met" {
                        expect["/result/structuredContent/message"] = json!(continue_now);
                    }
                    d.prompt(&format!(
                        "call:{}",
                        json!({"name":"synthesis_submit","arguments":arguments,"__fixture_expect":expect})
                    ))
                    .await
                    .unwrap();
                }
                s.read(|db| {
                    assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='retry' AND type='SYNTHESIS_SUBMITTED'",
                        [], |r| r.get::<_,i64>(0))?, 1);
                    Ok(())
                }).unwrap();
            }
            let corrected = json!({"name":"synthesis_submit","arguments":{
                "text":"Rechecked the fixture.",
                "not_verified":[{"item":gap,"blocker":"pytest is not installed"}]},
                "__fixture_expect":{"/result/isError":false,"/result/structuredContent/recorded":true}});
            // An acknowledged identical retry is idempotent.
            for _ in 0..2 {
                d.prompt(&format!("call:{corrected}")).await.unwrap();
            }
            d.shutdown().await.unwrap();
            assert_eq!(
                h.advance("retry".into(), o.clone()).await.unwrap()["state"],
                "COMPLETED"
            );
            s.read(|db| {
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='retry' AND type='SYNTHESIS_SUBMITTED'",
                    [], |r| r.get::<_,i64>(0))?, revision);
                let text: String = db.query_row("SELECT synthesis FROM runtime_tasks WHERE task_id='retry'", [], |r| r.get(0))?;
                assert_eq!(text.matches("## Not verified").count(), 1);
                assert!(text.contains(gap));
                Ok(())
            }).unwrap();
        }
        drop(s);
        close(h).await;
    }
    true
}

/// The engine receives readable blocks for retained attachments, shaped by its
/// advertised promptCapabilities, and each attachment only once per session.
async fn attachments_reach_the_engine_as_readable_blocks() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    std::fs::write(p.0.join("record-prompts"), "").unwrap();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Goal".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    let dir = p.0.join(".cortex-dev/t");
    std::fs::create_dir_all(dir.join(".source_blocks")).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = "r_333333333333";
    let body = "Owner text";
    std::fs::write(dir.join(format!("{source}.md")), body).unwrap();
    std::fs::set_permissions(
        dir.join(format!("{source}.md")),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    // kind, mime, uri, bytes
    let items: [(&str, Option<&str>, Option<&str>, &[u8]); 7] = [
        (
            "resource",
            Some("text/plain"),
            Some("file:///notes.txt"),
            b"plain notes",
        ),
        ("image", Some("image/png"), None, b"\x89PNG\r\n\x1a\n"),
        ("image", Some("image/svg+xml"), None, b"<svg/>"),
        (
            "resource",
            Some("application/pdf"),
            Some("file:///doc.pdf"),
            b"%PDF-1.4 \xff\xfe",
        ),
        (
            "resource",
            Some("application/zip"),
            Some("file:///a.zip"),
            b"PK\x03\x04\x00\x00",
        ),
        (
            "resource_link",
            None,
            Some("file:///linked.md"),
            b"# linked text",
        ),
        (
            "resource",
            Some("application/json"),
            Some("file:///d.json"),
            b"{\"a\":1}",
        ),
    ];
    s.write(|db| {
        db.execute("INSERT INTO reports VALUES (?,'t','source',?,'fixture')", (source, format!("{source}.md")))?;
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,'Input','Input','user','fixture',?,?)",
            rusqlite::params![source, format!("{:x}", Sha256::digest(body.as_bytes())), body.len() as i64])?;
        db.execute("INSERT INTO source_revisions VALUES ('t',1,'client','message',?,'[]')", [source])?;
        for (index, (kind, mime, uri, bytes)) in items.iter().enumerate() {
            let name = format!("body{index}.bin");
            let path = dir.join(".source_blocks").join(&name);
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            db.execute("INSERT INTO source_blocks VALUES ('t',?,?,?,?,NULL,?,?,?,'stored',NULL,'user_direct',?,NULL,'fixture')",
                rusqlite::params![source, index as i64, kind, mime, uri, format!("{:x}", Sha256::digest(bytes)), bytes.len() as i64, name])?;
        }
        Ok(())
    }).unwrap();
    let log = || -> Vec<Value> {
        std::fs::read_to_string(p.0.join("prompt-log.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["prompt"].clone())
            .collect()
    };
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    d.prompt("first").await.unwrap();
    d.prompt("second").await.unwrap();
    let all = log();
    let first = all[0].as_array().unwrap();
    let second = all[1].as_array().unwrap();
    let text_of = |b: &Value| b["text"].as_str().unwrap_or("").to_owned();
    assert!(first.iter().any(|b| b["type"] == "resource"
        && b["resource"]["uri"] == "file:///notes.txt"
        && b["resource"]["text"] == "plain notes"));
    assert!(first.iter().any(|b| b["type"] == "resource"
        && b["resource"]["uri"] == "file:///linked.md"
        && b["resource"]["text"] == "# linked text"));
    assert!(first
        .iter()
        .any(|b| b["type"] == "resource" && b["resource"]["mimeType"] == "application/json"));
    assert!(first
        .iter()
        .any(|b| b["type"] == "image" && b["mimeType"] == "image/png"));
    assert!(
        first
            .iter()
            .any(|b| text_of(b).contains("not readable") && text_of(b).contains("doc.pdf")),
        "codex PDF marker: {first:?}"
    );
    assert!(first
        .iter()
        .any(|b| text_of(b).contains("unsupported image")));
    assert!(first
        .iter()
        .any(|b| text_of(b).contains("a.zip") && text_of(b).contains("sha256")));
    assert!(!first
        .iter()
        .any(|b| b["resource"]["blob"].is_string() || b.to_string().contains("UEsDB")));
    // Later prompts reference by marker; no body is re-sent.
    assert_eq!(second.len(), 2, "{second:?}");
    assert!(
        text_of(&second[1]).contains("already provided")
            && text_of(&second[1]).contains("notes.txt")
    );
    assert!(!second
        .iter()
        .any(|b| b["type"] == "image" || b["type"] == "resource"));
    d.shutdown().await.unwrap();
    // An engine that advertises no promptCapabilities gets text and markers only.
    std::fs::write(p.0.join("prompt-caps.txt"), "none").unwrap();
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    d.prompt("third").await.unwrap();
    let third = log().pop().unwrap();
    let third = third.as_array().unwrap();
    assert!(
        !third
            .iter()
            .any(|b| b["type"] == "image" || b["type"] == "resource"),
        "{third:?}"
    );
    assert!(third
        .iter()
        .any(|b| text_of(b).contains("plain notes") && text_of(b).contains("notes.txt")));
    assert!(third
        .iter()
        .any(|b| text_of(b).contains("does not accept image input")));
    d.shutdown().await.unwrap();
    close(h).await;
}
/// A project change observed while the verification commands run must never become a
/// pass: the review no longer describes the verified revision.
async fn verification_drift_blocks_the_pass() {
    let p = Project::new();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&p.0)
        .status()
        .unwrap()
        .success());
    std::fs::write(p.0.join("work.txt"), "before").unwrap();
    if !verification_available(&p.0) {
        println!("native verification unavailable: drift scenario not run");
        return;
    }
    let h = p.host().await;
    let o = h
        .create("drift".into(), "Implement fixture".into(), "owner".into())
        .await
        .unwrap();
    let coordinator = h
        .dispatch(
            "drift".into(),
            o.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Change work.txt","dependencies":[],"scope":["work.txt"],"model":"auto"}],
        "verification":[[std::env::current_exe().unwrap().to_str().unwrap(),"sleep-ms","2000"]]});
    coordinator
        .prompt(&call(
            "plan_submit",
            json!({"plan":plan,"request_key":"plan"}),
        ))
        .await
        .unwrap();
    coordinator.shutdown().await.unwrap();
    let step = h.advance("drift".into(), o.clone()).await.unwrap();
    let a = step["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let worker = h
        .dispatch(
            "drift".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: a.clone(),
            },
            p.native_writer_route(),
        )
        .await
        .unwrap();
    // The worker's engine edits the file natively, inside its turn and without a
    // Cortex tool: the turn's change record attributes it to the task.
    let edit = worker
        .prompt(r#"native-write:{"path":"work.txt","content":"after"}"#)
        .await
        .unwrap();
    assert_eq!(edit.stop_reason, "end_turn");
    assert_eq!(
        std::fs::read_to_string(p.0.join("work.txt")).unwrap(),
        "after"
    );
    submit_result(&h, &worker, "drift", &o, &a, json!({"report":"# Work\n\nChanged.","summary":"Changed the fixture file.","checks":[],"request_key":"result"})).await;
    worker.shutdown().await.unwrap();
    let step = h.advance("drift".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "REVIEWING");
    let r = step["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let review = h
        .dispatch(
            "drift".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: r.clone(),
            },
            p.route(),
        )
        .await
        .unwrap();
    review_reads(&p, &review, "drift", &r).await;
    submit_result(&h, &review, "drift", &o, &r, json!({"report":"# Review\n\nLooks right.","summary":"Approved.","checks":[],"verdict":"APPROVE","request_key":"review"})).await;
    review.shutdown().await.unwrap();
    let root = p.0.clone();
    let observer = p.store();
    let writer = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !observer.read(|db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id='drift' AND type='NATIVE_COMMAND_STARTED' AND json_extract(payload_json,'$.revision') IS NOT NULL)", [], |r| r.get::<_,bool>(0))?)).unwrap() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("verifier did not start");
        std::fs::write(root.join("work.txt"), "changed while verifying").unwrap();
    });
    let fresh = h.advance("drift".into(), o.clone()).await.unwrap();
    writer.await.unwrap();
    assert_eq!(fresh["state"], "REVIEWING", "{fresh}");
    assert_eq!(fresh["assignments"][0]["stage"], "review", "{fresh}");
    p.store()
        .read(|db| {
            let passed: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_PASSED'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(passed, 0);
            let settled_outputs: i64 = db.query_row(
                "SELECT count(*) FROM runtime_requests
                 WHERE operation='native_command_output' AND json_extract(result_json,'$.observation.complete')=1 AND json_extract(result_json,'$.observation.exit_code')=0",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(settled_outputs, 1);
            Ok(())
        })
        .unwrap();
    let fresh_review_ref = fresh["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let fresh_review = h
        .dispatch(
            "drift".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: fresh_review_ref.clone(),
            },
            p.route(),
        )
        .await
        .unwrap();
    review_reads(&p, &fresh_review, "drift", &fresh_review_ref).await;
    submit_result(
        &h,
        &fresh_review,
        "drift",
        &o,
        &fresh_review_ref,
        json!({
            "report":"# Fresh review\n\nThe changed checkout was reviewed again.",
            "summary":"Fresh review approves the current checkout.",
            "checks":[],
            "verdict":"APPROVE",
            "findings":[],
            "request_key":"fresh-review"
        }),
    )
    .await;
    fresh_review.shutdown().await.unwrap();
    let recheck = h
        .advance_step("drift".into(), o.clone(), true)
        .await
        .unwrap();
    assert_eq!(recheck["recheck"], true, "{recheck}");
    p.store()
        .read(|db| {
            let passed: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_PASSED'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(passed, 1);
            let state: String = db.query_row(
                "SELECT state FROM runtime_tasks WHERE task_id='drift'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(state, "RECONCILING");
            let changed: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISION_CHANGED'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(changed, 1);
            let approved: i64 = db.query_row(
                "SELECT count(*) FROM runtime_nodes WHERE stage='review' AND status='completed'
                    AND json_extract(result_json,'$.verdict')='APPROVE'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(approved, 2);
            let outputs: i64 = db.query_row(
                "SELECT count(*) FROM runtime_requests
                 WHERE operation='native_command_output' AND json_extract(result_json,'$.observation.complete')=1 AND json_extract(result_json,'$.observation.exit_code')=0",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(outputs, 2);
            let reports: i64 = db.query_row(
                "SELECT count(DISTINCT json_extract(result_json,'$.report_ref'))
                 FROM runtime_nodes WHERE stage='review' AND status='completed'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(reports, 2);
            let unsettled: i64 = db.query_row(
                "SELECT count(*) FROM runtime_requests
                 WHERE operation='native_verification_intent'
                   AND NOT EXISTS (SELECT 1 FROM runtime_requests finish
                     WHERE finish.task_id=runtime_requests.task_id
                       AND finish.actor='native-verifier-finish'
                       AND finish.request_key=runtime_requests.request_key)",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(unsettled, 0);
            Ok(())
        })
        .unwrap();
    close(h).await;
}

/// Recovery reads, preflight, directory/delete/skill-link effects and the
/// structured tool errors, all through the real gateway with the fake engine.
async fn recovery_project_operations_and_structured_tool_errors() {
    async fn checked(
        d: &workspace_service::native_sessions::SessionDriver,
        name: &str,
        args: Value,
        expected: Value,
        stop: &str,
    ) {
        let target = args["request_key"].as_str().or_else(|| args["path"].as_str()).unwrap_or("-");
        let prompt = format!(
            "call:{}",
            json!({"name":name,"arguments":args,"__fixture_expect":expected})
        );
        let turn = tokio::time::timeout(Duration::from_secs(15), d.prompt(&prompt))
            .await
            .unwrap_or_else(|_| panic!("{name} fixture action {target}: no reply"))
            .unwrap_or_else(|e| panic!("{name} fixture action {target}: {e:?}"));
        assert_eq!(turn.stop_reason, stop, "{name}");
    }
    let p = Project::new();
    let h = p.host().await;
    let o = h
        .create("t".into(), "Recovery fixture".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    let d = h
        .dispatch("t".into(), o.clone(), SessionRole::Coordinator, p.route())
        .await
        .unwrap();
    checked(&d, "task_get_context", json!({}), json!({"/result/isError":false,
        "/result/structuredContent/task":"t","/result/structuredContent/original_goal":"Recovery fixture",
        "/result/structuredContent/state":"PLANNING"}), "end_turn").await;
    // An identical repeat read costs a compact unchanged receipt.
    checked(
        &d,
        "task_get_context",
        json!({}),
        json!({
        "/result/structuredContent/unchanged_since_last_read":true}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "read_capsule",
        json!({}),
        json!({"/result/isError":false,
        "/result/structuredContent/recipient/role":"coordinator",
        "/result/structuredContent/original_goal":"Recovery fixture"}),
        "end_turn",
    )
    .await;
    checked(
        &d,
        "read_capsule",
        json!({"at_revision":5}),
        json!({"/result/isError":true,
        "/result/structuredContent/code":"at_revision_out_of_range",
        "/result/structuredContent/field":"at_revision"}),
        "end_turn",
    )
    .await;
    // read_report without report_id or cursor names the missing pipeline, not a refusal.
    checked(
        &d,
        "read_report",
        json!({}),
        json!({"/result/isError":true,
        "/result/structuredContent/code":"pipeline_missing"}),
        "end_turn",
    )
    .await;
    // An absent bare name has no candidates to probe. It is reported missing
    // even when the read-only sandbox cannot launch.
    checked(&d, "check_preflight", json!({"argv":["cortex-fixture-missing-program","--version"]}),
        json!({"/result/isError":false,"/result/structuredContent/available":false,
            "/result/structuredContent/reason":"program_missing","/result/structuredContent/evidence":false}),
        "end_turn").await;
    checked(&d, "check_preflight", json!({"argv":["cargo","test","|","cat"]}), json!({"/result/isError":true,
        "/result/structuredContent/code":"verification_shell_syntax","/result/structuredContent/field":"argv"}),
        "end_turn").await;
    // The runtime's initial pipeline edition (published at the driver boundary before
    // the first coordinator turn) is the default read_report document.
    let pipeline = cortex_core::pipeline::sync(&s, "t", None).unwrap().unwrap();
    checked(&d, "read_report", json!({}), json!({"/result/isError":false,"/result/structuredContent/report_id":pipeline,
        "/result/structuredContent/kind":"pipeline","/result/structuredContent/title":"Initial pipeline",
        "/result/structuredContent/source_attachments":[]}), "end_turn").await;
    // A model draft of the pipeline stays refused: the template is not offered.
    checked(
        &d,
        "create_draft",
        json!({"template":"pipeline","request_key":"own-pipeline"}),
        json!({
        "/error/code":-32602,"/error/data/code":"invalid_arguments"}),
        "refusal",
    )
    .await;
    // read_batch: evidence reads run in order through the same handlers as direct calls;
    // one failing item leaves the others and makes the batch result incomplete.
    let direct = cortex_core::store::documents::Publisher::new(&s)
        .read_report("t", &pipeline, None, 300)
        .unwrap();
    checked(&d, "read_batch", json!({"calls":[
        {"id":"list","tool":"list_reports","arguments":{"limit":5}},
        {"id":"pipe","tool":"read_report","arguments":{"report_id":pipeline,"limit":300}},
        {"id":"bad","tool":"read_report","arguments":{"report_id":"r_000000000000"}},
        {"id":"src","tool":"list_sources","arguments":{}}]}),
        json!({"/result/isError":true,"/result/structuredContent/outcome":"incomplete",
            "/result/structuredContent/executed_count":4,"/result/structuredContent/returned_count":3,
            "/result/structuredContent/results/0/id":"list","/result/structuredContent/results/0/status":"ok",
            "/result/structuredContent/results/0/result/reports/0/report_id":pipeline,
            "/result/structuredContent/results/1/status":"ok",
            "/result/structuredContent/results/1/result/markdown":direct.markdown,
            "/result/structuredContent/results/1/result/sha256":direct.sha256,
            "/result/structuredContent/results/1/result/next_cursor":direct.next_cursor,
            "/result/structuredContent/results/2/status":"error",
            "/result/structuredContent/results/2/error/code":"not_found",
            "/result/structuredContent/results/3/status":"ok"}), "end_turn").await;
    // A write, a decision, a probe and a recursive batch are not batchable: the advertised
    // item enum refuses them before anything runs, naming the exact field and constraint.
    for tool in [
        "project_write",
        "plan_submit",
        "check_preflight",
        "read_batch",
    ] {
        checked(
            &d,
            "read_batch",
            json!({"calls":[{"id":"x","tool":tool,"arguments":{}}]}),
            json!({"/error/code":-32602,"/error/data/code":"invalid_arguments",
                "/error/data/field":"arguments.calls[0].tool","/error/data/reason":"invalid_enum"}),
            "refusal",
        )
        .await;
    }
    // Newly batchable reads run through their own handlers. A private path is refused
    // inside the batch (a direct project_read of it is decided by the runtime and
    // asks the owner outside Auto); the other items still deliver.
    std::fs::write(p.0.join("notes.txt"), "batched line\n").unwrap();
    checked(
        &d,
        "read_batch",
        json!({"calls":[
        {"id":"file","tool":"project_read","arguments":{"path":"notes.txt"}},
        {"id":"dir","tool":"project_read","arguments":{"path":"."}},
        {"id":"private","tool":"project_read","arguments":{"path":".cortex/cortex.sqlite3"}},
        {"id":"skill","tool":"read_skill","arguments":{"skill":"no-such-skill"}},
        {"id":"ctx","tool":"task_get_context","arguments":{}},
        {"id":"tool","tool":"tools_discover","arguments":{"name":"read_report"}}]}),
        json!({"/result/isError":true,"/result/structuredContent/outcome":"incomplete",
            "/result/structuredContent/executed_count":6,
            "/result/structuredContent/results/0/status":"ok",
            "/result/structuredContent/results/0/result/content":"1\tbatched line\n",
            "/result/structuredContent/results/1/status":"ok",
            "/result/structuredContent/results/2/status":"error",
            "/result/structuredContent/results/2/error/code":"private_or_invalid_path",
            "/result/structuredContent/results/3/status":"error"}),
        "end_turn",
    )
    .await;
    // An inline batched project_read leaves the same coverage as a direct read.
    checked(&d, "read_batch", json!({"calls":[{"id":"file","tool":"project_read","arguments":{"path":"notes.txt"}}]}),
        json!({"/result/isError":false,"/result/structuredContent/results/0/result/read_coverage_complete":true}), "end_turn").await;
    // At most eight items, unique ids, member arguments valid for that member.
    let nine: Vec<_> = (0..9)
        .map(|n| json!({"id":format!("i{n}"),"tool":"list_reports","arguments":{}}))
        .collect();
    checked(
        &d,
        "read_batch",
        json!({"calls":nine}),
        json!({"/error/code":-32602}),
        "refusal",
    )
    .await;
    checked(&d, "read_batch", json!({"calls":[{"id":"a","tool":"list_reports","arguments":{}},{"id":"a","tool":"list_reports","arguments":{}}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"invalid_arguments","/result/structuredContent/field":"calls[1].id"}), "end_turn").await;
    checked(
        &d,
        "read_batch",
        json!({"calls":[{"id":"a","tool":"read_report","arguments":{"report_id":"bad"}}]}),
        json!({"/result/isError":true,"/result/structuredContent/field":"calls[0].arguments"}),
        "end_turn",
    )
    .await;
    checked(&d, "read_batch", json!({"calls":[{"id":"a","tool":"list_reports","arguments":{"padding":"x".repeat(33000)}}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"request_too_large"}), "end_turn").await;
    // An item reply above 6144 bytes is retained whole and referenced, not inlined.
    // A deferred item is a normal outcome: the batch is complete and not an error.
    {
        let api = cortex_core::store::documents::Publisher::new(&s);
        let draft = api
            .create_draft("t", "runtime", "general", "large")
            .unwrap();
        api.write_draft(
            "t",
            "runtime",
            &draft.draft_id,
            &format!(
                "Cortex draft ID: `{}`\n\n# Large\n\n{}",
                draft.draft_id,
                "é".repeat(5000)
            ),
        )
        .unwrap();
        let large = api
            .publish(
                "t",
                "runtime",
                &draft.draft_id,
                "large",
                cortex_core::store::documents::Publication {
                    title: "Large",
                    summary: "Large evidence",
                    author: "runtime",
                    source_revision: 0,
                    artifacts: json!([]),
                },
            )
            .unwrap();
        checked(&d, "read_batch", json!({"calls":[{"id":"big","tool":"read_report","arguments":{"report_id":large.report_id}}]}),
            json!({"/result/isError":false,"/result/structuredContent/results/0/status":"deferred",
                "/result/structuredContent/outcome":"complete","/result/structuredContent/deferred_count":1,
                "/result/structuredContent/returned_count":0}), "end_turn").await;
        // An item error still makes the batch incomplete and an MCP error.
        checked(&d, "read_batch", json!({"calls":[{"id":"big","tool":"read_report","arguments":{"report_id":large.report_id}},
            {"id":"gone","tool":"read_report","arguments":{"report_id":"r_000000000000"}}]}),
            json!({"/result/isError":true,"/result/structuredContent/outcome":"incomplete",
                "/result/structuredContent/results/0/status":"deferred","/result/structuredContent/results/1/status":"error"}), "end_turn").await;
    }
    d.shutdown().await.unwrap();

    std::fs::create_dir_all(p.0.join("src")).unwrap();
    std::fs::write(p.0.join("src/file.txt"), "remove me").unwrap();
    let file_sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"remove me"));
    use cortex_core::store::owner_file_leases as leases;
    let checkout = leases::Checkout::identify(&p.0).unwrap();
    let epoch = leases::EditorEpoch::new();
    let editor = leases::Editor { epoch: &epoch, session: "fixture-editor" };
    // Model an effect that reaches the gateway after a lease was granted,
    // independent of whether normal scheduler admission would have held it.
    std::fs::create_dir_all(p.0.join(".claude/skills")).unwrap();
    std::fs::create_dir_all(p.0.join(".agents/skills/lease-demo")).unwrap();
    std::fs::write(p.0.join(".agents/skills/lease-demo/SKILL.md"), "fixture").unwrap();
    let held = ["src/file.txt", "src/fenced-dir", ".claude/skills/lease-demo"].map(|p| leases::RelativePath::new(p).unwrap());
    let token = s.write(|db| {
        leases::activate_editor_tx(db, &epoch)?;
        held.iter().map(|path| leases::acquire_tx(db, editor, &checkout, path)).collect::<cortex_core::CoreResult<Vec<_>>>()
    }).unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\",\".claude\",\".gitignore\"]','execution','running','a',NULL)", [])?;
        Ok(())
    })
    .unwrap();
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    checked(
        &w,
        "read_capsule",
        json!({}),
        json!({"/result/isError":false,
        "/result/structuredContent/recipient/role":"worker",
        "/result/structuredContent/recipient/assignment":"a"}),
        "end_turn",
    )
    .await;
    let fenced = [
        ("project_write", json!({"path":"src/file.txt","content":"bad","expected_sha256":file_sha,"request_key":"lease-write"})),
        ("project_edit", json!({"path":"src/file.txt","edits":[{"old_string":"remove me","new_string":"bad"}],"request_key":"lease-edit"})),
        ("project_delete", json!({"path":"src/file.txt","expected_sha256":file_sha,"request_key":"lease-delete"})),
        ("project_mkdir", json!({"path":"src/fenced-dir","request_key":"lease-mkdir"})),
        ("project_skill_link", json!({"action":"link","name":"lease-demo","request_key":"lease-link"})),
    ];
    for replay in [false, true] {
        if replay {
            s.write(|db| {
                for (path, token) in held.iter().zip(&token) {
                    leases::release_tx(db, editor, &checkout, path, token.expose())?;
                }
                Ok(())
            }).unwrap();
        }
        for (name, args) in &fenced {
            checked(&w, name, args.clone(), json!({
                "/result/isError":true,
                "/result/structuredContent/code":"owner_file_lease_held",
                "/result/structuredContent/hint":"The owner is editing this file in Code. Leave it unchanged and report the conflict. Read it again after the lease is released before making a new request."
            }), "end_turn").await;
        }
        assert_eq!(std::fs::read(p.0.join("src/file.txt")).unwrap(), b"remove me");
        assert!(!p.0.join("src/fenced-dir").exists());
        assert!(!p.0.join(".claude/skills/lease-demo").exists());
    }
    checked(&w, "project_mkdir", json!({"path":"src/fenced-dir","request_key":"after-lease"}),
        json!({"/result/structuredContent/created":true}), "end_turn").await;

    // A fully rendered text page exceeds the batch inline bound: the batch trims the
    // page to fit inline and records only what it returned.
    let large_text = (0..5)
        .map(|n| format!("{n}:{}\n", "x".repeat(1400)))
        .collect::<String>();
    let large_path = "src/batch-coverage.txt";
    std::fs::write(p.0.join(large_path), &large_text).unwrap();
    let sha = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(large_text.as_bytes())
    );
    checked(
        &w,
        "read_batch",
        json!({"calls":[{"id":"file","tool":"project_read","arguments":{"path":large_path}}]}),
        json!({"/result/isError":false,"/result/structuredContent/results/0/status":"ok",
            "/result/structuredContent/results/0/result/read_coverage_complete":false}),
        "end_turn",
    )
    .await;
    let replacement = json!({"path":large_path,"content":"replacement\n","expected_sha256":sha});
    checked(
        &w,
        "project_write",
        replacement.clone(),
        json!({"/result/isError":true,"/result/structuredContent/error":"gateway_complete_read_required"}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "project_edit",
        json!({"path":large_path,"edits":[{"old_string":"no-such-text","new_string":"first:"}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_found"}),
        "end_turn",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(p.0.join(large_path)).unwrap(),
        large_text
    );

    // The failed edit returned the current page, which already counts as a complete
    // read; further inline pages keep it complete and the replacement is allowed.
    for (offset, limit, complete) in [(1, 4, true), (5, 1, true)] {
        checked(
            &w,
            "read_batch",
            json!({"calls":[{"id":"file","tool":"project_read",
                "arguments":{"path":large_path,"offset":offset,"limit":limit}}]}),
            json!({"/result/isError":false,"/result/structuredContent/results/0/status":"ok",
                "/result/structuredContent/results/0/result/read_coverage_complete":complete}),
            "end_turn",
        )
        .await;
    }
    checked(
        &w,
        "project_write",
        replacement,
        json!({"/result/isError":false,"/result/structuredContent/drastic_shrink":true}),
        "end_turn",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(p.0.join(large_path)).unwrap(),
        "replacement\n"
    );

    // A multimodal item is refused in a batch and contributes no coverage.
    let image = b"\x89PNG\r\n\x1a\n";
    let image_path = "src/batch-coverage.png";
    std::fs::write(p.0.join(image_path), image).unwrap();
    checked(
        &w,
        "read_batch",
        json!({"calls":[{"id":"image","tool":"project_read","arguments":{"path":image_path}}]}),
        json!({"/result/isError":true,"/result/structuredContent/results/0/error/code":"batch_multimodal_unsupported"}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "project_write",
        json!({"path":image_path,"content":"replacement","expected_sha256":
            format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(image))}),
        json!({"/result/isError":true,"/result/structuredContent/error":"gateway_complete_read_required"}),
        "end_turn",
    )
    .await;
    assert_eq!(std::fs::read(p.0.join(image_path)).unwrap(), image);

    // A coordinator-only tool stays a named authority refusal for a worker.
    checked(
        &w,
        "task_get_context",
        json!({}),
        json!({"/error/code":-32001,
        "/error/data/code":"tool_not_offered"}),
        "refusal",
    )
    .await;
    checked(
        &w,
        "project_mkdir",
        json!({"path":"src/new/deep"}),
        json!({"/result/isError":false,
        "/result/structuredContent/created":true}),
        "end_turn",
    )
    .await;
    assert!(p.0.join("src/new/deep").is_dir());
    checked(
        &w,
        "project_mkdir",
        json!({"path":"src/new/deep"}),
        json!({
        "/result/structuredContent/created":true}),
        "end_turn",
    )
    .await;
    checked(&w, "project_mkdir", json!({"path":"src/file.txt/child"}), json!({"/result/isError":true,
        "/result/structuredContent/code":"gateway_path_exists","/result/structuredContent/field":"path"}),
        "end_turn").await;
    checked(
        &w,
        "project_mkdir",
        json!({"path":"src/x","request_key":"mk"}),
        json!({"/result/isError":false}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "project_mkdir",
        json!({"path":"src/y","request_key":"mk"}),
        json!({"/result/isError":true,
        "/result/structuredContent/code":"request_key_conflict"}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "project_mkdir",
        json!({"path":".git/hooks"}),
        json!({"/result/isError":true,
        "/result/structuredContent/code":"private_or_invalid_path"}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "project_delete",
        json!({"path":"src/file.txt","expected_sha256":"0".repeat(64)}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_write_conflict",
            "/result/structuredContent/field":"expected_sha256"}),
        "end_turn",
    )
    .await;
    // Guarded without an owner card: the deletion is not confirmed and nothing is removed;
    // an identical keyless retry reuses that refusal instead of asking again.
    for _ in 0..2 {
        checked(
            &w,
            "project_delete",
            json!({"path":"src/file.txt","expected_sha256":file_sha}),
            json!({"/error/code":-32001,"/error/data/code":"deletion_not_confirmed"}),
            "refusal",
        )
        .await;
    }
    assert!(p.0.join("src/file.txt").is_file());
    // `.claude` is a rule path with no default protection: linking needs only a source.
    checked(
        &w,
        "project_skill_link",
        json!({"action":"link","name":"demo","request_key":"l0"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"skill_source_missing",
            "/result/structuredContent/field":"name"}),
        "end_turn",
    )
    .await;
    std::fs::create_dir_all(p.0.join(".agents/skills/demo")).unwrap();
    std::fs::write(
        p.0.join(".agents/skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Demo.\n---\n",
    )
    .unwrap();
    checked(
        &w,
        "project_skill_link",
        json!({"action":"link","name":"demo","request_key":"l1"}),
        json!({"/result/isError":false,"/result/structuredContent/changed":true,
            "/result/structuredContent/path":".claude/skills/demo",
            "/result/structuredContent/target":"../../.agents/skills/demo",
            "/result/structuredContent/git/link_ignored":null}),
        "end_turn",
    )
    .await;
    assert_eq!(
        std::fs::read_link(p.0.join(".claude/skills/demo")).unwrap(),
        PathBuf::from("../../.agents/skills/demo")
    );
    checked(
        &w,
        "project_skill_link",
        json!({"action":"link","name":"demo","request_key":"l2"}),
        json!({"/result/structuredContent/changed":false}),
        "end_turn",
    )
    .await;
    std::fs::create_dir_all(p.0.join(".agents/skills/other")).unwrap();
    std::fs::write(p.0.join(".agents/skills/other/SKILL.md"), "# other\n").unwrap();
    std::os::unix::fs::symlink("elsewhere", p.0.join(".claude/skills/other")).unwrap();
    checked(
        &w,
        "project_skill_link",
        json!({"action":"link","name":"other","request_key":"o1"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"skill_link_occupied",
            "/result/structuredContent/field":"replace_symlink_target"}),
        "end_turn",
    )
    .await;
    checked(&w, "project_skill_link", json!({"action":"link","name":"other","request_key":"o2","replace_symlink_target":"elsewhere"}),
        json!({"/error/code":-32001,"/error/data/code":"skill_link_replace_not_confirmed"}), "refusal").await;
    assert_eq!(
        std::fs::read_link(p.0.join(".claude/skills/other")).unwrap(),
        PathBuf::from("elsewhere")
    );
    checked(
        &w,
        "project_skill_link",
        json!({"action":"link","name":"Bad/Name","request_key":"b"}),
        json!({"/result/isError":true,"/result/structuredContent/code":"skill_link_name_invalid"}),
        "end_turn",
    )
    .await;
    checked(
        &w,
        "check_preflight",
        json!({"argv":["cortex-fixture-missing-program"]}),
        json!({"/result/isError":false,"/result/structuredContent/available":false,
            "/result/structuredContent/reason":"program_missing","/result/structuredContent/evidence":false}),
        "end_turn",
    )
    .await;
    // A batched worker question keeps each member's topic label with that question.
    checked(
        &w,
        "ask_question",
        json!({"request_key":"topics","questions":[
        {"question":"Run paid checks?","recommended":2,"options":["Yes","No"],"topic":"Paid checks"},
        {"question":"Which model?","recommended":1,"options":["Small","Large"]}]}),
        json!({"/result/isError":false,"/result/structuredContent/questions/0/topic":"Paid checks",
            "/result/structuredContent/questions/0/recommended":2,
            "/result/structuredContent/questions/1/recommended":1,
            "/result/structuredContent/questions/1/question":"Which model?"}),
        "end_turn",
    )
    .await;
    let topic_kept = s.read(|db| Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id='t' AND payload_json LIKE '%\"topic\":\"Paid checks\"%'", [], |r| r.get::<_, i64>(0))?)).unwrap();
    assert!(topic_kept >= 1, "the wait record keeps the topic");
    w.shutdown().await.unwrap();

    // Auto still asks the owner about deletions.
    s.write(|db| {
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n2','worker-backend-dev','More work','[]','[\"src\",\".claude\",\".gitignore\"]','execution','running','b',NULL)", [])?;
        cortex_core::access::switch_tx(db, "t", "auto", "fixture", "acp_set_mode")
    })
    .unwrap();
    let w = h
        .dispatch(
            "t".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: "b".into(),
            },
            p.route(),
        )
        .await
        .unwrap();
    // Auto asks the owner for every deletion; with no owner client connected the
    // request is refused and nothing is deleted. Replacement cards stay answered by Auto.
    checked(
        &w,
        "project_delete",
        json!({"path":"src/file.txt","expected_sha256":file_sha,"request_key":"del"}),
        json!({"/error/data/code":"deletion_not_confirmed"}),
        "refusal",
    )
    .await;
    assert!(p.0.join("src/file.txt").exists());
    // The retained refusal answers an identical retry; the owner is not asked again.
    checked(
        &w,
        "project_delete",
        json!({"path":"src/file.txt","expected_sha256":file_sha,"request_key":"del"}),
        json!({"/error/data/code":"deletion_not_confirmed"}),
        "refusal",
    )
    .await;
    std::fs::write(p.0.join(".gitignore"), "target/\n").unwrap();
    let ignore_sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"target/\n"));
    checked(&w, "project_delete", json!({"path":".gitignore","expected_sha256":ignore_sha}),
        json!({"/error/data/code":"deletion_not_confirmed"}), "refusal").await;
    assert_eq!(
        std::fs::read_to_string(p.0.join(".gitignore")).unwrap(),
        "target/\n"
    );
    checked(&w, "project_skill_link", json!({"action":"link","name":"other","request_key":"o3","replace_symlink_target":"elsewhere"}),
        json!({"/result/isError":false,"/result/structuredContent/changed":true,
            "/result/structuredContent/previous/target":"elsewhere"}), "end_turn").await;
    checked(
        &w,
        "project_skill_link",
        json!({"action":"unlink","name":"demo","request_key":"u1"}),
        json!({"/result/structuredContent/changed":true}),
        "end_turn",
    )
    .await;
    assert!(!p.0.join(".claude/skills/demo").exists());
    assert!(p.0.join(".agents/skills/demo/SKILL.md").is_file());
    w.shutdown().await.unwrap();
    close(h).await;
}
async fn linked_checkout_command_journey() {
    let scratch = Project::new();
    let p = Project::new_at(&scratch.0, "logical");
    let trees = Project::new_at(&scratch.0, "checkouts");
    let state = Project::new_at(&scratch.0, "state");
    let home = Project::new_at(&scratch.0, "engine-home");
    std::fs::create_dir(home.0.join(".claude")).unwrap();
    let git = |argv: &[&str]| {
        let out = std::process::Command::new("/usr/bin/git").args(argv).current_dir(&p.0)
            .env_clear().env("PATH", "/usr/bin:/bin").env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        assert!(out.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    std::fs::write(p.0.join("fixture.txt"), "checkout bytes\n").unwrap();
    git(&["add", "fixture.txt"]);
    git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "base"]);
    let store = Store::open(p.key(), OpenMode::Initialize).unwrap();
    store.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)", [p.0.to_str()])?;
        Ok(())
    }).unwrap();
    let make = |id: &str| {
        use cortex_core::workspace::{create_draft, start_native_worktree, worktrees::resolve};
        let draft = create_draft(&store, "p", "owner", &json!({
            "protocol":{"major":1,"minor":8},"requestId":format!("draft-{id}"),"projectId":"p",
            "actorId":"owner","operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":format!("Linked task {id}"),"checkout":{"kind":"worktree"}}
        })).unwrap();
        let started = start_native_worktree(&store, "p", "owner", &json!({
            "protocol":{"major":1,"minor":8},"requestId":format!("start-{id}"),"projectId":"p",
            "actorId":"owner","operation":"tasks.start","target":draft.receipt["target"],
            "expectedVersion":"1","ownerEpoch":null,"payload":{}
        }), &trees.0).unwrap();
        let task = started["task"].as_str().unwrap().to_owned();
        let owner = cortex_core::ownership::OwnerEpoch {
            owner:"owner".into(), epoch:started["owner"]["epoch"].as_i64().unwrap(),
        };
        let checkout = resolve(&store, &task, &owner).unwrap().worktree.unwrap();
        (task, owner, checkout)
    };
    let (task, owner, checkout) = make("writer");
    let (sibling_task, sibling_owner, sibling) = make("sibling");
    let assignment = store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING', revision=1 WHERE task_id IN (?,?)", (&task,&sibling_task))?;
        for id in [&task, &sibling_task] {
            db.execute("INSERT INTO runtime_nodes VALUES (?,1,'writer','worker-backend-dev','Work','[]','[\"fixture.txt\"]','execution','pending',NULL,NULL)", [id])?;
        }
        let policy = cortex_core::scheduler::policy::NativePolicy::default();
        let first = cortex_core::scheduler::claim_ready_tx(db, &task, &owner, "fixture", &policy)?;
        let second = cortex_core::scheduler::claim_ready_tx(db, &sibling_task, &sibling_owner, "fixture", &policy)?;
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        cortex_core::access::switch_tx(db, &task, "development", "fixture", "acp_set_mode")?;
        Ok(first.into_iter().next().unwrap())
    }).unwrap();
    drop(store);
    std::fs::write(p.0.join("fixture.txt"), "main bytes\n").unwrap();
    std::fs::create_dir_all(p.0.join("docs/tmp")).unwrap();
    std::fs::write(p.0.join("docs/tmp/doc.txt"), "host doc\n").unwrap();
    let subdir = checkout.path.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let key = p.key();
    let state_path = state.0.clone();
    let host = tokio::task::spawn_blocking(move || NativeHost::open_workspace(key, Default::default(), state_path))
        .await.unwrap().unwrap();
    let mut route = p.legacy_route();
    route.launch.native_home = home.0.clone();
    route.launch.credential_directory = home.0.join(".claude");
    route.launch.base_environment.insert("PATH".into(), "/usr/bin:/bin".into());
    let driver = host.dispatch(task.clone(), owner.clone(), SessionRole::Worker { assignment }, route.clone()).await.unwrap();
    async fn probe(driver: &workspace_service::native_sessions::SessionDriver, name: &str, args: Value, expected: Value) {
        let key = args["request_key"].as_str().unwrap_or("-").to_owned();
        let prompt = format!("call:{}", json!({"name":name,"arguments":args,"__fixture_expect":expected}));
        tokio::time::timeout(Duration::from_secs(20), driver.prompt(&prompt)).await
            .unwrap_or_else(|_| panic!("{name} {key} did not settle"))
            .unwrap_or_else(|e| panic!("{name} {key}: {e:?}"));
    }
    probe(&driver, "project_read", json!({"path":"fixture.txt","line_numbers":false}),
        json!({"/result/structuredContent/content":"checkout bytes\n"})).await;
    let main_only = p.0.join("main-only");
    std::fs::write(&main_only, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&main_only, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    probe(&driver, "check_preflight", json!({"argv":["/usr/bin/git","diff","--check","HEAD"]}),
        json!({"/result/structuredContent/available":true,"/result/structuredContent/evidence":false})).await;
    probe(&driver, "check_preflight", json!({"argv":[main_only]}),
        json!({"/result/structuredContent/available":true,"/result/structuredContent/evidence":false})).await;
    probe(&driver, "check_preflight", json!({"argv":[p.0.join("absent-program")]}),
        json!({"/result/structuredContent/available":false,"/result/structuredContent/reason":"program_missing"})).await;
    probe(&driver, "command_run", json!({"argv":["/usr/bin/pwd"],"request_key":"linked-pwd"}),
        json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/cwd":checkout.path,
            "/result/structuredContent/output":format!("{}\n",checkout.path.display()),
            "/result/structuredContent/checkout/id":checkout.id,"/result/structuredContent/checkout/owner_epoch":owner.epoch,
            "/result/structuredContent/profile":"worktree_read_only"})).await;
    probe(&driver, "command_run", json!({"argv":["/usr/bin/bash","-ec",r#"
        [ "$(pwd -P)" = "$1" ]
        [ "$(cat ../fixture.txt)" = "checkout bytes" ]
        [ "$(cat "$2/docs/tmp/doc.txt")" = "host doc" ] && [ "$(cat "$2/fixture.txt")" = "main bytes" ]
        [ -f "$3/fixture.txt" ] && [ -r /etc/hostname ] && ! (printf bad > "$3/fixture.txt")
        ! (printf bad > "$2/.cortex-dev/probe") && ! (printf bad > "$2/.cortex/probe")
        ! (printf bad > "$2/fixture.txt") && ! (printf bad > "$2/docs/tmp/new.txt")
        ! (printf bad > ../fixture.txt) && ! (printf bad > ../.git)
        printf done > "$TMPDIR/only-here"
    "#,"check",subdir,p.0,sibling.path],"cwd":subdir,"request_key":"linked-boundary"}),
        json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/cwd":subdir,
            "/result/structuredContent/checkout/id":checkout.id})).await;
    for (name, args, error) in [
        ("command_run", json!({"argv":["/usr/bin/true"],"cwd":p.0,"request_key":"main-cwd"}), "worktree_command_cwd_refused"),
        ("command_run", json!({"argv":["/usr/bin/true"],"cwd":sibling.path,"request_key":"sibling-cwd"}), "worktree_command_cwd_refused"),
        ("command_run", json!({"argv":["sudo","/usr/bin/true"],"request_key":"sudo"}), "worktree_host_effect_unavailable"),
        ("outside_write", json!({"path":p.0.join("docs/tmp/new.txt"),"content":"x","request_key":"outside-write"}), "worktree_host_effect_unavailable"),
    ] {
        probe(&driver, name, args, json!({"/result/isError":true,"/result/structuredContent/code":error})).await;
    }
    probe(&driver, "project_read", json!({"path":p.0.join("docs/tmp/doc.txt"),"line_numbers":false}),
        json!({"/result/structuredContent/content":"host doc\n"})).await;
    probe(&driver, "project_read", json!({"path":p.0.join("docs/tmp")}),
        json!({"/result/structuredContent/entries/0":"doc.txt"})).await;
    // Auto: the owner's outside command runs on the host as the owner.
    p.store().write(|db| cortex_core::access::switch_tx(db, &task, "auto", "fixture-auto", "acp_set_mode")).unwrap();
    probe(&driver, "outside_command", json!({"argv":["/usr/bin/cat","docs/tmp/doc.txt"],"cwd":p.0,"request_key":"outside-read"}),
        json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/output":"host doc\n"})).await;
    // Administrative host commands in Auto still require a fresh owner decision.
    // Use a harmless fixture whose basename is `systemctl`; it only creates a
    // marker, so this exercises the consent route without touching a service.
    let admin_dir = p.0.join("admin-fixture");
    std::fs::create_dir(&admin_dir).unwrap();
    let marker = admin_dir.join("ran");
    let admin_program = admin_dir.join("systemctl");
    std::fs::write(&admin_program, format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&admin_program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let execution = p.store().read(|db| Ok(db.query_row(
        "SELECT execution_id FROM workspace_executions WHERE runtime_task_id=?", [&task], |r| r.get::<_, String>(0))?)).unwrap();
    let session = p.store().read(|db| Ok(db.query_row(
        "SELECT session_id FROM acp_host_sessions WHERE task_id=?", [&task], |r| r.get::<_, String>(0))?)).unwrap();
    let admin_store = p.store();
    async fn answer_admin(
        store: &Store, execution: &str, session: &str, request_id: &str, label: &str,
    ) {
        use cortex_core::workspace::control;
        let item = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let items = control::decisions(store, "p", "owner", &json!({
                    "operation":"decisions.list","target":execution,"limit":100,
                })).unwrap()["items"].as_array().cloned().unwrap_or_default();
                if let Some(item) = items.into_iter().find(|d| d["status"] == "pending" && d["kind"] == "permission") {
                    break item;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap_or_else(|_| panic!("administrative permission card did not open: {:?}", control::decisions(store, "p", "owner", &json!({"operation":"decisions.list","target":execution,"limit":100}))));
        assert_eq!(item["permission"]["operation"], "run");
        assert_eq!(item["permission"]["access"], "auto");
        assert!(item["prompt"].as_str().unwrap().contains("administrative access"));
        assert_eq!(item["permission"]["rule"], "administrative_command");
        let option = item["options"].as_array().unwrap().iter()
            .find(|o| o["label"] == label).unwrap()["optionId"].clone();
        control::answer(store, "p", "owner", &json!({
            "protocol":{"major":1,"minor":8},"projectId":"p","actorId":"owner",
            "requestId":request_id,"operation":"decisions.answer","target":item["decisionId"],
            "expectedVersion":item["version"],"ownerEpoch":item["ownerEpoch"],
            "payload":{"executionId":execution,"acpSessionId":session,"batchId":null,"answers":[{
                "decisionId":item["decisionId"],"questionId":item["questionId"],"expectedVersion":item["version"],"optionId":option,"text":null,
            }]}
        })).unwrap();
    }
    let denied_call = call("outside_command", json!({
        "argv":[admin_program,"restart","fixture"],"cwd":p.0,"request_key":"admin-deny"
    }));
    let denied = driver.prompt(&denied_call);
    let deny_answer = answer_admin(&admin_store, &execution, &session, "admin-deny-answer", "Deny");
    let (denied, ()) = tokio::join!(denied, deny_answer);
    assert_eq!(denied.unwrap().stop_reason, "end_turn");
    assert!(!marker.exists(), "denied administrative command ran");
    let allowed_call = call("outside_command", json!({
        "argv":[admin_program,"restart","fixture"],"cwd":p.0,"request_key":"admin-allow"
    }));
    let allowed = driver.prompt(&allowed_call);
    let allow_answer = answer_admin(&admin_store, &execution, &session, "admin-allow-answer", "Allow once");
    let (allowed, ()) = tokio::join!(allowed, allow_answer);
    assert_eq!(allowed.unwrap().stop_reason, "end_turn");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran");
    std::fs::remove_file(&marker).unwrap();
    let allowed_again_call = call("outside_command", json!({
        "argv":[admin_program,"restart","fixture"],"cwd":p.0,"request_key":"admin-allow-again"
    }));
    let allowed_again = driver.prompt(&allowed_again_call);
    let allow_again_answer = answer_admin(&admin_store, &execution, &session, "admin-allow-again-answer", "Deny");
    let (allowed_again, ()) = tokio::join!(allowed_again, allow_again_answer);
    assert_eq!(allowed_again.unwrap().stop_reason, "end_turn");
    assert!(!marker.exists(), "Allow once token was reused");
    p.store().write(|db| cortex_core::access::switch_tx(db, &task, "development", "fixture-dev-again", "acp_set_mode")).unwrap();
    probe(&driver, "project_git", json!({"kind":"status","request_key":"linked-git"}),
        json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/cwd":checkout.path,
            "/result/structuredContent/checkout/id":checkout.id})).await;
    for (kind, extra) in [("log", json!({})), ("show", json!({"rev":"HEAD"})), ("cat_file", json!({"rev":"HEAD","path":"fixture.txt"})),
        ("ls_files", json!({})), ("blame", json!({"path":"fixture.txt"}))] {
        let mut a = json!({"kind":kind,"request_key":format!("linked-git-{kind}")});
        a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        probe(&driver, "project_git", a, json!({"/result/structuredContent/exit_code":0,
            "/result/structuredContent/checkout/id":checkout.id})).await;
    }
    // The shell above only inspects the view, so it must not count as a
    // nontrivial check. Use a real Git check to exercise work-set evidence.
    probe(&driver, "command_run", json!({"argv":["/usr/bin/git","diff","--check","HEAD"],
        "cwd":subdir,"request_key":"linked-check"}),
        json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/cwd":subdir,
            "/result/structuredContent/checkout/id":checkout.id})).await;
    let store = p.store();
    let binding = store.read(|db| {
        let raw: String = db.query_row("SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key='linked-check'", [&task], |r|r.get(0))?;
        let run: Value = serde_json::from_str(&raw)?;
        assert_eq!(run["checkout"]["root"], json!(checkout.path));
        assert_eq!(run["cwd"], json!(subdir));
        Ok(run)
    }).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let record = store.read(|db| cortex_core::lifecycle::worker_checks::work_set_record(
            db, &task, binding["tool_call_ref"].as_str().unwrap())).unwrap();
        if let Some(record) = record {
            assert_eq!(record["checkout"], binding["checkout"]);
            assert_eq!(record["cwd"], binding["cwd"]);
            // Main deliberately has different bytes at the same relative path.
            // Compute the expected work-set witness from the fixture's Git file.
            use sha2::{Digest, Sha256};
            let leaf = Sha256::digest(b"checkout bytes\n");
            let expected = Sha256::digest(format!("cortex-work-set-v1\nfixture.txt\n\0fixture.txt\0file:644:{leaf:x}\n"));
            assert_eq!(record["end_digest"], format!("{expected:x}"));
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no retained work-set observation");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    driver.shutdown().await.unwrap();
    assert_eq!(std::fs::read_to_string(p.0.join("fixture.txt")).unwrap(), "main bytes\n");
    assert_eq!(std::fs::read_to_string(checkout.path.join("fixture.txt")).unwrap(), "checkout bytes\n");
    assert_eq!(std::fs::read_to_string(sibling.path.join("fixture.txt")).unwrap(), "checkout bytes\n");

    let reader = store.write(|db| {
        db.execute("UPDATE runtime_nodes SET role='worker-explorer' WHERE task_id=?", [&sibling_task])?;
        cortex_core::access::switch_tx(db, &sibling_task, "auto", "fixture-auto", "acp_set_mode")?;
        Ok(db.query_row("SELECT assignment_ref FROM runtime_nodes WHERE task_id=?", [&sibling_task], |r| r.get(0))?)
    }).unwrap();
    let reader = host.dispatch(sibling_task, sibling_owner, SessionRole::Worker { assignment: reader }, route).await.unwrap();
    // Install a locked local dependency with no registry, credentials or lifecycle scripts.
    let dependency = sibling.path.join("fixture-dep");
    std::fs::create_dir(&dependency).unwrap();
    std::fs::write(dependency.join("package.json"), r#"{"name":"fixture-dep","version":"1.0.0"}"#).unwrap();
    std::fs::write(sibling.path.join("package.json"), r#"{"name":"fixture","dependencies":{"fixture-dep":"file:fixture-dep"}}"#).unwrap();
    std::fs::write(sibling.path.join("package-lock.json"), r#"{"name":"fixture","lockfileVersion":3,"packages":{"":{"name":"fixture","dependencies":{"fixture-dep":"file:fixture-dep"}},"fixture-dep":{"version":"1.0.0"},"node_modules/fixture-dep":{"resolved":"fixture-dep","link":true}}}"#).unwrap();
    probe(&reader, "command_run", json!({"argv":["/bin/sh","-ec",
        r#"printf '%s' "$CORTEX_COMMAND_TMPDIR" > scratch-path; npm ci --offline --ignore-scripts --no-audit --no-fund --cache "$CORTEX_COMMAND_TMPDIR/npm" > "$CORTEX_COMMAND_TMPDIR/install.log"; test -f node_modules/fixture-dep/package.json"#],
        "request_key":"reader-install"}), json!({"/result/structuredContent/exit_code":0})).await;
    assert!(sibling.path.join("node_modules/fixture-dep/package.json").is_file());
    let scratch_path = std::fs::read_to_string(sibling.path.join("scratch-path")).unwrap();
    assert!(!scratch_path.is_empty() && !std::path::Path::new(&scratch_path).exists());
    reader.shutdown().await.unwrap();
}

// The sudo shims use a filesystem socket in XDG_RUNTIME_DIR. Keep it in the
// hermetic runtime directory even when TMPDIR makes its normal address too
// long. The fixture's child processes also need to resolve the short path.
fn short_askpass_runtime() -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;

    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    if !dir.parent().is_some_and(hermetic::is_private_home)
        || dir
            .join("cw-askpass-0123456789abcdef/socket")
            .as_os_str()
            .len()
            < 108
    {
        return None;
    }
    let runtime = std::fs::File::open(dir).unwrap();
    std::env::set_var(
        "XDG_RUNTIME_DIR",
        format!("/proc/{}/fd/{}", std::process::id(), runtime.as_raw_fd()),
    );
    Some(runtime)
}

fn main() {
    // The askpass helper and the sudo shim of host commands are this executable too.
    workspace_service::native_askpass::dispatch();
    if std::env::args().nth(1).as_deref()
        == Some(workspace_service::native_command_supervisor::MODE)
    {
        std::process::exit(
            if workspace_service::native_command_supervisor::run().is_ok() {
                0
            } else {
                1
            },
        );
    }
    // Test-only verification command: succeeds after the given milliseconds.
    if std::env::args().nth(1).as_deref() == Some("sleep-ms") {
        let ms = std::env::args()
            .nth(2)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        std::thread::sleep(Duration::from_millis(ms));
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("external-fixture") {
        external_fixture();
        return;
    }
    // A contained server that refuses to start after its own startup checks.
    if std::env::args().nth(1).as_deref() == Some("external-exit") {
        std::fs::write(std::env::var("EXIT_MARKER").unwrap(), "exited").unwrap();
        std::process::exit(3);
    }
    let mode = std::env::args().nth(1).unwrap_or_default();
    if ["native-host", "mcp-host", "external-fixture"].contains(&mode.as_str())
        || mode.starts_with('/')
    {
        peer::main();
        return;
    }
    let _askpass_runtime = short_askpass_runtime();
    let containment = matches!(
        mode.as_str(),
        "broker-command-acceptance" | "managed-command-acceptance"
    )
    .then(command_acceptance::Inputs::new);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        if mode == "linked-checkout-command" {
            tokio::time::timeout(Duration::from_secs(90), linked_checkout_command_journey())
                .await.expect("linked checkout journey exceeded deadline");
            println!("linked checkout: supervised cwd, mounts, work-set receipts and refusals passed");
            return;
        }
        if mode == "worker-question" {
            tokio::time::timeout(Duration::from_secs(30), worker_question_answer_exact_delivery_and_native_denial())
                .await.expect("worker question fixture exceeded deadline");
            println!("worker question recommendation, answer delivery and native denial passed");
            return;
        }
        if mode == "prompt-usage" {
            tokio::time::timeout(
                Duration::from_secs(30),
                prompt_usage_follows_real_native_dispatches(),
            )
            .await
            .expect("prompt usage fixture exceeded deadline");
            println!("native prompt usage: dispatch, settle, failure and result teardown passed");
            return;
        }
        if mode == "verifier-command-acceptance" {
            let inputs = command_acceptance::VerifierInputs::new();
            tokio::time::timeout(
                Duration::from_secs(120),
                native_plan_review_verification_journey(false, false, false, false, Some(&inputs)),
            )
            .await
            .expect("live verifier acceptance exceeded deadline");
            return;
        }
        if let Some(inputs) = &containment {
            tokio::time::timeout(Duration::from_secs(120), async {
                match mode.as_str() {
                    "broker-command-acceptance" => command_acceptance::contained(inputs, false).await,
                    "managed-command-acceptance" => command_acceptance::contained(inputs, true).await,
                    _ => unreachable!(),
                }
            })
            .await
            .expect("command containment acceptance exceeded deadline");
            return;
        }
        if matches!(
            mode.as_str(),
            "command-role-live" | "command-role-denials"
        ) {
            tokio::time::timeout(
                Duration::from_secs(120),
                command_acceptance::role_mode_matrix(mode == "command-role-live"),
            )
            .await
            .expect("command role/mode matrix exceeded deadline");
            return;
        }
        if mode == "outside-auto-matrix" {
            tokio::time::timeout(
                Duration::from_secs(60),
                command_acceptance::auto_outside_matrix(),
            )
            .await
            .expect("Auto outside-action matrix exceeded deadline");
            return;
        }
        if mode == "outside-command-consent" {
            tokio::time::timeout(Duration::from_secs(30), command_acceptance::consent())
                .await
                .expect("outside command consent acceptance exceeded deadline");
            return;
        }
        if mode == "recovery-project-operations" {
            recovery_project_operations_and_structured_tool_errors().await;
            println!("recovery project operations, including batch read coverage, passed");
            return;
        }
        if mode == "answer-replan" {
            tokio::time::timeout(
                Duration::from_secs(45),
                answer_turn_replan_settles_the_answer_delivery(),
            )
            .await
            .expect("answer replan scenario exceeded deadline");
            println!("answer-turn replan settles the answer delivery");
            return;
        }
        if mode == "verification-drift" {
            verification_drift_blocks_the_pass().await;
            return;
        }
        if mode == "shared-checkout" {
            tokio::time::timeout(
                Duration::from_secs(90),
                shared_checkout_writer_waits_until_review_and_both_verify(),
            )
            .await
            .expect("shared checkout journey exceeded deadline");
            println!("shared checkout writer barrier passed");
            return;
        }
        if mode == "synthesis-feedback" {
            if tokio::time::timeout(Duration::from_secs(60), reopened_synthesis_feedback())
                .await
                .expect("synthesis feedback journey exceeded deadline")
            {
                println!("reopened synthesis feedback: Claude and Codex fixture journeys passed");
            }
            return;
        }
        // `plain-folder`: only the lifecycle of a task in a folder without Git.
        if mode == "plain-folder" {
            tokio::time::timeout(
                Duration::from_secs(60),
                plain_folder_task_runs_its_whole_lifecycle(),
            )
            .await
            .expect("plain-folder lifecycle exceeded deadline");
            println!("plain-folder lifecycle passed");
            return;
        }
        if mode.starts_with("uncontained-external") {
            tokio::time::timeout(Duration::from_secs(120), uncontained_external_server_runs_with_owner_authority())
                .await.expect("uncontained external exceeded deadline");
            println!("uncontained external MCP offered identically to Codex and Claude passed");
            return;
        }
        if mode == "external-worker-grants" {
            tokio::time::timeout(Duration::from_secs(90), configured_external_worker_grants())
                .await
                .expect("external worker grants exceeded deadline");
            println!("external worker grants, including Claude review discovery, passed");
            return;
        }
        if mode == "command-start-latency" {
            tokio::time::timeout(Duration::from_secs(600), command_start_never_waits_for_project_observation())
                .await
                .expect("command start latency scenario exceeded deadline");
            println!("command start without project observation passed");
            return;
        }
        if mode == "sudo-host-route" {
            tokio::time::timeout(Duration::from_secs(90), sudo_prompts_and_host_reroute())
                .await
                .expect("sudo host route scenario exceeded deadline");
            println!("sudo password prompts and host reroute passed");
            return;
        }
        // `engine-modes`: only the native permission and engine-mode scenarios.
        if mode == "engine-modes" {
            tokio::time::timeout(Duration::from_secs(90), async {
                claude_permission_requests_follow_runtime_policy().await;
                owner_cards_stay_pending_and_record_the_owner().await;
                desktop_permission_decisions().await;
                read_only_channels_start_restricted_in_every_access_mode().await;
                access_modes_and_outside_routes().await;
            })
            .await
            .expect("engine mode scenarios exceeded deadline");
            println!("engine mode and native permission scenarios passed");
            return;
        }
        if mode == "project-command-tools" {
            tokio::time::timeout(Duration::from_secs(90), channel_source_project_and_commands())
                .await
                .expect("project and command tool scenarios exceeded deadline");
            println!("project and command tool scenarios passed");
            return;
        }
        // This executable serially exercises all scenarios, including full
        // before/after project observations for evidence-bearing commands.
        macro_rules! run_case {
            ($case:expr) => {{
                eprintln!("native session case: {}", stringify!($case));
                $case.await;
            }};
        }
        tokio::time::timeout(Duration::from_secs(90), async {
            run_case!(channel_source_project_and_commands());
            run_case!(attachments_reach_the_engine_as_readable_blocks());
            run_case!(claude_permission_requests_follow_runtime_policy());
            run_case!(owner_cards_stay_pending_and_record_the_owner());
            run_case!(desktop_permission_decisions());
            run_case!(read_only_channels_start_restricted_in_every_access_mode());
            run_case!(reopened_synthesis_feedback());
            run_case!(access_modes_and_outside_routes());
            run_case!(sudo_prompts_and_host_reroute());
            run_case!(route_effort_starts_codex_and_claude_coordinators());
            run_case!(document_tools_through_engine_and_relay());
            run_case!(configured_external_worker_grants());
            run_case!(optional_external_failures_withhold_only_that_server());
            run_case!(uncontained_external_server_runs_with_owner_authority());
            run_case!(native_workspace_binding_projects_activity_and_scoped_skills());
            run_case!(project_skills_read_through_the_channel());
            run_case!(recovery_project_operations_and_structured_tool_errors());
            run_case!(legacy_task_admission_and_session_lifetime_fence());
            run_case!(coordinator_tools_not_chat_and_lease());
            run_case!(engine_model_name_is_recorded_with_the_configured_session());
            run_case!(worker_question_answer_exact_delivery_and_native_denial());
            run_case!(answer_turn_replan_settles_the_answer_delivery());
            run_case!(pause_resume_does_not_revive_old_coordinator_and_cancel_is_not_completion());
            run_case!(startup_failure_settles_only_owned_reaped_process());
            run_case!(launch_refusal_before_spawn_is_a_confirmed_absence());
            run_case!(codex_launcher_sessions_hand_relays_to_app_server_not_acp());
            run_case!(claude_update_behind_session_new_reply_never_fails_configure());
            run_case!(configure_transport_loss_starts_once_more_and_records_both_attempts());
            run_case!(repeated_configure_transport_loss_stops_with_the_roles_blocker());
            run_case!(inline_result_is_published_once_bound_to_attempt_and_reconciled_after_restart());
            run_case!(result_failure_and_review_refusal_are_not_acceptance());
            run_case!(prompt_usage_follows_real_native_dispatches());
            run_case!(native_plan_review_verification_journey(false, false, false, false, None));
            run_case!(native_plan_review_verification_journey(true, false, false, false, None));
            run_case!(native_plan_review_verification_journey(false, true, false, false, None));
            run_case!(native_plan_review_verification_journey(false, false, true, false, None));
            run_case!(native_plan_review_verification_journey(false, false, false, true, None));
            run_case!(plain_folder_task_runs_its_whole_lifecycle());
            run_case!(shared_checkout_writer_waits_until_review_and_both_verify());
            run_case!(verification_drift_blocks_the_pass());
        })
        .await
        .expect("native session scenarios exceeded deadline");
    });
    if ![
        "prompt-usage",
        "recovery-project-operations",
        "answer-replan",
        "verification-drift",
        "shared-checkout",
        "synthesis-feedback",
        "engine-modes",
        "plain-folder",
        "external-worker-grants",
        "sudo-host-route",
        "project-command-tools",
        "outside-command-consent",
        "broker-command-acceptance",
        "managed-command-acceptance",
        "verifier-command-acceptance",
    ]
    .contains(&mode.as_str())
    {
        println!("native source/project/command, read-only, Git and access/outside scenarios asserted with existing session regressions (fake ACP/relay processes, disposable stores; unavailable verification is reported above)");
    }
}

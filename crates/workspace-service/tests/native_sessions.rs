//! Custom harness: this executable is also the disposable ACP peer and relay.
#[path = "support/hermetic.rs"]
mod hermetic;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;
// harness = false keeps cfg(test) but strips #[test], so this reused module's unit-test helpers look unused.
#[allow(dead_code, unused_imports)]
#[path = "../src/native_command_sandbox.rs"]
mod sandbox;
// The sandbox reads the owner's tool search path from this module.
#[allow(dead_code, unused_imports)]
#[path = "../src/tool_path.rs"]
mod tool_path;
// The command backend uses only the runtime random-identity helper.
mod native_effects {
    pub fn identity() -> std::io::Result<String> {
        use std::io::Read;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

fn verification_available(root: &std::path::Path) -> bool {
    let sandbox = match sandbox::Sandbox::prepare(root, sandbox::Profile::ReadOnly, &[], &[], false)
    {
        Ok(s) => s,
        Err(e) => {
            assert!(
                e.to_string()
                    .starts_with("native_command_sandbox_unavailable:"),
                "{e}"
            );
            return false;
        }
    };
    let mut probe = sandbox.command(&["/bin/true".into()]).unwrap();
    match probe.status() {
        Ok(status) => {
            assert!(status.success());
            true
        }
        Err(e) => {
            assert_eq!(e.raw_os_error(), Some(libc::ENOTSUP));
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
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "native-session-long-project-path-regression-{}-{}",
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
        .expect("exact result receipt missing");
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
        let prompt = format!(
            "call:{}",
            json!({"name":n,"arguments":a,"__fixture_expect":e})
        );
        d.prompt(&prompt).await.unwrap();
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
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        cortex_core::access::switch_tx(db,"t","development","fixture-development","acp_set_mode")
    }).unwrap();
    let mut route = p.route();
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
        "project_edit",
        json!({"path":"src/value","edits":[{"old_string":"one","new_string":"ONE"}]}),
        json!({"/result/structuredContent/error":"gateway_read_required","/result/structuredContent/write_state":"rejected"}),
    )
    .await;
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
        json!({"/result/structuredContent/error":"gateway_stale_read","/result/structuredContent/write_state":"rejected"}),
    )
    .await;
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
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_unique","/result/structuredContent/field":"edits"}),
    )
    .await;
    // Ordered replacements are atomic, including when a later anchor is absent.
    check(
        &w,
        "project_edit",
        json!({"path":"src/overlap","edits":[
        {"old_string":"aaa","new_string":"first"},{"old_string":"missing","new_string":"second"}]}),
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_found","/result/structuredContent/field":"edits"}),
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
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_no_change","/result/structuredContent/field":"edits"}),
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
        json!({"/result/isError":true,"/result/structuredContent/code":"gateway_edit_not_unique","/result/structuredContent/field":"edits"}),
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
    if verification_available(&p.0) {
        // A writing role's declared reservation does not restrict project writes.
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","new-unreserved"],"request_key":"unreserved"}),
            json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"development"})).await;
        assert!(p.0.join("new-unreserved").exists());
        // /tmp is intentionally shared/writable even in Guarded. Use the
        // literal host /var/tmp to distinguish the host-writable profile.
        let outside = Project(PathBuf::from("/var/tmp").join(p.0.file_name().unwrap()));
        std::fs::create_dir(&outside.0).unwrap();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch",outside.0.join("host-effect")],"request_key":"host-effect"}),json!({"/result/structuredContent/exit_code":0})).await;
        assert!(outside.0.join("host-effect").exists());
        std::fs::write(p.0.join("src/denied"), "owner-protected").unwrap();
        s.write(|db| {
            cortex_core::lifecycle::event_tx(
                db,
                "t",
                "USER_STEERING",
                json!({"deny_write":["src/denied"]}),
            )?;
            cortex_core::access::switch_tx(db, "t", "guarded", "fixture-guarded", "acp_set_mode")
        })
        .unwrap();
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","guarded-unreserved"],"request_key":"guarded-write"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"guarded"})).await;
        check(
            &w,
            "command_run",
            json!({"argv":["/usr/bin/touch","src/denied"],"request_key":"owner-denied"}),
            json!({"/result/structuredContent/exit_code":1}),
        )
        .await;
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
        check(&w,"command_run",json!({"argv":["/usr/bin/touch",outside.0.join("auto-effect")],"request_key":"auto-effect"}),json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/profile":"development"})).await;
        assert!(outside.0.join("auto-effect").exists());
        check(&w,"command_run",json!({"argv":["/bin/pwd"],"cwd":"/","request_key":"outside-cwd"}),
            json!({"/result/structuredContent/exit_code":0,"/result/structuredContent/output":"/\n"})).await;
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
        check(&w,"command_run",json!({"argv":["/bin/cat",".cortex-dev/cortex.sqlite3"],"request_key":"private"}),json!({"/result/structuredContent/exit_code":1,"/result/structuredContent/output_truncated":false})).await;
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
        check(&w,"command_run",json!({"argv":["/usr/bin/touch","src/nested/.cortex-dev/denied"],"request_key":"private-child"}),json!({"/result/structuredContent/exit_code":1})).await;
        assert!(!p.0.join("src/nested/.cortex-dev/denied").exists());
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
            p.route(),
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
    // A verification that only searches is a correctable content error with its exact
    // code, never an authority refusal; nothing is recorded and the coordinator may retry.
    let trivial = json!({"name":"plan_submit","arguments":{"request_key":"trivial","plan":{
        "tasks":[{"id":"w","role":"backend-dev","objective":"Write notes.txt","dependencies":[],
            "scope":["notes.txt"],"model":"auto"}],
        "verification":[["grep","-qx","ok","notes.txt"]]}},
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
            if path.exists() {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
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
            if mode == "development" {
                std::fs::create_dir(outside.0.join(".cortex")).unwrap();
                checked(&d,"outside_write",json!({"path":outside.0.join(".cortex/guarded"),"content":"no","request_key":"protected"}),
                    json!({"/result/isError":true}),"end_turn").await;
                assert!(!outside.0.join(".cortex/guarded").exists());
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
                json!({"question":"Wait?","options":["Yes","No"],"request_key":"wait"}),
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
            assert_eq!(
                d.prompt(&call(name, args)).await.unwrap().stop_reason,
                "refusal"
            );
        }
        assert!(!outside.0.join("no").exists());
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
            ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED') ORDER BY id DESC LIMIT 1",
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
            (
                native("edit", json!({"file_path":control}), Some(control.clone())),
                &reject,
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
            db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_REQUESTED'",[],|r|r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_PERMISSION_DECIDED','NATIVE_PERMISSION_CANCELLED')
                AND json_extract(payload_json,'$.rule') IS NOT NULL",[],|r|r.get(0))?,
        ))).unwrap();
        assert_eq!((requested, ruled), (14, 14));
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
        "/claudeCode/options/settings/autoMemoryEnabled":false,
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
    // A writing Codex worker runs in workspace-write in every access mode, and its
    // escalations (an apply_patch outside the project, into credentials, Git
    // controls or Cortex state) are answered by the runtime per the decision table.
    for access in ["auto", "guarded", "development"] {
        let p = Project::new();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&p.0)
            .status()
            .unwrap()
            .success());
        let outside = Project::new();
        let h = p.host().await;
        let o = h
            .create("t".into(), "Work".into(), "owner".into())
            .await
            .unwrap();
        let s = p.store();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
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
        assert_eq!(confirmed, "workspace-write", "{access}");
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
            (p.0.join(".cortex/injected"), "reject", "cortex_private"),
            (p.0.join(".cortex-dev/injected"), "reject", "cortex_private"),
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
            } else if rule == "cortex_private" {
                "runtime".into()
            } else {
                "no_owner_client".into()
            };
            assert_eq!(payload["decider"], decider, "{access} {target:?}");
        }
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
    for (model, label) in [("default", Some("Default (Opus 5.5)")), ("fixture-model", Some("Fixture"))] {
        let p = Project::new();
        std::fs::write(p.0.join("model-alias"), "").unwrap();
        let h = p.host().await;
        let o = h.create("t".into(), "Work".into(), "owner".into()).await.unwrap();
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
            .dispatch("t".into(), o, SessionRole::Worker { assignment: "a".into() }, route)
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
    let q = call(
        "ask_question",
        json!({"question":"Which output?","options":["Detailed","Brief"],"request_key":"question"}),
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
        json!({"question":"Wait for which result?","options":["First","Second"],"request_key":"question2"}),
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
        }
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
    let coordinator = h
        .dispatch(
            "journey".into(),
            o.clone(),
            SessionRole::Coordinator,
            p.route(),
        )
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Change work.txt","dependencies":[],"scope":["work.txt"],"model":"auto"}],
        "verification":if truncated{json!([["/usr/bin/dd","if=/dev/zero","bs=1048577","count=1","status=none"]])}else if failing{json!([["/bin/false"]])}else{json!([["/usr/bin/cmp","/dev/null","/dev/null"]])}});
    // Invalid proposals cannot change the task, create nodes or consume the key; each is
    // a correctable content error (isError with its code), not an authority refusal.
    for bad in [
        json!({"tasks":[],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"code-reviewer","objective":"Reserved","dependencies":[],"scope":["."],"model":"auto"}],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Cycle","dependencies":["work"],"scope":["."],"model":"auto"}],"verification":[["true"]]}),
        json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Private","dependencies":[],"scope":[".cortex/private"],"model":"auto"}],"verification":[["true"]]}),
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
    let a = step["assignments"][0]["assignment_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(step["state"], "EXECUTING");
    let worker = h
        .dispatch(
            "journey".into(),
            o.clone(),
            SessionRole::Worker {
                assignment: a.clone(),
            },
            p.route(),
        )
        .await
        .unwrap();
    std::fs::write(
        p.0.join("work.txt"),
        if truncated {
            "x".repeat(1048577)
        } else {
            "after".into()
        },
    )
    .unwrap();
    let result = json!({"report":"# Work\n\nFixture file changed.","summary":"Changed the fixture file.","checks":[],"request_key":"result"});
    submit_result(&h, &worker, "journey", &o, &a, result).await;
    // A report never stands in for a process wait or begins verification.
    assert_eq!(
        h.advance("journey".into(), o.clone()).await.unwrap()["state"],
        "EXECUTING"
    );
    worker.shutdown().await.unwrap();
    let mut step = h.advance("journey".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "REVIEWING");
    assert_eq!(step["assignments"][0]["role"], "worker-code-reviewer");
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
    let mut result = json!({"report":"# Independent review\n\nObserved fixture file and worker report.","summary":"Reviewed the fixture.",
        "checks":[],"verdict":if fix{"REQUEST_CHANGES"}else{"APPROVE"},"request_key":"review",
        "findings":if fix{json!([{"node":"work","files":["work.txt"],"fix":"Make the fixture correct.","cross_cutting":false}])}else{json!([])}});
    if restart {
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
        assert_eq!(output["profile"],"native-readonly-bubblewrap");
        assert_eq!(output["observation"]["complete"],!truncated);
        assert_eq!(output["observation"]["output_truncated"],truncated);
        assert_eq!(output["observation"]["exit_code"],if failing{1}else{0});
        let event:String=db.query_row("SELECT type FROM runtime_events WHERE type IN ('VERIFICATION_FAILED','VERIFICATION_PASSED')",[],|r|r.get(0))?;
        assert_eq!(event,if failing||truncated{"VERIFICATION_FAILED"}else{"VERIFICATION_PASSED"});
        Ok(())
    }).unwrap();
    close(h).await;
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
            p.route(),
        )
        .await
        .unwrap();
    std::fs::write(p.0.join("work.txt"), "after").unwrap();
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
    assert!(!encoded.contains("private-fixture-value"));
    assert!(!encoded.contains("Skills already included"));
    assert!(page.next_cursor.is_none());
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
        "options":["Split","Single"],"request_key":"midstream"},
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
    for (role, expectation) in [
        ("worker-explorer", "read"),
        ("worker-backend-dev", "write"),
        ("worker-senior-consultant", "none"),
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
                "env":{"FIXTURE_SECRET":"memory-only-fixture"}
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
        registry
            .validate_client(&json!([{"name":"fixture","command":"/untrusted/override"}]))
            .unwrap();
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
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
            // `docs/styleguide.md` does not exist yet: it is withheld, never refused.
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Work','[]',?,'execution','running','a',NULL)",[role,r#"["src","docs/styleguide.md"]"#])?;
            Ok(())
        }).unwrap();
        let mut route = p.route();
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
            d.prompt(expectation)
                .await
                .unwrap_or_else(|e| panic!(
                    "fixture {role}: {e}; {}",
                    std::fs::read_to_string(p.0.join("mcp-probe")).unwrap_or_default()
                ))
                .stop_reason,
            "end_turn"
        );
        assert_eq!(
            p.0.join("src/external-write").exists(),
            expectation == "write"
        );
        assert!(!p.0.join("outside-scope").exists());
        assert!(!p.0.join("docs").exists());
        store.read(|db|{
            let withheld:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_EXTERNAL_MCP_WITHHELD'",[],|r|r.get(0))?;
            assert_eq!(withheld,0,"a not-yet-created scope withholds nothing");
            let count:i64=db.query_row("SELECT count(*) FROM runtime_requests WHERE operation='external_call'",[],|r|r.get(0))?;
            assert_eq!(count,match expectation {"write"=>2,"read"=>1,_=>0});
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
    let mut route = p.route();
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
    let mut route = p.route();
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
                json!({"tools":[{"name":"lookup","inputSchema":{"type":"object"}},{"name":"write","inputSchema":{"type":"object"}}]})
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
                    assert!(std::net::TcpListener::bind("127.0.0.1:0").is_err());
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
                s.write(|db| {
                    assert!(domain::reopen_tx(db, "retry")?);
                    domain::settle_recovery_tx(db, "retry", TaskState::Planning)
                })
                .unwrap();
            }
            let d = h
                .dispatch("retry".into(), o.clone(), SessionRole::Coordinator, route())
                .await
                .unwrap();
            // Permission and a valid schema cannot bypass the task-state gate; the refusal
            // is a structured tool error naming synthesis_not_expected.
            assert_eq!(
                d.prompt(&call("synthesis_submit", json!({"text":"Too early."})))
                    .await
                    .unwrap()
                    .stop_reason,
                "end_turn"
            );
            d.prompt(&call("plan_submit", json!({
                "request_key":format!("plan-{revision}"),
                "plan":{"tasks":[{"id":"recheck","role":"qa-engineer","objective":"Check unchanged work.txt",
                    "dependencies":[],"scope":["work.txt"],"model":"auto"}],
                    "verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]}
            }))).await.unwrap();
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
                        "SELECT payload_json FROM runtime_events WHERE task_id='retry' AND type='NATIVE_PERMISSION_DECIDED' ORDER BY id DESC LIMIT 1",
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
            p.route(),
        )
        .await
        .unwrap();
    std::fs::write(p.0.join("work.txt"), "after").unwrap();
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
    submit_result(&h, &review, "drift", &o, &r, json!({"report":"# Review\n\nLooks right.","summary":"Approved.","checks":[],"verdict":"APPROVE","request_key":"review"})).await;
    review.shutdown().await.unwrap();
    let root = p.0.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        std::fs::write(root.join("work.txt"), "changed while verifying").unwrap();
    });
    let refused = h.advance("drift".into(), o.clone()).await.unwrap_err();
    writer.await.unwrap();
    assert_eq!(refused.code(), "verification_review_revision_changed");
    p.store()
        .read(|db| {
            let passed: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_PASSED'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(passed, 0);
            let state: String = db.query_row(
                "SELECT state FROM runtime_tasks WHERE task_id='drift'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(state, "VERIFYING");
            let ran: i64 = db.query_row(
                "SELECT count(*) FROM runtime_requests WHERE operation='native_command_output'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(ran, 1);
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
        let prompt = format!(
            "call:{}",
            json!({"name":name,"arguments":args,"__fixture_expect":expected})
        );
        assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, stop, "{name}");
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
    // A project or skill read, a write and a recursive batch are not batchable reads:
    // the advertised item schema refuses them before anything runs.
    for tool in ["project_read", "read_skill", "project_write", "read_batch"] {
        checked(
            &d,
            "read_batch",
            json!({"calls":[{"id":"x","tool":tool,"arguments":{}}]}),
            json!({"/error/code":-32602,"/error/data/code":"invalid_arguments"}),
            "refusal",
        )
        .await;
    }
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
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
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
        json!({"/result/structuredContent/reason":"program_missing"}),
        "end_turn",
    )
    .await;
    // A batched worker question keeps each member's topic label with that question.
    checked(
        &w,
        "ask_question",
        json!({"request_key":"topics","questions":[
        {"question":"Run paid checks?","options":["Yes","No"],"topic":"Paid checks"},
        {"question":"Which model?","options":["Small","Large"]}]}),
        json!({"/result/isError":false,"/result/structuredContent/questions/0/topic":"Paid checks",
            "/result/structuredContent/questions/1/question":"Which model?"}),
        "end_turn",
    )
    .await;
    let topic_kept = s.read(|db| Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id='t' AND payload_json LIKE '%\"topic\":\"Paid checks\"%'", [], |r| r.get::<_, i64>(0))?)).unwrap();
    assert!(topic_kept >= 1, "the wait record keeps the topic");
    w.shutdown().await.unwrap();

    // Auto answers the deletion and replacement cards itself.
    s.write(|db| {
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n2','worker-backend-dev','More work','[]','[\"src\"]','execution','running','b',NULL)", [])?;
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
    checked(
        &w,
        "project_delete",
        json!({"path":"src/file.txt","expected_sha256":file_sha,"request_key":"del"}),
        json!({"/result/isError":false,"/result/structuredContent/deleted":true}),
        "end_turn",
    )
    .await;
    assert!(!p.0.join("src/file.txt").exists());
    // The retained receipt answers an identical retry; nothing is deleted twice.
    checked(
        &w,
        "project_delete",
        json!({"path":"src/file.txt","expected_sha256":file_sha,"request_key":"del"}),
        json!({"/result/structuredContent/deleted":true}),
        "end_turn",
    )
    .await;
    std::fs::write(p.0.join(".gitignore"), "target/\n").unwrap();
    let ignore_sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"target/\n"));
    checked(&w, "project_delete", json!({"path":".gitignore","expected_sha256":ignore_sha}),
        json!({"/result/structuredContent/deleted":false,"/result/structuredContent/gitignore_guard":true}), "end_turn").await;
    assert_eq!(
        std::fs::read_to_string(p.0.join(".gitignore")).unwrap(),
        "/.cortex-dev/\n"
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
fn main() {
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
    tokio::runtime::Runtime::new().unwrap().block_on(async {
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
        // This executable serially exercises all scenarios, including full
        // before/after project observations for evidence-bearing commands.
        tokio::time::timeout(Duration::from_secs(90), async {
            channel_source_project_and_commands().await;
            attachments_reach_the_engine_as_readable_blocks().await;
            claude_permission_requests_follow_runtime_policy().await;
            owner_cards_stay_pending_and_record_the_owner().await;
            desktop_permission_decisions().await;
            read_only_channels_start_restricted_in_every_access_mode().await;
            reopened_synthesis_feedback().await;
            access_modes_and_outside_routes().await;
            route_effort_starts_codex_and_claude_coordinators().await;
            document_tools_through_engine_and_relay().await;
            configured_external_worker_grants().await;
            optional_external_failures_withhold_only_that_server().await;
            uncontained_external_server_runs_with_owner_authority().await;
            native_workspace_binding_projects_activity_and_scoped_skills().await;
            project_skills_read_through_the_channel().await;
            recovery_project_operations_and_structured_tool_errors().await;
            legacy_task_admission_and_session_lifetime_fence().await;
            coordinator_tools_not_chat_and_lease().await;
            engine_model_name_is_recorded_with_the_configured_session().await;
            worker_question_answer_exact_delivery_and_native_denial().await;
            pause_resume_does_not_revive_old_coordinator_and_cancel_is_not_completion().await;
            startup_failure_settles_only_owned_reaped_process().await;
            launch_refusal_before_spawn_is_a_confirmed_absence().await;
            codex_launcher_sessions_hand_relays_to_app_server_not_acp().await;
            claude_update_behind_session_new_reply_never_fails_configure().await;
            configure_transport_loss_starts_once_more_and_records_both_attempts().await;
            repeated_configure_transport_loss_stops_with_the_roles_blocker().await;
            inline_result_is_published_once_bound_to_attempt_and_reconciled_after_restart().await;
            result_failure_and_review_refusal_are_not_acceptance().await;
            native_plan_review_verification_journey(false, false, false, false).await;
            native_plan_review_verification_journey(true, false, false, false).await;
            native_plan_review_verification_journey(false, true, false, false).await;
            native_plan_review_verification_journey(false, false, true, false).await;
            native_plan_review_verification_journey(false, false, false, true).await;
            plain_folder_task_runs_its_whole_lifecycle().await;
            verification_drift_blocks_the_pass().await;
        })
        .await
        .expect("native session scenarios exceeded deadline");
    });
    if !["synthesis-feedback", "engine-modes", "plain-folder"].contains(&mode.as_str()) {
        println!("native source/project/command, read-only, Git and access/outside scenarios asserted with existing session regressions (fake ACP/relay processes, disposable stores; unavailable verification is reported above)");
    }
}

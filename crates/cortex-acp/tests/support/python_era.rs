//! Python-produced project archives. The runtime never invokes Python.
use super::{json, Client, Path, PathBuf, Value};
use cortex_core::{store::documents::Publisher, OpenMode, ProjectKey, Store, StoreChannel};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
};

const ROOT: &str = "/tmp/cortex-python-era-fixture-v1";
const SESSION: &str = "9b40b9a3-1de9-4a1f-a7ba-444af198d95c";
const ARCHIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/python-era"
);
const SNAPSHOT: &[(&str, &str)] = &[
    ("tasks", "SELECT json_array(id,project_root,created_at) FROM tasks ORDER BY id"),
    ("reports", "SELECT json_array(id,task_id,kind,filename) FROM reports ORDER BY id"),
    ("editions", "SELECT json_array(sequence,report_id,title,summary,author,digest,size_bytes) FROM editions ORDER BY sequence"),
    ("drafts", "SELECT json_array(id,task_id,owner_thread_id,kind,template,path,created_at,published_report_id) FROM drafts ORDER BY id"),
    ("defaults", "SELECT json_array(project_root,json(defaults),json(ceiling),updated_at) FROM project_configuration_defaults ORDER BY project_root"),
    ("sessions", "SELECT json_array(session_id,project_root,task_id,json(client_info),json(client_capabilities),json(mcp_servers),created_at,closed_at) FROM acp_host_sessions WHERE session_id='9b40b9a3-1de9-4a1f-a7ba-444af198d95c' ORDER BY session_id"),
    ("transcript", "SELECT json_array(session_id,seq,kind,text,json(attachments_json),created_at) FROM host_transcript ORDER BY session_id,seq"),
    ("runtime_tasks", "SELECT json_array(task_id,state,goal) FROM runtime_tasks ORDER BY task_id"),
];

struct PrivateRoot(PathBuf);

impl PrivateRoot {
    fn new() -> Self {
        let root = PathBuf::from(ROOT);
        // Never reuse or remove a preexisting path. Only one run can own this
        // exact DB identity per filesystem namespace.
        private_dir(&root);
        Self(root)
    }

    fn install(&self, version: u32, store: &str) -> PathBuf {
        let root = self.0.join(format!("format-{version}"));
        if !root.exists() {
            private_dir(&root);
        }
        let destination = root.join(store);
        let parent = destination.parent().unwrap();
        if parent != root && !parent.exists() {
            private_dir(parent);
        }
        copy_tree(&source(version), &destination);
        root
    }
}

impl Drop for PrivateRoot {
    fn drop(&mut self) {
        let result = fs::remove_dir_all(&self.0);
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|e| panic!("cannot exclusively create {}: {e}", path.display()));
}

fn source(version: u32) -> PathBuf {
    Path::new(ARCHIVE).join(format!("format-{version}/store"))
}

fn copy_tree(source: &Path, destination: &Path) {
    private_dir(destination);
    for entry in fs::read_dir(source).unwrap().map(Result::unwrap) {
        let path = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &path);
        } else {
            assert!(kind.is_file(), "{}", entry.path().display());
            use std::io::Write;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap()
                .write_all(&fs::read(entry.path()).unwrap())
                .unwrap();
        }
    }
}

fn expected(version: u32) -> Value {
    serde_json::from_slice(
        &fs::read(Path::new(ARCHIVE).join(format!("format-{version}/expected.json"))).unwrap(),
    )
    .unwrap()
}

fn verify_store(store: &Store, version: u32, after_acp: bool) {
    let expected = expected(version);
    let recovering_task = expected["runtime_tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|task| task[2] == "Historical unfinished goal")
        .and_then(|task| task[0].as_str());
    for &(label, sql) in SNAPSHOT {
        if expected.get(label).is_none() {
            assert_eq!((version, label), (11, "runtime_tasks"));
            continue;
        }
        let rows = store
            .read(|db| {
                Ok(db
                    .prepare(sql)?
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap_or_else(|e| panic!("format {version} {label}: {e:?}"));
        let actual = Value::Array(
            rows.into_iter()
                .map(|text| serde_json::from_str(&text).unwrap())
                .collect(),
        );
        if after_acp && matches!(label, "reports" | "editions" | "drafts") {
            // Shared startup can publish a new runtime pipeline for the
            // unfinished task. Every historical row must still match.
            let (historical, additions): (Vec<_>, Vec<_>) =
                actual.as_array().unwrap().iter().partition(|row| {
                    expected[label]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|old| old[0] == row[0])
                });
            assert_eq!(json!(historical), expected[label], "historical {label}");
            if label == "reports" {
                assert!(additions.len() <= 1);
            }
            for row in additions {
                let task_id = recovering_task.expect("unexpected recovery in format 11");
                match label {
                    "reports" => assert_eq!(
                        &row.as_array().unwrap()[1..],
                        &[json!(task_id), json!("pipeline"), json!("pipeline.md")]
                    ),
                    "drafts" => assert_eq!(
                        &row.as_array().unwrap()[1..5],
                        &[
                            json!(task_id),
                            json!("runtime"),
                            json!("pipeline"),
                            json!("pipeline")
                        ]
                    ),
                    "editions" => {
                        assert_eq!(row[4], "runtime");
                        let report_id = row[1].as_str().unwrap();
                        let matches: i64 = store.read(|db| {
                            Ok(db.query_row("SELECT count(*) FROM reports WHERE id=? AND task_id=? AND kind='pipeline'",
                                [report_id, task_id], |row| row.get(0))?)
                        }).unwrap();
                        assert_eq!(matches, 1);
                    }
                    _ => unreachable!(),
                }
            }
        } else if after_acp && label == "runtime_tasks" {
            let retained_id_and_goal = |rows: &Value| {
                rows.as_array()
                    .unwrap()
                    .iter()
                    .map(|row| json!([row[0], row[2]]))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                retained_id_and_goal(&actual),
                retained_id_and_goal(&expected[label])
            );
            // Recovery may fence the unfinished task; it must not reopen the cancelled task.
            assert_eq!(
                actual
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row[2] == "Historical terminal goal")
                    .unwrap()[1],
                "CANCELLED"
            );
        } else {
            assert_eq!(actual, expected[label], "format {version} {label}");
        }
    }

    let publisher = Publisher::new(store);
    let reports = expected["reports"].as_array().unwrap();
    for task in expected["tasks"].as_array().unwrap() {
        let task_id = task[0].as_str().unwrap();
        let catalog = publisher.list(task_id, None, 10).unwrap();
        assert!(catalog.next_cursor.is_none());
        let matching: Vec<_> = reports
            .iter()
            .filter(|report| report[1] == task[0])
            .collect();
        let additions: Vec<_> = catalog
            .reports
            .iter()
            .filter(|report| !matching.iter().any(|old| old[0] == report.report_id))
            .collect();
        assert_eq!(catalog.reports.len(), matching.len() + additions.len());
        if after_acp && recovering_task == Some(task_id) {
            assert!(additions.len() <= 1);
            for report in additions {
                assert_eq!((&*report.kind, &*report.author), ("pipeline", "runtime"));
            }
        } else {
            assert!(additions.is_empty());
        }
        for report in matching {
            let id = report[0].as_str().unwrap();
            let filename = report[3].as_str().unwrap();
            assert!(catalog
                .reports
                .iter()
                .any(|metadata| metadata.report_id == id && metadata.kind == report[2]));
            let page = publisher.read_report(task_id, id, None, 4000).unwrap();
            assert!(page.next_cursor.is_none());
            assert_eq!(
                page.markdown.as_bytes(),
                fs::read(source(version).join(task_id).join(filename)).unwrap()
            );
        }
    }

    for row in expected["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row[7].is_null())
    {
        let id = row[0].as_str().unwrap();
        let task = row[1].as_str().unwrap();
        let owner = row[2].as_str().unwrap();
        assert_eq!(publisher.own_drafts(task, owner, "", 10).unwrap(), [id]);
        let filename = format!("{id}.md");
        let archived = fs::read(source(version).join("draft-reports").join(&filename)).unwrap();
        if store.key().channel() == StoreChannel::Release {
            let page = publisher.read_draft(task, owner, id, None, 1000).unwrap();
            assert!(page.next_cursor.is_none());
            assert_eq!(page.markdown.as_bytes(), archived);
        } else {
            // The unmodified Python Release draft row still names .cortex.
            // Check the copied body and index in Dev, not cross-channel draft access.
            assert_eq!(
                fs::read(store.key().directory().join("draft-reports").join(filename)).unwrap(),
                archived
            );
        }
    }

    verify_settings(store.key(), version, &expected["defaults"][0]);
}

fn verify_settings(key: &ProjectKey, version: u32, expected: &Value) {
    use workspace_service::{
        config_read_model::ConfigurationReader, native_routes::RouteEnvironment,
        settings_api::read_project_ceiling,
    };
    // Explicit isolated host roots: no existing owner policy or catalog.
    let home = key.root().parent().unwrap().join("settings-home");
    let env = RouteEnvironment {
        config: home.join("config"),
        state: home.join("state"),
        home,
        routing_override: None,
    };
    let project =
        read_project_ceiling(&env, key.channel().directory(), &json!(key.root())).unwrap();
    assert_eq!(project["exists"], true);
    assert_eq!(project["project_root"], expected[0]);
    assert_eq!(project["project_defaults"], expected[1]);
    assert_eq!(project["ceiling"], expected[2]);

    let routing = key.directory().join("routing.json");
    if version == 12 {
        assert_eq!(
            fs::read(&routing).unwrap(),
            fs::read(source(version).join("routing.json")).unwrap()
        );
        let observed = ConfigurationReader::new(env)
            .unwrap()
            .routing_management(Some((key.root(), &routing)))
            .unwrap();
        assert_eq!(observed["effective_source"], "project");
        assert_eq!(observed["effective_valid"], true);
        assert_eq!(
            observed["effective_rules"]["deny"],
            json!(["fixture-disabled-model"])
        );
    } else {
        assert!(!routing.exists());
    }
}

fn current_channel() -> StoreChannel {
    if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    }
}

fn release_key(root: &Path) -> ProjectKey {
    ProjectKey::new(root, StoreChannel::Release).unwrap()
}

fn version_at(path: &Path) -> u32 {
    u32::from_be_bytes(fs::read(path).unwrap()[60..64].try_into().unwrap())
}

fn verify_migration(store: &Store, original_version: u32) {
    assert_eq!(
        store
            .read(|db| Ok(db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?))
            .unwrap(),
        12
    );
    let backups = store.key().directory().join(".migration-backups");
    if original_version == 12 {
        assert!(!backups.exists());
        return;
    }
    let files: Vec<_> = fs::read_dir(backups).unwrap().map(Result::unwrap).collect();
    assert_eq!(files.len(), 1);
    let backup = files[0].path();
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(version_at(&backup), 11);
    store.read(|db| {
        assert_eq!(db.query_row(
            "SELECT count(*) FROM format_history WHERE action='migrate' AND from_version=11 AND to_version=12 AND backup_path=? AND length(backup_digest)=64",
            [backup.to_str().unwrap()],
            |row| row.get::<_, i64>(0),
        )?, 1);
        Ok(())
    }).unwrap();
}

async fn load(client: &mut Client, root: &Path, id: i64) -> Value {
    client
        .send(
            id,
            "session/load",
            json!({"cwd":root,"sessionId":SESSION,"mcpServers":[]}),
        )
        .await;
    client.response(id).await
}

async fn acp_roundtrip(root: &Path, version: u32) {
    let key = ProjectKey::new(root, current_channel()).unwrap();
    let mut client = Client::new();
    client.initialize("scripted-client").await;
    if version == 11 {
        let before = fs::read(key.database()).unwrap();
        let reply = load(&mut client, root, 2).await;
        assert_eq!(
            reply["error"]["data"]["reason"], "legacy_storage_migration_required",
            "{reply}"
        );
        assert_eq!(fs::read(key.database()).unwrap(), before);
        assert!(!key.directory().join(".migration-backups").exists());
    }

    client
        .send(3, "session/new", json!({"cwd":root,"mcpServers":[]}))
        .await;
    let reply = client.response(3).await;
    let new_session = reply["result"]["sessionId"].as_str().unwrap_or_else(|| {
        panic!(
            "format {version} session/new: {reply}; post-refusal header {}, backup directory {}",
            version_at(&key.database()),
            key.directory().join(".migration-backups").exists()
        )
    });
    assert_ne!(new_session, SESSION);
    assert!(!new_session.is_empty());
    let store = Store::open(key, OpenMode::LiveReadOnly).unwrap();
    verify_migration(&store, version);
    verify_store(&store, version, true);
    drop(store);

    let reply = load(&mut client, root, 4).await;
    assert!(reply.get("error").is_none(), "{reply}");
    let replay: Vec<_> = client
        .updates
        .iter()
        .filter(|frame| {
            frame["method"] == "session/update" && frame["params"]["sessionId"] == SESSION
        })
        .filter_map(|frame| {
            let update = &frame["params"]["update"];
            update["content"]["text"]
                .as_str()
                .map(|text| (update["sessionUpdate"].as_str().unwrap(), text))
        })
        .collect();
    assert_eq!(
        replay,
        [
            ("user_message_chunk", "Historical question"),
            ("agent_message_chunk", "Historical Python answer"),
        ]
    );
    client
        .send(
            5,
            "session/prompt",
            json!({"sessionId":SESSION,"prompt":[{"type":"text","text":"Must not resume"}]}),
        )
        .await;
    super::assert_legacy_refusal(client.response(5).await);

    fs::write(root.join("scenario.txt"), "incoming-legacy-open").unwrap();
    client.send(6, "session/prompt", json!({"sessionId":new_session,"prompt":[{"type":"text","text":"Can this new session answer?"}]})).await;
    let reply = client.response(6).await;
    assert_eq!(reply["result"]["stopReason"], "end_turn", "{reply}");
    let text = client
        .updates
        .iter()
        .filter(|frame| {
            frame["params"]["sessionId"] == new_session
                && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
        })
        .filter_map(|frame| frame["params"]["update"]["content"]["text"].as_str())
        .collect::<String>();
    assert!(
        text.contains("The new session can answer without changing the old project."),
        "{text:?}"
    );
    client.close().await;
    let store = Store::open(
        ProjectKey::new(root, current_channel()).unwrap(),
        OpenMode::LiveReadOnly,
    )
    .unwrap();
    store
        .read(|db| {
            let completed: i64 = db.query_row(
            "SELECT count(*) FROM acp_host_sessions s JOIN runtime_tasks t ON s.task_id=t.task_id
             WHERE s.session_id=? AND t.state='COMPLETED'
               AND t.synthesis='The new session can answer without changing the old project.'",
            [new_session],
            |row| row.get(0),
        )?;
            assert_eq!(completed, 1);
            let old_messages: i64 = db.query_row(
                "SELECT count(*) FROM host_transcript WHERE session_id=?",
                [SESSION],
                |row| row.get(0),
            )?;
            assert_eq!(old_messages, 2, "read-only prompt must not append history");
            Ok(())
        })
        .unwrap();
}

fn profile_controls() {
    use workspace_service::config_read_model::parse_profiles;
    for version in [1, 2] {
        let bytes =
            fs::read(Path::new(ARCHIVE).join(format!("profiles/format-{version}.json"))).unwrap();
        let input: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(input["format"], version);
        let output = parse_profiles(Some(&input)).unwrap();
        assert_eq!(output["document"]["format"], 2);
        assert_eq!(output["document"]["active"], input["active"]);
        let name = format!("historical-v{version}");
        let profile = &output["document"]["profiles"][&name];
        assert_eq!(
            profile["description"],
            input["profiles"][&name]["description"]
        );
        if version == 1 {
            assert_eq!(output["migrated_from"], 1);
            assert_eq!(profile["sections"], json!(["routing"]));
            assert_eq!(
                profile["settings"]["routing"],
                input["profiles"][&name]["rules"]
            );
        } else {
            assert!(output["migrated_from"].is_null());
            assert_eq!(profile["sections"], input["profiles"][&name]["sections"]);
            assert_eq!(
                profile["settings"]["routing"],
                input["profiles"][&name]["settings"]["routing"]
            );
        }
    }
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap().map(Result::unwrap) {
            let path = entry.path();
            let ty = entry.file_type().unwrap();
            if ty.is_dir() {
                walk(root, &path, files);
            } else {
                assert!(ty.is_file());
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn refusal_code(key: ProjectKey, mode: OpenMode) -> &'static str {
    match Store::open(key, mode) {
        Ok(_) => panic!("unexpectedly opened a refusal control"),
        Err(error) => error.code(),
    }
}

async fn rejection_controls(fixture: &PrivateRoot) {
    let wrong = fixture.0.join("wrong-project");
    private_dir(&wrong);
    copy_tree(&source(12), &wrong.join(".cortex"));
    assert_eq!(
        refusal_code(release_key(&wrong), OpenMode::Initialize),
        "project_storage_mismatch"
    );

    // Same Python payload in the layout recognized by Python's one-way move.
    let root = fixture.install(12, ".codex/cortex");
    let legacy = root.join(".codex/cortex");
    let before = tree(&legacy);
    assert_eq!(
        refusal_code(release_key(&root), OpenMode::Initialize),
        "legacy_storage_root_detected"
    );
    assert!(!root.join(".cortex").exists());
    let dev = Store::open(
        ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
        OpenMode::Initialize,
    )
    .unwrap();
    assert_eq!(
        dev.read(|db| Ok(db.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))?))
            .unwrap(),
        0
    );
    drop(dev);
    assert_eq!(tree(&legacy), before);
    assert!(!root.join(".cortex").exists());
    fs::remove_dir_all(root.join(".cortex-dev")).unwrap();
    fs::remove_dir_all(root.join(".codex")).unwrap();

    let channel = current_channel();
    for (version, explanation) in [
        (10u32, "format 10 from the retired Python Cortex runtime"),
        (99, "does not recognize this database"),
    ] {
        let root = fixture.install(12, channel.directory());
        let key = ProjectKey::new(&root, channel).unwrap();
        // Change only user_version on a disposable copy; no producer claim.
        let mut bytes = fs::read(key.database()).unwrap();
        bytes[60..64].copy_from_slice(&version.to_be_bytes());
        fs::write(key.database(), bytes).unwrap();
        let before = tree(&key.directory());
        assert_eq!(
            refusal_code(key.clone(), OpenMode::Initialize),
            "unsupported_storage"
        );
        let mut client = Client::new();
        client.initialize("scripted-client").await;
        client
            .send(2, "session/new", json!({"cwd":root,"mcpServers":[]}))
            .await;
        for reply in [client.response(2).await, load(&mut client, &root, 3).await] {
            assert_eq!(reply["error"]["code"], -32602);
            assert_eq!(
                reply["error"]["data"],
                json!({"reason":"unsupported_storage","retryable":false})
            );
            assert!(
                reply["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(explanation),
                "{reply}"
            );
        }
        client.close().await;
        assert!(!key.directory().join(".migration-backups").exists());
        let after = tree(&key.directory());
        for (path, bytes) in &before {
            assert!(
                after.get(path) == Some(bytes),
                "format {version} refusal changed an existing file: {}",
                path.display()
            );
        }
        // Live header probing and the fenced writer may leave empty lock/WAL
        // files or an SQLite shared-memory index. No other new file is allowed.
        for (path, bytes) in after.iter().filter(|(path, _)| !before.contains_key(*path)) {
            match path.to_str().unwrap() {
                ".core-owner.lock" | ".scheduler-owner.lock" | "cortex.sqlite3-wal" => {
                    assert!(bytes.is_empty(), "non-empty {}", path.display());
                }
                "cortex.sqlite3-shm" => {}
                _ => panic!("format {version} refusal created {}", path.display()),
            }
        }
        fs::remove_dir_all(key.directory()).unwrap();
    }
}

fn direct_store(fixture: &PrivateRoot, version: u32) {
    let root = fixture.install(version, ".cortex");
    let key = release_key(&root);
    assert_eq!(version_at(&key.database()), version);
    if version == 11 {
        let before = tree(&key.directory());
        assert_eq!(
            refusal_code(key.clone(), OpenMode::InspectImmutable),
            "legacy_storage_migration_required"
        );
        assert!(
            tree(&key.directory()) == before,
            "immutable inspection wrote storage"
        );
    }
    let store = Store::open(key.clone(), OpenMode::Initialize).unwrap_or_else(|e| {
        panic!("format {version} Store::Initialize: {e:?}; post-refusal header {}, backup directory {}",
            version_at(&key.database()), key.directory().join(".migration-backups").exists())
    });
    verify_migration(&store, version);
    verify_store(&store, version, false);
    drop(store);
    fs::remove_dir_all(key.directory()).unwrap();
    println!("Python-produced format {version}: store, documents, and retained rows passed");
}

pub(super) async fn run(selection: &str) {
    let fixture = PrivateRoot::new();
    let all = matches!(selection, "" | "python-era");
    if all || selection == "python-era-controls" {
        profile_controls();
        rejection_controls(&fixture).await;
        println!(
            "Python-era fixture: profiles, project binding, channel isolation and refusals passed"
        );
        if !all {
            return;
        }
        fs::remove_dir_all(fixture.0.join("format-12")).unwrap();
    }
    for version in [12, 11] {
        if all || selection == format!("python-era-store-{version}") {
            direct_store(&fixture, version);
        }
        if all || selection == format!("python-era-acp-{version}") {
            let root = fixture.install(version, current_channel().directory());
            acp_roundtrip(&root, version).await;
            println!(
                "Python-produced format {version}: {:?} incoming ACP passed",
                current_channel()
            );
        }
    }
}

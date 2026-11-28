//! Real Git authority and an offline ACP peer, with no provider calls.
use super::*;
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::DirBuilderExt, path::Path};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cortex-worktree-exec-{}",
            crate::identity().unwrap()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) {
    let run = std::process::Command::new("/usr/bin/git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
fn repo(f: &Fixture) -> (ProjectKey, PathBuf) {
    let root = f.0.join("project");
    let trees = f.0.join("checkouts");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&trees).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("source.txt"), "committed").unwrap();
    git(&root, &["add", "source.txt"]);
    git(&root, &["commit", "-qm", "initial"]);
    (
        ProjectKey::new(&root, cortex_core::StoreChannel::Dev).unwrap(),
        trees,
    )
}

fn provision(
    store: &cortex_core::Store,
    trees: &Path,
    suffix: &str,
) -> CoreResult<(String, OwnerEpoch, PathBuf)> {
    use cortex_core::workspace::{create_draft, start_native_worktree, worktrees::resolve};
    let name = format!("Worktree {suffix}");
    let draft = create_draft(
        store,
        "p",
        "owner",
        &json!({
            "protocol":{"major":1,"minor":8},"requestId":format!("draft-{suffix}"),
            "projectId":"p","actorId":"owner","operation":"tasks.create","target":"p","expectedVersion":"0",
            "ownerEpoch":null,"payload":{"description":name,"checkout":{"kind":"worktree"}}
        }),
    )?;
    let value = start_native_worktree(
        store,
        "p",
        "owner",
        &json!({
            "protocol":{"major":1,"minor":8},"requestId":format!("start-{suffix}"),
            "projectId":"p","actorId":"owner","operation":"tasks.start","target":draft.receipt["target"],
            "expectedVersion":"1","ownerEpoch":null,"payload":{}
        }),
        trees,
    )?;
    let task = value["task"].as_str().unwrap().to_owned();
    let owner = OwnerEpoch {
        owner: "owner".into(),
        epoch: value["owner"]["epoch"].as_i64().unwrap(),
    };
    let root = resolve(store, &task, &owner)?.path().to_path_buf();
    Ok((task, owner, root))
}

fn register(store: &cortex_core::Store) -> CoreResult<()> {
    store.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
            [store.key().root().to_str()],
        )?;
        Ok(())
    })
}

fn worker(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<()> {
    db.execute(
        "UPDATE runtime_tasks SET state='EXECUTING', revision=1 WHERE task_id=?",
        [task],
    )?;
    db.execute("INSERT INTO runtime_nodes VALUES (?,1,'writer','worker-backend-dev','Work','[]','[\"source.txt\",\"blob.bin\",\"bad.bin\",\"linked.txt\"]','execution','running','fixture-assignment',NULL)", [task])?;
    Ok(())
}

#[test]
fn worktree_file_effects_require_checkout_bound_coverage_and_receipts() {
    use crate::native_project;
    let f = Fixture::new();
    let (key, trees) = repo(&f);
    let store = cortex_core::Store::open(key.clone(), cortex_core::OpenMode::Initialize).unwrap();
    register(&store).unwrap();
    let (task, owner, root) = provision(&store, &trees, "files").unwrap();
    let (_, _, sibling) = provision(&store, &trees, "other").unwrap();
    let (credential, identity) = store
        .write(|db| {
            worker(db, &task)?;
            let c = channels::issue_tx(
                db,
                &task,
                &owner,
                Principal::Worker {
                    assignment: "fixture-assignment",
                },
            )?;
            channels::bind_session_tx(db, &c, "fake-session")?;
            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
            let hash = format!("{:x}", Sha256::digest(b"committed"));
            // The same bytes, on the same channel, are not proof of a worktree read
            // when the record carries no worktree identity.
            let coverage = json!({"complete":true,"observed":true,"sha256":hash,"lines":[1]});
            db.execute(
                "INSERT INTO runtime_requests VALUES (?, ?, ?, 'native_file_read', ?, ?)",
                (
                    &task,
                    format!("file-read:{:x}", Sha256::digest(b"source.txt")),
                    &c.channel_id,
                    hash,
                    coverage.to_string(),
                ),
            )?;
            Ok((c, i))
        })
        .unwrap();
    let invoke = |tool, args| {
        native_project::execute(
            &store,
            &identity,
            &credential,
            tool,
            &args,
            false,
            &root,
            true,
        )
    };
    // Absolute paths are host reads, even when the contents match the bound
    // checkout. Neither the main checkout nor a sibling read grants coverage.
    for checkout in [key.root(), sibling.as_path()] {
        let read = invoke(
            "project_read",
            json!({"path":checkout.join("source.txt"),"line_numbers":false}),
        )
        .unwrap();
        assert_eq!(read["structuredContent"]["host"], true);
        assert_eq!(read["structuredContent"]["content"], "committed\n");
    }
    let denied = invoke(
        "project_write",
        json!({
            "path":"source.txt",
            "content":"not authorized by a host read",
            "expected_sha256":format!("{:x}", Sha256::digest(b"committed")),
        }),
    )
    .unwrap();
    assert_eq!(denied["isError"], true);
    assert_eq!(
        denied["structuredContent"]["error"],
        "gateway_complete_read_required"
    );
    assert_eq!(
        fs::read_to_string(root.join("source.txt")).unwrap(),
        "committed"
    );

    let edit = json!({"path":"source.txt","edits":[{"old_string":"committed","new_string":"edited worktree"}]});
    // A failed match returns the current page inline and that read is bound to this checkout.
    let miss = invoke(
        "project_edit",
        json!({"path":"source.txt","edits":[{"old_string":"absent text","new_string":"x"}]}),
    )
    .unwrap();
    assert_eq!(miss["structuredContent"]["code"], "gateway_edit_not_found");
    assert_eq!(
        miss["structuredContent"]["current_read"]["sha256"],
        format!("{:x}", Sha256::digest(b"committed"))
    );
    for path in [
        ".git",
        ".cortex/private",
        ".cortex-dev/private",
        "../source.txt",
    ] {
        assert!(
            invoke("project_read", json!({"path":path})).is_err(),
            "{path}"
        );
    }
    assert_eq!(
        invoke(
            "project_read",
            json!({"path":"source.txt","line_numbers":false})
        )
        .unwrap()["structuredContent"]["content"],
        "committed"
    );
    let mut edit = edit;
    edit["request_key"] = json!("write");
    assert_eq!(
        invoke("project_edit", edit.clone()).unwrap()["structuredContent"]["outcome"],
        "written"
    );
    let binding = native_project::checkout_binding(&store, &identity)
        .unwrap()
        .unwrap();
    assert!(native_project::retained_checkout_matches(&store, &task, &binding).unwrap());
    let mut wrong = binding.clone();
    wrong["root"] = json!(key.root());
    assert!(!native_project::retained_checkout_matches(&store, &task, &wrong).unwrap());
    let saved: Value = store
        .read(|db| {
            let value: String = db.query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key='write'",
                [&task],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&value)?)
        })
        .unwrap();
    assert_eq!(saved["checkout"], binding);
    assert_eq!(
        invoke("project_edit", edit).unwrap()["structuredContent"]["outcome"],
        "written"
    );
    assert_eq!(
        fs::read_to_string(root.join("source.txt")).unwrap(),
        "edited worktree"
    );
    assert_eq!(
        fs::read_to_string(key.root().join("source.txt")).unwrap(),
        "committed"
    );
    assert_eq!(
        fs::read_to_string(sibling.join("source.txt")).unwrap(),
        "committed"
    );

    // Idempotent edit: an old_string equal to new_string that is present succeeds
    // without writing.
    let same = invoke(
        "project_edit",
        json!({"path":"source.txt","edits":[{"old_string":"edited","new_string":"edited"}]}),
    )
    .unwrap();
    assert_eq!(same["structuredContent"]["outcome"], "unchanged");
    assert_eq!(same["isError"], false);
    // A stale read does not block an edit whose old_string still matches uniquely.
    fs::write(root.join("source.txt"), "edited worktree, changed outside").unwrap();
    let applied = invoke(
        "project_edit",
        json!({"path":"source.txt","edits":[{"old_string":"worktree","new_string":"tree"}]}),
    )
    .unwrap();
    assert_eq!(applied["structuredContent"]["outcome"], "written");
    assert_eq!(
        fs::read_to_string(root.join("source.txt")).unwrap(),
        "edited tree, changed outside"
    );
    // Binary write: base64 of the exact bytes, same receipts as a text write.
    use base64::Engine as _;
    let blob = [0u8, 1, 2, 255, 254, 0];
    let encoded = base64::engine::general_purpose::STANDARD.encode(blob);
    let created = invoke(
        "project_write",
        json!({"path":"blob.bin","content":encoded,"encoding":"base64","expected_sha256":"absent","request_key":"bin"}),
    )
    .unwrap();
    assert_eq!(created["structuredContent"]["outcome"], "written");
    assert_eq!(created["structuredContent"]["encoding"], "base64");
    assert_eq!(created["structuredContent"]["size_bytes"], blob.len());
    assert_eq!(
        created["structuredContent"]["sha256"],
        format!("{:x}", Sha256::digest(blob))
    );
    assert_eq!(fs::read(root.join("blob.bin")).unwrap(), blob);
    let receipt: Value = store
        .read(|db| {
            let value: String = db.query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key='bin'",
                [&task],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&value)?)
        })
        .unwrap();
    assert_eq!(receipt["complete"], true);
    assert_eq!(receipt["reply"]["structuredContent"]["encoding"], "base64");
    // Replaying the keyed write returns the retained receipt and writes nothing.
    let replay = invoke(
        "project_write",
        json!({"path":"blob.bin","content":encoded,"encoding":"base64","expected_sha256":"absent","request_key":"bin"}),
    )
    .unwrap();
    assert_eq!(replay["structuredContent"]["outcome"], "written");
    // Overwriting a binary file needs its identity read, then replaces the bytes.
    let read = invoke("project_read", json!({"path":"blob.bin"})).unwrap();
    assert_eq!(read["structuredContent"]["binary"], true);
    let sha = read["structuredContent"]["sha256"].clone();
    let replaced = invoke(
        "project_write",
        json!({"path":"blob.bin","content":base64::engine::general_purpose::STANDARD.encode([9u8, 0, 8]),"encoding":"base64","expected_sha256":sha}),
    )
    .unwrap();
    assert_eq!(replaced["structuredContent"]["outcome"], "written");
    assert_eq!(fs::read(root.join("blob.bin")).unwrap(), [9u8, 0, 8]);
    // Malformed base64 changes nothing.
    let bad = invoke(
        "project_write",
        json!({"path":"bad.bin","content":"***","encoding":"base64","expected_sha256":"absent"}),
    )
    .unwrap();
    assert_eq!(bad["isError"], true);
    assert!(!root.join("bad.bin").exists());
    // A hard-linked file reads, and a write replaces the directory entry without
    // touching the other link's inode.
    let external = f.0.join("external.txt");
    fs::write(&external, "shared").unwrap();
    fs::hard_link(&external, root.join("linked.txt")).unwrap();
    let linked = invoke("project_read", json!({"path":"linked.txt"})).unwrap();
    assert_eq!(linked["isError"], false);
    assert_eq!(
        linked["structuredContent"]["sha256"],
        format!("{:x}", Sha256::digest(b"shared"))
    );
    let edited = invoke(
        "project_edit",
        json!({"path":"linked.txt","edits":[{"old_string":"shared","new_string":"private"}]}),
    )
    .unwrap();
    assert_eq!(edited["structuredContent"]["outcome"], "written");
    assert_eq!(fs::read_to_string(root.join("linked.txt")).unwrap(), "private");
    assert_eq!(fs::read_to_string(&external).unwrap(), "shared");
    use std::os::unix::fs::MetadataExt;
    assert_eq!(fs::metadata(root.join("linked.txt")).unwrap().nlink(), 1);
}

fn have_namespace() -> bool {
    let available = crate::native_sandbox::abi() >= 1;
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(available, "worktree fixtures require Landlock");
    }
    available
}

// Return the observed process cwd only if it matches ACP session/new cwd. A linked
// route starts the engine in its worktree; like every native engine it is not
// confined (owner decision), so it sees the main checkout too.
const PEER: &str = r#"
cwd=$(pwd -P)
if [ "$1" = linked ]; then
    logical=${2#logical=}
    [ -e "$logical/source.txt" ] || exit 21
fi
options='[{"id":"model","currentValue":"fake","options":[{"value":"fake"}]},{"id":"mode","currentValue":"default","options":[{"value":"default"}]}]'
while IFS= read -r frame; do
    id=$(printf '%s\n' "$frame" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$frame" in
       *'"method":"initialize"'*) result='{"protocolVersion":1}';;
       *'"method":"session/new"'*)
          case "$frame" in *"\"cwd\":\"$cwd\""*) ;; *) exit 24;; esac
          result=$(printf '{"sessionId":"%s","configOptions":%s}' "$cwd" "$options");;
       *'"method":"session/set_config_option"'*) result=$(printf '{"configOptions":%s}' "$options");;
       *) continue;;
    esac
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#;

#[test]
fn escalation_gateway_returns_the_committed_receipt() {
    let fixture = Fixture::new();
    let (key, _) = repo(&fixture);
    let root = key.root().to_path_buf();
    let broker_parent = key.directory();
    let peer = fixture.0.join("fake-acp.sh");
    fs::write(&peer, PEER).unwrap();
    let home = fixture.0.join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    let host = NativeHost::open(key).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
    let task = "consultation";
    let owner = host.create(task.into(), "Consultation fixture".into(), "owner".into()).await.unwrap();
    receive(host.executor.write(move |db| worker(db, task)).unwrap()).await.unwrap();
    let route = SessionRoute {
        launch: LaunchSpec {
            engine: cortex_engine::environment::EngineKind::Claude,
            executable: "/usr/bin/bash".into(),
            args: vec![peer.display().to_string(), "main".into()],
            cwd: root.clone(),
            native_home: home.clone(),
            credential_directory: home.join(".claude"),
            base_environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            codex_launcher: None,
            confinement: None,
        },
        relay: std::env::current_exe().unwrap(),
        broker_parent,
        connection: "fixture".into(),
        connection_limit: 2,
        model: "fake".into(),
        parameters: Default::default(),
        instructions: "Offline fixture only".into(),
        deadline: Duration::from_secs(5),
    };
    let driver = host.dispatch(
        task.into(), owner,
        SessionRole::Worker { assignment: "fixture-assignment".into() },
        route,
    ).await.unwrap();
    let credential = &driver.credential;
    let gateway = Gateway::new(
        driver.policy.clone(),
        Credentials::new(credential.channel_id.clone(), root, credential.secret().into()).unwrap(),
    );
    let reply = gateway.handle(json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"escalation_request","arguments":{
                "reason":"Choose whether to keep or revise the approach. The available fixture evidence does not establish which is sound.",
                "question":"Should we keep or revise the current approach?",
                "report_ids":[],"request_key":"escalate"
            }
        }
    })).await.unwrap();
    assert_eq!(reply["id"], 2);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(reply["result"]["structuredContent"], json!({
        "accepted":true,"decision":"senior_consultation","scope_granted":false,"state":"RECOVERING"
    }));
    assert_eq!(host.state(task.into()).await.unwrap(), TaskState::Recovering);
    driver.shutdown().await.unwrap();
    });
}

#[test]
fn worktree_native_cwd_and_command_mounts_with_real_namespace() {
    if !have_namespace() {
        return;
    }
    let f = Fixture::new();
    let (key, trees) = repo(&f);
    let logical = key.root().to_path_buf();
    let home = f.0.join("home");
    let state = f.0.join("state");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir(&state).unwrap();
    let peer = f.0.join("fake-acp.sh");
    fs::write(&peer, PEER).unwrap();
    let broker = key.directory();
    let mut host = NativeHost::open(key).unwrap();
    host.workspace_state = Some(state);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ((task, owner, root), (_, _, sibling)) = receive(
            host.executor
                .publication(move |store| {
                    register(store)?;
                    Ok((
                        provision(store, &trees, "engine")?,
                        provision(store, &trees, "sibling")?,
                    ))
                })
                .unwrap(),
        )
        .await
        .unwrap();
        let route = |mode: &str| SessionRoute {
            launch: LaunchSpec {
                engine: cortex_engine::environment::EngineKind::Claude,
                executable: "/usr/bin/bash".into(),
                args: vec![
                    peer.display().to_string(),
                    mode.into(),
                    format!("logical={}", logical.display()),
                    format!("sibling={}", sibling.display()),
                ],
                cwd: logical.clone(),
                native_home: home.clone(),
                credential_directory: home.join(".claude"),
                base_environment: [("PATH".into(), "/usr/bin:/bin".into())].into(),
                codex_launcher: None,
                confinement: None,
            },
            relay: std::env::current_exe().unwrap(),
            broker_parent: broker.clone(),
            connection: "fake-acp".into(),
            connection_limit: 2,
            model: "fake".into(),
            parameters: Default::default(),
            instructions: "Offline fixture only".into(),
            deadline: Duration::from_secs(5),
        };
        let mut unsupported = route("linked");
        unsupported.launch.engine = cortex_engine::environment::EngineKind::OpenCode;
        assert!(matches!(
            host.dispatch(
                task.clone(),
                owner.clone(),
                SessionRole::Coordinator,
                unsupported
            )
            .await,
            Err(CoreError::Refused("worktree_engine_route_unavailable"))
        ));
        let session = host
            .dispatch(
                task.clone(),
                owner.clone(),
                SessionRole::Coordinator,
                route("linked"),
            )
            .await
            .unwrap();
        assert_eq!(session.session_id(), root.to_str().unwrap());
        assert_eq!(session.policy.logical, logical);
        let creds = Credentials::new(
            session.credential.channel_id.clone(),
            logical.clone(),
            session.credential.secret().into(),
        )
        .unwrap();
        let channel = session.policy.authenticate(&creds).await.unwrap();
        let read = session
            .policy
            .dispatch_call(
                &creds,
                &channel,
                "project_read",
                json!({"path":"source.txt","line_numbers":false}),
                false,
            )
            .await
            .unwrap();
        assert_eq!(read["structuredContent"]["content"], "committed");
        session.shutdown().await.unwrap();
        let main_owner = host
            .create("main".into(), "Main fixture".into(), "owner".into())
            .await
            .unwrap();
        let main = host
            .dispatch(
                "main".into(),
                main_owner,
                SessionRole::Coordinator,
                route("main"),
            )
            .await
            .unwrap();
        assert_eq!(main.session_id(), logical.to_str().unwrap());
        assert!(main.policy.worktree.is_none());
        main.shutdown().await.unwrap();

        let assigned_owner = host
            .create("isolated".into(), "Isolated writer fixture".into(), "owner".into())
            .await
            .unwrap();
        let assignment_dir = f.0.join("assignments");
        fs::DirBuilder::new().mode(0o700).create(&assignment_dir).unwrap();
        let o = assigned_owner.clone();
        let isolated = receive(host.executor.publication(move |store| {
            store.write(|db| worker(db, "isolated"))?;
            cortex_core::workspace::worktrees::assignments::provision(
                store, "isolated", &o, "fixture-assignment", &assignment_dir,
            )
        }).unwrap()).await.unwrap();
        let isolated_session = host.dispatch(
            "isolated".into(), assigned_owner,
            SessionRole::Worker { assignment: "fixture-assignment".into() },
            route("linked"),
        ).await.unwrap();
        assert_eq!(isolated_session.session_id(), isolated.path().to_str().unwrap());
        let creds = Credentials::new(isolated_session.credential.channel_id.clone(), logical.clone(),
            isolated_session.credential.secret().into()).unwrap();
        let channel = isolated_session.policy.authenticate(&creds).await.unwrap();
        isolated_session.policy.dispatch_call(
            &creds, &channel, "project_read", json!({"path":"source.txt","line_numbers":false}), false,
        ).await.unwrap();
        let written = isolated_session.policy.dispatch_call(
            &creds, &channel, "project_edit",
            json!({"path":"source.txt","edits":[{"old_string":"committed","new_string":"isolated"}]}),
            false,
        ).await.unwrap();
        assert_eq!(written["structuredContent"]["outcome"], "written", "{written}");
        assert_eq!(fs::read_to_string(isolated.path().join("source.txt")).unwrap(), "isolated");
        assert_eq!(fs::read_to_string(logical.join("source.txt")).unwrap(), "committed");
        isolated_session.shutdown().await.unwrap();

        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        // Source is read from the physical checkout and the optional cwd is its
        // actual subdirectory. Private .git control remains read-only.
        let sandbox =
            crate::native_command_sandbox::Sandbox::prepare_worktree(&root, &logical, &[]).unwrap();
        let argv = vec![
            "/bin/bash".into(),
            "-ec".into(),
            r#"
            [ "$(pwd -P)" = "$1" ]
            [ "$(cat ../source.txt)" = committed ]
            [ -f "$2/source.txt" ] && [ -f "$3/source.txt" ] && [ -r /etc/hostname ] && ! (printf bad > "$3/source.txt")
            ! (printf bad > "$2/source.txt")
            for f in "$2"/.cortex/* "$2"/.cortex-dev/*; do [ -f "$f" ] || continue; ! cat "$f" >/dev/null 2>&1 || exit 7; done
            ! (printf bad > ../source.txt) && ! (printf bad > ../.git)
            [ "$(git -C .. rev-parse --show-toplevel)" = "$4" ]
            printf good > "$TMPDIR/cortex-check-output"
        "#
            .into(),
            "mounts".into(),
            sub.display().to_string(),
            logical.display().to_string(),
            sibling.display().to_string(),
            root.display().to_string(),
        ];
        let out = sandbox
            .command_at(&argv, Some(&sub))
            .unwrap()
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(sandbox.command_at(&argv, Some(&logical)).is_err());
        assert!(sandbox.command_at(&argv, Some(&sibling)).is_err());
        assert_eq!(
            fs::read_to_string(root.join("source.txt")).unwrap(),
            "committed"
        );
        fs::rename(&root, root.with_extension("moved")).unwrap();
        assert!(matches!(
            host.dispatch(task, owner, SessionRole::Coordinator, route("linked"))
                .await,
            Err(CoreError::Refused("worktree_unavailable"))
        ));
    });
}

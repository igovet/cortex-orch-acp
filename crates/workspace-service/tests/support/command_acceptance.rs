//! No-model acceptance against the real broker, supervisor and verifier.
use super::{call, close, Project};
use serde_json::{json, Value};
use std::{
    fs,
    os::fd::AsRawFd,
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::UnixListener,
    },
    path::Path,
    sync::Arc,
};
use tokio::sync::{mpsc, oneshot};
use workspace_service::native_sessions::{ClientPermissions, SessionDriver, SessionRole};

const SENTINEL: &str = "synthetic-private-input";
const CHECK_BODY: &str = r#"
                socket=$1
                shift
                # The owner's agent socket stays at its real path, as on the host.
                test -S "$socket"
                test "$(cat src/.env.example)" = ordinary
                for secret do
                    # Landlock refuses the read; an open failure contains no private bytes.
                    test -z "$(cat "$secret" 2>/dev/null || :)" || exit 31
                done
                # Read-only commands cannot write the checkout.
                if (: > checkout-output) 2>/dev/null; then exit 32; fi
                case "$TMPDIR" in /*) ;; *) exit 33;; esac
                cc src/main.c -o "$TMPDIR/fixture"
                "$TMPDIR/fixture"
            "#;
pub(super) const OUTPUT: &str = "contained-compiler-ran\n";
// Landlock hides Cortex control storage and the project's secret files, not
// arbitrary project or host inputs.
const PRIVATE: &[&str] = &[".cortex/fixture-private", ".cortex-dev/fixture-private", ".env"];

fn agent_address(directory: &fs::File) -> std::path::PathBuf {
    format!("/proc/self/fd/{}/agent", directory.as_raw_fd()).into()
}

pub(super) struct Inputs {
    outside: Project,
    socket: UnixListener,
}

impl Inputs {
    // Called before the harness creates any threads.
    pub(super) fn new() -> Self {
        let outside = Project::new_at(&std::env::temp_dir(), "cw-command-inputs");
        fs::write(outside.0.join("secret"), SENTINEL).unwrap();
        let path = outside.0.join("agent");
        let directory = fs::File::open(&outside.0).unwrap();
        let socket = UnixListener::bind(agent_address(&directory)).unwrap();
        std::env::set_var("SSH_AUTH_SOCK", path);
        Self { outside, socket }
    }

    pub(super) fn prepare(&self, root: &Path) {
        for store in [".cortex", ".cortex-dev"] {
            if !root.join(store).exists() {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(root.join(store))
                    .unwrap();
            }
        }
        fs::create_dir(root.join("src")).unwrap();
        for path in PRIVATE {
            fs::write(root.join(path), SENTINEL).unwrap();
        }
        fs::write(root.join("src/.env.example"), "ordinary").unwrap();
        fs::write(
            root.join("src/main.c"),
            "#include <stdio.h>\nint main(void) { return puts(\"contained-compiler-ran\") < 0; }\n",
        )
        .unwrap();

        // Falsification control: the denied files are readable and the socket
        // is connectable from the fixture host before entering containment.
        self.unchanged(root);
        let directory = fs::File::open(&self.outside.0).unwrap();
        std::os::unix::net::UnixStream::connect(agent_address(&directory)).unwrap();
        self.socket.accept().unwrap();
    }

    pub(super) fn argv(&self) -> Vec<String> {
        let mut args = vec![
            "/bin/sh".into(),
            "-ec".into(),
            CHECK_BODY.into(),
            "containment-fixture".into(),
            self.outside
                .0
                .join("agent")
                .into_os_string()
                .into_string()
                .unwrap(),
        ];
        args.extend(PRIVATE.iter().map(|path| (*path).into()));
        args
    }

    pub(super) fn unchanged(&self, root: &Path) {
        for path in PRIVATE {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), SENTINEL);
        }
        assert_eq!(
            fs::read_to_string(self.outside.0.join("secret")).unwrap(),
            SENTINEL
        );
        for denied in [
            root.join("target"),
            root.join("checkout-output"),
            self.outside.0.join("escaped"),
        ] {
            assert!(!denied.exists(), "unexpected fixture effect at {denied:?}");
        }
    }
}

pub(super) const VERIFIER_OUTPUT: &str = "live-verifier-compiler-ran\n";

pub(super) struct VerifierInputs {
    outside: Project,
}

impl VerifierInputs {
    pub(super) fn new() -> Self {
        let outside = Project::new_at(&std::env::temp_dir(), "cw-live-verifier-input");
        fs::write(outside.0.join("input"), "benign-host-input").unwrap();
        Self { outside }
    }

    pub(super) fn prepare(&self, root: &Path) {
        fs::write(root.join(".gitignore"), "/target/\n").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join(".cortex-dev/fixture-private"), SENTINEL).unwrap();
        fs::write(
            root.join("fixture.c"),
            "#include <stdio.h>\nint main(void) { return puts(\"live-verifier-compiler-ran\") < 0; }\n",
        )
        .unwrap();

        // Plan verification accepts literal argv; run the boundary checks in a script.
        let script = root.join("verify-fixture");
        fs::write(
            &script,
            r#"#!/bin/sh -e
test "$(cat "$1")" = benign-host-input
test -z "$(cat .cortex-dev/fixture-private 2>/dev/null || :)"
# A non-ignored write is refused by Landlock.
if (: > checkout-output) 2>/dev/null; then exit 32; fi
cc fixture.c -o target/fixture
./target/fixture
"#,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    }

    pub(super) fn argv(&self) -> Vec<String> {
        vec![
            "./verify-fixture".into(),
            self.outside.0.join("input").into_os_string().into_string().unwrap(),
        ]
    }

    pub(super) fn assert_effects(&self, root: &Path) {
        assert!(
            root.join("target/fixture").is_file(),
            "ignored build output must persist"
        );
        assert!(
            !root.join("checkout-output").exists(),
            "non-ignored output must not persist"
        );
        assert_eq!(
            fs::read_to_string(root.join(".cortex-dev/fixture-private")).unwrap(),
            SENTINEL
        );
    }
}

async fn checked(d: &SessionDriver, name: &str, args: &Value, expected: Value) {
    let stop = if expected.get("/error/data/code").is_some() {
        "refusal"
    } else {
        "end_turn"
    };
    let prompt = format!(
        "call:{}",
        json!({"name":name,"arguments":args,"__fixture_expect":expected})
    );
    assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, stop);
}

fn running_worker(p: &Project, incoming: bool) {
    running_worker_in(p, incoming, "worker-backend-dev", "auto");
}

fn running_worker_in(p: &Project, incoming: bool, profile: &str, mode: &str) {
    p.store().write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n',?,'Work','[]','[\"src\"]','execution','running','a',NULL)", [profile])?;
        if incoming {
            db.execute("INSERT INTO acp_host_sessions VALUES ('fixture-client',?,'t','{\"name\":\"cortex-rust-acp\"}','{}','[]','2026-11-17T00:00:00Z',NULL)", [p.0.to_str().unwrap()])?;
        }
        cortex_core::access::switch_tx(db, "t", mode, "fixture", "acp_set_mode")
    }).unwrap();
}

fn receipt(p: &Project, key: &str) -> Value {
    p.store()
        .read(|db| {
            let raw: String = db.query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id='t' AND request_key=?",
                [key],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&raw)?)
        })
        .unwrap()
}

pub(super) async fn contained(inputs: &Inputs, managed: bool) {
    let p = Project::new();
    let h = p.host().await;
    let owner = h
        .create("t".into(), "Contained command".into(), "owner".into())
        .await
        .unwrap();
    inputs.prepare(&p.0);
    println!("synthetic files are host-readable and the SSH socket is connectable");
    // Both routes use the read-only profile; Guarded also denies outside cwd.
    running_worker_in(&p, false, "worker-explorer", "guarded");
    let mut route = if managed { p.route() } else { p.legacy_route() };
    let account = p.0.join("fixture-account");
    fs::create_dir(&account).unwrap();
    route.launch.credential_directory = account;
    let d = h
        .dispatch(
            "t".into(),
            owner,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            route,
        )
        .await
        .unwrap();
    println!("fake engine connected to the command broker");
    let outside = json!({"argv":["/usr/bin/touch","escaped"],"cwd":inputs.outside.0,"request_key":"outside-cwd"});
    let denied = json!({"/error/code":-32001,"/error/data/code":"outside_cwd_denied"});
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        checked(&d, "command_run", &outside, denied),
    )
    .await
    .expect("outside cwd refusal did not return");
    assert!(!inputs.outside.0.join("escaped").exists());
    println!("outside cwd refused before execution");

    let argv = inputs.argv();
    let prompt = call(
        "command_run",
        json!({"argv":argv,"request_key":"compile","timeout_seconds":20}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(75), d.prompt(&prompt))
        .await
        .expect("compiler command returned no prompt outcome")
        .unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(24), async {
        loop {
            let row = receipt(&p, "compile");
            if row["settled"] == true {
                break row;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("command did not settle");
    assert!(observed["rerouted_to"].is_null(), "{observed}");
    inputs.unchanged(&p.0);
    d.shutdown().await.unwrap();
    close(h).await;

    // The supervised terminal receipt has no `launched` field. A complete,
    // successful receipt with the compiler's exact output proves execution.
    assert_eq!(observed["complete"], true, "{observed}");
    assert_eq!(observed["output_truncated"], false, "{observed}");
    assert_eq!(observed["exit_code"], 0, "{observed}");
    assert_eq!(observed["profile"], "read_only", "{observed}");
    assert_eq!(observed["output"], OUTPUT, "{observed}");
    println!(
        "{}: compiler executed in private output; hidden files and outside cwd",
        if managed {
            "managed Codex"
        } else {
            "ordinary broker"
        }
    );
}

// This is the capability contract for the parity candidate, not a replacement
// for the dedicated containment/verifier tests. All inputs and outputs are fake.
pub(super) async fn role_mode_matrix(allow_live: bool) {
    use cortex_engine::environment::EngineKind;

    for engine in [EngineKind::Claude, EngineKind::OpenCode, EngineKind::Codex] {
        for (mode, profile, allowed) in [
            ("development", "worker-backend-dev", true),
            ("auto", "worker-backend-dev", true),
            ("guarded", "worker-backend-dev", false),
            ("development", "worker-explorer", false),
            ("auto", "worker-explorer", true),
            ("guarded", "worker-explorer", false),
        ] {
            if allowed != allow_live {
                continue;
            }
            let p = Project::new();
            let outside = Project::new();
            // This sibling is outside the command checkout and the command's
            // own private scratch. Only the host route may write there.
            let host_target = (profile == "worker-explorer" && mode != "guarded").then(Project::new);
            let output = host_target
                .as_ref()
                .map_or_else(|| p.0.join("src/output"), |target| target.0.join("output"));
            fs::create_dir(p.0.join("src")).unwrap();
            fs::write(outside.0.join("input"), SENTINEL).unwrap();
            let h = p.host().await;
            let owner = h
                .create("t".into(), "Command role/mode matrix".into(), "owner".into())
                .await
                .unwrap();
            // A real client binding ensures that an unexpected permission request
            // would be observable rather than silently having no card recipient.
            running_worker_in(&p, true, profile, mode);
            let (cards, mut inbox) = mpsc::unbounded_channel();
            h.bind_permissions(Arc::new(Owner(cards)));
            let mut route = p.route();
            route.launch.engine = engine;
            route.launch.credential_directory = p.0.join(if engine == EngineKind::OpenCode {
                "opencode"
            } else {
                "fixture-account"
            });
            fs::create_dir(&route.launch.credential_directory).unwrap();
            let d = h
                .dispatch(
                    "t".into(),
                    owner,
                    SessionRole::Worker { assignment: "a".into() },
                    route,
                )
                .await
                .unwrap();
            let args = json!({
                "argv": [
                    "/bin/sh", "-ec",
                    "test \"$(pwd -P)\" = \"$1\"; /bin/cat ./input >> \"$2\" || exit 24",
                    "role-mode-fixture", outside.0, output
                ],
                "cwd":outside.0, "request_key":"live-checkout", "timeout_seconds":10
            });
            let scenario = format!("{engine:?}/{mode}/{profile}");
            println!("command capability matrix: {scenario}");
            // Auto runs every role unsandboxed on the host as the owner (receipt of the host route).
            let auto = mode == "auto";
            let current = || -> Value {
                if auto { receipt(&p, "live-checkout.host")["reply"]["structuredContent"].clone() } else { receipt(&p, "live-checkout") }
            };
            // Outside cwd is mode-dependent; it does not grant a read-only role writes outside Auto.
            if mode != "guarded" {
                let prompt = call("command_run", args);
                assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, "end_turn");
                let row = tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    loop {
                        let row = current();
                        if row["settled"] == true {
                            break row;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                }).await.expect("matrix command did not settle");
                let reference = row["tool_call_ref"].as_str().expect("settled command reference");
                let finished: i64 = p.store().read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE task_id='t'
                     AND type='NATIVE_COMMAND_FINISHED'
                     AND json_extract(payload_json,'$.receipt.tool_call_ref')=?",
                    [reference],
                    |r| r.get(0),
                )?)).unwrap();
                assert_eq!(finished, 1, "{scenario}: no unique finished receipt");
                assert_eq!(row["complete"], true, "{scenario}: {row}");
                assert_eq!(row["profile"], if auto { "outside_confirmed" } else if allowed { "development" } else { "read_only" });
                assert_eq!(row["exit_code"], if allowed { 0 } else { 24 }, "{scenario}: {row}");
                if allowed {
                    assert_eq!(fs::read_to_string(&output).unwrap(), SENTINEL, "{scenario}");
                } else {
                    assert!(!output.exists(), "{scenario}: read-only command wrote outside its checkout and scratch");
                }

                // An exact retry after settlement must return the retained
                // result, not start another preparation or append again.
                let preparations = || -> i64 {
                    p.store()
                        .read(|db| {
                            Ok(db.query_row(
                                "SELECT count(*) FROM runtime_events
                                 WHERE task_id='t' AND type='NATIVE_COMMAND_PREPARATION'",
                                [],
                                |r| r.get(0),
                            )?)
                        })
                        .unwrap()
                };
                let before_retry = preparations();
                assert_eq!(d.prompt(&prompt).await.unwrap().stop_reason, "end_turn");
                assert_eq!(preparations(), before_retry, "{scenario}: retry prepared a command");
                assert_eq!(current(), row, "{scenario}: receipt changed");
                if allowed {
                    assert_eq!(fs::read_to_string(&output).unwrap(), SENTINEL, "{scenario}");
                } else {
                    assert!(!output.exists(), "{scenario}: replay wrote to the checkout");
                }
            } else {
                checked(
                    &d,
                    "command_run",
                    &args,
                    json!({"/error/data/code":"outside_cwd_denied"}),
                )
                .await;
                assert!(!output.exists(), "{scenario}: denied payload changed the checkout");
            }
            assert!(inbox.try_recv().is_err(), "{scenario}: unexpected permission card");
            assert_eq!(fs::read_to_string(outside.0.join("input")).unwrap(), SENTINEL);
            d.shutdown().await.unwrap();
            close(h).await;
        }
    }
}

// Exercise the owner-approved Auto contract separately from the existing
// manual-consent regression. Neither command sandbox nor historical Git is needed.
pub(super) async fn auto_outside_matrix() {
    use cortex_engine::environment::EngineKind;

    for engine in [EngineKind::Claude, EngineKind::OpenCode, EngineKind::Codex] {
        let p = Project::new();
        let outside = Project::new();
        let h = p.host().await;
        let owner = h
            .create("t".into(), "Auto outside action matrix".into(), "owner".into())
            .await
            .unwrap();
        running_worker(&p, true);
        let (cards, mut inbox) = mpsc::unbounded_channel();
        h.bind_permissions(Arc::new(Owner(cards)));
        let mut route = p.route();
        route.launch.engine = engine;
        route.launch.credential_directory = p.0.join(if engine == EngineKind::OpenCode {
            "opencode"
        } else {
            "fixture-account"
        });
        fs::create_dir(&route.launch.credential_directory).unwrap();
        let d = h
            .dispatch(
                "t".into(),
                owner,
                SessionRole::Worker {
                    assignment: "a".into(),
                },
                route,
            )
            .await
            .unwrap();

        let file = outside.0.join("written");
        let count = outside.0.join("count");
        let requests = [
            (
                "outside_write",
                json!({"path":file,"content":SENTINEL,"request_key":"auto-write"}),
                json!({"/result/isError":false}),
            ),
            (
                "outside_command",
                json!({
                    "argv":["/bin/sh","-ec","printf run >> count"],
                    "cwd":outside.0,"request_key":"auto-command"
                }),
                json!({"/result/isError":false,
                       "/result/structuredContent/profile":"outside_confirmed",
                       "/result/structuredContent/exit_code":0}),
            ),
        ];
        for (name, args, expected) in &requests {
            for _ in 0..2 {
                tokio::select! {
                    biased;
                    Some(_) = inbox.recv() => panic!("{engine:?}/{name}: Auto asked the owner"),
                    () = checked(&d, name, args, expected.clone()) => {}
                }
                assert!(inbox.try_recv().is_err(), "{engine:?}/{name}: unexpected card");
            }
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), SENTINEL, "{engine:?}");
        assert_eq!(fs::read_to_string(&count).unwrap(), "run", "{engine:?}");

        let decisions = p
            .store()
            .read(|db| {
                let mut stmt = db.prepare(
                    "SELECT payload_json FROM runtime_events
                     WHERE type='NATIVE_OUTSIDE_SETTLED' ORDER BY id",
                )?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(decisions.len(), requests.len(), "{engine:?}");
        for (decision, (name, args, _)) in decisions.iter().zip(&requests) {
            let decision: Value = serde_json::from_str(decision).unwrap();
            assert_eq!(decision["rule"], *name, "{engine:?}");
            assert_eq!(decision["request_key"], args["request_key"], "{engine:?}");
            assert_eq!(decision["allowed"], true, "{engine:?}");
            assert_eq!(decision["decider"], "mode:auto", "{engine:?}");
        }
        d.shutdown().await.unwrap();
        close(h).await;
    }
}

type Card = (String, Value, oneshot::Sender<&'static str>);
struct Owner(mpsc::UnboundedSender<Card>);
impl ClientPermissions for Owner {
    fn decide<'a>(
        &'a self,
        session: &'a str,
        request: &'a Value,
    ) -> cortex_engine::FutureResult<'a, cortex_engine::engine::PermissionDecision> {
        Box::pin(async move {
            let (reply, answer) = oneshot::channel();
            self.0
                .send((session.to_owned(), request.clone(), reply))
                .unwrap();
            Ok(cortex_engine::engine::PermissionDecision::Selected(
                answer.await.unwrap().into(),
            ))
        })
    }
}

pub(super) async fn consent() {
    for mode in ["guarded", "development"] {
        consent_in(mode).await;
    }
}

async fn consent_in(mode: &str) {
    let p = Project::new();
    let outside = Project::new();
    let result = outside.0.join("count");
    let h = p.host().await;
    let (cards, mut inbox) = mpsc::unbounded_channel();
    h.bind_permissions(Arc::new(Owner(cards)));
    let owner = h
        .create("t".into(), "Outside consent".into(), "owner".into())
        .await
        .unwrap();
    running_worker_in(&p, true, "worker-backend-dev", mode);
    let d = h
        .dispatch(
            "t".into(),
            owner,
            SessionRole::Worker {
                assignment: "a".into(),
            },
            p.legacy_route(),
        )
        .await
        .unwrap();
    let content = || {
        fs::read_to_string(&result).unwrap_or_else(|e| {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            String::new()
        })
    };
    for (key, choice, before, after) in [
        ("approve-first", "allow", "", "run\n"),
        ("deny-second", "deny", "run\n", "run\n"),
        ("approve-third", "allow", "run\n", "run\nrun\n"),
    ] {
        let args = json!({
            "argv":["/bin/sh","-ec","printf 'run\\n' >> count"],
            "cwd":outside.0,"request_key":key
        });
        let expect = if choice == "allow" {
            json!({"/result/isError":false,
                "/result/structuredContent/profile":"outside_confirmed",
                "/result/structuredContent/exit_code":0})
        } else {
            json!({"/result/isError":true,
                "/result/content/0/text":"outside_action_not_confirmed"})
        };
        let turn = checked(&d, "outside_command", &args, expect.clone());
        let decide = async {
            let (session, card, answer) = inbox.recv().await.unwrap();
            assert_eq!(session, "fixture-client");
            assert_eq!(card["toolCall"]["toolCallId"], key);
            assert_eq!(card["toolCall"]["rawInput"], args);
            assert!(card["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option["optionId"] == "allow" && option["kind"] == "allow_once"));
            assert_eq!(content(), before, "payload ran before its own consent");
            assert_eq!(receipt(&p, key)["complete"], false);
            answer.send(choice).unwrap();
        };
        tokio::join!(turn, decide);
        assert_eq!(content(), after);
        // Exact retry is a retained outcome, not a new execution or permission request.
        checked(&d, "outside_command", &args, expect).await;
        assert!(inbox.try_recv().is_err());
        assert_eq!(content(), after);
    }
    p.store().read(|db| {
        let mut stmt = db.prepare("SELECT json_extract(payload_json,'$.request_key'), json_extract(payload_json,'$.allowed'), json_extract(payload_json,'$.mode') FROM runtime_events WHERE type='NATIVE_OUTSIDE_SETTLED' ORDER BY id")?;
        let decisions = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(decisions, [
            ("approve-first".into(), true, mode.into()),
            ("deny-second".into(), false, mode.into()),
            ("approve-third".into(), true, mode.into()),
        ]);
        Ok(())
    }).unwrap();
    d.shutdown().await.unwrap();
    close(h).await;
    println!("{mode}: three independent owner decisions, no effects before consent or after denial, retries did not execute");
}

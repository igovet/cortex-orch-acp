use cortex_engine::{
    broker::Broker,
    gateway::{Channel, Credentials, Gateway, GatewayCore},
    schema, wire, Error, FutureResult,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::UnixStream,
};
struct Core {
    restricted: AtomicBool,
    live: AtomicBool,
    calls: AtomicUsize,
}
fn credentials() -> Credentials {
    Credentials::new(
        "ch_123456abcdef".into(),
        std::env::current_dir().unwrap(),
        "fixture-token-only-0123456789".into(),
    )
    .unwrap()
}
fn channel() -> Channel {
    Channel {
        task: "fixture-task".into(),
        attempt: "fixture-attempt".into(),
        owner_epoch: 7,
        principal: "fixture-worker".into(),
        role: "worker".into(),
        profile: Some("worker-backend-dev".into()),
        stage: Some("execution".into()),
    }
}
impl GatewayCore for Core {
    fn supports_channel(&self, channel: &Channel, name: &str) -> bool {
        !self.restricted.load(Ordering::SeqCst)
            || channel.stage.as_deref() == Some("execution") && name == "project_read"
    }
    fn authenticate<'a>(&'a self, c: &'a Credentials) -> FutureResult<'a, Channel> {
        Box::pin(async move {
            if !self.live.load(Ordering::SeqCst) || c.secret() != "fixture-token-only-0123456789" {
                return Err(Error::Forbidden);
            }
            Ok(channel())
        })
    }
    fn execute<'a>(
        &'a self,
        c: &'a Credentials,
        _: &'a Channel,
        name: &'a str,
        args: Value,
        _: Value,
    ) -> FutureResult<'a, Value> {
        Box::pin(async move {
            self.authenticate(c).await?;
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(name, "project_read");
            Ok(json!({"content":[{"type":"text","text":args["path"]}]}))
        })
    }
}
fn core() -> Arc<Core> {
    Arc::new(Core {
        restricted: AtomicBool::new(false),
        live: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    })
}
#[tokio::test]
async fn advertised_schemas_enforce_channel_and_ignore_forged_meta() {
    let c = core();
    let g = Gateway::new(c.clone(), credentials());
    let list = g
        .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap();
    assert!(list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "result_submit"));
    for (id, args) in [
        (2, json!({"path":"Cargo.toml","unexpected":true})),
        (3, json!({"path":""})),
        (4, json!({"path":"Cargo.toml"})),
    ] {
        let r=g.handle(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"project_read","arguments":args,"_meta":{"role":"coordinator","owner_epoch":999}}})).await.unwrap();
        assert_eq!(r.get("result").is_some(), id == 4);
    }
    assert_eq!(c.calls.load(Ordering::SeqCst), 1);
    assert!(g.handle(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"project_read","arguments":{"path":"Cargo.toml"}}})).await.is_none());
    assert_eq!(c.calls.load(Ordering::SeqCst), 1);
    c.live.store(false, Ordering::SeqCst);
    assert!(g
        .handle(json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}))
        .await
        .unwrap()
        .get("error")
        .is_some());
    assert!(g.handle(json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"project_read","arguments":{"path":"Cargo.toml"}}})).await.unwrap().get("error").is_some());
}
#[tokio::test]
async fn channel_restriction_filters_discovery_and_calls() {
    let c = core();
    c.restricted.store(true, Ordering::SeqCst);
    let g = Gateway::new(c.clone(), credentials());
    let listing = g
        .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap();
    let tools = listing["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "project_read");
    let refused=g.handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"result_submit","arguments":{"report":"# Not allowed","summary":"No","checks":[],"request_key":"r"}}})).await.unwrap();
    assert!(refused.get("error").is_some());
    // A tool outside this channel's catalogue is a refusal that names its reason.
    assert_eq!(refused["error"]["code"], -32001);
    assert_eq!(refused["error"]["data"]["code"], "tool_not_offered");
    assert_eq!(
        refused["error"]["message"],
        "authority_refused: tool_not_offered"
    );
    // Schema-invalid arguments name the code and point at the live schema.
    let invalid = g
        .handle(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"project_read","arguments":{"path":"a","invented":1}}}))
        .await
        .unwrap();
    assert_eq!(invalid["error"]["code"], -32602);
    assert_eq!(invalid["error"]["data"]["code"], "invalid_arguments");
    assert_eq!(c.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn a_named_core_refusal_keeps_authority_and_its_code() {
    struct Refusing;
    impl GatewayCore for Refusing {
        fn authenticate<'a>(&'a self, _: &'a Credentials) -> FutureResult<'a, Channel> {
            Box::pin(async { Ok(channel()) })
        }
        fn execute<'a>(
            &'a self,
            _: &'a Credentials,
            _: &'a Channel,
            _: &'a str,
            _: Value,
            _: Value,
        ) -> FutureResult<'a, Value> {
            Box::pin(async { Err(Error::Refused("write_denied")) })
        }
    }
    let g = Gateway::new(Arc::new(Refusing), credentials());
    let r = g
        .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"project_read","arguments":{"path":"src"}}}))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], -32001);
    assert_eq!(r["error"]["data"]["code"], "write_denied");
    assert_eq!(r["error"]["data"]["retryable"], false);
    assert!(r["error"]["data"]["hint"]
        .as_str()
        .unwrap()
        .contains("protected"));
}
/// Each tool definition is keyed by the sha256 of its canonical JSON (object keys
/// sorted, no whitespace), and every catalogue entry names an existing key. A
/// definition edited in place must be re-keyed, together with its catalogue
/// references; otherwise the key no longer identifies the advertised contract.
#[test]
fn tool_definition_keys_are_the_sha256_of_their_content() {
    use sha2::{Digest, Sha256};
    fn canonical(v: &Value) -> String {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<_> = m.keys().collect();
                keys.sort();
                let fields: Vec<String> = keys
                    .into_iter()
                    .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&m[k])))
                    .collect();
                format!("{{{}}}", fields.join(","))
            }
            Value::Array(a) => format!(
                "[{}]",
                a.iter().map(canonical).collect::<Vec<_>>().join(",")
            ),
            other => other.to_string(),
        }
    }
    let d: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    let tools = d["tools"].as_object().unwrap();
    for (key, tool) in tools {
        let digest = format!("{:x}", Sha256::digest(canonical(tool).as_bytes()));
        assert_eq!(
            &digest, key,
            "tool {} is keyed {key} but its content hashes to {digest}; re-key it and its catalogue references",
            tool["name"]
        );
    }
    for (name, ids) in d["catalogues"].as_object().unwrap() {
        for id in ids.as_array().unwrap() {
            assert!(
                tools.contains_key(id.as_str().unwrap()),
                "catalogue {name} names unknown tool key {id}"
            );
        }
    }
}
#[test]
fn catalogues_all_profiles_review_and_no_legacy_mutation_route() {
    let d: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    assert_eq!(d["catalogues"].as_object().unwrap().len(), 47);
    assert!(schema::catalogue("untrusted", None, None).is_err());
    assert!(schema::catalogue("worker", Some("invented"), None).is_err());
    for key in d["catalogues"].as_object().unwrap().keys() {
        let p: Vec<_> = key.split('|').collect();
        let tools =
            schema::catalogue(p[0], (!p[1].is_empty()).then_some(p[1]), Some(p[2])).unwrap();
        assert!(tools.iter().any(|t| t["name"] == "create_draft"));
        assert!(!tools.iter().any(|t| matches!(
            t["name"].as_str(),
            Some("delegate" | "accept" | "complete_task")
        )));
    }
    let s = json!({"type":"object","required":["a"],"additionalProperties":false,"properties":{"a":{"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"string","minLength":2,"maxLength":3,"pattern":"^[a-zé]+$"}}}});
    assert!(schema::validate(&s, &json!({"a":["éé"]})).is_ok());
    for value in [
        json!({}),
        json!({"a":["a"]}),
        json!({"a":["abcd"]}),
        json!({"a":["aa","aa"]}),
        json!({"a":["AA"]}),
        json!({"a":["aa"],"b":true}),
    ] {
        assert!(schema::validate(&s, &value).is_err());
    }
}
#[tokio::test]
async fn wire_rejects_duplicates_nested_and_truncated_frames() {
    for bytes in [
        b"{\"a\":1,\"a\":2}".as_slice(),
        b"{\"a\":{\"b\":1,\"b\":2}}".as_slice(),
        b"{} {}".as_slice(),
    ] {
        assert!(wire::parse(bytes).is_err());
    }
    assert!(wire::parse(&vec![b' '; wire::MAX_FRAME + 1]).is_err());
    assert!(wire::read_frame(&mut BufReader::new(b"{}".as_slice()))
        .await
        .is_err());
    let (mut writer, reader) = tokio::io::duplex(64);
    tokio::spawn(async move {
        for chunk in ["{\"x\":\"", "é", "é\"}\n"] {
            writer.write_all(chunk.as_bytes()).await.unwrap();
        }
    });
    assert_eq!(
        wire::read_frame(&mut BufReader::new(reader))
            .await
            .unwrap()
            .unwrap()["x"],
        "éé"
    );
}
async fn connect(b: &Broker, token: &str) -> UnixStream {
    let mut s = UnixStream::connect(b.endpoint()).await.unwrap();
    s.write_all(format!("{token}\n").as_bytes()).await.unwrap();
    s
}
#[tokio::test]
async fn private_broker_relay_secret_free_and_revocation() {
    let c = core();
    let g = Arc::new(Gateway::new(c.clone(), credentials()));
    let b = Broker::new(&std::env::temp_dir(), g).unwrap();
    b.bind_engine(std::process::id()).unwrap();
    let entry = b
        .entry(
            "cortex",
            std::path::Path::new(env!("CARGO_BIN_EXE_cortex-mcp-relay")),
        )
        .unwrap();
    let text = serde_json::to_string(&entry).unwrap();
    assert!(!text.contains(credentials().secret()));
    assert!(!text.contains("relay.token"));
    let token =
        std::fs::read_to_string(b.endpoint().parent().unwrap().join("relay.token")).unwrap();
    let mut bad = connect(&b, "wrong").await;
    let _ = wire::write_frame(&mut bad, &json!({"jsonrpc":"2.0","id":1,"method":"ping"})).await;
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        wire::read_frame(&mut BufReader::new(bad))
    )
    .await
    .unwrap()
    .is_err_or_none());
    assert_eq!(b.tools_receipt().state, "not_observed");
    let mut good = connect(&b, &token).await;
    wire::write_frame(
        &mut good,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await
    .unwrap();
    let mut reader = BufReader::new(good);
    assert!(wire::read_frame(&mut reader)
        .await
        .unwrap()
        .unwrap()
        .get("result")
        .is_some());
    assert_eq!(b.tools_receipt().state, "complete");
    assert_eq!(b.tools_receipt().worker_result_submit_present, Some(true));
    c.live.store(false, Ordering::SeqCst);
    wire::write_frame(
        reader.get_mut(),
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await
    .unwrap();
    assert!(wire::read_frame(&mut reader)
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    drop(reader);
    b.shutdown().await.unwrap();
}
/// A relay started inside an engine filesystem view (its own PID namespace and
/// procfs) cannot use the service's `/proc/<pid>/fd/...` endpoint; it reaches the
/// same broker through the read-only view directory, and the broker still admits
/// it only as a descendant of the bound (view) process. Needs user namespaces;
/// `CORTEX_REQUIRE_NAMESPACE_FIXTURES` makes their absence a failure.
#[tokio::test]
async fn a_confined_relay_reaches_its_broker_through_the_view_directory() {
    use std::process::Stdio;
    let available = std::process::Command::new("/usr/bin/bwrap")
        .args(["--unshare-pid", "--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib"])
        .args(["--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin"])
        .args(["--proc", "/proc", "--dev", "/dev", "--", "/usr/bin/true"])
        .env_clear()
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(available, "the confined relay fixture requires user namespaces");
    }
    if !available {
        println!("NOT_RUN confined relay: user namespaces unavailable");
        return;
    }
    let c = core();
    let g = Arc::new(Gateway::new(c.clone(), credentials()));
    let b = Broker::new(&std::env::temp_dir(), g).unwrap();
    let relay = std::path::Path::new(env!("CARGO_BIN_EXE_cortex-mcp-relay"))
        .canonicalize()
        .unwrap();
    let directory = b.directory().canonicalize().unwrap();
    let shown = std::path::Path::new(cortex_engine::broker::VIEW_DIRECTORY)
        .join(directory.file_name().unwrap());
    let mut child = tokio::process::Command::new("/usr/bin/bwrap")
        .args(["--die-with-parent", "--unshare-pid", "--proc", "/proc", "--dev", "/dev"])
        .args(["--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib"])
        .args(["--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin"])
        .arg("--ro-bind")
        .arg(&relay)
        .arg(&relay)
        .arg("--ro-bind")
        .arg(&directory)
        .arg(&shown)
        .arg("--")
        .arg(&relay)
        .arg(b.endpoint())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    b.bind_engine(child.id().unwrap()).unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    wire::write_frame(
        &mut input,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await
    .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(10), wire::read_frame(&mut output))
        .await
        .expect("the confined relay answered")
        .unwrap()
        .unwrap();
    assert!(reply.get("result").is_some(), "{reply}");
    assert_eq!(b.tools_receipt().state, "complete");
    drop(input);
    let _ = child.kill().await;
    b.shutdown().await.unwrap();
}
#[tokio::test]
async fn fake_engine_launches_real_stdio_relay_into_bound_gateway() {
    use cortex_engine::{
        engine::{Engine, PermissionDecision, PermissionPolicy},
        environment::{EngineKind, LaunchSpec},
    };
    use std::collections::BTreeMap;
    struct Policy;
    impl PermissionPolicy for Policy {
        fn authority(&self) -> FutureResult<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn decide<'a>(
            &'a self,
            _: &'a str,
            _: &'a Value,
            _: u64,
            _: &'a Value,
        ) -> FutureResult<'a, PermissionDecision> {
            Box::pin(async { Ok(PermissionDecision::Cancelled) })
        }
    }
    let c = core();
    let g = Arc::new(Gateway::new(c.clone(), credentials()));
    let b = Broker::new(&std::env::temp_dir(), g).unwrap();
    let home = std::env::current_dir().unwrap();
    let e = Engine::launch(
        LaunchSpec {
            engine: EngineKind::Codex,
            executable: env!("CARGO_BIN_EXE_fake-engine").into(),
            args: vec!["relay".into()],
            cwd: home.clone(),
            native_home: home.clone(),
            credential_directory: home.clone(),
            base_environment: BTreeMap::new(),
            codex_launcher: None,
            confinement: None,
        },
        Arc::new(Policy),
    )
    .await
    .unwrap();
    b.bind_engine(e.pid()).unwrap();
    e.initialize(Duration::from_secs(3)).await.unwrap();
    let entry = b
        .entry(
            "cortex",
            std::path::Path::new(env!("CARGO_BIN_EXE_cortex-mcp-relay")),
        )
        .unwrap();
    e.start_session(&home, vec![entry], Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(c.calls.load(Ordering::SeqCst), 1);
    e.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
trait Ended {
    fn is_err_or_none(self) -> bool;
}
impl Ended for cortex_engine::Result<Option<Value>> {
    fn is_err_or_none(self) -> bool {
        !matches!(self, Ok(Some(_)))
    }
}

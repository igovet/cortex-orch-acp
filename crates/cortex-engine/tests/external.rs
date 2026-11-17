use cortex_engine::{
    broker::Handler,
    external::{ExternalCore, ExternalLaunch, ExternalProxy, Grants, Reservation},
    gateway::{Channel, Credentials},
    Error, FutureResult,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
struct Core {
    live: AtomicBool,
    requests: Mutex<HashMap<String, (String, Option<Value>)>>,
    dispatches: AtomicUsize,
}
impl ExternalCore for Core {
    fn authorize<'a>(&'a self, _: &'a Credentials, _: &'a str) -> FutureResult<'a, Channel> {
        Box::pin(async move {
            if !self.live.load(Ordering::SeqCst) {
                return Err(Error::Forbidden);
            }
            Ok(Channel {
                task: "fixture".into(),
                attempt: "fixture".into(),
                owner_epoch: 1,
                principal: "fixture".into(),
                role: "worker".into(),
                profile: Some("worker-backend-dev".into()),
                stage: Some("execution".into()),
            })
        })
    }
    fn reserve<'a>(
        &'a self,
        _: &'a Credentials,
        _: &'a Channel,
        _: &'a str,
        id: Value,
        digest: String,
    ) -> FutureResult<'a, Reservation> {
        Box::pin(async move {
            let mut r = self.requests.lock().unwrap();
            if let Some((d, response)) = r.get(&id.to_string()) {
                if *d != digest {
                    return Err(Error::Invalid);
                }
                return Ok(match response {
                    Some(reply) => Reservation::Replay(reply.clone()),
                    None => Reservation::Unknown,
                });
            }
            r.insert(id.to_string(), (digest, None));
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            Ok(Reservation::Dispatch)
        })
    }
    fn complete<'a>(
        &'a self,
        _: &'a Credentials,
        _: &'a str,
        id: Value,
        reply: Value,
    ) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .get_mut(&id.to_string())
                .unwrap()
                .1 = Some(reply);
            Ok(())
        })
    }
}
fn core() -> Arc<Core> {
    Arc::new(Core {
        live: AtomicBool::new(true),
        requests: Mutex::new(HashMap::new()),
        dispatches: AtomicUsize::new(0),
    })
}
async fn proxy(mode: &str, c: Arc<Core>) -> ExternalProxy {
    let home = std::env::current_dir().unwrap();
    ExternalProxy::launch(
        ExternalLaunch {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_fake-engine")),
            args: vec![mode.into()],
            cwd: home,
            environment: BTreeMap::new(),
        },
        Credentials::new(
            "ch_123456abcdef".into(),
            std::env::current_dir().unwrap(),
            "fixture-secret-12345".into(),
        )
        .unwrap(),
        c,
        Grants {
            name: "fixture".into(),
            allowed_tools: BTreeSet::from(["lookup".into(), "hang".into()]),
            always_load: BTreeSet::from(["lookup".into()]),
            read_only_tools: BTreeSet::from(["lookup".into()]),
            coordinator_tools: None,
        },
    )
    .await
    .unwrap()
}
fn request(id: u64, tool: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":{}}})
}
#[tokio::test]
async fn external_filter_receipts_replay_and_conflict() {
    let c = core();
    let p = proxy("external", c.clone()).await;
    let tools = p
        .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(
        tools["result"]["tools"][0]["_meta"]["anthropic/alwaysLoad"],
        true
    );
    assert!(p
        .handle(request(2, "write"))
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    let r = p.handle(request(3, "lookup")).await.unwrap().unwrap();
    assert!(r.get("result").is_some());
    assert_eq!(p.handle(request(3, "lookup")).await.unwrap().unwrap(), r);
    let mut changed = request(3, "lookup");
    changed["params"]["arguments"] = json!({"different":true});
    assert!(p
        .handle(changed)
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    assert_eq!(c.dispatches.load(Ordering::SeqCst), 1);
    let r = p
        .handle(json!({"jsonrpc":"2.0","id":4,"method":"resources/list"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["result"]["resources"], json!([]));
    assert!(p
        .handle(json!({"jsonrpc":"2.0","id":5,"method":"sampling/createMessage"}))
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    p.shutdown().await.unwrap();
}
#[tokio::test]
async fn mismatched_response_and_dropped_action_stay_unknown() {
    let c = core();
    let p = proxy("mismatch", c.clone()).await;
    assert!(p
        .handle(request(1, "lookup"))
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    assert!(
        p.handle(request(1, "lookup")).await.unwrap().unwrap()["error"]["message"]
            == "deadline_outcome_unknown"
    );
    assert_eq!(c.dispatches.load(Ordering::SeqCst), 1);
    p.shutdown().await.unwrap();
    let c = core();
    let p = proxy("external", c.clone()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(40), p.handle(request(2, "hang")))
            .await
            .is_err()
    );
    assert!(
        p.handle(request(2, "hang")).await.unwrap().unwrap()["error"]["message"]
            == "deadline_outcome_unknown"
    );
    assert_eq!(c.dispatches.load(Ordering::SeqCst), 1);
    p.shutdown().await.unwrap();
}
#[tokio::test]
async fn revoked_external_channel_cannot_dispatch() {
    let c = core();
    let p = proxy("external", c.clone()).await;
    c.live.store(false, Ordering::SeqCst);
    assert!(p
        .handle(request(1, "lookup"))
        .await
        .unwrap()
        .unwrap()
        .get("error")
        .is_some());
    assert_eq!(c.dispatches.load(Ordering::SeqCst), 0);
    tokio::time::timeout(Duration::from_secs(3), p.shutdown())
        .await
        .unwrap()
        .unwrap();
}

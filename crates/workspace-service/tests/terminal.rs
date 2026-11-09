use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame, Config, LaunchSpec, Service};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "workspace-pty-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(root.join("project")).unwrap();
        Self(root)
    }
    fn config(&self) -> Config {
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.clone(),
            launch: LaunchSpec::new("/usr/bin/python3".into(), vec!["-B".into()]).unwrap(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 2,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn rpc(s: &Service, method: &str, params: Value) -> Value {
    let mut c = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut c,
        &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), read_frame(&mut c))
        .await
        .unwrap()
        .unwrap()
}
async fn ok(s: &Service, method: &str, params: Value) -> Value {
    let result = rpc(s, method, params).await;
    assert!(result.get("error").is_none(), "service operation refused");
    result["result"].clone()
}
async fn wait_file(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
fn running(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(") ").map(|(_, tail)| !tail.starts_with("Z ")))
        .unwrap_or(false)
}

fn isolated_terminal_test(name: &str) -> bool {
    if std::env::var("CW_PTY_ISOLATED_TEST").as_deref() == Ok(name) { return true; }
    let fixture = Fixture::new();
    let home = fixture.0.join("shell-home");
    std::fs::create_dir(&home).unwrap();
    // Re-exec only this test: never alter the process-wide environment while sibling
    // tests run, and never load/save the owner's shell rc or history.
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name])
        .env("CW_PTY_ISOLATED_TEST", name)
        .env("HOME", &home).env("SHELL", "/bin/sh")
        .env("XDG_CONFIG_HOME", &home).env("XDG_STATE_HOME", &home)
        .env("XDG_CACHE_HOME", &home)
        .env_remove("ENV").env_remove("BASH_ENV")
        .output().unwrap().status;
    assert!(status.success(), "isolated terminal test failed");
    false
}

#[tokio::test]
async fn terminal_real_service_round_trip_detach_resize_close_and_stop() {
    if !isolated_terminal_test("terminal_real_service_round_trip_detach_resize_close_and_stop") { return; }
    let f = Fixture::new();
    let s = Service::bind(f.config()).await.unwrap();
    let p = s.register(&f.0.join("project"), "PTY").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let serve = tokio::spawn(s.clone().serve());
    let opened = ok(
        &s,
        "terminal.open",
        json!({"projectId":p,"cols":80,"rows":24}),
    )
    .await;
    let id = opened["sessionId"].clone();
    // Every rpc above opens and detaches a real same-user Unix socket connection.
    let same = ok(
        &s,
        "terminal.open",
        json!({"projectId":p,"cols":90,"rows":30}),
    )
    .await;
    assert_eq!(same["sessionId"], id);
    ok(
        &s,
        "terminal.resize",
        json!({"projectId":p,"sessionId":id,"cols":91,"rows":31}),
    )
    .await;
    ok(&s,"terminal.write",json!({"projectId":p,"sessionId":id,"data":"printf 'CW_%s' PTY; pwd > cwd; id -u > uid; stty size > dimensions; echo $$ > shell-pid; sleep 60 & echo $! > job-pid\n"})).await;
    for name in ["cwd", "uid", "dimensions", "shell-pid", "job-pid"] {
        wait_file(&f.0.join("project").join(name)).await;
    }
    assert_eq!(
        std::fs::read_to_string(f.0.join("project/cwd"))
            .unwrap()
            .trim(),
        f.0.join("project").to_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("project/uid"))
            .unwrap()
            .trim(),
        unsafe { libc::geteuid() }.to_string()
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("project/dimensions"))
            .unwrap()
            .trim(),
        "31 91"
    );
    let pid = std::fs::read_to_string(f.0.join("project/shell-pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    let job = std::fs::read_to_string(f.0.join("project/job-pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    let page = ok(
        &s,
        "terminal.read",
        json!({"projectId":p,"sessionId":id,"after":0}),
    )
    .await;
    let bytes: Vec<u8> = serde_json::from_value(page["data"].clone()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("CW_PTY"));
    assert!(running(pid) && running(job));
    // Fail-closed service validation is independent of the native bridge.
    for extra in [
        "cwd",
        "root",
        "command",
        "argv",
        "env",
        "shell",
        "executable",
        "taskId",
    ] {
        let mut params = json!({"projectId":p,"cols":80,"rows":24});
        params[extra] = json!("/not/allowed");
        assert!(rpc(&s, "terminal.open", params)
            .await
            .get("error")
            .is_some());
    }
    for method in ["terminal.exec", "shell.exec", "process.spawn"] {
        assert!(rpc(&s, method, json!({"projectId":p}))
            .await
            .get("error")
            .is_some());
    }
    for params in [
        json!({"projectId":"unknown","sessionId":id,"after":0}),
        json!({"projectId":p,"sessionId":"unknown","after":0}),
    ] {
        assert!(rpc(&s, "terminal.read", params)
            .await
            .get("error")
            .is_some());
    }
    let bad = rpc(
        &s,
        "terminal.write",
        json!({"projectId":p,"sessionId":id,"data":"x".repeat(4097)}),
    )
    .await;
    assert!(bad.get("error").is_some());
    // High output is drained into a fixed 64 KiB tail and explicitly reports lost bytes.
    ok(&s,"terminal.write",json!({"projectId":p,"sessionId":id,"data":"head -c 200000 /dev/zero | tr '\\000' x; touch flood-done\n"})).await;
    wait_file(&f.0.join("project/flood-done")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let page = ok(
        &s,
        "terminal.read",
        json!({"projectId":p,"sessionId":id,"after":0}),
    )
    .await;
    assert_eq!(page["truncated"], true);
    assert!(page["data"].as_array().unwrap().len() <= 16384);
    let start = page["next"].as_u64().unwrap() - page["data"].as_array().unwrap().len() as u64;
    let mut next = page["next"].as_u64().unwrap();
    for _ in 0..8 {
        let page = ok(
            &s,
            "terminal.read",
            json!({"projectId":p,"sessionId":id,"after":next}),
        )
        .await;
        let end = page["next"].as_u64().unwrap();
        assert!(end - start <= 65536);
        if end == next {
            break;
        }
        next = end;
    }
    ok(&s, "terminal.close", json!({"projectId":p,"sessionId":id})).await;
    assert!(!running(pid) && !running(job));
    assert!(rpc(
        &s,
        "terminal.read",
        json!({"projectId":p,"sessionId":id,"after":0})
    )
    .await
    .get("error")
    .is_some());
    let new = ok(
        &s,
        "terminal.open",
        json!({"projectId":p,"cols":80,"rows":24}),
    )
    .await;
    assert_ne!(new["sessionId"], id);
    let id2 = new["sessionId"].clone();
    ok(
        &s,
        "terminal.write",
        json!({"projectId":p,"sessionId":id2,"data":"echo $$ > stop-pid\n"}),
    )
    .await;
    wait_file(&f.0.join("project/stop-pid")).await;
    let pid = std::fs::read_to_string(f.0.join("project/stop-pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    s.stop().await.unwrap();
    serve.await.unwrap().unwrap();
    assert!(!running(pid));
    drop(s);
    let s = Service::bind(f.config()).await.unwrap();
    let serve = tokio::spawn(s.clone().serve());
    assert!(rpc(
        &s,
        "terminal.read",
        json!({"projectId":p,"sessionId":id2,"after":0})
    )
    .await
    .get("error")
    .is_some());
    s.stop().await.unwrap();
    serve.await.unwrap().unwrap();
}
#[tokio::test]
async fn terminal_exit_is_retained_and_removal_reaps_the_shell() {
    if !isolated_terminal_test("terminal_exit_is_retained_and_removal_reaps_the_shell") { return; }
    let f = Fixture::new();
    let s = Service::bind(f.config()).await.unwrap();
    let p = s.register(&f.0.join("project"), "PTY").await.unwrap()["projectId"].as_str().unwrap().to_owned();
    let serve = tokio::spawn(s.clone().serve());
    let opened = ok(&s, "terminal.open", json!({"projectId":p,"cols":80,"rows":24})).await;
    let id = opened["sessionId"].clone();
    ok(&s, "terminal.write", json!({"projectId":p,"sessionId":id,"data":"exit\n"})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let page = ok(&s, "terminal.read", json!({"projectId":p,"sessionId":id,"after":0})).await;
            if page["exited"] == true { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    let retained = ok(&s, "terminal.open", json!({"projectId":p,"cols":80,"rows":24})).await;
    assert_eq!(retained["sessionId"], id);
    assert_eq!(retained["exited"], true);
    ok(&s, "terminal.close", json!({"projectId":p,"sessionId":id})).await;
    let opened = ok(&s, "terminal.open", json!({"projectId":p,"cols":80,"rows":24})).await;
    let id = opened["sessionId"].clone();
    ok(&s, "terminal.write", json!({"projectId":p,"sessionId":id,"data":"echo $$ > remove-pid\n"})).await;
    wait_file(&f.0.join("project/remove-pid")).await;
    let pid = std::fs::read_to_string(f.0.join("project/remove-pid")).unwrap().trim().parse::<i32>().unwrap();
    s.remove(&p).await.unwrap();
    assert!(!running(pid));
    s.stop().await.unwrap(); serve.await.unwrap().unwrap();
}

#[tokio::test]
async fn terminal_limits_removed_project_and_changed_root_are_refused() {
    if !isolated_terminal_test("terminal_limits_removed_project_and_changed_root_are_refused") { return; }
    let f = Fixture::new();
    let s = Service::bind(f.config()).await.unwrap();
    let serve = tokio::spawn(s.clone().serve());
    let mut projects = Vec::new();
    for i in 0..9 {
        let root = f.0.join(format!("p{i}"));
        std::fs::create_dir(&root).unwrap();
        let p = s.register(&root, "PTY").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let result = rpc(
            &s,
            "terminal.open",
            json!({"projectId":p,"cols":80,"rows":24}),
        )
        .await;
        assert_eq!(result.get("error").is_none(), i < 8);
        projects.push(p);
    }
    s.remove(&projects[0]).await.unwrap();
    assert!(rpc(
        &s,
        "terminal.open",
        json!({"projectId":projects[0],"cols":80,"rows":24})
    )
    .await
    .get("error")
    .is_some());
    ok(
        &s,
        "terminal.open",
        json!({"projectId":projects[8],"cols":80,"rows":24}),
    )
    .await;
    let p = s.register(&f.0.join("project"), "Changed").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::rename(f.0.join("project"), f.0.join("moved")).unwrap();
    symlink(f.0.join("moved"), f.0.join("project")).unwrap();
    assert!(rpc(
        &s,
        "terminal.open",
        json!({"projectId":p,"cols":80,"rows":24})
    )
    .await
    .get("error")
    .is_some());
    s.stop().await.unwrap();
    serve.await.unwrap().unwrap();
}

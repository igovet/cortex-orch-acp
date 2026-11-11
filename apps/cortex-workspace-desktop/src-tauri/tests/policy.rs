use cortex_workspace_desktop::policy::{authorize, local_url};
use serde_json::{json, Value};

fn query(op: &str) -> Value {
    json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"test","projectId":"project-1","operation":op})
}

#[test]
fn the_window_grant_is_exactly_its_own_frame() {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
    assert_eq!(capability["windows"], json!(["main"]));
    assert_eq!(capability["local"], json!(true));
    assert!(capability.get("remote").is_none());
    let window: Vec<&str> = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|permission| permission.starts_with("core:window:"))
        .collect();
    assert_eq!(
        window,
        [
            "core:window:allow-minimize",
            "core:window:allow-toggle-maximize",
            "core:window:allow-is-maximized",
            "core:window:allow-close",
            "core:window:allow-start-dragging",
        ]
    );
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let csp = config["app"]["security"]["csp"].as_str().unwrap();
    assert!(csp.contains("font-src 'self';") && csp.contains("script-src 'self';"));
    assert!(!csp.contains("unsafe-eval") && !csp.contains("https:"));
}

#[test]
fn saving_an_attachment_takes_only_a_plain_name_and_bounded_bytes() {
    let call = |message: Value| authorize("main", "tauri://localhost/", json!({"operation":"native.save_attachment","message":message}));
    assert!(call(json!({"name":"shot.png","data":"eA=="})).is_ok());
    assert!(call(json!({"name":"../shot.png","data":"eA=="})).is_err());
    assert!(call(json!({"name":"","data":"eA=="})).is_err());
    assert!(call(json!({"name":"a\nb","data":"eA=="})).is_err());
    assert!(call(json!({"name":"shot.png","data":"eA==","path":"/tmp/x"})).is_err());
    assert!(call(json!({"name":"shot.png","data":"x".repeat(workspace_protocol::ATTACHMENT_BYTES * 2)})).is_err());
    assert!(authorize("other", "tauri://localhost/", json!({"operation":"native.save_attachment","message":{"name":"a","data":""}})).is_err());
}

#[test]
fn native_login_and_maintenance_are_closed_owner_actions() {
    for engine in ["codex-acp", "claude-agent-acp"] {
        assert!(authorize("main", "tauri://localhost/", json!({"operation":"native.engine_login","message":{"engine":engine}})).is_ok());
    }
    for message in [json!({"engine":"opencode"}), json!({"engine":"codex-acp","program":"/tmp/other"}), json!({"engine":"codex-acp","key":"fixture"})] {
        assert!(authorize("main", "tauri://localhost/", json!({"operation":"native.engine_login","message":message})).is_err());
    }
    for op in ["maintenance", "engine.connection", "workflow.describe"] {
        let call = json!({"operation":"machine.call","message":{"op":op,"params":{}}});
        assert!(authorize("main", "tauri://localhost/", call.clone()).is_ok());
        assert!(authorize("main", "https://example.invalid", call).is_err());
    }
}

#[test]
fn only_bundled_main_can_call() {
    let body = json!({"operation":"projects.list","message":{}});
    assert!(authorize("main", "tauri://localhost/", body.clone()).is_ok());
    assert!(authorize("main", "tauri://localhost", body.clone()).is_ok());
    for (window, url) in [
        ("other", "tauri://localhost/"),
        ("main", "https://evil.test/"),
        ("main", "tauri://evil/"),
        ("main", "http://localhost:5181/"),
        ("main", "tauri://localhost@evil/"),
        ("main", "file:///tmp/a.html"),
    ] {
        assert!(authorize(window, url, body.clone()).is_err());
    }
    for url in [
        "https://example.com",
        "javascript:alert(1)",
        "data:text/html,x",
        "about:blank",
        "tauri://localhost.evil/",
        "tauri://localhost:99/",
        "tauri://localhost.evil",
        "tauri://localhost@evil",
        "tauri://localhost?external=true",
    ] {
        assert!(!local_url(url));
    }
}

#[test]
fn attachment_bridge_takes_bytes_and_never_client_paths() {
    for op in ["native.clipboard_image", "native.take_dropped_files"] {
        let call = json!({"operation":op,"message":{}});
        assert!(authorize("main", "tauri://localhost/", call.clone()).is_ok());
        assert!(authorize("main", "https://example.invalid/", call.clone()).is_err());
        assert!(authorize("other", "tauri://localhost/", call).is_err());
        assert!(authorize("main", "tauri://localhost/", json!({"operation":op,"message":{"path":"/a"}})).is_err());
    }
    let mut prompt = json!({"operation":"executions.prompt","message":{"projectId":"p","executionId":"e","requestId":"r","text":"Keep",
        "attachments":[{"name":"notes.txt","mimeType":"text/plain","data":"Zml4dHVyZQ=="}]}});
    assert!(authorize("main", "tauri://localhost/", prompt.clone()).is_ok());
    prompt["message"]["attachments"] = json!([{"path":"/a"}]);
    assert!(authorize("main", "tauri://localhost/", prompt).is_err());
}

#[test]
fn an_owner_save_is_a_closed_machine_call_and_opening_a_file_names_only_a_target() {
    let save = json!({"operation":"machine.call","message":{"op":"repo.file.save","projectId":"p","params":{"file":"a.txt","text":"x","revision":"r"}}});
    assert!(authorize("main", "tauri://localhost/", save).is_ok());
    // The bridge alone asks where a file is on disk; a page never learns or names a host path.
    let locate = json!({"operation":"machine.call","message":{"op":"repo.locate","projectId":"p","params":{"file":"a.txt"}}});
    assert!(authorize("main", "tauri://localhost/", locate).is_err());
    let open = |message: Value| authorize("main", "tauri://localhost/", json!({"operation":"native.open_editor","message":message}));
    assert!(open(json!({"projectId":"p"})).is_ok());
    assert!(open(json!({"projectId":"p","checkoutId":"p:main","file":"src/a.rs","line":12})).is_ok());
    for bad in [json!({"projectId":"p","line":0}), json!({"projectId":"p","line":"1"}), json!({"projectId":"p","file":""}), json!({"projectId":"p","file":"a\0b"}),
        json!({"projectId":"p","command":"sh"}), json!({"projectId":"p","checkoutId":7}), json!({"checkoutId":"p:main"})] {
        assert!(open(bad).is_err());
    }
}

#[test]
fn exact_allowlist_and_arguments() {
    let registration = json!({"operation":"projects.register","message":{"root":"/disposable/project","displayName":"Project"}});
    assert!(authorize("main", "tauri://localhost/", registration.clone()).is_ok());
    for field in ["command", "socketPath", "env", "launchCommand"] {
        let mut extra = registration.clone();
        extra["message"][field] = json!("forbidden");
        assert!(authorize("main", "tauri://localhost/", extra).is_err());
    }
    for root in ["relative", "~other/project", "", "/bad\0path"] {
        let mut bad = registration.clone();
        bad["message"]["root"] = json!(root);
        assert!(authorize("main", "tauri://localhost/", bad).is_err());
    }
    // The service expands the owner's own home; the shell's other forms mean nothing here.
    let mut home = registration.clone();
    home["message"]["root"] = json!("~/project");
    assert!(authorize("main", "tauri://localhost/", home).is_ok());
    let mut create = registration.clone();
    create["message"]["create"] = json!(true);
    assert!(authorize("main", "tauri://localhost/", create.clone()).is_ok());
    create["message"]["create"] = json!("yes");
    assert!(authorize("main", "tauri://localhost/", create).is_err());
    let mut new_folder = registration.clone();
    new_folder["message"]["newFolder"] = json!(true);
    assert!(authorize("main", "tauri://localhost/", new_folder.clone()).is_ok());
    new_folder["message"]["newFolder"] = json!(1);
    assert!(authorize("main", "tauri://localhost/", new_folder).is_err());
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"directories.search","message":{"session":"pick-1","query":"~/code","limit":30}})).is_ok());
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"directories.cancel","message":{"session":"pick-1"}})).is_ok());
    for message in [json!({"session":"pick-1","query":"x","root":"/"}), json!({"session":"pick-1","query":"x","limit":1000}), json!({"query":"x"}), json!({"session":"../x","query":"x"})] {
        assert!(authorize("main", "tauri://localhost/", json!({"operation":"directories.search","message":message})).is_err());
    }
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"directories.delete","message":{"session":"pick-1"}})).is_err());
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"projects.rename","message":{"projectId":"wp_1","displayName":"Cedar"}})).is_ok());
    for message in [json!({"projectId":"wp_1","displayName":" "}), json!({"projectId":"wp_1"}), json!({"projectId":"wp_1","displayName":"Cedar","root":"/"})] {
        assert!(authorize("main", "tauri://localhost/", json!({"operation":"projects.rename","message":message})).is_err());
    }
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"native.pick_folder","message":{}})).is_ok());
    assert!(authorize("main", "tauri://localhost/", json!({"operation":"native.pick_folder","message":{"root":"/"}})).is_err());
    for op in ["service.status", "service.capabilities", "projects.list"] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":op,"message":{}})
        )
        .is_ok());
    }
    for op in [
        "service.stop",
        "projects.register",
        "shell.open",
        "workspace",
        "arbitrary.rpc",
    ] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":op,"message":{}})
        )
        .is_err());
    }
    for key in [
        "path",
        "root",
        "socketPath",
        "command",
        "argv",
        "launch",
        "credentials",
        "filePath",
        "paths",
        "workingDirectory",
        "launchCommand",
        "executable_path",
    ] {
        let mut body = json!({"operation":"projects.list","message":{}});
        body["message"][key] = json!("/not/allowed");
        assert!(authorize("main", "tauri://localhost/", body).is_err());
        let mut body = json!({"operation":"tasks.create","message":{
            "protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"c1","projectId":"project-1","target":"c1",
            "operation":"tasks.create","expectedVersion":"0","ownerEpoch":null,"actorId":"client",
            "payload":{"description":"hello"}}});
        body["message"]["payload"][key] = json!("not allowed");
        assert!(authorize("main", "tauri://localhost/", body).is_err());
    }
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"tasks.list","message":query("tasks.get")})
    )
    .is_err());
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"tasks.list","message":query("tasks.list"),"path":"x"})
    )
    .is_err());
}

#[test]
fn machine_calls_are_a_closed_set_and_cannot_name_a_root() {
    let call = |message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"machine.call","message":message}),
        )
    };
    assert!(call(json!({"op":"overview"})).is_ok());
    assert!(call(json!({"op":"git","projectId":"wp_1","params":{"action":"stage","path":"a.txt"}})).is_ok());
    assert!(call(json!({"op":"shell","params":{"line":"true"}})).is_err());
    assert!(call(json!({"op":"overview","root":"/etc"})).is_err());
    assert!(call(json!({"op":"overview","stateDir":"/tmp"})).is_err());
    assert!(call(json!({"op":"git","projectId":"../other"})).is_err());
    let run = |operation: &str, message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":operation,"message":message}),
        )
    };
    let target = json!({"projectId":"wp_1","executionId":"we_1"});
    assert!(run("executions.cancel", target.clone()).is_ok());
    assert!(run("executions.prompt", target.clone()).is_err());
    assert!(run(
        "executions.prompt",
        json!({"projectId":"wp_1","executionId":"we_1","text":"Stop and explain"})
    )
    .is_ok());
    assert!(run(
        "executions.cancel",
        json!({"projectId":"wp_1","executionId":"we_1","command":"x"})
    )
    .is_err());
}

/// Every machine operation the bundled real port names must pass the bridge, or the
/// whole workspace fails to load with `bridge_request_refused` (startup reads `product.facts`).
#[test]
fn every_machine_operation_the_real_port_names_is_allowed() {
    let source = include_str!("../../../../packages/workspace-ui/src/port/real.ts");
    let mut ops = Vec::new();
    for (at, _) in source.match_indices("machine") {
        let rest = &source[at + "machine".len()..];
        // `machine("op"` or `machine<Type>("op"`; the generic may hold one nested `<...>`.
        let rest = if rest.starts_with('<') {
            let mut depth = 0;
            let Some(end) = rest.char_indices().find_map(|(i, c)| {
                match c { '<' => depth += 1, '>' => { depth -= 1; if depth == 0 { return Some(i + 1); } } _ => {} }
                None
            }) else { continue };
            &rest[end..]
        } else { rest };
        if let Some(op) = rest.strip_prefix("(\"").and_then(|tail| tail.split('"').next()) {
            if !ops.contains(&op) { ops.push(op); }
        }
    }
    assert!(ops.contains(&"product.facts") && ops.contains(&"repo.snapshot") && ops.len() >= 15, "{ops:?}");
    for op in ops {
        let call = json!({"operation":"machine.call","message":{"op":op,"projectId":"wp_1","params":{}}});
        assert!(authorize("main", "tauri://localhost/", call).is_ok(), "the bridge refuses machine op {op}");
    }
}

#[test]
fn owner_check_reruns_name_only_retained_result_evidence() {
    let body = json!({"operation":"executions.control","message":{
        "projectId":"p","executionId":"e","requestId":"r","action":"rerun_check",
        "checkIndex":0,"checkName":"Required check","digest":"observed-digest","resultVersion":"47"}});
    assert!(authorize("main", "tauri://localhost/", body.clone()).is_ok());
    assert!(authorize("other", "tauri://localhost/", body.clone()).is_err());
    assert!(authorize("main", "https://example.invalid/", body.clone()).is_err());
    for (key, value) in [
        ("checkIndex", json!(-1)), ("checkIndex", json!(100)), ("checkIndex", json!("0")),
        ("checkName", json!("")), ("digest", json!(null)), ("resultVersion", json!("0")),
        ("resultVersion", json!(47)), ("argv", json!(["true"])), ("cwd", json!("/tmp")),
        ("extra", json!(true)),
    ] {
        let mut bad = body.clone();
        bad["message"][key] = value;
        assert!(authorize("main", "tauri://localhost/", bad).is_err(), "{key}");
    }
    for key in ["checkIndex", "checkName", "digest", "resultVersion"] {
        let mut bad = body.clone();
        bad["message"].as_object_mut().unwrap().remove(key);
        assert!(authorize("main", "tauri://localhost/", bad).is_err(), "{key}");
    }
}

#[test]
fn public_envelopes_validate_and_capabilities_are_native() {
    for op in [
        "project.snapshot",
        "tasks.list",
        "tasks.get",
        "executions.get",
        "decisions.list",
        "decisions.get",
        "results.get",
        "events.page",
        "history.page",
        "requests.get",
    ] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":op,"message":query(op)})
        )
        .is_ok());
    }
    assert!(authorize("main","tauri://localhost/",json!({"operation":"tasks.list","message":{
        "protocol":{"major":99,"minor":0},"requestId":"q","projectId":"p","operation":"tasks.list"}})).is_err());
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"projects.activate","message":{"projectId":"p"}})
    )
    .is_ok());
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"projects.activate","message":{"projectId":"../p"}})
    )
    .is_err());
    let body = json!({"operation":"tasks.create","message":{
        "protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"c1","projectId":"p","target":"c1",
        "operation":"tasks.create","expectedVersion":"0","ownerEpoch":null,"actorId":"client",
        "payload":{"description":"Please examine /src and explain the launch command"}}});
    assert!(authorize("main", "tauri://localhost/", body).is_ok());
}

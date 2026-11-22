use cortex_workspace_desktop::policy::{authorize, local_url};
use serde_json::{json, Value};

fn query(op: &str) -> Value {
    json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"test","projectId":"project-1","operation":op})
}

#[test]
fn native_queries_are_typed_read_requests_not_mutation_or_sql_escape_hatches() {
    let message = json!({"protocol":{"major":2,"mapping":2},"requestId":"read","projectId":"p","operation":"tasks.state","payload":{"id":"t"}});
    let call = |message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"native.query","message":message}),
        )
    };
    assert!(call(message.clone()).is_ok());
    let mut bad = message.clone();
    bad["actorId"] = json!("owner");
    assert!(call(bad).is_err());
    let mut bad = message.clone();
    bad["payload"]["sql"] = json!("select 1");
    assert!(call(bad).is_err());
    let mut bad = message.clone();
    bad["operation"] = json!("tasks.start");
    assert!(call(bad).is_err());
    let mut bad = message.clone();
    bad["protocol"]["mapping"] = json!(1);
    assert!(call(bad).is_err());
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
    // Pictures, video and sound of an open file play from in-memory bytes only, never from a remote source.
    assert!(csp.contains("media-src 'self' blob:;") && csp.contains("frame-src 'none';"));
}

/// The window's two requests to the service's notifier are exact shapes, and a phone sends neither.
#[test]
fn the_window_reports_focus_and_kinds_and_takes_a_click_target_in_exact_shapes() {
    let call = |operation: &str, message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":operation,"message":message}),
        )
    };
    let all = json!(["question", "stop", "result", "failed", "planning", "paused"]);
    assert!(call("notifications.report", json!({"focused":null,"kinds":all})).is_ok());
    assert!(call(
        "notifications.report",
        json!({"focused":{"project":"p","task":"t"},"kinds":[]})
    )
    .is_ok());
    assert!(call(
        "notifications.report",
        json!({"focused":{"project":"p","sprint":"s"},"kinds":["result"]})
    )
    .is_ok());
    assert!(call("notifications.take", json!({})).is_ok());
    // The no-progress kind and its threshold, in one of the offered hours.
    let with_stalled =
        json!(["question", "stop", "result", "failed", "planning", "paused", "stalled"]);
    for hours in [1, 2, 3, 6] {
        assert!(call(
            "notifications.report",
            json!({"focused":null,"kinds":with_stalled,"stallHours":hours})
        )
        .is_ok());
    }
    for hours in [json!(0), json!(4), json!("3"), json!(null)] {
        assert!(call(
            "notifications.report",
            json!({"focused":null,"kinds":["stalled"],"stallHours":hours})
        )
        .is_err());
    }
    for (operation, message) in [
        (
            "notifications.report",
            json!({"focused":{"project":"p","task":"t","sprint":"s"},"kinds":[]}),
        ),
        (
            "notifications.report",
            json!({"focused":{"project":"p"},"kinds":[]}),
        ),
        (
            "notifications.report",
            json!({"focused":null,"kinds":["everything"]}),
        ),
        (
            "notifications.report",
            json!({"focused":null,"kinds":[],"path":"/tmp"}),
        ),
        (
            "notifications.report",
            json!({"focused":{"project":"a b","task":"t"},"kinds":[]}),
        ),
        ("notifications.take", json!({"target":"x"})),
        // The click claim and its activation token belong to the native shell alone.
        ("notifications.opened", json!({"nonce":"n"})),
    ] {
        assert!(
            call(operation, message.clone()).is_err(),
            "{operation} {message}"
        );
    }
    for (operation, message) in [
        ("notifications.report", json!({"focused":null,"kinds":[]})),
        ("notifications.take", json!({})),
    ] {
        assert!(cortex_workspace_desktop::policy::authorize_phone(
            json!({"operation":operation,"message":message})
        )
        .is_err());
    }
}

/// A notification's click opens a place the window named itself; the window grant stays the same
/// three commands, and the click is not a bridge request at all.
#[test]
fn a_notification_carries_only_a_closed_target_and_adds_no_window_command() {
    use cortex_workspace_desktop::policy::notification_target;
    let question = json!({"kind":"question","project":"p","task":"t","decision":"d"});
    assert!(notification_target("Fix the race", question.clone()).is_ok());
    assert!(notification_target(
        "Plan",
        json!({"kind":"planning","project":"p","sprint":"s","decision":"d"})
    )
    .is_ok());
    for bad in [
        json!({"kind":"question","project":"p","task":"t","decision":"d","url":"https://example.invalid"}),
        json!({"kind":"route","href":"/settings/app/updates"}),
        json!({"kind":"question","project":"p","task":"t"}),
        json!({"kind":"question","project":"p","task":"../t","decision":"d\u{0}"}),
        json!({"kind":"question","project":"p q","task":"t","decision":"d"}),
        json!(null),
    ] {
        assert!(notification_target("Fix", bad.clone()).is_err(), "{bad}");
    }
    assert!(notification_target(&"x".repeat(1001), question).is_err());
    // The target is not a request the window may send through the bridge.
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"native.navigate","message":{"kind":"question"}})
    )
    .is_err());
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
    let commands: Vec<&str> = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|permission| permission.starts_with("allow-"))
        .collect();
    assert_eq!(
        commands,
        [
            "allow-workspace-request",
            "allow-workspace-notify",
            "allow-workspace-close"
        ]
    );
}

#[test]
fn saving_an_attachment_takes_only_a_plain_name_and_bounded_bytes() {
    let call = |message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"native.save_attachment","message":message}),
        )
    };
    assert!(call(json!({"name":"shot.png","data":"eA=="})).is_ok());
    assert!(call(json!({"name":"../shot.png","data":"eA=="})).is_err());
    assert!(call(json!({"name":"","data":"eA=="})).is_err());
    assert!(call(json!({"name":"a\nb","data":"eA=="})).is_err());
    assert!(call(json!({"name":"shot.png","data":"eA==","path":"/tmp/x"})).is_err());
    assert!(call(
        json!({"name":"shot.png","data":"x".repeat(workspace_protocol::ATTACHMENT_BYTES * 2)})
    )
    .is_err());
    assert!(authorize(
        "other",
        "tauri://localhost/",
        json!({"operation":"native.save_attachment","message":{"name":"a","data":""}})
    )
    .is_err());
}

#[test]
fn native_login_and_maintenance_are_closed_owner_actions() {
    for engine in ["codex-acp", "claude-agent-acp"] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"native.engine_login","message":{"engine":engine}})
        )
        .is_ok());
    }
    for message in [
        json!({"engine":"opencode"}),
        json!({"engine":"codex-acp","program":"/tmp/other"}),
        json!({"engine":"codex-acp","key":"fixture"}),
    ] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"native.engine_login","message":message})
        )
        .is_err());
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
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":op,"message":{"path":"/a"}})
        )
        .is_err());
    }
    let mut prompt = json!({"operation":"executions.prompt","message":{"projectId":"p","executionId":"e","requestId":"r","text":"Keep",
        "attachments":[{"name":"notes.txt","mimeType":"text/plain","size":7,"sha256":"a".repeat(64)}]}});
    assert!(authorize("main", "tauri://localhost/", prompt.clone()).is_ok());
    // Bytes go only to the one upload; a prompt carries references.
    prompt["message"]["attachments"] =
        json!([{"name":"notes.txt","mimeType":"text/plain","data":"Zml4dHVyZQ=="}]);
    assert!(authorize("main", "tauri://localhost/", prompt.clone()).is_err());
    let upload = json!({"operation":"machine.call","message":{"op":"product.attachment",
        "params":{"action":"upload","files":[{"name":"notes.txt","mimeType":"text/plain","data":"Zml4dHVyZQ=="}]}}});
    assert!(authorize("main", "tauri://localhost/", upload).is_ok());
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
    let skills = json!({"operation":"machine.call","message":{"op":"repo.skills","projectId":"p","params":{}}});
    assert!(authorize("main", "tauri://localhost/", skills).is_ok());
    let open = |message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"native.open_editor","message":message}),
        )
    };
    assert!(open(json!({"projectId":"p"})).is_ok());
    assert!(
        open(json!({"projectId":"p","checkoutId":"p:main","file":"src/a.rs","line":12})).is_ok()
    );
    for bad in [
        json!({"projectId":"p","line":0}),
        json!({"projectId":"p","line":"1"}),
        json!({"projectId":"p","file":""}),
        json!({"projectId":"p","file":"a\0b"}),
        json!({"projectId":"p","command":"sh"}),
        json!({"projectId":"p","checkoutId":7}),
        json!({"checkoutId":"p:main"}),
    ] {
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
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"directories.cancel","message":{"session":"pick-1"}})
    )
    .is_ok());
    for message in [
        json!({"session":"pick-1","query":"x","root":"/"}),
        json!({"session":"pick-1","query":"x","limit":1000}),
        json!({"query":"x"}),
        json!({"session":"../x","query":"x"}),
    ] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"directories.search","message":message})
        )
        .is_err());
    }
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"directories.delete","message":{"session":"pick-1"}})
    )
    .is_err());
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"projects.rename","message":{"projectId":"wp_1","displayName":"Cedar"}})
    )
    .is_ok());
    for message in [
        json!({"projectId":"wp_1","displayName":" "}),
        json!({"projectId":"wp_1"}),
        json!({"projectId":"wp_1","displayName":"Cedar","root":"/"}),
    ] {
        assert!(authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":"projects.rename","message":message})
        )
        .is_err());
    }
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"native.pick_folder","message":{}})
    )
    .is_ok());
    assert!(authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":"native.pick_folder","message":{"root":"/"}})
    )
    .is_err());
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
    assert!(call(
        json!({"op":"git","projectId":"wp_1","params":{"action":"stage","path":"a.txt"}})
    )
    .is_ok());
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
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                None
            }) else {
                continue;
            };
            &rest[end..]
        } else {
            rest
        };
        if let Some(op) = rest
            .strip_prefix("(\"")
            .and_then(|tail| tail.split('"').next())
        {
            if !ops.contains(&op) {
                ops.push(op);
            }
        }
    }
    assert!(
        ops.contains(&"product.facts") && ops.contains(&"repo.snapshot") && ops.len() >= 15,
        "{ops:?}"
    );
    for op in ops {
        let call =
            json!({"operation":"machine.call","message":{"op":op,"projectId":"wp_1","params":{}}});
        assert!(
            authorize("main", "tauri://localhost/", call).is_ok(),
            "the bridge refuses machine op {op}"
        );
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
        ("checkIndex", json!(-1)),
        ("checkIndex", json!(100)),
        ("checkIndex", json!("0")),
        ("checkName", json!("")),
        ("digest", json!(null)),
        ("resultVersion", json!("0")),
        ("resultVersion", json!(47)),
        ("argv", json!(["true"])),
        ("cwd", json!("/tmp")),
        ("extra", json!(true)),
    ] {
        let mut bad = body.clone();
        bad["message"][key] = value;
        assert!(
            authorize("main", "tauri://localhost/", bad).is_err(),
            "{key}"
        );
    }
    for key in ["checkIndex", "checkName", "digest", "resultVersion"] {
        let mut bad = body.clone();
        bad["message"].as_object_mut().unwrap().remove(key);
        assert!(
            authorize("main", "tauri://localhost/", bad).is_err(),
            "{key}"
        );
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

/// What the bundled web interface sends, read from its own source: every `executions.control` action and `machine.call`
/// operation the real port's tables name (`CONTROL_ACTIONS`, `MACHINE_OPS` in real.ts, the only ones its `control` and
/// `machine` accept), every literal operation it and the desktop window pass to the transport, the wire operations it
/// reads and commands, and its native reads. Each has a representative message here, and the bridge must accept every
/// one: a request the port sends and the bridge refuses would fail only when the owner clicks.
#[test]
fn every_request_the_real_port_sends_is_allowed() {
    let ui = "../../../packages/workspace-ui/src/";
    let read = |path: &str| std::fs::read_to_string(format!("{ui}{path}")).unwrap();
    let real = read("port/real.ts");
    let table = |name: &str| -> Vec<String> {
        let start = real
            .find(&format!("export const {name} = ["))
            .unwrap_or_else(|| panic!("{name} not found in real.ts"));
        let body = &real[start..];
        let body = &body[body.find('[').unwrap() + 1..body.find("] as const").unwrap()];
        body.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    };
    // Every `prefix"name"` in a source, for literal operation names.
    let literals = |source: &str, prefix: &str| -> Vec<String> {
        source
            .match_indices(prefix)
            .filter_map(|(at, _)| source[at + prefix.len()..].split('"').next())
            .filter(|name| !name.is_empty() && !name.ends_with('.'))
            .map(str::to_owned)
            .collect()
    };
    let protocol =
        json!({"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR});
    // As the port builds them: a creation is version 0 of the project; an answer carries the decision's version and owner epoch.
    let command = |operation: &str, payload: Value| {
        let (version, epoch) = match operation {
            "tasks.create" => (json!("0"), Value::Null),
            "decisions.answer" => (json!("5"), json!("2")),
            _ => (json!("1"), Value::Null),
        };
        json!({"protocol":protocol,"requestId":"c1","actorId":"client","projectId":"wp_1","target":"wt_1",
            "operation":operation,"expectedVersion":version,"ownerEpoch":epoch,"payload":payload})
    };
    let native = |operation: &str, payload: Value| json!({"protocol":{"major":2,"mapping":2},"requestId":"read","projectId":"wp_1","operation":operation,"payload":payload});
    let control = |action: &str| -> Value {
        let mut message =
            json!({"projectId":"wp_1","executionId":"we_1","requestId":"r1","action":action});
        let extra = match action {
            "pause" | "resume" => json!({"expectedVersion":"3"}),
            "retry" => json!({}),
            "worker_message" => json!({"workerId":"w1","text":"Keep the old message"}),
            "worker_stop" => json!({"workerId":"w1","reason":"Not needed"}),
            "allow_write" => json!({"path":"test/run.js"}),
            "command_stop" => json!({"toolCallRef":"call-1","reason":"Too slow"}),
            "rerun_check" => {
                json!({"checkIndex":0,"checkName":"npm test","digest":"d","resultVersion":"4"})
            }
            "task_settings" => {
                json!({"settings":{"model":"fable","effort":"medium","mode":"guarded","language":"auto"}})
            }
            "close_task" => json!({"confirmed":true,"expectedVersion":"3"}),
            other => panic!("add a representative executions.control message for {other}"),
        };
        for (k, v) in extra.as_object().unwrap() {
            message[k] = v.clone();
        }
        message
    };
    let sample = |operation: &str| -> Value {
        match operation {
            "service.status"
            | "service.capabilities"
            | "projects.list"
            | "notifications.take"
            | "native.clipboard_image"
            | "native.take_dropped_files"
            | "native.pick_folder"
            | "native.open_logs"
            | "remote.status"
            | "remote.pairing.start"
            | "remote.pairing.cancel" => json!({}),
            "projects.status" | "projects.activate" => json!({"projectId":"wp_1"}),
            "projects.remove" => json!({"projectId":"wp_1","confirmed":true}),
            "projects.rename" => json!({"projectId":"wp_1","displayName":"Cedar"}),
            "projects.register" => {
                json!({"root":"~/code/cedar","displayName":"Cedar","create":true})
            }
            "native.open_editor" => {
                json!({"projectId":"wp_1","checkoutId":"wp_1:main","file":"src/a.ts","line":3})
            }
            "native.save_attachment" => json!({"name":"shot.png","data":"eA=="}),
            "native.engine_login" => json!({"engine":"claude-agent-acp"}),
            "native.open_pull_request" => json!({"projectId":"wp_1","taskId":"wt_1"}),
            "native.query" => native(
                "tasks.page",
                json!({"target":"wp_1","cursor":null,"limit":50}),
            ),
            "native.board" => native("tasks.board", json!({"id":"wp_1"})),
            "executions.prompt" => {
                json!({"projectId":"wp_1","executionId":"we_1","text":"Go on","requestId":"r1"})
            }
            "executions.cancel" => json!({"projectId":"wp_1","executionId":"we_1"}),
            "directories.search" => json!({"session":"pick-1","query":"~/code","limit":30}),
            "directories.cancel" => json!({"session":"pick-1"}),
            "notifications.report" => {
                json!({"focused":{"project":"wp_1","task":"wt_1"},"kinds":["question","stop"]})
            }
            "terminal.open" => json!({"projectId":"wp_1","cols":80,"rows":24}),
            "terminal.read" => json!({"projectId":"wp_1","sessionId":"s1","after":0}),
            "terminal.write" => json!({"projectId":"wp_1","sessionId":"s1","data":"ls\r"}),
            "terminal.resize" => json!({"projectId":"wp_1","sessionId":"s1","cols":100,"rows":30}),
            "terminal.close" => json!({"projectId":"wp_1","sessionId":"s1"}),
            "voice.start" => json!({"requestId":"voice-1"}),
            "voice.read" | "voice.stop" | "voice.cancel" => json!({"sessionId":"v1"}),
            "remote.enable" => json!({"enabled":true}),
            "remote.pairing.decide" => json!({"requestId":"pr1","allow":true}),
            "remote.devices.revoke" => json!({"deviceId":"d1"}),
            "remote.relay.set" => json!({"url":"https://relay.example"}),
            "remote.sudo.set" => json!({"allowed":false}),
            "askpass.list" => json!({}),
            "askpass.answer" => json!({"promptId":"ab12","cancel":true}),
            "owner_actions.list" => json!({}),
            "owner_actions.act" => json!({"projectId":"wp_1","actionId":"oa_1","action":"recheck"}),
            "capabilities.get" => json!({"protocol":protocol,"requestId":"read-capabilities"}),
            "reports.read" => {
                let mut q = query("reports.read");
                q["target"] = json!("r_0123456789ab");
                q["limit"] = json!(4000);
                q
            }
            "decisions.list" | "events.page" | "history.page" | "executions.get"
            | "results.get" | "tasks.list" | "tasks.get" => {
                let mut q = query(operation);
                q["target"] = json!("we_1");
                q
            }
            "project.snapshot" => query(operation),
            "tasks.create" => command(
                "tasks.create",
                json!({"description":"Add a helper","checkout":{"kind":"main"}}),
            ),
            "tasks.start" => command("tasks.start", json!({})),
            // A permission card's answer names one of its two options by id.
            "decisions.answer" => command(
                "decisions.answer",
                json!({"executionId":"we_1","acpSessionId":"s_1","batchId":null,"answers":[{
                "decisionId":"wd_q1","questionId":"q1","expectedVersion":"5","optionId":"wo_allow","text":"Allow once"}]}),
            ),
            other => panic!("add a representative message for {other}"),
        }
    };
    let allowed = |operation: &str, message: Value| {
        authorize(
            "main",
            "tauri://localhost/",
            json!({"operation":operation,"message":message}),
        )
        .map_err(|e| format!("the bridge refuses {operation} {message}: {e}"))
    };
    let mut problems = Vec::new();
    let actions = table("CONTROL_ACTIONS");
    assert!(
        actions.contains(&"allow_write".to_owned()) && actions.len() >= 10,
        "{actions:?}"
    );
    for action in &actions {
        problems.extend(allowed("executions.control", control(action)).err());
    }
    let ops = table("MACHINE_OPS");
    assert!(
        ops.contains(&"product.attachment".to_owned()) && ops.len() >= 25,
        "{ops:?}"
    );
    for op in &ops {
        problems.extend(
            allowed(
                "machine.call",
                json!({"op":op,"projectId":"wp_1","params":{"action":"read"}}),
            )
            .err(),
        );
    }
    // Literal operations the port and the window pass to the transport, the wire reads and native reads it names,
    // the voice and terminal clients, remote access and the window's notifications.
    let mut operations: Vec<String> = literals(&real, "request(\"");
    // A typed wire read: `read("<Type>", "<operation>"`.
    for (at, _) in real.match_indices("read(\"") {
        // Not the tail of a longer name (`nativeRead(`, `.read(`).
        if real[..at].ends_with(|c: char| c.is_alphanumeric() || c == '.' || c == '_') {
            continue;
        }
        let mut parts = real[at..].split('"').skip(1);
        if let (Some(kind), Some(op)) = (parts.next(), parts.nth(1)) {
            if kind.starts_with(char::is_uppercase) {
                operations.push(op.to_owned());
            }
        }
    }
    operations.extend(literals(&read("terminal-client.ts"), "request(\""));
    operations.extend(
        literals(&read("voice-client.ts"), "request(\"")
            .into_iter()
            .map(|op| {
                if op.contains('.') {
                    op
                } else {
                    format!("voice.{op}")
                }
            }),
    );
    operations.extend(
        literals(&read("remote-access.tsx"), "(\"remote.")
            .into_iter()
            .map(|op| format!("remote.{op}")),
    );
    operations.extend(literals(
        &std::fs::read_to_string("../src/main.tsx").unwrap(),
        "request(\"",
    ));
    // Sent with an operation the code chooses: the native reads, the native login and the wire commands.
    operations.extend(
        [
            "native.query",
            "native.board",
            "native.engine_login",
            "projects.activate",
            "projects.register",
            "tasks.create",
            "tasks.start",
            "decisions.answer",
        ]
        .map(str::to_owned),
    );
    operations.retain(|op| !op.is_empty() && op != "machine.call" && op != "executions.control");
    operations.sort();
    operations.dedup();
    for needed in [
        "notifications.report",
        "terminal.open",
        "voice.start",
        "remote.enable",
        "reports.read",
        "decisions.list",
        "history.page",
    ] {
        assert!(
            operations.iter().any(|op| op == needed),
            "{needed} not found in {operations:?}"
        );
    }
    for operation in &operations {
        problems.extend(allowed(operation, sample(operation)).err());
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

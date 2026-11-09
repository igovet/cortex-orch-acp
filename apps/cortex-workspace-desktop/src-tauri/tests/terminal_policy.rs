use cortex_workspace_desktop::policy::authorize;
use serde_json::{json, Value};
fn allowed(method: &str, message: Value) -> bool {
    authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":method,"message":message}),
    )
    .is_ok()
}
#[test]
fn terminal_surface_is_closed_and_owner_window_only() {
    for (method, message) in [
        (
            "terminal.open",
            json!({"projectId":"p","cols":80,"rows":24}),
        ),
        (
            "terminal.read",
            json!({"projectId":"p","sessionId":"s","after":0}),
        ),
        (
            "terminal.write",
            json!({"projectId":"p","sessionId":"s","data":"text\r"}),
        ),
        (
            "terminal.resize",
            json!({"projectId":"p","sessionId":"s","cols":80,"rows":24}),
        ),
        ("terminal.close", json!({"projectId":"p","sessionId":"s"})),
    ] {
        assert!(allowed(method, message.clone()));
        for (window, url) in [
            ("other", "tauri://localhost/"),
            ("main", "https://evil.invalid/"),
        ] {
            assert!(authorize(window, url, json!({"operation":method,"message":message})).is_err());
        }
        for field in [
            "cwd",
            "path",
            "root",
            "command",
            "executable",
            "argv",
            "env",
            "shell",
            "extra",
            "taskId",
        ] {
            let mut injected = message.clone();
            injected[field] = json!("ignored");
            assert!(!allowed(method, injected));
        }
    }
    for method in [
        "terminal.exec",
        "terminal.spawn",
        "terminal.command",
        "terminal.attach",
        "process.exec",
        "shell.execute",
    ] {
        assert!(!allowed(method, json!({"projectId":"p"})));
    }
    for message in [
        json!({"projectId":"p","cols":0,"rows":24}),
        json!({"projectId":"p","cols":80,"rows":501}),
        json!({"projectId":"p","cols":80,"rows":24,"data":"run me"}),
        json!({"projectId":"p"}),
    ] {
        assert!(!allowed("terminal.open", message));
    }
    for data in ["".to_string(), "x".repeat(4097), "🦀".repeat(1025)] {
        assert!(!allowed(
            "terminal.write",
            json!({"projectId":"p","sessionId":"s","data":data})
        ));
    }
    for cursor in [json!(-1), json!(9007199254740992u64), json!("0")] {
        assert!(!allowed(
            "terminal.read",
            json!({"projectId":"p","sessionId":"s","after":cursor})
        ));
    }
    assert!(!allowed(
        "terminal.close",
        json!({"projectId":"p","sessionId":""})
    ));
}

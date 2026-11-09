use cortex_workspace_desktop::policy::authorize;
use serde_json::json;
#[test]
fn voice_is_closed_and_local_window_only() {
    for (method, message) in [
        ("voice.start", json!({"requestId":"r"})),
        ("voice.read", json!({"sessionId":"s"})),
        ("voice.stop", json!({"sessionId":"s"})),
        ("voice.cancel", json!({"sessionId":"s"})),
    ] {
        let call = json!({"operation":method,"message":message});
        assert!(authorize("main","tauri://localhost/",call.clone()).is_ok());
        assert!(authorize("other","tauri://localhost/",call.clone()).is_err());
        assert!(authorize("main","https://remote.invalid/",call).is_err());
        for field in ["path", "audio", "url", "command", "env", "send"] {
            let mut injected = message.clone(); injected[field] = json!("forbidden");
            assert!(authorize("main","tauri://localhost/",json!({"operation":method,"message":injected})).is_err());
        }
    }
    assert!(authorize("main","tauri://localhost/",json!({"operation":"voice.upload","message":{}})).is_err());
}

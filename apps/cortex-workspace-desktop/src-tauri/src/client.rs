//! The desktop's bridge to the machine service: the shared [`workspace_client::Bridge`]
//! over this account's Unix socket, with the service-manager start and the desktop-only
//! operations (native login, editor and log openers, the native capability entries).
use crate::opener;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::UnixStream, time::timeout};
pub use workspace_client::bridge::{frame_failure, incoming, Incoming, Refused, ORDINARY_REQUESTS};
use workspace_client::{BoxFuture, Connector, Stream};
use workspace_service::connect;

/// Marks an error as the machine's own answer, so a caller can tell it from a reply that never came.
const REFUSED: &str = "refused:";
const STARTUP_DEADLINE: Duration = workspace_client::bridge::STARTUP_DEADLINE;

/// This account's service socket, started through the user service manager when absent.
struct Unix {
    socket: PathBuf,
    start_unit: bool,
    starting: tokio::sync::Mutex<()>,
}
impl Unix {
    async fn connection(&self) -> Result<UnixStream, String> {
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        if !self.start_unit {
            return Err("service_unavailable".into());
        }
        // Serialize startup, never replay the caller's RPC or expose a shell to JS.
        let _guard = self.starting.lock().await;
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        Bridge::start_service(
            "/usr/bin/systemctl",
            &workspace_protocol::channel::current().unit,
        )
        .await?;
        self.wait_for_socket(STARTUP_DEADLINE).await
    }
    async fn wait_for_socket(&self, grace: Duration) -> Result<UnixStream, String> {
        let deadline = tokio::time::Instant::now() + grace;
        while tokio::time::Instant::now() < deadline {
            if let Ok(stream) = connect(&self.socket).await {
                return Ok(stream);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("service_start_timeout".into())
    }
}
impl Connector for Unix {
    fn connect(&self) -> BoxFuture<'_, Result<Stream, String>> {
        Box::pin(async move { Ok(Box::new(self.connection().await?) as Stream) })
    }
}

pub struct Bridge {
    shared: workspace_client::Bridge,
    /// The socket connector itself, for the late-socket wait the tests drive directly.
    #[cfg_attr(not(test), allow(dead_code))]
    unix: Arc<Unix>,
}
impl Bridge {
    /// The socket is resolved by trusted native startup, never a JavaScript argument.
    pub fn for_user() -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("service_unavailable")?;
        if !PathBuf::from(&runtime).is_absolute() {
            return Err("service_unavailable".into());
        }
        Ok(Self::with(
            workspace_protocol::channel::current()
                .runtime_dir(&PathBuf::from(runtime))
                .join("service.sock"),
            true,
        ))
    }
    pub fn new(socket: PathBuf) -> Self {
        Self::with(socket, false)
    }
    fn with(socket: PathBuf, start_unit: bool) -> Self {
        let unix = Arc::new(Unix {
            socket,
            start_unit,
            starting: tokio::sync::Mutex::new(()),
        });
        Self {
            shared: workspace_client::Bridge::new(unix.clone()),
            unix,
        }
    }
    /// The shared bridge this desktop bridge wraps.
    pub fn shared(&self) -> &workspace_client::Bridge {
        &self.shared
    }
    #[cfg(test)]
    async fn wait_for_socket(&self, grace: Duration) -> Result<UnixStream, String> {
        self.unix.wait_for_socket(grace).await
    }
    async fn start_service(systemctl: &str, unit: &str) -> Result<(), String> {
        use std::process::Stdio;
        // Reload also handles first launch in a manager that predates package install.
        for args in [
            vec!["--user", "daemon-reload"],
            vec!["--user", "start", "--no-block", unit],
        ] {
            let mut child = tokio::process::Command::new(systemctl)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| "service_manager_unavailable")?;
            let status = timeout(Duration::from_secs(8), child.wait())
                .await
                .map_err(|_| "service_manager_timeout")?
                .map_err(|_| "service_manager_unavailable")?;
            if !status.success() {
                return Err("service_start_failed".into());
            }
        }
        Ok(())
    }
    /// One attempt on a fresh authenticated connection. A timeout drops this connection;
    /// nothing retries a mutation or lets a late response satisfy another request.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        self.shared.rpc(method, params).await
    }
    pub async fn request(&self, call: crate::policy::Call) -> Result<Value, String> {
        match call.operation.as_str() {
            "service.capabilities" => {
                let mut value = self.rpc("service.capabilities", call.message).await?;
                if let Some(methods) = value["methods"].as_array_mut() {
                    methods.push(json!("native.clipboard_image"));
                    methods.push(json!("native.take_dropped_files"));
                    methods.push(json!("native.pick_folder"));
                    methods.push(json!("native.save_attachment"));
                }
                let vs_code = opener::vs_code().is_some();
                let system = opener::system_opener().is_some();
                if let Some(methods) = value["methods"].as_array_mut() {
                    if vs_code {
                        methods.push(json!("native.open_editor.vscode"));
                    }
                    if vs_code || system {
                        methods.push(json!("native.open_editor"));
                    }
                    if system {
                        methods.push(json!("native.open_logs"));
                    }
                }
                Ok(value)
            }
            "native.engine_login" => {
                let reply = self.rpc("machine.call", json!({"op":"engine.connection","params":{"action":"login_command","engine":call.message["engine"]}})).await?;
                if reply["ok"] != true {
                    return Err(format!(
                        "{REFUSED}{}",
                        reply["message"]
                            .as_str()
                            .unwrap_or("Native login is unavailable.")
                    ));
                }
                let program = reply["result"]["program"]
                    .as_str()
                    .ok_or("Native login is unavailable.")?;
                let args: Vec<std::ffi::OsString> = reply["result"]["args"]
                    .as_array()
                    .ok_or("Native login is unavailable.")?
                    .iter()
                    .map(|v| v.as_str().unwrap_or("").into())
                    .collect();
                opener::engine_login(program, &args)
                    .await
                    .map_err(|s| format!("{REFUSED}{s}"))?;
                Ok(
                    json!({"message":"Native login opened in a terminal. Finish there, then recheck here."}),
                )
            }
            "native.open_editor" => self
                .open_editor(&call.message)
                .await
                .map_err(|text| format!("{REFUSED}{text}")),
            "native.open_logs" => self
                .open_logs()
                .await
                .map_err(|text| format!("{REFUSED}{text}")),
            _ => self.shared.request(call).await,
        }
    }
    /// Where to open, as the service resolves it. A plain project opens without Git; a checkout or file needs its answer.
    async fn located(&self, message: &Value) -> Result<(String, Option<String>), String> {
        if message.get("checkoutId").is_none() && message.get("file").is_none() {
            let projects = self.rpc("projects.list", json!({})).await?;
            let project = projects
                .as_array()
                .and_then(|rows| rows.iter().find(|p| p["projectId"] == message["projectId"]))
                .ok_or("This project is no longer on this machine.")?;
            return Ok((
                project["canonicalRoot"]
                    .as_str()
                    .ok_or("This project has no folder.")?
                    .to_owned(),
                None,
            ));
        }
        let mut params = serde_json::Map::new();
        for key in ["checkoutId", "file"] {
            if let Some(value) = message.get(key) {
                params.insert(key.to_owned(), value.clone());
            }
        }
        let reply = self
            .rpc(
                "machine.call",
                json!({"op":"repo.locate","projectId":message["projectId"],"params":params}),
            )
            .await?;
        if reply["ok"] != true {
            return Err(reply["message"]
                .as_str()
                .unwrap_or("This file could not be found.")
                .to_owned());
        }
        let root = reply["result"]["root"]
            .as_str()
            .ok_or("This file could not be found.")?
            .to_owned();
        Ok((root, reply["result"]["path"].as_str().map(str::to_owned)))
    }
    async fn open_editor(&self, message: &Value) -> Result<Value, String> {
        let (root, file) = self.located(message).await?;
        if let Some(editor) = opener::vs_code() {
            let args = opener::vs_code_args(
                &root,
                file.as_deref(),
                message["line"].as_u64().unwrap_or(1),
            );
            opener::launch(&editor, &args, "VS Code could not open this.").await?;
        } else {
            let (program, args) = opener::system_opener()
                .ok_or("No external editor or file opener is installed on this machine.")?;
            let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
            args.push(file.unwrap_or(root).into());
            opener::launch(
                &program,
                &args,
                "The default application could not open this.",
            )
            .await?;
        }
        Ok(json!({"outcome":"opened"}))
    }
    async fn open_logs(&self) -> Result<Value, String> {
        let reply = self
            .rpc("machine.call", json!({"op":"logs.folder"}))
            .await?;
        if reply["ok"] != true {
            return Err("Private service logs are unavailable.".into());
        }
        let folder = reply["result"]["folder"]
            .as_str()
            .ok_or("Private service logs are unavailable.")?;
        let (program, args) =
            opener::system_opener().ok_or("No folder opener is installed on this machine.")?;
        let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        args.push(folder.into());
        opener::launch(&program, &args, "The folder opener could not open logs.").await?;
        Ok(json!({"outcome":"opened"}))
    }
    pub async fn subscribe(&self, notify: impl Fn(Value)) {
        self.shared.subscribe(notify).await
    }
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn first_subscription_waits_for_a_late_socket_without_reporting_offline() {
        let root = std::env::temp_dir().join(format!("cw-late-ready-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let bridge = Bridge::new(path.clone());
        assert!(bridge
            .wait_for_socket(std::time::Duration::from_millis(20))
            .await
            .is_err());
        let events = std::sync::Mutex::new(Vec::new());
        let publish = |event| events.lock().unwrap().push(event);
        let subscription = bridge.subscribe(publish);
        tokio::pin!(subscription);
        let server = async {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            // Incoming fake subscription/status requests use the real authenticated framing.
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let request = workspace_service::read_frame(&mut stream).await.unwrap();
                    let result = if request["method"] == "projects.list" {
                        serde_json::json!([])
                    } else {
                        serde_json::json!({"subscribed":true})
                    };
                    workspace_service::write_frame(
                        &mut stream,
                        &serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                    )
                    .await
                    .unwrap();
                    std::future::pending::<()>().await;
                });
            }
        };
        tokio::pin!(server);
        tokio::select! {
            _ = &mut server => unreachable!(),
            _ = &mut subscription => panic!("subscription ended"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
        }
        let seen = events.lock().unwrap();
        assert!(seen.iter().any(|e| e["state"] == "connecting"));
        assert!(seen.iter().any(|e| e["state"] == "connected"));
        assert!(!seen.iter().any(|e| e["state"] == "disconnected"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn startup_reloads_and_starts_only_the_fixed_unit_and_retries_after_failure() {
        let root = std::env::temp_dir().join(format!("cw-start-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("systemctl");
        let trace = root.join("trace");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                trace.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            Bridge::start_service(script.to_str().unwrap(), "fixture.service")
                .await
                .is_err()
        );
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                trace.display()
            ),
        )
        .unwrap();
        Bridge::start_service(script.to_str().unwrap(), "fixture.service")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&trace).unwrap(),
            "--user daemon-reload\n--user daemon-reload\n--user start --no-block fixture.service\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

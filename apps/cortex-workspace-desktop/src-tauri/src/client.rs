//! The desktop's bridge to the machine service: the shared [`workspace_client::Bridge`]
//! over this account's Unix socket, with the service-manager start and the desktop-only
//! operations (native login, editor, file manager and log openers, native capabilities).
use crate::opener;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::UnixStream, process::Command, time::timeout};
pub use workspace_client::bridge::{frame_failure, incoming, Incoming, Refused, ORDINARY_REQUESTS};
use workspace_client::{BoxFuture, Connector, Stream};
use workspace_service::{
    connect,
    dev_extraction::{DevExtraction, OpenerCapture},
};

/// Marks an error as the machine's own answer, so a caller can tell it from a reply that never came.
const REFUSED: &str = "refused:";
fn add_reveal_capability(methods: &mut Vec<Value>, file_manager: bool, system: bool) {
    if file_manager || system {
        methods.push(json!("native.reveal_file"));
    }
}

const STARTUP_DEADLINE: Duration = workspace_client::bridge::STARTUP_DEADLINE;

/// This account's service socket, started through the user service manager when absent.
struct Unix {
    socket: PathBuf,
    startup: Startup,
    starting: tokio::sync::Mutex<()>,
}
#[derive(Clone, Copy)]
enum Startup {
    Installed,
    SocketOnly,
    Extracted,
}
impl Unix {
    async fn connection(&self) -> Result<UnixStream, String> {
        self.connection_with(|| Command::new("/usr/bin/systemctl"))
            .await
    }
    async fn connection_with(&self, systemctl: impl Fn() -> Command) -> Result<UnixStream, String> {
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        match self.startup {
            Startup::Installed => (),
            Startup::SocketOnly => return Err("service_unavailable".into()),
            Startup::Extracted => return Err("extracted_dev_service_unavailable".into()),
        }
        // Serialize startup, never replay the caller's RPC or expose a shell to JS.
        let _guard = self.starting.lock().await;
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        Bridge::start_service(systemctl, &workspace_protocol::channel::current().unit).await?;
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
    opener_capture: Option<OpenerCapture>,
    /// The socket connector itself, for the late-socket wait the tests drive directly.
    #[cfg_attr(not(test), allow(dead_code))]
    unix: Arc<Unix>,
}
impl Bridge {
    /// The socket is resolved by trusted native startup, never a JavaScript argument.
    pub fn for_user() -> Result<Self, String> {
        if let Some(extraction) = DevExtraction::from_env().map_err(|error| error.to_string())? {
            let mut bridge = Self::with(
                extraction.runtime_dir().join("service.sock"),
                Startup::Extracted,
            );
            bridge.opener_capture = extraction
                .qualification_opener()
                .map_err(|_| "qualification_opener_invalid")?;
            return Ok(bridge);
        }
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("service_unavailable")?;
        if !PathBuf::from(&runtime).is_absolute() {
            return Err("service_unavailable".into());
        }
        Ok(Self::with(
            workspace_protocol::channel::current()
                .runtime_dir(&PathBuf::from(runtime))
                .join("service.sock"),
            Startup::Installed,
        ))
    }
    pub fn new(socket: PathBuf) -> Self {
        Self::with(socket, Startup::SocketOnly)
    }
    fn with(socket: PathBuf, startup: Startup) -> Self {
        let unix = Arc::new(Unix {
            socket,
            startup,
            starting: tokio::sync::Mutex::new(()),
        });
        Self {
            shared: workspace_client::Bridge::new(unix.clone()),
            opener_capture: None,
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
    async fn start_service(systemctl: impl Fn() -> Command, unit: &str) -> Result<(), String> {
        use std::process::Stdio;
        // Reload also handles first launch in a manager that predates package install.
        for args in [
            vec!["--user", "daemon-reload"],
            vec!["--user", "start", "--no-block", unit],
        ] {
            let mut child = systemctl()
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
                let file_manager = !system && opener::file_manager_available().await;
                if let Some(methods) = value["methods"].as_array_mut() {
                    add_reveal_capability(methods, file_manager, system);
                    if vs_code {
                        methods.push(json!("native.open_editor.vscode"));
                    }
                    if vs_code || system {
                        methods.push(json!("native.open_editor"));
                    }
                    if system {
                        methods.push(json!("native.open_logs"));
                    }
                    let pr_opener = match self.unix.startup {
                        Startup::Extracted => self.opener_capture.is_some(),
                        _ => system,
                    };
                    if pr_opener {
                        methods.push(json!("native.open_pull_request"));
                        methods.push(json!("native.open_mcp_signin"));
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
            "native.reveal_file" => self
                .reveal_file(&call.message)
                .await
                .map_err(|text| format!("{REFUSED}{text}")),
            "native.open_logs" => self
                .open_logs()
                .await
                .map_err(|text| format!("{REFUSED}{text}")),
            "native.open_pull_request" => self
                .open_pull_request(&call.message)
                .await
                .map_err(|text| format!("{REFUSED}{text}")),
            "native.open_mcp_signin" => self
                .open_mcp_signin(&call.message)
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
    async fn reveal_file(&self, message: &Value) -> Result<Value, String> {
        let (_, file) = self.located(message).await?;
        let file = file.as_deref().ok_or("This file could not be found.")?;
        opener::reveal_file(file).await?;
        Ok(json!({"outcome":"opened"}))
    }

    /// Same handoff used by the real View PR call. An extracted app must never fall
    /// through to an installed opener, including when no recorder was configured.
    fn capture_pull_request(&self, url: &str) -> Result<bool, String> {
        if !matches!(self.unix.startup, Startup::Extracted) {
            return Ok(false);
        }
        let capture = self
            .opener_capture
            .as_ref()
            .ok_or("qualification_opener_missing")?;
        capture
            .record(url)
            .map_err(|_| "qualification_opener_write_failed")?;
        Ok(true)
    }

    #[cfg(any(feature = "desktop", test))]
    pub(crate) fn probe_qualification_opener(&self) -> Result<(), String> {
        match self
            .capture_pull_request("https://qualification.invalid/cortex/view-pr?exact=1&arg=2")?
        {
            true => Ok(()),
            false => Err("qualification_opener_inactive".into()),
        }
    }

    /// A task's pull request in the system browser. The address comes from the service, which
    /// recorded it from GitHub CLI and checks it against the repository's remote host; it is
    /// checked here again to be one plain https address before any program sees it.
    async fn open_pull_request(&self, message: &Value) -> Result<Value, String> {
        let reply = self
            .rpc(
                "machine.call",
                json!({"op":"repo.locate","projectId":message["projectId"],"params":{"pullRequest":message["taskId"]}}),
            )
            .await?;
        if reply["ok"] != true {
            return Err(reply["message"]
                .as_str()
                .unwrap_or("This pull request could not be found.")
                .to_owned());
        }
        let url = reply["result"]["url"]
            .as_str()
            .filter(|u| opener::web_address(u))
            .ok_or(
                "This pull request's address is not a plain https address, so it was not opened.",
            )?;
        if self.capture_pull_request(url)? {
            return Ok(json!({"outcome":"opened"}));
        }
        let (program, args) =
            opener::system_opener().ok_or("No browser opener is installed on this machine.")?;
        let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        args.push(url.into());
        opener::launch(
            &program,
            &args,
            "The browser could not open this pull request.",
        )
        .await?;
        Ok(json!({"outcome":"opened"}))
    }
    /// A machine MCP sign-in in the system browser. The service holds the address; it is
    /// checked here to be one plain https address before any program sees it.
    async fn open_mcp_signin(&self, message: &Value) -> Result<Value, String> {
        let reply = self
            .rpc(
                "owner_actions.act",
                json!({"projectId":"machine","actionId":message["actionId"],"action":"open"}),
            )
            .await
            .map_err(|_| "This sign-in is no longer waiting.".to_owned())?;
        let url = reply["url"]
            .as_str()
            .filter(|u| opener::web_address(u))
            .ok_or("The sign-in address is not available yet or is not a plain https address.")?;
        if self.capture_pull_request(url)? {
            return Ok(json!({"outcome":"opened"}));
        }
        let (program, args) =
            opener::system_opener().ok_or("No browser opener is installed on this machine.")?;
        let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        args.push(url.into());
        opener::launch(&program, &args, "The browser could not open this sign-in.").await?;
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
    use super::{add_reveal_capability, Bridge, Startup};
    use serde_json::json;
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::Path,
    };
    use tokio::process::Command;

    #[tokio::test]
    async fn first_subscription_waits_for_a_late_socket_without_reporting_offline() {
        // The runner's TMPDIR can be too long for a filesystem Unix socket.
        let root = Path::new("/tmp").join(format!("cw-late-ready-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
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

    #[test]
    fn qualification_opener_is_mandatory_for_extracted_pr_handoffs() {
        let socket = Path::new("/nonexistent/service.sock").to_owned();
        let extracted = Bridge::with(socket.clone(), Startup::Extracted);
        assert_eq!(
            extracted.probe_qualification_opener(),
            Err("qualification_opener_missing".into())
        );
        let ordinary = Bridge::with(socket, Startup::Installed);
        assert_eq!(
            ordinary.probe_qualification_opener(),
            Err("qualification_opener_inactive".into())
        );
    }

    #[tokio::test]
    async fn extracted_dev_connects_only_to_its_socket_and_never_starts_the_installed_unit() {
        let root = Path::new("/tmp").join(format!("cw-isolated-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let path = root.join("service.sock");
        let bridge = Bridge::with(path.clone(), Startup::Extracted);
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let no_systemctl = || panic!("an extracted desktop must not invoke systemctl");
        bridge.unix.connection_with(no_systemctl).await.unwrap();
        drop(listener);
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            bridge.unix.connection_with(no_systemctl).await.unwrap_err(),
            "extracted_dev_service_unavailable"
        );
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn reveal_capability_requires_a_file_manager_or_system_opener() {
        for (file_manager, system) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut methods = vec![json!("native.open_editor")];
            add_reveal_capability(&mut methods, file_manager, system);
            let expected = usize::from(file_manager || system);
            assert_eq!(
                methods.iter().filter(|m| **m == "native.reveal_file").count(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn startup_reloads_and_starts_only_the_fixed_unit_and_retries_after_failure() {
        let root = std::env::temp_dir().join(format!("cw-start-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let script = root.join("systemctl");
        let trace = root.join("trace");
        let mut fixture = std::fs::File::create(&script).unwrap();
        fixture
            .write_all(
                b"trace=$1\nstatus=$2\nshift 2\nprintf '%s\\n' \"$*\" >> \"$trace\" || exit 2\nexit \"$status\"\n",
            )
            .unwrap();
        // Invoke the fixture as data. A concurrent spawn can inherit a writer until exec
        // and make directly executing the script fail with ETXTBSY. Keep a writer open
        // across both attempts so this test exercises that condition deterministically.
        let systemctl = |status: &str| {
            let mut command = Command::new("/bin/sh");
            command.arg(&script).arg(&trace).arg(status);
            command
        };
        assert_eq!(
            Bridge::start_service(|| systemctl("1"), "fixture.service").await,
            Err("service_start_failed".into())
        );
        Bridge::start_service(|| systemctl("0"), "fixture.service")
            .await
            .unwrap();
        drop(fixture);
        assert_eq!(
            std::fs::read_to_string(&trace).unwrap(),
            "--user daemon-reload\n--user daemon-reload\n--user start --no-block fixture.service\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

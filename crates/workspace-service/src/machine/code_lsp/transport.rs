//! Bounded LSP stdio. Inbound methods are handled locally, never relayed.
use super::{file_uri, unavailable, Language, Result, SETTINGS};
use crate::native_command_sandbox::Sandbox;
use serde_json::{json, Value};
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, Semaphore},
    task::JoinHandle,
    time::timeout,
};

const FRAME: usize = 1024 * 1024;
const WRITE: Duration = Duration::from_secs(2);
const INIT: Duration = Duration::from_secs(8);
const BUNDLE: &str = "/opt/cortex-code-lsp";
const NODE: &str = "/opt/cortex-code-node";
const OVERRIDE: &str = "/opt/cortex-dev-language-server";
const TS_REQUEST: &str = "typescript.tsserverRequest";
static PREPARE_LIMIT: Semaphore = Semaphore::const_new(2);

#[derive(Clone)]
pub(super) struct Program {
    mounts: Vec<(PathBuf, PathBuf)>,
    argv: Vec<String>,
    tsserver: Option<String>,
    #[cfg(test)]
    pub(super) fixture: Option<Vec<String>>,
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
impl Program {
    pub(super) fn resolve(
        language: Language,
        dev: bool,
        settings: &Value,
        runtime: Option<&Path>,
    ) -> Result<Self> {
        let configured = dev
            .then(|| {
                settings[match language {
                    Language::Rust => SETTINGS[0],
                    Language::TypeScript => SETTINGS[1],
                }]
                .as_str()
            })
            .flatten();
        let bundle = runtime
            .and_then(Path::parent)
            .map(|p| p.join("engines/lsp"));
        let ts_bundle = bundle
            .as_ref()
            .filter(|p| p.join("typescript/lib/tsserver.js").is_file());
        if let Some((path, sandbox_path)) = configured
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .and_then(|p| {
                let basename = p.file_name()?.to_str()?;
                let real = p.canonicalize().ok()?;
                executable(&real).then(|| (real, format!("{OVERRIDE}/{basename}")))
            })
        {
            // Keep argv[0]'s basename for multicall executables (e.g. rustup).
            let mut mounts = vec![(path, PathBuf::from(&sandbox_path))];
            let tsserver = (language == Language::TypeScript)
                .then_some(ts_bundle)
                .flatten()
                .map(|path| {
                    mounts.push((path.clone(), BUNDLE.into()));
                    format!("{BUNDLE}/typescript/lib/tsserver.js")
                });
            return Ok(Self {
                mounts,
                argv: if language == Language::Rust {
                    vec![sandbox_path]
                } else {
                    vec![sandbox_path, "--stdio".into()]
                },
                tsserver,
                #[cfg(test)]
                fixture: None,
            });
        }
        let missing = || {
            unavailable(if configured.is_some() {
                "dev_override_invalid"
            } else {
                "server_not_installed"
            })
        };
        let bundle = bundle.ok_or_else(missing)?;
        match language {
            Language::Rust if executable(&bundle.join("rust-analyzer")) => Ok(Self {
                mounts: vec![(
                    bundle.join("rust-analyzer"),
                    format!("{BUNDLE}/rust-analyzer").into(),
                )],
                argv: vec![format!("{BUNDLE}/rust-analyzer")],
                tsserver: None,
                #[cfg(test)]
                fixture: None,
            }),
            Language::TypeScript => {
                let node = bundle.parent().unwrap().join("node/bin/node");
                if !executable(&node)
                    || !bundle
                        .join("typescript-language-server/lib/cli.mjs")
                        .is_file()
                    || !bundle.join("typescript/lib/tsserver.js").is_file()
                {
                    return Err(missing());
                }
                Ok(Self {
                    mounts: vec![(bundle, BUNDLE.into()), (node, NODE.into())],
                    argv: vec![
                        NODE.into(),
                        format!("{BUNDLE}/typescript-language-server/lib/cli.mjs"),
                        "--stdio".into(),
                    ],
                    tsserver: Some(format!("{BUNDLE}/typescript/lib/tsserver.js")),
                    #[cfg(test)]
                    fixture: None,
                })
            }
            _ => Err(missing()),
        }
    }

    #[cfg(test)]
    pub(super) fn fixture(argv: Vec<String>) -> Self {
        Self {
            mounts: Vec::new(),
            argv: Vec::new(),
            tsserver: None,
            fixture: Some(argv),
        }
    }

    async fn spawn(
        self,
        root: &Path,
        language: Language,
    ) -> std::result::Result<(Child, Option<Sandbox>), &'static str> {
        #[cfg(test)]
        if let Some(argv) = &self.fixture {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]).current_dir(root);
            return spawn(command)
                .map(|child| (child, None))
                .map_err(|_| "start_failed");
        }
        let root = root.to_path_buf();
        let rust = language == Language::Rust;
        let (sandbox, command) = timeout(INIT, async move {
            let permit = PREPARE_LIMIT
                .acquire()
                .await
                .map_err(|_| "sandbox_unavailable")?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let sandbox = Sandbox::prepare_language(&root, &self.mounts, rust)?;
                let command = sandbox.command(&self.argv)?;
                io::Result::Ok((sandbox, command))
            })
            .await
            .map_err(|_| "sandbox_unavailable")?
            .map_err(|_| "sandbox_unavailable")
        })
        .await
        .map_err(|_| "sandbox_timeout")??;
        Ok((
            spawn(command.into()).map_err(|_| "start_failed")?,
            Some(sandbox),
        ))
    }
}

fn spawn(mut command: Command) -> io::Result<Child> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

pub(super) struct Peer {
    // The PID namespace is torn down when bwrap exits, including descendants.
    _child: Child,
    _sandbox: Option<Sandbox>,
    stdin: ChildStdin,
    reader: JoinHandle<()>,
    rx: mpsc::Receiver<Value>,
    sequence: u64,
    pub(super) sync_kind: u8,
    pub(super) ts_diagnostics: bool,
    configuration: Value,
    root_uri: String,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self._child.start_kill();
    }
}

impl Peer {
    pub(super) async fn start(
        program: Program,
        root: &Path,
        language: Language,
    ) -> std::result::Result<Self, &'static str> {
        let configuration = match language {
            Language::Rust => json!({
                "checkOnSave":false, "procMacro":{"enable":false},
                "cargo":{"buildScripts":{"enable":false},"extraEnv":{"CARGO_NET_OFFLINE":"true"}},
                "diagnostics":{"enable":true}, "cachePriming":{"enable":false}, "numThreads":4,
            }),
            Language::TypeScript => {
                let mut config = json!({"disableAutomaticTypingAcquisition":true,"maxTsServerMemory":512,"plugins":[]});
                if let Some(path) = &program.tsserver {
                    config["tsserver"] = json!({"path":path});
                }
                config
            }
        };
        let root_uri = file_uri(root);
        let (mut child, sandbox) = program.spawn(root, language).await?;
        let stdin = child.stdin.take().ok_or("start_failed")?;
        let stdout = child.stdout.take().ok_or("start_failed")?;
        let (send, rx) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            while let Ok(message) = read(&mut stdout).await {
                if send.send(message).await.is_err() {
                    break;
                }
            }
        });
        let mut peer = Self {
            _child: child,
            _sandbox: sandbox,
            stdin,
            reader,
            rx,
            sequence: 0,
            sync_kind: 0,
            ts_diagnostics: false,
            configuration,
            root_uri,
        };
        let id = peer.request("initialize", json!({
            "processId":null, "rootUri":peer.root_uri,
            "workspaceFolders":[{"uri":peer.root_uri,"name":"checkout"}],
            "capabilities":{
                "general":{"positionEncodings":["utf-16"]},
                "textDocument":{"publishDiagnostics":{"versionSupport":true},"definition":{"linkSupport":true}},
                "workspace":{"configuration":true,"workspaceFolders":true,"applyEdit":false},
            },
            "initializationOptions":peer.configuration,
        })).await.map_err(|_| "start_failed")?;
        timeout(INIT, async {
            loop {
                let message = peer.recv().await.ok_or("start_failed")?;
                if message["id"] == id && message.get("method").is_none() {
                    let caps = &message["result"]["capabilities"];
                    if !caps.is_object() {
                        return Err("initialization_failed");
                    }
                    if !caps["positionEncoding"].is_null() && caps["positionEncoding"] != "utf-16" {
                        return Err("unsupported_position_encoding");
                    }
                    let sync = if caps["textDocumentSync"].is_number() {
                        &caps["textDocumentSync"]
                    } else {
                        &caps["textDocumentSync"]["change"]
                    };
                    peer.sync_kind = match sync.as_u64() {
                        Some(1) => 1,
                        Some(2) => 2,
                        _ => return Err("unsupported_document_sync"),
                    };
                    peer.ts_diagnostics = language == Language::TypeScript
                        && caps["executeCommandProvider"]["commands"]
                            .as_array()
                            .is_some_and(|commands| commands.iter().any(|c| c == TS_REQUEST));
                    break;
                }
                peer.server_request(&message)
                    .await
                    .map_err(|_| "initialization_failed")?;
            }
            peer.notify("initialized", json!({}))
                .await
                .map_err(|_| "initialization_failed")?;
            Ok(peer)
        })
        .await
        .map_err(|_| "initialization_timeout")?
    }

    pub(super) async fn recv(&mut self) -> Option<Value> {
        self.rx.recv().await
    }

    pub(super) async fn request(&mut self, method: &str, params: Value) -> io::Result<u64> {
        self.sequence += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.sequence,"method":method,"params":params}))
            .await?;
        Ok(self.sequence)
    }

    pub(super) async fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }

    pub(super) async fn ts_diagnostic_request(
        &mut self,
        uri: &str,
        kind: super::TsDiagnosticKind,
    ) -> io::Result<u64> {
        self.request(
            "workspace/executeCommand",
            json!({"command":TS_REQUEST,
                "arguments":[kind.command(), {"file":uri,"includeLinePosition":true}]}),
        )
        .await
    }

    pub(super) async fn server_request(&mut self, message: &Value) -> io::Result<()> {
        let Some(method) = message["method"].as_str() else {
            return Ok(());
        };
        let Some(id) = message
            .get("id")
            .filter(|i| i.is_number() || i.as_str().is_some_and(|s| s.len() <= 128))
        else {
            return Ok(());
        };
        let response = match method {
            "workspace/workspaceFolders" => {
                json!({"result":[{"uri":self.root_uri,"name":"checkout"}]})
            }
            "workspace/configuration" => {
                let values: Vec<_> = message["params"]["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(32)
                    .map(|item| {
                        let Some(section) = item["section"].as_str() else {
                            return Value::Null;
                        };
                        if section == "rust-analyzer" {
                            self.configuration.clone()
                        } else {
                            Value::Null
                        }
                    })
                    .collect();
                json!({"result":values})
            }
            "workspace/applyEdit" => {
                json!({"result":{"applied":false,"failureReason":"Code language sessions are read-only."}})
            }
            _ => json!({"error":{"code":-32601,"message":"Unsupported client method"}}),
        };
        let mut response = response;
        response["jsonrpc"] = json!("2.0");
        response["id"] = id.clone();
        self.send(response).await
    }

    async fn send(&mut self, message: Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(&message)?;
        if bytes.len() > 4 * FRAME {
            return Err(io::Error::other("message too large"));
        }
        timeout(WRITE, async {
            self.stdin
                .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
                .await?;
            self.stdin.write_all(&bytes).await?;
            self.stdin.flush().await
        })
        .await?
    }
}

pub(super) async fn next(peer: &mut Option<Peer>) -> Option<Value> {
    match peer {
        Some(peer) => peer.recv().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn read(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Value> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid language frame");
    let mut length = None;
    let mut header = Vec::new();
    let mut terminated = false;
    for _ in 0..8 {
        header.clear();
        let n = (&mut *reader)
            .take(1024)
            .read_until(b'\n', &mut header)
            .await?;
        if n < 2 || !header.ends_with(b"\r\n") {
            return Err(invalid());
        }
        if header == b"\r\n" {
            terminated = true;
            break;
        }
        let line = std::str::from_utf8(&header[..n - 2]).map_err(|_| invalid())?;
        let (name, val) = line.split_once(':').ok_or_else(invalid)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid());
            }
            length = Some(val.trim().parse::<usize>().map_err(|_| invalid())?);
        }
    }
    let length = length
        .filter(|n| terminated && (1..=FRAME).contains(n))
        .ok_or_else(invalid)?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["jsonrpc"] != "2.0" {
        return Err(invalid());
    }
    Ok(value)
}

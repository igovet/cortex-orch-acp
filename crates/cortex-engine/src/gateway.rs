//! Channel identity is supplied by the broker, never arguments or _meta.
//! Python oracle: src/cortex_runtime/server.py:167-213,248-345;
//! src/cortex_runtime/tool_read_api.py:66-96.
//! Core must revalidate and execute atomically, including receipt/read-evidence policy.
use crate::{schema, wire, Error, FutureResult, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
pub struct Credentials {
    pub channel_id: String,
    pub project: PathBuf,
    secret: String,
}
impl Credentials {
    pub fn new(channel_id: String, project: PathBuf, secret: String) -> Result<Self> {
        if !regex::Regex::new("^ch_[0-9a-f]{12}$")
            .unwrap()
            .is_match(&channel_id)
            || !project.is_absolute()
            || !regex::Regex::new("^[A-Za-z0-9_-]{16,256}$")
                .unwrap()
                .is_match(&secret)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            channel_id,
            project,
            secret,
        })
    }
    /// Only the trusted Core authenticator receives this credential.
    pub fn secret(&self) -> &str {
        &self.secret
    }
}
#[derive(Clone)]
pub struct Channel {
    pub task: String,
    pub attempt: String,
    pub owner_epoch: u64,
    pub principal: String,
    pub role: String,
    pub profile: Option<String>,
    pub stage: Option<String>,
}
/// Implement in Core/service. Authenticate checks secret, expiry, owner epoch and
/// current assignment; execute repeats those checks in its mutation transaction.
/// Core owns requests, file safety, consent, command receipts and publication.
/// No default/allow-all implementation is provided.
pub trait GatewayCore: Send + Sync + 'static {
    /// A partially migrated host advertises only its implemented tool handlers.
    /// This can narrow the role catalogue, never grant a tool outside it.
    fn supports(&self, _name: &str) -> bool {
        true
    }
    /// Additional stage-specific restriction; the fixed role catalogue and
    /// supports() still apply, so this hook cannot widen authority.
    fn supports_channel(&self, _channel: &Channel, _name: &str) -> bool {
        true
    }
    fn authenticate<'a>(&'a self, credentials: &'a Credentials) -> FutureResult<'a, Channel>;
    /// A call the gateway refused before `execute` (arguments outside the advertised
    /// schema, a tool not offered). A host may count these: a model that repeats such a
    /// refusal never reaches `execute`.
    fn call_rejected(&self, _name: &str, _arguments: &Value, _code: &str) {}
    fn execute<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        name: &'a str,
        arguments: Value,
        request_id: Value,
    ) -> FutureResult<'a, Value>;
}
/// A short corrective pointer for a named authority refusal. It never names
/// another task, a path outside the project or a credential.
pub fn refusal_hint(code: &str) -> &'static str {
    match code {
        "tool_not_offered" => "This tool is not offered to this channel's role and stage. Use only the tools listed by tools/list.",
        "channel_authority_refused" | "channel_identity_changed" | "invalid_channel" | "session_mismatch" =>
            "This channel no longer holds authority (revoked, paused, replaced or finished). End the turn; the runtime resumes current work on a new channel.",
        "writing_worker_required" | "worker_required" | "readonly_delete_refused" =>
            "This role or stage holds no write authority for this action. Report the need in the result or use the advertised request route.",
        "write_denied" => "The owner protected this path from writes. Choose another path or report the blocker; never work around it.",
        "read_only_task_writer_refused" => "This task is read-only: plan only read-only specialists and change no file.",
        "private_or_invalid_path" => "Use a project-relative path without '..', empty components or private runtime storage (.git, .cortex).",
        "outside_cwd_denied" => "This working directory is not available in the current access mode.",
        "deletion_not_confirmed" | "outside_action_not_confirmed" | "skill_link_replace_not_confirmed" =>
            "The owner did not confirm this action, so nothing happened. Do not retry it to obtain consent; continue without it or report the blocker.",
        "writing_scope_required" => "This action requires an assignment with a non-empty declared work area.",
        _ => "This action is refused under the runtime's authority rules; do not retry it unchanged.",
    }
}
pub struct Gateway {
    core: Arc<dyn GatewayCore>,
    credentials: Credentials,
}
impl Gateway {
    pub fn new(core: Arc<dyn GatewayCore>, credentials: Credentials) -> Self {
        Self { core, credentials }
    }
    pub async fn handle(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        if request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || id
                .as_ref()
                .is_some_and(|v| !v.is_string() && v.as_i64().is_none())
        {
            return Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}),
            );
        }
        if id.is_none() {
            // Notifications cannot execute tools or select an identity.
            return None;
        }
        let id = id.unwrap();
        let r = self
            .dispatch(
                request["method"].as_str().unwrap(),
                request.get("params").cloned().unwrap_or(json!({})),
                id.clone(),
            )
            .await;
        Some(match r {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(Error::Refused(code)) => {
                // A named refusal keeps the authority code and adds its stable reason.
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,
                    "message":format!("authority_refused: {code}"),
                    "data":{"code":code,"retryable":false,"hint":refusal_hint(code)}}})
            }
            Err(Error::InvalidField { field, reason }) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"invalid_arguments","data":{"code":"invalid_arguments","field":field,"reason":reason,"hint":"Consult tools_discover for this tool and correct the indicated argument."}}})
            }
            Err(Error::Invalid) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"invalid_arguments",
                    "data":{"code":"invalid_arguments","retryable":false,
                        "hint":"The arguments do not match this tool's advertised inputSchema. Read the tool's live schema (tools_discover with its name) and send only its exact properties within their bounds."}}})
            }
            Err(e) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":match e{Error::Protocol=>-32601,_=>-32001},"message":e.to_string()}})
            }
        })
    }
    async fn dispatch(&self, method: &str, p: Value, id: Value) -> Result<Value> {
        if method == "initialize" {
            if !p["protocolVersion"].is_string()
                || !p["capabilities"].is_object()
                || !p["clientInfo"]["name"].is_string()
                || !p["clientInfo"]["version"].is_string()
            {
                return Err(Error::Invalid);
            }
            return Ok(
                json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"cortex","version":"1.0.6"}}),
            );
        }
        if method == "server/discover" {
            return Ok(
                json!({"resultType":"complete","supportedVersions":["2025-11-25"],"capabilities":{"tools":{"listChanged":false}},"ttlMs":3600000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"cortex","version":"1.0.6"}}}),
            );
        }
        if method == "ping" {
            return Ok(json!({}));
        }
        if !matches!(method, "tools/list" | "tools/call") {
            return Err(Error::Protocol);
        }
        let channel = self.core.authenticate(&self.credentials).await?;
        let mut tools = schema::catalogue(
            &channel.role,
            channel.profile.as_deref(),
            channel.stage.as_deref(),
        )?;
        tools.retain(|tool| {
            tool["name"].as_str().is_some_and(|name| {
                self.core.supports(name) && self.core.supports_channel(&channel, name)
            })
        });
        if method == "tools/list" {
            return Ok(json!({"tools":tools}));
        }
        let obj = p.as_object().ok_or(Error::Invalid)?;
        if obj
            .keys()
            .any(|k| !matches!(k.as_str(), "name" | "arguments" | "_meta"))
        {
            return Err(Error::Invalid);
        }
        let name = p["name"].as_str().ok_or(Error::Invalid)?;
        let args = p.get("arguments").cloned().unwrap_or(json!({}));
        let Some(tool) = tools.iter().find(|t| t["name"] == name) else {
            self.core.call_rejected(name, &args, "tool_not_offered");
            return Err(Error::Refused("tool_not_offered"));
        };
        if let Err(error) = schema::validate(&tool["inputSchema"], &args) {
            self.core.call_rejected(name, &args, "invalid_arguments");
            return Err(error);
        }
        self.core
            .execute(&self.credentials, &channel, name, args, id)
            .await
    }
    pub async fn serve<R, W>(&self, read: R, mut write: W) -> Result<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut read = tokio::io::BufReader::new(read);
        while let Some(v) = wire::read_frame(&mut read).await? {
            if let Some(reply) = self.handle(v).await {
                wire::write_frame(&mut write, &reply).await?
            }
        }
        Ok(())
    }
}

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
    fn execute<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        name: &'a str,
        arguments: Value,
        request_id: Value,
    ) -> FutureResult<'a, Value>;
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
            Err(e) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":match e{Error::Invalid=>-32602,Error::Protocol=>-32601,_=>-32001},"message":e.to_string()}})
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
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .ok_or(Error::Forbidden)?;
        let args = p.get("arguments").cloned().unwrap_or(json!({}));
        schema::validate(&tool["inputSchema"], &args)?;
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

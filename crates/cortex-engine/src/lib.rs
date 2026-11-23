//! Outgoing ACP and channel-bound MCP adapters. No Python runtime dependency.
//! The service must inject Core policy; this crate never opens a second Store.
pub mod broker;
pub mod engine;
pub mod environment;
pub mod external;
pub mod filesystem_view;
pub mod gateway;
pub mod permission;
pub mod schema;
pub mod wire;

use std::{future::Future, pin::Pin};
pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("transport_closed_outcome_unknown")]
    Closed,
    #[error("deadline_outcome_unknown")]
    Unknown,
    #[error("protocol_error")]
    Protocol,
    #[error("frame_too_large")]
    FrameTooLarge,
    #[error("invalid_arguments")]
    Invalid,
    #[error("invalid_arguments")]
    InvalidField { field: String, reason: &'static str },
    #[error("authority_refused")]
    Forbidden,
    /// An authority refusal that names its stable, secret-free refusal code
    /// (for example `scope_not_granted`). Enforcement is identical to
    /// `Forbidden`; only the reported reason is more precise.
    #[error("authority_refused")]
    Refused(&'static str),
    #[error("operation_busy_or_unreconciled")]
    Busy,
    #[error("io_error")]
    Io,
    #[error("engine_rpc_error:{0}")]
    Rpc(i64),
    #[error("engine_rpc_error:{code}")]
    ProviderLimit { code: i64, limit: ProviderLimit, before_work: bool },
}

/// Only allowlisted enum values and numeric timing hints survive the wire boundary.
/// In particular, adapter messages, arbitrary data and unknown strings never enter
/// the error or its Debug/Display output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    Rate,
    Quota,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderLimit {
    pub kind: LimitKind,
    pub retry_after_seconds: Option<u32>,
    pub resets_at_unix_seconds: Option<u64>,
}

impl Error {
    pub fn provider_limit(&self) -> Option<ProviderLimit> {
        match self {
            Self::ProviderLimit { limit, .. } => Some(*limit),
            _ => None,
        }
    }

    pub fn provider_limit_before_work(&self) -> Option<ProviderLimit> {
        match self {
            Self::ProviderLimit { limit, before_work: true, .. } => Some(*limit),
            _ => None,
        }
    }

    fn from_rpc(engine: environment::EngineKind, error: &serde_json::Value) -> Self {
        use environment::EngineKind;
        let code = error["code"].as_i64();
        let data = &error["data"];
        // Auth, unsupported methods and invalid arguments remain their own RPC
        // errors. Do not infer limiting from 429, prose, or unknown data shapes.
        let kind = match (code, engine, data["codexErrorInfo"].as_str(), data["errorKind"].as_str()) {
            (Some(-32603), EngineKind::Codex, Some("usageLimitExceeded"), None)
                if data["errorKind"].is_null() => LimitKind::Quota,
            (Some(-32603), EngineKind::Codex, Some("rateLimitExceeded"), None)
                if data["errorKind"].is_null() => LimitKind::Rate,
            (Some(-32603), EngineKind::Claude, None, Some("rate_limit"))
                if data["codexErrorInfo"].is_null() => LimitKind::Rate,
            _ => return Self::Rpc(code.unwrap_or(-32603)),
        };
        Self::ProviderLimit {
            code: -32603,
            before_work: false,
            limit: ProviderLimit {
                kind,
                retry_after_seconds: data["retryAfterSeconds"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok()),
                resets_at_unix_seconds: [
                    data["resetsAt"].as_u64(),
                    (engine == EngineKind::Claude)
                        .then(|| data["rateLimit"]["resetsAt"].as_u64())
                        .flatten(),
                ]
                .into_iter()
                .flatten()
                .filter(|v| *v <= 253_402_300_799) // Unix seconds, not milliseconds.
                .max(),
            },
        }
    }
}
#[cfg(test)]
mod provider_limit_tests {
    use super::{environment::EngineKind::*, *};
    use serde_json::json;

    #[test]
    fn provider_limit_retains_only_adapter_enum_and_numeric_hints() {
        for (engine, data, kind) in [
            (Codex, json!({"codexErrorInfo":"rateLimitExceeded"}), LimitKind::Rate),
            (Codex, json!({"codexErrorInfo":"usageLimitExceeded"}), LimitKind::Quota),
            (Claude, json!({"errorKind":"rate_limit"}), LimitKind::Rate),
        ] {
            let mut data = data;
            data["retryAfterSeconds"] = json!(21);
            if engine == Claude {
                data["rateLimit"] = json!({"status":"rejected","resetsAt":1_800_000_000});
            } else {
                data["resetsAt"] = json!(1_800_000_000);
            }
            data["details"] = json!("discard-me");
            let error = Error::from_rpc(engine, &json!({
                "code":-32603, "message":"discard-me", "data":data
            }));
            assert_eq!(error.provider_limit(), Some(ProviderLimit {
                kind, retry_after_seconds: Some(21), resets_at_unix_seconds: Some(1_800_000_000),
            }));
            assert_eq!(error.to_string(), "engine_rpc_error:-32603");
            assert!(!format!("{error:?}").contains("discard-me"));
        }
    }

    #[test]
    fn untyped_hard_conflicting_and_malformed_refusals_are_not_limits() {
        for (engine, code, data) in [
            (Codex,-32000,json!({"codexErrorInfo":"usageLimitExceeded"})),
            (Codex,-32602,json!({"codexErrorInfo":"rateLimitExceeded"})),
            (Codex,429,json!({})),
            (Codex,-32603,json!({"status":429,"retryAfterSeconds":30})),
            (Codex,-32603,json!({"codexErrorInfo":"unauthorized"})),
            (Codex,-32603,json!({"codexErrorInfo":"rateLimitExceeded","errorKind":"billing_error"})),
            (Codex,-32603,json!({"codexErrorInfo":{"rateLimitExceeded":true}})),
            (Claude,-32603,json!({"errorKind":"billing_error"})),
            (Claude,-32603,json!({"errorKind":"model_not_found"})),
            (Claude,-32603,json!({"errorKind":"account_on_hold"})),
            (Claude,-32603,json!({"errorKind":"insufficient_quota"})),
            (OpenCode,-32603,json!({"errorKind":"rate_limit"})),
        ] {
            let error = Error::from_rpc(engine, &json!({
                "code":code,"message":"Rate limit exceeded","data":data
            }));
            assert!(matches!(error, Error::Rpc(actual) if actual == code));
        }
        let error = Error::from_rpc(Codex, &json!({"data":{"codexErrorInfo":"usageLimitExceeded"}}));
        assert!(matches!(error, Error::Rpc(-32603)));
        let error = Error::from_rpc(Codex, &json!({"code":-32603,"data":{
            "codexErrorInfo":"rateLimitExceeded","retryAfterSeconds":"1","resetsAt":-5
        }}));
        assert_eq!(error.provider_limit(), Some(ProviderLimit {
            kind: LimitKind::Rate, retry_after_seconds: None, resets_at_unix_seconds: None,
        }));
    }
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

//! Completed connection/MCP metadata and advisory usage/context producers.
//! No revalidation write, engine credential read, quota lock, CLI run or authorization.
use super::{parse_connections, settings_view, usage_windows, ConfigurationReader, ENGINES};
use crate::native_routes::{codex_quota, read_bytes};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::MetadataExt, path::PathBuf};

/// Advisory failures are retained separately, never converted to zero usage.
pub struct SettingsRead {
    pub view: Value,
    pub diagnostics: Vec<Value>,
}
fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| format!("sha256:{:x}", Sha256::digest(b)))
        .unwrap_or("absent".into())
}
fn connection_view(r: &Value) -> Value {
    let mut out = json!({});
    for k in [
        "id",
        "name",
        "engine",
        "provider",
        "auth_mode",
        "key_kind",
        "endpoint",
        "launch_command",
        "state",
        "qualified",
        "max_concurrent_sessions",
        "created_at",
        "updated_at",
    ] {
        out[k] = r[k].clone();
    }
    let reference = &r["credential_ref"];
    out["credential"] = match reference["kind"].as_str() {
        Some("engine_native_dir") => json!({"kind":"engine_native_dir","owned":reference["owned"]}),
        Some("secret_file") => json!({"kind":"secret_file"}),
        Some("os_keyring") => json!({"kind":"os_keyring"}),
        _ => json!({"kind":"unknown"}),
    };
    out["optional_engine"] = json!(r["engine"] == "opencode");
    out
}
pub(crate) fn management_view(registry: &Value, token: &str, exists: bool) -> Value {
    let mut records: Vec<_> = registry["connections"]
        .as_object()
        .unwrap()
        .values()
        .collect();
    records.sort_by_key(|r| (r["engine"].as_str(), r["name"].as_str(), r["id"].as_str()));
    let engines: Vec<_> = ENGINES.iter().map(|engine| {
        let versions = match *engine {
            "codex-acp" => json!(["1.12.0","1.13.0","2.0.1","2.1.1","2.3.0","2.6.0"]),
            "claude-agent-acp" => json!(["0.78.0","0.87.0","0.88.0","0.97.0","0.107.0"]),
            _ => json!(["2.x.x"]),
        };
        json!({"engine":engine,"optional":*engine=="opencode",
            "onboarding":if *engine=="opencode" {"explicit_opt_in"} else {"discovery_or_setup"},
            "auth_modes": if *engine=="opencode" {json!({"native_account":true})}
                else {json!({"native_account":true,"api_key":true,"gateway":false})},
            "qualified_versions":versions,
            "connection_ids":records.iter().filter(|r| r["engine"]==*engine).map(|r| r["id"].clone()).collect::<Vec<_>>()})
    }).collect();
    json!({"surface":"connections","revision":token,"exists":exists,
        "connections":records.into_iter().map(connection_view).collect::<Vec<_>>(),"engines":engines,
        "operations":["rename","set_max_concurrent_sessions","validate","remove"],
        "max_concurrent_sessions_limit":64})
}
fn diagnostic(id: &str, source: &str, code: &str) -> Value {
    json!({"connection_id":id,"source":source,"code":code})
}
impl ConfigurationReader {
    pub(super) fn registry_snapshot(&self) -> CoreResult<(Value, Value)> {
        let bytes = read_bytes(
            &self.environment.config.join("cortex/connections.json"),
            true,
        )?;
        // Retained registry reads normalize an existing empty file to empty state.
        let parsed = bytes
            .as_deref()
            .filter(|b| !b.is_empty())
            .map(serde_json::from_slice::<Value>)
            .transpose()
            .map_err(|_| CoreError::Refused("invalid_registry"))?;
        let registry = parse_connections(parsed.as_ref()).map_err(CoreError::Refused)?;
        let view = management_view(&registry, &revision(bytes.as_deref()), bytes.is_some());
        Ok((registry, view))
    }
    /// Exact-byte revision and credential-redacted management view. No validation write.
    pub fn connection_management(&self) -> CoreResult<Value> {
        Ok(self.registry_snapshot()?.1)
    }
    fn codex_directory(&self, r: &Value, registry: &Value) -> Option<PathBuf> {
        self.native_directory(r, registry, "codex-home", ".codex")
    }
    /// A connection's own native account directory, only when its reference is
    /// exactly the expected private, unshared, owner-only directory.
    fn native_directory(
        &self,
        r: &Value,
        registry: &Value,
        owned_leaf: &str,
        shared_leaf: &str,
    ) -> Option<PathBuf> {
        let reference = &r["credential_ref"];
        if reference["kind"] != "engine_native_dir" {
            return None;
        }
        let id = r["id"].as_str()?;
        let expected = if reference["owned"] == true {
            self.environment
                .state
                .join("cortex/connections")
                .join(id)
                .join(owned_leaf)
        } else {
            self.environment.home.join(shared_leaf)
        };
        if reference["path"].as_str()? != expected.to_str()? {
            return None;
        }
        // Ambiguous references are unknown, never an excuse to inspect another account.
        if registry["connections"].as_object()?.values().any(|other| {
            other["id"] != r["id"] && other["credential_ref"]["path"] == reference["path"]
        }) {
            return None;
        }
        for directory in expected.ancestors() {
            if !std::fs::symlink_metadata(directory)
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            {
                return None;
            }
        }
        let m = std::fs::metadata(&expected).ok()?;
        (m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0).then_some(expected)
    }
    fn visible_usage(&self, registry: &Value, now: f64, diagnostics: &mut Vec<Value>) -> Value {
        let records = registry["connections"].as_object().unwrap();
        let mut usage = Vec::new();
        for (id, record) in records {
            let engine = record["engine"].as_str().unwrap();
            if engine == "opencode" {
                continue;
            }
            let windows = if engine == "codex-acp" {
                match self.codex_directory(record, registry) {
                    Some(home) => {
                        // A free account read newer than the last rollout replaces its windows.
                        let cache = self.cache("quota", id).unwrap_or_else(|_| {
                            diagnostics.push(diagnostic(id, "quota_cache", "cache_unavailable"));
                            json!({})
                        });
                        let windows =
                            super::usage_refresh::newest_codex(&codex_quota(&home), &cache);
                        usage_windows(engine, &windows, now)
                    }
                    None => {
                        diagnostics.push(diagnostic(
                            id,
                            "codex_rollout",
                            "native_directory_unavailable",
                        ));
                        json!([])
                    }
                }
            } else {
                match self.usage_cache(id, engine, now) {
                    Ok(windows) => windows,
                    Err(_) => {
                        diagnostics.push(diagnostic(id, "quota_cache", "cache_unavailable"));
                        json!([])
                    }
                }
            };
            usage.push(json!({"id":id,"engine":engine,"windows":windows}));
        }
        json!(usage)
    }
    /// Configured Codex or Claude connections of `engine` whose native account
    /// directory verifies (owned `codex-home`/`claude-config`, or the referenced
    /// `~/.codex`/`~/.claude`), sorted by id: the only accounts a plan-window
    /// refresh may read. Nothing is written.
    pub fn usage_targets(&self, engine: &str) -> CoreResult<Vec<(String, PathBuf)>> {
        let (owned, shared) = match engine {
            "codex-acp" => ("codex-home", ".codex"),
            "claude-agent-acp" => ("claude-config", ".claude"),
            _ => return Err(CoreError::Refused("unsupported_engine")),
        };
        let (registry, _) = self.registry_snapshot()?;
        let mut targets: Vec<_> = registry["connections"]
            .as_object()
            .unwrap()
            .values()
            .filter(|r| r["engine"] == engine && r["state"] == "configured")
            .filter_map(|r| {
                let directory = self.native_directory(r, &registry, owned, shared)?;
                Some((r["id"].as_str()?.to_owned(), directory))
            })
            .collect();
        targets.sort();
        Ok(targets)
    }
    /// Native connection/MCP/usage/context IO wired to the settings compositor.
    /// The embedding must still supply the other three completed management surfaces,
    /// catalog entries (with connection_id/model), routes, discovery and workspace.
    /// This is NOT config.read dispatch or a refresh/credential authorizer.
    pub fn settings_with_readers(
        &self,
        observations: &Value,
        now: f64,
    ) -> CoreResult<SettingsRead> {
        if !now.is_finite()
            || !observations["surfaces"].is_object()
            || !observations["workspace"].is_object()
            || !observations["entries"].is_array()
        {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        let (registry, connections) = self.registry_snapshot()?;
        let records = registry["connections"].as_object().unwrap();
        // Preflight every binding before reading a cache.
        for entry in observations["entries"].as_array().unwrap() {
            if !entry["connection_id"]
                .as_str()
                .is_some_and(|id| records.contains_key(id))
                || !entry["model"].is_string()
                || !entry["route_id"].is_string()
            {
                return Err(CoreError::Refused("invalid_settings_entry"));
            }
        }
        let mut o = observations.clone();
        o["surfaces"]["connections"] = json!({"ok":true,"result":connections});
        o["surfaces"]["mcp_servers"] = self.mcp_management_observation()?;
        let mut diagnostics = Vec::new();
        o["usage_records"] = self.visible_usage(&registry, now, &mut diagnostics);
        for entry in o["entries"].as_array_mut().unwrap() {
            let id = entry["connection_id"].as_str().unwrap();
            entry["context_window"] = match self.context(id, entry["model"].as_str()) {
                Ok(size) => json!(size),
                Err(_) => {
                    diagnostics.push(diagnostic(id, "context_cache", "cache_unavailable"));
                    Value::Null
                }
            };
        }
        let view = settings_view(&o).map_err(CoreError::Refused)?;
        Ok(SettingsRead { view, diagnostics })
    }
}

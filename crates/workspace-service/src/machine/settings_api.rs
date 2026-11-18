//! The Paseo plugin's settings API under the Rust `cortex-acp` launch
//! (`cortex-paseo-tools config`): every management-API operation answered by
//! the native Settings implementations, never Python. Requests arrive already
//! shape-checked by the caller; refusals keep the management payload shape.
//! A shape that only the retained Python provider could answer (an OS keyring
//! backend, the OpenCode namespace jail, an engine without a qualified native
//! catalog producer) refuses with `config_api_unavailable` and its reason.
use super::{config_action as action, native::Refused, project_config_save::management as m};
use crate::native_routes::RouteEnvironment;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub use m::{
    error, expected_revision, read_project_ceiling, update_mcp, update_project_ceiling,
    update_routing, Answer,
};

/// Trusted host inputs: the owner roots and Cortex's runtime payload directory.
pub struct Host {
    pub environment: RouteEnvironment,
    /// The project store directory name selected by the host channel.
    pub store: String,
    /// The directory holding `pinned-dependencies.json` (engine installation).
    pub runtime_root: PathBuf,
}

fn unavailable(reason: &str) -> Value {
    error(
        "config_api_unavailable",
        format!("{reason} This shape is not available from Paseo while Cortex runs from the Rust cortex-acp entry."),
        None,
    )
}
/// A native refusal as its management payload. `fields` names the request
/// field of each code (recorded details win); a retained-provider admission
/// refusal becomes the explicit `config_api_unavailable` with its reason.
fn payload(refused: Refused, fields: &[(&str, &str)]) -> Value {
    if refused.code == "config_action_unavailable" {
        return unavailable(&refused.message);
    }
    let field = fields
        .iter()
        .find(|(code, _)| *code == refused.code)
        .map(|(_, field)| *field);
    m::refusal(refused, field)
}

impl Host {
    fn config(&self) -> Result<crate::Config, Value> {
        let root = if self.runtime_root.is_dir() {
            self.runtime_root.clone()
        } else {
            PathBuf::from("/")
        };
        let mut launch = crate::LaunchSpec::native(root)
            .map_err(|_| unavailable("The Cortex runtime directory is unavailable."))?;
        // The owner configuration root is this host's, in every channel.
        launch.dev_config = Some(self.environment.config.clone());
        // Catalogs, routes and engine state belong to this host's state root, the same
        // one its route reader and runtime use, never a settings-private copy.
        launch.dev_state = Some(self.environment.state.clone());
        Ok(crate::Config {
            runtime_dir: self.environment.state.join("cortex-settings-runtime"),
            state_dir: self.environment.state.join("cortex-settings-state"),
            allowed_root: self.environment.home.clone(),
            launch,
            request_timeout: Duration::from_secs(25),
            shutdown_timeout: Duration::from_secs(3),
            event_capacity: 16,
        })
    }
    fn reader(&self) -> Result<crate::config_read_model::ConfigurationReader, Value> {
        crate::config_read_model::ConfigurationReader::new(self.environment.clone())
            .map_err(|e| read_error(e.code()))
    }
    fn project(&self, project: &Value) -> Option<(PathBuf, PathBuf)> {
        let root = PathBuf::from(project.as_str().filter(|s| s.starts_with('/'))?);
        let routing = root.join(&self.store).join("routing.json");
        Some((root, routing))
    }

    pub fn update_routing(
        &self,
        target: &Value,
        expected: &Value,
        rules: &str,
        project: &Value,
    ) -> Answer {
        update_routing(
            &self.environment,
            &self.store,
            target,
            expected,
            rules,
            project,
        )
    }
    pub fn update_mcp(&self, expected: &Value, operations: &str) -> Answer {
        update_mcp(&self.environment, expected, operations)
    }
    pub fn read_project_ceiling(&self, project: &Value) -> Answer {
        read_project_ceiling(&self.environment, &self.store, project)
    }
    pub fn update_project_ceiling(
        &self,
        project: &Value,
        expected: &Value,
        ceiling: &Value,
    ) -> Answer {
        update_project_ceiling(&self.environment, &self.store, project, expected, ceiling)
    }

    /// `routing_profiles.mutate`: `request` holds the management fields.
    pub fn profile(&self, verb: &str, request: &Value) -> Answer {
        let config = self.config()?;
        let mut p = json!({"verb":verb});
        for key in [
            "expected_revision",
            "expected_revisions",
            "name",
            "rename_to",
            "description",
            "discard_modifications",
        ] {
            if let Some(value) = request.get(key) {
                p[key] = value.clone();
            }
        }
        let name_valid = request["name"]
            .as_str()
            .is_some_and(|s| crate::config_read_model::matches(r"^[a-z0-9][a-z0-9-]{0,39}$", s));
        let renamed = if verb == "update" && name_valid {
            "rename_to"
        } else {
            "name"
        };
        let mut result = action::management_profile(&config, &p).map_err(|e| {
            payload(
                e,
                &[
                    ("unsupported_action", "action"),
                    ("invalid_expected_revision", "expected_revision"),
                    ("invalid_expected_revisions", "expected_revisions"),
                    ("invalid_value", "discard_modifications"),
                    ("invalid_profile_name", renamed),
                    ("invalid_profile_description", "description"),
                    ("profile_not_found", "name"),
                    ("profile_exists", renamed),
                    ("no_active_profile", "name"),
                    ("profile_not_active", "name"),
                    ("profile_interrupted", "name"),
                    ("profile_modified", "discard_modifications"),
                ],
            )
        })?;
        // The project only changes which source shadows the user routing.
        if let Some(pair) = self.project(&request["project"]) {
            let view = self
                .reader()?
                .profiles_management_observation(Some((pair.0.as_path(), pair.1.as_path())))
                .map_err(|e| read_error(e.code()))?;
            result["shadowed_by"] = view["result"]["shadowed_by"].clone();
        }
        Ok(result)
    }

    /// `update_connection` (rename, session limit, validate, remove).
    pub fn connection_update(&self, request: &Value) -> Answer {
        let config = self.config()?;
        action::management_connection(&config, request).map_err(|e| {
            payload(
                e,
                &[
                    ("invalid_expected_revision", "expected_revision"),
                    ("invalid_connection_id", "connection_id"),
                    ("unsupported_operation", "operation"),
                    ("unknown_connection", "connection_id"),
                ],
            )
        })
    }

    pub fn read_default_preset(&self) -> Answer {
        action::management_preset_snapshot(&self.config()?).map_err(|e| payload(e, &[]))
    }
    pub fn apply_default_preset(&self) -> Answer {
        action::management_preset_apply(&self.config()?).map_err(|e| payload(e, &[]))
    }
    pub fn reset_default_preset(&self, expected: &Value, confirm: &Value) -> Answer {
        if confirm != &json!(true) {
            return Err(error(
                "confirmation_required",
                "Send confirm: true to replace both user preset files.",
                Some("confirm"),
            ));
        }
        let expected = expected_revision(expected)?;
        action::management_preset_reset(&self.config()?, &expected).map_err(|e| payload(e, &[]))
    }

    /// `refresh_route_catalog`: the fresh catalog with one outcome per connection.
    pub fn refresh_catalog(&self, project: &Value) -> Answer {
        let config = self.config()?;
        let records = action::management_records(&config).map_err(|e| payload(e, &[]))?;
        let outcomes = if records["connections"]
            .as_object()
            .is_none_or(|m| m.is_empty())
        {
            json!([])
        } else {
            action::management_refresh(&config).map_err(|e| payload(e, &[]))?["refresh"].clone()
        };
        let pair = self.project(project);
        let mut view = self
            .reader()?
            .catalog_selector(pair.as_ref().map(|(a, b)| (a.as_path(), b.as_path())))
            .map_err(|e| read_error(e.code()))?["routes"]
            .clone();
        view["refresh"] = outcomes;
        Ok(view)
    }

    pub fn discover_connections(&self) -> Answer {
        action::management_discovery(&self.config()?).map_err(|e| payload(e, &[]))
    }
    /// `onboard_connection`: explicit native-account onboarding of one engine.
    pub fn onboard_connection(&self, request: &Value) -> Answer {
        let Some(engine) = request["engine"]
            .as_str()
            .filter(|e| matches!(*e, "codex-acp" | "claude-agent-acp" | "opencode"))
        else {
            return Err(error(
                "unsupported_engine",
                "Choose codex-acp, claude-agent-acp or opencode.",
                Some("engine"),
            ));
        };
        if request["explicit_opt_in"] != true {
            return Err(error(
                "explicit_opt_in_required",
                "Send explicit_opt_in: true to connect a native account.",
                Some("explicit_opt_in"),
            ));
        }
        let expected = expected_revision(&request["expected_revision"])?;
        action::management_onboard(&self.config()?, engine, &expected).map_err(|e| payload(e, &[]))
    }
    pub fn install_engine(&self, engine: &Value, deadline: Instant) -> Answer {
        let config = self.config()?;
        action::management_install(
            &config,
            &json!({"action":"install_engine","engine":engine}),
            deadline,
        )
        .map_err(|e| {
            payload(
                e,
                &[
                    ("unsupported_engine", "engine"),
                    ("engine_install_unsupported", "engine"),
                    ("engine_install_failed", "engine"),
                ],
            )
        })
    }
    /// `discover_opencode`: no candidate binary needs no probe; a candidate
    /// needs the OpenCode namespace jail.
    pub fn discover_opencode(&self, binary: &Value) -> Answer {
        if !binary.is_null() && !binary.as_str().is_some_and(|b| b.starts_with('/')) {
            return Err(error(
                "invalid_binary",
                "binary must be an absolute path.",
                Some("binary"),
            ));
        }
        let config = self.config()?;
        let candidate = named_candidate(binary)
            || action::management_opencode_candidate(&config).map_err(|e| payload(e, &[]))?;
        if candidate {
            return Err(opencode_jail());
        }
        let view = self
            .reader()?
            .connection_management()
            .map_err(|e| read_error(e.code()))?;
        let onboarded: Vec<Value> = view["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["engine"] == "opencode")
            .map(|c| c["id"].clone())
            .collect();
        let qualified = view["engines"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["engine"] == "opencode")
            .map(|e| e["qualified_versions"].clone())
            .unwrap_or(Value::Null);
        Ok(
            json!({"found":false,"onboarded_connection_ids":onboarded,"qualified_versions":qualified}),
        )
    }
    pub fn onboard_opencode(&self, request: &Value) -> Answer {
        let binary = &request["binary"];
        let mut p = request.clone();
        p["action"] = json!("onboard_opencode");
        // Validation precedes the lock and discovery in the management API.
        if let Some(refused) = action::management_onboard_opencode_refusal(&p) {
            return Err(opencode_payload(refused));
        }
        if !binary.is_null() && !binary.as_str().is_some_and(|b| b.starts_with('/')) {
            return Err(error(
                "invalid_binary",
                "binary must be an absolute path.",
                Some("binary"),
            ));
        }
        if named_candidate(binary) {
            return Err(opencode_jail());
        }
        action::management_onboard_opencode(&self.config()?, &p).map_err(opencode_payload)
    }
}
fn opencode_payload(refused: Refused) -> Value {
    payload(
        refused,
        &[
            ("explicit_opt_in_required", "explicit_opt_in"),
            ("invalid_expected_revision", "expected_revision"),
            ("opencode_not_found", "binary"),
        ],
    )
}
fn opencode_jail() -> Value {
    unavailable(
        "An OpenCode binary is installed or named: its version probe must run inside the OpenCode network-less namespace jail, which this native route cannot start. Nothing was changed.",
    )
}
/// An existing binary named by the request or `$CORTEX_OPENCODE_BINARY`
/// (`discover_opencode` skips a candidate that does not resolve).
fn named_candidate(binary: &Value) -> bool {
    let env = std::env::var_os("CORTEX_OPENCODE_BINARY").map(PathBuf::from);
    binary
        .as_str()
        .map(PathBuf::from)
        .into_iter()
        .chain(env)
        .any(|p| !p.as_os_str().is_empty() && Path::new(&p).canonicalize().is_ok())
}
fn read_error(code: &str) -> Value {
    error(
        code,
        "The Cortex configuration storage refused the read; nothing was changed.",
        None,
    )
}

#[cfg(test)]
mod state_root_tests {
    use super::*;
    /// Catalog refreshes through this API publish where the route reader and runtime
    /// read: the host's own state root, never `<settings-state>/state`.
    #[test]
    fn settings_actions_use_the_host_state_root() {
        let base = std::env::temp_dir().join(format!("settings-state-root-{}", std::process::id()));
        let environment = RouteEnvironment {
            home: base.join("home"),
            config: base.join("config"),
            state: base.join("state"),
            routing_override: None,
        };
        let host = Host {
            environment: environment.clone(),
            store: ".cortex-dev".into(),
            runtime_root: base.join("runtime"),
        };
        let config = host.config().unwrap();
        let resolved = super::super::config_read::environment(&config).unwrap();
        assert_eq!(resolved.state, environment.state);
        assert_eq!(resolved.config, environment.config);
    }
}

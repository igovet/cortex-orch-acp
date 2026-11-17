//! Native config.read host dispatch. The machine socket uses Display only.
//! Explicit refresh is separate from display; no engine, keyring or Python fallback.
use crate::{
    config_read_model::{ConfigurationReader, SettingsRead, SetupEnvironment},
    config_refresh::refresh_connections,
    native_routes::RouteEnvironment,
};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

/// Trusted host intent, never inferred from opening settings or supplied JSON.
pub enum ConfigReadAction<'a> {
    Display,
    RefreshConnections {
        ids: &'a [String],
        expected_revision: &'a str,
        observed_at: &'a str,
        wait: Duration,
    },
}

/// Explicit host roots and already-sanitized optional project observations.
/// No project store, scheduler, owner lease or process-global environment is opened.
pub struct ConfigReadHost<'a> {
    pub environment: &'a RouteEnvironment,
    pub workspace_state: &'a Path,
    pub setup: &'a SetupEnvironment,
    pub project: Option<(&'a Path, &'a Path)>,
}

pub struct ConfigReadResult {
    pub settings: SettingsRead,
    /// A successful explicit refresh receipt, not account/dispatch authorization.
    pub refresh: Option<Value>,
}

impl ConfigReadHost<'_> {
    fn display(&self, observations: &Value, now: f64) -> CoreResult<SettingsRead> {
        let reader = ConfigurationReader::new(self.environment.clone())?;
        let mut result = reader.settings_with_setup_readers(
            observations,
            now,
            self.project,
            self.workspace_state,
            self.setup,
        )?;
        // Stored state is still displayed, but is never presented as a fresh
        // keyring observation. Do not echo service/account or credential values.
        for r in result.view["connections"].as_array().into_iter().flatten() {
            if r["detail"]["credential"] == "os_keyring" {
                result.diagnostics.push(json!({
                    "connection_id":r["id"], "source":"keyring", "code":"keyring_not_observed"
                }));
            }
        }
        Ok(result)
    }

    /// Compose display or explicitly refresh selected filesystem states and re-read.
    /// Refusals propagate without replay. A failed/mismatched read after successful
    /// refresh is fenced as unknown, even on a no-op: inspect before another action.
    pub fn read(
        &self,
        action: ConfigReadAction<'_>,
        observations: &Value,
        now: f64,
    ) -> CoreResult<ConfigReadResult> {
        if !now.is_finite() || now < 0.0 {
            return Err(CoreError::Refused("invalid_settings_time"));
        }
        // Validate the complete display before allowing any refresh effects.
        let before = self.display(observations, now)?;
        let ConfigReadAction::RefreshConnections {
            ids,
            expected_revision,
            observed_at,
            wait,
        } = action
        else {
            return Ok(ConfigReadResult {
                settings: before,
                refresh: None,
            });
        };
        let refresh =
            refresh_connections(self.environment, ids, expected_revision, observed_at, wait)?;
        finish_refresh(refresh, || self.display(observations, now))
    }
}

fn finish_refresh(
    refresh: Value,
    display: impl FnOnce() -> CoreResult<SettingsRead>,
) -> CoreResult<ConfigReadResult> {
    let settings = display().map_err(|_| CoreError::Refused("machine_outcome_unknown"))?;
    if settings.view["tokens"]["connections"] != refresh["revision"] {
        return Err(CoreError::Refused("machine_outcome_unknown"));
    }
    Ok(ConfigReadResult {
        settings,
        refresh: Some(refresh),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_time_refuses_before_any_environment_read() {
        let environment = RouteEnvironment {
            home: "relative".into(),
            config: "relative".into(),
            state: "relative".into(),
            routing_override: None,
        };
        let setup = SetupEnvironment {
            data: "relative".into(),
            path: vec![],
            node_override: None,
            bundle: None,
        };
        let host = ConfigReadHost {
            environment: &environment,
            workspace_state: Path::new("relative"),
            setup: &setup,
            project: None,
        };
        for now in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(
                host.read(ConfigReadAction::Display, &json!({}), now)
                    .err()
                    .unwrap()
                    .code(),
                "invalid_settings_time"
            );
        }
    }
    #[test]
    fn successful_refresh_never_replays_a_failed_or_superseded_projection() {
        let receipt = json!({"revision":"sha256:committed","changed":true});
        let failed = finish_refresh(receipt.clone(), || {
            Err(CoreError::Refused("injected_read_failure"))
        });
        assert_eq!(failed.err().unwrap().code(), "machine_outcome_unknown");
        let stale = finish_refresh(receipt, || {
            Ok(SettingsRead {
                view: json!({"tokens":{"connections":"sha256:other"}}),
                diagnostics: vec![],
            })
        });
        assert_eq!(stale.err().unwrap().code(), "machine_outcome_unknown");
    }
}

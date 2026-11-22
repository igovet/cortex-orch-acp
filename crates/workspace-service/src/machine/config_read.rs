//! Passive machine settings. Host roots only; no Python or project owner. A due Claude
//! `/usage` refresh is started in the background and never awaited.
use super::native::{refuse, Result};
use crate::{
    config_read_dispatch::{ConfigReadAction, ConfigReadHost},
    config_read_model::SetupEnvironment,
    native_routes::RouteEnvironment,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn environment(config: &crate::Config) -> Result<RouteEnvironment> {
    RouteEnvironment::for_workspace(config)
        .map_err(|e| refuse(e.code(), "Settings roots unavailable."))
}

fn engine_data_root(dev: bool, state: &Path, home: &Path, xdg: Option<PathBuf>) -> PathBuf {
    if dev {
        state.join("data")
    } else {
        xdg.unwrap_or_else(|| home.join(".local/share"))
    }
}

/// The service's adapter store, independent of the sanitized child environment.
pub(super) fn engine_data(config: &crate::Config, home: &Path) -> Result<PathBuf> {
    let data = engine_data_root(
        workspace_protocol::channel::current().is_dev(),
        &config.state_dir,
        home,
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
    );
    SetupEnvironment::validate_data(home, &data)
        .map_err(|e| refuse(e.code(), "Engine data root unavailable."))?;
    Ok(data)
}

pub fn read(config: &crate::Config) -> Result<Value> {
    // Before the read, so a connection that needs its models observed already says so in this reply.
    let refresh = super::config_action::catalog_refresh::startup::ensure(config);
    let environment = environment(config)?;
    // Never waits: a due Claude `/usage` refresh runs in the background and the next read shows it.
    super::usage_refresh::ensure(config, &environment);
    let setup = SetupEnvironment {
        data: engine_data(config, &environment.home)?,
        path: std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .filter(|p| p.is_absolute())
                    .collect()
            })
            .unwrap_or_default(),
        node_override: std::env::var_os("CORTEX_NODE_BINARY").map(PathBuf::from),
        // Never select another installation/channel or a client-provided bundle.
        bundle: config.launch.bundled_engines(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| refuse("invalid_settings_time", "Settings time unavailable."))?
        .as_secs_f64();
    // config.read is machine-global, just like the retained handler. Project
    // overrides/ceilings belong to project.config.read, never an unverified JSON root.
    let result = ConfigReadHost {
        environment: &environment,
        workspace_state: &config.state_dir,
        setup: &setup,
        project: None,
    }
    .read(ConfigReadAction::Display, &json!({"surfaces":{}}), now)
    .map_err(|e| refuse(e.code(), "Native settings read refused."))?;
    let mut view = result.settings.view;
    view["diagnostics"] = json!(result.settings.diagnostics);
    view["catalogueRefresh"] = refresh;
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_data_root_is_channel_local_or_xdg_with_home_fallback() {
        let home = Path::new("/native-home");
        let state = Path::new("/workspace-state");
        let xdg = PathBuf::from("/isolated-data");
        assert_eq!(engine_data_root(false, state, home, Some(xdg.clone())), xdg);
        assert_eq!(
            engine_data_root(false, state, home, None),
            home.join(".local/share")
        );
        assert_eq!(
            engine_data_root(true, state, home, Some(xdg)),
            state.join("data")
        );
    }
}

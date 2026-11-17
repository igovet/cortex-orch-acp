//! Native machine-routing save. Cached observations never authorize a route.
//! Compose and budget the complete candidate reply before committing; never replay.
use super::*;
use std::path::PathBuf;

/// Trusted host environment only; JSON parameters cannot choose roots or refresh.
pub(crate) fn save(
    config: &crate::Config,
    environment: &crate::native_routes::RouteEnvironment,
    params: &Value,
) -> Result<Value> {
    let reader = crate::config_read_model::ConfigurationReader::new(environment.clone())
        .map_err(|e| refuse(e.code(), "The host configuration environment is unsafe."))?;
    if environment.config.join("cortex") != super::reader::owner_folder(config)? {
        return Err(refuse(
            "native_config_unavailable",
            "Host configuration roots do not match; nothing was changed.",
        ));
    }
    // Retain the legacy pre-read before editor conversion, including registry errors.
    let registry = reader.connections().map_err(|e| {
        refuse(
            e.code(),
            "The connection registry is invalid; nothing was written.",
        )
    })?;
    routing::view::observation(environment, None, &registry)?;
    let prepared = PreparedUserRouting::prepare(config, params)?;
    let registry = reader.connections().map_err(|e| {
        refuse(
            e.code(),
            "The connection registry is invalid; nothing was written.",
        )
    })?;
    let mut reply =
        routing::view::observation_with_user(environment, None, &registry, Some(prepared.body()))?;
    for (key, value) in prepared.outcome().as_object().unwrap() {
        reply[key] = value.clone();
    }
    reply["shadowed_by"] = if reply["effective_source"] == "user" {
        Value::Null
    } else {
        reply["effective_source"].clone()
    };
    reply["profile"] = reader.selection_with_routing(prepared.body());
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native reply limit.",
        ));
    }
    prepared.commit()?;
    Ok(reply)
}

#[cfg(test)]
#[path = "user_routing_tests.rs"]
mod tests;

/// Holds the shared owner lock from revision admission through atomic replacement.
/// Dropping an uncommitted preparation changes no settings bytes.
pub(crate) struct PreparedUserRouting {
    path: PathBuf,
    body: Vec<u8>,
    previous_revision: String,
    changed: bool,
    _lock: cortex_core::store::locks::AccessLock,
}
impl PreparedUserRouting {
    pub(crate) fn prepare(config: &crate::Config, params: &Value) -> Result<Self> {
        if !params.is_object() {
            return Err(validation::object_error());
        }
        let folder = reader::owner_folder(config)?;
        let path = folder.join("routing.json");
        // Match config_save's read-then-editor-conversion order. Invalid existing
        // rules are not supplied to the editor; malformed bytes still refuse below.
        let before = reader::read_plain(&path, false)?;
        let previous = before
            .as_ref()
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .filter(super::super::project_routing::valid)
            .unwrap_or(json!({}));
        let document = routing::document(&params["value"], &previous)?;
        let mut params = params.clone();
        if !super::super::workers::truthy(&params["tokens"]["routing"]) {
            if super::super::workers::truthy(&params["tokens"]) && !params["tokens"].is_object() {
                return Err(validation::object_error());
            }
            params["tokens"] = json!({"routing":token(before.as_deref())});
        }
        let expected = expected(&params, "routing")?;
        routing::validate_rules(&document)?;
        let lock = owner_lock(&folder)?;
        user_defaults::ignore(&folder)?;
        let current = reader::read_plain(&path, false)?;
        let revision = token(current.as_deref());
        check("routing (user)", &expected, &revision)?;
        let body = routing::pretty(&path, &document, current.as_deref())?;
        if body.len() > FRAME_BUDGET {
            return Err(refuse(
                "machine_reply_too_large",
                "Settings exceed the native write limit.",
            ));
        }
        Ok(Self {
            path,
            changed: Some(body.as_slice()) != current.as_deref(),
            body,
            previous_revision: revision,
            _lock: lock,
        })
    }

    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }

    pub(crate) fn outcome(&self) -> Value {
        json!({"changed":self.changed, "written_revision":token(Some(&self.body)), "target":"user"})
    }

    pub(crate) fn commit(self) -> Result<Value> {
        self.commit_with(replace)
    }

    fn commit_with(self, write: impl FnOnce(&Path, &[u8]) -> Result<()>) -> Result<Value> {
        // Cooperative locking is not hostile same-user isolation. Detect an
        // already-observed non-cooperating edit instead of overwriting it.
        let current = reader::read_plain(&self.path, false)?;
        check(
            "routing (user)",
            &self.previous_revision,
            &token(current.as_deref()),
        )?;
        write(&self.path, &self.body)?;
        Ok(self.outcome())
    }
}

use super::*;

/// Trusted embedding authorization for observed engine, account, model and role.
pub trait NativeRoutes: Send + Sync {
    #[doc(hidden)]
    fn active_steering_timeout(&self, _root: &Path) -> std::time::Duration {
        ACTIVE_STEERING_TURN_BOUND
    }

    #[doc(hidden)]
    fn owner_decision_timeout(&self, _root: &Path) -> std::time::Duration {
        OWNER_DECISION_TURN_BOUND
    }

    fn default_language(&self) -> Option<String> {
        None
    }
    /// Persist a pre-task language choice as the user-wide default. Returns
    /// false when this provider owns no user defaults (the choice stays
    /// session-local); an error means nothing was persisted.
    fn persist_default_language(&self, _language: Option<&str>) -> CoreResult<bool> {
        Ok(false)
    }
    /// Persist a pre-task coordinator `model`/`thought_level` choice (`option`)
    /// as the user-default route, from the session's `previous` and new
    /// `settings`. Same contract as `persist_default_language`.
    fn persist_default_route(
        &self,
        _root: &Path,
        _previous: &Value,
        _settings: &Value,
        _option: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    fn route(&self, root: &Path, role: &str) -> CoreResult<SessionRoute>;
    /// The nearest allowed coordinator route for a stored task choice that is no
    /// longer allowed: the same model with its effort moved into the allowed range
    /// when the model is still listed, else the default coordinator.
    fn fallback_coordinator(&self, root: &Path, _stored: &Value) -> CoreResult<SessionRoute> {
        self.route(root, "orchestrator")
    }
    /// The automatic route of a sprint task of `complexity` (low, normal, high),
    /// from the owner's routing profile (`RouteResolver::sprint_route`); None
    /// leaves the owner's default coordinator.
    fn sprint_route(&self, _root: &Path, _complexity: &str) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    /// The exact coordinator route a task starting with `settings` resolves to, as
    /// a durable pin `{route_id, parameters}`. Persisted at start so a later change
    /// of the owner default never re-routes a running task. None: not pinnable.
    fn coordinator_pin(&self, _root: &Path, _settings: &Value) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    /// The session route of a pin from `coordinator_pin`. `effort` is the task's
    /// explicit thought-level choice, if any.
    fn pinned_route(
        &self,
        root: &Path,
        _pin: &Value,
        _effort: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, "orchestrator")
    }
    /// A durable coordinator pin `{route_id, parameters}` for another qualified model
    /// than `current` (preferably another engine or account), for the no-stop policy's
    /// change of approach (a degenerate model, the `reassign` rung, an unavailable
    /// coordinator route). None: no other model qualifies.
    fn alternate_coordinator_pin(
        &self,
        _root: &Path,
        _current: &SessionRoute,
    ) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    fn worker_route(
        &self,
        root: &Path,
        role: &str,
        _node: &Value,
        _evidence: &cortex_core::routing::Evidence,
        _escalated: bool,
        _previous: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, role)
    }
    fn worker_selection(
        &self,
        root: &Path,
        role: &str,
        node: &Value,
        evidence: &cortex_core::routing::Evidence,
        escalated: bool,
        previous: Option<&str>,
        _context: &mut crate::native_routes::WorkerRouteContext,
    ) -> CoreResult<(SessionRoute, Value)> {
        Ok((
            self.worker_route(root, role, node, evidence, escalated, previous)?,
            json!({}),
        ))
    }
    fn menu(&self, _root: &Path, _evidence: &cortex_core::routing::Evidence) -> CoreResult<Value> {
        Err(CoreError::Refused("native_route_menu_unavailable"))
    }
    fn previews(
        &self,
        _root: &Path,
        _plan: &Value,
        _evidence: &cortex_core::routing::Evidence,
    ) -> CoreResult<Value> {
        Ok(json!([]))
    }
    fn mcp_registry(&self) -> CoreResult<crate::native_mcp::Registry> {
        Ok(crate::native_mcp::Registry::default())
    }
    /// The registry for sessions of the project at `root`: the global registry with
    /// that project's owner-private overrides applied.
    fn mcp_registry_for(&self, _root: &Path) -> CoreResult<crate::native_mcp::Registry> {
        self.mcp_registry()
    }
    /// Only host-qualified identities may be exposed to an incoming client.
    fn options(&self, root: &Path, settings: &Value) -> Value {
        match self.task_route(root, settings) {
            Ok(route) => json!([{"id":"model","name":"Model","category":"model","type":"select",
                "currentValue":route.model,"options":[{"value":route.model,"name":route.model}]}]),
            Err(_) => json!([]),
        }
    }
    /// Selectable routes `{route, model, label, disambiguated_label}` for
    /// `/cortex-model` resolution.
    fn listing(&self, _root: &Path) -> Vec<Value> {
        Vec::new()
    }
    /// Every registered connection's persisted state, in registry order.
    fn connection_states(&self) -> Vec<String> {
        Vec::new()
    }
    /// Registered connection ids, in registry order.
    fn connection_ids(&self) -> Vec<String> {
        Vec::new()
    }
    /// A new, prompt-free qualified observation of the same connection.
    fn authentication_observed_after(
        &self,
        _root: &Path,
        _connection: &str,
        _after: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    /// Automatic coordination respects the same observed quota as automatic worker routing.
    fn automatic_coordinator_health(&self, _root: &Path, _route: &SessionRoute) -> CoreResult<()> {
        Ok(())
    }
    /// One prompt-free catalogue refresh of a registered connection (Python's
    /// per-session `_refresh_catalog_for_session`); true only when a new
    /// observation was published. Never a model prompt.
    fn refresh_catalog(&self, _connection: &str) -> bool {
        false
    }
    /// The route a `provider`/`model`/`thought_level` choice selects (Python
    /// `_apply_config_selection`): `{route, model, connection, echo}`, where
    /// `echo` marks a pre-task re-selection of the saved or pending route.
    fn select(&self, _root: &Path, settings: &Value, id: &str, value: &str) -> CoreResult<Value> {
        let route = if id == "thought_level" {
            settings["model"].clone()
        } else {
            json!(value)
        };
        Ok(json!({"route":route,"model":route,"connection":null,"echo":false}))
    }
    fn task_route(&self, root: &Path, settings: &Value) -> CoreResult<SessionRoute> {
        let route = self.route(root, "orchestrator")?;
        if settings["model"]
            .as_str()
            .is_some_and(|m| !m.is_empty() && m != route.model)
            || settings["effort"].as_str().is_some_and(|e| {
                !e.is_empty()
                    && route.parameters.get("thought_level").map(String::as_str) != Some(e)
            })
        {
            return Err(CoreError::Refused("native_task_route_unavailable"));
        }
        Ok(route)
    }
}

pub struct UnqualifiedRoutes;
impl NativeRoutes for UnqualifiedRoutes {
    fn route(&self, _: &Path, _: &str) -> CoreResult<SessionRoute> {
        Err(CoreError::Refused("native_route_authorization_unavailable"))
    }
}

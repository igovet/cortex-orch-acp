//! Complete routing management reads; display observations are not authorization.
use super::{ConfigurationReader, SettingsRead};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::path::Path;

impl ConfigurationReader {
    /// The optional project pair is (logical root, resolved channel routing path).
    /// Both paths are trusted host inputs, never fields accepted from a model/socket.
    pub fn routing_management(&self, project: Option<(&Path, &Path)>) -> CoreResult<Value> {
        if project.is_some_and(|(root, _)| {
            !root.is_absolute()
                || root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        }) {
            return Err(CoreError::Refused("invalid_project_root"));
        }
        let (registry, _) = self.registry_snapshot()?;
        crate::machine::routing_observation(&self.environment, project, &registry)
            .map_err(|e| CoreError::Refused(e.code))
    }

    /// Replaces the supplied routing seam with actual native file/catalog observations,
    /// then composes all already-ported connection/MCP/advisory readers.
    /// Defaults, profiles, workspace, route catalog/selector and setup remain supplied.
    /// This additive library method is not a config.read dispatcher or refresh producer.
    pub fn settings_with_routing_readers(
        &self,
        observations: &Value,
        now: f64,
        project: Option<(&Path, &Path)>,
    ) -> CoreResult<SettingsRead> {
        if !observations["surfaces"].is_object() {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        let mut observations = observations.clone();
        observations["surfaces"]["routing"] =
            json!({"ok":true,"result":self.routing_management(project)?});
        self.settings_with_readers(&observations, now)
    }
}

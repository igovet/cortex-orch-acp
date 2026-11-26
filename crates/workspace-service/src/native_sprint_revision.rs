//! `sprint_revision_submit`: the coordinator of a sprint member records its sprint
//! impact verdict through this dispatched, channel-verified tool. The argument
//! contract lives in the advertised schema; the service validates the revision
//! against the live sprint graph (`machine::sprint_revision`) and a refusal names
//! the failing change and rule so the coordinator can correct it in the same turn.
use cortex_core::{
    channels::{self, Credential},
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

pub(super) fn supports(name: &str) -> bool {
    name == "sprint_revision_submit"
}

pub(super) async fn execute(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    state: Option<PathBuf>,
    arguments: Value,
) -> CoreResult<Value> {
    let c = credential;
    executor
        .publication(move |store| {
            let i = store.read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))?;
            if i.role != "coordinator" {
                return Err(CoreError::Refused("tool_refused"));
            }
            let tools = cortex_engine::schema::catalogue(&i.role, None, None)
                .map_err(|_| CoreError::Refused("tool_refused"))?;
            let tool = tools
                .iter()
                .find(|t| t["name"] == "sprint_revision_submit")
                .ok_or(CoreError::Refused("tool_refused"))?;
            cortex_engine::schema::validate(&tool["inputSchema"], &arguments)
                .map_err(|_| CoreError::Refused("invalid_arguments"))?;
            let state = state.ok_or(CoreError::Refused("sprint_membership_required"))?;
            crate::machine::sprint_revision::submit(store, &state, &i.task, &c.channel_id, &arguments)
        })?
        .await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}

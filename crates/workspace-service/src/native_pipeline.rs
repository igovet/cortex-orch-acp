//! The native runtime's hook for the runtime-owned pipeline (`cortex_core::pipeline`).
//! Called at each driver boundary; it never blocks or fails a task: a failed
//! publication is recorded by its code and repaired by the next boundary.
use crate::native_sessions::SessionRoute;
use cortex_core::{store::executor::StoreExecutor, CoreError};
use serde_json::json;
use std::sync::Arc;

pub(crate) async fn publish(
    executor: &Arc<StoreExecutor>,
    task: &str,
    route: Option<&SessionRoute>,
) {
    let t = task.to_owned();
    let route = route.map(|r| {
        let effort = ["effort", "thought_level", "reasoning_effort"]
            .iter()
            .find_map(|k| r.parameters.get(*k).cloned());
        (r.launch.engine.name().to_owned(), r.model.clone(), effort)
    });
    let Ok(reply) = executor.publication(move |store| {
        let named = route
            .as_ref()
            .map(|(engine, model, effort)| cortex_core::pipeline::Route {
                engine,
                model,
                effort: effort.as_deref(),
            });
        let outcome = cortex_core::pipeline::sync(store, &t, named.as_ref());
        if let Err(error) = &outcome {
            let code = error.code();
            let _ = store.write(|db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    &t,
                    "PIPELINE_PUBLICATION_FAILED",
                    json!({"code":code}),
                )?;
                Ok(())
            });
        }
        outcome.map(|_| ())
    }) else {
        return;
    };
    let _ = reply
        .await
        .map_err(|_| CoreError::Refused("native_host_closed"));
}

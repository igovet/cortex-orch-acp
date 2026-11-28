//! Typed sprint evidence accessor, used only after native sprint authentication.
use cortex_core::{deliverables, CoreResult, Store};
use serde_json::Value;
use std::path::Path;

pub(crate) fn read(
    store: &Store,
    _state: &Path,
    consumer_runtime: &str,
    producer: &str,
    name: &str,
    version: &str,
) -> CoreResult<Value> {
    store.read(|db| {
        deliverables::read_for_consumer(store, db, consumer_runtime, producer, name, version)
    })
}

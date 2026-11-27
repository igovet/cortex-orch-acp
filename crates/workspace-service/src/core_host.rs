//! Reusable Core projections served to owner reads, one reader per logical project/channel.
use crate::{actor, failure, local::core_failure as err, project_guard, Service};
use cortex_core::{
    events::StreamFrame, projections::ProjectionReader, OpenMode, ProjectKey, Store, StoreChannel,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use workspace_protocol::core::RequestEnvelope;

#[cfg(feature = "test-observations")]
fn broadcast_identity(
    event: &Value,
) -> Option<(
    cortex_core::test_observations::Batch,
    Vec<cortex_core::test_observations::Change>,
)> {
    use cortex_core::test_observations::{Batch, Change};
    let batch = &event["batch"];
    Some((
        Batch {
            project: event["projectId"].as_str()?.into(),
            generation: batch["generation"].as_str()?.into(),
            from: batch["fromSequence"].as_str()?.parse().ok()?,
            through: batch["throughSequence"].as_str()?.parse().ok()?,
        },
        batch["changes"]
            .as_array()?
            .iter()
            .map(|change| {
                Some(Change {
                    kind: change["kind"].as_str()?.into(),
                    id: change["id"].as_str()?.into(),
                    version: change["newVersion"].as_str()?.parse().ok()?,
                })
            })
            .collect::<Option<_>>()?,
    ))
}

struct Entry {
    project: String,
    reader: ProjectionReader,
    subscription: cortex_core::events::Subscription,
}
#[derive(Default)]
pub(crate) struct Registry {
    slots: Mutex<BTreeMap<PathBuf, Arc<Mutex<Option<Entry>>>>>,
}
// A writer can advance the outbox between poll_model and its read transaction.
fn consistent_read<T>(
    mut read: impl FnMut() -> cortex_core::CoreResult<T>,
) -> cortex_core::CoreResult<T> {
    for attempt in 0..3 {
        match read() {
            Err(error) if error.code() == "snapshot_expired" && attempt < 2 => continue,
            result => return result,
        }
    }
    unreachable!()
}
fn key(root: &std::path::Path) -> io::Result<ProjectKey> {
    ProjectKey::new(
        root,
        if workspace_protocol::channel::current().is_dev() {
            StoreChannel::Dev
        } else {
            StoreChannel::Release
        },
    )
    .map_err(err)
}
impl Registry {
    fn slot(&self, key: &ProjectKey) -> io::Result<Arc<Mutex<Option<Entry>>>> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| failure("storage_unavailable"))?;
        Ok(slots.entry(key.root().to_path_buf()).or_default().clone())
    }
    fn query(
        &self,
        root: PathBuf,
        project: String,
        request: RequestEnvelope<workspace_protocol::core::CoreQuery>,
    ) -> io::Result<Value> {
        // Conversation pages and task facts answer the owner's screen: read at the session's
        // disk priority, not the level the service unit gives task processes (`io_priority`).
        let _disk = cortex_core::io_priority::raised();
        project_guard::check(&root)?;
        let key = key(&root)?;
        let slot = self.slot(&key)?;
        let mut entry = slot.lock().map_err(|_| failure("storage_unavailable"))?;
        if entry.is_none() {
            // The owner waits for this read: queue behind an exclusive migration, never `busy`.
            let store = Store::open_waiting(key, OpenMode::LiveReadOnly).map_err(err)?;
            let reader = ProjectionReader::new(store, project.clone(), actor()).map_err(err)?;
            let reply = consistent_read(|| reader.query(request.body.clone())).map_err(err)?;
            let subscription = reader
                .subscribe(&reply.generation, reply.high_watermark)
                .map_err(err)?;
            *entry = Some(Entry {
                project,
                reader,
                subscription,
            });
            return serde_json::to_value(reply).map_err(|_| failure("invalid_reply"));
        }
        if entry.as_ref().unwrap().project != project {
            return Err(failure("project_context_conflict"));
        }
        let reader = &entry.as_ref().unwrap().reader;
        let reply = consistent_read(|| reader.query(request.body.clone())).map_err(err)?;
        serde_json::to_value(reply).map_err(|_| failure("invalid_reply"))
    }
    fn poll(&self, root: PathBuf, project: String) -> io::Result<Vec<Value>> {
        // A stalled poll holds this project's reader, which the next page read waits for.
        let _disk = cortex_core::io_priority::raised();
        project_guard::check(&root)?;
        let key = key(&root)?;
        let slot = self.slot(&key)?;
        let mut entry = slot.lock().map_err(|_| failure("storage_unavailable"))?;
        let Some(current) = entry.as_ref() else {
            return Ok(Vec::new());
        };
        if current.project != project {
            return Ok(Vec::new());
        }
        if let Err(error) = current.reader.poll() {
            // Drop the stale generation; the next snapshot opens a fresh read provider.
            *entry = None;
            return Ok(vec![
                json!({"type":"native_resync","projectId":project,"reason":error.code()}),
            ]);
        }
        Ok(ProjectionReader::drain(&current.subscription)
            .into_iter()
            .map(|frame| match frame {
                StreamFrame::Delta(batch) => {
                    json!({"type":"native_delta","projectId":project,"batch":batch})
                }
                StreamFrame::Resync(r) => {
                    json!({"type":"native_resync","projectId":project,"reason":r.reason})
                }
            })
            .collect())
    }
}
#[cfg(test)]
mod startup_reads {
    use super::*;
    #[test]
    fn retries_snapshot_races_but_never_hides_persistent_or_policy_failures() {
        let mut calls = 0;
        let result = consistent_read(|| {
            calls += 1;
            if calls < 3 {
                Err(cortex_core::CoreError::Refused("snapshot_expired"))
            } else {
                Ok(31)
            }
        });
        assert_eq!(result.unwrap(), 31);
        assert_eq!(calls, 3);
        calls = 0;
        let failure = consistent_read::<()>(|| {
            calls += 1;
            Err(cortex_core::CoreError::Refused("snapshot_expired"))
        });
        assert_eq!(failure.unwrap_err().code(), "snapshot_expired");
        assert_eq!(calls, 3);
        calls = 0;
        let failure = consistent_read::<()>(|| {
            calls += 1;
            Err(cortex_core::CoreError::Refused("project_inactive"))
        });
        assert_eq!(failure.unwrap_err().code(), "project_inactive");
        assert_eq!(calls, 1);
    }
}
impl Service {
    pub async fn native_query(&self, params: Value) -> io::Result<Value> {
        let request = workspace_protocol::core::decode_native_query(
            &serde_json::to_vec(&params).map_err(|_| failure("invalid_request"))?,
        )
        .map_err(failure)?;
        let project = request
            .project_id
            .clone()
            .ok_or_else(|| failure("invalid_project"))?;
        let root = {
            let state = self.state.lock().await;
            let p = state
                .projects
                .iter()
                .find(|p| p.project_id == project)
                .ok_or_else(|| failure("project_not_found"))?;
            if p.activation.is_none() {
                return Err(failure("project_inactive"));
            }
            p.canonical_root.clone()
        };
        let host = self.core.clone();
        tokio::task::spawn_blocking(move || host.query(root, project, request))
            .await
            .map_err(|_| failure("storage_unavailable"))?
    }
    pub(crate) async fn native_events(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tokio::select! {
                _=shutdown.changed()=>break,
                _=interval.tick()=>{
                    if *shutdown.borrow(){break;}
                    let projects=self.state.lock().await.projects.clone();
                    let mut reads=tokio::task::JoinSet::new();
                    for p in projects.into_iter().filter(|p|p.activation.is_some()) {
                        let host=self.core.clone();
                        reads.spawn_blocking(move||host.poll(p.canonical_root,p.project_id));
                    }
                    while let Some(result)=reads.join_next().await {
                        if let Ok(Ok(events))=result {
                            for event in events {
                                #[cfg(feature = "test-observations")]
                                let trace = (event["type"] == "native_delta"
                                    && cortex_core::test_observations::observing(None))
                                    .then(|| broadcast_identity(&event));
                                let _sent = self.events.send(event);
                                #[cfg(feature = "test-observations")]
                                if let Some(trace) = trace {
                                    use cortex_core::test_observations::{record, Observation};
                                    let sent = std::time::Instant::now();
                                    record(match (trace, _sent) {
                                        (Some((batch, changes)), Ok(receivers)) => {
                                            Observation::Broadcast { batch, changes, sent, receivers }
                                        }
                                        _ => Observation::Missing,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

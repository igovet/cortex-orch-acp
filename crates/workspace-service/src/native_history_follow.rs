//! Private committed conversation delivery. The shared event bus carries only
//! invalidations. Each interested local socket gets its own authorized pages.
use crate::{Service, failure, write_frame};
use cortex_core::projections::HistoryFollowRead;
use serde_json::{Value, json};
use std::io;
use tokio::{
    io::AsyncReadExt,
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    sync::broadcast,
    time::timeout,
};
use workspace_protocol::{core::Version, read::HistoryFollowRequest};

const PAGE_BYTES: usize = 96 * 1024;
const EVENT_DRAIN: usize = 64;

#[derive(Default)]
struct Observation {
    projection_reads: u64,
    decision_reads: u64,
    empty_reads: u64,
}

impl Observation {
    fn mark(&self, event: &mut Value) {
        if cfg!(feature = "test-observations") {
            event["observation"] = json!({
                "projectionReads":self.projection_reads,
                "decisionReads":self.decision_reads,
                "emptyReads":self.empty_reads,
            });
        }
    }
}

fn notice(
    value: &Value,
    request: &HistoryFollowRequest,
    through: Version,
    source_position: Version,
    questions_empty: bool,
) -> Result<bool, &'static str> {
    if value["type"] == "resync_required" {
        return Err("subscription_overflow");
    }
    if value["projectId"] != request.project_id {
        return Ok(false);
    }
    match value["type"].as_str() {
        Some("native_resync") => Err("history_reset"),
        Some("project_state") if value["movingHistory"] == true => Err("history_reset"),
        Some("native_delta") => {
            let batch = &value["batch"];
            if batch["generation"].as_str() != request.generation.as_deref() {
                return Err("history_reset");
            }
            let high: Version = serde_json::from_value(batch["throughSequence"].clone())
                .map_err(|_| "history_reset")?;
            if high <= through {
                return Ok(false);
            }
            let changes = batch["changes"].as_array().ok_or("history_reset")?;
            if changes.iter().any(|change| change["kind"] == "project") {
                return Err("history_reset");
            }
            let task_changed = changes
                .iter()
                .any(|change| change["kind"] == "task" && change["id"] == request.task_id);
            let covered = questions_empty
                && value["historyHeads"].as_array().is_some_and(|heads| {
                    heads.iter().any(|head| {
                        head["taskId"] == request.task_id
                            && head["executionId"] == request.execution_id
                            && head["questionsEmpty"] == true
                            && head["position"].as_str().and_then(|s| s.parse().ok())
                                == Some(source_position.get())
                    })
                });
            Ok(task_changed && !covered)
        }
        _ => Ok(false),
    }
}

fn page_event(
    request: &HistoryFollowRequest,
    read: &HistoryFollowRead,
    observation: &Observation,
) -> io::Result<Value> {
    let mut event = json!({
        "type":"native_history_page",
        "projectId":request.project_id,"taskId":request.task_id,"executionId":request.execution_id,
        "generation":read.generation,"throughSequence":read.through,"fromCursor":request.cursor,
        "page":read.page,
    });
    if let Some(decisions) = &read.decisions {
        event["decisions"] = decisions.clone();
    }
    observation.mark(&mut event);
    if serde_json::to_vec(&event)
        .map_err(|_| failure("invalid_history_page"))?
        .len()
        > PAGE_BYTES
    {
        return Err(failure("history_follow_backlog"));
    }
    Ok(event)
}

impl Service {
    async fn history_follow_frame(
        &self,
        writer: &mut OwnedWriteHalf,
        value: &Value,
    ) -> io::Result<()> {
        timeout(self.config.request_timeout, write_frame(writer, value))
            .await
            .map_err(|_| failure("client_write_timeout"))?
    }

    async fn history_follow_event(
        &self,
        writer: &mut OwnedWriteHalf,
        event: Value,
    ) -> io::Result<()> {
        self.history_follow_frame(
            writer,
            &json!({"jsonrpc":"2.0","method":"service.event","params":event}),
        )
        .await
    }

    async fn history_follow_resync(
        &self,
        writer: &mut OwnedWriteHalf,
        request: &HistoryFollowRequest,
        reason: &str,
        observation: &Observation,
    ) -> io::Result<()> {
        let mut event = json!({"type":"native_history_resync","projectId":request.project_id,
            "taskId":request.task_id,"executionId":request.execution_id,"reason":reason});
        observation.mark(&mut event);
        self.history_follow_event(writer, event).await
    }

    pub(crate) async fn serve_history_follow(
        &self,
        mut reader: OwnedReadHalf,
        mut writer: OwnedWriteHalf,
        id: Value,
        params: Value,
        dedicated: bool,
    ) -> io::Result<()> {
        // The receive position must precede the first snapshot. A commit while
        // the snapshot opens is delivered or already covered by its watermark.
        let mut notices = self.subscribe();
        let mut observation = Observation::default();
        let opened = async {
            self.accepting()?;
            if !dedicated {
                return Err(failure("history_follow_dedicated_socket_required"));
            }
            let permit = self
                .history_follows
                .try_acquire()
                .map_err(|_| failure("history_follow_busy"))?;
            let request: HistoryFollowRequest =
                serde_json::from_value(params).map_err(|_| failure("invalid_request"))?;
            request.validate().map_err(failure)?;
            if self
                .moving_history
                .lock()
                .unwrap()
                .contains(&request.project_id)
            {
                return Err(failure("history_reset"));
            }
            let (root, registration) = self.projection_registration(&request.project_id).await?;
            let mut stopped = registration.stopped();
            let read =
                self.follow_history_read(root.clone(), registration.clone(), request.clone(), None);
            observation.projection_reads += 1;
            let first = tokio::select! {
                biased;
                _ = stopped.changed() => return Err(failure("project_inactive")),
                result = read => result?,
            };
            observation.decision_reads += 1;
            let event = page_event(&request, &first, &observation)?;
            Ok::<_, io::Error>((request, root, registration, stopped, first, event, permit))
        };
        let result = tokio::select! {
            biased;
            _ = reader.read_u8() => return Ok(()),
            result = opened => result,
        };
        let (mut request, root, registration, mut stopped, first, event, _permit) = match result {
            Ok(opened) => opened,
            Err(error) => {
                return self.history_follow_frame(&mut writer, &json!({
                    "jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.to_string()}
                })).await;
            }
        };
        if *stopped.borrow() {
            return Ok(());
        }
        self.history_follow_frame(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":id,"result":{"subscribed":true}}),
        )
        .await?;
        if *stopped.borrow() {
            return self
                .history_follow_resync(&mut writer, &request, "project_inactive", &observation)
                .await;
        }
        self.history_follow_event(&mut writer, event).await?;
        request.generation = Some(first.generation);
        request.cursor = first.page.tail_cursor;
        let mut through = first.through;
        let mut revision = first.page.decision_revision;
        let mut position = first.page.high_watermark;
        let mut questions_empty = first.decisions.as_ref().is_some_and(empty_decisions);
        loop {
            let next = tokio::select! {
                biased;
                _ = reader.read_u8() => return Ok(()),
                _ = stopped.changed() => Err("project_inactive"),
                event = self.receive_event(&mut notices) => match event {
                    Ok(event) => notice(&event, &request, through, position, questions_empty),
                    Err(_) => return Ok(()),
                },
            };
            let mut changed = match next {
                Ok(changed) => changed,
                Err(reason) => {
                    return self
                        .history_follow_resync(&mut writer, &request, reason, &observation)
                        .await;
                }
            };
            for _ in 0..EVENT_DRAIN {
                match notices.try_recv() {
                    Ok(event) => match notice(&event, &request, through, position, questions_empty)
                    {
                        Ok(wake) => changed |= wake,
                        Err(reason) => {
                            return self
                                .history_follow_resync(&mut writer, &request, reason, &observation)
                                .await;
                        }
                    },
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        return self
                            .history_follow_resync(
                                &mut writer,
                                &request,
                                "subscription_overflow",
                                &observation,
                            )
                            .await;
                    }
                    Err(_) => break,
                }
            }
            if !changed {
                continue;
            }
            let read = self.follow_history_read(
                root.clone(),
                registration.clone(),
                request.clone(),
                revision.clone(),
            );
            observation.projection_reads += 1;
            let next = tokio::select! {
                biased;
                _ = reader.read_u8() => return Ok(()),
                _ = stopped.changed() => Err(failure("project_inactive")),
                result = read => result,
            };
            let next = match next {
                Ok(next) if !*stopped.borrow() && next.through > through => next,
                Ok(_) => {
                    return self
                        .history_follow_resync(&mut writer, &request, "history_reset", &observation)
                        .await;
                }
                Err(error) => {
                    return self
                        .history_follow_resync(
                            &mut writer,
                            &request,
                            &error.to_string(),
                            &observation,
                        )
                        .await;
                }
            };
            observation.decision_reads += 1;
            through = next.through;
            if next.page.items.is_empty()
                && next.page.tail_cursor == request.cursor
                && next.page.decision_revision == revision
            {
                observation.empty_reads += 1;
                continue;
            }
            let event = match page_event(&request, &next, &observation) {
                Ok(event) => event,
                Err(error) => {
                    return self
                        .history_follow_resync(
                            &mut writer,
                            &request,
                            &error.to_string(),
                            &observation,
                        )
                        .await;
                }
            };
            self.history_follow_event(&mut writer, event).await?;
            request.cursor = next.page.tail_cursor;
            position = next.page.high_watermark;
            if next.page.decision_revision != revision {
                questions_empty = next.decisions.as_ref().is_some_and(empty_decisions);
            }
            revision = next.page.decision_revision;
        }
    }
}

fn empty_decisions(page: &Value) -> bool {
    page["items"].as_array().is_some_and(Vec::is_empty)
}

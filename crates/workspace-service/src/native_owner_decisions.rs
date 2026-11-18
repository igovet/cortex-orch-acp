//! "Ask the owner" for a native permission request (the access mode's decision
//! table, `permission::classify`) and for Cortex's `outside_*` consent cards.
//!
//! An incoming ACP client (Paseo, Zed) answers on its own transport, as before.
//! A native Workspace task has no such client: its request becomes an owner
//! decision of kind `permission` in the task's decisions projection
//! (`cortex_core::permission_decisions`), shown in the Conversation and raised
//! by the notifier like a question. The engine's request stays pending while it
//! is open; the decision is withdrawn when the task pauses or stops, the
//! worker's channel or session ends, or the request is abandoned.
use crate::native_sessions::ClientPermissions;
use cortex_core::{
    channels::{self, Credential},
    permission_decisions as pd,
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use cortex_engine::{engine::PermissionDecision, permission};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// How the owner answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    Allowed,
    Denied,
    /// The desktop decision closed without an answer, with its reason.
    Withdrawn(String),
    /// No owner surface exists for this task.
    Unavailable,
}

const POLL: Duration = Duration::from_millis(200);

async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}

/// Closes a still-open decision when its waiter is dropped (the engine's
/// request ended, the turn was cancelled, the session closed).
struct Guard {
    executor: Arc<StoreExecutor>,
    task: String,
    id: String,
    armed: bool,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed {
            let (task, id) = (self.task.clone(), self.id.clone());
            let _ = self.executor.write(move |db| {
                // A pause or stop ends the turn first; name that rather than the turn end.
                let state: Option<String> = db
                    .query_row(
                        "SELECT state FROM runtime_tasks WHERE task_id=?",
                        [&task],
                        |r| r.get(0),
                    )
                    .ok();
                let reason = match state.as_deref() {
                    Some("PAUSED") => "task_paused",
                    Some("CANCELLED" | "COMPLETED" | "FAILED") => "task_stopped",
                    _ => "engine_request_ended",
                };
                pd::withdraw_tx(db, &task, &id, reason)
            });
        }
    }
}

/// Asks the owner about `card` (an ACP permission request with Allow once and
/// Deny). `details` carries role, operation, paths, reason, access mode and
/// rule for the desktop decision; `request` identifies the engine request.
pub(crate) async fn ask(
    executor: &Arc<StoreExecutor>,
    credential: &Credential,
    provider: Option<Arc<dyn ClientPermissions>>,
    incoming: Option<String>,
    request: &str,
    card: &Value,
    details: Value,
) -> CoreResult<Owner> {
    if let (Some(provider), Some(incoming)) = (provider, incoming) {
        return Ok(match provider.decide(&incoming, card).await {
            Ok(PermissionDecision::Selected(option))
                if permission::offered_allow(card, &option) =>
            {
                Owner::Allowed
            }
            _ => Owner::Denied,
        });
    }
    let c = credential.clone();
    let prompt = card["toolCall"]["title"].as_str().unwrap_or("").to_owned();
    let request = request.to_owned();
    let opened = recv(executor.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if !pd::workspace_task_tx(db, &i.task)? {
            return Ok(None);
        }
        let id = pd::open_tx(
            db,
            &i.task,
            i.assignment.as_deref(),
            &c.channel_id,
            &request,
            &prompt,
            &details,
        )?;
        Ok(Some((i.task, id)))
    })?)
    .await?;
    let Some((task, id)) = opened else {
        return Ok(Owner::Unavailable);
    };
    let mut guard = Guard {
        executor: executor.clone(),
        task: task.clone(),
        id: id.clone(),
        armed: true,
    };
    loop {
        tokio::time::sleep(POLL).await;
        let c = credential.clone();
        let (t, q) = (task.clone(), id.clone());
        let seen = recv(executor.read(move |db| {
            let state = pd::state_tx(db, &q)?;
            let live = channels::authenticate(db, &c.channel_id, c.secret(), true).is_ok();
            let task_state: Option<String> = db
                .query_row(
                    "SELECT state FROM runtime_tasks WHERE task_id=?",
                    [&t],
                    |r| r.get(0),
                )
                .ok();
            Ok((state, live, task_state))
        })?)
        .await?;
        let (state, live, task_state) = seen;
        match state {
            Some((status, answer)) if status == "answered" => {
                guard.armed = false;
                return Ok(if answer.as_deref() == Some(pd::ALLOWED) {
                    Owner::Allowed
                } else {
                    Owner::Denied
                });
            }
            Some((status, _)) if status != "open" => {
                guard.armed = false;
                return Ok(Owner::Withdrawn(status));
            }
            None => {
                guard.armed = false;
                return Ok(Owner::Withdrawn("missing".into()));
            }
            _ => {}
        }
        let reason = match task_state.as_deref() {
            Some("PAUSED") => Some("task_paused"),
            Some("CANCELLED" | "COMPLETED" | "FAILED") | None => Some("task_stopped"),
            _ if !live => Some("session_ended"),
            _ => None,
        };
        if let Some(reason) = reason {
            let (t, q) = (task.clone(), id.clone());
            recv(executor.write(move |db| pd::withdraw_tx(db, &t, &q, reason))?).await?;
            guard.armed = false;
            return Ok(Owner::Withdrawn(reason.into()));
        }
    }
}

/// The decision details for an asked native request.
pub(crate) fn details(request: &Value, role: &str, rule: &str, access: &str) -> Value {
    serde_json::json!({
        "role": role,
        "operation": permission::operation(request),
        "paths": permission::paths(request),
        "reason": permission::reason_code(rule),
        "reason_text": permission::ask_reason(rule),
        "access": access,
        "rule": rule,
        "tool": permission::tool_name(request),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::collections::HashMap;

    /// A pending permission decision raises the notifier's `question`
    /// notification, whose click opens that decision in its task.
    #[test]
    fn a_pending_permission_decision_is_a_question_notification() {
        let tasks = [
            json!({"taskId":"t1","description":"Work","phase":"execution","executionId":"e-t1","lastConfirmedAt":"t1"}),
        ];
        let product = json!({"tasks":{"t1":{"title":"Work"}},"sprints":[]});
        let decision = json!({"decisionId":"wd_np_1","kind":"permission","status":"pending",
            "prompt":"Owner decision: worker-backend-dev wants to read /etc/hosts because it reads outside the project",
            "allowedOperations":["decisions.answer"]});
        let decisions = HashMap::from([("t1".to_owned(), vec![decision])]);
        let found = crate::notifier::items("p", &tasks, &product, &decisions);
        let item = found.iter().find(|i| i.place.id == "t1").unwrap();
        assert_eq!(item.kind, crate::notifier::Kind::Question);
        assert_eq!(
            item.target(),
            json!({"kind":"question","project":"p","task":"t1","decision":"wd_np_1"})
        );
    }
}

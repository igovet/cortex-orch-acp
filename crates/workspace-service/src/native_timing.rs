use cortex_core::{
    observability::{self, Category, Outcome, Scope},
    store::executor::StoreExecutor,
};
use std::time::Duration;

pub(super) async fn record(
    executor: &StoreExecutor,
    task: &str,
    scope: Scope,
    category: Category,
    outcome: Outcome,
    elapsed: Duration,
) {
    let task = task.to_owned();
    let recorded = async {
        executor
            .write(move |db| {
                observability::record_tx(db, &task, &scope, category, outcome, elapsed)
            })?
            .await
            .map_err(|_| cortex_core::CoreError::Refused("storage_executor_closed"))?
    }
    .await;
    if let Err(error) = recorded {
        eprintln!(
            "workspace-service: timing write unavailable ({})",
            error.code()
        );
    }
}

pub(super) fn outcome<T, E>(result: &Result<T, E>) -> Outcome {
    if result.is_ok() {
        Outcome::Returned
    } else {
        Outcome::Error
    }
}

pub(super) fn turn_category(
    db: &rusqlite::Connection,
    task: &str,
    role: &str,
    stage: Option<&str>,
) -> cortex_core::CoreResult<Option<Category>> {
    if role == "worker" {
        return Ok(Some(if stage == Some("review") {
            Category::ReviewTurn
        } else {
            Category::WorkerTurn
        }));
    }
    let state: String = db.query_row(
        "SELECT state FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    Ok(match state.as_str() {
        "PLANNING" => Some(Category::PlanningTurn),
        "RECONCILING" => Some(Category::ReconciliationTurn),
        _ => None,
    })
}

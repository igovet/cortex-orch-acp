//! Task CAS epochs and a separate lifetime Core owner fence.
//! Parity: ownership.py:30-63; shared .access.lock is NEVER the owner lock.
use crate::{
    error::refuse,
    store::{locks::AccessLock, ProjectKey},
    CoreResult,
};
use rusqlite::{OptionalExtension, Transaction};
use std::time::Duration;
pub struct ProjectOwner {
    _lease: AccessLock,
}
impl ProjectOwner {
    pub fn acquire(key: &ProjectKey) -> CoreResult<Self> {
        Ok(Self {
            _lease: AccessLock::acquire(
                &key.directory().join(".core-owner.lock"),
                true,
                true,
                Duration::ZERO,
            )?,
        })
    }
}
/// Shared by updated legacy ingress and native executors. Keep this guard on the
/// writer lane until its last session reference is gone, not merely until a
/// NativeHost handle is dropped. Older runtimes do not honor this fence.
pub struct SchedulerOwner {
    _lease: AccessLock,
}
impl SchedulerOwner {
    pub fn acquire(key: &ProjectKey) -> CoreResult<Self> {
        let lease = AccessLock::acquire(
            &key.directory().join(".scheduler-owner.lock"),
            true,
            true,
            Duration::ZERO,
        )
        .map_err(|e| {
            if e.code() == "busy" {
                crate::CoreError::Refused("scheduler_owner_busy")
            } else {
                e
            }
        })?;
        Ok(Self { _lease: lease })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerEpoch {
    pub owner: String,
    pub epoch: i64,
}
pub fn acquire_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &str,
    expected: Option<i64>,
) -> CoreResult<OwnerEpoch> {
    if owner.is_empty() {
        return refuse("invalid_owner_ref");
    }
    let row: Option<i64> = db
        .query_row(
            "SELECT epoch FROM task_owners WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    if row.map_or(expected.unwrap_or(0) != 0, |n| Some(n) != expected) {
        return refuse("stale_owner");
    }
    let epoch = row
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(crate::CoreError::Refused("owner_epoch_overflow"))?;
    db.execute("INSERT INTO task_owners VALUES (?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET owner_ref=excluded.owner_ref,epoch=excluded.epoch,acquired_at=excluded.acquired_at",
        (task,owner,epoch,crate::now()))?;
    Ok(OwnerEpoch {
        owner: owner.into(),
        epoch,
    })
}
pub fn check_tx(db: &Transaction<'_>, task: &str, owner: &OwnerEpoch) -> CoreResult<()> {
    let row: Option<(String, i64)> = db
        .query_row(
            "SELECT owner_ref,epoch FROM task_owners WHERE task_id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if row != Some((owner.owner.clone(), owner.epoch)) {
        return refuse("stale_owner");
    }
    Ok(())
}

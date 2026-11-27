//! In-process metadata capture for the disposable service fixture. Never built by default.
use crate::ProjectKey;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

const CAPACITY: usize = 65_536;
static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Change {
    pub kind: String,
    pub id: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub project: String,
    pub generation: String,
    pub from: i64,
    pub through: i64,
}

pub enum Observation {
    Commit {
        project: String,
        from: i64,
        through: i64,
        returned: Instant,
    },
    Projection {
        batch: Batch,
        depth: usize,
        changes: Vec<(Change, Duration)>,
    },
    Broadcast {
        batch: Batch,
        changes: Vec<Change>,
        sent: Instant,
        receivers: usize,
    },
    Missing,
}

struct Capture {
    root: PathBuf,
    records: Vec<Observation>,
    overflow: bool,
}

/// Exactly one disposable project in this process; the fixture owns its server lifetime.
pub struct Observer(bool);

impl Observer {
    pub fn start(root: PathBuf) -> Self {
        let mut slot = CAPTURE.lock().unwrap();
        assert!(slot.is_none(), "test observation already active");
        *slot = Some(Capture {
            root,
            records: Vec::new(),
            overflow: false,
        });
        Self(true)
    }

    pub fn finish(mut self) -> (Vec<Observation>, bool) {
        let capture = CAPTURE.lock().unwrap().take().unwrap();
        self.0 = false;
        (capture.records, capture.overflow)
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        if self.0 {
            *CAPTURE.lock().unwrap() = None;
        }
    }
}

pub fn observing(root: Option<&Path>) -> bool {
    CAPTURE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|capture| root.is_none_or(|root| capture.root == root))
}

pub fn record(observation: Observation) {
    if let Some(capture) = CAPTURE.lock().unwrap().as_mut() {
        if capture.records.len() == CAPACITY {
            capture.overflow = true;
        } else {
            capture.records.push(observation);
        }
    }
}

type Watermarks = BTreeMap<String, i64>;

fn watermarks(db: &Connection) -> rusqlite::Result<Watermarks> {
    db.prepare("SELECT project_id, high_watermark FROM workspace_outbox_state")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

pub(crate) struct Before(Option<Watermarks>);
pub(crate) struct Committing(Option<(Watermarks, Watermarks)>);

pub(crate) fn before_write(key: &ProjectKey, db: &Connection) -> Option<Before> {
    observing(Some(key.root())).then(|| Before(watermarks(db).ok()))
}

impl Before {
    pub(crate) fn finish(self, db: &Connection) -> Committing {
        // Both observations are inside the very same BEGIN IMMEDIATE. Comparing
        // the committed transaction's range also handles rolled-back savepoints.
        Committing(self.0.zip(watermarks(db).ok()))
    }
}

pub(crate) fn committed(mark: Committing, returned: Instant) {
    let Some((before, after)) = mark.0 else {
        record(Observation::Missing);
        return;
    };
    if before
        .iter()
        .any(|(project, old)| after.get(project).is_none_or(|new| new < old))
    {
        record(Observation::Missing);
    }
    for (project, through) in after {
        let old = before.get(&project).copied().unwrap_or(0);
        if through > old {
            record(Observation::Commit {
                project,
                from: old + 1,
                through,
                returned,
            });
        }
    }
}

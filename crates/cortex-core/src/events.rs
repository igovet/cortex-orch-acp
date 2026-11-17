//! Bounded, generation-local replay. Durable outbox metadata is NEVER replayed as DTOs.
use crate::{error::refuse, CoreResult};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use workspace_protocol::{
    core::Version,
    events::{DeltaBatch, ResyncRequired, StreamKey},
};
const MAX_BATCHES: usize = 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(300);
const SUBSCRIBER_FRAMES: usize = 128;
#[derive(Debug, Clone)]
pub enum StreamFrame {
    Delta(DeltaBatch),
    Resync(ResyncRequired),
}
#[derive(Clone)]
pub struct Subscription {
    queue: Arc<Mutex<VecDeque<StreamFrame>>>,
}
impl Subscription {
    pub fn next(&self) -> Option<StreamFrame> {
        self.queue.lock().ok()?.pop_front()
    }
}
pub struct EventStream {
    generation: String,
    key: StreamKey,
    through: Version,
    ring: VecDeque<(Instant, usize, DeltaBatch)>,
    bytes: usize,
    versions: BTreeMap<String, Version>,
    versions_bytes: usize,
    subscribers: Vec<std::sync::Weak<Mutex<VecDeque<StreamFrame>>>>,
}
impl EventStream {
    pub fn new(key: StreamKey, high_watermark: Version) -> Self {
        Self {
            generation: uuid::Uuid::new_v4().to_string(),
            key,
            through: high_watermark,
            ring: VecDeque::new(),
            bytes: 0,
            versions: BTreeMap::new(),
            versions_bytes: 0,
            subscribers: Vec::new(),
        }
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn high_watermark(&self) -> Version {
        self.through
    }
    fn resync(&self, reason: &str) -> ResyncRequired {
        ResyncRequired {
            stream_key: self.key.clone(),
            generation: self.generation.clone(),
            reason: reason.into(),
        }
    }
    fn evict(&mut self, now: Instant) {
        while self.ring.front().is_some_and(|(at, _, _)| {
            self.ring.len() > MAX_BATCHES
                || self.bytes > MAX_BYTES
                || now.duration_since(*at) >= MAX_AGE
        }) {
            if let Some((_, n, _)) = self.ring.pop_front() {
                self.bytes -= n;
            }
        }
    }
    /// Called only AFTER commit by the project actor, before the next mutation.
    pub fn publish(&mut self, batch: DeltaBatch) -> CoreResult<()> {
        self.publish_at(batch, Instant::now())
    }
    fn publish_at(&mut self, batch: DeltaBatch, now: Instant) -> CoreResult<()> {
        batch.validate().map_err(crate::CoreError::Refused)?;
        if batch.generation != self.generation
            || batch.stream_key != self.key
            || self.through.get().checked_add(1) != Some(batch.from_sequence.get())
        {
            return refuse("resync_required");
        }
        let mut updates = BTreeMap::new();
        for c in &batch.changes {
            let id = format!("{:?}:{}", c.kind, c.id);
            if let Some(v) = updates.get(&id).or_else(|| self.versions.get(&id)) {
                if c.base_version != *v {
                    return refuse("resync_required");
                }
            }
            updates.insert(id, c.new_version);
        }
        // Conservative budget for the disposable per-entity projection version map.
        let added = updates
            .keys()
            .filter(|k| !self.versions.contains_key(*k))
            .map(|k| k.len() + 256)
            .sum::<usize>();
        if self.versions_bytes.saturating_add(added) > 32 * 1024 * 1024 {
            return refuse("projection_budget_exceeded");
        }
        self.versions_bytes += added;
        self.versions.extend(updates);
        self.through = batch.through_sequence;
        let size = serde_json::to_vec(&batch)?.len();
        self.ring.push_back((now, size, batch.clone()));
        self.bytes += size;
        self.evict(now);
        let overflow = self.resync("subscriber_overflow");
        self.subscribers.retain(|weak| {
            let Some(q) = weak.upgrade() else {
                return false;
            };
            let Ok(mut q) = q.lock() else {
                return false;
            };
            if q.len() >= SUBSCRIBER_FRAMES {
                q.clear();
                q.push_back(StreamFrame::Resync(overflow.clone()));
            } else if !q.iter().any(|f| matches!(f, StreamFrame::Resync(_))) {
                q.push_back(StreamFrame::Delta(batch.clone()));
            }
            true
        });
        Ok(())
    }
    pub fn replay(
        &mut self,
        generation: &str,
        after: Version,
    ) -> Result<Vec<DeltaBatch>, ResyncRequired> {
        self.evict(Instant::now());
        if generation != self.generation {
            return Err(self.resync("generation_changed"));
        }
        if after > self.through {
            return Err(self.resync("future_cursor"));
        }
        if after == self.through {
            return Ok(Vec::new());
        }
        let Some((_, _, first)) = self.ring.front() else {
            return Err(self.resync("floor_expired"));
        };
        if after.get() < first.from_sequence.get() - 1 {
            return Err(self.resync("floor_expired"));
        }
        let retained: Vec<_> = self
            .ring
            .iter()
            .filter(|(_, _, b)| b.through_sequence > after)
            .map(|(_, _, b)| b.clone())
            .collect();
        if retained
            .first()
            .is_none_or(|b| after.get().checked_add(1) != Some(b.from_sequence.get()))
        {
            return Err(self.resync("batch_boundary_required"));
        }
        Ok(retained)
    }
    pub fn subscribe(&mut self, generation: &str, after: Version) -> Subscription {
        let mut queue = VecDeque::new();
        match self.replay(generation, after) {
            Ok(batches) if batches.len() <= SUBSCRIBER_FRAMES => {
                queue.extend(batches.into_iter().map(StreamFrame::Delta))
            }
            Ok(_) => queue.push_back(StreamFrame::Resync(self.resync("subscriber_overflow"))),
            Err(e) => queue.push_back(StreamFrame::Resync(e)),
        }
        let q = Arc::new(Mutex::new(queue));
        self.subscribers.push(Arc::downgrade(&q));
        Subscription { queue: q }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use workspace_protocol::events::{DeltaOperation, EntityChange, EntityKind};
    fn stream() -> EventStream {
        EventStream::new(
            StreamKey::Project {
                project_id: "p".into(),
            },
            Version::new(0).unwrap(),
        )
    }
    fn batch(s: &EventStream, n: i64) -> DeltaBatch {
        DeltaBatch {
            generation: s.generation.clone(),
            stream_key: s.key.clone(),
            from_sequence: Version::new(n).unwrap(),
            through_sequence: Version::new(n).unwrap(),
            changes: vec![EntityChange {
                kind: EntityKind::Task,
                id: "t".into(),
                base_version: Version::new(n - 1).unwrap(),
                new_version: Version::new(n).unwrap(),
                operation: DeltaOperation::Invalidate,
            }],
        }
    }
    #[test]
    fn replay_is_contiguous_generation_bound_and_project_scoped() {
        let mut s = stream();
        let g = s.generation.clone();
        for n in 1..=3 {
            s.publish(batch(&s, n)).unwrap();
        }
        assert_eq!(s.replay(&g, Version::new(1).unwrap()).unwrap().len(), 2);
        assert!(s.replay("restarted", Version::new(1).unwrap()).is_err());
        assert!(s.replay(&g, Version::new(4).unwrap()).is_err());
        let mut wrong = batch(&s, 4);
        wrong.stream_key = StreamKey::Project {
            project_id: "other".into(),
        };
        assert!(s.publish(wrong).is_err());
        let mut backwards = batch(&s, 4);
        backwards.changes[0].base_version = Version::new(1).unwrap();
        assert!(s.publish(backwards).is_err());
        assert!(s.publish(batch(&s, 5)).is_err());
        assert_eq!(s.high_watermark().get(), 3);
    }
    #[test]
    fn subscriber_overflow_requires_resnapshot_and_does_not_stop_publishing() {
        let mut s = stream();
        let g = s.generation.clone();
        let sub = s.subscribe(&g, Version::new(0).unwrap());
        for n in 1..=200 {
            s.publish(batch(&s, n)).unwrap();
        }
        assert!(matches!(sub.next(), Some(StreamFrame::Resync(_))));
        assert!(sub.next().is_none());
        assert_eq!(s.high_watermark().get(), 200);
    }
    #[test]
    fn ring_evicts_by_count_age_and_byte_budget() {
        let mut s = stream();
        let g = s.generation.clone();
        for n in 1..=1025 {
            s.publish(batch(&s, n)).unwrap();
        }
        assert!(s.replay(&g, Version::new(0).unwrap()).is_err());
        assert_eq!(s.ring.len(), 1024);
        s.evict(Instant::now() + Duration::from_secs(301));
        assert!(s.ring.is_empty());
        assert_eq!(s.bytes, 0);
        let mut s = stream();
        for n in 1..=300 {
            let mut b = batch(&s, n);
            b.changes = (0..200)
                .map(|j| {
                    let mut c = b.changes[0].clone();
                    c.id = format!("{j:0120}");
                    c
                })
                .collect();
            s.publish(b).unwrap();
        }
        assert!(s.bytes <= MAX_BYTES);
        assert!(s.ring.len() < 300);
    }
    #[test]
    fn transaction_ranges_resume_only_at_whole_batch_boundaries() {
        let mut s = stream();
        let g = s.generation.clone();
        let mut b = batch(&s, 1);
        b.through_sequence = Version::new(4).unwrap();
        s.publish(b).unwrap();
        assert!(s.replay(&g, Version::new(2).unwrap()).is_err());
        assert_eq!(s.replay(&g, Version::new(0).unwrap()).unwrap().len(), 1);
    }
    #[test]
    fn projection_budget_failure_does_not_claim_event_was_published() {
        let mut s = stream();
        s.versions_bytes = 32 * 1024 * 1024;
        assert_eq!(
            s.publish(batch(&s, 1)).unwrap_err().code(),
            "projection_budget_exceeded"
        );
        assert_eq!(s.high_watermark().get(), 0);
    }
}

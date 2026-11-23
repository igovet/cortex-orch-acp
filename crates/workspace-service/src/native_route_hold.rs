//! Temporary provider refusals are local to one exact project route, not a
//! routing-policy change. The first launch after expiry is a single probe.
use cortex_engine::{LimitKind, ProviderLimit};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Notify, time::Instant};

pub(super) const LIMITED: &str = "native_provider_temporarily_limited";

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct RouteKey(pub &'static str, pub String, pub String);

#[derive(Default)]
pub(super) struct Holds {
    states: Mutex<HashMap<RouteKey, Hold>>,
    pub changed: Notify,
}

struct Hold {
    limit: LimitKind,
    until: Instant,
    failures: u32,
    probe: bool,
}

pub(super) enum Admission {
    Ready,
    Waiting { reason: &'static str, until: Option<Instant> },
}

// A probe is owned by precisely one launch future. Dropping the future or
// failing to start it must not strand other waiters behind an abandoned probe.
pub(super) struct Attempt {
    holds: Arc<Holds>,
    key: RouteKey,
    probe: bool,
    pub refused_channel: Option<String>,
}

impl Attempt {
    pub fn new(holds: Arc<Holds>, key: RouteKey) -> Self {
        Self { holds, key, probe: false, refused_channel: None }
    }

    pub fn key(&self) -> &RouteKey {
        &self.key
    }

    pub fn admit(&mut self, first_for_route: bool) -> Admission {
        let mut states = self.holds.states.lock().unwrap();
        let Some(hold) = states.get_mut(&self.key) else {
            self.probe = false;
            return Admission::Ready;
        };
        let now = Instant::now();
        let owns_probe = self.probe && hold.probe;
        if hold.until > now || (!owns_probe && (hold.probe || !first_for_route)) {
            return Admission::Waiting {
                reason: match hold.limit {
                    LimitKind::Rate => "provider_rate_limited",
                    LimitKind::Quota => "provider_quota_limited",
                },
                until: (hold.until > now && (!hold.probe || owns_probe)).then_some(hold.until),
            };
        }
        hold.probe = true;
        self.probe = true;
        Admission::Ready
    }

    pub(super) fn reservation_gate(&self) -> ReservationGate {
        ReservationGate {
            holds: self.holds.clone(),
            key: self.key.clone(),
            probe: self.probe,
        }
    }

    pub fn limited(&mut self, limit: ProviderLimit, channel: String) {
        let unix_now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        self.limited_at(limit, channel, unix_now);
    }

    fn limited_at(&mut self, limit: ProviderLimit, channel: String, unix_now: Duration) {
        let mut states = self.holds.states.lock().unwrap();
        let previous = states.get(&self.key);
        let failures = previous.map_or(0, |h| h.failures.saturating_add(1));
        // Only the fallback is capped. Never cut short a recognized provider hint.
        let backoff = Duration::from_secs((5 * (1 << failures.min(6))).min(300));
        let retry_after = Duration::from_secs(limit.retry_after_seconds.unwrap_or(0).into());
        let reset = limit.resets_at_unix_seconds
            .map(|at| Duration::from_secs(at).saturating_sub(unix_now))
            .unwrap_or_default();
        let until = Instant::now() + backoff.max(retry_after).max(reset);
        let until = previous.map_or(until, |h| h.until.max(until));
        // An already-running request may report a new refusal while a probe is
        // running. Extend the hold, but leave that probe's ownership intact.
        let probe = !self.probe && previous.is_some_and(|h| h.probe);
        states.insert(self.key.clone(), Hold { limit: limit.kind, until, failures, probe });
        self.probe = false;
        self.refused_channel = Some(channel);
        self.holds.changed.notify_waiters();
    }

    pub fn succeeded(&mut self) {
        if self.probe {
            let mut states = self.holds.states.lock().unwrap();
            if let Some(hold) = states.get_mut(&self.key) {
                if hold.until > Instant::now() {
                    // A concurrent, already-running request extended the hold.
                    hold.probe = false;
                } else {
                    states.remove(&self.key);
                }
            }
            self.probe = false;
            self.holds.changed.notify_waiters();
        }
    }
}

/// Recheck inside the store writer, not just before queuing the write. A
/// refusal and the reservation must be ordered against the same hold state.
pub(super) struct ReservationGate {
    holds: Arc<Holds>,
    key: RouteKey,
    probe: bool,
}

pub(super) struct ReservationGuard<'a> {
    _states: MutexGuard<'a, HashMap<RouteKey, Hold>>,
}

impl ReservationGate {
    pub(super) fn lock_ready(&self) -> Option<ReservationGuard<'_>> {
        let states = self.holds.states.lock().unwrap();
        let ready = states.get(&self.key).is_none_or(|hold| {
            self.probe && hold.probe && hold.until <= Instant::now()
        });
        ready.then_some(ReservationGuard { _states: states })
    }
}

#[cfg(test)]
impl Holds {
    pub(super) fn expire_for_test(&self, key: &RouteKey) {
        self.states.lock().unwrap().get_mut(key).unwrap().until = Instant::now();
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn provider_hold_honors_reset_and_retry_after_and_allows_one_probe() {
        let holds = Arc::new(Holds::default());
        let key = RouteKey("codex-acp", "conn".into(), "one".into());
        let mut first = Attempt::new(holds.clone(), key.clone());
        let mut second = Attempt::new(holds.clone(), key.clone());
        let mut other = Attempt::new(holds.clone(), RouteKey("codex-acp", "conn".into(), "two".into()));
        let rate = ProviderLimit {
            kind: LimitKind::Rate, retry_after_seconds: Some(11), resets_at_unix_seconds: Some(120),
        };
        let now = Instant::now();
        first.limited_at(rate, "first".into(), Duration::from_secs(100));
        assert!(matches!(
            first.admit(true), Admission::Waiting { until: Some(at), .. } if at == now + Duration::from_secs(20)
        ));
        assert!(matches!(other.admit(true), Admission::Ready));
        tokio::time::advance(Duration::from_secs(19)).await;
        assert!(matches!(first.admit(true), Admission::Waiting { .. }));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(first.admit(true), Admission::Ready));
        assert!(matches!(second.admit(true), Admission::Waiting { until: None, .. }));
        first.limited_at(rate, "retry".into(), Duration::from_secs(120));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(matches!(second.admit(true), Admission::Waiting { .. }));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(second.admit(true), Admission::Ready));
        second.succeeded();
        assert!(matches!(first.admit(true), Admission::Ready));
        assert!(holds.states.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn provider_hold_rechecks_a_probe_after_a_concurrent_extension() {
        let holds = Arc::new(Holds::default());
        let key = RouteKey("codex-acp", "conn".into(), "one".into());
        let mut running = Attempt::new(holds.clone(), key.clone());
        let mut probe = Attempt::new(holds.clone(), key.clone());
        let mut follower = Attempt::new(holds, key);
        assert!(matches!(probe.admit(true), Admission::Ready));
        let before_hold = probe.reservation_gate();
        let limit = ProviderLimit {
            kind: LimitKind::Rate,
            retry_after_seconds: Some(15),
            resets_at_unix_seconds: None,
        };
        running.limited_at(limit, "first".into(), Duration::ZERO);
        assert!(before_hold.lock_ready().is_none());
        tokio::time::advance(Duration::from_secs(15)).await;
        assert!(matches!(probe.admit(true), Admission::Ready));
        let gate = probe.reservation_gate();
        assert!(gate.lock_ready().is_some());

        let extended_until = Instant::now() + Duration::from_secs(15);
        running.limited_at(limit, "concurrent".into(), Duration::ZERO);
        assert!(gate.lock_ready().is_none());
        assert!(matches!(
            probe.admit(true),
            Admission::Waiting { reason: "provider_rate_limited", until: Some(at) }
                if at == extended_until
        ));
        assert!(matches!(follower.admit(true), Admission::Waiting { until: None, .. }));
        tokio::time::advance(Duration::from_secs(14)).await;
        assert!(gate.lock_ready().is_none());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(follower.admit(true), Admission::Waiting { until: None, .. }));
        assert!(matches!(probe.admit(true), Admission::Ready));
        assert!(gate.lock_ready().is_some());
        probe.succeeded();
        assert!(matches!(follower.admit(true), Admission::Ready));
    }

    #[tokio::test(start_paused = true)]
    async fn provider_hold_bounds_repeated_fallback_and_releases_a_cancelled_probe() {
        let holds = Arc::new(Holds::default());
        let key = RouteKey("codex-acp", "conn".into(), "one".into());
        let mut probe = Attempt::new(holds.clone(), key.clone());
        let limit = ProviderLimit {
            kind: LimitKind::Quota, retry_after_seconds: None, resets_at_unix_seconds: None,
        };
        for delay in [5, 10, 20, 40, 80, 160, 300, 300] {
            let since = Instant::now();
            probe.limited_at(limit, "fixture".into(), Duration::ZERO);
            assert!(matches!(
                probe.admit(true), Admission::Waiting { reason: "provider_quota_limited", until: Some(at) }
                    if at == since + Duration::from_secs(delay)
            ));
            tokio::time::advance(Duration::from_secs(delay)).await;
            assert!(matches!(probe.admit(true), Admission::Ready));
        }
        drop(probe);
        let mut replacement = Attempt::new(holds.clone(), key);
        assert!(matches!(replacement.admit(true), Admission::Waiting { .. }));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(replacement.admit(true), Admission::Ready));
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if self.probe {
            let mut states = self.holds.states.lock().unwrap();
            if let Some(hold) = states.get_mut(&self.key) {
                hold.probe = false;
                hold.until = hold.until.max(Instant::now() + Duration::from_secs(1));
            }
            self.holds.changed.notify_waiters();
        }
    }
}

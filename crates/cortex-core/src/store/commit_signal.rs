//! Process-local wake hints, not a log of commits.
use super::ProjectKey;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use tokio::sync::Notify;

static SIGNALS: Mutex<BTreeMap<ProjectKey, Weak<CommitSignal>>> = Mutex::new(BTreeMap::new());

fn signals() -> MutexGuard<'static, BTreeMap<ProjectKey, Weak<CommitSignal>>> {
    SIGNALS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wake hint for a single logical project's store in this process. Keep the
/// returned handle for the lifetime of the poller, with one waiter per key.
pub struct CommitSignal {
    key: ProjectKey,
    notify: Notify,
}

impl CommitSignal {
    /// Wait for a commit hint. Any number of commits while this poller is busy
    /// leave at most one pending permit. Always read from the durable cursor:
    /// a wake isn't a count, and commits in other processes can't signal here.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

impl Drop for CommitSignal {
    fn drop(&mut self) {
        let mut signals = signals();
        // A concurrent subscriber may already have replaced our expired Weak.
        if signals
            .get(&self.key)
            .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self))
        {
            signals.remove(&self.key);
        }
    }
}

impl ProjectKey {
    /// Subscribe without opening storage. Subscribe before the initial poll;
    /// commits before registration need not have left a permit.
    pub fn commit_signal(&self) -> Arc<CommitSignal> {
        let mut signals = signals();
        if let Some(signal) = signals.get(self).and_then(Weak::upgrade) {
            return signal;
        }
        let signal = Arc::new(CommitSignal {
            key: self.clone(),
            notify: Notify::new(),
        });
        signals.insert(self.clone(), Arc::downgrade(&signal));
        signal
    }

    pub(super) fn notify_committed(&self) {
        let signal = signals().get(self).and_then(Weak::upgrade);
        // Release the registry mutex before waking (or dropping the last Arc).
        if let Some(signal) = signal {
            signal.notify.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoreChannel;

    #[test]
    fn commit_signal_registry_retires_unused_keys() {
        let key = ProjectKey::new(env!("CARGO_MANIFEST_DIR"), StoreChannel::Dev).unwrap();
        key.notify_committed();
        assert!(!signals().contains_key(&key));

        let first = key.commit_signal();
        let second = key.commit_signal();
        assert!(Arc::ptr_eq(&first, &second));
        drop(first);
        assert!(signals().contains_key(&key));
        drop(second);
        assert!(!signals().contains_key(&key));
    }
}

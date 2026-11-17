//! Native bounded folder search. Cancellation stops traversal cooperatively. A thread
//! blocked by a dead mount retains one of two permits, never the async reactor.
use crate::local::failure;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::{oneshot, Mutex, Semaphore};
use workspace_protocol::directories::{Request, Search};

/// The native walk stops at two seconds; a stuck syscall cannot hold the reply indefinitely.
const HARD_DEADLINE: Duration = Duration::from_secs(4);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, LaunchSpec, Service};
    use std::sync::Arc;
    async fn fixture() -> (PathBuf, Arc<Service>) {
        let root = std::env::temp_dir().join(format!(
            "cw-search-queue-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let service = Service::bind(Config {
            runtime_dir: root.join("runtime"),
            state_dir: root.join("state"),
            allowed_root: root.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 2,
        })
        .await
        .unwrap();
        (root, service)
    }
    fn request(query: &str) -> Request {
        Request::Search(serde_json::from_value(json!({"session":"picker","query":query})).unwrap())
    }
    async fn running(service: &Service) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if service.searches.running.lock().await.contains_key("picker") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn queued_search_can_be_cancelled_and_replaced_without_spawning() {
        let (root, s) = fixture().await;
        let slots = s.searches.slots.acquire_many(2).await.unwrap();
        let first = {
            let s = s.clone();
            tokio::spawn(async move { s.directories(request("ce")).await.unwrap() })
        };
        running(&s).await;
        let second = {
            let s = s.clone();
            tokio::spawn(async move { s.directories(request("ced")).await.unwrap() })
        };
        let replaced = first.await.unwrap();
        assert_eq!(replaced["cancelled"], true);
        assert_eq!(replaced["query"], "ce");
        running(&s).await;
        let cancel = Request::Cancel(serde_json::from_value(json!({"session":"picker"})).unwrap());
        assert_eq!(s.directories(cancel).await.unwrap()["outcome"], "cancelled");
        assert_eq!(second.await.unwrap()["cancelled"], true);
        drop(slots);
        s.stop().await.unwrap();
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn dropping_a_waiting_search_does_not_hold_shutdown() {
        let (root, s) = fixture().await;
        let slots = s.searches.slots.acquire_many(2).await.unwrap();
        let task = {
            let s = s.clone();
            tokio::spawn(async move { s.directories(request("cedar")).await })
        };
        running(&s).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), s.stop())
            .await
            .unwrap()
            .unwrap();
        drop(slots);
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
}
/// Where a machine keeps data outside the owner's home, searched as well when they exist.
const MOUNT_ROOTS: &[&str] = &["/mnt", "/media", "/srv"];

pub struct Searches {
    slots: std::sync::Arc<Semaphore>,
    running: Mutex<BTreeMap<String, (u64, oneshot::Sender<()>)>>,
    next: AtomicU64,
}
impl Default for Searches {
    fn default() -> Self {
        Self {
            slots: std::sync::Arc::new(Semaphore::new(2)),
            running: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
        }
    }
}
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}
fn empty(search: &Search, home: &Path, complete: bool) -> Value {
    json!({"query":search.query,"home":home,"entries":[],"complete":complete})
}

impl crate::Service {
    pub async fn directories(&self, request: Request) -> io::Result<Value> {
        match request {
            Request::Search(search) => self.search_directories(search).await,
            Request::Cancel(cancel) => {
                let stopped = self
                    .searches
                    .running
                    .lock()
                    .await
                    .remove(&cancel.session)
                    .is_some();
                Ok(json!({"outcome":if stopped {"cancelled"} else {"not_running"}}))
            }
        }
    }
    /// Roots for a fragment: the home, whatever disks are mounted, and wherever projects already live.
    async fn search_roots(&self, home: &Path) -> Vec<PathBuf> {
        let mut roots = vec![home.to_path_buf()];
        let state = self.state.lock().await;
        let beside = state
            .projects
            .iter()
            .filter_map(|p| p.canonical_root.parent().map(Path::to_path_buf));
        roots.extend(beside);
        drop(state);
        roots.extend(MOUNT_ROOTS.iter().map(PathBuf::from));
        roots.push(self.config.allowed_root.clone());
        let allowed = &self.config.allowed_root;
        let mut kept: Vec<PathBuf> = Vec::new();
        for root in roots {
            if root.starts_with(allowed)
                && root != Path::new("/")
                && !kept.iter().any(|k| root.starts_with(k))
            {
                kept.retain(|k| !k.starts_with(&root));
                kept.push(root);
            }
        }
        kept
    }
    async fn search_directories(&self, search: Search) -> io::Result<Value> {
        self.accepting()?;
        let home = home();
        let (stop, mut stopped) = oneshot::channel();
        let turn = self.searches.next.fetch_add(1, Ordering::Relaxed);
        // Dropping the sender is what tells the search it was replaced.
        self.searches
            .running
            .lock()
            .await
            .insert(search.session.clone(), (turn, stop));
        if search.query.trim().is_empty() {
            self.finish_search(&search.session, turn).await;
            return Ok(empty(&search, &home, true));
        }
        let result = tokio::select! {
            result = self.run_search(&search, &home) => result,
            _ = &mut stopped => Ok(json!({"query":search.query,"home":home,"entries":[],"complete":false,"cancelled":true})),
        };
        self.finish_search(&search.session, turn).await;
        result
    }
    async fn finish_search(&self, session: &str, turn: u64) {
        let mut running = self.searches.running.lock().await;
        if running.get(session).is_some_and(|(mine, _)| *mine == turn) {
            running.remove(session);
        }
    }
    async fn run_search(&self, search: &Search, home: &Path) -> io::Result<Value> {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        struct Cancel(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let _cancel = Cancel(cancel.clone());
        let native = crate::directory_search::Search::new(
            search.query.clone(),
            home.to_path_buf(),
            self.config.allowed_root.clone(),
            self.search_roots(home).await,
            vec![
                self.config.state_dir.clone(),
                self.endpoint
                    .path
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_path_buf(),
            ],
            search.limit.unwrap_or(30) as usize,
            cancel,
        );
        let deadline = tokio::time::Instant::now() + HARD_DEADLINE;
        let slot = tokio::time::timeout_at(deadline, self.searches.slots.clone().acquire_owned())
            .await
            .map_err(|_| failure("directory_search_timeout"))?
            .map_err(|_| failure("service_stopping"))?;
        let (send, receive) = oneshot::channel();
        std::thread::Builder::new()
            .name("directory-search".into())
            .spawn(move || {
                let _slot = slot;
                let _ = send.send(native.run());
            })?;
        let mut reply = match tokio::time::timeout_at(deadline, receive).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => return Err(failure("directory_search_failed")),
            Err(_) => json!({"entries":[],"complete":false,"stopped":"deadline"}),
        };
        reply["query"] = json!(search.query);
        reply["home"] = json!(home);
        Ok(reply)
    }
}

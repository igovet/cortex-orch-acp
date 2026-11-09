//! Folder search on this machine, for choosing a project. One bounded helper run per search, and a
//! newer search from the same picker replaces the one it supersedes. The search runs in its own
//! process so a read stuck on a dead mount is killed with it and can never hold the service.
use crate::local::failure;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{oneshot, Mutex, Semaphore},
};
use workspace_protocol::directories::{Request, Search};

/// The helper stops itself at two seconds and sends what it has. This is for one that cannot.
const HARD_DEADLINE: Duration = Duration::from_secs(4);
const REPLY_BYTES: u64 = workspace_protocol::FRAME_BYTES as u64 - 1024;
/// Where a machine keeps data outside the owner's home, searched as well when they exist.
const MOUNT_ROOTS: &[&str] = &["/mnt", "/media", "/srv"];

pub struct Searches {
    slots: Semaphore,
    running: Mutex<BTreeMap<String, (u64, oneshot::Sender<()>)>>,
    next: AtomicU64,
}
impl Default for Searches {
    fn default() -> Self {
        Self {
            slots: Semaphore::new(2),
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
                let stopped = self.searches.running.lock().await.remove(&cancel.session).is_some();
                Ok(json!({"outcome":if stopped {"cancelled"} else {"not_running"}}))
            }
        }
    }
    /// Roots for a fragment: the home, whatever disks are mounted, and wherever projects already live.
    async fn search_roots(&self, home: &Path) -> Vec<PathBuf> {
        let mut roots = vec![home.to_path_buf()];
        let state = self.state.lock().await;
        let beside = state.projects.iter().filter_map(|p| p.canonical_root.parent().map(Path::to_path_buf));
        roots.extend(beside);
        drop(state);
        roots.extend(MOUNT_ROOTS.iter().map(PathBuf::from));
        roots.push(self.config.allowed_root.clone());
        let allowed = &self.config.allowed_root;
        let mut kept: Vec<PathBuf> = Vec::new();
        for root in roots {
            if root.starts_with(allowed) && root != Path::new("/") && !kept.iter().any(|k| root.starts_with(k)) {
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
        self.searches.running.lock().await.insert(search.session.clone(), (turn, stop));
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
        let (python, tools) = self
            .config
            .launch
            .tools()
            .ok_or_else(|| failure("directory_search_failed"))?;
        let request = serde_json::to_vec(&json!({
            "query":search.query,"limit":search.limit,"home":home,
            "allowedRoot":self.config.allowed_root,"roots":self.search_roots(home).await,
            "excluded":[&self.config.state_dir,self.endpoint.path.parent().unwrap_or(Path::new("/"))],
        }))
        .map_err(|_| failure("invalid_json"))?;
        let deadline = tokio::time::Instant::now() + HARD_DEADLINE;
        let _slot = tokio::time::timeout_at(deadline, self.searches.slots.acquire())
            .await
            .map_err(|_| failure("directory_search_timeout"))?
            .map_err(|_| failure("service_stopping"))?;
        let mut command = Command::new(python);
        command
            .args(["-I", "-B"])
            .arg(tools.with_file_name("directory_search.py"))
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        crate::child::runtime_environment(&mut command, &self.config)?;
        let mut process = command.spawn()?;
        let mut input = process.stdin.take().ok_or_else(|| failure("directory_search_failed"))?;
        let output = process.stdout.take().ok_or_else(|| failure("directory_search_failed"))?;
        // One line is the whole answer. A process held in the kernel may never reach end of file.
        let read = async move {
            input.write_all(&request).await?;
            input.shutdown().await?;
            drop(input);
            let mut line = String::new();
            BufReader::new(output.take(REPLY_BYTES)).read_line(&mut line).await?;
            io::Result::Ok(line)
        };
        let line = tokio::time::timeout_at(deadline, read)
            .await
            .map_err(|_| failure("directory_search_timeout"))??;
        let mut reply: Value = serde_json::from_str(&line).map_err(|_| failure("directory_search_failed"))?;
        if reply.get("error").is_some() || !reply["entries"].is_array() {
            return Err(failure("directory_search_failed"));
        }
        reply["query"] = json!(search.query);
        reply["home"] = json!(home);
        Ok(reply)
    }
}

//! Integration (merge-panel) facts of tasks in Ready to merge for presentation reads,
//! off those reads' critical path. Inspecting where each task's recorded files stand
//! runs Git per file (`task_facts_git::read`) and took seconds per task, so every product
//! read of a project with tasks waiting to merge waited for it, including the read of a
//! task in review that shows none of it. One inspection of a folder runs at a time, in
//! the background; a deferring read (`defer`) answers at once with the facts of an
//! unchanged folder, or with the last answer marked `refreshing`, or with a `pending`
//! marker, and the read that follows waits for the inspection. Committing and merging
//! never use these: they inspect fresh (`integration`).
use super::{git::Git, native::*};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

/// Why a task's merge facts are not shown yet.
pub(crate) const CHECKING_FILES: &str =
    "Checking this task\u{2019}s files against its target branch\u{2026}";
/// How long a settling read waits for the inspection in progress before it inspects itself.
const SETTLE_WAIT: Duration = Duration::from_secs(25);
/// How many tasks' files one background inspection checks at once.
const TASKS_AT_ONCE: usize = 3;

/// One inspection's answers: the change signal taken before it, when it started, and
/// each task's facts with the record they were computed from.
struct Landed {
    signal: Option<String>,
    at: Instant,
    tasks: HashMap<String, (String, Value)>,
}
type Done = tokio::sync::watch::Receiver<Option<Arc<Landed>>>;
#[derive(Default)]
struct Slot {
    last: Option<Arc<Landed>>,
    running: Option<Done>,
}
static SLOTS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<Slot>>>>> = LazyLock::new(Default::default);

fn unreadable() -> Refused {
    refuse("state_unreadable", "Saved tasks are unreadable.")
}
/// Everything the facts depend on, cheaply: the folder (`folder_signal`), every task
/// checkout outside it, and every ref with its upstream. None (never reused) when any
/// part cannot be taken.
fn signal(root: &Path, checkouts: &[PathBuf]) -> Option<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for folder in std::iter::once(root).chain(checkouts.iter().map(PathBuf::as_path)) {
        h.update(super::task_facts::folder_signal(folder)?.as_bytes());
    }
    h.update(super::task_facts::signal_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)",
        ],
    )?);
    Some(format!("{:x}", h.finalize()))
}
fn checkouts(tasks: &[(String, Value)]) -> Vec<PathBuf> {
    tasks
        .iter()
        .filter(|(_, r)| r["checkout"]["kind"].as_str().unwrap_or("main") != "main")
        .filter_map(|(_, r)| r["checkout"]["path"].as_str().map(PathBuf::from))
        .collect()
}
fn pending(record: &Value) -> Value {
    json!({"targetBranch":record["targetBranch"].as_str().unwrap_or(""),"alreadyInTarget":false,
        "pushStatus":null,"pending":true,"reason":CHECKING_FILES})
}
fn fits(landed: &Landed, task: &str, record: &Value) -> Option<Value> {
    landed
        .tasks
        .get(task)
        .filter(|(seen, _)| *seen == record.to_string())
        .map(|(_, facts)| facts.clone())
}
/// The inspection in progress of this folder, or a new one of `tasks`.
fn running(
    root: &Path,
    slot: &Arc<Mutex<Slot>>,
    tasks: &[(String, Value)],
    git: &Git,
) -> Result<Done> {
    let mut held = slot.lock().map_err(|_| unreadable())?;
    if let Some(done) = &held.running {
        return Ok(done.clone());
    }
    let (send, done) = tokio::sync::watch::channel(None);
    held.running = Some(done.clone());
    let (root, owner, tasks, git) = (
        root.to_path_buf(),
        slot.clone(),
        tasks.to_vec(),
        git.clone(),
    );
    let started = std::thread::Builder::new()
        .name("integration-facts".into())
        .spawn(move || {
            let at = Instant::now();
            #[cfg(test)]
            if let Some(wait) = SLOW.lock().ok().and_then(|s| s.get(&root).copied()) {
                std::thread::sleep(wait);
            }
            let before = signal(&root, &checkouts(&tasks));
            let mut answers = HashMap::new();
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                // A few tasks are inspected at once (each already checks its files a few at a time).
                let gate = Arc::new(tokio::sync::Semaphore::new(TASKS_AT_ONCE));
                let mut inspections = tokio::task::JoinSet::new();
                for (task, record) in tasks {
                    let (gate, root, mut git) = (gate.clone(), root.clone(), git.clone());
                    inspections.spawn_on(
                        async move {
                            let _slot = gate.acquire_owned().await;
                            let facts =
                                super::task_facts_git::read(&root, &record, &task, &mut git).await;
                            (task, record, facts)
                        },
                        runtime.handle(),
                    );
                }
                runtime.block_on(async {
                    while let Some(done) = inspections.join_next().await {
                        // A task whose inspection fails is left out: a settling read inspects it itself.
                        if let Ok((task, record, Ok(facts))) = done {
                            answers.insert(task, (record.to_string(), facts));
                        }
                    }
                });
            }
            let landed = Arc::new(Landed {
                signal: before,
                at,
                tasks: answers,
            });
            if let Ok(mut held) = owner.lock() {
                held.last = Some(landed.clone());
                held.running = None;
            }
            let _ = send.send(Some(landed));
        });
    if started.is_err() {
        held.running = None;
    }
    Ok(done)
}
async fn landed(mut done: Done) -> Option<Arc<Landed>> {
    tokio::time::timeout(SETTLE_WAIT, async move {
        loop {
            if let Some(landed) = done.borrow().clone() {
                return Some(landed);
            }
            if done.changed().await.is_err() {
                return done.borrow().clone();
            }
        }
    })
    .await
    .ok()
    .flatten()
}
/// `integrationFacts` of every task in Ready to merge, for a product read. A settling
/// read (`defer` false) returns exactly what an inspection of the current folder
/// says, as before; a deferring one never waits for an inspection.
pub(crate) async fn apply(root: &Path, product: &mut Value, git: &Git, defer: bool) -> Result<()> {
    let tasks = product["tasks"]
        .as_object()
        .ok_or_else(unreadable)?
        .iter()
        .filter(|(_, v)| v["status"] == "ready_to_merge")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(());
    }
    let began = Instant::now();
    let probe = (root.to_path_buf(), checkouts(&tasks));
    let now = tokio::task::spawn_blocking(move || signal(&probe.0, &probe.1))
        .await
        .ok()
        .flatten();
    let slot = SLOTS
        .lock()
        .map_err(|_| unreadable())?
        .entry(root.to_path_buf())
        .or_default()
        .clone();
    let last = slot.lock().map_err(|_| unreadable())?.last.clone();
    // An answer for the same folder, refs and record, younger than the reuse window.
    let current = last
        .as_ref()
        .filter(|l| now.is_some() && l.signal == now && l.at.elapsed() < super::task_facts::RECENT);
    let mut missing = Vec::new();
    for (task, record) in &tasks {
        match current.and_then(|l| fits(l, task, record)) {
            Some(facts) => product["tasks"][task]["integrationFacts"] = facts,
            None => missing.push((task.clone(), record.clone())),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let done = running(root, &slot, &tasks, git)?;
    if defer {
        for (task, record) in &missing {
            // The last answer while it is recent, marked as being checked again; else pending.
            let earlier = last
                .as_ref()
                .filter(|l| l.at.elapsed() < super::task_facts::RECENT * 2)
                .and_then(|l| fits(l, task, record))
                .map(|mut facts| {
                    facts["refreshing"] = json!(true);
                    facts
                });
            product["tasks"][task]["integrationFacts"] = earlier.unwrap_or_else(|| pending(record));
        }
        return Ok(());
    }
    // Only an inspection begun by this read, or one of the folder as it is now, answers it.
    let landed = landed(done)
        .await
        .filter(|l| l.at >= began || (now.is_some() && l.signal == now));
    let mut git = git.clone();
    for (task, record) in missing {
        product["tasks"][&task]["integrationFacts"] =
            match landed.as_deref().and_then(|l| fits(l, &task, &record)) {
                Some(facts) => facts,
                None => super::task_facts_git::read(root, &record, &task, &mut git).await?,
            };
    }
    Ok(())
}
/// A test's slow inspection of one folder: the background inspection waits this long first.
#[cfg(test)]
static SLOW: LazyLock<Mutex<HashMap<PathBuf, Duration>>> = LazyLock::new(Default::default);
#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success());
    }
    /// A project with a task waiting to merge: the screen's first read does not wait for the
    /// slow inspection of that task's files (its facts say pending), the next read waits for
    /// that same inspection and has exactly what a direct inspection says, and a read of the
    /// unchanged folder after it reuses it without waiting.
    #[tokio::test]
    async fn a_deferring_read_never_waits_for_the_ready_to_merge_inspection() {
        let root =
            std::env::temp_dir().join(format!("cw-presented-{}", super::super::settings::unique()));
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "base"]);
        // A folder written moments ago has no reusable signal; let it settle.
        std::thread::sleep(Duration::from_millis(3100));
        SLOW.lock()
            .unwrap()
            .insert(root.clone(), Duration::from_secs(2));
        let record =
            json!({"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"});
        let product = json!({"tasks":{"wt_merge":record,"wt_review":{"status":"review"}}});
        let direct =
            super::super::task_facts_git::read(&root, &record, "wt_merge", &mut Git::new())
                .await
                .unwrap();
        let began = Instant::now();
        let mut first = product.clone();
        apply(&root, &mut first, &Git::new(), true).await.unwrap();
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(
            first["tasks"]["wt_merge"]["integrationFacts"]["pending"], true,
            "{first}"
        );
        assert_eq!(
            first["tasks"]["wt_merge"]["integrationFacts"]["reason"],
            CHECKING_FILES
        );
        assert!(first["tasks"]["wt_review"]["integrationFacts"].is_null());
        let mut settled = product.clone();
        apply(&root, &mut settled, &Git::new(), false)
            .await
            .unwrap();
        assert!(began.elapsed() >= Duration::from_secs(2));
        assert_eq!(settled["tasks"]["wt_merge"]["integrationFacts"], direct);
        SLOW.lock().unwrap().remove(&root);
        let again = Instant::now();
        let mut reused = product.clone();
        apply(&root, &mut reused, &Git::new(), true).await.unwrap();
        assert!(again.elapsed() < Duration::from_secs(1));
        assert_eq!(reused["tasks"]["wt_merge"]["integrationFacts"], direct);
        // A new commit moves the signal: a deferring read shows the last answer as being
        // checked again, and a settling read inspects the folder as it is now.
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "next"]);
        std::thread::sleep(Duration::from_millis(3100));
        let mut moved = product.clone();
        apply(&root, &mut moved, &Git::new(), true).await.unwrap();
        assert_eq!(
            moved["tasks"]["wt_merge"]["integrationFacts"]["refreshing"], true,
            "{moved}"
        );
        let mut now = product.clone();
        apply(&root, &mut now, &Git::new(), false).await.unwrap();
        assert!(now["tasks"]["wt_merge"]["integrationFacts"]["refreshing"].is_null());
        if let Ok(mut slots) = SLOTS.lock() {
            slots.remove(&root);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}

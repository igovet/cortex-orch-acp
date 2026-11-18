//! Opt-in timing of the owner facts reads (`product.facts`, `workers`) on a disposable store copy.
//! Ignored by default and inert without `PERF_ROOT`.
//!
//! `PERF_ROOT` must hold the marker `.cortex-perf-copy`, which no real project has. The usual
//! way to run it is inside a private mount namespace in which the project path is an overlay
//! (the real folder as its read-only lower layer, a private upper layer holding the copied
//! `.cortex-dev` store and the marker) with an empty directory bound over its release `.cortex`,
//! so neither the real store nor the real folder can be written. Every engine route is
//! unqualified, so nothing can start. Output (stderr) is timings, counts and sizes only.
//!
//! Environment: `PERF_ROOT`, `PERF_STATE` (a private state directory holding a copied
//! `product/<projectId>.json`), `PERF_PROJECT`, optional `PERF_TASK` (one task's `workers`
//! read) and `PERF_ROUNDS` (default 3).
use crate::native_runtime::{Runtime, UnqualifiedRoutes};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Instant};

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in: needs PERF_* and a disposable store copy"]
async fn owner_facts_timings_on_a_store_copy() {
    let Some(root) = var("PERF_ROOT").map(PathBuf::from) else {
        return;
    };
    assert!(
        root.join(".cortex-perf-copy").is_file(),
        "PERF_ROOT is not a marked disposable copy; refusing to touch it"
    );
    let state = PathBuf::from(var("PERF_STATE").expect("PERF_STATE"));
    let project = var("PERF_PROJECT").expect("PERF_PROJECT");
    let rounds: usize = var("PERF_ROUNDS").and_then(|r| r.parse().ok()).unwrap_or(3);
    // A copied store has a new directory identity: bind the copy's own dev marker, exactly as
    // `project_guard::reserve` would for a fresh store.
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let marker = root.join(".cortex-dev/owner.json");
        let m = std::fs::metadata(root.join(".cortex-dev")).unwrap();
        let _ = std::fs::remove_file(&marker);
        let body = json!({"channel":"dev","root":root,"uid":unsafe { libc::getuid() },"store":[m.dev(),m.ino()]});
        std::io::Write::write_all(
            &mut std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)
                .unwrap(),
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();
    }
    crate::project_guard::check(&root).unwrap();
    let opened = Instant::now();
    let path = root.clone();
    let id = project.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || Runtime::open(&path, &id, Arc::new(UnqualifiedRoutes)))
            .await
            .unwrap()
            .unwrap(),
    );
    eprintln!("perf open_ms={}", opened.elapsed().as_millis());
    let started: Vec<String> = {
        let store = crate::machine::context::open(&root, cortex_core::OpenMode::LiveReadOnly)
            .unwrap()
            .unwrap();
        store
            .read(|db| {
                Ok(db
                    .prepare("SELECT task_id FROM workspace_tasks WHERE execution_id IS NOT NULL ORDER BY sequence")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap()
    };
    let task = var("PERF_TASK");
    let mut reads = vec![("product.facts", json!({"taskIds":started}))];
    if let Some(task) = &task {
        reads.push(("workers", json!({"taskIds":[task]})));
    }
    for round in 0..rounds {
        for (op, params) in &reads {
            let t = Instant::now();
            let saved = crate::machine::task_facts::native_saved(
                &root,
                &state,
                &project,
                &mut crate::machine::git::Git::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("native_saved: {}", e.code));
            let saved_ms = t.elapsed().as_millis();
            let f = Instant::now();
            let facts = runtime
                .owner_facts(&project, saved, params.clone())
                .await
                .unwrap();
            let facts_ms = f.elapsed().as_millis();
            let reply = if *op == "workers" {
                facts["workers"].clone()
            } else {
                facts.clone()
            };
            let workers = facts["workers"].as_array().map_or(0, Vec::len);
            let activity: usize = facts["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| w["activity"].as_array().map_or(0, Vec::len))
                .sum();
            eprintln!(
                "perf round={round} op={op} saved_ms={saved_ms} facts_ms={facts_ms} total_ms={} workers={workers} activity={activity} bytes={} partial={}",
                t.elapsed().as_millis(),
                serde_json::to_vec(&reply).unwrap().len(),
                facts["partial"]
            );
        }
    }
    // The service's own `product.facts` read (machine.rs): the folder observation starts
    // first and runs while the saved records and the store are read; rounds after the
    // first reuse it while the folder is unchanged (`recent_observation`).
    // BEGIN owner_workers
    for round in 0..rounds {
        let t = Instant::now();
        let observing = tokio::spawn(crate::machine::task_facts::recent_observation(root.clone()));
        let saved = crate::machine::task_facts::native_saved(
            &root,
            &state,
            &project,
            &mut crate::machine::git::Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("native_saved: {}", e.code));
        let facts = runtime
            .owner_facts_observed(&project, saved, reads[0].1.clone(), observing)
            .await
            .unwrap();
        eprintln!(
            "perf round={round} op=product.facts(service) total_ms={} workers={} bytes={}",
            t.elapsed().as_millis(),
            facts["workers"].as_array().map_or(0, Vec::len),
            serde_json::to_vec(&facts).unwrap().len()
        );
    }
    // END owner_workers
    // The service's own `workers` read (machine.rs): the binding check, then the worker rows alone.
    // BEGIN owner_workers
    if let Some(task) = &task {
        for round in 0..rounds {
            let t = Instant::now();
            crate::machine::task_facts::materialized(&root, &project)
                .unwrap_or_else(|e| panic!("materialized: {}", e.code));
            let workers = runtime
                .owner_workers(&project, json!({"taskIds":[task]}))
                .await
                .unwrap();
            eprintln!(
                "perf round={round} op=workers(service) total_ms={} workers={} bytes={}",
                t.elapsed().as_millis(),
                workers.as_array().map_or(0, Vec::len),
                serde_json::to_vec(&workers).unwrap().len()
            );
        }
    }
    // END owner_workers
    // BEGIN owner_workers
    for _ in 0..rounds {
        let t = Instant::now();
        let digest = crate::native_effects::observe_digest(root.clone()).await;
        eprintln!(
            "perf observe_digest_ms={} ok={}",
            t.elapsed().as_millis(),
            digest.is_ok()
        );
    }
    // END owner_workers
    let t = Instant::now();
    let observed = crate::native_effects::observe(root.clone())
        .await
        .map(|e| e.project_digest.is_some());
    eprintln!(
        "perf observe_ms={} ok={:?}",
        t.elapsed().as_millis(),
        observed.ok()
    );
    if let Some(out) = var("PERF_OUT") {
        let saved = crate::machine::task_facts::native_saved(
            &root,
            &state,
            &project,
            &mut crate::machine::git::Git::new(),
        )
        .await
        .unwrap();
        let mut all = Vec::<Value>::new();
        for (_, params) in &reads {
            all.push(
                runtime
                    .owner_facts(&project, saved.clone(), params.clone())
                    .await
                    .unwrap(),
            );
        }
        // BEGIN owner_workers
        if let Some(task) = &task {
            all.push(
                runtime
                    .owner_workers(&project, json!({"taskIds":[task]}))
                    .await
                    .unwrap(),
            );
        }
        let observing = tokio::spawn(crate::machine::task_facts::recent_observation(root.clone()));
        let saved = crate::machine::task_facts::native_saved(
            &root,
            &state,
            &project,
            &mut crate::machine::git::Git::new(),
        )
        .await
        .unwrap();
        all.push(
            runtime
                .owner_facts_observed(&project, saved, reads[0].1.clone(), observing)
                .await
                .unwrap(),
        );
        // END owner_workers
        std::fs::write(out, serde_json::to_vec(&all).unwrap()).unwrap();
    }
    runtime.shutdown().await;
}

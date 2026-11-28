use super::*;
// A writing worker's catalogue discovery without arguments (as Codex workers call
// it) answers with the catalogue's first page, never an authority refusal.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_discovers_its_tool_catalogue_without_arguments() {
    let root = std::env::temp_dir().join(format!("discover-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let outcome = runtime.host.executor.publication(|store| {
        let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "g", "owner")?;
        let c = store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-devops-engineer','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
            let c = cortex_core::channels::issue_tx(db, "t", &owner, cortex_core::channels::Principal::Worker { assignment: "a" })?;
            cortex_core::channels::bind_session_tx(db, &c, "engine")?;
            Ok(c)
        })?;
        let i = store.read(|db| cortex_core::channels::authenticate(db, &c.channel_id, c.secret(), true))?;
        let mut out = Vec::new();
        for args in [json!({}), json!({"limit":20}), json!({"limit":50})] {
            out.push(crate::native_documents::execute(store, &c, &i, "tools_discover", &args).map(|v| v["tools"].as_array().map(Vec::len)).map_err(|e| e.code()));
        }
        Ok(out)
    }).unwrap().await.unwrap().unwrap();
    for o in &outcome {
        assert!(matches!(o, Ok(Some(n)) if *n > 0), "{outcome:?}");
    }
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

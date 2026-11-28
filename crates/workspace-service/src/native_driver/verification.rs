//! Bound, supervised verification and retry of environmental failures.
use super::*;
use crate::native_cargo::Mode as CargoMode;
use cortex_core::{lifecycle::plan, ports::CommandObservation};

impl NativeHost {
    /// Runs the plan's verification commands once on the reviewed binding and records
    /// the outcome. An environmental outcome (`completion::verifier_environment`) is
    /// rerun once by the runtime itself (`VERIFICATION_RERUN`, no model call); if it
    /// fails the same way again, the verification becomes the runtime's own wait
    /// (`VERIFICATION_ENVIRONMENT_WAIT`, no replan and no ladder rung) and is run again
    /// after the next backoff step (1, 2, 5, 10 minutes), after which the runtime asks the
    /// owner one question. A real failure stays a failure; nothing
    /// unverified is recorded as passed.
    pub(super) async fn verify_plan(&self, task: &str, owner: &OwnerEpoch) -> CoreResult<Verified> {
        // A verification waiting for its environment is not run, nor the project
        // observed, before its recheck is due.
        let t = task.to_owned();
        let wait = recv(
            self.executor
                .read(move |db| completion::verification_wait_tx(db, &t))?,
        )
        .await?;
        let now = unix_now()?;
        let rechecking = match &wait {
            Some(w) => {
                let due = w["retry_not_before"].as_f64().unwrap_or(0.0);
                if due > now {
                    return Ok(Verified::Waiting(std::time::Duration::from_secs_f64(
                        due - now,
                    )));
                }
                true
            }
            None => false,
        };
        let started = std::time::Instant::now();
        let result = self.run_verification(task, owner, rechecking).await;
        let outcome = match &result {
            Ok(Verified::Waiting(_) | Verified::Replanned) => cortex_core::observability::Outcome::Waiting,
            _ => crate::native_timing::outcome(&result),
        };
        crate::native_timing::record(
            &self.executor,
            task,
            cortex_core::observability::Scope::Task,
            cortex_core::observability::Category::VerificationRun,
            outcome,
            started.elapsed(),
        ).await;
        result
    }

    async fn run_verification(&self, task: &str, owner: &OwnerEpoch, rechecking: bool) -> CoreResult<Verified> {
        let started = std::time::Instant::now();
        // One observation anchors the review preflight, the recorded intent
        // digest, every post-command drift check and the final record.
        // Verification compares what the approving review bound: the task's own files
        // (identified in the observation) or the whole checkout.
        let checkout = self.execution_checkout(task, owner).await?;
        let root = checkout.path().to_owned();
        let mut e = native_effects::observe(root.clone()).await?;
        e.checkout_evidence = checkout.evidence(owner.epoch);
        crate::native_change_set::bind_task_files(&self.executor, &root, task, &mut e).await?;
        let e = std::sync::Arc::new(e);
        let digest = e.project_digest.clone();
        let t = task.to_owned();
        let o = owner.clone();
        let intent_digest = digest.clone();
        let preflight = std::sync::Arc::clone(&e);
        let run = format!("verify-{}", native_effects::identity()?);
        let intent=recv(self.executor.write(move|db|{
            ownership::check_tx(db,&t,&o)?;
            let (_,revision)=domain::task_tx(db,&t)?;
            // A run of this revision that started and never finished (a restart or crash
            // mid-run) has an unknown outcome: it is settled as unknown and verification
            // runs again, instead of every later verification of the revision being refused.
            completion::settle_interrupted_verification_tx(db,&t)?;
            match completion::verification_preflight_tx(db,&t,&preflight) {
                Err(e) if e.code()=="verification_review_revision_changed" => {
                    completion::review_changed_checkout_tx(db,&t,&preflight)?;
                    return Ok(None);
                }
                result => result?,
            }
            // A sprint planning task without commands: the runtime verifies its reviewed plan.
            if lifecycle::sprint_planning::uses_runtime_verification(db,&t)? {
                lifecycle::sprint_planning::verify_tx(db,&t)?;
                return Ok(None);
            }
            let p:String=db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?;
            let commands=plan::verification(&serde_json::from_str::<Value>(&p)?["verification"])?;
            let bound=completion::current_binding_digest(db,&t,&preflight)?;
            lifecycle::request_tx(db,&t,"native-verifier",&run,"native_verification_intent",
                &json!({"revision":revision,"digest":intent_digest,"binding_digest":bound,"commands":commands}),||Ok(json!({"revision":revision,"digest":intent_digest,"binding_digest":bound})))?;
            Ok(Some((commands,run,bound,revision)))
        })?).await?;
        let Some((commands, run, bound, intent_revision)) = intent else {
            return Ok(Verified::Recorded);
        };
        let mut keys = Vec::new();
        let mut all_exits_observed = true;
        for (index, argv) in commands.into_iter().enumerate() {
            let mode = if crate::native_cargo::selection(&argv).is_some()
                && !e.incomplete && !e.metadata_only && !e.cap_reached
            {
                CargoMode::Observe
            } else {
                CargoMode::Ordinary
            };
            let cargo_revision = (mode != CargoMode::Ordinary).then_some(intent_revision);
            let key = format!("{run}:{index}");
            let (mut receipt, mut environment, mut stale) = self
                .verification_run(task, owner, &run, &key, &argv, &bound, &digest, mode, cargo_revision)
                .await?;
            let mut key = key;
            let mut receipts = vec![receipt["tool_call_ref"].clone()];
            // The first environmental outcome of a fresh verification is rerun once,
            // exactly, by the runtime; a due recheck of a waiting verification is
            // itself that rerun.
            if let (Some(reason), false) = (environment, rechecking || stale.is_some()) {
                let (t, o, a, first) = (
                    task.to_owned(),
                    owner.clone(),
                    json!(argv),
                    receipt["tool_call_ref"].clone(),
                );
                recv(self.executor.write(move |db| {
                    ownership::check_tx(db, &t, &o)?;
                    completion::record_verification_rerun_tx(db, &t, &a, reason, &first)
                })?)
                .await?;
                key = format!("{run}:{index}:rerun");
                (receipt, environment, stale) = self
                    .verification_run(task, owner, &run, &key, &argv, &bound, &digest, mode, cargo_revision)
                    .await?;
                receipts.push(receipt["tool_call_ref"].clone());
            }
            if let Some(first_build) = stale {
                let current = self.execution_checkout(task, owner).await?;
                if current.path() != root || current.evidence(owner.epoch) != checkout.evidence(owner.epoch) {
                    return Err(CoreError::Refused("worktree_binding_invalid"));
                }
                if native_effects::observe_digest(root.clone()).await.ok().as_ref() != digest.as_ref() {
                    let (t, o, r, last) = (task.to_owned(), owner.clone(), run.clone(), key);
                    recv(self.executor.write(move |db| {
                        ownership::check_tx(db, &t, &o)?;
                        lifecycle::request_tx(db, &t, "native-verifier-finish", &r, "native_verification_finished",
                            &json!({"last_output":last}), || Ok(json!({
                                "settled":true,"last_output":last,"reason":"source_changed_before_cargo_rebuild"
                            })))?;
                        Ok(())
                    })?).await?;
                    // Let the next normal preflight read the active review binding.
                    return Ok(Verified::Waiting(std::time::Duration::from_millis(200)));
                }
                let (t, o, first, r) = (task.to_owned(), owner.clone(), receipt["tool_call_ref"].clone(), run.clone());
                let revision = receipt["revision"].as_i64();
                recv(self.executor.write(move |db| {
                    ownership::check_tx(db, &t, &o)?;
                    let (state, active) = domain::task_tx(db, &t)?;
                    if state != cortex_core::TaskState::Verifying || Some(active) != revision {
                        return Err(CoreError::Refused("verification_intent_missing"));
                    }
                    lifecycle::event_tx(db, &t, "NATIVE_CARGO_ARTIFACT_REBUILD",
                        json!({"stale_ref":first,"built_ref":first_build,"run":r,"revision":active}))?;
                    Ok(())
                })?).await?;
                key = format!("{run}:{index}:cargo-rebuild");
                (receipt, environment, _) = self
                    .verification_run(task, owner, &run, &key, &argv, &bound, &digest, CargoMode::Rebuild, revision)
                    .await?;
                receipts.push(receipt["tool_call_ref"].clone());
            }
            // Drift is a change of what the review bound: with a task-file binding,
            // another actor's edit elsewhere in the checkout is not drift.
            let current = self.execution_checkout(task, owner).await?;
            if current.path() != root { return Err(CoreError::Refused("worktree_binding_invalid")); }
            let mut fresh = native_effects::observe(root.clone()).await?;
            fresh.checkout_evidence = current.evidence(owner.epoch);
            crate::native_change_set::bind_task_files(&self.executor, &root, task, &mut fresh)
                .await?;
            let t = task.to_owned();
            let (fresh, fresh_bound) = recv(self.executor.read(move |db| {
                let d = completion::current_binding_digest(db, &t, &fresh).ok();
                Ok((fresh, d))
            })?)
            .await?;
            all_exits_observed &= receipt["observation"]["complete"] == true
                && receipt["observation"]["exit_code"].is_i64();
            if fresh_bound.as_deref() != Some(bound.as_str()) {
                // The reviewed bytes changed while the command ran: whatever it
                // observed (an unobserved exit is an unknown outcome, never a pass) does
                // not verify them. The run is settled and a fresh review binds the new
                // bytes.
                let outcome = if all_exits_observed {
                    "observed"
                } else {
                    "unknown"
                };
                let (t, o) = (task.to_owned(), owner.clone());
                recv(self.executor.write(move |db| {
                    ownership::check_tx(db,&t,&o)?;
                    lifecycle::request_tx(db,&t,"native-verifier-finish",&run,"native_verification_finished",
                        &json!({"last_output":key}),||Ok(json!({"settled":true,"reason":"verification_review_revision_changed","last_output":key,
                            "outcome":outcome,"settled_by":"runtime_reverify"})))?;
                    completion::review_changed_checkout_tx(db,&t,&fresh)
                })?).await?;
                return Ok(Verified::Recorded);
            }
            if let Some(reason) = environment {
                let (t, o, a) = (task.to_owned(), owner.clone(), json!(argv));
                let wait = recv(self.executor.write(move |db| {
                    ownership::check_tx(db, &t, &o)?;
                    lifecycle::request_tx(db,&t,"native-verifier-finish",&run,"native_verification_finished",
                        &json!({"last_output":key}),||Ok(json!({"settled":true,"reason":"verifier_environment","environment":reason,"last_output":key})))?;
                    completion::record_verification_wait_tx(db, &t, &a, reason, &json!(receipts), unix_now()?)
                })?)
                .await?;
                // After the 10-minute recheck the runtime asked the owner instead
                // (WAITING_USER): nothing more to wait for here.
                if wait["owner_question"].is_string() {
                    return Ok(Verified::Recorded);
                }
                let left = wait["retry_after_s"].as_u64().unwrap_or(60);
                return Ok(Verified::Waiting(std::time::Duration::from_secs(left)));
            }
            let t = task.to_owned();
            let o = owner.clone();
            keys.push(key.clone());
            recv(self.executor.write(move |db| {
                ownership::check_tx(db, &t, &o)?;
                let observed = CommandObservation {
                    complete: receipt["observation"]["complete"] == true,
                    exit_code: receipt["observation"]["exit_code"]
                        .as_i64()
                        .map(|v| v as i32),
                    receipt_ref: receipt["tool_call_ref"].as_str().unwrap().into(),
                };
                completion::retain_check_tx(db, &t, &o, &key, &argv, &observed, &fresh)?;
                Ok(())
            })?)
            .await?;
        }
        let t = task.to_owned();
        let o = owner.clone();
        let checkout_id = checkout.worktree.as_ref().map(|w| w.id.clone());
        let verified = recv(self.executor.publication(move |store| store.write(|db| {
            ownership::check_tx(db, &t, &o)?;
            let ready = require_producer_outputs_tx(store, db, &t, false)?;
            if ready {
                let passed = completion::record_verification_tx(db, &t, &keys, &e)?;
                if let Some(id) = &checkout_id {
                    if passed && !keys.is_empty() {
                        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX).max(1);
                        lifecycle::event_tx(db, &t, "NATIVE_WORKTREE_VERIFICATION_TIME",
                            json!({"checkout_id":id, "run":run, "elapsed_ms":elapsed_ms}))?;
                    }
                }
            }
            lifecycle::request_tx(
                db,
                &t,
                "native-verifier-finish",
                &run,
                "native_verification_finished",
                &json!({"keys":keys}),
                || Ok(json!({"settled":true})),
            )?;
            Ok(ready)
        }))?)
        .await?;
        Ok(if verified { Verified::Recorded } else { Verified::Replanned })
    }
    /// One run of one verification command under the run's intent: the receipt (its
    /// output retained under `key` as the runtime's own diagnostic) and whether its
    /// outcome was environmental (`completion::verifier_environment`), judged against
    /// the task's own worker runs of the same command on the same checkout.
    #[allow(clippy::too_many_arguments)]
    async fn verification_run(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        run: &str,
        key: &str,
        argv: &[String],
        bound: &str,
        digest: &Option<String>,
        mode: CargoMode,
        expected_revision: Option<i64>,
    ) -> CoreResult<(Value, Option<&'static str>, Option<String>)> {
        let mut receipt = match self.owner_verify_mode(task, owner, argv, mode, expected_revision).await {
            Ok(r) => r,
            Err(err) => {
                json!({"tool_call_ref":key,"argv":argv,"profile":"native-readonly-landlock-seccomp",
                "observation":{"complete":false,"failure":err.code(),"exit_code":null}})
            }
        };
        // The owner verifier observes the bytes again immediately after the
        // selected command. An absent or changed source witness cannot bind Cargo.
        if digest.as_ref().is_none_or(|d| receipt["cargo_artifact"]["source_digest"] != *d)
            || receipt["observation"]["complete"] != true
        {
            receipt.as_object_mut().unwrap().remove("cargo_artifact");
        }
        if mode == CargoMode::Rebuild && receipt["observation"]["exit_code"] == 0
            && !receipt["cargo_artifact"].is_object()
        {
            receipt["observation"]["complete"] = json!(false);
            receipt["observation"]["failure"] = json!("cargo_artifact_unobservable");
        }
        if mode != CargoMode::Ordinary {
            receipt["cargo_artifact_status"] = json!(if !receipt["cargo_artifact"].is_object() {
                "unknown"
            } else if receipt["cargo_artifact"]["fresh"] == false {
                "built"
            } else {
                "reused"
            });
        }
        // The run binds what the verification binds; the checkout it ran in is
        // kept beside it.
        receipt["revision_digest"] = json!(bound);
        receipt["checkout_digest"] = json!(digest);
        receipt["owner_epoch"] = json!(owner.epoch);
        receipt["run"] = json!(run);
        let t = task.to_owned();
        let mut raw = receipt.clone();
        let run_key = run.to_owned();
        let output_key = key.to_owned();
        let digests = [bound.to_owned(), digest.clone().unwrap_or_default()];
        let argv = argv.to_vec();
        let (environment, stale, raw) = recv(self.executor.write(move|db|{
            // A command already dispatched under this intent is an observed
            // effect even if later state/epoch/project changes disqualify it.
            let revision: Option<i64>=db.query_row("SELECT json_extract(result_json,'$.revision') FROM runtime_requests WHERE task_id=? AND actor='native-verifier' AND request_key=? AND operation='native_verification_intent'",(&t,&run_key),|r|r.get(0)).optional()?;
            if revision.is_none() {return Err(CoreError::Refused("verification_intent_missing"));}
            if raw["revision"].as_i64() != revision && mode != CargoMode::Ordinary {
                raw.as_object_mut().unwrap().remove("cargo_artifact");
                raw["cargo_artifact_status"] = json!("unknown");
                raw["observation"]["complete"] = json!(false);
                raw["observation"]["failure"] = json!("verification_intent_missing");
            }
            let mut stale = None;
            if mode == CargoMode::Observe && raw["cargo_artifact"]["fresh"] == true {
                let prior = prior_cargo_build(db, &t, &raw)?;
                if let Some((reference, build)) = prior {
                    if crate::native_cargo::stale(&raw["cargo_artifact"], &build) {
                        stale = Some(reference.clone());
                    }
                    raw["cargo_artifact_prior_ref"] = json!(reference);
                    raw["cargo_artifact"]["built_source_digest"] = build["source_digest"].clone();
                } else {
                    raw["cargo_artifact_status"] = json!("unknown");
                }
                // The current source is observed at reuse; only the older
                // compiler receipt can identify what the artifact was built from.
                raw["cargo_artifact"].as_object_mut().unwrap().remove("source_digest");
            }
            if let Some(first_build) = &stale {
                raw["cargo_artifact_status"] = json!("stale");
                raw["cargo_artifact_prior_ref"] = json!(first_build);
                raw["cargo_artifact_observed_exit"] = raw["observation"]["exit_code"].clone();
                raw["observation"]["complete"] = json!(false);
                raw["observation"]["failure"] = json!("cargo_artifact_stale");
            }
            lifecycle::request_tx(db,&t,"native-verifier-output",&output_key,"native_command_output",&raw,||Ok(raw.clone()))?;
            let observation=&raw["observation"];
            let failed=observation["complete"]==true && observation["exit_code"].as_i64().is_some_and(|c|c!=0);
            let worker_passed=mode != CargoMode::Rebuild && failed && completion::worker_passed_tx(db,&t,&argv,&[digests[0].as_str(),digests[1].as_str()],&raw)?;
            Ok((completion::verifier_environment(observation,worker_passed), stale, raw))
        })?).await?;
        Ok((raw, environment, stale))
    }
}

fn prior_cargo_build(db: &rusqlite::Connection, task: &str, current: &Value) -> CoreResult<Option<(String, Value)>> {
    let mut query = db.prepare(
        "SELECT output.result_json FROM (
           SELECT rowid AS receipt_order,result_json FROM runtime_requests WHERE task_id=? AND actor='native-verifier-output'
           AND operation='native_command_output' ORDER BY rowid DESC LIMIT 64
         ) output JOIN runtime_requests intent
           ON intent.task_id=?1 AND intent.actor='native-verifier'
           AND intent.operation='native_verification_intent'
           AND intent.request_key=json_extract(output.result_json,'$.run')
           AND json_extract(intent.result_json,'$.revision')=json_extract(output.result_json,'$.revision')
           AND json_extract(intent.result_json,'$.digest')=json_extract(output.result_json,'$.cargo_artifact.source_digest')
         WHERE json_extract(output.result_json,'$.cargo_artifact.fresh')=0
           AND json_extract(output.result_json,'$.cargo_artifact.private_target')=0
           AND json_extract(output.result_json,'$.cargo_artifact.binary_sha256')=?2
           AND json_extract(output.result_json,'$.owner_epoch')=?3 ORDER BY output.receipt_order DESC",
    )?;
    let mut found: Option<(String, Value)> = None;
    for raw in query.query_map((task, current["cargo_artifact"]["binary_sha256"].as_str(), current["owner_epoch"].as_i64()), |row| row.get::<_, String>(0))? {
        let earlier: Value = serde_json::from_str(&raw?)?;
        if earlier["argv"] == current["argv"] && earlier["cwd"] == current["cwd"]
            && earlier["checkout"] == current["checkout"]
            && earlier["tool_call_ref"].is_string()
            && crate::native_cargo::same_artifact(&current["cargo_artifact"], &earlier["cargo_artifact"])
        {
            let artifact = &earlier["cargo_artifact"];
            if let Some((_, selected)) = &found {
                if selected["source_digest"] != artifact["source_digest"] {
                    return Ok(None);
                }
            } else {
                found = Some((earlier["tool_call_ref"].as_str().unwrap().to_owned(), artifact.clone()));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod cargo_provenance_tests {
    use super::*;

    #[test]
    fn cargo_reuse_requires_a_unique_source_bound_to_a_real_verification_intent() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runtime_requests (
            task_id TEXT, actor TEXT, request_key TEXT, operation TEXT, result_json TEXT
        )").unwrap();
        let binary = "a".repeat(64);
        let old_source = "b".repeat(64);
        let new_source = "c".repeat(64);
        let mut old = json!({"argv":["cargo","test","--lib"],"cwd":"/fixture","owner_epoch":1,
            "run":"run1","revision":2,"tool_call_ref":"built","cargo_artifact":{
                "fresh":false,"private_target":false,"package":"fixture","target":null,
                "binary_sha256":binary,"binary_path":"debug/deps/fixture-cargo",
                "file_identity":"identity","source_digest":old_source
            }});
        let mut current = old.clone();
        current["tool_call_ref"] = json!("current");
        current["cargo_artifact"]["fresh"] = json!(true);
        let insert = |actor: &str, key: &str, operation: &str, result: Value| {
            db.execute("INSERT INTO runtime_requests VALUES('task',?,?,?,?)",
                (actor,key,operation,result.to_string())).unwrap();
        };
        insert("native-verifier-output","out1","native_command_output",old.clone());
        assert!(prior_cargo_build(&db, "task", &current).unwrap().is_none());
        insert("native-verifier","run1","native_verification_intent",json!({"revision":2,"digest":old_source}));
        let prior = prior_cargo_build(&db,"task",&current).unwrap().unwrap();
        assert_eq!(prior.0,"built");
        assert!(!crate::native_cargo::stale(&current["cargo_artifact"], &prior.1));
        current["cargo_artifact"]["source_digest"] = json!(new_source);
        assert!(crate::native_cargo::stale(&current["cargo_artifact"], &prior.1));
        old["run"] = json!("run2");
        old["tool_call_ref"] = json!("different_build");
        old["cargo_artifact"]["source_digest"] = json!(new_source);
        insert("native-verifier-output","out2","native_command_output",old);
        insert("native-verifier","run2","native_verification_intent",json!({"revision":2,"digest":new_source}));
        assert!(prior_cargo_build(&db,"task",&current).unwrap().is_none());
    }
}

/// What one verification step left: an outcome recorded (passed, failed, or a
/// fresh review of changed bytes), or the runtime's own wait for the verifier's
/// environment with the time until its recheck.
pub(super) enum Verified {
    Recorded,
    Replanned,
    Waiting(std::time::Duration),
}

fn unix_now() -> CoreResult<f64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CoreError::Refused("clock_unavailable"))?
        .as_secs_f64())
}


//! Run: cargo test --locked --offline -p workspace-service --features test-observations
//!      --test native_desktop -- native-baseline-probe
//! Set TMPDIR to a private scratch location on the intended device. The optional
//! CORTEX_BASELINE_DEVICE (major:minor) is checked against the actual SQLite file.
use super::{command, namespace_host, read_page, Fixture, Routes};
use cortex_core::{
    test_observations::{Batch, Change, Observation, Observer},
    OpenMode, Store,
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::BTreeMap,
    os::unix::fs::MetadataExt,
    sync::Arc,
    time::{Duration, Instant},
};
use workspace_service::Service;

const RUNS: usize = 5;
const COMMAND_KINDS: &[&str] = &[
    "NATIVE_COMMAND_DIAGNOSTIC",
    "NATIVE_COMMAND_FINISHED",
    "NATIVE_COMMAND_PENDING_REPLY",
    "NATIVE_COMMAND_PREPARATION",
    "NATIVE_COMMAND_PROCESS",
    "NATIVE_COMMAND_RECOVERED",
    "NATIVE_COMMAND_REFUSED",
    "NATIVE_COMMAND_STARTED",
];
const TURN: &str = "NATIVE_TURN_SETTLED";
type Census = BTreeMap<String, i64>;
type Outbox = BTreeMap<i64, Change>;

#[derive(Default)]
struct Samples {
    dto_us: Vec<i64>,
    coverage_us: Vec<i64>,
    task_projection_us: Vec<i64>,
    project_projection_us: Vec<i64>,
    depth: Vec<i64>,
}

fn micros(duration: Duration) -> i64 {
    duration.as_micros().try_into().unwrap()
}

fn value_identity(event: &Value) -> (Batch, Vec<Change>) {
    let batch = &event["batch"];
    let num = |v: &Value| v.as_str().unwrap().parse::<i64>().unwrap();
    (
        Batch {
            project: event["projectId"].as_str().unwrap().into(),
            generation: batch["generation"].as_str().unwrap().into(),
            from: num(&batch["fromSequence"]),
            through: num(&batch["throughSequence"]),
        },
        batch["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Change {
                kind: v["kind"].as_str().unwrap().into(),
                id: v["id"].as_str().unwrap().into(),
                version: num(&v["newVersion"]),
            })
            .collect(),
    )
}

fn correlate(
    project: &str,
    generation: &str,
    window: std::ops::RangeInclusive<i64>,
    outbox: &Outbox,
    delivered: &[(Batch, Vec<Change>)],
    observations: Vec<Observation>,
) -> Result<Samples, &'static str> {
    let mut commits = BTreeMap::new();
    let mut projections = BTreeMap::new();
    let mut broadcasts = BTreeMap::new();
    let valid_batch = |batch: &Batch| {
        batch.project == project
            && batch.generation == generation
            && batch.from <= batch.through
            && window.contains(&batch.from)
            && window.contains(&batch.through)
    };
    for record in observations {
        match record {
            Observation::Commit {
                project: id,
                from,
                through,
                returned,
            } => {
                if id != project
                    || !window.contains(&from)
                    || !window.contains(&through)
                    || from > through
                {
                    return Err("commit outside census window");
                }
                for seq in from..=through {
                    if commits.insert(seq, returned).is_some() {
                        return Err("duplicate commit");
                    }
                }
            }
            Observation::Projection {
                batch,
                depth,
                changes,
            } => {
                if !valid_batch(&batch)
                    || projections
                        .insert(batch.from, (batch, depth, changes))
                        .is_some()
                {
                    return Err("invalid or duplicate projection");
                }
            }
            Observation::Broadcast {
                batch,
                changes,
                sent,
                receivers,
            } => {
                if !valid_batch(&batch)
                    || receivers == 0
                    || broadcasts
                        .insert(batch.from, (batch, changes, sent))
                        .is_some()
                {
                    return Err("invalid or duplicate broadcast");
                }
            }
            Observation::Missing => return Err("missing observation"),
        }
    }

    let mut samples = Samples::default();
    let mut next = *window.start();
    for (batch, changes) in delivered {
        if !valid_batch(batch) || batch.from != next {
            return Err("delivery gap or duplicate");
        }
        let (published, keys, sent) = broadcasts.remove(&next).ok_or("missing broadcast")?;
        let (projected, depth, timed) = projections.remove(&next).ok_or("missing projection")?;
        if published != *batch
            || projected != *batch
            || keys != *changes
            || !timed.iter().map(|(change, _)| change).eq(changes)
            || depth as i64 != batch.through - batch.from + 1
        {
            return Err("inconsistent change or batch identity");
        }
        samples.depth.push(depth as i64);
        let mut sources = BTreeMap::new();
        for seq in batch.from..=batch.through {
            let id = outbox.get(&seq).ok_or("missing retained outbox row")?;
            let committed = commits.remove(&seq).ok_or("missing commit")?;
            // A separate WAL reader can see the committed row before the
            // writing thread returns from COMMIT (or is scheduled to stamp it).
            // Once identities match, preserve the observed ordering, not a zero.
            let elapsed = match sent.checked_duration_since(committed) {
                Some(duration) => micros(duration),
                None => -micros(committed.duration_since(sent)),
            };
            samples.coverage_us.push(elapsed);
            if id.kind == "task" && sources.insert(id, elapsed).is_some() {
                return Err("duplicate source task version");
            }
        }
        for (key, duration) in timed {
            match key.kind.as_str() {
                "task" => {
                    samples
                        .dto_us
                        .push(*sources.get(&key).ok_or("unmatched task DTO")?);
                    samples.task_projection_us.push(micros(duration));
                }
                // A project invalidation represents the batch, not a single outbox entity.
                "project" => samples.project_projection_us.push(micros(duration)),
                _ => return Err("unexpected projection kind"),
            }
        }
        next = batch.through + 1;
    }
    if next != window.end() + 1
        || !commits.is_empty()
        || !projections.is_empty()
        || !broadcasts.is_empty()
        || samples.coverage_us.len() != outbox.len()
        || samples.dto_us.is_empty()
    {
        return Err("incomplete window");
    }
    Ok(samples)
}

fn summarize_counts(rows: Vec<(String, i64)>) -> Census {
    let mut census: Census = COMMAND_KINDS
        .iter()
        .copied()
        .chain([TURN])
        .map(|kind| (kind.into(), 0))
        .collect();
    for (kind, count) in rows {
        assert!(count > 0);
        assert_eq!(census.insert(kind, count).unwrap_or(0), 0);
    }
    census
}

fn self_check() {
    let batch = Batch {
        project: "p".into(),
        generation: "g".into(),
        from: 1,
        through: 2,
    };
    let source = |version| Change {
        kind: "task".into(),
        id: "t".into(),
        version,
    };
    let outbox = Outbox::from([(1, source(1)), (2, source(2))]);
    let delivered = [(batch.clone(), vec![source(2)])];
    let now = Instant::now();
    let records = || {
        vec![
            Observation::Commit {
                project: "p".into(),
                from: 1,
                through: 1,
                returned: now,
            },
            Observation::Commit {
                project: "p".into(),
                from: 2,
                through: 2,
                returned: now + Duration::from_micros(10),
            },
            Observation::Projection {
                batch: batch.clone(),
                depth: 2,
                changes: vec![(source(2), Duration::from_micros(7))],
            },
            Observation::Broadcast {
                batch: batch.clone(),
                changes: vec![source(2)],
                sent: now + Duration::from_micros(70),
                receivers: 1,
            },
        ]
    };
    let check = |observations| correlate("p", "g", 1..=2, &outbox, &delivered, observations);
    let samples = check(records()).unwrap();
    assert_eq!(samples.dto_us, [60]);
    assert_eq!(samples.coverage_us, [70, 60]);
    assert_eq!(samples.task_projection_us, [7]);
    assert_eq!(samples.depth, [2]);
    for missing in 0..records().len() {
        let mut partial = records();
        partial.remove(missing);
        assert!(check(partial).is_err());
    }
    let mut replay = records();
    replay.push(records().remove(0));
    assert!(check(replay).is_err());
    let mut mismatch = records();
    let Observation::Broadcast { changes, .. } = mismatch.last_mut().unwrap() else {
        unreachable!();
    };
    changes[0].version = 1;
    assert!(check(mismatch).is_err());
    assert!(correlate("p", "g", 1..=2, &Outbox::new(), &delivered, records()).is_err());
    assert!(correlate("p", "g", 1..=2, &outbox, &[], records()).is_err());
    let mut reordered = records();
    reordered.reverse();
    assert_eq!(check(reordered).unwrap().dto_us, [60]);
    let mut visible_before_return = records();
    let Observation::Commit { returned, .. } = &mut visible_before_return[1] else {
        unreachable!();
    };
    *returned = now + Duration::from_micros(80);
    let samples = check(visible_before_return).unwrap();
    assert_eq!(samples.dto_us, [-10]);
    assert_eq!(samples.coverage_us, [70, -10]);
    assert!(summarize_counts(Vec::new())
        .values()
        .all(|count| *count == 0));
    assert_eq!(
        summarize_counts(vec![("NATIVE_COMMAND_EXTRA".into(), 1)])["NATIVE_COMMAND_EXTRA"],
        1
    );
}

async fn workload(observe: bool, phase: &Cell<&str>) -> (Census, Option<Samples>) {
    let fixture = Fixture::new();
    let service = Service::bind_native(fixture.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&fixture.0.join("project"), "Fixture")
        .await
        .unwrap();
    let project = registered["projectId"].as_str().unwrap();
    service.activate(project).await.unwrap();
    let initial = read_page(
        &service,
        json!({
            "protocol":{"major":2,"mapping":2},"requestId":"baseline-initial","projectId":project,
            "operation":"tasks.page","payload":{"target":project,"cursor":null,"limit":50}
        }),
    )
    .await;
    let after: i64 = initial["highWatermark"].as_str().unwrap().parse().unwrap();
    let generation = initial["generation"].as_str().unwrap();
    let mut events = service.subscribe();
    let live = fixture.store();
    let key = live.key().clone();
    let database = key.database();
    let dev = std::fs::metadata(&database).unwrap().dev();
    let wal = database.with_extension("sqlite3-wal");
    assert_eq!(dev, std::fs::metadata(wal).unwrap().dev());
    assert_eq!(dev, std::fs::metadata(std::env::temp_dir()).unwrap().dev());
    let device = format!("{}:{}", libc::major(dev), libc::minor(dev));
    if let Ok(expected) = std::env::var("CORTEX_BASELINE_DEVICE") {
        assert_eq!(device, expected);
    }
    drop(live);

    phase.set("create and start");
    let observer = observe.then(|| Observer::start(key.root().to_path_buf()));
    let runner = tokio::spawn(service.clone().serve());
    let created = service
        .workspace(command(
            project,
            "tasks.create",
            json!(project),
            "baseline-create",
            json!({"description":"Implement disposable fixture","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let started = service
        .workspace(command(
            project,
            "tasks.start",
            created["target"].clone(),
            "baseline-start",
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(started["outcome"], "recorded");
    let workspace_task = created["target"].as_str().unwrap();
    let execution = started["target"].as_str().unwrap();

    phase.set("receive terminal task delta");
    let mut delivered = Vec::new();
    loop {
        let event = service.receive_event(&mut events).await.unwrap();
        assert_ne!(event["type"], "resync_required");
        assert_ne!(event["type"], "native_resync");
        if event["type"] != "native_delta" {
            continue;
        }
        delivered.push(value_identity(&event));
        let complete = event["batch"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| {
                change["id"] == workspace_task
                    && change["operation"]["data"]["data"]["runtimeState"] == "COMPLETED"
                    && change["operation"]["data"]["data"]["executionId"] == execution
            });
        if complete {
            break;
        }
    }
    // COMPLETED is a product state, not the end of the engine's cleanup.
    // Wait for natural termination before requesting shutdown, then drain through
    // that snapshot's high watermark without calling the projection reader.
    let store = Store::open(key, OpenMode::LiveReadOnly).unwrap();
    phase.set("await natural attempt termination");
    let terminal_high = loop {
        let position = store
            .read(|db| {
                let (total, terminated): (i64, i64) = db.query_row(
                    "SELECT count(*), coalesce(sum(d.status='terminated'),0)
                 FROM delegation_provisioning d JOIN workspace_executions e
                   ON d.task_id=e.runtime_task_id WHERE e.execution_id=?",
                    [execution],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if (total, terminated) != (3, 3) {
                    return Ok(None);
                }
                Ok(Some(db.query_row(
                    "SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?",
                    [project],
                    |r| r.get::<_, i64>(0),
                )?))
            })
            .unwrap();
        if let Some(high) = position {
            break high;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    phase.set("drain final watermark");
    while delivered.last().unwrap().0.through < terminal_high {
        let event = service.receive_event(&mut events).await.unwrap();
        assert_ne!(event["type"], "resync_required");
        assert_ne!(event["type"], "native_resync");
        if event["type"] == "native_delta" {
            delivered.push(value_identity(&event));
        }
    }
    phase.set("join service");
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    phase.set("verify complete census");
    let captured = observer.map(Observer::finish);
    assert!(!cortex_core::test_observations::observing(None));
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("project/work.txt")).unwrap(),
        "after"
    );

    let (census, outbox, high, all, runtime_task, settled_roles) = store.read(|db| {
        let (runtime_task, state): (String, String) = db.query_row(
            "SELECT e.runtime_task_id,r.state FROM workspace_executions e
             JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
             WHERE e.task_id=? AND e.execution_id=?",
            [workspace_task, execution], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(state, "COMPLETED");
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_tasks", [], |r| r.get::<_, i64>(0))?, 1);
        assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE task_id=? AND status='terminated'", [&runtime_task], |r| r.get::<_, i64>(0))?, 3);
        assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE task_id=? AND status<>'terminated'", [&runtime_task], |r| r.get::<_, i64>(0))?, 0);

        let predicate = "type='NATIVE_TURN_SETTLED' OR type GLOB 'NATIVE_COMMAND_*'";
        let counts = db.prepare(&format!(
            "SELECT type,count(*) FROM runtime_events WHERE task_id=? AND ({predicate}) GROUP BY type"
        ))?.query_map([&runtime_task], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let events: i64 = db.query_row(&format!("SELECT count(*) FROM runtime_events WHERE {predicate}"), [], |r| r.get(0))?;
        assert_eq!(counts.iter().map(|(_, count)| *count).sum::<i64>(), events);
        let census = summarize_counts(counts);
        let all: i64 = db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=?", [&runtime_task], |r| r.get(0))?;
        let settled_roles = db.prepare(
            "SELECT d.role_id,count(*) FROM runtime_events ev
             JOIN delegation_provisioning d ON d.task_id=ev.task_id
               AND d.channel_id=json_extract(ev.payload_json,'$.channel_id')
             WHERE ev.task_id=? AND ev.type='NATIVE_TURN_SETTLED' GROUP BY d.role_id",
        )?.query_map([&runtime_task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<Census, _>>()?;
        assert_eq!(settled_roles.values().sum::<i64>(), census[TURN]);
        assert!(census[TURN] > 0);
        assert_eq!(db.query_row(
            "SELECT count(*) FROM runtime_events s JOIN runtime_events f ON s.task_id=f.task_id
             AND s.id<f.id AND json_extract(s.payload_json,'$.ref')=
                 json_extract(f.payload_json,'$.receipt.tool_call_ref')
             WHERE s.task_id=? AND s.type='NATIVE_COMMAND_STARTED' AND f.type='NATIVE_COMMAND_FINISHED'",
            [&runtime_task], |r| r.get::<_, i64>(0)
        )?, 1);

        let (high, floor): (i64, i64) = db.query_row(
            "SELECT high_watermark,retained_floor FROM workspace_outbox_state WHERE project_id=?",
            [project], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert!(floor <= after && high > after);
        let outbox = db.prepare("SELECT sequence,kind,entity_id,entity_version
             FROM workspace_outbox WHERE project_id=? AND sequence>? ORDER BY sequence")?
            .query_map((project, after), |r| Ok((r.get(0)?, Change {
                kind: r.get(1)?, id: r.get(2)?, version: r.get(3)?,
            })))?.collect::<Result<Outbox, _>>()?;
        assert!(outbox.keys().copied().eq(after + 1..=high));
        Ok((census, outbox, high, all, runtime_task, settled_roles))
    }).unwrap();

    assert_eq!(delivered.first().unwrap().0.from, after + 1);
    assert_eq!(delivered.last().unwrap().0.through, high);
    assert!(delivered
        .iter()
        .all(|(batch, _)| batch.generation == generation));
    assert!(delivered
        .windows(2)
        .all(|w| w[0].0.through + 1 == w[1].0.from));
    let matched = captured.map(|(observations, overflow)| {
        assert!(!overflow, "trace overflow");
        correlate(
            project,
            generation,
            after + 1..=high,
            &outbox,
            &delivered,
            observations,
        )
        .expect("incomplete disposable fixture evidence")
    });
    let commands: i64 = census
        .iter()
        .filter(|(kind, _)| kind.starts_with("NATIVE_COMMAND_"))
        .map(|(_, count)| count)
        .sum();
    println!("baseline_task observed={observe} device={device} project={project} generation={generation} task={runtime_task} outbox={}..={high} runtime_total={all} command_total={commands} counts={} settled_roles={}",
        after + 1, json!(census), json!(settled_roles));
    (census, matched)
}

fn emit(name: &str, unit: &str, mut values: Vec<i64>) {
    values.sort_unstable();
    if values.is_empty() {
        println!("baseline_metric {name} unit={unit} n=0");
        return;
    }
    let rank = |percent: usize| values[(values.len() * percent).div_ceil(100usize) - 1];
    println!(
        "baseline_metric {name} unit={unit} n={} p50={} p95={} min={} max={} negative={}",
        values.len(),
        rank(50),
        rank(95),
        values.first().unwrap(),
        values.last().unwrap(),
        values.partition_point(|value| *value < 0)
    );
}

pub(super) fn run() {
    self_check();
    assert!(
        !namespace_host::assert_unavailable_view(),
        "baseline requires the positive sandbox path"
    );
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let bounded = |observe| async move {
            let phase = Cell::new("register and activate");
            tokio::time::timeout(Duration::from_secs(90), workload(observe, &phase))
                .await
                .unwrap_or_else(|_| panic!("disposable baseline task timed out: {}", phase.get()))
        };
        let (control, trace) = bounded(false).await;
        assert!(trace.is_none());
        let (_warmup, trace) = bounded(true).await;
        assert!(trace.is_some());
        let mut combined = Samples::default();
        for _ in 0..RUNS {
            let (census, trace) = bounded(true).await;
            // Compare the command behavior; turn rows are an observed outcome,
            // not an expectation that every terminated channel emitted a settle.
            for kind in ["NATIVE_COMMAND_STARTED", "NATIVE_COMMAND_FINISHED"] {
                assert_eq!(
                    control[kind], census[kind],
                    "instrumentation parity: {kind}"
                );
            }
            let mut trace = trace.unwrap();
            combined.dto_us.append(&mut trace.dto_us);
            combined.coverage_us.append(&mut trace.coverage_us);
            combined
                .task_projection_us
                .append(&mut trace.task_projection_us);
            combined
                .project_projection_us
                .append(&mut trace.project_projection_us);
            combined.depth.append(&mut trace.depth);
        }
        emit("commit_return_to_task_dto_broadcast", "us", combined.dto_us);
        emit(
            "commit_return_to_covering_broadcast",
            "us",
            combined.coverage_us,
        );
        emit("task_projection", "us", combined.task_projection_us);
        emit(
            "project_invalidation_projection",
            "us",
            combined.project_projection_us,
        );
        emit("projection_queue_depth", "outbox_rows", combined.depth);
    });
}

use super::*;
use crate::session_bus::{self, Arg, Body, Bus};
use serde_json::json;
use std::time::Duration;

fn task(id: &str, phase: &str) -> Value {
    json!({"taskId":id,"description":format!("Do {id}\nmore"),"phase":phase,"executionId":format!("e-{id}"),"lastConfirmedAt":"t1"})
}
fn question(id: &str, stop: bool) -> Value {
    let mut d = json!({"decisionId":id,"status":"pending","prompt":"Never shown"});
    if stop {
        d["runtimeStop"] = json!(true);
        d["headline"] = json!("The task stopped: the independent review blocked the result.");
    }
    d
}
fn all() -> BTreeSet<Kind> {
    Kind::ALL.into_iter().collect()
}
fn place(id: &str) -> Place {
    Place {
        project: "p".into(),
        sprint: false,
        id: id.into(),
    }
}

#[test]
fn each_owner_moment_maps_to_its_kind_and_target() {
    let tasks = [
        task("q", "waiting"),
        task("stop", "waiting"),
        task("r", "result"),
        task("f", "failed"),
        task("run", "execution"),
        task("auto", "result"),
        task("done", "result"),
        task("gone", "failed"),
    ];
    let product = json!({"tasks":{"q":{"title":"Ask"},"r":{"title":"Review me"},"done":{"status":"ready_to_merge"},"gone":{"archive":{"at":1}}},
        "sprints":[
            {"id":"s1","title":"Plan","planning":"question","conversation":[{"kind":"question","id":"m1"},{"kind":"note"}],"queueState":"idle"},
            {"id":"s2","title":"Queue","planning":"created","queueState":"paused","pauseReason":"A task needs attention or has an unknown outcome."},
            {"id":"s3","planning":"created","queueState":"paused","pauseReason":"This sprint is in manual mode."},
            {"id":"s4","planning":"created","queueState":"paused","pauseReason":"x","archivedAt":5},
            {"id":"s5","planning":"created","runMode":"hybrid","taskIds":["auto"],"queueState":"running"}]});
    let decisions = HashMap::from([
        (
            "q".to_owned(),
            vec![
                json!({"decisionId":"old","status":"answered"}),
                question("wd_1", false),
            ],
        ),
        ("stop".to_owned(), vec![question("wd_2", true)]),
    ]);
    let found = items("p", &tasks, &product, &decisions);
    let by = |id: &str| found.iter().find(|i| i.place.id == id).cloned();
    let q = by("q").unwrap();
    assert_eq!(
        (q.kind, q.label.as_str(), q.detail.clone()),
        (Kind::Question, "Ask", None)
    );
    assert_eq!(
        q.target(),
        json!({"kind":"question","project":"p","task":"q","decision":"wd_1"})
    );
    let stop = by("stop").unwrap();
    assert_eq!(stop.kind, Kind::Stop);
    assert_eq!(stop.label, "Do stop");
    assert!(stop
        .detail
        .as_deref()
        .unwrap()
        .starts_with("The task stopped"));
    assert_eq!(stop.target()["kind"], "question");
    assert_eq!(
        by("r").unwrap().target(),
        json!({"kind":"result","project":"p","task":"r"})
    );
    assert_eq!(
        by("f").unwrap().target(),
        json!({"kind":"task","project":"p","task":"f"})
    );
    assert_eq!(
        by("s1").unwrap().target(),
        json!({"kind":"planning","project":"p","sprint":"s1","decision":"m1"})
    );
    let paused = by("s2").unwrap();
    assert_eq!(
        paused.target(),
        json!({"kind":"sprint","project":"p","sprint":"s2"})
    );
    assert_eq!(
        paused.detail.as_deref(),
        Some("A task needs attention or has an unknown outcome.")
    );
    // Routine progress, an automatic acceptance in progress, settled, archived, Manual mode's
    // stop and an archived sprint are not news to the owner.
    for quiet in ["run", "auto", "done", "gone", "s3", "s4", "s5"] {
        assert!(by(quiet).is_none(), "{quiet}");
    }
    // The question body never reaches a notification.
    assert!(found.iter().all(|i| !i.body().contains("Never shown")));
}

#[test]
fn a_sprint_planning_task_notifies_only_as_its_sprint() {
    let tasks = [task("plan", "waiting"), task("done-plan", "result")];
    let product = json!({"tasks":{}, "sprints":[
        {"id":"s1","title":"Sprint","planning":"planning","planningTaskId":"plan","queueState":"idle"},
        {"id":"s2","title":"Approved","planning":"created","planningTaskId":"done-plan","queueState":"idle"}]});
    let decisions = HashMap::from([("plan".into(), vec![question("ask", false)])]);
    let found = items("p", &tasks, &product, &decisions);
    assert_eq!(
        found.len(),
        1,
        "{:?}",
        found.iter().map(|i| &i.place.id).collect::<Vec<_>>()
    );
    assert_eq!(found[0].kind, Kind::Planning);
    assert_eq!(found[0].place.id, "s1");
    assert!(found[0].place.sprint);
    assert_eq!(found[0].decision.as_deref(), Some("ask"));
}

#[test]
fn automatic_sprint_notifies_only_actual_owner_moments() {
    let failed = task("failed", "failed");
    let mut stopped = task("stopped", "waiting");
    stopped["automaticContinuation"] = json!({"policy":"automatic_continuation"});
    let tasks = [failed, stopped, task("question", "waiting")];
    let product = json!({"tasks":{}, "sprints":[
        {"id":"held","runMode":"automatic","taskIds":["failed","stopped","question"],
            "queueState":"paused","pauseReason":"Route unavailable",
            "automaticQueue":{"kind":"continuation","needsOwner":false}},
        {"id":"merge","runMode":"automatic","taskIds":[],"queueState":"paused",
            "pauseReason":"Confirm integration","automaticQueue":{"kind":"owner_integration","needsOwner":true}},
        {"id":"cap","runMode":"automatic","taskIds":[],"queueState":"paused",
            "automaticQueue":{"kind":"continuation","code":"continuation_cap","taskId":"stopped",
                "reason":"Open the task to resume","needsOwner":true}},
        {"id":"deps","runMode":"automatic","taskIds":[],"queueState":"paused",
            "automaticQueue":{"kind":"settlement_blocked","code":"dependencies_blocked","taskId":null,
                "reason":"Review dependencies","needsOwner":true}}
    ]});
    let decisions = HashMap::from([
        ("stopped".into(), vec![question("stop", true)]),
        ("question".into(), vec![question("work", false)]),
    ]);
    let found = items("p", &tasks, &product, &decisions);
    assert_eq!(found.len(), 4);
    for id in ["cap", "deps"] {
        assert!(found
            .iter()
            .any(|item| item.place.id == id && item.kind == Kind::Paused));
    }
    assert!(found
        .iter()
        .any(|item| item.place.id == "question" && item.kind == Kind::Question));
    assert!(found
        .iter()
        .any(|item| item.place.id == "merge" && item.kind == Kind::Paused));
}

#[test]
fn a_newer_state_replaces_and_a_resolved_item_withdraws() {
    let mut book = Book::default();
    let ask = |d: &str| Item {
        kind: Kind::Question,
        place: place("t"),
        decision: Some(d.into()),
        label: "T".into(),
        detail: None,
        state: format!("q:{d}"),
    };
    let raised = book.plan("p", vec![ask("a")], None, &all(), false);
    assert_eq!(
        raised,
        vec![Action::Raise {
            item: ask("a"),
            replaces: 0
        }]
    );
    book.raised(&ask("a"), 7);
    // The same state again (any later read) raises nothing.
    assert!(book
        .plan("p", vec![ask("a")], None, &all(), false)
        .is_empty());
    // A newer question in the same task replaces its notification.
    assert_eq!(
        book.plan("p", vec![ask("b")], None, &all(), false),
        vec![Action::Raise {
            item: ask("b"),
            replaces: 7
        }]
    );
    book.raised(&ask("b"), 7);
    // The result that follows the answer replaces it too: one notification per task.
    let result = Item {
        kind: Kind::Result,
        place: place("t"),
        decision: None,
        label: "T".into(),
        detail: None,
        state: "r:e:1".into(),
    };
    assert_eq!(
        book.plan("p", vec![result.clone()], None, &all(), false),
        vec![Action::Raise {
            item: result.clone(),
            replaces: 7
        }]
    );
    book.raised(&result, 8);
    // Accepted by hand (or any other way): the item is gone and its notification withdrawn.
    assert_eq!(
        book.plan("p", vec![], None, &all(), false),
        vec![Action::Withdraw { id: 8 }]
    );
    assert!(book.plan("p", vec![], None, &all(), false).is_empty());
    // Another project's read never touches this one's notifications.
    book.raised(&ask("c"), 9);
    assert!(book.plan("other", vec![], None, &all(), false).is_empty());
}

#[test]
fn nothing_is_raised_for_the_place_on_view_a_kind_turned_off_or_the_first_read() {
    let mut book = Book::default();
    let ask = Item {
        kind: Kind::Question,
        place: place("t"),
        decision: Some("d".into()),
        label: "T".into(),
        detail: None,
        state: "q:d".into(),
    };
    // Open in the focused window: seen, never announced later either.
    assert!(book
        .plan("p", vec![ask.clone()], Some(&place("t")), &all(), false)
        .is_empty());
    assert!(book
        .plan("p", vec![ask.clone()], None, &all(), false)
        .is_empty());
    // One already out is withdrawn when the owner opens its place.
    let other = Item {
        place: place("u"),
        state: "q:e".into(),
        ..ask.clone()
    };
    book.plan("p", vec![other.clone()], None, &all(), false);
    book.raised(&other, 3);
    assert_eq!(
        book.plan("p", vec![other.clone()], Some(&place("u")), &all(), false),
        vec![Action::Withdraw { id: 3 }]
    );
    // A kind the owner turned off: nothing, and its open notification goes.
    let result = Item {
        kind: Kind::Result,
        place: place("r"),
        decision: None,
        label: "R".into(),
        detail: None,
        state: "r:1".into(),
    };
    let no_results: BTreeSet<Kind> = all().into_iter().filter(|k| *k != Kind::Result).collect();
    assert!(book
        .plan("p", vec![result.clone()], None, &no_results, false)
        .is_empty());
    // The very first read records what is open without announcing it.
    let mut fresh = Book::default();
    assert!(fresh
        .plan("p", vec![ask.clone()], None, &all(), true)
        .is_empty());
    assert!(fresh.plan("p", vec![ask], None, &all(), false).is_empty());
}

#[test]
fn a_click_on_a_raised_id_returns_its_target_and_token_once() {
    let mut book = Book::default();
    let ask = Item {
        kind: Kind::Stop,
        place: place("t"),
        decision: Some("d".into()),
        label: "T".into(),
        detail: None,
        state: "q:d".into(),
    };
    book.raised(&ask, 41);
    book.token(41, "fixture-token");
    assert_eq!(book.invoked(40, "default"), None);
    assert_eq!(book.invoked(41, "open"), None);
    assert_eq!(
        book.invoked(41, "default"),
        Some((ask.target(), Some("fixture-token".into())))
    );
    assert_eq!(book.invoked(41, "default"), None);
    book.raised(&ask, 42);
    book.token(42, "bad token");
    assert_eq!(book.invoked(42, "default").unwrap().1, None);
    book.raised(&ask, 43);
    book.closed(43);
    assert_eq!(book.invoked(43, "default"), None);
}

#[test]
fn the_hand_off_is_claimed_by_a_window_or_kept_once_for_a_started_one() {
    let mut h = Handoff::default();
    let now = Instant::now();
    let target = json!({"kind":"task","project":"p","task":"t"});
    let n = h.clicked(target.clone(), Some("tok".into()), now);
    assert_eq!(h.opened("other"), None);
    assert_eq!(h.opened(&n), Some(Some("tok".into())));
    // Claimed: the timer finds nothing to start, and no target is pending.
    assert_eq!(h.unclaimed(&n, now), None);
    assert_eq!(h.take(now), None);
    // Unclaimed: the app is started with the token, and the target waits once.
    let n = h.clicked(target.clone(), Some("tok2".into()), now);
    assert_eq!(h.unclaimed(&n, now), Some(Some("tok2".into())));
    assert_eq!(h.opened(&n), None);
    assert_eq!(h.take(now + Duration::from_secs(1)), Some(target.clone()));
    assert_eq!(h.take(now + Duration::from_secs(1)), None);
    // A stale one is dropped.
    let n = h.clicked(target, None, now);
    h.unclaimed(&n, now);
    assert_eq!(h.take(now + PENDING + Duration::from_secs(1)), None);
    // A newer click replaces an unclaimed older one.
    let a = h.clicked(json!({"kind":"task","project":"p","task":"a"}), None, now);
    let b = h.clicked(json!({"kind":"task","project":"p","task":"b"}), None, now);
    assert_eq!(h.unclaimed(&a, now), None);
    assert!(h.unclaimed(&b, now).is_some());
}

#[test]
fn a_window_report_is_a_closed_shape() {
    let (place, kinds) =
        report(json!({"focused":{"project":"p","task":"t"},"kinds":["question","result"]}))
            .unwrap();
    assert_eq!(
        place,
        Some(Place {
            project: "p".into(),
            sprint: false,
            id: "t".into()
        })
    );
    assert_eq!(kinds, BTreeSet::from([Kind::Question, Kind::Result]));
    assert_eq!(report(json!({"focused":null,"kinds":[]})).unwrap().0, None);
    assert!(
        report(json!({"focused":{"project":"p","sprint":"s"},"kinds":[]}))
            .unwrap()
            .0
            .unwrap()
            .sprint
    );
    for bad in [
        json!({"focused":{"project":"p","task":"t","sprint":"s"},"kinds":[]}),
        json!({"focused":{"project":"p"},"kinds":[]}),
        json!({"focused":null,"kinds":["everything"]}),
        json!({"focused":null,"kinds":[],"url":"x"}),
        json!({"focused":{"project":"a b","task":"t"},"kinds":[]}),
        json!({"kinds":[]}),
    ] {
        assert!(report(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn the_app_is_started_through_the_user_manager_with_the_token() {
    let argv = launch_command(Some("tok-1"));
    assert_eq!(
        &argv[..5],
        [
            "systemd-run",
            "--user",
            "--collect",
            "--quiet",
            "--no-block"
        ]
    );
    assert!(argv.contains(&"--setenv=DESKTOP_STARTUP_ID=tok-1".to_owned()));
    assert!(argv.contains(&"--setenv=XDG_ACTIVATION_TOKEN=tok-1".to_owned()));
    assert_eq!(argv[argv.len() - 2], "--");
    assert!(argv
        .last()
        .unwrap()
        .ends_with(&workspace_protocol::channel::current().desktop_binary));
    // A token that could carry another argument or variable is not passed at all.
    assert!(!launch_command(Some("a b"))
        .iter()
        .any(|a| a.contains("STARTUP")));
    assert!(!launch_command(None).iter().any(|a| a.contains("STARTUP")));
}

#[test]
fn only_project_changes_wake_the_notifier() {
    assert_eq!(
        changed_project(&json!({"type":"native_delta","projectId":"p","batch":{}})),
        Some("p")
    );
    assert_eq!(
        changed_project(&json!({"type":"invalidation","projectId":"p"})),
        Some("p")
    );
    assert_eq!(changed_project(&json!({"type":"remote_state"})), None);
    assert_eq!(
        changed_project(&json!({"type":"notification_open","projectId":"p"})),
        None
    );
}

/// A private `dbus-daemon` (never the owner's session bus), a stand-in notification server
/// that answers `Notify` with 41 and reports a click unicast to the sender as GNOME Shell does,
/// and the service's own bus client and book on the other side.
#[test]
#[ignore = "needs dbus-daemon; starts a private bus in a temporary directory"]
fn the_service_raises_and_receives_the_click_on_a_private_bus() {
    let dir = std::env::temp_dir().join(format!("cw-notify-bus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("bus");
    let config = dir.join("bus.conf");
    std::fs::write(&config, format!("<busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth>\
        <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"false\"/><allow eavesdrop=\"false\"/><allow own=\"*\"/></policy></busconfig>",
        socket.display())).unwrap();
    /// The private daemon ends with the test, whatever its outcome.
    struct Daemon(std::process::Child, PathBuf);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let _daemon = Daemon(
        std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("dbus-daemon"),
        dir.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // The stand-in server.
    let (calls, observed) = std::sync::mpsc::channel::<session_bus::Message>();
    let server = Bus::connect(&socket, move |m| {
        if m.kind == 1 {
            let _ = calls.send(m);
        }
    })
    .unwrap();
    let mut name = Body::default();
    name.str(NAME).u32(0);
    server
        .call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            "su",
            &name.bytes,
        )
        .unwrap();
    // The service side.
    let (signals, received) = std::sync::mpsc::channel::<session_bus::Message>();
    let client = Bus::connect(&socket, move |m| {
        if m.kind == 4 {
            let _ = signals.send(m);
        }
    })
    .unwrap();
    let item = Item {
        kind: Kind::Question,
        place: place("t"),
        decision: Some("wd_1".into()),
        label: "Fix <the> race".into(),
        detail: None,
        state: "q:wd_1".into(),
    };
    let caller = client.clone();
    let body = notify_body(&item, 0);
    let raise =
        std::thread::spawn(move || caller.call(NAME, PATH, NAME, "Notify", "susssasa{sv}i", &body));
    let call = observed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (call.member.as_str(), call.sender.as_str()),
        ("Notify", client.unique.as_str())
    );
    assert_eq!(
        call.body[0],
        Arg::Str(workspace_protocol::channel::current().title.clone())
    );
    assert_eq!(call.body[3], Arg::Str("Question waiting".into()));
    assert_eq!(call.body[4], Arg::Str("Fix &lt;the&gt; race".into()));
    let mut id = Body::default();
    id.u32(41);
    server
        .reply(&call.sender, call.serial, "u", &id.bytes)
        .unwrap();
    let reply = raise.join().unwrap().unwrap();
    assert_eq!(reply.body, vec![Arg::U32(41)]);
    assert_eq!(reply.sender, server.unique);
    let mut book = Book::default();
    book.raised(&item, 41);
    for (member, signature, body) in [
        ("ActivationToken", "us", {
            let mut b = Body::default();
            b.u32(41).str("fixture-token");
            b.bytes
        }),
        ("ActionInvoked", "us", {
            let mut b = Body::default();
            b.u32(40).str("default");
            b.bytes
        }),
        ("ActionInvoked", "us", {
            let mut b = Body::default();
            b.u32(41).str("default");
            b.bytes
        }),
    ] {
        server
            .emit(&client.unique, PATH, NAME, member, signature, &body)
            .unwrap();
    }
    let mut clicks = Vec::new();
    let mut seen = 0;
    while seen < 3 {
        let m = received.recv_timeout(Duration::from_secs(5)).unwrap();
        // The bus's own signals (NameAcquired) reach every connection; the service ignores them too.
        if m.interface != NAME {
            continue;
        }
        seen += 1;
        assert_eq!(
            (m.sender.as_str(), m.interface.as_str(), m.path.as_str()),
            (server.unique.as_str(), NAME, PATH)
        );
        let (id, text) = (
            m.body[0].u32().unwrap(),
            m.body[1].str().unwrap().to_owned(),
        );
        match m.member.as_str() {
            "ActivationToken" => book.token(id, &text),
            _ => clicks.extend(book.invoked(id, &text)),
        }
    }
    assert_eq!(clicks, vec![(item.target(), Some("fixture-token".into()))]);
    // Withdrawal on the same connection reaches the server as CloseNotification(41).
    let mut id = Body::default();
    id.u32(41);
    client
        .notify_only(NAME, PATH, NAME, "CloseNotification", "u", &id.bytes)
        .unwrap();
    let close = observed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (close.member.as_str(), close.body.clone()),
        ("CloseNotification", vec![Arg::U32(41)])
    );
}

#[test]
fn notification_details_never_show_internal_codes() {
    assert_eq!(
        plain_detail("continuation_cap"),
        "Open it to see what is needed."
    );
    assert_eq!(
        plain_detail("Queue paused: review_cycle_limit."),
        "Queue paused: review cycle limit."
    );
    // Paths and file names are kept as written.
    assert_eq!(
        plain_detail("Changed src/task_facts.rs and notes_v2.md"),
        "Changed src/task_facts.rs and notes_v2.md"
    );
    assert_eq!(
        plain_detail("Resolve the task conflict first."),
        "Resolve the task conflict first."
    );
}

/// L4: a Hybrid queue notifies only when the owner acts next, and never for the
/// owner's own pause.
#[test]
fn hybrid_sprint_notifies_only_owner_holds_and_never_the_owners_own_pause() {
    let product = json!({"tasks":{}, "sprints":[
        {"id":"mine","runMode":"hybrid","taskIds":[],"queueState":"paused","pauseReason":"Paused by you"},
        {"id":"route","runMode":"hybrid","taskIds":[],"queueState":"paused",
            "pauseReason":"The engine is not available right now.",
            "automaticQueue":{"kind":"continuation","code":"external_blocker","taskId":"t",
                "reason":"The engine is not available right now.","needsOwner":false,"nextActor":"runtime"}},
        {"id":"missing","runMode":"hybrid","taskIds":[],"queueState":"paused",
            "pauseReason":"A sprint task could not be read.",
            "automaticQueue":{"kind":"settlement_blocked","code":"task_missing","taskId":null,
                "reason":"A sprint task could not be read.","needsOwner":false,"nextActor":"runtime"}},
        {"id":"owner","runMode":"hybrid","taskIds":[],"queueState":"paused",
            "pauseReason":"The last run of this task ended as failed.",
            "automaticQueue":{"kind":"settlement_blocked","code":"task_failed","taskId":"t",
                "reason":"The last run of this task ended as failed.","needsOwner":true,"nextActor":"owner"}},
        {"id":"automatic-mine","runMode":"automatic","taskIds":[],"queueState":"paused","pauseReason":"Paused by you"}
    ]});
    let found = items("p", &[], &product, &HashMap::new());
    let paused: Vec<&str> = found
        .iter()
        .filter(|item| item.kind == Kind::Paused)
        .map(|item| item.place.id.as_str())
        .collect();
    assert_eq!(paused, ["owner"], "{found:?}");
}

//! Focused service API regressions. The coordinator is a scripted ACP peer; the
//! ordinary desktop worker/reviewer fixtures handle owned work and review.
use super::*;
use cortex_engine::wire;
use tokio::io::BufReader;

const VERIFY: &str = "service-regression-verification";
const BATCH: &str = "service-regression-answer-files";
const SPRINT: &str = "service-regression-sprint-questions";
const FAILED: [&str; 3] = ["/usr/bin/cmp", "expected.txt", "work.txt"];
const FIXED: [&str; 3] = ["/usr/bin/cmp", "correct-expected.txt", "work.txt"];
const QUESTIONS: [&str; 3] = ["Which output?", "Which language?", "Which format?"];
const ANSWERS: [&str; 3] = [
    "Detailed: include examples",
    "English: plain terms",
    "Text: one section",
];

fn question() -> Value {
    json!({"name":"user_question_request","arguments":{"request_key":"owner-decisions","questions":[
        {"question":QUESTIONS[0],"recommended":2,"options":["Detailed","Brief"]},
        {"question":QUESTIONS[1],"recommended":1,"options":["English","French"]},
        {"question":QUESTIONS[2],"recommended":2,"options":["Text","HTML"]}
    ]}})
}

pub(super) fn run(mode: &str) -> bool {
    if !matches!(mode, VERIFY | BATCH | SPRINT) {
        return false;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        match mode {
            VERIFY => {
                assert!(
                    !namespace_host::assert_unavailable_view(),
                    "verification regression requires user namespaces"
                );
                verification().await;
            }
            BATCH => batch().await,
            SPRINT => sprint().await,
            _ => return false,
        }
        println!("{mode}: passed");
        true
    })
}

async fn until<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} did not arrive"))
}

async fn setup(
    mode: &str,
) -> (
    Fixture,
    Arc<Service>,
    String,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), mode).unwrap();
    if mode == VERIFY {
        std::fs::write(f.0.join("project/expected.txt"), "stale expectation").unwrap();
        std::fs::write(f.0.join("project/correct-expected.txt"), "after").unwrap();
    }
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let project = service
        .register(&f.0.join("project"), "Service regression")
        .await
        .unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    service.activate(&project).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    (f, service, project, runner)
}

async fn start(service: &Service, project: &str) -> String {
    let draft = service
        .workspace(command(
            project,
            "tasks.create",
            json!(project),
            "create",
            json!({"description":"Service regression","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    service
        .workspace(command(
            project,
            "tasks.start",
            draft["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap()["target"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn decisions(service: &Service, project: &str, execution: &str) -> Vec<Value> {
    service.workspace(json!({"protocol":{"major":1,"minor":8},"projectId":project,"requestId":"read-decisions",
        "operation":"decisions.list","target":execution,"cursor":null,"limit":20}))
        .await.unwrap()["items"].as_array().unwrap().clone()
}

async fn answer(
    service: &Service,
    project: &str,
    execution: &str,
    rows: &[Value],
    texts: &[&str],
    files: &[Vec<Value>],
) {
    assert_eq!(rows.len(), texts.len());
    assert_eq!(rows.len(), files.len());
    let first = &rows[0];
    let batch = &first["batchId"];
    let answers: Vec<Value> = rows.iter().zip(texts).zip(files).map(|((row,text),files)| {
        assert_eq!(&row["batchId"], batch);
        assert_eq!(row["acpSessionId"], first["acpSessionId"]);
        json!({"decisionId":row["decisionId"],"questionId":row["questionId"],"expectedVersion":row["version"],
            "optionId":null,"text":text,"attachments":files})
    }).collect();
    let mut request = command(
        project,
        "decisions.answer",
        if batch.is_null() {
            first["decisionId"].clone()
        } else {
            batch.clone()
        },
        "answer",
        json!({"executionId":execution,"acpSessionId":first["acpSessionId"],"batchId":batch,"answers":answers}),
    );
    request["expectedVersion"] = first["version"].clone();
    request["ownerEpoch"] = first["ownerEpoch"].clone();
    let receipt = service.workspace(request.clone()).await.unwrap();
    assert_eq!(
        service.workspace(request).await.unwrap(),
        receipt,
        "an exact retry must not duplicate answers"
    );
}

async fn open_questions(
    service: &Service,
    project: &str,
    execution: &str,
    count: usize,
) -> Vec<Value> {
    until("all open questions", || async {
        let rows: Vec<_> = decisions(service, project, execution)
            .await
            .into_iter()
            .filter(|row| row["status"] == "pending")
            .collect();
        (rows.len() == count).then_some(rows)
    })
    .await
}

fn state(store: &Store, execution: &str) -> (String, Option<String>) {
    store
        .read(|db| {
            Ok(db.query_row(
                "SELECT t.state,t.blocker FROM runtime_tasks t
        JOIN workspace_executions x ON x.runtime_task_id=t.task_id WHERE x.execution_id=?",
                [execution],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap()
}

fn events(store: &Store, execution: &str, kind: &str) -> Vec<Value> {
    store
        .read(|db| {
            let mut query = db.prepare(
                "SELECT e.payload_json FROM runtime_events e
            JOIN workspace_executions x ON x.runtime_task_id=e.task_id
            WHERE x.execution_id=? AND e.type=? ORDER BY e.id",
            )?;
            let raw = query
                .query_map([execution, kind], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(raw
                .iter()
                .map(|entry| serde_json::from_str(entry).unwrap())
                .collect())
        })
        .unwrap()
}

async fn completed(store: &Store, execution: &str) {
    until("task completion", || async {
        let (state, blocker) = state(store, execution);
        assert!(
            !matches!(state.as_str(), "FAILED" | "CANCELLED"),
            "{state}: {blocker:?}"
        );
        if state == "COMPLETED" {
            assert_eq!(blocker, None);
            Some(())
        } else {
            None
        }
    })
    .await
}

pub(super) async fn verification() {
    let (f, service, project, runner) = setup(VERIFY).await;
    let execution = start(&service, &project).await;
    let store = f.store();
    let rows = open_questions(&service, &project, &execution, 1).await;

    assert_eq!(
        rows[0]["prompt"],
        "Use the corrected check?",
        "fixture tool replies: {:?}",
        std::fs::read_to_string(f.0.join("fixture-tool-log"))
    );
    let failed = events(&store, &execution, "VERIFICATION_FAILED");
    let revised = events(&store, &execution, "VERIFICATION_REVISED");
    let passed = events(&store, &execution, "VERIFICATION_PASSED");
    assert_eq!(failed.len(), 1);
    assert_eq!(revised.len(), 1);
    assert_eq!(passed.len(), 1);
    assert_eq!(revised[0]["verification"], json!([FIXED]));
    let first = &failed[0]["checks"][0];
    let second = &passed[0]["checks"][0];
    assert_eq!(failed[0]["revision"], passed[0]["revision"]);
    for (check, argv, exit) in [(first, json!(FAILED), 1), (second, json!(FIXED), 0)] {
        assert_eq!(check["argv"], argv);
        assert_eq!(check["complete"], true);
        assert_eq!(check["exit_code"], exit);
        assert!(check["receipt_ref"].as_str().is_some_and(|r| !r.is_empty()));
    }
    assert_ne!(first["receipt_ref"], second["receipt_ref"]);
    assert_eq!(state(&store, &execution).0, "WAITING_USER");
    // Removing rather than passing the old command requires a task-global owner
    // acknowledgement asked after the removal. The passing command is not a waiver.
    answer(
        &service,
        &project,
        &execution,
        &rows,
        &["Accept corrected check"],
        &[vec![]],
    )
    .await;
    completed(&store, &execution).await;
    assert_eq!(events(&store, &execution, "VERIFICATION_FAILED"), failed);
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

fn assert_question_batch(rows: &[Value]) {
    assert_eq!(rows.len(), QUESTIONS.len());
    assert!(rows[0]["batchId"].is_string());
    for (row, prompt) in rows.iter().zip(QUESTIONS) {
        assert_eq!(row["prompt"], prompt, "{row}");
    }
}

pub(super) async fn batch() {
    let (f, service, project, runner) = setup(BATCH).await;
    let execution = start(&service, &project).await;
    let store = f.store();
    let open = open_questions(&service, &project, &execution, 3).await;
    assert_question_batch(&open);
    let files = [
        vec![
            sent_file(&service, "first.png", "image/png", PNG_BYTES).await,
            sent_file(
                &service,
                "second.png",
                "image/png",
                b"\x89PNG\r\n\x1a\nsecond image",
            )
            .await,
        ],
        vec![],
        vec![
            sent_file(
                &service,
                "third.png",
                "image/png",
                b"\x89PNG\r\n\x1a\nthird image",
            )
            .await,
        ],
    ];
    answer(&service, &project, &execution, &open, &ANSWERS, &files).await;
    completed(&store, &execution).await;
    let settled = decisions(&service, &project, &execution).await;
    assert_question_batch(&settled);
    for (index, row) in settled.iter().enumerate() {
        assert_eq!(row["questionId"], open[index]["questionId"]);
        assert_eq!(row["status"], "answered");
        assert_eq!(row["answerText"], ANSWERS[index]);
        if files[index].is_empty() {
            assert!(row["answerAttachments"].is_null() || row["answerAttachments"] == json!([]));
        } else {
            assert_eq!(row["answerAttachments"], json!(files[index]));
        }
    }
    let captures = events(&store, &execution, "USER_ANSWER_ATTACHMENTS");
    assert_eq!(
        captures.len(),
        1,
        "one batch capture, including after replay"
    );
    assert_eq!(
        captures[0]["question_ids"],
        json!([
            open[0]["questionId"],
            open[0]["questionId"],
            open[2]["questionId"]
        ])
    );

    let prompts = engine_prompts(&f);
    let resumed: Vec<_> = prompts
        .iter()
        .filter(|prompt| prompt.to_string().contains(ANSWERS[0]))
        .collect();
    assert_eq!(resumed.len(), 1, "{prompts:?}");
    let encoded = resumed[0].to_string();
    let positions: Vec<_> = ANSWERS
        .iter()
        .map(|text| {
            encoded
                .find(text)
                .expect("answer missing from delivered prompt")
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    let images: Vec<_> = resumed[0]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "image")
        .collect();
    use base64::Engine as _;
    let expected: [&[u8]; 3] = [
        PNG_BYTES,
        b"\x89PNG\r\n\x1a\nsecond image",
        b"\x89PNG\r\n\x1a\nthird image",
    ];
    assert_eq!(images.len(), expected.len());
    for ((block, bytes), file) in images.iter().zip(expected).zip(files.iter().flatten()) {
        assert_eq!(block["mimeType"], "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(block["data"].as_str().unwrap())
                .unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(
                f.0.join("state/attachments")
                    .join(file["sha256"].as_str().unwrap())
            )
            .unwrap(),
            bytes
        );
    }
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

pub(super) async fn sprint() {
    let (f, service, project, runner) = setup(SPRINT).await;
    let sprint = machine(&service,"product.sprint",&project,
        json!({"action":"create","description":"Choose the sprint scope","runMode":"hybrid","checkout":{"kind":"main"}}))
        .await["sprintId"].clone();
    // If the service's initial reconciliation observes the new sprint first,
    // it marks the unstarted plan interrupted. Resume it through the owner API.
    let resumed = std::cell::Cell::new(false);
    let execution = until("sprint planning execution", || async {
        let board = machine(&service,"product.board",&project,json!({})).await;
        let current = board["sprints"].as_array().unwrap().iter().find(|row| row["id"] == sprint).unwrap();
        if current["planning"] == "cancelled" {
            assert!(!resumed.replace(true), "{current}");
            machine(&service,"product.sprint",&project,
                json!({"action":"patch","sprintId":sprint,"patch":{"planning":"planning"}})).await;
            return None;
        }
        let draft = current["planningTaskId"].as_str()?;
        f.store().read(|db| {
            use rusqlite::OptionalExtension;
            Ok(db.query_row("SELECT e.execution_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?", [draft], |row| row.get::<_,String>(0)).optional()?)
        }).unwrap()
    }).await;
    let store = f.store();
    let questions = open_questions(&service, &project, &execution, 3).await;
    assert_question_batch(&questions);
    assert_eq!(events(&store, &execution, "SPRINT_PLANNING_BOUND").len(), 1);
    assert_eq!(state(&store, &execution).0, "WAITING_USER");
    answer(
        &service,
        &project,
        &execution,
        &questions,
        &ANSWERS,
        &[vec![], vec![], vec![]],
    )
    .await;
    until("planning resumed with owner answers", || async {
        let prompts = engine_prompts(&f);
        prompts
            .iter()
            .any(|prompt| {
                let body = prompt.to_string();
                ANSWERS.iter().all(|answer| body.contains(answer))
            })
            .then_some(())
    })
    .await;
    until("the resumed planner's submission", || async {
        (events(&store, &execution, "SPRINT_PLAN_SUBMITTED").len() == 1).then_some(())
    })
    .await;
    let transitions = events(&store, &execution, "TASK_STATE_CHANGED");
    assert!(
        transitions
            .iter()
            .any(|event| event["previous"] == "WAITING_USER" && event["state"] == "PLANNING"),
        "{transitions:?}"
    );
    let settled = decisions(&service, &project, &execution).await;
    assert_question_batch(&settled);
    for ((row, question), text) in settled.iter().zip(questions).zip(ANSWERS) {
        assert_eq!(row["questionId"], question["questionId"]);
        assert_eq!(row["status"], "answered");
        assert_eq!(row["answerText"], text);
    }
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

pub(super) async fn peer() {
    let scenario = std::fs::read_to_string("scenario.txt").unwrap();
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut relay = Value::Null;
    let config = |mode: &str| {
        json!([
            {"id":"model","type":"select","currentValue":"fixture-model","options":[{"value":"fixture-model","name":"Fixture"}]},
            {"id":"mode","type":"select","currentValue":mode,"options":(["agent","workspace-write","read-only","plan"].map(|v|json!({"value":v,"name":v})))}
        ])
    };
    while let Some(frame) = wire::read_frame(&mut input).await.unwrap() {
        let params = &frame["params"];
        let result = match frame["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"image":true,"embeddedContext":true}}})
            }
            "session/new" => {
                relay = params["mcpServers"][0].clone();
                json!({"sessionId":"scripted-coordinator","configOptions":config("agent")})
            }
            "session/set_config_option" => {
                json!({"configOptions":config(params["value"].as_str().unwrap())})
            }
            "session/prompt" => {
                let prompt = &params["prompt"];
                let body = prompt.to_string();
                if scenario != VERIFY {
                    use std::io::Write as _;
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("prompt-log.jsonl")
                        .unwrap();
                    writeln!(log, "{}", json!({"prompt":prompt})).unwrap();
                }
                let call = if scenario == VERIFY {
                    let context = prompt
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|block| block["text"].as_str())
                        .flat_map(str::lines)
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .find(|value| {
                            value["original_goal"].is_string() && value["recipient"].is_object()
                        })
                        .unwrap_or(Value::Null);
                    let failed = context["retained_decisions"]
                        .as_array()
                        .is_some_and(|events| {
                            events
                                .iter()
                                .any(|event| event["type"] == "VERIFICATION_FAILED")
                        });
                    if body.contains("Accept corrected check") && context["state"] != "RECONCILING"
                    {
                        json!({"name":"answer_submit","arguments":{"request_key":"finish","text":"Corrected check passed."}})
                    } else if context["state"] == "RECONCILING" {
                        json!({"name":"user_question_request","arguments":{"request_key":"acknowledge","question":"Use the corrected check?","recommended":1,"options":["Accept corrected check","Keep original"]}})
                    } else if failed {
                        json!({"name":"verification_submit","arguments":{"request_key":"revise-check","verification":[FIXED]}})
                    } else {
                        json!({"name":"plan_submit","arguments":{"request_key":"initial-plan","plan":{
                            "tasks":[{"id":"work","role":"backend-dev","objective":"Write the output","dependencies":[],"scope":["work.txt"],"model":"auto"}],
                            "verification":[FAILED]}}})
                    }
                } else if body.contains(ANSWERS[0]) {
                    if scenario == SPRINT {
                        json!({"name":"sprint_plan_submit","arguments":{"request_key":"sprint-plan","tasks":[
                            {"key":"examples","title":"Document examples","description":"Write examples in plain English.","dependsOn":[]}
                        ]}})
                    } else {
                        json!({"name":"answer_submit","arguments":{"request_key":"finish","text":"Owner answers received."}})
                    }
                } else {
                    question()
                };
                call_tool(&relay, call).await;
                json!({"stopReason":"end_turn"})
            }
            _ if frame.get("id").is_none() => continue,
            method => panic!("unexpected ACP method {method}"),
        };
        wire::write_frame(
            &mut output,
            &json!({"jsonrpc":"2.0","id":frame["id"],"result":result}),
        )
        .await
        .unwrap();
    }
}

async fn call_tool(relay: &Value, call: Value) {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new(relay["command"].as_str().unwrap())
        .args(
            relay["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = BufReader::new(child.stdout.take().unwrap());
    let mut output = child.stdin.take().unwrap();
    wire::write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":call}),
    )
    .await
    .unwrap();
    let reply = wire::read_frame(&mut input).await.unwrap().unwrap();
    if std::fs::read_to_string("scenario.txt").is_ok_and(|mode| mode == VERIFY) {
        use std::io::Write as _;
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("../fixture-tool-log")
            .unwrap();
        writeln!(
            log,
            "{}: refused={}; code={}",
            call["name"], reply["result"]["isError"], reply["result"]["structuredContent"]["code"]
        )
        .unwrap();
    }
    assert!(
        reply.get("error").is_none() && reply["result"]["isError"] != true,
        "{} refused: {reply}",
        call["name"]
    );
    child.kill().await.unwrap();
}

//! Coordinator questions and evidence-backed skill consent. No project mutations.
use super::{domain, event_tx, request_tx};
use crate::{error::refuse, CoreError, CoreResult, TaskState};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace('ſ', "s")
}
pub fn safe_options(options: &Value, action: &str) -> bool {
    let pairs = match action {
        "create" => [
            ["Create the skill", "Not now"],
            ["Создать навык", "Не сейчас"],
            ["Approve creating the skill", "Decline creating the skill"],
            ["Одобрить создание навыка", "Отклонить создание навыка"],
        ],
        "update" => [
            ["Update the skill", "Not now"],
            ["Обновить навык", "Не сейчас"],
            ["Approve updating the skill", "Decline updating the skill"],
            ["Одобрить обновление навыка", "Отклонить обновление навыка"],
        ],
        _ => return false,
    };
    options.as_array().is_some_and(|a| {
        a.len() == 2
            && pairs.iter().any(|p| {
                a.iter()
                    .zip(p)
                    .all(|(a, b)| a.as_str().is_some_and(|s| normalized(s) == normalized(b)))
            })
    })
}
pub fn decide(options: &Value, answer: &str, action: &str) -> &'static str {
    if !safe_options(options, action) {
        return "unclear";
    }
    let answer = normalized(answer);
    if answer == "1" || answer == normalized(options[0].as_str().unwrap()) {
        "approved"
    } else if answer == "2" || answer == normalized(options[1].as_str().unwrap()) {
        "declined"
    } else {
        "unclear"
    }
}
fn skill_state(root: &Path, name: &str) -> &'static str {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::ffi::OsStrExt,
        },
    };
    let Ok(root) = CString::new(root.as_os_str().as_bytes()) else {
        return "other";
    };
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return "other";
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
    for part in [".agents", "skills", name, "SKILL.md"] {
        let c = CString::new(part).unwrap();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if part == "SKILL.md" {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), c.as_ptr(), flags) };
        if fd < 0 {
            return if part != "SKILL.md"
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
            {
                "absent"
            } else {
                "other"
            };
        }
        parent = unsafe { OwnedFd::from_raw_fd(fd) };
        if part == "SKILL.md" {
            let file = std::fs::File::from(parent);
            return if file.metadata().is_ok_and(|m| m.is_file()) {
                "skill"
            } else {
                "other"
            };
        }
    }
    "other"
}
fn validate(db: &Transaction<'_>, root: &Path, task: &str, args: &Value) -> CoreResult<Value> {
    let mut p = args["skill_proposal"].clone();
    let name = p["name"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_skill_name"))?;
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return refuse("invalid_skill_name");
    }
    let action = p["action"].as_str().unwrap_or("");
    if !safe_options(&args["options"], action) {
        return refuse("skill_proposal_options");
    }
    let evidence = p["evidence"]
        .as_array()
        .ok_or(CoreError::Refused("skill_proposal_evidence"))?;
    let refs: BTreeSet<_> = evidence.iter().map(|e| e["report_id"].as_str()).collect();
    if !(2..=5).contains(&evidence.len()) || refs.len() != evidence.len() {
        return refuse("skill_proposal_evidence");
    }
    let mut snapshots = Vec::new();
    for item in evidence {
        let id = item["report_id"]
            .as_str()
            .ok_or(CoreError::Refused("skill_proposal_evidence"))?;
        let row: Option<(String,String)> = db.query_row(
            "SELECT task_id,coalesce((SELECT title FROM editions WHERE report_id=reports.id ORDER BY sequence DESC LIMIT 1),'')
             FROM reports WHERE id=? AND kind='report' AND EXISTS(SELECT 1 FROM editions WHERE report_id=reports.id)",
            [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (source, title) = row.ok_or(CoreError::Refused("skill_proposal_evidence_unknown"))?;
        snapshots.push(json!({"report_id":id,"task_id":source,"title":title.chars().take(200).collect::<String>(),"case":item["case"]}));
    }
    let rows = db.prepare("SELECT task_id,payload_json FROM runtime_events WHERE type='USER_QUESTION_REQUESTED' AND json_extract(payload_json,'$.skill_proposal') IS NOT NULL")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (other, raw) in rows {
        let q: Value = serde_json::from_str(&raw)?;
        let status: Option<String> = db
            .query_row(
                "SELECT status FROM pending_questions WHERE id=?",
                [q["question_id"].as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if !status
            .as_deref()
            .is_some_and(|s| s == "open" || s == "answered")
        {
            continue;
        }
        if other == task {
            return refuse("skill_proposal_limit");
        }
        if q["skill_proposal"]["name"] == name {
            if status.as_deref() == Some("open") {
                return refuse("skill_proposal_pending");
            }
            if q["skill_proposal"]["action"] == action {
                return refuse("skill_proposal_already_asked");
            }
        }
    }
    let state = skill_state(root, name);
    // Descriptor-relative inspection never follows project symlink ancestry.
    if action == "create" && state != "absent" {
        return refuse("skill_proposal_exists");
    }
    if action == "update" && state != "skill" {
        return refuse("skill_proposal_missing");
    }
    p["evidence"] = json!(snapshots);
    p["observed_skill_state"] = json!(state);
    Ok(p)
}
pub fn request(
    db: &Transaction<'_>,
    root: &Path,
    task: &str,
    channel: &str,
    key: &str,
    args: &Value,
) -> CoreResult<Value> {
    request_tx(
        db,
        task,
        channel,
        key,
        "user_question_request",
        args,
        || {
            let (state, revision) = domain::task_tx(db, task)?;
            if !matches!(state, TaskState::Planning | TaskState::Reconciling) {
                return refuse("question_state");
            }
            let proposal = if args.get("skill_proposal").is_some() {
                Some(validate(db, root, task, args)?)
            } else {
                None
            };
            let forms = args["questions"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![args.clone()]);
            if forms.is_empty() || forms.len() > 8 {
                return refuse("question_batch_invalid");
            }
            // A question the owner already answered (the same text and options) is
            // not asked again: the reply carries the earlier answer
            // (`waits::already_answered_tx`). A skill proposal has its own rules.
            if proposal.is_none() {
                let asked: Vec<super::waits::QuestionForm> = forms
                    .iter()
                    .filter_map(|f| {
                        Some(super::waits::QuestionForm {
                            question: f["question"].as_str()?.to_owned(),
                            options: f["options"]
                                .as_array()?
                                .iter()
                                .filter_map(|o| o.as_str().map(str::to_owned))
                                .collect(),
                        })
                    })
                    .collect();
                if let Some(mut refused) =
                    super::waits::already_answered_tx(db, task, None, &asked)?
                {
                    refused["state"] = json!(state);
                    return Ok(refused);
                }
            }
            let mut questions = Vec::new();
            for f in forms {
                if f["question"]
                    .as_str()
                    .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 4000)
                {
                    return refuse("question_invalid");
                }
                if f.get("topic").is_some_and(|v| {
                    v.as_str()
                        .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 24)
                }) {
                    return refuse("question_topic_invalid");
                }
                let options = f["options"]
                    .as_array()
                    .ok_or(CoreError::Refused("question_options_required"))?;
                let labels: Vec<_> = options
                    .iter()
                    .map(|v| v.as_str().map(|s| s.trim().to_lowercase()))
                    .collect();
                let unique: BTreeSet<_> = labels.iter().collect();
                if !(2..=8).contains(&options.len())
                    || unique.len() != options.len()
                    || labels.iter().any(|s| {
                        s.as_ref().is_none_or(|s| {
                            s.is_empty()
                                || [
                                    "other",
                                    "other...",
                                    "other…",
                                    "type your answer",
                                    "reply in chat or type an answer under other…",
                                ]
                                .contains(&s.as_str())
                        })
                    })
                {
                    return refuse("question_options_invalid");
                }
                let id = format!("pq_{}", uuid::Uuid::new_v4().simple());
                let mut text = f["question"]
                    .as_str()
                    .ok_or(CoreError::Refused("invalid_question"))?
                    .to_owned();
                if let Some(p) = &proposal {
                    let mut header = format!(
                        "{} ({})\n",
                        p["name"].as_str().unwrap(),
                        p["action"].as_str().unwrap()
                    );
                    for e in p["evidence"].as_array().unwrap() {
                        header.push_str(&format!(
                            "- {} {}: {}\n",
                            e["report_id"].as_str().unwrap(),
                            e["title"].as_str().unwrap(),
                            e["case"].as_str().unwrap()
                        ));
                    }
                    text = format!("{header}\n{text}");
                }
                db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) VALUES (?,?,?,?,?,'open',?)",
                (&id,task,format!("{key}:{}",questions.len()),&text,f["options"].to_string(),crate::now()))?;
                questions.push(json!({"question_id":id,"question":text,"options":f["options"],"topic":f["topic"]}));
            }
            let mut saved = json!({"question_id":questions[0]["question_id"],"question":questions[0]["question"],
            "options":questions[0]["options"],"resume_state":"PLANNING","revision":revision,"channel_id":channel,"coordinator":true});
            if state == TaskState::Reconciling {
                saved["after_verification"] = json!(true);
            }
            if questions.len() > 1 {
                saved["batch_id"] = json!(format!("qb_{}", uuid::Uuid::new_v4().simple()));
                saved["question_ids"] = json!(questions
                    .iter()
                    .map(|q| q["question_id"].clone())
                    .collect::<Vec<_>>());
                saved["questions"] = json!(questions);
                event_tx(db, task, "QUESTION_BATCH_CREATED", saved.clone())?;
            }
            if let Some(p) = proposal {
                saved["skill_proposal"] = p;
            }
            event_tx(db, task, "USER_QUESTION_REQUESTED", saved.clone())?;
            domain::state_tx(db, task, TaskState::WaitingUser, None)?;
            let mut receipt = json!({"state":"WAITING_USER","question_id":saved["question_id"],
                "next":"The question and its options are shown to the user exactly as you wrote them. End this turn now without writing any further text: do not explain, restate, rewrite or summarize the question, its findings or its options. The runtime continues the task after the user answers."});
            if !saved["batch_id"].is_null() {
                receipt["batch_id"] = saved["batch_id"].clone();
                receipt["question_ids"] = saved["question_ids"].clone();
            }
            Ok(receipt)
        },
    )
}
pub fn record_decision(
    db: &Transaction<'_>,
    task: &str,
    saved: &Value,
    answer: &str,
) -> CoreResult<()> {
    if let Some(p) = saved.get("skill_proposal") {
        let decision = decide(
            &saved["options"],
            answer,
            p["action"].as_str().unwrap_or(""),
        );
        let mut result = json!({"question_id":saved["question_id"],"name":p["name"],"action":p["action"],"decision":decision,"revision":saved["revision"]});
        if decision == "approved" {
            result["request"] = json!(format!("{} the project skill {} in .agents/skills/{} and expose it to Claude through project_skill_link. Purpose: {}",
                if p["action"]=="create" {"Create"}else{"Update"},p["name"].as_str().unwrap(),p["name"].as_str().unwrap(),p["purpose"].as_str().unwrap()));
        }
        event_tx(db, task, "SKILL_PROPOSAL_DECIDED", result)?;
    }
    Ok(())
}
/// Retained consent remains visible after restart; it never dispatches work itself.
pub fn planning_instruction(db: &rusqlite::Connection, task: &str) -> CoreResult<String> {
    let latest: Option<(i64,String)> = db.query_row("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='SKILL_PROPOSAL_DECIDED' ORDER BY id DESC LIMIT 1",[task],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((id, raw)) = latest else {
        return Ok(String::new());
    };
    let v: Value = serde_json::from_str(&raw)?;
    let planned: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type='PLAN_ACCEPTED'",
        [task],
        |r| r.get(0),
    )?;
    if v["decision"] == "approved" && planned < id {
        Ok(format!("The user approved this exact skill proposal. /skill-management {}. Read the coordinator-only shared skill skill-management using read_skill before planning. Treat the supplied request as the complete skill-management request. This workflow is active only because the user approved this exact skill proposal; ordinary prose does not activate it. Plan only this skill work.",v["request"].as_str().unwrap()))
    } else if v["decision"] != "approved" {
        Ok(format!("The user did not approve the proposed project skill {} ({}). Do not create, update or link it and do not propose it again; leave the project unchanged for it. Otherwise close with answer_submit.",v["name"].as_str().unwrap(),v["decision"].as_str().unwrap()))
    } else {
        Ok(String::new())
    }
}
/// Claim once before delivery. Unknown deliveries never replay automatically.
pub fn continuation(
    db: &Transaction<'_>,
    task: &str,
    channel: &str,
    claim: bool,
) -> CoreResult<Option<String>> {
    let row: Option<String> = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED' AND json_extract(payload_json,'$.channel_id')=? ORDER BY id DESC LIMIT 1",
        (task,channel),|r|r.get(0)).optional()?;
    let Some(raw) = row else {
        return Ok(None);
    };
    let q: Value = serde_json::from_str(&raw)?;
    let id = q["question_id"].as_str().unwrap();
    let used: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='COORDINATOR_ANSWER_CLAIMED' AND json_extract(payload_json,'$.question_id')=?)",(task,id),|r|r.get(0))?;
    if used {
        return Ok(None);
    }
    if domain::task_tx(db, task)?.0 == TaskState::WaitingUser {
        return Ok(Some("open".into()));
    }
    let answer: Option<String> = db
        .query_row(
            "SELECT answer_text FROM pending_questions WHERE id=? AND status='answered'",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(answer) = answer else {
        return Ok(None);
    };
    if !claim {
        return Ok(Some("ready".into()));
    }
    event_tx(
        db,
        task,
        "COORDINATOR_ANSWER_CLAIMED",
        json!({"question_id":id,"channel_id":channel}),
    )?;
    let mut text =
        format!("Recorded owner answer: {answer}. Continue planning from retained state.");
    if let Some(p) = q.get("skill_proposal") {
        let decision = decide(&q["options"], &answer, p["action"].as_str().unwrap_or(""));
        if decision == "approved" {
            let request: String=db.query_row("SELECT json_extract(payload_json,'$.request') FROM runtime_events WHERE task_id=? AND type='SKILL_PROPOSAL_DECIDED' AND json_extract(payload_json,'$.question_id')=? ORDER BY id DESC LIMIT 1",(task,id),|r|r.get(0))?;
            text.push_str(&format!(" The user approved this exact skill proposal. /skill-management {request}. Read the coordinator-only shared skill skill-management using read_skill before planning. Treat the supplied request as the complete skill-management request. This workflow is active only because the user approved this exact skill proposal; ordinary prose does not activate it. Plan only this skill work."));
        } else {
            text.push_str(&format!(" The user did not approve the proposed project skill ({decision}). Do not create, update or link it and do not propose it again; leave the project unchanged for it. Otherwise close with answer_submit."));
        }
    } else if let Some(forms) = q["questions"].as_array() {
        for f in forms {
            let a: String = db.query_row(
                "SELECT answer_text FROM pending_questions WHERE id=?",
                [f["question_id"].as_str()],
                |r| r.get(0),
            )?;
            text.push_str(&format!("\n{}: {a}", f["question"].as_str().unwrap()));
        }
    }
    Ok(Some(text))
}

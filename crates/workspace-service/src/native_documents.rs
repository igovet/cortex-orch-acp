//! Channel-bound publication and bounded discovery over the existing Core domains.
//! Executed on the serial publication lane; never exposes private store paths as authority.
use cortex_core::{
    channels::{self, Credential, Identity},
    lifecycle::{self, waits},
    store::documents::{ContentPage, Publication, Publisher},
    CoreError, CoreResult, Store,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn supports(name: &str) -> bool {
    matches!(
        name,
        "create_draft"
            | "draft_write"
            | "write_report"
            | "list_reports"
            | "read_report"
            | "read_draft"
            | "tools_discover"
    )
}
fn text<'a>(args: &'a Value, key: &str) -> CoreResult<&'a str> {
    args[key]
        .as_str()
        .ok_or(CoreError::Refused("invalid_arguments"))
}
pub(super) fn authenticate(store: &Store, c: &Credential, expected: &Identity) -> CoreResult<()> {
    store.read(|db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if &i != expected {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        Ok(())
    })
}
fn catalogue(i: &Identity) -> CoreResult<Vec<Value>> {
    Ok(
        cortex_engine::schema::catalogue(&i.role, i.profile.as_deref(), i.stage.as_deref())
            .map_err(|_| CoreError::Refused("tool_profile_unavailable"))?
            .into_iter()
            .filter(|t| {
                crate::native_instructions::supports(
                    i.stage.as_deref(),
                    t["name"].as_str().unwrap_or(""),
                )
            })
            .collect(),
    )
}
pub(super) fn validate(i: &Identity, name: &str, args: &Value) -> CoreResult<()> {
    let tools = catalogue(i)?;
    let tool = tools
        .iter()
        .find(|t| t["name"] == name)
        .ok_or(CoreError::Refused("tool_refused"))?;
    cortex_engine::schema::validate(&tool["inputSchema"], args)
        .map_err(|_| CoreError::Refused("invalid_arguments"))?;
    // Redaction of newly captured native input is a distinct, unfinished domain.
    // Never accept credentials here and then silently retain them unchanged.
    if args["redact_values"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        return Err(CoreError::Refused("native_input_redaction_unavailable"));
    }
    Ok(())
}
/// The attachment descriptors (kind, type, name, size, digest, `source_ref`;
/// never bytes) of the newest source revision recorded for this document, as
/// the former runtime reported them. Only captured user input has a source
/// revision; an ordinary report or the pipeline has none, so its list is empty.
fn source_attachments(store: &Store, task: &str, report: &str) -> CoreResult<Value> {
    let raw: Option<String> = store.read(|db| {
        Ok(db
            .query_row(
                "SELECT attachments FROM source_revisions WHERE task_id=? AND report_id=? ORDER BY revision DESC LIMIT 1",
                (task, report),
                |r| r.get(0),
            )
            .optional()?)
    })?;
    Ok(raw
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(|| json!([])))
}
fn page(id_key: &str, id: &str, p: ContentPage) -> Value {
    let mut v = json!({"markdown":p.markdown,"sha256":p.sha256,
        "total_characters":p.total_characters,"next_cursor":p.next_cursor});
    v[id_key] = json!(id);
    v
}
fn report_id(args: &Value) -> CoreResult<String> {
    if let Some(id) = args["report_id"].as_str() {
        return Ok(id.into());
    }
    let parts: Vec<_> = text(args, "cursor")?.split('.').collect();
    if parts.len() != 5
        || parts[0] != "cr"
        || parts[2].len() != 12
        || !parts[2].bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(CoreError::Refused("invalid_cursor"));
    }
    // Publisher validates the task, selected report and content fingerprint.
    Ok(format!("r_{}", parts[2]))
}
fn discover(i: &Identity, args: &Value) -> CoreResult<Value> {
    let tools = catalogue(i)?;
    let revision = digest(&serde_json::to_vec(&tools)?);
    let binding = digest(&serde_json::to_vec(&(
        &i.task,
        &i.principal,
        &i.attempt,
        i.epoch,
    ))?);
    if let Some(name) = args["name"].as_str() {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .ok_or(CoreError::Refused("tool_not_available"))?;
        let result = json!({"catalog_revision":revision,"tool":tool});
        return Ok(result);
    }
    let mut index = 0;
    if let Some(cursor) = args["cursor"].as_str() {
        let p: Vec<_> = cursor.split('.').collect();
        if p.len() != 4 || p[0] != "ntd1" || p[1] != &binding[..24] {
            return Err(CoreError::Refused("invalid_cursor"));
        }
        if p[2] != &revision[..24] {
            return Err(CoreError::Refused("discovery_cursor_stale"));
        }
        index = p[3]
            .parse::<usize>()
            .map_err(|_| CoreError::Refused("invalid_cursor"))?;
        if index >= tools.len() || index.to_string() != p[3] {
            return Err(CoreError::Refused("invalid_cursor"));
        }
    }
    let limit = args["limit"].as_u64().unwrap_or(20) as usize;
    let mut entries = Vec::new();
    let mut next = Value::Null;
    while index < tools.len() && entries.len() < limit {
        let t = &tools[index];
        let description = t["description"].as_str().unwrap_or("");
        let entry = json!({"name":t["name"],"title":t["title"],
            "description":description.chars().take(160).collect::<String>(),
            "description_truncated":description.chars().count()>160,
            "read_batch_supported":crate::native_batch::READS.contains(&t["name"].as_str().unwrap_or(""))});
        let candidate_next = if index + 1 < tools.len() {
            json!(format!(
                "ntd1.{}.{}.{}",
                &binding[..24],
                &revision[..24],
                index + 1
            ))
        } else {
            Value::Null
        };
        entries.push(entry);
        let candidate =
            json!({"catalog_revision":revision,"tools":entries,"next_cursor":candidate_next});
        if serde_json::to_vec(&candidate)?.len() > 16384 {
            entries.pop();
            break;
        }
        next = candidate_next;
        index += 1;
    }
    if entries.is_empty() && index < tools.len() {
        return Err(CoreError::Refused("schema_too_large"));
    }
    Ok(json!({"catalog_revision":revision,"tools":entries,"next_cursor":next}))
}
fn read(store: &Store, i: &Identity, name: &str, args: &Value) -> CoreResult<Value> {
    let publisher = Publisher::new(store);
    let count = args["limit"]
        .as_u64()
        .unwrap_or(if name == "list_reports" { 25 } else { 4000 }) as usize;
    let cursor = args["cursor"].as_str();
    match name {
        "read_report" => {
            let start = args["start"].as_u64();
            if start.is_some() && (cursor.is_some() || args["report_id"].as_str().is_none()) {
                return Err(CoreError::Refused("invalid_arguments"));
            }
            // Without report_id and cursor the read starts at the newest beginning
            // of this task's current pipeline.
            let id = if args.get("report_id").is_none() && args.get("cursor").is_none() {
                store
                    .read(|db| {
                        Ok(db
                            .query_row(
                                "SELECT id FROM reports WHERE task_id=? AND kind='pipeline' ORDER BY rowid DESC LIMIT 1",
                                [&i.task],
                                |r| r.get::<_, String>(0),
                            )
                            .optional()?)
                    })?
                    .ok_or(CoreError::Refused("pipeline_missing"))?
            } else {
                report_id(args)?
            };
            let p = match start {
                Some(start) => publisher.immutable_report_from(&i.task, &id, start, count)?,
                None => publisher.read_report(&i.task, &id, cursor, count)?,
            };
            let mut value = page("report_id", &id, p);
            let metadata = store.read(|db| {
                Ok(db.query_row(
                    "SELECT e.title,e.summary,e.author,r.kind,e.size_bytes,
                    coalesce(p.source_revision,0),coalesce(p.artifacts,'[]')
                    FROM reports r JOIN editions e ON e.report_id=r.id
                    LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence
                    WHERE r.id=? AND r.task_id=? ORDER BY e.sequence DESC LIMIT 1",
                    (&id, &i.task),
                    |r| {
                        Ok(json!({"title":r.get::<_,String>(0)?,
                        "summary":r.get::<_,String>(1)?,"author":r.get::<_,String>(2)?,
                        "kind":r.get::<_,String>(3)?,"size_bytes":r.get::<_,i64>(4)?,
                        "source_revision":r.get::<_,i64>(5)?,
                        "artifacts":r.get::<_,String>(6)?}))
                    },
                )?)
            })?;
            if !matches!(metadata["kind"].as_str(), Some("report" | "pipeline")) {
                return Err(CoreError::Refused("use_read_source"));
            }
            let (created, updated) = store.read(|db| {
                Ok(db.query_row(
                "SELECT r.created_at,e.updated_at FROM reports r JOIN editions e ON e.report_id=r.id
                 WHERE r.id=? AND r.task_id=? ORDER BY e.sequence DESC LIMIT 1",
                (&id,&i.task),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?)
            })?;
            value["created_at"] = json!(created);
            value["updated_at"] = json!(updated);
            value["source_attachments"] = source_attachments(store, &i.task, &id)?;
            for (k, v) in metadata.as_object().unwrap() {
                value[k] = v.clone();
            }
            value["artifacts"] = serde_json::from_str(text(&metadata, "artifacts")?)?;
            Ok(value)
        }
        "read_draft" => {
            let id = text(args, "draft_id")?;
            let mut value = page(
                "draft_id",
                id,
                publisher.read_draft(&i.task, &i.principal, id, cursor, count)?,
            );
            let (path,kind,template)=store.read(|db|Ok(db.query_row(
                "SELECT path,kind,template FROM drafts WHERE id=? AND task_id=? AND owner_thread_id=?",
                (id,&i.task,&i.principal),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?))?;
            value["draft_path"] = json!(path);
            value["kind"] = json!(kind);
            value["template"] = json!(template);
            Ok(value)
        }
        "list_reports" => {
            let reports = publisher.list(&i.task, cursor, count)?;
            let mut metadata = serde_json::to_value(&reports.reports)?;
            for report in metadata.as_array_mut().unwrap() {
                let (created,updated)=store.read(|db|Ok(db.query_row(
                    "SELECT r.created_at,e.updated_at FROM reports r JOIN editions e ON e.report_id=r.id
                     WHERE r.id=? AND r.task_id=? ORDER BY e.sequence DESC LIMIT 1",
                    (text(report,"report_id")?,&i.task),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?))?;
                report["created_at"] = json!(created);
                report["updated_at"] = json!(updated);
                let attachments = source_attachments(store, &i.task, text(report, "report_id")?)?;
                report["source_attachments"] = attachments;
            }
            let drafts = publisher.own_drafts(
                &i.task,
                &i.principal,
                args["drafts_after"].as_str().unwrap_or(""),
                count,
            )?;
            let more_drafts = drafts.len() == count
                && !publisher
                    .own_drafts(&i.task, &i.principal, drafts.last().unwrap(), 1)?
                    .is_empty();
            let drafts_next = if more_drafts {
                drafts.last().cloned()
            } else {
                None
            };
            let mut own_drafts = Vec::new();
            for id in &drafts {
                own_drafts.push(store.read(|db|Ok(db.query_row(
                    "SELECT path,kind,template,created_at FROM drafts WHERE id=? AND task_id=? AND owner_thread_id=?",
                    (id,&i.task,&i.principal),|r|Ok(json!({"draft_id":id,
                        "draft_path":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,
                        "template":r.get::<_,String>(2)?,"created_at":r.get::<_,String>(3)?})))?))?);
            }
            let after = args["changes_after"].as_i64().unwrap_or(0);
            let mut changes = store.read(|db| {
                Ok(db.prepare("SELECT sequence,kind,reference,created_at FROM task_changes
                    WHERE task_id=? AND sequence>? ORDER BY sequence LIMIT ?")?
                    .query_map((&i.task,after,(count+1) as i64), |r|Ok(json!({
                        "sequence":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,
                        "reference":r.get::<_,Option<String>>(2)?,"created_at":r.get::<_,String>(3)?,"observation":null})))?
                    .collect::<Result<Vec<_>,_>>()?)
            })?;
            let more_changes = changes.len() > count;
            changes.truncate(count);
            let changes_next = if more_changes {
                changes.last().unwrap()["sequence"].clone()
            } else {
                Value::Null
            };
            Ok(json!({"reports":metadata,"next_cursor":reports.next_cursor,
                "own_drafts":own_drafts,"drafts_next":drafts_next,
                "changes":changes,"changes_next":changes_next}))
        }
        "tools_discover" => discover(i, args),
        _ => Err(CoreError::Refused("tool_refused")),
    }
}
fn finish(store: &Store, i: &Identity, name: &str, mut value: Value) -> CoreResult<Value> {
    let tools = catalogue(i)?;
    let tool = tools
        .iter()
        .find(|t| t["name"] == name)
        .ok_or(CoreError::Refused("tool_refused"))?;
    if let Some(schema) = tool.get("outputSchema") {
        let (revision,governance)=store.read(|db| {
            let revision:i64=db.query_row("SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
                [&i.task],|r|r.get(0))?;
            let governance=db.query_row("SELECT id,mode,report_id FROM governance WHERE task_id=? ORDER BY rowid DESC LIMIT 1",
                [&i.task],|r|Ok(json!({"status":"selected","governance_id":r.get::<_,String>(0)?,
                    "mode":r.get::<_,String>(1)?,"report_id":r.get::<_,String>(2)?}))).optional()?
                .unwrap_or(json!({"status":"unset","governance_id":null,"mode":null,"report_id":null}));
            Ok((revision,governance))
        })?;
        value["binding"] = json!({"receipt":"channel-managed","thread_id":"channel-managed","parent_thread_id":null,"state":"cortex"});
        value["source_capture"] = json!({"status":"not_attempted","reason":"native_prompt_capture_only","revision":revision,"pending_turns":0});
        value["governance"] = governance;
        cortex_engine::schema::validate(schema, &value)
            .map_err(|_| CoreError::Refused("native_output_schema_mismatch"))?;
    }
    Ok(value)
}
/// The document tools that only read: run on the reader pool (see [`read_tool`]).
pub(super) fn read_only(name: &str) -> bool {
    matches!(
        name,
        "read_report" | "read_draft" | "list_reports" | "tools_discover"
    )
}
/// The observable, channel-bound receipt of one successful report page. Review
/// completion uses these receipts to prove that the reviewer actually consumed every
/// predecessor report through its final page; prose in the result is not evidence.
pub(super) struct PageReceipt {
    task: String,
    payload: Value,
    required: Option<(Credential, Identity)>,
}
impl PageReceipt {
    pub(super) fn required(&self) -> bool {
        self.required.is_some()
    }
    pub(super) fn record(&self, db: &rusqlite::Transaction<'_>) -> CoreResult<()> {
        if let Some((credential, identity)) = &self.required {
            let current = channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
            if current != *identity {
                return Err(CoreError::Refused("channel_identity_changed"));
            }
            let duplicate: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REPORT_PAGE_READ'
                 AND actor='runtime' AND payload_json=?)",
                (&self.task, self.payload.to_string()), |r| r.get(0),
            )?;
            if duplicate {
                return Ok(());
            }
        }
        cortex_core::lifecycle::event_tx(db, &self.task, "REPORT_PAGE_READ", self.payload.clone())?;
        Ok(())
    }
}
/// An authenticated, schema-checked read tool, with no write of its own: the reply
/// before any oversized-output deferral, and the page receipt the caller records
/// (`read_report`). On a reader-pool store, an operation that needs a write (a crash
/// reconciliation of a publication) refuses `storage_read_lane` and is repeated on
/// the writer, where repeating it is harmless.
pub(super) fn read_tool(
    store: &Store,
    c: &Credential,
    i: &Identity,
    name: &str,
    args: &Value,
) -> CoreResult<(Value, Option<PageReceipt>)> {
    authenticate(store, c, i)?;
    validate(i, name, args)?;
    let value = read(store, i, name, args)?;
    authenticate(store, c, i)?;
    let value = finish(store, i, name, value)?;
    let report_id = value["report_id"].as_str().unwrap_or_default();
    let required = name == "read_report" && i.assignment.as_deref()
        .map(|assignment| store.read(|db| cortex_core::required_reports::for_assignment(db, &i.task, assignment))
            .map(|reports| reports.iter().any(|r| r["report_id"] == report_id)))
        .transpose()?.unwrap_or(false);
    if required && serde_json::to_vec(&value)?.len() > 32768 {
        return Err(CoreError::Refused("required_report_page_too_large"));
    }
    let start = if name == "read_report" {
        args["cursor"].as_str().map(cortex_core::store::documents::report_cursor_offset)
            .transpose()?.or(args["start"].as_u64()).unwrap_or(0)
    } else { 0 };
    let receipt = (name == "read_report" && !report_id.is_empty()).then(|| PageReceipt {
        task: i.task.clone(),
        required: required.then(|| (c.clone(), i.clone())),
        payload: json!({
            "channel_id": c.channel_id,
            "assignment_ref": i.assignment,
            "attempt": i.attempt,
            "report_id": report_id,
            "cursor": args["cursor"],
            "next_cursor": value["next_cursor"],
            "sha256": value["sha256"],
            "start_character": start,
            "end_character": start + value["markdown"].as_str().unwrap_or_default().chars().count() as u64,
            "required_inline": required,
            "total_characters": value["total_characters"]
        }),
    });
    Ok((value, receipt))
}
/// Authenticated, schema-checked operation; no filesystem effect crosses a SQL transaction.
pub(super) fn execute(
    store: &Store,
    c: &Credential,
    i: &Identity,
    name: &str,
    args: &Value,
) -> CoreResult<Value> {
    if read_only(name) {
        return execute_read(store, c, i, name, args, false);
    }
    authenticate(store, c, i)?;
    validate(i, name, args)?;
    let generated = crate::identity()?;
    let key = args["request_key"].as_str().unwrap_or(&generated);
    let mut fresh = false;
    let intent = store.write(|db| {
        let current = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if &current != i {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        if let Some(a) = &i.assignment {
            waits::check_mutation_tx(db, &i.task, a)?;
        }
        if name == "write_report" {
            let typed: Vec<_> = args["artifacts"].as_array().into_iter().flatten()
                .filter(|a| a.get("kind").is_some())
                .map(|a| cortex_core::deliverables::artifact(a)
                    .ok_or(CoreError::Refused("invalid_deliverable_artifact")))
                .collect::<Result<_, _>>()?;
            if !typed.is_empty() {
                if i.role != "worker" || i.stage.as_deref() != Some("execution") {
                    return Err(CoreError::Refused("deliverable_publication_not_expected"));
                }
                let contract = cortex_core::deliverables::runtime_contract(db, &i.task)?
                    .ok_or(CoreError::Refused("deliverable_not_declared"))?;
                let declared = cortex_core::deliverables::outputs(&contract)
                    .ok_or(CoreError::Refused("invalid_deliverable_contract"))?;
                if typed.iter().any(|v| !declared.contains(v)) {
                    return Err(CoreError::Refused("deliverable_not_declared"));
                }
            }
        }
        lifecycle::request_tx(db, &i.task, &c.channel_id, key, name, args, || {
            fresh = true;
            let revision: i64 = db.query_row(
                "SELECT coalesce(max(revision),0)
                FROM source_revisions WHERE task_id=?",
                [&i.task],
                |r| r.get(0),
            )?;
            Ok(json!({"status":"pending","source_revision":revision}))
        })
    })?;
    if intent["status"] == "complete" {
        let mut result = intent["result"].clone();
        if result.get("replayed").is_some() {
            result["replayed"] = json!(true);
        }
        return finish(store, i, name, result);
    }
    // A draft replacement may have happened before a crash. Never overwrite a
    // newer draft by replaying an ambiguous write; keep the intent for diagnosis.
    if !fresh && name == "draft_write" {
        return Err(CoreError::Refused("native_draft_write_outcome_unknown"));
    }
    let publisher = Publisher::new(store);
    let result = match name {
        "create_draft" => {
            let d = publisher.create_draft(&i.task, &i.principal, text(args, "template")?, key)?;
            let markers: Vec<_> = d
                .markdown
                .lines()
                .filter(|s| {
                    s.starts_with("<!--") && s.ends_with("-->")
                        || s.starts_with("{{") && s.ends_with("}}")
                })
                .collect();
            json!({"draft_id":d.draft_id,"draft_path":d.draft_path,"kind":d.kind,
                "template":args["template"],"markdown":d.markdown,
                "required_first_line":d.markdown.lines().next().unwrap_or(""),
                "edit_instruction":"Fill this owned draft with draft_write. Preserve required_first_line, replace every listed placeholder and retain complete UTF-8 Markdown. The runtime owns pipeline editions.",
                "replaceable_markers":markers,"required_replacement_count":markers.len(),
                "total_characters":d.markdown.chars().count(),"sha256":digest(d.markdown.as_bytes()),
                "replayed":!fresh})
        }
        "draft_write" => {
            publisher.write_draft(
                &i.task,
                &i.principal,
                text(args, "draft_id")?,
                text(args, "content")?,
            )?;
            json!({"draft_id":args["draft_id"],"outcome":"written"})
        }
        "write_report" => {
            let id = text(args, "draft_id")?;
            let revision = args["source_revision"].as_i64().unwrap_or(store.read(|db| {
                Ok(db.query_row(
                    "SELECT source_revision FROM draft_revisions WHERE draft_id=?",
                    [id],
                    |r| r.get(0),
                )?)
            })?);
            serde_json::to_value(publisher.publish(
                &i.task,
                &i.principal,
                id,
                key,
                Publication {
                    title: text(args, "title")?,
                    summary: text(args, "summary")?,
                    author: text(args, "author")?,
                    source_revision: revision,
                    artifacts: args.get("artifacts").cloned().unwrap_or(json!([])),
                },
            )?)?
        }
        _ => return Err(CoreError::Refused("tool_refused")),
    };
    let mut result = result;
    if name == "write_report" {
        result["summary"] = args["summary"].clone();
        result["replayed"] = json!(!fresh);
    }
    let result = finish(store, i, name, result)?;
    store.write(|db| {
        let current = channels::authenticate(db,&c.channel_id,c.secret(),true)?;
        if &current != i { return Err(CoreError::Refused("channel_identity_changed")); }
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?",
            (json!({"status":"complete","complete":true,"result":result}).to_string(),&i.task,&c.channel_id,key))?;
        lifecycle::event_tx(db,&i.task,"NATIVE_DOCUMENT_TOOL",json!({
            "name":name,"channel_id":c.channel_id,"request_key":key}))?;
        Ok(())
    })?;
    Ok(result)
}

pub(super) fn execute_read(
    store: &Store, c: &Credential, i: &Identity, name: &str, args: &Value, batched: bool,
) -> CoreResult<Value> {
    let (value, receipt) = read_tool(store, c, i, name, args)?;
    if let Some(receipt) = receipt {
        if receipt.required() && batched {
            return Err(CoreError::Refused("required_report_direct_read"));
        }
        store.write(|db| receipt.record(db))?;
    }
    crate::native_commands::defer_json(store, i, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        Identity {
            read_only: false,
            task: "t".into(),
            attempt: "a".into(),
            principal: "p".into(),
            epoch: 1,
            role: "coordinator".into(),
            profile: Some("orchestrator".into()),
            stage: None,
            assignment: None,
            session: Some("s".into()),
        }
    }
    #[test]
    fn discovery_pages_are_complete_channel_bound_and_match_effective_records() {
        let i = identity();
        let expected = catalogue(&i).unwrap();
        let mut names = Vec::new();
        let mut args = json!({"limit":1});
        let first = discover(&i, &args).unwrap();
        let first_cursor = first["next_cursor"].as_str().unwrap().to_owned();
        loop {
            let page = discover(&i, &args).unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len() <= 16384);
            for t in page["tools"].as_array().unwrap() {
                names.push(t["name"].clone());
                let exact = discover(&i, &json!({"name":t["name"]})).unwrap();
                assert_eq!(
                    exact["tool"],
                    *expected.iter().find(|e| e["name"] == t["name"]).unwrap()
                );
                assert!(serde_json::to_vec(&exact).unwrap().len() <= 32768);
            }
            let Some(next) = page["next_cursor"].as_str() else {
                break;
            };
            args = json!({"limit":1,"cursor":next});
        }
        assert_eq!(
            names,
            expected
                .iter()
                .map(|t| t["name"].clone())
                .collect::<Vec<_>>()
        );
        let mut other = i.clone();
        other.attempt = "other".into();
        assert_eq!(
            discover(&other, &json!({"cursor":first_cursor}))
                .unwrap_err()
                .code(),
            "invalid_cursor"
        );
        let stale = first_cursor
            .split('.')
            .enumerate()
            .map(|(n, s)| {
                if n == 2 {
                    "000000000000000000000000"
                } else {
                    s
                }
            })
            .collect::<Vec<_>>()
            .join(".");
        assert_eq!(
            discover(&i, &json!({"cursor":stale})).unwrap_err().code(),
            "discovery_cursor_stale"
        );
        assert!(discover(&i, &json!({"name":"command_run"})).is_err());
    }
}

#[cfg(test)]
mod required_read_tests {
    use super::*;
    use cortex_core::{
        lifecycle::{completion, plan, FoundationGuards, TaskMachine},
        OpenMode, ProjectKey, StoreChannel, TaskState,
    };

    #[test]
    fn exact_required_read_survives_reopen_but_not_a_replacement_attempt() {
        let tmp = std::env::temp_dir().join(format!("required-reading-{}", crate::identity().unwrap()));
        std::fs::create_dir(&tmp).unwrap();
        let key = ProjectKey::new(&tmp, StoreChannel::Dev).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        let machine = TaskMachine::new(&store);
        let owner = machine.create("t", "Use the input", "owner").unwrap();
        machine.transition("t", &owner, "planning", TaskState::Planning, None, &FoundationGuards).unwrap();
        let publish = |store: &Store, author: &str, text: &str| {
            let publisher = Publisher::new(store);
            let draft = publisher.create_draft("t", author, "general", author).unwrap();
            let body = format!("{}\n\n{text}", draft.markdown.lines().next().unwrap());
            publisher.write_draft("t", author, &draft.draft_id, &body).unwrap();
            publisher.publish("t", author, &draft.draft_id, author, Publication {
                title: "Required input", summary: "Observed result", author,
                source_revision: 0, artifacts: json!([]),
            }).unwrap()
        };
        let seed = publish(&store, "seed", "Established owner input");
        let report = publish(&store, "producer", &"éЖ".repeat(2500));
        let plan = json!({
            "outcomes":[{"id":"used","statement":"The reports are used"}],
            "verification":[["test","-d","."]],
            "tasks":[
                {"id":"producer","role":"explorer","objective":"Create source data","scope":["."],"dependencies":[],"competencies":["data"]},
                {"id":"section","role":"explorer","objective":"Use part of the source","scope":["."],"dependencies":["producer"],"delivers":["used"],"competencies":["data"],
                    "required_reports":[{"from_node":"producer","section":{"start":45,"end":65}},{"report_id":seed.report_id,"sha256":seed.sha256}]},
                {"id":"whole","role":"explorer","objective":"Inspect entire source","scope":["."],"dependencies":["producer"],"competencies":["data"],
                    "required_reports":[{"from_node":"producer"}]}
            ]
        });
        let mut wrong_dependency = plan.clone();
        wrong_dependency["tasks"][1]["dependencies"] = json!([]);
        assert_eq!(plan::validate(&wrong_dependency).unwrap_err().code(), "invalid_required_reports");
        let mut wrong_digest = plan.clone();
        wrong_digest["tasks"][1]["required_reports"][1]["sha256"] = json!("0".repeat(64));
        assert_eq!(store.write(|db|plan::accept_tx(db, "t","coord","wrong", &wrong_digest)).unwrap_err().code(), "required_report_unavailable");
        store.write(|db| {
            plan::accept_tx(db, "t", "coord", "plan", &plan)?;
            db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'", [])?;
            db.execute("UPDATE runtime_nodes SET status='running',assignment_ref=node_id WHERE task_id='t'", [])?;
            Ok(())
        }).unwrap();
        let issue = |store: &Store, assignment: &str| store.write(|db| {
            let c = channels::issue_tx(db, "t", &owner, channels::Principal::Worker{assignment})?;
            channels::bind_session_tx(db, &c, assignment)?;
            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
            Ok((c, i))
        }).unwrap();
        let (producer, _) = issue(&store, "producer");
        let record = |store: &Store, assignment: &str, published: &cortex_core::store::documents::Published| store.write(|db| {
            db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
                VALUES ('t','worker_result',?,?,?,'native-runtime','now')",
                (assignment,format!("record:{assignment}"),json!({"report_ref":published.report_id}).to_string()))?;
            let mut snapshot = completion::EvidenceSnapshot::default();
            snapshot.artifacts.insert(published.report_id.clone(), published.sha256.clone());
            completion::record_result_tx(db, "t", assignment,
                &json!({"report_ref":published.report_id,"summary":"Completed","checks":[]}), &snapshot)
        });
        record(&store, "producer", &report).unwrap();
        drop(producer);
        let (section, section_identity) = issue(&store, "section");
        let (whole, whole_identity) = issue(&store, "whole");
        let selected = store.read(|db| cortex_core::assignment_brief::extras(db,"t","section","worker-explorer","execution","en")).unwrap();
        assert_eq!(selected["required_reports"][0]["report_id"], report.report_id);
        assert_eq!(selected["required_reports"][0]["sha256"], report.sha256);
        let unread = |store: &Store, channel: &Credential, i: &Identity| {
            crate::native_results::outstanding(store, channel, i, &json!({"checks":[]})).unwrap()
        };
        let read = |store: &Store, channel: &Credential, i: &Identity, args: Value| {
            execute(store,channel,i,"read_report",&args).unwrap()
        };
        assert_eq!(unread(&store,&section,&section_identity).unwrap().0, "required_reports_unread");
        // Reading only the end using a real positioned read is not a whole-report chain.
        assert!(read(&store,&whole,&whole_identity,json!({"report_id":report.report_id,"start":4000}))["next_cursor"].is_null());
        assert!(unread(&store,&whole,&whole_identity).is_some());
        let first = read(&store,&whole,&whole_identity,json!({"report_id":report.report_id}));
        assert!(first["next_cursor"].is_string());
        read(&store,&whole,&whole_identity,json!({"cursor":first["next_cursor"]}));
        assert!(unread(&store,&whole,&whole_identity).is_none());
        assert!(unread(&store,&section,&section_identity).is_some());

        read(&store,&section,&section_identity,json!({"report_id":seed.report_id}));
        read(&store,&section,&section_identity,json!({"report_id":report.report_id,"start":60,"limit":5}));
        read(&store,&section,&section_identity,json!({"report_id":report.report_id,"start":60,"limit":5}));
        assert!(unread(&store,&section,&section_identity).is_some());
        let first = read(&store,&section,&section_identity,json!({"report_id":report.report_id,"start":45,"limit":10}));
        assert_eq!(first["markdown"].as_str().unwrap().chars().count(), 10);
        read(&store,&section,&section_identity,json!({"cursor":first["next_cursor"],"limit":10}));
        assert!(unread(&store,&section,&section_identity).is_none());
        drop(store);
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        assert!(unread(&store,&section,&section_identity).is_none());
        let filename = store.read(|db| Ok(db.query_row("SELECT filename FROM reports WHERE id=?", [&seed.report_id], |r| r.get::<_,String>(0))?)).unwrap();
        let path = store.key().directory().join("t").join(filename);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(unread(&store,&section,&section_identity).is_some());
        std::fs::write(&path, original).unwrap();
        store.write(|db| {
            channels::revoke_tx(db, &section.channel_id)?;
            db.execute("UPDATE runtime_nodes SET assignment_ref='replacement' WHERE task_id='t' AND node_id='section'", [])?;
            Ok(())
        }).unwrap();
        let (replacement, replacement_identity) = issue(&store, "replacement");
        assert!(unread(&store,&replacement,&replacement_identity).is_some());
        let submission = json!({
            "request_key":"selected-material","summary":"Used the exact selected source","checks":[],
            "objective_status":"complete",
            "report":"## Evidence examined\n\nRead the selected source.\n\n## Findings\n\nUsed the evidence.\n\n## Gaps and next steps\n\nNone."
        });
        validate(&replacement_identity, "result_submit", &submission).unwrap();
        let submit = || crate::native_results::submit(
            &store, &replacement, &replacement_identity, &submission, completion::EvidenceSnapshot::default(),
        );
        assert_eq!(submit().unwrap_err().code(), "required_reports_unread");
        let published_count = || store.read(|db| Ok(db.query_row("SELECT count(*) FROM reports WHERE task_id='t'", [], |r| r.get::<_,i64>(0))?)).unwrap();
        assert_eq!(published_count(),2);
        read(&store,&replacement,&replacement_identity,json!({"report_id":seed.report_id}));
        read(&store,&replacement,&replacement_identity,json!({"report_id":report.report_id,"start":45,"limit":20}));
        assert!(unread(&store,&replacement,&replacement_identity).is_none());
        assert_eq!(submit().unwrap()["outcome"], "recorded");
        assert_eq!(published_count(),3);
        drop(store);
        std::fs::remove_dir_all(tmp).unwrap();
    }
}

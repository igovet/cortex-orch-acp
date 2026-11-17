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
fn authenticate(store: &Store, c: &Credential, expected: &Identity) -> CoreResult<()> {
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
fn validate(i: &Identity, name: &str, args: &Value) -> CoreResult<()> {
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
            "read_batch_supported":false});
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
            let id = report_id(args)?;
            let p = publisher.read_report(&i.task, &id, cursor, count)?;
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
            value["source_attachments"] = json!([]);
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
                report["source_attachments"] = json!([]);
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
/// Authenticated, schema-checked operation; no filesystem effect crosses a SQL transaction.
pub(super) fn execute(
    store: &Store,
    c: &Credential,
    i: &Identity,
    name: &str,
    args: &Value,
) -> CoreResult<Value> {
    authenticate(store, c, i)?;
    validate(i, name, args)?;
    if matches!(
        name,
        "read_report" | "read_draft" | "list_reports" | "tools_discover"
    ) {
        let value = read(store, i, name, args)?;
        authenticate(store, c, i)?;
        let value = finish(store, i, name, value)?;
        return crate::native_commands::defer_json(store, i, value);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        Identity {
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

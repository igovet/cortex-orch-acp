//! Task-bound source reads matching the retained source-ref and byte-page contracts.
use base64::Engine;
use cortex_core::{channels::Identity, CoreError, CoreResult, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
pub(super) fn supports(n: &str) -> bool {
    matches!(n, "list_sources" | "read_source")
}
pub(super) fn reply(v: Value) -> Value {
    json!({"content":[{"type":"text","text":v.to_string()}],"structuredContent":v,"isError":false})
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn questions(store: &Store, i: &Identity) -> CoreResult<Vec<Value>> {
    store.read(|db|{
        let mut records=db.prepare("SELECT id,text,answer_text,answer_source,created_at,answered_at,options_json,assignment_ref,dedup_key FROM pending_questions WHERE task_id=? AND status='answered' AND answer_source IN ('text','elicitation') AND answer_text IS NOT NULL ORDER BY created_at,id")?
            .query_map([&i.task],|r|{
                let id:String=r.get(0)?;let assignment:Option<String>=r.get(7)?;let dedup:String=r.get(8)?;
                Ok(json!({"source_ref":format!("{id}:answer"),"question_id":id,"kind":"question_answer",
                    "question_text":r.get::<_,String>(1)?,"answer_text":r.get::<_,String>(2)?,"answer_source":r.get::<_,String>(3)?,
                    "asked_at":r.get::<_,String>(4)?,"answered_at":r.get::<_,Option<String>>(5)?,"options":serde_json::from_str::<Value>(&r.get::<_,Option<String>>(6)?.unwrap_or("null".into())).unwrap_or(Value::Null),
                    "assignment_ref":assignment,"asked_by":if dedup.starts_with("runtime:stop:"){"runtime"}else if assignment.is_some(){"worker"}else{"coordinator"},
                    "trust_label":"user_direct","session_id":null,"origin":null,"session_binding":"unavailable","session_binding_reason":"answer_session_not_recorded"}))
            })?.collect::<Result<Vec<_>,_>>()?;
        for record in &mut records{
            let id=record["question_id"].as_str().unwrap().to_owned();
            let dedup:String=db.query_row("SELECT dedup_key FROM pending_questions WHERE id=?",[&id],|r|r.get(0))?;
            if let Some(rest)=dedup.strip_prefix("runtime:").filter(|r|!r.starts_with("stop:")) {
                let channel=rest.split(':').next().unwrap_or("");
                let role:Option<String>=db.query_row("SELECT role FROM trusted_channels WHERE channel_id=? AND task_id=?",(channel,&i.task),|r|r.get(0)).optional()?;
                record["asked_by"]=json!(role.filter(|r|r=="worker"||r=="coordinator").unwrap_or("unknown".into()));
            }
            let raw:Option<String>=db.query_row("SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=? AND event_type='progress' AND origin='runtime'",
                (&i.task,format!("question_answered:{id}")),|r|r.get(0)).optional()?;
            let session=raw.and_then(|s|serde_json::from_str::<Value>(&s).ok()).and_then(|v|v["session_id"].as_str().map(str::to_owned));
            let mut session=session;
            if session.is_none(){
                let raw:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_QUESTION_ANSWERED' AND json_extract(payload_json,'$.question_id')=? ORDER BY id LIMIT 1",(&i.task,&id),|r|r.get(0)).optional()?;
                session=raw.and_then(|s|serde_json::from_str::<Value>(&s).ok()).and_then(|v|v["session_id"].as_str().map(str::to_owned));
            }
            if session.is_none(){
                for prefix in ["elicitation_answer","cortex_answer"]{
                    let origin:Option<String>=db.query_row("SELECT origin FROM task_events WHERE task_id=? AND dedup_key=? AND event_type='user_message'",
                        (&i.task,format!("{prefix}:{id}")),|r|r.get(0)).optional()?;
                    if origin.is_some(){session=origin;break;}
                }
            }
            if let Some(session)=session.filter(|s|!s.is_empty()){
                record["session_id"]=json!(session);record["origin"]=json!(format!("acp:{session}"));
                record["session_binding"]=json!("recorded");record["session_binding_reason"]=Value::Null;
            }
        }Ok(records)
    })
}
fn provenance(store: &Store, i: &Identity, report: &str) -> CoreResult<(Value, Value)> {
    let message: Option<String> = store.read(|db| {
        Ok(db
            .query_row(
                "SELECT message_id FROM source_revisions WHERE task_id=? AND report_id=?",
                (&i.task, report),
                |r| r.get(0),
            )
            .optional()?)
    })?;
    Ok(message
        .and_then(|s| s.rsplit_once(':').map(|(a, b)| (json!(a), json!(b))))
        .unwrap_or((Value::Null, Value::Null)))
}
fn page(mut meta: Value, bytes: &[u8], offset: usize, limit: usize, lossy: bool) -> Value {
    let start = offset.min(bytes.len());
    let end = offset.saturating_add(limit).min(bytes.len());
    let window = &bytes[start..end];
    let (chunk, encoding) = if lossy {
        (String::from_utf8_lossy(window).into_owned(), "utf-8")
    } else {
        match std::str::from_utf8(window) {
            Ok(s) => (s.to_owned(), "utf-8"),
            Err(_) => (
                base64::engine::general_purpose::STANDARD.encode(window),
                "base64",
            ),
        }
    };
    meta["chunk"] = json!(chunk);
    meta["encoding"] = json!(encoding);
    meta["total_size"] = json!(bytes.len());
    meta["offset"] = json!(offset);
    meta["next_offset"] = if end < bytes.len() {
        json!(end)
    } else {
        Value::Null
    };
    meta
}
fn expired(
    store: &Store,
    report: &str,
    index: i64,
    mut meta: Value,
    reason: &str,
) -> CoreResult<Value> {
    store.write(|db|{db.execute("INSERT INTO source_block_reads(report_id,block_index,observed_at,availability,reason) VALUES (?,?,?,'expired',?)",
        (report,index,cortex_core::now(),reason))?;Ok(())})?;
    meta["availability"] = json!("expired");
    meta["reason"] = json!(reason);
    meta["chunk"] = json!("");
    meta["next_offset"] = Value::Null;
    Ok(reply(meta))
}
pub(super) fn execute(store: &Store, i: &Identity, name: &str, args: &Value) -> CoreResult<Value> {
    if name == "list_sources" {
        let after = args["after"].as_i64().unwrap_or(0);
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;
        let rows=store.read(|db|Ok(db.prepare("SELECT sr.revision,sr.report_id,sr.message_id,e.updated_at FROM source_revisions sr JOIN editions e ON e.report_id=sr.report_id WHERE sr.task_id=? AND sr.revision>? ORDER BY sr.revision LIMIT ?")?
            .query_map(rusqlite::params![i.task,after,(limit+1) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?))?;
        let mut entries = Vec::new();
        for (revision, report, message, created) in rows.iter().take(limit) {
            let (origin, event) = message
                .rsplit_once(':')
                .map(|(a, b)| (json!(a), json!(b)))
                .unwrap_or((Value::Null, Value::Null));
            entries.push(json!({"source_ref":format!("{report}:-1"),"revision":revision,"kind":"text","origin":origin,"event_id":event,"trust_label":"user_direct","created_at":created}));
            let blocks=store.read(|db|Ok(db.prepare("SELECT block_index,kind,mime,name,uri,availability,reason,trust_label,sha256,size_bytes,created_at FROM source_blocks WHERE task_id=? AND report_id=? ORDER BY block_index")?
                .query_map((&i.task,report),|r|Ok(json!({"source_ref":format!("{report}:{}",r.get::<_,i64>(0)?),"revision":revision,
                    "origin":origin,"event_id":event,"kind":r.get::<_,String>(1)?,"mime":r.get::<_,Option<String>>(2)?,"name":r.get::<_,Option<String>>(3)?,"uri":r.get::<_,Option<String>>(4)?,
                    "availability":r.get::<_,String>(5)?,"reason":r.get::<_,Option<String>>(6)?,"trust_label":r.get::<_,String>(7)?,"sha256":r.get::<_,Option<String>>(8)?,"size_bytes":r.get::<_,Option<i64>>(9)?,"created_at":r.get::<_,String>(10)?})))?.collect::<Result<Vec<_>,_>>()?))?;
            entries.extend(blocks);
        }
        let answered = questions(store, i)?;
        let total = answered.len();
        let mut answers = answered.into_iter().take(200).collect::<Vec<_>>();
        for q in &mut answers {
            for field in ["question_text", "answer_text"] {
                let text = q[field].as_str().unwrap().to_owned();
                q[format!("{field}_truncated")] = json!(text.chars().count() > 1000);
                q[field] = json!(text.chars().take(1000).collect::<String>());
            }
        }
        return Ok(reply(crate::native_commands::defer_json(
            store,
            i,
            json!({"entries":entries,"next_cursor":if rows.len()>limit{rows[limit-1].0}else{after},
            "answered_questions":answers,"answered_questions_total":total,"answered_questions_complete":total<=200}),
        )?));
    }
    let reference = args["source_ref"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_source_ref"))?;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(65536) as usize;
    if reference.starts_with("pq_") && reference.ends_with(":answer") {
        let mut q = questions(store, i)?
            .into_iter()
            .find(|q| q["source_ref"] == reference)
            .ok_or(CoreError::Refused("unknown_source_ref"))?;
        let bytes = q["answer_text"].as_str().unwrap().as_bytes().to_vec();
        q["block_index"] = Value::Null;
        q["availability"] = json!("stored");
        q["reason"] = Value::Null;
        q["sha256"] = json!(hash(&bytes));
        return Ok(reply(page(q, &bytes, offset, limit, true)));
    }
    // A constraint omitted from a recovery capsule stays readable in full.
    if let Some(id) = reference.strip_suffix(":constraint") {
        let id = id
            .parse::<i64>()
            .map_err(|_| CoreError::Refused("invalid_source_ref"))?;
        let row: Option<(String, String, String, String)> = store.read(|db| {
            Ok(db
                .query_row(
                    "SELECT event_type,payload_json,created_at,origin FROM task_events WHERE task_id=? AND id=?
                    AND event_type IN ('user_message','coordinator_clarification','coordinator_decision')",
                    (&i.task, id),
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?)
        })?;
        let (kind, payload, created, origin) = row.ok_or(CoreError::Refused("unknown_source_ref"))?;
        let payload: Value = serde_json::from_str(&payload)?;
        let bytes = match payload["text"].as_str() {
            Some(text) => text.as_bytes().to_vec(),
            None => payload.to_string().into_bytes(),
        };
        let meta = json!({"source_ref":reference,"block_index":null,"kind":"constraint","event_type":kind,
            "event_id":id,"origin":origin,"created_at":created,
            "trust_label":if kind=="user_message"{"user_direct"}else{"runtime_recorded"},
            "availability":"stored","reason":null,"sha256":hash(&bytes)});
        return Ok(reply(page(meta, &bytes, offset, limit, true)));
    }
    let (report, index) = reference
        .rsplit_once(':')
        .ok_or(CoreError::Refused("invalid_source_ref"))?;
    let index = index
        .parse::<i64>()
        .map_err(|_| CoreError::Refused("invalid_source_ref"))?;
    let (origin, event) = provenance(store, i, report)?;
    if index == -1 {
        let row:Option<(String,String,i64)>=store.read(|db|Ok(db.query_row("SELECT r.filename,e.digest,e.size_bytes FROM reports r JOIN editions e ON e.report_id=r.id JOIN source_revisions sr ON sr.report_id=r.id WHERE r.task_id=? AND r.id=? ORDER BY e.sequence DESC LIMIT 1",
            (&i.task,report),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?))?;
        let (file, expected, size) = row.ok_or(CoreError::Refused("unknown_source_ref"))?;
        if file.contains('/') || file.contains("..") {
            return Err(CoreError::Refused("source_identity_invalid"));
        }
        let mut file = cortex_core::store::locks::open_private(
            &store.key().directory().join(&i.task).join(file),
            false,
            false,
        )?;
        let (actual, count) = crate::native_commands::fingerprint(&mut file)?;
        if count as i64 != size || actual != expected {
            return Err(CoreError::Refused("source_integrity_mismatch"));
        }
        file.seek(SeekFrom::Start(offset as u64))?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        let next = offset.saturating_add(bytes.len());
        return Ok(reply(
            json!({"source_ref":reference,"block_index":-1,"origin":origin,"event_id":event,"kind":"text","trust_label":"user_direct","availability":"stored","reason":null,"sha256":expected,
            "chunk":String::from_utf8_lossy(&bytes),"encoding":"utf-8","total_size":count,"offset":offset,
            "next_offset":if (next as u64)<count{json!(next)}else{Value::Null}}),
        ));
    }
    let (mut meta,body)=store.read(|db|Ok(db.query_row("SELECT kind,mime,name,uri,sha256,size_bytes,availability,reason,trust_label,body_ref FROM source_blocks WHERE task_id=? AND report_id=? AND block_index=?",
        rusqlite::params![i.task,report,index],|r|Ok((json!({"source_ref":reference,"block_index":index,"origin":origin,"event_id":event,"kind":r.get::<_,String>(0)?,
            "mime":r.get::<_,Option<String>>(1)?,"name":r.get::<_,Option<String>>(2)?,"uri":r.get::<_,Option<String>>(3)?,"sha256":r.get::<_,Option<String>>(4)?,
            "total_size":r.get::<_,Option<i64>>(5)?.unwrap_or(0),"availability":r.get::<_,String>(6)?,"reason":r.get::<_,Option<String>>(7)?,"trust_label":r.get::<_,String>(8)?}),
            r.get::<_,Option<String>>(9)?)))?))?;
    meta["offset"] = json!(offset);
    meta["chunk"] = json!("");
    meta["encoding"] = json!("utf-8");
    meta["next_offset"] = Value::Null;
    if meta["availability"] != "stored" {
        if meta["availability"] == "reference_only" {
            if let Some(uri) = meta["uri"].as_str().filter(|s| s.starts_with("file:")) {
                let (availability, reason) =
                    crate::incoming_prompt::observe_reference(store.key().root(), uri);
                if availability != "reference_only" {
                    store.write(|db| { db.execute("INSERT INTO source_block_reads(report_id,block_index,observed_at,availability,reason) VALUES (?,?,?,?,?)",
                        (report,index,cortex_core::now(),availability,reason))?; Ok(()) })?;
                }
            }
        }
        return Ok(reply(meta));
    }
    let body = body.ok_or(CoreError::Refused("source_body_missing"))?;
    if body.contains('/') || body.contains("..") {
        return Err(CoreError::Refused("source_identity_invalid"));
    }
    let path = store
        .key()
        .directory()
        .join(&i.task)
        .join(".source_blocks")
        .join(body);
    let f = match cortex_core::store::locks::open_private(&path, false, false) {
        Ok(f) => f,
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return expired(store, report, index, meta, "stored_body_missing")
        }
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    f.take((cortex_core::input::MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > cortex_core::input::MAX_BYTES
        || meta["sha256"] != hash(&bytes)
        || meta["total_size"] != bytes.len()
    {
        return expired(store, report, index, meta, "stored_body_changed");
    }
    if meta["kind"] == "image"
        && meta["mime"]
            .as_str()
            .is_some_and(|s| s.starts_with("image/"))
    {
        meta["encoding"] = json!("base64");
        meta["image_in_content"] = json!(true);
        let mut result = reply(meta.clone());
        result["content"].as_array_mut().unwrap().push(json!({"type":"image","mimeType":meta["mime"],"data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
        return Ok(result);
    }
    Ok(reply(page(meta, &bytes, offset, limit, false)))
}

//! Journaled profile activation. Admission precedes effects; uncertain writes never replay.
use super::*;
#[path = "config_materialize.rs"]
mod materialize;
const KEYS: [&str; 11] = [
    "model_policy_version",
    "pin",
    "deny",
    "enabled",
    "auto_allow",
    "tiers",
    "providers",
    "engine_affinity",
    "models",
    "quota_windows",
    "coordinator_models",
];
fn candidates(
    doc: &O,
    p: &Value,
    items: &[Section],
    folder: &Path,
    now: &str,
    notes: &mut Vec<String>,
) -> Result<Vec<(usize, Vec<u8>)>> {
    let entry = get(get(doc, "profiles"), p["name"].as_str().unwrap_or(""));
    let mut targets = vec![];
    for (i, s) in SECTIONS.iter().enumerate() {
        if value(get(entry, "sections"))
            .as_array()
            .is_some_and(|a| a.contains(&json!(s)))
        {
            if let Some(body) =
                materialize::build(i, get(get(entry, "settings"), s), items, folder, now, notes)?
            {
                targets.push((i, body));
            }
        }
    }
    Ok(targets)
}
pub(super) fn activate(
    folder: &Path,
    path: &Path,
    before: Option<&[u8]>,
    mut doc: O,
    parsed: &Value,
    mut items: Vec<Section>,
    p: &Value,
    now: &str,
    persist: &mut impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let n = p["name"].as_str().unwrap();
    let dv = value(&doc);
    if !dv["active"].is_null()
        && p["discard_modifications"] != true
        && dv["pending"]["profile"] != n
    {
        let current: Vec<_> = items
            .iter()
            .map(|i| projection::Snapshot {
                revision: i.observation.revision.clone(),
                capture: i.observation.capture.clone(),
            })
            .collect();
        let mut without_pending = parsed.clone();
        without_pending["document"]["pending"] = Value::Null;
        let status =
            projection::profile_view(path, before, &without_pending, &current, Value::Null);
        if matches!(
            status["result"]["state"].as_str(),
            Some("modified" | "unavailable")
        ) {
            return Err(refuse("profile_modified",format!(
                "The selected profile '{}' has unsaved changes; save them, or activate with discard_modifications: true. Nothing was written.",
                dv["active"].as_str().unwrap())));
        }
    }
    let mut notes = vec![];
    let targets = candidates(&doc, p, &items, folder, now, &mut notes)?;
    set(
        &mut doc,
        "pending",
        object(vec![
            ("operation", scalar("activate")),
            ("profile", scalar(n)),
            ("at", scalar(now)),
        ]),
    );
    let pending = pretty(&doc);
    let pending_doc = doc.clone();
    let originals: Vec<_> = items.iter().map(|i| i.bytes.clone()).collect();
    for (i, b) in &targets {
        items[*i] = Section {
            bytes: Some(b.clone()),
            read_error: None,
            observation: projection::Snapshot {
                revision: json!(write::token(Some(b))),
                capture: capture_diagnostics(SECTIONS[*i], Some(b)),
            },
        };
    }
    set(&mut doc, "active", scalar(n));
    set(
        &mut doc,
        "applied",
        object(vec![
            ("profile", scalar(n)),
            ("at", scalar(now)),
            ("revisions", revisions(&items)),
        ]),
    );
    set(&mut doc, "pending", scalar(Value::Null));
    let final_body = pretty(&doc);
    let (_, final_parsed) = parse(Some(&final_body))?;
    let mut reply = view(path, Some(&final_body), &final_parsed, items);
    reply["changed"] = json!(true);
    notes.extend(
        reply["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
    notes.sort();
    notes.dedup();
    if parsed["migrated_from"] == 1 {
        notes.insert(0, "migrated_from_format_1".into());
    }
    reply["notes"] = json!(notes);
    if [
        pending.len(),
        final_body.len(),
        serde_json::to_vec(&reply).unwrap().len(),
    ]
    .iter()
    .any(|n| *n > FRAME_BUDGET)
        || targets.iter().any(|(_, b)| b.len() > FRAME_BUDGET)
    {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    write::check(
        "routing_profiles",
        &write::token(before),
        &write::token(reader::read_plain(path, true)?.as_deref()),
    )?;
    // Recheck every input under the same shared lock immediately before publication.
    for (i, file) in FILES.iter().enumerate() {
        write::check(
            SECTIONS[i],
            &write::token(originals[i].as_deref()),
            &write::token(reader::read_plain(&folder.join(file), true)?.as_deref()),
        )?;
    }
    if parsed["migrated_from"] == 1 {
        migration_backup(folder, before.unwrap(), now, persist)?;
    }
    persist(path, &pending)?;
    if parsed["migrated_from"] == 1 {
        let verified =
            reader::read_plain(path, true).is_ok_and(|b| b.as_deref() == Some(pending.as_slice()));
        if !verified {
            persist(path, before.unwrap()).map_err(|_| {
                refuse(
                    "profile_rollback_failed",
                    "Profile migration verification and restoration failed.",
                )
            })?;
            return Err(refuse("profile_migration_failed","Migrating routing-profiles.json to format 2 could not be verified; the original file was restored and a private backup kept in .migration-backups. Nothing was changed."));
        }
    }
    let mut written: Vec<usize> = vec![];
    for (i, b) in &targets {
        if let Err(error) = persist(&folder.join(FILES[*i]), b) {
            if error.code == "machine_outcome_unknown" {
                return Err(error);
            }
            let mut restored = true;
            for j in written.into_iter().rev() {
                let target = folder.join(FILES[j]);
                let result = if let Some(bytes) = &originals[j] {
                    persist(&target, bytes)
                } else {
                    std::fs::remove_file(&target)
                        .map_err(Refused::from)
                        .and_then(|_| {
                            std::fs::File::open(folder)?
                                .sync_all()
                                .map_err(Refused::from)
                        })
                };
                if result.is_err() {
                    restored = false;
                }
            }
            if restored {
                // Python clears pending on the attempted document, not the previous selection.
                let mut cleared = pending_doc;
                set(&mut cleared, "pending", scalar(Value::Null));
                // Unlike Python, an unconfirmed journal clear is never reported as the plain
                // section failure: the journal may still claim this interrupted activation.
                persist(path, &pretty(&cleared)).map_err(|_| refuse("machine_outcome_unknown",
                    "Profile activation failed and its settings files were restored, but clearing the pending activation is unconfirmed. Re-read settings before recovery."))?;
            } else {
                return Err(refuse("machine_outcome_unknown","Profile activation rollback could not be confirmed; inspect pending settings before recovery."));
            }
            return Err(error);
        }
        written.push(*i);
    }
    persist(path,&final_body).map_err(|_|refuse("machine_outcome_unknown",
        "Profile activation changed settings but final journal publication is unconfirmed. Re-read settings before recovery."))?;
    Ok(reply)
}

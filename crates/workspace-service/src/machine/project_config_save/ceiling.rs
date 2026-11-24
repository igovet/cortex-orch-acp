use super::{validation::*, *};
use cortex_core::{CoreError, OpenMode};
use rusqlite::OptionalExtension;
pub(super) fn empty() -> Value {
    json!({"allowed_routes":null,"allowed_connections":null,"allowed_data_recipients":null,
        "permission_level":null,"limits":{},"spend_limit":null,"spend_limit_enforceable":false,
        "fallback_policy":{"allowed_target_routes":null,"allowed_failure_classes":null,"budget":null,"data_recipient_consent":false}})
}
pub(super) fn document(v: &Value) -> Result<Value> {
    if !v.is_object() {
        return Err(object_error());
    }
    let s = if truth(&v["stored"]) {
        &v["stored"]
    } else {
        &Value::Null
    };
    if !s.is_null() && !s.is_object() {
        return Err(object_error());
    }
    let mut limits = json!({});
    for (key, value) in [
        ("attempts", &v["attempts"]),
        ("calls", &s["calls"]),
        ("tokens", &s["tokens"]),
        ("concurrency", &s["concurrency"]),
    ] {
        if !value.is_null() {
            limits[key] = value.clone();
        }
    }
    Ok(
        json!({"allowed_routes":v["allowedRoutes"],"allowed_connections":v["allowedConnections"],
        "allowed_data_recipients":v["allowedRecipients"],"permission_level":s["permissionLevel"],
        "limits":limits,"spend_limit":s["spendLimit"],"spend_limit_enforceable":truth(&s["spendEnforceable"]),
        "fallback_policy":{"allowed_target_routes":v["targetRoutes"],"allowed_failure_classes":v["failureClasses"],
            "budget":v["budget"],"data_recipient_consent":truth(&v["consent"])}}),
    )
}
fn int(v: &Value) -> bool {
    v.as_u64().is_some()
}
fn sets(v: &mut Value, key: &str, field: &str, e: &mut Errors) {
    if v[key].is_null() {
        return;
    }
    if !unique(&v[key], |v| v.as_str().is_some_and(|s| !s.is_empty())) {
        e.add(
            field,
            "must be null (unrestricted) or an array of unique non-empty strings",
        );
    } else {
        let mut a = v[key].as_array().unwrap().clone();
        a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        v[key] = a.into();
    }
}
pub(super) fn parse(v: &Value) -> Result<Value> {
    let mut e = Errors::default();
    let v = collect(v, &mut e);
    e.finish()?;
    Ok(v)
}
pub(super) fn collect(v: &Value, e: &mut Errors) -> Value {
    let mut v = v.clone();
    for k in [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
    ] {
        sets(&mut v, k, &format!("ceiling.{k}"), e);
    }
    if !v["permission_level"].is_null() && level(&v["permission_level"]).is_none() {
        e.add(
            "ceiling.permission_level",
            "must be null or one of READ_ONLY, STANDARD, ELEVATED",
        );
    }
    for k in ["attempts", "calls", "tokens", "concurrency"] {
        if let Some(n) = v["limits"].get(k) {
            if !int(n) {
                e.add(
                    format!("ceiling.limits.{k}"),
                    "must be a non-negative integer",
                );
            }
        }
    }
    if !v["spend_limit"].is_null() {
        if !int(&v["spend_limit"]) {
            e.add(
                "ceiling.spend_limit",
                "must be null or a non-negative integer",
            );
        }
        if v["spend_limit_enforceable"] != true {
            e.add("ceiling.spend_limit","may only be set with spend_limit_enforceable: true (unknown cost is never treated as zero)");
        }
    }
    let f = &mut v["fallback_policy"];
    for k in ["allowed_target_routes", "allowed_failure_classes"] {
        sets(f, k, &format!("ceiling.fallback_policy.{k}"), e);
    }
    let denied = f["allowed_failure_classes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| {
            [
                "cancellation",
                "refusal",
                "authorization_loss",
                "provider_restriction",
            ]
            .contains(s)
        })
        .collect::<Vec<_>>();
    if !denied.is_empty() {
        e.add(
            "ceiling.fallback_policy.allowed_failure_classes",
            format!("may never name {}", denied.join(", ")),
        );
    }
    if !f["budget"].is_null() && !int(&f["budget"]) {
        e.add(
            "ceiling.fallback_policy.budget",
            "must be null or a non-negative integer",
        );
    }
    v
}
fn level(v: &Value) -> Option<usize> {
    ["READ_ONLY", "STANDARD", "ELEVATED"]
        .iter()
        .position(|s| v == s)
}
fn violation(field: &str, detail: String) -> Refused {
    let message=format!("'{field}' would expand beyond the already-effective ceiling ({detail}); a project ceiling may only narrow the user ceiling, and a task/assignment override may only narrow the effective ceiling further.");
    let mut e = Errors::default();
    e.coded(format!("ceiling.{field}"), "ceiling_violation", message);
    e.finish().unwrap_err()
}
fn subset(a: &Value, b: &Value, field: &str) -> Result<()> {
    if let Some(b) = b.as_array() {
        if !a
            .as_array()
            .is_some_and(|a| a.iter().all(|v| b.contains(v)))
        {
            let shown = b
                .iter()
                .map(|v| repr(v.as_str().unwrap()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(violation(field, format!("ceiling allows only [{shown}]")));
        }
    }
    Ok(())
}
pub(super) fn narrow(a: &Value, b: &Value) -> Result<()> {
    for k in [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
    ] {
        subset(&a[k], &b[k], k)?;
    }
    if let Some(cap) = level(&b["permission_level"]) {
        if level(&a["permission_level"]).is_none_or(|n| n > cap) {
            return Err(violation(
                "permission_level",
                format!(
                    "ceiling allows at most {}",
                    b["permission_level"].as_str().unwrap()
                ),
            ));
        }
    }
    for k in ["attempts", "calls", "concurrency", "tokens"] {
        if let Some(cap) = b["limits"][k].as_u64() {
            if a["limits"][k].as_u64().is_none_or(|v| v > cap) {
                return Err(violation(
                    &format!("limits.{k}"),
                    format!("ceiling allows at most {cap}"),
                ));
            }
        }
    }
    if let Some(cap) = b["spend_limit"].as_u64() {
        if a["spend_limit"].as_u64().is_none_or(|v| v > cap) || a["spend_limit_enforceable"] != true
        {
            return Err(violation(
                "spend_limit",
                format!("ceiling allows at most {cap}"),
            ));
        }
    }
    for k in ["allowed_target_routes", "allowed_failure_classes"] {
        subset(
            &a["fallback_policy"][k],
            &b["fallback_policy"][k],
            &format!("fallback_policy.{k}"),
        )?;
    }
    if let Some(cap) = b["fallback_policy"]["budget"].as_u64() {
        if a["fallback_policy"]["budget"]
            .as_u64()
            .is_none_or(|v| v > cap)
        {
            return Err(violation(
                "fallback_policy.budget",
                format!("ceiling allows at most {cap}"),
            ));
        }
    }
    if a["fallback_policy"]["data_recipient_consent"] == true
        && b["fallback_policy"]["data_recipient_consent"] != true
    {
        return Err(violation(
            "fallback_policy.data_recipient_consent",
            "ceiling does not grant data_recipient_consent".into(),
        ));
    }
    Ok(())
}
pub(super) fn user(config: &crate::Config) -> Result<Value> {
    user_at(&reader::owner_folder(config)?)
}
/// The owner ceiling of `folder/defaults.json` (unrestricted when absent).
pub(super) fn user_at(folder: &Path) -> Result<Value> {
    let bytes = reader::read_plain(&folder.join("defaults.json"), true)?;
    let v: Value = bytes
        .map(|b| serde_json::from_slice(&b))
        .transpose()
        .map_err(|_| {
            refuse(
                "invalid_user_defaults",
                "defaults.json is not valid UTF-8 JSON with the expected shape.",
            )
        })?
        .unwrap_or(json!({}));
    if !v.is_object() {
        return Err(object_error());
    }
    match v.get("ceiling") {
        Some(c) => document(&reader::ceiling(c)?),
        None => Ok(empty()),
    }
}
pub(super) fn intersect(a: &Value, b: &Value) -> Value {
    let mut v = a.clone();
    for (key, fallback) in [
        ("allowed_routes", false),
        ("allowed_connections", false),
        ("allowed_data_recipients", false),
        ("allowed_target_routes", true),
        ("allowed_failure_classes", true),
    ] {
        let (a, b) = if fallback {
            (&a["fallback_policy"][key], &b["fallback_policy"][key])
        } else {
            (&a[key], &b[key])
        };
        let c = if a.is_null() {
            b.clone()
        } else if b.is_null() {
            a.clone()
        } else {
            json!(a
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| b.as_array().unwrap().contains(v))
                .collect::<Vec<_>>())
        };
        if fallback {
            v["fallback_policy"][key] = c;
        } else {
            v[key] = c;
        }
    }
    v["permission_level"] = if level(&a["permission_level"]).unwrap_or(3)
        < level(&b["permission_level"]).unwrap_or(3)
    {
        a["permission_level"].clone()
    } else {
        b["permission_level"].clone()
    };
    for k in ["attempts", "calls", "tokens", "concurrency"] {
        let c = match (a["limits"][k].as_u64(), b["limits"][k].as_u64()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(c) = c {
            v["limits"][k] = json!(c);
        }
    }
    for (key, fallback) in [("spend_limit", false), ("budget", true)] {
        let (aa, bb) = if fallback {
            (&a["fallback_policy"][key], &b["fallback_policy"][key])
        } else {
            (&a[key], &b[key])
        };
        let c = match (aa.as_u64(), bb.as_u64()) {
            (Some(a), Some(b)) => json!(a.min(b)),
            _ => {
                if aa.is_null() {
                    bb.clone()
                } else {
                    aa.clone()
                }
            }
        };
        if fallback {
            v["fallback_policy"][key] = c;
        } else {
            v[key] = c;
        }
    }
    v["spend_limit_enforceable"] = json!(
        !v["spend_limit"].is_null()
            && (a["spend_limit_enforceable"] == true || b["spend_limit_enforceable"] == true)
    );
    v["fallback_policy"]["data_recipient_consent"] = json!(
        a["fallback_policy"]["data_recipient_consent"] == true
            && b["fallback_policy"]["data_recipient_consent"] == true
    );
    v
}
pub(super) type Row = (String, String, String);
pub(super) fn row(db: &rusqlite::Connection, root: &Path) -> cortex_core::CoreResult<Option<Row>> {
    let r=db.query_row("SELECT CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB))<=?2 THEN defaults END,CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB))<=?2 THEN ceiling END,CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB))<=?2 THEN updated_at END FROM project_configuration_defaults WHERE project_root=?1",
        rusqlite::params![root.to_string_lossy().as_ref(),FRAME_BUDGET as i64],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
    match r {
        Some((Some(a), Some(b), Some(c))) => Ok(Some((a, b, c))),
        Some(_) => Err(CoreError::Refused("machine_reply_too_large")),
        None => Ok(None),
    }
}
pub(super) fn revision(row: Option<&Row>) -> String {
    row.map(|(a, b, c)| {
        token(Some(
            serde_json::to_string(&json!([a, b, c])).unwrap().as_bytes(),
        ))
    })
    .unwrap_or("absent".into())
}
pub(super) fn response(root: &Path, written: Option<&Row>, user: &Value) -> Result<Value> {
    let mut out = json!({"surface":"project_ceiling","project_root":root,"revision":revision(written),
        "exists":written.is_some(),"user_ceiling":user,"narrows_user_ceiling":true,"violation":null,"changed":true});
    if let Some((d, c, t)) = written {
        let c: Value = serde_json::from_str(c)
            .map_err(|_| refuse("invalid_ceiling_snapshot", "A stored ceiling is invalid."))?;
        out["ceiling"] = c.clone();
        out["project_defaults"] = serde_json::from_str(d)
            .map_err(|_| refuse("machine_tool_failed", "JSONDecodeError"))?;
        out["effective_ceiling"] = intersect(user, &c);
        out["updated_at"] = json!(t);
    } else {
        out["ceiling"] = Value::Null;
        out["project_defaults"] = json!({});
        out["effective_ceiling"] = user.clone();
        out["updated_at"] = Value::Null;
    }
    if serde_json::to_vec(&out).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native reply limit.",
        ));
    }
    Ok(out)
}
pub(super) fn save(root: &Path, config: &crate::Config, params: &Value) -> Result<Value> {
    let expected = super::expected(params, "project_ceiling")?;
    let candidate = if params["value"].is_null() {
        None
    } else {
        Some(parse(&document(&params["value"])?)?)
    };
    let folder = reader::owner_folder(config)?;
    // Cooperates with retained writers of the user ceiling and registry.
    let _lock = super::owner_lock(&folder)?;
    let user = user(config)?;
    if let Some(c) = &candidate {
        narrow(c, &user)?;
    }
    let store = context::open(root, OpenMode::RuntimeValidate)?
        .ok_or_else(|| {
            refuse(
                "project_store_missing",
                "The project store refused the request.",
            )
        })?;
    let mut effect = false;
    store.write(|db|{
        let current=row(db,root)?;
        if let Err(e)=check("project_ceiling",&expected,&revision(current.as_ref())) {return Ok(Err(e));}
        if candidate.is_none(){
            if let Some((defaults,_,_))=&current {
                let defaults:Value=match serde_json::from_str(defaults){Ok(v)=>v,Err(_)=>return Ok(Err(refuse("machine_tool_failed","JSONDecodeError")))};
                if truth(&defaults){return Ok(Err(refuse("project_defaults_present","This project row also holds project defaults; replace the ceiling instead of removing it.")));}
            }
        }
        let written=candidate.as_ref().map(|c|(current.as_ref().map(|r|r.0.clone()).unwrap_or("{}".into()),c.to_string(),cortex_core::now()));
        // Parse and budget the entire response before the first modifying statement.
        let reply=match response(root,written.as_ref(),&user){Ok(v)=>v,Err(e)=>return Ok(Err(e))};
        effect=true;
        if let Some((d,c,t))=&written {
            db.execute("INSERT INTO project_configuration_defaults VALUES (?1,?2,?3,?4) ON CONFLICT(project_root) DO UPDATE SET ceiling=excluded.ceiling,updated_at=excluded.updated_at",
                rusqlite::params![root.to_string_lossy().as_ref(),d,c,t])?;
        }else{
            db.execute("DELETE FROM project_configuration_defaults WHERE project_root=?1",[root.to_string_lossy().as_ref()])?;
        }
        Ok(Ok(reply))
    }).map_err(|e|if effect{unknown()}else{context::core(e)})?
}

//! Helpers shared by every store reader (`STORE_PYTHON_HELPERS` in
//! `integrations/paseo/server/worker-history.ts`) plus small Python idioms
//! the readers repeat.
use crate::py::{self, Crash, Py};
use crate::scrub::scrub;
use crate::store::{self, Fail, Row, Sv};
use rusqlite::Connection;

/// `{key: value, ...}` from literal pairs, in order.
pub fn dict(entries: Vec<(&str, Py)>) -> Py {
    Py::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

/// `print(json.dumps(value, ensure_ascii=False))` output.
pub fn printed(value: &Py) -> Result<String, Crash> {
    let mut text = py::dumps(value)?;
    text.push('\n');
    Ok(text)
}

/// `clip(value, limit)` of the pipeline and result readers: `None` for a
/// non-string, else the scrubbed, stripped and bounded text.
pub fn clip_scrubbed(value: &Py, limit: usize) -> Py {
    match value.as_str() {
        None => Py::Null,
        Some(text) => Py::str(py::ellipsis(&scrub(py::strip(text)), limit)),
    }
}

/// `isinstance(value, str) and value.strip()`.
pub fn nonblank(value: &Py) -> Option<&str> {
    value.as_str().filter(|s| !py::strip(s).is_empty())
}

/// `str(c.get('outcome') or 'unverified')`.
pub fn outcome_text(check: &Py) -> String {
    let outcome = check.get("outcome");
    if outcome.truthy() {
        py::to_str(outcome)
    } else {
        "unverified".into()
    }
}

/// `sum_tokens(rows)`: one per-turn usage row per prompt; `None` when no row counts.
pub fn sum_tokens(rows: &[Row]) -> Result<Option<Py>, Crash> {
    let keys = ["input", "output", "cachedRead", "cachedWrite", "total"];
    let mut totals: Vec<String> = vec!["0".into(); keys.len()];
    let (mut known, mut unknown) = ("0".to_string(), "0".to_string());
    for row in rows {
        let basis = row.get("basis")?;
        if basis.eq_str("unknown")
            && row.has("failed")
            && row.get("failed")?.eq_int(0)
            && row.has("stop_reason")
            && row.get("stop_reason")?.eq_str("released_after_result")
        {
            continue;
        }
        let raw = row.get("tokens_json")?;
        let tokens = if raw.truthy() {
            raw.loads().unwrap_or_else(|_| Py::dict())
        } else {
            Py::dict()
        };
        if basis.eq_str("unknown") || !tokens.is_dict() || !tokens.truthy() {
            unknown = py::add_decimal(&unknown, "1");
            continue;
        }
        let count = |key: &str| -> String {
            match tokens.get(key) {
                Py::Int(text) if !text.starts_with('-') => text.clone(),
                _ => "0".into(),
            }
        };
        known = py::add_decimal(&known, "1");
        let parts = [
            count("inputTokens"),
            count("outputTokens"),
            count("cachedReadTokens"),
            count("cachedWriteTokens"),
        ];
        for (index, part) in parts.iter().enumerate() {
            totals[index] = py::add_decimal(&totals[index], part);
        }
        let total = match tokens.get("totalTokens") {
            Py::Int(text) if !text.starts_with('-') => text.clone(),
            _ => parts
                .iter()
                .fold("0".to_string(), |sum, part| py::add_decimal(&sum, part)),
        };
        totals[4] = py::add_decimal(&totals[4], &total);
    }
    if known == "0" && unknown == "0" {
        return Ok(None);
    }
    let mut out: Vec<(&str, Py)> = keys
        .iter()
        .zip(totals)
        .map(|(key, value)| (*key, Py::Int(value)))
        .collect();
    out.push(("knownCalls", Py::Int(known)));
    out.push(("unknownCalls", Py::Int(unknown)));
    Ok(Some(dict(out)))
}

/// `route_selection(detail)`: how the runtime chose a route.
pub fn route_selection(detail: &Py) -> Result<Option<Py>, Crash> {
    let decision = if detail.is_dict() {
        detail.get("route_decision")
    } else {
        &py::NONE
    };
    if !decision.is_dict() {
        return Ok(None);
    }
    fn text(value: &Py) -> Option<String> {
        value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
    }
    let mut tier = text(decision.get("tier"));
    let cross_tier = decision.get("cross_tier");
    let role_tier = text(decision.get("role_tier"));
    if tier.is_some()
        && decision.get("chosen_cross_tier").truthy()
        && cross_tier.is_dict()
        && text(cross_tier.get("from_tier")).is_some()
    {
        let from = cross_tier.get("from_tier").as_str().unwrap_or_default();
        tier = tier.map(|t| format!("{t}→{from}"));
    } else if let (Some(t), Some(r)) = (&tier, &role_tier) {
        if r != t {
            tier = Some(format!("{r}→{t}"));
        }
    }
    let hint = text(decision.get("model_hint_status"));
    let mut used = Py::Null;
    let candidates = decision.get("candidates");
    if candidates.truthy() {
        for candidate in candidates.iterate()? {
            if candidate.is_dict()
                && py::eq(candidate.get("route"), decision.get("chosen"))
                && py::eq(
                    candidate.get("connection_id"),
                    decision.get("chosen_connection_id"),
                )
            {
                let value = candidate.get("used_percent");
                if value.is_number() {
                    used = Py::Float(value.to_float()?);
                }
                break;
            }
        }
    }
    let decided = text(decision.get("decided_by"));
    let decided_by = match decided.as_deref() {
        Some("user_request") => Py::str("your request"),
        Some(other) => Py::str(other),
        None => Py::Null,
    };
    let opt = |value: Option<String>| value.map(Py::Str).unwrap_or(Py::Null);
    Ok(Some(dict(vec![
        ("tier", opt(tier)),
        ("hint", opt(hint.filter(|h| h != "auto"))),
        ("decidedBy", decided_by),
        ("planUsedPercent", used),
    ])))
}

/// `usage_fallback_refs(db, task_id, execution_refs)`: a routed retry's
/// audited predecessor chain, so usage includes every earlier route hop.
pub fn usage_fallback_refs(
    db: &Connection,
    task_id: &Sv,
    execution_refs: &[String],
) -> Result<Vec<String>, Fail> {
    let mut refs: Vec<String> = Vec::new();
    for r in execution_refs {
        if !refs.contains(r) {
            refs.push(r.clone());
        }
    }
    if !store::columns(db, "attempt_route_audit")?
        .iter()
        .any(|c| c == "predecessor_attempt_id")
    {
        refs.sort();
        return Ok(refs);
    }
    for start in execution_refs {
        let mut current = start.clone();
        for _ in 0..50 {
            let row = store::query_one(
                db,
                "select predecessor_attempt_id from attempt_route_audit \
                 where task_id=? and attempt_id=? order by recorded_at desc, id desc limit 1",
                &[task_id.clone(), Sv::text(current.clone())],
            )?;
            let previous = match &row {
                Some(row) => row.get("predecessor_attempt_id")?.clone(),
                None => Sv::Null,
            };
            if !previous.truthy() {
                break;
            }
            // A non-text predecessor would make Python's sorted() raise.
            let previous = previous.as_text().ok_or(Fail::Crash)?.to_owned();
            if refs.contains(&previous) {
                break;
            }
            refs.push(previous.clone());
            current = previous;
        }
    }
    refs.sort();
    Ok(refs)
}

/// `process_identity_absent(row)`: true only when the durable worker
/// identity is provably gone; unreadable identity data stays indeterminate.
pub fn process_identity_absent(row: &Row) -> Result<bool, Crash> {
    if !row.get("terminated_at")?.is_null() {
        return Ok(true);
    }
    let boot_id = row.get("boot_id")?.clone();
    let pid = row.get("pid")?.to_str();
    let start_ticks = row.get("start_ticks")?.to_str();
    let Ok(boot) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") else {
        return Ok(false);
    };
    let current = py::strip(&boot);
    if current.is_empty() || !boot_id.eq_str(current) {
        return Ok(true);
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return Ok(false);
    };
    let Some(close) = stat.rfind(')') else {
        return Ok(false);
    };
    let fields: Vec<&str> = stat[close + 1..]
        .split(py::is_space)
        .filter(|f| !f.is_empty())
        .collect();
    if fields.len() < 20 {
        return Ok(false);
    }
    Ok(fields[19] != start_ticks)
}

/// The pipeline/result readers' check projection of one payload's `checks`:
/// `[{'name', 'outcome', 'blocker'?, ...} for c in payload.get('checks') or [] if isinstance(c, dict)]`.
pub fn result_checks(checks: &Py, extended: bool) -> Result<Vec<Py>, Crash> {
    let mut out = Vec::new();
    if !checks.truthy() {
        return Ok(out);
    }
    for c in checks.iterate()? {
        if !c.is_dict() {
            continue;
        }
        let name = clip_scrubbed(c.get("name"), 300);
        let mut check = dict(vec![
            (
                "name",
                if name.truthy() {
                    name
                } else {
                    Py::str("check")
                },
            ),
            ("outcome", Py::str(outcome_text(&c))),
        ]);
        if nonblank(c.get("blocker")).is_some() {
            check.set("blocker", clip_scrubbed(c.get("blocker"), 600));
        }
        if extended {
            if nonblank(c.get("closed_by")).is_some() {
                check.set("closedBy", clip_scrubbed(c.get("closed_by"), 128));
            }
            for (key, field) in [
                ("claimedOutcome", "claimed_outcome"),
                ("evidenceGap", "evidence_gap"),
            ] {
                if nonblank(c.get(field)).is_some() {
                    check.set(key, c.get(field).clone());
                }
            }
        }
        out.push(check);
    }
    out.truncate(30);
    Ok(out)
}

//! Bounded display-only worker activity. Cursor positions never cross attempts or namespaces.
use super::workers::{object, query, text, truthy};
use cortex_core::{CoreError, CoreResult};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(super) const HIDDEN: &[&str] = &[
    "agent_thought_chunk",
    "thought",
    "thinking",
    "reasoning",
    "wait",
    "wait_agent",
    "task_notification",
    "usage_update",
    "current_mode_update",
    "config_option_update",
    "available_commands_update",
    "session_info_update",
    "turn_started",
    "turn_complete",
];
fn operation(s: &str) -> &str {
    s.rsplit("__")
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
}
fn hidden(s: &str) -> bool {
    HIDDEN.contains(&operation(s.trim()).to_lowercase().as_str())
}
pub(super) fn dump(v: &Value, ascii: bool) -> String {
    match v {
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(|v| dump(v, ascii))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", dump(&json!(k), ascii), dump(v, ascii)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::String(s) if ascii => {
            let mut out = String::from("\"");
            for c in s.chars() {
                if c.is_ascii() && c != char::from(127) {
                    let q = serde_json::to_string(&c.to_string()).unwrap();
                    out.push_str(&q[1..q.len() - 1]);
                } else {
                    let mut units = [0; 2];
                    for unit in c.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
            out.push('"');
            out
        }
        _ => v.to_string(),
    }
}
fn py_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if (-4..16).contains(&exp) {
        let fixed = format!("{f}");
        if fixed.contains('.') {
            fixed
        } else {
            format!("{fixed}.0")
        }
    } else {
        format!(
            "{mantissa}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    }
}
fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_control() => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"))
                } else {
                    out.push_str(&format!("\\u{n:04x}"))
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match n
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| n.as_u64().map(|u| u.to_string()))
        {
            Some(i) => i,
            None => py_float(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
/// Python `str(value)` of a decoded JSON value.
pub(super) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => py_repr(v),
    }
}
pub(super) fn bytes(v: &Value) -> usize {
    dump(v, false).len()
}
pub(super) fn clipped(s: &str) -> String {
    if bytes(&json!(s)) <= 8192 {
        return s.into();
    }
    let mut prefix = String::new();
    for c in s.chars() {
        prefix.push(c);
        if bytes(&json!(prefix)) > 8192 - 128 {
            prefix.pop();
            break;
        }
    }
    format!(
        "{prefix}\n[Content clipped: first {} of {} UTF-8 bytes]",
        prefix.len(),
        s.len()
    )
}
/// Python `iso_ms`: `int(datetime.fromisoformat(text.replace("Z", "+00:00"))
/// .timestamp() * 1000)`, or None for a non-string or invalid ISO value. Naive
/// values are local time, exactly like the legacy reader.
pub(super) fn at(_db: &Connection, value: &Value) -> CoreResult<Value> {
    Ok(value
        .as_str()
        .and_then(iso_ms)
        .map_or(Value::Null, |ms| json!(ms)))
}
fn digits(b: &[u8], at: usize, n: usize) -> Option<i64> {
    let part = b.get(at..at + n)?;
    part.iter()
        .all(u8::is_ascii_digit)
        .then(|| part.iter().fold(0, |v, d| v * 10 + i64::from(d - b'0')))
}
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}
fn month_days(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
/// CPython's `_find_isoformat_datetime_separator`: where the date part ends.
fn separator(b: &[u8]) -> Option<usize> {
    let len = b.len();
    if len == 7 {
        return Some(7);
    }
    if b[4] == b'-' {
        if b[5] != b'W' {
            return Some(10);
        }
        if len > 8 && b[8] == b'-' {
            if len == 9 {
                return None;
            }
            return Some(if len > 10 && b[10].is_ascii_digit() {
                8
            } else {
                10
            });
        }
        return Some(8);
    }
    if b[4] == b'W' {
        let mut at = 7;
        while at < len && b[at].is_ascii_digit() {
            at += 1;
        }
        return Some(if at < 9 {
            at
        } else if at % 2 == 0 {
            7
        } else {
            8
        });
    }
    Some(8)
}
fn iso_date(b: &[u8]) -> Option<(i64, i64, i64)> {
    let year = digits(b, 0, 4)?;
    let mut pos = 4;
    let sep = b.get(pos) == Some(&b'-');
    pos += usize::from(sep);
    let (y, m, d);
    if b.get(pos) == Some(&b'W') {
        let week = digits(b, pos + 1, 2)?;
        pos += 3;
        let mut day = 1;
        if pos < b.len() {
            if sep {
                if b.get(pos) != Some(&b'-') {
                    return None;
                }
                pos += 1;
            }
            day = digits(b, pos, 1)?;
            pos += 1;
        }
        if pos != b.len() || !(1..=7).contains(&day) || !(1..=53).contains(&week) {
            return None;
        }
        let iso_start = |year: i64| {
            let jan4 = days_from_civil(year, 1, 4);
            jan4 - (jan4 + 3).rem_euclid(7)
        };
        if week == 53 && iso_start(year + 1) - iso_start(year) != 371 {
            return None;
        }
        let days = iso_start(year) + (week - 1) * 7 + day - 1;
        // Back to a civil date.
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        d = doy - (153 * mp + 2) / 5 + 1;
        m = if mp < 10 { mp + 3 } else { mp - 9 };
        y = yoe + era * 400 + i64::from(m <= 2);
    } else {
        m = digits(b, pos, 2)?;
        pos += 2;
        if sep {
            if b.get(pos) != Some(&b'-') {
                return None;
            }
            pos += 1;
        }
        d = digits(b, pos, 2)?;
        pos += 2;
        if pos != b.len() {
            return None;
        }
        y = year;
    }
    ((1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= month_days(y, m))
        .then_some((y, m, d))
}
/// CPython's `_parse_hh_mm_ss_ff`: hour, minute, second, microsecond.
fn hh_mm_ss_ff(b: &[u8]) -> Option<[i64; 4]> {
    let mut parts = [0; 4];
    let mut pos = 0;
    let mut sep = false;
    for comp in 0..3 {
        if b.len() - pos < 2 {
            return None;
        }
        parts[comp] = digits(b, pos, 2)?;
        pos += 2;
        let next = b.get(pos).copied();
        if comp == 0 {
            sep = next == Some(b':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if sep && next != Some(b':') {
            return None;
        }
        pos += usize::from(sep);
    }
    if pos < b.len() {
        if !matches!(b[pos], b'.' | b',') {
            return None;
        }
        pos += 1;
        let rest = b.len() - pos;
        let take = rest.min(6);
        let mut micro = digits(b, pos, take)?;
        for _ in take..6 {
            micro *= 10;
        }
        if !b[pos + take..].iter().all(u8::is_ascii_digit) {
            return None;
        }
        parts[3] = micro;
    }
    Some(parts)
}
fn iso_ms(text: &str) -> Option<i64> {
    let text = text.replace('Z', "+00:00");
    let b = text.as_bytes();
    if !text.is_ascii() || b.len() < 7 {
        return None;
    }
    let sep = separator(b)?;
    // A short dashed date ("2026-1-1") names a separator past its end; Python returns None.
    let (y, mo, d) = iso_date(b.get(..sep)?)?;
    let tail = b.get(sep + 1..).unwrap_or(&[]);
    let (mut h, mut mi, mut sec, mut us) = (0, 0, 0, 0);
    let mut offset: Option<i64> = None;
    if !tail.is_empty() {
        if tail.len() < 2 {
            return None;
        }
        let zone = tail
            .iter()
            .position(|c| *c == b'-')
            .or_else(|| tail.iter().position(|c| *c == b'+'));
        let clock = zone.map_or(tail, |at| &tail[..at]);
        [h, mi, sec, us] = hh_mm_ss_ff(clock)?;
        if let Some(at) = zone {
            let rest = &tail[at + 1..];
            if matches!(rest.len(), 0 | 1 | 3) {
                return None;
            }
            let [zh, zm, zs, zu] = hh_mm_ss_ff(rest)?;
            let total = ((zh * 60 + zm) * 60 + zs) * 1_000_000 + zu;
            if total >= 86_400_000_000 {
                return None;
            }
            offset = Some(if tail[at] == b'-' { -total } else { total });
        }
    }
    // Hour 24 is only the end of day: exactly 24:00:00 rolls to the next day.
    let mut extra_days = 0;
    if h == 24 {
        if mi != 0 || sec != 0 || us != 0 {
            return None;
        }
        h = 0;
        extra_days = 1;
    }
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d) + extra_days;
    let seconds = days * 86400 + h * 3600 + mi * 60 + sec;
    let timestamp = match offset {
        Some(offset) => {
            let total = seconds * 1_000_000 + us - offset;
            total as f64 / 1_000_000.0
        }
        None => {
            // Naive values are interpreted in the local zone, like datetime.timestamp().
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            tm.tm_year = (y - 1900) as i32;
            tm.tm_mon = (mo - 1) as i32;
            tm.tm_mday = (d + extra_days) as i32;
            tm.tm_hour = h as i32;
            tm.tm_min = mi as i32;
            tm.tm_sec = sec as i32;
            tm.tm_isdst = -1;
            let local = unsafe { libc::mktime(&mut tm) };
            if local == -1 {
                return None;
            }
            local as f64 + us as f64 / 1e6
        }
    };
    Some((timestamp * 1000.0) as i64)
}
fn shell(a: &[Value]) -> String {
    a.iter()
        .map(|v| {
            let s = v.as_str().unwrap_or("");
            if !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
            {
                s.into()
            } else {
                format!("'{}'", s.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}
fn output(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(output)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(m) => ["output", "text", "stdout", "formatted_output", "content"]
            .iter()
            .find_map(|k| m.get(*k))
            .map(output)
            .unwrap_or_default(),
        _ => String::new(),
    }
}
fn merge(old: &mut Value, new: &Value, recursive: bool) {
    for (k, v) in new.as_object().into_iter().flatten() {
        if !recursive
            && (v.is_null()
                || v.as_str() == Some("")
                || v.as_array().is_some_and(Vec::is_empty)
                || v.as_object().is_some_and(|m| m.is_empty()))
        {
            continue;
        }
        if recursive || ["rawInput", "raw_input", "input"].contains(&k.as_str()) {
            if old[k].is_object() && v.is_object() {
                merge(&mut old[k], v, true);
                continue;
            }
        }
        old[k] = v.clone();
    }
}
fn tool_ref(v: &Value) -> Option<String> {
    let r = v["activity_ref"].as_str()?;
    let r = r
        .strip_prefix("tool-error:")
        .map(|r| format!("tool:{r}"))
        .unwrap_or_else(|| r.into());
    (r.starts_with("tool:") || truthy(&v["tool_kind"])).then_some(r)
}
pub(super) fn detail(v: &Value) -> CoreResult<(Value, String)> {
    let mut raw = v
        .get("rawInput")
        .or_else(|| v.get("raw_input"))
        .or_else(|| v.get("input"))
        .cloned()
        .unwrap_or(json!({}));
    if let Some(s) = raw.as_str() {
        raw = object(s);
    }
    if raw["server"] == "cortex" && raw["arguments"].is_object() {
        raw = raw["arguments"].clone();
    } else if raw["input"].is_object() {
        raw = raw["input"].clone();
    }
    // Preserve Python's insertion order inside the nested args JSON string.
    let mut pairs: Vec<(String, Value)> = vec![];
    let put = |pairs: &mut Vec<(String, Value)>, k: &str, v: Value| {
        if let Some((_, old)) = pairs.iter_mut().find(|(name, _)| name == k) {
            *old = v;
        } else {
            pairs.push((k.into(), v));
        }
    };
    put(
        &mut pairs,
        "title",
        json!(if truthy(&v["title"]) {
            py_str(&v["title"])
        } else {
            "Tool activity".into()
        }),
    );
    put(
        &mut pairs,
        "callId",
        json!(if truthy(&v["toolCallId"]) {
            py_str(&v["toolCallId"])
        } else if truthy(&v["activity_ref"]) {
            py_str(&v["activity_ref"])
        } else {
            String::new()
        }),
    );
    if !v["status"].is_null() {
        put(&mut pairs, "status", v["status"].clone());
    }
    let kind = v
        .get("tool_kind")
        .or_else(|| v.get("kind"))
        .unwrap_or(&Value::Null);
    if !kind.is_null() {
        put(&mut pairs, "kind", kind.clone());
    }
    for (name, keys) in [
        (
            "path",
            vec!["file_path", "filePath", "path", "absolutePath"],
        ),
        ("command", vec!["command", "cmd"]),
        ("query", vec!["query", "pattern", "q"]),
        (
            "ref",
            vec!["report_id", "report_ref", "draft_id", "source_ref", "skill"],
        ),
    ] {
        if let Some(s) = keys.iter().find_map(|k| raw[*k].as_str()) {
            put(&mut pairs, name, json!(s));
        }
        if name == "path" && !pairs.iter().any(|(k, _)| k == "path") {
            if let Some(s) = v["locations"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v["path"].as_str())
            {
                put(&mut pairs, "path", json!(s));
            }
        }
        if name == "command" {
            if let Some(a) = raw["command"]
                .as_array()
                .filter(|a| a.iter().all(Value::is_string))
            {
                put(&mut pairs, "command", json!(shell(a)));
            }
            if !pairs.iter().any(|(k, _)| k == "command") {
                if let Some(a) = raw["argv"]
                    .as_array()
                    .filter(|a| a.iter().all(Value::is_string))
                {
                    put(&mut pairs, "command", json!(shell(a)));
                }
            }
        }
    }
    let op = operation(pairs[0].1.as_str().unwrap_or("Tool activity")).to_lowercase();
    let private = [
        "create_draft",
        "draft_write",
        "write_report",
        "result_submit",
    ]
    .contains(&op.as_str());
    if private && kind == "edit" {
        put(&mut pairs, "kind", json!("other"));
    }
    let edit = !private
        && (kind == "edit"
            || [
                "project_edit",
                "project_write",
                "outside_write",
                "edit",
                "write",
                "multiedit",
                "apply_patch",
            ]
            .contains(&op.as_str()));
    if edit {
        for (name, keys) in [
            ("oldText", vec!["old_string", "oldString", "oldText"]),
            (
                "newText",
                vec!["new_string", "newString", "newText", "content"],
            ),
        ] {
            if let Some(s) = keys.iter().find_map(|k| raw[*k].as_str()) {
                put(&mut pairs, name, json!(s));
            }
        }
        if let Some(a) = raw["edits"].as_array() {
            for (target, key) in [("oldText", "old_string"), ("newText", "new_string")] {
                put(
                    &mut pairs,
                    target,
                    json!(a
                        .iter()
                        .filter(|v| v.is_object())
                        .map(|v| v.get(key).map(text).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\n")),
                );
            }
        }
    }
    if let Some(a) = v["content"].as_array() {
        for b in a.iter().filter(|v| v["type"] == "diff") {
            // A private report body is never rendered as a diff: the block is
            // dropped and the step says so (the legacy reader would have shown it).
            if private {
                put(
                    &mut pairs,
                    "notice",
                    json!("A private report body is not shown in this view."),
                );
                continue;
            }
            for k in ["path", "oldText", "newText"] {
                if let Some(s) = b[k].as_str() {
                    put(&mut pairs, k, json!(s));
                }
            }
        }
    }
    if pairs.iter().any(|(k, v)| k == "command" && truthy(v)) {
        let mut out = output(
            v.get("rawOutput")
                .or_else(|| v.get("raw_output"))
                .or_else(|| v.get("output"))
                .unwrap_or(&Value::Null),
        );
        if out.is_empty() {
            out = output(&v["content"]);
        }
        put(&mut pairs, "output", json!(out));
    }
    if truthy(&v["projection_partial"]) {
        put(
            &mut pairs,
            "notice",
            json!("Only the last 100 updates of this call are available in this view."),
        );
    }
    if truthy(&v["payload_offloaded"]) {
        put(
            &mut pairs,
            "notice",
            json!("Only a clipped preview of this step was retained in this view."),
        );
    }
    let render = |pairs: &[(String, Value)]| {
        format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", dump(&json!(k), false), dump(v, false)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    while render(&pairs).len() > 18000 {
        let i = pairs
            .iter()
            .enumerate()
            .filter(|(_, (_, v))| v.is_string())
            .max_by_key(|(i, (_, v))| (v.as_str().unwrap().chars().count(), std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
            .unwrap_or(usize::MAX);
        if i == usize::MAX {
            break;
        }
        let s = pairs[i].1.as_str().unwrap();
        pairs[i].1 = json!(
            s.chars().take(s.chars().count() / 2).collect::<String>() + "\n[Content clipped]"
        );
    }
    let args = render(&pairs);
    Ok((Value::Object(pairs.into_iter().collect()), args))
}
fn activity(db: &Connection, rows: &[Value]) -> CoreResult<Vec<Value>> {
    let mut calls: Vec<(Value, Value)> = vec![];
    for row in rows {
        let mut payload = object(row["payload_json"].as_str().unwrap_or(""));
        if let Some(r) = tool_ref(&payload) {
            payload["activity_ref"] = json!(r);
            if let Some((_, old)) = calls.iter_mut().find(|(_, v)| v["activity_ref"] == r) {
                merge(old, &payload, false);
                continue;
            }
        }
        calls.push((row.clone(), payload));
    }
    let mut out = vec![];
    for (row, v) in calls {
        // Python: `kind = payload.get("tool_kind")`; hidden_step applies str().
        let tool = truthy(&v["tool_kind"]);
        let kind = if tool {
            py_str(&v["tool_kind"])
        } else {
            String::new()
        };
        let kind = kind.as_str();
        if hidden(kind) || (tool && hidden(&py_str(&v["title"]))) {
            continue;
        }
        let (d, args) = detail(&v)?;
        let title = ["title", "message", "summary", "text"]
            .iter()
            .find_map(|k| v.get(*k).filter(|v| truthy(v)))
            .unwrap_or(&row["event_type"]);
        let mut item = json!({"error":v["status"]=="failed"||row["event_type"]=="assignment_failed","sequence":row["id"].as_i64().unwrap()*2,
            "text":clipped(&match title { Value::String(s) => s.clone(), other => dump(other, false) }),"kind":if tool{"tool"}else{"note"},"at":at(db,&row["created_at"])?});
        if tool {
            item["args"] = json!(args);
            if truthy(&v["activity_ref"]) {
                item["toolCallRef"] = v["activity_ref"].clone();
            }
        }
        if truthy(&d["command"]) {
            item["kind"] = json!("command");
            item["text"] = json!(clipped(d["command"].as_str().unwrap_or("")));
            item["command"] = json!({"state":"native","exit":null,"took":null,"tail":clipped(d["output"].as_str().unwrap_or("")),"reason":null});
        } else if v["tool_kind"] == "edit" || d.get("newText").is_some() {
            let split = |s: &str| {
                let mut lines = Vec::new();
                let mut line = String::new();
                let mut chars = s.chars().peekable();
                while let Some(c) = chars.next() {
                    if matches!(
                        c as u32,
                        10 | 13 | 11 | 12 | 28 | 29 | 30 | 133 | 8232 | 8233
                    ) {
                        lines.push(std::mem::take(&mut line));
                        if c == '\r' && chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                    } else {
                        line.push(c);
                    }
                }
                if !line.is_empty() {
                    lines.push(line);
                }
                lines
            };
            let old = split(d["oldText"].as_str().unwrap_or(""));
            let new = split(d["newText"].as_str().unwrap_or(""));
            item["kind"] = json!("edit");
            item["diff"] = json!({"add":new.len(),"del":old.len(),"lines":old.iter().map(|s|format!("-{s}")).chain(new.iter().map(|s|format!("+{s}"))).collect::<Vec<_>>()});
        }
        out.push(item);
    }
    Ok(out)
}
pub(super) struct History<'a> {
    pub runtime: &'a str,
    pub refs: &'a [Value],
    pub channel: Option<&'a str>,
    pub task: &'a str,
    pub worker: &'a str,
    pub status: &'a str,
    pub binding: &'a str,
}
impl History<'_> {
    pub fn page(&self, db: &Connection, cursor: &Value, budget: usize) -> CoreResult<Value> {
        let mut before_t: i64 = db.query_row(
            "SELECT coalesce(max(id),0)+1 FROM task_events WHERE task_id=?",
            [self.runtime],
            |r| r.get(0),
        )?;
        let mut before_r: i64 = db.query_row(
            "SELECT coalesce(max(id),0)+1 FROM runtime_events WHERE task_id=?",
            [self.runtime],
            |r| r.get(0),
        )?;
        if !cursor.is_null() {
            let parts = cursor.as_str().unwrap_or("").split('.').collect::<Vec<_>>();
            if parts.len() != 4 || parts[0] != "wa2" || parts[1] != self.binding {
                return Err(CoreError::Refused("invalid_cursor"));
            }
            let parse = |s: &str| {
                s.parse::<i64>()
                    .ok()
                    .filter(|n| *n > 0 && n.to_string() == s)
            };
            before_t = parse(parts[2]).ok_or(CoreError::Refused("invalid_cursor"))?;
            before_r = parse(parts[3]).ok_or(CoreError::Refused("invalid_cursor"))?;
        }
        let marks = vec!["?"; self.refs.len()].join(",");
        let mut args = vec![json!(self.runtime)];
        args.extend_from_slice(self.refs);
        args.push(json!(before_t));
        args.extend(HIDDEN.iter().map(|s| json!(s)));
        let tasks=query(db,&format!("SELECT id,event_type,created_at,payload_json FROM task_events WHERE task_id=? AND assignment_ref IN ({marks}) AND id<? AND (event_type IN ('worker_result','worker_question','assignment_failed','assignment_cancelled') OR (json_valid(payload_json) AND json_extract(payload_json,'$.kind')='worker_activity' AND lower(trim(coalesce(json_extract(payload_json,'$.tool_kind'),''))) NOT IN ({}))) ORDER BY id DESC LIMIT 201",vec!["?";HIDDEN.len()].join(",")),&args)?;
        let runtime = if let Some(ch) = self.channel {
            query(db,"SELECT id,type,created_at,payload_json FROM runtime_events WHERE task_id=? AND id<? AND ((type='COMMAND_PROCESS_STARTED' AND actor=?) OR (type='WORKER_GUIDANCE' AND json_extract(payload_json,'$.channelId')=?)) ORDER BY id DESC LIMIT 201",&[json!(self.runtime),json!(before_r),json!(ch),json!(ch)])?
        } else {
            vec![]
        };
        let (mut ti, mut ri) = (0, 0);
        let mut pending = vec![];
        while ti < tasks.len() || ri < runtime.len() {
            let take = ti == tasks.len()
                || (ri < runtime.len()
                    && (
                        runtime[ri]["created_at"].as_str(),
                        runtime[ri]["id"].as_i64().unwrap() * 2 + 1,
                    ) > (
                        tasks[ti]["created_at"].as_str(),
                        tasks[ti]["id"].as_i64().unwrap() * 2,
                    ));
            if take {
                pending.push((&runtime[ri], true));
                ri += 1;
            } else {
                pending.push((&tasks[ti], false));
                ti += 1;
            }
        }
        let mut items: Vec<Value> = vec![];
        let mut messages = vec![];
        let mut consumed = 0;
        for (row, is_runtime) in pending.iter().take(200) {
            let (added, said) = if *is_runtime {
                self.controls(db, row)?
            } else {
                let payload = object(row["payload_json"].as_str().unwrap_or(""));
                let related = if let Some(r) = tool_ref(&payload) {
                    let err = r
                        .strip_prefix("tool:")
                        .map(|r| format!("tool-error:{r}"))
                        .unwrap_or_else(|| r.clone());
                    let sql=format!("SELECT id,event_type,created_at,payload_json FROM task_events WHERE task_id=? AND +assignment_ref IN ({marks}) AND json_extract(payload_json,'$.activity_ref') IN (?,?) AND id<? ");
                    let mut args = vec![json!(self.runtime)];
                    args.extend_from_slice(self.refs);
                    args.extend([json!(r), json!(err), json!(before_t)]);
                    let mut related =
                        query(db, &(sql.clone() + "ORDER BY +id DESC LIMIT 100"), &args)?;
                    related.reverse();
                    let first = query(db, &(sql + "ORDER BY +id LIMIT 1"), &args)?;
                    if let Some(first) = first.first() {
                        if related.first().is_none_or(|r| r["id"] != first["id"]) {
                            related.insert(0, first.clone());
                        }
                    }
                    if related.len() >= 100 {
                        if let Some(last) = related.last_mut() {
                            let mut p = object(last["payload_json"].as_str().unwrap_or(""));
                            p["projection_partial"] = json!(true);
                            last["payload_json"] = json!(p.to_string());
                        }
                    }
                    related
                } else {
                    vec![(*row).clone()]
                };
                (activity(db, &related)?, vec![])
            };
            let (t, r) = if *is_runtime {
                (before_t, row["id"].as_i64().unwrap())
            } else {
                (row["id"].as_i64().unwrap(), before_r)
            };
            let continuation = format!("wa2.{}.{t}.{r}", self.binding);
            let candidate = json!({"activity":items.iter().chain(added.iter()).collect::<Vec<_>>(),"messages":messages.iter().chain(said.iter()).collect::<Vec<_>>(),"activityPartial":true,"activityCursor":continuation});
            if bytes(&candidate) > budget {
                break;
            }
            for item in added {
                if !items.iter().any(|v| v["sequence"] == item["sequence"]) {
                    items.push(item);
                }
            }
            messages.extend(said);
            before_t = t;
            before_r = r;
            consumed += 1;
        }
        items.sort_by_key(|v| {
            (
                v["at"].as_i64().unwrap_or(0),
                v["sequence"].as_i64().unwrap_or(0),
            )
        });
        let partial = consumed < pending.len();
        Ok(
            json!({"activity":items,"messages":messages,"activityPartial":partial,"activityCursor":partial.then(||format!("wa2.{}.{before_t}.{before_r}",self.binding))}),
        )
    }
    fn controls(&self, db: &Connection, row: &Value) -> CoreResult<(Vec<Value>, Vec<Value>)> {
        let data = object(row["payload_json"].as_str().unwrap_or(""));
        let sequence = row["id"].as_i64().unwrap() * 2 + 1;
        let at = at(db, &row["created_at"])?;
        if row["type"] == "COMMAND_PROCESS_STARTED" {
            let r = data["tool_call_ref"].clone();
            let later=query(db,"SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type IN ('COMMAND_PROCESS_EXITED','COMMAND_STOPPED','COMMAND_STOP_MISSED','COMMAND_STOP_REQUESTED') AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id DESC LIMIT 1",&[json!(self.runtime),r.clone()])?;
            let detail = later
                .first()
                .map(|v| object(v["payload_json"].as_str().unwrap_or("")))
                .unwrap_or(json!({}));
            let kind = later.first().and_then(|v| v["type"].as_str()).unwrap_or("");
            let state = match kind {
                "COMMAND_STOP_REQUESTED" => "stopping",
                "COMMAND_STOPPED" => "stopped",
                "COMMAND_STOP_MISSED" => "failed",
                _ if later.is_empty() => "running",
                _ if detail["exit_code"] == 0 && !truthy(&detail["failure"]) => "passed",
                _ => "failed",
            };
            let command = data["argv"]
                .as_array()
                .filter(|a| a.iter().all(Value::is_string))
                .map(|a| shell(a))
                .unwrap_or_else(|| "Command text was not recorded".into());
            let completed=query(db,"SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_COMPLETED' AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id DESC LIMIT 1",&[json!(self.runtime),r.clone()])?;
            let result = completed
                .first()
                .map(|v| object(v["payload_json"].as_str().unwrap_or("")))
                .unwrap_or(json!({}));
            // Python: `history_text(str(...))` of whatever the retained receipt holds.
            let tail = result["result"]
                .get("output")
                .map(py_str)
                .unwrap_or_default();
            return Ok((
                vec![
                    json!({"sequence":sequence,"kind":"command","text":clipped(&command),"at":at,"toolCallRef":r,"command":{"state":state,"exit":detail["exit_code"],"took":null,"tail":clipped(&tail),"reason":truthy(&detail["reason"]).then(||clipped(&py_str(&detail["reason"])))}}),
                ],
                vec![],
            ));
        }
        let delivered=query(db,"SELECT type,created_at FROM runtime_events WHERE task_id=? AND type IN ('WORKER_GUIDANCE_DELIVERED','WORKER_GUIDANCE_WITHDRAWN') AND json_extract(payload_json,'$.messageEvent')=? ORDER BY id DESC LIMIT 1",&[json!(self.runtime),row["id"].clone()])?;
        let delivery = match delivered.first().and_then(|v| v["type"].as_str()) {
            Some("WORKER_GUIDANCE_DELIVERED") => "delivered",
            Some(_) => "withdrawn",
            None if ["working", "waiting_you"].contains(&self.status) => "recorded",
            None => "failed",
        };
        // A recorded guidance row without text (malformed) is shown as unavailable
        // content, never as invented text.
        let text = clipped(&match data.get("text") {
            Some(v) => py_str(v),
            None => "[Content unavailable]".to_owned(),
        });
        Ok((
            vec![
                json!({"sequence":sequence,"kind":"message","text":text,"messageId":data["requestId"],"at":at}),
            ],
            vec![
                json!({"id":data["requestId"],"taskId":self.task,"workerId":self.worker,"text":text,"mode":"send_now","delivery":delivery,"at":at,
                "deliveredAt":match delivered.first(){Some(v)=>super::worker_history::at(db,&v["created_at"])?,None=>Value::Null},
                "failure":(delivery=="failed").then_some("Not delivered · worker finished, stopped or replaced"),
                "attachments":data["attachmentNames"].as_array().into_iter().flatten().map(|a|a["name"].clone()).collect::<Vec<_>>()}),
            ],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_or_odd_dates_are_unparsed_never_a_panic() {
        for text in [
            "2026-1-1",
            "2026-11-1",
            "2026-1-1T",
            "2026-W",
            "2026-W1",
            "2026W",
            "2026-",
            "2026-11",
            "2026-1-1Z",
            "20261-1-1",
        ] {
            assert_eq!(iso_ms(text), None, "{text}");
        }
    }

    // Expected values come from CPython 3.14 `int(datetime.fromisoformat(
    // text.replace("Z", "+00:00")).timestamp() * 1000)`.
    #[test]
    fn other_iso_forms_match_the_reference_parser() {
        for (text, expected) in [
            ("2026-11-12T01:02:03+00:00", Some(1794445323000)),
            ("2026-11-12 01:02:03.5Z", Some(1794445323500)),
            ("20261112T010203+0200", Some(1794438123000)),
            ("2026-W46-4T01:02:03Z", Some(1794445323000)),
            ("2026W464T010203Z", Some(1794445323000)),
            ("2026-11-12T01:02:03,123456789-05:30", Some(1794465123123)),
            ("2026-11-12T24:00:00Z", Some(1794528000000)),
            ("2026-11-12T01:02:03.Z", Some(1794445323000)),
            ("1969-12-31T23:59:59.9999+00:00", Some(0)),
            ("2026-02-30T00:00:00Z", None),
            ("2026-11-12T25:00:00Z", None),
            ("2026-11-12T01:02:03+24:00", None),
            ("2026-11-12T01:02:03+0", None),
            ("abc", None),
            ("", None),
        ] {
            assert_eq!(iso_ms(text), expected, "{text}");
        }
        assert_eq!(
            at(&Connection::open_in_memory().unwrap(), &json!(5)).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn naive_values_are_local_time_like_the_reference() {
        // The date-only and hour-only forms are local midnight/hour; their offset
        // from the zoned form is the local UTC offset, a whole number of minutes.
        for text in [
            "2026-11-12",
            "2026-11-12T01",
            "2026-11-12 01:02:03.25",
            "2026W464",
        ] {
            let local = iso_ms(text).unwrap();
            let zoned = iso_ms(&format!("{}+00:00", text.trim_end()))
                .or_else(|| iso_ms(&format!("{text}T00:00:00+00:00")));
            if let Some(zoned) = zoned.filter(|_| text.contains(':') || text.contains('T')) {
                assert!((zoned - local).abs() <= 14 * 3_600_000, "{text}");
                assert_eq!((zoned - local) % 60_000, 0, "{text}");
            }
        }
        let midnight = iso_ms("2026-11-12").unwrap();
        assert_eq!(midnight, iso_ms("2026-11-12T00:00:00").unwrap());
        assert_eq!(midnight, iso_ms("20261112").unwrap());
    }
    #[test]
    fn str_of_decoded_json_matches_the_reference() {
        assert_eq!(py_str(&json!("plain")), "plain");
        assert_eq!(
            py_str(&json!({"a":[1,2.5,null,true,"it's"],"b":1e22})),
            "{'a': [1, 2.5, None, True, \"it's\"], 'b': 1e+22}"
        );
        assert_eq!(py_str(&json!(1e-5)), "1e-05");
        assert_eq!(py_str(&json!(1e16)), "1e+16");
        assert_eq!(py_str(&json!(123456789012345.0)), "123456789012345.0");
        assert_eq!(py_str(&json!(["a\nb", "q'\"x"])), "['a\\nb', 'q\\'\"x']");
        assert_eq!(py_str(&json!(false)), "False");
    }
    #[test]
    fn non_string_display_fields_are_shown_not_delegated() {
        let (d, args) =
            detail(&json!({"title":["Edit","x"],"toolCallId":7,"status":3,"tool_kind":5})).unwrap();
        assert_eq!(d["title"], "['Edit', 'x']");
        assert_eq!(d["callId"], "7");
        assert_eq!(d["status"], 3);
        assert!(args.starts_with("{\"title\": \"['Edit', 'x']\", \"callId\": \"7\""));
    }
    #[test]
    fn private_report_diff_blocks_are_withheld_with_a_notice() {
        let (d, _) = detail(&json!({
            "title":"cortex__write_report","tool_kind":"edit",
            "content":[{"type":"diff","path":"r.md","oldText":"","newText":"SECRET BODY"}]
        }))
        .unwrap();
        assert!(d.get("newText").is_none() && d.get("oldText").is_none());
        assert_eq!(
            d["notice"],
            "A private report body is not shown in this view."
        );
        assert_eq!(d["kind"], "other");
        let (d, _) = detail(&json!({
            "title":"edit","tool_kind":"edit",
            "content":[{"type":"diff","path":"a","oldText":"o","newText":"n"}]
        }))
        .unwrap();
        assert_eq!(d["newText"], "n");
    }
}

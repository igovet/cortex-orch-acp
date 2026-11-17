//! Display model of a conflict left in a task checkout (task_git.py `conflicts`).
//! Reads only; the user's files and index are never changed here.
use crate::machine::{git::Git, native::*};
use serde_json::{json, Value};
use std::path::Path;

const NOTE_TYPES: &str = "Add/delete, symlink or mode conflict. Resolve the intended file type, contents and executable bit in Git/editor, then git add (or git rm for deletion) and Finish merge.";
const NOTE_BINARY: &str = "Binary conflict. Resolve the intended bytes with Git/editor, git add the file, then Finish merge.";
const NOTE_LIMIT: &str =
    "Preview limit reached. Resolve this file with Git/editor, git add the file, then Finish merge.";

/// Python `str.splitlines` line boundaries.
fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut current));
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => lines.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

pub(super) async fn build(
    path: &Path,
    source: &str,
    target: &str,
    record: &Value,
    git: &mut Git,
) -> Result<Value> {
    let mut files: Vec<Value> = Vec::new();
    let mut remaining: usize = 30_000;
    let mut failure = Value::Null;
    let names = git
        .out(path, &["diff", "--name-only", "--diff-filter=U", "-z"])
        .await?;
    for name in names.split('\0').filter(|n| !n.is_empty()) {
        let literal = format!(":(literal){name}");
        let listing = git.out(path, &["ls-files", "-u", "--", &literal]).await?;
        let stages: Vec<Vec<String>> = listing
            .lines()
            .map(|line| {
                line.split_once('\t')
                    .map_or(line, |(a, _)| a)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
        let numbers: std::collections::BTreeSet<_> =
            stages.iter().filter_map(|s| s.get(2)).cloned().collect();
        let modes: std::collections::BTreeSet<_> =
            stages.iter().filter_map(|s| s.first()).cloned().collect();
        let mut note = String::new();
        if stages.iter().any(|s| s.len() != 3)
            || numbers != ["1", "2", "3"].iter().map(|s| s.to_string()).collect()
            || modes.len() != 1
            || modes.iter().any(|m| m != "100644" && m != "100755")
        {
            note = NOTE_TYPES.into();
        }
        let mut sides = serde_json::Map::new();
        for (key, stage) in [("base", 1), ("ours", 2), ("theirs", 3)] {
            let spec = format!(":{stage}:{name}");
            let size = if git.success(path, &["cat-file", "-e", &spec]).await? {
                git.out(path, &["cat-file", "-s", &spec]).await?
            } else {
                String::new()
            };
            let mut text = String::new();
            let admitted = size
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n <= 30_000.min(remaining));
            if admitted.is_some() && note.is_empty() {
                let raw = git.bytes(path, &["show", &spec]).await?;
                match String::from_utf8(raw.clone()) {
                    Ok(decoded) if !decoded.contains('\0') => {
                        remaining -= raw.len().min(remaining);
                        text = decoded;
                    }
                    _ => note = NOTE_BINARY.into(),
                }
            } else if note.is_empty() {
                note = NOTE_LIMIT.into();
            }
            sides.insert(
                key.into(),
                json!({"ref":key,"start":1,"lines":splitlines(&text),"hit":[]}),
            );
        }
        if !note.is_empty() {
            for side in sides.values_mut() {
                side["lines"] = json!([note]);
            }
        }
        let ours = sides["ours"]["lines"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let theirs = sides["theirs"]["lines"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let both: Vec<Value> = ours.iter().chain(theirs.iter()).cloned().collect();
        let mut block = json!({"id":name,"title":name,"choice":null,"cortexNote":note,
            "results":{
                "ours":{"start":1,"lines":ours,"hit":[]},
                "theirs":{"start":1,"lines":theirs,"hit":[]},
                "both":{"start":1,"lines":both,"hit":[]},
                "cortex":{"start":1,"lines":[],"hit":[]}}});
        for (key, value) in sides {
            block[key] = value;
        }
        files.push(json!({"path":name,"blocks":[block]}));
        if serde_json::to_vec(&files).map_or(usize::MAX, |v| v.len()) > 350_000
            || files.len() >= 100
        {
            files.pop();
            failure = json!("Conflict display budget reached. Additional paths remain unresolved. Use Git status in the task checkout, resolve and stage every file, then Finish merge.");
            break;
        }
    }
    Ok(
        json!({"source":target,"into":source,"startedAt":now_ms(),"handedAt":null,
        "finishedAt":null,"guidance":null,"step":0,"failure":failure,"finished":false,
        "files":files,"clean":[],"checks":[],
        "before":{"status":"ready_to_merge","acceptance":record["acceptance"],"result":null,"digest":null}}),
    )
}

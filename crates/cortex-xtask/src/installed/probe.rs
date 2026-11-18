//! Read-only service probes for a confirmation wait: timed framed RPCs against the desktop
//! user's own service socket, and the service's private diagnostic logs (event lines only).

use super::util::*;
use crate::json::{parse, Res, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One framed JSON-RPC call; its duration in ms and a short outcome (never the reply body).
pub fn rpc(socket: &Path, method: &str, params: Value, timeout: Duration) -> (i64, Res<Value>) {
    let started = Instant::now();
    let result = (|| -> Res<Value> {
        let mut stream = UnixStream::connect(socket).map_err(|e| format!("connect: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        let request = line(&obj(vec![
            ("jsonrpc", s("2.0")),
            ("id", n(1)),
            ("method", s(method)),
            ("params", params),
        ]));
        let mut frame = (request.len() as u32).to_be_bytes().to_vec();
        frame.extend(request.as_bytes());
        stream.write_all(&frame).map_err(|e| e.to_string())?;
        let mut head = [0u8; 4];
        stream
            .read_exact(&mut head)
            .map_err(|e| format!("reply: {e}"))?;
        let size = u32::from_be_bytes(head) as usize;
        require(size > 0 && size <= 64 * 1_048_576, "reply size")?;
        let mut body = vec![0u8; size];
        stream
            .read_exact(&mut body)
            .map_err(|e| format!("reply body: {e}"))?;
        parse(&String::from_utf8_lossy(&body))
    })();
    (started.elapsed().as_millis() as i64, result)
}

fn outcome(reply: &Res<Value>) -> String {
    match reply {
        Err(e) => format!("error:{e}"),
        Ok(v) => {
            if let Some(e) = v.get("error") {
                return format!(
                    "rpc_error:{}",
                    e.get("message").and_then(Value::as_str).unwrap_or("?")
                );
            }
            let r = v.get("result");
            match r.and_then(|r| r.get("ok")) {
                Some(Value::Bool(true)) => "ok".into(),
                Some(_) => format!(
                    "refused:{}",
                    r.and_then(|r| r.get("code"))
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                ),
                None => "ok".into(),
            }
        }
    }
}

/// Registered project ids of this account.
pub fn project_ids(socket: &Path) -> Vec<String> {
    let (_, reply) = rpc(
        socket,
        "projects.list",
        obj(vec![]),
        Duration::from_secs(10),
    );
    let Ok(reply) = reply else { return vec![] };
    let list = reply.get("result");
    let items = list
        .and_then(Value::as_arr)
        .or_else(|| list.and_then(|l| l.get("projects")).and_then(Value::as_arr))
        .unwrap_or(&[]);
    items
        .iter()
        .filter_map(|p| {
            p.get("projectId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// One round of the reads a board refresh makes, each timed.
pub fn round(socket: &Path, project: &str, label: &str, since: Instant) {
    let machine = |op: &str, params: Value| {
        obj(vec![
            ("op", s(op)),
            ("projectId", s(project)),
            ("params", params),
        ])
    };
    let native = |operation: &str| {
        obj(vec![
            ("protocol", obj(vec![("major", n(2)), ("mapping", n(2))])),
            ("requestId", s(format!("probe-{operation}"))),
            ("projectId", s(project)),
            ("operation", s(operation)),
            ("payload", obj(vec![("id", s(project))])),
        ])
    };
    let calls: Vec<(&str, &str, Value)> = vec![
        ("service.status", "service.status", obj(vec![])),
        (
            "machine.call",
            "product.read",
            machine("product.read", Value::Null),
        ),
        (
            "machine.call",
            "product.board",
            machine("product.board", Value::Null),
        ),
        (
            "machine.call",
            "product.facts",
            machine("product.facts", obj(vec![("taskIds", arr(vec![]))])),
        ),
        (
            "machine.call",
            "product.facts+defer",
            machine(
                "product.facts",
                obj(vec![("taskIds", arr(vec![])), ("defer", b(true))]),
            ),
        ),
        (
            "machine.call",
            "repo.snapshot",
            machine("repo.snapshot", Value::Null),
        ),
        ("native.board", "tasks.board", native("tasks.board")),
    ];
    let mut rows = Vec::new();
    for (method, name, params) in calls {
        let at = since.elapsed().as_millis() as i64;
        let (ms, reply) = rpc(socket, method, params, Duration::from_secs(40));
        rows.push(obj(vec![
            ("call", s(name)),
            ("at_ms", n(at)),
            ("ms", n(ms)),
            ("outcome", s(outcome(&reply))),
        ]));
    }
    emit(
        "confirmation_probe",
        vec![
            ("label", s(label)),
            ("project", s(project)),
            ("calls", arr(rows)),
        ],
    );
}

/// One timed round of reads for every registered project, then the service's own logs.
pub fn diagnose(socket: &Path, label: &str) {
    let since = Instant::now();
    let ids = project_ids(socket);
    emit(
        "confirmation_probe_projects",
        vec![("label", s(label)), ("count", n(ids.len() as i64))],
    );
    for id in &ids {
        round(socket, id, label, since);
    }
    dump_logs(label);
}

/// The service's private diagnostic logs of this account: event names, timings and route
/// fields only (they never hold prompts, answers or task text). Bounded to the last lines.
pub fn dump_logs(label: &str) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut stack = vec![home.join(".local/state")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if name == "short-text.log" || name == "service.log" {
                    found.push(path);
                }
            }
        }
    }
    for path in found {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let tail = &lines[lines.len().saturating_sub(40)..];
        emit(
            "confirmation_log",
            vec![
                ("label", s(label)),
                ("file", s(path.to_string_lossy())),
                ("lines", n(lines.len() as i64)),
                ("tail", arr(tail.iter().map(|l| s(*l)).collect())),
            ],
        );
    }
    // Processes of this user: command names only.
    let mut procs = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
            let argv0 = std::fs::read(entry.path().join("cmdline"))
                .ok()
                .and_then(|c| {
                    c.split(|b| *b == 0)
                        .next()
                        .map(|a| String::from_utf8_lossy(a).into_owned())
                })
                .unwrap_or_default();
            procs.push(s(format!("{name} {} {argv0}", comm.trim())));
        }
    }
    emit(
        "confirmation_processes",
        vec![("label", s(label)), ("processes", arr(procs))],
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    emit(
        "confirmation_clock",
        vec![("label", s(label)), ("unix_ms", n(now_ms))],
    );
}

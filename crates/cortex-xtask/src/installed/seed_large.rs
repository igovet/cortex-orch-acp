//! The populated-store fixture: 68 saved tasks, a 31-member sprint and 100,000 retained
//! history rows. Tasks are created through the running service's public workspace API; the
//! history rows are added only after that service has stopped.

use super::channel::Channel;
use super::util::*;
use crate::json::{parse, Res, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HOME: &str = "/home/acceptance";

pub fn rpc(socket: &Path, method: &str, params: Value) -> Res<Value> {
    // A just-started service may not be listening yet; anything else is a real failure.
    let started = std::time::Instant::now();
    let mut stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(e)
                if started.elapsed() < Duration::from_secs(30)
                    && matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(e) => return Err(format!("service socket: {e}")),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    let request = line(&obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", s("fixture")),
        ("method", s(method)),
        ("params", params),
    ]));
    let mut frame = (request.len() as u32).to_be_bytes().to_vec();
    frame.extend(request.as_bytes());
    stream.write_all(&frame).map_err(|e| e.to_string())?;
    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .map_err(|_| "service_closed".to_string())?;
    let size = u32::from_be_bytes(head) as usize;
    require(size > 0 && size <= 1_048_576, "service reply size")?;
    let mut body = vec![0u8; size];
    stream
        .read_exact(&mut body)
        .map_err(|_| "service_closed".to_string())?;
    let reply = parse(&String::from_utf8_lossy(&body))?;
    if reply.get("error").is_some() {
        return Err(format!(
            "rpc {method} failed: {}",
            line(reply.get("error").unwrap())
        ));
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| "rpc reply without result".to_string())
}

fn seed_file() -> PathBuf {
    Path::new(HOME).join("large-seed.json")
}

pub fn api(channel: &Channel) -> Res<()> {
    let root = Path::new(HOME).join("large-project");
    let registered = rpc(
        &channel.socket,
        "projects.register",
        obj(vec![
            ("root", s(root.to_string_lossy())),
            ("displayName", s("Large board fixture")),
        ]),
    )?;
    let project = registered
        .get("projectId")
        .and_then(Value::as_str)
        .ok_or("registration returned no project identity")?
        .to_string();
    rpc(
        &channel.socket,
        "projects.activate",
        obj(vec![("projectId", s(&project))]),
    )?;
    wait_secs("large project ready", 60, || {
        let status = rpc(
            &channel.socket,
            "projects.status",
            obj(vec![("projectId", s(&project))]),
        )?;
        Ok(status.get("ready").map(Value::truthy).unwrap_or(false))
    })?;
    let mut ids = Vec::new();
    for i in 0..68 {
        let request = obj(vec![
            ("protocol", obj(vec![("major", n(1)), ("minor", n(0))])),
            ("requestId", s(format!("fixture-create-{i}"))),
            ("projectId", s(&project)),
            ("actorId", s("uid:1001")),
            ("operation", s("tasks.create")),
            ("target", s(&project)),
            ("expectedVersion", s("0")),
            ("ownerEpoch", Value::Null),
            (
                "payload",
                obj(vec![(
                    "description",
                    s(format!("Large board saved task {i}")),
                )]),
            ),
        ]);
        let receipt = rpc(&channel.socket, "workspace", request)?;
        let receipt = receipt.get("receipt").cloned().unwrap_or(receipt);
        require(
            receipt.get("outcome").and_then(Value::as_str) == Some("recorded"),
            &format!("task creation not recorded: {}", line(&receipt)),
        )?;
        ids.push(
            receipt
                .get("target")
                .and_then(Value::as_str)
                .ok_or("receipt without target")?
                .to_string(),
        );
    }
    write(
        &seed_file(),
        &line(&obj(vec![
            ("projectId", s(&project)),
            ("tasks", arr(ids.iter().map(s).collect())),
        ])),
    )?;
    Ok(())
}

pub fn store(channel: &Channel) -> Res<()> {
    let seed = parse(&read_to_string(&seed_file())?)?;
    let project = seed
        .get("projectId")
        .and_then(Value::as_str)
        .ok_or("seed without project")?
        .to_string();
    let ids: Vec<Value> = seed
        .get("tasks")
        .and_then(Value::as_arr)
        .ok_or("seed without tasks")?
        .to_vec();
    require(ids.len() == 68, "seed does not hold 68 tasks")?;
    let root = Path::new(HOME).join("large-project");
    let db = channel.store_db(&root);
    let padding = "x".repeat(1000);
    let update = format!("{{\"sessionUpdate\": \"usage_update\", \"padding\": \"{padding}\"}}");
    let sql = format!(
        "BEGIN;\
         INSERT INTO tasks VALUES ('large-history-runtime','{root}','2026-11-11T00:00:00Z');\
         INSERT INTO acp_host_sessions VALUES ('unrelated-fixture-history','{root}','large-history-runtime','{{}}','{{}}','[]','2026-11-11T00:00:00Z',NULL);\
         WITH RECURSIVE c(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM c WHERE i<99999) \
         INSERT INTO host_updates SELECT 'unrelated-fixture-history', i, '{update}', '2026-11-11T00:00:00Z' FROM c;\
         COMMIT;",
        root = root.display()
    );
    let done = capture(
        std::process::Command::new("sqlite3")
            .args(["-batch", "-cmd", ".timeout 30000"])
            .arg(&db)
            .arg(sql),
        Duration::from_secs(300),
    )?;
    require(
        done.code == 0,
        &format!("history rows failed: {}", tail(&done.stderr)),
    )?;
    let rows = super::native::query(
        &db,
        "SELECT count(*) FROM host_updates WHERE session_id='unrelated-fixture-history'",
    )
    .unwrap_or_default();
    let sprint = obj(vec![
        ("id", s("sp_large")),
        ("projectId", s(&project)),
        ("description", s("Large saved sprint")),
        ("title", s("Large saved sprint")),
        ("runMode", s("automatic")),
        ("targetBranch", Value::Null),
        ("planning", s("created")),
        ("conversation", arr(vec![])),
        ("plan", Value::Null),
        ("materializedVersion", n(1)),
        ("taskIds", arr(ids[..31].to_vec())),
        ("queueState", s("idle")),
        ("pauseReason", Value::Null),
    ]);
    let product = channel
        .state(Path::new(HOME))
        .join("product")
        .join(format!("{project}.json"));
    write(
        &product,
        &line(&obj(vec![
            ("tasks", obj(vec![])),
            ("sprints", arr(vec![sprint])),
        ])),
    )?;
    chmod(&product, 0o600)?;
    emit("large_store_rows", vec![("host_updates", s(rows))]);
    Ok(())
}

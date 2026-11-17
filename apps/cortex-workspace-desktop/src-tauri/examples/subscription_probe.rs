//! Read-only diagnostic for the desktop's machine-service subscription.
//!
//! `subscription_probe <socket> <seconds>` subscribes the way the desktop does, applies the
//! desktop's own frame validation (`client::incoming`) to every event and prints one line per
//! frame: elapsed time, event type, frame bytes and the validation outcome. It times every
//! resync read the desktop would issue (`projects.list`) and a `service.status` liveness read
//! every three seconds, as the desktop window does. It sends no other request and never prints
//! frame content: refusals name fixed protocol codes and schema paths only.
use cortex_workspace_desktop::client::{frame_failure, incoming, Bridge, Incoming, Refused};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;

/// Quoted strings in a serde message can be data; only backticked schema words are kept.
fn redact(text: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
            out.push_str(if quoted { "\"<" } else { ">\"" });
        } else if !quoted {
            out.push(c);
        }
    }
    out.chars().take(240).collect()
}

/// Which part of a refused delta failed, by schema path only.
fn explain(event: &Value) -> String {
    let batch = &event["batch"];
    let bytes = serde_json::to_vec(batch).map(|b| b.len()).unwrap_or(0);
    let mut kinds = BTreeMap::<String, usize>::new();
    let mut detail = Vec::new();
    for change in batch["changes"].as_array().into_iter().flatten() {
        let entity = &change["operation"]["data"];
        let label = format!(
            "{}/{}/{}",
            change["kind"].as_str().unwrap_or("?"),
            change["operation"]["type"].as_str().unwrap_or("?"),
            entity["kind"].as_str().unwrap_or("-")
        );
        *kinds.entry(label).or_default() += 1;
        if entity["kind"] == "task_projection" {
            if let Err(error) = workspace_protocol::validate("Task", &entity["data"]) {
                detail.push(format!("task_schema:{}", redact(&error)));
            }
        }
        if let Err(error) =
            serde_json::from_value::<workspace_protocol::events::EntityChange>(change.clone())
        {
            detail.push(format!("change_shape:{}", redact(&error.to_string())));
        }
        if change["newVersion"]
            .as_str()
            .zip(change["baseVersion"].as_str())
            .is_some_and(|(new, base)| new.parse::<u64>().ok() <= base.parse::<u64>().ok())
        {
            detail.push("version_not_increasing".into());
        }
    }
    if let Err(error) =
        serde_json::from_value::<workspace_protocol::events::DeltaBatch>(batch.clone())
    {
        detail.push(format!("batch_shape:{}", redact(&error.to_string())));
    }
    detail.sort();
    detail.dedup();
    detail.truncate(6);
    format!("batch_bytes={bytes} changes={kinds:?} detail={detail:?}")
}

async fn timed(bridge: &Bridge, method: &str) -> (u128, String) {
    let started = Instant::now();
    let outcome = match bridge.rpc(method, json!({})).await {
        Ok(value) if method == "projects.list" => format!(
            "ok active_projects={}",
            value
                .as_array()
                .map(|rows| rows.iter().filter(|p| !p["activation"].is_null()).count())
                .unwrap_or(0)
        ),
        Ok(_) => "ok".into(),
        Err(error) => format!("error={}", redact(&error)),
    };
    (started.elapsed().as_millis(), outcome)
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let socket = PathBuf::from(args.next().expect("socket path"));
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);
    let started = Instant::now();
    let at = || format!("{:>8.3}s", started.elapsed().as_secs_f64());
    let bridge = Bridge::new(socket.clone());
    let mut stream = workspace_service::connect(&socket).await.expect("connect");
    workspace_service::write_frame(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":"probe-subscribe","method":"service.subscribe","params":{}}),
    )
    .await
    .expect("subscribe write");
    let ack = tokio::time::timeout(
        Duration::from_secs(5),
        workspace_service::read_frame(&mut stream),
    )
    .await
    .map(|r| r.map(|v| v["result"]["subscribed"] == true));
    println!(
        "{} subscribe_ack {:?}",
        at(),
        ack.as_ref().map(|r| r.as_ref().ok())
    );
    let (ms, outcome) = timed(&bridge, "projects.list").await;
    println!("{} resync projects.list ms={ms} {outcome}", at());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    let mut refusals = 0usize;
    let mut status = tokio::time::interval(Duration::from_secs(3));
    let mut status_worst = 0u128;
    let mut status_failures = 0usize;
    let mut status_reads = 0usize;
    let end_reason = loop {
        // A frame read is not cancellation-safe; status reads run beside it, not instead of it.
        let read = async {
            let n = stream.read_u32().await? as usize;
            if n == 0 || n > workspace_protocol::FRAME_BYTES {
                return Err(std::io::Error::other("frame_too_large"));
            }
            let mut bytes = vec![0; n];
            stream.read_exact(&mut bytes).await?;
            let mut frame = (n as u32).to_be_bytes().to_vec();
            frame.extend(bytes);
            workspace_protocol::decode_frame(&frame)
                .map(|value| (n, value))
                .map_err(|_| std::io::Error::other("invalid_json"))
        };
        tokio::pin!(read);
        let frame = loop {
            tokio::select! {
                frame = &mut read => break Some(frame),
                _ = tokio::time::sleep_until(deadline) => break None,
                _ = status.tick() => {
                    let (ms, outcome) = timed(&bridge, "service.status").await;
                    status_reads += 1;
                    status_worst = status_worst.max(ms);
                    if outcome != "ok" || ms > 1000 {
                        if outcome != "ok" { status_failures += 1; }
                        println!("{} liveness service.status ms={ms} {outcome}", at());
                    }
                }
            }
        };
        let Some(frame) = frame else {
            break "deadline".to_string();
        };
        let (bytes, message) = match frame {
            Ok(frame) => frame,
            Err(error) => break format!("frame_read_failed:{}", frame_failure(&error)),
        };
        let kind = message["params"]["type"].as_str().unwrap_or("?").to_owned();
        let entry = counts.entry(kind.clone()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(bytes);
        match incoming(&message) {
            Ok(Incoming::Resync) => {
                let (ms, outcome) = timed(&bridge, "projects.list").await;
                println!(
                    "{} event={kind} bytes={bytes} reason={} -> resync projects.list ms={ms} {outcome}",
                    at(),
                    message["params"]["reason"].as_str().map(redact).unwrap_or_default()
                );
            }
            Ok(Incoming::Forward(_)) => {
                if kind != "native_delta" || bytes > 32 * 1024 {
                    println!("{} event={kind} bytes={bytes} ok", at());
                }
            }
            Err(Refused { code, project }) => {
                refusals += 1;
                println!(
                    "{} event={kind} bytes={bytes} REFUSED code={code} recoverable={} {}",
                    at(),
                    project.is_some(),
                    if kind == "native_delta" {
                        explain(&message["params"])
                    } else {
                        String::new()
                    }
                );
            }
        }
    };
    println!(
        "{} end reason={end_reason} refusals={refusals} counts(type:(frames,max_bytes))={counts:?} status_reads={status_reads} status_failures={status_failures} status_worst_ms={status_worst}",
        at()
    );
}

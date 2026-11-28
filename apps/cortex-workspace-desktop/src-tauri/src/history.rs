//! A single visible local conversation. The webview awaits one frame at a time,
//! so a busy renderer applies socket backpressure without an IPC event queue.
use crate::client::Bridge;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use workspace_client::{
    frames::{read_frame, write_frame},
    Stream,
};

#[derive(Default)]
pub struct History(Mutex<Option<Session>>);

struct Session {
    id: String,
    params: Value,
    socket: Arc<AsyncMutex<Option<Stream>>>,
    pending: Option<oneshot::Sender<()>>,
}

fn valid(params: &Value) -> bool {
    serde_json::from_value::<workspace_protocol::read::HistoryFollowRequest>(params.clone())
        .is_ok_and(|request| request.validate().is_ok())
}

async fn receive(
    bridge: &Bridge,
    socket: &AsyncMutex<Option<Stream>>,
    params: &Value,
) -> Result<Value, &'static str> {
    let mut held = socket.lock().await;
    if held.is_none() {
        *held = Some(tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut stream = bridge.history_socket().await?;
            write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":"history","method":"native.history.follow","params":params}))
                .await.map_err(|_| "history_disconnected")?;
            let ack = read_frame(&mut stream).await.map_err(|_| "history_disconnected")?;
            if ack != json!({"jsonrpc":"2.0","id":"history","result":{"subscribed":true}}) {
                return Err("history_refused");
            }
            Ok(stream)
        }).await.map_err(|_| "history_timeout")??);
    }
    let mut frame = read_frame(held.as_mut().unwrap())
        .await
        .map_err(|_| "history_disconnected")?;
    let event = &frame["params"];
    if frame["jsonrpc"] != "2.0"
        || frame["method"] != "service.event"
        || frame.get("id").is_some()
        || !matches!(
            event["type"].as_str(),
            Some("native_history_page" | "native_history_resync")
        )
        || ["projectId", "taskId", "executionId"]
            .iter()
            .any(|k| event[k] != params[k])
    {
        return Err("history_invalid");
    }
    Ok(frame["params"].take())
}

impl History {
    pub fn close(&self, id: &str) {
        let mut active = self.0.lock().unwrap();
        if active.as_ref().is_some_and(|s| s.id == id) {
            active.take();
        }
    }

    pub fn open(&self, id: String, params: Value) -> Result<(), String> {
        if id.is_empty() || id.len() > 80 || !valid(&params) {
            return Err("history_request_refused".into());
        }
        *self.0.lock().unwrap() = Some(Session {
            id,
            params,
            socket: Default::default(),
            pending: None,
        });
        Ok(())
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().take();
    }

    pub async fn next(&self, bridge: &Bridge, id: &str) -> Result<Value, String> {
        let (cancel, stopped) = oneshot::channel();
        let (socket, params) = {
            let mut active = self.0.lock().unwrap();
            let session = active
                .as_mut()
                .filter(|s| s.id == id && s.pending.is_none())
                .ok_or("history_closed")?;
            session.pending = Some(cancel);
            (session.socket.clone(), session.params.clone())
        };
        let result = tokio::select! {
            _ = stopped => return Err("history_closed".into()),
            result = receive(bridge, &socket, &params) => result
        };
        let mut active = self.0.lock().unwrap();
        let Some(session) = active.as_mut().filter(|s| Arc::ptr_eq(&s.socket, &socket)) else {
            return Err("history_closed".into());
        };
        session.pending.take();
        if result.is_err()
            || result
                .as_ref()
                .is_ok_and(|e| e["type"] == "native_history_resync")
        {
            active.take();
        }
        result.map_err(str::to_owned)
    }
}

//! The phone's side of remote access, without any web view: the paired machines, which one is
//! open, pairing, and the bridge to the open machine. The Android shell exposes exactly these
//! entry points as its closed command set; the host builds and tests this module as is.
//!
//! The interface never supplies an address, a key or a relay: it names a machine by the id the
//! pairing produced, and everything else is read from the phone's private state.

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};
use workspace_client::{policy, Bridge, RemoteConnector};
use workspace_remote::{
    link::LinkError, Client, ClientConfig, ConnectionState, PairError, PairingAttempt, PathKind,
    RemoteConnection, INCOMPATIBLE_VERSION_CODE,
};

/// Receives the two event streams of the shell: `phone-state` and `workspace-transport`.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// A remote machine answers slower than a local socket, and a lost one must not hold the
/// interface for the desktop's start-up allowance.
const STARTUP_GRACE: Duration = Duration::from_secs(5);
/// How long the subscription waits before it tries a lost machine again.
const RESUBSCRIBE: Duration = Duration::from_secs(2);
const SETTINGS: &str = "phone.json";

struct Link {
    connection: RemoteConnection,
    bridge: Arc<Bridge>,
    watcher: JoinHandle<()>,
}
impl Drop for Link {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

#[derive(Default)]
struct Inner {
    active: Option<String>,
    links: HashMap<String, Link>,
    subscription: Option<JoinHandle<()>>,
    attempt: Option<PairingAttempt>,
    waiting: Option<oneshot::Sender<()>>,
}

pub struct Phone {
    client: Client,
    device_name: String,
    settings: PathBuf,
    emit: Emit,
    inner: Mutex<Inner>,
    this: Weak<Phone>,
}

fn pair_code(error: &PairError, begin: bool) -> &'static str {
    match error {
        PairError::InvalidLink(LinkError::UnsupportedVersion(_)) => {
            "unsupported_pairing_link_version"
        }
        PairError::InvalidLink(_) => "invalid_link",
        PairError::IncompatibleVersion(_) => INCOMPATIBLE_VERSION_CODE,
        PairError::Expired => "expired",
        PairError::Denied => "denied",
        PairError::Cancelled => "cancelled",
        PairError::Unreachable => "unreachable",
        // A wrong or used secret, a refused name or a broken exchange: the computer did not take it.
        _ if begin => "refused",
        _ => "unreachable",
    }
}

fn machine_connection(
    seen: Option<ConnectionState>,
    last: Option<u64>,
) -> (&'static str, Option<&'static str>, Option<u64>) {
    match seen {
        Some(ConnectionState::Connecting) => ("connecting", None, last),
        Some(ConnectionState::Connected { path }) => (
            "connected",
            Some(match path {
                PathKind::Direct => "direct",
                PathKind::Relay => "relay",
                PathKind::Unknown => "unknown",
            }),
            last,
        ),
        Some(ConnectionState::Offline { last_connected_ms }) => {
            ("offline", None, last_connected_ms.or(last))
        }
        Some(ConnectionState::Rejected) => ("rejected", None, last),
        Some(ConnectionState::IncompatibleVersion) => ("update_needed", None, last),
        None => ("offline", None, last),
    }
}

impl Phone {
    /// `state_dir` is the app's private data folder. Must be called inside the Tokio runtime.
    pub fn open(state_dir: &Path, device_name: &str, emit: Emit) -> Result<Arc<Self>, String> {
        let client = Client::new(ClientConfig::new(state_dir.join("remote")))
            .map_err(|_| "phone_state_unavailable".to_string())?;
        let device_name = workspace_remote::link::validate_name(device_name)
            .unwrap_or_else(|| "Android phone".into());
        let settings = state_dir.join(SETTINGS);
        let saved = std::fs::read(&settings)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["active"].as_str().map(str::to_owned));
        let phone = Arc::new_cyclic(|this| Self {
            client,
            device_name,
            settings,
            emit,
            inner: Mutex::new(Inner::default()),
            this: this.clone(),
        });
        let machines = phone.client.machines();
        let active = saved
            .filter(|id| machines.iter().any(|m| &m.machine_id == id))
            .or_else(|| machines.first().map(|m| m.machine_id.clone()));
        for machine in &machines {
            phone.link(&machine.machine_id)?;
        }
        if let Some(id) = active {
            phone.open_machine(&id)?;
        }
        // The other machines are only asked whether they answer, so the list shows the truth.
        for machine in &machines {
            phone.probe(&machine.machine_id);
        }
        Ok(phone)
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn publish(&self) {
        (self.emit)("phone-state", self.state());
    }

    /// The connection and bridge of one paired machine, made once.
    fn link(&self, machine_id: &str) -> Result<(), String> {
        if self.lock().links.contains_key(machine_id) {
            return Ok(());
        }
        let connection = self
            .client
            .connection(machine_id)
            .map_err(|_| "unknown_machine".to_string())?;
        let bridge = Arc::new(
            Bridge::phone(Arc::new(RemoteConnector::new(connection.clone())))
                .with_startup_grace(STARTUP_GRACE),
        );
        let mut changes = connection.state();
        let this = self.this.clone();
        let watcher = tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                match this.upgrade() {
                    Some(phone) => phone.publish(),
                    None => break,
                }
            }
        });
        self.lock().links.insert(
            machine_id.to_owned(),
            Link {
                connection,
                bridge,
                watcher,
            },
        );
        Ok(())
    }

    /// Asks a machine whether it answers, without sending it anything.
    fn probe(&self, machine_id: &str) {
        let connection = match self.lock().links.get(machine_id) {
            Some(link) => link.connection.clone(),
            None => return,
        };
        tokio::spawn(async move {
            drop(connection.open().await);
        });
    }

    /// Makes one machine the open one: its events, and only its events, reach the interface.
    fn open_machine(&self, machine_id: &str) -> Result<(), String> {
        self.link(machine_id)?;
        let bridge = {
            let mut inner = self.lock();
            if let Some(previous) = inner.subscription.take() {
                previous.abort();
            }
            inner.active = Some(machine_id.to_owned());
            inner.links[machine_id].bridge.clone()
        };
        let emit = self.emit.clone();
        let id = machine_id.to_owned();
        let subscription = tokio::spawn(async move {
            loop {
                let (emit, id) = (emit.clone(), id.clone());
                bridge
                    .subscribe(move |event| {
                        emit(
                            "workspace-transport",
                            json!({"machineId": id, "event": event}),
                        )
                    })
                    .await;
                tokio::time::sleep(RESUBSCRIBE).await;
            }
        });
        self.lock().subscription = Some(subscription);
        self.save();
        Ok(())
    }

    fn save(&self) {
        let active = self.lock().active.clone();
        let body = serde_json::to_vec(&json!({"active": active})).unwrap_or_default();
        let staged = self.settings.with_extension("json.new");
        if std::fs::write(&staged, body).is_ok() {
            let _ = std::fs::rename(&staged, &self.settings);
        }
    }

    /// What the interface draws: every paired machine with its connection, and the open one.
    pub fn state(&self) -> Value {
        let inner = self.lock();
        let machines: Vec<Value> = self
            .client
            .machines()
            .into_iter()
            .map(|machine| {
                let seen = inner
                    .links
                    .get(&machine.machine_id)
                    .map(|link| link.connection.state().borrow().clone());
                let (state, path, last) = machine_connection(seen, machine.last_connected_ms);
                json!({"id": machine.machine_id, "name": machine.name, "state": state, "path": path, "lastConnectedAt": last})
            })
            .collect();
        json!({"active": inner.active, "machines": machines})
    }

    /// One request of the interface to the open machine, checked against the phone's allowlist
    /// before anything is sent.
    pub async fn request(
        &self,
        machine_id: &str,
        operation: String,
        message: Value,
    ) -> Result<Value, String> {
        let bridge = {
            let inner = self.lock();
            if inner.active.as_deref() != Some(machine_id) {
                return Err("machine_not_open".into());
            }
            inner.links.get(machine_id).map(|link| link.bridge.clone())
        }
        .ok_or_else(|| "unknown_machine".to_string())?;
        let call = policy::authorize_phone(json!({"operation": operation, "message": message}))?;
        bridge.request(call).await
    }

    /// Connects to the computer named by a scanned or pasted link and presents this phone.
    pub async fn pair_begin(&self, link: &str) -> Result<Value, String> {
        self.pair_cancel().await;
        let attempt = self
            .client
            .pair_begin(link, &self.device_name)
            .await
            .map_err(|error| pair_code(&error, true).to_string())?;
        let reply =
            json!({"machineName": attempt.machine_name, "fingerprint": attempt.fingerprint});
        self.lock().attempt = Some(attempt);
        Ok(reply)
    }

    /// Waits for the owner's answer at the computer. The machine is kept only when it allowed.
    pub async fn pair_finish(&self, name: &str) -> Result<Value, String> {
        let (attempt, cancelled) = {
            let mut inner = self.lock();
            let attempt = inner
                .attempt
                .take()
                .ok_or_else(|| "cancelled".to_string())?;
            let (stop, cancelled) = oneshot::channel();
            inner.waiting = Some(stop);
            (attempt, cancelled)
        };
        let outcome = tokio::select! {
            outcome = attempt.finish() => outcome.map_err(|error| pair_code(&error, false).to_string()),
            // Dropping the attempt closes its connection, which withdraws the request.
            _ = cancelled => Err("cancelled".to_string()),
        };
        self.lock().waiting = None;
        let machine = outcome?;
        if let Some(name) = workspace_remote::link::validate_name(name) {
            if name != machine.name {
                let _ = self.client.rename(&machine.machine_id, &name);
            }
        }
        self.link(&machine.machine_id)?;
        if self.lock().active.is_none() {
            self.open_machine(&machine.machine_id)?;
        } else {
            self.probe(&machine.machine_id);
        }
        self.publish();
        Ok(json!({"machineId": machine.machine_id}))
    }

    pub async fn pair_cancel(&self) {
        let (attempt, waiting) = {
            let mut inner = self.lock();
            (inner.attempt.take(), inner.waiting.take())
        };
        if let Some(stop) = waiting {
            let _ = stop.send(());
        }
        if let Some(attempt) = attempt {
            attempt.cancel().await;
        }
    }

    pub fn select(&self, machine_id: &str) -> Result<(), String> {
        if !self
            .client
            .machines()
            .iter()
            .any(|m| m.machine_id == machine_id)
        {
            return Err("unknown_machine".into());
        }
        self.open_machine(machine_id)?;
        self.publish();
        Ok(())
    }

    pub fn rename(&self, machine_id: &str, name: &str) -> Result<(), String> {
        let name = workspace_remote::link::validate_name(name)
            .ok_or_else(|| "invalid_name".to_string())?;
        self.client
            .rename(machine_id, &name)
            .map_err(|_| "unknown_machine".to_string())?;
        self.publish();
        Ok(())
    }

    /// Removes a machine from this phone. The computer keeps its record until the owner revokes it there.
    pub async fn forget(&self, machine_id: &str) -> Result<(), String> {
        let was_open = {
            let mut inner = self.lock();
            inner.links.remove(machine_id);
            let was_open = inner.active.as_deref() == Some(machine_id);
            if was_open {
                if let Some(subscription) = inner.subscription.take() {
                    subscription.abort();
                }
                inner.active = None;
            }
            was_open
        };
        self.client
            .forget(machine_id)
            .await
            .map_err(|_| "unknown_machine".to_string())?;
        if was_open {
            match self.client.machines().first() {
                Some(next) => self.open_machine(&next.machine_id.clone())?,
                None => self.save(),
            }
        }
        self.publish();
        Ok(())
    }

    /// Tries a machine again now instead of waiting for the next attempt.
    pub fn retry(&self, machine_id: &str) -> Result<(), String> {
        if !self.lock().links.contains_key(machine_id) {
            return Err("unknown_machine".into());
        }
        self.probe(machine_id);
        Ok(())
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        if let Some(subscription) = self.lock().subscription.take() {
            subscription.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace_remote::VersionInfo;

    #[test]
    fn distinguishes_pairing_link_and_transport_versions_from_invalid_links() {
        for begin in [true, false] {
            assert_eq!(
                pair_code(&PairError::InvalidLink(LinkError::Malformed("test")), begin),
                "invalid_link"
            );
            assert_eq!(
                pair_code(
                    &PairError::InvalidLink(LinkError::UnsupportedVersion(42)),
                    begin
                ),
                "unsupported_pairing_link_version"
            );
            assert_eq!(
                pair_code(
                    &PairError::IncompatibleVersion(VersionInfo {
                        supported: vec![],
                        wire: "2.0".into(),
                    }),
                    begin
                ),
                "remote_incompatible_version"
            );
        }
    }

    #[test]
    fn reports_refused_machines_separately_from_offline() {
        for (state, expected) in [
            (Some(ConnectionState::Rejected), "rejected"),
            (Some(ConnectionState::IncompatibleVersion), "update_needed"),
            (None, "offline"),
        ] {
            assert_eq!(
                machine_connection(state, Some(42)),
                (expected, None, Some(42))
            );
        }
    }
}

//! Local, read-only Code language sessions. Buffers never enter the project store.
#[cfg(test)]
mod tests;
mod transport;

use super::{context, files, git::Git, native::*, settings, MachineCall};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, Mutex},
    task::JoinHandle,
    time::{sleep_until, timeout, Instant},
};
use transport::{Peer, Program};

const MAX_PROCESSES: usize = 8;
const MAX_DOCS: usize = 12;
const IDLE: Duration = Duration::from_secs(180);
const CALL: Duration = Duration::from_secs(15);
const DIAGNOSTIC_LIMIT: usize = 64;
const DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(200);
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(8);
const DIAGNOSTIC_RETRY_INITIAL: Duration = Duration::from_secs(1);
const DIAGNOSTIC_RETRY_MAX: Duration = Duration::from_secs(8);
const MAX_VERSION: u32 = i32::MAX as u32;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Language {
    Rust,
    TypeScript,
}

fn language(path: &str) -> Option<(Language, &'static str)> {
    use Language::*;
    Some(match Path::new(path).extension()?.to_str()? {
        "rs" => (Rust, "rust"),
        "ts" | "mts" | "cts" => (TypeScript, "typescript"),
        "tsx" => (TypeScript, "typescriptreact"),
        "js" | "mjs" | "cjs" => (TypeScript, "javascript"),
        "jsx" => (TypeScript, "javascriptreact"),
        _ => return None,
    })
}

pub(super) const SETTINGS: [&str; 2] = ["rustAnalyzerPath", "typescriptLanguageServerPath"];

fn unavailable(reason: &'static str) -> Refused {
    refuse(
        "lsp_unavailable",
        "Language features are unavailable. Editing and saving still work.",
    )
    .with(json!({"reason":reason}))
}
fn invalid() -> Refused {
    refuse("invalid_request", "Invalid Code language request.")
}
fn stale() -> Refused {
    refuse(
        "lsp_document_stale",
        "Refresh the language session for this file.",
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Open {
    file: String,
    checkout_id: Option<String>,
    task_id: Option<String>,
    text: String,
    version: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Handle {
    document_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    document_id: String,
    text: String,
    version: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Definition {
    document_id: String,
    version: u32,
    position: Position,
}
#[derive(Copy, Clone, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(deny_unknown_fields)]
struct Position {
    line: u32,
    character: u32,
}

fn bounded(text: &str, version: u32) -> bool {
    text.len() <= files::MAX_FILE && !text.contains('\0') && (1..=MAX_VERSION).contains(&version)
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone)]
struct Key {
    project: String,
    checkout: PathBuf,
    language: Language,
}

enum Action {
    Open(Document),
    Status(String),
    Close(String),
    Change(Change),
    Definition(Definition),
}
type Reply = oneshot::Sender<Result<Value>>;

struct Slot {
    id: String,
    tx: mpsc::Sender<(Action, Reply)>,
    join: JoinHandle<()>,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.join.abort();
    }
}

pub(crate) struct Manager {
    slots: Mutex<BTreeMap<Key, Slot>>,
    events: broadcast::Sender<Value>,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            events: broadcast::channel(64).0,
        }
    }
}
impl Manager {
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub(crate) async fn close(&self, project: Option<&str>) {
        let mut slots = self.slots.lock().await;
        let keys: Vec<_> = slots
            .keys()
            .filter(|key| project.is_none_or(|id| id == key.project))
            .cloned()
            .collect();
        let mut stopped = Vec::new();
        for key in keys {
            if let Some(slot) = slots.remove(&key) {
                slot.join.abort();
                stopped.push(slot);
            }
        }
        for slot in &mut stopped {
            let _ = (&mut slot.join).await;
        }
    }

    async fn submit(tx: mpsc::Sender<(Action, Reply)>, action: Action) -> Result<Value> {
        let (reply, answer) = oneshot::channel();
        tx.try_send((action, reply)).map_err(|e| match e {
            mpsc::error::TrySendError::Closed(_) => stale(),
            mpsc::error::TrySendError::Full(_) => unavailable("busy"),
        })?;
        timeout(CALL, answer)
            .await
            .map_err(|_| unavailable("busy"))?
            .map_err(|_| stale())?
    }

    async fn existing(&self, project: &str, doc: &str) -> Result<mpsc::Sender<(Action, Reply)>> {
        let slot = doc.get(..32).ok_or_else(invalid)?;
        self.slots
            .lock()
            .await
            .iter()
            .find(|(key, value)| key.project == project && slot == value.id)
            .map(|(_, value)| value.tx.clone())
            .ok_or_else(stale)
    }
}

fn random_id() -> io::Result<String> {
    Ok(crate::identity()?[3..].to_owned())
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T> {
    serde_json::from_value(params).map_err(|_| invalid())
}

pub(super) fn handles(op: &str) -> bool {
    matches!(
        op,
        "repo.lsp.open"
            | "repo.lsp.change"
            | "repo.lsp.close"
            | "repo.lsp.status"
            | "repo.lsp.definition"
            | "repo.lsp.settings"
    )
}

pub(super) async fn call(service: &crate::Service, call: MachineCall) -> io::Result<Value> {
    Ok(envelope(run(service, call).await))
}

async fn run(service: &crate::Service, call: MachineCall) -> Result<Value> {
    service.accepting()?;
    if call.op == "repo.lsp.settings" {
        if call.params != json!({}) || call.project_id.is_some() {
            return Err(invalid());
        }
        return settings::language_paths(&service.config.state_dir);
    }
    let project = call.project_id.ok_or_else(invalid)?;
    let manager = &service.code_lsp;
    if call.op == "repo.lsp.open" {
        let open: Open = parse(call.params)?;
        if !bounded(&open.text, open.version)
            || [&open.checkout_id, &open.task_id]
                .into_iter()
                .flatten()
                .any(|id| workspace_protocol::validate("Identity", &json!(id)).is_err())
        {
            return Err(invalid());
        }
        let (language, language_id) =
            language(&open.file).ok_or_else(|| unavailable("unsupported_language"))?;
        let root = service
            .state
            .lock()
            .await
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .map(|p| p.canonical_root.clone())
            .ok_or_else(|| refuse("project_not_found", "Unknown project."))?;
        let mut git = Git::new();
        let (checkout_id, checkout) = context::file_checkout(
            &root,
            &project,
            &json!({"checkoutId":open.checkout_id,"taskId":open.task_id}),
            &mut git,
        )
        .await?;
        // Use the same private-path and no-link boundary as Code, with no filesystem writes.
        files::lsp_text(&checkout, &open.file)?;
        if git.is_repo(&checkout).await?
            && !git
                .filters(&checkout, std::slice::from_ref(&open.file))
                .await?
                .is_empty()
        {
            return Err(refuse(
                "filtered_file",
                "This file is managed by a Git filter.",
            ));
        }
        let key = Key {
            project: project.clone(),
            checkout,
            language,
        };
        let (tx, id) = {
            // Keep registry removal and creation ordered. No awaited process work under either lock.
            let state = service.state.lock().await;
            service.accepting()?;
            if !state
                .projects
                .iter()
                .any(|p| p.project_id == project && p.canonical_root == root)
            {
                return Err(refuse("project_not_found", "Unknown project."));
            }
            let mut slots = manager.slots.lock().await;
            slots.retain(|_, slot| !slot.tx.is_closed() && !slot.join.is_finished());
            if !slots.contains_key(&key) {
                if slots.len() == MAX_PROCESSES {
                    return Err(unavailable("process_limit"));
                }
                let dev = workspace_protocol::channel::current().is_dev();
                let settings = settings::language_paths(&service.config.state_dir)?;
                let program = Program::resolve(
                    language,
                    dev,
                    &settings,
                    service.config.launch.runtime_root(),
                )?;
                let id = random_id()?;
                let (tx, rx) = mpsc::channel(8);
                let actor = Session::new(key.clone(), program, manager.events.clone());
                slots.insert(
                    key.clone(),
                    Slot {
                        id,
                        tx,
                        join: tokio::spawn(actor.run(rx)),
                    },
                );
            }
            let slot = &slots[&key];
            (slot.tx.clone(), slot.id.clone())
        };
        let doc = Document {
            id: format!("{id}{}", random_id()?),
            event_id: random_id()?,
            uri: file_uri(&key.checkout.join(&open.file)),
            checkout_id,
            language_id,
            text: open.text,
            version: open.version,
            wire_version: 0,
            // Unversioned messages are not safe after a change or after reusing a URI.
            baseline: true,
            diagnostic_due: None,
            diagnostics: Value::Null,
        };
        return Manager::submit(tx, Action::Open(doc)).await;
    }

    let (id, action) = match call.op.as_str() {
        "repo.lsp.change" => {
            let p: Change = parse(call.params)?;
            if !bounded(&p.text, p.version) {
                return Err(invalid());
            }
            (p.document_id.clone(), Action::Change(p))
        }
        "repo.lsp.definition" => {
            let p: Definition = parse(call.params)?;
            (p.document_id.clone(), Action::Definition(p))
        }
        "repo.lsp.status" => {
            let p: Handle = parse(call.params)?;
            (p.document_id.clone(), Action::Status(p.document_id))
        }
        "repo.lsp.close" => {
            let p: Handle = parse(call.params)?;
            (p.document_id.clone(), Action::Close(p.document_id))
        }
        _ => return Err(invalid()),
    };
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    match manager.existing(&project, &id).await {
        Ok(tx) => Manager::submit(tx, action).await,
        Err(_) if call.op == "repo.lsp.close" => Ok(json!({"closed":true})),
        Err(e) => Err(e),
    }
}

struct Document {
    id: String,
    event_id: String,
    uri: String,
    checkout_id: String,
    language_id: &'static str,
    text: String,
    version: u32,
    wire_version: u32,
    baseline: bool,
    diagnostic_due: Option<DiagnosticSchedule>,
    diagnostics: Value,
}
impl Document {
    fn opened(&self) -> Value {
        json!({"textDocument":{"uri":self.uri,"languageId":self.language_id,
            "version":self.wire_version,"text":self.text}})
    }
}
#[derive(Copy, Clone)]
enum TsDiagnosticKind {
    Syntax,
    Semantic,
    Suggestion,
}

impl TsDiagnosticKind {
    const ALL: [Self; 3] = [Self::Syntax, Self::Semantic, Self::Suggestion];

    fn command(self) -> &'static str {
        match self {
            Self::Syntax => "syntacticDiagnosticsSync",
            Self::Semantic => "semanticDiagnosticsSync",
            Self::Suggestion => "suggestionDiagnosticsSync",
        }
    }
}

#[derive(Copy, Clone)]
struct DiagnosticSchedule {
    at: Instant,
    retry_delay: Duration,
}

impl DiagnosticSchedule {
    fn initial() -> Self {
        Self {
            at: Instant::now() + DIAGNOSTIC_DEBOUNCE,
            retry_delay: DIAGNOSTIC_RETRY_INITIAL,
        }
    }
}

struct DiagnosticPull {
    document_id: String,
    wire_version: u32,
    deadline: Instant,
    retry_delay: Duration,
    pending: Vec<(u64, TsDiagnosticKind)>,
    diagnostics: Option<Vec<Value>>,
}

struct Session {
    key: Key,
    program: Program,
    events: broadcast::Sender<Value>,
    docs: HashMap<String, Document>,
    diagnostic_pull: Option<DiagnosticPull>,
    // A URI reopened on the same process cannot safely consume an old unversioned publication.
    seen: std::collections::HashSet<String>,
    last_failure: Option<&'static str>,
    failures: u8,
    generation: u32,
    wire_version: u32,
}

impl Session {
    fn new(key: Key, program: Program, events: broadcast::Sender<Value>) -> Self {
        Self {
            key,
            program,
            events,
            docs: HashMap::new(),
            diagnostic_pull: None,
            seen: Default::default(),
            last_failure: None,
            failures: 0,
            generation: 0,
            wire_version: 0,
        }
    }

    fn snapshot(&self, doc: &Document, ready: bool) -> Value {
        let mut result = json!({"eventId":doc.event_id,"version":doc.version,
            "generation":self.generation,"status":if ready {"ready"} else {"starting"},
            "diagnostics":doc.diagnostics});
        if !ready {
            if let Some(reason) = self.last_failure {
                result["status"] = json!("lsp_unavailable");
                result["reason"] = json!(reason);
            }
        }
        result
    }

    fn emit(&self, doc: &Document, ready: bool) {
        let mut event = self.snapshot(doc, ready);
        event["type"] = json!("code_lsp");
        event["projectId"] = json!(self.key.project);
        let _ = self.events.send(event);
    }

    fn failed(&mut self, reason: &'static str) -> Instant {
        self.failures += 1;
        self.last_failure = Some(reason);
        self.diagnostic_pull = None;
        for doc in self.docs.values_mut() {
            doc.diagnostic_due = None;
            doc.diagnostics = Value::Null;
        }
        for doc in self.docs.values() {
            self.emit(doc, false);
        }
        Instant::now() + Duration::from_secs(1 << (self.failures - 1))
    }

    async fn start(&mut self) -> std::result::Result<Peer, &'static str> {
        let mut peer =
            Peer::start(self.program.clone(), &self.key.checkout, self.key.language).await?;
        self.seen.clear();
        for doc in self.docs.values_mut() {
            doc.baseline = true;
            self.seen.insert(doc.uri.clone());
            peer.notify("textDocument/didOpen", doc.opened())
                .await
                .map_err(|_| "server_disconnected")?;
        }
        self.generation += 1;
        self.last_failure = None;
        for doc in self.docs.values() {
            self.emit(doc, true);
        }
        Ok(peer)
    }

    async fn run(mut self, mut rx: mpsc::Receiver<(Action, Reply)>) {
        let mut peer: Option<Peer> = None;
        let mut idle = Instant::now() + IDLE;
        let mut retry = Instant::now();
        loop {
            let diagnostic_at = self.diagnostic_at();
            tokio::select! {
                _ = sleep_until(diagnostic_at.unwrap_or(idle)), if diagnostic_at.is_some() && peer.is_some() => {
                    if self.advance_diagnostics(peer.as_mut().unwrap()).await.is_err() {
                        peer = None;
                        retry = self.failed("server_disconnected");
                    }
                },
                _ = sleep_until(idle) => {
                    self.last_failure = Some("idle");
                    for doc in self.docs.values_mut() { doc.diagnostics = Value::Null; }
                    for doc in self.docs.values() { self.emit(doc, false); }
                    return;
                },
                command = rx.recv() => {
                    let Some((action, reply)) = command else { return };
                    if reply.is_closed() { continue; }
                    idle = Instant::now() + IDLE;
                    let (result, broken) = self.command(action, &mut peer).await;
                    if self.docs.is_empty() {
                        rx.close();
                        drop(peer);
                        let _ = reply.send(result);
                        return;
                    }
                    let _ = reply.send(result);
                    if broken {
                        peer = None;
                        retry = self.failed("server_disconnected");
                    }
                },
                _ = sleep_until(retry), if peer.is_none() && !self.docs.is_empty() && self.failures < 3 => {
                    match self.start().await {
                        Ok(server) => peer = Some(server),
                        Err(reason) => retry = self.failed(reason),
                    }
                },
                message = transport::next(&mut peer), if peer.is_some() => {
                    match message {
                        Some(message) => {
                            if self.message(peer.as_mut().unwrap(), message).await.is_err() {
                                peer = None;
                                retry = self.failed("server_disconnected");
                            }
                        },
                        None => {
                            peer = None;
                            retry = self.failed("server_disconnected");
                        }
                    }
                }
            }
        }
    }

    async fn command(&mut self, action: Action, peer: &mut Option<Peer>) -> (Result<Value>, bool) {
        match action {
            Action::Open(mut doc) => {
                if self.docs.len() >= MAX_DOCS || self.seen.len() >= 1024 {
                    return (Err(unavailable("document_limit")), false);
                }
                if self.docs.values().any(|open| open.uri == doc.uri) {
                    return (Err(unavailable("document_already_open")), false);
                }
                self.wire_version += 1;
                doc.wire_version = self.wire_version;
                doc.baseline = self.seen.insert(doc.uri.clone());
                let mut broken = false;
                if let Some(peer) = peer {
                    broken = peer
                        .notify("textDocument/didOpen", doc.opened())
                        .await
                        .is_err();
                    if !broken && !doc.baseline && peer.ts_diagnostics {
                        doc.diagnostic_due = Some(DiagnosticSchedule::initial());
                    }
                }
                let mut result = self.snapshot(&doc, peer.is_some() && !broken);
                result["documentId"] = json!(doc.id);
                self.docs.insert(doc.id.clone(), doc);
                (Ok(result), broken)
            }
            Action::Status(id) => (
                self.docs
                    .get(&id)
                    .map(|d| self.snapshot(d, peer.is_some()))
                    .ok_or_else(stale),
                false,
            ),
            Action::Close(id) => {
                let mut broken = false;
                if let (Some(doc), Some(peer)) = (self.docs.remove(&id), peer) {
                    broken = peer
                        .notify(
                            "textDocument/didClose",
                            json!({"textDocument":{"uri":doc.uri}}),
                        )
                        .await
                        .is_err();
                }
                (Ok(json!({"closed":true})), broken)
            }
            Action::Change(p) => {
                let Some(doc) = self.docs.get_mut(&p.document_id) else {
                    return (Err(stale()), false);
                };
                if p.version <= doc.version {
                    return (Err(stale()), false);
                }
                self.wire_version += 1;
                doc.wire_version = self.wire_version;
                doc.version = p.version;
                let previous = std::mem::replace(&mut doc.text, p.text);
                doc.diagnostics = Value::Null;
                doc.baseline = false;
                let broken = if let Some(peer) = peer {
                    let mut content = json!({"text":doc.text});
                    if peer.sync_kind == 2 {
                        // Incremental-only peers still receive a single bounded edit,
                        // covering the exact previous buffer, never a disk revision.
                        let line = previous.bytes().filter(|&b| b == b'\n').count();
                        let last = previous.rsplit('\n').next().unwrap_or("");
                        let character = last.encode_utf16().count();
                        content["range"] = json!({"start":{"line":0,"character":0},
                            "end":{"line":line,"character":character}});
                    }
                    let broken = peer
                        .notify(
                            "textDocument/didChange",
                            json!({
                                "textDocument":{"uri":doc.uri,"version":doc.wire_version},
                                "contentChanges":[content],
                            }),
                        )
                        .await
                        .is_err();
                    if !broken && peer.ts_diagnostics {
                        doc.diagnostic_due = Some(DiagnosticSchedule::initial());
                    }
                    broken
                } else {
                    false
                };
                let doc = &self.docs[&p.document_id];
                self.emit(doc, peer.is_some() && !broken);
                (Ok(self.snapshot(doc, peer.is_some() && !broken)), broken)
            }
            Action::Definition(p) => {
                let Some(peer) = peer else {
                    return (
                        Err(unavailable(self.last_failure.unwrap_or("starting"))),
                        false,
                    );
                };
                match self.definition(peer, p).await {
                    Ok(v) => (v, false),
                    Err(_) => (Err(unavailable("definition_interrupted")), true),
                }
            }
        }
    }

    fn diagnostic_at(&self) -> Option<Instant> {
        if let Some(pull) = &self.diagnostic_pull {
            Some(pull.deadline)
        } else {
            self.docs
                .values()
                .filter_map(|doc| doc.diagnostic_due.map(|schedule| schedule.at))
                .min()
        }
    }

    fn retry_diagnostics(&mut self, pull: &DiagnosticPull) {
        // An edit already schedules its own pull; an old failure must not
        // replace that schedule or resurrect a closed document.
        if let Some(doc) = self
            .docs
            .get_mut(&pull.document_id)
            .filter(|doc| doc.wire_version == pull.wire_version)
        {
            doc.diagnostic_due = Some(DiagnosticSchedule {
                at: Instant::now() + pull.retry_delay,
                retry_delay: (pull.retry_delay * 2).min(DIAGNOSTIC_RETRY_MAX),
            });
        }
    }

    async fn advance_diagnostics(&mut self, peer: &mut Peer) -> io::Result<()> {
        if let Some(expired) = self.diagnostic_pull.take() {
            for (id, _) in &expired.pending {
                peer.notify("$/cancelRequest", json!({"id":id})).await?;
            }
            self.retry_diagnostics(&expired);
            return Ok(());
        }
        let Some((schedule, doc)) = self
            .docs
            .values_mut()
            .filter_map(|doc| Some((doc.diagnostic_due?, doc)))
            .min_by_key(|(schedule, _)| schedule.at)
        else {
            return Ok(());
        };
        doc.diagnostic_due = None;
        // typescript-language-server 4.4.0 forwards didChange to tsserver, queuing its
        // change as a fence before these read-only queries. Only their matching
        // responses can attest to the edited buffer; a push has no such binding.
        let mut pending = Vec::with_capacity(TsDiagnosticKind::ALL.len());
        for kind in TsDiagnosticKind::ALL {
            pending.push((peer.ts_diagnostic_request(&doc.uri, kind).await?, kind));
        }
        self.diagnostic_pull = Some(DiagnosticPull {
            document_id: doc.id.clone(),
            wire_version: doc.wire_version,
            deadline: Instant::now() + DIAGNOSTIC_TIMEOUT,
            retry_delay: schedule.retry_delay,
            pending,
            diagnostics: Some(Vec::new()),
        });
        Ok(())
    }

    fn diagnostic_response(&mut self, message: &Value) {
        let (Some(id), Some(pull)) = (message["id"].as_u64(), self.diagnostic_pull.as_mut()) else {
            return;
        };
        let Some(index) = pull.pending.iter().position(|(pending, _)| *pending == id) else {
            return;
        };
        let (_, kind) = pull.pending.swap_remove(index);
        let doc = self
            .docs
            .get_mut(&pull.document_id)
            .filter(|doc| doc.wire_version == pull.wire_version && Instant::now() < pull.deadline);
        if let Some(doc) = doc.as_deref() {
            let result = &message["result"];
            match result["body"].as_array().filter(|_| {
                message.get("error").is_none()
                    && result["type"] == "response"
                    && result["success"] == true
                    && result["command"] == kind.command()
            }) {
                Some(items) => {
                    if let Some(diagnostics) = &mut pull.diagnostics {
                        diagnostics.extend(
                            items
                                .iter()
                                .take(DIAGNOSTIC_LIMIT - diagnostics.len())
                                .filter_map(|item| ts_diagnostic(item, &doc.text)),
                        );
                    }
                }
                None => pull.diagnostics = None,
            }
        } else {
            pull.diagnostics = None;
        }
        if pull.pending.is_empty() {
            let mut pull = self.diagnostic_pull.take().unwrap();
            if let (Some(doc), Some(diagnostics)) = (doc, pull.diagnostics.take()) {
                doc.diagnostics = json!(diagnostics);
                self.emit(&self.docs[&pull.document_id], true);
            } else {
                self.retry_diagnostics(&pull);
            }
        }
    }

    async fn message(&mut self, peer: &mut Peer, message: Value) -> io::Result<()> {
        if message["method"] == "textDocument/publishDiagnostics" && message.get("id").is_none() {
            let params = &message["params"];
            let uri = params["uri"]
                .as_str()
                .and_then(uri_file)
                .map(|p| file_uri(&p));
            let Some(doc) = self
                .docs
                .values_mut()
                .find(|d| uri.as_deref() == Some(d.uri.as_str()))
            else {
                return Ok(());
            };
            if match params.get("version") {
                None | Some(Value::Null) => !doc.baseline,
                Some(v) => v.as_u64() != Some(doc.wire_version.into()),
            } {
                return Ok(());
            }
            let Some(items) = params["diagnostics"].as_array() else {
                return Ok(());
            };
            let mut diagnostics = Vec::new();
            for d in items.iter().take(DIAGNOSTIC_LIMIT) {
                let severity = d["severity"]
                    .as_u64()
                    .filter(|n| (1..=4).contains(n))
                    .unwrap_or(3);
                if let Some(diagnostic) =
                    diagnostic(&d["range"], &d["message"], severity, &doc.text)
                {
                    diagnostics.push(diagnostic);
                }
            }
            doc.diagnostics = json!(diagnostics);
            let id = doc.id.clone();
            self.emit(&self.docs[&id], true);
        } else if message.get("method").is_none() {
            self.diagnostic_response(&message);
        } else {
            peer.server_request(&message).await?;
        }
        Ok(())
    }

    async fn definition(&mut self, peer: &mut Peer, p: Definition) -> io::Result<Result<Value>> {
        let Some(doc) = self.docs.get(&p.document_id) else {
            return Ok(Err(stale()));
        };
        if doc.version != p.version {
            return Ok(Err(stale()));
        }
        if !valid_position(p.position, &doc.text) {
            return Ok(Err(invalid()));
        }
        let checkout_id = doc.checkout_id.clone();
        let id = peer.request("textDocument/definition", json!({
            "textDocument":{"uri":doc.uri},"position":{"line":p.position.line,"character":p.position.character},
        })).await?;
        timeout(Duration::from_secs(5), async {
            loop {
                let message = peer
                    .recv()
                    .await
                    .ok_or_else(|| io::Error::other("server disconnected"))?;
                if message.get("method").is_none() && message["id"] == id {
                    if message.get("error").is_some() {
                        return Ok(Err(unavailable("definition_failed")));
                    }
                    let result = &message["result"];
                    let locations = result
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(std::slice::from_ref(result));
                    let targets: Vec<_> = locations
                        .iter()
                        .take(16)
                        .filter_map(|location| self.location(location, &checkout_id))
                        .collect();
                    return Ok(Ok(
                        json!({"version":p.version,"generation":self.generation,"targets":targets}),
                    ));
                }
                self.message(peer, message).await?;
            }
        })
        .await?
    }

    fn location(&self, location: &Value, checkout_id: &str) -> Option<Value> {
        let (uri, range) = if let Some(uri) = location["targetUri"].as_str() {
            (uri, &location["targetSelectionRange"])
        } else {
            (location["uri"].as_str()?, &location["range"])
        };
        let path = uri_file(uri)?;
        let file = path.strip_prefix(&self.key.checkout).ok()?.to_str()?;
        let disk = files::lsp_text(&self.key.checkout, file).ok()?;
        let text = self
            .docs
            .values()
            .find(|d| d.uri == file_uri(&path))
            .map(|d| d.text.as_str())
            .unwrap_or(&disk);
        let range = checked_range(range, text)?;
        Some(json!({"file":file,"checkoutId":checkout_id,"range":range}))
    }
}

fn ts_diagnostic(item: &Value, text: &str) -> Option<Value> {
    // tsserver uses one-based lines and UTF-16 offsets.
    let position = |value: &Value| {
        Some(json!({"line":value["line"].as_u64()?.checked_sub(1)?,
            "character":value["offset"].as_u64()?.checked_sub(1)?}))
    };
    let range = json!({"start":position(&item["startLocation"])?,
        "end":position(&item["endLocation"])?});
    let severity = match item["category"].as_str()? {
        "error" => 1,
        "warning" => 2,
        "suggestion" => 4,
        _ => 3,
    };
    diagnostic(&range, &item["message"], severity, text)
}

fn diagnostic(range: &Value, message: &Value, severity: u64, text: &str) -> Option<Value> {
    let range = checked_range(range, text)?;
    let message: String = message
        .as_str()?
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(1024)
        .collect();
    Some(json!({"range":range,"severity":severity,"message":message}))
}

fn checked_range(range: &Value, text: &str) -> Option<Value> {
    let start: Position = serde_json::from_value(range["start"].clone()).ok()?;
    let end: Position = serde_json::from_value(range["end"].clone()).ok()?;
    (start <= end && valid_position(start, text) && valid_position(end, text))
        .then(|| json!({"start":range["start"],"end":range["end"]}))
}

fn valid_position(pos: Position, text: &str) -> bool {
    let Some(line) = text.split('\n').nth(pos.line as usize) else {
        return false;
    };
    let mut offset = 0;
    for c in line.strip_suffix('\r').unwrap_or(line).chars() {
        if offset == pos.character {
            return true;
        }
        if offset > pos.character {
            return false;
        }
        offset += c.len_utf16() as u32;
    }
    offset == pos.character
}

fn file_uri(path: &Path) -> String {
    use std::fmt::Write;
    let mut uri = "file://".to_owned();
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(char::from(b));
        } else {
            write!(uri, "%{b:02X}").unwrap();
        }
    }
    uri
}

fn uri_file(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let path = uri.strip_prefix("file://")?;
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#'])
        || path.len() > 16384
    {
        return None;
    }
    let mut bytes = path.bytes();
    let mut decoded = Vec::new();
    while let Some(b) = bytes.next() {
        let b = if b == b'%' {
            let a = char::from(bytes.next()?).to_digit(16)? as u8;
            let b = char::from(bytes.next()?).to_digit(16)? as u8;
            a * 16 + b
        } else {
            b
        };
        if b == 0 {
            return None;
        }
        decoded.push(b);
    }
    if decoded
        .split(|b| *b == b'/')
        .skip(1)
        .any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return None;
    }
    Some(std::ffi::OsString::from_vec(decoded).into())
}

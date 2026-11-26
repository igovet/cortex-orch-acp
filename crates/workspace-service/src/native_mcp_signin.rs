//! External MCP servers that wait for the owner to sign in.
//!
//! An enabled server (machine or project registry) that never answers MCP `initialize`
//! because it needs an OAuth sign-in (for example `mcp-remote`, which prints an
//! authorization address on stderr) is shown as one machine-wide owner card, "Sign in to
//! <server>", until it is ready or the owner switches that server off in Settings. No model
//! call is involved: the service starts the server with no channel and no tool grant, sends
//! one `initialize` and reads its stderr. The probe process that saw the sign-in request is
//! kept alive, so its own local OAuth callback can complete; once it answers `initialize`
//! the card resolves and later sessions attach the server. A card whose probe ended is
//! checked again by a fresh probe on the next pass (every 10 s while a card is open).
//!
//! The sign-in address lives only in memory, in this module; it is never logged, written
//! to a report, an event or a list reply. The desktop asks for it once (`owner_actions.act`
//! `open`) and hands it to the system browser after its own https check.
use crate::native_mcp::ProbeTarget;
use cortex_engine::external::{sign_in_need, SignInProbe};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The `projectId` of cards that belong to the machine, not to a project.
pub(crate) const MACHINE: &str = "machine";
/// How often servers and open cards are looked at.
pub(crate) const PERIOD: Duration = Duration::from_secs(10);
/// How long a first look at a server waits for an answer or a sign-in request.
pub(crate) const DISCOVER_WAIT: Duration = Duration::from_secs(30);
/// How long a repeated look (after a probe ended) waits.
pub(crate) const REPROBE_WAIT: Duration = Duration::from_secs(8);

struct Info {
    name: String,
    since_ms: u64,
    url: Option<String>,
}
#[derive(Default)]
struct State {
    open: BTreeMap<String, Info>,
    /// Servers (by fingerprint) already looked at; a changed or new entry is looked at again.
    discovered: BTreeSet<String>,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);
static PROBES: tokio::sync::Mutex<BTreeMap<String, SignInProbe>> = tokio::sync::Mutex::const_new(BTreeMap::new());
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}
fn card_id(fingerprint: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(fingerprint.as_bytes());
    format!("mcp_{}", digest[..12].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// A session saw `server` start without answering or exit: look at it again soon.
pub(crate) fn request(server: &str) {
    let prefix = format!("{server}:");
    state(|s| s.discovered.retain(|f| !f.starts_with(&prefix)));
    WAKE.notify_one();
}

/// The open cards as `owner_actions.list` shows them, oldest first. Never carries the address.
pub(crate) fn list() -> Vec<(u64, Value)> {
    state(|s| {
        s.open
            .iter()
            .map(|(fp, i)| {
                (
                    i.since_ms,
                    json!({
                        "actionId": card_id(fp),
                        "projectId": MACHINE,
                        "projectName": null,
                        "taskId": null,
                        "task": null,
                        "kind": "mcp_signin",
                        "service": i.name,
                        "title": format!("Sign in to {}", i.name),
                        "detail": "This MCP server needs you to sign in before sessions can use it. Open the sign-in, finish it in the browser, and this card closes by itself. Turn the server off in Settings to remove the card.",
                        "terminal": null,
                        "token": null,
                        "recheck": true,
                        "signIn": {"available": i.url.is_some()},
                        "openedAt": i.since_ms,
                        "checkedAt": null,
                        "lastStatus": null,
                    }),
                )
            })
            .collect()
    })
}
/// Whether card `id` is open.
pub(crate) fn is_open(id: &str) -> bool {
    state(|s| s.open.keys().any(|fp| card_id(fp) == id))
}
/// The sign-in address of card `id`, for the desktop's browser opener only.
pub(crate) fn url_of(id: &str) -> Option<String> {
    state(|s| s.open.iter().find(|(fp, _)| card_id(fp) == id).and_then(|(_, i)| i.url.clone()))
}
/// The open cards for notifications: (id, server, opened-at).
pub(crate) fn notices() -> Vec<(String, String, u64)> {
    state(|s| s.open.iter().map(|(fp, i)| (card_id(fp), i.name.clone(), i.since_ms)).collect())
}
/// Asks the background pass to run now (the owner pressed check again).
pub(crate) fn wake() {
    WAKE.notify_one();
}
pub(crate) async fn woken() {
    WAKE.notified().await;
}

enum Seen {
    Ready,
    Needs(Option<String>),
    Ended(Option<Option<String>>),
    Quiet,
}
async fn watch(probe: &SignInProbe, wait: Duration) -> Seen {
    let until = Instant::now() + wait;
    loop {
        if probe.ready() {
            return Seen::Ready;
        }
        if let Some(need) = probe.sign_in() {
            if need.url.is_some() || probe.exited() {
                return Seen::Needs(need.url);
            }
            // A phrase without an address yet: give the address a moment to arrive.
            if Instant::now() + Duration::from_millis(500) >= until {
                return Seen::Needs(None);
            }
        }
        if probe.exited() {
            return Seen::Ended(None);
        }
        if Instant::now() >= until {
            return Seen::Quiet;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// One pass over `targets` (the enabled servers now). Returns whether the set of open cards
/// or an address changed. Only one pass runs at a time.
pub(crate) async fn pass(targets: &mut Vec<ProbeTarget>, discover_wait: Duration, reprobe_wait: Duration) -> bool {
    let mut probes = PROBES.lock().await;
    let mut changed = false;
    let present: BTreeSet<String> = targets.iter().map(|t| t.fingerprint.clone()).collect();
    // A server switched off (or removed) closes its card.
    let gone: Vec<String> = state(|s| {
        s.discovered.retain(|f| present.contains(f));
        let gone: Vec<String> = s.open.keys().filter(|f| !present.contains(*f)).cloned().collect();
        for f in &gone {
            s.open.remove(f);
        }
        gone
    });
    changed |= !gone.is_empty();
    let stale: Vec<String> = probes.keys().filter(|f| !present.contains(*f)).cloned().collect();
    for f in stale {
        if let Some(p) = probes.remove(&f) {
            p.shutdown().await;
        }
    }
    // Cards still open: ready closes them; an ended probe is replaced.
    let open: Vec<String> = state(|s| s.open.keys().cloned().collect());
    for fp in open {
        let mut resolved = false;
        if let Some(p) = probes.get(&fp) {
            if p.ready() {
                resolved = true;
            } else if p.exited() {
                if let Some(p) = probes.remove(&fp) {
                    p.shutdown().await;
                }
            } else if let Some(url) = p.sign_in().and_then(|n| n.url) {
                changed |= state(|s| s.open.get_mut(&fp).is_some_and(|i| i.url.replace(url.clone()).as_deref() != Some(&url)));
            }
        }
        if !resolved && !probes.contains_key(&fp) {
            if let Some(index) = targets.iter().position(|t| t.fingerprint == fp) {
                let target = targets.remove(index);
                if let Ok(probe) = SignInProbe::launch(target.launch, target.sandbox.map(|s| Arc::new(s) as Arc<dyn cortex_engine::external::Containment>)).await {
                    match watch(&probe, reprobe_wait).await {
                        Seen::Ready => {
                            resolved = true;
                            probe.shutdown().await;
                        }
                        Seen::Needs(url) => {
                            changed |= state(|s| s.open.get_mut(&fp).is_some_and(|i| url.is_some() && i.url != url && { i.url = url.clone(); true }));
                            probes.insert(fp.clone(), probe);
                        }
                        Seen::Quiet => {
                            probes.insert(fp.clone(), probe);
                        }
                        Seen::Ended(_) => probe.shutdown().await,
                    }
                }
            }
        }
        if resolved {
            if let Some(p) = probes.remove(&fp) {
                p.shutdown().await;
            }
            state(|s| s.open.remove(&fp));
            changed = true;
        }
    }
    // Servers not looked at yet.
    let fresh: Vec<ProbeTarget> = {
        let mut keep = Vec::new();
        let mut fresh = Vec::new();
        for t in targets.drain(..) {
            let skip = state(|s| s.open.contains_key(&t.fingerprint) || s.discovered.contains(&t.fingerprint));
            if skip { keep.push(t) } else { fresh.push(t) }
        }
        *targets = keep;
        fresh
    };
    for target in fresh {
        let (fp, name) = (target.fingerprint.clone(), target.name.clone());
        state(|s| s.discovered.insert(fp.clone()));
        let Ok(probe) = SignInProbe::launch(target.launch, target.sandbox.map(|s| Arc::new(s) as Arc<dyn cortex_engine::external::Containment>)).await else {
            continue;
        };
        match watch(&probe, discover_wait).await {
            Seen::Needs(url) => {
                state(|s| {
                    s.open.insert(fp.clone(), Info { name, since_ms: now_ms(), url });
                });
                probes.insert(fp, probe);
                changed = true;
            }
            Seen::Ended(_) => {
                // Gone without a word about signing in, or with one: the text decides.
                probe.shutdown().await;
            }
            Seen::Ready | Seen::Quiet => probe.shutdown().await,
        }
    }
    changed
}

/// Whether stderr text shows a sign-in requirement (exposed for session start).
#[allow(dead_code)]
pub(crate) fn shows_sign_in(text: &str) -> bool {
    sign_in_need(text).is_some()
}

impl crate::Service {
    /// The enabled servers of every activated project (open or not: a paused task's project
    /// still needs its sign-in before it resumes), one per distinct entry.
    async fn mcp_targets(&self) -> Vec<ProbeTarget> {
        let Some(routes) = self.config.launch.native_routes.clone() else { return Vec::new() };
        let roots: Vec<std::path::PathBuf> = {
            let state = self.state.lock().await;
            state.projects.iter().map(|p| p.canonical_root.clone()).collect()
        };
        let scratch = self.config.state_dir.join("mcp-signin");
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for root in roots {
            let routes = routes.clone();
            let scratch = scratch.clone();
            let found = tokio::task::spawn_blocking(move || {
                use std::os::unix::fs::DirBuilderExt;
                let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&scratch);
                routes.mcp_registry_for(&root).map(|r| r.probe_targets(&root, &scratch)).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            for t in found {
                if seen.insert(t.fingerprint.clone()) {
                    out.push(t);
                }
            }
        }
        out
    }
    /// Looks at the enabled MCP servers in the background (see the module text) and keeps
    /// the sign-in cards. Called once by the real service.
    pub fn start_mcp_signin(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut shutdown = service.shutdown.subscribe();
            // Sessions and projects settle first.
            tokio::time::sleep(Duration::from_secs(3)).await;
            loop {
                if *shutdown.borrow() {
                    break;
                }
                let mut targets = service.mcp_targets().await;
                if pass(&mut targets, DISCOVER_WAIT, REPROBE_WAIT).await {
                    service.notifier.touch(MACHINE);
                }
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = tokio::time::sleep(PERIOD) => {}
                    _ = woken() => {}
                }
            }
            let mut probes = PROBES.lock().await;
            for (_, p) in std::mem::take(&mut *probes) {
                p.shutdown().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_engine::external::ExternalLaunch;
    use std::path::PathBuf;

    // The shared module state is process-wide: these tests run one after another.
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A fake server: prints the sign-in line, and answers `initialize` only once the gate
    /// file exists (as `mcp-remote` does after the owner finishes signing in).
    fn target(dir: &std::path::Path, name: &str) -> ProbeTarget {
        let script = dir.join(format!("{name}.sh"));
        std::fs::write(&script, "#!/bin/sh\necho 'Please authorize this client by visiting: https://example.test/oauth/authorize?state=fixture' >&2\nwhile [ ! -f \"$GATE\" ]; do sleep 0.1; done\nread line\necho '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"serverInfo\":{\"name\":\"fake\",\"version\":\"1\"}}}'\nsleep 5\n").unwrap();
        let env = BTreeMap::from([("GATE".to_owned(), dir.join(format!("{name}.gate")).to_string_lossy().into_owned()), ("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
        ProbeTarget {
            name: name.into(),
            fingerprint: format!("{name}:fixture"),
            launch: ExternalLaunch { executable: PathBuf::from("/bin/sh"), args: vec![script.to_string_lossy().into_owned()], cwd: dir.to_path_buf(), environment: env },
            sandbox: None,
        }
    }
    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cortex-signin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    const SHORT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn a_server_asking_for_sign_in_gets_one_card_that_resolves_when_it_answers() {
        let _one = ONE_AT_A_TIME.lock().await;
        let d = dir("resolve");
        let mut changed = pass(&mut vec![target(&d, "kling")], SHORT, SHORT).await;
        assert!(changed);
        let cards = list();
        assert_eq!(cards.len(), 1);
        let card = &cards[0].1;
        assert_eq!(card["service"], "kling");
        assert_eq!(card["kind"], "mcp_signin");
        assert_eq!(card["signIn"]["available"], true);
        assert!(!card.to_string().contains("example.test"), "the address never leaves in a list");
        let id = card["actionId"].as_str().unwrap().to_owned();
        assert_eq!(url_of(&id).as_deref(), Some("https://example.test/oauth/authorize?state=fixture"));
        // Another pass finds the same server: still one card.
        changed = pass(&mut vec![target(&d, "kling")], SHORT, SHORT).await;
        assert!(!changed);
        assert_eq!(list().len(), 1);
        // The owner signs in: the held process answers and the card closes.
        std::fs::write(d.join("kling.gate"), "").unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(pass(&mut vec![target(&d, "kling")], SHORT, SHORT).await);
        assert!(list().is_empty());
        assert!(!is_open(&id));
        // A server that answers was looked at and stays quiet afterwards.
        assert!(!pass(&mut vec![target(&d, "kling")], SHORT, SHORT).await);
        assert!(PROBES.lock().await.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn the_card_goes_when_the_server_is_switched_off() {
        let _one = ONE_AT_A_TIME.lock().await;
        let d = dir("off");
        assert!(pass(&mut vec![target(&d, "other")], SHORT, SHORT).await);
        assert_eq!(list().len(), 1);
        // Settings turned the server off: it is no longer an enabled target.
        assert!(pass(&mut Vec::new(), SHORT, SHORT).await);
        assert!(list().is_empty());
        assert!(PROBES.lock().await.is_empty(), "the held process ended");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_ready_server_makes_no_card() {
        let _one = ONE_AT_A_TIME.lock().await;
        let d = dir("ready");
        std::fs::write(d.join("fine.gate"), "").unwrap();
        // With the gate open the fake still prints its line first; it answers within the wait.
        let changed = pass(&mut vec![target(&d, "fine")], SHORT, SHORT).await;
        assert!(!changed);
        assert!(list().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

//! Owner actions: access a task lacks, shown to the owner as a card, resolved by the owner.
//!
//! The runtime records `OWNER_ACTION_REQUIRED` (`{"id":"oa_…","kind":"access","service":…,
//! "detail":…,"node":…,"revision":n}`) when a task's work needs access the machine lacks: a
//! GitHub, Codex or Claude login, a `sudo` password, or a named service. The task does not
//! stop; only the blocked work waits. This module lists the open actions of every active
//! project (`owner_actions.list`) and resolves one (`owner_actions.act`) by recording
//! `OWNER_ACTION_RESOLVED` (`{"id":…,"how":"terminal_login"|"token"|"recheck"|"dismissed"}`),
//! on which the runtime retries the blocked work. An action resolves only on evidence: the
//! service's own probe ([`crate::native_preflight::recheck`], exit codes only) sees the access,
//! a `sudo` prompt of the same task was answered, or the owner dismissed it.
//!
//! What the owner can do depends on the service ([`access`]): open the CLI's own login in the
//! desktop's project terminal (`gh auth login`, `codex login`, `claude auth login`), paste a
//! token where the CLI has a documented non-interactive login on stdin (GitHub only:
//! `gh auth login --with-token`), ask for a check now, or dismiss. While an action is open the
//! service sweeps for due checks every [`RECHECK_EVERY`] and shares the task
//! driver's persisted recheck schedule.
//!
//! A pasted token is handled like a `sudo` password ([`crate::native_askpass`]): moved from the
//! request into a [`Secret`], written once to the CLI's stdin, overwritten with zeros, never
//! stored, logged, put in an event, a receipt, a reply or a notification, never shown to a
//! model. The CLI's own output is discarded; only its exit code is used. A paired phone may
//! send one only while the owner allows passwords and tokens from phones.
use crate::native_askpass::Secret;
use cortex_core::{lifecycle::obstacles::access_service_ok, store::executor::StoreExecutor};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How often the service looks for due background card checks.
pub const RECHECK_EVERY: Duration = Duration::from_secs(60);
/// Longest token accepted (one line).
pub const TOKEN_BYTES: usize = 4096;
/// How long a token login may take.
const TOKEN_DEADLINE: Duration = Duration::from_secs(30);
/// Open actions read per project.
const LIST_LIMIT: usize = 200;
/// How long a terminal login opened for an action names how it resolved.
const TERMINAL_WINDOW: Duration = Duration::from_secs(3600);

/// A token login a CLI documents: the token is read from stdin.
pub(crate) struct TokenLogin {
    pub program: &'static str,
    pub args: &'static [&'static str],
    pub label: &'static str,
    pub hint: &'static str,
}
/// What the owner can do for one service.
pub(crate) struct Access {
    /// The preflight probe that shows the access is there.
    pub probe: Option<&'static str>,
    pub title: &'static str,
    /// The CLI's own interactive login, run in the project terminal.
    pub terminal: Option<&'static [&'static str]>,
    pub token: Option<TokenLogin>,
}

/// The card of `service`. Login commands are the installed CLIs' own (checked with their
/// `--help`): `gh auth login`, `codex login`, `claude auth login`. Only `gh` documents a
/// token login on stdin for its own account; `codex login --with-api-key` would replace the
/// native subscription login with API-key billing, and `claude` has none.
pub(crate) fn access(service: &str) -> Access {
    match service {
        "github" => Access {
            probe: Some("gh_auth"),
            title: "Sign in to GitHub",
            terminal: Some(&["gh", "auth", "login"]),
            token: Some(TokenLogin {
                program: "gh",
                args: &["auth", "login", "--with-token"],
                label: "GitHub token",
                hint: "A personal access token (classic) with the repo, read:org and gist scopes. It goes once to gh auth login on the computer and is not saved or shown to the model.",
            }),
        },
        "codex" => Access {
            probe: Some("codex_login"),
            title: "Sign in to Codex",
            terminal: Some(&["codex", "login"]),
            token: None,
        },
        "claude" => Access {
            probe: Some("claude_login"),
            title: "Sign in to Claude",
            terminal: Some(&["claude", "auth", "login"]),
            token: None,
        },
        "sudo" => Access {
            probe: Some("sudo"),
            title: "sudo needs your password",
            terminal: None,
            token: None,
        },
        _ => Access {
            probe: None,
            title: "Access needed",
            terminal: None,
            token: None,
        },
    }
}

/// One open action as the store records it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Open {
    pub id: String,
    pub runtime_task: String,
    pub service: String,
    pub detail: String,
    pub created_at: String,
    pub opened_ms: u64,
    pub task: Option<String>,
    pub execution: Option<String>,
    pub task_title: Option<String>,
    ru: bool,
}

fn printable(text: &str, limit: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.bytes().all(|b| b.is_ascii_graphic())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

const OPEN_SQL: &str = "SELECT r.task_id, r.payload_json, r.created_at,
     CAST((julianday(r.created_at) - 2440587.5) * 86400000 AS INTEGER), w.task_id, wt.description, w.execution_id
     FROM runtime_events r JOIN runtime_tasks t ON t.task_id=r.task_id
     LEFT JOIN workspace_executions w ON w.runtime_task_id=r.task_id
     LEFT JOIN workspace_tasks wt ON wt.task_id=w.task_id
     WHERE r.type='OWNER_ACTION_REQUIRED' AND t.state NOT IN ('COMPLETED','CANCELLED','FAILED')
     AND NOT EXISTS(SELECT 1 FROM runtime_events x WHERE x.task_id=r.task_id
         AND x.type='OWNER_ACTION_RESOLVED'
         AND json_extract(x.payload_json,'$.id')=json_extract(r.payload_json,'$.id'))
     ORDER BY r.id DESC LIMIT ?";

/// The open access actions of one project store, oldest first, one per task and service
/// (the newest). A malformed payload is skipped, never shown.
pub(crate) fn open_actions(db: &rusqlite::Connection) -> cortex_core::CoreResult<Vec<Open>> {
    let mut statement = db.prepare(OPEN_SQL)?;
    let rows = statement
        .query_map([LIST_LIMIT as i64 * 2], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (runtime_task, payload, created_at, ms, task, description, execution) in rows {
        let Ok(payload) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        let (Some(id), Some(service)) = (
            payload["id"].as_str().filter(|id| id_ok(id)),
            payload["service"].as_str().filter(|s| access_service_ok(s)),
        ) else {
            continue;
        };
        if payload["kind"] != "access" || !seen.insert((runtime_task.clone(), service.to_owned())) {
            continue;
        }
        let ru = cortex_core::lifecycle::domain::task_language_tx(db, &runtime_task)? == "ru";
        out.push(Open {
            id: id.to_owned(),
            runtime_task,
            ru,
            service: service.to_owned(),
            detail: printable(payload["detail"].as_str().unwrap_or(""), 600),
            created_at,
            opened_ms: ms.and_then(|m| u64::try_from(m).ok()).unwrap_or(0),
            task,
            execution,
            task_title: description
                .as_deref()
                .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
                .map(|l| printable(l, 160)),
        });
        if out.len() >= LIST_LIMIT {
            break;
        }
    }
    out.reverse();
    Ok(out)
}

/// Records `OWNER_ACTION_RESOLVED` for an action that is still open, in the same
/// transaction as the check. Answers whether it was open (a second resolution records
/// nothing).
pub(crate) fn resolve_tx(
    db: &rusqlite::Transaction<'_>,
    runtime_task: &str,
    id: &str,
    how: &str,
) -> cortex_core::CoreResult<bool> {
    let open: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='OWNER_ACTION_REQUIRED'
             AND json_extract(payload_json,'$.id')=?2)
         AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='OWNER_ACTION_RESOLVED'
             AND json_extract(payload_json,'$.id')=?2)",
        (runtime_task, id),
        |r| r.get(0),
    )?;
    if open {
        cortex_core::lifecycle::event_tx(
            db,
            runtime_task,
            "OWNER_ACTION_RESOLVED",
            json!({"id":id,"how":how}),
        )?;
    }
    Ok(open)
}

/// The owner's last check of an action, and whether the owner opened its terminal login.
#[derive(Default)]
struct Marks {
    checks: BTreeMap<(String, String), (u64, String)>,
    terminals: BTreeMap<(String, String), u64>,
    /// `sudo` prompts answered with a password: project root, runtime task, when (store time).
    sudo: Vec<(PathBuf, String, String)>,
}
static MARKS: Mutex<Option<Marks>> = Mutex::new(None);
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
fn marks<T>(f: impl FnOnce(&mut Marks) -> T) -> T {
    let mut guard = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Marks::default))
}

/// A `sudo` prompt of `task` (runtime task) in `root` was answered with a password: its
/// open sudo action resolves on the next pass, which starts now.
pub(crate) fn sudo_answered(root: &Path, task: &str) {
    let at = cortex_core::now();
    marks(|m| {
        m.sudo.push((root.to_owned(), task.to_owned(), at));
        let excess = m.sudo.len().saturating_sub(64);
        m.sudo.drain(..excess);
    });
    WAKE.notify_one();
}

/// One active project: where its store is and its runtime to wake.
#[derive(Clone)]
pub(crate) struct Place {
    pub project: String,
    pub name: String,
    pub root: PathBuf,
    pub executor: Arc<StoreExecutor>,
    pub runtime: Option<Arc<crate::native_runtime::Runtime>>,
}

async fn read_open(place: &Place) -> io::Result<Vec<Open>> {
    let unavailable = || crate::failure("storage_unavailable");
    place
        .executor
        .read(open_actions)
        .map_err(|_| unavailable())?
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())
}

/// The public view of one open action. Nothing secret exists to leave out.
fn view(place: &Place, open: &Open) -> Value {
    let access = access(&open.service);
    let key = (place.project.clone(), open.id.clone());
    let check = marks(|m| m.checks.get(&key).cloned());
    let detail = if open.detail.is_empty() {
        match (open.service.as_str(), open.ru) {
            ("sudo", false) => "A command in this task needs sudo.".to_owned(),
            ("sudo", true) => "Для команды нужен sudo.".to_owned(),
            (other, false) => format!("This task needs access to {other}."),
            (other, true) => format!("Для продолжения нужен доступ к {other}."),
        }
    } else {
        cortex_core::lifecycle::obstacles::owner_access_detail(&open.service, &open.detail, open.ru)
    };
    let title = match (open.service.as_str(), open.ru) {
        ("github", true) => "Войдите в GitHub".to_owned(),
        ("codex", true) => "Войдите в Codex".to_owned(),
        ("claude", true) => "Войдите в Claude".to_owned(),
        ("sudo", true) => "Для sudo нужен ваш пароль".to_owned(),
        (service, true) => format!("Нужен доступ: {service}"),
        (service, false) if access.probe.is_none() => format!("{}: {service}", access.title),
        (_, false) => access.title.to_owned(),
    };
    json!({
        "actionId": open.id,
        "projectId": place.project,
        "projectName": place.name,
        "taskId": open.task,
        "task": open.task_title,
        "kind": "access",
        "service": open.service,
        "title": title,
        "detail": detail,
        "terminal": access.terminal.map(|argv| json!({"command": argv.join(" "), "argv": argv})),
        "token": access.token.as_ref().map(|t| json!({
            "label": if open.ru { "Токен GitHub" } else { t.label },
            "hint": if open.ru {
                "Персональный токен доступа (classic) с правами repo, read:org и gist. Он передаётся один раз команде gh auth login на компьютере, не сохраняется и не показывается модели."
            } else {
                t.hint
            },
        })),
        "recheck": access.probe.is_some(),
        "openedAt": open.opened_ms,
        "checkedAt": check.as_ref().map(|c| c.0),
        "lastStatus": check.map(|c| c.1),
    })
}

/// What `owner_actions.act` asks.
pub(crate) enum Act {
    Recheck,
    Terminal,
    Dismiss,
    Token(Secret),
    /// Machine MCP sign-in card only: the desktop asks for the sign-in address.
    Open,
}
/// `owner_actions.act`: `{projectId, actionId, action}` with `action` one of `recheck`,
/// `terminal`, `dismiss`, or `token` with exactly one more field, `token`. The token string
/// is moved, never copied, into a [`Secret`].
pub(crate) fn parse_act(mut params: Value) -> io::Result<(String, String, Act)> {
    let invalid = || crate::failure("invalid_params");
    let map = params.as_object_mut().ok_or_else(invalid)?;
    let text = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .filter(|t| id_ok(t))
            .map(str::to_owned)
    };
    let (project, id) = (
        text("projectId").ok_or_else(invalid)?,
        text("actionId").ok_or_else(invalid)?,
    );
    let action = map.get("action").and_then(Value::as_str).unwrap_or("");
    let act = match (action, map.len()) {
        ("recheck", 3) => Act::Recheck,
        ("terminal", 3) => Act::Terminal,
        ("dismiss", 3) => Act::Dismiss,
        ("open", 3) => Act::Open,
        ("token", 4) => {
            let Some(Value::String(token)) = map.get_mut("token").map(Value::take) else {
                return Err(invalid());
            };
            let secret = Secret::new(token);
            if secret.bytes().is_empty()
                || secret.bytes().len() > TOKEN_BYTES
                || secret
                    .bytes()
                    .iter()
                    .any(|b| matches!(b, b'\n' | b'\r' | 0))
            {
                return Err(crate::failure("invalid_owner_token"));
            }
            Act::Token(secret)
        }
        _ => return Err(invalid()),
    };
    Ok((project, id, act))
}

#[cfg(test)]
static TEST_PROGRAMS: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());
/// The program a probe or token login runs: a test fixture's fake when one is set.
fn program(name: &str) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(fake) = TEST_PROGRAMS.lock().unwrap().get(name) {
        return Some(fake.clone());
    }
    crate::tool_path::locate(name).map(|(p, _)| p)
}
async fn probe(name: &'static str) -> Option<Value> {
    #[cfg(test)]
    {
        let fake = TEST_PROGRAMS.lock().unwrap().get(name).cloned();
        if let Some(fake) = fake {
            return crate::native_preflight::recheck_at(name, Some(fake), None).await;
        }
    }
    crate::native_preflight::recheck(name).await
}

/// Runs `login` with `secret` on stdin, once. stdout and stderr are discarded; only the
/// exit code is used. The secret is dropped (zeroed) as soon as it is written.
async fn token_login(login: &TokenLogin, secret: Secret) -> Result<(), &'static str> {
    use tokio::io::AsyncWriteExt;
    let program = program(login.program).ok_or("owner_token_unavailable")?;
    let mut env: Vec<_> = crate::tool_path::environment().into_iter().collect();
    for (k, v) in [("GH_PROMPT_DISABLED", "1"), ("NO_COLOR", "1")] {
        env.retain(|(name, _)| name != k);
        env.push((k.into(), v.into()));
    }
    let mut child = tokio::process::Command::new(program)
        .args(login.args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "owner_token_unavailable")?;
    let group = child.id();
    let mut stdin = child.stdin.take().ok_or("owner_token_unavailable")?;
    let work = async {
        // Unbuffered: the bytes go straight to the pipe.
        let written =
            stdin.write_all(secret.bytes()).await.is_ok() && stdin.write_all(b"\n").await.is_ok();
        drop(secret);
        drop(stdin);
        (written, child.wait().await)
    };
    match tokio::time::timeout(TOKEN_DEADLINE, work).await {
        Ok((true, Ok(status))) if status.success() => Ok(()),
        Ok(_) => Err("owner_token_refused"),
        Err(_) => {
            if let Some(pid) = group {
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            }
            Err("owner_token_timeout")
        }
    }
}

/// How the evidence of one check reads: resolved with `how`, or still needed with a status.
enum Checked {
    Resolved(&'static str),
    Needed(String),
}
async fn check(
    place: &Place,
    open: &Open,
    probed: &mut BTreeMap<&'static str, Option<Value>>,
) -> Checked {
    let key = (place.project.clone(), open.id.clone());
    if sudo_was_answered(place, open) {
        return Checked::Resolved("recheck");
    }
    let Some(name) = access(&open.service).probe else {
        return Checked::Needed("not_checkable".into());
    };
    let fact = match probed.get(name) {
        Some(fact) => fact.clone(),
        None => {
            let fact = probe(name).await;
            probed.insert(name, fact.clone());
            fact
        }
    };
    let status = fact
        .as_ref()
        .and_then(|f| f["status"].as_str())
        .unwrap_or("unknown")
        .to_owned();
    marks(|m| m.checks.insert(key.clone(), (now_ms(), status.clone())));
    if !fact
        .as_ref()
        .is_some_and(crate::native_preflight::has_access)
    {
        return Checked::Needed(status);
    }
    let terminal = marks(|m| {
        m.terminals
            .get(&key)
            .is_some_and(|at| now_ms().saturating_sub(*at) < TERMINAL_WINDOW.as_millis() as u64)
    });
    Checked::Resolved(if terminal {
        "terminal_login"
    } else {
        "recheck"
    })
}

/// Records the resolution and wakes the project's runtime, which retries the blocked work.
async fn resolve(place: &Place, open: &Open, how: &'static str) -> io::Result<bool> {
    let unavailable = || crate::failure("storage_unavailable");
    let (task, id) = (open.runtime_task.clone(), open.id.clone());
    let resolved = place
        .executor
        .write(move |db| resolve_tx(db, &task, &id, how))
        .map_err(|_| unavailable())?
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    let key = (place.project.clone(), open.id.clone());
    marks(|m| {
        m.checks.remove(&key);
        m.terminals.remove(&key);
        if open.service == "sudo" {
            m.sudo
                .retain(|(root, task, _)| !(*root == place.root && *task == open.runtime_task));
        }
    });
    if let Some(runtime) = &place.runtime {
        runtime.wake.notify_waiters();
    }
    Ok(resolved)
}

fn sudo_was_answered(place: &Place, open: &Open) -> bool {
    open.service == "sudo" && marks(|m| {
        m.sudo.iter().any(|(root, task, at)| {
            *root == place.root && *task == open.runtime_task && *at >= open.created_at
        })
    })
}

/// Claim the same durable deadlines as the task driver. A confirmed sudo answer
/// is new evidence, not a periodic probe, so it may resolve the card immediately.
pub(crate) async fn pass(places: &[Place]) -> usize {
    pass_at(places, (now_ms() / 1000) as i64).await
}

async fn pass_at(places: &[Place], now: i64) -> usize {
    let mut probed = BTreeMap::new();
    let mut resolved = 0;
    for place in places {
        let Ok(opens) = read_open(place).await else {
            continue;
        };
        let tasks: std::collections::BTreeSet<_> = opens.iter().map(|o| o.runtime_task.clone()).collect();
        let claimed = match place.executor.write(move |db| {
            let mut claimed = std::collections::BTreeSet::new();
            for task in tasks {
                claimed.extend(cortex_core::lifecycle::obstacles::claim_environment_rechecks_tx(db, &task, now)?);
            }
            Ok(claimed)
        }) {
            Ok(reply) => reply.await.ok().and_then(Result::ok).unwrap_or_default(),
            Err(_) => Default::default(),
        };
        for open in &opens {
            if !sudo_was_answered(place, open) && !claimed.contains(&open.id) {
                continue;
            }
            if let Checked::Resolved(how) = check(place, open, &mut probed).await {
                if resolve(place, open, how).await.unwrap_or(false) {
                    resolved += 1;
                }
            }
        }
    }
    resolved
}

/// The open actions of `places`, oldest first, and whether a phone may send a token.
pub(crate) async fn list(places: &[Place]) -> Value {
    let mut actions = Vec::new();
    for place in places {
        if let Ok(opens) = read_open(place).await {
            actions.extend(opens.iter().map(|open| (open.opened_ms, view(place, open))));
        }
    }
    // Machine-wide sign-in cards live in process memory; unit tests of project actions run
    // beside the sign-in tests in one process, so they list project actions only.
    if cfg!(not(test)) {
        actions.extend(crate::native_mcp_signin::list());
    }
    actions.sort_by_key(|(at, _)| *at);
    json!({
        "actions": actions.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "phoneTokens": crate::native_askpass::phone_allowed(),
    })
}

/// Serves one `owner_actions.act` in `place`.
pub(crate) async fn act(place: &Place, id: &str, act: Act) -> io::Result<Value> {
    let open = read_open(place)
        .await?
        .into_iter()
        .find(|o| o.id == id)
        .ok_or_else(|| crate::failure("unknown_owner_action"))?;
    let access = access(&open.service);
    let key = (place.project.clone(), open.id.clone());
    let checked = match act {
        Act::Dismiss => Checked::Resolved("dismissed"),
        Act::Open => return Err(crate::failure("owner_open_unsupported")),
        Act::Terminal => {
            let argv = access
                .terminal
                .ok_or_else(|| crate::failure("owner_terminal_unsupported"))?;
            marks(|m| m.terminals.insert(key, now_ms()));
            return Ok(json!({"outcome":"terminal","command":argv.join(" "),"argv":argv}));
        }
        Act::Recheck => check(place, &open, &mut BTreeMap::new()).await,
        Act::Token(secret) => {
            let login = access
                .token
                .as_ref()
                .ok_or_else(|| crate::failure("owner_token_unsupported"))?;
            token_login(login, secret).await.map_err(crate::failure)?;
            match check(place, &open, &mut BTreeMap::new()).await {
                Checked::Resolved(_) => Checked::Resolved("token"),
                needed => needed,
            }
        }
    };
    match checked {
        Checked::Resolved(how) => {
            if !resolve(place, &open, how).await? {
                return Err(crate::failure("unknown_owner_action"));
            }
            Ok(json!({"outcome":"resolved","how":how}))
        }
        Checked::Needed(status) => Ok(json!({"outcome":"still_needed","status":status})),
    }
}

impl crate::Service {
    /// Every active project's store, with its runtime.
    pub(crate) async fn owner_places(&self) -> Vec<Place> {
        let state = self.state.lock().await;
        state
            .native
            .iter()
            .filter_map(|(id, runtime)| {
                let project = state.projects.iter().find(|p| &p.project_id == id)?;
                Some(Place {
                    project: id.clone(),
                    name: project.display_name.clone(),
                    root: project.canonical_root.clone(),
                    executor: runtime.host.executor.clone(),
                    runtime: Some(runtime.clone()),
                })
            })
            .collect()
    }
    /// `owner_actions.list` and `owner_actions.act` on the service socket.
    pub(crate) async fn owner_actions_call(
        &self,
        method: &str,
        params: Value,
    ) -> io::Result<Value> {
        match method {
            "owner_actions.list" if params == json!({}) => {
                Ok(list(&self.owner_places().await).await)
            }
            "owner_actions.act" => {
                let (project, id, request) = parse_act(params)?;
                if project == crate::native_mcp_signin::MACHINE {
                    return mcp_act(&id, request).await;
                }
                let place = self
                    .owner_places()
                    .await
                    .into_iter()
                    .find(|p| p.project == project)
                    .ok_or_else(|| crate::failure("project_not_found"))?;
                act(&place, &id, request).await
            }
            _ => Err(crate::failure("invalid_params")),
        }
    }
    /// Checks open owner actions again in the background every [`RECHECK_EVERY`], and at
    /// once after a `sudo` prompt was answered. Called once by the real service.
    pub fn start_owner_actions(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut shutdown = service.shutdown.subscribe();
            loop {
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = tokio::time::sleep(RECHECK_EVERY) => {}
                    _ = WAKE.notified() => {}
                }
                if *shutdown.borrow() {
                    break;
                }
                pass(&service.owner_places().await).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::store::{OpenMode, ProjectKey, Store, StoreChannel};
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        root: PathBuf,
        place: Place,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "owner-actions-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Release).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        cortex_core::lifecycle::TaskMachine::new(&store)
            .create("rt1", "Install the build tools", "owner")
            .unwrap();
        let r = root.to_string_lossy().into_owned();
        store
            .write(move |db| {
                let now = cortex_core::now();
                db.execute(
                    "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='rt1'",
                    [],
                )?;
                db.execute(
                    "INSERT INTO workspace_projects VALUES ('wp_1',?,'owner',?,1)",
                    (&r, &now),
                )?;
                db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('wt_1','wp_1','Publish the release\nmore',?)", [&now])?;
                db.execute("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at) VALUES ('we_1','wt_1','rt1','s1',1,?)", [&now])?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let executor = Arc::new(StoreExecutor::open(key).unwrap());
        Fixture {
            place: Place {
                project: format!("wp_{name}"),
                name: "Cedar".into(),
                root: root.clone(),
                executor,
                runtime: None,
            },
            root,
        }
    }
    async fn require(place: &Place, id: &str, service: &str) {
        let payload = json!({"id":id,"kind":"access","service":service,
            "detail":"gh is not logged in, so the pull request cannot be opened.","node":"n1","revision":1});
        place
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, "rt1", "OWNER_ACTION_REQUIRED", payload)?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    async fn events(place: &Place) -> Vec<(String, String)> {
        place
            .executor
            .read(|db| {
                let mut s = db.prepare(
                    "SELECT type,payload_json FROM runtime_events WHERE task_id='rt1' ORDER BY id",
                )?;
                let rows = s
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    }
    /// A fake CLI: exits with the code in `<dir>/<name>.exit`, and copies stdin to
    /// `<dir>/<name>.stdin` when asked to read it.
    fn fake(dir: &Path, name: &str, code: i32) -> PathBuf {
        let path = dir.join(format!("fake-{name}"));
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncase \"$*\" in *--with-token*) cat > '{0}/{name}.stdin';; esac\necho \"noise $*\"\necho err >&2\nexit $(cat '{0}/{name}.exit')\n",
                dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(dir.join(format!("{name}.exit")), code.to_string()).unwrap();
        path
    }
    fn set_exit(dir: &Path, name: &str, code: i32) {
        std::fs::write(dir.join(format!("{name}.exit")), code.to_string()).unwrap();
    }
    /// The fake programs are process-wide: tests that set them run one at a time.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test(flavor = "multi_thread")]
    async fn owner_access_card_uses_effective_language_and_keeps_command() {
        let f = fixture("access-language");
        require(&f.place, "oa_gh", "github").await;
        for (language, title, label) in [
            ("en", "Sign in to GitHub", "GitHub token"),
            ("ru", "Войдите в GitHub", "Токен GitHub"),
        ] {
            f.place.executor.write(move |db| {
                cortex_core::lifecycle::event_tx(
                    db, "rt1", "NATIVE_TASK_SETTINGS", json!({"settings":{"language":language}}),
                )?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            let listed = list(std::slice::from_ref(&f.place)).await;
            let card = &listed["actions"][0];
            assert_eq!(card["title"], title);
            assert_eq!(card["token"]["label"], label);
            assert_eq!(card["terminal"]["command"], "gh auth login");
            assert_eq!(card["detail"], "gh is not logged in, so the pull request cannot be opened.");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn owner_access_notifications_keep_each_card_and_withdraw_resolved_cards() {
        use crate::notifier::{items, Action, Book, Kind};
        let f = fixture("notices");
        require(&f.place, "oa_github", "github").await;
        require(&f.place, "oa_codex", "codex").await;
        let tasks = [json!({"taskId":"wt_1","executionId":"we_1","phase":"blocked"})];
        let product = json!({"tasks": {}});
        let decisions = std::collections::HashMap::from([("wt_1".into(), vec![
            json!({"decisionId":"decision1","status":"pending"})
        ])]);
        let mut book = Book::default();
        let enabled = Kind::ALL.into_iter().collect();
        let project = &f.place.project;
        let read = || async {
            f.place.executor.read(open_actions).unwrap().await.unwrap().unwrap()
        };
        let cards = read().await;
        let found = items(project, &tasks, &product, &decisions, &cards);
        assert_eq!(found.len(), 3);
        assert!(book.plan(project, found.clone(), None, &enabled, false).len() == 3);
        for (i, item) in found.iter().enumerate() {
            if item.kind == Kind::Stop {
                assert_eq!(item.target()["kind"], "task");
                assert!(!item.detail.as_deref().unwrap().contains("pull request"));
            }
            book.raised(item, i as u32 + 1);
        }
        assert!(book.plan(project, found, None, &enabled, false).is_empty());
        f.place.executor.write(|db| {
            cortex_core::lifecycle::event_tx(db, "rt1", "OWNER_ACTION_RESOLVED",
                json!({"id":"oa_github","how":"dismissed"}))?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        assert_eq!(
            book.plan(project, items(project, &tasks, &product, &decisions, &read().await),
                None, &enabled, false),
            vec![Action::Withdraw { id: 1 }]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_actions_are_listed_with_their_task_and_the_right_card() {
        let f = fixture("list");
        require(&f.place, "oa_gh1", "github").await;
        require(&f.place, "oa_cx1", "codex").await;
        require(&f.place, "oa_su1", "sudo").await;
        require(&f.place, "oa_x1", "vault").await;
        // A second open action of the same task and service shows the newest only.
        require(&f.place, "oa_gh2", "github").await;
        let listed = list(std::slice::from_ref(&f.place)).await;
        let actions = listed["actions"].as_array().unwrap();
        let ids: Vec<&str> = actions
            .iter()
            .map(|a| a["actionId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["oa_cx1", "oa_su1", "oa_x1", "oa_gh2"]);
        let gh = &actions[3];
        assert_eq!(gh["title"], "Sign in to GitHub");
        assert_eq!(
            (gh["projectId"].as_str(), gh["projectName"].as_str()),
            (Some("wp_list"), Some("Cedar"))
        );
        assert_eq!(
            (gh["taskId"].as_str(), gh["task"].as_str()),
            (Some("wt_1"), Some("Publish the release"))
        );
        assert_eq!(gh["terminal"]["command"], "gh auth login");
        assert_eq!(gh["token"]["label"], "GitHub token");
        assert_eq!(gh["recheck"], true);
        assert!(gh["openedAt"].as_u64().unwrap() > 1_700_000_000_000);
        let cx = &actions[0];
        assert_eq!(cx["terminal"]["argv"], json!(["codex", "login"]));
        assert!(cx["token"].is_null());
        let su = &actions[1];
        assert!(su["terminal"].is_null() && su["token"].is_null());
        assert_eq!(su["recheck"], true);
        let other = &actions[2];
        assert_eq!(other["title"], "Access needed: vault");
        assert_eq!(other["recheck"], false);
        assert_eq!(
            access("claude").terminal,
            Some(&["claude", "auth", "login"][..])
        );
        assert!(listed["phoneTokens"].is_boolean());
        // A finished task's actions are not listed.
        f.place
            .executor
            .write(|db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='rt1'",
                    [],
                )?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            list(std::slice::from_ref(&f.place)).await["actions"],
            json!([])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_npm_auth_lists_a_dismissible_registry_host_action() {
        use cortex_core::lifecycle::obstacles::{self, Output, Signals};

        let f = fixture("registry-auth");
        let id = f
            .place
            .executor
            .write(|db| {
                let found = obstacles::classify(&Signals {
                    outputs: vec![Output {
                        argv: vec!["npm".into(), "install".into()],
                        text: "npm error 401 Unauthorized - GET https://registry.npmjs.org/".into(),
                    }],
                    ..Default::default()
                });
                Ok(obstacles::open_obstacle_tx(db, "rt1", &found, None, 0)?.unwrap())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let places = std::slice::from_ref(&f.place);
        let listed = list(places).await;
        let actions = listed["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["actionId"], id);
        assert_eq!(actions[0]["service"], "registry.npmjs.org");
        assert_eq!(actions[0]["recheck"], false);
        assert!(actions[0]["terminal"].is_null());
        assert!(actions[0]["token"].is_null());

        assert_eq!(
            act(&f.place, &id, Act::Dismiss).await.unwrap(),
            json!({"outcome":"resolved","how":"dismissed"})
        );
        assert_eq!(list(places).await["actions"], json!([]));
        assert!(events(&f.place).await.iter().any(|(kind, payload)| {
            kind == "OWNER_ACTION_RESOLVED"
                && serde_json::from_str::<Value>(payload).unwrap()
                    == json!({"id":id,"how":"dismissed"})
        }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dismiss_and_terminal_resolve_through_the_store_once() {
        let f = fixture("dismiss");
        require(&f.place, "oa_cx1", "codex").await;
        let reply = act(&f.place, "oa_cx1", Act::Terminal).await.unwrap();
        assert_eq!(
            reply,
            json!({"outcome":"terminal","command":"codex login","argv":["codex","login"]})
        );
        let reply = act(&f.place, "oa_cx1", Act::Dismiss).await.unwrap();
        assert_eq!(reply, json!({"outcome":"resolved","how":"dismissed"}));
        let recorded = events(&f.place).await;
        let resolved: Vec<_> = recorded
            .iter()
            .filter(|(t, _)| t == "OWNER_ACTION_RESOLVED")
            .collect();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&resolved[0].1).unwrap(),
            json!({"id":"oa_cx1","how":"dismissed"})
        );
        assert_eq!(
            act(&f.place, "oa_cx1", Act::Dismiss)
                .await
                .unwrap_err()
                .to_string(),
            "unknown_owner_action"
        );
        // A service without a terminal login has no terminal action.
        require(&f.place, "oa_su1", "sudo").await;
        assert_eq!(
            act(&f.place, "oa_su1", Act::Terminal)
                .await
                .unwrap_err()
                .to_string(),
            "owner_terminal_unsupported"
        );
        assert_eq!(
            act(&f.place, "oa_su1", Act::Token(Secret::new("x".into())))
                .await
                .unwrap_err()
                .to_string(),
            "owner_token_unsupported"
        );
    }

    /// "Check again" runs the one probe (a fake `gh`): no access keeps the action open
    /// with the status; access resolves it, named a terminal login after the owner opened
    /// one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_recheck_resolves_only_when_access_is_there() {
        let _one = SERIAL.lock().await;
        let f = fixture("recheck");
        let gh = fake(&f.root, "gh", 1);
        TEST_PROGRAMS
            .lock()
            .unwrap()
            .insert("gh_auth".into(), gh.clone());
        require(&f.place, "oa_gh1", "github").await;
        let reply = act(&f.place, "oa_gh1", Act::Recheck).await.unwrap();
        assert_eq!(
            reply,
            json!({"outcome":"still_needed","status":"not_logged_in"})
        );
        let listed = list(std::slice::from_ref(&f.place)).await;
        assert_eq!(listed["actions"][0]["lastStatus"], "not_logged_in");
        act(&f.place, "oa_gh1", Act::Terminal).await.unwrap();
        set_exit(&f.root, "gh", 0);
        let reply = act(&f.place, "oa_gh1", Act::Recheck).await.unwrap();
        assert_eq!(reply, json!({"outcome":"resolved","how":"terminal_login"}));
        assert_eq!(
            list(std::slice::from_ref(&f.place)).await["actions"],
            json!([])
        );
        TEST_PROGRAMS.lock().unwrap().clear();
    }

    /// The pasted token reaches the fake CLI's stdin and nothing else: not the store, an
    /// event, the reply, the list or the request's leftovers.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_token_reaches_only_the_cli_stdin() {
        let _one = SERIAL.lock().await;
        let f = fixture("token");
        let gh = fake(&f.root, "gh", 0);
        {
            let mut programs = TEST_PROGRAMS.lock().unwrap();
            programs.insert("gh".into(), gh.clone());
            programs.insert("gh_auth".into(), gh.clone());
        }
        require(&f.place, "oa_gh1", "github").await;
        let token = "ghp_fixtureTOKEN0123456789abcdef";
        // A refused token: the CLI exits non-zero, the action stays open.
        set_exit(&f.root, "gh", 1);
        let (project, id, request) = parse_act(
            json!({"projectId":"wp_1","actionId":"oa_gh1","action":"token","token":token}),
        )
        .unwrap();
        assert_eq!((project.as_str(), id.as_str()), ("wp_1", "oa_gh1"));
        let refused = act(&f.place, &id, request).await.unwrap_err().to_string();
        assert_eq!(refused, "owner_token_refused");
        assert!(!refused.contains(token));
        set_exit(&f.root, "gh", 0);
        let (_, _, request) = parse_act(
            json!({"projectId":"wp_1","actionId":"oa_gh1","action":"token","token":token}),
        )
        .unwrap();
        let reply = act(&f.place, "oa_gh1", request).await.unwrap();
        assert_eq!(reply, json!({"outcome":"resolved","how":"token"}));
        assert_eq!(
            std::fs::read_to_string(f.root.join("gh.stdin")).unwrap(),
            format!("{token}\n")
        );
        let recorded = events(&f.place).await;
        assert!(
            recorded.iter().all(|(_, p)| !p.contains(token)),
            "{recorded:?}"
        );
        assert!(recorded
            .iter()
            .any(|(t, p)| t == "OWNER_ACTION_RESOLVED" && p.contains("\"token\"")));
        assert!(!list(std::slice::from_ref(&f.place))
            .await
            .to_string()
            .contains(token));
        // Nothing in the store file holds it either.
        let db = f.root.join(".cortex");
        for entry in std::fs::read_dir(&db).unwrap().flatten() {
            if entry.path().is_file() {
                let bytes = std::fs::read(entry.path()).unwrap();
                assert!(
                    !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
                    "{}",
                    entry.path().display()
                );
            }
        }
        TEST_PROGRAMS.lock().unwrap().clear();
    }

    #[test]
    fn an_act_request_has_exactly_one_shape() {
        for bad in [
            json!({}),
            json!({"projectId":"wp_1","actionId":"oa_1"}),
            json!({"projectId":"wp_1","actionId":"oa_1","action":"other"}),
            json!({"projectId":"wp_1","actionId":"oa_1","action":"recheck","extra":1}),
            json!({"projectId":"wp_1","actionId":"oa_1","action":"token"}),
            json!({"projectId":"wp_1","actionId":"oa_1","action":"token","token":7}),
            json!({"projectId":"wp_1","actionId":"oa_1","action":"token","password":"x"}),
            json!({"projectId":"wp 1","actionId":"oa_1","action":"recheck"}),
            json!({"projectId":"wp_1","actionId":"","action":"recheck"}),
        ] {
            assert_eq!(
                parse_act(bad.clone()).err().unwrap().to_string(),
                "invalid_params",
                "{bad}"
            );
        }
        for bad in ["", "a\nb", &"x".repeat(TOKEN_BYTES + 1)] {
            assert_eq!(
                parse_act(
                    json!({"projectId":"wp_1","actionId":"oa_1","action":"token","token":bad})
                )
                .err()
                .unwrap()
                .to_string(),
                "invalid_owner_token"
            );
        }
        for action in ["recheck", "terminal", "dismiss"] {
            assert!(
                parse_act(json!({"projectId":"wp_1","actionId":"oa_1","action":action})).is_ok()
            );
        }
    }

    /// The background pass resolves an action once its probe sees the access, and an
    /// answered `sudo` prompt of the same task resolves its sudo action.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_background_pass_resolves_when_access_appears() {
        let _one = SERIAL.lock().await;
        let f = fixture("pass");
        let gh = fake(&f.root, "gh", 1);
        let sudo = fake(&f.root, "sudo", 1);
        {
            let mut programs = TEST_PROGRAMS.lock().unwrap();
            programs.insert("gh_auth".into(), gh);
            programs.insert("sudo".into(), sudo);
        }
        require(&f.place, "oa_gh1", "github").await;
        require(&f.place, "oa_su1", "sudo").await;
        let places = vec![f.place.clone()];
        assert_eq!(pass_at(&places, 100).await, 0);
        set_exit(&f.root, "gh", 0);
        assert_eq!(pass_at(&places, 159).await, 0);
        assert_eq!(pass_at(&places, 160).await, 1);
        let left = list(&places).await["actions"].clone();
        assert_eq!(left[0]["actionId"], "oa_su1", "{left}");
        sudo_answered(&f.root, "rt1");
        assert_eq!(pass_at(&places, 161).await, 1);
        assert_eq!(list(&places).await["actions"], json!([]));
        let hows: Vec<Value> = events(&f.place)
            .await
            .into_iter()
            .filter(|(t, _)| t == "OWNER_ACTION_RESOLVED")
            .map(|(_, p)| serde_json::from_str(&p).unwrap())
            .collect();
        assert_eq!(
            hows,
            [
                json!({"id":"oa_gh1","how":"recheck"}),
                json!({"id":"oa_su1","how":"recheck"})
            ]
        );
        TEST_PROGRAMS.lock().unwrap().clear();
    }
}

/// `owner_actions.act` on a machine MCP sign-in card. The card closes only when the server
/// is ready or the owner turns the server off in Settings; it cannot be dismissed.
async fn mcp_act(id: &str, act: Act) -> io::Result<Value> {
    use crate::native_mcp_signin as signin;
    if !signin::is_open(id) {
        return Err(crate::failure("unknown_owner_action"));
    }
    match act {
        Act::Open => signin::url_of(id)
            .map(|url| json!({"outcome":"open","url":url}))
            .ok_or_else(|| crate::failure("mcp_signin_link_unavailable")),
        Act::Recheck => {
            signin::wake();
            tokio::time::sleep(Duration::from_millis(1500)).await;
            Ok(if signin::is_open(id) {
                json!({"outcome":"still_needed","status":"waiting_for_sign_in"})
            } else {
                json!({"outcome":"resolved","how":"recheck"})
            })
        }
        Act::Dismiss => Err(crate::failure("mcp_signin_turn_off_in_settings")),
        Act::Terminal => Err(crate::failure("owner_terminal_unsupported")),
        Act::Token(_) => Err(crate::failure("owner_token_unsupported")),
    }
}

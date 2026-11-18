//! Short owner-facing texts written by a model: task titles, commit messages and
//! pull request texts.
//!
//! One mechanism serves every text. The route is the cheapest model of the
//! connection whose plan limit is the least used
//! ([`crate::native_routes::short_text`]). The completion is one prompt in a
//! minimal ACP engine session on that route, launched with the same hardened
//! launch every engine session uses ([`crate::short_text_engine`]): no tools or
//! MCP servers, an empty private working directory, the engine's read-only mode,
//! every permission request denied, a fixed budget, and the engine shut down
//! and reaped afterwards. It has no runtime task, writes no project store and
//! never appears in a task's history.
//!
//! Nothing waits for a model: an action starts a background job and returns its
//! id at once, and `short_text_poll` reads the job. A job that cannot get a
//! route, a hardened launch, an answer in time or a usable answer ends with a
//! deterministic fallback text instead. A title is saved only under a free
//! `machine.lock` taken without waiting (retried in the background for a
//! bounded time), and never over a title the owner wrote.
//!
//! The private `short-text.log` in the service log folder records which route
//! was chosen and why, and how each job ended: event names, engine, model,
//! connection id, tier, reason and duration only, never a prompt or an answer.
use super::{context, git::Git, native::*, settings};
use crate::native_routes::short_text::ShortTextRoute;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// One model run: engine start, session setup, the prompt and its answer.
/// Long enough for a cold engine start (the session setup steps have 15 s each
/// at most) and short enough that a dialog still waiting for its text is not
/// left waiting for long.
pub(super) const RUN_BUDGET: Duration = Duration::from_secs(25);
/// How long a generated title waits for a free `machine.lock` before it is dropped.
const SAVE_BUDGET: Duration = Duration::from_secs(20);
/// Model runs at once on this machine; a job over it uses its fallback text.
const MAX_RUNNING: usize = if cfg!(test) { 64 } else { 2 };
const MAX_DESCRIPTION: usize = 6000;
const MAX_EXCERPT: usize = 12_000;
const MAX_FILE_EXCERPT: usize = 2_000;
const MAX_LISTED: usize = 200;

/// Whether `p` is one of this module's `product.task` actions.
pub(super) fn handles(p: &Value) -> bool {
    matches!(
        p["action"].as_str(),
        Some("generate_title" | "draft_integration_text" | "short_text_poll")
    )
}

// ---------------------------------------------------------------- jobs

struct Job {
    project: String,
    task: String,
    key: String,
    started: Instant,
    done: Option<Value>,
}
fn jobs() -> std::sync::MutexGuard<'static, BTreeMap<String, Job>> {
    static JOBS: OnceLock<Mutex<BTreeMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
fn running_models() -> &'static Mutex<usize> {
    static RUNNING: OnceLock<Mutex<usize>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(0))
}
fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = format!(
        "{}:{}:{:?}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
    );
    format!("st_{}", &super::files::revision(seed.as_bytes())[..20])
}
/// Register a job, or return the running one with the same key for the same task.
fn begin(project: &str, task: &str, key: &str) -> (String, bool) {
    let mut table = jobs();
    table.retain(|_, j| j.started.elapsed() < Duration::from_secs(600));
    if let Some((id, _)) = table
        .iter()
        .find(|(_, j)| j.done.is_none() && j.project == project && j.task == task && j.key == key)
    {
        return (id.clone(), false);
    }
    while table.len() >= 64 {
        let oldest = table
            .iter()
            .min_by_key(|(_, j)| j.started)
            .map(|(id, _)| id.clone());
        match oldest {
            Some(id) => table.remove(&id),
            None => break,
        };
    }
    let id = new_id();
    table.insert(
        id.clone(),
        Job {
            project: project.into(),
            task: task.into(),
            key: key.to_owned(),
            started: Instant::now(),
            done: None,
        },
    );
    (id, true)
}
fn finish(id: &str, reply: Value) {
    if let Some(job) = jobs().get_mut(id) {
        job.done = Some(reply);
    }
}
fn spawn(id: String, work: impl FnOnce() -> Value + Send + 'static) -> Result<()> {
    let job = id.clone();
    std::thread::Builder::new()
        .name("short-text".into())
        .spawn(move || {
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| json!({"state":"done","source":"fallback","reason":"failed"}));
            finish(&job, reply);
        })
        .map(|_| ())
        .map_err(|_| {
            finish(
                &id,
                json!({"state":"done","source":"fallback","reason":"failed"}),
            );
            refuse("machine_busy", "The text could not be started.")
        })
}

// ---------------------------------------------------------------- log

/// One private diagnostic line: names and route identity only, never text.
pub(super) fn log(state: &Path, event: &str, fields: Value) {
    let _ = (|| -> Result<()> {
        let _lock = settings::Lock::new(state, "short-text-log.lock")?;
        let folder = settings::log_folder(state)?;
        let current = folder.join("short-text.log");
        if std::fs::symlink_metadata(&current)
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(refuse("unsafe_log_file", "Unsafe log file."));
        }
        if std::fs::metadata(&current).is_ok_and(|m| m.len() >= 65536) {
            std::fs::rename(&current, folder.join("short-text.log.1"))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&current)?;
        let mut line = json!({"at":now_ms(),"event":event});
        if let Some(map) = fields.as_object() {
            for (k, v) in map {
                line[k] = v.clone();
            }
        }
        writeln!(file, "{line}")?;
        Ok(())
    })();
}
fn route_fields(route: &ShortTextRoute) -> Value {
    json!({"engine":route.engine,"model":route.model,"connection":route.connection,
        "tier":route.tier,"used":route.used,"reason":route.reason})
}

// ---------------------------------------------------------------- one completion

/// A chosen route and the one run it allows.
pub(super) struct Chosen {
    pub route: ShortTextRoute,
    /// One prompt with its budget in, the answer or the reason there is none out.
    pub run: Box<dyn FnOnce(&str, Duration) -> std::result::Result<String, &'static str> + Send>,
}

/// The route for one short text on this machine and its hardened ACP launch
/// ([`crate::native_routes::SelectedRoute::session_launch`]), run through
/// [`crate::short_text_engine::complete`], or why there is none.
fn choose(config: &crate::Config, root: &Path) -> std::result::Result<Chosen, &'static str> {
    let environment = super::config_read::environment(config).map_err(|_| "no_settings")?;
    let key = context::key(root).map_err(|_| "no_project_store")?;
    let resolver = crate::native_routes::RouteResolver::load(root, &key.directory(), environment)
        .map_err(|_| "no_routing")?;
    let (route, selected) = resolver.short_text_route().ok_or("no_route")?;
    let launch = selected
        .session_launch(root)
        .map_err(|_| "engine_unavailable")?;
    let (model, parameters) = (selected.model().to_owned(), selected.parameters.clone());
    Ok(Chosen {
        route,
        run: Box::new(move |prompt, budget| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "engine_unavailable")?;
            runtime.block_on(crate::short_text_engine::complete(
                launch,
                &model,
                &parameters,
                prompt,
                budget,
            ))
        }),
    })
}

/// The model's text for one purpose, or why the fallback is used. Logs the choice.
pub(super) fn generate(
    state: &Path,
    purpose: &'static str,
    prompt: &str,
    chosen: std::result::Result<Chosen, &'static str>,
    budget: Duration,
) -> (std::result::Result<String, &'static str>, Value) {
    let chosen = match chosen {
        Ok(chosen) => chosen,
        Err(reason) => {
            log(
                state,
                "short_text_fallback",
                json!({"purpose":purpose,"outcome":reason}),
            );
            return (Err(reason), Value::Null);
        }
    };
    let fields = route_fields(&chosen.route);
    let mut selected = fields.clone();
    selected["purpose"] = json!(purpose);
    log(state, "short_text_selected", selected);
    {
        let mut running = running_models().lock().unwrap_or_else(|e| e.into_inner());
        if *running >= MAX_RUNNING {
            drop(running);
            log(
                state,
                "short_text_fallback",
                json!({"purpose":purpose,"outcome":"too_many_running"}),
            );
            return (Err("too_many_running"), fields);
        }
        *running += 1;
    }
    let started = Instant::now();
    // The slot is given back even if the run panics.
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            let mut running = running_models().lock().unwrap_or_else(|e| e.into_inner());
            *running = running.saturating_sub(1);
        }
    }
    let slot = Slot;
    let result = (chosen.run)(prompt, budget);
    drop(slot);
    let mut done = fields.clone();
    done["purpose"] = json!(purpose);
    done["ms"] = json!(started.elapsed().as_millis() as u64);
    match &result {
        Ok(_) => log(state, "short_text_generated", done),
        Err(reason) => {
            done["outcome"] = json!(reason);
            log(state, "short_text_fallback", done)
        }
    }
    (result, fields)
}

// ---------------------------------------------------------------- text shaping

/// Unicode format characters (general category Cf): bidirectional embeddings,
/// overrides and isolates, zero-width characters, the byte order mark, invisible
/// operators and tag characters. They can make a title or a commit line read
/// differently from what it is, so no text this module produces keeps them.
pub(super) fn is_format(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}
/// `text` without control characters (except newlines) and format characters.
pub(super) fn visible(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || !(c.is_control() || is_format(*c)))
        .collect()
}
fn clean(text: &str) -> String {
    let text = visible(text);
    let mut lines: Vec<&str> = text.lines().collect();
    // A fenced answer keeps only what is inside the fence.
    if lines
        .first()
        .is_some_and(|l| l.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if let Some(end) = lines.iter().position(|l| l.trim_start().starts_with("```")) {
            lines.truncate(end);
        }
    }
    lines.join("\n").trim().to_owned()
}
fn strip_label(line: &str, labels: &[&str]) -> String {
    let quotes: &[char] = &[
        '"', '\'', '`', '\u{201c}', '\u{201d}', '\u{00ab}', '\u{00bb}', '*',
    ];
    let mut line = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(quotes)
        .trim()
        .to_owned();
    for label in labels {
        if line
            .get(..label.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(label))
        {
            line = line[label.len()..].trim().to_owned();
        }
    }
    line.trim_matches(quotes).trim().to_owned()
}
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    match head.rfind(' ') {
        Some(at) if at > max / 2 => head[..at].trim_end().to_owned(),
        _ => head,
    }
}
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A usable title from a model answer.
pub(super) fn title_of(answer: &str) -> Option<String> {
    let first = clean(answer)
        .lines()
        .find(|l| !l.trim().is_empty())?
        .to_owned();
    let title = one_line(&strip_label(&first, &["title:", "task title:"]));
    let title = title
        .trim_end_matches(['.', '!', ';', ':'])
        .trim()
        .to_owned();
    let title = cut(&title, 80);
    (!title.is_empty()).then_some(title)
}
/// The label shown when no title could be generated: the first sentence, at most 60 characters.
pub(super) fn fallback_title(description: &str) -> String {
    let description = visible(description);
    let trimmed = description.trim();
    let mut first = trimmed.split('\n').next().unwrap_or("");
    for (i, c) in first.char_indices() {
        if matches!(c, '.' | '!' | '?')
            && first[i + c.len_utf8()..].starts_with(char::is_whitespace)
        {
            first = &first[..i];
            break;
        }
    }
    let first = one_line(first);
    let first = first.trim_end_matches(['.', '!', '?']);
    let label = if first.chars().count() > 60 {
        format!("{}…", cut(first, 60))
    } else {
        first.to_owned()
    };
    let mut chars = label.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Untitled task".into(),
    }
}
fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[…]", &text[..end])
}
fn title_prompt(description: &str) -> String {
    format!(
        "Write a title for the software task below.\n\
         Rules: at most 8 words; name what the task changes or delivers, not how it is asked; \
         use the language of the task text; no quotes, no trailing period, no prefix.\n\
         Reply with the title only.\n\n<task>\n{}\n</task>\n",
        bounded(description, MAX_DESCRIPTION)
    )
}

// ---------------------------------------------------------------- titles

fn owner_titled(root: &Path, project: &str, task: &str) -> Result<bool> {
    let Some(store) = context::open(root, cortex_core::OpenMode::LiveReadOnly)? else {
        return Ok(false);
    };
    let options: Option<String> = store
        .read(|db| {
            use rusqlite::OptionalExtension;
            Ok(db
                .query_row(
                    "SELECT o.options_json FROM workspace_task_options o JOIN workspace_tasks t USING(task_id)
                     WHERE t.project_id=? AND t.task_id=?",
                    (project, task),
                    |r| r.get(0),
                )
                .optional()?)
        })
        .map_err(context::core)?;
    Ok(options
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .is_some_and(|o| o.get("draftTitle").is_some()))
}
fn description(root: &Path, project: &str, task: &str) -> Result<Option<String>> {
    let Some(store) = context::open(root, cortex_core::OpenMode::LiveReadOnly)? else {
        return Ok(None);
    };
    store
        .read(|db| {
            use rusqlite::OptionalExtension;
            Ok(db
                .query_row(
                    "SELECT description FROM workspace_tasks WHERE project_id=? AND task_id=?",
                    (project, task),
                    |r| r.get(0),
                )
                .optional()?)
        })
        .map_err(context::core)
}

/// Save a title unless the owner named the task meanwhile. Takes `machine.lock`
/// only when it is free, retrying until `budget` ends. Whether it was saved.
pub(super) fn save_title(
    state: &Path,
    project: &str,
    task: &str,
    title: &str,
    generated: bool,
    owner_titled: impl Fn() -> bool,
    budget: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + budget;
    let _lock = loop {
        if let Some(lock) = settings::Lock::try_new(state, "machine.lock")? {
            break lock;
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if owner_titled() {
        return Ok(false);
    }
    let mut product = super::native::product(state, project)?;
    if !product["tasks"].is_object() {
        return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
    }
    let record = &mut product["tasks"][task];
    if !record.is_object() {
        *record = json!({});
    }
    if record["titleState"] == "user"
        || record["purged"] == true
        || (record["titleState"] == "generated" && record["title"].is_string())
    {
        return Ok(false);
    }
    record["title"] = json!(title);
    record["titleState"] = json!(if generated { "generated" } else { "failed" });
    settings::write_json(&product_path(state, project)?, &product)?;
    Ok(true)
}

/// The title job: a model title, else the fallback label, saved in the background.
pub(super) fn title_job(
    state: &Path,
    project: &str,
    task: &str,
    description: &str,
    chosen: std::result::Result<Chosen, &'static str>,
    owner_titled: impl Fn() -> bool,
    budget: Duration,
) -> Value {
    let (answer, route) = generate(state, "title", &title_prompt(description), chosen, budget);
    let (title, reason) = match answer.map(|a| title_of(&a).ok_or("unusable_answer")) {
        Ok(Ok(title)) => (title, None),
        Ok(Err(reason)) | Err(reason) => (fallback_title(description), Some(reason)),
    };
    let saved = save_title(
        state,
        project,
        task,
        &title,
        reason.is_none(),
        owner_titled,
        SAVE_BUDGET,
    )
    .unwrap_or(false);
    json!({"state":"done","source":if reason.is_none(){"model"}else{"fallback"},
        "reason":reason,"saved":saved,"route":route})
}

// ---------------------------------------------------------------- changes

/// What changed for one integration text, read from Git.
#[derive(Default, Debug)]
pub(super) struct Changes {
    /// `(path, added, deleted)`; `None` counts are binary files.
    pub files: Vec<(String, Option<u64>, Option<u64>)>,
    pub untracked: Vec<String>,
    pub excerpt: String,
}
/// Whether a changed file is listed by name only and never read for the excerpt:
/// environment files, SSH keys, certificates and key stores, package-registry
/// and netrc credentials, Terraform variables and state, Kubernetes configs,
/// cloud credential and service-account files, htpasswd, and any name that
/// mentions a secret, credential, password or token.
pub(crate) fn private_name(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    name.starts_with(".env")
        || [
            "id_rsa",
            "id_dsa",
            "id_ecdsa",
            "id_ed25519",
            "service-account",
            "kubeconfig",
        ]
        .iter()
        .any(|p| name.starts_with(p))
        || [
            ".netrc",
            ".npmrc",
            ".pypirc",
            ".htpasswd",
            "credentials",
            "credentials.json",
        ]
        .contains(&name)
        || [
            ".pem",
            ".key",
            ".p12",
            ".pfx",
            ".keystore",
            ".jks",
            ".kdbx",
            ".tfvars",
            ".tfstate",
            ".tfstate.backup",
        ]
        .iter()
        .any(|s| name.ends_with(s))
        || path == ".kube/config"
        || path.ends_with("/.kube/config")
        || ["secret", "credential", "password", "token"]
            .iter()
            .any(|w| name.contains(w))
}
fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 4096
        && !p.starts_with('/')
        && !p.split('/').any(|s| s == "..")
        && !p.chars().any(char::is_control)
}
/// Changed files, counts and a bounded excerpt of the diff. For `commit` the
/// owner's pending paths against `HEAD`; otherwise the checkout against its
/// merge base with `target`. Untracked files are listed by name only, and files
/// whose names look private never contribute content.
pub(super) async fn changes(
    path: &Path,
    method: &str,
    target: &str,
    pending: &[String],
    git: &mut Git,
) -> Result<Changes> {
    let base = if method == "commit" {
        "HEAD".to_owned()
    } else {
        if target.starts_with('-') || target.is_empty() {
            return Err(refuse("invalid_branch", "Choose a valid target branch."));
        }
        git.out(path, &["merge-base", target, "HEAD"])
            .await?
            .trim()
            .to_owned()
    };
    let literal: Vec<String> = pending.iter().map(|p| format!(":(literal){p}")).collect();
    let mut numstat = vec!["diff", "--numstat", "--no-renames", base.as_str(), "--"];
    numstat.extend(literal.iter().map(String::as_str));
    let mut changes = Changes::default();
    if method != "commit" || !pending.is_empty() {
        for line in git.out(path, &numstat).await?.lines().take(MAX_LISTED) {
            let mut parts = line.splitn(3, '\t');
            let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            changes
                .files
                .push((p.to_owned(), a.parse().ok(), d.parse().ok()));
        }
    }
    let listed: std::collections::BTreeSet<_> = changes.files.iter().map(|f| f.0.clone()).collect();
    if method == "commit" {
        changes.untracked = pending
            .iter()
            .filter(|p| !listed.contains(*p))
            .take(MAX_LISTED)
            .cloned()
            .collect();
    } else {
        let others = git
            .out(path, &["ls-files", "--others", "--exclude-standard", "-z"])
            .await
            .unwrap_or_default();
        changes.untracked = others
            .split('\0')
            .filter(|p| !p.is_empty())
            .take(MAX_LISTED)
            .map(str::to_owned)
            .collect();
    }
    // Smallest edits first, so more files fit in the excerpt.
    let mut order: Vec<_> = changes
        .files
        .iter()
        .filter(|(p, a, d)| a.is_some() && d.is_some() && !private_name(p))
        .collect();
    order.sort_by_key(|(_, a, d)| a.unwrap_or(0) + d.unwrap_or(0));
    for (file, _, _) in order.into_iter().take(15) {
        if changes.excerpt.len() >= MAX_EXCERPT {
            break;
        }
        let spec = format!(":(literal){file}");
        let Ok(diff) = git
            .out(
                path,
                &[
                    "diff",
                    "--unified=2",
                    "--no-renames",
                    base.as_str(),
                    "--",
                    &spec,
                ],
            )
            .await
        else {
            continue;
        };
        let room = (MAX_EXCERPT - changes.excerpt.len()).min(MAX_FILE_EXCERPT);
        changes.excerpt.push_str(&bounded(diff.trim_end(), room));
        changes.excerpt.push('\n');
    }
    Ok(changes)
}
fn file_lines(changes: &Changes, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = changes
        .files
        .iter()
        .map(|(p, a, d)| match (a, d) {
            (Some(a), Some(d)) => format!("{} (+{a} -{d})", visible(p)),
            _ => format!("{} (binary)", visible(p)),
        })
        .chain(
            changes
                .untracked
                .iter()
                .map(|p| format!("{} (new)", visible(p))),
        )
        .collect();
    let total = lines.len();
    if total > max {
        lines.truncate(max);
        lines.push(format!("and {} more", total - max));
    }
    lines
}
fn areas(changes: &Changes) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for p in changes
        .files
        .iter()
        .map(|f| &f.0)
        .chain(changes.untracked.iter())
    {
        let parts: Vec<&str> = p.split('/').collect();
        let area = if parts.len() > 2 {
            format!("{}/{}", parts[0], parts[1])
        } else if parts.len() == 2 {
            parts[0].to_owned()
        } else {
            p.clone()
        };
        if !seen.contains(&area) {
            seen.push(area);
        }
    }
    seen
}

/// A deterministic text from the real changes, used when no model text exists.
pub(super) fn fallback_integration(
    method: &str,
    changes: &Changes,
    description: &str,
    summary: &str,
) -> Value {
    let count = changes.files.len() + changes.untracked.len();
    let areas = areas(changes);
    let subject = match count {
        0 => format!("Apply task: {}", fallback_title(description)),
        _ => {
            let shown: Vec<_> = areas.iter().take(3).cloned().collect();
            let more = if areas.len() > 3 { " and more" } else { "" };
            format!(
                "Update {count} file{} in {}{more}",
                if count == 1 { "" } else { "s" },
                shown.join(", ")
            )
        }
    };
    let subject = cut(&subject, 72);
    let mut body = Vec::new();
    if !summary.trim().is_empty() {
        body.push(bounded(visible(summary).trim(), 2000));
    } else if count > 0 && !description.trim().is_empty() {
        body.push(fallback_title(description));
    }
    let files = file_lines(changes, 20);
    if !files.is_empty() {
        body.push(format!(
            "Changed files:\n{}",
            files
                .iter()
                .map(|l| format!("- {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let body = body.join("\n\n");
    if method == "pr" {
        json!({"title":subject,"body":body})
    } else {
        json!({"message":if body.is_empty(){subject}else{format!("{subject}\n\n{body}")}})
    }
}
fn integration_prompt(method: &str, changes: &Changes, description: &str, summary: &str) -> String {
    let what = if method == "pr" {
        "a pull request title and description. First line: the title (at most 72 characters, imperative mood, no prefix). \
         Then one empty line, then a short Markdown description: what changed and why, then a bullet list of the main changes"
    } else {
        "a Git commit message. First line: a summary of at most 72 characters in imperative mood, no prefix, no trailing period. \
         Then one empty line, then a short body: what changed and why, wrapped at 72 characters, bullet points allowed"
    };
    let files = file_lines(changes, 60).join("\n");
    format!(
        "Write {what}.\nDescribe the actual changes shown below; do not invent changes, do not mention tools, models or reviews. \
         Write in English. Reply with the text only, no code fences.\n\n<task>\n{}\n</task>\n\n<result_summary>\n{}\n</result_summary>\n\n\
         <changed_files>\n{}\n</changed_files>\n\n<diff_excerpt>\n{}\n</diff_excerpt>\n",
        bounded(description, MAX_DESCRIPTION),
        bounded(summary, 2000),
        files,
        changes.excerpt
    )
}
/// A usable integration text from a model answer.
pub(super) fn integration_of(method: &str, answer: &str) -> Option<Value> {
    let text = clean(answer);
    let mut lines = text.lines();
    let first = lines.by_ref().find(|l| !l.trim().is_empty())?;
    let subject = one_line(&strip_label(
        first,
        &["title:", "subject:", "commit message:"],
    ));
    let subject = cut(subject.trim_end_matches('.'), 100);
    if subject.is_empty() {
        return None;
    }
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
    let body = bounded(
        &strip_body_label(&body),
        if method == "pr" { 8000 } else { 4000 },
    );
    Some(if method == "pr" {
        json!({"title":subject,"body":body})
    } else {
        json!({"message":if body.is_empty(){subject}else{format!("{subject}\n\n{body}")}})
    })
}
fn strip_body_label(body: &str) -> String {
    for label in ["Description:", "Body:"] {
        if let Some(rest) = body.strip_prefix(label) {
            return rest.trim().to_owned();
        }
    }
    body.to_owned()
}

/// The integration text job: changes from Git, then the model text or the fallback.
pub(super) fn integration_job(
    state: &Path,
    method: &str,
    changes: Result<Changes>,
    description: &str,
    summary: &str,
    chosen: std::result::Result<Chosen, &'static str>,
    budget: Duration,
) -> Value {
    let purpose = if method == "pr" {
        "pull_request"
    } else {
        "commit"
    };
    let (changes, read) = match changes {
        Ok(c) => (c, None),
        Err(e) => (Changes::default(), Some(e.code)),
    };
    let fallback = || fallback_integration(method, &changes, description, summary);
    if let Some(code) = read {
        log(
            state,
            "short_text_fallback",
            json!({"purpose":purpose,"outcome":"changes_unreadable","code":code}),
        );
        return json!({"state":"done","source":"fallback","reason":"changes_unreadable","text":fallback(),"route":null});
    }
    let prompt = integration_prompt(method, &changes, description, summary);
    let (answer, route) = generate(state, purpose, &prompt, chosen, budget);
    match answer.map(|a| integration_of(method, &a).ok_or("unusable_answer")) {
        Ok(Ok(text)) => {
            json!({"state":"done","source":"model","reason":null,"text":text,"route":route})
        }
        Ok(Err(reason)) | Err(reason) => {
            json!({"state":"done","source":"fallback","reason":reason,"text":fallback(),"route":route})
        }
    }
}

// ---------------------------------------------------------------- actions

/// A draft job's identity: everything its text depends on. Two requests share a
/// running job only when they ask for the same method, target, pending paths and
/// summary; the paths and summary are kept as a digest.
pub(super) fn integration_key(
    method: &str,
    target: &str,
    pending: &[String],
    summary: &str,
) -> String {
    let inputs =
        serde_json::to_vec(&json!({"paths":pending,"summary":summary})).unwrap_or_default();
    format!(
        "integration:{method}:{}:{}",
        super::files::revision(target.as_bytes()),
        super::files::revision(&inputs)
    )
}
fn task_id(p: &Value) -> Result<&str> {
    p["taskId"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
        })
        .ok_or_else(|| refuse("invalid_task", "Unknown task."))
}

/// `generate_title`, `draft_integration_text` and `short_text_poll`. Never waits
/// for a model, `machine.lock` or the project writer.
pub(super) async fn run(
    root: &Path,
    project: &str,
    state: &Path,
    p: &Value,
    config: &crate::Config,
) -> Result<Value> {
    object_params(p)?;
    let task = task_id(p)?.to_owned();
    match p["action"].as_str().unwrap_or("") {
        "short_text_poll" => {
            let id = p["jobId"].as_str().unwrap_or("");
            let table = jobs();
            let job = table
                .get(id)
                .filter(|j| j.project == project && j.task == task)
                .ok_or_else(|| refuse("unknown_job", "This text is no longer being written."))?;
            Ok(job.done.clone().unwrap_or(json!({"state":"running"})))
        }
        "generate_title" => {
            let product = super::native::product(state, project)?;
            let record = &product["tasks"][&task];
            if record["titleState"] == "user" || owner_titled(root, project, &task)? {
                return Ok(json!({"jobId":null,"skipped":"owner_title"}));
            }
            let description = description(root, project, &task)?
                .ok_or_else(|| refuse("unknown_task", "Unknown task."))?;
            let (id, fresh) = begin(project, &task, "title");
            if fresh {
                let (root, project, state, config, task_for_job) = (
                    root.to_owned(),
                    project.to_owned(),
                    state.to_owned(),
                    config.clone(),
                    task.clone(),
                );
                spawn(id.clone(), move || {
                    #[cfg(test)]
                    let chosen = tests::held_route(&root).unwrap_or_else(|| choose(&config, &root));
                    #[cfg(not(test))]
                    let chosen = choose(&config, &root);
                    let owner = || owner_titled(&root, &project, &task_for_job).unwrap_or(true);
                    title_job(
                        &state,
                        &project,
                        &task_for_job,
                        &description,
                        chosen,
                        owner,
                        RUN_BUDGET,
                    )
                })?;
            }
            Ok(json!({"jobId":id}))
        }
        "draft_integration_text" => {
            let method = p["method"]
                .as_str()
                .filter(|m| matches!(*m, "commit" | "squash" | "pr"))
                .ok_or_else(|| refuse("invalid_request", "Choose commit, squash or pull request."))?
                .to_owned();
            let target = p["targetBranch"].as_str().unwrap_or("").to_owned();
            let pending: Vec<String> = match &p["paths"] {
                Value::Null => Vec::new(),
                Value::Array(a) if a.len() <= 2000 => a
                    .iter()
                    .map(|v| v.as_str().filter(|s| valid_path(s)).map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| refuse("invalid_request", "Invalid file list."))?,
                _ => return Err(refuse("invalid_request", "Invalid file list.")),
            };
            let summary = p["summary"]
                .as_str()
                .map(|s| bounded(s, 4000))
                .unwrap_or_default();
            let (id, fresh) = begin(
                project,
                &task,
                &integration_key(&method, &target, &pending, &summary),
            );
            if fresh {
                let (root, project, state, config) = (
                    root.to_owned(),
                    project.to_owned(),
                    state.to_owned(),
                    config.clone(),
                );
                let task_for_job = task.clone();
                spawn(id.clone(), move || {
                    let description = description(&root, &project, &task_for_job)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let read = (|| -> Result<Changes> {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()?;
                        runtime.block_on(async {
                            let mut git = Git::for_service(&config);
                            let product =
                                context::product(&root, &state, &project, &mut git).await?;
                            let checkout = &product["tasks"][&task_for_job]["checkout"];
                            let path = checkout["path"]
                                .as_str()
                                .filter(|_| checkout["kind"] == "worktree")
                                .map(PathBuf::from)
                                .unwrap_or_else(|| root.clone());
                            changes(&path, &method, &target, &pending, &mut git).await
                        })
                    })();
                    let chosen = choose(&config, &root);
                    integration_job(
                        &state,
                        &method,
                        read,
                        &description,
                        &summary,
                        chosen,
                        RUN_BUDGET,
                    )
                })?;
            }
            Ok(json!({"jobId":id}))
        }
        _ => Err(refuse(
            "unsupported_action",
            "This machine does not support that yet.",
        )),
    }
}

#[cfg(test)]
#[path = "short_text_tests.rs"]
mod tests;

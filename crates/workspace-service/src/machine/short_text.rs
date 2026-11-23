//! Background drafts for task titles and Git text. Task titles may use a
//! hardened, read-only ACP model session; Git drafts use only English
//! templates and Git metadata. Neither blocks the caller or changes Git.
//!
//! A title is saved only under a free `machine.lock`, retried for a bounded
//! time, and never over a title the owner wrote.
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
const MAX_LISTED: usize = 200;
pub(super) const MAX_PUSH_SUMMARY: usize = 8000;

/// Whether `p` is one of this module's `product.task` actions.
pub(super) fn handles(p: &Value) -> bool {
    matches!(
        p["action"].as_str(),
        Some("generate_title" | "draft_integration_text" | "draft_checkout_commit" | "draft_push_summary" | "short_text_poll")
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
/// Changed files and line counts. Git drafts do not read diff excerpts.
/// Task commits use the pending paths against `HEAD`; other task drafts use
/// the checkout against its merge base with `target`.
pub(super) async fn changes(
    path: &Path,
    method: &str,
    target: &str,
    pending: &[String],
    git: &mut Git,
) -> Result<Changes> {
    let checkout = method == "checkout_commit";
    let commit = checkout || method == "commit";
    let staged = checkout && !git.out(path, &["diff", "--cached", "--name-only", "-z", "--"]).await?.is_empty();
    let unborn = checkout && !staged && !git.success(path, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).await?;
    let base = if commit {
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
    let against = if staged { "--cached" } else { base.as_str() };
    let mut numstat = vec!["diff", "--numstat", "--no-renames", "-z", against, "--"];
    numstat.extend(literal.iter().map(String::as_str));
    let mut changes = Changes::default();
    if !unborn && (method != "commit" || !pending.is_empty()) {
        let stats = git.out(path, &numstat).await?;
        for line in stats.split_terminator('\0').take(MAX_LISTED) {
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
    } else if !staged {
        let others = git
            .out(path, &["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;

        changes.untracked = others
            .split('\0')
            .filter(|p| !p.is_empty())
            .take(MAX_LISTED)
            .map(str::to_owned)
            .collect();
    }
    Ok(changes)
}
fn file_lines(changes: &Changes, max: usize) -> Vec<String> {
    let total = changes.files.len() + changes.untracked.len();
    let mut lines: Vec<String> = changes
        .files
        .iter()
        .map(|(p, a, d)| match (a, d) {
            (Some(a), Some(d)) => format!("- `{}` (+{a} -{d})", git_path_text(p)),
            _ => format!("- `{}` (binary)", git_path_text(p)),
        })
        .chain(changes.untracked.iter().map(|p| format!("- `{}` (new)", git_path_text(p))))
        .take(max)
        .collect();
    if total > max {
        lines.push(format!("- and {} more", total - max));
    }
    lines
}
// Paths are Git identifiers, not prose. Keep them on a single line and
// escape non-ASCII and Markdown delimiters without interpreting their text.
fn git_path_text(path: &str) -> String {
    path.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_owned(),
            '`' => "\\u{60}".to_owned(),
            c if c.is_ascii_graphic() || c == ' ' => c.to_string(),
            c => c.escape_unicode().to_string(),
        })
        .collect::<Vec<_>>()
        .concat()
}

fn integration_text(method: &str, changes: &Changes) -> Value {
    let subject = match changes.files.len() + changes.untracked.len() {
        0 => "Apply task changes".to_owned(),
        1 => "Update 1 file".to_owned(),
        count => format!("Update {count} files"),
    };
    let files = file_lines(changes, 20);
    let body = if files.is_empty() {
        String::new()
    } else {
        format!("Changed files:\n{}", files.join("\n"))
    };
    if method == "pr" {
        json!({"title":subject,"body":body})
    } else {
        json!({"message":if body.is_empty(){subject}else{format!("{subject}\n\n{body}")}})
    }
}

fn clip_utf8(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Limit an owner-approved body without changing its Markdown or line breaks.
pub(super) fn push_body(text: &str) -> String {
    clip_utf8(&visible(text), MAX_PUSH_SUMMARY).to_owned()
}

fn integration_job(state: &Path, method: &str, changes: Result<Changes>) -> Value {
    match changes {
        Ok(changes) => json!({"state":"done","source":"template","reason":null,
            "text":integration_text(method, &changes),"route":null}),
        Err(error) => {
            let purpose = if method == "pr" { "pull_request" } else { "commit" };
            log(state, "short_text_fallback",
                json!({"purpose":purpose,"outcome":"changes_unreadable","code":error.code}));
            json!({"state":"done","source":"fallback","reason":"changes_unreadable",
                "text":integration_text(method, &Changes::default()),"route":null})
        }
    }
}

fn saved_pr(changes: Option<&Changes>, body: String) -> Value {
    let mut text = integration_text("pr", changes.unwrap_or(&Changes::default()));
    text["body"] = json!(body);
    json!({"state":"done","source":"saved","reason":null,"text":text,"route":null})
}

// ---------------------------------------------------------------- outgoing commits

/// Inspect only local refs. A new destination branch uses the known remote refs;
/// without any local baseline, do not present the entire repository as outgoing.
async fn outgoing(git: &mut Git, path: &Path, remote: &str, branch: &str) -> Result<usize> {
    let remote = git.configured_remote(path, remote).await?;
    let head = format!("refs/heads/{branch}");
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let namespace = format!("refs/remotes/{remote}/");
    let remote_exclusion = format!("--remotes={remote}");
    let mut range = vec![head.as_str(), "--not"];
    if git.success(path, &["rev-parse", "--verify", "--quiet", &format!("{tracking}^{{commit}}")]).await? {
        range.push(tracking.as_str());
    } else if git.out(path, &["for-each-ref", "--count=1", "--format=%(objectname)", &namespace]).await?.trim().is_empty() {
        return Err(refuse("push_baseline_unavailable", "Fetch the selected remote to establish which commits are outgoing."));
    } else {
        range.push(remote_exclusion.as_str());
    }
    let mut count = vec!["rev-list", "--count"];
    count.extend(range);
    count.push("--");
    match git.out(path, &count).await?.trim().parse::<usize>() {
        Ok(0) => Err(refuse("no_outgoing_commits", "There are no outgoing commits in the local tracking information.")),
        Ok(count) => Ok(count),
        Err(_) => Err(refuse("commits_unreadable", "Git did not return an outgoing commit count.")),
    }
}

fn push_job(state: &Path, outgoing: Result<usize>) -> Value {
    match outgoing {
        Ok(count) => {
            let noun = if count == 1 { "commit" } else { "commits" };
            json!({"state":"done","source":"template","reason":null,"route":null,
                "text":{"title":format!("Push {count} outgoing {noun}"),
                    "body":format!("This push includes {count} outgoing {noun}.")}})
        }
        Err(error) => {
            let reason = match error.code {
                "no_outgoing_commits" => "no_outgoing_commits",
                "push_baseline_unavailable" => "push_baseline_unavailable",
                _ => "commits_unreadable",
            };
            log(state, "short_text_fallback", json!({"purpose":"push","outcome":reason}));
            json!({"state":"done","source":"fallback","reason":reason,"text":{},"route":null})
        }
    }
}

// ---------------------------------------------------------------- actions

/// A draft job's identity: everything its text depends on. Two requests share a
/// running job only when they ask for the same method, target and pending paths
/// (or the same checkout, remote and branch for a push summary).
pub(super) fn integration_key(
    method: &str,
    target: &str,
    pending: &[String],
    remote: &str,
) -> String {
    let inputs =
        serde_json::to_vec(&json!({"paths":pending,"remote":remote})).unwrap_or_default();
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
    let action = p["action"].as_str().unwrap_or("");
    if matches!(action, "draft_checkout_commit" | "draft_push_summary") {
        let checkout = string(p, "checkoutId")?;
        let remote = p["remote"].as_str().unwrap_or("");
        let branch = p["branch"].as_str().unwrap_or("");
        if action == "draft_push_summary" && (!super::git::valid_branch(branch) || remote.is_empty() || remote.len() > 4096) {
            return Err(refuse("invalid_request", "Choose the remote and branch to summarize."));
        }
        let mut git = Git::for_service(config);
        let path = git.resolve(root, project, checkout).await?;
        let key = integration_key(action, branch, &[checkout.to_owned()], remote);
        let (id, fresh) = begin(project, "", &key);
        if fresh {
            let (state, action, remote, branch) = (
                state.to_path_buf(), action.to_owned(), remote.to_owned(), branch.to_owned(),
            );
            spawn(id.clone(), move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
                if action == "draft_checkout_commit" {
                    let read = runtime.map_err(Refused::from).and_then(|rt| {
                        rt.block_on(changes(&path, "checkout_commit", "", &[], &mut git))
                    });
                    if read.as_ref().is_ok_and(|c| c.files.is_empty() && c.untracked.is_empty()) {
                        log(&state, "short_text_fallback", json!({"purpose":"commit","outcome":"no_changes"}));
                        return json!({"state":"done","source":"fallback","reason":"no_changes","text":{},"route":null});
                    }
                    integration_job(&state, "commit", read)
                } else {
                    let read = runtime.map_err(Refused::from).and_then(|rt| {
                        rt.block_on(outgoing(&mut git, &path, &remote, &branch))
                    });
                    push_job(&state, read)
                }
            })?;
        }
        return Ok(json!({"jobId":id}));
    }
    let task = if action == "short_text_poll" && p["taskId"].is_null() {
        String::new()
    } else {
        task_id(p)?.to_owned()
    };
    match action {
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
            super::task_actions::require_consistent_records(root, project, &task)?;
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
                    let owner = || {
                        super::task_actions::require_consistent_records(
                            &root,
                            &project,
                            &task_for_job,
                        )
                        .is_err()
                            || owner_titled(&root, &project, &task_for_job).unwrap_or(true)
                    };
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
            super::task_actions::require_consistent_records(root, project, &task)?;
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
            let (id, fresh) = begin(project, &task, &integration_key(&method, &target, &pending, ""));
            if fresh {
                let (root, project, state, config) = (
                    root.to_owned(),
                    project.to_owned(),
                    state.to_owned(),
                    config.clone(),
                );
                let task_for_job = task.clone();
                spawn(id.clone(), move || {
                    let mut saved_body = None;
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
                            if method == "pr" {
                                saved_body = git.saved_push_summary(&path, &state, &project).await.ok().flatten();
                            }
                            changes(&path, &method, &target, &pending, &mut git).await
                        })
                    })();
                    if let Some(body) = saved_body {
                        log(&state, "short_text_saved_push", json!({"purpose":"pull_request"}));
                        return saved_pr(read.as_ref().ok(), body);
                    }
                    integration_job(&state, &method, read)
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

//! Python `acp.config_options`/`catalog.labels` presentation of the coordinator
//! `provider`, `model` and `thought_level` options for an incoming ACP client.
//! Display only: dispatch still resolves through `RouteResolver` eligibility.
use super::{is_concrete_effort, Candidate, RouteResolver};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, path::Path};

const UNAVAILABLE: &str = "Saved route is unavailable — choose a route";
const UNAVAILABLE_THOUGHT: &str = "Depends on an unavailable route — choose a route";
const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
const SEPARATOR: &str = " · ";

/// A selectable route as a person chooses it (Python `presented_entries`).
#[derive(Clone)]
pub(super) struct Entry {
    candidate: Candidate,
    name: String,
}
impl Entry {
    fn route(&self) -> &str {
        self.candidate.route_id()
    }
    fn model(&self) -> &str {
        self.candidate.model()
    }
    fn connection(&self) -> &str {
        self.candidate.connection_id()
    }
    fn engine(&self) -> &str {
        self.candidate.engine()
    }
    fn description(&self) -> Option<&str> {
        self.candidate.entry["model_description"]
            .as_str()
            .filter(|s| !s.is_empty())
    }
}
/// Python `_current_selection_context` for one session.
#[derive(Default)]
pub(super) struct Current {
    route: Option<String>,
    state: Option<&'static str>,
    revision: Option<i64>,
    parameters: Map<String, Value>,
    resolution_error: Option<&'static str>,
    coordinator: Value,
}
pub(super) struct View {
    pub options: Vec<Value>,
    entries: Vec<Entry>,
    current: Current,
}

pub(super) fn engine_short_name(engine: &str) -> &str {
    match engine {
        "claude-agent-acp" => "claude",
        "codex-acp" => "codex",
        other => other,
    }
}
fn category(c: char) -> icu_properties::props::GeneralCategory {
    icu_properties::CodePointMapData::<icu_properties::props::GeneralCategory>::new().get(c)
}
/// Invisible format characters (Unicode `Cf`) that Python's label sanitizer drops.
fn format_character(c: char) -> bool {
    category(c) == icu_properties::props::GeneralCategory::Format
}
/// Python `unicodedata.category(c)[0] == "M"`.
fn combining_mark(c: char) -> bool {
    icu_properties::props::GeneralCategoryGroup::Mark.contains(category(c))
}
/// Python `_sanitize_component`: NFKC first (`_normalize`), then controls,
/// `Cf` and stray combining marks are dropped and whitespace is collapsed.
fn sanitize(value: &str) -> Option<String> {
    let normalized = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc().normalize(value);
    let mut kept = String::new();
    let mut previous: Option<char> = None;
    for c in normalized.chars() {
        let code = c as u32;
        if code <= 0x1F || (0x7F..=0x9F).contains(&code) || format_character(c) {
            continue;
        }
        if combining_mark(c) && previous.is_none_or(|p| p.is_ascii()) {
            continue;
        }
        kept.push(c);
        previous = Some(c);
    }
    let cleaned = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return None;
    }
    Some(truncate(&cleaned, 64))
}
fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let head: String = value.chars().take(limit - 1).collect();
    format!("{}…", head.trim_end())
}
/// Python `model_display_name`: `claude-opus-5-5[1m]` -> `Opus 5.5 (1M)`.
fn model_display_name(model: &str) -> String {
    let Some(m) = version_captures(model) else {
        return model.to_owned();
    };
    let (family, version, context) = m;
    let mut name = family[..1].to_uppercase() + &family[1..];
    name = format!("{name} {}", version.replace('-', "."));
    match context {
        Some(context) => format!("{name} ({})", context.to_uppercase()),
        None => name,
    }
}
fn version_captures(model: &str) -> Option<(String, String, Option<String>)> {
    let re = super::grammar::re(
        r"^(?:(?:claude-)?([a-z]+)-(\d+(?:-\d{1,2})*)(?:-\d{8})?(?:\[([a-z0-9]+)\])?)$",
    );
    let c = re.captures(model)?;
    Some((
        c[1].to_owned(),
        c[2].to_owned(),
        c.get(3).map(|m| m.as_str().to_owned()),
    ))
}
/// Python `resolved_default_model`.
fn resolved_default_model(
    model: &str,
    description: Option<&str>,
    listed: &[&str],
) -> Option<String> {
    let description = description?;
    if model.to_lowercase() != "default"
        || !super::grammar::re(r"^(?:[a-z][a-z0-9.]*(?:-[a-z0-9.]+)+(?:\[[a-z0-9]+\])?)$")
            .is_match(description)
        || listed.contains(&description)
    {
        return None;
    }
    Some(description.to_owned())
}
/// Python `disambiguate_labels`: only colliding labels get a route-id suffix.
fn disambiguate(pairs: &[(String, String)]) -> HashMap<String, String> {
    let mut counts = HashMap::new();
    for (_, label) in pairs {
        *counts.entry(label.as_str()).or_insert(0) += 1;
    }
    pairs
        .iter()
        .map(|(id, label)| {
            let value = if counts[label.as_str()] > 1 {
                let suffix: String = if id.starts_with("xr_") {
                    id.chars().skip(3).take(8).collect()
                } else {
                    id.chars().take(8).collect()
                };
                format!("{label} (#{suffix})")
            } else {
                label.clone()
            };
            (id.clone(), value)
        })
        .collect()
}
fn state_suffix(state: Option<&str>) -> Option<&'static str> {
    match state {
        Some("selected" | "pending_next_use") => Some("(pending — applies to the next run)"),
        Some("active") => Some("(active)"),
        _ => None,
    }
}
fn unavailable_state(state: Option<&str>) -> bool {
    matches!(state, Some("default_unavailable" | "selection_unavailable"))
}
fn decorate(
    option: &mut Value,
    description: Option<&str>,
    state: Option<&str>,
    revision: Option<i64>,
) {
    if let Some(description) = description {
        option["description"] = json!(description);
    }
    if let Some(state) = state {
        option["_meta"] = json!({"_cortex":{"state":state,"revision":revision}});
    }
}
fn effective_state<'a>(option: &'a Value, fallback: Option<&'a str>) -> Option<&'a str> {
    option["_meta"]["_cortex"]["state"].as_str().or(fallback)
}
fn lost_state(state: Option<&str>) -> &'static str {
    if state == Some("default") {
        "default_unavailable"
    } else {
        "selection_unavailable"
    }
}

/// Python `connections.account_identity.resolve_account_identity`: an identity
/// claim only, read from the connection's own credential directory. Every
/// failure is `None`; no credential byte leaves this function.
fn account_identity(connection: &Value) -> Option<String> {
    let directory = connection["credential_ref"]["path"].as_str()?;
    if connection["credential_ref"]["kind"] != "engine_native_dir" {
        return None;
    }
    match connection["engine"].as_str()? {
        "codex-acp" => codex_account(Path::new(directory)),
        "claude-agent-acp" => claude_account(directory),
        _ => None,
    }
}
fn codex_account(directory: &Path) -> Option<String> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join("auth.json"))
        .ok()?;
    let info = file.metadata().ok()?;
    if !info.is_file()
        || info.nlink() > 1
        || info.uid() != unsafe { libc::getuid() }
        || info.mode() & 0o077 != 0
    {
        return None;
    }
    let mut raw = Vec::new();
    file.take(super::MAX_JSON + 1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64 > super::MAX_JSON {
        return None;
    }
    let auth: Value = serde_json::from_slice(&raw).ok()?;
    let token = auth["tokens"]["id_token"].as_str()?;
    if token.matches('.').count() != 2 {
        return None;
    }
    use base64::Engine;
    let segment = token.split('.').nth(1)?.trim_end_matches('=');
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .ok()?;
    let claims: Value = serde_json::from_slice(&claims).ok()?;
    claims["email"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
/// `claude auth status` from PATH, under this connection's own
/// `CLAUDE_CONFIG_DIR` and absolute PATH entries only, bounded to five seconds.
fn claude_account(directory: &str) -> Option<String> {
    use std::{os::unix::fs::PermissionsExt, process::Stdio};
    let path = std::env::var("PATH").ok().filter(|p| !p.is_empty())?;
    let filtered: Vec<_> = path
        .split(':')
        .filter(|e| !e.is_empty() && e.starts_with('/'))
        .collect();
    // Unlike Python's `shutil.which`, a relative or empty (cwd) entry is never searched.
    let binary = filtered
        .iter()
        .map(|d| Path::new(d).join("claude"))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })?;
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["auth", "status"])
        .env_clear()
        .env("PATH", filtered.join(":"))
        .env("CLAUDE_CONFIG_DIR", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Route presentation runs in the host's spawn_blocking lane. Keep pipe I/O
    // on its async reactor so an inherited stdout never needs a blocking join.
    let out = tokio::runtime::Handle::try_current()
        .ok()?
        .block_on(account_status_output(
            command,
            std::time::Duration::from_secs(5),
        ))?;
    let payload: Value = serde_json::from_slice(&out).ok()?;
    payload["email"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
async fn account_status_output(
    mut command: tokio::process::Command,
    budget: std::time::Duration,
) -> Option<Vec<u8>> {
    use tokio::{
        io::AsyncReadExt,
        time::{timeout_at, Instant},
    };
    const MAX_OUTPUT: u64 = 1024 * 1024;
    let deadline = Instant::now() + budget;
    let mut child = command.process_group(0).kill_on_drop(true).spawn().ok()?;
    let group = child.id()? as i32;
    let result = timeout_at(deadline, async {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("account_status_stdout_missing"))?;
        let mut out = Vec::new();
        let read = async {
            stdout.take(MAX_OUTPUT + 1).read_to_end(&mut out).await?;
            if out.len() as u64 > MAX_OUTPUT {
                return Err(std::io::Error::other("account_status_output_too_large"));
            }
            Ok(())
        };
        let (status, ()) = tokio::try_join!(child.wait(), read)?;
        Ok::<_, std::io::Error>(status.success().then_some(out))
    })
    .await;
    // Kill the dedicated group even if its leader exited while a descendant
    // retained stdout. Cleanup shares the deadline; no blocking wait or join.
    // Tokio's kill-on-drop/reaper owns a child not yet waitable at that deadline.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    let _ = child.start_kill();
    let _ = timeout_at(deadline, child.wait()).await;
    result.ok()?.ok()?
}

fn provider_label(entry: &Entry) -> String {
    let engine = sanitize(engine_short_name(entry.engine())).unwrap_or_default();
    match account_identity(&entry.candidate.connection).and_then(|a| sanitize(&a)) {
        Some(account) => truncate(&format!("{engine}{SEPARATOR}{account}"), 200),
        None => engine,
    }
}

fn provider_option(
    entries: &[Entry],
    current: Option<&str>,
    state: Option<&str>,
    revision: Option<i64>,
) -> Option<Value> {
    let mut firsts: Vec<&Entry> = Vec::new();
    for entry in entries {
        if !firsts.iter().any(|e| e.connection() == entry.connection()) {
            firsts.push(entry);
        }
    }
    let first = firsts.first()?;
    let pairs: Vec<_> = firsts
        .iter()
        .map(|e| (e.connection().to_owned(), provider_label(e)))
        .collect();
    let labels = disambiguate(&pairs);
    let options: Vec<_> = pairs
        .iter()
        .map(|(id, _)| json!({"value":id,"name":labels[id]}))
        .collect();
    let lost = current.is_some_and(|c| !firsts.iter().any(|e| e.connection() == c));
    let (value, state, description) = if lost {
        (
            first.connection(),
            Some(lost_state(state)),
            Some(UNAVAILABLE),
        )
    } else {
        (
            current.unwrap_or(first.connection()),
            state,
            state_suffix(state),
        )
    };
    let mut option = json!({"id":"provider","name":"Provider","type":"select","category":"provider",
        "currentValue":value,"options":options});
    decorate(&mut option, description, state, revision);
    Some(option)
}
fn model_option(
    entries: &[&Entry],
    current: Option<&str>,
    state: Option<&str>,
    revision: Option<i64>,
) -> Option<Value> {
    let mut firsts: Vec<&Entry> = Vec::new();
    for entry in entries {
        if !firsts.iter().any(|e| e.model() == entry.model()) {
            firsts.push(entry);
        }
    }
    let first = firsts.first()?;
    let pairs: Vec<_> = firsts
        .iter()
        .map(|e| {
            (
                e.model().to_owned(),
                sanitize(&e.name).unwrap_or_else(|| e.model().into()),
            )
        })
        .collect();
    let labels = disambiguate(&pairs);
    let options: Vec<_> = firsts
        .iter()
        .map(|e| json!({"value":e.model(),"name":labels[e.model()],"description":engine_short_name(e.engine())}))
        .collect();
    let lost =
        unavailable_state(state) || current.is_some_and(|c| !firsts.iter().any(|e| e.model() == c));
    let (value, state, description) = if lost {
        let state = if unavailable_state(state) {
            state.unwrap()
        } else {
            lost_state(state)
        };
        (first.model(), Some(state), Some(UNAVAILABLE))
    } else {
        (current.unwrap_or(first.model()), state, state_suffix(state))
    };
    let mut option = json!({"id":"model","name":"Model","type":"select","category":"model",
        "currentValue":value,"options":options});
    decorate(&mut option, description, state, revision);
    Some(option)
}
/// Python `combined_model_values`: a model id when it names exactly one route.
pub(super) fn combined_values(entries: &[Entry]) -> HashMap<String, String> {
    let model = |e: &Entry| -> String {
        match e.description() {
            Some(d) if e.model().to_lowercase() == "default" => d.to_owned(),
            _ => e.model().to_owned(),
        }
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in entries {
        *counts.entry(model(e)).or_default() += 1;
    }
    entries
        .iter()
        .map(|e| {
            let m = model(e);
            let unique = counts[&m] == 1 && !entries.iter().any(|x| x.route() == m);
            (
                e.route().to_owned(),
                if unique { m } else { e.route().to_owned() },
            )
        })
        .collect()
}
fn combined_model_option(
    entries: &[Entry],
    current: Option<&str>,
    fallback: &str,
    state: Option<&str>,
    revision: Option<i64>,
) -> Option<Value> {
    let first = entries.first()?;
    let values = combined_values(entries);
    let pairs: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.route().to_owned(),
                sanitize(&e.name).unwrap_or_else(|| e.model().into()),
            )
        })
        .collect();
    let labels = disambiguate(&pairs);
    let options: Vec<_> = entries
        .iter()
        .map(|e| json!({"value":values[e.route()],"name":labels[e.route()],"description":engine_short_name(e.engine())}))
        .collect();
    let preferred = entries
        .iter()
        .find(|e| e.connection() == fallback)
        .map(|e| values[e.route()].clone())
        .unwrap_or_else(|| values[first.route()].clone());
    let lost = current.is_some_and(|c| !entries.iter().any(|e| e.route() == c));
    let (value, state, description) = if lost {
        (preferred, Some(lost_state(state)), Some(UNAVAILABLE))
    } else {
        let value = current.map(|c| values[c].clone()).unwrap_or(preferred);
        (value, state, state_suffix(state))
    };
    let mut option = json!({"id":"model","name":"Model","type":"select","category":"model",
        "currentValue":value,"options":options});
    decorate(&mut option, description, state, revision);
    Some(option)
}
fn effort_rank(value: &str) -> (usize, String) {
    let folded = value.to_lowercase();
    (
        EFFORTS
            .iter()
            .position(|e| *e == folded)
            .unwrap_or(EFFORTS.len()),
        folded,
    )
}
/// Choose the nearest advertised effort; the ordered list favors lower effort on ties.
pub(super) fn nearest_effort<'a>(values: &'a [String], preferred: Option<&str>) -> Option<&'a str> {
    let preferred = preferred.filter(|v| is_concrete_effort(v)).unwrap_or("medium");
    if let Some(exact) = values.iter().find(|v| *v == preferred) {
        return Some(exact);
    }
    let rank = effort_rank(preferred).0;
    let target = if rank == EFFORTS.len() {
        effort_rank("medium").0
    } else {
        rank
    };
    values
        .iter()
        .min_by_key(|v| effort_rank(v).0.abs_diff(target))
        .map(String::as_str)
}

/// The concrete efforts a route offers, in strength order.
pub(super) fn efforts(candidate: &Candidate) -> Vec<String> {
    let pairs: Vec<(String, String)> =
        serde_json::from_value(candidate.entry["route"]["supported_parameters"].clone())
            .unwrap_or_default();
    let mut values: Vec<String> = Vec::new();
    for (name, value) in pairs {
        if name == "thought_level" && is_concrete_effort(&value) && !values.contains(&value) {
            values.push(value);
        }
    }
    values.sort_by_key(|v| effort_rank(v));
    values
}
fn thought_option(
    entry: Option<&Entry>,
    parameters: &Map<String, Value>,
    state: Option<&str>,
    revision: Option<i64>,
) -> Option<Value> {
    let values = efforts(&entry?.candidate);
    let saved = parameters.get("thought_level").and_then(Value::as_str);
    let (current, description) = if unavailable_state(state) {
        (nearest_effort(&values, None)?, Some(UNAVAILABLE_THOUGHT))
    } else {
        (nearest_effort(&values, saved)?, state_suffix(state))
    };
    let options: Vec<_> = values.iter().map(|v| json!({"value":v,"name":v})).collect();
    let mut option = json!({"id":"thought_level","name":"Thought level","type":"select",
        "category":"thought_level","currentValue":current,"options":options});
    decorate(&mut option, description, state, revision);
    Some(option)
}
/// Python `build_config_options` without the language/access options.
fn build(entries: &[Entry], current: &Current, combined: bool) -> Vec<Value> {
    let mut options = Vec::new();
    let Some(first) = entries.first() else {
        return options;
    };
    let current_entry = current
        .route
        .as_deref()
        .and_then(|r| entries.iter().find(|e| e.route() == r));
    let current_connection = current_entry
        .map(Entry::connection)
        .or(current.route.as_deref());
    let (provider_state, chosen) = if combined {
        (
            current.state.map(str::to_owned),
            current_entry.unwrap_or(first).connection().to_owned(),
        )
    } else {
        let Some(provider) =
            provider_option(entries, current_connection, current.state, current.revision)
        else {
            return options;
        };
        let state = effective_state(&provider, current.state).map(str::to_owned);
        let chosen = provider["currentValue"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        options.push(provider);
        (state, chosen)
    };
    let provider_entries: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.connection() == chosen)
        .collect();
    let model = if combined {
        combined_model_option(
            entries,
            current.route.as_deref(),
            &chosen,
            provider_state.as_deref(),
            current.revision,
        )
    } else {
        model_option(
            &provider_entries,
            current_entry.map(Entry::model),
            provider_state.as_deref(),
            current.revision,
        )
    };
    if let Some(model) = model {
        let chosen_model = model["currentValue"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let chosen_entry = if combined {
            let values = combined_values(entries);
            entries.iter().find(|e| values[e.route()] == chosen_model)
        } else {
            provider_entries
                .iter()
                .copied()
                .find(|e| e.model() == chosen_model)
        };
        let state = effective_state(&model, provider_state.as_deref()).map(str::to_owned);
        options.push(model);
        if let Some(thought) = thought_option(
            chosen_entry,
            &current.parameters,
            state.as_deref(),
            current.revision,
        ) {
            options.push(thought);
        }
    }
    if let Some(diagnostic) = current.resolution_error {
        for option in &mut options {
            option["description"] = json!(UNAVAILABLE);
            option["_meta"] = json!({"_cortex":{"state":"default_unavailable","revision":current.revision,
                "diagnostic":diagnostic,"configured_coordinator":current.coordinator}});
        }
    }
    options
}
fn falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}
fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

impl RouteResolver {
    /// Enabled, non-denied and launchable owner routes in registry/snapshot order.
    fn entries(&self, root: &Path) -> Vec<Entry> {
        let selectable: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| {
                self.allowed(c, false)
                    && self
                        .selected(c, json!({}), "incoming_picker")
                        .is_ok_and(|s| s.launch(root).is_ok())
            })
            .collect();
        let mut presented = Vec::new();
        for c in &selectable {
            let listed: Vec<&str> = selectable
                .iter()
                .filter(|x| x.connection_id() == c.connection_id())
                .map(|x| x.model())
                .collect();
            let model_name = c.entry["model_name"].as_str().filter(|s| !s.is_empty());
            let name = if c.model().to_lowercase() == "default" {
                let description = c.entry["model_description"]
                    .as_str()
                    .filter(|s| !s.is_empty());
                match resolved_default_model(c.model(), description, &listed) {
                    Some(resolved) => model_display_name(&resolved),
                    None => continue,
                }
            } else {
                match model_name {
                    Some(name) if name != c.model() => name.to_owned(),
                    _ => model_display_name(c.model()),
                }
            };
            presented.push(Entry {
                candidate: (*c).clone(),
                name,
            });
        }
        presented
    }
    /// Python `_current_selection_context` plus `_config_options_for_session`'s
    /// saved-identity fallback. `settings.state` marks a session-local pre-task
    /// choice; `settings.task` a task-bound session (no revision history here).
    fn current(&self, entries: &[Entry], settings: &Value) -> Current {
        let effort_parameters = |effort: Option<&str>| -> Map<String, Value> {
            effort
                .map(|e| Map::from_iter([("thought_level".to_owned(), json!(e))]))
                .unwrap_or_default()
        };
        if settings["legacy"] == true {
            // A Python-created session: its task's latest live revision.
            return Current {
                route: text(&settings["model"]).map(str::to_owned),
                state: match settings["state"].as_str() {
                    Some("selected") => Some("selected"),
                    Some("pending_next_use") => Some("pending_next_use"),
                    Some("active") => Some("active"),
                    _ => None,
                },
                revision: settings["revision"].as_i64(),
                parameters: settings["parameters"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                ..Current::default()
            };
        }
        if settings["task"] == true {
            return match self.task_coordinator(settings) {
                Ok(s) => Current {
                    route: Some(s.route_id().to_owned()),
                    parameters: s
                        .parameters
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(v)))
                        .collect(),
                    ..Current::default()
                },
                Err(_) => Current::default(),
            };
        }
        if settings["state"] == "pending_next_use" {
            return Current {
                route: text(&settings["model"]).map(str::to_owned),
                state: Some("pending_next_use"),
                parameters: effort_parameters(
                    text(&settings["effort"])
                        .filter(|v| is_concrete_effort(v))
                        .or_else(|| self.settings_effort()),
                ),
                ..Current::default()
            };
        }
        if settings["loaded"] == true {
            // Python's session/load keeps no default selection for a task-less session.
            return Current::default();
        }
        let d = &self.defaults["defaults"];
        if !d["coordinator"].is_null() && falsy(&d["route_id"]) {
            let mut coordinator = d["coordinator"].clone();
            if let Some(parameters) = d.get("parameters") {
                coordinator["parameters"] = parameters.clone();
            }
            let parameters = coordinator["parameters"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            return match self.coordinator() {
                Ok(s) => Current {
                    route: Some(s.route_id().to_owned()),
                    state: Some("default"),
                    parameters,
                    ..Current::default()
                },
                Err(e) => Current {
                    state: Some("default"),
                    parameters,
                    resolution_error: Some(match e {
                        CoreError::Refused(code) => code,
                        _ => "portable_coordinator_unavailable",
                    }),
                    coordinator,
                    ..Current::default()
                },
            };
        }
        let Some(route) = text(&d["route_id"]) else {
            return Current::default();
        };
        let mut route = route.to_owned();
        if !entries.iter().any(|e| e.route() == route) {
            if let Some(e) = entries.iter().find(|e| {
                text(&d["route_connection_id"]) == Some(e.connection())
                    && text(&d["route_model"]) == Some(e.model())
            }) {
                route = e.route().to_owned();
            }
        }
        Current {
            route: Some(route),
            state: Some("default"),
            parameters: d["parameters"].as_object().cloned().unwrap_or_default(),
            ..Current::default()
        }
    }
    /// Selectable routes for `/cortex-model` resolution (Python
    /// `resolve_model_argument` reads ids, labels and model ids).
    pub(super) fn listing(&self, root: &Path) -> Vec<Value> {
        self.entries(root)
            .iter()
            .map(|e| {
                json!({"route":e.route(),"model":e.model(),"label":e.candidate.entry["label"],
                    "disambiguated_label":e.candidate.entry["disambiguated_label"]})
            })
            .collect()
    }
    pub(super) fn view(&self, root: &Path, settings: &Value) -> View {
        let entries = self.entries(root);
        let current = self.current(&entries, settings);
        View {
            options: build(&entries, &current, settings["combined"] == true),
            entries,
            current,
        }
    }
    /// Python `_apply_config_selection`'s target route for a provider, model or
    /// thought-level choice, plus its pre-task echo rule (`echo`: the saved or
    /// pending route is selected again, which changes nothing).
    pub(super) fn select(
        &self,
        root: &Path,
        settings: &Value,
        id: &str,
        value: &str,
    ) -> CoreResult<Value> {
        let view = self.view(root, settings);
        let combined = settings["combined"] == true;
        // Python `_apply_pre_task_selection` first resolves a saved portable
        // coordinator default; when it cannot bind, the route choice itself
        // is refused with that resolution error.
        if matches!(id, "provider" | "model" | "route") && view.current.state == Some("default") {
            let d = &self.defaults["defaults"];
            if !d["coordinator"].is_null() && falsy(&d["route_id"]) {
                if let Err(e) = self.coordinator() {
                    return Err(CoreError::Refused(match e.code() {
                        "portable_coordinator_ambiguous" => {
                            "portable_selection:portable_coordinator_ambiguous"
                        }
                        "unsupported_dependent_setting" => {
                            "portable_selection:unsupported_dependent_setting"
                        }
                        _ => "portable_selection:portable_coordinator_unavailable",
                    }));
                }
            }
        }
        let current_of = |key: &str| {
            view.options
                .iter()
                .find(|o| o["id"] == key)
                .and_then(|o| o["currentValue"].as_str())
                .map(str::to_owned)
        };
        let provider = current_of("provider");
        let model = current_of("model");
        let values = combined_values(&view.entries);
        let model_name = if combined {
            model
                .as_deref()
                .and_then(|m| view.entries.iter().find(|e| values[e.route()] == m))
                .map(|e| e.model().to_owned())
        } else {
            model.clone()
        };
        let matches = |e: &&Entry, wanted: Option<&str>| {
            if combined {
                Some(values[e.route()].as_str()) == wanted
            } else {
                Some(e.connection()) == provider.as_deref() && Some(e.model()) == wanted
            }
        };
        let target = match id {
            "provider" => {
                let own: Vec<&Entry> = view
                    .entries
                    .iter()
                    .filter(|e| e.connection() == value)
                    .collect();
                own.iter()
                    .copied()
                    .find(|e| Some(e.model()) == model_name.as_deref())
                    .or(own.first().copied())
            }
            "model" => view.entries.iter().find(|e| matches(e, Some(value))),
            // `/cortex-model`: a whole route by id, across every provider.
            "route" => view.entries.iter().find(|e| e.route() == value),
            _ => view.entries.iter().find(|e| matches(e, model.as_deref())),
        }
        .ok_or(CoreError::Refused("selected_route_unavailable"))?;
        let echo = matches!(id, "provider" | "model" | "route")
            && matches!(view.current.state, Some("default" | "pending_next_use"))
            && view
                .current
                .route
                .as_deref()
                .is_some_and(|r| r == target.route());
        Ok(
            json!({"route":target.route(),"model":target.model(),"connection":target.connection(),"echo":echo}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(connection: &str, route: &str, model: &str, efforts: &[&str]) -> Entry {
        let pairs: Vec<_> = efforts
            .iter()
            .map(|e| json!(["thought_level", e]))
            .collect();
        Entry {
            candidate: Candidate {
                connection: json!({"id":connection,"engine":"claude-agent-acp"}),
                entry: json!({"route":{"id":route,"engine":"claude-agent-acp","model":model,
                    "supported_parameters":pairs}}),
            },
            name: model.into(),
        }
    }
    #[test]
    fn labels_follow_oracle_catalog_labels() {
        assert_eq!(model_display_name("claude-opus-5-5[1m]"), "Opus 5.5 (1M)");
        assert_eq!(model_display_name("opus[1m]"), "opus[1m]");
        assert_eq!(
            resolved_default_model("default", Some("claude-x-1"), &["m"]).as_deref(),
            Some("claude-x-1")
        );
        assert_eq!(
            resolved_default_model("default", Some("m-1"), &["m-1"]),
            None
        );
        assert_eq!(sanitize(" a\u{200b}b \t c ").as_deref(), Some("ab c"));
        assert_eq!(sanitize("\u{0301}"), None);
        // NFKC first: compatibility forms fold and a mark composes onto its base.
        assert_eq!(
            sanitize("\u{ff26}\u{2160}\u{00b2} l\u{0301}").as_deref(),
            Some("FI2 \u{013a}")
        );
        assert_eq!(sanitize(&"x".repeat(70)).unwrap().chars().count(), 64);
        let labels = disambiguate(&[
            ("xr_0123456789ab".into(), "M".into()),
            ("cn_abcdefabcdef".into(), "M".into()),
            ("xr_ffff".into(), "N".into()),
        ]);
        assert_eq!(labels["xr_0123456789ab"], "M (#01234567)");
        assert_eq!(labels["cn_abcdefabcdef"], "M (#cn_abcde)");
        assert_eq!(labels["xr_ffff"], "N");
    }
    #[test]
    fn efforts_values_and_cross_account_ids_follow_oracle() {
        let large = entry(
            "cn_a",
            "xr_a1",
            "large",
            &["high", "default", "engine_default", "Low", "zeta", "max"],
        );
        let options = efforts(&large.candidate);
        assert_eq!(options, ["Low", "high", "max", "zeta"]);
        assert_eq!(nearest_effort(&options, Some("default")), Some("Low"));
        assert_eq!(nearest_effort(&options, Some("high")), Some("high"));
        let entries = vec![
            large,
            entry("cn_b", "xr_b1", "large", &[]),
            entry("cn_b", "xr_b2", "small", &[]),
        ];
        let values = combined_values(&entries);
        assert_eq!(values["xr_a1"], "xr_a1");
        assert_eq!(values["xr_b1"], "xr_b1");
        assert_eq!(values["xr_b2"], "small");
        let current = Current {
            route: Some("xr_b2".into()),
            state: Some("pending_next_use"),
            ..Current::default()
        };
        let options = build(&entries, &current, false);
        assert_eq!(options[0]["currentValue"], "cn_b");
        assert_eq!(options[1]["currentValue"], "small");
        assert_eq!(
            options.len(),
            2,
            "a route without efforts has no thought level"
        );
        assert_eq!(options[1]["_meta"]["_cortex"]["state"], "pending_next_use");
        let lost = Current {
            route: Some("xr_gone".into()),
            state: Some("default"),
            ..Current::default()
        };
        let options = build(&entries, &lost, true);
        assert_eq!(
            options[0]["_meta"]["_cortex"]["state"],
            "default_unavailable"
        );
        assert_eq!(options[0]["currentValue"], "xr_a1");
        assert_eq!(options[1]["currentValue"], "Low");
    }
}

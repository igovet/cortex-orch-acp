//! In-process machine operations. Core validates all authoritative project-store access.
use super::{context, files, git, product as private_product, settings};
use serde_json::{json, Value};
use std::{
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const RESERVED: &[&str] = &[
    ".git",
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
    ".cortex-workspace.json",
];
pub const FRAME_BUDGET: usize = workspace_protocol::FRAME_BYTES - 65536;
#[derive(Debug)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
    /// Structured refusal details. The machine envelope publishes only the
    /// explicitly allowed holderTaskId for a Code lease refusal.
    pub detail: Option<Box<Value>>,
}
impl Refused {
    /// The same refusal with management-API details merged into its detail.
    pub fn with(mut self, detail: Value) -> Self {
        let mut merged = self.detail.map_or_else(|| json!({}), |d| *d);
        for (key, value) in detail.as_object().into_iter().flatten() {
            merged[key] = value.clone();
        }
        self.detail = Some(Box::new(merged));
        self
    }
}
pub type Result<T> = std::result::Result<T, Refused>;
impl From<io::Error> for Refused {
    fn from(error: io::Error) -> Self {
        // A folder the owner can fix is named with what is wrong, not hidden behind a generic failure.
        if let Some(folder) = crate::local::UnsafePrivateDirectory::of(&error) {
            return refuse("private_directory_unsafe", folder.owner_message());
        }
        refuse("machine_tool_failed", "Filesystem operation failed.")
    }
}
pub fn refuse(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
        detail: None,
    }
}
/// Python truthiness of one JSON value.
pub fn python_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
/// Private marker for the retained helper's uncaught `AttributeError`, which it
/// raised for `params.get` on truthy non-object params (`params or {}`). It
/// printed no JSON, so the socket answers the ordinary `machine_tool_failed` RPC error.
pub const UNCAUGHT_ATTRIBUTE_ERROR: &str = "UncaughtAttributeError";
pub fn object_params(p: &Value) -> Result<()> {
    if python_truthy(p) && !p.is_object() {
        return Err(refuse("machine_tool_failed", UNCAUGHT_ATTRIBUTE_ERROR));
    }
    Ok(())
}
/// Python `repr` of a scalar the JSON parser keeps exact: `None`, booleans,
/// integers and ASCII strings. Floats (whose literal form the parser loses) and
/// non-ASCII text (whose escaping needs Unicode printability) return `None`.
pub fn python_scalar_repr(v: &Value) -> Option<String> {
    match v {
        Value::Null => Some("None".into()),
        Value::Bool(true) => Some("True".into()),
        Value::Bool(false) => Some("False".into()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        Value::String(s) if s.is_ascii() => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::from(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\t' => out.push_str("\\t"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                        out.push_str(&format!("\\x{:02x}", c as u32))
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            Some(out)
        }
        _ => None,
    }
}
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn string<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .ok_or_else(|| refuse("invalid_request", "Required text is missing."))
}
pub fn relative(file: &str) -> Result<&str> {
    if file.is_empty()
        || file.len() > 4096
        || file.starts_with('/')
        || file.contains(['\0', '\\'])
        || file
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || RESERVED.contains(&file.split('/').next().unwrap_or(""))
    {
        return Err(refuse(
            "invalid_path",
            "That path is not inside this checkout.",
        ));
    }
    Ok(file)
}
pub fn product_path(state: &Path, project: &str) -> Result<PathBuf> {
    if project.is_empty()
        || project.len() > 128
        || !project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
    {
        return Err(refuse("invalid_project", "Unknown project."));
    }
    Ok(state.join("product").join(format!("{project}.json")))
}
pub fn product(state: &Path, project: &str) -> Result<Value> {
    // Unmarked files predate Hybrid: their "automatic" meant queueing and
    // acceptance, never automatic integration. The next product write persists
    // the migrated values and marker together in the same atomic replacement.
    let mut value = json!({"tasks":{},"sprints":[],"drafts":{},"messages":[],"comments":{}});
    let mut saved = settings::read_json(&product_path(state, project)?, json!({}))?;
    // Inline bodies saved by an earlier version move to private files once.
    super::attachment_files::migrate(state, project, &mut saved)?;
    if saved.get("sprintRunModeVersion").is_none() {
        let mut migrated = false;
        for sprint in saved
            .get_mut("sprints")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if sprint["runMode"] == "automatic" {
                sprint["runMode"] = json!("hybrid");
                migrated = true;
            }
        }
        // Products that have never used the ambiguous spelling need no marker.
        if migrated {
            saved["sprintRunModeVersion"] = json!(1);
        }
    }
    if let Some(map) = saved.as_object() {
        for (k, v) in map {
            value[k] = v.clone();
        }
    }
    Ok(value)
}
/// Both queued modes share conservative result acceptance; only Automatic
/// may subsequently finish in the same branch without confirmation.
pub(crate) fn queue_driven_mode(sprint: &Value) -> bool {
    matches!(sprint["runMode"].as_str(), Some("hybrid" | "automatic"))
        && sprint["archivedAt"].is_null()
}

pub fn supported(op: &str) -> bool {
    matches!(
        op,
        "product.integrate"
            | "config.read"
            | "config.save"
            | "config.mcp.discover"
            | "project.config.read"
            | "project.config.save"
            | "cleanup.run"
            | "workers"
            | "command.output"
            | "maintenance"
            | "workflow.describe"
            | "product.board"
            | "repo.context"
            | "repo.snapshot"
            | "repo.locate"
            | "repo.list"
            | "repo.skills"
            | "git"
            | "git_init"
            | "logs.folder"
            | "machine.update"
            | "overview"
            | "diagnostics.run"
            | "product.message"
            | "product.comment"
            | "product.sprint"
            | "product.attachment"
    )
}
pub async fn run(
    op: &str,
    root: Option<&Path>,
    project: &str,
    state: &Path,
    params: &Value,
    projects: &Value,
    config: &crate::Config,
    repository_context: Option<&Value>,
    deadline: std::time::Instant,
) -> Result<Value> {
    match op {
        "config.action" => super::config_action::action(config, params, deadline),
        "engine.connection" if params["action"] == "recheck" => {
            super::engine_connection::recheck(config, params).await
        }
        "engine.connection" => super::engine_connection::action(config, params),
        "config.read" => super::config_read::read(config),
        "config.save" => super::config_save::save(config, params),
        "config.mcp.discover" => Ok(match root {
            Some(root) => super::mcp_discovery::discover_in_project(&params["server"], config, root).await,
            None => super::mcp_discovery::discover(&params["server"], config).await,
        }),
        "cleanup.run" => {
            super::retention::cleanup(state, projects, &mut git::Git::for_service(config)).await
        }
        "maintenance" => super::maintenance::action(state, params, projects).await,
        "workflow.describe" => super::workflow::describe(params),
        "product.board" => {
            let mut saved = product(state, project)?;
            if let Some(root) = root {
                if let Some(store) = context::open(root, cortex_core::OpenMode::LiveReadOnly)? {
                    super::task_rework::project(&store, project, &mut saved, true)?;
                }
            }
            Ok(
                json!({"tasks":saved["tasks"],"sprints":saved["sprints"],"messages":[],"comments":{},"drafts":{}}),
            )
        }
        "logs.folder" => Ok(json!({"folder":settings::log_folder(state)?})),
        "machine.update" => settings::update(state, params, projects).await,
        "overview" => settings::overview(state, projects),
        "diagnostics.run" => settings::diagnostics(state, projects),
        "product.attachment" => super::attachment_files::action(state, params),
        "product.message" | "product.comment" => {
            private_product::conversation(state, project, params, op == "product.comment")
        }
        "product.sprint"
            if matches!(
                params["action"].as_str(),
                Some("delete_prepare" | "delete_finish")
            ) =>
        {
            super::retention::delete_sprint(
                root.ok_or_else(|| refuse("project_required", "Unknown project."))?,
                project,
                state,
                params,
                &mut git::Git::for_service(config),
            )
            .await
        }
        // Create tasks from a plan: every task and the sprint's transition commit together.
        "product.sprint" if params["action"] == "materialize" => {
            let root = root.ok_or_else(|| refuse("project_required", "Unknown project."))?;
            // The profile's routes as they are now; unreadable settings leave
            // every task on the owner's default coordinator.
            let resolver = super::config_read::environment(config).ok().and_then(|env| {
                let key = super::context::key(root).ok()?;
                crate::native_routes::RouteResolver::load(root, &key.directory(), env).ok()
            });
            super::sprint_materialize::materialize_routed(root, project, state, params, &|c| {
                resolver.as_ref()?.sprint_route(c).ok().flatten()
            })
        }
        "product.sprint" if super::sprint_live::handles(params) => {
            let root = root.ok_or_else(|| refuse("project_required", "Unknown project."))?;
            // Revision-added tasks route by complexity like a created sprint's tasks.
            let resolver = super::config_read::environment(config).ok().and_then(|env| {
                let key = super::context::key(root).ok()?;
                crate::native_routes::RouteResolver::load(root, &key.directory(), env).ok()
            });
            super::sprint_live::owner_action(root, state, project, params, &|c| {
                resolver.as_ref()?.sprint_route(c).ok().flatten()
            })
        }
        "product.sprint" => {
            let mut reply = private_product::sprint(state, project, params, root)?;
            // A new sprint is named in the background by the cheapest light model.
            if params["action"] == "create" {
                if let (Some(root), Some(id)) = (root, reply["sprintId"].as_str().map(str::to_owned)) {
                    if let Ok(Some(job)) = super::short_text::start_sprint_title(root, project, state, config, &id) {
                        reply["titleJobId"] = json!(job);
                    }
                }
            }
            Ok(reply)
        }
        "automatic.queue" => private_product::automatic_queue(state, project, params, root),
        _ => {
            let root = root.ok_or_else(|| refuse("project_required", "Unknown project."))?;
            let git = git::Git::for_service(config);
            // An owner read waits for a slow repository instead of failing it.
            let mut git = if super::owner_read(op, params) { git.owner_read() } else { git };
            match op {
                "product.read" if params["attachments"] == true => {
                    context::attachments(root, state, project, string(params, "taskId")?)
                }
                "command.output" => {
                    use crate::owner_command_output::{self, ReadOutput};
                    let request = ReadOutput::parse(params).map_err(context::core)?;
                    let store = context::open(root, cortex_core::OpenMode::LiveReadOnly)?
                        .ok_or_else(|| refuse("command_receipt_not_found", "The command receipt is unavailable."))?;
                    owner_command_output::read(&store, project, request).map_err(context::core)
                }
                "project.config.read" => super::project_config::read(root, config),
                "project.config.save" => super::project_config_save::save(root, config, params),
                "automatic.finish" => {
                    super::integration::automatic(root, project, state, params, &mut git).await
                }
                "product.integrate" => {
                    super::integration::run(root, project, state, params, &mut git).await
                }
                // Short model texts never take machine.lock or wait for a model.
                "product.task" if super::short_text::handles(params) => {
                    super::short_text::run(root, project, state, params, config).await
                }
                // The side chat reads the task and answers off it; it never changes the task.
                "product.task" if super::side_chat::handles(params) => {
                    super::side_chat::run(root, project, state, params, config).await
                }
                "product.task" => {
                    let mut reply =
                        super::task_actions::run(root, project, state, params, &mut git).await?;
                    // An Automatic task closed as is is committed like the owner would by
                    // hand, after the close released the machine lock.
                    if params["action"] == "close"
                        && params["choice"] == "as_is"
                        && params["preflight"] != true
                        && reply["done"] == true
                    {
                        if let Some(task) = params["taskId"].as_str() {
                            if let Some(outcome) =
                                super::close_commit::commit(root, project, state, task, &mut git)
                                    .await
                            {
                                reply["asIsCommit"] = outcome;
                            }
                        }
                    }
                    Ok(reply)
                }
                "product.read" => super::task_facts::read(root, state, project, &mut git).await,
                "workers" => super::workers::read(root, project, params),
                "product.facts" => {
                    let product = super::task_facts::read(root, state, project, &mut git).await?;
                    let workers = if params["taskIds"].is_null()
                        || params["taskIds"].as_array().is_some_and(Vec::is_empty)
                    {
                        json!([])
                    } else {
                        // Bundle requests never apply the detail-only worker/cursor filters.
                        super::workers::read(root, project, &json!({"taskIds":params["taskIds"]}))?
                    };
                    Ok(json!({"product":product,"workers":workers}))
                }
                "repo.context" => context::product(root, state, project, &mut git).await,
                "repo.snapshot" => {
                    let context = match repository_context {
                        Some(value) => value.clone(),
                        None => context::product(root, state, project, &mut git).await?,
                    };
                    git.snapshot(root, project, state, Some(&context)).await
                }
                "git" | "git_init" => {
                    let p = if op == "git_init" {
                        json!({"action":"git_init"})
                    } else {
                        params.clone()
                    };
                    git.action(root, project, state, &p).await
                }
                // The recorded pull request address of a task, for the desktop's opener.
                "repo.locate" if params.get("pullRequest").is_some() => {
                    super::integration::pull_request_address(root, project, state, params, &mut git)
                        .await
                }
                "repo.locate" => {
                    let wanted = params["checkoutId"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{project}:main"));
                    let path = git.resolve(root, project, &wanted).await?;
                    files::locate(&path, params)
                }
                "repo.skills" => {
                    let wanted = params["checkoutId"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{project}:main"));
                    let path = git.resolve(root, project, &wanted).await?;
                    super::project_skills::list(&path)
                }
                "repo.list" => {
                    let wanted = params["checkoutId"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{project}:main"));
                    let path = git.resolve(root, project, &wanted).await?;
                    git::listing::list(&path, params, &mut git).await
                }
                _ => Err(refuse(
                    "unsupported_operation",
                    "Unsupported machine operation.",
                )),
            }
        }
    }
}
pub fn envelope(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(e) if e.code == "task_file_lease_held" && e.detail.is_some() => {
            json!({"ok":false,"code":e.code,"message":e.message,"holderTaskId":e.detail.unwrap()["holderTaskId"]})
        }
        Err(e) if e.code == "lsp_unavailable" && e.detail.is_some() => {
            json!({"ok":false,"code":e.code,"message":e.message,"reason":e.detail.unwrap()["reason"]})
        }
        Err(e) => json!({"ok":false,"code":e.code,"message":e.message}),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A private folder the owner can fix is reported by name, not as a generic failure.
    #[test]
    fn unsafe_private_folder_is_named_in_the_refusal() {
        let folder = PathBuf::from("/fixture/config/cortex-workspace-dev");
        let error = io::Error::new(
            io::ErrorKind::PermissionDenied,
            crate::local::UnsafePrivateDirectory {
                path: folder.clone(),
                problem: "writable by every account on this machine",
            },
        );
        let reply = envelope(Err(error.into()));
        assert_eq!(reply["code"], "private_directory_unsafe");
        let message = reply["message"].as_str().unwrap();
        assert!(
            message.contains("/fixture/config/cortex-workspace-dev"),
            "{message}"
        );
        assert!(message.contains("unsafe permissions"), "{message}");
        let other: Refused = io::Error::from(io::ErrorKind::NotFound).into();
        assert_eq!(
            (other.code, other.message.as_str()),
            ("machine_tool_failed", "Filesystem operation failed.")
        );
    }
}

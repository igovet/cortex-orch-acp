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
    /// Management-API refusal details (`field`, `errors`, `current_revision`)
    /// that the desktop protocol's flat `{code, message}` envelope never shows.
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
    let mut value = json!({"tasks":{},"sprints":[],"drafts":{},"messages":[],"comments":{}});
    let mut saved = settings::read_json(&product_path(state, project)?, json!({}))?;
    // Inline bodies saved by an earlier version move to private files once.
    super::attachment_files::migrate(state, project, &mut saved)?;
    if let Some(map) = saved.as_object() {
        for (k, v) in map {
            value[k] = v.clone();
        }
    }
    Ok(value)
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
            | "maintenance"
            | "workflow.describe"
            | "product.board"
            | "repo.context"
            | "repo.snapshot"
            | "repo.file"
            | "repo.file.save"
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
        "config.mcp.discover" => {
            Ok(super::mcp_discovery::discover(&params["server"], config).await)
        }
        "cleanup.run" => {
            super::retention::cleanup(state, projects, &mut git::Git::for_service(config)).await
        }
        "maintenance" => super::maintenance::action(state, params, projects).await,
        "workflow.describe" => super::workflow::describe(params),
        "product.board" => {
            let saved = product(state, project)?;
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
        "product.sprint" => private_product::sprint(state, project, params),
        _ => {
            let root = root.ok_or_else(|| refuse("project_required", "Unknown project."))?;
            let mut git = git::Git::for_service(config);
            match op {
                "product.read" if params["attachments"] == true => {
                    context::attachments(root, state, project, string(params, "taskId")?)
                }
                "project.config.read" => super::project_config::read(root, config),
                "project.config.save" => super::project_config_save::save(root, config, params),
                "product.integrate" => {
                    super::integration::run(root, project, state, params, &mut git).await
                }
                // Short model texts never take machine.lock or wait for a model.
                "product.task" if super::short_text::handles(params) => {
                    super::short_text::run(root, project, state, params, config).await
                }
                "product.task" => {
                    super::task_actions::run(root, project, state, params, &mut git).await
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
                "repo.file" | "repo.file.save" => {
                    context::file(
                        root,
                        state,
                        project,
                        params,
                        op == "repo.file.save",
                        &mut git,
                    )
                    .await
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

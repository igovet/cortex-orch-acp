//! `cortex-paseo-tools config`: the Paseo plugin's settings API for the Rust
//! `cortex-acp` launch. One JSON request on stdin, one JSON response on
//! stdout and exit 0 (`ok`) or 1 (refusal), the same protocol and output
//! format as `cortex_runtime.configuration.management.main`.
//!
//! Request validation (`invalid_request`, `unsupported_api_version`,
//! `unknown_surface`, `unsupported_request_field`, `unsupported_action`)
//! mirrors that module's `_dispatch` in its order. Reads are answered by
//! workspace-service's native `ConfigurationReader` producers (byte-compatible
//! ports of the Python reads); every change, refresh, discovery, onboarding,
//! engine download, preset and project-ceiling operation by its native
//! management-API implementation (`workspace_service::settings_api`), with the
//! same revision fences, owner lock, file bytes and refusal payloads. Only a
//! shape the retained Python provider alone could answer (an OS keyring
//! backend, the OpenCode namespace jail, an engine without a qualified native
//! catalog producer, numbers outside the exact native range) refuses with
//! `config_api_unavailable` and its reason, before any effect.
use crate::py::{self, Py};
use serde_json::Value;
use std::path::{Path, PathBuf};
use workspace_service::{
    config_read_model::ConfigurationReader, native_routes::RouteEnvironment, settings_api,
};

const API_VERSION: i64 = 1;
const SURFACES: [&str; 6] = [
    "user_defaults",
    "project_ceiling",
    "routing",
    "routing_profiles",
    "mcp_servers",
    "connections",
];
const ROUTE_CATALOG: &str = "route_catalog";
/// `routing_profiles` mutations and their request fields.
const PROFILE_ACTION_FIELDS: [(&str, &[&str]); 6] = [
    (
        "create",
        &[
            "expected_revision",
            "expected_revisions",
            "name",
            "description",
            "project",
        ],
    ),
    (
        "save",
        &["expected_revision", "expected_revisions", "name", "project"],
    ),
    (
        "update",
        &[
            "expected_revision",
            "name",
            "rename_to",
            "description",
            "project",
        ],
    ),
    ("delete", &["expected_revision", "name", "project"]),
    (
        "activate",
        &[
            "expected_revision",
            "expected_revisions",
            "name",
            "discard_modifications",
            "project",
        ],
    ),
    ("deactivate", &["expected_revision", "project"]),
];

/// The roots the settings API reads, taken from the tool's own environment
/// (the plugin passes the launch record's HOME/XDG values and nothing else).
pub struct Environment {
    pub home: Option<String>,
    pub config_home: Option<String>,
    pub state_home: Option<String>,
    pub routing: Option<String>,
    /// `CORTEX_ACP_RUNTIME_ROOT`: the payload holding `pinned-dependencies.json`.
    pub runtime_root: Option<String>,
}

impl Environment {
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME").filter(|v| !v.is_empty()),
            state_home: var("XDG_STATE_HOME").filter(|v| !v.is_empty()),
            routing: var("CORTEX_ROUTING").filter(|v| !v.is_empty()),
            runtime_root: var("CORTEX_ACP_RUNTIME_ROOT").filter(|v| v.starts_with('/')),
        }
    }
}

/// The channel's owner config and state roots (`<config>/cortex`,
/// `<state>/cortex`) for these environment values, or none when unusable.
pub fn owner_roots(environment: &Environment) -> Option<(PathBuf, PathBuf)> {
    let host = Api { environment }.route_environment().ok()?;
    Some((host.config.join("cortex"), host.state.join("cortex")))
}

/// `cortex-acp`'s own payload rule: the operator-selected root, else this checkout.
fn runtime_root(environment: &Environment) -> PathBuf {
    environment.runtime_root.as_deref().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    )
}

/// A native management answer in the decoded-Python form.
fn native(answer: settings_api::Answer) -> Answer {
    answer
        .map(|v| from_value(&v))
        .map_err(|e| Stop::Refused(from_value(&e)))
}

/// One request value as the native writers take it.
fn value(request: &[(String, Py)], key: &str) -> Result<Value, Stop> {
    write_value(get(request, key))
}

/// One request value as JSON text in its own key order (file bodies keep it).
fn text(request: &[(String, Py)], key: &str) -> Result<String, Stop> {
    let raw = get(request, key);
    write_value(raw)?;
    py::dumps(raw).map_err(|py::Crash| Stop::Crash)
}

/// A refusal as `error_payload` writes it.
fn error(code: &str, message: &str, field: Option<&str>) -> Py {
    Py::Dict(vec![
        ("category".into(), Py::str("validation")),
        ("code".into(), Py::str(code)),
        ("field".into(), field.map_or(Py::Null, Py::str)),
        ("message".into(), Py::str(message)),
        ("retry_eligible".into(), Py::Bool(false)),
    ])
}

/// Why `execute` answers without a result.
enum Stop {
    /// A refusal envelope with this error payload.
    Refused(Py),
    /// An exception Python would not catch: no output, exit 1.
    Crash,
}

type Answer = Result<Py, Stop>;

fn refused(code: &str, message: &str, field: Option<&str>) -> Stop {
    Stop::Refused(error(code, message, field))
}

/// The surface/action pair has no native management implementation yet.
fn unavailable(what: &str) -> Stop {
    refused(
        "config_api_unavailable",
        &format!(
            "{what} is not available from Paseo yet while Cortex runs from the Rust cortex-acp \
             entry: no native implementation exists for it. Nothing was read or changed."
        ),
        None,
    )
}

/// A native producer refusal. Its code is the producer's own; the message
/// is this tool's, because the native producers report no message text.
fn native_refusal(code: &str) -> Py {
    error(
        code,
        &format!(
            "The Cortex configuration storage refused the read ({code}); nothing was changed."
        ),
        None,
    )
}

/// `_fields`: refuse request keys this action does not accept.
fn fields(request: &[(String, Py)], allowed: &[&str]) -> Result<(), Stop> {
    let mut unknown: Vec<&str> = request
        .iter()
        .map(|(key, _)| key.as_str())
        .filter(|key| !["api_version", "action", "surface"].contains(key) && !allowed.contains(key))
        .collect();
    // Python's `sorted` over str orders by code point, as byte order does.
    unknown.sort_unstable();
    unknown.dedup();
    match unknown.first() {
        None => Ok(()),
        Some(first) => Err(refused(
            "unsupported_request_field",
            &format!(
                "Unknown request field(s) {} for this action.",
                unknown.join(", ")
            ),
            Some(first),
        )),
    }
}

fn get<'a>(request: &'a [(String, Py)], key: &str) -> &'a Py {
    request
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map_or(&py::NONE, |(_, value)| value)
}

/// `serde_json::Value` to the decoded-Python form. serde's maps are sorted,
/// which is the `sort_keys=True` order the Python API prints.
fn from_value(value: &Value) -> Py {
    match value {
        Value::Null => Py::Null,
        Value::Bool(b) => Py::Bool(*b),
        Value::Number(n) if n.is_i64() || n.is_u64() => Py::Int(n.to_string()),
        Value::Number(n) => Py::Float(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => Py::Str(s.clone()),
        Value::Array(items) => Py::List(items.iter().map(from_value).collect()),
        Value::Object(map) => Py::Dict(
            map.iter()
                .map(|(key, item)| (key.clone(), from_value(item)))
                .collect(),
        ),
    }
}

/// `sort_keys=True` for a dict this module built itself.
fn sorted(value: Py) -> Py {
    match value {
        Py::Dict(mut entries) => {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Py::Dict(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Py::List(items) => Py::List(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

struct Api<'a> {
    environment: &'a Environment,
}

impl Api<'_> {
    fn route_environment(&self) -> Result<RouteEnvironment, Py> {
        fn absolute(value: &Option<String>) -> Option<&str> {
            value.as_deref().filter(|v| v.starts_with('/'))
        }
        let Some(home) = absolute(&self.environment.home) else {
            return Err(error(
                "unsafe_storage",
                "'HOME' must be an absolute path.",
                None,
            ));
        };
        let home = PathBuf::from(home);
        for (name, value) in [
            ("XDG_CONFIG_HOME", &self.environment.config_home),
            ("XDG_STATE_HOME", &self.environment.state_home),
        ] {
            if value.is_some() && absolute(value).is_none() {
                return Err(error(
                    "unsafe_storage",
                    &format!("'{name}' must be an absolute path."),
                    None,
                ));
            }
        }
        let environment = RouteEnvironment {
            config: absolute(&self.environment.config_home)
                .map_or_else(|| home.join(".config"), PathBuf::from),
            state: absolute(&self.environment.state_home)
                .map_or_else(|| home.join(".local/state"), PathBuf::from),
            routing_override: self.environment.routing.as_deref().map(PathBuf::from),
            home,
        };
        Ok(cortex_acp::startup::channel_environment(environment))
    }

    fn reader(&self) -> Result<ConfigurationReader, Py> {
        ConfigurationReader::new(self.route_environment()?).map_err(|e| native_refusal(e.code()))
    }

    /// The native management host for this request's roots and channel.
    fn host(&self) -> Result<settings_api::Host, Stop> {
        Ok(settings_api::Host {
            environment: self.route_environment().map_err(Stop::Refused)?,
            store: cortex_acp::startup::project_store_name().to_owned(),
            runtime_root: runtime_root(self.environment),
        })
    }

    /// The trusted (logical root, channel routing file) pair, or none.
    fn project(&self, value: &Py) -> Result<Option<(PathBuf, PathBuf)>, Stop> {
        match value {
            Py::Null => Ok(None),
            // The plugin sends only resolved absolute roots; a relative or
            // empty one is refused rather than read against a working directory.
            Py::Str(root) if root.starts_with('/') => {
                let environment = self.route_environment().map_err(Stop::Refused)?;
                settings_api::view_project(
                    &environment,
                    &Value::String(root.clone()),
                    cortex_acp::startup::project_store_name(),
                )
                .map_err(|error| Stop::Refused(from_value(&error)))
            }
            Py::Str(_) => Err(refused(
                "invalid_project",
                "project must be an existing absolute project root without symlinks.",
                Some("project"),
            )),
            _ => Err(Stop::Crash),
        }
    }

    /// One surface read as its `{ok, result | error}` envelope.
    fn surface(
        &self,
        reader: &ConfigurationReader,
        surface: &str,
        project: Option<(&Path, &Path)>,
    ) -> Value {
        let produced = match surface {
            "user_defaults" => reader.user_defaults_management_observation(),
            "routing_profiles" => reader.profiles_management_observation(project),
            "mcp_servers" => reader.mcp_management_observation(),
            "routing" => reader
                .routing_management(project)
                .map(|view| serde_json::json!({"ok": true, "result": view})),
            "connections" => reader
                .connection_management()
                .map(|view| serde_json::json!({"ok": true, "result": view})),
            ROUTE_CATALOG => reader
                .catalog_selector(project)
                .map(|view| serde_json::json!({"ok": true, "result": view["routes"]})),
            _ => unreachable!("unsupported native read"),
        };
        produced.unwrap_or_else(|e| {
            let payload = native_refusal(e.code());
            serde_json::json!({"ok": false, "error": to_value(&payload)})
        })
    }

    fn read(&self, surface: &str, project: &Py) -> Answer {
        let project = self.project(project)?;
        let reader = self.reader().map_err(Stop::Refused)?;
        let pair = project.as_ref().map(|(a, b)| (a.as_path(), b.as_path()));
        let envelope = self.surface(&reader, surface, pair);
        if envelope["ok"] == true {
            Ok(from_value(&envelope["result"]))
        } else {
            Err(Stop::Refused(from_value(&envelope["error"])))
        }
    }

    fn read_all(&self, project: &Py) -> Answer {
        let pair = self.project(project)?;
        let reader = self.reader().map_err(Stop::Refused)?;
        let paths = pair.as_ref().map(|(a, b)| (a.as_path(), b.as_path()));
        let mut result: Vec<(String, Py)> = [
            "user_defaults",
            "routing",
            "routing_profiles",
            "mcp_servers",
            "connections",
        ]
        .iter()
        .map(|surface| {
            (
                surface.to_string(),
                from_value(&self.surface(&reader, surface, paths)),
            )
        })
        .collect();
        if !project.is_null() {
            let entry = match native(self.host()?.read_project_ceiling(&write_value(project)?)) {
                Ok(view) => Py::Dict(vec![("ok".into(), Py::Bool(true)), ("result".into(), view)]),
                Err(Stop::Refused(payload)) => Py::Dict(vec![
                    ("error".into(), payload),
                    ("ok".into(), Py::Bool(false)),
                ]),
                Err(Stop::Crash) => return Err(Stop::Crash),
            };
            result.push(("project_ceiling".into(), entry));
        }
        Ok(sorted(Py::Dict(result)))
    }

    /// `_dispatch`, in its order.
    fn dispatch(&self, request: &Py) -> Answer {
        let Py::Dict(request) = request else {
            return Err(refused(
                "invalid_request",
                "The request must be a JSON object.",
                None,
            ));
        };
        let request = request.as_slice();
        if let Some(version) = request.iter().rev().find(|(k, _)| k == "api_version") {
            if !py::eq(&version.1, &Py::int(API_VERSION)) {
                return Err(refused(
                    "unsupported_api_version",
                    &format!("This Cortex answers configuration API version {API_VERSION}."),
                    Some("api_version"),
                ));
            }
        }
        let action = get(request, "action");
        let surface = get(request, "surface");
        let is = |value: &Py, text: &str| py::eq_str(value, text);
        if is(action, "read") && is(surface, "all") {
            fields(request, &["project"])?;
            return self.read_all(get(request, "project"));
        }
        if is(surface, ROUTE_CATALOG) {
            if !is(action, "read") && !is(action, "refresh") {
                return Err(refused(
                    "unsupported_action",
                    "route_catalog is read-only (or 'refresh' to re-read the engines' model lists); \
                     enable or disable an engine or model with a routing update.",
                    Some("action"),
                ));
            }
            fields(request, &["project"])?;
            if is(action, "refresh") {
                let project = get(request, "project");
                self.project(project)?;
                return native(self.host()?.refresh_catalog(&write_value(project)?));
            }
            return self.read(ROUTE_CATALOG, get(request, "project"));
        }
        if is(surface, "default_preset") {
            if is(action, "read") {
                fields(request, &[])?;
                return native(self.host()?.read_default_preset());
            }
            if is(action, "apply_default_preset") {
                fields(request, &[])?;
                return native(self.host()?.apply_default_preset());
            }
            if is(action, "reset_default_preset") {
                fields(request, &["expected_revision", "confirm"])?;
                return native(self.host()?.reset_default_preset(
                    &value(request, "expected_revision")?,
                    &value(request, "confirm")?,
                ));
            }
        }
        let Some(name) = SURFACES.iter().find(|name| is(surface, name)) else {
            return Err(refused(
                "unknown_surface",
                &format!(
                    "surface must be one of {} (or 'all' or '{ROUTE_CATALOG}' for read).",
                    SURFACES.join(", ")
                ),
                Some("surface"),
            ));
        };
        match (*name, action.as_str()) {
            ("user_defaults", Some("read")) => {
                fields(request, &[])?;
                return self.read("user_defaults", &Py::Null);
            }
            ("user_defaults", Some("update")) => {
                fields(request, &["expected_revision", "defaults", "ceiling"])?;
                let environment = self.route_environment().map_err(Stop::Refused)?;
                let ceiling = request
                    .iter()
                    .find(|(k, _)| k == "ceiling")
                    .map(|(_, v)| write_value(v))
                    .transpose()?;
                return workspace_service::update_user_defaults(
                    &environment,
                    &write_value(get(request, "expected_revision"))?,
                    &write_value(get(request, "defaults"))?,
                    ceiling.as_ref(),
                )
                .map(|v| from_value(&v))
                .map_err(|e| Stop::Refused(from_value(&e)));
            }
            ("project_ceiling", Some(verb @ ("read" | "update"))) => {
                if verb == "read" {
                    fields(request, &["project"])?;
                    return native(
                        self.host()?
                            .read_project_ceiling(&value(request, "project")?),
                    );
                }
                fields(request, &["project", "expected_revision", "ceiling"])?;
                if !request.iter().any(|(k, _)| k == "ceiling") {
                    return Err(refused(
                        "missing_field",
                        "Send 'ceiling' (an object, or null to remove it).",
                        Some("ceiling"),
                    ));
                }
                return native(self.host()?.update_project_ceiling(
                    &value(request, "project")?,
                    &value(request, "expected_revision")?,
                    &value(request, "ceiling")?,
                ));
            }
            ("routing", Some("read")) => {
                fields(request, &["project"])?;
                return self.read("routing", get(request, "project"));
            }
            ("routing", Some("update")) => {
                fields(
                    request,
                    &["target", "expected_revision", "rules", "project"],
                )?;
                return native(self.host()?.update_routing(
                    &value(request, "target")?,
                    &value(request, "expected_revision")?,
                    &text(request, "rules")?,
                    &value(request, "project")?,
                ));
            }
            ("routing_profiles", _) => {
                if is(action, "read") {
                    fields(request, &["project"])?;
                    return self.read("routing_profiles", get(request, "project"));
                }
                // `action in PROFILE_ACTION_FIELDS` hashes the value.
                if matches!(action, Py::List(_) | Py::Dict(_)) {
                    return Err(Stop::Crash);
                }
                if let Some((verb, allowed)) = PROFILE_ACTION_FIELDS
                    .iter()
                    .find(|(verb, _)| is(action, verb))
                {
                    fields(request, allowed)?;
                    let mut fields = serde_json::Map::new();
                    for key in allowed.iter() {
                        if request.iter().any(|(k, _)| k == key) {
                            fields.insert(key.to_string(), value(request, key)?);
                        }
                    }
                    return native(self.host()?.profile(verb, &Value::Object(fields)));
                }
            }
            ("mcp_servers", Some("read")) => {
                fields(request, &[])?;
                return self.read("mcp_servers", &Py::Null);
            }
            ("mcp_servers", Some("update")) => {
                fields(request, &["expected_revision", "operations"])?;
                return native(self.host()?.update_mcp(
                    &value(request, "expected_revision")?,
                    &text(request, "operations")?,
                ));
            }
            ("connections", Some(verb)) => {
                let allowed: Option<&[&str]> = match verb {
                    "read" => {
                        fields(request, &[])?;
                        return self.read("connections", &Py::Null);
                    }
                    "update" => Some(&[
                        "expected_revision",
                        "connection_id",
                        "operation",
                        "name",
                        "max_concurrent_sessions",
                    ]),
                    "discover_connections" => Some(&[]),
                    "onboard_connection" => {
                        Some(&["engine", "expected_revision", "explicit_opt_in"])
                    }
                    "install_engine" => Some(&["engine"]),
                    "discover_opencode" => Some(&["binary"]),
                    "onboard_opencode" => {
                        Some(&["expected_revision", "explicit_opt_in", "binary", "name"])
                    }
                    _ => None,
                };
                if let Some(allowed) = allowed {
                    fields(request, allowed)?;
                    let mut fields = serde_json::Map::new();
                    for key in allowed {
                        if request.iter().any(|(k, _)| k == key) {
                            fields.insert(key.to_string(), value(request, key)?);
                        }
                    }
                    let fields = Value::Object(fields);
                    let host = self.host()?;
                    return native(match verb {
                        "update" => host.connection_update(&fields),
                        "discover_connections" => host.discover_connections(),
                        "onboard_connection" => host.onboard_connection(&fields),
                        "install_engine" => host.install_engine(
                            &fields["engine"],
                            std::time::Instant::now() + std::time::Duration::from_secs(85),
                        ),
                        "discover_opencode" => host.discover_opencode(&fields["binary"]),
                        _ => host.onboard_opencode(&fields),
                    });
                }
            }
            _ => (),
        }
        Err(refused(
            "unsupported_action",
            &format!(
                "Action {} is not supported for surface {}.",
                py::repr(action),
                py::repr(surface)
            ),
            Some("action"),
        ))
    }
}

/// A number the native writers cannot keep exact refuses before any effect;
/// a non-finite one never becomes a removal marker, a large integer never rounds.
fn write_value(value: &Py) -> Result<Value, Stop> {
    match value {
        Py::Int(s) if s.parse::<i64>().is_err() && s.parse::<u64>().is_err() => {
            Err(unavailable("Integers beyond the native exact range"))
        }
        Py::Float(f) if !f.is_finite() => Err(unavailable("Non-finite settings numbers")),
        Py::List(items) => items
            .iter()
            .map(write_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Py::Dict(entries) => entries
            .iter()
            .map(|(k, v)| Ok((k.clone(), write_value(v)?)))
            .collect::<Result<serde_json::Map<_, _>, Stop>>()
            .map(Value::Object),
        Py::Int(s) => Ok(serde_json::from_str(s).expect("bounded integer")),
        _ => Ok(to_value(value)),
    }
}

fn to_value(value: &Py) -> Value {
    match value {
        Py::Null => Value::Null,
        Py::Bool(b) => Value::Bool(*b),
        Py::Int(text) => text
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(text.clone())),
        Py::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Py::Str(s) => Value::String(s.clone()),
        Py::List(items) => Value::Array(items.iter().map(to_value).collect()),
        Py::Dict(entries) => Value::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), to_value(v)))
                .collect(),
        ),
    }
}

/// Answer one request: `(stdout, exit code)`.
pub fn execute(input: &[u8], environment: &Environment) -> (String, i32) {
    let api = Api { environment };
    let request = std::str::from_utf8(input).ok().and_then(py::loads);
    let answer = match request {
        None => Err(refused(
            "invalid_request",
            "stdin must hold one JSON request object.",
            None,
        )),
        Some(request) => api.dispatch(&request),
    };
    let version = ("api_version".to_string(), Py::int(API_VERSION));
    let (response, code) = match answer {
        Ok(result) => (
            Py::Dict(vec![
                version,
                ("ok".into(), Py::Bool(true)),
                ("result".into(), result),
            ]),
            0,
        ),
        Err(Stop::Refused(payload)) => (
            Py::Dict(vec![
                version,
                ("error".into(), sorted(payload)),
                ("ok".into(), Py::Bool(false)),
            ]),
            1,
        ),
        Err(Stop::Crash) => return (String::new(), 1),
    };
    match py::dumps(&response) {
        Ok(text) => (text + "\n", code),
        Err(py::Crash) => (String::new(), 1),
    }
}

/// `config --host` reports the resolved config directory without reading files.
/// Otherwise the request is stdin; host roots never come from request fields.
pub fn run(argv: &[String]) -> (String, i32) {
    use std::io::Read;
    if argv == ["--host"] {
        let environment = Environment::from_process();
        return match (Api {
            environment: &environment,
        })
        .route_environment()
        {
            Ok(host) => (
                format!(
                    "{}\n",
                    serde_json::json!({
                        "config_home": host.config.join("cortex"),
                        "state_home": host.state.join("cortex"),
                        "store_name": cortex_acp::startup::project_store_name(),
                        "channel": workspace_protocol::channel::current().name,
                    })
                ),
                0,
            ),
            Err(_) => (String::new(), 1),
        };
    }
    if !argv.is_empty() {
        return (String::new(), 2);
    }
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return (String::new(), 1);
    }
    execute(&input, &Environment::from_process())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> Environment {
        Environment {
            home: Some("/nonexistent-cortex-home".into()),
            config_home: None,
            state_home: None,
            routing: None,
            runtime_root: None,
        }
    }

    fn answer(input: &str) -> (String, i32) {
        execute(input.as_bytes(), &environment())
    }

    #[test]
    fn invalid_json_is_the_python_refusal() {
        assert_eq!(
            answer("{"),
            (
                "{\"api_version\": 1, \"error\": {\"category\": \"validation\", \"code\": \"invalid_request\", \
                 \"field\": null, \"message\": \"stdin must hold one JSON request object.\", \
                 \"retry_eligible\": false}, \"ok\": false}\n"
                    .into(),
                1
            )
        );
    }

    #[test]
    fn writes_validate_revision_and_fields_before_storage() {
        let (out, code) = answer(r#"{"action":"update","surface":"user_defaults"}"#);
        assert_eq!(code, 1);
        assert!(
            out.contains("\"code\": \"invalid_expected_revision\""),
            "{out}"
        );
        let (out, _) = answer(r#"{"action":"update","surface":"user_defaults","bogus":1}"#);
        assert!(
            out.contains("\"code\": \"unsupported_request_field\""),
            "{out}"
        );
    }

    #[test]
    fn defaults_never_create_project_local_owner_storage() {
        for marker in [".cortex", ".cortex-dev"] {
            let root = std::env::temp_dir().join(format!(
                "cortex-settings-missing-{}-{}",
                std::process::id(),
                marker
            ));
            assert!(!root.exists());
            let mut env = environment();
            env.config_home = Some(root.join(marker).join("config").to_str().unwrap().into());
            let (out, code) = execute(
                br#"{"action":"update","surface":"user_defaults","expected_revision":"absent","defaults":{"language":"en"}}"#,
                &env,
            );
            assert_eq!(code, 1);
            assert!(out.contains("connection_storage_under_project"), "{out}");
            assert!(!root.exists());
        }
    }

    #[test]
    fn unhashable_profile_action_crashes_like_python() {
        assert_eq!(
            answer(r#"{"action":["x"],"surface":"routing_profiles"}"#),
            (String::new(), 1)
        );
    }
}

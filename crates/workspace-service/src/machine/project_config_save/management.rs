//! Management-API writers (`cortex_runtime.configuration.management`) for the
//! Rust Paseo settings route: routing, MCP servers and the project ceiling.
//! Inputs and refusals keep the management shapes (`field`, `errors`,
//! `current_revision`); files are written with the same lock, bytes and modes.
use super::{routing::view::Ordered, user_defaults::ceiling_patch, validation::Errors, *};
use crate::config_read_model::{mcp, ConfigurationReader};
use crate::native_routes::RouteEnvironment;
use cortex_core::{store::{locks::AccessLock, StoreRoot}, OpenMode, ProjectKey, Store, StoreChannel};
use std::path::{Component, PathBuf};

pub type Answer = std::result::Result<Value, Value>;

/// `error_payload` of a management refusal.
pub fn error(code: &str, message: impl Into<String>, field: Option<&str>) -> Value {
    json!({"category":"validation","code":code,"message":message.into(),"field":field,
        "retry_eligible":false})
}
/// A native refusal as its management payload; recorded details win over `field`.
pub fn refusal(refused: Refused, field: Option<&str>) -> Value {
    let mut payload = error(refused.code, refused.message, field);
    for (key, value) in refused.detail.iter().flat_map(|d| d.as_object()).flatten() {
        payload[key] = value.clone();
    }
    payload
}
/// `StoreError` as the API reports it.
fn store_error(code: &str) -> Value {
    error(code, "The project store refused the request.", None)
}
fn from_store(error: cortex_core::CoreError) -> Value {
    match error.storage_diagnostic() {
        Some(diagnostic) => self::error(diagnostic.reason, diagnostic.message, None),
        None => store_error(error.code()),
    }
}
pub fn expected_revision(value: &Value) -> std::result::Result<String, Value> {
    value
        .as_str()
        .filter(|s| {
            *s == "absent"
                || s.len() == 71
                    && s.starts_with("sha256:")
                    && s[7..]
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                "invalid_expected_revision",
                "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>').",
                Some("expected_revision"),
            )
        })
}
/// The owner configuration root (`registry.config_root`), fenced like every writer.
pub fn owner(environment: &RouteEnvironment) -> std::result::Result<PathBuf, Value> {
    if [&environment.config, &environment.state]
        .iter()
        .any(|path| {
            path.components().any(
                |part| matches!(part, Component::Normal(s) if s == ".cortex" || s == ".cortex-dev"),
            )
        })
    {
        return Err(error(
            "connection_storage_under_project",
            "Connection storage must be user-scoped, never inside a project's Cortex store.",
            None,
        ));
    }
    environment.validate().map_err(|e| {
        error(
            e.code(),
            "The host configuration environment is unsafe.",
            None,
        )
    })?;
    Ok(environment.config.join("cortex"))
}
/// `_owner_config_lock`: the private root, its ignore file and `connections.lock`.
pub fn lock(folder: &Path) -> std::result::Result<cortex_core::store::locks::AccessLock, Value> {
    let lock = owner_lock(folder).map_err(|e| refusal(e, None))?;
    user_defaults::ignore(folder).map_err(|e| refusal(e, None))?;
    Ok(lock)
}
fn reader(environment: &RouteEnvironment) -> std::result::Result<ConfigurationReader, Value> {
    ConfigurationReader::new(environment.clone()).map_err(|e| {
        error(
            e.code(),
            "The Cortex configuration storage refused the read; nothing was changed.",
            None,
        )
    })
}
fn native_read(e: cortex_core::CoreError) -> Value {
    error(
        e.code(),
        "The Cortex configuration storage refused the read; nothing was changed.",
        None,
    )
}
/// `str(Path(value))` when it is also the resolved real directory.
fn canonical(value: &str) -> Option<PathBuf> {
    let path: PathBuf = Path::new(value).components().collect();
    (path.is_absolute() && path.is_dir() && path.canonicalize().ok()? == path).then_some(path)
}
fn channel(store: &str) -> std::result::Result<StoreChannel, Value> {
    match store {
        ".cortex" => Ok(StoreChannel::Release),
        ".cortex-dev" => Ok(StoreChannel::Dev),
        _ => Err(store_error("project_storage_channel_invalid")),
    }
}
/// `object.get(key)` of a parsed request value, in its own order.
fn member<'a>(node: &'a Ordered, key: &str) -> Option<&'a Ordered> {
    match node {
        Ordered::Object(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}
fn plain(node: &Ordered) -> Value {
    serde_json::to_value(node).unwrap()
}
fn parse(text: &str) -> std::result::Result<(Value, Ordered), Value> {
    let invalid = || {
        error(
            "invalid_request",
            "The request must be a JSON object.",
            None,
        )
    };
    Ok((
        serde_json::from_str(text).map_err(|_| invalid())?,
        serde_json::from_str(text).map_err(|_| invalid())?,
    ))
}

// --- routing -----------------------------------------------------------------

/// `_project_routing_path`.
fn project_routing_path(
    environment: &RouteEnvironment,
    project: &Value,
    store: &str,
) -> std::result::Result<(PathBuf, AccessLock), Value> {
    let Some(raw) = project.as_str().filter(|s| s.starts_with('/')) else {
        return Err(error(
            "invalid_project",
            "The project target needs the absolute project root.",
            Some("project_root"),
        ));
    };
    let Some(root) = canonical(raw) else {
        return Err(store_error("invalid_project"));
    };
    let missing = || {
        error(
            "project_store_missing",
            format!("{raw} has no Cortex project store (.cortex); open a task there first."),
            Some("project_root"),
        )
    };
    if root.as_os_str() != raw {
        return Err(missing());
    }
    let key = routing_key(environment, &root, store).map_err(from_store)?;
    let anchor = key.anchor_directory();
    // Serialize writes of legacy config-only folders with the first store
    // initialization that copies their routing. The same lock fences an
    // existing local database's relocation.
    cortex_core::store::locks::private_directory(&anchor, false).map_err(|error| {
        if crate::local::store_missing(&key, &error) { missing() } else { from_store(error) }
    })?;
    let lease = AccessLock::acquire(
        &anchor.join(".access.lock"),
        true,
        false,
        std::time::Duration::ZERO,
    ).map_err(from_store)?;
    let directory = crate::store_locations::directory(key).map_err(from_store)?;
    let meta = fs::symlink_metadata(&directory).map_err(|_| missing())?;
    if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(error(
            "unsafe_storage",
            format!("{} is not the owner's real directory.", directory.display()),
            Some("project_root"),
        ));
    }
    Ok((directory.join("routing.json"), lease))
}
/// The read view's project pair: the trusted root and its routing file.
pub fn view_project(
    environment: &RouteEnvironment,
    project: &Value,
    store: &str,
) -> std::result::Result<Option<(PathBuf, PathBuf)>, Value> {
    let Some(project) = project.as_str().filter(|s| s.starts_with('/')) else {
        return Ok(None);
    };
    let root = PathBuf::from(project);
    let key = routing_key(environment, &root, store).map_err(from_store)?;
    let routing = crate::store_locations::directory(key).map_err(from_store)?
        .join("routing.json");
    Ok(Some((root, routing)))
}

fn routing_key(
    environment: &RouteEnvironment,
    root: &Path,
    store: &str,
) -> cortex_core::CoreResult<ProjectKey> {
    let channel = channel(store)
        .map_err(|_| cortex_core::CoreError::Refused("project_storage_channel_invalid"))?;
    let key = StoreRoot::new(&environment.state, channel)?.project_key(root)?;
    if key.root() != root {
        return Err(cortex_core::CoreError::Refused("project_context_conflict"));
    }
    Ok(key)
}

/// `update_routing`. `rules` is the request's JSON text (its key order is kept).
pub fn update_routing(
    environment: &RouteEnvironment,
    store: &str,
    target: &Value,
    expected: &Value,
    rules: &str,
    project: &Value,
) -> Answer {
    if target == "env" {
        return Err(error(
            "routing_source_read_only",
            "$CORTEX_ROUTING names a file Cortex never modifies; edit it where it is kept, or unset the variable.",
            Some("target"),
        ));
    }
    let Some(target) = target.as_str().filter(|t| matches!(*t, "project" | "user")) else {
        return Err(error(
            "invalid_target",
            "target must be one of project, user.",
            Some("target"),
        ));
    };
    let expected = expected_revision(expected)?;
    let (value, ordered) = parse(rules)?;
    routing::validate_management_rules(&value, &ordered).map_err(|e| refusal(e, None))?;
    let fields: Vec<(String, Ordered)> = routing::KEYS
        .iter()
        .filter_map(|k| member(&ordered, k).map(|v| (k.to_string(), v.clone())))
        .collect();
    let (project_path, _project_lease) = if target == "project" {
        let (path, lease) = project_routing_path(environment, project, store)?;
        (Some(path), Some(lease))
    } else {
        (None, None)
    };
    let pair = view_project(environment, project, store)?;
    let folder = owner(environment)?;
    let path = project_path.unwrap_or_else(|| folder.join("routing.json"));
    let (body, before) = {
        let _lock = lock(&folder)?;
        let before = reader::read_plain(&path, false).map_err(|e| refusal(e, None))?;
        check(
            &format!("routing ({target})"),
            &expected,
            &token(before.as_deref()),
        )
        .map_err(|e| refusal(e, None))?;
        let body = routing::pretty_ordered(&path, fields, before.as_deref())
            .map_err(|e| refusal(e, None))?;
        if body.len() > FRAME_BUDGET {
            return Err(error(
                "machine_reply_too_large",
                "Settings exceed the native write limit.",
                None,
            ));
        }
        replace(&path, &body).map_err(|e| refusal(e, None))?;
        (body, before)
    };
    let reader = reader(environment)?;
    let profile = if target == "user" {
        reader.selection_with_routing(&body)
    } else {
        Value::Null
    };
    let mut view = reader
        .routing_management(pair.as_ref().map(|(a, b)| (a.as_path(), b.as_path())))
        .map_err(native_read)?;
    view["changed"] = json!(Some(body.as_slice()) != before.as_deref());
    view["target"] = json!(target);
    view["written_revision"] = json!(token(Some(&body)));
    view["shadowed_by"] = if view["effective_source"] == target {
        Value::Null
    } else {
        view["effective_source"].clone()
    };
    view["profile"] = profile;
    Ok(view)
}

// --- MCP servers ---------------------------------------------------------------

fn server_name(name: &str) -> bool {
    name != "cortex"
        && crate::config_read_model::matches(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$", name)
}
/// `_old_env`.
fn old_env(value: &Value) -> Vec<(String, String)> {
    if let Some(m) = value.as_object() {
        return m
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some((
                v["name"].as_str()?.to_owned(),
                v["value"].as_str()?.to_owned(),
            ))
        })
        .collect()
}
fn keep(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == 1 && m.get("keep") == Some(&json!(true)))
}
/// `_resolve_env`: `None` when `new` is absent or not a supported shape.
fn resolve_env(
    new: Option<&Ordered>,
    old: &Value,
    field: &str,
    errors: &mut Errors,
) -> Option<Vec<(String, Ordered)>> {
    let new = new.filter(|n| !matches!(n, Ordered::Scalar(Value::Null)))?;
    let previous = old_env(old);
    let pairs: Vec<(Value, Value)> = match new {
        Ordered::Object(m) => m.iter().map(|(k, v)| (json!(k), plain(v))).collect(),
        Ordered::Array(items) if items.iter().all(Ordered::is_object) => items
            .iter()
            .map(|item| {
                let item = plain(item);
                let value = if item.get("keep") == Some(&json!(true)) {
                    json!({"keep":true})
                } else {
                    item.get("value").cloned().unwrap_or(Value::Null)
                };
                (item.get("name").cloned().unwrap_or(Value::Null), value)
            })
            .collect(),
        _ => {
            errors.coded(
                field,
                "invalid_type",
                "must map NAME -> value or {\"keep\": true}",
            );
            return None;
        }
    };
    let mut result: Vec<(String, Ordered)> = vec![];
    for (name, value) in pairs {
        let Some(name) = name.as_str().filter(|n| {
            crate::config_read_model::matches(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$", n)
                && !result.iter().any(|(k, _)| k == n)
        }) else {
            errors.coded(
                field,
                "invalid_env_name",
                "variable names are unique identifiers",
            );
            continue;
        };
        if keep(&value) {
            match previous.iter().rev().find(|(k, _)| k == name) {
                Some((_, stored)) => result.push((name.into(), Ordered::Scalar(json!(stored)))),
                None => errors.coded(
                    format!("{field}.{name}"),
                    "unknown_env_value",
                    "has no stored value to keep",
                ),
            }
        } else if value.as_str().is_some_and(|s| !s.contains('\0')) {
            result.push((name.into(), Ordered::Scalar(value)));
        } else {
            errors.coded(
                format!("{field}.{name}"),
                "invalid_value",
                "must be a string or {\"keep\": true}",
            );
        }
    }
    Some(result)
}
fn entries(node: &mut Ordered) -> &mut Vec<(String, Ordered)> {
    match node {
        Ordered::Object(m) => m,
        _ => unreachable!("checked object"),
    }
}
fn put(map: &mut Vec<(String, Ordered)>, key: &str, value: Ordered) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some((_, old)) => *old = value,
        None => map.push((key.into(), value)),
    }
}
fn mcp_view(path: &Path, bytes: &[u8]) -> Answer {
    let observed = mcp::observation(path, Some(bytes)).map_err(native_read)?;
    if observed["ok"] == true {
        Ok(observed["result"].clone())
    } else {
        Err(observed["error"].clone())
    }
}

/// `update_mcp_servers`. `operations` is the request's JSON text.
pub fn update_mcp(environment: &RouteEnvironment, expected: &Value, operations: &str) -> Answer {
    let expected = expected_revision(expected)?;
    let (value, ordered) = parse(operations)?;
    let Some(items) = value
        .as_array()
        .filter(|a| !a.is_empty() && a.iter().all(Value::is_object))
    else {
        return Err(error(
            "invalid_operations",
            "operations must be a non-empty array of operation objects.",
            Some("operations"),
        ));
    };
    let Ordered::Array(ordered_items) = ordered else {
        unreachable!("checked array")
    };
    let folder = owner(environment)?;
    let path = folder.join("mcp-servers.json");
    let _lock = lock(&folder)?;
    let data = reader::read_plain(&path, true).map_err(|e| refusal(e, None))?;
    check("mcp_servers", &expected, &token(data.as_deref())).map_err(|e| refusal(e, None))?;
    let current = mcp::observation(&path, data.as_deref()).map_err(native_read)?;
    if current["ok"] != true {
        return Err(current["error"].clone());
    }
    let key = current["result"]["root_key"].as_str().unwrap().to_owned();
    if let Some(bytes) = &data {
        user_defaults::exact_numbers(bytes).map_err(|e| refusal(e, None))?;
    }
    let mut document: Ordered = serde_json::from_slice(
        data.as_deref()
            .unwrap_or(b"{\"version\": 1, \"servers\": {}}"),
    )
    .map_err(|_| error("invalid_mcp_registry", "mcp-servers.json is invalid.", None))?;
    let raw = plain(&document);
    if raw
        .get("version")
        .is_some_and(|v| v != &json!(1) && v != &json!(1.0) && v != &json!(true))
    {
        return Err(error(
            "unsupported_mcp_registry_version",
            "mcp-servers.json 'version' must be 1.",
            Some("version"),
        ));
    }
    let mut servers = member(&document, &key).cloned().unwrap();
    let mut errors = Errors::default();
    for (index, (operation, node)) in items.iter().zip(&ordered_items).enumerate() {
        let field = format!("operations[{index}]");
        let name = &operation["name"];
        let known = name.as_str().and_then(|n| {
            entries(&mut servers)
                .iter()
                .position(|(k, _)| k == n)
                .map(|i| (n.to_owned(), i))
        });
        match operation["op"].as_str() {
            Some("upsert") => {
                let Some(name) = name.as_str().filter(|n| server_name(n)) else {
                    errors.coded(
                        format!("{field}.name"),
                        "invalid_server_name",
                        "must be a short name of letters, digits, '.', '_' or '-' other than 'cortex'",
                    );
                    continue;
                };
                let Some(spec @ Ordered::Object(spec_fields)) = member(node, "server") else {
                    errors.coded(
                        format!("{field}.server"),
                        "invalid_type",
                        "must be an object",
                    );
                    continue;
                };
                let mut unknown: Vec<&String> = spec_fields
                    .iter()
                    .map(|(k, _)| k)
                    .filter(|k| !mcp::KEYS.contains(&k.as_str()) && *k != mcp::CONTAINMENT)
                    .collect();
                unknown.sort();
                unknown.dedup();
                for item in unknown {
                    errors.coded(
                        format!("{field}.server.{item}"),
                        "unsupported_setting",
                        format!("use one of {}", mcp::KEYS.join(", ")),
                    );
                }
                let mut entry: Vec<(String, Ordered)> = mcp::KEYS
                    .iter()
                    .filter(|k| **k != "env")
                    .filter_map(|k| member(spec, k).map(|v| (k.to_string(), v.clone())))
                    .collect();
                let previous = known
                    .as_ref()
                    .map(|(_, i)| plain(&entries(&mut servers)[*i].1))
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({}));
                let resolved = resolve_env(
                    member(spec, "env"),
                    &previous["env"],
                    &format!("{field}.server.env"),
                    &mut errors,
                );
                if let Some(env) = resolved.filter(|e| !e.is_empty()) {
                    entry.push(("env".into(), Ordered::Object(env)));
                }
                if let Some(c) = member(spec, mcp::CONTAINMENT) {
                    entry.push((mcp::CONTAINMENT.into(), c.clone()));
                }
                let entry = Ordered::Object(entry);
                if member(&entry, "enabled").is_some_and(|v| !plain(v).is_boolean()) {
                    errors.coded(
                        format!("{field}.server.enabled"),
                        "invalid_type",
                        "must be true or false",
                    );
                    continue;
                }
                if let Some(problem) = mcp::problem(name, &plain(&entry), &path) {
                    errors.coded(
                        format!("{field}.server"),
                        problem_code(&problem),
                        problem["message"].as_str().unwrap(),
                    );
                    continue;
                }
                put(entries(&mut servers), name, entry);
            }
            Some("remove") => match known {
                Some((name, _)) => entries(&mut servers).retain(|(k, _)| *k != name),
                None => errors.coded(
                    format!("{field}.name"),
                    "unknown_server",
                    "names no configured server",
                ),
            },
            Some("set_enabled") => {
                let enabled = &operation["enabled"];
                match known {
                    Some((_, i)) if entries(&mut servers)[i].1.is_object() => {
                        if !enabled.is_boolean() {
                            errors.coded(
                                format!("{field}.enabled"),
                                "invalid_type",
                                "must be true or false",
                            );
                        } else {
                            let spec = &mut entries(&mut servers)[i].1;
                            put(entries(spec), "enabled", Ordered::Scalar(enabled.clone()));
                        }
                    }
                    _ => errors.coded(
                        format!("{field}.name"),
                        "unknown_server",
                        "names no configured server",
                    ),
                }
            }
            _ => errors.coded(
                format!("{field}.op"),
                "unsupported_operation",
                "must be upsert, remove or set_enabled",
            ),
        }
    }
    errors.finish().map_err(|e| refusal(e, None))?;
    // The runtime loader's verdict on the whole registry blocks the write.
    for (name, spec) in entries(&mut servers).iter() {
        let spec = plain(spec);
        let problem = if name.is_empty() || name == "cortex" || !spec.is_object() {
            Some(json!({"code":"invalid_mcp_registry","message":
                format!("{} contains an invalid server entry.", path.display())}))
        } else if spec["enabled"] == false {
            None
        } else {
            mcp::problem(name, &spec, &path)
        };
        if let Some(problem) = problem {
            let mut e = Errors::default();
            e.coded(
                "servers",
                problem_code(&problem),
                problem["message"].as_str().unwrap(),
            );
            e.finish().map_err(|e| refusal(e, None))?;
        }
    }
    let document_fields = entries(&mut document);
    if !document_fields.iter().any(|(k, _)| k == "version") {
        document_fields.push(("version".into(), Ordered::Scalar(json!(1))));
    }
    put(document_fields, &key, servers);
    let mut body = document.pretty();
    body.push(b'\n');
    let mut reply = mcp_view(&path, &body)?;
    reply["changed"] = json!(Some(body.as_slice()) != data.as_deref());
    if body.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(error(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
            None,
        ));
    }
    replace(&path, &body).map_err(|e| refusal(e, None))?;
    Ok(reply)
}
fn problem_code(problem: &Value) -> &'static str {
    match problem["code"].as_str() {
        Some("external_mcp_explicit_tool_grants_required") => {
            "external_mcp_explicit_tool_grants_required"
        }
        _ => "invalid_mcp_registry",
    }
}

// --- project ceiling -------------------------------------------------------------

/// `_open_project_store`: the canonical root and its opened store.
fn project_store(
    environment: &RouteEnvironment,
    project: &Value,
    store: &str,
    write: bool,
) -> std::result::Result<(PathBuf, Store), Value> {
    let Some(root) = project.as_str().and_then(canonical) else {
        return Err(error(
            "invalid_project",
            "project must be an existing absolute project root without symlinks.",
            Some("project"),
        ));
    };
    let channel = channel(store)?;
    let missing = || {
        error(
            "project_store_missing",
            format!(
                "{} has no Cortex project store yet; open a task there first.",
                root.display()
            ),
            Some("project"),
        )
    };
    let key = StoreRoot::new(&environment.state, channel)
        .and_then(|state| state.project_key(&root))
        .map_err(from_store)?;
    if key.root() != root && !root.join(store).join("cortex.sqlite3").is_file() {
        return Err(missing());
    }
    let mode = if write {
        OpenMode::RuntimeValidate
    } else {
        OpenMode::LiveReadOnly
    };
    let opened = match Store::open(key.clone(), mode) {
        Ok(store) => store,
        Err(e) if crate::local::store_missing(&key, &e) => return Err(missing()),
        Err(e) => return Err(from_store(e)),
    };
    Ok((root, opened))
}
fn user_ceiling(environment: &RouteEnvironment) -> std::result::Result<Value, Value> {
    ceiling::user_at(&owner(environment)?).map_err(|e| refusal(e, None))
}
/// `_project_view`.
fn project_view(root: &Path, row: Option<&ceiling::Row>, user: &Value) -> Answer {
    let mut view = ceiling::response(root, row, user).map_err(|e| refusal(e, None))?;
    view.as_object_mut().unwrap().remove("changed");
    if let Some((_, stored, _)) = row {
        let stored: Value = serde_json::from_str(stored).map_err(|_| {
            error(
                "invalid_ceiling_snapshot",
                "A stored ceiling is invalid.",
                None,
            )
        })?;
        if let Err(violation) = ceiling::narrow(&stored, user) {
            let detail = violation.detail.as_deref().cloned().unwrap_or_default();
            let item = &detail["errors"][0];
            view["violation"] = json!({"code":item["code"],
                "field":item["field"].as_str().map(|f| f.trim_start_matches("ceiling.")),
                "message":item["message"]});
            view["narrows_user_ceiling"] = json!(false);
        }
    }
    Ok(view)
}
/// `read_project_ceiling` (through `_open_project_store(project, write=False)`).
pub fn read_project_ceiling(
    environment: &RouteEnvironment,
    store: &str,
    project: &Value,
) -> Answer {
    let (root, opened) = project_store(environment, project, store, false)?;
    let row = opened
        .read(|db| ceiling::row(db, &root))
        .map_err(from_store)?;
    let user = user_ceiling(environment)?;
    project_view(&root, row.as_ref(), &user)
}
/// `update_project_ceiling` (through `_open_project_store(project, write=True)`).
pub fn update_project_ceiling(
    environment: &RouteEnvironment,
    store: &str,
    project: &Value,
    expected: &Value,
    candidate: &Value,
) -> Answer {
    let (root, opened) = project_store(environment, project, store, true)?;
    let expected = expected_revision(expected)?;
    let candidate = if candidate.is_null() {
        None
    } else {
        let mut problems = user_defaults::Problems::default();
        let parsed = ceiling_patch(candidate, &mut problems);
        problems.finish()?;
        let user = user_ceiling(environment)?;
        ceiling::narrow(&parsed, &user).map_err(|e| refusal(e, None))?;
        Some(parsed)
    };
    let mut outcome: Option<Answer> = None;
    let written = opened.write(|db| {
        let current = ceiling::row(db, &root)?;
        if let Err(e) = check("project_ceiling", &expected, &ceiling::revision(current.as_ref())) {
            outcome = Some(Err(refusal(e, None)));
            return Ok(None);
        }
        match &candidate {
            None => {
                if let Some((defaults, _, _)) = &current {
                    let defaults: Value = serde_json::from_str(defaults)?;
                    if validation::truth(&defaults) {
                        outcome = Some(Err(error(
                            "project_defaults_present",
                            "This project row also holds project defaults; replace the ceiling instead of removing it.",
                            Some("ceiling"),
                        )));
                        return Ok(None);
                    }
                    db.execute(
                        "DELETE FROM project_configuration_defaults WHERE project_root=?1",
                        [root.to_string_lossy().as_ref()],
                    )?;
                }
            }
            Some(c) => {
                let defaults = current.as_ref().map_or_else(|| "{}".into(), |r| r.0.clone());
                db.execute(
                    "INSERT INTO project_configuration_defaults VALUES (?1,?2,?3,?4) ON CONFLICT(project_root) DO UPDATE SET ceiling=excluded.ceiling,updated_at=excluded.updated_at",
                    rusqlite::params![root.to_string_lossy().as_ref(), defaults, c.to_string(), cortex_core::now()],
                )?;
            }
        }
        ceiling::row(db, &root).map(Some)
    });
    if let Some(answer) = outcome {
        return answer;
    }
    let row = written.map_err(from_store)?.flatten();
    let user = user_ceiling(environment)?;
    let mut view = project_view(&root, row.as_ref(), &user)?;
    view["changed"] = json!(true);
    Ok(view)
}

//! Complete routing save reply; display observations never authorize a route.
use super::*;
use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
#[derive(Clone)]
pub(in crate::machine) enum Ordered {
    Object(Vec<(String, Ordered)>),
    Array(Vec<Ordered>),
    Scalar(Value),
}
impl Ordered {
    pub fn pretty(&self) -> Vec<u8> {
        fn float(v: &serde_json::Number) -> String {
            let raw = v.to_string();
            if !v.is_f64() {
                return raw;
            }
            let (sign, raw) = raw
                .strip_prefix('-')
                .map(|s| ("-", s))
                .unwrap_or(("", raw.as_str()));
            let (base, power) = raw
                .split_once('e')
                .or_else(|| raw.split_once('E'))
                .map(|(b, e)| (b, e.parse::<i32>().unwrap()))
                .unwrap_or((raw, 0));
            let point = base.find('.').unwrap_or(base.len()) as i32;
            let all = base.replace('.', "");
            let leading = all.bytes().take_while(|b| *b == b'0').count();
            let digits = all[leading..].trim_end_matches('0');
            if digits.is_empty() {
                return format!("{sign}0.0");
            }
            let exponent = power + point - leading as i32 - 1;
            if !(-4..16).contains(&exponent) {
                let tail = if digits.len() > 1 {
                    format!(".{}", &digits[1..])
                } else {
                    String::new()
                };
                return format!("{sign}{}{tail}e{exponent:+03}", &digits[..1]);
            }
            if exponent < 0 {
                return format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize));
            }
            let n = exponent as usize + 1;
            if digits.len() <= n {
                format!("{sign}{digits}{}.0", "0".repeat(n - digits.len()))
            } else {
                format!("{sign}{}.{}", &digits[..n], &digits[n..])
            }
        }
        fn render(v: &Ordered, level: usize) -> String {
            let pad = "  ".repeat(level + 1);
            let end = "  ".repeat(level);
            match v {
                Ordered::Object(m) if !m.is_empty() => format!(
                    "{{\n{}\n{end}}}",
                    m.iter()
                        .map(|(k, v)| format!(
                            "{pad}{}: {}",
                            serde_json::to_string(k).unwrap(),
                            render(v, level + 1)
                        ))
                        .collect::<Vec<_>>()
                        .join(",\n")
                ),
                Ordered::Array(a) if !a.is_empty() => format!(
                    "[\n{}\n{end}]",
                    a.iter()
                        .map(|v| format!("{pad}{}", render(v, level + 1)))
                        .collect::<Vec<_>>()
                        .join(",\n")
                ),
                Ordered::Object(_) => "{}".into(),
                Ordered::Array(_) => "[]".into(),
                Ordered::Scalar(Value::Number(n)) => float(n),
                Ordered::Scalar(v) => serde_json::to_string(v).unwrap(),
            }
        }
        render(self, 0).into_bytes()
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }
    pub fn from_value(v: &Value) -> Self {
        match v {
            Value::Object(m) => Self::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
            Value::Array(a) => Self::Array(a.iter().map(Self::from_value).collect()),
            v => Self::Scalar(v.clone()),
        }
    }
}
impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Scalar(v) => v.serialize(s),
            Self::Object(m) => {
                let mut out = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    out.serialize_entry(k, v)?;
                }
                out.end()
            }
            Self::Array(a) => {
                let mut out = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    out.serialize_element(v)?;
                }
                out.end()
            }
        }
    }
}
struct OV;
impl<'de> Visitor<'de> for OV {
    type Value = Ordered;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("finite JSON")
    }
    fn visit_unit<E: Error>(self) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(Value::Null))
    }
    fn visit_bool<E: Error>(self, v: bool) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(v.into()))
    }
    fn visit_i64<E: Error>(self, v: i64) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(v.into()))
    }
    fn visit_u64<E: Error>(self, v: u64) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(v.into()))
    }
    fn visit_f64<E: Error>(self, v: f64) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(
            serde_json::Number::from_f64(v)
                .ok_or_else(|| E::custom("nonfinite"))?
                .into(),
        ))
    }
    fn visit_str<E: Error>(self, v: &str) -> std::result::Result<Ordered, E> {
        Ok(Ordered::Scalar(v.into()))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> std::result::Result<Ordered, A::Error> {
        let mut out: Vec<(String, Ordered)> = vec![];
        let mut positions = std::collections::BTreeMap::new();
        while let Some((k, v)) = m.next_entry::<String, Ordered>()? {
            if let Some(i) = positions.get(&k) {
                out[*i] = (k, v);
            } else {
                positions.insert(k.clone(), out.len());
                out.push((k, v));
            }
        }
        Ok(Ordered::Object(out))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Ordered, A::Error> {
        let mut out = vec![];
        while let Some(v) = a.next_element()? {
            out.push(v);
        }
        Ok(Ordered::Array(out))
    }
}
impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(OV)
    }
}
fn normalize(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(m) = v.get_mut("engine_affinity").and_then(Value::as_object_mut) {
        if let Some(legacy) = m.remove("implementation") {
            for k in &KINDS[..3] {
                m.entry(k.to_string()).or_insert(legacy.clone());
            }
        }
    }
    if let Some(models) = v.get_mut("models").and_then(Value::as_object_mut) {
        for c in models.values_mut() {
            if let Some(ps) = c.get_mut("policy").and_then(Value::as_object_mut) {
                for p in ps.values_mut() {
                    for k in ["preferred_for", "also_standard_for"] {
                        if let Some(a) = p[k].as_array() {
                            let mut expanded = vec![];
                            for n in a {
                                let values = if n == "implementation" {
                                    KINDS[..3].iter().map(|s| json!(s)).collect::<Vec<_>>()
                                } else {
                                    vec![n.clone()]
                                };
                                for n in values {
                                    if !expanded.contains(&n) {
                                        expanded.push(n);
                                    }
                                }
                            }
                            p[k] = json!(expanded);
                        }
                    }
                }
            }
        }
    }
    v
}
fn payload(code: &str, message: &str, field: Value) -> Value {
    json!({"category":"validation","code":code,"message":message,"field":field,"retry_eligible":false})
}
fn file_view(source: &str, path: &Path, replacement: Option<&[u8]>) -> Result<Value> {
    let mut out = json!({"source":source,"path":path,"writable":source!="env"});
    let bytes = if let Some(b) = replacement {
        Some(b.to_vec())
    } else {
        match reader::read_plain(path, false) {
            Ok(b) => b,
            Err(e) if e.code == "unsafe_storage" => {
                out["exists"] = json!(true);
                out["revision"] = Value::Null;
                out["valid"] = json!(false);
                out["error"]=payload("unsafe_storage",&format!("{} is a symlink or cannot be opened safely; Cortex configuration files must be real files.",path.display()),Value::Null);
                return Ok(out);
            }
            Err(e) => return Err(e),
        }
    };
    out["exists"] = json!(bytes.is_some());
    out["revision"] = json!(token(bytes.as_deref()));
    let Some(b) = bytes else {
        return Ok(out);
    };
    let d: Value = match serde_json::from_slice(&b) {
        Ok(v) => v,
        Err(_) => {
            // Native diagnostic is deliberately bounded and does not echo bytes.
            out["valid"] = json!(false);
            out["error"] = json!({"code":"invalid_routing_overrides","message":"Saved routing is not valid UTF-8 JSON."});
            return Ok(out);
        }
    };
    let named = if !d.is_object() {
        None
    } else if d["model_policy_version"].is_null() {
        Some(("model_policy_migration_required","Explicitly migrate routing.json to model_policy_version=2 with exact enabled/auto identities and per-engine tiers."))
    } else if d["model_policy_version"].as_i64() != Some(2) {
        Some((
            "model_policy_version_unsupported",
            "Only model_policy_version=2 is supported.",
        ))
    } else {
        None
    };
    if let Some((code, msg)) = named {
        out["valid"] = json!(false);
        out["error"] = payload(code, msg, json!("model_policy_version"));
        return Ok(out);
    }
    if !super::super::super::project_routing::valid_routing_structure(&d) {
        out["valid"] = json!(false);
        out["error"] = json!({"code":"invalid_routing_overrides","message":"Saved routing has an invalid shape."});
        return Ok(out);
    }
    // Preserve source insertion order for named first-error diagnostics.
    let ordered: Ordered = serde_json::from_slice(&b).unwrap();
    let models = match &ordered {
        Ordered::Object(m) => m.iter().find(|(k, _)| k == "models").map(|(_, v)| v),
        _ => None,
    };
    let mut labels = std::collections::BTreeMap::new();
    if let Some(Ordered::Object(engines)) = models {
        for (engine, config) in engines {
            let config_value = &d["models"][engine];
            for model in config_value["auto"].as_array().into_iter().flatten() {
                let name = model.as_str().unwrap();
                if config_value["tiers"][name].is_null() {
                    out["valid"] = json!(false);
                    out["error"] = payload(
                        "policy_missing",
                        &format!("models.{engine}.tiers.{name} needs nonempty explicit tiers."),
                        json!("model_policy_version"),
                    );
                    return Ok(out);
                }
            }
            let policies = match config {
                Ordered::Object(m) => m.iter().find(|(k, _)| k == "policy").map(|(_, v)| v),
                _ => None,
            };
            if let Some(Ordered::Object(policies)) = policies {
                for (name, _) in policies {
                    let identity = format!("{engine}/{name}");
                    for label in config_value["policy"][name]["request_names"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let label = label.as_str().unwrap();
                        if labels.get(label).is_some_and(|old| old != &identity) {
                            out["valid"] = json!(false);
                            out["error"] = payload(
                                "model_request_label_ambiguous",
                                &format!(
                                    "Request label '{label}' names more than one exact identity."
                                ),
                                json!("model_policy_version"),
                            );
                            return Ok(out);
                        }
                        labels.insert(label.to_owned(), identity.clone());
                    }
                }
            }
        }
    }
    let mut rules = json!({});
    for k in KEYS {
        if let Some(v) = d.get(k) {
            rules[k] = v.clone();
        }
    }
    // The Workspace editor manages the coordinator list; other management clients keep it as an ignored key.
    if let Some(v) = d
        .get(crate::native_routes::COORDINATOR_MODELS)
        .filter(|v| v.is_array())
    {
        rules[crate::native_routes::COORDINATOR_MODELS] = v.clone();
    }
    out["valid"] = json!(true);
    out["rules"] = rules;
    out["ignored_keys"] = json!(d
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !KEYS.contains(&k.as_str()))
        .collect::<Vec<_>>());
    Ok(out)
}
fn builtin() -> Value {
    json!({"model_efforts":{},"supported_engines":ENGINES,"optional_engines":["opencode"],"model_policy_version":2,
    "default_auto_deny_engines":[],"default_auto_deny_families":[],"tiers":TIERS,"efforts":EFFORTS,
    "work_kinds":&KINDS[..13],"work_kind_labels":{
        "implementation_backend":"Backend implementation","implementation_frontend":"Frontend implementation (no design)",
        "implementation_frontend_design":"Frontend implementation with UI design","operations":"Operations","debugging":"Debugging",
        "testing":"Testing","investigation":"Investigation","review":"Review","design":"Design and architecture",
        "writing":"Documentation and writing","planning":"Planning","analysis":"Analysis","consultation":"Senior consultation"},
    "work_kind_roles":{
        "implementation_backend":["worker-backend-dev","worker-data-engineer","worker-fullstack-dev","worker-general","worker-refactorer"],
        "implementation_frontend":["worker-accessibility-fixer","worker-frontend-dev","worker-mobile-dev"],"implementation_frontend_design":[],
        "operations":["worker-devops-engineer"],"debugging":["worker-debugger","worker-performance-engineer"],
        "testing":["worker-build-verification","worker-qa-engineer"],"investigation":["worker-explorer"],
        "review":["worker-accessibility-auditor","worker-code-reviewer","worker-security-auditor"],
        "design":["worker-architect","worker-database-architect","worker-ux-designer"],
        "writing":["worker-technical-writer"],"planning":["worker-planner"],"analysis":[],"consultation":["worker-senior-consultant"]},
    "builtin_affinity":{"implementation_backend":"codex-acp","implementation_frontend":"codex-acp",
        "implementation_frontend_design":"claude-agent-acp","operations":"codex-acp","debugging":null,"testing":null,
        "investigation":"claude-agent-acp","review":"claude-agent-acp","design":"claude-agent-acp","writing":"claude-agent-acp","planning":"claude-agent-acp","analysis":"claude-agent-acp","consultation":"claude-agent-acp"},
    "default_user_deny":[],"selection_modes":["listed"],"tier_sources":["configured"],
    "policy_attributes":["effort_floor","max_effort","effort_ladder","also_standard_for","preferred_for","preferred_when","request_names"],
    "policy_efforts":&EFFORTS[..5],"max_request_names":16})
}
fn state_root(config: &crate::Config) -> Result<std::path::PathBuf> {
    if workspace_protocol::channel::current().is_dev() {
        return Ok(config.state_dir.join("state/cortex"));
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or("/".into());
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(home.join(".local/state"));
    if !root.is_absolute() {
        return Err(refuse(
            "unsafe_storage",
            "'XDG_STATE_HOME' must be an absolute path.",
        ));
    }
    Ok(root.join("cortex"))
}
pub(crate) fn valid_entry(entry: &Value, id: &str) -> bool {
    let r = &entry["route"];
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    if r["connection_id"] != id
        || entry["connection_id"] != id
        || !entry["catalog_snapshot_id"]
            .as_str()
            .is_some_and(|s| crate::config_read_model::matches(r"^cs_[a-f0-9]{12}$", s))
        || ["label", "disambiguated_label"]
            .iter()
            .any(|k| !text(&entry[k]))
        || ["model_name", "model_description"]
            .iter()
            .any(|k| !entry[k].is_null() && !text(&entry[k]))
        || (!entry["context_window_tokens"].is_null()
            && !entry["context_window_tokens"]
                .as_u64()
                .is_some_and(|n| n > 0))
    {
        return false;
    }
    let Some(facts) = entry["evidence"].as_object() else {
        return false;
    };
    for (fact, p) in facts {
        let Some(source) = p["source"].as_str() else {
            return false;
        };
        if ![
            "configured",
            "authorization_required",
            "catalog_obtained",
            "selected",
            "configuration_applied",
            "execution_verified",
        ]
        .contains(&fact.as_str())
            || ![
                "discovery",
                "manual",
                "engine_report",
                "observation",
                "seed",
            ]
            .contains(&source)
            || !text(&p["adapter_version"])
            || !text(&p["observed_at"])
            || (!p["evidence_ref"].is_null() && !text(&p["evidence_ref"]))
            || (fact == "catalog_obtained" && source == "manual")
            || (["configuration_applied", "execution_verified"].contains(&fact.as_str())
                && (!["engine_report", "observation"].contains(&source)
                    || !text(&p["evidence_ref"])))
        {
            return false;
        }
    }
    true
}
pub(in crate::machine::project_config_save) fn efforts(
    state: &Path,
    registry: &Value,
) -> Result<Value> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    efforts_at(state, registry, home.as_deref())
}
fn efforts_at(state: &Path, registry: &Value, home: Option<&Path>) -> Result<Value> {
    if !state.is_absolute()
        || state
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(refuse(
            "unsafe_storage",
            "Catalog state must use an absolute owner-scoped directory.",
        ));
    }
    for ancestor in state.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {
                return Err(refuse(
                    "unsafe_storage",
                    "Catalog state has an unsafe directory ancestor.",
                ))
            }
        }
        if home != Some(ancestor)
            && [".git", ".cortex", ".cortex-dev"]
                .iter()
                .any(|k| ancestor.join(k).symlink_metadata().is_ok())
        {
            return Err(refuse(
                "connection_storage_under_project",
                "Catalog state must remain owner-scoped, never under a project store.",
            ));
        }
    }
    let mut out = json!({});
    if let Some(records) = registry["connections"].as_object() {
        for id in records.keys() {
            let directory = state.join("catalog").join(id);
            // Never enumerate a linked/private-target directory before descriptor reads.
            // Unsafe shared roots refuse; one connection's unsafe directory only
            // withholds that connection's observations (the catalog reports it).
            let mut private = true;
            for (p, shared) in [
                (&state.to_path_buf(), true),
                (&state.join("catalog"), true),
                (&directory, false),
            ] {
                match fs::symlink_metadata(p) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Ok(m)
                        if m.is_dir()
                            && !m.file_type().is_symlink()
                            && m.uid() == unsafe { libc::getuid() }
                            && m.mode() & 0o077 == 0 => {}
                    _ if !shared => private = false,
                    _ => {
                        return Err(refuse(
                            "unsafe_storage",
                            "Catalog observations require private real directories.",
                        ))
                    }
                }
            }
            if !private {
                continue;
            }
            let mut files = match fs::read_dir(&directory) {
                Ok(d) => d.take(4097).collect::<std::io::Result<Vec<_>>>()?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if files.len() > 4096 {
                return Err(refuse(
                    "machine_reply_too_large",
                    "Catalog observations exceed the native limit.",
                ));
            }
            files.retain(|f| {
                f.file_name().to_str().is_some_and(|s| {
                    crate::config_read_model::matches(r"^cs_[a-f0-9]{12}\.json$", s)
                })
            });
            files.sort_by_key(|f| std::cmp::Reverse(f.metadata().and_then(|m| m.modified()).ok()));
            let mut connection = Value::Null;
            for file in files {
                // Invalid snapshots contribute no observations for this connection.
                let snapshot = crate::native_routes::read(&file.path(), true)
                    .ok()
                    .flatten();
                let Some(s) = snapshot else {
                    break;
                };
                let Some(entries) = s["entries"].as_array() else {
                    break;
                };
                if entries.is_empty()
                    && !s["unavailable_reason"].is_null()
                    && s["unavailable_reason"] != "no_models_observed"
                {
                    continue;
                }
                let mut valid = true;
                for entry in entries {
                    let r = &entry["route"];
                    let pairs: Vec<(String, String)> =
                        match serde_json::from_value(r["supported_parameters"].clone()) {
                            Ok(p) => p,
                            Err(_) => {
                                valid = false;
                                break;
                            }
                        };
                    if !valid_entry(entry, id)
                        || crate::native_routes::accepted_route_id(
                            &r["id"],
                            &r["engine"],
                            &r["connection_id"],
                            &r["model"],
                            pairs,
                        )
                        .is_none()
                        || !model(&r["model"])
                        || !ENGINES.iter().any(|e| r["engine"] == *e)
                    {
                        valid = false;
                        break;
                    }
                }
                if valid {
                    connection = s;
                }
                break;
            }
            if let Some(entries) = connection["entries"].as_array() {
                for entry in entries {
                    let r = &entry["route"];
                    if entry["evidence"]["catalog_obtained"].is_null()
                        || entry["evidence"]["catalog_obtained"]["source"] == "seed"
                    {
                        continue;
                    }
                    let engine = r["engine"].as_str().unwrap();
                    let name = r["model"].as_str().unwrap();
                    // A pure alias entry names no concrete model and is never offered.
                    if crate::native_routes::is_alias_model(name) {
                        continue;
                    }
                    let mut levels = r["supported_parameters"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|p| {
                            p[0] == "thought_level"
                                && p[1]
                                    .as_str()
                                    .is_some_and(crate::native_routes::is_concrete_effort)
                        })
                        .map(|p| p[1].as_str().unwrap().to_owned())
                        .collect::<Vec<_>>();
                    levels.sort_by_key(|s| {
                        (
                            EFFORTS
                                .iter()
                                .position(|e| *e == s.to_lowercase())
                                .unwrap_or(6),
                            s.to_lowercase(),
                        )
                    });
                    levels.dedup();
                    if out[engine].is_null() {
                        out[engine] = json!({});
                    }
                    let levels = json!(levels);
                    if out[engine].get(name).is_some_and(|v| *v != levels) {
                        out[engine][name] = Value::Null;
                    } else {
                        out[engine][name] = levels;
                    }
                }
            }
        }
    }
    Ok(out)
}
/// Complete read-only routing surface from explicit host-owned roots.
/// Project routing is supplied by the host after logical-project/channel resolution.
pub(crate) fn observation(
    environment: &crate::native_routes::RouteEnvironment,
    project: Option<(&Path, &Path)>,
    registry: &Value,
) -> Result<Value> {
    observation_with_user(environment, project, registry, None)
}
pub(in crate::machine::project_config_save) fn observation_with_user(
    environment: &crate::native_routes::RouteEnvironment,
    project: Option<(&Path, &Path)>,
    registry: &Value,
    user: Option<&[u8]>,
) -> Result<Value> {
    let mut paths = Vec::new();
    if let Some((_, path)) = project {
        paths.push(("project", path.to_path_buf()));
    }
    if let Some(path) = &environment.routing_override {
        paths.push(("env", path.clone()));
    }
    paths.push(("user", environment.config.join("cortex/routing.json")));
    for (_, path) in &paths {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(refuse(
                "unsafe_storage",
                "Routing observations require absolute host paths.",
            ));
        }
        for p in path.parent().into_iter().flat_map(Path::ancestors) {
            match fs::symlink_metadata(p) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
                _ => {
                    return Err(refuse(
                        "unsafe_storage",
                        "Routing observations have an unsafe directory ancestor.",
                    ))
                }
            }
        }
    }
    let sources = paths
        .iter()
        .map(|(s, p)| file_view(s, p, if *s == "user" { user } else { None }))
        .collect::<Result<Vec<_>>>()?;
    // Match the retained first-is_file rule, including invalid earlier files.
    let effective = paths
        .iter()
        .zip(&sources)
        .find(|((s, p), _)| (*s == "user" && user.is_some()) || p.is_file())
        .map(|(_, v)| v);
    let mut builtin = builtin();
    builtin["model_efforts"] = efforts_at(
        &environment.state.join("cortex"),
        registry,
        Some(&environment.home),
    )?;
    Ok(
        json!({"surface":"routing","project_root":project.map(|(root, _)|root),
        "precedence":sources.iter().map(|v|v["source"].clone()).collect::<Vec<_>>(),
        "sources":sources,"effective_source":effective.map(|v|v["source"].clone()).unwrap_or(json!("missing")),
        "effective_valid":effective.is_some_and(|v|v["valid"]==true),
        "effective_rules":effective.map(|v|v.get("rules").map(normalize).unwrap_or(Value::Null)).unwrap_or(json!({})),
        "builtin":builtin}),
    )
}
pub(super) fn routing(
    root: &Path,
    config: &crate::Config,
    replacement: Option<&[u8]>,
) -> Result<Value> {
    let folder = reader::owner_folder(config)?;
    let path = super::super::context::key(root)
        .map_err(|_| {
            refuse(
                "project_context_conflict",
                "The project store refused the request.",
            )
        })?
        .directory()
        .join("routing.json");
    // The retained service's fixed child environment does not forward CORTEX_ROUTING.
    let sources = vec![
        file_view("project", &path, replacement)?,
        file_view("user", &folder.join("routing.json"), None)?,
    ];
    let effective = if replacement.is_some() || path.is_file() {
        Some(&sources[0])
    } else if folder.join("routing.json").is_file() {
        Some(&sources[1])
    } else {
        None
    };
    let mut builtin = builtin();
    if let Some(bytes) = reader::read_plain(&folder.join("connections.json"), true)? {
        let registry: Value = if bytes.is_empty() {
            json!({"connections":{}})
        } else {
            serde_json::from_slice(&bytes).map_err(|_| {
                refuse(
                    "invalid_registry",
                    "The connection registry is not valid JSON.",
                )
            })?
        };
        if registry["connections"]
            .as_object()
            .is_some_and(|m| !m.is_empty())
        {
            builtin["model_efforts"] = efforts(&state_root(config)?, &registry)?;
        }
    }
    Ok(
        json!({"surface":"routing","project_root":root,"precedence":["project","user"],"sources":sources,
        "effective_source":effective.map(|v|v["source"].clone()).unwrap_or(json!("missing")),
        "effective_valid":effective.is_some_and(|v|v["valid"]==true),
        "effective_rules":effective.map(|v|v.get("rules").map(normalize).unwrap_or(Value::Null)).unwrap_or(json!({})),
        "builtin":builtin}),
    )
}

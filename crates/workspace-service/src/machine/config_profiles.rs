//! Revision-locked profiles and qualified four-section activation; unqualified shapes stay retained.
use super::{ensure_ignore, reader, write};
use crate::config_read_model::{self as model, management as projection};
use crate::machine::native::*;
use serde_json::{json, Value};
use std::path::Path;
#[path = "config_profiles_json.rs"]
mod profile_json;
use profile_json::Ordered as O;

#[cfg(test)]
#[path = "config_profiles_tests.rs"]
mod tests;

#[path = "config_activate.rs"]
mod activate;

const SECTIONS: [&str; 4] = ["routing", "user_defaults", "mcp_servers", "connections"];
const FILES: [&str; 4] = [
    "routing.json",
    "defaults.json",
    "mcp-servers.json",
    "connections.json",
];
const WRAPPER: [&str; 7] = [
    "expected_revision",
    "expected_revisions",
    "name",
    "rename_to",
    "description",
    "discard_modifications",
    "verb",
];

fn object(fields: Vec<(&str, O)>) -> O {
    O::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn scalar(v: impl Into<Value>) -> O {
    O::Scalar(v.into())
}
fn value(o: &O) -> Value {
    serde_json::to_value(o).unwrap()
}
fn get<'a>(o: &'a O, key: &str) -> &'a O {
    if let O::Object(m) = o {
        if let Some((_, v)) = m.iter().find(|(k, _)| k == key) {
            return v;
        }
    }
    static NULL: O = O::Scalar(Value::Null);
    &NULL
}
fn set(o: &mut O, key: &str, v: O) {
    let O::Object(m) = o else { unreachable!() };
    if let Some((_, old)) = m.iter_mut().find(|(k, _)| k == key) {
        *old = v;
    } else {
        m.push((key.into(), v));
    }
}
fn remove(o: &mut O, key: &str) {
    let O::Object(m) = o else { unreachable!() };
    m.retain(|(k, _)| k != key);
}
fn pretty(o: &O) -> Vec<u8> {
    let mut b = o.pretty();
    b.push(b'\n');
    b
}
// Inspect only, before any effect. Owner files holding NaN/Infinity, non-finite or
// beyond-64-bit numbers in modeled fields, or a routing-profiles.json whose modeled
// fields are such numbers have no exact native representation: they are refused
// naming the file, never reinterpreted or rewritten. Wider ignored profile-document
// metadata is native. The locked parser fences a file changing into an unqualified
// shape after this inspection.
pub(super) fn unrepresentable_file(config: &crate::Config) -> Option<&'static str> {
    let Ok(folder) = reader::owner_folder(config) else {
        return None;
    };
    if let Ok(Some(bytes)) = reader::read_plain(&folder.join("routing-profiles.json"), true) {
        if profile_json::parse(&bytes).is_err_and(|e| e.code == "native_config_unavailable") {
            return Some("routing-profiles.json");
        }
    }
    FILES.iter().copied().find(|file| {
        let Ok(Some(bytes)) = reader::read_plain(&folder.join(file), true) else {
            return false;
        };
        if write::user_defaults::exact_numbers(&bytes).is_err() {
            return true;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return false;
        };
        let Ok(tokens) = super::registry_json::tokens(text) else {
            return false;
        };
        tokens.iter().any(|t| {
            matches!(*t, "NaN" | "Infinity" | "-Infinity")
                || !t.starts_with('"')
                    && t.contains(['.', 'e', 'E'])
                    && t.parse::<f64>().is_ok_and(|n| !n.is_finite())
        })
    })
}
// Preset seeding uses the same capture/normalization as explicit profile creation.
pub(super) fn seed_default(bytes: &[Option<Vec<u8>>; 4], now: &str) -> Result<Option<Vec<u8>>> {
    let mut items = vec![];
    for (section, bytes) in SECTIONS.iter().zip(bytes) {
        let capture = if *section == "connections" {
            // Only identifiers and limits are captured; nothing is rewritten.
            let parsed = super::registry_json::RegistryJson::parse_lossy(bytes.as_deref())?;
            let state = super::registry::normalize(&parsed.value)?;
            let mut limits = json!({});
            for (id, r) in state["connections"].as_object().unwrap() {
                limits[id] = json!({"max_concurrent_sessions":r["max_concurrent_sessions"]});
            }
            Ok(limits)
        } else {
            capture_diagnostics(section, bytes.as_deref())
        };
        items.push(Section {
            bytes: bytes.clone(),
            read_error: None,
            observation: projection::Snapshot {
                revision: json!(write::token(bytes.as_deref())),
                capture,
            },
        });
    }
    let settings = O::Object(
        SECTIONS
            .iter()
            .zip(&items)
            .map(|(s, i)| Ok(((*s).into(), ordered_capture(s, i)?)))
            .collect::<Result<_>>()?,
    );
    let template: Value = serde_json::from_str(super::preset::TEMPLATE).unwrap();
    let expected_defaults =
        json!({"defaults":{"coordinator":template["coordinator"],"language":template["language"]}});
    let expected_mcp = json!({"servers":template["mcp_servers"]});
    if value(get(&settings, "routing")) != template["routing"]
        || value(get(&settings, "user_defaults"))
            != projection::capture(
                "user_defaults",
                Some(&serde_json::to_vec(&expected_defaults).unwrap()),
            )
            .unwrap()
        || value(get(&settings, "mcp_servers"))
            != projection::capture(
                "mcp_servers",
                Some(&serde_json::to_vec(&expected_mcp).unwrap()),
            )
            .unwrap()
    {
        return Ok(None);
    }
    let doc = object(vec![
        ("format", scalar(2)),
        ("active", scalar("default")),
        ("pending", scalar(Value::Null)),
        (
            "applied",
            object(vec![
                ("profile", scalar("default")),
                ("at", scalar(now)),
                ("revisions", revisions(&items)),
            ]),
        ),
        (
            "profiles",
            object(vec![(
                "default",
                object(vec![
                    ("description", scalar("Bundled default settings")),
                    ("created_at", scalar(now)),
                    ("updated_at", scalar(now)),
                    ("sections", O::from_value(&json!(SECTIONS))),
                    ("settings", settings),
                ]),
            )]),
        ),
    ]);
    let body = pretty(&doc);
    parse(Some(&body))?;
    Ok(Some(body))
}
fn migration_backup(
    folder: &Path,
    before: &[u8],
    now: &str,
    persist: &mut impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let directory = folder.join(".migration-backups");
    crate::local::private_tree(&directory)?;
    let stamp = now
        .replace(['-', ':', '.'], "")
        .replace("+0000", "Z")
        .replace("+00:00", "Z");
    let backup = directory.join(format!("routing-profiles.format-1.{stamp}.json"));
    if backup.symlink_metadata().is_ok() {
        return Err(refuse(
            "unsafe_storage",
            "A profile migration backup already exists; nothing was replaced.",
        ));
    }
    persist(&backup, before)
}
fn valid_revision(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s == "absent"
            || s.len() == 71
                && s.starts_with("sha256:")
                && s[7..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn expected(p: &Value) -> Result<&str> {
    if !valid_revision(&p["expected_revision"]) {
        return Err(refuse("invalid_expected_revision", "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>')."));
    }
    Ok(p["expected_revision"].as_str().unwrap())
}
fn name(v: &Value) -> Result<&str> {
    v.as_str()
        .filter(|s| {
            regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,39}$")
                .unwrap()
                .is_match(s)
        })
        .ok_or_else(|| {
            refuse(
                "invalid_profile_name",
                "Profile name must be 1-40 lowercase letters, digits or hyphens.",
            )
        })
}
fn description(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.chars().count() <= 256 && !s.chars().any(|c| c < ' ' || c == '\u{7f}'))
}
fn check_description(v: &Value) -> Result<()> {
    if description(v) {
        Ok(())
    } else {
        Err(refuse(
            "invalid_profile_description",
            "Description must be at most 256 printable characters.",
        ))
    }
}
fn invalid(message: &str) -> Refused {
    refuse("invalid_profiles", message)
}

fn normalize_settings(raw: &O, normalized: &Value) -> O {
    O::Object(
        SECTIONS
            .iter()
            .filter_map(|s| {
                normalized.get(*s).map(|v| {
                    let original = get(raw, s);
                    let out = if *s == "user_defaults" {
                        object(vec![
                            ("defaults", get(original, "defaults").clone()),
                            ("ceiling", O::from_value(&v["ceiling"])),
                        ])
                    } else {
                        original.clone()
                    };
                    ((*s).into(), out)
                })
            })
            .collect(),
    )
}
fn parse(bytes: Option<&[u8]>) -> Result<(O, Value)> {
    let Some(bytes) = bytes else {
        let doc = object(vec![
            ("format", scalar(2)),
            ("active", scalar(Value::Null)),
            ("applied", scalar(Value::Null)),
            ("pending", scalar(Value::Null)),
            ("profiles", O::Object(vec![])),
        ]);
        return Ok((
            doc.clone(),
            json!({"document":value(&doc),"migrated_from":null}),
        ));
    };
    let raw = profile_json::parse(bytes)?;
    if !raw.is_object() {
        return Err(invalid("routing-profiles.json must contain a JSON object; repair it by hand. Nothing was changed."));
    }
    let input = value(&raw);
    let version=input["format"].as_u64().filter(|n| *n==1 || *n==2).ok_or_else(||
        refuse("unsupported_profiles_format","routing-profiles.json format must be 2 (or 1, which is migrated on the first change)."))?;
    let entries = input["profiles"]
        .as_object()
        .ok_or_else(|| invalid("routing-profiles.json has invalid profile metadata."))?;
    if entries.len() > 32
        || entries
            .iter()
            .any(|(k, v)| name(&json!(k)).is_err() || !v.is_object())
        || !input["active"].is_null()
            && !input["active"]
                .as_str()
                .is_some_and(|n| entries.contains_key(n))
        || !input["applied"].is_null() && !input["applied"].is_object()
    {
        return Err(invalid(
            "routing-profiles.json has invalid profile metadata.",
        ));
    }
    for e in entries.values() {
        if version == 1 {
            if !e["rules"].is_object() || !description(&e["description"]) {
                return Err(invalid(
                    "A routing profile has invalid rules or description.",
                ));
            }
        } else {
            let sections = e["sections"].as_array();
            if !description(&e["description"])
                || ["created_at", "updated_at"]
                    .iter()
                    .any(|k| !e[k].is_null() && !e[k].is_string())
                || e.as_object().unwrap().keys().any(|k| {
                    ![
                        "description",
                        "created_at",
                        "updated_at",
                        "sections",
                        "settings",
                    ]
                    .contains(&k.as_str())
                })
                || sections.is_none_or(|a| {
                    a.is_empty()
                        || a.iter()
                            .any(|s| !s.as_str().is_some_and(|s| SECTIONS.contains(&s)))
                        || a.iter()
                            .map(Value::to_string)
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            != a.len()
                })
                || !e["settings"].as_object().is_some_and(|m| {
                    sections.is_some_and(|a| {
                        m.len() == a.len() && m.keys().all(|k| a.contains(&json!(k)))
                    })
                })
            {
                return Err(invalid(
                    "A profile has an invalid description, sections or settings.",
                ));
            }
        }
    }
    let parsed = model::parse_profiles(Some(&input)).map_err(|_| {
        if !input["pending"].is_null()
            && version == 2
            && (!input["pending"].is_object()
                || input["pending"]["operation"] != "activate"
                || !input["pending"]["profile"]
                    .as_str()
                    .is_some_and(|n| entries.contains_key(n))
                || !input["pending"]["at"].is_string())
        {
            invalid("routing-profiles.json has an invalid pending activation.")
        } else {
            invalid("A profile holds an invalid setting; repair or delete that profile.")
        }
    })?;
    let normalized = &parsed["document"];
    let mut doc = if version == 1 {
        object(vec![
            ("format", scalar(2)),
            ("active", O::from_value(&normalized["active"])),
            (
                "applied",
                if normalized["applied"].is_null() {
                    scalar(Value::Null)
                } else {
                    object(vec![
                        ("profile", O::from_value(&normalized["applied"]["profile"])),
                        ("at", O::from_value(&normalized["applied"]["at"])),
                        (
                            "revisions",
                            object(vec![(
                                "routing",
                                O::from_value(&normalized["applied"]["revisions"]["routing"]),
                            )]),
                        ),
                    ])
                },
            ),
            ("pending", scalar(Value::Null)),
        ])
    } else {
        let mut d = raw.clone();
        remove(&mut d, "profiles");
        d
    };
    let O::Object(original) = get(&raw, "profiles") else {
        unreachable!()
    };
    let mut converted = vec![];
    for (n, e) in original {
        let norm = &normalized["profiles"][n];
        let mut out = if version == 1 {
            let mut out = object(vec![("description", get(e, "description").clone())]);
            for k in ["created_at", "updated_at"] {
                if input["profiles"][n][k].is_string() {
                    set(&mut out, k, get(e, k).clone());
                }
            }
            set(&mut out, "sections", O::from_value(&norm["sections"]));
            set(
                &mut out,
                "settings",
                object(vec![("routing", get(e, "rules").clone())]),
            );
            out
        } else {
            let mut out = e.clone();
            set(&mut out, "sections", O::from_value(&norm["sections"]));
            set(
                &mut out,
                "settings",
                normalize_settings(get(e, "settings"), &norm["settings"]),
            );
            out
        };
        // Format-1 routing is normalized/validated but never replaced by a template.
        if version == 1 {
            set(&mut out, "sections", O::from_value(&norm["sections"]));
        }
        converted.push((n.clone(), out));
    }
    set(&mut doc, "profiles", O::Object(converted));
    for k in ["pending", "applied", "active"] {
        if value(&doc).get(k).is_none() {
            set(&mut doc, k, scalar(Value::Null));
        }
    }
    Ok((doc, parsed))
}

// Action diagnostics retain management messages; passive display deliberately uses
// coarse diagnostics and is not a drop-in action provider.
fn capture_diagnostics(section: &str, bytes: Option<&[u8]>) -> std::result::Result<Value, Value> {
    let outcome = projection::capture(section, bytes);
    let Err(e) = &outcome else { return outcome };
    if e["code"] != "invalid_configuration" {
        return outcome;
    }
    let raw: Value = bytes
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(json!({}));
    let mut errors: Vec<(String, String)> = vec![];
    if section == "mcp_servers" {
        let servers = if raw.get("servers").is_some() {
            &raw["servers"]
        } else {
            &raw["mcpServers"]
        };
        if servers.is_object() {
            // Capture order is file order, not lexical order.
            let ordered: O = serde_json::from_slice(bytes.unwrap()).unwrap();
            let key = if raw.get("servers").is_some() {
                "servers"
            } else {
                "mcpServers"
            };
            if let O::Object(entries) = get(&ordered, key) {
                for (n, _) in entries {
                    if n == "cortex" || !model::matches(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$", n) {
                        errors.push((
                            format!("settings.mcp_servers.{n}"),
                            "must be a configured server name other than 'cortex'".into(),
                        ));
                    }
                }
            }
        }
    } else if section == "user_defaults" {
        let defaults = &raw["defaults"];
        for k in [
            "coordinator",
            "route_id",
            "route_connection_id",
            "route_model",
            "language",
            "parameters",
        ] {
            let v = &defaults[k];
            if v.is_null() {
                continue;
            }
            let field = format!("settings.user_defaults.defaults.{k}");
            let mut add = |message: &str| errors.push((field.clone(), message.into()));
            match k {
                "coordinator" => {
                    if !v.is_object()
                        || v.as_object().unwrap().len() != 3
                        || !v["engine"]
                            .as_str()
                            .is_some_and(|s| model::ENGINES.contains(&s))
                        || !v["model"].as_str().is_some_and(|s| !s.trim().is_empty())
                        || !v["parameters"].as_object().is_some_and(|m| {
                            m.iter().all(|(k, v)| {
                                !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty())
                            })
                        })
                    {
                        add("Use exact engine/model and a parameter string mapping.");
                    }
                }
                "route_id"
                    if !v
                        .as_str()
                        .is_some_and(|s| model::matches(r"^xr_[a-f0-9]{12,64}$", s)) =>
                {
                    add("must be an observed route id (xr_<hex>)")
                }
                "route_connection_id"
                    if !v
                        .as_str()
                        .is_some_and(|s| model::matches(r"^cn_[a-f0-9]{12}$", s)) =>
                {
                    add("must be a connection id (cn_<12 hex>)")
                }
                "route_model" if !model::clean(v, 256) => add("must be the route's exact model id"),
                "language" if v != "en" && v != "ru" => add("must be null (auto) or one of en, ru"),
                "parameters" => {
                    if !v.is_object() {
                        add("must be an object of parameter name -> value");
                    } else {
                        let ordered: O = serde_json::from_slice(bytes.unwrap()).unwrap();
                        if let O::Object(m) = get(get(&ordered, "defaults"), "parameters") {
                            for (n, item) in m {
                                if !model::matches(r"^[a-z][a-z0-9_]{0,63}$", n)
                                    || !model::clean(&value(item), 128)
                                {
                                    errors.push((format!("{field}.{n}"),"parameter names are lowercase identifiers and values non-empty strings".into()));
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        }
        let present = ["route_id", "route_connection_id", "route_model"]
            .iter()
            .filter(|k| defaults.get(**k).is_some())
            .count();
        if !defaults["coordinator"].is_null() && present > 0 {
            errors.push((
                "settings.user_defaults".into(),
                "portable coordinator and bound route are mutually exclusive".into(),
            ));
        }
        if present > 0 && present != 3 {
            errors.push((
                "settings.user_defaults.defaults".into(),
                "route_id, route_connection_id and route_model are stored together".into(),
            ));
        }
    }
    if errors.is_empty() {
        return outcome;
    }
    let shown = errors
        .iter()
        .take(5)
        .map(|(f, m)| format!("{f}: {m}"))
        .collect::<Vec<_>>()
        .join("; ");
    let more = if errors.len() > 5 {
        format!(" (and {} more)", errors.len() - 5)
    } else {
        String::new()
    };
    Err(
        json!({"code":"invalid_configuration","message":format!("Correct the invalid setting(s); nothing was written. {shown}{more}")}),
    )
}
struct Section {
    bytes: Option<Vec<u8>>,
    read_error: Option<Refused>,
    observation: projection::Snapshot,
}
fn snapshot(folder: &Path) -> Vec<Section> {
    SECTIONS.iter().zip(FILES).map(|(section,file)| match reader::read_plain(&folder.join(file),true) {
        Ok(bytes) => {
            let capture=if *section=="connections" {
                let parsed=super::registry_json::RegistryJson::parse(bytes.as_deref());
                parsed.and_then(|p|super::registry::normalize(&p.value)).map(|r| {
                    let mut out=json!({});
                    for (id,record) in r["connections"].as_object().unwrap() {
                        out[id]=json!({"max_concurrent_sessions":record["max_concurrent_sessions"]});
                    } out
                }).map_err(|e|json!({"code":e.code,"message":e.message}))
            } else if let Some(e)=bytes.as_deref().and_then(|b|write::user_defaults::exact_numbers(b).err()) {
                Err(json!({"code":e.code,"message":e.message}))
            } else {capture_diagnostics(section,bytes.as_deref())};
            Section { observation:projection::Snapshot {revision:json!(write::token(bytes.as_deref())),capture},bytes,read_error:None }
        },
        Err(e) => Section {bytes:None,read_error:Some(e),observation:projection::Snapshot {
            revision:Value::Null, capture:Err(json!({"code":"unsafe_storage","message":"The current section is not a safe readable owner file."})) }}
    }).collect()
}
fn revisions(items: &[Section]) -> O {
    O::Object(
        SECTIONS
            .iter()
            .zip(items)
            .map(|(s, i)| ((*s).into(), O::from_value(&i.observation.revision)))
            .collect(),
    )
}
fn ordered_capture(section: &str, item: &Section) -> Result<O> {
    let v=item.observation.capture.as_ref().map_err(|e| refuse("profile_section_unavailable",format!(
        "The current {} settings cannot be stored in a profile ({}); repair them first. Nothing was written.",
        if section=="routing" {"routing (user)"} else {section},e["code"].as_str().unwrap()))
        .with(json!({"field":format!("settings.{section}")})))?;
    let raw = item
        .bytes
        .as_ref()
        .and_then(|b| serde_json::from_slice::<O>(b).ok())
        .unwrap_or(O::Object(vec![]));
    let keys = |raw: &O, keys: &[&str]| {
        O::Object(
            keys.iter()
                .filter_map(|k| v.get(*k).map(|_| ((*k).into(), get(raw, k).clone())))
                .collect(),
        )
    };
    Ok(match section {
        "routing" => keys(
            &raw,
            &[
                "model_policy_version",
                "pin",
                "deny",
                "enabled",
                "auto_allow",
                "tiers",
                "providers",
                "engine_affinity",
                "models",
                "quota_windows",
                "coordinator_models",
            ],
        ),
        "user_defaults" => {
            let mut defaults = O::Object(vec![]);
            for k in [
                "coordinator",
                "route_id",
                "route_connection_id",
                "route_model",
                "language",
                "parameters",
            ] {
                if v["defaults"].get(k).is_some() {
                    set(&mut defaults, k, get(get(&raw, "defaults"), k).clone());
                }
            }
            object(vec![
                ("defaults", defaults),
                ("ceiling", O::from_value(&v["ceiling"])),
            ])
        }
        "mcp_servers" => {
            let servers = if value(&raw).get("servers").is_some() {
                get(&raw, "servers")
            } else {
                get(&raw, "mcpServers")
            };
            let mut out = vec![];
            if let O::Object(m) = servers {
                for (n, spec) in m {
                    let mut policy = O::Object(vec![]);
                    for k in [
                        "allowed_tools",
                        "read_only_tools",
                        "coordinator_tools",
                        "always_load_tools",
                        "read_paths",
                        "write_paths",
                        "network",
                        "runtime_dir_env",
                        "roles",
                        "engines",
                    ] {
                        if v[n].get(k).is_some() {
                            set(&mut policy, k, get(spec, k).clone());
                        }
                    }
                    set(&mut policy, "enabled", O::from_value(&v[n]["enabled"]));
                    out.push((n.clone(), policy));
                }
            }
            O::Object(out)
        }
        _ => O::from_value(v),
    })
}
fn view(path: &Path, bytes: Option<&[u8]>, parsed: &Value, items: Vec<Section>) -> Value {
    let current: Vec<_> = items.into_iter().map(|i| i.observation).collect();
    // The retained machine child filters out CORTEX_ROUTING, and its wrapper
    // drops project. Preserve that machine-global shadow contract, not the
    // wider host-only reader environment.
    projection::profile_view(path, bytes, parsed, &current, Value::Null)["result"].clone()
}
pub(super) fn action(config: &crate::Config, p: &Value) -> Result<Value> {
    if let Some(file) = unrepresentable_file(config) {
        return Err(refuse(
            "invalid_settings",
            format!("{file} holds non-finite (NaN/Infinity) or beyond-64-bit numbers that Cortex cannot represent exactly. Repair or remove those values in {file}; nothing was changed."),
        ));
    }
    action_with(config, p, &cortex_core::now(), write::replace)
}
fn action_with(
    config: &crate::Config,
    p: &Value,
    now: &str,
    mut persist: impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let verb = p["verb"].as_str().unwrap_or("");
    let allowed: &[&str] = match verb {
        "read" => &[],
        "create" => &[
            "expected_revision",
            "expected_revisions",
            "name",
            "description",
        ],
        "save" => &["expected_revision", "expected_revisions", "name"],
        "activate" => &[
            "expected_revision",
            "expected_revisions",
            "name",
            "discard_modifications",
        ],
        "update" => &["expected_revision", "name", "rename_to", "description"],
        "delete" => &["expected_revision", "name"],
        "deactivate" => &["expected_revision"],
        _ => {
            return Err(refuse(
                "unsupported_action",
                "Unknown routing profile operation.",
            ))
        }
    };
    let unknown: Vec<_> = WRAPPER
        .iter()
        .filter(|k| **k != "verb" && p.get(**k).is_some() && !allowed.contains(k))
        .copied()
        .collect();
    if !unknown.is_empty() {
        let mut unknown = unknown;
        unknown.sort();
        return Err(refuse(
            "unsupported_request_field",
            format!(
                "Unknown request field(s) {} for this action.",
                unknown.join(", ")
            ),
        ));
    }
    let folder = reader::owner_folder(config)?;
    let path = folder.join("routing-profiles.json");
    if verb == "read" {
        let bytes = reader::read_plain(&path, true)?;
        let (_, parsed) = parse(bytes.as_deref())?;
        return Ok(view(&path, bytes.as_deref(), &parsed, snapshot(&folder)));
    }
    let expected = expected(p)?;
    if matches!(verb, "create" | "save" | "activate")
        && !p["expected_revisions"].as_object().is_some_and(|m| {
            m.len() == 4
                && SECTIONS
                    .iter()
                    .all(|s| m.get(*s).is_some_and(valid_revision))
        })
    {
        return Err(refuse("invalid_expected_revisions","expected_revisions must hold the revision a read returned for each of routing, user_defaults, mcp_servers, connections ('absent' or 'sha256:<64 hex>')."));
    }
    if p.get("discard_modifications")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(refuse(
            "invalid_value",
            "discard_modifications must be true or false.",
        ));
    }
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let _lock = write::owner_lock(&folder)?;
    let before = reader::read_plain(&path, true)?;
    let (mut doc, parsed) = parse(before.as_deref())?;
    write::check(
        "routing_profiles",
        expected,
        &write::token(before.as_deref()),
    )?;
    let dv = value(&doc);
    let n = if matches!(verb, "create" | "deactivate") {
        ""
    } else {
        let n = name(&p["name"])?;
        if dv["profiles"].get(n).is_none() {
            return Err(refuse("profile_not_found", "No profile has that name."));
        }
        n
    };
    let items = snapshot(&folder);
    if matches!(verb, "create" | "save" | "activate") {
        for (s, item) in SECTIONS.iter().zip(&items) {
            if let Some(e) = &item.read_error {
                return Err(refuse(&e.code, &e.message));
            }
            write::check(
                if *s == "routing" { "routing (user)" } else { s },
                p["expected_revisions"][*s].as_str().unwrap(),
                item.observation.revision.as_str().unwrap(),
            )?;
        }
    }
    let pending = dv["pending"]["profile"].as_str();
    match verb {
        "activate" => {
            return activate::activate(
                &folder,
                &path,
                before.as_deref(),
                doc,
                &parsed,
                items,
                p,
                now,
                &mut persist,
            )
        }
        "create" | "save" => {
            let n = if verb == "create" {
                let n = name(&p["name"])?;
                if dv["profiles"].get(n).is_some() {
                    return Err(refuse(
                        "profile_exists",
                        "A profile with that name already exists.",
                    ));
                }
                if dv["profiles"].as_object().unwrap().len() >= 32 {
                    return Err(refuse(
                        "too_many_profiles",
                        "At most 32 profiles are supported.",
                    ));
                }
                if !p["description"].is_null() {
                    check_description(&p["description"])?;
                }
                n
            } else {
                if dv["active"].is_null() && pending.is_none() {
                    return Err(refuse(
                        "no_active_profile",
                        "No profile is selected; create a new profile instead.",
                    ));
                }
                if dv["active"] != n && pending != Some(n) {
                    return Err(refuse("profile_not_active","Only the selected profile can be saved; select it first or save as a new profile."));
                }
                n
            };
            let captured = O::Object(
                SECTIONS
                    .iter()
                    .zip(&items)
                    .map(|(s, i)| Ok(((*s).into(), ordered_capture(s, i)?)))
                    .collect::<Result<_>>()?,
            );
            let mut e = if verb == "create" {
                object(vec![
                    (
                        "description",
                        scalar(if p["description"].is_null() {
                            json!("")
                        } else {
                            p["description"].clone()
                        }),
                    ),
                    ("created_at", scalar(now)),
                    ("updated_at", scalar(now)),
                    ("sections", O::from_value(&json!(SECTIONS))),
                    ("settings", captured.clone()),
                ])
            } else {
                get(get(&doc, "profiles"), n).clone()
            };
            set(&mut e, "sections", O::from_value(&json!(SECTIONS)));
            set(&mut e, "settings", captured);
            set(&mut e, "updated_at", scalar(now));
            let mut entries = get(&doc, "profiles").clone();
            set(&mut entries, n, e);
            set(&mut doc, "profiles", entries);
            set(&mut doc, "active", scalar(n));
            set(
                &mut doc,
                "applied",
                object(vec![
                    ("profile", scalar(n)),
                    ("at", scalar(now)),
                    ("revisions", revisions(&items)),
                ]),
            );
            set(&mut doc, "pending", scalar(Value::Null));
        }
        "update" => {
            let target = if !p["rename_to"].is_null() {
                name(&p["rename_to"])?
            } else {
                n
            };
            if target != n && dv["profiles"].get(target).is_some() {
                return Err(refuse(
                    "profile_exists",
                    "A profile with that name already exists.",
                ));
            }
            if !p["description"].is_null() {
                check_description(&p["description"])?;
            }
            let mut e = get(get(&doc, "profiles"), n).clone();
            if !p["description"].is_null() {
                set(&mut e, "description", O::from_value(&p["description"]));
            }
            set(&mut e, "updated_at", scalar(now));
            let mut entries = get(&doc, "profiles").clone();
            remove(&mut entries, n);
            set(&mut entries, target, e);
            if let O::Object(m) = &mut entries {
                m.sort_by(|a, b| a.0.cmp(&b.0));
            }
            set(&mut doc, "profiles", entries);
            if dv["active"] == n {
                set(&mut doc, "active", scalar(target));
            }
            for k in ["applied", "pending"] {
                if dv[k]["profile"] == n {
                    let mut item = get(&doc, k).clone();
                    set(&mut item, "profile", scalar(target));
                    set(&mut doc, k, item);
                }
            }
        }
        "delete" => {
            if !dv["pending"].is_null() {
                return Err(refuse(
                    "profile_interrupted",
                    "Resolve the interrupted activation before deleting a profile.",
                ));
            }
            if dv["active"] == n {
                set(&mut doc, "active", scalar(Value::Null));
                set(&mut doc, "applied", scalar(Value::Null));
            }
            let mut entries = get(&doc, "profiles").clone();
            remove(&mut entries, n);
            set(&mut doc, "profiles", entries);
        }
        "deactivate" => {
            for k in ["active", "applied", "pending"] {
                set(&mut doc, k, scalar(Value::Null));
            }
        }
        _ => unreachable!(),
    }
    let body = pretty(&doc);
    // Validate and budget the complete candidate reply before any replacement.
    let (_, after_parsed) = parse(Some(&body))?;
    let mut reply = view(&path, Some(&body), &after_parsed, items);
    reply["changed"] = json!(true);
    if parsed["migrated_from"] == 1 {
        let mut notes = vec![json!("migrated_from_format_1")];
        notes.extend(reply["notes"].as_array().unwrap().iter().cloned());
        reply["notes"] = json!(notes);
    }
    if body.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    write::check(
        "routing_profiles",
        &write::token(before.as_deref()),
        &write::token(reader::read_plain(&path, true)?.as_deref()),
    )?;
    if parsed["migrated_from"] == 1 {
        let backup_dir = folder.join(".migration-backups");
        crate::local::private_tree(&backup_dir)?;
        let stamp = now
            .replace(['-', ':', '.'], "")
            .replace("+0000", "Z")
            .replace("+00:00", "Z");
        let backup = backup_dir.join(format!("routing-profiles.format-1.{stamp}.json"));
        if backup.symlink_metadata().is_ok() {
            return Err(refuse(
                "unsafe_storage",
                "A profile migration backup already exists; nothing was replaced.",
            ));
        }
        persist(&backup, before.as_ref().unwrap())?;
    }
    if let Err(e) = persist(&path, &body) {
        if e.code == "machine_outcome_unknown" {
            return Err(e);
        }
        if verb == "delete" {
            return Err(refuse(
                "profile_delete_failed",
                "Profile deletion failed; original snapshot bytes retained or restored.",
            ));
        }
        return Err(e);
    }
    // Migration and deletion verify their own single-file effect. Never replay
    // an uncertain replace, and never turn a failed restoration into success.
    if parsed["migrated_from"] == 1 || verb == "delete" {
        let verified =
            reader::read_plain(&path, true).is_ok_and(|b| b.as_deref() == Some(body.as_slice()));
        let preserves = if parsed["migrated_from"] == 1 {
            let original = value(&profile_json::parse(before.as_ref().unwrap())?);
            let after = value(&doc);
            original["profiles"]
                .as_object()
                .unwrap()
                .iter()
                .all(|(key, e)| {
                    if matches!(verb, "save" | "update" | "delete") && key == n {
                        return true;
                    }
                    after["profiles"].get(key).is_some_and(|got| {
                        got["settings"]["routing"] == e["rules"]
                            && got["description"] == e["description"]
                            && got["created_at"] == e["created_at"]
                    })
                })
                && (verb != "delete"
                    || original["active"] == n
                    || after["active"] == original["active"])
        } else {
            true
        };
        if !verified || !preserves {
            if let Some(original) = before.as_deref() {
                persist(&path,original).map_err(|_|refuse("profile_rollback_failed","Profile verification and rollback failed; inspect the private original or migration backup."))?;
            }
            return Err(refuse(
                if verb == "delete" {
                    "profile_delete_failed"
                } else {
                    "profile_migration_failed"
                },
                if verb == "delete" {
                    "Profile deletion failed; original snapshot bytes retained or restored."
                } else {
                    "Migrating routing-profiles.json to format 2 could not be verified; the original file was restored and a private backup kept in .migration-backups. Nothing was changed."
                },
            ));
        }
    }
    Ok(reply)
}

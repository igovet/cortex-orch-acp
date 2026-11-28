//! Pure management-to-Workspace display projection. Inputs are trusted, sanitized
//! observations, not client requests or evidence of route authorization.
use super::ModelResult;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const ENGINES: [&str; 3] = ["claude-agent-acp", "codex-acp", "opencode"];
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
fn or(v: &Value, fallback: Value) -> Value {
    if truth(v) {
        v.clone()
    } else {
        fallback
    }
}
fn rows(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}
fn pairs(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    v.as_object().into_iter().flatten()
}
fn revision(v: &Value) -> u64 {
    v.as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| {
            s.len() >= 12
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .and_then(|s| u64::from_str_radix(&s[..12], 16).ok())
        .unwrap_or(0)
}

/// Runtime timestamps are UTC ISO dates or datetimes, optionally with an offset.
/// No local timezone/environment is consulted by the display projection.
fn iso_ms(v: &Value) -> Value {
    let Some(s) = v.as_str() else {
        return Value::Null;
    };
    static ISO: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ISO.get_or_init(|| regex::Regex::new(
        r"^([0-9]{4})-([0-9]{2})-([0-9]{2})(?:[T ]([0-9]{2}):([0-9]{2}):([0-9]{2})(?:[.,]([0-9]{1,6}))?(Z|[+-][0-9]{2}:[0-9]{2})?)?$").unwrap());
    let Some(c) = re.captures(s) else {
        return Value::Null;
    };
    let n = |i| {
        c.get(i)
            .and_then(|s| s.as_str().parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (year, month, day) = (n(1), n(2), n(3));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if year < 1 || day < 1 || day > max || n(4) > 23 || n(5) > 59 || n(6) > 59 {
        return Value::Null;
    }
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    t.tm_year = (year - 1900) as i32;
    t.tm_mon = (month - 1) as i32;
    t.tm_mday = day as i32;
    t.tm_hour = n(4) as i32;
    t.tm_min = n(5) as i32;
    t.tm_sec = n(6) as i32;
    let seconds = unsafe { libc::timegm(&mut t) };
    let mut offset = 0;
    if let Some(z) = c.get(8).map(|s| s.as_str()).filter(|s| *s != "Z") {
        let h = z[1..3].parse::<i64>().unwrap();
        let m = z[4..6].parse::<i64>().unwrap();
        if h > 23 || m > 59 {
            return Value::Null;
        }
        offset = (h * 3600 + m * 60) * if z.starts_with('-') { -1 } else { 1 };
    }
    let fraction = c
        .get(7)
        .map(|s| format!("{:0<6}", s.as_str()).parse::<i64>().unwrap())
        .unwrap_or(0);
    // Match Python int(timestamp * 1000), including truncation toward zero.
    json!((((seconds - offset) as f64 + fraction as f64 / 1_000_000.0) * 1000.0) as i64)
}
fn reset_time(v: &Value) -> Value {
    let Some(n) = v.as_f64().filter(|n| *n != 0.0 && n.is_finite()) else {
        return Value::Null;
    };
    if n < i64::MIN as f64 || n >= i64::MAX as f64 {
        return Value::Null;
    }
    let seconds = n.floor() as libc::time_t;
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::gmtime_r(&seconds, &mut t) }.is_null() {
        return Value::Null;
    }
    json!(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        i64::from(t.tm_year) + 1900,
        t.tm_mon + 1,
        t.tm_mday,
        t.tm_hour,
        t.tm_min,
        t.tm_sec
    ))
}
fn routing(r: &Value) -> Value {
    let mut engines = json!({});
    for e in ENGINES {
        let c = &r["models"][e];
        let mut policy = json!({});
        for (m, p) in pairs(&c["policy"]) {
            policy[m] = json!({"effortFloor":p["effort_floor"],"maxEffort":p["max_effort"],
                "effortLadder":p["effort_ladder"],"alsoStandardFor":p["also_standard_for"],
                "preferredFor":p["preferred_for"],"preferredWhen":p["preferred_when"],"requestNames":p["request_names"]});
            if !p["tier_efforts"].is_null() {
                policy[m]["tierEfforts"] = p["tier_efforts"].clone();
            }
        }
        let mut tiers = json!({});
        for (m, t) in pairs(&c["tiers"]) {
            tiers[m] = if t.is_array() { t.clone() } else { json!([t]) };
        }
        engines[e] = json!({"enabled":or(&r["enabled"][e],json!([])),"auto":or(&c["auto"],json!([])),
            "tiers":tiers,"policy":policy,"quota":or(&r["quota_windows"][e],json!([]))});
    }
    let mut pins = json!({});
    for (role, v) in pairs(&r["pin"]) {
        let s = v.as_str().unwrap_or("");
        let (route, effort) = s.split_once(':').unwrap_or((s, ""));
        let (engine, model) = route.split_once('/').unwrap_or((route, ""));
        pins[role] = json!({"engine":engine,"model":model,"effort":if effort.is_empty(){Value::Null}else{json!(effort)}});
    }
    let legacy: Vec<_> = pairs(&r["tiers"]).map(|(k, _)| k).collect();
    let mut view = json!({"version":or(&r["model_policy_version"],json!(2)),"deny":or(&r["deny"],json!([])),
        "providers":or(&r["providers"],json!(ENGINES)),"engines":engines,"pins":pins,
        "affinity":or(&r["engine_affinity"],json!({})),"autoAllow":or(&r["auto_allow"],json!([])),"legacyTiers":legacy});
    // The coordinator list only when the profile has one (legacy profiles fall back to worker tiers).
    if r[crate::native_routes::COORDINATOR_MODELS].is_array() {
        view["coordinatorModels"] = r[crate::native_routes::COORDINATOR_MODELS].clone();
    }
    view
}
fn ceiling(r: &Value) -> Value {
    let f = &r["fallback_policy"];
    let l = &r["limits"];
    json!({"allowedRoutes":r["allowed_routes"],"allowedConnections":r["allowed_connections"],"allowedRecipients":r["allowed_data_recipients"],
        "attempts":l["attempts"],"failureClasses":f["allowed_failure_classes"],"targetRoutes":f["allowed_target_routes"],
        "budget":f["budget"],"consent":truth(&f["data_recipient_consent"]),
        "stored":{"permissionLevel":r["permission_level"],"calls":l["calls"],"tokens":l["tokens"],"concurrency":l["concurrency"],
            "spendLimit":r["spend_limit"],"spendEnforceable":truth(&r["spend_limit_enforceable"])}})
}
fn defaults(v: &Value, workspace: &Value) -> Value {
    let r = &v["defaults"];
    let c = &r["coordinator"];
    let parameters = or(&c["parameters"], json!({}));
    let choice = if c.is_object() {
        json!({"engine":c["engine"],"model":c["model"],"effort":or(&c["effort"],or(&parameters["thought_level"],json!(""))),"parameters":parameters})
    } else {
        Value::Null
    };
    let route = if truth(&r["route_id"]) {
        json!({"routeId":r["route_id"],"connectionId":r["route_connection_id"],"model":r["route_model"],"parameters":or(&r["parameters"],json!({}))})
    } else {
        Value::Null
    };
    json!({"coordinator":choice,"routeDefault":route,"language":or(&r["language"],json!("auto")),
        "mode":workspace.get("mode").unwrap_or(&json!("guarded")),"checkout":workspace.get("checkout").unwrap_or(&json!("main")),
        "ceiling":ceiling(&v["ceiling"]),"preserved":or(&v["preserved_keys"],json!([]))})
}
pub(super) fn mcp(v: &Value) -> Value {
    json!(rows(&v["servers"]).map(|e| {
        let c = &e["config"];
        let env: Vec<_> = rows(&c["env"]).map(|v| json!({"name":v["name"],"stored":truth(&v["value_set"]),"action":"keep","value":""})).collect();
        let mut row = json!({"name":e["name"],"command":or(&c["command"],json!("")),"args":or(&c["args"],json!([])),
            "enabled":e["enabled"]==true,"env":env,"roles":c["roles"],"engines":c["engines"],
            "allowedTools":or(&c["allowed_tools"],json!([])),"readOnlyTools":or(&c["read_only_tools"],json!([])),
            "coordinatorTools":c["coordinator_tools"],"alwaysLoadTools":c["always_load_tools"],
            "readPaths":or(&c["read_paths"],json!([])),"writePaths":or(&c["write_paths"],json!([])),
            "network":truth(&c["network"]),"runtimeDirEnv":or(&c["runtime_dir_env"],json!([])),"handTools":[],
            "unsupportedKeys":or(&e["unsupported_keys"],json!([])),"problem":e["problem"]["message"]});
        // Effective opt-out (own entry, else the registry default); absent when contained.
        if c.is_object() && super::mcp::uncontained(c, &v[super::mcp::CONTAINMENT]) {
            row["containment"] = json!("none");
        }
        row
    }).collect::<Vec<_>>())
}
fn sections(s: &Value, workspace: &Value) -> Value {
    let mut result = json!({});
    if s.get("routing").is_some() {
        result["routing"] = routing(&s["routing"]);
    }
    if s.get("user_defaults").is_some() {
        result["defaults"] = defaults(&s["user_defaults"], workspace);
    }
    if s.get("mcp_servers").is_some() {
        let servers: Vec<_> = pairs(&s["mcp_servers"]).map(|(name,spec)| json!({"name":name,"enabled":spec.get("enabled").unwrap_or(&json!(true)),"config":spec})).collect();
        let mut servers = mcp(&json!({"servers":servers}));
        for server in servers.as_array_mut().unwrap() {
            for key in [
                "command",
                "args",
                "env",
                "handTools",
                "unsupportedKeys",
                "problem",
            ] {
                server.as_object_mut().unwrap().remove(key);
            }
        }
        result["mcp"] = servers;
    }
    if s.get("connections").is_some() {
        result["sessions"] = Value::Object(
            pairs(&s["connections"])
                .map(|(k, v)| (k.clone(), v["max_concurrent_sessions"].clone()))
                .collect(),
        );
    }
    result
}
fn connection(r: &Value) -> Value {
    let engine = match r["engine"].as_str() {
        Some("codex-acp") => "codex",
        Some("claude-agent-acp") => "claude",
        _ => "opencode",
    };
    let state = match r["state"].as_str().unwrap_or("") {
        "configured" | "ready" | "validated" | "connected" => "connected",
        "unavailable" | "invalid" => "unavailable",
        _ => "needs_login",
    };
    let command = rows(&r["launch_command"])
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    json!({"id":r["id"],"engine":engine,"label":r["name"],"note":or(&r["auth_mode"],json!("")),
        "sessions":or(&r["max_concurrent_sessions"],json!(1)),"state":state,"checkedAt":null,
        "detail":{"connectionId":r["id"],"provider":or(&r["provider"],json!("")),"authMode":or(&r["auth_mode"],json!("")),
            "keyKind":r["key_kind"],"endpoint":r["endpoint"],"launchCommand":command,
            "credential":r["credential"].get("kind").unwrap_or(&json!("")),"qualified":truth(&r["qualified"]),
            "createdAt":or(&r["created_at"],json!("")),"updatedAt":or(&r["updated_at"],json!(""))}})
}

/// Render the complete settings envelope from independently completed observations.
/// Required keys: surfaces (management envelopes), workspace, entries (selectable
/// route metadata), usage_records (visible quota windows). routes/discovery may be
/// null when their producer refused. This never reads files, probes or authorizes.
pub fn settings_view(o: &Value) -> ModelResult<Value> {
    if !o["surfaces"].is_object()
        || !o["workspace"].is_object()
        || !o["entries"].is_array()
        || !o["usage_records"].is_array()
    {
        return Err("invalid_settings_observation");
    }
    let surfaces = &o["surfaces"];
    let workspace = &o["workspace"];
    for name in [
        "user_defaults",
        "routing",
        "routing_profiles",
        "mcp_servers",
        "connections",
    ] {
        let surface = &surfaces[name];
        if !surface["ok"].is_boolean()
            || (surface["ok"] == true && !surface["result"].is_object())
            || (surface["ok"] == false && !surface["error"].is_object())
        {
            return Err("invalid_settings_observation");
        }
    }
    let part = |name: &str| {
        if truth(&surfaces[name]["ok"]) {
            &surfaces[name]["result"]
        } else {
            &Value::Null
        }
    };
    let (d, r, p, m, c) = (
        part("user_defaults"),
        part("routing"),
        part("routing_profiles"),
        part("mcp_servers"),
        part("connections"),
    );
    let unavailable = [
        "user_defaults",
        "routing",
        "routing_profiles",
        "mcp_servers",
        "connections",
        "project_ceiling",
    ]
    .iter()
    .find(|name| surfaces.get(**name).is_some() && !truth(&surfaces[**name]["ok"]))
    .map(|name| surfaces[*name]["error"]["message"].clone())
    .unwrap_or(Value::Null);
    let mut catalogue = json!({});
    let mut setup = json!({});
    for engine in ENGINES {
        catalogue[engine] = json!({"observedAt":null,"failed":false,"models":[]});
        setup[engine] = json!({"status":"not_downloaded","automatic":false,"reason":null,"autoConnectDisabled":false});
    }
    let entries: BTreeMap<_, _> = rows(&o["entries"])
        .filter_map(|v| v["route_id"].as_str().map(|id| (id, v)))
        .collect();
    for route in rows(&o["routes"]["routes"]) {
        let engine = route["engine"].as_str().unwrap_or("");
        // The model-management catalogue retains observed models so an owner can
        // re-enable them. Task pickers apply routing policy separately.
        let selectable = route.get("selectable").unwrap_or(&route["in_selector"]);
        let connection_ready = route["route_id"]
            .as_str()
            .is_some_and(|id| entries.contains_key(id));
        if !ENGINES.contains(&engine) || (!truth(selectable) && !connection_ready) {
            continue;
        }
        let models = catalogue[engine]["models"].as_array_mut().unwrap();
        if models.iter().any(|m| m["routeId"] == route["route_id"]) {
            continue;
        }
        let entry = route["route_id"]
            .as_str()
            .and_then(|id| entries.get(id))
            .copied()
            .unwrap_or(&Value::Null);
        let mut model = json!({"id":route["model"],"routeId":route["route_id"],"label":or(&route["model_name"],route["model"].clone()),
            "efforts":entry.get("efforts").cloned().unwrap_or(json!([])),"observed":truth(&route["observed"])});
        if route["previous_route_ids"].as_array().is_some_and(|a| !a.is_empty()) {
            model["previousRouteIds"] = route["previous_route_ids"].clone();
        }
        if truth(&entry["context_window"]) {
            model["contextWindow"] = entry["context_window"].clone();
        }
        models.push(model);
    }
    for engine in ENGINES {
        let models = catalogue[engine]["models"].as_array_mut().unwrap();
        let mut counts = BTreeMap::new();
        for model in models.iter() {
            *counts.entry(model["label"].to_string()).or_insert(0) += 1;
        }
        let mut numbers = BTreeMap::new();
        for model in models.iter_mut() {
            let label = model["label"].to_string();
            if counts[&label] > 1 {
                let n = numbers.entry(label).or_insert(0);
                *n += 1;
                let owner = model["routeId"]
                    .as_str()
                    .and_then(|id| entries.get(id))
                    .map(|v| &v["connection_name"])
                    .filter(|v| truth(v))
                    .cloned()
                    .unwrap_or(json!(format!("Connection {n}")));
                model["label"] = json!(format!(
                    "{} · {}",
                    model["label"].as_str().unwrap_or(""),
                    owner.as_str().unwrap_or("")
                ));
            }
        }
    }
    for engine in rows(&o["routes"]["engines"]) {
        let id = engine["engine"].as_str().unwrap_or("");
        if ENGINES.contains(&id) {
            if let Some(time) = rows(&engine["connections"])
                .filter_map(|c| c["catalog_observed_at"].as_str())
                .filter(|s| !s.is_empty())
                .max()
            {
                catalogue[id]["observedAt"] = iso_ms(&json!(time));
            }
            // Only the engine owning an unreadable stored catalog reports a failed list.
            if rows(&engine["connections"]).any(|c| {
                rows(&o["routes"]["catalog_problems"]).any(|p| p["connection_id"] == c["id"])
            }) {
                catalogue[id]["failed"] = json!(true);
            }
        }
    }
    for item in rows(&o["discovery"]["engines"]) {
        let engine = item["engine"].as_str().unwrap_or("");
        if ENGINES.contains(&engine) {
            setup[engine] = json!({"status":item.get("status").unwrap_or(&json!("not_downloaded")),
                "automatic":truth(&item["automatic_setup"]),"reason":item["automatic_setup_reason"],
                "autoConnectDisabled":truth(&item["auto_connect_disabled"])});
            if let Some(phase) = item["automatic_setup"].as_str() {
                setup[engine]["automaticPhase"] = json!(phase);
            }
            if let Some(stage) = item["automatic_setup_stage"].as_str() {
                setup[engine]["automaticStage"] = json!(stage);
            }
        }
    }
    let user = rows(&r["sources"])
        .filter(|s| s["source"] == "user")
        .last()
        .unwrap_or(&Value::Null);
    let tokens = json!({"routing":or(&user["revision"],json!("absent")),"user_defaults":or(&d["revision"],json!("absent")),
        "mcp_servers":or(&m["revision"],json!("absent")),"routing_profiles":or(&p["revision"],json!("absent")),
        "connections":or(&c["revision"],json!("absent")),"profile_sections":or(&p["current_revisions"],json!({}))});
    let profiles: Vec<_> = rows(&p["profiles"]).map(|e| json!({"id":e["name"],"name":e["name"],
        "description":or(&e["description"],json!("")),"updatedAt":iso_ms(&e["updated_at"]),"sections":sections(&e["settings"],workspace)})).collect();
    let mut missing = Vec::new();
    for (name, exists) in [
        ("routing.json", &user["exists"]),
        ("defaults.json", &d["exists"]),
        ("mcp-servers.json", &m["exists"]),
    ] {
        if !truth(exists) {
            missing.push(name);
        }
    }
    let (usage, seen) = plan_usage(&o["usage_records"]);
    Ok(
        json!({"config":{"revisions":{"routing":revision(&tokens["routing"]),"user_defaults":revision(&tokens["user_defaults"]),
            "project_ceiling":0,"mcp_servers":revision(&tokens["mcp_servers"])},
        "surfaces":{"routing":routing(&r["effective_rules"]),"user_defaults":defaults(d,workspace),"mcp_servers":mcp(m)},
        "projects":{},"active":p["active"],"interrupted":truth(&p["pending"]),"unavailable":unavailable,"profiles":profiles,
        "catalogue":catalogue,"quotaObserved":seen,"mcpObserved":{},"setup":setup,
        "files":{"envRouting":null,"invalidRouting":truth(r) && !truth(&r["effective_valid"]) && r["effective_source"]!="missing",
            "migration":false,"missing":missing,"backups":0,"preservedRouting":[],"preservedDefaults":or(&d["preserved_keys"],json!([])),
            "askpass":false,"env":{"engineUpdate":null,"engineFollow":null,"opencodeBinary":null}}},
        "connections":rows(&c["connections"]).map(connection).collect::<Vec<_>>(),"tokens":tokens,"usage":usage}),
    )
}
/// Claude's weekly windows one by one: exact `seven_day` (all models) and
/// `seven_day_<model>` names only, general window first. A window reported by
/// several connections keeps its highest use. Unreported windows are absent.
fn claude_windows(row: &mut Value, windows: &Value) {
    let exact = regex::Regex::new(r"^seven_day(_[a-z0-9]+)*$").unwrap();
    for w in rows(windows) {
        let (Some(name), Some(used)) = (w["name"].as_str(), w["usedPercent"].as_f64()) else {
            continue;
        };
        if !exact.is_match(name) || name.len() > 64 {
            continue;
        }
        let item = json!({"name":name,"usedPercent":w["usedPercent"],"resetAt":reset_time(&w["resetsAt"]),
            "observedAt":reset_time(&w["observedAt"])});
        let list = row["windows"].as_array_mut().unwrap();
        match list.iter().position(|o| o["name"] == name) {
            Some(at)
                if list[at]["usedPercent"]
                    .as_f64()
                    .is_some_and(|old| old >= used) => {}
            Some(at) => list[at] = item,
            None => list.push(item),
        }
        list.sort_by_key(|o| {
            let n = o["name"].as_str().unwrap_or("").to_owned();
            (n != "seven_day", n)
        });
    }
}
fn plan_usage(records: &Value) -> (Value, Value) {
    let mut seen = json!({"claude-agent-acp":[],"codex-acp":[],"opencode":[]});
    let mut result: Vec<Value> = Vec::new();
    let mut records: Vec<_> = rows(records).collect();
    records.sort_by_key(|r| r["id"].as_str().unwrap_or(""));
    for r in records {
        let e = r["engine"].as_str().unwrap_or("");
        let name = match e {
            "codex-acp" => "codex",
            "claude-agent-acp" => "claude",
            _ => continue,
        };
        let i = result.iter().position(|u| u["engine"] == name).unwrap_or_else(|| {
            result.push(json!({"engine":name,"connected":true,"usedPercent":null,"resetAt":null,"observedAt":null,"window":"weekly"}));
            if name == "claude" {
                result.last_mut().unwrap()["windows"] = json!([]);
            }
            result.len()-1
        });
        if name == "claude" {
            claude_windows(&mut result[i], &r["windows"]);
        }
        for window in rows(&r["windows"]) {
            let label = window["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .replace('ſ', "s");
            if if e == "codex-acp" {
                window["windowMinutes"] != 10080
            } else {
                !label.contains("week") && !label.contains("seven")
            } {
                continue;
            }
            seen[e]
                .as_array_mut()
                .unwrap()
                .push(json!({"window":window["name"],"percent":window["usedPercent"]}));
            let Some(used) = window["usedPercent"].as_f64() else {
                continue;
            };
            if result[i]["usedPercent"]
                .as_f64()
                .is_none_or(|old| used > old)
            {
                result[i]["usedPercent"] = window["usedPercent"].clone();
                result[i]["resetAt"] = reset_time(&window["resetsAt"]);
                result[i]["observedAt"] = reset_time(&window["observedAt"]);
            }
        }
    }
    (json!(result), seen)
}

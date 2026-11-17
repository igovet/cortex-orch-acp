//! Real socket parity for launch-capable Python producer captures.
use super::*;
use std::path::Path;
fn tree(path: &Path, key: &str, out: &mut Value) {
    let m = fs::symlink_metadata(path).unwrap();
    if m.file_type().is_symlink() {
        out[key] = json!({"symlink":fs::read_link(path).unwrap()});
        return;
    }
    let mode = m.permissions().mode() & 0o777;
    if m.is_dir() {
        out[key] = json!({"directory":true,"mode":mode});
        for p in fs::read_dir(path).unwrap() {
            let p = p.unwrap().path();
            tree(
                &p,
                &format!("{key}/{}", p.file_name().unwrap().to_str().unwrap()),
                out,
            );
        }
    } else {
        assert!(m.is_file());
        assert_eq!(m.len(), fs::read(&path).unwrap().len() as u64);
        let bytes = fs::read(path).unwrap();
        out[key] = match String::from_utf8(bytes.clone()) {
            Ok(content) => json!({"content":content,"mode":mode}),
            Err(_) => {
                use base64::Engine;
                json!({"base64":base64::engine::general_purpose::STANDARD.encode(bytes),"mode":mode})
            }
        };
    }
}
fn normalize(
    mut tree: Value,
    f: &Fixture,
    case: &Value,
    corpus: &Value,
) -> (Value, Option<(String, String)>) {
    let root = &f.root;
    let mut binding = None;
    let mut instance_binding = None;
    let mut out = json!({});
    for (path, item) in tree.as_object_mut().unwrap() {
        let mut path = path.clone();
        if path.contains("/engine-logs/eirt_") {
            let id = path
                .split("/engine-logs/")
                .nth(1)
                .unwrap()
                .split('/')
                .next()
                .unwrap()
                .to_owned();
            assert!(regex::Regex::new("^eirt_[0-9a-f]{12}$")
                .unwrap()
                .is_match(&id));
            assert!(
                instance_binding.as_ref().is_none_or(|prior| prior == &id),
                "multiple engine identities"
            );
            instance_binding = Some(id.clone());
            path = path.replace(&id, "eirt_@INSTANCE@");
            if path.ends_with("observations.jsonl") {
                let mut rows = Vec::new();
                for line in item["content"].as_str().unwrap().lines() {
                    let mut row: Value = serde_json::from_str(line).unwrap();
                    assert_eq!(row["engine_instance_id"], id);
                    assert!(row["time_ns"].as_u64().unwrap() > 0);
                    row["engine_instance_id"] = json!("eirt_@INSTANCE@");
                    row["time_ns"] = json!("@TIME_NS@");
                    if row["event"] == "starting" {
                        let cwd = row["cwd"].as_str().unwrap();
                        assert!(Path::new(cwd).is_absolute());
                        assert!(!Path::new(cwd).exists(), "scratch workspace survived");
                        row["cwd"] = json!("@WORKSPACE@");
                        let initial = case["initial"]["config/cortex/connections.json"]["content"]
                            .as_str()
                            .unwrap();
                        let registry: Value = serde_json::from_str(initial).unwrap();
                        let mut argv = registry["connections"]
                            .as_object()
                            .unwrap()
                            .values()
                            .next()
                            .unwrap()["launch_command"]
                            .as_array()
                            .unwrap()
                            .clone();
                        if case["name"].as_str().unwrap().starts_with("codex-pinned-") {
                            argv[1] = json!("@ROOT@/state/cortex/connections/cn_000000000001/codex-home/manual-review-adapter/dist/index.js");
                        }
                        let actual = argv
                            .iter()
                            .map(|v| {
                                v.as_str()
                                    .unwrap()
                                    .replace("@ROOT@/config", f.config().to_str().unwrap())
                                    .replace(
                                        "@ROOT@/state",
                                        f.credentials_state().to_str().unwrap(),
                                    )
                                    .replace("@ROOT@", root.to_str().unwrap())
                            })
                            .collect::<Vec<_>>()
                            .join("\u{1f}");
                        assert_eq!(
                            row["command"]["argv_hash"],
                            format!("{:x}", Sha256::digest(actual.as_bytes()))[..16]
                        );
                        let normalized = argv
                            .iter()
                            .map(|v| v.as_str().unwrap())
                            .collect::<Vec<_>>()
                            .join("\u{1f}");
                        row["command"]["argv_hash"] =
                            json!(format!("{:x}", Sha256::digest(normalized.as_bytes()))[..16]);
                    }
                    if row["event"] == "transport_eof" {
                        assert!(row["alive"].is_boolean());
                        row["alive"] = json!("@EOF_REAP_RACE@");
                    }
                    rows.push(row.to_string());
                }
                rows.sort();
                item["content"] = json!(rows.join("\n") + "\n");
            }
        }
        if path.ends_with(".json")
            && path.contains("/cs_")
            && !case["initial"].as_object().unwrap().contains_key(&path)
        {
            assert!(binding.is_none(), "multiple new snapshots");
            let snapshot: Value = serde_json::from_str(item["content"].as_str().unwrap()).unwrap();
            let id = snapshot["snapshot_id"].as_str().unwrap().to_owned();
            let now = snapshot["created_at"].as_str().unwrap().to_owned();
            assert!(regex::Regex::new("^cs_[0-9a-f]{12}$")
                .unwrap()
                .is_match(&id));
            assert!(
                regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\+00:00$")
                    .unwrap()
                    .is_match(&now)
            );
            assert!(path.ends_with(&format!("/{id}.json")));
            for entry in snapshot["entries"].as_array().unwrap() {
                assert_eq!(entry["catalog_snapshot_id"], id);
                for provenance in entry["evidence"].as_object().unwrap().values() {
                    assert_eq!(provenance["observed_at"], now);
                }
            }
            let body = item["content"]
                .as_str()
                .unwrap()
                .replace(&id, corpus["snapshot"].as_str().unwrap())
                .replace(&now, corpus["now"].as_str().unwrap());
            item["content"] = json!(body);
            path = path.replace(&id, corpus["snapshot"].as_str().unwrap());
            binding = Some((id, now));
        }
        out[&path] = item.clone();
    }
    (
        serde_json::from_str(
            &out.to_string()
                .replace(f.config().to_str().unwrap(), "@ROOT@/config")
                .replace(f.credentials_state().to_str().unwrap(), "@ROOT@/state")
                .replace(root.to_str().unwrap(), "@ROOT@"),
        )
        .unwrap(),
        binding,
    )
}
pub(super) async fn check(f: &Fixture) {
    let c: Value = serde_json::from_str(include_str!(
        "../fixtures/config-catalog-producer-python.json"
    ))
    .unwrap();
    assert_eq!(c["forbiddenAttempts"], json!([]));
    let roots = [
        ("home", f.root.join("home")),
        ("config/cortex", f.config().join("cortex")),
        ("state/cortex", f.credentials_state().join("cortex")),
    ];
    for case in c["cases"].as_array().unwrap() {
        for (_, top) in &roots {
            if top.exists() {
                fs::remove_dir_all(top).unwrap();
            }
        }
        for (relative, item) in case["initial"].as_object().unwrap() {
            let (prefix, top) = roots
                .iter()
                .find(|(p, _)| relative == *p || relative.starts_with(&format!("{p}/")))
                .unwrap();
            let path = top.join(
                relative
                    .strip_prefix(prefix)
                    .unwrap()
                    .trim_start_matches('/'),
            );
            if let Some(target) = item["symlink"].as_str() {
                std::os::unix::fs::symlink(
                    target.replace("@ROOT@", f.root.to_str().unwrap()),
                    &path,
                )
                .unwrap();
                continue;
            }
            if item["directory"] == true {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let bytes = if let Some(content) = item["content"].as_str() {
                    content
                        .replace("@ROOT@/config", f.config().to_str().unwrap())
                        .replace("@ROOT@/state", f.credentials_state().to_str().unwrap())
                        .replace("@ROOT@", f.root.to_str().unwrap())
                        .into_bytes()
                } else {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(item["base64"].as_str().unwrap())
                        .unwrap()
                };
                fs::write(&path, bytes).unwrap();
            }
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(item["mode"].as_u64().unwrap() as u32),
            )
            .unwrap();
        }
        let peer = f.root.join("bin/peer");
        let seen = f.root.join("bin/seen");
        let _ = fs::remove_file(&peer);
        let _ = fs::remove_file(&seen);
        let _ = fs::remove_file(f.root.join("bin/debug-seen"));
        if let Some(script) = case["peer"].as_str() {
            fs::write(
                &peer,
                script
                    .replace("@ROOT@/config", f.config().to_str().unwrap())
                    .replace("@ROOT@/state", f.credentials_state().to_str().unwrap())
                    .replace("@ROOT@", f.root.to_str().unwrap()),
            )
            .unwrap();
            fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let calls_before = fs::read_to_string(f.root.join("called")).unwrap_or_default();
        let actual = f.action(case["params"].clone()).await;
        assert_eq!(
            fs::read_to_string(f.root.join("called")).unwrap_or_default(),
            calls_before,
            "helper called: {}",
            case["name"]
        );
        let mut files = json!({});
        for (prefix, top) in &roots {
            if top.exists() {
                tree(top, prefix, &mut files);
            }
        }
        let (files, binding) = normalize(files, f, case, &c);
        let mut expected_files = case["after"].clone();
        // The only intentional artifact change: validate the exact old Python
        // source, then compare the complete new native artifact and original mode.
        let key = "state/cortex/connections/cn_000000000001/codex-home/codex-acp-runtime";
        if let Some(old) = expected_files[key]["content"].as_str() {
            let template: Value =
                serde_json::from_str(include_str!("../../src/machine/config_catalog_codex.json"))
                    .unwrap();
            let prefix = "BINARY = ";
            let binary_literal = old.lines().find_map(|l| l.strip_prefix(prefix)).unwrap();
            assert_eq!(
                old,
                template["wrapper"]
                    .as_str()
                    .unwrap()
                    .replace("'@BINARY@'", binary_literal)
            );
            // These retained catalog captures have ordinary single-quoted paths.
            assert!(binary_literal.starts_with('\'') && binary_literal.ends_with('\''));
            let binary = &binary_literal[1..binary_literal.len() - 1];
            let native = workspace_service::native_codex_runtime::artifact(
                Path::new(env!("CARGO_BIN_EXE_workspace-service")),
                Path::new(&binary.replace("@ROOT@", f.root.to_str().unwrap())),
            )
            .unwrap();
            expected_files[key]["content"] =
                json!(native.replace(f.root.to_str().unwrap(), "@ROOT@"));
        }
        assert_eq!(
            files.as_object().unwrap().keys().collect::<Vec<_>>(),
            expected_files
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            "tree paths: {}",
            case["name"]
        );
        for (path, item) in files.as_object().unwrap() {
            if item != &expected_files[path] {
                // Keep diagnostics bounded without dropping any comparison.
                panic!(
                    "producer file/tree/mode mismatch: {} {path}; actual={:x}; expected={:x}",
                    case["name"],
                    Sha256::digest(item.to_string()),
                    Sha256::digest(expected_files[path].to_string())
                );
            }
        }
        let mut actual = actual.to_string();
        if let Some((_, now)) = binding {
            actual = actual.replace(&now, c["now"].as_str().unwrap());
        }
        actual = actual.replace(f.root.to_str().unwrap(), "@ROOT@");
        let mut expected = case["reply"].clone();
        // Native policy denials now apply to selector flags as well as dispatch.
        if let Some(routes) = expected
            .get_mut("result")
            .and_then(|v| v.get_mut("routes"))
            .and_then(Value::as_array_mut)
        {
            for route in routes {
                route["selectable"] =
                    json!(route["selectable"] == true && route["disabled"] != true);
            }
        }
        assert_eq!(
            serde_json::from_str::<Value>(&actual).unwrap(),
            expected,
            "producer complete reply: {}",
            case["name"]
        );
        assert_eq!(
            json!(fs::read_to_string(f.root.join("bin/debug-seen"))
                .unwrap_or_default()
                .lines()
                .collect::<Vec<_>>()),
            case["debugCalls"],
            "offline debug argv: {}",
            case["name"]
        );
        let calls = fs::read_to_string(&seen).unwrap_or_default();
        let calls: Vec<Value> = calls
            .lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                if value["method"] == "session/new" {
                    value["params"]["cwd"] = json!("@WORKSPACE@");
                }
                value
            })
            .collect();
        assert_eq!(
            json!(calls),
            case["engineCalls"],
            "exact prompt-free peer calls: {}",
            case["name"]
        );
    }
    // A noncooperating registry replacement during a successful observation is
    // unknown, not a conflict claiming no effects and never a Python replay.
    let script = c["cases"][0]["peer"].as_str().unwrap().replace(
        "*session/new*)",
        &format!("*session/new*) printf ' ' >> '{}';", f.path().display()),
    );
    assert!(script.contains("printf ' ' >>"));
    fs::write(
        f.root.join("bin/peer"),
        script.replace("@ROOT@", f.root.to_str().unwrap()),
    )
    .unwrap();
    let before = fs::read_to_string(f.root.join("called")).unwrap_or_default();
    let catalogs = f.credentials_state().join("cortex/catalog/cn_000000000001");
    assert!(
        catalogs.is_dir(),
        "the last capture must leave a prior snapshot"
    );
    let mut preserved = json!({});
    tree(&catalogs, "catalog", &mut preserved);
    let reply = f.action(json!({"action":"refresh"})).await;
    assert_eq!(reply["code"], "machine_outcome_unknown", "{reply}");
    assert_eq!(
        fs::read_to_string(f.root.join("called")).unwrap_or_default(),
        before
    );
    let mut after_race = json!({});
    tree(&catalogs, "catalog", &mut after_race);
    assert_eq!(
        after_race, preserved,
        "all prior snapshot bytes/modes survive; raced observation publishes nothing"
    );
    // Selection-only regressions for unported shapes. None can launch the peer,
    // and each complete request reaches the retained producer exactly once.
    let baseline: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    // A buildable install with no declared bin: Python's verifier, not its
    // resolver, decides it, so the outcome is uncertain and stays retained.
    // (No `@openai/codex` at all would be a qualified unverified direct launch.)
    fs::write(f.root.join("bin/package.json"), "{}").unwrap();
    let modules = f.root.join("bin/node_modules/@openai");
    let vendor = modules.join("codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin");
    fs::create_dir_all(modules.join("codex")).unwrap();
    fs::create_dir_all(&vendor).unwrap();
    fs::write(
        modules.join("codex/package.json"),
        r#"{"version":"0.159.3"}"#,
    )
    .unwrap();
    fs::write(vendor.join("codex"), "#!/bin/sh\nexit 91\n").unwrap();
    fs::set_permissions(vendor.join("codex"), fs::Permissions::from_mode(0o700)).unwrap();
    for shape in ["pinned-argv", "api-key", "opencode", "unsafe-name"] {
        let mut registry = baseline.clone();
        let r = &mut registry["connections"]["cn_000000000001"];
        match shape {
            "pinned-argv" => {
                r["launch_command"] =
                    json!([f.root.join("bin/peer"), f.root.join("bin/adapter.js")])
            }
            "api-key" => {
                r["auth_mode"] = json!("api_key");
                r["key_kind"] = json!("openai_api_key");
                r["credential_ref"] =
                    json!({"kind":"os_keyring","service":"fixture","account":"synthetic"});
            }
            "opencode" => {
                r["engine"] = json!("opencode");
                r["launch_command"] = json!([f.root.join("bin/peer"), "acp"]);
            }
            _ => r["name"] = json!("https://unsafe.example"),
        }
        fs::write(f.path(), serde_json::to_vec(&registry).unwrap()).unwrap();
        if shape == "unsafe-name" {
            // Python observes the engine first and refuses the label only when
            // building the catalogue: the (cooperating) peer really runs.
            fs::write(
                f.root.join("bin/peer"),
                c["cases"][0]["peer"]
                    .as_str()
                    .unwrap()
                    .replace("@ROOT@", f.root.to_str().unwrap()),
            )
            .unwrap();
        }
        let helpers = fs::read_to_string(f.root.join("called"))
            .unwrap_or_default()
            .lines()
            .count();
        let peers = fs::read_to_string(f.root.join("bin/seen")).unwrap();
        let reply = f.action(json!({"action":"refresh"})).await;
        // Native: an unported shape is an explicit failed row, never a launch,
        // a delegation or a fabricated observation.
        if shape == "opencode" {
            // The recorded auth shape is not valid for OpenCode: the registry
            // itself is refused (nothing launched, nothing written).
            assert_eq!(reply["ok"], false, "{shape}: {reply}");
            assert_eq!(reply["code"], "unsupported_auth_mode", "{shape}: {reply}");
            continue;
        }
        assert_eq!(reply["ok"], true, "{shape}: {reply}");
        let rows = reply["result"]["refresh"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|r| r["connection_id"] == "cn_000000000001")
            .unwrap();
        assert_eq!(row["success"], false, "{shape}: {reply}");
        if shape == "unsafe-name" {
            assert_eq!(
                row["reason"], "invalid_catalog:unsafe_connection_label",
                "{reply}"
            );
            assert_ne!(fs::read_to_string(f.root.join("bin/seen")).unwrap(), peers);
            continue;
        }
        assert!(
            row["reason"]
                .as_str()
                .unwrap()
                .starts_with("engine_unreachable:"),
            "{shape}: {reply}"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("called"))
                .unwrap_or_default()
                .lines()
                .count(),
            helpers,
            "{shape}"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("bin/seen")).unwrap(),
            peers,
            "{shape}"
        );
    }
}

fn materialize(f: &Fixture, roots: &[(&str, std::path::PathBuf)], initial: &Value) {
    for (_, top) in roots {
        if top.exists() {
            let _ = fs::set_permissions(top, fs::Permissions::from_mode(0o700));
            fs::remove_dir_all(top).unwrap();
        }
    }
    // Parents before children; a read-only leaf directory is created last.
    for (relative, item) in initial.as_object().unwrap() {
        let (prefix, top) = roots
            .iter()
            .find(|(p, _)| relative == *p || relative.starts_with(&format!("{p}/")))
            .unwrap();
        let path = top.join(
            relative
                .strip_prefix(prefix)
                .unwrap()
                .trim_start_matches('/'),
        );
        if item["directory"] == true {
            fs::create_dir_all(&path).unwrap();
        } else {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let content = item["content"]
                .as_str()
                .unwrap()
                .replace("@ROOT@/config", f.config().to_str().unwrap())
                .replace("@ROOT@/state", f.credentials_state().to_str().unwrap())
                .replace("@ROOT@", f.root.to_str().unwrap());
            fs::write(&path, content).unwrap();
        }
        // Channel-specific parents outside the captured tree are private, as on a host.
        for p in path
            .parent()
            .unwrap()
            .ancestors()
            .take_while(|p| p.starts_with(&f.root) && *p != f.root)
        {
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let mut items: Vec<_> = initial.as_object().unwrap().iter().collect();
    items.sort_by_key(|(relative, _)| std::cmp::Reverse(relative.len()));
    for (relative, item) in items {
        let (prefix, top) = roots
            .iter()
            .find(|(p, _)| relative == *p || relative.starts_with(&format!("{p}/")))
            .unwrap();
        let path = top.join(
            relative
                .strip_prefix(prefix)
                .unwrap()
                .trim_start_matches('/'),
        );
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(item["mode"].as_u64().unwrap() as u32),
        )
        .unwrap();
    }
}

/// A provider-less host refuses keyring refresh without the effects of an
/// observed Python import failure. Installer refusals still match the oracle.
pub(super) async fn check_keyring_less(f: &Fixture) {
    let c: Value = serde_json::from_str(include_str!(
        "../fixtures/settings-native-boundary-python.json"
    ))
    .unwrap();
    assert_eq!(c["forbiddenAttempts"], json!([]));
    let roots = [
        ("home", f.root.join("home")),
        ("config/cortex", f.config().join("cortex")),
        ("state/cortex", f.credentials_state().join("cortex")),
    ];
    // The oracle's existing XDG config/state homes, which Cortex never creates itself.
    for home in [f.config(), f.credentials_state()] {
        fs::create_dir_all(&home).unwrap();
        for p in home
            .ancestors()
            .take_while(|p| p.starts_with(&f.root) && *p != f.root)
        {
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let peer = f.root.join("bin/peer");
    let launched = f.root.join("bin/launched");
    fs::write(
        &peer,
        format!("#!/bin/sh\nprintf x >> '{}'\nexit 97\n", launched.display()),
    )
    .unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
    let refresh = c["refresh"].as_array().unwrap();
    assert_eq!(refresh.len(), 8);
    for case in refresh {
        materialize(f, &roots, &case["initial"]);
        let actual = f
            .wire(
                "machine.call",
                json!({"op":"config.action","params":case["params"]}),
            )
            .await;
        // Native: the OS keyring is never opened, so each such row is an explicit
        // failed attempt (never an observed state) and nothing is published.
        assert!(actual.get("error").is_none(), "{}: {actual}", case["name"]);
        assert_eq!(actual["result"]["ok"], true, "{}: {actual}", case["name"]);
        let outcomes = actual["result"]["result"]["refresh"].as_array().unwrap();
        assert!(!outcomes.is_empty(), "{}", case["name"]);
        for outcome in outcomes {
            assert_eq!(outcome["success"], false, "{}: {actual}", case["name"]);
            assert!(
                outcome["reason"]
                    .as_str()
                    .unwrap()
                    .starts_with("engine_unreachable:")
                    || outcome["reason"] == "engine_home_read_only",
                "{}: {actual}",
                case["name"]
            );
        }
        assert!(!launched.exists(), "engine launched: {}", case["name"]);
        let mut files = json!({});
        for (prefix, top) in &roots {
            if top.exists() {
                tree(top, prefix, &mut files);
            }
        }
        let (files, binding) = normalize(files, f, case, &c);
        assert!(binding.is_none(), "no snapshot may be published");
        assert_eq!(
            files, case["initial"],
            "unobserved keyring tree: {}",
            case["name"]
        );
    }
    let refusals = c["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 24);
    for case in refusals {
        materialize(f, &roots, &case["initial"]);
        let actual = f.action(case["params"].clone()).await;
        assert_eq!(actual, case["reply"], "refusal: {}", case["name"]);
        let mut files = json!({});
        for (prefix, top) in &roots {
            if top.exists() {
                tree(top, prefix, &mut files);
            }
        }
        let (files, _) = normalize(files, f, case, &c);
        assert_eq!(files, case["initial"], "refusal effects: {}", case["name"]);
    }
    for (_, top) in &roots {
        if top.exists() {
            let _ = fs::set_permissions(top, fs::Permissions::from_mode(0o700));
        }
    }
}

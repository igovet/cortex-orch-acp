//! Write workflows: each case runs its request sequence against Rust on a
//! reset fixture, then compares every response and the resulting files with
//! the frozen outcomes of the retired Python entry. Placeholders resolve against
//! the fixture:
//! `{project}`, `{rev:<file>}` (the file's revision), `{read:<surface>}`
//! (the revision a read of that surface returns) and `{revs}` (the four
//! profile backing-file revisions a profiles read returns).
use super::{registry, snapshot, Fixture, Report};
use cortex_paseo_tools::py::{self, Py};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};

struct Case {
    name: &'static str,
    files: Vec<(&'static str, String)>,
    /// Initialize a real project store in the fixture project first.
    store: bool,
    /// The documents carry timestamps: normalize revisions derived from them.
    timestamped: bool,
    steps: Vec<String>,
    /// Steps that must succeed (exit 0) on both sides.
    succeed: Vec<usize>,
}

fn call(fixture: &Fixture, input: &str) -> (Vec<u8>, Option<i32>) {
    fixture.rust(input)
}

fn result(fixture: &Fixture, request: &str) -> Py {
    let (out, _) = call(fixture, request);
    py::loads(String::from_utf8_lossy(&out).trim_end())
        .map(|v| v.get("result").clone())
        .unwrap_or(Py::Null)
}

fn fill(fixture: &Fixture, template: &str) -> String {
    let mut text = template.replace("{project}", &fixture.project());
    while let Some(start) = text.find("{rev:") {
        let end = start + text[start..].find('}').unwrap();
        let path = fixture.at(&text[start + 5..end]);
        let revision = fs::read(&path)
            .map(|b| format!("sha256:{:x}", Sha256::digest(b)))
            .unwrap_or_else(|_| "absent".into());
        text.replace_range(start..=end, &revision);
    }
    while let Some(start) = text.find("{read:") {
        let end = start + text[start..].find('}').unwrap();
        let surface = text[start + 6..end].to_owned();
        let project = if surface == "project_ceiling" {
            format!(r#","project":"{}""#, fixture.project())
        } else {
            String::new()
        };
        let request = format!(r#"{{"action":"read","surface":"{surface}"{project}}}"#);
        let view = result(fixture, &request);
        let revision = view.get("revision").as_str().unwrap_or("absent").to_owned();
        text.replace_range(start..=end, &revision);
    }
    if text.contains("{revs}") {
        let view = result(fixture, r#"{"action":"read","surface":"routing_profiles"}"#);
        let revisions = py::dumps(view.get("current_revisions")).unwrap();
        text = text.replace("{revs}", &revisions);
    }
    text
}

fn normalize(text: &str, timestamped: bool) -> String {
    let time = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?\+00:00").unwrap();
    let stamp = regex::Regex::new(r"cortex-backup-\d+").unwrap();
    let text = time.replace_all(text, "<time>");
    let text = stamp
        .replace_all(&text, "cortex-backup-<stamp>")
        .into_owned();
    if timestamped {
        let revision = regex::Regex::new(r"sha256:[0-9a-f]{64}").unwrap();
        revision
            .replace_all(&text, "sha256:<revision>")
            .into_owned()
    } else {
        text
    }
}

/// The fixture's files with modes and (normalized) contents. The project
/// store's own database and sidecars are compared through reads instead.
fn tree(fixture: &Fixture, timestamped: bool) -> String {
    fn walk(
        path: &Path,
        store: &Path,
        fixture_base: &[u8],
        timestamped: bool,
        out: &mut Vec<String>,
    ) {
        if path.starts_with(store) && path != store && !path.ends_with("routing.json") {
            return;
        }
        let meta = fs::symlink_metadata(path).unwrap();
        let mode = meta.permissions().mode();
        if meta.is_dir() {
            out.push(format!("{} d{mode:o}", path.display()));
            let mut entries: Vec<_> = fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            for entry in entries {
                walk(&entry, store, fixture_base, timestamped, out);
            }
        } else {
            let bytes = super::rebase(&fs::read(path).unwrap(), fixture_base);
            let shown = match std::str::from_utf8(&bytes) {
                Ok(text) => normalize(text, timestamped),
                Err(_) => format!("{:x}", Sha256::digest(&bytes)),
            };
            let name = normalize(&path.display().to_string(), false);
            out.push(format!("{name} f{mode:o} {shown}"));
        }
    }
    let mut out = Vec::new();
    let store = fixture.at("workspace/.cortex");
    walk(
        &fixture.base,
        &store,
        fixture.base.to_str().unwrap().as_bytes(),
        timestamped,
        &mut out,
    );
    out.join("\n")
}

fn prepare(fixture: &mut Fixture, case: &Case) {
    fixture.reset(&case.files, false);
    let store = fixture.at("workspace/.cortex");
    let _ = fs::remove_dir_all(&store);
    fs::DirBuilder::new().mode(0o700).create(&store).unwrap();
    for (relative, body) in &case.files {
        if relative.starts_with("workspace/.cortex/") {
            let path = fixture.at(relative);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    if case.store {
        let key =
            cortex_core::store::ProjectKey::new(Path::new(&fixture.project()), workspace_channel())
                .unwrap();
        cortex_core::store::Store::open(key, cortex_core::store::OpenMode::Initialize).unwrap();
    }
}

fn workspace_channel() -> cortex_core::store::StoreChannel {
    if workspace_protocol::channel::current().is_dev() {
        cortex_core::store::StoreChannel::Dev
    } else {
        cortex_core::store::StoreChannel::Release
    }
}

fn play(fixture: &mut Fixture, case: &Case) -> (Vec<(Vec<u8>, Option<i32>)>, String) {
    prepare(fixture, case);
    let mut answers = vec![];
    for step in &case.steps {
        let request = fill(fixture, step);
        let (out, code) = call(fixture, &request);
        let text = normalize(&String::from_utf8_lossy(&out), case.timestamped);
        answers.push((text.into_bytes(), code));
    }
    (answers, tree(fixture, case.timestamped))
}

pub fn run(fixture: &mut Fixture, report: &mut Report, route: &str) {
    for case in cases(route) {
        let recorded = fixture.oracle(&format!("workflow/{}", case.name));
        let python: Vec<(Vec<u8>, Option<i32>)> = recorded["answers"]
            .as_array()
            .unwrap()
            .iter()
            .map(super::outcome)
            .collect();
        let python_tree = recorded["tree"].as_str().unwrap().to_owned();
        let (rust, rust_tree) = play(fixture, &case);
        for (index, (p, r)) in python.into_iter().zip(rust).enumerate() {
            if case.succeed.contains(&index) && (p.1 != Some(0) || r.1 != Some(0)) {
                report.failures.push(format!(
                    "{}/{index}: expected success, python {:?} rust {:?}: {}",
                    case.name,
                    p.1,
                    r.1,
                    String::from_utf8_lossy(&r.0)
                ));
            }
            report.compare(&format!("{}/{index}", case.name), p, r);
        }
        report.cases += 1;
        if python_tree != rust_tree {
            let diff: Vec<_> = python_tree
                .lines()
                .zip(rust_tree.lines())
                .filter(|(a, b)| a != b)
                .take(4)
                .collect();
            report.failures.push(format!(
                "{}: resulting files differ: {diff:?} ({} vs {} entries)",
                case.name,
                python_tree.lines().count(),
                rust_tree.lines().count()
            ));
        }
    }
}

/// Documented native limits: an explicit `config_api_unavailable` refusal
/// with nothing changed (Python is not run: it would launch or probe).
pub fn native_limits(fixture: &mut Fixture, report: &mut Report) {
    let custom = Case {
        name: "limit-preset-apply-custom",
        files: vec![(
            "config/cortex/routing.json",
            r#"{"model_policy_version": 2, "enabled": {"codex-acp": ["custom"]}}"#.into(),
        )],
        store: false,
        timestamped: false,
        steps: vec![],
        succeed: vec![],
    };
    prepare(fixture, &custom);
    let before = tree(fixture, false);
    for request in [
        r#"{"action":"discover_opencode","surface":"connections","binary":"/bin/sh"}"#,
        r#"{"action":"onboard_opencode","surface":"connections","expected_revision":"absent","explicit_opt_in":true,"binary":"/bin/sh"}"#,
    ] {
        report.cases += 1;
        let (out, code) = fixture.rust(request);
        let text = String::from_utf8_lossy(&out);
        if code != Some(1)
            || !text.contains("\"code\": \"config_api_unavailable\"")
            || !text.contains("Rust cortex-acp entry")
        {
            report
                .failures
                .push(format!("native limit {request}: {code:?} {text}"));
        }
    }
    if tree(fixture, false) != before {
        report
            .failures
            .push("a refused native-limit request changed a fixture file".into());
    }
}

fn update(surface: &str, body: &str) -> String {
    format!(r#"{{"api_version":1,"action":"update","surface":"{surface}",{body}}}"#)
}
fn action(surface: &str, action: &str, body: &str) -> String {
    let sep = if body.is_empty() { "" } else { "," };
    format!(r#"{{"action":"{action}","surface":"{surface}"{sep}{body}}}"#)
}
fn read(surface: &str) -> String {
    format!(r#"{{"action":"read","surface":"{surface}"}}"#)
}

fn cases(route: &str) -> Vec<Case> {
    let routing = |body: &str| update("routing", body);
    let mcp = |body: &str| update("mcp_servers", body);
    let connection = |body: &str| update("connections", body);
    let profile = |verb: &str, body: &str| action("routing_profiles", verb, body);
    let preset_mcp = r#"{"mcpServers": {"docs": {"command": "/opt/never-run", "args": ["--x"],
        "env": {"API_TOKEN": "fixture-secret", "OTHER": "o"}, "allowed_tools": ["read"]}, "off": {"enabled": false}}}"#;
    let owner = || {
        vec![
            ("config/cortex/connections.json", registry()),
            (
                "state/cortex/catalog/cn_000000000001/cs_000000000001.json",
                snapshot(route),
            ),
        ]
    };
    let with = |mut files: Vec<(&'static str, String)>, extra: Vec<(&'static str, String)>| {
        files.extend(extra);
        files
    };
    vec![
        // --- routing ---------------------------------------------------------
        Case {
            name: "routing-user-first",
            files: owner(),
            store: false,
            timestamped: false,
            steps: vec![
                routing(
                    r#""target":"user","expected_revision":"absent","rules":{"model_policy_version":2,
                    "enabled":{"opencode":["prov/m"],"codex-acp":["exact"]},"deny":["claude-agent-acp/*"],
                    "engine_affinity":{"review":"codex-acp","*":null}}"#,
                ),
                read("routing"),
                routing(r#""target":"user","expected_revision":"absent","rules":{"model_policy_version":2}"#),
            ],
            succeed: vec![0, 1],
        },
        Case {
            name: "routing-user-preserve",
            files: with(
                owner(),
                vec![(
                    "config/cortex/routing.json",
                    r#"{"unknown_key": {"z": 1, "a": [1.5, "Ω"]}, "model_policy_version": 2, "pin": {"*": "codex-acp/x"}}"#
                        .into(),
                )],
            ),
            store: false,
            timestamped: false,
            steps: vec![
                routing(
                    r#""target":"user","expected_revision":"{rev:config/cortex/routing.json}",
                    "rules":{"pin":{"reviewer":"codex-acp/exact:high"},"model_policy_version":2,
                    "models":{"codex-acp":{"tiers":{"exact":["deep","standard"]},"auto":["exact"],
                    "policy":{"exact":{"request_names":["fast"],"effort_floor":"low"}}}}}"#,
                ),
                read("routing"),
            ],
            succeed: vec![0, 1],
        },
        Case {
            name: "routing-refusals",
            files: owner(),
            store: false,
            timestamped: false,
            steps: vec![
                routing(
                    r#""target":"user","expected_revision":"absent","rules":{"bogus":1,"pin":{"bad role":"x",
                    "*":"nope/m","r":"opencode/m:huge"},"deny":["a b"],"auto_allow":["x/*","foo/bar"],
                    "tiers":{"m":"zzz"},"providers":[],"engine_affinity":{"nope":"codex-acp"},"enabled":{"x":[]},
                    "models":{"codex-acp":{"auto":["m","m"],"extra":1,"tiers":{"m":["deep","deep"]},
                    "policy":{"m":{"effort_floor":"max","max_effort":"low","request_names":["the"]}}}},
                    "quota_windows":5}"#,
                ),
                routing(r#""target":"user","expected_revision":"absent","rules":{}"#),
                routing(r#""target":"user","expected_revision":"absent","rules":{"model_policy_version":3}"#),
                routing(
                    r#""target":"user","expected_revision":"absent","rules":{"model_policy_version":2,
                    "models":{"codex-acp":{"auto":["m"]}}}"#,
                ),
                routing(r#""target":"user","expected_revision":"absent","rules":{"model_policy_version":2,"tiers":{"m":"deep"}}"#),
                routing(r#""target":"user","expected_revision":"absent","rules":null"#),
                routing(r#""target":"env","expected_revision":"absent","rules":{}"#),
                routing(r#""target":7,"expected_revision":"absent","rules":{}"#),
                routing(r#""target":"user","expected_revision":"sha256:x","rules":{}"#),
                routing(r#""target":"project","expected_revision":"absent","rules":{"model_policy_version":2},"project":"rel""#),
                routing(r#""target":"project","expected_revision":"absent","rules":{"model_policy_version":2},"project":"/""#),
            ],
            succeed: vec![],
        },
        Case {
            name: "routing-project",
            files: owner(),
            store: false,
            timestamped: false,
            steps: vec![
                routing(
                    r#""target":"project","expected_revision":"absent","project":"{project}",
                    "rules":{"model_policy_version":2,"deny":["opencode/*"]}"#,
                ),
                routing(
                    r#""target":"user","expected_revision":"absent","project":"{project}",
                    "rules":{"model_policy_version":2}"#,
                ),
                action("routing", "read", r#""project":"{project}""#),
                routing(
                    r#""target":"project","expected_revision":"absent","project":"{project}",
                    "rules":{"model_policy_version":2}"#,
                ),
            ],
            succeed: vec![0, 1, 2],
        },
        // --- MCP servers -------------------------------------------------------
        Case {
            name: "mcp-create",
            files: vec![],
            store: false,
            timestamped: false,
            steps: vec![
                mcp(
                    r#""expected_revision":"absent","operations":[{"op":"upsert","name":"web",
                    "server":{"allowed_tools":["fetch"],"command":"/usr/bin/web","env":{"Z_KEY":"z","A_KEY":"a"},
                    "network":true}}]"#,
                ),
                read("mcp_servers"),
            ],
            succeed: vec![0, 1],
        },
        Case {
            name: "mcp-edit",
            files: vec![("config/cortex/mcp-servers.json", preset_mcp.into())],
            store: false,
            timestamped: false,
            steps: vec![
                mcp(
                    r#""expected_revision":"{rev:config/cortex/mcp-servers.json}","operations":[
                    {"op":"upsert","name":"docs","server":{"command":"/opt/never-run","allowed_tools":["read","write"],
                    "env":[{"name":"API_TOKEN","keep":true},{"name":"NEW","value":"n"}]}},
                    {"op":"set_enabled","name":"off","enabled":true},
                    {"op":"set_enabled","name":"docs","enabled":false}]"#,
                ),
                read("mcp_servers"),
                mcp(
                    r#""expected_revision":"{rev:config/cortex/mcp-servers.json}","operations":[
                    {"op":"remove","name":"docs"},{"op":"remove","name":"off"}]"#,
                ),
                read("mcp_servers"),
            ],
            succeed: vec![1, 2, 3],
        },
        Case {
            name: "mcp-refusals",
            files: vec![("config/cortex/mcp-servers.json", preset_mcp.into())],
            store: false,
            timestamped: false,
            steps: vec![
                mcp(r#""expected_revision":"absent","operations":[]"#),
                mcp(r#""expected_revision":"absent","operations":[{"op":"remove","name":"docs"}]"#),
                mcp(
                    r#""expected_revision":"{rev:config/cortex/mcp-servers.json}","operations":[
                    {"op":"nope"},{"op":"upsert","name":"cortex","server":{}},{"op":"upsert","name":"ok","server":[]},
                    {"op":"upsert","name":"x","server":{"command":"/c","allowed_tools":["t"],"bogus":1,"zzz":2,"enabled":"yes"}},
                    {"op":"upsert","name":"y","server":{"command":"/c","allowed_tools":["t"],"env":{"bad name":"v","K":{"keep":true},"N":5}}},
                    {"op":"upsert","name":"z","server":{"command":"/c"}},
                    {"op":"remove","name":"missing"},{"op":"set_enabled","name":"docs","enabled":1},
                    {"op":"set_enabled","name":"missing","enabled":true}]"#,
                ),
                mcp(r#""expected_revision":"sha256:0","operations":[{"op":"remove","name":"docs"}]"#),
            ],
            succeed: vec![],
        },
        // --- connections -------------------------------------------------------
        Case {
            name: "connections-edit",
            files: owner(),
            store: false,
            timestamped: true,
            steps: vec![
                connection(
                    r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001",
                    "operation":"rename","name":"Renamed Ω""#,
                ),
                connection(
                    r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001",
                    "operation":"set_max_concurrent_sessions","max_concurrent_sessions":8"#,
                ),
                read("connections"),
                connection(
                    r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001",
                    "operation":"remove""#,
                ),
                read("connections"),
            ],
            succeed: vec![0, 1, 2, 3, 4],
        },
        Case {
            name: "connections-validate",
            files: owner(),
            store: false,
            timestamped: true,
            steps: vec![
                connection(
                    r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001",
                    "operation":"validate""#,
                ),
                read("connections"),
            ],
            succeed: vec![0, 1],
        },
        Case {
            name: "refresh-registered",
            files: vec![(
                "config/cortex/connections.json",
                registry().replace(r#"["/fixture/never-execute"]"#, "null"),
            )],
            store: false,
            timestamped: false,
            steps: vec![action("route_catalog", "refresh", ""), read("route_catalog")],
            succeed: vec![0, 1],
        },
        Case {
            name: "connections-refusals",
            files: owner(),
            store: false,
            timestamped: false,
            steps: vec![
                connection(r#""expected_revision":"absent","connection_id":"cn_000000000001","operation":"rename","name":"x""#),
                connection(r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"bad","operation":"rename","name":"x""#),
                connection(r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001","operation":"fly""#),
                connection(r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001","operation":"rename","name":"  ""#),
                connection(r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_000000000001","operation":"set_max_concurrent_sessions","max_concurrent_sessions":true"#),
                connection(r#""expected_revision":"{rev:config/cortex/connections.json}","connection_id":"cn_0000000000ff","operation":"rename","name":"x""#),
                connection(r#""expected_revision":7,"connection_id":"cn_000000000001","operation":"rename","name":"x""#),
            ],
            succeed: vec![],
        },
        // --- routing profiles --------------------------------------------------
        Case {
            name: "profiles-lifecycle",
            files: with(
                owner(),
                vec![(
                    "config/cortex/routing.json",
                    r#"{"model_policy_version": 2, "enabled": {"codex-acp": ["exact"]}}"#.into(),
                )],
            ),
            store: false,
            timestamped: true,
            steps: vec![
                profile(
                    "create",
                    r#""expected_revision":"absent","expected_revisions":{revs},"name":"work","description":"Ω desk""#,
                ),
                profile(
                    "update",
                    r#""expected_revision":"{read:routing_profiles}","name":"work","rename_to":"play","description":"d""#,
                ),
                profile("deactivate", r#""expected_revision":"{read:routing_profiles}""#),
                profile(
                    "activate",
                    r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"play""#,
                ),
                profile(
                    "save",
                    r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"play","project":"{project}""#,
                ),
                read("routing_profiles"),
                profile("delete", r#""expected_revision":"{read:routing_profiles}","name":"play""#),
                read("routing_profiles"),
            ],
            succeed: vec![0, 1, 2, 3, 4, 5, 6, 7],
        },
        Case {
            name: "profiles-activate-unmigrated",
            files: owner(),
            store: false,
            timestamped: true,
            steps: vec![
                profile("create", r#""expected_revision":"absent","expected_revisions":{revs},"name":"empty""#),
                profile("deactivate", r#""expected_revision":"{read:routing_profiles}""#),
                profile(
                    "activate",
                    r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"empty""#,
                ),
            ],
            succeed: vec![0, 1],
        },
        Case {
            name: "profiles-refusals",
            files: with(
                owner(),
                vec![("config/cortex/routing.json", r#"{"model_policy_version": 2}"#.into())],
            ),
            store: false,
            timestamped: true,
            steps: vec![
                profile("create", r#""expected_revision":"absent","expected_revisions":{revs},"name":"Bad Name""#),
                profile("create", r#""expected_revision":"absent","expected_revisions":{},"name":"ok""#),
                profile("delete", r#""expected_revision":"absent","name":"missing""#),
                profile("activate", r#""expected_revision":"absent","expected_revisions":{revs},"name":"x","discard_modifications":1"#),
                profile("create", r#""expected_revision":"absent","expected_revisions":{revs},"name":"one""#),
                profile("create", r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"one""#),
                profile("update", r#""expected_revision":"{read:routing_profiles}","name":"one","rename_to":"Bad""#),
                profile("save", r#""expected_revision":"absent","expected_revisions":{revs},"name":"one""#),
                profile("create", r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"two""#),
                routing(r#""target":"user","expected_revision":"{rev:config/cortex/routing.json}","rules":{"model_policy_version":2,"deny":["opencode/*"]}"#),
                profile("activate", r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"one""#),
                profile("save", r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"one""#),
                profile("activate", r#""expected_revision":"{read:routing_profiles}","expected_revisions":{revs},"name":"one","discard_modifications":true"#),
                read("routing"),
            ],
            succeed: vec![4, 8, 9, 12, 13],
        },
        // --- default preset ----------------------------------------------------
        Case {
            name: "preset-fresh",
            files: vec![],
            store: false,
            timestamped: true,
            steps: vec![
                read("default_preset"),
                action("default_preset", "apply_default_preset", ""),
                read("default_preset"),
                read("routing_profiles"),
                action("default_preset", "apply_default_preset", ""),
            ],
            succeed: vec![0, 1, 2, 3, 4],
        },
        Case {
            name: "preset-apply-existing",
            files: vec![
                (
                    "config/cortex/routing.json",
                    r#"{"model_policy_version": 2, "enabled": {"codex-acp": ["custom"]}}"#.into(),
                ),
                ("config/cortex/defaults.json", r#"{"defaults": {"language": "ru"}}"#.into()),
                (
                    "config/cortex/mcp-servers.json",
                    r#"{"servers": {"bad": {"command": "/c"}, "off": {"enabled": false}}}"#.into(),
                ),
            ],
            store: false,
            timestamped: true,
            steps: vec![action("default_preset", "apply_default_preset", "")],
            succeed: vec![0],
        },
        Case {
            name: "preset-apply-legacy",
            files: vec![
                ("config/cortex/routing.json", "{}".into()),
                ("config/cortex/defaults.json", r#"{"defaults": {"language": "xx"}}"#.into()),
            ],
            store: false,
            timestamped: true,
            steps: vec![action("default_preset", "apply_default_preset", "")],
            succeed: vec![0],
        },
        Case {
            name: "preset-apply-unsupported",
            files: vec![
                ("config/cortex/routing.json", r#"{"model_policy_version": 3}"#.into()),
                (
                    "config/cortex/defaults.json",
                    r#"{"defaults": {"coordinator": {"engine": "codex-acp", "model": "m", "parameters": {}}}}"#.into(),
                ),
            ],
            store: false,
            timestamped: true,
            steps: vec![action("default_preset", "apply_default_preset", "")],
            succeed: vec![0],
        },
        Case {
            name: "preset-reset",
            files: vec![
                ("config/cortex/routing.json", r#"{"model_policy_version": 2}"#.into()),
                ("config/cortex/defaults.json", r#"{"defaults": {"language": "en"}}"#.into()),
            ],
            store: false,
            timestamped: false,
            steps: vec![
                action("default_preset", "reset_default_preset", r#""expected_revision":"{read:default_preset}""#),
                action("default_preset", "reset_default_preset", r#""expected_revision":"absent","confirm":true"#),
                action("default_preset", "reset_default_preset", r#""expected_revision":null,"confirm":true"#),
                action("default_preset", "reset_default_preset", r#""expected_revision":"{read:default_preset}","confirm":true"#),
                read("default_preset"),
                read("user_defaults"),
            ],
            succeed: vec![3, 4, 5],
        },
        // --- project ceiling -----------------------------------------------------
        Case {
            name: "ceiling-lifecycle",
            files: vec![(
                "config/cortex/defaults.json",
                r#"{"defaults": {}, "ceiling": {"allowed_routes": ["a", "b", "c"], "allowed_connections": null,
                "allowed_data_recipients": null, "permission_level": "STANDARD", "limits": {"tokens": 100},
                "spend_limit": null, "spend_limit_enforceable": false, "fallback_policy": {"allowed_target_routes": null,
                "allowed_failure_classes": null, "budget": null, "data_recipient_consent": false}}}"#
                    .into(),
            )],
            store: true,
            timestamped: true,
            steps: vec![
                action("project_ceiling", "read", r#""project":"{project}""#),
                update(
                    "project_ceiling",
                    r#""project":"{project}","expected_revision":"absent","ceiling":{"allowed_routes":["c","a"],
                    "permission_level":"READ_ONLY","limits":{"tokens":50}}"#,
                ),
                action("project_ceiling", "read", r#""project":"{project}""#),
                action("all", "read", r#""project":"{project}""#),
                update(
                    "project_ceiling",
                    r#""project":"{project}","expected_revision":"absent","ceiling":null"#,
                ),
                update(
                    "project_ceiling",
                    r#""project":"{project}","expected_revision":"{read:project_ceiling}","ceiling":{"allowed_routes":["z"]}"#,
                ),
                update(
                    "project_ceiling",
                    r#""project":"{project}","expected_revision":"{read:project_ceiling}","ceiling":{"bogus":1,"limits":{"tokens":-1}}"#,
                ),
                update(
                    "project_ceiling",
                    r#""project":"{project}","expected_revision":"{read:project_ceiling}","ceiling":null"#,
                ),
                action("project_ceiling", "read", r#""project":"{project}""#),
            ],
            succeed: vec![0, 1, 2, 3, 7, 8],
        },
        Case {
            name: "ceiling-refusals",
            files: vec![],
            store: false,
            timestamped: false,
            steps: vec![
                action("project_ceiling", "read", r#""project":"{project}""#),
                action("project_ceiling", "read", r#""project":"relative""#),
                action("project_ceiling", "read", r#""project":null"#),
                update("project_ceiling", r#""project":"{project}","expected_revision":"absent","ceiling":null"#),
            ],
            succeed: vec![],
        },
        // --- catalog refresh, discovery, onboarding, installation -------------------
        Case {
            name: "refresh-and-discovery",
            files: vec![],
            store: false,
            timestamped: false,
            steps: vec![
                action("route_catalog", "refresh", ""),
                action("route_catalog", "refresh", r#""project":"{project}""#),
                action("connections", "discover_connections", ""),
                action("connections", "discover_opencode", ""),
                action("connections", "discover_opencode", r#""binary":"relative""#),
                action("connections", "discover_opencode", r#""binary":"/nonexistent/opencode""#),
                action("connections", "onboard_connection", r#""engine":"codex-acp","expected_revision":"absent","explicit_opt_in":true"#),
                action("connections", "onboard_connection", r#""engine":"nope","expected_revision":"absent","explicit_opt_in":true"#),
                action("connections", "onboard_connection", r#""engine":"claude-agent-acp","expected_revision":"absent","explicit_opt_in":false"#),
                action("connections", "onboard_connection", r#""engine":"opencode","expected_revision":"absent","explicit_opt_in":true"#),
                action("connections", "install_engine", r#""engine":"opencode""#),
                action("connections", "install_engine", r#""engine":7"#),
                action("connections", "onboard_opencode", r#""expected_revision":"absent","explicit_opt_in":true"#),
                action("connections", "onboard_opencode", r#""expected_revision":"absent","explicit_opt_in":false"#),
                action("connections", "onboard_opencode", r#""expected_revision":"x","explicit_opt_in":true"#),
                action("connections", "onboard_opencode", r#""expected_revision":"absent","explicit_opt_in":true,"name":"""#),
                action("connections", "onboard_opencode", r#""expected_revision":"absent","explicit_opt_in":true,"binary":"rel""#),
            ],
            succeed: vec![0, 1, 2, 3, 5],
        },
        Case {
            name: "onboard-existing",
            files: owner(),
            store: false,
            timestamped: false,
            steps: vec![
                action(
                    "connections",
                    "onboard_connection",
                    r#""engine":"codex-acp","expected_revision":"{rev:config/cortex/connections.json}","explicit_opt_in":true"#,
                ),
                action(
                    "connections",
                    "onboard_connection",
                    r#""engine":"codex-acp","expected_revision":"absent","explicit_opt_in":true"#,
                ),
            ],
            succeed: vec![0],
        },
    ]
}

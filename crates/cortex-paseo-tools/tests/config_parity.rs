//! Settings-API parity with the frozen outcomes of the retired Python
//! `cortex_runtime.configuration.management` entry. Every request is answered
//! by `cortex-paseo-tools config` on a channel-resolved fixture home and
//! project and compared with the golden outcome recorded once from that
//! entry (`tests/config-parity-golden.json`): stdout bytes, exit status and
//! the resulting files. Reads, request-shape refusals and every write
//! workflow (routing, MCP servers, connections, profiles, default preset,
//! project ceiling, catalog refresh, discovery and onboarding) must match
//! byte-for-byte; only wall-clock timestamps (and, for timestamped
//! documents, the revisions derived from them) are normalized. The test
//! needs no Python; the oracle can no longer be re-run.
use cortex_paseo_tools::py::{self, Py};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/config-parity-golden.json"
);
const TOOL: &str = env!("CARGO_BIN_EXE_cortex-paseo-tools");
struct Fixture {
    base: PathBuf,
    routing_override: bool,
    /// Recorded oracle outcomes by label.
    golden: BTreeMap<String, Value>,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("cxpt-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let golden =
            serde_json::from_slice(&fs::read(GOLDEN).expect("tests/config-parity-golden.json"))
                .unwrap();
        let fixture = Self {
            base,
            routing_override: false,
            golden,
        };
        for name in ["home", "config", "state", "workspace", "workspace/.cortex"] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(fixture.at(name))
                .unwrap();
        }
        fixture
    }

    // Expected compiled-channel layout, independently of the production resolver.
    fn at(&self, relative: &str) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            for (prefix, mapped) in [
                ("config", "config/cortex-workspace-dev"),
                ("state", "state/cortex-workspace-dev/state"),
                ("workspace/.cortex", "workspace/.cortex-dev"),
            ] {
                if relative == prefix {
                    return self.base.join(mapped);
                }
                if let Some(rest) = relative.strip_prefix(&format!("{prefix}/")) {
                    return self.base.join(mapped).join(rest);
                }
            }
        }
        self.base.join(relative)
    }

    fn project(&self) -> String {
        self.base.join("workspace").to_str().unwrap().to_owned()
    }

    fn reset(&mut self, files: &[(&str, String)], routing_override: bool) {
        for relative in [
            "config/cortex",
            "state/cortex",
            "workspace/.cortex/routing.json",
            "override.json",
        ] {
            let path = self.at(relative);
            if path.is_dir() {
                fs::remove_dir_all(&path).unwrap();
            } else {
                let _ = fs::remove_file(&path);
            }
        }
        for (relative, body) in files {
            let path = self.at(relative);
            let mut parent = PathBuf::new();
            for part in path.parent().unwrap().components() {
                parent.push(part);
                if !parent.exists() {
                    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
                }
            }
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        self.routing_override = routing_override;
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let at = |p: &str| self.base.join(p).to_str().unwrap().to_owned();
        let mut env = vec![
            ("HOME", at("home")),
            ("XDG_CONFIG_HOME", at("config")),
            ("XDG_STATE_HOME", at("state")),
            ("TZ", "UTC".into()),
            (
                "CORTEX_WORKSPACE_CHANNEL",
                workspace_protocol::channel::current().name.clone(),
            ),
        ];
        if self.routing_override {
            env.push(("CORTEX_ROUTING", at("override.json")));
        }
        env
    }

    /// The frozen oracle outcome for `label`; fixture paths are stored as `BASE`.
    fn oracle(&self, label: &str) -> Value {
        let base = self.base.to_str().unwrap();
        let stored = self
            .golden
            .get(label)
            .unwrap_or_else(|| panic!("no golden outcome for {label}"));
        serde_json::from_str(&stored.to_string().replace("BASE", base)).unwrap()
    }

    fn run(
        &self,
        program: &Path,
        args: &[&str],
        cwd: &Path,
        input: &str,
    ) -> (Vec<u8>, Option<i32>) {
        let environment = self.env();
        let mut child = Command::new(program)
            .args(args)
            .env_clear()
            .envs(environment)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (output.stdout, output.status.code())
    }

    fn rust(&self, input: &str) -> (Vec<u8>, Option<i32>) {
        self.run(Path::new(TOOL), &["config"], Path::new("/"), input)
    }

    /// Every file and directory under the fixture with its mode and bytes.
    fn tree(&self) -> String {
        fn walk(path: &Path, base: &[u8], out: &mut Vec<String>) {
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
                    walk(&entry, base, out);
                }
            } else {
                let digest = format!(
                    "{:x}",
                    Sha256::digest(rebase(&fs::read(path).unwrap(), base))
                );
                out.push(format!("{} f{mode:o} {digest}", path.display()));
            }
        }
        let mut out = Vec::new();
        walk(&self.base, self.base.to_str().unwrap().as_bytes(), &mut out);
        out.join("\n")
    }
}

/// File bytes with the fixture's base directory replaced by `BASE`, so digests
/// do not depend on the temp directory.
pub fn rebase(bytes: &[u8], base: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.windows(base.len()).position(|w| w == base) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(b"BASE");
        rest = &rest[at + base.len()..];
    }
    out.extend_from_slice(rest);
    out
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// workspace-service's declared native deltas (its own Python-oracle
/// fixtures `generate_routing_management_fixture.py` and
/// `generate_defaults_profiles_fixture.py` record them): these two messages
/// are bounded, value-free native text. Accepted only at that exact field,
/// for that exact code, when the native text is the declared one.
const ROUTING_NATIVE: [&str; 2] = [
    "Saved routing has an invalid shape.",
    "Saved routing is not valid UTF-8 JSON.",
];
const PROFILE_NATIVE: &str =
    "The current section cannot be represented as a profile; repair it first.";

fn has_keys(value: &Py, keys: &[&str]) -> bool {
    matches!(value, Py::Dict(entries) if entries.len() == keys.len()
        && keys.iter().all(|key| entries.iter().any(|(k, _)| k == key)))
}

/// Put each declared native message into the Python tree; count them.
fn declared(python: &mut Py, rust: &Py, count: &mut usize) {
    let native = |value: &Py| {
        rust.get("error")
            .get("message")
            .as_str()
            .map(str::to_owned)
            .filter(|_| {
                value.get("error").get("code").as_str() == Some("invalid_routing_overrides")
            })
    };
    if has_keys(python, &["code", "message", "section"])
        && python.get("code").as_str() == Some("invalid_configuration")
        && rust.get("message").as_str() == Some(PROFILE_NATIVE)
    {
        python.set("message", Py::str(PROFILE_NATIVE));
        *count += 1;
    } else if python.contains("source")
        && python.contains("writable")
        && python.get("error").is_dict()
    {
        if let Some(message) = native(python).filter(|m| ROUTING_NATIVE.contains(&m.as_str())) {
            let mut error = python.get("error").clone();
            error.set("message", Py::str(message));
            python.set("error", error);
            *count += 1;
        }
    }
    match (python, rust) {
        (Py::Dict(entries), Py::Dict(other)) => {
            for (key, value) in entries.iter_mut() {
                if let Some((_, theirs)) = other.iter().find(|(k, _)| k == key) {
                    declared(value, theirs, count);
                }
            }
        }
        (Py::List(items), Py::List(other)) => {
            for (value, theirs) in items.iter_mut().zip(other) {
                declared(value, theirs, count);
            }
        }
        _ => (),
    }
}

/// The Python response with only the declared native deltas applied,
/// re-printed by the Python-compatible encoder.
fn with_declared(
    python: (Vec<u8>, Option<i32>),
    rust: &[u8],
    count: &mut usize,
) -> (Vec<u8>, Option<i32>) {
    let parse = |bytes: &[u8]| py::loads(std::str::from_utf8(bytes).ok()?.trim_end());
    let (Some(mut tree), Some(theirs)) = (parse(&python.0), parse(rust)) else {
        return python;
    };
    let before = *count;
    declared(&mut tree, &theirs, count);
    if *count == before {
        return python;
    }
    (
        format!("{}\n", py::dumps(&tree).unwrap()).into_bytes(),
        python.1,
    )
}

#[derive(Default)]
struct Report {
    cases: usize,
    declared: usize,
    failures: Vec<String>,
    outputs: String,
}

impl Report {
    fn compare(
        &mut self,
        name: &str,
        python: (Vec<u8>, Option<i32>),
        rust: (Vec<u8>, Option<i32>),
    ) {
        self.cases += 1;
        let python = with_declared(python, &rust.0, &mut self.declared);
        if python != rust {
            self.failures.push(format!(
                "{name}:\n  python {:?} {}\n  rust   {:?} {}",
                python.1,
                String::from_utf8_lossy(&python.0),
                rust.1,
                String::from_utf8_lossy(&rust.0)
            ));
        }
        if python.0.is_empty() || !python.0.ends_with(b"\n") {
            self.failures
                .push(format!("{name}: the Python oracle printed no response"));
        }
        self.outputs.push_str(&String::from_utf8_lossy(&rust.0));
    }

    fn expect(&mut self, needle: &str) {
        if !self.outputs.contains(needle) {
            self.failures
                .push(format!("content guard: no output contains {needle:?}"));
        }
    }
}

fn outcome(value: &Value) -> (Vec<u8>, Option<i32>) {
    (
        value["stdout"].as_str().unwrap().as_bytes().to_vec(),
        value["code"].as_i64().map(|c| c as i32),
    )
}

fn json(value: &str) -> String {
    value.to_owned()
}

/// The retired runtime's `ExecutionRoute(...).id` for the fixture's catalog route.
fn route_id(fixture: &Fixture) -> String {
    fixture.oracle("route_id").as_str().unwrap().to_owned()
}

fn registry() -> String {
    json(
        r#"{"schema_version": 1, "connections": {"cn_000000000001": {"id": "cn_000000000001", "name": "Fixture Ω",
        "engine": "codex-acp", "provider": "Fixture", "auth_mode": "native_account", "key_kind": null, "endpoint": null,
        "launch_command": ["/fixture/never-execute"], "credential_ref": {"kind": "engine_native_dir", "path": "/fixture/native",
        "owned": false}, "approved_fingerprint": "untrusted", "state": "configured", "qualified": true,
        "created_at": "now", "updated_at": "now"}}}"#,
    )
}

fn snapshot(route: &str) -> String {
    format!(
        r#"{{"snapshot_id": "cs_000000000001", "created_at": "now", "entries": [{{"route": {{"id": "{route}",
        "engine": "codex-acp", "connection_id": "cn_000000000001", "model": "exact", "supported_parameters": [["mode", "agent"]]}},
        "connection_id": "cn_000000000001", "label": "exact", "disambiguated_label": "exact",
        "catalog_snapshot_id": "cs_000000000001", "evidence": {{"catalog_obtained": {{"source": "discovery",
        "adapter_version": "fixture", "observed_at": "now", "evidence_ref": null}}}}}}]}}"#
    )
}

fn main() {
    let mut fixture = Fixture::new();
    let mut report = Report::default();
    let project = fixture.project();
    let route = route_id(&fixture);
    let ceiling = fixture.oracle("ceiling").as_str().unwrap().to_owned();
    let profile = |settings: &str, active: &str| {
        format!(
            r#"{{"format": 2, "active": {active}, "profiles": {{"a": {{"description": "Fixture Ω", "sections": ["routing"],
            "settings": {settings}}}, "z": {{"description": "floats", "sections": ["routing"],
            "settings": {{"routing": {{"extra": [1e-06, 1e+16, -0.0, 1.25, 0.0001, "Ω"]}}}}}}}}}}"#
        )
    };
    let reads = |project: &str| -> Vec<(String, String)> {
        let p = format!(r#","project":"{project}""#);
        let mut out = vec![];
        for (surface, projected) in [
            ("user_defaults", false),
            ("routing", true),
            ("routing_profiles", true),
            ("mcp_servers", false),
            ("connections", false),
            ("route_catalog", true),
            ("all", true),
        ] {
            out.push((
                surface.to_owned(),
                format!(r#"{{"api_version":1,"action":"read","surface":"{surface}"}}"#),
            ));
            if projected {
                out.push((
                    format!("{surface}+project"),
                    format!(r#"{{"api_version":1,"action":"read","surface":"{surface}"{p}}}"#),
                ));
            }
        }
        out
    };
    let cases: Vec<(&str, Vec<(&str, String)>, bool)> = vec![
        ("absent", vec![], false),
        (
            "owner-files",
            vec![
                (
                    "config/cortex/defaults.json",
                    format!(
                        r#"{{"defaults": {{"language": "ru", "extra": {{"kept": 1}}, "route_model": "m"}}, "ceiling": {ceiling}, "other": true}}"#
                    ),
                ),
                (
                    "config/cortex/routing.json",
                    json(
                        r#"{"model_policy_version": 2, "enabled": {"codex-acp": ["exact"]}, "deny": ["claude-agent-acp/*"], "unknown_key": 1}"#,
                    ),
                ),
                (
                    "config/cortex/mcp-servers.json",
                    json(
                        r#"{"mcpServers": {"docs": {"command": "/opt/never-run", "args": ["--token", "fixture-secret"],
                    "env": {"API_TOKEN": "fixture-secret"}, "allowed_tools": ["read"]}, "off": {"enabled": false}}}"#,
                    ),
                ),
                ("config/cortex/connections.json", registry()),
                (
                    "config/cortex/routing-profiles.json",
                    profile(r#"{"routing": {}}"#, r#""a""#),
                ),
                (
                    "state/cortex/catalog/cn_000000000001/cs_000000000001.json",
                    snapshot(&route),
                ),
            ],
            false,
        ),
        (
            "coordinator-bound",
            vec![
                (
                    "config/cortex/defaults.json",
                    json(
                        r#"{"defaults": {"coordinator": {"engine": "codex-acp", "model": "exact", "parameters": {"mode": "agent"}}}}"#,
                    ),
                ),
                (
                    "config/cortex/routing.json",
                    json(r#"{"model_policy_version": 2, "enabled": {"codex-acp": ["exact"]}}"#),
                ),
                ("config/cortex/connections.json", registry()),
                (
                    "state/cortex/catalog/cn_000000000001/cs_000000000001.json",
                    snapshot(&route),
                ),
            ],
            false,
        ),
        (
            "damaged-files",
            vec![
                ("config/cortex/defaults.json", json("{")),
                ("config/cortex/routing.json", json("[]")),
                ("config/cortex/mcp-servers.json", json("[]")),
                (
                    "config/cortex/routing-profiles.json",
                    json(r#"{"format": 9, "profiles": {}}"#),
                ),
            ],
            false,
        ),
        (
            "project-and-override",
            vec![
                (
                    "workspace/.cortex/routing.json",
                    json(r#"{"model_policy_version": 2, "deny": ["opencode/*"]}"#),
                ),
                ("override.json", json(r#"{"model_policy_version": 2}"#)),
                (
                    "config/cortex/routing-profiles.json",
                    profile(r#"{"routing": {}}"#, "null"),
                ),
            ],
            true,
        ),
    ];
    for (name, files, routing_override) in &cases {
        let files: Vec<(&str, String)> = files.iter().map(|(p, b)| (*p, b.clone())).collect();
        fixture.reset(&files, *routing_override);
        for (read, request) in reads(&project) {
            let label = format!("{name}/{read}");
            let python = outcome(&fixture.oracle(&format!("read/{label}")));
            let before = fixture.tree();
            let rust = fixture.rust(&request);
            if fixture.tree() != before {
                report
                    .failures
                    .push(format!("{label}: the Rust read changed the fixture"));
            }
            report.compare(&label, python, rust);
        }
    }

    // Request-shape refusals, in the Python dispatcher's order.
    fixture.reset(&[], false);
    let shapes = [
        ("invalid-json", "{".to_owned()),
        ("trailing-data", "{} x".to_owned()),
        ("not-object", "[1]".to_owned()),
        (
            "api-version-2",
            r#"{"api_version":2,"action":"read","surface":"all"}"#.to_owned(),
        ),
        (
            "api-version-true",
            r#"{"api_version":true,"action":"read","surface":"mcp_servers"}"#.to_owned(),
        ),
        (
            "api-version-float",
            r#"{"api_version":1.0,"action":"read","surface":"connections"}"#.to_owned(),
        ),
        (
            "unknown-surface",
            r#"{"action":"read","surface":"nope"}"#.to_owned(),
        ),
        ("missing-surface", r#"{"action":"read"}"#.to_owned()),
        (
            "unknown-fields",
            r#"{"action":"read","surface":"user_defaults","zz":1,"b":2,"project":"/x"}"#.to_owned(),
        ),
        (
            "all-unknown-field",
            r#"{"action":"read","surface":"all","other":null}"#.to_owned(),
        ),
        (
            "catalog-update",
            r#"{"action":"update","surface":"route_catalog"}"#.to_owned(),
        ),
        (
            "unsupported-action",
            r#"{"action":"remove","surface":"mcp_servers"}"#.to_owned(),
        ),
        (
            "unsupported-action-int",
            r#"{"action":7,"surface":"connections"}"#.to_owned(),
        ),
        (
            "unsupported-action-unicode",
            r#"{"action":"Ω'\"","surface":"routing"}"#.to_owned(),
        ),
        (
            "profile-unknown-verb",
            r#"{"action":"rename","surface":"routing_profiles"}"#.to_owned(),
        ),
        (
            "preset-unknown-action",
            r#"{"action":"update","surface":"default_preset"}"#.to_owned(),
        ),
        (
            "ceiling-update-missing",
            r#"{"action":"update","surface":"project_ceiling","project":"/x"}"#.to_owned(),
        ),
        (
            "write-unknown-field",
            r#"{"action":"update","surface":"routing","bogus":1}"#.to_owned(),
        ),
        (
            "profile-unknown-field",
            r#"{"action":"delete","surface":"routing_profiles","rename_to":"x"}"#.to_owned(),
        ),
        (
            "connections-unknown-field",
            r#"{"action":"install_engine","surface":"connections","binary":"/x"}"#.to_owned(),
        ),
    ];
    for (name, request) in &shapes {
        let python = outcome(&fixture.oracle(&format!("shape/{name}")));
        let rust = fixture.rust(request);
        report.compare(&format!("shape/{name}"), python, rust);
    }

    defaults_writes(&mut fixture, &mut report);
    defaults_write_fences(&mut fixture);
    workflows::run(&mut fixture, &mut report, &route);
    workflows::native_limits(&mut fixture, &mut report);

    // Content guards: the parity above is not vacuous.
    for needle in [
        "\"exists\": true",
        "\"language\": \"ru\"",
        "\"preserved_keys\": [\"extra\"]",
        "\"coordinator_resolution\": {\"connection_id\": \"cn_000000000001\"",
        "\"state\": \"bound\"",
        "\"effective_source\": \"project\"",
        "\"effective_source\": \"env\"",
        "\"precedence\": [\"project\", \"env\", \"user\"]",
        "\"ignored_keys\": [\"unknown_key\"]",
        "\"name\": \"Fixture Ω\"",
        "\"value_set\": true",
        "\"surface\": \"route_catalog\"",
        "\"route_count\": 1",
        "\"shadowed_by\": \"project\"",
        "\"code\": \"invalid_user_defaults\"",
        "\"code\": \"unsupported_profiles_format\"",
        "1e-06",
        "1e+16",
        "-0.0",
    ] {
        report.expect(needle);
    }
    // The API returns MCP env names only (arguments are redacted by the plugin).
    if report.outputs.contains("\"API_TOKEN\": \"fixture-secret\"") {
        report
            .failures
            .push("an MCP env value reached a response".into());
    }

    if report.failures.is_empty() {
        println!(
            "config parity: {} cases match the frozen Python outcomes byte-for-byte or refuse explicitly \
             ({} declared native message deltas applied)",
            report.cases, report.declared
        );
    } else {
        for failure in &report.failures {
            eprintln!("{failure}");
        }
        eprintln!(
            "config parity: {} of {} checks failed",
            report.failures.len(),
            report.cases
        );
        std::process::exit(1);
    }
}

fn defaults_write_fences(fixture: &mut Fixture) {
    use serde_json::Value;
    fixture.reset(&[], false);
    let request = r#"{"action":"update","surface":"user_defaults","expected_revision":"absent","defaults":{"language":"ru"}}"#;
    let (a, b) = std::thread::scope(|scope| {
        let left = scope.spawn(|| fixture.rust(request));
        let right = scope.spawn(|| fixture.rust(request));
        (left.join().unwrap(), right.join().unwrap())
    });
    let answers = [&a, &b].map(|r| serde_json::from_slice::<Value>(&r.0).unwrap());
    assert_eq!(answers.iter().filter(|v| v["ok"] == true).count(), 1);
    assert_eq!(
        answers
            .iter()
            .filter(|v| v["error"]["code"] == "revision_conflict")
            .count(),
        1
    );
    let body = fs::read(fixture.at("config/cortex/defaults.json")).unwrap();
    let revision = format!("sha256:{:x}", Sha256::digest(&body));
    assert!(answers
        .iter()
        .any(|v| v["error"]["current_revision"] == revision));
    assert!(!fixture.at("state/defaults.json").exists());
    fixture.reset(
        &[(
            "config/cortex/defaults.json",
            r#"{"defaults":{"language":"en"}}"#.into(),
        )],
        false,
    );
    let target = fixture.at("config/cortex/defaults.json");
    let sentinel = fixture.base.join("sentinel.json");
    fs::write(&sentinel, b"sentinel").unwrap();
    fs::remove_file(&target).unwrap();
    std::os::unix::fs::symlink(&sentinel, &target).unwrap();
    let (out, code) = fixture.rust(request);
    assert_eq!(code, Some(1));
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["error"]["code"], "unsafe_storage");
    assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");
    fs::remove_file(&target).unwrap();
    for raw in ["NaN", "Infinity", "18446744073709551616"] {
        let request = format!(
            r#"{{"action":"update","surface":"user_defaults","expected_revision":"absent","defaults":{{"language":{raw}}}}}"#
        );
        let (out, code) = fixture.rust(&request);
        assert_eq!(code, Some(1));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["error"]["code"], "config_api_unavailable");
        assert!(!target.exists());
    }
}

/// Write, inspect the actual bytes/modes, then read back using the same fixture root.
fn defaults_writes(fixture: &mut Fixture, report: &mut Report) {
    use serde_json::Value;
    let seed = r#"{"extra":{"keep":"Ω"},"defaults":{"language":"en","extra":42}}"#;
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "first-language",
            json!({"defaults":{"language":"ru"}}),
            false,
        ),
        (
            "preserve-metadata",
            json!({"defaults":{"language":"ru"}}),
            true,
        ),
        (
            "remove-language",
            json!({"defaults":{"language":null}}),
            true,
        ),
        ("no-patch", json!({}), true),
        (
            "portable",
            json!({"defaults":{"coordinator":{"engine":"codex-acp","model":"fixture","parameters":{}}}}),
            false,
        ),
        (
            "ceiling",
            json!({"ceiling":{"allowed_routes":["b","a"],"limits":{"tokens":10},"fallback_policy":{"budget":2,"data_recipient_consent":true}}}),
            false,
        ),
        ("clear-ceiling", json!({"ceiling":null}), true),
        (
            "preserve-ceiling",
            json!({"defaults":{"language":"ru"}}),
            true,
        ),
        (
            "bound-route",
            json!({"defaults":{"route_id":"xr_000000000001","route_connection_id":"cn_000000000001","route_model":"m","parameters":{"thought_level":"high"}}}),
            false,
        ),
        (
            "route-conflict",
            json!({"defaults":{"coordinator":{"engine":"codex-acp","model":"m","parameters":{}},"route_id":"xr_000000000001","route_connection_id":"cn_000000000001","route_model":"m"}}),
            false,
        ),
        (
            "stale",
            json!({"expected_revision":"absent","defaults":{"language":"ru"}}),
            true,
        ),
        (
            "bad-revision",
            json!({"expected_revision":null,"defaults":{"language":"ru"}}),
            false,
        ),
        ("bad-defaults", json!({"defaults":[]}), false),
        ("bad-language", json!({"defaults":{"language":"xx"}}), false),
        (
            "unknown-default",
            json!({"defaults":{"unknown":true}}),
            false,
        ),
        (
            "bad-coordinator",
            json!({"defaults":{"coordinator":{}}}),
            false,
        ),
        (
            "bad-parameters",
            json!({"defaults":{"parameters":{"invalid key":"value"}}}),
            false,
        ),
        (
            "incomplete-route",
            json!({"defaults":{"route_id":"xr_000000000001"}}),
            false,
        ),
        (
            "unknown-connection",
            json!({"defaults":{"route_id":"xr_000000000001","route_connection_id":"cn_000000000001","route_model":"m"}}),
            false,
        ),
        ("bad-ceiling-type", json!({"ceiling":[]}), false),
        (
            "bad-ceiling-field",
            json!({"ceiling":{"unknown":true}}),
            false,
        ),
        (
            "bad-ceiling-sets",
            json!({"ceiling":{"allowed_routes":["a","a"]}}),
            false,
        ),
        (
            "bad-limit",
            json!({"ceiling":{"limits":{"tokens":true}}}),
            false,
        ),
        (
            "bad-fallback",
            json!({"ceiling":{"fallback_policy":{"allowed_failure_classes":["refusal"]}}}),
            false,
        ),
        (
            "unenforceable-spend",
            json!({"ceiling":{"spend_limit":10}}),
            false,
        ),
    ];
    for (name, patch, populated) in cases {
        let mut document: Value = serde_json::from_str(seed).unwrap();
        if matches!(name, "clear-ceiling" | "preserve-ceiling") {
            document["ceiling"] = json!({"allowed_routes":["saved"],"allowed_connections":null,
                "allowed_data_recipients":null,"permission_level":null,"limits":{},"spend_limit":null,
                "spend_limit_enforceable":false,"fallback_policy":{"allowed_target_routes":null,
                "allowed_failure_classes":null,"budget":null,"data_recipient_consent":false}});
        }
        let seed = document.to_string();
        let mut files = if populated {
            vec![("config/cortex/defaults.json", seed.clone())]
        } else {
            vec![]
        };
        if name == "bound-route" {
            files.push(("config/cortex/connections.json", registry()));
        }
        let revision = if populated {
            format!("sha256:{:x}", Sha256::digest(seed.as_bytes()))
        } else {
            "absent".into()
        };
        let mut request =
            json!({"action":"update","surface":"user_defaults","expected_revision":revision});
        for (k, v) in patch.as_object().unwrap() {
            request[k] = v.clone();
        }
        let request = request.to_string();
        let recorded = fixture.oracle(&format!("defaults-write/{name}"));
        let python = outcome(&recorded["write"]);
        let python_tree = recorded["tree"].as_str().unwrap().to_owned();
        let python_read = outcome(&recorded["read"]);
        fixture.reset(&files, false);
        let rust = fixture.rust(&request);
        let rust_tree = fixture.tree();
        let rust_read = fixture.rust(r#"{"action":"read","surface":"user_defaults"}"#);
        if python_tree != rust_tree {
            report
                .failures
                .push(format!("defaults-write/{name}: file bytes or modes differ"));
        }
        if fixture.at("state/defaults.json").exists() {
            report
                .failures
                .push(format!("defaults-write/{name}: wrote Workspace defaults"));
        }
        if matches!(
            name,
            "first-language"
                | "preserve-metadata"
                | "remove-language"
                | "no-patch"
                | "portable"
                | "ceiling"
                | "clear-ceiling"
                | "preserve-ceiling"
                | "bound-route"
        ) && rust.1 != Some(0)
        {
            report.failures.push(format!(
                "defaults-write/{name}: expected a successful change"
            ));
        }
        report.compare(&format!("defaults-write/{name}"), python, rust);
        report.compare(
            &format!("defaults-read-back/{name}"),
            python_read,
            rust_read,
        );
    }
}

#[path = "config_parity/workflows.rs"]
mod workflows;

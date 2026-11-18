//! No-paid ACP v1 schema/startup probe of the real `cortex-acp` binary.
//!
//! Every frame the binary writes is validated against the verbatim ACP
//! TypeScript SDK 1.4.0 JSON Schema (`tests/fixtures/acp/schema-1.4.0.json`)
//! as `AgentResponse` or `AgentNotification`. The probe runs in a disposable
//! HOME/XDG with a disposable project and dispatches no engine prompt: a
//! prompt without a configured route fails closed with `orchestration_unavailable`.
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{channel, Receiver},
    time::Duration,
};

const SCHEMA: &str = include_str!("fixtures/acp/schema-1.4.0.json");

/// The subset of JSON Schema (draft 2020-12) the ACP schema uses: `$ref`,
/// `type` (single or list), `properties`, `required`, `additionalProperties`,
/// `items`, `const`, `enum`, `minimum`, `allOf`, `anyOf` and `oneOf`.
/// Annotation keywords and `format` are ignored.
struct Schema(Value);

impl Schema {
    fn definition(&self, name: &str) -> &Value {
        self.0["$defs"]
            .get(name)
            .unwrap_or_else(|| panic!("schema has no definition {name}"))
    }

    fn valid(&self, schema: &Value, value: &Value) -> Result<(), String> {
        self.check(schema, value, "$")
    }

    fn check(&self, schema: &Value, value: &Value, path: &str) -> Result<(), String> {
        let Some(object) = schema.as_object() else {
            return if schema == &Value::Bool(false) {
                Err(format!("{path}: forbidden"))
            } else {
                Ok(())
            };
        };
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            let name = reference
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("unsupported $ref {reference}"));
            self.check(self.definition(name), value, path)?;
        }
        if let Some(expected) = object.get("const") {
            if expected != value {
                return Err(format!("{path}: expected const {expected}"));
            }
        }
        if let Some(options) = object.get("enum").and_then(Value::as_array) {
            if !options.contains(value) {
                return Err(format!("{path}: not one of {options:?}"));
            }
        }
        if let Some(types) = object.get("type") {
            let names: Vec<&str> = match types {
                Value::String(name) => vec![name.as_str()],
                Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            let matches = |name: &str| match name {
                "null" => value.is_null(),
                "boolean" => value.is_boolean(),
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.is_number(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                other => panic!("unsupported type {other}"),
            };
            if !names.iter().any(|name| matches(name)) {
                return Err(format!("{path}: type {names:?} expected, got {value}"));
            }
        }
        if let (Some(minimum), Some(number)) = (object.get("minimum"), value.as_f64()) {
            if number < minimum.as_f64().unwrap() {
                return Err(format!("{path}: below minimum {minimum}"));
            }
        }
        if let Some(fields) = value.as_object() {
            let properties = object.get("properties").and_then(Value::as_object);
            if let Some(required) = object.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !fields.contains_key(name) {
                        return Err(format!("{path}: missing required {name}"));
                    }
                }
            }
            for (name, field) in fields {
                let declared = properties.and_then(|p| p.get(name));
                match (declared, object.get("additionalProperties")) {
                    (Some(schema), _) => self.check(schema, field, &format!("{path}.{name}"))?,
                    (None, Some(Value::Bool(false))) => {
                        return Err(format!("{path}: unexpected property {name}"))
                    }
                    (None, Some(extra @ Value::Object(_))) => {
                        self.check(extra, field, &format!("{path}.{name}"))?
                    }
                    _ => {}
                }
            }
        }
        if let (Some(items), Some(list)) = (object.get("items"), value.as_array()) {
            for (index, item) in list.iter().enumerate() {
                self.check(items, item, &format!("{path}[{index}]"))?;
            }
        }
        if let Some(all) = object.get("allOf").and_then(Value::as_array) {
            for branch in all {
                self.check(branch, value, path)?;
            }
        }
        for (keyword, exactly_one) in [("anyOf", false), ("oneOf", true)] {
            if let Some(branches) = object.get(keyword).and_then(Value::as_array) {
                let passing = branches
                    .iter()
                    .filter(|branch| self.check(branch, value, path).is_ok())
                    .count();
                if passing == 0 || (exactly_one && passing != 1) {
                    return Err(format!(
                        "{path}: {passing} {keyword} branches match {value}"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn schema() -> Schema {
    Schema(serde_json::from_str(SCHEMA).unwrap())
}

#[test]
fn the_validator_accepts_and_rejects_by_the_real_schema() {
    let schema = schema();
    let response = json!({"$ref": "#/$defs/AgentResponse"});
    let good =
        json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}});
    schema.valid(&response, &good).unwrap();
    let bad = json!({"jsonrpc":"2.0","id":1,"error":{"code":"not-an-integer","message":"m"}});
    assert!(schema.valid(&response, &bad).is_err());
    let notification = json!({"$ref": "#/$defs/AgentNotification"});
    let update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s",
        "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}}});
    schema.valid(&notification, &update).unwrap();
    // A known update kind with a wrongly typed field is refused.
    let mistyped = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":7,
        "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}}});
    // The generic notification envelope admits any params; the update payload
    // is checked against its own definition as well.
    schema.valid(&notification, &mistyped).unwrap();
    let session_update = json!({"$ref": "#/$defs/SessionNotification"});
    schema.valid(&session_update, &update["params"]).unwrap();
    assert!(schema.valid(&session_update, &mistyped["params"]).is_err());
}

struct Probe {
    child: Child,
    input: Option<ChildStdin>,
    frames: Receiver<Option<String>>,
    schema: Schema,
    root: PathBuf,
}

impl Probe {
    fn start(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("cxacp-schema-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for directory in ["project", "home"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let home = root.join("home");
        let mut child = Command::new(env!("CARGO_BIN_EXE_cortex-acp"))
            .env_clear()
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("PATH", "/usr/bin:/bin")
            .current_dir(root.join("project"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("cortex-acp");
        let (sender, frames) = channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.ok()).is_err() {
                    return;
                }
            }
            let _ = sender.send(None);
        });
        let input = child.stdin.take();
        Self {
            child,
            input,
            frames,
            schema: schema(),
            root,
        }
    }

    fn send(&mut self, id: i64, method: &str, params: Value) {
        let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{frame}").unwrap();
        input.flush().unwrap();
    }

    fn receive_frame(&self) -> Value {
        let line = self
            .frames
            .recv_timeout(Duration::from_secs(20))
            .expect("the binary answered within 20 seconds")
            .expect("the binary closed unexpectedly");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("non-JSON stdout frame: {line}"))
    }

    /// The next frame, validated as the given kind.
    fn receive(&self, kind: &str) -> Value {
        let frame = self.receive_frame();
        let definition = if kind == "response" {
            "AgentResponse"
        } else {
            "AgentNotification"
        };
        self.schema
            .valid(&json!({"$ref": format!("#/$defs/{definition}")}), &frame)
            .unwrap_or_else(|reason| panic!("{kind} violates the ACP schema: {reason}\n{frame}"));
        self.check_update(&frame);
        frame
    }

    /// `session/update` payloads must also satisfy `SessionNotification`.
    fn check_update(&self, frame: &Value) {
        if frame["method"] == "session/update" {
            self.schema
                .valid(
                    &json!({"$ref": "#/$defs/SessionNotification"}),
                    &frame["params"],
                )
                .unwrap_or_else(|reason| {
                    panic!("update violates the ACP schema: {reason}\n{frame}")
                });
        }
    }

    fn prompt(&mut self, id: i64, session: &str, text: &str) {
        self.send(
            id,
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":text}]}),
        );
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn binary_frames_are_schema_valid_and_startup_dispatches_no_engine() {
    let mut probe = Probe::start("startup");
    probe.send(
        1,
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{}}),
    );
    let init = probe.receive("response")["result"].clone();
    assert_eq!(init["protocolVersion"], 1);
    assert_eq!(init["agentCapabilities"]["loadSession"], true);
    // Admission snapshots real bytes before task dispatch; audio stays unsupported.
    assert_eq!(
        init["agentCapabilities"]["promptCapabilities"],
        json!({"image":true,"audio":false,"embeddedContext":true})
    );
    let channel = init["_meta"]["cortexChannel"].as_str().unwrap().to_owned();
    assert!(["release", "dev"].contains(&channel.as_str()));

    let project = probe.root.join("project");
    probe.send(2, "session/new", json!({"cwd":project,"mcpServers":[]}));
    let new = probe.receive("response")["result"].clone();
    let sid = new["sessionId"].as_str().unwrap().to_owned();
    let ids = |options: &Value| -> BTreeSet<String> {
        options
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        ids(&new["configOptions"]),
        BTreeSet::from(["language".to_owned(), "access_mode".to_owned()])
    );
    let mode = new["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "access_mode")
        .unwrap();
    assert_eq!(mode["currentValue"], "development");
    // Only Paseo receives access as ACP `modes`.
    assert!(new.get("modes").is_none());
    let values: BTreeSet<_> = mode["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, BTreeSet::from(["development", "guarded", "auto"]));
    let commands = probe.receive("notification")["params"]["update"]["availableCommands"].clone();
    let names: BTreeSet<_> = commands
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    for expected in [
        "cortex-status",
        "cortex-setup",
        "cortex-model",
        "cortex-effort",
        "cortex-language",
    ] {
        assert!(
            names.contains(expected),
            "{expected} missing from {names:?}"
        );
    }

    probe.prompt(3, &sid, "/cortex-status");
    assert_eq!(
        probe.receive("notification")["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        probe.receive("response")["result"]["stopReason"],
        "end_turn"
    );

    probe.prompt(30, &sid, "/cortex-language ru");
    assert_eq!(
        probe.receive("notification")["params"]["update"]["sessionUpdate"],
        "config_option_update"
    );
    assert_eq!(
        probe.receive("notification")["params"]["update"]["content"]["text"],
        "cortex-language set to 'ru' (pending — applies to the next run)."
    );
    assert_eq!(
        probe.receive("response")["result"]["stopReason"],
        "end_turn"
    );

    // An explicit harvest and an ordinary prompt are not commands; with no
    // route they fail closed before any engine is reached. An absolute path
    // is not a command-shaped slash token.
    for (id, text) in [
        (31, "/harvest"),
        (32, "/absolute/project/path"),
        (4, "Must not reach an engine"),
    ] {
        probe.prompt(id, &sid, text);
        let error = probe.receive("response")["error"].clone();
        assert_eq!(error["message"], "orchestration_unavailable");
        if id == 4 {
            assert_eq!(error["code"], -32001);
        }
    }

    // Every implemented method and intentional refusal stays schema-valid.
    probe.send(10, "authenticate", json!({"methodId":"unused"}));
    assert_eq!(
        probe.receive("response")["error"],
        json!({"code":-32601,"message":"Method 'authenticate' is not supported."})
    );
    probe.send(
        11,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"language","value":"en"}),
    );
    assert!(!probe.receive("response")["result"]["configOptions"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        probe.receive("notification")["params"]["update"]["sessionUpdate"],
        "config_option_update"
    );
    let refusals: Vec<(i64, &str, Value, &str)> = vec![
        (
            12,
            "session/set_mode",
            json!({"sessionId":sid,"modeId":"invalid"}),
            "'invalid' is not a valid mode; expected one of ['development', 'guarded', 'auto'].",
        ),
        (
            13,
            "session/set_config_option",
            json!({"sessionId":sid,"configId":"cortex_question_answer",
            "value":json!({"batchId":"missing","answers":[],"requestKey":"none"}).to_string()}),
            "invalid_question_answer",
        ),
        (
            14,
            "session/set_config_option",
            json!({"sessionId":sid,"configId":"cortex_queued_message",
            "value":json!({"clientMessageId":"no-task","content":[{"type":"text","text":"Not dispatched"}]}).to_string()}),
            "session_has_no_task",
        ),
        (
            15,
            "session/set_config_option",
            json!({"sessionId":sid,"configId":"cortex_stop_command",
            "value":json!({"toolCallRef":"none","reason":"Not running","requestKey":"stop"}).to_string()}),
            "session_has_no_task",
        ),
        (
            16,
            "_cortex/workspace_control",
            json!({}),
            "project_inactive",
        ),
        (
            17,
            "session/new",
            json!({"cwd":project,"mcpServers":[
            {"name":"ungranted","command":"/bin/false","args":[],"env":[]}]}),
            "external_mcp_server_unconfigured",
        ),
        (
            18,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"image","mimeType":"image/png","data":"%%%"}]}),
            "An image content block data field must be standard base64.",
        ),
        (
            19,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[
            {"type":"resource_link","name":"unfetched","uri":"https://example.invalid"}]}),
            "orchestration_unavailable",
        ),
    ];
    for (id, method, params, reason) in refusals {
        probe.send(id, method, params);
        assert_eq!(
            probe.receive("response")["error"]["message"],
            reason,
            "request {id}"
        );
    }

    // Reloading replays the transcript, interleaved with the recorded language
    // selections, in order.
    probe.send(20, "session/cancel", json!({"sessionId":sid}));
    probe.send(
        21,
        "session/load",
        json!({"cwd":project,"sessionId":sid,"mcpServers":[]}),
    );
    let mut languages = vec![];
    loop {
        let frame = probe.receive_frame();
        if frame.get("id").is_some() {
            probe
                .schema
                .valid(&json!({"$ref":"#/$defs/AgentResponse"}), &frame)
                .unwrap();
            assert!(!frame["result"]["configOptions"]
                .as_array()
                .unwrap()
                .is_empty());
            break;
        }
        probe
            .schema
            .valid(&json!({"$ref":"#/$defs/AgentNotification"}), &frame)
            .unwrap();
        probe.check_update(&frame);
        let replayed = &frame["params"]["update"];
        if replayed["sessionUpdate"] == "config_option_update" {
            let language = replayed["configOptions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == "language")
                .unwrap();
            languages.push(language["currentValue"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(languages, ["ru", "en"]);
    assert_eq!(
        probe.receive("notification")["params"]["update"]["sessionUpdate"],
        "available_commands_update"
    );

    // Exactly one project store exists for the logical project and channel, and
    // no runtime task was ever created.
    let directory = if channel == "dev" {
        ".cortex-dev"
    } else {
        ".cortex"
    };
    let databases: Vec<_> = fs::read_dir(&project)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".cortex"))
        .map(|e| e.path().join("cortex.sqlite3"))
        .filter(|p| p.is_file())
        .collect();
    assert_eq!(
        databases,
        vec![project.join(directory).join("cortex.sqlite3")]
    );

    drop(probe.input.take());
    assert!(probe.child.wait().unwrap().success());
}

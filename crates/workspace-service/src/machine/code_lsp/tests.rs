use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("cortex-code-language-{}", random_id().unwrap()));
        fs::create_dir(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// A subprocess exercising real Content-Length framing and peer lifecycle. It
// is deliberately selected only by cfg(test), not through Dev settings.
const FAKE: &str = r#"
set -e
export LC_ALL=C
emit() { printf 'Content-Length: %s\r\n\r\n%s' "${#1}" "$1"; }
diag() {
    emit '{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"'"$uri"'","version":'"$1"',"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"severity":2,"message":"'"$2"'"}]}}'
}
while IFS= read -r length && IFS= read -r blank; do
    length=${length%$'\r'}
    read -r -N "${length#Content-Length: }" body
    if [[ $body == *'"method":"initialize"'* ]]; then
        emit '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":{"openClose":true,"change":2}}}}'
    elif [[ $body == *'"method":"textDocument/didOpen"'* ]]; then
        [[ $body =~ \"uri\":\"([^\"]+)\" ]]
        uri=${BASH_REMATCH[1]}
        [[ $body =~ \"version\":([0-9]+) ]]
        version=${BASH_REMATCH[1]}
        emit '{"jsonrpc":"2.0","id":"apply","method":"workspace/applyEdit","params":{"edit":{}}}'
    elif [[ $body == *'"id":"apply"'* ]]; then
        [[ $body == *'"applied":false'* ]]
        diag "$version" "open snapshot"
    elif [[ $body == *'"method":"textDocument/didChange"'* ]]; then
        [[ $body =~ \"version\":([0-9]+) ]]
        new_version=${BASH_REMATCH[1]}
        [[ $body == *'"range":'* ]]
        if [[ $body == *'crash-once'* && ! -f crashed ]]; then
            : > crashed
            exit 23
        fi
        diag "$version" "stale diagnostic"
        diag null "stale unversioned diagnostic"
        version=$new_version
        diag "$version" "change snapshot"
    elif [[ $body == *'"method":"textDocument/definition"'* ]]; then
        [[ $body =~ \"id\":([0-9]+) ]]
        request=${BASH_REMATCH[1]}
        range='{"start":{"line":0,"character":0},"end":{"line":0,"character":2}}'
        emit '{"jsonrpc":"2.0","id":'"$request"',"result":[{"uri":"'"$uri"'","range":'"$range"'},{"targetUri":"'"$2"'","targetSelectionRange":'"$range"'},{"uri":"file:///etc/passwd","range":'"$range"'},{"uri":"'"$3"'","range":'"$range"'}]}'
    fi
done
"#;

async fn event(rx: &mut broadcast::Receiver<Value>, predicate: impl Fn(&Value) -> bool) -> Value {
    event_within(rx, Duration::from_secs(8), predicate).await
}

async fn event_within(
    rx: &mut broadcast::Receiver<Value>,
    wait: Duration,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    timeout(wait, async {
        loop {
            let value = rx.recv().await.unwrap();
            workspace_protocol::validate("CodeLspEvent", &value).unwrap();
            if predicate(&value) {
                return value;
            }
        }
    })
    .await
    .expect("expected language event")
}

#[tokio::test]
async fn code_lsp_peer_diagnostics_definition_versioning_and_crash_restart() {
    let f = Fixture::new();
    fs::write(f.0.join("a.rs"), "😀 disk\n").unwrap();
    fs::write(f.0.join("b.rs"), "ok\n").unwrap();
    fs::create_dir(f.0.join(".cortex")).unwrap();
    fs::write(f.0.join(".cortex/private.rs"), "no\n").unwrap();
    let program = Program::fixture(vec![
        "/bin/bash".into(),
        "-c".into(),
        FAKE.into(),
        "fixture".into(),
        "unused".into(),
        file_uri(&f.0.join("b.rs")),
        file_uri(&f.0.join(".cortex/private.rs")),
    ]);
    let (events, mut listener) = broadcast::channel(32);
    let key = Key {
        project: "project".into(),
        checkout: f.0.clone(),
        language: Language::Rust,
    };
    let (sender, receiver) = mpsc::channel(4);
    let session = tokio::spawn(Session::new(key, program, events).run(receiver));
    let id = "f".repeat(64);
    let doc = Document {
        id: id.clone(),
        event_id: "1".repeat(32),
        uri: file_uri(&f.0.join("a.rs")),
        version: 10,
        wire_version: 0,
        checkout_id: "project:main".into(),
        language_id: "rust",
        text: "😀 unsaved\n".into(),
        baseline: true,
        diagnostic_due: None,
        diagnostics: Value::Null,
    };
    let open = Manager::submit(sender.clone(), Action::Open(doc))
        .await
        .unwrap();
    assert_eq!(open["status"], "starting");
    assert_eq!(open["documentId"], id);
    let first = event(&mut listener, |p| {
        p["diagnostics"][0]["message"] == "open snapshot"
    })
    .await;
    assert_eq!(first["version"], 10);
    assert_eq!(first["generation"], 1);

    let bad = Manager::submit(
        sender.clone(),
        Action::Definition(Definition {
            document_id: id.clone(),
            version: 10,
            position: Position {
                line: 0,
                character: 1,
            },
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(bad.code, "invalid_request");
    let definition = Manager::submit(
        sender.clone(),
        Action::Definition(Definition {
            document_id: id.clone(),
            version: 10,
            position: Position {
                line: 0,
                character: 2,
            },
        }),
    )
    .await
    .unwrap();
    assert_eq!(definition["targets"].as_array().unwrap().len(), 2);
    assert_eq!(definition["targets"][1]["file"], "b.rs");
    assert_eq!(definition["targets"][1]["checkoutId"], "project:main");

    Manager::submit(
        sender.clone(),
        Action::Change(Change {
            document_id: id.clone(),
            version: 11,
            text: "😀 replacement\n".into(),
        }),
    )
    .await
    .unwrap();
    let latest = event(&mut listener, |p| {
        assert_ne!(p["diagnostics"][0]["message"], "stale diagnostic");
        assert_ne!(
            p["diagnostics"][0]["message"],
            "stale unversioned diagnostic"
        );
        p["diagnostics"][0]["message"] == "change snapshot"
    })
    .await;
    assert_eq!(latest["version"], 11);
    let stale = Manager::submit(
        sender.clone(),
        Action::Definition(Definition {
            document_id: id.clone(),
            version: 10,
            position: Position {
                line: 0,
                character: 2,
            },
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "lsp_document_stale");

    Manager::submit(
        sender.clone(),
        Action::Change(Change {
            document_id: id.clone(),
            version: 12,
            text: "😀 crash-once".into(),
        }),
    )
    .await
    .unwrap();
    let failure = event(&mut listener, |p| p["status"] == "lsp_unavailable").await;
    assert!(failure["diagnostics"].is_null());
    let recovered = event(&mut listener, |p| {
        p["generation"] == 2 && p["diagnostics"][0]["message"] == "open snapshot"
    })
    .await;
    assert_eq!(recovered["version"], 12);
    let observed = fs::read_to_string(f.0.join("a.rs")).unwrap();
    assert_eq!(observed, "😀 disk\n");

    Manager::submit(sender, Action::Close(id)).await.unwrap();
    timeout(Duration::from_secs(2), session)
        .await
        .unwrap()
        .unwrap();
}

// Hold the first pull replies until a second edit has invalidated them. A
// workbench-level stale guard cannot catch messages already labeled as current.
const FAKE_TS: &str = r#"
set -e
export LC_ALL=C
emit() { printf 'Content-Length: %s\r\n\r\n%s' "${#1}" "$1"; }
reply() {
    emit '{"jsonrpc":"2.0","id":'"$1"',"result":{"type":"response","success":true,"command":"'"$2"'","body":'"$3"'}}'
}
bad='[{"startLocation":{"line":1,"offset":1},"endLocation":{"line":1,"offset":3},"category":"error","message":"old pull"}]'
while IFS= read -r length && IFS= read -r blank; do
    length=${length%$'\r'}
    read -r -N "${length#Content-Length: }" body
    if [[ $body == *'"method":"initialize"'* ]]; then
        emit '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":1,"executeCommandProvider":{"commands":["typescript.tsserverRequest"]}}}}'
    elif [[ $body == *'"method":"textDocument/didOpen"'* ]]; then
        [[ $body =~ \"uri\":\"([^\"]+)\" ]]
        uri=${BASH_REMATCH[1]}
    elif [[ $body == *'"method":"textDocument/didChange"'* ]]; then
        if [[ $body == *'second edit'* ]]; then
            emit '{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"'"$uri"'","diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"severity":1,"message":"old push"}]}}'
            reply "$old_syntax" syntacticDiagnosticsSync "$bad"
            reply "$old_semantic" semanticDiagnosticsSync '[]'
            reply "$old_suggestion" suggestionDiagnosticsSync '[]'
            second=1
        fi
    elif [[ $body == *'"method":"workspace/executeCommand"'* ]]; then
        [[ $body == *'"command":"typescript.tsserverRequest"'* ]]
        [[ $body == *'"file":"'"$uri"'"'* ]]
        [[ $body =~ \"id\":([0-9]+) ]]
        id=${BASH_REMATCH[1]}
        [[ $body =~ \"([[:alpha:]]+DiagnosticsSync)\" ]]
        command=${BASH_REMATCH[1]}
        if [[ $second ]]; then
            reply "$id" "$command" '[]'
        else
            case "$command" in
                syntacticDiagnosticsSync) old_syntax=$id;;
                semanticDiagnosticsSync) old_semantic=$id;;
                suggestionDiagnosticsSync)
                    old_suggestion=$id
                    # A request to the client marks both the in-flight pull and
                    # our readiness for the second edit without a sleep.
                    emit '{"jsonrpc":"2.0","id":"pull-started","method":"workspace/workspaceFolders"}'
                    ;;
            esac
        fi
    elif [[ $body == *'"id":"pull-started"'* ]]; then
        : > pull-started
    fi
done
"#;

#[tokio::test]
async fn code_lsp_typescript_drops_delayed_push_and_obsolete_pull() {
    let f = Fixture::new();
    let program = Program::fixture(vec!["/bin/bash".into(), "-c".into(), FAKE_TS.into()]);
    let (events, mut listener) = broadcast::channel(32);
    let (sender, receiver) = mpsc::channel(4);
    let key = Key {
        project: "project".into(),
        checkout: f.0.clone(),
        language: Language::TypeScript,
    };
    let session = tokio::spawn(Session::new(key, program, events).run(receiver));
    let id = "f".repeat(64);
    Manager::submit(
        sender.clone(),
        Action::Open(Document {
            id: id.clone(),
            event_id: "1".repeat(32),
            uri: file_uri(&f.0.join("a.ts")),
            version: 1,
            wire_version: 0,
            checkout_id: "project:main".into(),
            language_id: "typescript",
            text: "const a = 1;".into(),
            baseline: true,
            diagnostic_due: None,
            diagnostics: Value::Null,
        }),
    )
    .await
    .unwrap();
    event(&mut listener, |p| p["status"] == "ready").await;

    let change = |version, text: &str| {
        Manager::submit(
            sender.clone(),
            Action::Change(Change {
                document_id: id.clone(),
                version,
                text: text.into(),
            }),
        )
    };
    assert!(change(2, "// first edit").await.unwrap()["diagnostics"].is_null());
    timeout(Duration::from_secs(4), async {
        while !f.0.join("pull-started").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first diagnostic pull must be in flight");
    assert!(change(3, "// second edit").await.unwrap()["diagnostics"].is_null());
    let latest = event(&mut listener, |p| {
        assert_eq!(p["status"], "ready");
        assert!(
            p["diagnostics"].is_null() || p["diagnostics"] == json!([]),
            "{p}"
        );
        p["diagnostics"] == json!([])
    })
    .await;
    assert_eq!(latest["version"], 3);
    Manager::submit(sender, Action::Close(id)).await.unwrap();
    timeout(Duration::from_secs(2), session)
        .await
        .unwrap()
        .unwrap();
}

// Fail three consecutive pulls for the same edit: a JSON-RPC error, a
// malformed tsserver response, and a timeout. Respond to the timed-out request
// only once the next pull is in flight, then serve fresh diagnostics.
const FAKE_TS_RETRY: &str = r#"
set -e
export LC_ALL=C
emit() { printf 'Content-Length: %s\r\n\r\n%s' "${#1}" "$1"; }
reply() {
    emit '{"jsonrpc":"2.0","id":'"$1"',"result":{"type":"response","success":true,"command":"'"$2"'","body":'"$3"'}}'
}
item='{"startLocation":{"line":1,"offset":1},"endLocation":{"line":1,"offset":3},"category":"error","message":'
attempt=0
while IFS= read -r length && IFS= read -r blank; do
    length=${length%$'\r'}
    read -r -N "${length#Content-Length: }" body
    if [[ $body == *'"method":"initialize"'* ]]; then
        emit '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":1,"executeCommandProvider":{"commands":["typescript.tsserverRequest"]}}}}'
    elif [[ $body == *'"method":"textDocument/didChange"'* ]]; then
        [[ -z $changed && $body == *'retry edit'* ]]
        changed=1
    elif [[ $body == *'"method":"workspace/executeCommand"'* ]]; then
        [[ $changed && $body =~ \"id\":([0-9]+) ]]
        id=${BASH_REMATCH[1]}
        [[ $body =~ \"([[:alpha:]]+DiagnosticsSync)\" ]]
        command=${BASH_REMATCH[1]}
        if [[ $command == syntacticDiagnosticsSync ]]; then
            attempt=$((attempt + 1))
            case $attempt in
                1) emit '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32603,"message":"try again"}}' ;;
                2) reply "$id" "$command" '{}' ;;
                3) expired=$id ;;
                4)
                    [[ $cancelled ]]
                    reply "$expired" "$command" "[$item\"expired diagnostics\"}]"
                    reply "$id" "$command" "[$item\"recovered diagnostics\"}]"
                    ;;
                *) exit 1 ;;
            esac
        else
            reply "$id" "$command" '[]'
        fi
    elif [[ $body == *'"method":"$/cancelRequest"'* ]]; then
        [[ $body == *'"params":{"id":'"$expired"'}'* ]]
        cancelled=1
    fi
done
"#;

#[tokio::test]
async fn code_lsp_typescript_retries_failed_and_timed_out_pulls_without_an_edit() {
    let f = Fixture::new();
    let program = Program::fixture(vec!["/bin/bash".into(), "-c".into(), FAKE_TS_RETRY.into()]);
    let (events, mut listener) = broadcast::channel(32);
    let (sender, receiver) = mpsc::channel(4);
    let key = Key {
        project: "project".into(),
        checkout: f.0.clone(),
        language: Language::TypeScript,
    };
    let session = tokio::spawn(Session::new(key, program, events).run(receiver));
    let id = "f".repeat(64);
    Manager::submit(
        sender.clone(),
        Action::Open(Document {
            id: id.clone(),
            event_id: "1".repeat(32),
            uri: file_uri(&f.0.join("a.ts")),
            version: 1,
            wire_version: 0,
            checkout_id: "project:main".into(),
            language_id: "typescript",
            text: "const a = 1;".into(),
            baseline: true,
            diagnostic_due: None,
            diagnostics: Value::Null,
        }),
    )
    .await
    .unwrap();
    let opened = event(&mut listener, |p| p["status"] == "ready").await;
    let change = Manager::submit(
        sender.clone(),
        Action::Change(Change {
            document_id: id.clone(),
            version: 2,
            text: "// retry edit".into(),
        }),
    )
    .await
    .unwrap();
    assert!(change["diagnostics"].is_null());

    let recovered = event_within(&mut listener, Duration::from_secs(30), |p| {
        assert_eq!(p["status"], "ready", "{p}");
        assert_eq!(p["generation"], opened["generation"], "{p}");
        assert_eq!(p["version"], 2, "{p}");
        if p["diagnostics"].is_null() {
            return false;
        }
        let items = p["diagnostics"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{p}");
        assert_eq!(items[0]["message"], "recovered diagnostics", "{p}");
        true
    })
    .await;
    let status = Manager::submit(sender.clone(), Action::Status(id.clone()))
        .await
        .unwrap();
    assert_eq!(status["version"], 2);
    assert_eq!(status["diagnostics"], recovered["diagnostics"]);
    Manager::submit(sender, Action::Close(id)).await.unwrap();
    timeout(Duration::from_secs(2), session)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn code_lsp_resolution_dev_override_only_and_missing_bundle() {
    let f = Fixture::new();
    let dev = f.0.join("local-analyzer");
    fs::write(&dev, "fixture").unwrap();
    fs::set_permissions(&dev, fs::Permissions::from_mode(0o755)).unwrap();
    let settings = json!({"rustAnalyzerPath":dev});
    assert!(Program::resolve(Language::Rust, true, &settings, None).is_ok());
    let err = Program::resolve(Language::Rust, false, &settings, None)
        .err()
        .unwrap();
    assert_eq!(err.code, "lsp_unavailable");
    assert_eq!(err.detail.unwrap()["reason"], "server_not_installed");
    let bundled = f.0.join("engines/lsp/rust-analyzer");
    fs::create_dir_all(bundled.parent().unwrap()).unwrap();
    fs::copy(&dev, &bundled).unwrap();
    assert!(Program::resolve(Language::Rust, false, &settings, Some(&f.0.join("runtime"))).is_ok());
    for ext in [
        "example.ts",
        "example.tsx",
        "example.js",
        "example.jsx",
        "example.mts",
    ] {
        assert!(matches!(language(ext), Some((Language::TypeScript, _))));
    }
    assert!(language("readme.md").is_none());
}

#[test]
fn code_lsp_paths_are_checkout_local_and_not_aliases() {
    let f = Fixture::new();
    let private = f.0.join(".codex");
    fs::create_dir(&private).unwrap();
    fs::write(private.join("private.rs"), "secret").unwrap();
    symlink(private.join("private.rs"), f.0.join("alias.rs")).unwrap();
    fs::hard_link(private.join("private.rs"), f.0.join("hard.rs")).unwrap();
    for path in [
        ".codex/private.rs",
        "alias.rs",
        "hard.rs",
        "../outside.rs",
        "/etc/passwd",
    ] {
        assert!(files::lsp_text(&f.0, path).is_err(), "{path}");
    }
    let name = "заметка #%.rs";
    assert_eq!(
        uri_file(&file_uri(&f.0.join(name))).unwrap(),
        f.0.join(name)
    );
    assert!(uri_file("file://remote.example/etc/passwd").is_none());
    assert!(uri_file("file:///etc/%2e%2e/passwd").is_none());
}

fn sandbox_cargo(root: &Path) {
    let sandbox =
        crate::native_command_sandbox::Sandbox::prepare_language(root, &[], true).unwrap();
    let mut command = sandbox
        .command(&[
            "/bin/sh".into(),
            "-c".into(),
            r#"case ":$PATH:" in *:/tmp/home/.cargo/bin:*) echo path=yes;; *) echo path=no;; esac
[ -d /tmp/home ] && echo scratch=yes
[ -d /tmp/home/.cargo/bin ] && echo bindir=yes
[ -x /tmp/home/.cargo/bin/cargo ] && echo executable=yes
[ -f /tmp/home/.rustup/settings.toml ] && echo rustup_settings=yes
test ! -r .cortex/private.rs || exit 85
(touch write-forbidden) 2>/dev/null && exit 86
cargo metadata --locked --offline --format-version=1 --no-deps"#
                .into(),
        ])
        .unwrap();
    let mount_added = command
        .get_args()
        .any(|arg| arg == "/tmp/home/.cargo/bin/cargo");
    let host_home_has_cargo = std::env::var_os("HOME")
        .is_some_and(|home| PathBuf::from(home).join(".cargo/bin/cargo").is_file());
    let host_cargo_home_has_cargo = std::env::var_os("CARGO_HOME")
        .is_some_and(|home| PathBuf::from(home).join("bin/cargo").is_file());
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shell_flags: Vec<_> = stdout
        .lines()
        .filter(|line| {
            [
                "path=",
                "scratch=",
                "bindir=",
                "executable=",
                "rustup_settings=",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    assert!(output.status.success(), "Sandbox Cargo metadata: exit {:?}; {:?}; mount={mount_added}; HOME Cargo={host_home_has_cargo}; CARGO_HOME Cargo={host_cargo_home_has_cargo}; lockfile={}, read-only={}, toolchain={}, not-found={}, path-error={}, sandbox={}",
        output.status.code(), shell_flags, stderr.contains("lockfile"), stderr.to_lowercase().contains("read-only"),
        stderr.contains("toolchain"), stderr.contains("not found"), stderr.contains("No such file"), stderr.contains("bwrap"));
    assert!(!root.join("write-forbidden").exists());
}

#[test]
#[ignore = "requires usable bubblewrap namespaces and a local Rust toolchain"]
fn code_lsp_sandbox_projects_are_read_only() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("src")).unwrap();
    fs::create_dir(f.0.join(".cortex")).unwrap();
    fs::write(f.0.join(".cortex/private.rs"), "known fixture").unwrap();
    fs::write(
        f.0.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        f.0.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.0.1\"\n",
    )
    .unwrap();
    fs::write(f.0.join("src/lib.rs"), "pub fn test() {}\n").unwrap();
    sandbox_cargo(&f.0);
}

/// Requires an actual staging tree from bundle-engines and a user namespace.
/// No models, no desktop session, no network from either language server.
#[tokio::test]
#[ignore = "requires staged pinned language payload and usable bubblewrap namespaces"]
async fn code_lsp_pinned_servers_diagnostics_and_definitions() {
    let runtime = PathBuf::from(
        std::env::var_os("CORTEX_CODE_TEST_RUNTIME").expect("sibling of staged engines"),
    );
    for (language, language_id, file, target, text) in [
        (
            Language::Rust,
            "rust",
            "src/lib.rs",
            "src/helper.rs",
            "mod helper;\npub fn caller() { helper::target(); let broken = ; }\n",
        ),
        (
            Language::TypeScript,
            "typescript",
            "a.ts",
            "b.ts",
            "import { target } from './b';\ntarget();\nexport const bad: number = 'not a number';\n",
        ),
    ] {
        let f = Fixture::new();
        if language == Language::Rust {
            fs::create_dir(f.0.join("src")).unwrap();
            fs::write(
                f.0.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
            )
            .unwrap();
            fs::write(
                f.0.join("Cargo.lock"),
                "version = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.0.1\"\n",
            )
            .unwrap();
            fs::write(f.0.join(target), "pub fn target() {}\n").unwrap();
        } else {
            fs::write(
                f.0.join("tsconfig.json"),
                r#"{"compilerOptions":{"strict":true,"noEmit":true}}"#,
            )
            .unwrap();
            fs::write(f.0.join(target), "export function target() {}\n").unwrap();
        }
        fs::write(f.0.join(file), text).unwrap();
        let program = Program::resolve(language, false, &Value::Null, Some(&runtime))
            .unwrap_or_else(|error| panic!("{language_id}: {}", error.code));
        let (events, mut listener) = broadcast::channel(32);
        let (sender, receiver) = mpsc::channel(2);
        let key = Key {
            project: "project".into(),
            checkout: f.0.clone(),
            language,
        };
        let session = tokio::spawn(Session::new(key, program, events).run(receiver));
        let id = "f".repeat(64);
        Manager::submit(
            sender.clone(),
            Action::Open(Document {
                id: id.clone(),
                event_id: "2".repeat(32),
                uri: file_uri(&f.0.join(file)),
                version: 20,
                wire_version: 0,
                checkout_id: "project:main".into(),
                language_id,
                text: text.into(),
                baseline: true,
                diagnostic_due: None,
                diagnostics: Value::Null,
            }),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(40), async {
            loop {
                let event = listener.recv().await.unwrap();
                assert_ne!(event["status"], "lsp_unavailable", "{language_id}: {event}");
                if event["diagnostics"]
                    .as_array()
                    .is_some_and(|d| !d.is_empty())
                {
                    workspace_protocol::validate("CodeLspEvent", &event).unwrap();
                    break;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{language_id}: missing diagnostics"));

        let position = Position {
            line: 1,
            character: text.lines().nth(1).unwrap().find("target").unwrap() as u32,
        };
        let result = timeout(Duration::from_secs(25), async {
            loop {
                let result = Manager::submit(
                    sender.clone(),
                    Action::Definition(Definition {
                        document_id: id.clone(),
                        version: 20,
                        position,
                    }),
                )
                .await
                .unwrap_or_else(|error| panic!("{language_id}: definition: {}", error.code));
                if result["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["file"] == target)
                {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{language_id}: missing cross-file definition"));
        workspace_protocol::validate("CodeLspDefinitionResult", &result).unwrap();
        if language == Language::TypeScript {
            let clean = text.replace("'not a number'", "42");
            let changed_error = text.replace("'not a number'", "true");
            for (version, edited, expected_error) in
                [(21, clean.as_str(), false), (22, changed_error.as_str(), true)]
            {
                let changed = Manager::submit(
                    sender.clone(),
                    Action::Change(Change {
                        document_id: id.clone(),
                        version,
                        text: edited.into(),
                    }),
                )
                .await
                .unwrap();
                assert_eq!(changed["version"], version);
                assert!(changed["diagnostics"].is_null());
                let latest = event(&mut listener, |event| {
                    assert_eq!(event["status"], "ready", "{event}");
                    event["version"] == version && event["diagnostics"].is_array()
                })
                .await;
                let diagnostics = latest["diagnostics"].as_array().unwrap();
                if expected_error {
                    assert!(
                        diagnostics.iter().any(|d| {
                            d["severity"] == 1
                                && d["range"]["start"]["line"] == 2
                                && d["message"]
                                    .as_str()
                                    .unwrap()
                                    .contains("Type 'boolean' is not assignable to type 'number'")
                        }),
                        "{latest}"
                    );
                } else {
                    assert!(diagnostics.is_empty(), "{latest}");
                }
                let status = Manager::submit(sender.clone(), Action::Status(id.clone()))
                    .await
                    .unwrap();
                assert_eq!(status["version"], version);
                assert_eq!(status["diagnostics"], latest["diagnostics"]);
            }
        }
        Manager::submit(sender, Action::Close(id)).await.unwrap();
        timeout(Duration::from_secs(2), session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fs::read_to_string(f.0.join(file)).unwrap(), text);
    }
}

#[tokio::test]
async fn code_lsp_transport_rejects_ambiguous_or_unbounded_frames() {
    for payload in [
        "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        "Content-Length: 2097152\r\n\r\n{}",
        "Content-Length: 2\n\n{}",
        "Content-Length: 2\r\n\r\n{}",
    ] {
        let mut source = payload.as_bytes();
        assert!(transport::read(&mut source).await.is_err());
    }
}

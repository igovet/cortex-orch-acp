use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
fn config(base: &Path) -> crate::Config {
    let owner = base.join("config/cortex");
    fs::create_dir_all(&owner).unwrap();
    fs::set_permissions(&owner, fs::Permissions::from_mode(0o700)).unwrap();
    let mut launch = crate::LaunchSpec::bare();
    launch.dev_config = Some(base.join("config"));
    crate::Config {
        runtime_dir: base.join("run"),
        state_dir: base.join("state"),
        allowed_root: base.into(),
        launch,
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(100),
        event_capacity: 16,
    }
}
fn bind(v: &Value, base: &Path) -> Value {
    match v {
        Value::String(s) => json!(s.replace("$BASE", base.to_str().unwrap())),
        Value::Array(a) => json!(a.iter().map(|v| bind(v, base)).collect::<Vec<_>>()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), bind(v, base))).collect())
        }
        v => v.clone(),
    }
}
/// The scripted MCP peer of the fixture corpus as a POSIX shell script: one scripted row per
/// request (a `sleep`, a `raw` line, a `repeat`ed line, notifications, or a reply), then it idles.
fn scripted_peer(replies: &Value) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let truthy = |row: &Value, key: &str| {
        row.get(key).is_some_and(|v| match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64() != Some(0.0),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        })
    };
    let mut script = String::new();
    for row in replies.as_array().unwrap() {
        script += r#"read -r request || exit 0
case "$request" in *notifications/initialized*) read -r request || exit 0;; esac
id=${request#*\"id\":}; id=${id%%,*}; id=${id%%\}*}
"#;
        if truthy(row, "sleep") {
            script += &format!("sleep {}\n", row["sleep"]);
        } else if let Some(raw) = row.get("raw") {
            script += &format!("printf '%s\\n' {}\n", quote(raw.as_str().unwrap()));
        } else if let Some(repeat) = row.get("repeat") {
            script += &format!("yes x | head -n {repeat} | tr -d '\\n'; echo\n");
        } else {
            for _ in 0..row
                .get("notifications")
                .and_then(Value::as_u64)
                .unwrap_or(0)
            {
                script += "printf '{\"method\":\"notice\"}\\n'\n";
            }
            if truthy(row, "many") {
                script += &format!(
                    r#"tools='{{"name":"read"}}'; n=1
while [ $n -lt {} ]; do tools="$tools,"'{{"name":"read"}}'; n=$((n+1)); done
result="{{\"tools\":[$tools]}}"
"#,
                    row["many"]
                );
            } else {
                let result = row.get("result").cloned().unwrap_or(Value::Null);
                script += &format!("result={}\n", quote(&result.to_string()));
            }
            let id = row
                .get("id")
                .map_or_else(|| "\"$id\"".to_owned(), |id| quote(&id.to_string()));
            let error = if truthy(row, "error") {
                r#","error":{"message":"fixture-only"}"#
            } else {
                ""
            };
            script += &format!(
                "printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s{error}}}\\n' {id} \"$result\"\n"
            );
        }
    }
    script + "sleep 30\n"
}
fn prepare(case: &Value, fixture: &Value, base: &Path) -> (crate::Config, Value) {
    let config = config(base);
    let path = base.join("config/cortex/mcp-servers.json");
    if case.get("saved").is_some() || case.get("savedRaw").is_some() {
        fs::write(
            &path,
            case["savedRaw"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| case["saved"].to_string()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut server = bind(&case["server"], base);
    if server["command"] == "$PEER" {
        server["command"] = json!("/bin/sh");
        server["args"] = json!(["-c", scripted_peer(&case["replies"])]);
    }
    let _ = fixture;
    (config, server)
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/mcp-discovery-python.json"
    ))
    .unwrap()
}
fn normalize(reply: &mut Value, before: u64, after: u64) {
    if reply["result"]["ok"] == true {
        let at = reply["result"]["observation"]["at"]
            .as_u64()
            .expect("real observation time");
        assert!((before..=after).contains(&at));
        reply["result"]["observation"]["at"] = json!(1234567890125u64);
    }
}
#[tokio::test]
async fn discovery_matches_the_frozen_oracle_without_helper_or_engine() {
    let fixture = fixture();
    for case in fixture["redactions"].as_array().unwrap() {
        assert_eq!(
            mcp_redact::scrub(case["input"].as_str().unwrap()),
            case["expected"],
            "redaction case"
        );
    }
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 77);
    for case in fixture["cases"].as_array().unwrap() {
        let base = Scratch::new().unwrap();
        let (config, server) = prepare(case, &fixture, &base.0);
        let registry = base.0.join("config/cortex/mcp-servers.json");
        let before_bytes = fs::read(&registry).ok();
        let before = now_ms();
        let mut reply = if case.get("timeout").is_some() {
            json!({"ok":true,"result":discover_with_timeout(&server,&config,Duration::from_millis(100)).await})
        } else {
            let service = crate::Service::bind(config).await.unwrap();
            let result = service
                .machine(
                    serde_json::from_value(
                        json!({"op":"config.mcp.discover","params":{"server":server}}),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            service.stop().await.unwrap();
            result
        };
        normalize(&mut reply, before, now_ms());
        assert_eq!(reply, case["expected"], "{}", case["name"]);
        assert_eq!(fs::read(&registry).ok(), before_bytes, "registry mutated");
        assert!(!base.0.join(".cortex").exists());
    }
}
#[tokio::test]
async fn discovery_refuses_unsafe_owner_files_and_private_grants() {
    let base = Scratch::new().unwrap();
    let config = config(&base.0);
    let registry = base.0.join("config/cortex/mcp-servers.json");
    let outside = base.0.join("outside");
    fs::write(&outside, b"{}").unwrap();
    symlink(&outside, &registry).unwrap();
    let server = json!({"name":"sample","command":"/bin/false"});
    let reply = discover(&server, &config).await;
    assert_eq!(reply["diagnostic"], "configuration:PortableError");
    assert_eq!(fs::read(&outside).unwrap(), b"{}");
    fs::remove_file(&registry).unwrap();
    fs::hard_link(&outside, &registry).unwrap();
    assert_eq!(
        discover(&server, &config).await["diagnostic"],
        "configuration:PortableError"
    );
    fs::remove_file(&registry).unwrap();
    fs::write(&registry, b"{}").unwrap();
    fs::set_permissions(&registry, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        discover(&server, &config).await["diagnostic"],
        "configuration:PortableError"
    );
    fs::remove_file(&registry).unwrap();
    let grant_root = base.0.join("grant");
    fs::create_dir(&grant_root).unwrap();
    fs::create_dir(grant_root.join(".cortex")).unwrap();
    assert_eq!(
        discover(
            &json!({"name":"sample","command":"/bin/false","readPaths":[grant_root]}),
            &config
        )
        .await["code"],
        "path_refused"
    );
    fs::remove_dir(grant_root.join(".cortex")).unwrap();
    symlink(&outside, grant_root.join("link")).unwrap();
    assert_eq!(
        discover(
            &json!({"name":"sample","command":"/bin/false","writePaths":[grant_root]}),
            &config
        )
        .await["code"],
        "path_refused"
    );
}
#[tokio::test]
async fn discovery_kills_descendants_on_success() {
    let base = Scratch::new().unwrap();
    let config = config(&base.0);
    // A descendant that outlives the server's stdio shares its process group.
    let peer = r#"
sleep 30 &
pid=$!
while IFS= read -r line; do
    case "$line" in *'"id"'*) ;; *) continue ;; esac
    id=${line#*\"id\":}; id=${id%%,*}; id=${id%%\}*}
    case "$line" in
    *'"method":"initialize"'*)
        printf '{"id":%s,"result":{"protocolVersion":"2024-11-05"}}\n' "$id" ;;
    *)
        printf '{"id":%s,"result":{"tools":[{"name":"pid","description":"%s"}]}}\n' "$id" "$pid"
        break ;;
    esac
done
sleep 30
"#;
    let server = json!({"name":"sample","command":"/bin/sh","args":["-c",peer]});
    let reply = discover(&server, &config).await;
    assert_eq!(reply["ok"], true, "{reply}");
    let pid = reply["observation"]["tools"][0]["description"]
        .as_str()
        .unwrap()
        .parse::<i32>()
        .unwrap();
    // A killed orphan can remain a zombie until the host init reaps it.
    for _ in 0..50 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if stat.is_empty() || stat.split(") ").nth(1).is_some_and(|s| s.starts_with('Z')) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("discovery descendant remained alive");
}
async fn dead(pid: i32) {
    for _ in 0..100 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if stat.is_empty() || stat.split(") ").nth(1).is_some_and(|s| s.starts_with('Z')) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("discovery process {pid} remained alive");
}
#[tokio::test]
async fn discovery_cancels_and_times_out_with_process_group_cleanup() {
    for cancel in [false, true] {
        let base = Scratch::new().unwrap();
        let config = config(&base.0);
        let granted = base.0.join("granted");
        fs::create_dir(&granted).unwrap();
        let marker = granted.join("pids");
        let peer = r#"
sleep 30 &
echo "$$ $!" > "$1"
sleep 30
"#;
        let server = json!({"name":"sample","command":"/bin/sh","args":["-c",peer,"sh",marker],"writePaths":[granted]});
        let task = tokio::spawn(async move {
            discover_with_timeout(&server, &config, Duration::from_secs(2)).await
        });
        for _ in 0..100 {
            if fs::read_to_string(&marker).is_ok_and(|s| s.split_whitespace().count() == 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let ids = fs::read_to_string(marker).expect("contained peer wrote lifecycle marker");
        let ids = ids
            .split_whitespace()
            .map(|s| s.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(task.await.unwrap()["code"], "timeout");
        }
        for pid in ids {
            dead(pid).await;
        }
    }
}
#[tokio::test]
async fn discovery_limits_files_and_enforces_explicit_grants_without_tool_calls() {
    let base = Scratch::new().unwrap();
    let config = config(&base.0);
    let denied = base.0.join("ungranted");
    fs::write(&denied, b"fixture-private").unwrap();
    let allowed = base.0.join("allowed");
    fs::write(&allowed, b"readable").unwrap();
    let output = base.0.join("output");
    fs::create_dir(&output).unwrap();
    // $1 ungranted file, $2 read-only grant, $3 writable grant. Every denial is the sandbox's.
    let peer = r#"
if cat "$1" >/dev/null 2>&1; then exit 11; fi
[ "$(cat "$2")" = readable ] || exit 12
if (: > "$2") 2>/dev/null; then exit 13; fi
network=$( { exec 3<>/dev/tcp/127.0.0.1/9; } 2>&1 )
case "$network" in *"not permitted"*|*"Permission denied"*) ;; *) exit 14 ;; esac
if setsid -w true 2>/dev/null; then exit 15; fi
printf granted > "$3/marker"
while IFS= read -r line; do
    case "$line" in *'"id"'*) ;; *) continue ;; esac
    id=${line#*\"id\":}; id=${id%%,*}; id=${id%%\}*}
    case "$line" in
    *'"method":"initialize"'*) result='{"protocolVersion":"2024-11-05"}' ;;
    *'"method":"tools/list"'*) result='{"tools":[]}' ;;
    *) exit 16 ;;
    esac
    printf '{"id":%s,"result":%s}\n' "$id" "$result"
done
"#;
    let server = json!({"name":"sample","command":"/bin/bash","args":["-c",peer,"bash",denied,allowed,output],"readPaths":[allowed],"writePaths":[output]});
    assert_eq!(discover(&server, &config).await["ok"], true);
    assert_eq!(fs::read(output.join("marker")).unwrap(), b"granted");
    assert_eq!(fs::read(denied).unwrap(), b"fixture-private");
    let registry = base.0.join("config/cortex/mcp-servers.json");
    let f = fs::File::create(&registry).unwrap();
    fs::set_permissions(&registry, fs::Permissions::from_mode(0o600)).unwrap();
    f.set_len(FRAME_BUDGET as u64 + 1).unwrap();
    assert_eq!(
        discover(&server, &config).await["diagnostic"],
        "configuration:PortableError"
    );
}
/// `"containment": "none"` (the editor's value, else the saved entry's, else the
/// saved registry default) lists tools with no Landlock/seccomp, the owner's HOME
/// and the owner PATH; an explicit "contained" still applies the sandbox.
#[tokio::test]
async fn uncontained_discovery_runs_without_the_sandbox_only_when_chosen() {
    let base = Scratch::new().unwrap();
    let config = config(&base.0);
    let ungranted = base.0.join("ungranted");
    fs::write(&ungranted, b"fixture-private").unwrap();
    let home = std::env::var("HOME").unwrap();
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap();
    // $1 an ungranted file, $2 the expected HOME, $3 the service's runtime directory.
    let peer = r#"
[ "$(cat "$1" 2>/dev/null)" = fixture-private ] || exit 11
network=$( { exec 3<>/dev/tcp/127.0.0.1/9; } 2>&1 )
case "$network" in *"not permitted"*) exit 14 ;; esac
setsid -w true 2>/dev/null || exit 15
[ "$HOME" = "$2" ] || exit 16
case ":$PATH:" in *":$2/.local/bin:"*) ;; *) exit 17 ;; esac
grep -q '^Seccomp:[[:space:]]*0$' /proc/self/status || exit 18
[ "$XDG_RUNTIME_DIR" = "$3" ] && [ -z "${TMPDIR+set}" ] || exit 20
while IFS= read -r line; do
    case "$line" in *'"id"'*) ;; *) continue ;; esac
    id=${line#*\"id\":}; id=${id%%,*}; id=${id%%\}*}
    case "$line" in
    *'"method":"initialize"'*) result='{"protocolVersion":"2024-11-05"}' ;;
    *'"method":"tools/list"'*) result='{"tools":[{"name":"read"}]}' ;;
    *) exit 19 ;;
    esac
    printf '{"id":%s,"result":%s}\n' "$id" "$result"
done
"#;
    let server = |containment: Option<&str>| {
        let mut s = json!({"name":"sample","command":"/bin/bash",
            "args":["-c",peer,"bash",ungranted,home,runtime],
            // Grants are not checked for an uncontained server (this one is private).
            "readPaths":[base.0.join(".ssh")]});
        if let Some(c) = containment {
            s["containment"] = json!(c);
        }
        s
    };
    let listed =
        |reply: &Value| reply["ok"] == true && reply["observation"]["tools"][0]["name"] == "read";
    let reply = discover(&server(Some("none")), &config).await;
    assert!(listed(&reply), "{reply}");
    // Default contained: the private grant is refused before any launch.
    assert_eq!(
        discover(&server(None), &config).await["code"],
        "path_refused"
    );
    let registry = base.0.join("config/cortex/mcp-servers.json");
    fs::write(
        &registry,
        br#"{"version":1,"containment":"none","servers":{}}"#,
    )
    .unwrap();
    fs::set_permissions(&registry, fs::Permissions::from_mode(0o600)).unwrap();
    let reply = discover(&server(None), &config).await;
    assert!(listed(&reply), "registry default: {reply}");
    // An explicit "contained" overrides the default; the sandbox then denies the peer.
    let mut contained = server(Some("contained"));
    contained["readPaths"] = json!([]);
    let reply = discover(&contained, &config).await;
    assert_eq!(reply["ok"], false, "{reply}");
    assert_ne!(reply["code"], "path_refused");
    assert_eq!(
        discover(&server(Some("off")), &config).await["diagnostic"],
        "configuration:PortableError"
    );
}

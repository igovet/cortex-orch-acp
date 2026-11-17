//! Native "Download engine" against real retained-helper captures
//! (tests/generate_engine_install_fixture.py): replies, npm invocations and the
//! complete home/config trees, with the same offline fake node/npm executables.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU64, Ordering},
};

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/engine-install-python.json"
    ))
    .unwrap()
}
fn replace(v: &Value, base: &str) -> Value {
    serde_json::from_str(&serde_json::to_string(v).unwrap().replace("$BASE", base)).unwrap()
}
fn tree(base: &Path) -> Vec<Value> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<Value>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let meta = fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            if meta.is_dir() {
                out.push(json!([relative, "dir", mode]));
                walk(base, &path, out);
            } else {
                let digest = format!("{:x}", Sha256::digest(fs::read(&path).unwrap()));
                out.push(json!([relative, digest, mode]));
            }
        }
    }
    let mut out = vec![];
    for top in ["config", "home"] {
        let root = base.join(top);
        out.push(json!([
            top,
            "dir",
            fs::metadata(&root).unwrap().permissions().mode() & 0o777
        ]));
        walk(base, &root, &mut out);
    }
    // The generator sorts Python paths, which compare by their parts.
    out.sort_by(|a, b| {
        let parts = |v: &Value| {
            v[0].as_str()
                .unwrap()
                .split('/')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        parts(a).cmp(&parts(b))
    });
    out
}
fn calls(base: &Path) -> Vec<Value> {
    fs::read_to_string(base.join("npm-calls"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn install_engine_captures_match_natively() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let c = corpus();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pinned-dependencies.json");
    let manifest_hash = format!("{:x}", Sha256::digest(fs::read(&manifest).unwrap()));
    assert_eq!(c["sources"]["pinned-dependencies.json"], manifest_hash);
    let cases = c["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 38);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let base = PathBuf::from(format!(
            "/tmp/cw-inst-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        for leaf in ["home", "config", "xstate", "state", "bin", "other"] {
            fs::create_dir(base.join(leaf)).unwrap();
            fs::set_permissions(base.join(leaf), fs::Permissions::from_mode(0o700)).unwrap();
        }
        if case["path"] != "none" {
            fs::write(base.join("bin/node"), c["fakeNode"].as_str().unwrap()).unwrap();
            fs::set_permissions(base.join("bin/node"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let npm = match case["npm"].as_str().unwrap() {
            "beside" => Some(base.join("bin/npm")),
            "path" => Some(base.join("other/npm")),
            _ => None,
        };
        if let Some(npm) = &npm {
            fs::write(npm, c["fakeNpm"].as_str().unwrap()).unwrap();
            fs::set_permissions(npm, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(base.join("npm-mode.json"), case["mode"].to_string()).unwrap();
        let engine = case["engine"].as_str().unwrap_or("opencode");
        let package = c["packages"][engine].as_str().unwrap_or("None");
        for (path, content) in case["setup"].as_object().unwrap() {
            let path = base.join(path.replace("{package}", package));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content.as_str().unwrap()).unwrap();
        }
        for private in ["config/cortex", "home/.codex", "home/.claude"] {
            let private = base.join(private);
            if private.exists() {
                fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
                for child in fs::read_dir(&private).unwrap().flatten() {
                    fs::set_permissions(child.path(), fs::Permissions::from_mode(0o600)).unwrap();
                }
            }
        }
        let path = match case["path"].as_str().unwrap() {
            "none" => base.join("other").into_os_string(),
            p => std::env::join_paths(p.split(':').map(|p| base.join(p))).unwrap(),
        };
        let home = base.join("home");
        let host = Host {
            route: RouteEnvironment {
                home: home.clone(),
                config: base.join("config"),
                state: base.join("xstate"),
                routing_override: None,
            },
            data: home.join(".local/share"),
            path: Some(path),
            bundle: None,
            manifest: manifest.clone(),
        };
        assert_eq!(host.unprojected(), None, "{name}");
        let held = (case["busy"] == true).then(|| {
            let lock = home.join(format!(".local/share/cortex/engines/{engine}/.lock"));
            fs::create_dir_all(lock.parent().unwrap()).unwrap();
            let file = fs::File::create(&lock).unwrap();
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            file
        });
        let environment = |command: &mut tokio::process::Command| {
            command
                .env_clear()
                .env("HOME", &home)
                .env("LANG", "C.UTF-8")
                .env("XDG_CONFIG_HOME", base.join("config"))
                .env("XDG_STATE_HOME", base.join("xstate"))
                .env("CORTEX_WORKSPACE_CHANNEL", "release")
                .env("CORTEX_WORKSPACE_STATE_DIR", base.join("state"))
                .env("PYTHONDONTWRITEBYTECODE", "1");
            Ok(())
        };
        let reply = envelope(if case["params"]["action"] == "onboard_opencode" {
            onboard_opencode(&host, &base.join("config/cortex"), &case["params"])
        } else {
            install(&host, engine, Instant::now() + TIMEOUT, &environment)
        });
        drop(held);
        let b = base.to_str().unwrap();
        assert_eq!(reply, replace(&case["expected"], b), "{name}");
        assert_eq!(
            calls(&base),
            replace(&case["npmCalls"], b).as_array().unwrap().clone(),
            "{name}"
        );
        assert_eq!(
            tree(&base),
            case["files"].as_array().unwrap().clone(),
            "{name}"
        );
        fs::remove_dir_all(&base).unwrap();
    }
}

#[test]
fn opencode_candidates_markers_and_registries_keep_the_retained_provider() {
    let base = PathBuf::from(format!("/tmp/cw-inst-r-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("home/.opencode/bin")).unwrap();
    fs::create_dir_all(base.join("config/cortex")).unwrap();
    fs::create_dir_all(base.join("bin")).unwrap();
    let host = |path: &str| Host {
        route: RouteEnvironment {
            home: base.join("home"),
            config: base.join("config"),
            state: base.join("state"),
            routing_override: None,
        },
        data: base.join("home/.local/share"),
        path: Some(base.join(path).into_os_string()),
        bundle: None,
        manifest: base.join("missing.json"),
    };
    assert!(!host("bin").opencode_candidate());
    assert_eq!(host("bin").unprojected(), None);
    fs::write(base.join("bin/opencode"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(base.join("bin/opencode"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(host("bin").opencode_candidate());
    assert_eq!(host("bin").unprojected(), None);
    fs::remove_file(base.join("bin/opencode")).unwrap();
    fs::write(base.join("home/.opencode/bin/opencode"), "").unwrap();
    assert!(host("bin").opencode_candidate());
    assert_eq!(host("bin").unprojected(), None);
    fs::remove_file(base.join("home/.opencode/bin/opencode")).unwrap();
    fs::write(base.join("config/cortex/connections.json"), "{not json").unwrap();
    assert_eq!(
        host("bin").unprojected(),
        Some("connection_registry_unprojected")
    );
    fs::remove_file(base.join("config/cortex/connections.json")).unwrap();
    // A missing manifest is the retained "retry after checking npm/network" refusal.
    let error = install(&host("bin"), "codex-acp", Instant::now() + TIMEOUT, &|_| {
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "engine_install_failed");
    assert!(error.message.contains("npm/network"));
    assert!(!base.join("home/.local").exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn npm_is_killed_at_its_bound_and_leaves_staging_for_the_next_pass() {
    let base = PathBuf::from(format!("/tmp/cw-inst-t-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    for leaf in ["home", "config", "bin"] {
        fs::create_dir_all(base.join(leaf)).unwrap();
    }
    fs::write(base.join("bin/node"), "#!/bin/sh\nexit 97\n").unwrap();
    fs::write(
        base.join("bin/npm"),
        format!(
            "#!/bin/sh\necho $$ > {}\nexec /bin/sleep 30\n",
            base.join("npm-pid").display()
        ),
    )
    .unwrap();
    for f in ["bin/node", "bin/npm"] {
        fs::set_permissions(base.join(f), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let host = Host {
        route: RouteEnvironment {
            home: base.join("home"),
            config: base.join("config"),
            state: base.join("state"),
            routing_override: None,
        },
        data: base.join("home/.local/share"),
        path: Some(base.join("bin").into_os_string()),
        bundle: None,
        manifest: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pinned-dependencies.json"),
    };
    let started = std::time::Instant::now();
    // Model a request whose queue/preparation already used most of its 30 s.
    // npm must use the remaining absolute budget, not start another 60/30 s.
    let request_deadline = started + CLEANUP_RESERVE + Duration::from_millis(300);
    let error = install(&host, "codex-acp", npm_deadline(request_deadline), &|_| {
        Ok(())
    })
    .unwrap_err();
    assert!(Instant::now() < request_deadline);
    let pid: i32 = fs::read_to_string(base.join("npm-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(error.code, "engine_install_failed");
    assert!(error.message.contains("npm/network"));
    // The lock is released; no install or status was published.
    let root = base.join("home/.local/share/cortex/engines/codex-acp");
    assert!(root.join(".lock").exists());
    let lock = fs::File::open(root.join(".lock")).unwrap();
    // A sibling test thread that forks while the install holds the lock gives its child a
    // copy of the descriptor until that child execs (O_CLOEXEC closes it then); the flock
    // follows the shared open file description. The install itself releases at once, so
    // wait out such a transient fork window instead of failing on it.
    let free = (0..100).any(|_| {
        let taken = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !taken {
            std::thread::sleep(Duration::from_millis(50));
        }
        taken
    });
    assert!(free, "the install lock stayed held for 5 s after the failed install");
    drop(lock);
    // An exhausted request must not launch even if the executable would succeed.
    fs::remove_file(base.join("npm-pid")).unwrap();
    assert!(run_npm(
        &base.join("bin/npm"),
        host.path.as_ref(),
        &root,
        "unused",
        npm_deadline(Instant::now()),
        &|_| Ok(())
    )
    .is_err());
    assert!(!base.join("npm-pid").exists());
    assert!(TIMEOUT < super::super::super::HELPER_DEADLINE);
    assert!(!root.join("2.3.0").exists());
    assert!(!base.join("config/cortex").exists());
    fs::remove_dir_all(&base).unwrap();
}

fn opencode_host(base: &Path) -> Host {
    for directory in [
        "home",
        "config/cortex",
        "state",
        "home/.opencode/bin",
        "home/.local/share/opencode",
    ] {
        fs::create_dir_all(base.join(directory)).unwrap();
    }
    for directory in ["home", "config", "config/cortex", "state"] {
        fs::set_permissions(base.join(directory), fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(base.join("home/.local/share/opencode/auth.json"), "{}").unwrap();
    fs::set_permissions(
        base.join("home/.local/share/opencode"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(
        base.join("home/.local/share/opencode/auth.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let script = r##"#!/bin/sh
if [ "$1" = "--version" ]; then echo "opencode 2.0.19"; exit 0; fi
while IFS= read -r line; do
 id=${line#*\"id\"}
 id=${id#*:}
 id=${id%%,*}
 id=${id%%\}*}
 id=${id# }
 case "$line" in
 *initialize*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"agentInfo":{"version":"2.0.19"}}}' ;;
 *session/new*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s1","configOptions":[{"id":"mode","name":"Mode","type":"select","category":"mode","currentValue":"cortex","options":[{"value":"cortex","name":"Cortex"}]},{"id":"model","name":"Model","type":"select","category":"model","currentValue":"m-a","options":[{"value":"m-a","name":"Model A"}]}]}}' ;;
 *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"configOptions":[{"id":"mode","name":"Mode","type":"select","category":"mode","currentValue":"cortex","options":[{"value":"cortex","name":"Cortex"}]},{"id":"model","name":"Model","type":"select","category":"model","currentValue":"m-a","options":[{"value":"m-a","name":"Model A"}]}]}}' ;;
 esac
done
"##;
    let binary = base.join("home/.opencode/bin/opencode");
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    Host {
        route: RouteEnvironment {
            home: base.join("home").canonicalize().unwrap(),
            config: base.join("config").canonicalize().unwrap(),
            state: base.join("state").canonicalize().unwrap(),
            routing_override: None,
        },
        data: base.join("home/.local/share"),
        path: Some("/usr/bin:/bin".into()),
        bundle: None,
        manifest: base.join("missing.json"),
    }
}

#[test]
fn opencode_is_discovered_onboarded_and_observed_natively_without_the_owners_binary() {
    let probe = crate::machine::config_action::opencode_engine::isolating_bwrap(Some(
        &"/usr/bin:/bin".into(),
    ))
    .is_some_and(|b| {
        std::process::Command::new(b)
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--ro-bind",
                "/",
                "/",
                "/bin/true",
            ])
            .status()
            .is_ok_and(|s| s.success())
    });
    if !probe {
        eprintln!("bubblewrap with user namespaces is unavailable: OpenCode onboarding not run");
        return;
    }
    let base = PathBuf::from(format!("/tmp/cw-inst-oc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let host = opencode_host(&base);
    let folder = base.join("config/cortex").canonicalize().unwrap();
    // Discovery names the sandbox-probed version.
    let view = host.discovery().unwrap();
    let row = view["engines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["engine"] == "opencode")
        .unwrap();
    assert_eq!(row["status"], "ready_to_connect");
    assert_eq!(row["version"], "2.0.19");
    assert_eq!(row["qualified"], true);
    // A stale revision is refused before any discovery or write.
    let stale = json!({"action":"onboard_opencode","explicit_opt_in":true,
        "expected_revision":format!("sha256:{}", "0".repeat(64))});
    assert_eq!(
        onboard_opencode(&host, &folder, &stale).unwrap_err().code,
        "revision_conflict"
    );
    let params =
        json!({"action":"onboard_opencode","explicit_opt_in":true,"expected_revision":"absent"});
    let reply = onboard_opencode(&host, &folder, &params).unwrap();
    assert_eq!(reply["changed"], true);
    assert_eq!(reply["catalog_status"], "ready");
    assert_eq!(reply["connection"]["engine"], "opencode");
    assert_eq!(reply["connection"]["name"], "OpenCode");
    assert_eq!(reply["connection"]["state"], "configured");
    assert_eq!(reply["opencode"]["version"], "2.0.19");
    assert_eq!(reply["opencode"]["next_action"], "refresh_catalog");
    assert_eq!(
        reply["opencode"]["automatic_routing"],
        "excluded_until_routing_opts_in"
    );
    let registry: Value =
        serde_json::from_slice(&fs::read(folder.join("connections.json")).unwrap()).unwrap();
    let record = registry["connections"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(record["provider"], "opencode");
    assert_eq!(record["auth_mode"], "native_account");
    assert_eq!(record["credential_ref"]["kind"], "engine_native_dir");
    assert_eq!(record["credential_ref"]["owned"], false);
    assert_eq!(record["launch_command"][1], "acp");
    // Re-onboarding the same binary reuses the one connection.
    let revision = reply["revision"].as_str().unwrap().to_owned();
    let again = onboard_opencode(
        &host,
        &folder,
        &json!({"action":"onboard_opencode","explicit_opt_in":true,"expected_revision":revision}),
    )
    .unwrap();
    assert_eq!(again["connection"]["id"], reply["connection"]["id"]);
    let registry: Value =
        serde_json::from_slice(&fs::read(folder.join("connections.json")).unwrap()).unwrap();
    assert_eq!(registry["connections"].as_object().unwrap().len(), 1);
    // Discovery now reports the connection.
    let view = host.discovery().unwrap();
    assert!(view["engines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["engine"] == "opencode" && e["status"] == "connected"));
    fs::remove_dir_all(&base).unwrap();
}

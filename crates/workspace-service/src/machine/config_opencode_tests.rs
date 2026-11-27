use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self::new_at(&std::env::temp_dir())
    }
    fn new_at(parent: &Path) -> Self {
        let root = parent.join(format!("cw-opencode-engine-{}", unique()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn binary(&self, name: &str, script: &str, mode: u32) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn landlock_usable() -> bool {
    crate::native_sandbox::abi() >= 1
}

#[test]
fn version_is_the_first_word_bounded_x_y_z() {
    for (text, expected) in [
        ("opencode 2.0.19\n", Some("2.0.19")),
        ("v2.1.3", Some("2.1.3")),
        ("2.0.19-beta", Some("2.0.19")),
        ("x2.0.19", None),
        ("2.0.19a", None),
        ("2.0", None),
        ("version 10.20.30.40", Some("10.20.30")),
        ("", None),
    ] {
        assert_eq!(first_version(text).as_deref(), expected, "{text:?}");
    }
}

#[test]
fn trust_matches_the_oracle_refusals() {
    let t = Temp::new();
    let home = t.0.join("home");
    fs::create_dir(&home).unwrap();
    let good = t.binary("good", "#!/bin/sh\n", 0o700);
    assert_eq!(trusted_binary(&good, &home), Ok(()));
    assert_eq!(
        trusted_binary(Path::new("relative"), &home),
        Err("engine_install_untrusted_location")
    );
    assert_eq!(
        trusted_binary(&t.0.join("missing"), &home),
        Err("engine_install_drifted")
    );
    let writable = t.binary("writable", "#!/bin/sh\n", 0o722);
    assert_eq!(
        trusted_binary(&writable, &home),
        Err("engine_install_untrusted_location")
    );
    let plain = t.binary("plain", "data", 0o600);
    assert_eq!(trusted_binary(&plain, &home), Err("engine_install_drifted"));
    let link = t.0.join("link");
    symlink(&good, &link).unwrap();
    assert_eq!(
        trusted_binary(&link, &home),
        Err("engine_install_untrusted_location")
    );
    let codeg = home.join(".config/app.codeg");
    fs::create_dir_all(&codeg).unwrap();
    let inside = codeg.join("opencode");
    fs::write(&inside, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        trusted_binary(&inside, &home.canonicalize().unwrap()),
        Err("engine_install_untrusted_location")
    );
}

#[test]
fn jsonc_comments_and_trailing_commas_are_removed_outside_strings() {
    let text = "{\n // line\n \"a\": \"x // not a comment, }\", /* block */\n \"b\": [1, 2,],\n}\n";
    let value: Value = serde_json::from_str(&strip_jsonc(text).unwrap()).unwrap();
    assert_eq!(value, json!({"a":"x // not a comment, }","b":[1,2]}));
    assert!(strip_jsonc("{ /* open").is_err());
}

#[test]
fn owner_providers_are_carried_without_serializing_credentials() {
    let t = Temp::new();
    let home = t.0.join("home");
    let config = home.join(".config/opencode");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("token.txt"), "file-secret\n").unwrap();
    fs::write(
        config.join("opencode.jsonc"),
        r#"{
          // owner config
          "provider": {
            "local": {"npm": "@ai-sdk/openai-compatible", "options": {"baseURL": "http://localhost:1/v1", "apiKey": "{env:LOCAL_KEY}"},
                      "models": {"m": {}}},
            "filed": {"options": {"headers": {"X-One": "{file:token.txt}"}}},
            "evil": {"npm": "left-pad"},
            "empty": {"options": {"apiKey": "{env:NOT_SET}"}},
            "bad id!": {}
          },
          "enabled_providers": ["local", "filed"],
          "ignored_key": true,
        }"#,
    )
    .unwrap();
    let environ = BTreeMap::from([("LOCAL_KEY".to_owned(), "env-secret".to_owned())]);
    let owner = owner_providers(&home, "/jail/tmp", &environ);
    assert_eq!(owner.imported, ["empty", "filed", "local"]);
    assert!(owner
        .skipped
        .contains(&("evil".into(), "package_not_allowed".into())));
    assert!(owner
        .skipped
        .contains(&("?".into(), "invalid_provider_id".into())));
    // Credentials live in jail files, never in the configuration text.
    let text = serde_json::to_string(&owner.config).unwrap();
    assert!(
        !text.contains("env-secret") && !text.contains("file-secret"),
        "{text}"
    );
    assert_eq!(owner.files.len(), 2);
    assert!(owner.files.iter().any(|(path, bytes)| path
        == "/jail/tmp/cortex-opencode-credential-0"
        && bytes == b"file-secret"));
    assert!(owner.files.iter().any(|(_, bytes)| bytes == b"env-secret"));
    assert_eq!(owner.config["provider"]["empty"]["options"], json!({}));
    assert_eq!(owner.config["enabled_providers"], json!(["local", "filed"]));
    assert!(owner.config.get("ignored_key").is_none());
    // Process configuration: only Cortex's agent, deny rules and the carried providers.
    let env = process_environment("/jail/tmp/prompt.md", &owner.config);
    let content: Value = serde_json::from_str(&env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(content["default_agent"], "cortex");
    assert_eq!(
        content["agent"]["cortex"]["prompt"],
        "{file:/jail/tmp/prompt.md}"
    );
    assert_eq!(content["permission"]["*"], "ask");
    assert_eq!(
        content["provider"]["local"]["npm"],
        "@ai-sdk/openai-compatible"
    );
    assert_eq!(env["OPENCODE_DISABLE_PROJECT_CONFIG"], "1");
    // Unreadable owner files are reported, never guessed.
    fs::write(config.join("config.json"), "{broken").unwrap();
    let owner = owner_providers(&home, "/jail/tmp", &environ);
    assert!(owner
        .skipped
        .contains(&("*".into(), "config.json:unreadable".into())));
}

#[test]
fn discovery_reads_the_version_inside_the_sandbox_and_skips_untrusted_candidates() {
    if !landlock_usable() {
        eprintln!("Landlock is unavailable: probe not run");
        return;
    }
    let t = Temp::new();
    let home = t.0.join("home");
    fs::create_dir_all(home.join(".opencode/bin")).unwrap();
    let path = std::ffi::OsString::from("/usr/bin:/bin");
    let fake = home.join(".opencode/bin/opencode");
    fs::write(&fake, "#!/bin/sh\necho \"opencode 2.0.19\"\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let found = discover(&home, Some(&path), None).expect("trusted candidate");
    assert_eq!(found.version, "2.0.19");
    assert_eq!(found.binary, fake.canonicalize().unwrap());
    assert!(qualified(&found.version));
    // Keep the sentinel outside the version probe’s own writable scratch.
    let host = Temp::new();
    let sentinel = host.0.join("owner-sentinel");
    fs::write(&sentinel, "owner").unwrap();
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\n[ \"$HOME\" != \"$OWNER_HOME\" ] || exit 7\n[ -z \"$SECRET\" ] || exit 8\n\
             ! (echo changed > '{}') 2>/dev/null || exit 9\necho 3.1.4\n",
            sentinel.display(),
        ),
    )
    .unwrap();
    assert_eq!(probe_version(&fake, Some(&path)).as_deref(), Some("3.1.4"));
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "owner");
    // A failing or silent binary reports no version; an unqualified one is named.
    fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(probe_version(&fake, Some(&path)).is_none());
    fs::write(&fake, "#!/bin/sh\necho 1.9.0\n").unwrap();
    assert!(!qualified(&probe_version(&fake, Some(&path)).unwrap()));
    // A group-writable binary is never executed.
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(discover(&home, Some(&path), None).is_none());
}


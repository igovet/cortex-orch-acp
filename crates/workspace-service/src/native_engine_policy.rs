//! The native engine permission mode and the Claude session options every
//! Cortex-launched session starts with.
//!
//! No Cortex session runs in an engine mode that skips the engine's own
//! permission requests (`bypassPermissions`, `agent-full-access`). A read-only
//! channel (any non-worker, a read-only profile, or a review-stage session; see
//! [`read_only`]) runs in the engine's asking or read-only mode in every access
//! mode, and its start is refused when the engine offers none.
use cortex_core::{channels::Identity, scheduler::policy};
use cortex_engine::{environment::EngineKind, permission::PathClass, Error};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The runtime's single read-only predicate, shared with the permission
/// responder and the write tools: a non-worker channel, a read-only profile, or
/// a review-stage session.
pub(crate) fn read_only(i: &Identity) -> bool {
    i.role != "worker"
        || policy::read_only(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or(""),
        )
}

/// Whether the engine's `mode` config option offers `value` (also inside a
/// grouped option list, as codex-acp groups modes when permission profiles exist).
fn offers(options: &Value, value: &str) -> bool {
    fn listed(values: &Value, value: &str) -> bool {
        values.as_array().is_some_and(|values| {
            values.iter().any(|v| {
                v["value"] == value || (v.get("group").is_some() && listed(&v["options"], value))
            })
        })
    }
    options.as_array().is_some_and(|a| {
        a.iter()
            .any(|o| o["id"] == "mode" && listed(&o["options"], value))
    })
}

pub(crate) const CODEX_NATIVE_MODE: &str = "read-only";

pub(crate) fn codex_native_confirmed(options: &Value) -> bool {
    let Some(options) = options.as_array() else {
        return false;
    };
    let mut mode = options.iter().filter(|o| o["id"] == "mode");
    mode.next()
        .is_some_and(|o| o["currentValue"] == CODEX_NATIVE_MODE)
        && mode.next().is_none()
}

/// The engine permission mode of a session, independent of the access mode:
///
/// - claude-agent-acp: `default`, the mode in which every tool Claude Code does
///   not auto-approve reaches the runtime as `session/request_permission`.
/// - codex-acp: `read-only` for every Cortex-managed channel. Only the
///   authenticated Cortex gateway can grant a writing worker a project effect.
/// - OpenCode: `cortex`, whose permissions ask for every tool.
///
/// A mode the engine does not offer refuses the start with
/// `restricted_mode_unavailable`; no wider mode is substituted.
pub(crate) fn native_mode(engine: EngineKind, options: &Value) -> Result<&'static str, Error> {
    let mode = match engine {
        EngineKind::Claude => "default",
        EngineKind::Codex => CODEX_NATIVE_MODE,
        EngineKind::OpenCode => "cortex",
    };
    if offers(options, mode) {
        Ok(mode)
    } else {
        Err(Error::Refused("restricted_mode_unavailable"))
    }
}

/// Native Claude Code tools no Cortex session is offered. Shell, edit, write,
/// notebook and skill tools have scoped Cortex MCP replacements (`command_run`,
/// `project_write`/`project_edit`, `read_skill`); subagent, scheduling,
/// worktree, plan-mode, task-stop and outward notification tools are owned by
/// the runtime. Read, Grep, Glob, WebFetch and WebSearch stay, answered per
/// request by the runtime.
pub(crate) const CLAUDE_DISALLOWED_TOOLS: &[&str] = &[
    // Shell, file mutation and skills.
    "Bash",
    "BashOutput",
    "KillShell",
    "KillBash",
    "PowerShell",
    "Edit",
    "MultiEdit",
    "Write",
    "NotebookEdit",
    "Skill",
    // Native subagents and deferred wake-ups.
    "Agent",
    "Task",
    "Workflow",
    "TeamCreate",
    "TeamDelete",
    "SendMessage",
    "ScheduleWakeup",
    "Monitor",
    "CronCreate",
    "CronDelete",
    "CronList",
    "RemoteTrigger",
    // Lifecycle and outward notification tools.
    "EnterWorktree",
    "ExitWorktree",
    "PushNotification",
    "EnterPlanMode",
    "ExitPlanMode",
    "TaskStop",
    "Artifact",
    "SendFeedback",
];

/// Project-relative Cortex private state no native read may reach. Claude Code
/// reads inside its working directory without asking, so these deny rules (a
/// deny rule wins over every allow rule) keep the store out of native reads.
pub(crate) const CLAUDE_PRIVATE_READ_DENIALS: &[&str] = &[
    "Read(./.cortex/**)",
    "Read(./.cortex-dev/**)",
    "Read(./.cortex-dev-owner.json)",
    "Read(./.cortex-workspace-dev.json)",
];

/// Credential and key file names Claude Code would otherwise read inside the
/// project without asking (the names of `machine::short_text::private_name`):
/// an `ask` rule makes it send a permission request the runtime answers.
pub(crate) const CLAUDE_CREDENTIAL_READ_ASKS: &[&str] = &[
    "Read(**/.env*)",
    "Read(**/id_rsa*)",
    "Read(**/id_dsa*)",
    "Read(**/id_ecdsa*)",
    "Read(**/id_ed25519*)",
    "Read(**/service-account*)",
    "Read(**/kubeconfig*)",
    "Read(**/.netrc)",
    "Read(**/.npmrc)",
    "Read(**/.pypirc)",
    "Read(**/.htpasswd)",
    "Read(**/credentials*)",
    "Read(**/*.pem)",
    "Read(**/*.key)",
    "Read(**/*.p12)",
    "Read(**/*.pfx)",
    "Read(**/*.keystore)",
    "Read(**/*.jks)",
    "Read(**/*.kdbx)",
    "Read(**/*.tfvars)",
    "Read(**/*.tfstate*)",
    "Read(**/.kube/config)",
    "Read(**/*secret*)",
    "Read(**/*credential*)",
    "Read(**/*password*)",
    "Read(**/*token*)",
];

/// `session/new` `_meta` for claude-agent-acp (`_meta.claudeCode.options`, merged
/// into the SDK query options by the adapter): bypass unavailable, only the MCP
/// servers Cortex attaches, no plugins, skills, hooks or auto-memory, and the
/// tools above removed from the model's context.
///
/// `settingSources` is `["project"]`: the adapter's default
/// (`user`, `project`, `local`) would load the owner's personal Claude Code
/// configuration (`~/.claude/settings.json`, `~/.claude/CLAUDE.md`) and their
/// private per-project overrides (`.claude/settings.local.json`,
/// `CLAUDE.local.md`) into every Cortex session, including allow rules, model,
/// output-style and environment settings Cortex does not govern. The project's
/// shared settings and its `CLAUDE.md` conventions still load.
pub(crate) fn claude_session_meta() -> Value {
    let settings = json!({
        "disableAllHooks": true,
        "autoMemoryEnabled": false,
        "permissions": {"deny": CLAUDE_PRIVATE_READ_DENIALS, "ask": CLAUDE_CREDENTIAL_READ_ASKS},
    });
    json!({"claudeCode":{"options":{
        "allowDangerouslySkipPermissions": false,
        "settingSources": ["project"],
        "strictMcpConfig": true,
        "plugins": [],
        "skills": [],
        "settings": settings,
        "managedSettings": {"disableAllHooks": true, "autoMemoryEnabled": false},
        "disallowedTools": CLAUDE_DISALLOWED_TOOLS,
    }}})
}

/// Home locations of credential and key material (beyond the private file names).
const HOME_CREDENTIALS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".docker/config.json",
    ".kube",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/gh/hosts.yml",
    ".local/share/keyrings",
    ".password-store",
    ".bash_history",
    ".zsh_history",
    ".python_history",
    ".node_repl_history",
    ".psql_history",
    ".mysql_history",
    ".local/share/fish/fish_history",
    ".codex/auth.json",
    ".claude/.credentials.json",
    ".claude.json",
];
const PRIVATE_PROJECT_NAMES: &[&str] = &[
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
];

/// All project stores and owner markers, not just the current checkout or channel.
/// This also protects the names before an entry exists or while Git metadata is
/// unavailable. Call with both the named and resolved paths if following aliases.
pub(crate) fn project_private(path: &Path) -> bool {
    path.components().any(|part| {
        PRIVATE_PROJECT_NAMES
            .iter()
            .any(|name| part.as_os_str() == *name)
    })
}

/// The class of a resolved absolute path a native request names
/// ([`permission::PathClass`]): Cortex private state (the project's `.cortex`,
/// `.cortex-dev` and dev owner files, the service state directory); credential
/// and key material (a private file name anywhere, the home credential
/// locations above, the engine's own credential files); Git control files and
/// engine/Cortex configuration (every other runtime-protected target); else
/// ordinary.
pub(crate) fn path_class(root: &Path, path: &Path) -> PathClass {
    if project_private(path) {
        return PathClass::Private;
    }
    let env: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> =
        std::env::vars_os().collect();
    let var = |k: &str| {
        env.get(std::ffi::OsStr::new(k))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home = var("HOME").filter(|h| h.is_absolute());
    let under = |base: &Path| {
        path.starts_with(base) || base.canonicalize().is_ok_and(|real| path.starts_with(real))
    };
    let logical = cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Release)
        .map(|k| k.root().to_path_buf())
        .unwrap_or_else(|_| root.to_path_buf());
    let mut private: Vec<PathBuf> = PRIVATE_PROJECT_NAMES
        .iter()
        .flat_map(|n| [logical.join(n), root.join(n)])
        .collect();
    if let Some(state) =
        var("XDG_STATE_HOME").or_else(|| home.as_ref().map(|h| h.join(".local/state")))
    {
        private.push(state.join("cortex"));
    }
    if private.iter().any(|p| under(p)) {
        return PathClass::Private;
    }
    let credential = crate::machine::short_text::private_name(&path.to_string_lossy())
        || home
            .as_ref()
            .is_some_and(|h| HOME_CREDENTIALS.iter().any(|c| under(&h.join(c))))
        || ["CLAUDE_CONFIG_DIR", "CODEX_HOME"]
            .iter()
            .filter_map(|k| var(k))
            .any(|d| {
                [".credentials.json", "auth.json"]
                    .iter()
                    .any(|f| under(&d.join(f)))
            });
    if credential {
        return PathClass::Credential;
    }
    if crate::native_command_sandbox::policy::protected_target(root, path) {
        return PathClass::Control;
    }
    PathClass::Ordinary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(values: &[&str]) -> Value {
        json!([{"id":"mode","options":values.iter().map(|v| json!({"value":v})).collect::<Vec<_>>()}])
    }

    #[test]
    fn no_session_gets_an_unrestricted_mode() {
        let all = modes(&[
            "agent",
            "agent-full-access",
            "workspace-write",
            "default",
            "acceptEdits",
            "bypassPermissions",
            "cortex",
            "read-only",
            "plan",
        ]);
        for (engine, expected) in [
            (EngineKind::Claude, "default"),
            (EngineKind::Codex, "read-only"),
            (EngineKind::OpenCode, "cortex"),
        ] {
            assert_eq!(native_mode(engine, &all).unwrap(), expected);
        }
    }

    #[test]
    fn a_missing_restricted_mode_refuses_instead_of_widening() {
        let wide = modes(&[
            "workspace-write",
            "agent",
            "agent-full-access",
            "bypassPermissions",
        ]);
        assert!(matches!(
            native_mode(EngineKind::Codex, &wide),
            Err(Error::Refused("restricted_mode_unavailable"))
        ));
        assert!(matches!(
            native_mode(EngineKind::Claude, &wide),
            Err(Error::Refused("restricted_mode_unavailable"))
        ));
        // Grouped codex-acp mode lists are searched too.
        let grouped = json!([{"id":"mode","options":[
            {"group":"sandbox-modes","options":[{"value":"read-only"},{"value":"agent"}]},
            {"group":"permission-profiles","options":[{"value":"permission-profile:x"}]}]}]);
        assert_eq!(
            native_mode(EngineKind::Codex, &grouped).unwrap(),
            "read-only"
        );
    }

    #[test]
    fn read_only_confirmation_requires_one_exact_mode() {
        let read_only = json!({"id":"mode","currentValue":"read-only"});
        assert!(codex_native_confirmed(&json!([read_only])));
        assert!(!codex_native_confirmed(&json!([read_only, read_only])));
        assert!(!codex_native_confirmed(
            &json!([{"id":"mode","currentValue":"workspace-write"}])
        ));
        assert!(!codex_native_confirmed(&json!([])));
    }

    #[test]
    fn another_project_and_its_linked_worktree_have_private_store_names() {
        use cortex_core::{ProjectKey, StoreChannel};
        use std::process::Command;
        let base = std::env::temp_dir().join(format!(
            "private-stores-{}",
            crate::native_effects::identity().unwrap()
        ));
        let current = base.join("current");
        let foreign = base.join("foreign");
        let linked = base.join("linked");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&foreign)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
            "--allow-empty",
        ]);
        git(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ]);
        for channel in [StoreChannel::Release, StoreChannel::Dev] {
            let key = ProjectKey::new(&linked, channel).unwrap();
            assert_eq!(key.root(), &foreign);
            assert_eq!(path_class(&current, &key.database()), PathClass::Private);
            assert_eq!(
                path_class(&current, &linked.join(channel.directory()).join("absent")),
                PathClass::Private
            );
        }
        for root in [&foreign, &linked] {
            for marker in [".cortex-dev-owner.json", ".cortex-workspace-dev.json"] {
                assert_eq!(path_class(&current, &root.join(marker)), PathClass::Private);
            }
            assert_eq!(
                path_class(&current, &root.join("src/file")),
                PathClass::Ordinary
            );
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn path_classes() {
        let root = std::env::temp_dir().join(format!("policy-class-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(
            path_class(&root, &root.join("src/a.rs")),
            PathClass::Ordinary
        );
        assert_eq!(
            path_class(&root, &root.join(".cortex/cortex.sqlite3")),
            PathClass::Private
        );
        assert_eq!(
            path_class(&root, &root.join(".cortex-dev/x")),
            PathClass::Private
        );
        assert_eq!(
            path_class(&root, &root.join("src/.env.local")),
            PathClass::Credential
        );
        assert_eq!(
            path_class(&root, &root.join("certs/server.pem")),
            PathClass::Credential
        );
        if let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|h| h.is_absolute())
        {
            assert_eq!(
                path_class(&root, &home.join(".ssh/config")),
                PathClass::Credential
            );
            assert_eq!(
                path_class(&root, &home.join(".bash_history")),
                PathClass::Credential
            );
            assert_eq!(
                path_class(&root, &home.join(".codex/config.toml")),
                PathClass::Control
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn claude_meta_disables_native_mutation_and_bypass() {
        let meta = claude_session_meta();
        let o = &meta["claudeCode"]["options"];
        assert_eq!(o["allowDangerouslySkipPermissions"], false);
        assert_eq!(o["strictMcpConfig"], true);
        // The owner's user-level and private local settings never load; the
        // project's shared settings and CLAUDE.md conventions do.
        assert_eq!(o["settingSources"], json!(["project"]));
        assert_eq!(o["managedSettings"]["disableAllHooks"], true);
        assert_eq!(o["settings"]["autoMemoryEnabled"], false);
        let denied: Vec<&str> = o["disallowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for tool in [
            "Bash",
            "Edit",
            "Write",
            "NotebookEdit",
            "Agent",
            "Task",
            "Skill",
        ] {
            assert!(denied.contains(&tool), "{tool}");
        }
        for kept in ["Read", "Grep", "Glob", "WebFetch", "WebSearch"] {
            assert!(!denied.contains(&kept), "{kept}");
        }
    }
}

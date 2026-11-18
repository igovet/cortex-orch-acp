//! What a paired remote device (the phone) may call through the remote-access tunnel.
//!
//! The tunnel's connections reach the same local socket as the owner's own windows and
//! carry the same account's credentials, so this allowlist is the only place a remote
//! request is refused. It is closed: a method or machine operation that is not listed
//! here is refused, including every method added to the service later until it is
//! classified on purpose.
//!
//! Refused for remote devices: every `remote.*` method (pairing and device control stay
//! with the owner at the machine), `service.stop`, every `terminal.*` and `voice.*`
//! method, anything that opens a native login, privilege prompt or desktop program on
//! the machine, and every write into a checkout's files or Git state (Code is read-only
//! from the phone in this version).
use serde_json::Value;

/// Service methods a remote device may call. `machine.call` is decided per operation by
/// [`machine_operation_allowed`].
const METHODS: &[&str] = &[
    "service.status",
    "service.capabilities",
    "service.subscribe",
    "projects.list",
    "projects.register",
    "projects.rename",
    "projects.remove",
    "projects.activate",
    "projects.status",
    "directories.search",
    "directories.cancel",
    "native.query",
    "native.board",
    "workspace",
    "executions.cancel",
    "executions.prompt",
    "executions.control",
];

/// `machine.call` operations a remote device may call whatever their parameters.
const MACHINE_OPERATIONS: &[&str] = &[
    // Machine and project settings, read and written.
    "overview",
    "workflow.describe",
    "diagnostics.run",
    "cleanup.run",
    "machine.update",
    "config.read",
    "config.save",
    "config.action",
    "config.mcp.discover",
    "project.config.read",
    "project.config.save",
    // Code and Git reads.
    "repo.snapshot",
    "repo.context",
    "repo.file",
    "repo.list",
    "repo.skills",
    // Tasks, sprints, results, history and the owner's actions on them.
    "workers",
    "product.read",
    "product.board",
    "product.facts",
    "product.task",
    "product.integrate",
    "product.sprint",
    "product.message",
    "product.comment",
    // Owner attachments: upload once, read one body back by reference.
    "product.attachment",
];

/// Whether a remote device may call `method` with `params`.
pub fn allowed(method: &str, params: &Value) -> bool {
    if method == "machine.call" {
        return machine_call_allowed(params);
    }
    METHODS.contains(&method)
}

fn machine_call_allowed(params: &Value) -> bool {
    let Some(call) = params.as_object() else {
        return false;
    };
    let Some(op) = call.get("op").and_then(Value::as_str) else {
        return false;
    };
    machine_operation_allowed(op, call.get("params").unwrap_or(&Value::Null))
}

/// Whether a remote device may run machine operation `op` with its `params`.
pub fn machine_operation_allowed(op: &str, params: &Value) -> bool {
    let action = params.get("action").and_then(Value::as_str);
    match op {
        // Only a recheck and an API key entry; `login_command` exists to open a native
        // login terminal on the machine.
        "engine.connection" => matches!(action, Some("recheck" | "add_key")),
        // Update and clone jobs, except installing or rolling back a package (a
        // privilege prompt on the machine's screen) and acknowledging an unconfirmed
        // install (the owner checks the package manager at the machine).
        "maintenance" => match action {
            Some("voice.status" | "update.status" | "update.check" | "read" | "cancel") => true,
            Some("start") => matches!(
                params.get("kind").and_then(Value::as_str),
                Some("clone" | "download")
            ),
            _ => false,
        },
        // Refused on purpose: `repo.file.save` (file saves and new files in a
        // checkout), `git` and `git_init` (every Git write: commit, stage, revert,
        // push, pull, fetch, branch, drafts, worktree deletion), `repo.locate` and
        // `logs.folder` (absolute paths for the desktop's own openers).
        _ => MACHINE_OPERATIONS.contains(&op),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn machine(op: &str, params: Value) -> bool {
        allowed(
            "machine.call",
            &json!({"op":op,"projectId":"p","params":params}),
        )
    }

    #[test]
    fn owner_only_and_machine_bound_methods_are_refused() {
        for method in [
            "remote.status",
            "remote.enable",
            "remote.pairing.start",
            "remote.pairing.cancel",
            "remote.pairing.decide",
            "remote.devices.revoke",
            "remote.relay.set",
            "remote.anything",
            "service.stop",
            "terminal.open",
            "terminal.read",
            "terminal.write",
            "terminal.resize",
            "terminal.close",
            "voice.start",
            "voice.read",
            "voice.stop",
            "voice.cancel",
        ] {
            assert!(!allowed(method, &json!({})), "{method}");
        }
    }

    #[test]
    fn unknown_or_malformed_requests_are_refused() {
        for method in [
            "",
            "brand.new",
            "service.status2",
            "native.engine_login",
            "native.open_editor",
        ] {
            assert!(!allowed(method, &json!({})), "{method}");
        }
        assert!(!allowed("machine.call", &Value::Null));
        assert!(!allowed("machine.call", &json!({})));
        assert!(!allowed("machine.call", &json!({"op":7})));
        assert!(!allowed("machine.call", &json!(["op", "overview"])));
        assert!(!machine("brand.new.op", json!({})));
    }

    #[test]
    fn checkout_writes_logins_and_desktop_openers_are_refused() {
        assert!(!machine(
            "repo.file.save",
            json!({"file":"a.txt","text":"x","revision":null})
        ));
        for action in [
            "commit",
            "stage",
            "revert",
            "push",
            "pull",
            "fetch",
            "branch",
            "draft",
            "delete_worktree",
        ] {
            assert!(!machine("git", json!({"action":action})), "{action}");
        }
        assert!(!machine("git_init", json!({})));
        assert!(!machine("repo.locate", json!({"file":"a.txt"})));
        assert!(!machine("logs.folder", json!({})));
        assert!(!machine(
            "engine.connection",
            json!({"action":"login_command","engine":"codex-acp"})
        ));
        assert!(!machine("engine.connection", json!({})));
        for kind in ["install", "rollback"] {
            assert!(
                !machine(
                    "maintenance",
                    json!({"action":"start","kind":kind,"id":"a"})
                ),
                "{kind}"
            );
        }
        assert!(!machine(
            "maintenance",
            json!({"action":"acknowledge","id":"a","confirmed":true})
        ));
        assert!(!machine("maintenance", json!({})));
    }

    #[test]
    fn reads_and_owner_actions_pass() {
        for method in [
            "service.status",
            "service.capabilities",
            "service.subscribe",
            "projects.list",
            "projects.register",
            "directories.search",
            "native.query",
            "workspace",
            "executions.prompt",
            "executions.control",
        ] {
            assert!(allowed(method, &json!({})), "{method}");
        }
        for op in [
            "overview",
            "config.read",
            "config.save",
            "repo.snapshot",
            "repo.file",
            "repo.list",
            "product.read",
            "product.task",
            "product.integrate",
            "product.sprint",
            "product.attachment",
        ] {
            assert!(machine(op, json!({})), "{op}");
        }
        assert!(machine(
            "engine.connection",
            json!({"action":"recheck","engine":"codex-acp"})
        ));
        assert!(machine("maintenance", json!({"action":"update.check"})));
        assert!(machine(
            "maintenance",
            json!({"action":"start","kind":"clone","id":"a"})
        ));
    }
}

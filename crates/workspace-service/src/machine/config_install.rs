//! Installer/onboarding requests that the retained handler refuses before any
//! effect: no environment, lock, registry, npm, discovery or engine access.
//! Every other installer/onboarding request keeps the retained provider.
use super::*;

const ENGINES: [&str; 3] = ["codex-acp", "claude-agent-acp", "opencode"];

/// The retained wrapper forwards only `engine` (install) or `expected_revision`,
/// `explicit_opt_in` and `name` (onboarding); other request fields are ignored.
pub(super) fn refusal(p: &Value) -> Option<Refused> {
    match p["action"].as_str()? {
        "install_engine" => {
            let engine = p["engine"].as_str().filter(|e| ENGINES.contains(e));
            match engine {
                None => Some(refuse(
                    "unsupported_engine",
                    "Choose codex-acp, claude-agent-acp or opencode.",
                )),
                Some("opencode") => Some(refuse(
                    "engine_install_unsupported",
                    "OpenCode must be installed using its native installer.",
                )),
                // Pinned npm installation needs the network: retained.
                Some(_) => None,
            }
        }
        "onboard_opencode" => {
            if p["explicit_opt_in"] != true {
                return Some(refuse(
                    "explicit_opt_in_required",
                    "OpenCode is optional and never onboarded implicitly; send explicit_opt_in: true.",
                ));
            }
            let revision = p["expected_revision"].as_str().unwrap_or("");
            if revision != "absent"
                && !crate::config_read_model::matches(r"^sha256:[0-9a-f]{64}$", revision)
            {
                return Some(refuse("invalid_expected_revision",
                    "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>')."));
            }
            // An absent name defaults to "OpenCode"; an explicit null is invalid.
            if p.get("name")
                .is_some_and(|name| !crate::config_read_model::clean(name, 120))
            {
                return Some(validation("name", "must be 1-120 printable characters"));
            }
            // The owner lock, revision check and binary discovery follow: retained.
            None
        }
        _ => None,
    }
}

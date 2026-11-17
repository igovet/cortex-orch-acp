//! Owner-facing development tools that used to be standalone scripts: the guarded
//! dev-Paseo wrapper and home clone, the no-paid ACP startup smoke harness, the two
//! paid engine probes (refusing without explicit authorization) and the Zed/codeg
//! registration recipes. Each command prints its own diagnostics.
use crate::{acp_smoke, client_recipes, owner_tools, paid_probe, paseo_clone, paseo_dev};

pub const COMMANDS: [&str; 9] = [
    "paseo-dev",
    "paseo-clone",
    "acp-smoke",
    "probe-codex",
    "probe-claude",
    "print-client-recipes",
    "client-recipes",
    "uninstall",
    "clone-dev-settings",
];

/// Run one development tool: `(stdout, exit code)`, or `None` for another command family.
pub fn run(command: &str, argv: &[String]) -> Option<(String, i32)> {
    Some(match command {
        "paseo-dev" => paseo_dev::run(argv),
        "paseo-clone" => paseo_clone::run(argv),
        "acp-smoke" => acp_smoke::run(argv),
        "probe-codex" => paid_probe::run("codex", argv),
        "probe-claude" => paid_probe::run("claude", argv),
        "print-client-recipes" | "client-recipes" => client_recipes::run(argv),
        "uninstall" => owner_tools::uninstall(argv),
        "clone-dev-settings" => owner_tools::clone_dev_settings(argv),
        _ => return None,
    })
}

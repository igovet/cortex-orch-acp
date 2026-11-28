//! Cortex readers for the Paseo plugin when it launches the Rust `cortex-acp`
//! entry. Each store subcommand is a byte-compatible port of one Python
//! reader embedded in `integrations/paseo/server/*.ts`: same argv, same
//! queries, same stdout and exit status. Stores open only through
//! cortex-core's non-initializing `LiveReadOnly` mode. `config` answers the
//! plugin's settings API (see `config`); `setup` runs the native automatic
//! first-run setup at plugin activation (see `setup`).
pub mod acp_smoke;
pub mod acp_wire;
pub mod client_recipes;
pub mod common;
pub mod config;
pub mod dev_tools;
pub mod owner_tools;
pub mod paid_probe;
pub mod paseo_clone;
pub mod paseo_dev;
pub mod paseo_setup;
pub mod project_root;
pub mod py;
pub mod readers;
pub mod scrub;
pub mod setup;
pub mod store;
pub mod task_timings;
pub mod worker_history;

/// The subcommands, in the order `--help` lists them.
pub const COMMANDS: [&str; 9] = [
    "project-root",
    "task-id",
    "worker-history",
    "worker-results",
    "running-commands",
    "pipeline",
    "task-timings",
    "config",
    "setup",
];

/// Run one subcommand: `(stdout, exit code)`, or `None` for an unknown name.
pub fn run(command: &str, argv: &[String]) -> Option<(String, i32)> {
    Some(match command {
        "project-root" => project_root::run(argv),
        "task-id" => readers::task_id(argv),
        "worker-history" => worker_history::run(argv),
        "worker-results" => readers::worker_results(argv),
        "running-commands" => readers::running_commands(argv),
        "pipeline" => readers::pipeline(argv),
        "task-timings" => task_timings::run(argv),
        "config" => config::run(argv),
        "setup" => setup::run(argv),
        other => return dev_tools::run(other, argv).or_else(|| paseo_setup::run(other, argv)),
    })
}

//! `cortex-paseo-tools setup`: the plugin-activation automatic setup for the
//! Rust `cortex-acp` launch. It runs cortex-acp's own native first-run pass
//! (`cortex_acp::startup::first_run`: absent-only routing defaults, approved
//! adapter following, pinned engine installation, native-account connection
//! registration and one prompt-free model observation) and waits for it. A
//! concurrent pass already holding `automatic-setup.lock` makes this one skip.
//! Diagnostics go only to `first-run-<engine>.json` and stderr; stdout is empty.
use workspace_service::native_routes::RouteEnvironment;

/// No arguments; exit 0 even when a step fails (the pass records it).
pub fn run(argv: &[String]) -> (String, i32) {
    if !argv.is_empty() {
        return (String::new(), 2);
    }
    let Ok(environment) = RouteEnvironment::for_user() else {
        return (String::new(), 1);
    };
    let environment = cortex_acp::startup::channel_environment(environment);
    if let Some(pass) = cortex_acp::startup::first_run(&environment) {
        if pass.join().is_err() {
            return (String::new(), 1);
        }
    }
    (String::new(), 0)
}

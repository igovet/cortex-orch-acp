//! The Codex launcher written by `cortex-acp` names `cortex-acp` itself, so
//! this binary must run the native Codex runtime mode (offline fixture only).
#[path = "support/codex_runtime.rs"]
mod support;

const EXE: &str = env!("CARGO_BIN_EXE_cortex-acp");

#[test]
fn launcher_naming_cortex_acp_runs_app_server_and_lists_models() {
    let f = support::Fixture::new("acp-mode");
    support::assert_runtime_mode_lists_models(&f, EXE);
}

/// A Codex session started through the Rust entry never writes project trust
/// into the owner's native Codex config (the fake App Server persists it for
/// any thread whose cwd lacks a trust layer, as codex-cli 0.164.0 does).
#[test]
fn codex_session_leaves_owner_codex_config_byte_identical() {
    let f = support::Fixture::new("acp-trust");
    let config = f.0.join("home/.codex/config.toml");
    let baseline = b"# owner config\nsandbox_mode = \"danger-full-access\"\n";
    std::fs::write(&config, baseline).unwrap();
    support::run_acp_first_run(&f, EXE);
    support::assert_first_run_listed_models(&f, EXE);
    assert_eq!(f.thread_start_config().as_deref(), Some(config.as_path()));
    assert_eq!(std::fs::read(&config).unwrap(), baseline);
}

#[test]
fn first_run_lists_codex_models_through_its_own_launcher() {
    let f = support::Fixture::new("acp-first-run");
    support::run_acp_first_run(&f, EXE);
    support::assert_first_run_listed_models(&f, EXE);
}

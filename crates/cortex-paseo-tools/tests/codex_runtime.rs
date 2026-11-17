//! `cortex-paseo-tools setup` writes the Codex launcher naming
//! `cortex-paseo-tools` itself, so this binary must run the native Codex
//! runtime mode (offline fixture only).
#[path = "../../cortex-acp/tests/support/codex_runtime.rs"]
mod support;

const EXE: &str = env!("CARGO_BIN_EXE_cortex-paseo-tools");

#[test]
fn launcher_naming_cortex_paseo_tools_runs_app_server_and_lists_models() {
    let f = support::Fixture::new("tools-mode");
    support::assert_runtime_mode_lists_models(&f, EXE);
}

#[test]
fn setup_lists_codex_models_through_its_own_launcher() {
    let f = support::Fixture::new("tools-setup");
    let output = f.command(EXE).arg("setup").output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    support::assert_first_run_listed_models(&f, EXE);
}

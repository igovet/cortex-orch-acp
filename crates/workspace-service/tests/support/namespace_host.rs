//! Capability-aware alternate assertions for positive-only integration journeys.
//! The required container run must execute the real journeys, never this branch.
pub mod native_effects {
    pub fn identity() -> std::io::Result<String> {
        use std::io::Read;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

pub fn assert_unavailable_view() -> bool {
    use std::process::{Command, Stdio};
    let available = Command::new("/usr/bin/bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(
            available,
            "positive integration journeys require real namespaces"
        );
    }
    if available {
        return false;
    }
    let root = std::env::temp_dir().join(format!(
        "namespace-refusal-{}",
        native_effects::identity().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source"), "unchanged").unwrap();
    for profile in ["read_only", "guarded", "development"] {
        let e = workspace_service::native_command_supervisor::observe_command_view(&root, profile)
            .expect_err("unavailable view accepted a payload-capable sandbox");
        assert!(
            e.to_string()
                .starts_with("native_command_sandbox_unavailable:"),
            "{e}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("source")).unwrap(),
        "unchanged"
    );
    std::fs::remove_dir_all(root).unwrap();
    println!(
        "namespace-restricted host: three legacy command profiles asserted fail-closed; positive integration journeys require the disposable container run"
    );
    true
}

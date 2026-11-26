//! Capability-aware alternate assertions for positive-only integration journeys.
//! On a kernel without Landlock every command profile fails closed; the positive
//! journeys need Landlock and must never take this branch where it is present.
pub mod native_effects {
    pub fn identity() -> std::io::Result<String> {
        use std::io::Read;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

pub fn assert_unavailable_view() -> bool {
    let root = std::env::temp_dir().join(format!(
        "landlock-refusal-{}",
        native_effects::identity().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source"), "unchanged").unwrap();
    let available =
        workspace_service::native_command_supervisor::observe_command_view(&root, "read_only")
            .is_ok();
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(available, "positive integration journeys require Landlock");
    }
    if available {
        std::fs::remove_dir_all(root).unwrap();
        return false;
    }
    for profile in ["read_only", "guarded", "development"] {
        let e = workspace_service::native_command_supervisor::observe_command_view(&root, profile)
            .expect_err("a host without Landlock accepted a payload-capable sandbox");
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
        "Landlock-less host: three command profiles asserted fail-closed; positive integration journeys NOT RUN"
    );
    true
}

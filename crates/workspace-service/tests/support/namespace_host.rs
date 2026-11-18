//! Capability-aware alternate assertions for positive-only integration journeys.
//! The required container run must execute the real journeys, never this branch.
#[path = "../../src/native_command_sandbox.rs"]
mod sandbox;
// The sandbox reads the owner's tool search path from this module.
#[allow(dead_code, unused_imports)]
#[path = "../../src/tool_path.rs"]
pub mod tool_path;
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
    for profile in [
        sandbox::Profile::ReadOnly,
        sandbox::Profile::Guarded,
        sandbox::Profile::Development,
    ] {
        let e = match sandbox::Sandbox::prepare(&root, profile, &[root.clone()], &[], false) {
            Err(e) => e,
            Ok(_) => panic!("unavailable view accepted a payload-capable sandbox"),
        };
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
    println!("namespace-restricted host: all three production command profiles asserted fail-closed; positive integration journeys require the disposable container run");
    true
}

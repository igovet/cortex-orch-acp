fn main() {
    println!("cargo:rerun-if-env-changed=CORTEX_WORKSPACE_CHANNEL");
    println!("cargo:rerun-if-env-changed=CORTEX_WORKSPACE_RELEASE_AUTHORIZED");
    let channel = std::env::var("CORTEX_WORKSPACE_CHANNEL").unwrap_or_else(|_| "dev".into());
    assert!(
        channel == "dev" || channel == "release",
        "unknown workspace channel"
    );
    if channel == "release" {
        assert!(
            matches!(
                std::env::var("CORTEX_WORKSPACE_RELEASE_AUTHORIZED").as_deref(),
                Ok("github-main-clean") | Ok("local-owner-test")
            ),
            "release channel requires guarded GitHub main or explicit local owner authorization"
        );
    }
    println!("cargo:rustc-env=CORTEX_WORKSPACE_CHANNEL={channel}");
}

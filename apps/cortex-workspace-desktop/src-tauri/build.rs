fn main() {
    #[cfg(feature = "desktop")]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "workspace_request",
            "workspace_notify",
            "workspace_open_result_target",
            "workspace_close",
        ]),
    ))
    .expect("desktop configuration invalid");
}

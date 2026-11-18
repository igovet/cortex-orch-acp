fn main() {
    // The Tauri context is generated only for the Android build; the host builds the
    // shell-independent logic and its tests without a web view.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                "workspace_request",
                "phone_info",
                "phone_state",
                "phone_pair_begin",
                "phone_pair_finish",
                "phone_pair_cancel",
                "phone_select",
                "phone_rename",
                "phone_forget",
                "phone_retry",
                "phone_listen",
                "phone_exit",
            ]),
        ))
        .expect("phone configuration invalid");
    }
}

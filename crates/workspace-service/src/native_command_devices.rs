//! Host devices and hardware resources for confined commands. The owner's
//! decision: a command must never be blocked from the machine's GPU, audio,
//! video, display or session services. Commands run at the real paths with the
//! host `/dev` (Landlock grants device writes; device permissions still apply),
//! so only the environment passthrough below is needed.

/// Variables a display, audio, GPU or desktop-session client needs. They name
/// sockets and drivers, never credentials; the credential filter would
/// otherwise drop `XAUTHORITY` (AUTH) and `DBUS_SESSION_BUS_ADDRESS` (SESSION).
pub(crate) fn hardware_variable(name: &str) -> bool {
    matches!(
        name,
        "DISPLAY" | "WAYLAND_DISPLAY" | "XAUTHORITY" | "XDG_RUNTIME_DIR" | "DBUS_SESSION_BUS_ADDRESS"
            | "PULSE_SERVER" | "PULSE_RUNTIME_PATH" | "PIPEWIRE_RUNTIME_DIR" | "PIPEWIRE_REMOTE"
            | "XDG_SESSION_TYPE" | "VK_ICD_FILENAMES" | "VK_DRIVER_FILES" | "VK_LAYER_PATH"
            | "OCL_ICD_VENDORS" | "LIBVA_DRIVER_NAME" | "LIBVA_DRIVERS_PATH" | "VDPAU_DRIVER"
            | "__GLX_VENDOR_LIBRARY_NAME" | "__NV_PRIME_RENDER_OFFLOAD" | "__VK_LAYER_NV_optimus"
            | "LD_LIBRARY_PATH_NVIDIA"
    ) || name.starts_with("NVIDIA_")
        || name.starts_with("CUDA_")
        || name.starts_with("__NV_")
        || name.starts_with("MESA_")
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_and_session_variables_bypass_the_credential_filter() {
        for name in ["DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS", "NVIDIA_VISIBLE_DEVICES"] {
            assert!(super::hardware_variable(name), "{name}");
        }
        assert!(!super::hardware_variable("GITHUB_TOKEN"));
    }
}

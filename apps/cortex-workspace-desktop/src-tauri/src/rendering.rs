use webkit2gtk::{WebProcessTerminationReason, WebView, WebViewExt};

/// Keep compositing available during use. Only after acknowledged UI closure,
/// bypass WebKit 305909's EGL/Skia TLS exit race in this view's web process.
/// This neither stops the independent service nor changes WebKit's sandbox.
pub fn close(view: &WebView, finish: impl Fn() + 'static) {
    view.connect_web_process_terminated(move |_, reason| {
        if reason == WebProcessTerminationReason::TerminatedByApi { finish(); }
    });
    view.terminate_web_process();
}

#[cfg(test)]
mod tests {
    use webkit2gtk::{HardwareAccelerationPolicy, Settings, SettingsExt};

    #[test]
    #[ignore = "requires a disposable GTK display"]
    fn native_renderer_keeps_compositing_available() {
        gtk::init().expect("disposable GTK display");
        assert_ne!(Settings::new().hardware_acceleration_policy(), HardwareAccelerationPolicy::Never);
    }
}

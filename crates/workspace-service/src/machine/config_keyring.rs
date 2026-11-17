//! Native OS-keyring boundary for Settings validation and catalog refresh.
//!
//! The service links no OS keyring backend and never opens a keyring entry.
//! Provider/helper availability says nothing about Python's `import keyring`
//! or the selected backend. Requests needing that observation retain Python;
//! unavailable infrastructure refuses without manufacturing credential state.
//! Tests inject a fake backend only to compare observed lookup results.
use serde_json::Value;

/// One `keyring.get_password(service, account)` call: `Ok(Some(_))` is any
/// non-None value, including empty text; `Err` is a raised backend exception.
pub(super) trait Backend {
    fn get_password(&self, service: &str, account: &str) -> Result<Option<String>, String>;
}

pub(super) fn reference(r: &Value) -> bool {
    r["credential_ref"]["kind"] == "os_keyring"
}

/// onboarding._derive_state for an observed backend lookup. The value itself
/// is dropped immediately, never kept. An absent provider is not an observation.
pub(super) fn state(backend: &dyn Backend, r: &Value) -> &'static str {
    let reference = &r["credential_ref"];
    match backend.get_password(
        reference["service"].as_str().unwrap_or(""),
        reference["account"].as_str().unwrap_or(""),
    ) {
        Ok(Some(_)) => "configured",
        Ok(None) => "authorization_required",
        Err(_) => "unavailable",
    }
}

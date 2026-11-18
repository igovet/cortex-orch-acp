//! Cortex Workspace phone shell. The shared interface runs in the Android web view; this crate
//! is its closed native bridge to paired remote machines. See `phone` for what it can do.

pub mod phone;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::run;

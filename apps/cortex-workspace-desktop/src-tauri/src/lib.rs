pub mod client;
pub mod history;
pub mod navigation;
mod opener;
pub mod policy;
pub mod stall;

#[cfg(feature = "desktop")]
mod attachments;
#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
mod notification;
#[cfg(feature = "desktop")]
mod rendering;
#[cfg(feature = "desktop")]
pub use desktop::run;

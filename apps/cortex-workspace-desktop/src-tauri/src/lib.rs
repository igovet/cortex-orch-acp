pub mod client;
mod opener;
pub mod policy;

#[cfg(feature = "desktop")]
mod attachments;
#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
mod rendering;
#[cfg(feature = "desktop")]
mod notification;
#[cfg(feature = "desktop")]
pub use desktop::run;

pub mod library;

#[cfg(feature = "desktop")]
pub mod mcp;

#[cfg(feature = "desktop")]
mod desktop;

#[cfg(feature = "desktop")]
pub use desktop::run;

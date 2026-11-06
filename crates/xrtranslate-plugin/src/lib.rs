//! Plugins own their work; the host supplies services and renders their controls.
//! The wire contract is shared by native plugins and the host, without a Rust ABI.
mod protocol;
mod server;
pub mod translation;
pub use protocol::*;
pub use server::{Context, Plugin, serve};
#[cfg(feature = "host")]
pub mod directory;
#[cfg(feature = "host")]
pub mod host;

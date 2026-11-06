//! Plugins own their work; the host supplies services and renders their controls.
//! The wire contract is shared by native plugins and the host, without a Rust ABI.
mod client;
mod protocol;
mod server;
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
pub use client::{Client, Request};
#[cfg(feature = "egui")]
pub mod surface;
pub mod translation;
pub use protocol::*;
pub use server::{Context, Plugin, serve};
#[cfg(feature = "host")]
pub mod directory;
#[cfg(feature = "host")]
pub mod host;

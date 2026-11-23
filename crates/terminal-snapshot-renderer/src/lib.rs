//! Terminal snapshot renderer: turns a terminal grid into a PNG image.
//! Re-exports the protocol, JSON, render and PNG validation modules.
//! It does no I/O with sessions or PTYs.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "protocol")]
mod json;
#[cfg(feature = "protocol")]
mod png_validation;
#[cfg(feature = "protocol")]
mod protocol;
#[cfg(feature = "render")]
mod render;

#[cfg(feature = "protocol")]
pub use json::*;
#[cfg(feature = "protocol")]
pub use png_validation::*;
#[cfg(feature = "protocol")]
pub use protocol::*;
#[cfg(feature = "render")]
pub use render::*;

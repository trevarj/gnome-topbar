//! Core types shared by the topbar panel and its services.
//!
//! This crate is deliberately dependency-light: no GTK, no tokio. It owns the
//! configuration schema, the CLI ↔ panel IPC protocol, theme color primitives,
//! pure layout math, and logging setup, so both the GTK crate and the services
//! crate can depend on it without pulling either stack into the other.

#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod ipc;
pub mod layout_math;
pub mod logging;
pub mod theme;
pub mod xkb_names;

pub use config::{Config, ConfigLoad, EXAMPLE_CONFIG_TOML, Warning};
pub use error::{Error, Result};
pub use ipc::{IpcRequest, IpcResponse, PROTOCOL_VERSION};
pub use theme::{Rgb, parse_hex_color};

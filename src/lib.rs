//! # sml_mcps - Small MCP Server
//!
//! A minimal, sync MCP server implementation. No tokio, no async, just works.
//!
//! ## Features
//!
//! - `schema` (default) - JSON Schema generation for tools via schemars
//! - `http` - Streamable HTTP transport via tiny_http
//! - `auth` - JWT validation for hosted deployments
//! - `hosted` - Enables both `http` and `auth`

pub mod types;
pub mod transport;
pub mod server;

#[cfg(feature = "auth")]
pub mod auth;

// Re-export commonly used types
pub use types::*;
pub use transport::{Transport, StdioTransport};
pub use server::{Server, ServerConfig, Tool, Resource, PromptDef, ToolEnv, LogLevel};

#[cfg(feature = "http")]
pub use transport::HttpTransport;

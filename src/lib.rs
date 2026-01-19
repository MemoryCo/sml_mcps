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

pub mod server;
pub mod transport;
pub mod types;

#[cfg(feature = "auth")]
pub mod auth;

// Re-export commonly used types
pub use server::{LogLevel, PromptDef, Resource, Server, ServerConfig, Tool, ToolEnv};
pub use transport::{StdioTransport, Transport};
pub use types::*;

#[cfg(feature = "http")]
pub use transport::HttpTransport;

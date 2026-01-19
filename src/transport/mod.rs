//! Transport Layer
//!
//! Abstracts communication between client and server.

mod stdio;

#[cfg(feature = "http")]
mod http;

pub use stdio::StdioTransport;

#[cfg(feature = "http")]
pub use http::HttpTransport;

use crate::types::{JsonRpcMessage, Result};

/// Transport trait - sync read/write of JSON-RPC messages
pub trait Transport: Send + Sync {
    /// Read a single message from the transport
    fn read(&mut self) -> Result<JsonRpcMessage>;
    
    /// Write a single message to the transport
    fn write(&mut self, message: &JsonRpcMessage) -> Result<()>;
    
    /// Close the transport
    fn close(&mut self) -> Result<()>;
}

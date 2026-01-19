//! HTTP Transport
//!
//! Streamable HTTP transport for remote MCP servers.
//! Returns SSE stream to support notifications and progress.

use crate::transport::Transport;
use crate::types::{JsonRpcMessage, McpError, Result};
use std::sync::{Arc, Mutex};

/// HTTP request/response transport with SSE support
///
/// Buffers all outgoing messages and returns them as an SSE stream.
pub struct HttpTransport {
    /// The request body (JSON-RPC message)
    request: Option<String>,
    /// Buffered messages to return as SSE stream
    messages: Arc<Mutex<Vec<String>>>,
}

impl HttpTransport {
    /// Create a new HTTP transport from a request body
    pub fn new(request_body: String) -> Self {
        Self {
            request: Some(request_body),
            messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Take the response as an SSE stream body
    ///
    /// Returns the messages formatted as SSE events:
    /// ```text
    /// data: {"jsonrpc":"2.0",...}
    ///
    /// data: {"jsonrpc":"2.0",...}
    ///
    /// ```
    pub fn take_sse_response(&mut self) -> String {
        let messages = self.messages.lock().unwrap();
        messages
            .iter()
            .map(|msg| format!("data: {}\n\n", msg))
            .collect()
    }

    /// Take response as plain JSON (for single response, no notifications)
    ///
    /// Returns just the last message (the actual response), or empty if none.
    pub fn take_response(&mut self) -> Option<String> {
        let messages = self.messages.lock().unwrap();
        messages.last().cloned()
    }

    /// Check if there are multiple messages (notifications + response)
    pub fn has_notifications(&self) -> bool {
        let messages = self.messages.lock().unwrap();
        messages.len() > 1
    }
}

impl Transport for HttpTransport {
    fn read(&mut self) -> Result<JsonRpcMessage> {
        let body = self.request.take().ok_or(McpError::TransportClosed)?;

        let message: JsonRpcMessage = serde_json::from_str(&body)?;
        Ok(message)
    }

    fn write(&mut self, message: &JsonRpcMessage) -> Result<()> {
        let json = serde_json::to_string(message)?;
        let mut messages = self
            .messages
            .lock()
            .map_err(|_| McpError::Internal("Lock poisoned".into()))?;
        messages.push(json);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JsonRpcMessage;

    #[test]
    fn test_http_transport_roundtrip() {
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut transport = HttpTransport::new(request.to_string());

        // Read the request
        let msg = transport.read().unwrap();
        if let JsonRpcMessage::Request(req) = msg {
            assert_eq!(req.method, "ping");
        } else {
            panic!("Expected request");
        }

        // Write a response
        let response = JsonRpcMessage::response(1i64, serde_json::json!({}));
        transport.write(&response).unwrap();

        // Get the response body
        let body = transport.take_response().unwrap();
        assert!(body.contains("\"result\":{}"));
    }

    #[test]
    fn test_http_transport_sse_multiple_messages() {
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"test"}"#;
        let mut transport = HttpTransport::new(request.to_string());

        // Simulate notification + response
        let notification = JsonRpcMessage::notification(
            "notifications/message",
            Some(serde_json::json!({"level": "info", "data": "hello"})),
        );
        transport.write(&notification).unwrap();

        let response = JsonRpcMessage::response(1i64, serde_json::json!({"result": "done"}));
        transport.write(&response).unwrap();

        assert!(transport.has_notifications());

        let sse = transport.take_sse_response();
        assert!(sse.contains("data: "));
        assert!(sse.contains("notifications/message"));
        assert!(sse.contains("\"result\":\"done\""));
    }
}

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

//
// HttpServer - high-level server wrapper
//

use crate::server::{Server, ServerConfig};
use tiny_http::{Header, Method, Response, Server as TinyServer};

#[cfg(feature = "auth")]
use crate::auth::{Claims, JwtValidator};

/// High-level HTTP MCP server
///
/// Wraps the request loop boilerplate for serving MCP over HTTP.
///
/// # Example (no auth)
/// ```ignore
/// HttpServer::new(config)
///     .with_tools(|s| {
///         s.add_tool(EchoTool)?;
///         s.add_tool(CounterTool)?;
///         Ok(())
///     })
///     .serve("127.0.0.1:3000", || AppContext::new())?;
/// ```
///
/// # Example (with JWT auth)
/// ```ignore
/// HttpServer::new(config)
///     .with_tools(|s| {
///         s.add_tool(WhoamiTool)?;
///         Ok(())
///     })
///     .serve_with_auth(
///         "127.0.0.1:3001",
///         JwtValidator::hs256(SECRET),
///         |claims| AuthContext {
///             user_id: claims.user_id().to_string(),
///             tenant_id: claims.tenant_id().to_string(),
///         },
///     )?;
/// ```
pub struct HttpServer<C> {
    config: ServerConfig,
    endpoint: String,
    setup: Option<Box<dyn Fn(&mut Server<C>) -> Result<()> + Send + Sync>>,
}

impl<C: Send + Sync + 'static> HttpServer<C> {
    /// Create a new HTTP server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            endpoint: "/mcp".to_string(),
            setup: None,
        }
    }

    /// Set the endpoint path (default: "/mcp")
    pub fn endpoint(mut self, path: impl Into<String>) -> Self {
        self.endpoint = path.into();
        self
    }

    /// Configure tools via a setup closure
    ///
    /// The closure is called for each request to set up a fresh server.
    pub fn with_tools<F>(mut self, setup: F) -> Self
    where
        F: Fn(&mut Server<C>) -> Result<()> + Send + Sync + 'static,
    {
        self.setup = Some(Box::new(setup));
        self
    }

    /// Serve without authentication
    ///
    /// The context factory is called for each request.
    pub fn serve<F>(self, addr: &str, context_factory: F) -> Result<()>
    where
        F: Fn() -> C,
    {
        let http_server = TinyServer::http(addr)
            .map_err(|e| McpError::Internal(format!("Failed to start HTTP server: {}", e)))?;

        eprintln!(
            "MCP HTTP server `{}` listening on http://{}{}",
            self.config.name, addr, self.endpoint
        );

        for mut request in http_server.incoming_requests() {
            let path = request.url().to_string();
            let method = request.method().clone();

            eprintln!("{} {}", method, path);

            // Validate endpoint
            if path != self.endpoint {
                let response = Response::from_string("Not Found").with_status_code(404);
                let _ = request.respond(response);
                continue;
            }

            // Validate method
            if method != Method::Post {
                let response = Response::from_string("Method Not Allowed").with_status_code(405);
                let _ = request.respond(response);
                continue;
            }

            // Read body
            let mut body = String::new();
            if let Err(e) = request.as_reader().read_to_string(&mut body) {
                eprintln!("  Failed to read body: {}", e);
                let response = Response::from_string("Bad Request").with_status_code(400);
                let _ = request.respond(response);
                continue;
            }

            eprintln!("  Request: {}", body);

            // Process request
            let mut ctx = context_factory();
            match self.process_request(body, &mut ctx) {
                Ok((response_body, content_type)) => {
                    eprintln!("  Response ({}): {}", content_type, response_body);
                    let header = Header::from_bytes("Content-Type", content_type).unwrap();
                    let response = Response::from_string(response_body).with_header(header);
                    if let Err(e) = request.respond(response) {
                        eprintln!("  Failed to send response: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("  Error: {}", e);
                    let response = Response::from_string(format!("Internal Error: {}", e))
                        .with_status_code(500);
                    let _ = request.respond(response);
                }
            }
        }

        Ok(())
    }

    /// Serve with JWT authentication
    ///
    /// The context factory receives validated claims and creates a context.
    #[cfg(feature = "auth")]
    pub fn serve_with_auth<F>(
        self,
        addr: &str,
        validator: JwtValidator,
        context_factory: F,
    ) -> Result<()>
    where
        F: Fn(&Claims) -> C,
    {
        let http_server = TinyServer::http(addr)
            .map_err(|e| McpError::Internal(format!("Failed to start HTTP server: {}", e)))?;

        eprintln!(
            "MCP HTTP server `{}` (authenticated) listening on http://{}{}",
            self.config.name, addr, self.endpoint
        );

        for mut request in http_server.incoming_requests() {
            let path = request.url().to_string();
            let method = request.method().clone();

            eprintln!("{} {}", method, path);

            // Validate endpoint
            if path != self.endpoint {
                let response = Response::from_string("Not Found").with_status_code(404);
                let _ = request.respond(response);
                continue;
            }

            // Validate method
            if method != Method::Post {
                let response = Response::from_string("Method Not Allowed").with_status_code(405);
                let _ = request.respond(response);
                continue;
            }

            // JWT Authentication
            let auth_header = request
                .headers()
                .iter()
                .find(|h| {
                    let field = h.field.as_str();
                    field == "Authorization" || field == "authorization"
                })
                .map(|h| h.value.as_str());

            let claims = match auth_header {
                Some(header) => match validator.validate_header(header) {
                    Ok(claims) => {
                        eprintln!(
                            "  ✓ Authenticated: user={}, tenant={}",
                            claims.user_id(),
                            claims.tenant_id()
                        );
                        claims
                    }
                    Err(e) => {
                        eprintln!("  ✗ Auth failed: {}", e);
                        let response = Response::from_string(format!("Unauthorized: {}", e))
                            .with_status_code(401);
                        let _ = request.respond(response);
                        continue;
                    }
                },
                None => {
                    eprintln!("  ✗ No Authorization header");
                    let response =
                        Response::from_string("Unauthorized: Missing Authorization header")
                            .with_status_code(401);
                    let _ = request.respond(response);
                    continue;
                }
            };

            // Read body
            let mut body = String::new();
            if let Err(e) = request.as_reader().read_to_string(&mut body) {
                eprintln!("  Failed to read body: {}", e);
                let response = Response::from_string("Bad Request").with_status_code(400);
                let _ = request.respond(response);
                continue;
            }

            eprintln!("  Request: {}", body);

            // Process request with auth context
            let mut ctx = context_factory(&claims);
            match self.process_request(body, &mut ctx) {
                Ok((response_body, content_type)) => {
                    eprintln!("  Response ({}): {}", content_type, response_body);
                    let header = Header::from_bytes("Content-Type", content_type).unwrap();
                    let response = Response::from_string(response_body).with_header(header);
                    if let Err(e) = request.respond(response) {
                        eprintln!("  Failed to send response: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("  Error: {}", e);
                    let response = Response::from_string(format!("Internal Error: {}", e))
                        .with_status_code(500);
                    let _ = request.respond(response);
                }
            }
        }

        Ok(())
    }

    /// Process a single request and return (body, content_type)
    fn process_request(&self, body: String, ctx: &mut C) -> Result<(String, &'static str)> {
        // Create fresh server
        let mut server: Server<C> = Server::new(self.config.clone());

        // Set up tools
        if let Some(ref setup) = self.setup {
            setup(&mut server)?;
        }

        // Create transport and process
        let transport = Arc::new(Mutex::new(HttpTransport::new(body)));
        server.process_one(transport.clone(), ctx)?;

        // Extract response
        let mut transport_guard = transport
            .lock()
            .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;

        if transport_guard.has_notifications() {
            Ok((transport_guard.take_sse_response(), "text/event-stream"))
        } else {
            Ok((
                transport_guard.take_response().unwrap_or_else(|| "{}".to_string()),
                "application/json",
            ))
        }
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

/// Integration tests for HttpServer
/// These spawn actual servers and make HTTP requests
#[cfg(test)]
mod http_server_tests {
    use super::*;
    use crate::server::{Server, ServerConfig, Tool, ToolEnv};
    use crate::types::{CallToolResult, Result};
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicI64, AtomicU16, Ordering};
    use std::thread;
    use std::time::Duration;

    // Port counter to avoid conflicts between tests
    static PORT: AtomicU16 = AtomicU16::new(13000);

    fn next_port() -> u16 {
        PORT.fetch_add(1, Ordering::SeqCst)
    }

    // Test context
    struct TestContext {
        counter: Arc<AtomicI64>,
    }

    // Simple echo tool (no notifications)
    struct EchoTool;
    impl Tool<TestContext> for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echo a message"
        }
        fn schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            })
        }
        fn execute(
            &self,
            args: Value,
            _ctx: &mut TestContext,
            _env: &ToolEnv,
        ) -> Result<CallToolResult> {
            let msg = args.get("message").and_then(|m| m.as_str()).unwrap_or("");
            Ok(CallToolResult::text(format!("Echo: {}", msg)))
        }
    }

    // Tool that sends notifications (triggers SSE)
    struct NotifyTool;
    impl Tool<TestContext> for NotifyTool {
        fn name(&self) -> &str {
            "notify"
        }
        fn description(&self) -> &str {
            "Tool that sends notifications"
        }
        fn schema(&self) -> Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn execute(
            &self,
            _args: Value,
            _ctx: &mut TestContext,
            env: &ToolEnv,
        ) -> Result<CallToolResult> {
            use crate::server::LogLevel;
            env.log(LogLevel::Info, "notification from tool")?;
            Ok(CallToolResult::text("done with notification"))
        }
    }

    // Counter tool that uses context
    struct CounterTool;
    impl Tool<TestContext> for CounterTool {
        fn name(&self) -> &str {
            "counter"
        }
        fn description(&self) -> &str {
            "Increment shared counter"
        }
        fn schema(&self) -> Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn execute(
            &self,
            _args: Value,
            ctx: &mut TestContext,
            _env: &ToolEnv,
        ) -> Result<CallToolResult> {
            let val = ctx.counter.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(CallToolResult::text(format!("Counter: {}", val)))
        }
    }

    /// Helper to make raw HTTP POST request
    fn http_post(addr: &str, path: &str, body: &str) -> std::io::Result<(u16, String, String)> {
        let mut stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let request = format!(
            "POST {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            path,
            addr,
            body.len(),
            body
        );

        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        // Parse status code
        let status_line = response.lines().next().unwrap_or("");
        let status_code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Parse content-type
        let content_type = response
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-type:"))
            .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
            .unwrap_or_default();

        // Parse body (after empty line)
        let body = response
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .to_string();

        Ok((status_code, content_type, body))
    }

    #[test]
    fn test_http_server_ping() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(EchoTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        // Give server time to start
        thread::sleep(Duration::from_millis(100));

        // Test ping
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let (status, content_type, response) = http_post(&addr, "/mcp", body).unwrap();

        assert_eq!(status, 200);
        assert_eq!(content_type, "application/json");
        assert!(response.contains("\"result\":{}"));

        drop(handle); // Server will stop when test ends
    }

    #[test]
    fn test_http_server_tool_call_json_response() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(EchoTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        // Call echo tool (no notifications -> JSON response)
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"message":"hello"}}}"#;
        let (status, content_type, response) = http_post(&addr, "/mcp", body).unwrap();

        assert_eq!(status, 200);
        assert_eq!(content_type, "application/json");
        assert!(response.contains("Echo: hello"));

        drop(handle);
    }

    #[test]
    fn test_http_server_tool_call_sse_response() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(NotifyTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        // Call notify tool (has notifications -> SSE response)
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"notify"}}"#;
        let (status, content_type, response) = http_post(&addr, "/mcp", body).unwrap();

        assert_eq!(status, 200);
        assert_eq!(content_type, "text/event-stream");
        assert!(response.contains("data: "));
        assert!(response.contains("notification from tool"));
        assert!(response.contains("done with notification"));

        drop(handle);
    }

    #[test]
    fn test_http_server_wrong_endpoint_404() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(EchoTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        // Wrong endpoint
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let (status, _, response) = http_post(&addr, "/wrong", body).unwrap();

        assert_eq!(status, 404);
        assert!(response.contains("Not Found"));

        drop(handle);
    }

    #[test]
    fn test_http_server_custom_endpoint() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .endpoint("/custom/path")
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(EchoTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        // Custom endpoint should work
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let (status, _, _) = http_post(&addr, "/custom/path", body).unwrap();
        assert_eq!(status, 200);

        // Default endpoint should 404
        let (status, _, _) = http_post(&addr, "/mcp", body).unwrap();
        assert_eq!(status, 404);

        drop(handle);
    }

    #[test]
    fn test_http_server_shared_context() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        // Shared counter
        let shared_counter = Arc::new(AtomicI64::new(0));
        let counter_for_server = shared_counter.clone();

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(CounterTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter_for_server.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"counter"}}"#;

        // First call
        let (_, _, response) = http_post(&addr, "/mcp", body).unwrap();
        assert!(response.contains("Counter: 1"));

        // Second call - should increment
        let (_, _, response) = http_post(&addr, "/mcp", body).unwrap();
        assert!(response.contains("Counter: 2"));

        // Verify shared counter
        assert_eq!(shared_counter.load(Ordering::SeqCst), 2);

        drop(handle);
    }

    #[test]
    fn test_http_server_tools_list() {
        let port = next_port();
        let addr = format!("127.0.0.1:{}", port);

        let config = ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: None,
        };

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let counter = Arc::new(AtomicI64::new(0));
            let _ = HttpServer::new(config)
                .with_tools(|s: &mut Server<TestContext>| {
                    s.add_tool(EchoTool)?;
                    s.add_tool(NotifyTool)?;
                    s.add_tool(CounterTool)?;
                    Ok(())
                })
                .serve(&server_addr, move || TestContext {
                    counter: counter.clone(),
                });
        });

        thread::sleep(Duration::from_millis(100));

        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let (status, _, response) = http_post(&addr, "/mcp", body).unwrap();

        assert_eq!(status, 200);
        assert!(response.contains("\"name\":\"echo\""));
        assert!(response.contains("\"name\":\"notify\""));
        assert!(response.contains("\"name\":\"counter\""));

        drop(handle);
    }

    #[cfg(feature = "auth")]
    mod auth_tests {
        use super::*;
        use crate::auth::JwtValidator;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header as JwtHeader};
        use serde::Serialize;

        const SECRET: &[u8] = b"test-secret-key";

        struct AuthContext {
            user_id: String,
        }

        struct WhoamiTool;
        impl Tool<AuthContext> for WhoamiTool {
            fn name(&self) -> &str {
                "whoami"
            }
            fn description(&self) -> &str {
                "Return user info"
            }
            fn schema(&self) -> Value {
                serde_json::json!({ "type": "object", "properties": {} })
            }
            fn execute(
                &self,
                _args: Value,
                ctx: &mut AuthContext,
                _env: &ToolEnv,
            ) -> Result<CallToolResult> {
                Ok(CallToolResult::text(format!("User: {}", ctx.user_id)))
            }
        }

        fn make_token(user_id: &str, tenant_id: &str) -> String {
            #[derive(Serialize)]
            struct Claims {
                sub: String,
                tenant_id: String,
                exp: u64,
            }

            let claims = Claims {
                sub: user_id.into(),
                tenant_id: tenant_id.into(),
                exp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 3600,
            };

            encode(
                &JwtHeader::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(SECRET),
            )
            .unwrap()
        }

        fn http_post_with_auth(
            addr: &str,
            path: &str,
            body: &str,
            token: Option<&str>,
        ) -> std::io::Result<(u16, String, String)> {
            let mut stream = TcpStream::connect(addr)?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;

            let auth_header = token
                .map(|t| format!("Authorization: Bearer {}\r\n", t))
                .unwrap_or_default();

            let request = format!(
                "POST {} HTTP/1.1\r\n\
                 Host: {}\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 {}Connection: close\r\n\
                 \r\n\
                 {}",
                path,
                addr,
                body.len(),
                auth_header,
                body
            );

            stream.write_all(request.as_bytes())?;
            stream.flush()?;

            let mut response = String::new();
            stream.read_to_string(&mut response)?;

            let status_line = response.lines().next().unwrap_or("");
            let status_code: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            let content_type = response
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-type:"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
                .unwrap_or_default();

            let body = response
                .split("\r\n\r\n")
                .nth(1)
                .unwrap_or("")
                .to_string();

            Ok((status_code, content_type, body))
        }

        #[test]
        fn test_http_server_auth_missing_header() {
            let port = next_port();
            let addr = format!("127.0.0.1:{}", port);

            let config = ServerConfig {
                name: "test-auth".into(),
                version: "1.0.0".into(),
                instructions: None,
            };

            let server_addr = addr.clone();
            let handle = thread::spawn(move || {
                let _ = HttpServer::new(config)
                    .with_tools(|s: &mut Server<AuthContext>| {
                        s.add_tool(WhoamiTool)?;
                        Ok(())
                    })
                    .serve_with_auth(&server_addr, JwtValidator::hs256(SECRET), |claims| {
                        AuthContext {
                            user_id: claims.user_id().to_string(),
                        }
                    });
            });

            thread::sleep(Duration::from_millis(100));

            let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
            let (status, _, response) = http_post_with_auth(&addr, "/mcp", body, None).unwrap();

            assert_eq!(status, 401);
            assert!(response.contains("Missing Authorization"));

            drop(handle);
        }

        #[test]
        fn test_http_server_auth_invalid_token() {
            let port = next_port();
            let addr = format!("127.0.0.1:{}", port);

            let config = ServerConfig {
                name: "test-auth".into(),
                version: "1.0.0".into(),
                instructions: None,
            };

            let server_addr = addr.clone();
            let handle = thread::spawn(move || {
                let _ = HttpServer::new(config)
                    .with_tools(|s: &mut Server<AuthContext>| {
                        s.add_tool(WhoamiTool)?;
                        Ok(())
                    })
                    .serve_with_auth(&server_addr, JwtValidator::hs256(SECRET), |claims| {
                        AuthContext {
                            user_id: claims.user_id().to_string(),
                        }
                    });
            });

            thread::sleep(Duration::from_millis(100));

            let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
            let (status, _, _) =
                http_post_with_auth(&addr, "/mcp", body, Some("invalid-token")).unwrap();

            assert_eq!(status, 401);

            drop(handle);
        }

        #[test]
        fn test_http_server_auth_valid_token() {
            let port = next_port();
            let addr = format!("127.0.0.1:{}", port);

            let config = ServerConfig {
                name: "test-auth".into(),
                version: "1.0.0".into(),
                instructions: None,
            };

            let server_addr = addr.clone();
            let handle = thread::spawn(move || {
                let _ = HttpServer::new(config)
                    .with_tools(|s: &mut Server<AuthContext>| {
                        s.add_tool(WhoamiTool)?;
                        Ok(())
                    })
                    .serve_with_auth(&server_addr, JwtValidator::hs256(SECRET), |claims| {
                        AuthContext {
                            user_id: claims.user_id().to_string(),
                        }
                    });
            });

            thread::sleep(Duration::from_millis(100));

            let token = make_token("alice", "tenant-1");
            let body =
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"whoami"}}"#;
            let (status, _, response) =
                http_post_with_auth(&addr, "/mcp", body, Some(&token)).unwrap();

            assert_eq!(status, 200);
            assert!(response.contains("User: alice"));

            drop(handle);
        }
    }
}

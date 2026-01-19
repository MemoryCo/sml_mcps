//! HTTP server example with SSE responses
//!
//! Implements Streamable HTTP spec - POST with SSE response for notifications.
//!
//! Run with: cargo run --example http_server --features http

use serde_json::Value;
use sml_mcps::{
    CallToolResult, HttpTransport, LogLevel, McpError, Result, Server, ServerConfig, Tool, ToolEnv,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server as TinyServer};

/// Shared context - thread-safe for HTTP
struct AppContext {
    counter: Arc<AtomicI64>,
}

impl AppContext {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
        }
    }
}

struct EchoTool;
impl Tool<AppContext> for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echo back the input message"
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "The message to echo back" }
            },
            "required": ["message"]
        })
    }
    fn execute(&self, args: Value, _ctx: &mut AppContext, env: &ToolEnv) -> Result<CallToolResult> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidParams("Missing 'message' parameter".into()))?;

        // This notification will be included in the SSE stream!
        env.log(LogLevel::Info, format!("Echoing: {}", message))?;

        Ok(CallToolResult::text(format!("Echo: {}", message)))
    }
}

struct CounterGetTool;
impl Tool<AppContext> for CounterGetTool {
    fn name(&self) -> &str {
        "counter_get"
    }
    fn description(&self) -> &str {
        "Get the current counter value"
    }
    fn schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn execute(
        &self,
        _args: Value,
        ctx: &mut AppContext,
        _env: &ToolEnv,
    ) -> Result<CallToolResult> {
        let value = ctx.counter.load(Ordering::SeqCst);
        Ok(CallToolResult::text(format!("Counter value: {}", value)))
    }
}

struct CounterIncrementTool;
impl Tool<AppContext> for CounterIncrementTool {
    fn name(&self) -> &str {
        "counter_increment"
    }
    fn description(&self) -> &str {
        "Increment the counter and return the new value"
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "amount": { "type": "integer", "description": "Amount to increment by (default: 1)" }
            }
        })
    }
    fn execute(&self, args: Value, ctx: &mut AppContext, env: &ToolEnv) -> Result<CallToolResult> {
        let amount = args.get("amount").and_then(|a| a.as_i64()).unwrap_or(1);
        let new_value = ctx.counter.fetch_add(amount, Ordering::SeqCst) + amount;

        // Log notification - will be in SSE stream
        env.log(
            LogLevel::Debug,
            format!("Counter incremented by {} to {}", amount, new_value),
        )?;

        Ok(CallToolResult::text(format!(
            "Counter incremented to: {}",
            new_value
        )))
    }
}

struct CounterResetTool;
impl Tool<AppContext> for CounterResetTool {
    fn name(&self) -> &str {
        "counter_reset"
    }
    fn description(&self) -> &str {
        "Reset the counter to zero"
    }
    fn schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn execute(
        &self,
        _args: Value,
        ctx: &mut AppContext,
        _env: &ToolEnv,
    ) -> Result<CallToolResult> {
        ctx.counter.store(0, Ordering::SeqCst);
        Ok(CallToolResult::text("Counter reset to 0"))
    }
}

fn main() {
    let addr = "127.0.0.1:3000";
    eprintln!("Starting HTTP MCP server on http://{}/mcp", addr);
    eprintln!("Responses use SSE format when tools send notifications");
    eprintln!();
    eprintln!("Test with:");
    eprintln!(
        "  curl -X POST http://{}/mcp -H 'Content-Type: application/json' \\",
        addr
    );
    eprintln!(
        "    -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"echo\",\"arguments\":{{\"message\":\"hello\"}}}}}}'"
    );
    eprintln!();

    let http_server = TinyServer::http(addr).expect("Failed to start HTTP server");

    // Shared context across requests
    let shared_context = AppContext::new();

    eprintln!("Server ready, waiting for requests...\n");

    for mut request in http_server.incoming_requests() {
        let path = request.url().to_string();
        let method = request.method().clone();

        eprintln!("{} {}", method, path);

        if path != "/mcp" {
            let response = Response::from_string("Not Found").with_status_code(404);
            let _ = request.respond(response);
            continue;
        }

        if method != Method::Post {
            let response = Response::from_string("Method Not Allowed").with_status_code(405);
            let _ = request.respond(response);
            continue;
        }

        // Read request body
        let mut body = String::new();
        if let Err(e) = request.as_reader().read_to_string(&mut body) {
            eprintln!("  Failed to read body: {}", e);
            let response = Response::from_string("Bad Request").with_status_code(400);
            let _ = request.respond(response);
            continue;
        }

        eprintln!("  Request: {}", body);

        // Create fresh server
        let config = ServerConfig {
            name: "simple-http".to_string(),
            version: "1.0.0".to_string(),
            instructions: Some("A simple HTTP MCP server with echo and counter tools.".to_string()),
        };

        let mut server: Server<AppContext> = Server::new(config);
        server.add_tool(EchoTool).unwrap();
        server.add_tool(CounterGetTool).unwrap();
        server.add_tool(CounterIncrementTool).unwrap();
        server.add_tool(CounterResetTool).unwrap();

        // Clone context for this request (shares counter via Arc)
        let mut ctx = AppContext {
            counter: shared_context.counter.clone(),
        };

        // Create transport wrapped in Arc<Mutex> so we can get it back
        let transport = Arc::new(Mutex::new(HttpTransport::new(body)));

        if let Err(e) = server.process_one(transport.clone(), &mut ctx) {
            eprintln!("  Error: {}", e);
            let response =
                Response::from_string(format!("Internal Error: {}", e)).with_status_code(500);
            let _ = request.respond(response);
            continue;
        }

        // Extract response from transport
        let mut transport_guard = transport.lock().unwrap();

        let (response_body, content_type) = if transport_guard.has_notifications() {
            // SSE format for tool calls with notifications
            let sse = transport_guard.take_sse_response();
            eprintln!("  Response (SSE):\n{}", sse);
            (sse, "text/event-stream")
        } else {
            // Plain JSON for simple requests
            let json = transport_guard
                .take_response()
                .unwrap_or_else(|| "{}".to_string());
            eprintln!("  Response (JSON): {}", json);
            (json, "application/json")
        };

        let content_type_header = Header::from_bytes("Content-Type", content_type).unwrap();
        let response = Response::from_string(response_body).with_header(content_type_header);

        if let Err(e) = request.respond(response) {
            eprintln!("  Failed to send response: {}", e);
        }
    }
}

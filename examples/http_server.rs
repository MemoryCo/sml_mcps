//! HTTP server example with SSE responses
//!
//! Implements Streamable HTTP spec - POST with SSE response for notifications.
//!
//! Run with: cargo run --example http_server --features http

use serde_json::Value;
use sml_mcps::{
    CallToolResult, HttpServer, LogLevel, McpError, Result, ServerConfig, Tool, ToolEnv,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// Shared context - thread-safe for HTTP
struct AppContext {
    counter: Arc<AtomicI64>,
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

fn main() -> Result<()> {
    let addr = "127.0.0.1:3000";

    eprintln!("Test with:");
    eprintln!(
        "  curl -X POST http://{}/mcp -H 'Content-Type: application/json' \\",
        addr
    );
    eprintln!(
        "    -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"echo\",\"arguments\":{{\"message\":\"hello\"}}}}}}'"
    );
    eprintln!();

    // Shared counter across requests
    let shared_counter = Arc::new(AtomicI64::new(0));

    let config = ServerConfig {
        name: "simple-http".to_string(),
        version: "1.0.0".to_string(),
        instructions: Some("A simple HTTP MCP server with echo and counter tools.".to_string()),
    };

    HttpServer::new(config)
        .with_tools(|server| {
            server.add_tool(EchoTool)?;
            server.add_tool(CounterGetTool)?;
            server.add_tool(CounterIncrementTool)?;
            server.add_tool(CounterResetTool)?;
            Ok(())
        })
        .serve(addr, {
            let counter = shared_counter.clone();
            move || AppContext {
                counter: counter.clone(),
            }
        })
}

//! Simple example MCP server with counter and echo tools.
//!
//! Now with proper context support like sovran-mcp!
//!
//! Run with: cargo run --example simple

use serde_json::Value;
use sml_mcps::{
    CallToolResult, LogLevel, McpError, Result, Server, ServerConfig, StdioTransport, Tool, ToolEnv,
};

/// Shared context between tools
struct AppContext {
    counter: i64,
    echo_count: u32,
}

impl AppContext {
    fn new() -> Self {
        Self {
            counter: 0,
            echo_count: 0,
        }
    }
}

/// Echo tool - demonstrates logging
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
                "message": {
                    "type": "string",
                    "description": "The message to echo back"
                }
            },
            "required": ["message"]
        })
    }

    fn execute(&self, args: Value, ctx: &mut AppContext, env: &ToolEnv) -> Result<CallToolResult> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidParams("Missing 'message' parameter".into()))?;

        ctx.echo_count += 1;

        // Log to client
        env.log(
            LogLevel::Info,
            format!("Echo #{}: {}", ctx.echo_count, message),
        )?;

        Ok(CallToolResult::text(format!("Echo: {}", message)))
    }
}

/// Counter get tool
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
        Ok(CallToolResult::text(format!(
            "Counter value: {}",
            ctx.counter
        )))
    }
}

/// Counter increment tool
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
                "amount": {
                    "type": "integer",
                    "description": "Amount to increment by (default: 1)"
                }
            }
        })
    }
    fn execute(&self, args: Value, ctx: &mut AppContext, env: &ToolEnv) -> Result<CallToolResult> {
        let amount = args.get("amount").and_then(|a| a.as_i64()).unwrap_or(1);
        ctx.counter += amount;

        env.log(
            LogLevel::Debug,
            format!("Counter incremented by {} to {}", amount, ctx.counter),
        )?;

        Ok(CallToolResult::text(format!(
            "Counter incremented to: {}",
            ctx.counter
        )))
    }
}

/// Counter reset tool  
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
        let old_value = ctx.counter;
        ctx.counter = 0;
        Ok(CallToolResult::text(format!(
            "Counter reset from {} to 0",
            old_value
        )))
    }
}

fn main() -> Result<()> {
    eprintln!("Starting simple MCP server...");

    let config = ServerConfig {
        name: "simple-example".to_string(),
        version: "1.0.0".to_string(),
        instructions: Some(
            "A simple example MCP server with echo and counter tools. \
             Use 'echo' to echo messages, and 'counter_*' tools to manage a counter."
                .to_string(),
        ),
    };

    let mut server = Server::new(config);

    // Add tools
    server.add_tool(EchoTool)?;
    server.add_tool(CounterGetTool)?;
    server.add_tool(CounterIncrementTool)?;
    server.add_tool(CounterResetTool)?;

    // Create context and start
    let context = AppContext::new();
    let transport = StdioTransport::new();

    eprintln!("Server ready, waiting for messages...");
    server.start(transport, context)?;

    eprintln!("Server shutting down.");
    Ok(())
}

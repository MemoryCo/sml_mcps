# sml_mcps

[![CI](https://github.com/pebblebed-tech/sml_mcps/actions/workflows/ci.yml/badge.svg)](https://github.com/pebblebed-tech/sml_mcps/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/pebblebed-tech/sml_mcps/graph/badge.svg)](https://codecov.io/gh/pebblebed-tech/sml_mcps)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**Small MCP Server** - A minimal, sync MCP server implementation. No tokio, no async, just works.

## Why?

The official `rmcp` SDK is async/tokio-based. That's fine for some use cases, but:

1. **Tokio is viral** - once you're async, everything wants to be async
2. **MCP is sequential** - request → response → request → response  
3. **53% test coverage** - rmcp is young and under-tested
4. **We want control** - our core crates are sync

sml_mcps gives us a clean, sync MCP server that we control.

## Features

```toml
[features]
default = ["schema"]
schema = ["dep:schemars"]     # JSON Schema generation for tools
http = ["dep:tiny_http"]       # Streamable HTTP transport (with SSE)
auth = ["dep:jsonwebtoken"]    # JWT validation for hosted
hosted = ["http", "auth"]      # Both HTTP and auth
```

## Usage (Stdio)

Define your context and tools, then wire them up:

```rust
use sml_mcps::{Server, ServerConfig, StdioTransport, Tool, ToolEnv, CallToolResult, Result, LogLevel};
use serde_json::Value;

// Your shared context
struct AppContext {
    counter: i64,
}

// Define a tool
struct IncrementTool;

impl Tool<AppContext> for IncrementTool {
    fn name(&self) -> &str { "increment" }
    fn description(&self) -> &str { "Increment the counter" }
    
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "amount": { "type": "integer", "description": "Amount to increment by" }
            }
        })
    }
    
    fn execute(&self, args: Value, ctx: &mut AppContext, env: &ToolEnv) -> Result<CallToolResult> {
        let amount = args.get("amount").and_then(|a| a.as_i64()).unwrap_or(1);
        ctx.counter += amount;
        
        // Send notification to client
        env.log(LogLevel::Info, format!("Counter is now {}", ctx.counter))?;
        
        Ok(CallToolResult::text(format!("Counter: {}", ctx.counter)))
    }
}

fn main() -> Result<()> {
    let config = ServerConfig {
        name: "my-server".to_string(),
        version: "1.0.0".to_string(),
        instructions: Some("A counter server".to_string()),
    };
    
    let mut server = Server::new(config);
    server.add_tool(IncrementTool)?;
    
    let context = AppContext { counter: 0 };
    let transport = StdioTransport::new();
    
    server.start(transport, context)
}
```

## HTTP Transport (Streamable HTTP with SSE)

With the `http` feature, you can serve over HTTP using the 2025-03-26 spec.

**Key feature**: When tools send notifications (via `env.log()` or `env.send_progress()`), 
the response is automatically formatted as SSE:

```
data: {"method":"notifications/message","params":{...},"jsonrpc":"2.0"}

data: {"id":1,"result":{...},"jsonrpc":"2.0"}

```

For requests without notifications, plain JSON is returned.

```rust
use sml_mcps::{Server, ServerConfig, HttpTransport};
use std::sync::{Arc, Mutex};

// In your HTTP handler:
let transport = Arc::new(Mutex::new(HttpTransport::new(request_body)));

server.process_one(transport.clone(), &mut context)?;

let mut t = transport.lock().unwrap();
if t.has_notifications() {
    // Tool sent notifications - return as SSE
    let sse_body = t.take_sse_response();
    // Set Content-Type: text/event-stream
} else {
    // No notifications - return plain JSON
    let json_body = t.take_response().unwrap_or_default();
    // Set Content-Type: application/json  
}
```

See `examples/http_server.rs` for a complete tiny_http example.

## JWT Authentication

With the `auth` feature, validate JWT tokens for hosted deployments:

```rust
use sml_mcps::auth::{JwtValidator, Claims};

// HS256 (symmetric)
let validator = JwtValidator::hs256(b"your-secret-key");

// RS256 (asymmetric)  
let validator = JwtValidator::rs256(&public_key_pem)?;

// Validate from Authorization header
let claims: Claims = validator.validate_header("Bearer eyJ...")?;

println!("User: {}", claims.user_id());
println!("Tenant: {}", claims.tenant_id());
```

See `examples/http_auth.rs` for a complete authenticated server.

## Tool Environment

During tool execution, `ToolEnv` provides:

```rust
// Send log notification
env.log(LogLevel::Info, "Processing...")?;

// Send progress update
env.send_progress("token", 0.5, Some(1.0))?;

// Access resources
let uris = env.list_resources();
let resource = env.get_resource("my://resource")?;
```

## Protocol Version

Implements MCP protocol version `2025-03-26` (Streamable HTTP).

## What's NOT Included

- **Client implementation** - this is a server SDK
- **Sampling/LLM callbacks** - not needed for tool servers  
- **Async anything** - by design

## License

MIT

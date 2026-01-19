//! HTTP server with JWT authentication and SSE responses
//!
//! Run with: cargo run --example http_auth --features hosted

use serde_json::Value;
use sml_mcps::{
    CallToolResult, HttpTransport, LogLevel, Result, Server, ServerConfig, Tool, ToolEnv,
    auth::{Claims, JwtValidator},
};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server as TinyServer};

// For demo: generate test tokens
use jsonwebtoken::{Algorithm, EncodingKey, Header as JwtHeader, encode};
use serde::Serialize;

const SECRET: &[u8] = b"super-secret-key-for-testing-only";

/// Context that includes auth info
struct AuthContext {
    user_id: String,
    tenant_id: String,
}

struct WhoamiTool;
impl Tool<AuthContext> for WhoamiTool {
    fn name(&self) -> &str {
        "whoami"
    }
    fn description(&self) -> &str {
        "Returns info about the authenticated user"
    }
    fn schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn execute(
        &self,
        _args: Value,
        ctx: &mut AuthContext,
        env: &ToolEnv,
    ) -> Result<CallToolResult> {
        env.log(
            LogLevel::Info,
            format!("User {} checking identity", ctx.user_id),
        )?;
        Ok(CallToolResult::text(format!(
            "You are user '{}' in tenant '{}'",
            ctx.user_id, ctx.tenant_id
        )))
    }
}

struct EchoTool;
impl Tool<AuthContext> for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echo back a message"
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "message": { "type": "string" } },
            "required": ["message"]
        })
    }
    fn execute(&self, args: Value, ctx: &mut AuthContext, env: &ToolEnv) -> Result<CallToolResult> {
        let msg = args
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)");
        env.log(
            LogLevel::Debug,
            format!("Echo from {}: {}", ctx.user_id, msg),
        )?;
        Ok(CallToolResult::text(format!("Echo: {}", msg)))
    }
}

fn generate_test_token(user_id: &str, tenant_id: &str) -> String {
    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        exp: u64,
        tenant_id: String,
        scope: String,
    }

    let claims = TestClaims {
        sub: user_id.to_string(),
        exp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600,
        tenant_id: tenant_id.to_string(),
        scope: "read write".to_string(),
    };

    encode(
        &JwtHeader::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET),
    )
    .unwrap()
}

fn main() {
    let addr = "127.0.0.1:3001";
    eprintln!(
        "Starting authenticated HTTP MCP server on http://{}/mcp",
        addr
    );

    let test_token = generate_test_token("user-123", "tenant-456");
    eprintln!("\n=== TEST TOKEN (valid for 1 hour) ===");
    eprintln!("{}", test_token);
    eprintln!("\nTest with:");
    eprintln!("curl -X POST http://{}/mcp \\", addr);
    eprintln!("  -H \"Content-Type: application/json\" \\");
    eprintln!("  -H \"Authorization: Bearer {}\" \\", test_token);
    eprintln!(
        "  -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"whoami\"}}}}'"
    );
    eprintln!("=====================================\n");

    let http_server = TinyServer::http(addr).expect("Failed to start HTTP server");
    let jwt_validator = JwtValidator::hs256(SECRET);

    eprintln!("Server ready, waiting for authenticated requests...\n");

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

        // JWT Authentication
        let auth_header = request
            .headers()
            .iter()
            .find(|h| h.field.as_str() == "Authorization" || h.field.as_str() == "authorization")
            .map(|h| h.value.as_str());

        let claims: Claims = match auth_header {
            Some(header) => match jwt_validator.validate_header(header) {
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
                    let response =
                        Response::from_string(format!("Unauthorized: {}", e)).with_status_code(401);
                    let _ = request.respond(response);
                    continue;
                }
            },
            None => {
                eprintln!("  ✗ No Authorization header");
                let response = Response::from_string("Unauthorized: Missing Authorization header")
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

        // Create context from auth
        let mut ctx = AuthContext {
            user_id: claims.user_id().to_string(),
            tenant_id: claims.tenant_id().to_string(),
        };

        // Create server
        let config = ServerConfig {
            name: "authenticated-http".to_string(),
            version: "1.0.0".to_string(),
            instructions: Some(format!(
                "Authenticated MCP server. User: {}, Tenant: {}",
                ctx.user_id, ctx.tenant_id
            )),
        };

        let mut server: Server<AuthContext> = Server::new(config);
        server.add_tool(WhoamiTool).unwrap();
        server.add_tool(EchoTool).unwrap();

        let transport = Arc::new(Mutex::new(HttpTransport::new(body)));

        if let Err(e) = server.process_one(transport.clone(), &mut ctx) {
            eprintln!("  Error: {}", e);
            let response =
                Response::from_string(format!("Internal Error: {}", e)).with_status_code(500);
            let _ = request.respond(response);
            continue;
        }

        // Extract response
        let mut transport_guard = transport.lock().unwrap();

        let (response_body, content_type) = if transport_guard.has_notifications() {
            let sse = transport_guard.take_sse_response();
            eprintln!("  Response (SSE):\n{}", sse);
            (sse, "text/event-stream")
        } else {
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

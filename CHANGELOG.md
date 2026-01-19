# Changelog

All notable changes to this project will be documented in this file.

## v0.1.0

Initial release.

### Features

- Sync MCP server implementation (no tokio/async)
- `Tool<C>` trait generic over context type (sovran-mcp style API)
- `ToolEnv` for notifications, progress reporting, and resource access
- Stdio transport for Claude Desktop
- HTTP transport with automatic SSE/JSON response switching (MCP 2025-03-26 spec)
- JWT authentication support for hosted deployments
- Full MCP protocol support: tools, resources, prompts
- 93% test coverage

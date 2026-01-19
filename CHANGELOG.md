## v0.2.0

### Changes

- Initial commit (b8427e9)
- sml_mcps: Sync MCP server with sovran-mcp style API, SSE support, 93% coverage (28e26e8)
- Add MIT license, GitHub Actions CI, and codecov integration (879cc96)
- updated readme. (c280e49)
- Fix: dtolnay/rust-toolchain action name (657cd5b)
- Fix clippy warnings and format, soften codecov failure (a7b59af)
- fixed format (7ac4589)
- Add release workflow and changelog (5121c4e)
- Fix edition and add crates.io metadata (66fde8e)
- Fix edition to 2024, shorten keywords (0b762bb)
- Fix release workflow YAML syntax (432ca75)

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

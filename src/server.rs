//! MCP Server
//!
//! Core server implementation with generic context support.

use crate::pagination::{DEFAULT_PAGE_SIZE, PageState, paginate};
use crate::transport::Transport;
use crate::types::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

//
// Tool Environment - passed to tools during execution
//

/// Environment provided to tools during execution
///
/// Gives tools access to notifications, progress reporting, and resources.
pub struct ToolEnv<'a> {
    transport: &'a Arc<Mutex<dyn Transport>>,
    resources: &'a HashMap<String, Box<dyn Resource>>,
}

impl<'a> ToolEnv<'a> {
    /// Send a notification to the client
    pub fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notification = JsonRpcMessage::notification(method, params);
        let mut transport = self
            .transport
            .lock()
            .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;
        transport.write(&notification)
    }

    /// Send a log message notification
    pub fn log(&self, level: LogLevel, message: impl Into<String>) -> Result<()> {
        self.send_notification(
            "notifications/message",
            Some(serde_json::json!({
                "level": level.as_str(),
                "data": message.into()
            })),
        )
    }

    /// Send progress update for long-running operations
    pub fn send_progress(&self, token: &str, progress: f64, total: Option<f64>) -> Result<()> {
        let mut params = serde_json::json!({
            "progressToken": token,
            "progress": progress
        });
        if let Some(t) = total {
            params["total"] = serde_json::json!(t);
        }
        self.send_notification("notifications/progress", Some(params))
    }

    /// List all resource URIs
    pub fn list_resources(&self) -> Vec<String> {
        self.resources.keys().cloned().collect()
    }

    /// Get a resource by URI
    pub fn get_resource(&self, uri: &str) -> Option<&dyn Resource> {
        self.resources.get(uri).map(|r| r.as_ref())
    }
}

/// Log levels for notifications
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }
}

//
// Tool trait
//

/// Tool implementation trait
///
/// Generic over context type `C` for shared state between tools.
pub trait Tool<C>: Send + Sync {
    /// Tool name (must be unique)
    fn name(&self) -> &str;

    /// Human-readable description
    fn description(&self) -> &str;

    /// JSON Schema for input arguments
    fn schema(&self) -> Value;

    /// Tool behavior annotations (hints for clients)
    ///
    /// Override this to provide hints about tool behavior:
    /// - `read_only_hint`: Tool only reads, never modifies
    /// - `idempotent_hint`: Safe to retry (same result on repeat calls)
    /// - `destructive_hint`: May overwrite or heavily mutate data
    ///
    /// Default returns None (no hints).
    fn annotations(&self) -> Option<ToolAnnotations> {
        None
    }

    /// Execute the tool
    fn execute(&self, args: Value, context: &mut C, env: &ToolEnv) -> Result<CallToolResult>;
}

//
// Resource trait
//

/// Resource implementation trait
pub trait Resource: Send + Sync {
    /// Resource URI (must be unique)
    fn uri(&self) -> String;

    /// Human-readable name
    fn name(&self) -> String;

    /// Description
    fn description(&self) -> String;

    /// MIME type
    fn mime_type(&self) -> String;

    /// Get resource content
    fn content(&self) -> Vec<ResourceContent>;

    /// Convert to protocol Resource type
    fn as_protocol_resource(&self) -> crate::types::Resource {
        crate::types::Resource {
            uri: self.uri(),
            name: self.name(),
            description: Some(self.description()),
            mime_type: Some(self.mime_type()),
        }
    }
}

//
// Prompt trait
//

/// Prompt implementation trait
pub trait PromptDef: Send + Sync {
    /// Prompt name (must be unique)
    fn name(&self) -> &str;

    /// Human-readable description
    fn description(&self) -> Option<&str>;

    /// Argument definitions
    fn arguments(&self) -> Vec<PromptArgument>;

    /// Generate prompt messages
    fn get_messages(&self, args: &HashMap<String, String>) -> Result<Vec<PromptMessage>>;

    /// Convert to protocol Prompt type
    fn as_protocol_prompt(&self) -> Prompt {
        Prompt {
            name: self.name().to_string(),
            description: self.description().map(String::from),
            arguments: self.arguments(),
        }
    }
}

//
// Server configuration
//

/// Server configuration
#[derive(Clone)]
pub struct ServerConfig {
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
    /// Page size for list operations (tools, resources, prompts)
    pub page_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: "sml_mcps".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            instructions: None,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

//
// MCP Server
//

/// MCP Server - generic over context type
pub struct Server<C> {
    config: ServerConfig,
    tools: HashMap<String, Box<dyn Tool<C>>>,
    resources: HashMap<String, Box<dyn Resource>>,
    prompts: HashMap<String, Box<dyn PromptDef>>,
    transport: Option<Arc<Mutex<dyn Transport>>>,
    initialized: bool,
}

impl<C: Send + Sync + 'static> Server<C> {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            transport: None,
            initialized: false,
        }
    }

    /// Add a tool to the server
    pub fn add_tool(&mut self, tool: impl Tool<C> + 'static) -> Result<()> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(McpError::Internal(format!("Duplicate tool: {}", name)));
        }
        self.tools.insert(name, Box::new(tool));
        Ok(())
    }

    /// Add a resource to the server
    pub fn add_resource(&mut self, resource: impl Resource + 'static) -> Result<()> {
        let uri = resource.uri();
        if self.resources.contains_key(&uri) {
            return Err(McpError::Internal(format!("Duplicate resource: {}", uri)));
        }
        self.resources.insert(uri, Box::new(resource));
        Ok(())
    }

    /// Add a prompt to the server
    pub fn add_prompt(&mut self, prompt: impl PromptDef + 'static) -> Result<()> {
        let name = prompt.name().to_string();
        if self.prompts.contains_key(&name) {
            return Err(McpError::Internal(format!("Duplicate prompt: {}", name)));
        }
        self.prompts.insert(name, Box::new(prompt));
        Ok(())
    }

    /// Run the server with the given transport and context (for stdio - continuous loop)
    pub fn start<T: Transport + 'static>(&mut self, transport: T, mut context: C) -> Result<()> {
        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(transport));
        self.transport = Some(transport.clone());

        eprintln!(
            "MCP Server `{}` started, version {}",
            self.config.name, self.config.version
        );

        loop {
            // Read message
            let message = {
                let mut t = transport
                    .lock()
                    .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;
                match t.read() {
                    Ok(msg) => msg,
                    Err(McpError::TransportClosed) => break,
                    Err(e) => return Err(e),
                }
            };

            // Handle message
            if let Some(response) = self.handle_message(message, &mut context)? {
                let mut t = transport
                    .lock()
                    .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;
                t.write(&response)?;
            }
        }

        Ok(())
    }

    /// Process a single request (for HTTP - single request/response)
    ///
    /// Pass an Arc<Mutex<T>> so that:
    /// 1. ToolEnv can write notifications to the transport during execution
    /// 2. You can extract buffered messages after processing
    ///
    /// For HttpTransport, use `transport.take_sse_response()` after this call.
    pub fn process_one<T: Transport + 'static>(
        &mut self,
        transport: Arc<Mutex<T>>,
        context: &mut C,
    ) -> Result<()> {
        // Store as dyn Transport for ToolEnv
        self.transport = Some(transport.clone() as Arc<Mutex<dyn Transport>>);

        // Read message
        let message = {
            let mut t = transport
                .lock()
                .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;
            t.read()?
        };

        // Handle and write response
        if let Some(response) = self.handle_message(message, context)? {
            let mut t = transport
                .lock()
                .map_err(|_| McpError::Internal("Transport lock poisoned".into()))?;
            t.write(&response)?;
        }

        Ok(())
    }

    /// Handle a single message
    fn handle_message(
        &mut self,
        message: JsonRpcMessage,
        context: &mut C,
    ) -> Result<Option<JsonRpcMessage>> {
        match message {
            JsonRpcMessage::Request(request) => {
                let response = self.handle_request(request, context);
                Ok(Some(response))
            }
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(notification)?;
                Ok(None)
            }
            JsonRpcMessage::Response(_) => Ok(None),
        }
    }

    /// Handle a request and return a response
    fn handle_request(&mut self, request: JsonRpcRequest, context: &mut C) -> JsonRpcMessage {
        let id = request.id.clone();

        match self.dispatch_request(&request, context) {
            Ok(result) => JsonRpcMessage::response(id, result),
            Err(e) => JsonRpcMessage::error(id, e.to_jsonrpc_error()),
        }
    }

    /// Dispatch a request to the appropriate handler
    fn dispatch_request(&mut self, request: &JsonRpcRequest, context: &mut C) -> Result<Value> {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request),
            "ping" => self.handle_ping(),
            "tools/list" => self.handle_list_tools(request),
            "tools/call" => self.handle_call_tool(request, context),
            "resources/list" => self.handle_list_resources(request),
            "resources/read" => self.handle_read_resource(request),
            "prompts/list" => self.handle_list_prompts(request),
            "prompts/get" => self.handle_get_prompt(request),
            method => Err(McpError::MethodNotFound(method.to_string())),
        }
    }

    fn handle_initialize(&mut self, request: &JsonRpcRequest) -> Result<Value> {
        let _params: InitializeParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => InitializeParams::default(),
        };

        self.initialized = true;

        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: if self.tools.is_empty() {
                    None
                } else {
                    Some(ToolsCapability::default())
                },
                resources: if self.resources.is_empty() {
                    None
                } else {
                    Some(ResourcesCapability::default())
                },
                prompts: if self.prompts.is_empty() {
                    None
                } else {
                    Some(PromptsCapability::default())
                },
                logging: None,
                experimental: None,
            },
            server_info: Implementation {
                name: self.config.name.clone(),
                version: self.config.version.clone(),
            },
            instructions: self.config.instructions.clone(),
        };

        Ok(serde_json::to_value(result)?)
    }

    fn handle_ping(&self) -> Result<Value> {
        Ok(serde_json::to_value(PingResult {})?)
    }

    fn handle_list_tools(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params: ListToolsParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => ListToolsParams::default(),
        };

        // Collect all tools (sorted for consistent pagination)
        let mut all_tools: Vec<crate::types::Tool> = self
            .tools
            .values()
            .map(|t| crate::types::Tool {
                name: t.name().to_string(),
                description: Some(t.description().to_string()),
                input_schema: t.schema(),
                annotations: t.annotations(),
            })
            .collect();
        all_tools.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply pagination
        let state = PageState::from_cursor(params.cursor.as_deref(), self.config.page_size);
        let (tools, next_cursor) = paginate(&all_tools, &state);

        Ok(serde_json::to_value(ListToolsResult {
            tools,
            next_cursor,
        })?)
    }

    fn handle_call_tool(&mut self, request: &JsonRpcRequest, context: &mut C) -> Result<Value> {
        let params: CallToolParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => return Err(McpError::InvalidParams("Missing params".into())),
        };

        let tool = self
            .tools
            .get(&params.name)
            .ok_or_else(|| McpError::ToolError(format!("Unknown tool: {}", params.name)))?;

        let env = ToolEnv {
            transport: self.transport.as_ref().unwrap(),
            resources: &self.resources,
        };

        let result = tool.execute(
            params.arguments.unwrap_or(serde_json::json!({})),
            context,
            &env,
        )?;

        Ok(serde_json::to_value(result)?)
    }

    fn handle_list_resources(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params: ListResourcesParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => ListResourcesParams::default(),
        };

        // Collect all resources (sorted for consistent pagination)
        let mut all_resources: Vec<crate::types::Resource> = self
            .resources
            .values()
            .map(|r| r.as_protocol_resource())
            .collect();
        all_resources.sort_by(|a, b| a.uri.cmp(&b.uri));

        // Apply pagination
        let state = PageState::from_cursor(params.cursor.as_deref(), self.config.page_size);
        let (resources, next_cursor) = paginate(&all_resources, &state);

        Ok(serde_json::to_value(ListResourcesResult {
            resources,
            next_cursor,
        })?)
    }

    fn handle_read_resource(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params: ReadResourceParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => return Err(McpError::InvalidParams("Missing params".into())),
        };

        let resource = self
            .resources
            .get(&params.uri)
            .ok_or_else(|| McpError::ResourceNotFound(params.uri.clone()))?;

        let contents = resource.content();
        Ok(serde_json::to_value(ReadResourceResult { contents })?)
    }

    fn handle_list_prompts(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params: ListPromptsParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => ListPromptsParams::default(),
        };

        // Collect all prompts (sorted for consistent pagination)
        let mut all_prompts: Vec<Prompt> = self
            .prompts
            .values()
            .map(|p| p.as_protocol_prompt())
            .collect();
        all_prompts.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply pagination
        let state = PageState::from_cursor(params.cursor.as_deref(), self.config.page_size);
        let (prompts, next_cursor) = paginate(&all_prompts, &state);

        Ok(serde_json::to_value(ListPromptsResult {
            prompts,
            next_cursor,
        })?)
    }

    fn handle_get_prompt(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params: GetPromptParams = match &request.params {
            Some(p) => serde_json::from_value(p.clone())?,
            None => return Err(McpError::InvalidParams("Missing params".into())),
        };

        let prompt = self
            .prompts
            .get(&params.name)
            .ok_or_else(|| McpError::PromptNotFound(params.name.clone()))?;

        let messages = prompt.get_messages(&params.arguments)?;
        let result = GetPromptResult {
            description: prompt.description().map(String::from),
            messages,
        };

        Ok(serde_json::to_value(result)?)
    }

    fn handle_notification(&mut self, notification: JsonRpcNotification) -> Result<()> {
        match notification.method.as_str() {
            "notifications/initialized" => Ok(()),
            "notifications/cancelled" => Ok(()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport;

    // Test context
    struct TestContext {
        counter: i32,
    }

    // Test tool
    struct IncrementTool;

    impl Tool<TestContext> for IncrementTool {
        fn name(&self) -> &str {
            "increment"
        }
        fn description(&self) -> &str {
            "Increment the counter"
        }
        fn schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "integer" }
                }
            })
        }
        fn execute(
            &self,
            args: Value,
            ctx: &mut TestContext,
            _env: &ToolEnv,
        ) -> Result<CallToolResult> {
            let amount = args.get("amount").and_then(|a| a.as_i64()).unwrap_or(1) as i32;
            ctx.counter += amount;
            Ok(CallToolResult::text(format!(
                "Counter is now: {}",
                ctx.counter
            )))
        }
    }

    // Tool that uses env to send notifications
    struct NotifyTool;

    impl Tool<TestContext> for NotifyTool {
        fn name(&self) -> &str {
            "notify"
        }
        fn description(&self) -> &str {
            "Send a notification"
        }
        fn schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _args: Value,
            _ctx: &mut TestContext,
            env: &ToolEnv,
        ) -> Result<CallToolResult> {
            env.log(LogLevel::Info, "test notification")?;
            env.send_progress("token", 0.5, Some(1.0))?;
            Ok(CallToolResult::text("done"))
        }
    }

    // Tool that fails
    struct FailingTool;

    impl Tool<TestContext> for FailingTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _args: Value,
            _ctx: &mut TestContext,
            _env: &ToolEnv,
        ) -> Result<CallToolResult> {
            Err(McpError::ToolError("intentional failure".into()))
        }
    }

    // Test resource
    struct TestResource {
        uri: String,
        data: String,
    }

    impl Resource for TestResource {
        fn uri(&self) -> String {
            self.uri.clone()
        }
        fn name(&self) -> String {
            "test-resource".into()
        }
        fn description(&self) -> String {
            "A test resource".into()
        }
        fn mime_type(&self) -> String {
            "text/plain".into()
        }
        fn content(&self) -> Vec<ResourceContent> {
            vec![ResourceContent::Text {
                uri: self.uri.clone(),
                text: self.data.clone(),
                mime_type: Some("text/plain".into()),
            }]
        }
    }

    // Test prompt
    struct TestPrompt;

    impl PromptDef for TestPrompt {
        fn name(&self) -> &str {
            "test-prompt"
        }
        fn description(&self) -> Option<&str> {
            Some("A test prompt")
        }
        fn arguments(&self) -> Vec<PromptArgument> {
            vec![PromptArgument {
                name: "name".into(),
                description: Some("Your name".into()),
                required: true,
            }]
        }
        fn get_messages(&self, args: &HashMap<String, String>) -> Result<Vec<PromptMessage>> {
            let name = args.get("name").cloned().unwrap_or_else(|| "world".into());
            Ok(vec![PromptMessage {
                role: Role::User,
                content: Content::text(format!("Hello, {}!", name)),
            }])
        }
    }

    // Mock transport for testing
    struct MockTransport {
        messages: Vec<JsonRpcMessage>,
        responses: Vec<JsonRpcMessage>,
        read_index: usize,
    }

    impl MockTransport {
        fn new(messages: Vec<JsonRpcMessage>) -> Self {
            Self {
                messages,
                responses: Vec::new(),
                read_index: 0,
            }
        }

        fn get_responses(&self) -> &[JsonRpcMessage] {
            &self.responses
        }
    }

    impl Transport for MockTransport {
        fn read(&mut self) -> Result<JsonRpcMessage> {
            if self.read_index >= self.messages.len() {
                return Err(McpError::TransportClosed);
            }
            let msg = self.messages[self.read_index].clone();
            self.read_index += 1;
            Ok(msg)
        }

        fn write(&mut self, message: &JsonRpcMessage) -> Result<()> {
            self.responses.push(message.clone());
            Ok(())
        }

        fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    // Helper to create a request message
    fn make_request(id: i64, method: &str, params: Option<Value>) -> JsonRpcMessage {
        JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(id),
            method: method.to_string(),
            params,
        })
    }

    // Helper to create a notification message
    fn make_notification(method: &str, params: Option<Value>) -> JsonRpcMessage {
        JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: Default::default(),
            method: method.to_string(),
            params,
        })
    }

    #[test]
    fn test_add_tool() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();
        assert!(server.tools.contains_key("increment"));
    }

    #[test]
    fn test_duplicate_tool_error() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();
        let result = server.add_tool(IncrementTool);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_resource() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let resource = TestResource {
            uri: "test://data".into(),
            data: "hello".into(),
        };
        server.add_resource(resource).unwrap();
        assert!(server.resources.contains_key("test://data"));
    }

    #[test]
    fn test_duplicate_resource_error() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let r1 = TestResource {
            uri: "test://data".into(),
            data: "hello".into(),
        };
        let r2 = TestResource {
            uri: "test://data".into(),
            data: "world".into(),
        };
        server.add_resource(r1).unwrap();
        let result = server.add_resource(r2);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_prompt() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_prompt(TestPrompt).unwrap();
        assert!(server.prompts.contains_key("test-prompt"));
    }

    #[test]
    fn test_duplicate_prompt_error() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_prompt(TestPrompt).unwrap();
        let result = server.add_prompt(TestPrompt);
        assert!(result.is_err());
    }

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.name, "sml_mcps");
        assert!(config.instructions.is_none());
    }

    #[test]
    fn test_log_level_as_str() {
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warning.as_str(), "warning");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    #[test]
    fn test_handle_initialize() {
        let mut server: Server<TestContext> = Server::new(ServerConfig {
            name: "test-server".into(),
            version: "1.0.0".into(),
            instructions: Some("test instructions".into()),
            ..Default::default()
        });
        server.add_tool(IncrementTool).unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        };

        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(result.is_ok());

        let value = result.unwrap();
        assert_eq!(value["serverInfo"]["name"], "test-server");
        assert!(server.initialized);
    }

    #[test]
    fn test_handle_initialize_no_params() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "initialize".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_ping() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "ping".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_tools_list() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/list".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "increment");
    }

    #[test]
    fn test_handle_tools_call() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();

        // Need to set up transport for ToolEnv
        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        server.transport = Some(transport);

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "increment",
                "arguments": { "amount": 5 }
            })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        assert_eq!(ctx.counter, 5);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("5"));
    }

    #[test]
    fn test_handle_tools_call_no_params() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/call".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_tools_call_unknown_tool() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        server.transport = Some(transport);

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({ "name": "nonexistent" })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::ToolError(_))));
    }

    #[test]
    fn test_handle_tools_call_tool_failure() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(FailingTool).unwrap();
        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        server.transport = Some(transport);

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({ "name": "fail" })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::ToolError(_))));
    }

    #[test]
    fn test_handle_resources_list() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server
            .add_resource(TestResource {
                uri: "test://data".into(),
                data: "hello".into(),
            })
            .unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "resources/list".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        let resources = result["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], "test://data");
    }

    #[test]
    fn test_handle_resources_read() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server
            .add_resource(TestResource {
                uri: "test://data".into(),
                data: "hello world".into(),
            })
            .unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "resources/read".to_string(),
            params: Some(serde_json::json!({ "uri": "test://data" })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        let contents = result["contents"].as_array().unwrap();
        assert_eq!(contents[0]["text"], "hello world");
    }

    #[test]
    fn test_handle_resources_read_not_found() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "resources/read".to_string(),
            params: Some(serde_json::json!({ "uri": "nonexistent://uri" })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::ResourceNotFound(_))));
    }

    #[test]
    fn test_handle_resources_read_no_params() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "resources/read".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::InvalidParams(_))));
    }

    #[test]
    fn test_handle_prompts_list() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_prompt(TestPrompt).unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "prompts/list".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        let prompts = result["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["name"], "test-prompt");
    }

    #[test]
    fn test_handle_prompts_get() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_prompt(TestPrompt).unwrap();

        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "prompts/get".to_string(),
            params: Some(serde_json::json!({
                "name": "test-prompt",
                "arguments": { "name": "Claude" }
            })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx).unwrap();

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("Claude")
        );
    }

    #[test]
    fn test_handle_prompts_get_not_found() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "prompts/get".to_string(),
            params: Some(serde_json::json!({ "name": "nonexistent", "arguments": {} })),
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::PromptNotFound(_))));
    }

    #[test]
    fn test_handle_prompts_get_no_params() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "prompts/get".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::InvalidParams(_))));
    }

    #[test]
    fn test_handle_unknown_method() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "unknown/method".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = server.dispatch_request(&request, &mut ctx);
        assert!(matches!(result, Err(McpError::MethodNotFound(_))));
    }

    #[test]
    fn test_handle_notification() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());

        let init_notification = JsonRpcNotification {
            jsonrpc: Default::default(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let result = server.handle_notification(init_notification);
        assert!(result.is_ok());

        let cancel_notification = JsonRpcNotification {
            jsonrpc: Default::default(),
            method: "notifications/cancelled".to_string(),
            params: Some(serde_json::json!({ "requestId": 1 })),
        };
        let result = server.handle_notification(cancel_notification);
        assert!(result.is_ok());

        let unknown_notification = JsonRpcNotification {
            jsonrpc: Default::default(),
            method: "unknown/notification".to_string(),
            params: None,
        };
        let result = server.handle_notification(unknown_notification);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_message_request() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        server.transport = Some(transport);

        let message = make_request(1, "ping", None);
        let mut ctx = TestContext { counter: 0 };
        let result = server.handle_message(message, &mut ctx).unwrap();

        assert!(result.is_some());
        if let Some(JsonRpcMessage::Response(resp)) = result {
            assert!(resp.result.is_some());
        } else {
            panic!("Expected response");
        }
    }

    #[test]
    fn test_handle_message_notification() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let message = make_notification("notifications/initialized", None);
        let mut ctx = TestContext { counter: 0 };
        let result = server.handle_message(message, &mut ctx).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_message_response() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let message = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            result: Some(serde_json::json!({})),
            error: None,
        });
        let mut ctx = TestContext { counter: 0 };
        let result = server.handle_message(message, &mut ctx).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_request_error_response() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(99),
            method: "unknown".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let response = server.handle_request(request, &mut ctx);

        if let JsonRpcMessage::Response(resp) = response {
            assert!(resp.error.is_some());
            assert_eq!(resp.id, RequestId::Number(99));
        } else {
            panic!("Expected response");
        }
    }

    #[test]
    fn test_process_one() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();

        let messages = vec![make_request(
            1,
            "tools/call",
            Some(serde_json::json!({
                "name": "increment",
                "arguments": { "amount": 10 }
            })),
        )];

        let transport = Arc::new(Mutex::new(MockTransport::new(messages)));
        let mut ctx = TestContext { counter: 0 };

        server.process_one(transport.clone(), &mut ctx).unwrap();

        assert_eq!(ctx.counter, 10);

        let t = transport.lock().unwrap();
        let responses = t.get_responses();
        assert_eq!(responses.len(), 1);
    }

    #[test]
    fn test_start_exits_on_transport_closed() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());

        // Empty transport will return TransportClosed immediately
        let transport = MockTransport::new(vec![]);
        let ctx = TestContext { counter: 0 };

        let result = server.start(transport, ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_start_processes_messages() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(IncrementTool).unwrap();

        let messages = vec![
            make_request(
                1,
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1.0" }
                })),
            ),
            make_request(
                2,
                "tools/call",
                Some(serde_json::json!({
                    "name": "increment",
                    "arguments": { "amount": 7 }
                })),
            ),
        ];

        let transport = MockTransport::new(messages);
        let ctx = TestContext { counter: 0 };

        // start will process messages until transport closes
        let result = server.start(transport, ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_env_list_resources() {
        let mut resources: HashMap<String, Box<dyn Resource>> = HashMap::new();
        resources.insert(
            "test://a".into(),
            Box::new(TestResource {
                uri: "test://a".into(),
                data: "a".into(),
            }),
        );
        resources.insert(
            "test://b".into(),
            Box::new(TestResource {
                uri: "test://b".into(),
                data: "b".into(),
            }),
        );

        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        let env = ToolEnv {
            transport: &transport,
            resources: &resources,
        };

        let uris = env.list_resources();
        assert_eq!(uris.len(), 2);
        assert!(uris.contains(&"test://a".to_string()));
        assert!(uris.contains(&"test://b".to_string()));
    }

    #[test]
    fn test_tool_env_get_resource() {
        let mut resources: HashMap<String, Box<dyn Resource>> = HashMap::new();
        resources.insert(
            "test://data".into(),
            Box::new(TestResource {
                uri: "test://data".into(),
                data: "hello".into(),
            }),
        );

        let transport: Arc<Mutex<dyn Transport>> = Arc::new(Mutex::new(MockTransport::new(vec![])));
        let env = ToolEnv {
            transport: &transport,
            resources: &resources,
        };

        let found = env.get_resource("test://data");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "test-resource");

        let not_found = env.get_resource("nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_tool_with_notifications() {
        let mut server: Server<TestContext> = Server::new(ServerConfig::default());
        server.add_tool(NotifyTool).unwrap();

        let messages = vec![make_request(
            1,
            "tools/call",
            Some(serde_json::json!({ "name": "notify" })),
        )];

        let transport = Arc::new(Mutex::new(MockTransport::new(messages)));
        let mut ctx = TestContext { counter: 0 };

        server.process_one(transport.clone(), &mut ctx).unwrap();

        let t = transport.lock().unwrap();
        let responses = t.get_responses();
        // Should have: notification, progress, response
        assert!(responses.len() >= 2);
    }

    #[test]
    fn test_resource_as_protocol_resource() {
        let resource = TestResource {
            uri: "test://uri".into(),
            data: "data".into(),
        };
        let proto = resource.as_protocol_resource();

        assert_eq!(proto.uri, "test://uri");
        assert_eq!(proto.name, "test-resource");
        assert_eq!(proto.description, Some("A test resource".into()));
        assert_eq!(proto.mime_type, Some("text/plain".into()));
    }

    #[test]
    fn test_prompt_as_protocol_prompt() {
        let prompt = TestPrompt;
        let proto = prompt.as_protocol_prompt();

        assert_eq!(proto.name, "test-prompt");
        assert_eq!(proto.description, Some("A test prompt".into()));
        assert_eq!(proto.arguments.len(), 1);
        assert_eq!(proto.arguments[0].name, "name");
    }

    #[test]
    fn test_capabilities_reflect_registered_items() {
        // Server with nothing
        let mut empty_server: Server<TestContext> = Server::new(ServerConfig::default());
        let req = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "initialize".to_string(),
            params: None,
        };
        let mut ctx = TestContext { counter: 0 };
        let result = empty_server.dispatch_request(&req, &mut ctx).unwrap();
        assert!(result["capabilities"]["tools"].is_null());
        assert!(result["capabilities"]["resources"].is_null());
        assert!(result["capabilities"]["prompts"].is_null());

        // Server with everything
        let mut full_server: Server<TestContext> = Server::new(ServerConfig::default());
        full_server.add_tool(IncrementTool).unwrap();
        full_server
            .add_resource(TestResource {
                uri: "test://x".into(),
                data: "x".into(),
            })
            .unwrap();
        full_server.add_prompt(TestPrompt).unwrap();

        let result = full_server.dispatch_request(&req, &mut ctx).unwrap();
        assert!(!result["capabilities"]["tools"].is_null());
        assert!(!result["capabilities"]["resources"].is_null());
        assert!(!result["capabilities"]["prompts"].is_null());
    }

    // Helper tool for pagination tests - creates many tools
    struct NumberedTool(u32);

    impl Tool<TestContext> for NumberedTool {
        fn name(&self) -> &str {
            // This is a bit hacky but works for tests
            Box::leak(format!("tool_{:03}", self.0).into_boxed_str())
        }
        fn description(&self) -> &str {
            "A numbered tool"
        }
        fn schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _args: Value,
            _ctx: &mut TestContext,
            _env: &ToolEnv,
        ) -> Result<CallToolResult> {
            Ok(CallToolResult::text(format!("Tool {}", self.0)))
        }
    }

    #[test]
    fn test_pagination_tools_list() {
        // Create server with small page size
        let mut server: Server<TestContext> = Server::new(ServerConfig {
            page_size: 3,
            ..Default::default()
        });

        // Add 10 tools
        for i in 0..10 {
            server.add_tool(NumberedTool(i)).unwrap();
        }

        let mut ctx = TestContext { counter: 0 };

        // First page - no cursor
        let req = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/list".to_string(),
            params: None,
        };
        let result = server.dispatch_request(&req, &mut ctx).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["name"], "tool_000");
        assert_eq!(tools[2]["name"], "tool_002");
        let next_cursor = result["nextCursor"].as_str().unwrap();

        // Second page - with cursor
        let req2 = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(2),
            method: "tools/list".to_string(),
            params: Some(serde_json::json!({ "cursor": next_cursor })),
        };
        let result2 = server.dispatch_request(&req2, &mut ctx).unwrap();
        let tools2 = result2["tools"].as_array().unwrap();
        assert_eq!(tools2.len(), 3);
        assert_eq!(tools2[0]["name"], "tool_003");
        assert_eq!(tools2[2]["name"], "tool_005");
        let next_cursor2 = result2["nextCursor"].as_str().unwrap();

        // Third page
        let req3 = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(3),
            method: "tools/list".to_string(),
            params: Some(serde_json::json!({ "cursor": next_cursor2 })),
        };
        let result3 = server.dispatch_request(&req3, &mut ctx).unwrap();
        let tools3 = result3["tools"].as_array().unwrap();
        assert_eq!(tools3.len(), 3);
        assert_eq!(tools3[0]["name"], "tool_006");
        let next_cursor3 = result3["nextCursor"].as_str().unwrap();

        // Fourth (last) page - only 1 item left
        let req4 = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(4),
            method: "tools/list".to_string(),
            params: Some(serde_json::json!({ "cursor": next_cursor3 })),
        };
        let result4 = server.dispatch_request(&req4, &mut ctx).unwrap();
        let tools4 = result4["tools"].as_array().unwrap();
        assert_eq!(tools4.len(), 1);
        assert_eq!(tools4[0]["name"], "tool_009");
        assert!(result4["nextCursor"].is_null()); // No more pages
    }

    #[test]
    fn test_pagination_invalid_cursor() {
        let mut server: Server<TestContext> = Server::new(ServerConfig {
            page_size: 3,
            ..Default::default()
        });
        server.add_tool(IncrementTool).unwrap();

        let mut ctx = TestContext { counter: 0 };

        // Invalid cursor should default to first page
        let req = JsonRpcRequest {
            jsonrpc: Default::default(),
            id: RequestId::Number(1),
            method: "tools/list".to_string(),
            params: Some(serde_json::json!({ "cursor": "garbage" })),
        };
        let result = server.dispatch_request(&req, &mut ctx).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1); // Should get the one tool
    }
}

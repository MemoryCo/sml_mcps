//! Error Types

use crate::types::JsonRpcError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum McpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Transport closed")]
    TransportClosed,

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid params: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Tool error: {0}")]
    ToolError(String),

    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    #[error("Prompt not found: {0}")]
    PromptNotFound(String),

    #[cfg(feature = "auth")]
    #[error("Auth error: {0}")]
    Auth(String),
}

impl McpError {
    pub fn to_jsonrpc_error(&self) -> JsonRpcError {
        match self {
            McpError::Json(e) => JsonRpcError::parse_error(e.to_string()),
            McpError::InvalidMessage(msg) => JsonRpcError::invalid_request(msg),
            McpError::MethodNotFound(method) => {
                JsonRpcError::method_not_found(format!("Method not found: {}", method))
            }
            McpError::InvalidParams(msg) => JsonRpcError::invalid_params(msg),
            McpError::Internal(msg) => JsonRpcError::internal_error(msg),
            McpError::ToolError(msg) => JsonRpcError::new(-32000, msg),
            McpError::ResourceNotFound(uri) => {
                JsonRpcError::new(-32001, format!("Resource not found: {}", uri))
            }
            McpError::PromptNotFound(name) => {
                JsonRpcError::new(-32002, format!("Prompt not found: {}", name))
            }
            McpError::Io(e) => JsonRpcError::internal_error(e.to_string()),
            McpError::TransportClosed => JsonRpcError::internal_error("Transport closed"),
            #[cfg(feature = "auth")]
            McpError::Auth(msg) => JsonRpcError::new(-32003, format!("Auth error: {}", msg)),
        }
    }
}

pub type Result<T> = std::result::Result<T, McpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let mcp_err = McpError::from(io_err);
        assert!(matches!(mcp_err, McpError::Io(_)));
        assert!(mcp_err.to_string().contains("IO error"));
    }

    #[test]
    fn test_json_error_conversion() {
        let json_err: serde_json::Error = serde_json::from_str::<String>("not valid json").unwrap_err();
        let mcp_err = McpError::from(json_err);
        assert!(matches!(mcp_err, McpError::Json(_)));
        assert!(mcp_err.to_string().contains("JSON error"));
    }

    #[test]
    fn test_transport_closed() {
        let err = McpError::TransportClosed;
        assert_eq!(err.to_string(), "Transport closed");
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32603); // internal error
    }

    #[test]
    fn test_invalid_message() {
        let err = McpError::InvalidMessage("bad message".into());
        assert!(err.to_string().contains("bad message"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32600); // invalid request
    }

    #[test]
    fn test_method_not_found() {
        let err = McpError::MethodNotFound("unknown/method".into());
        assert!(err.to_string().contains("unknown/method"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32601); // method not found
    }

    #[test]
    fn test_invalid_params() {
        let err = McpError::InvalidParams("missing required field".into());
        assert!(err.to_string().contains("missing required field"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32602); // invalid params
    }

    #[test]
    fn test_internal_error() {
        let err = McpError::Internal("something broke".into());
        assert!(err.to_string().contains("something broke"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32603); // internal error
    }

    #[test]
    fn test_tool_error() {
        let err = McpError::ToolError("tool failed".into());
        assert!(err.to_string().contains("tool failed"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32000); // custom error
    }

    #[test]
    fn test_resource_not_found() {
        let err = McpError::ResourceNotFound("file://missing".into());
        assert!(err.to_string().contains("file://missing"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32001); // custom error
    }

    #[test]
    fn test_prompt_not_found() {
        let err = McpError::PromptNotFound("missing-prompt".into());
        assert!(err.to_string().contains("missing-prompt"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32002); // custom error
    }

    #[cfg(feature = "auth")]
    #[test]
    fn test_auth_error() {
        let err = McpError::Auth("invalid token".into());
        assert!(err.to_string().contains("invalid token"));
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32003); // custom error
    }

    #[test]
    fn test_json_parse_error_to_jsonrpc() {
        let json_err: serde_json::Error = serde_json::from_str::<String>("{").unwrap_err();
        let mcp_err = McpError::from(json_err);
        let rpc_err = mcp_err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, -32700); // parse error
    }
}

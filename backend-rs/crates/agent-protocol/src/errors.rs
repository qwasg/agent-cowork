//! Error envelope + HTTP status mapping.
//!
//! Mirrors the Python `_maybe_raise` contract in `server.py`: a gateway returns
//! a JSON body `{ "error": { "code", "message" } }` and the transport maps the
//! `code` to an HTTP status. Replaces the `except Exception: pass` silent
//! degradation with strongly-typed errors (`thiserror`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ErrorEnvelope {
            error: ErrorBody {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

/// Strongly-typed application error carrying a stable `code`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ApiError {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope::new(self.code.clone(), self.message.clone())
    }

    /// HTTP status code for the error `code`, matching `server.py::_maybe_raise`.
    pub fn http_status(&self) -> u16 {
        match self.code.as_str() {
            "SESSION_NOT_FOUND"
            | "PLAN_NOT_FOUND"
            | "TODO_NOT_FOUND"
            | "RUN_NOT_FOUND"
            | "PLAN_NODE_NOT_FOUND"
            | "PROPOSAL_NOT_FOUND"
            | "PATH_NOT_FOUND"
            | "PATH_NOT_DIRECTORY"
            | "MODEL_NOT_FOUND"
            | "AUTH_USER_NOT_FOUND"
            | "SKILL_NOT_FOUND"
            | "TOOL_NOT_FOUND"
            | "PERMISSION_REQUEST_NOT_FOUND" => 404,
            "INVALID_TITLE"
            | "INVALID_PATH"
            | "PATH_OUTSIDE_ROOT"
            | "PATH_IS_DIRECTORY"
            | "TODO_INVALID"
            | "PLAN_INVALID_STATE"
            | "PROPOSAL_INVALID_STATE"
            | "TOOL_INVALID_ARGS"
            | "AUTH_INVALID_INPUT" => 400,
            "AUTH_BAD_CREDENTIALS" | "AUTH_MISSING" | "AUTH_INVALID" => 401,
            "TOOL_FORBIDDEN" => 403,
            "PROPOSAL_APPLY_FAILED" | "FILESYSTEM_ERROR" | "GIT_ERROR" | "COMMAND_ERROR" => 500,
            "NOT_A_GIT_REPO" | "AUTH_EMAIL_TAKEN" | "RUN_CANCELLED" => 409,
            "PROVIDER_HTTP_ERROR"
            | "PROVIDER_DECODE_ERROR"
            | "PROVIDER_STREAM_ERROR"
            | "WEB_SEARCH_ERROR"
            | "WEB_FETCH_ERROR" => 502,
            "PROVIDER_UNAVAILABLE" | "MCP_NOT_INSTALLED" | "MCP_DEMO_SERVER_MISSING" => 503,
            "COMMAND_TIMEOUT" => 504,
            _ => 400,
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// Convenience constructors for the most common error codes.
impl ApiError {
    pub fn session_not_found(id: &str) -> Self {
        ApiError::new("SESSION_NOT_FOUND", format!("session not found: {id}"))
    }
    pub fn plan_not_found(id: &str) -> Self {
        ApiError::new("PLAN_NOT_FOUND", format!("plan not found: {id}"))
    }
    pub fn run_not_found(id: &str) -> Self {
        ApiError::new("RUN_NOT_FOUND", format!("run not found: {id}"))
    }
    pub fn todo_not_found(id: &str) -> Self {
        ApiError::new("TODO_NOT_FOUND", format!("todo not found: {id}"))
    }
    pub fn path_not_found(p: &str) -> Self {
        ApiError::new("PATH_NOT_FOUND", format!("path not found: {p}"))
    }
    pub fn invalid_path(p: &str) -> Self {
        ApiError::new("INVALID_PATH", format!("invalid path: {p}"))
    }
    pub fn path_outside_root(p: &str) -> Self {
        ApiError::new(
            "PATH_OUTSIDE_ROOT",
            format!("path escapes workspace root: {p}"),
        )
    }
    pub fn filesystem(msg: impl Into<String>) -> Self {
        ApiError::new("FILESYSTEM_ERROR", msg)
    }
}

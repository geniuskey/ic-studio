//! IC Studio MCP tool layer, shared by the `ics-mcp` stdio server and the desktop app (live mode).

pub mod bridge;
pub mod tools;

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Map, Value, json};

/// Outcome of one tool call: the text content returned to the agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }

    pub fn to_json(&self) -> Value {
        json!({"text": self.text, "is_error": self.is_error})
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self { text: v.get("text")?.as_str()?.to_string(), is_error: v.get("is_error")?.as_bool()? })
    }

    /// The result parsed as JSON (successful tool results are JSON objects).
    pub fn value(&self) -> Option<Value> {
        serde_json::from_str(&self.text).ok()
    }
}

pub fn has_tool(name: &str) -> bool {
    tools::definitions().iter().any(|t| t["name"] == name)
}

/// Run a tool on `state`, turning unknown tools and panics into error results.
pub fn call_tool(state: &mut tools::State, name: &str, args: &Map<String, Value>) -> ToolResult {
    match catch_unwind(AssertUnwindSafe(|| state.call(name, args))) {
        Ok(Some(Ok(v))) => ToolResult { text: v.to_string(), is_error: false },
        Ok(Some(Err(e))) => ToolResult::error(e),
        Ok(None) => ToolResult::error(format!("unknown tool: {name}")),
        Err(_) => ToolResult::error(format!("internal error while running {name}")),
    }
}

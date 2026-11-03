//! Port of apptools.ts (the app tools, MCP over HTTP without the SDK). See PORTING.md.

// Stubs until apptools.rs is ported (an agent is on it); see PORTING.md.
use serde_json::Value;
use std::sync::Arc;

/// apptools.ts `Grant`: what one agent process's secret lets its tools do.
pub enum Grant {
    Worker { todo_id: i64, dir: String, on_post: Option<Arc<dyn Fn() + Send + Sync>>, via: Option<String> },
    Operator,
    Router { todo_id: i64 },
}
impl Grant {
    /// The worker grant of a Claude terminal (`kind: "worker"`, no onPost, `via: "Claude <n>"`).
    pub fn terminal(todo_id: i64, dir: &str, via: &str) -> Grant {
        Grant::Worker { todo_id, dir: dir.to_string(), on_post: None, via: Some(via.to_string()) }
    }
}
pub fn grant(_g: Grant, _header: impl Fn(&str) -> String) -> (String, Value) { todo!("port") }
pub fn revoke(_secret: &str) { todo!("port") }
pub fn worker_tools() -> Vec<String> { todo!("port") }
/// The agents' route to the app's MCP tools (/mcp/app): each agent session carries its own secret. Stub for http.rs.
pub async fn handle_mcp(_req: axum::extract::Request) -> axum::response::Response { todo!("port") }
pub fn operator_tools() -> Vec<String> { todo!("port") }
pub fn router_tools() -> Vec<String> { todo!("port") }

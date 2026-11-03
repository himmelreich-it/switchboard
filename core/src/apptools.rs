//! Port of apptools.ts (the app tools, MCP over HTTP without the SDK). See PORTING.md.

use serde_json::Value;
pub struct Grant;
impl Grant {
    /// The worker grant of a Claude terminal (`kind: "worker"`, no onPost, `via: "Claude <n>"`).
    pub fn terminal(_todo_id: i64, _dir: &str, _via: &str) -> Grant { todo!("port") }
}
pub fn grant(_g: Grant, _header: impl Fn(&str) -> String) -> (String, Value) { todo!("port") }
pub fn revoke(_secret: &str) { todo!("port") }
pub fn worker_tools() -> Vec<String> { todo!("port") }
/// The agents' route to the app's MCP tools (/mcp/app): each agent session carries its own secret. Stub for http.rs.
pub async fn handle_mcp(_req: axum::extract::Request) -> axum::response::Response { todo!("port") }

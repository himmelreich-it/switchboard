//! Port of runner.ts. See PORTING.md.

use serde_json::Value;

/// A run's key: a TODO id, or None for the operator.
pub type Key = Option<i64>;

pub fn is_running(_key: Key) -> bool {
    todo!("port")
}
pub fn background_count(_todo_id: i64) -> i64 {
    todo!("port")
}
pub fn background_tasks(_todo_id: i64) -> Value {
    todo!("port")
}
pub const TERMINAL_CONTRACT: &str = "";
pub fn cancel_routing(_todo_id: i64) { todo!("port") }
pub fn fire_due(_startup: bool) { todo!("port") }
pub async fn stop_all() { todo!("port") }
pub fn close_agents() { todo!("port") }
pub fn interrupt(_key: Key, _by: &'static str) { todo!("port") }
pub fn close_agent(_key: Key) { todo!("port") }
pub fn is_scanning(_project_id: i64) -> bool { todo!("port") }
pub fn scan_error(_project_id: i64) -> Option<String> { todo!("port") }

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

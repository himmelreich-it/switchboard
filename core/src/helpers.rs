//! Port of lib.ts: see PORTING.md.

// Stubs until helpers.rs is ported (an agent is on it); see PORTING.md.
use serde_json::Value;
pub fn safe_file_name(_name: &str) -> String { todo!("port") }
pub fn parse_git_status(_out: &str) -> Value { todo!("port") }
pub fn parse_ref_counts(_out: &str, _branch: Option<&str>) -> Value { todo!("port") }
pub fn schedule_text(_s: &Value) -> String { todo!("port") }
pub fn context_file(_c: &Value) -> String { todo!("port") }
pub fn descendants(_ps: &str, _root: i64) -> Vec<i64> { todo!("port") }
pub fn migrate_db(_c: &rusqlite::Connection) { todo!("port") }
pub fn status_after_run(_attention: bool) -> &'static str { todo!("port") }

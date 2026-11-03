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

// Stubs used by http.rs until runner.rs is ported (an agent is on it); see PORTING.md.
pub const DEFAULT_BACKGROUND_HOURS: f64 = 2.0;
pub const DEFAULT_STARTED_PER_TURN: i64 = 10;
/// runner.ts `Agent`: the live agent process of a TODO's (or the operator's) session.
pub struct Agent;
impl Agent {
    /// Reads and clears `configError` (server.ts applyNow: `const warning = a.configError || null; a.configError = ""`).
    pub fn take_config_error(&self) -> String { todo!("port") }
}
pub fn live_agent(_todo_id: i64) -> Option<std::sync::Arc<Agent>> { todo!("port") }
pub async fn apply_config(_todo_id: i64, _a: &Agent, _reset: Option<&str>) -> anyhow::Result<()> { todo!("port") }
pub fn contract_for(_agent: &str, _dir: &str) -> String { todo!("port") }
pub fn is_git_folder(_dir: &str) -> bool { todo!("port") }
pub fn answer_permission(_todo_id: i64, _option_id: &str) -> bool { todo!("port") }
/// `t`: { mode, agent }; `p`: a project row or { defaults: null }.
pub fn mode_of(_t: &Value, _p: &Value) -> Option<String> { todo!("port") }
pub fn meta_key(_key: &'static str, _agent: &str) -> String { todo!("port") }
pub fn agent_configs() -> Value { todo!("port") }
pub fn commands_of(_todo_id: i64) -> Value { todo!("port") }
pub fn commit_message(_todo_id: i64, _author: &'static str, _body: &str, _uploads: &[crate::server::Upload]) -> i64 { todo!("port") }
pub fn start(_todo_id: i64) { todo!("port") }
pub fn new_session(_todo_id: i64, _agent: Option<&str>, _scheduled: Option<&str>) { todo!("port") }
pub fn fire_schedule(_todo_id: i64, _how: &'static str) -> Option<String> { todo!("port") }
pub fn deliver(_todo_id: i64, _reason: &str) { todo!("port") }
pub fn worktree_root(_local_path: &str) -> Option<String> { todo!("port") }
pub fn change_repo(_todo_id: i64, _project_id: Option<i64>, _branch: Option<&str>) { todo!("port") }
pub async fn scan_repo(_project_id: i64) { todo!("port") }
pub fn commit_operator_message(_body: &str) -> i64 { todo!("port") }

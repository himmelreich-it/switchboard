//! Port of server.ts, second part: pull requests, the worktree scan, ports, the writes shared with runner.rs and
//! apptools.rs (createTodo, removeTodo, setSchedule), and the migration from auto-coder. See PORTING.md.

use serde_json::Value;

/// The PR the list shows for a TODO, from the PR store: set_pr's number wins over the worktree's branch.
/// Returns (the PR, when its project was last checked, the project's gh error) as in server.ts `summaries`.
pub fn pr_for(_project_id: Option<i64>, _pr_number: Option<i64>, _branch: Option<&str>) -> (Option<Value>, Option<String>, Value) {
    todo!("port")
}
pub fn migrate() { todo!("port") }
pub async fn refresh_all_prs() { todo!("port") }

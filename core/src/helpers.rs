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

// Stubs added for server_ops.rs (lib.ts names, PORTING.md signatures). The constants carry lib.ts's real values.
pub fn slugify(_name: &str) -> String { todo!("port") }
pub fn unique_slug(_slug: &str, _taken: &std::collections::HashSet<String>) -> String { todo!("port") }
pub fn parse_listen(_out: &str) -> Value { todo!("port") }
pub fn parse_cwd(_out: &str) -> std::collections::HashMap<i64, String> { todo!("port") }
pub fn parse_ps(_out: &str) -> std::collections::HashMap<i64, Value> { todo!("port") }
pub const DOCKER_FORMAT: &str = "{{.Names}}|{{.Ports}}|{{.Label \"com.docker.compose.project.working_dir\"}}|{{.RunningFor}}|{{.Image}}";
pub fn parse_docker(_out: &str) -> Value { todo!("port") }
pub fn owner_of(_folder: &str, _todos: &Value, _projects: &Value) -> Value { todo!("port") }
pub fn defaults_of(_raw: Option<&str>) -> Value { todo!("port") }
pub fn parse_github_remote(_url: &str) -> Option<Value> { todo!("port") }
pub fn parse_gh_logins(_json: Option<&str>, _text: &str) -> Vec<String> { todo!("port") }
pub const PR_FIELDS: &str = "fragment F on PullRequest { number url state isDraft mergeable mergeStateStatus reviewDecision baseRefName
  reviewThreads(first: 100) { nodes { isResolved } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } }";
pub fn pr_query(_branches: i64, _numbers: i64, _fields: &str) -> String { todo!("port") }
pub fn pr_from_node(_n: &Value) -> Value { todo!("port") }
pub const SCAN_PR_FIELDS: &str = "fragment F on PullRequest { number url state commits(first: 250) { nodes { commit { oid } } } }";
pub fn parse_worktree_list(_out: &str) -> Value { todo!("port") }
pub fn worktree_verdict(_f: &Value) -> Value { todo!("port") }
pub fn gh_outcome(_code: Option<i64>, _stdout: &str, _stderr: &str) -> Value { todo!("port") }
pub fn next_run(_s: &Value, _after: chrono::DateTime<chrono::Local>) -> Option<chrono::DateTime<chrono::Local>> { todo!("port") }

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

// Stubs added for http.rs (lib.ts names, PORTING.md signatures).
pub const MODES: &[&str] = &["auto", "default", "acceptEdits", "plan", "bypassPermissions"];
pub fn config_in(_v: &Value) -> Value { todo!("port") }
pub fn is_inside(_path: &str, _dir: &str) -> bool { todo!("port") }
pub fn token_ok(_token: &str, _header: Option<&str>, _query: Option<&str>) -> bool { todo!("port") }
pub fn parse_marks(_name_status_z: &str, _untracked_z: &str) -> Value { todo!("port") }
pub fn reveal_command(_platform: &str, _file: &str) -> Vec<String> { todo!("port") }
pub fn vscode_command(_platform: &str, _dir: &str) -> Vec<String> { todo!("port") }
pub fn schedule_in(_v: &Value) -> Result<Value, String> { todo!("port") }
// For runner.rs: the lib.ts helpers it imports, with PORTING.md's signature conventions.
pub fn agent_activity(_u: &Value) -> bool { todo!("port") }
pub fn attention_after_answer(_by: Option<&str>) -> i64 { todo!("port") }
pub fn auto_effort(_current: &Value, _wanted: &Value, _level: Option<&str>) -> Value { todo!("port") }
pub fn background_level(_ids: &[String], _m: &Value) -> Vec<String> { todo!("port") }
pub fn background_names(_names: &Value, _m: &Value) -> Value { todo!("port") }
pub fn background_step(_ids: &[String], _u: &Value) -> Vec<String> { todo!("port") }
pub fn config_changes(_current: &Value, _wanted: &Value, _models: bool) -> Value { todo!("port") }
pub fn cut_reason(_error: &str) -> Option<String> { todo!("port") }
pub fn cycle_end(_u: &Value) -> bool { todo!("port") }
pub fn config_view(_raw: &Value) -> Value { todo!("port") }
pub fn defaults_for(_raw: Option<&str>, _agent: &str) -> Value { todo!("port") }
pub fn ctx_step(_c: &Value, _u: &Value) -> Value { todo!("port") }
pub fn at_tool_boundary(_batch: &[Option<String>]) -> bool { todo!("port") }
pub fn cut_to_two_lines(_text: &str) -> String { todo!("port") }
pub fn delivery_action(_status: &str, _busy: bool) -> &'static str { todo!("port") }
pub fn delivery_prompt(_worktree: Option<&str>, _bodies: &[String], _header: &str) -> String { todo!("port") }
pub fn hand_over(_o: &Value) -> String { todo!("port") }
pub fn topic_intro(_p: Option<&Value>, _worktrees: Option<&str>) -> String { todo!("port") }
pub fn permission_of(_r: &Value, _at: &str) -> Value { todo!("port") }
pub fn plan_count(_entries: &Value) -> Option<String> { todo!("port") }
pub fn skip_reason(_t: &Value, _busy: bool, _queued: i64) -> Option<String> { todo!("port") }
pub fn slot_text(_d: chrono::DateTime<chrono::Local>) -> String { todo!("port") }
pub fn swap_place(_places: &Value, _from: Option<i64>, _to: Option<i64>, _cur: &Value, _exists: impl Fn(&str) -> bool) -> Value { todo!("port") }
pub fn tool_arg(_input: &Value) -> String { todo!("port") }
pub fn tool_line(_name: &str, _input: &Value) -> String { todo!("port") }
pub fn tool_meta(_prev: Option<&Value>, _u: &Value) -> Value { todo!("port") }
pub fn with_attachments(_body: &str, _paths: &[String]) -> String { todo!("port") }

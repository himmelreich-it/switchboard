//! Port of server.ts terminals (items 111-120), with the PTY host that was switchboard-core. See PORTING.md.

use serde_json::Value;

/// The shell of a new terminal: a login shell on macOS and Linux; PowerShell (else cmd.exe) on Windows, which has none.
pub fn shell() -> String {
    todo!("foundation")
}
pub const LOGIN_SHELL: bool = !cfg!(windows);
/// Whether the TODO has a terminal whose process still runs (Change topic refuses then).
pub fn any_running(_todo_id: i64) -> bool {
    todo!("foundation")
}
/// Ends the TODO's terminals, or every terminal (None, at shutdown).
pub fn close_terms(_todo_id: Option<i64>) {
    todo!("foundation")
}
/// /api/todos/:id/terminals[/:tid/(restart|close|busy)]: GET list, POST open, POST restart/close, GET busy.
pub async fn api(_method: &str, _todo_id: i64, _term_id: Option<i64>, _action: Option<&str>, _body: &Value) -> anyhow::Result<Value> {
    todo!("foundation")
}
/// The page's terminal WebSocket (/api/todos/:id/terminals/:tid/ws), after the HTTP layer checked token and origin.
pub fn ws(_upgrade: axum::extract::ws::WebSocketUpgrade, _todo_id: i64, _term_id: i64) -> axum::response::Response {
    todo!("foundation")
}

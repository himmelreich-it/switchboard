//! Port of agents.ts: the agent registry (P5). Which ACP agents a TODO can run, and what differs between them. For now
//! Claude is the only built-in (Codex and Gemini wait until they can be tested); the user's own agents would be rows of
//! the `agents` table, whose Settings page stays hidden while Claude is the only agent. runner.rs asks this file how to
//! start an agent and how to talk to it; acp.rs knows nothing about agents beyond the command line it is given.

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::acp::Spec;
use crate::util::{self, J};
use crate::{db, params};

struct Builtin {
    id: &'static str,
    name: &'static str,
    command: fn() -> Option<Vec<String>>,
    shown: &'static str,
    hint: Option<&'static str>,
}

/// The JS runtime for a bundled adapter: SWITCHBOARD_JS (a path, used as is), else bun, else node on PATH.
fn js_runtime() -> Option<String> {
    if let Ok(js) = std::env::var("SWITCHBOARD_JS") {
        return Some(js);
    }
    util::which("bun").or_else(|| util::which("node"))
}

/// A bundled adapter's command line: the JS runtime and the package's dist/index.js. None when either is missing.
/// PORT: TS ran the adapter on the Bun that ran the server (`process.execPath`, the package found with
/// Bun.resolveSync); the Rust server has no JS runtime of its own, so it looks for one, and the package under the
/// app's node_modules.
fn adapter(pkg: &str) -> Option<Vec<String>> {
    let script = util::join(&util::join(&util::join(util::app_dir(), "node_modules"), pkg), "dist/index.js");
    if !util::exists(&script) {
        return None;
    }
    Some(vec![js_runtime()?, script])
}

// Codex's adapter, @agentclientprotocol/codex-acp, ran on Bun too (P5 probe), so it would be
// `adapter("@agentclientprotocol/codex-acp")`.
const BUILTINS: &[Builtin] = &[Builtin {
    id: "claude",
    name: "Claude",
    command: || adapter("@agentclientprotocol/claude-agent-acp"),
    shown: "claude-agent-acp (bundled)",
    hint: None,
}];
pub const DEFAULT_AGENT: &str = "claude";

/// A command found on PATH, with its arguments. A Windows .cmd or .bat shim (npm's global installs) runs through
/// cmd.exe.
pub fn on_path(cmd: &str, args: &[String]) -> Option<Vec<String>> {
    let path = util::which(cmd)?;
    let lower = path.to_lowercase();
    if cfg!(windows) && (lower.ends_with(".cmd") || lower.ends_with(".bat")) {
        let mut v: Vec<String> = ["cmd.exe", "/d", "/s", "/c"].iter().map(|s| s.to_string()).collect();
        v.push(path);
        v.extend(args.iter().cloned());
        return Some(v);
    }
    let mut v = vec![path];
    v.extend(args.iter().cloned());
    Some(v)
}

fn custom_row(id: &str) -> Option<Value> {
    db::get("SELECT * FROM agents WHERE id = ?", params![id])
}
pub fn custom_rows() -> Vec<Value> {
    db::all("SELECT * FROM agents ORDER BY name", params![])
}

/// `JSON.parse(r.args || "[]")` as strings.
fn args_of(r: &Value) -> Vec<String> {
    let text = r.s("args").filter(|s| !s.is_empty()).unwrap_or("[]");
    match util::parse(text) {
        Value::Array(a) => a.iter().map(|x| util::str_of(Some(x))).collect(),
        _ => vec![],
    }
}
/// `JSON.parse(r.env || "{}")`; Bun's spawn turns the values into strings.
fn env_of(r: &Value) -> HashMap<String, String> {
    let text = r.s("env").filter(|s| !s.is_empty()).unwrap_or("{}");
    match util::parse(text) {
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), util::str_of(Some(v)))).collect(),
        _ => HashMap::new(),
    }
}

/// How to start an agent; None when its command is not there (not installed, or an id no longer in the registry).
pub fn spec_of(id: &str) -> Option<Spec> {
    if let Some(b) = BUILTINS.iter().find(|x| x.id == id) {
        return (b.command)().map(|command| Spec { command, env: HashMap::new() });
    }
    let r = custom_row(id)?;
    let command = on_path(r.s("command").unwrap_or(""), &args_of(&r))?;
    Some(Spec { command, env: env_of(&r) })
}

/// Every agent, built-ins first, for Settings › Agents and the agent selects (AgentInfo: id, name, builtin, command as
/// shown in Settings, found: the command can be started, hint: how to get it when it is not found).
pub fn agent_list() -> Vec<Value> {
    let mut out: Vec<Value> = BUILTINS
        .iter()
        .map(|b| json!({ "id": b.id, "name": b.name, "builtin": true, "command": b.shown, "found": (b.command)().is_some(), "hint": b.hint }))
        .collect();
    for r in custom_rows() {
        let cmd = r.s("command").unwrap_or("").to_string();
        let mut shown = vec![cmd.clone()];
        shown.extend(args_of(&r));
        out.push(json!({
            "id": r["id"], "name": r["name"], "builtin": false, "command": shown.join(" "),
            "found": util::which(&cmd).is_some(), "hint": null,
        }));
    }
    out
}

pub fn agent_name(id: Option<&str>) -> String {
    let want = id.filter(|s| !s.is_empty()).unwrap_or(DEFAULT_AGENT);
    agent_list()
        .into_iter()
        .find(|a| a.s("id") == Some(want))
        .and_then(|a| a.s("name").map(String::from))
        .or_else(|| id.map(String::from))
        .unwrap_or_else(|| "Claude".into())
}
pub fn is_builtin(id: &str) -> bool {
    BUILTINS.iter().any(|b| b.id == id)
}
pub fn is_known(id: &str) -> bool {
    is_builtin(id) || custom_row(id).is_some()
}

/// Claude gets the contract as a system-prompt append and its options through the adapter's `_meta`; every other
/// agent gets the contract as text in front of the first prompt of each session (design §9).
pub fn is_claude(id: Option<&str>) -> bool {
    id.filter(|s| !s.is_empty()).unwrap_or(DEFAULT_AGENT) == "claude"
}

/// How the contract names an app tool for this agent: Claude Code's own MCP tool name, else a description any agent
/// maps to its own naming (Codex, for one, does not use the mcp__ prefix).
pub fn tool_name(agent: Option<&str>, tool: &str) -> String {
    if is_claude(agent) {
        format!("mcp__app__{tool}")
    } else {
        format!("the app MCP server's {tool} tool")
    }
}

/// The command that resumes a session outside the app, where the agent has one (item 78).
pub fn resume_hint(agent: Option<&str>, session_id: &str) -> Option<String> {
    let id = agent.filter(|s| !s.is_empty()).unwrap_or(DEFAULT_AGENT);
    if id == "claude" {
        return Some(format!("claude --resume {session_id}"));
    }
    None
}

/// The MCP secret header: Claude Code expands ${VAR} from the agent process's environment, so the secret stays out of
/// the CLI's argv (readable by every user in /proc). Other agents get it as is: they take the MCP config over ACP.
pub fn secret_header(agent: Option<&str>, secret: &str) -> String {
    if is_claude(agent) {
        "${SWITCHBOARD_MCP_SECRET}".into()
    } else {
        secret.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_claude(None) && is_claude(Some("")) && is_claude(Some("claude")) && !is_claude(Some("codex")));
        assert_eq!(tool_name(None, "ask"), "mcp__app__ask");
        assert_eq!(tool_name(Some("codex"), "ask"), "the app MCP server's ask tool");
        assert_eq!(resume_hint(None, "abc").as_deref(), Some("claude --resume abc"));
        assert_eq!(resume_hint(Some("codex"), "abc"), None);
        assert_eq!(secret_header(Some("claude"), "s"), "${SWITCHBOARD_MCP_SECRET}");
        assert_eq!(secret_header(Some("codex"), "s"), "s");
        assert!(is_builtin("claude") && !is_builtin("codex"));
        assert_eq!(on_path("sh", &["-c".into()]).unwrap()[1], "-c");
        assert!(on_path("no-such-command-here", &[]).is_none());
    }
}

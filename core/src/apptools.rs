//! Port of apptools.ts. The app's MCP tools (R31): served over HTTP at /mcp/app on this loopback server, because an
//! ACP agent runs in its own process. Each agent session gets a random secret in the X-Switchboard-Session header; the
//! secret names the TODO (or the operator), so every handler still knows its TODO without matching session ids.
//! Stateless: the tool set is built for each request.
//!
//! The TS code used @modelcontextprotocol/sdk (McpServer over WebStandardStreamableHTTPServerTransport, stateless,
//! `enableJsonResponse: true`) and zod. This file speaks that protocol itself: the transport's checks and answers,
//! JSON-RPC dispatch (initialize, ping, tools/list, tools/call), the zod schemas as JSON Schema, and zod's argument
//! validation with its messages. core/tests/apptools-*.json hold what the TS module answered (recorded from apptools.ts
//! with SDK 1.32.1 and zod 4.6.5 at commit 2d1a247, the last with the TS server); the tests below replay them.

use crate::server::{self, add_event, add_message, changed, todo_row};
use crate::util::{self, basename, J};
use crate::{agents, db, helpers, params, runner, server_ops};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

const WORKER_NAMES: &[&str] = &[
    "post_message", "set_title", "set_attention", "attach_file", "set_worktree", "set_pr", "suggest_follow_ups", "list_comments", "reply_comment",
    "read_todo", "read_file", "create_todo", "remove_todo", "set_schedule", "set_effort",
];
const OPERATOR_NAMES: &[&str] = &["list_todos", "read_todo", "read_file", "create_todo", "send_message", "remove_todo"];
// The operator's routing run for one TODO (801): read other TODOs, then decide.
const ROUTER_NAMES: &[&str] = &["read_todo", "read_file", "route_todo", "ask_topic"];
fn app_names(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| format!("mcp__app__{n}")).collect()
}
/// TS `WORKER_TOOLS`: the `mcp__app__<name>` list.
pub fn worker_tools() -> Vec<String> {
    app_names(WORKER_NAMES)
}
/// TS `OPERATOR_TOOLS`.
pub fn operator_tools() -> Vec<String> {
    app_names(OPERATOR_NAMES)
}
/// TS `ROUTER_TOOLS`.
pub fn router_tools() -> Vec<String> {
    app_names(ROUTER_NAMES)
}

// ---------- grants ----------

/// Tells the run that the agent posted a chat message this turn (so its final text is not posted as well). TS
/// `onPost: () => void`: no arguments.
pub type OnPost = Arc<dyn Fn() + Send + Sync>;

/// apptools.ts `Grant`: what one agent process's secret lets its tools do. `via` names the Claude terminal a worker
/// grant belongs to (P6): its chat messages carry it.
#[derive(Clone)]
pub enum Grant {
    Worker { todo_id: i64, dir: String, on_post: Option<OnPost>, via: Option<String> },
    Operator,
    Router { todo_id: i64 },
}
impl Grant {
    /// The worker grant of a Claude terminal (`kind: "worker"`, no onPost, `via: "Claude <n>"`).
    pub fn terminal(todo_id: i64, dir: &str, via: &str) -> Grant {
        Grant::Worker { todo_id, dir: dir.to_string(), on_post: None, via: Some(via.to_string()) }
    }
}

static GRANTS: LazyLock<Mutex<HashMap<String, Grant>>> = LazyLock::new(Default::default);

/// A secret for one agent process, and the MCP server entry that names it. Revoke it when the agent process ends.
/// `header` gives the header value for the secret: Claude's is a ${VAR} reference to the agent process's environment
/// (runner.rs sets SWITCHBOARD_MCP_SECRET), see agents.rs secret_header.
pub fn grant(g: Grant, header: impl Fn(&str) -> String) -> (String, Value) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let secret: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    GRANTS.lock().unwrap().insert(secret.clone(), g);
    let server = json!({
        "type": "http", "name": "app", "url": format!("http://{}:{}/mcp/app", util::HOST, util::port()),
        "headers": [{ "name": "X-Switchboard-Session", "value": header(&secret) }],
    });
    (secret, server)
}
pub fn revoke(secret: &str) {
    GRANTS.lock().unwrap().remove(secret);
}

pub async fn handle_mcp(req: Request) -> Response {
    let g = GRANTS.lock().unwrap().get(&header_str(req.headers(), "x-switchboard-session").unwrap_or_default()).cloned();
    let Some(g) = g else { return (StatusCode::FORBIDDEN, "unknown session").into_response() };
    // Stateless: no server-initiated stream (GET) and no session to end (DELETE).
    if req.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    // PORT: TS threw here (a TODO row gone under a live grant) and Bun answered 500; so does this.
    let tools = match tools_for(&g) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    transport(req, &tools).await
}

/// The tools of one grant, registered in the TS order (tools/list keeps it).
fn tools_for(g: &Grant) -> anyhow::Result<Vec<Tool>> {
    let mut tools = Vec::new();
    match g {
        Grant::Worker { todo_id, dir, on_post, via } => {
            let todo_id = *todo_id;
            register_worker_tools(&mut tools, todo_id, dir, on_post.clone(), via.clone())?;
            read_tools(&mut tools, Arc::new(|_, _| {}));
            // Item 145: a worker's turn starts at its run; a Claude terminal has no turns in the app.
            // ponytail: a terminal's "turn" is the last 10 minutes; track its prompts if the cap needs to be exact there.
            let terminal = via.is_some();
            let via = via.clone();
            create_tool(
                &mut tools,
                By {
                    parent: Some(todo_id),
                    used: Arc::new(move |d| {
                        let detail = [via.as_ref().map(|v| format!("from {v}")), Some(d.to_string())].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                        add_event(Some(todo_id), "create_todo", "agent", Some(&detail), None);
                    }),
                    since: Arc::new(move || {
                        if terminal {
                            return Ok(util::iso(chrono::Utc::now() - chrono::Duration::milliseconds(600_000)));
                        }
                        Ok(row(todo_id)?.s("run_started_at").unwrap_or("").to_string())
                    }),
                },
            );
        }
        Grant::Router { todo_id } => {
            let todo_id = *todo_id;
            read_tools(&mut tools, Arc::new(move |name, detail| {
                add_event(Some(todo_id), name, "operator", detail, None);
            }));
            register_router_tools(&mut tools, todo_id);
        }
        Grant::Operator => {
            let used: Used = Arc::new(|name, detail| {
                add_event(None, name, "operator", detail, None);
            });
            read_tools(&mut tools, used.clone());
            register_operator_tools(&mut tools, used.clone());
            // The operator's turn starts with the user's last message to it.
            create_tool(
                &mut tools,
                By {
                    parent: None,
                    used: Arc::new(move |d| used("create_todo", Some(d))),
                    since: Arc::new(|| {
                        Ok(db::get("SELECT committed_at AS at FROM messages WHERE todo_id IS NULL AND author = 'user' ORDER BY id DESC LIMIT 1", params![])
                            .and_then(|r| r.s("at").map(String::from))
                            .unwrap_or_default())
                    }),
                },
            );
        }
    }
    Ok(tools)
}

// ---------- tools ----------

/// The zod types the tools use. Each turns into the JSON Schema zod's toJSONSchema gave and validates as zod did.
#[derive(Clone, Copy)]
enum Ty {
    /// z.string()
    Str,
    /// z.boolean()
    Bool,
    /// z.number().int()
    Int,
    /// z.number().int().positive()
    PosInt,
    /// z.any()
    Any,
    /// z.enum(AUTO_EFFORTS)
    Effort,
    /// z.array(z.string())
    Strs,
}
/// A field: name, type, required (false: `.optional()`).
type Field = (&'static str, Ty, bool);
type Run = Arc<dyn Fn(Value) -> BoxFuture<'static, anyhow::Result<Value>> + Send + Sync>;

struct Tool {
    name: &'static str,
    description: String,
    shape: &'static [Field],
    run: Run,
}

fn tool<F, Fut>(tools: &mut Vec<Tool>, name: &'static str, description: impl Into<String>, shape: &'static [Field], f: F)
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<Value>> + Send + 'static,
{
    tools.push(Tool { name, description: description.into(), shape, run: Arc::new(move |a| Box::pin(f(a))) });
}

fn ok(text: impl Into<String>) -> anyhow::Result<Value> {
    Ok(json!({ "content": [{ "type": "text", "text": text.into() }] }))
}
fn err(text: impl Into<String>) -> anyhow::Result<Value> {
    Ok(json!({ "content": [{ "type": "text", "text": text.into() }], "isError": true }))
}
/// `JSON.stringify(v, null, 1)`
fn json_text(v: &Value) -> anyhow::Result<Value> {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, serde_json::ser::PrettyFormatter::with_indent(b" "));
    v.serialize(&mut ser)?;
    ok(String::from_utf8(buf)?)
}

/// `todoRow(id)!`: TS threw a TypeError on a missing row, which the SDK returned as a tool error.
fn row(id: i64) -> anyhow::Result<Value> {
    todo_row(id).ok_or_else(|| anyhow::anyhow!("no TODO T-{id}"))
}
/// A string argument the schema made sure of.
fn arg(a: &Value, k: &str) -> String {
    a.s(k).unwrap_or_default().to_string()
}
/// JS `String.prototype.trim`: White_Space plus U+FEFF, without U+0085.
fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}
/// `[a, b].filter(Boolean).join(" · ")`
fn joined(parts: &[Option<String>]) -> String {
    parts.iter().flatten().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ")
}
fn from_via(via: &Option<String>) -> Option<String> {
    via.as_ref().map(|v| format!("from {v}"))
}
/// node:path `isAbsolute`.
fn is_absolute(p: &str) -> bool {
    // PORT: on Windows node also counts "/x" and "\x" as absolute; Rust's Path does not.
    Path::new(p).is_absolute()
}
/// node:path `resolve` of an absolute path: `.` and `..` folded, no trailing separator, symlinks kept.
fn resolve(p: &str) -> String {
    let mut out = PathBuf::new();
    for c in Path::new(p).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}
/// `s.slice(0, n)` counts UTF-16 code units.
fn js_slice(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

const TODO_ID: Field = ("todo_id", Ty::Int, true);

fn register_worker_tools(t: &mut Vec<Tool>, todo_id: i64, dir: &str, on_post: Option<OnPost>, via: Option<String>) -> anyhow::Result<()> {
    let dir = dir.to_string();
    let on_post = on_post.unwrap_or_else(|| Arc::new(|| {}));

    let (d, v) = (dir.clone(), via.clone());
    tool(t, "post_message", "Post a one or two line chat message to the user. Pass file_name to link a file you registered with attach_file.",
        &[("body", Ty::Str, true), ("file_name", Ty::Str, false)], move |a| {
        let (dir, via, on_post) = (d.clone(), v.clone(), on_post.clone());
        async move {
            let body = arg(&a, "body");
            let file_name = a.s("file_name").filter(|f| !f.is_empty()).map(String::from);
            if trim(&body).is_empty() {
                return err("body is empty");
            }
            if helpers::line_count(&body) > 2 {
                return err(format!("Chat messages are one or two lines. Put the longer content in a markdown file in {dir}, register it with attach_file, and link it with file_name."));
            }
            let mut file_id = None;
            if let Some(name) = &file_name {
                let f = server::files_of(todo_id).into_iter().find(|x| basename(x.s("path").unwrap_or("")) == basename(name));
                let Some(f) = f else { return err(format!("No attached file is named {name}. Call attach_file first.")) };
                file_id = f.i("id");
            }
            add_message(Some(todo_id), "agent", trim(&body), "delivered", file_id, via.as_deref());
            let detail = joined(&[from_via(&via), file_name]);
            add_event(Some(todo_id), "post_message", "agent", (!detail.is_empty()).then_some(detail.as_str()), None);
            on_post();
            ok("ok")
        }
    });

    let v = via.clone();
    tool(t, "remove_todo", "Remove a TODO this TODO created with create_todo, only when the user's instruction says to: like the user's Remove, its run stops and it moves to the Removed fold, where the user can restore it.",
        &[TODO_ID], move |a| {
        let via = v.clone();
        async move {
            let id = a.i("todo_id").unwrap_or_default();
            let Some(row) = todo_row(id).filter(|r| !r.truthy("removed_at")) else { return err(format!("no live TODO T-{id}")) };
            if row.i("parent_id") != Some(todo_id) {
                return err(format!("T-{id} was not created by this TODO (T-{todo_id}); only the user or the operator removes it."));
            }
            server_ops::remove_todo(id, &format!("T-{todo_id}"));
            add_event(Some(todo_id), "remove_todo", "agent", Some(&joined(&[from_via(&via), Some(format!("T-{id}"))])), None);
            ok(format!("Removed T-{id}. The user can restore it from the Removed fold."))
        }
    });

    let v = via.clone();
    tool(t, "set_schedule", "Set, change, pause or remove (schedule null) the schedule of a TODO this TODO created with create_todo, only when the user's instruction says to. schedule: {days: [1,2,3,4,5], times: [\"09:00\"]} or {days, every: 30, from: \"08:00\", to: \"18:00\"} (days 0 = Sunday, local time), optional prompt (each run's first message; left out, the TODO's first instruction) and paused.",
        &[TODO_ID, ("schedule", Ty::Any, true)], move |a| {
        let via = v.clone();
        async move {
            let id = a.i("todo_id").unwrap_or_default();
            let Some(row) = todo_row(id).filter(|r| !r.truthy("removed_at")) else { return err(format!("no live TODO T-{id}")) };
            if row.i("parent_id") != Some(todo_id) {
                return err(format!("T-{id} was not created by this TODO (T-{todo_id}); only the user changes its schedule."));
            }
            let sched = match a.get("schedule") {
                None | Some(Value::Null) => Value::Null,
                Some(s) => match helpers::schedule_in(s) {
                    Ok(s) => s,
                    Err(e) => return err(e),
                },
            };
            server_ops::set_schedule(id, &sched, &format!("T-{todo_id}"));
            add_event(Some(todo_id), "set_schedule", "agent", Some(&joined(&[from_via(&via), Some(format!("T-{id}"))])), None);
            let next = todo_row(id).and_then(|r| r.s("next_run_at").map(String::from));
            ok(if sched.is_null() {
                format!("Removed the schedule of T-{id}.")
            } else if sched.truthy("paused") {
                format!("T-{id} is paused.")
            } else {
                format!("T-{id} runs next at {}.", next.as_deref().unwrap_or("no slot"))
            })
        }
    });

    tool(t, "set_title", "Set this TODO's title: a short summary of the task.", &[("title", Ty::Str, true)], move |a| async move {
        let title = arg(&a, "title");
        let title = trim(&title);
        if title.is_empty() {
            return err("title is empty");
        }
        server::set_todo(todo_id, json!({ "title": title }), true);
        add_event(Some(todo_id), "set_title", "agent", Some(title), None);
        ok("ok")
    });

    tool(t, "set_attention", "Raise the attention flag (raised: true) when you need the user; clear it (raised: false) once their reply resolves it.",
        &[("raised", Ty::Bool, true)], move |a| async move {
        let raised = a.truthy("raised");
        server::set_todo(todo_id, json!({ "attention": if raised { 1 } else { 0 }, "attention_by": if raised { json!("agent") } else { Value::Null } }), true);
        add_event(Some(todo_id), if raised { "attention raised" } else { "attention cleared" }, "agent", None, None);
        ok("ok")
    });

    let d = dir.clone();
    tool(t, "attach_file", format!("Register a file you wrote in this TODO's artifact folder ({dir}) so the user can open it. Pass its absolute path."),
        &[("path", Ty::Str, true)], move |a| {
        let dir = d.clone();
        async move {
            let path = arg(&a, "path");
            if !is_absolute(&path) {
                return err("path must be absolute");
            }
            if !util::exists(&path) {
                return err(format!("{path} does not exist"));
            }
            let real = util::realpath(&path)?; // so a symlink cannot point the user's editor outside the folder
            if !helpers::is_inside(&real, &util::realpath(&dir)?) {
                return err(format!("{path} is outside this TODO's artifact folder {dir}"));
            }
            if !std::fs::metadata(&real)?.is_file() {
                return err(format!("{path} is not a file"));
            }
            // A file found on disk earlier has no session yet; registering it claims it for this one.
            db::run(
                "INSERT INTO files (todo_id, path, session_no) VALUES (?, ?, ?) ON CONFLICT(path) DO UPDATE SET session_no = COALESCE(session_no, excluded.session_no)",
                params![todo_id, real, server::live_session_no(Some(todo_id))],
            );
            changed(Some(todo_id));
            add_event(Some(todo_id), "attach_file", "agent", Some(&basename(&real)), None);
            ok(format!("attached as {}", basename(&real)))
        }
    });

    tool(t, "set_worktree", "Register the absolute path of the git worktree this TODO works in.", &[("path", Ty::Str, true)], move |a| async move {
        let path = arg(&a, "path");
        if !is_absolute(&path) || !util::exists(&path) || !std::fs::metadata(&path)?.is_dir() {
            return err(format!("{path} is not an existing directory (absolute path required)"));
        }
        server::set_todo(todo_id, json!({ "worktree_path": resolve(&path) }), true);
        add_event(Some(todo_id), "set_worktree", "agent", Some(&resolve(&path)), None);
        ok("ok")
    });

    tool(t, "set_pr", "Register the number of the GitHub pull request this TODO works on, in its topic's GitHub repository. The app shows its state, checks and reviews.",
        &[("number", Ty::PosInt, true)], move |a| async move {
        let number = a.i("number").unwrap_or_default();
        server::set_todo(todo_id, json!({ "pr_number": number }), true);
        add_event(Some(todo_id), "set_pr", "agent", Some(&format!("#{number}")), None);
        if let Some(pid) = row(todo_id)?.i("project_id").filter(|p| *p != 0) {
            util::spawn(server_ops::refresh_prs(pid));
        }
        ok("ok")
    });

    // Auto effort: the worker switches its own effort level as the work changes (CONTRACT's effort rules). Claude only;
    // a Claude terminal does not get it, as the level belongs to the TODO's session.
    if via.is_none() && agents::is_claude(row(todo_id)?.s("agent")) {
        tool(t, "set_effort", "Switch your effort level: low, medium, high or xhigh. reason: a few words the user reads, such as \"verifying the fix\". Refused when the user set the effort.",
            &[("level", Ty::Effort, true), ("reason", Ty::Str, true)], move |a| async move {
            let (level, reason) = (arg(&a, "level"), arg(&a, "reason"));
            let reason = if trim(&reason).is_empty() { "no reason given" } else { trim(&reason) };
            match runner::set_auto_effort(todo_id, &level, reason, "agent") {
                Some(user) => err(format!("The user set the effort to {user}. Keep it.")),
                None => ok(format!("Effort {level}.")),
            }
        });
    }

    // Item 146: the user's comments on selected text in the Files tab.
    tool(t, "list_comments", "List the user's open comments on files of this TODO: each has the file's path, the line where the selected text started (when known), the selected text (quote), the comment, and your earlier reply if the user edited the comment after it. Answer each with reply_comment.",
        &[], move |_| async move {
        let open = server::comments_of(todo_id, "state = 'open'");
        add_event(Some(todo_id), "list_comments", "agent", Some(&format!("{} open", open.len())), None);
        let list: Vec<Value> = open
            .iter()
            .map(|c| {
                let mut o = json!({ "id": c["id"], "path": c["path"], "line": c["line"], "quote": c["quote"], "comment": c["body"] });
                if c.truthy("reply") {
                    o["your_earlier_reply"] = c["reply"].clone();
                }
                o
            })
            .collect();
        json_text(&Value::Array(list))
    });

    tool(t, "reply_comment", "Reply to one of the user's comments from list_comments. You changed the file for it: start the reply with \"Applied\" and say what you changed in a few words. A question: answer it, no edit. Unclear: ask your question as the reply. The user resolves the comment or edits it to follow up.",
        &[("id", Ty::Int, true), ("reply", Ty::Str, true)], move |a| async move {
        let (id, reply) = (a.i("id").unwrap_or_default(), arg(&a, "reply"));
        let reply = trim(&reply);
        if reply.is_empty() {
            return err("reply is empty");
        }
        let Some(c) = db::get("SELECT id, path FROM comments WHERE id = ? AND todo_id = ?", params![id, todo_id]) else {
            return err(format!("this TODO has no comment {id}"));
        };
        db::run("UPDATE comments SET reply = ?, state = 'replied', replied_at = ? WHERE id = ?", params![reply, util::now(), id]);
        changed(Some(todo_id));
        add_event(Some(todo_id), "reply_comment", "agent", Some(&format!("{} · {}", basename(c.s("path").unwrap_or("")), js_slice(reply, 80))), None);
        ok("ok")
    });

    // Suggested follow-ups (pills): the TODO's agent only, a terminal has no chat turn to follow up on.
    if via.is_none() {
        tool(t, "suggest_follow_ups", "Suggest one or two follow-ups the user can send you with a click, each one line written as their instruction to you (e.g. \"Approve the PR\"). A later call replaces the earlier ones.",
            &[("items", Ty::Strs, true)], move |a| async move {
            let items: Vec<String> = a["items"].as_array().map(|v| v.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
            if let Some(e) = helpers::follow_up_error(&items) {
                return err(e);
            }
            let clean: Vec<String> = items.iter().map(|x| trim(x).to_string()).collect();
            server::set_todo(todo_id, json!({ "suggestions": serde_json::to_string(&clean)? }), false);
            add_event(Some(todo_id), "suggest", "agent", Some(&clean.join(" · ")), None);
            ok("ok")
        });
    }
    Ok(())
}

fn latest(todo_id: i64, n: i64) -> Vec<Value> {
    let mut v = db::all("SELECT author, body, committed_at AS at FROM messages WHERE todo_id = ? ORDER BY id DESC LIMIT ?", params![todo_id, n]);
    v.reverse();
    v
}

/// Logs a call: the tool's name and a detail.
type Used = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// A TODO's topic as the agents read it: its name, "none" for a TODO without a topic, null before routing (801).
fn topic_of(s: &Value) -> Value {
    match s.get("project_name") {
        Some(Value::String(n)) => json!(n),
        _ if s.truthy("routed_at") => json!("none"),
        _ => Value::Null,
    }
}
/// `s.schedule_text && { text: s.schedule_text, ...s.schedule }`
fn schedule_of(s: &Value) -> Value {
    if s.truthy("schedule_text") {
        util::merge(&json!({ "text": s["schedule_text"] }), s["schedule"].clone())
    } else {
        s.get("schedule_text").cloned().unwrap_or(Value::Null)
    }
}
fn created_by(s: &Value) -> Value {
    if s.truthy("parent_id") {
        json!(format!("T-{}", s.i("parent_id").unwrap_or_default()))
    } else {
        Value::Null
    }
}

fn register_operator_tools(t: &mut Vec<Tool>, used: Used) {
    let u = used.clone();
    tool(t, "list_todos", "List the TODOs: number (T-<id>), title, topic (null: not picked yet; \"none\": thinking only), status (not_started = in the backlog, or the Scheduled lane with a schedule), attention, branch, git state, pull request, session count, files, schedule and next run, the TODO that created it, and the latest messages. Removed TODOs only with include_removed.",
        &[("include_removed", Ty::Bool, false)], move |a| {
        let used = u.clone();
        async move {
            used("list_todos", None);
            let list = server::summaries(if a.truthy("include_removed") { "1" } else { "t.removed_at IS NULL" }, params![]).await;
            let out: Vec<Value> = list
                .iter()
                .map(|s| {
                    let id = s.i("id").unwrap_or_default();
                    json!({
                        "id": s["id"], "ref": format!("T-{id}"), "title": s["title"], "topic": topic_of(s), "status": s["status"], "attention": s["attention"],
                        "branch": s["branch"], "git": s["git"], "pr": s["pr"],
                        "worktree_path": s["worktree_path"], "sessions": s["session_count"], "queued_messages": s["queued_count"], "pending_messages": s["pending_count"],
                        "files": s["file_names"],
                        "schedule": schedule_of(s), "next_run_at": s["next_run_at"], "created_by": created_by(s),
                        "updated_at": s["updated_at"], "removed_at": s["removed_at"], "latest_messages": latest(id, 3),
                    })
                })
                .collect();
            json_text(&Value::Array(out))
        }
    });

    let u = used.clone();
    tool(t, "send_message", "Send a message to a TODO on the user's behalf. It is delivered like a message the user commits.",
        &[TODO_ID, ("body", Ty::Str, true)], move |a| {
        let used = u.clone();
        async move {
            let (id, body) = (a.i("todo_id").unwrap_or_default(), arg(&a, "body"));
            let Some(row) = todo_row(id).filter(|r| !r.truthy("removed_at")) else { return err(format!("no live TODO T-{id}")) };
            if trim(&body).is_empty() {
                return err("body is empty");
            }
            let status = if row.truthy("routing") { "not_started" } else { row.s("status").unwrap_or("") };
            let outcome = match helpers::delivery_action(status, runner::is_running(Some(id))) {
                "pending" => "stored; it is delivered when the user starts the TODO",
                "queue" => "queued; the agent gets it when its current turn ends",
                _ => "delivered now",
            };
            runner::commit_message(id, "user via operator", trim(&body), &[]);
            used("send_message", Some(&format!("T-{id}")));
            ok(format!("Message {outcome}."))
        }
    });

    let u = used;
    tool(t, "remove_todo", "Remove a TODO, only when the user asks: like the user's Remove, its run stops and it moves to the Removed fold, where the user can restore it.",
        &[TODO_ID], move |a| {
        let used = u.clone();
        async move {
            let id = a.i("todo_id").unwrap_or_default();
            if todo_row(id).filter(|r| !r.truthy("removed_at")).is_none() {
                return err(format!("no live TODO T-{id}"));
            }
            server_ops::remove_todo(id, "operator");
            used("remove_todo", Some(&format!("T-{id}")));
            ok(format!("Removed T-{id}. The user can restore it from the Removed fold."))
        }
    });
}

/// Item 145: create_todo for the operator (`parent` None), the workers and Claude terminals.
/// `used` records the call, with "· started" for the cap; `since` is when the caller's current turn began.
struct By {
    parent: Option<i64>,
    used: Arc<dyn Fn(&str) + Send + Sync>,
    since: Arc<dyn Fn() -> anyhow::Result<String> + Send + Sync>,
}

/// `${n}` for a number from `Number(...)`.
fn js_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

fn create_tool(t: &mut Vec<Tool>, by: By) {
    let op = by.parent.is_none();
    let topics = db::all("SELECT id, name FROM projects ORDER BY name", params![]);
    let list = topics.iter().filter_map(|x| x.s("name")).collect::<Vec<_>>().join(", ");
    let names = format!("{}, or \"none\"", if list.is_empty() { "(none yet)" } else { &list });
    let description = format!("Create a TODO with its first instruction. title: a short title for its card; leave it out and the TODO's agent sets one. topic: one of {names} for a thinking-only TODO; leave it out and routing picks it at the start. start: true starts it now, only when the user's words say to start it; otherwise it lands in the user's backlog. key: a dedupe key such as \"pr:copilot-service#490\"; when a TODO has that key already, removed ones too, nothing is created.")
        + " schedule: {days: [1,2,3,4,5], times: [\"09:00\"]} or {days, every: 30, from: \"08:00\", to: \"18:00\"} (days 0 = Sunday, local time) makes a scheduled TODO that runs a new session at each slot; only when the user asks for one.";
    let by = Arc::new(by);
    let (topics, names) = (Arc::new(topics), Arc::new(names));
    tool(t, "create_todo", description,
        &[("instruction", Ty::Str, true), ("title", Ty::Str, false), ("topic", Ty::Str, false), ("start", Ty::Bool, false), ("key", Ty::Str, false), ("schedule", Ty::Any, false)],
        move |a| {
        let (by, topics, names) = (by.clone(), topics.clone(), names.clone());
        async move {
            let instruction = arg(&a, "instruction");
            let go = a.truthy("start");
            if trim(&instruction).is_empty() {
                return err("instruction is empty");
            }
            // None: undefined (routing decides); Some(None): no topic.
            let mut project: Option<Option<i64>> = None;
            if let Some(topic) = a.s("topic") {
                let name = trim(topic).to_lowercase();
                let p = topics.iter().find(|x| x.s("name").unwrap_or("").to_lowercase() == name);
                if name != "none" && p.is_none() {
                    return err(format!("Unknown topic \"{topic}\". Topics: {names}."));
                }
                project = Some(p.and_then(|p| p.i("id")));
            }
            let sched = match a.get("schedule") {
                None | Some(Value::Null) => Value::Null,
                Some(s) => match helpers::schedule_in(s) {
                    Ok(s) => s,
                    Err(e) => return err(e),
                },
            };
            let k = a.s("key").map(trim).filter(|k| !k.is_empty()).map(String::from);
            let had = k.as_ref().and_then(|k| db::get("SELECT id, removed_at FROM todos WHERE key = ?", params![k]));
            if let (Some(had), Some(k)) = (had, &k) {
                let hid = had.i("id").unwrap_or_default();
                (by.used)(&format!("not created · T-{hid} has the key"));
                let removed = had.s("removed_at").map(|r| format!(" (removed {})", js_slice(r, 10))).unwrap_or_default();
                return ok(format!("Not created: T-{hid} has key {k}{removed}."));
            }
            // Number(getMeta(..)) || DEFAULT: NaN and 0 fall back.
            let cap = server::get_meta("started_todos_per_turn")
                .map(|s| if trim(&s).is_empty() { 0.0 } else { trim(&s).parse::<f64>().unwrap_or(f64::NAN) })
                .filter(|n| *n != 0.0 && !n.is_nan())
                .unwrap_or(runner::DEFAULT_STARTED_PER_TURN as f64);
            let since = (by.since)()?;
            let started = db::get(
                "SELECT COUNT(*) AS n FROM events WHERE todo_id IS ? AND kind = 'create_todo' AND detail LIKE '%· started%' AND at >= ?",
                params![by.parent, since],
            )
            .and_then(|r| r.i("n"))
            .unwrap_or(0);
            if go && started as f64 >= cap {
                return err(format!("Cap of {} started TODOs per turn reached. Create the rest with start: false, or the user raises it in Settings › Runs.", js_num(cap)));
            }
            let title = a.s("title").map(trim).filter(|t| !t.is_empty());
            let mut o = Map::new();
            o.insert("parent_id".into(), json!(by.parent));
            o.insert("key".into(), json!(k));
            if let Some(p) = project {
                o.insert("project_id".into(), json!(p));
            }
            o.insert("schedule".into(), sched.clone());
            o.insert("title".into(), json!(title));
            let id = server_ops::create_todo(trim(&instruction), if op { "operator" } else { "agent" }, &[], None, &Value::Object(o));
            if go {
                runner::start(id);
            }
            let name = project.map(|p| topics.iter().find(|x| p.is_some() && x.i("id") == p).and_then(|x| x.s("name")).unwrap_or("none").to_string());
            let scheduled = util::truthy(&sched);
            (by.used)(&joined(&[
                Some(format!("T-{id}")),
                go.then(|| "started".into()),
                scheduled.then(|| "scheduled".into()),
                name.as_ref().filter(|n| !n.is_empty()).map(|n| format!("topic {n}")),
            ]));
            let state = if go { "started" } else if scheduled { "scheduled" } else { "in the backlog" };
            let tail = match &name {
                Some(n) if !n.is_empty() => format!(" · topic {n}."),
                _ if go => "; routing picks its topic.".into(),
                _ => "; routing picks its topic when it starts.".into(),
            };
            ok(format!("Created T-{id} · {state}{tail}"))
        }
    });
}

/// read_todo and read_file, the same for the operator, its routing runs and the workers (801). `used` logs a call.
fn read_tools(t: &mut Vec<Tool>, used: Used) {
    let u = used.clone();
    tool(t, "read_todo", "Read one TODO by its number (T-42 is 42), removed ones too: its details, its sessions with their chat messages, and its file list.",
        &[TODO_ID], move |a| {
        let used = u.clone();
        async move {
            let todo_id = a.i("todo_id").unwrap_or_default();
            used("read_todo", Some(&format!("T-{todo_id}")));
            let Some(s) = server::summaries("t.id = ?", params![todo_id]).await.into_iter().next() else {
                return err(format!("no TODO T-{todo_id}"));
            };
            let messages: Vec<Value> = db::all(
                "SELECT m.id, m.author, m.body, m.state, m.committed_at AS at, m.session_no, f.path AS file
      FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.todo_id = ? ORDER BY m.id",
                params![todo_id],
            )
            .into_iter()
            // the user's uploads as absolute paths, for Read (an image opens as an image)
            .map(|mut m| {
                let id = m.as_object_mut().and_then(|o| o.shift_remove("id")).and_then(|v| v.as_i64()).unwrap_or_default();
                let paths: Vec<Value> = server::attachments_of(id).into_iter().map(|a| a["path"].clone()).collect();
                if !paths.is_empty() {
                    m["attachments"] = Value::Array(paths);
                }
                m
            })
            .collect();
            let sessions = server::sessions_of(todo_id);
            let live = sessions.last().map(|x| x["no"].clone()).unwrap_or(Value::Null);
            let label = |no: &Value| -> String {
                let body = messages
                    .iter()
                    .find(|m| &m["session_no"] == no && m.s("author").is_some_and(|a| a.starts_with("user")))
                    .and_then(|m| m.s("body"))
                    .unwrap_or("");
                body.split('\n').next().unwrap_or("").to_string()
            };
            let files: Vec<Value> = server::files_of(todo_id)
                .iter()
                .map(|f| json!({ "name": basename(f.s("path").unwrap_or("")), "path": f["path"], "session": f["session_no"] }))
                .collect();
            let sessions: Vec<Value> = sessions
                .iter()
                .map(|x| {
                    let mine: Vec<Value> = messages.iter().filter(|m| m["session_no"] == x["no"]).cloned().collect();
                    util::merge(x, json!({ "label": label(&x["no"]), "messages": mine }))
                })
                .collect();
            // pending (before the start) and queued messages wait for the live session
            let waiting: Vec<Value> = messages
                .iter()
                .filter(|m| m["session_no"].is_null() && m.s("state") != Some("delivered"))
                .map(|m| util::merge(m, json!({ "for_session": live })))
                .collect();
            json_text(&json!({
                "id": s["id"], "ref": format!("T-{}", s.i("id").unwrap_or_default()), "title": s["title"], "topic": topic_of(&s), "status": s["status"],
                "attention": s["attention"], "branch": s["branch"], "git": s["git"], "pr": s["pr"],
                "pr_checked_at": s["pr_checked_at"], "gh_error": s["gh_error"], "worktree_path": s["worktree_path"], "live_session_id": s["session_id"],
                "created_at": s["created_at"], "removed_at": s["removed_at"], "removed_by": s["removed_by"],
                "schedule": schedule_of(&s), "next_run_at": s["next_run_at"], "created_by": created_by(&s), "key": s["key"],
                "files": files,
                "sessions": sessions,
                "waiting_messages": waiting,
            }))
        }
    });

    let u = used;
    tool(t, "read_file", "Read one of a TODO's text files by its name: its attached markdown files and the user's uploads. For an image or other binary file it returns the path to open with Read.",
        &[TODO_ID, ("name", Ty::Str, true)], move |a| {
        let used = u.clone();
        async move {
            let (todo_id, name) = (a.i("todo_id").unwrap_or_default(), arg(&a, "name"));
            used("read_file", Some(&format!("T-{todo_id} {name}")));
            let uploads = db::all("SELECT a.name, a.path FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.todo_id = ? ORDER BY a.id", params![todo_id]);
            let path = server::files_of(todo_id)
                .into_iter()
                .find(|x| basename(x.s("path").unwrap_or("")) == name)
                .and_then(|x| x.s("path").map(String::from))
                .or_else(|| {
                    uploads
                        .iter()
                        .find(|a| basename(a.s("path").unwrap_or("")) == name || a.s("name") == Some(name.as_str()))
                        .and_then(|a| a.s("path").map(String::from))
                });
            let Some(path) = path else { return err(format!("T-{todo_id} has no file named {name}")) };
            if !util::exists(&path) {
                return err(format!("{path} no longer exists on disk"));
            }
            let data = std::fs::read(&path)?;
            // ponytail: a NUL byte means binary (every PNG, JPEG, PDF has one); UTF-16 text would count as binary too
            if data.contains(&0) {
                return ok(format!("{path} is not a text file. Open it with Read."));
            }
            ok(String::from_utf8_lossy(&data).into_owned())
        }
    });
}

/// The routing run's decision (801). The handlers only record it; runner.rs starts session 1 once the run ends.
fn register_router_tools(t: &mut Vec<Tool>, todo_id: i64) {
    fn topics() -> Vec<Value> {
        db::all("SELECT id, name FROM projects ORDER BY name", params![])
    }
    fn find(name: &str) -> Option<Value> {
        let all = topics();
        let lower = trim(name).to_lowercase();
        all.iter().find(|x| x.s("name") == Some(name)).or_else(|| all.iter().find(|x| x.s("name").unwrap_or("").to_lowercase() == lower)).cloned()
    }
    fn names() -> String {
        let n = topics().iter().filter_map(|x| x.s("name").map(String::from)).collect::<Vec<_>>().join(", ");
        if n.is_empty() {
            "none".into()
        } else {
            n
        }
    }
    fn decided(todo_id: i64) -> anyhow::Result<bool> {
        let r = row(todo_id)?;
        // PORT: JSON.parse threw on a broken routing column; it reads as not asking here.
        Ok(r.truthy("routed_at") || util::parse(r.s("routing").unwrap_or("{}")).s("state") == Some("asking"))
    }

    tool(t, "route_todo", "Decide which topic this TODO belongs to: a topic's name, or \"none\" for a thinking-only TODO without a topic. The reason is one short sentence the user reads. effort: the level the TODO's agent starts at (low, medium, high, xhigh); leave it out for medium. Stop after this call.",
        &[("topic", Ty::Str, true), ("reason", Ty::Str, true), ("effort", Ty::Effort, false)], move |a| async move {
        if decided(todo_id)? {
            return err("This TODO's routing is decided already. Stop now.");
        }
        let (topic, reason) = (arg(&a, "topic"), arg(&a, "reason"));
        let none = trim(&topic).to_lowercase() == "none";
        let p = if none { None } else { find(&topic) };
        if !none && p.is_none() {
            return err(format!("Unknown topic \"{topic}\". Topics: {}, or \"none\".", names()));
        }
        let pname = p.as_ref().and_then(|p| p.s("name").map(String::from));
        add_event(Some(todo_id), "route_todo", "operator", Some(&format!("topic: \"{}\"", pname.as_deref().unwrap_or("none"))), None);
        runner::route_to(todo_id, p.as_ref().and_then(|p| p.i("id")), Some(&reason));
        if let Some(effort) = a.s("effort").filter(|e| !e.is_empty()) {
            if agents::is_claude(row(todo_id)?.s("agent")) {
                runner::set_auto_effort(todo_id, effort, "picked at the start", "operator");
            }
        }
        ok(format!("Routed to {}. The app starts session 1 there when you stop.", pname.as_deref().unwrap_or("no topic")))
    });

    tool(t, "ask_topic", "Ask the user which topic this TODO belongs to, when you cannot tell: a short question and the 2 or 3 likely topics by name. The user also gets a \"No topic\" choice. Stop after this call.",
        &[("question", Ty::Str, true), ("options", Ty::Strs, true)], move |a| async move {
        if decided(todo_id)? {
            return err("This TODO's routing is decided already. Stop now.");
        }
        let question = arg(&a, "question");
        let question = trim(&question);
        if question.is_empty() {
            return err("question is empty");
        }
        let options: Vec<String> = a["options"].as_array().map(|v| v.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
        let picked: Vec<Option<Value>> = options.iter().map(|o| find(o)).collect();
        if picked.iter().any(Option::is_none) {
            return err(format!("Unknown topic in {}. Topics: {}.", serde_json::to_string(&options)?, names()));
        }
        // one per topic, in the order first named (a Map keyed by id)
        let mut opts: Vec<Value> = Vec::new();
        for p in picked.into_iter().flatten() {
            let o = json!({ "id": p["id"], "name": p["name"] });
            match opts.iter_mut().find(|x| x["id"] == p["id"]) {
                Some(x) => *x = o,
                None => opts.push(o),
            }
        }
        if opts.is_empty() || opts.len() > 3 {
            return err("Give one to three topics.");
        }
        let list = opts.iter().filter_map(|o| o.s("name")).collect::<Vec<_>>().join(", ");
        add_event(Some(todo_id), "ask_topic", "operator", Some(&list), None);
        add_message(Some(todo_id), "operator", question, "delivered", None, None);
        let routing = json!({ "state": "asking", "question": question, "options": opts });
        server::set_todo(todo_id, json!({ "routing": routing.to_string() }), true);
        ok("Asked. The user's click or reply decides; stop now.")
    });
}

// ---------- tool schemas and argument validation (zod, as the SDK used it) ----------

const MAX_SAFE: i64 = 9_007_199_254_740_991;

fn type_schema(ty: Ty) -> Value {
    match ty {
        Ty::Str => json!({ "type": "string" }),
        Ty::Bool => json!({ "type": "boolean" }),
        Ty::Int => json!({ "type": "integer", "minimum": -MAX_SAFE, "maximum": MAX_SAFE }),
        Ty::PosInt => json!({ "type": "integer", "exclusiveMinimum": 0, "maximum": MAX_SAFE }),
        Ty::Any => json!({}),
        Ty::Effort => json!({ "type": "string", "enum": helpers::AUTO_EFFORTS }),
        Ty::Strs => json!({ "type": "array", "items": { "type": "string" } }),
    }
}
/// The tool as tools/list sends it: the SDK's z4mini.toJSONSchema(schema, { target: "draft-7", io: "input" }), and
/// registerTool's `execution: { taskSupport: "forbidden" }`.
fn tool_def(t: &Tool) -> Value {
    let mut props = Map::new();
    for &(k, ty, _) in t.shape {
        props.insert(k.into(), type_schema(ty));
    }
    let mut schema = json!({ "$schema": "http://json-schema.org/draft-07/schema#", "type": "object", "properties": props });
    let required: Vec<&str> = t.shape.iter().filter(|f| f.2).map(|f| f.0).collect();
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    json!({ "name": t.name, "description": t.description, "inputSchema": schema, "execution": { "taskSupport": "forbidden" } })
}

/// zod's name for a value's type in its messages.
fn received(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}
fn invalid_type(expected: &str, v: Option<&Value>) -> String {
    format!("Invalid input: expected {expected}, received {}", received(v))
}
/// A JSON number as an integer when it is one (`3.0` is 3, as JSON.parse makes it); else its f64.
fn as_int(n: &serde_json::Number) -> Result<i128, f64> {
    if let Some(i) = n.as_i64() {
        return Ok(i as i128);
    }
    if let Some(u) = n.as_u64() {
        return Ok(u as i128);
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e30 {
        Ok(f as i128)
    } else {
        Err(f)
    }
}

/// One field: Ok(the value zod passes on) or its issues as (message, path below the field).
fn check(ty: Ty, v: Option<&Value>, issues: &mut Vec<(String, Vec<Value>)>) -> Option<Value> {
    let before = issues.len();
    let mut fail = |m: String| issues.push((m, vec![]));
    let out = match ty {
        Ty::Any => v.cloned().or_else(|| {
            fail(invalid_type("nonoptional", v));
            None
        }),
        Ty::Str => v.filter(|v| v.is_string()).cloned().or_else(|| {
            fail(invalid_type("string", v));
            None
        }),
        Ty::Bool => v.filter(|v| v.is_boolean()).cloned().or_else(|| {
            fail(invalid_type("boolean", v));
            None
        }),
        Ty::Effort => v.filter(|v| v.as_str().is_some_and(|s| helpers::AUTO_EFFORTS.contains(&s))).cloned().or_else(|| {
            let all = helpers::AUTO_EFFORTS.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join("|");
            fail(format!("Invalid option: expected one of {all}"));
            None
        }),
        Ty::Int | Ty::PosInt => match v {
            Some(Value::Number(n)) => match as_int(n) {
                // A fraction stops the checks; an integer out of the safe range does not.
                Err(f) if f.is_finite() => {
                    fail(invalid_type("int", v));
                    None
                }
                Err(f) => {
                    fail(format!("Invalid input: expected number, received {}", if f.is_nan() { "NaN" } else { "Infinity" }));
                    None
                }
                Ok(i) => {
                    if i > MAX_SAFE as i128 {
                        fail(format!("Too big: expected int to be <={MAX_SAFE}"));
                    } else if i < -MAX_SAFE as i128 {
                        fail(format!("Too small: expected int to be >={}", -MAX_SAFE));
                    }
                    if matches!(ty, Ty::PosInt) && i <= 0 {
                        fail("Too small: expected number to be >0".into());
                    }
                    Some(json!(i as i64))
                }
            },
            _ => {
                fail(invalid_type("number", v));
                None
            }
        },
        Ty::Strs => match v {
            Some(Value::Array(a)) => {
                for (i, x) in a.iter().enumerate() {
                    if !x.is_string() {
                        issues.push((invalid_type("string", Some(x)), vec![json!(i)]));
                    }
                }
                Some(Value::Array(a.clone()))
            }
            _ => {
                fail(invalid_type("array", v));
                None
            }
        },
    };
    if issues.len() > before {
        None
    } else {
        out
    }
}

/// The arguments as zod parsed them (unknown keys dropped), or the SDK's message for the issues: one line each,
/// `<message> at <path>`.
fn validate(shape: &[Field], args: &Map<String, Value>) -> Result<Value, String> {
    let mut out = Map::new();
    let mut lines = Vec::new();
    for &(k, ty, required) in shape {
        let v = args.get(k);
        if v.is_none() && !required {
            continue;
        }
        let mut issues = Vec::new();
        match check(ty, v, &mut issues) {
            Some(x) if issues.is_empty() => {
                out.insert(k.into(), x);
            }
            _ => {
                for (m, sub) in issues {
                    // getDotPath: "items[0]"
                    let path = sub.iter().fold(k.to_string(), |acc, s| match s {
                        Value::Number(n) => format!("{acc}[{n}]"),
                        other => format!("{acc}.{}", other.as_str().unwrap_or("")),
                    });
                    lines.push(format!("{m} at {path}"));
                }
            }
        }
    }
    if lines.is_empty() {
        Ok(Value::Object(out))
    } else {
        Err(lines.join("\n"))
    }
}

// ---------- the transport (WebStandardStreamableHTTPServerTransport: stateless, JSON responses) ----------

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[LATEST_PROTOCOL_VERSION, "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_BATCH_SIZE: usize = 100;
const RELATED_TASK_META_KEY: &str = "io.modelcontextprotocol/related-task";

/// A header as the Fetch API's Headers.get gives it: bytes as Latin-1, repeated headers joined with ", ".
fn header_str(h: &HeaderMap, name: &str) -> Option<String> {
    let vals: Vec<String> = h.get_all(name).iter().map(|v| v.as_bytes().iter().map(|&b| b as char).collect()).collect();
    (!vals.is_empty()).then(|| vals.join(", "))
}

/// The transport's error answer: `{ jsonrpc, error, id: null }`.
fn json_error(status: u16, code: i64, message: &str) -> Response {
    let body = json!({ "jsonrpc": "2.0", "error": { "code": code, "message": message }, "id": null });
    json_response(status, &body)
}
fn json_response(status: u16, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("response")
}

static TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+/[!#$%&'*+.^_`|~0-9A-Za-z-]+$").unwrap());
static PARAM_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"; *([!#$%&'*+.^_`|~0-9A-Za-z-]+) *= *("(?:[\x0b\x20\x21\x23-\x5b\x5d-\x7e\x{80}-\x{ff}]|\\[\x0b\x20-\x{ff}])*"|[!#$%&'*+.^_`|~0-9A-Za-z-]+) *"#).unwrap()
});
/// The `content-type` package's parse: the lowercased media type, None where it throws.
fn parse_content_type(h: &str) -> Option<String> {
    let index = h.find(';');
    let ty = trim(index.map_or(h, |i| &h[..i]));
    if !TYPE_RE.is_match(ty) {
        return None;
    }
    if let Some(mut i) = index {
        while let Some(m) = PARAM_RE.find_at(h, i) {
            if m.start() != i {
                return None;
            }
            i = m.end();
        }
        if i != h.len() {
            return None;
        }
    }
    Some(ty.to_lowercase())
}
/// The SDK's isJsonContentType: the parsed media type, with a fallback for a sloppy parameter section.
fn is_json_content_type(h: Option<&str>) -> bool {
    let Some(h) = h else { return false };
    if h == "application/json" {
        return true;
    }
    if h.is_empty() {
        return false;
    }
    let essence = match parse_content_type(h) {
        Some(t) => Some(t),
        None => {
            let e = trim(h.split(';').next().unwrap_or("")).to_lowercase();
            (!e.is_empty() && !h.get(e.len()..).unwrap_or("").contains(',')).then_some(e)
        }
    };
    essence.as_deref() == Some("application/json")
}

fn safe_int(v: &Value) -> bool {
    match v {
        Value::Number(n) => as_int(n).is_ok_and(|i| i.abs() <= MAX_SAFE as i128),
        _ => false,
    }
}
fn valid_id(v: &Value) -> bool {
    v.is_string() || safe_int(v)
}
fn valid_meta(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Object(m)) => {
            m.get("progressToken").is_none_or(valid_id)
                && m.get(RELATED_TASK_META_KEY).is_none_or(|r| r.get("taskId").is_some_and(Value::is_string) && r.is_object())
        }
        _ => false,
    }
}
fn valid_params(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Object(m)) => valid_meta(m.get("_meta")),
        _ => false,
    }
}
fn only_keys(m: &Map<String, Value>, keys: &[&str]) -> bool {
    m.keys().all(|k| keys.contains(&k.as_str()))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Request,
    Notification,
    Response,
}
/// JSONRPCMessageSchema: a request, a notification, a result or an error, each a strict object.
fn kind(v: &Value) -> Option<Kind> {
    let m = v.as_object()?;
    if m.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return None;
    }
    let method_ok = m.get("method").is_some_and(Value::is_string) && valid_params(m.get("params"));
    if method_ok && only_keys(m, &["jsonrpc", "id", "method", "params"]) && m.get("id").is_some_and(valid_id) {
        return Some(Kind::Request);
    }
    if method_ok && only_keys(m, &["jsonrpc", "method", "params"]) {
        return Some(Kind::Notification);
    }
    if only_keys(m, &["jsonrpc", "id", "result"]) && m.get("id").is_some_and(valid_id) && m.get("result").is_some_and(|r| r.is_object() && valid_meta(r.get("_meta"))) {
        return Some(Kind::Response);
    }
    let error_ok = m.get("error").and_then(Value::as_object).is_some_and(|e| e.get("code").is_some_and(safe_int) && e.get("message").is_some_and(Value::is_string));
    if only_keys(m, &["jsonrpc", "id", "error"]) && m.get("id").is_none_or(valid_id) && error_ok {
        return Some(Kind::Response);
    }
    None
}
/// An id as JSON.parse made it: `3.0` is 3.
fn norm_id(v: &Value) -> Value {
    match v {
        Value::Number(n) => as_int(n).map(|i| json!(i as i64)).unwrap_or_else(|_| v.clone()),
        other => other.clone(),
    }
}

async fn read_body(body: Body) -> Result<Option<Vec<u8>>, axum::Error> {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        if buf.len() > MAX_BODY {
            return Ok(None);
        }
    }
    Ok(Some(buf))
}

/// handlePostRequest in JSON response mode. The SDK answered GET with SSE and DELETE by closing the session, but
/// handle_mcp answers every method but POST with 405 before the transport sees it.
async fn transport(req: Request, tools: &[Tool]) -> Response {
    let headers = req.headers().clone();
    let accept = header_str(&headers, "accept");
    if !accept.as_deref().is_some_and(|a| a.contains("application/json") && a.contains("text/event-stream")) {
        return json_error(406, -32000, "Not Acceptable: Client must accept both application/json and text/event-stream");
    }
    if !is_json_content_type(header_str(&headers, "content-type").as_deref()) {
        return json_error(415, -32000, "Unsupported Media Type: Content-Type must be application/json");
    }
    let too_large = format!("Payload Too Large: Request body must not exceed {MAX_BODY} bytes");
    // Number(header) > max: a header that is not a number never refuses.
    if header_str(&headers, "content-length").and_then(|l| trim(&l).parse::<f64>().ok()).is_some_and(|l| l > MAX_BODY as f64) {
        return json_error(413, -32000, &too_large);
    }
    let raw = match read_body(req.into_body()).await {
        Ok(None) => return json_error(413, -32000, &too_large),
        Ok(Some(bytes)) => {
            // TextDecoder drops a byte order mark
            let text = String::from_utf8_lossy(&bytes);
            serde_json::from_str::<Value>(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
        }
        Err(_) => None,
    };
    let Some(raw) = raw else { return json_error(400, -32700, "Parse error: Invalid JSON") };
    let messages = match raw {
        Value::Array(a) if a.len() > MAX_BATCH_SIZE => {
            return json_error(400, -32600, &format!("Invalid Request: Batch must not exceed {MAX_BATCH_SIZE} messages"));
        }
        Value::Array(a) => a,
        other => vec![other],
    };
    let Some(kinds) = messages.iter().map(kind).collect::<Option<Vec<Kind>>>() else {
        return json_error(400, -32700, "Parse error: Invalid JSON-RPC message");
    };
    let is_init = messages.iter().zip(&kinds).any(|(m, k)| *k == Kind::Request && is_initialize_request(m));
    if is_init && messages.len() > 1 {
        return json_error(400, -32600, "Invalid Request: Only one initialization request is allowed");
    }
    if !is_init {
        if let Some(v) = header_str(&headers, "mcp-protocol-version").filter(|v| !SUPPORTED_PROTOCOL_VERSIONS.contains(&v.as_str())) {
            return json_error(400, -32000, &format!("Bad Request: Unsupported protocol version: {v} (supported versions: {})", SUPPORTED_PROTOCOL_VERSIONS.join(", ")));
        }
    }
    // Notifications (initialized, cancelled, progress) and the client's responses need no action from this server.
    // PORT: the SDK aborted a running request on notifications/cancelled for it, and then never answered the HTTP
    // request; this one answers it.
    let requests: Vec<&Value> = messages.iter().zip(&kinds).filter(|(_, k)| **k == Kind::Request).map(|(m, _)| m).collect();
    if requests.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }
    // PORT: the SDK collected answers in a Map by request id, so a batch that repeats an id got fewer answers.
    let mut answers = futures_util::future::join_all(requests.into_iter().map(|m| answer(tools, m))).await;
    let body = if answers.len() == 1 { answers.remove(0) } else { Value::Array(answers) };
    json_response(200, &body)
}

/// Request-level schema issues, as zod reports them (the SDK answered them with -32603 and the issues as JSON).
#[derive(Default)]
struct Issues(Vec<Value>);
impl Issues {
    fn expect(&mut self, path: &[&str], v: Option<&Value>, expected: &str, optional: bool) -> bool {
        let fine = match expected {
            "string" => v.is_some_and(Value::is_string),
            "number" => v.is_some_and(Value::is_number),
            "array" => v.is_some_and(Value::is_array),
            _ => v.is_some_and(Value::is_object), // object, record
        };
        if fine || (optional && v.is_none()) {
            return fine;
        }
        self.0.push(json!({ "expected": expected, "code": "invalid_type", "path": path, "message": invalid_type(expected, v) }));
        false
    }
    fn result(self) -> Result<(), String> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(serde_json::to_string_pretty(&self.0).unwrap_or_default())
        }
    }
}
/// InitializeRequestSchema.
// PORT: `capabilities` and `clientInfo.icons` are checked as an object and an array, not field by field.
fn initialize_issues(m: &Value) -> Result<(), String> {
    let mut is = Issues::default();
    let p = m.get("params");
    if is.expect(&["params"], p, "object", false) {
        is.expect(&["params", "protocolVersion"], p.and_then(|p| p.get("protocolVersion")), "string", false);
        is.expect(&["params", "capabilities"], p.and_then(|p| p.get("capabilities")), "object", false);
        let ci = p.and_then(|p| p.get("clientInfo"));
        if is.expect(&["params", "clientInfo"], ci, "object", false) {
            for (k, ty, optional) in [("name", "string", false), ("title", "string", true), ("icons", "array", true), ("version", "string", false), ("websiteUrl", "string", true), ("description", "string", true)] {
                is.expect(&["params", "clientInfo", k], ci.and_then(|c| c.get(k)), ty, optional);
            }
        }
    }
    is.result()
}
fn is_initialize_request(m: &Value) -> bool {
    m.s("method") == Some("initialize") && initialize_issues(m).is_ok()
}
/// isTaskAugmentedRequestParams(params) with a task: the client asked for a task, which this server cannot make.
fn task_requested(params: Option<&Value>) -> bool {
    params.and_then(|p| p.get("task")).is_some_and(|t| t.is_object() && t.get("ttl").is_none_or(Value::is_number))
}

/// One request's JSON-RPC answer.
async fn answer(tools: &[Tool], m: &Value) -> Value {
    let id = norm_id(&m["id"]);
    let method = m.s("method").unwrap_or("");
    let params = m.get("params");
    let fail = |code: i64, message: String| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    if !matches!(method, "initialize" | "ping" | "tools/list" | "tools/call") {
        return fail(-32601, "Method not found".into());
    }
    if task_requested(params) {
        return fail(-32603, format!("Server does not support task creation (required for {method})"));
    }
    let result = match method {
        "initialize" => initialize_issues(m).map(|_| {
            let asked = params.and_then(|p| p.s("protocolVersion")).unwrap_or("");
            let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&asked) { asked } else { LATEST_PROTOCOL_VERSION };
            json!({ "protocolVersion": version, "capabilities": { "tools": { "listChanged": true } }, "serverInfo": { "name": "app", "version": "1.0.0" } })
        }),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut is = Issues::default();
            is.expect(&["params", "cursor"], params.and_then(|p| p.get("cursor")), "string", true);
            is.result().map(|_| json!({ "tools": tools.iter().map(tool_def).collect::<Vec<_>>() }))
        }
        _ => {
            let mut is = Issues::default();
            if is.expect(&["params"], params, "object", false) {
                let task = params.and_then(|p| p.get("task"));
                if is.expect(&["params", "task"], task, "object", true) {
                    is.expect(&["params", "task", "ttl"], task.and_then(|t| t.get("ttl")), "number", true);
                }
                is.expect(&["params", "name"], params.and_then(|p| p.get("name")), "string", false);
                is.expect(&["params", "arguments"], params.and_then(|p| p.get("arguments")), "record", true);
            }
            match is.result() {
                Err(e) => Err(e),
                Ok(()) => Ok(call_tool(tools, params.and_then(|p| p.s("name")).unwrap_or(""), params.and_then(|p| p.get("arguments"))).await),
            }
        }
    };
    match result {
        Ok(r) => json!({ "result": r, "jsonrpc": "2.0", "id": id }),
        Err(message) => fail(-32603, message),
    }
}

/// tools/call: McpServer's handler. Every failure is a tool result with isError, as the SDK made it.
async fn call_tool(tools: &[Tool], name: &str, args: Option<&Value>) -> Value {
    let tool_error = |text: String| json!({ "content": [{ "type": "text", "text": text }], "isError": true });
    let Some(t) = tools.iter().find(|t| t.name == name) else {
        return tool_error(format!("MCP error -32602: Tool {name} not found"));
    };
    let empty = Map::new();
    let args = match validate(t.shape, args.and_then(Value::as_object).unwrap_or(&empty)) {
        Ok(a) => a,
        Err(e) => return tool_error(format!("MCP error -32602: Input validation error: Invalid arguments for tool {name}: {e}")),
    };
    // A panic (an SQL error, say) is a thrown error in TS terms: the agent gets its message as a tool error.
    match tokio::spawn((t.run)(args)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => tool_error(e.to_string()),
        Err(e) => {
            let msg = e.try_into_panic().ok().and_then(|p| p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())));
            tool_error(msg.unwrap_or_else(|| "Internal error".into()))
        }
    }
}

#[cfg(test)]
// The db::test_db guard is held across awaits on purpose: it keeps tests off the shared database while one runs, and
// each #[tokio::test] has its own runtime, so nothing else waits on it there.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// The rows the golden files assume (db::test_db makes them): topics Alpha (2) and beta (1), TODO 1 with the
    /// default agent and TODO 2 on codex. One grant per golden grant name.
    fn setup() -> (std::sync::MutexGuard<'static, ()>, &'static HashMap<String, String>) {
        let turn = db::test_db();
        static GRANT_SECRETS: OnceLock<HashMap<String, String>> = OnceLock::new();
        let secrets = GRANT_SECRETS.get_or_init(|| {
            let noop: OnPost = Arc::new(|| {});
            let id = |s: &str| s.to_string();
            [
                ("worker", Grant::Worker { todo_id: 1, dir: "/tmp/sb-data/1".into(), on_post: Some(noop.clone()), via: None }),
                ("worker_terminal", Grant::terminal(1, "/tmp/sb-data/1", "Claude 2")),
                ("worker_codex", Grant::Worker { todo_id: 2, dir: "/tmp/sb-data/2".into(), on_post: Some(noop), via: None }),
                ("operator", Grant::Operator),
                ("router", Grant::Router { todo_id: 1 }),
            ]
            .into_iter()
            .map(|(name, g)| (name.to_string(), grant(g, id).0))
            .collect()
        });
        (turn, secrets)
    }

    async fn send(method: &str, headers: &[(&str, &str)], body: Option<String>) -> (u16, Option<String>, String) {
        let mut b = axum::http::Request::builder().method(method).uri("http://127.0.0.1:4477/mcp/app");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = handle_mcp(b.body(Body::from(body.unwrap_or_default())).unwrap()).await;
        let status = res.status().as_u16();
        let ct = res.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, ct, String::from_utf8(bytes.to_vec()).unwrap())
    }
    fn post_headers(secret: &str) -> Vec<(&str, &str)> {
        vec![("x-switchboard-session", secret), ("accept", "application/json, text/event-stream"), ("content-type", "application/json")]
    }

    #[tokio::test]
    async fn tools_list_matches_the_sdk() {
        let (_turn, secrets) = setup();
        let golden: Value = serde_json::from_str(include_str!("../tests/apptools-tools-list.json")).unwrap();
        for (name, expected) in golden.as_object().unwrap() {
            let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();
            let (status, _, text) = send("POST", &post_headers(&secrets[name]), Some(body)).await;
            assert_eq!(status, 200, "{name}");
            let got: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(got["result"].to_string(), expected.to_string(), "tools/list for {name}");
        }
    }

    #[tokio::test]
    async fn tool_lists_name_the_granted_tools() {
        let (_turn, secrets) = setup();
        let golden: Value = serde_json::from_str(include_str!("../tests/apptools-tools-list.json")).unwrap();
        let names = |g: &str| -> Vec<String> {
            let mut v: Vec<String> = golden[g]["tools"].as_array().unwrap().iter().map(|t| format!("mcp__app__{}", t["name"].as_str().unwrap())).collect();
            v.sort();
            v
        };
        let sorted = |l: &[String]| -> Vec<String> {
            let mut v: Vec<String> = l.to_vec();
            v.sort();
            v
        };
        assert_eq!(names("worker"), sorted(&worker_tools()));
        assert_eq!(names("operator"), sorted(&operator_tools()));
        assert_eq!(names("router"), sorted(&router_tools()));
        assert!(!secrets.is_empty());
    }

    /// Every request in apptools-protocol.json, answered as the TS module answered it. A case whose tool body reaches a
    /// module that is not ported yet (a `todo!()` stub) is skipped.
    #[tokio::test]
    async fn protocol_matches_the_sdk() {
        let (_turn, secrets) = setup();
        let golden: Value = serde_json::from_str(include_str!("../tests/apptools-protocol.json")).unwrap();
        let (mut ran, mut skipped) = (0, Vec::new());
        for c in golden.as_array().unwrap() {
            let name = c.s("name").unwrap();
            let secret = &secrets[c.s("grant").unwrap()];
            let headers: Vec<(String, String)> = c["headers"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), if v == "$secret" { secret.clone() } else { v.as_str().unwrap().to_string() }))
                .collect();
            let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let (status, ct, text) = send(c.s("method").unwrap(), &h, c.s("body").map(String::from)).await;
            if text.contains("not yet implemented") {
                skipped.push(name.to_string());
                continue;
            }
            ran += 1;
            assert_eq!(status as i64, c.i("status").unwrap(), "status of {name}: {text}");
            if let Some(expected) = c.s("content_type") {
                assert_eq!(ct.as_deref(), Some(expected), "content type of {name}");
            }
            assert_eq!(text, c.s("response").unwrap(), "answer to {name}");
        }
        eprintln!("{ran} protocol cases matched; skipped (stubs): {skipped:?}");
        assert!(ran > 80);
    }

    async fn call(secret: &str, name: &str, args: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } }).to_string();
        let (status, _, text) = send("POST", &post_headers(secret), Some(body)).await;
        assert_eq!(status, 200);
        serde_json::from_str::<Value>(&text).unwrap()["result"].clone()
    }

    /// Tool bodies that reach only the database and server.rs (TODO 6 is this test's own).
    #[tokio::test]
    async fn tool_bodies_write_the_database() {
        let (_turn, secrets) = setup();
        let posted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let p = posted.clone();
        let (worker, _) = grant(
            Grant::Worker { todo_id: 6, dir: "/tmp/sb-data/6".into(), on_post: Some(Arc::new(move || p.store(true, std::sync::atomic::Ordering::SeqCst))), via: Some("Claude 1".into()) },
            |s| s.into(),
        );

        let r = call(&worker, "set_title", json!({ "title": "  Fix the flaky test " })).await;
        assert_eq!(r, json!({ "content": [{ "type": "text", "text": "ok" }] }));
        assert_eq!(todo_row(6).unwrap().s("title"), Some("Fix the flaky test"));

        call(&worker, "set_attention", json!({ "raised": true })).await;
        let t = todo_row(6).unwrap();
        assert_eq!((t.i("attention"), t.s("attention_by")), (Some(1), Some("agent")));

        let r = call(&worker, "reply_comment", json!({ "id": 3, "reply": " Applied: shorter intro " })).await;
        assert_eq!(r["content"][0]["text"], "ok");
        let c = db::get("SELECT reply, state FROM comments WHERE id = 3", params![]).unwrap();
        assert_eq!((c.s("reply"), c.s("state")), (Some("Applied: shorter intro"), Some("replied")));
        let r = call(&worker, "reply_comment", json!({ "id": 99, "reply": "x" })).await;
        assert_eq!(r["content"][0]["text"], "this TODO has no comment 99");

        // A terminal's grant has no follow-ups and no effort switch.
        let r = call(&worker, "suggest_follow_ups", json!({ "items": ["a"] })).await;
        assert_eq!(r["content"][0]["text"], "MCP error -32602: Tool suggest_follow_ups not found");

        let r = call(&worker, "create_todo", json!({ "instruction": "Review it", "key": " pr:x#1 " })).await;
        assert_eq!(r["content"][0]["text"], "Not created: T-5 has key pr:x#1 (removed 2026-10-01).");
        let r = call(&secrets["operator"], "create_todo", json!({ "instruction": "x", "topic": "Gamma" })).await;
        assert_eq!(r["content"][0]["text"], "Unknown topic \"Gamma\". Topics: Alpha, beta, or \"none\".");

        let r = call(&worker, "remove_todo", json!({ "todo_id": 2 })).await;
        assert_eq!(r["content"][0]["text"], "T-2 was not created by this TODO (T-6); only the user or the operator removes it.");
        let r = call(&worker, "remove_todo", json!({ "todo_id": 5 })).await;
        assert_eq!(r["content"][0]["text"], "no live TODO T-5");

        let ev = db::all("SELECT kind, actor, detail FROM events WHERE todo_id = 6 ORDER BY id", params![]);
        let ev: Vec<String> = ev.iter().map(|e| format!("{} {} {}", e.s("kind").unwrap(), e.s("actor").unwrap(), e.s("detail").unwrap_or("-"))).collect();
        assert_eq!(ev, ["set_title agent Fix the flaky test", "attention raised agent -", "reply_comment agent plan.md · Applied: shorter intro", "create_todo agent from Claude 1 · not created · T-5 has the key"]);
        assert!(!posted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn router_asks_once() {
        let (_turn, secrets) = setup();
        let r = call(&secrets["router"], "ask_topic", json!({ "question": " Which repo? ", "options": ["alpha", "Alpha", "beta"] })).await;
        assert_eq!(r["content"][0]["text"], "Asked. The user's click or reply decides; stop now.");
        let t = todo_row(1).unwrap();
        assert_eq!(t.s("routing"), Some(r#"{"state":"asking","question":"Which repo?","options":[{"id":2,"name":"Alpha"},{"id":1,"name":"beta"}]}"#));
        let m = db::get("SELECT author, body, state FROM messages WHERE todo_id = 1", params![]).unwrap();
        assert_eq!(m, json!({ "author": "operator", "body": "Which repo?", "state": "delivered" }));
        let r = call(&secrets["router"], "route_todo", json!({ "topic": "beta", "reason": "r" })).await;
        assert_eq!(r["content"][0]["text"], "This TODO's routing is decided already. Stop now.");
        let r = call(&secrets["router"], "ask_topic", json!({ "question": "q", "options": ["nope"] })).await;
        assert_eq!(r["content"][0]["text"], "This TODO's routing is decided already. Stop now.");
    }

    #[tokio::test]
    async fn revoked_secret_is_refused() {
        let _turn = setup();
        let (secret, server) = grant(Grant::Router { todo_id: 1 }, |s| format!("<{s}>"));
        assert_eq!(server["headers"][0]["value"], format!("<{secret}>"));
        assert_eq!(server["url"], format!("http://127.0.0.1:{}/mcp/app", util::port()));
        let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
        assert_eq!(send("POST", &post_headers(&secret), Some(ping.clone())).await.0, 200);
        revoke(&secret);
        let (status, _, text) = send("POST", &post_headers(&secret), Some(ping)).await;
        assert_eq!((status, text.as_str()), (403, "unknown session"));
    }

    #[test]
    fn content_types() {
        for (h, want) in [
            ("application/json", true),
            ("Application/JSON; charset=utf-8", true),
            ("application/json;", true),
            ("application/json; a=\"b,c\"", true),
            ("application/json; a=b,c", false),
            ("text/plain; a=application/json", false),
            ("", false),
        ] {
            assert_eq!(is_json_content_type(Some(h)), want, "{h}");
        }
    }
}

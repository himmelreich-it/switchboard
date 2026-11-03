//! Port of server.ts terminals (items 111-120), with the PTY host that was switchboard-core folded in: the PTYs, their
//! scrollback and the page's terminal WebSockets live in this process, outside the request handlers' work.
//! Kept in memory only: a terminal ends with its tab, its TODO's removal or the app (item 115).

use crate::server::{artifact_dir, emit, files_of, git_touched, project_row, sessions_of, summaries, todo_row};
use crate::util::{self, join, J};
use crate::{agents, apptools, db, fail, helpers, params, runner};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

const TERM_BUF: usize = 256 * 1024; // scrollback replayed when a page attaches
const INPUT_EVERY: Duration = Duration::from_secs(1); // typing gives the worktree a git look at most this often

struct Term {
    id: i64,
    todo_id: i64,
    kind: &'static str, // "shell" | "claude"
    root: &'static str, // "worktree" | "project"
    cwd: String,
    no: i64,
    message: String,
    title: Option<String>,
    exited: bool,
    code: Option<i64>,
    signal: Option<String>,
    secret: Option<String>, // a Claude terminal's grant for the app tools (P6), revoked when its process ends
    cols: u16,
    rows: u16,
    // the process
    gen: u64, // bumped by each spawn and kill, so a replaced process's output and exit are dropped
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pid: Option<u32>,
    buf: VecDeque<Vec<u8>>,
    buf_len: usize,
    sockets: HashMap<u64, UnboundedSender<Message>>,
    fresh: bool,
    last_input: Option<Instant>,
}

#[derive(Default)]
struct State {
    terms: HashMap<i64, Term>,
    nos: HashMap<String, i64>, // "todo:kind" -> last number; numbers only count up (Shell 1, Shell 2, ...)
    seq: i64,
    pending_kills: Vec<(Instant, Vec<u32>)>, // trees sent SIGHUP; SIGKILL follows after 5 s or at shutdown
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
static GEN: AtomicU64 = AtomicU64::new(0);
static SOCKET: AtomicU64 = AtomicU64::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The shell of a new terminal: a login shell on macOS and Linux; PowerShell (else cmd.exe) on Windows, which has none.
pub fn shell() -> String {
    if let Ok(s) = std::env::var("SWITCHBOARD_SHELL") {
        if !s.is_empty() {
            return s;
        }
    }
    if cfg!(windows) {
        util::which("pwsh").or_else(|| util::which("powershell")).or_else(|| std::env::var("COMSPEC").ok()).unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into())
    }
}
pub const LOGIN_SHELL: bool = !cfg!(windows);

fn term_out(x: &Term) -> Value {
    json!({ "id": x.id, "kind": x.kind, "root": x.root, "cwd": x.cwd, "no": x.no, "title": x.title, "exited": x.exited, "code": x.code, "signal": x.signal })
}

/// Whether the TODO has a terminal whose process still runs (Change topic refuses then).
pub fn any_running(todo_id: i64) -> bool {
    state().terms.values().any(|x| x.todo_id == todo_id && !x.exited)
}

/// Item 114: the TODO as a Claude terminal reads it, rewritten at each launch.
async fn write_context(t: &Value) -> anyhow::Result<()> {
    let id = t.i("id").unwrap_or(0);
    let s = summaries("t.id = ?", params![id]).await.into_iter().next().unwrap_or(Value::Null);
    let msgs = db::all("SELECT author, body, committed_at AS at, session_no FROM messages WHERE todo_id = ? ORDER BY id", params![id]);
    let dir = artifact_dir(id);
    let (g, pr) = (&s["git"], &s["pr"]);
    let project = match s.s("project_name") {
        Some(name) => format!("{name} (folder {})", project_row(t.i("project_id")).and_then(|p| p.s("local_path").map(String::from)).unwrap_or_default()),
        None if t.truthy("routed_at") => "none (thinking only)".into(),
        None => "not picked yet".into(),
    };
    let git = (!g.is_null()).then(|| {
        let push = match g.s("push") {
            Some("none") => Some("not pushed".to_string()),
            Some("unpushed") => Some(format!("{} unpushed", g["unpushed"])),
            Some(p) => Some(p.to_string()),
            None => None,
        };
        [
            g.i("commits").map(|c| format!("{c} commits")),
            g.i("changed").filter(|&c| c != 0).map(|c| format!("{c} changed")),
            g.i("untracked").filter(|&c| c != 0).map(|c| format!("{c} untracked")),
            push.filter(|p| !p.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    });
    let pr = if !pr.is_null() {
        Some(format!("#{} {}{} · {}", pr["number"], pr.s("state").unwrap_or(""), if pr.truthy("draft") { " (draft)" } else { "" }, pr.s("url").unwrap_or("")))
    } else {
        s.i("pr_number").map(|n| format!("#{n}"))
    };
    let files: Vec<Value> = files_of(id).into_iter().filter(|f| !f.truthy("work")).map(|f| f["path"].clone()).collect();
    let text = helpers::context_file(&json!({
        "title": s["title"], "project": project, "status": s["status"], "first": msgs.first().map(|m| m["body"].clone()).unwrap_or(json!("")),
        "worktree": s["worktree_path"], "branch": s["branch"], "git": git, "pr": pr,
        "sessions": sessions_of(id), "messages": msgs, "files": files,
    }));
    std::fs::create_dir_all(join(&dir, "terminal"))?;
    std::fs::write(join(&join(&dir, "terminal"), "context.md"), text)?;
    Ok(())
}

/// Starts the terminal's process, or a new one in place of the old (Restart): the pages see a reset, then the new output.
async fn spawn_term(id: i64) -> anyhow::Result<()> {
    let (todo_id, kind, no, message, cwd, cols, rows) = {
        let s = state();
        let x = s.terms.get(&id).ok_or_else(|| util::http_error(404, "no such terminal"))?;
        (x.todo_id, x.kind, x.no, x.message.clone(), x.cwd.clone(), x.cols, x.rows)
    };
    let t = todo_row(todo_id).ok_or_else(|| util::http_error(404, "no such TODO"))?;
    let dir = artifact_dir(todo_id);
    std::fs::create_dir_all(&dir)?;
    let mut secret = None;
    let cmd: Vec<String> = if kind == "claude" {
        let Some(claude) = agents::on_path("claude", &[]) else { fail!(400, "claude is not on PATH") };
        write_context(&t).await?;
        // closed, or restarted twice, while the context file was written
        if state().terms.get(&id).is_none_or(|x| x.pid.is_some()) {
            return Ok(());
        }
        // P6: the app tools, granted to this process and named in files rather than argv. The header is a ${VAR}
        // reference that Claude Code expands from its environment, so the secret is in neither the file nor the command line.
        let (sec, server) = apptools::grant(apptools::Grant::terminal(todo_id, &dir, &format!("Claude {no}")), |_| "${SWITCHBOARD_MCP_SECRET}".to_string());
        if let Some(x) = state().terms.get_mut(&id) {
            x.secret = Some(sec.clone());
        }
        secret = Some(sec);
        let term_dir = join(&dir, "terminal");
        let (mcp, contract) = (join(&term_dir, "mcp.json"), join(&term_dir, "contract.md"));
        let cfg = json!({ "mcpServers": { "app": { "type": "http", "url": server["url"], "headers": { "X-Switchboard-Session": "${SWITCHBOARD_MCP_SECRET}" } } } });
        std::fs::write(&mcp, serde_json::to_string_pretty(&cfg)?)?;
        std::fs::write(&contract, runner::TERMINAL_CONTRACT.replace("{artifact_dir}", &dir))?;
        // Run directly with its arguments, not through a shell, so the first message needs no quoting.
        // `--add-dir` and the other list flags take every argument after them, so `--` ends them before the first message.
        let tools: Vec<String> = apptools::worker_tools().into_iter().filter(|n| !n.ends_with("__suggest_follow_ups") && !n.ends_with("__set_effort")).collect();
        let mut c = claude;
        c.extend(["--add-dir".into(), dir.clone(), "--mcp-config".into(), mcp, "--append-system-prompt-file".into(), contract, "--allowedTools".into(), tools.join(",")]);
        if !message.is_empty() {
            c.extend(["--".into(), message]);
        }
        c
    } else if LOGIN_SHELL {
        vec![shell(), "-l".into()]
    } else {
        vec![shell()]
    };
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("SWITCHBOARD_TODO_ID".into(), todo_id.to_string());
    env.insert("SWITCHBOARD_FILES".into(), dir.clone());
    if let Some(s) = &secret {
        env.insert("SWITCHBOARD_MCP_SECRET".into(), s.clone());
    }
    env.remove("SWITCHBOARD_PARENT"); // a server started in the terminal must not take this server's stdin rule
    if let Some(x) = state().terms.get_mut(&id) {
        x.exited = false;
        x.code = None;
        x.signal = None;
        x.title = None;
    }
    if let Err(e) = start_process(id, &cmd, &cwd, env, cols, rows) {
        // e.g. the worktree was deleted after a merge: the tab shows the failure instead of a terminal with no process
        if let Some(x) = state().terms.get_mut(&id) {
            x.exited = true;
            end_grant(x);
        }
        emit(json!({ "type": "terminals", "id": todo_id }));
        fail!(400, "could not start {}: {e}", cmd[0]);
    }
    emit(json!({ "type": "terminals", "id": todo_id }));
    Ok(())
}

fn start_process(id: i64, cmd: &[String], cwd: &str, env: HashMap<String, String>, cols: u16, rows: u16) -> Result<(), String> {
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut c = CommandBuilder::new(&cmd[0]);
    c.args(&cmd[1..]);
    c.cwd(cwd);
    c.env_clear();
    for (k, v) in env {
        c.env(k, v);
    }
    let child = pair.slave.spawn_command(c).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let gen = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    {
        let mut s = state();
        let old = s.terms.get_mut(&id).and_then(stop);
        if let Some(tree) = old {
            s.pending_kills.push((Instant::now(), tree));
        }
        let Some(t) = s.terms.get_mut(&id) else { return Err("closed".into()) };
        t.gen = gen;
        t.pid = child.process_id();
        t.master = Some(pair.master);
        t.writer = Some(Arc::new(Mutex::new(writer)));
        t.buf.clear();
        t.buf_len = 0;
        for tx in t.sockets.values() {
            let _ = tx.send(Message::Binary(b"\x1bc".to_vec()));
        }
    }
    std::thread::spawn(move || read_output(id, gen, reader));
    std::thread::spawn(move || wait_exit(id, gen, child));
    Ok(())
}

fn read_output(id: i64, gen: u64, mut reader: Box<dyn Read + Send>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let data = chunk[..n].to_vec();
        let mut s = state();
        let Some(t) = s.terms.get_mut(&id).filter(|t| t.gen == gen) else { return };
        t.buf_len += data.len();
        t.buf.push_back(data.clone());
        while t.buf_len > TERM_BUF && t.buf.len() > 1 {
            t.buf_len -= t.buf.pop_front().map_or(0, |b| b.len());
        }
        if let Some(title) = osc_title(&String::from_utf8_lossy(&data)) {
            if t.title.as_deref() != Some(title.as_str()) {
                t.title = Some(title);
                emit(json!({ "type": "terminals", "id": t.todo_id }));
            }
        }
        for tx in t.sockets.values() {
            let _ = tx.send(Message::Binary(data.clone()));
        }
    }
}

// The exit code comes from the process; the PTY's end only says the stream closed.
fn wait_exit(id: i64, gen: u64, mut child: Box<dyn Child + Send + Sync>) {
    let status = child.wait();
    let (code, signal) = match &status {
        Ok(st) => match st.signal() {
            Some(desc) => (None, Some(signal_name(desc))),
            None => (Some(st.exit_code() as i64), None),
        },
        Err(_) => (None, None),
    };
    let mut s = state();
    let Some(t) = s.terms.get_mut(&id).filter(|t| t.gen == gen) else { return };
    t.exited = true;
    t.code = code;
    t.signal = signal.clone();
    t.writer = None;
    end_grant(t);
    let msg = json!({ "type": "exit", "code": code, "signal": signal }).to_string();
    for tx in t.sockets.values() {
        let _ = tx.send(Message::Text(msg.clone()));
    }
    emit(json!({ "type": "terminals", "id": t.todo_id }));
}

fn end_grant(x: &mut Term) {
    if let Some(s) = x.secret.take() {
        apptools::revoke(&s);
    }
}

// Ends the terminal's process: SIGHUP to its whole tree, as when a terminal window closes; the processes started in it
// end too, also nohup or disowned ones. The tree is collected while the PTY is still open: closing it first would hang
// up the shell, and its background jobs, now children of init, would be missed. Returned for the SIGKILL that follows.
fn stop(t: &mut Term) -> Option<Vec<u32>> {
    t.gen = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    end_grant(t);
    let tree = match t.pid.take() {
        Some(pid) if !t.exited => Some(hangup_tree(pid)), // an ended shell's pid may be reused
        _ => None,
    };
    t.writer = None;
    t.master = None;
    tree
}
fn kill_term(s: &mut State, id: i64) {
    if let Some(tree) = s.terms.get_mut(&id).and_then(stop) {
        s.pending_kills.push((Instant::now(), tree));
    }
}
fn drop_term(s: &mut State, id: i64) {
    kill_term(s, id);
    if let Some(t) = s.terms.remove(&id) {
        for tx in t.sockets.values() {
            let _ = tx.send(Message::Close(None));
        }
    }
}

// Item 156: a shell runs a foreground program when the terminal's foreground process group is not the shell's own.
// Background jobs (`cmd &`) do not count; closing the tab ends them with the shell. Unknown (Windows): busy.
fn term_busy(x: &Term) -> bool {
    if x.exited || x.pid.is_none() {
        return false;
    }
    if x.kind == "claude" || cfg!(windows) {
        return true;
    }
    #[cfg(unix)]
    {
        match x.master.as_ref().and_then(|m| m.process_group_leader()) {
            Some(fg) => x.pid != Some(fg as u32),
            None => true,
        }
    }
    #[cfg(not(unix))]
    true
}

/// Ends the TODO's terminals, or every terminal (None, at shutdown).
pub fn close_terms(todo_id: Option<i64>) {
    {
        let mut s = state();
        let ids: Vec<i64> = s.terms.values().filter(|x| todo_id.is_none_or(|t| x.todo_id == t)).map(|x| x.id).collect();
        for id in ids {
            drop_term(&mut s, id);
        }
    }
    if let Some(t) = todo_id {
        emit(json!({ "type": "terminals", "id": t }));
    }
}

/// SIGKILL to the trees sent SIGHUP at least `after` ago. Runs every second; at shutdown with Duration::ZERO.
pub fn kill_pending(after: Duration) {
    let due: Vec<Vec<u32>> = {
        let mut s = state();
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut s.pending_kills).into_iter().partition(|(at, _)| at.elapsed() >= after);
        s.pending_kills = keep;
        due.into_iter().map(|(_, tree)| tree).collect()
    };
    #[cfg(unix)]
    for tree in due {
        signal_all(&tree, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    drop(due);
}
pub fn start_reaper() {
    util::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            kill_pending(Duration::from_secs(5));
        }
    });
}

fn todo_or_404(id: i64) -> anyhow::Result<Value> {
    todo_row(id).ok_or_else(|| util::http_error(404, "no such TODO"))
}

/// /api/todos/:id/terminals[/:tid/(restart|close|busy)]: GET list, POST open, POST restart/close, GET busy.
pub async fn api(method: &str, id: i64, term_id: Option<i64>, action: Option<&str>, body: &Value) -> anyhow::Result<Value> {
    let Some(tid) = term_id else {
        if method == "GET" {
            todo_or_404(id)?;
            let s = state();
            let mut list: Vec<&Term> = s.terms.values().filter(|x| x.todo_id == id).collect();
            list.sort_by_key(|x| x.id);
            let terminals: Vec<Value> = list.into_iter().map(|x| util::merge(&term_out(x), json!({ "busy": term_busy(x) }))).collect();
            return Ok(json!({ "terminals": terminals, "unsupported": null }));
        }
        if method == "POST" {
            let t = todo_or_404(id)?;
            if t.truthy("removed_at") {
                fail!(409, "this TODO is removed");
            }
            let kind = if body.s("kind") == Some("claude") { "claude" } else { "shell" };
            let wt = t.s("worktree_path").filter(|p| util::exists(p));
            let root = if body.s("root") == Some("worktree") && wt.is_some() { "worktree" } else { "project" };
            // A TODO without a topic (not routed yet, or thinking only) opens its terminals in the artifact folder (801).
            let cwd = match root {
                "worktree" => wt.unwrap_or_default().to_string(),
                _ => project_row(t.i("project_id")).and_then(|p| p.s("local_path").map(String::from)).unwrap_or_else(|| artifact_dir(id)),
            };
            let num = |k: &str, d: i64, max: i64| body.i(k).filter(|&n| n != 0).unwrap_or(d).clamp(2, max) as u16;
            let x_id = {
                let mut s = state();
                let key = format!("{id}:{kind}");
                let no = s.nos.get(&key).copied().unwrap_or(0) + 1;
                s.nos.insert(key, no);
                s.seq += 1;
                let x_id = s.seq;
                s.terms.insert(x_id, Term {
                    id: x_id, todo_id: id, kind, root, cwd, no, message: util::str_of(body.get("message")).trim().to_string(),
                    title: None, exited: false, code: None, signal: None, secret: None, cols: num("cols", 100, 500), rows: num("rows", 30, 300),
                    gen: 0, master: None, writer: None, pid: None, buf: VecDeque::new(), buf_len: 0, sockets: HashMap::new(), fresh: false, last_input: None,
                });
                x_id
            };
            if let Err(e) = spawn_term(x_id).await {
                state().terms.remove(&x_id);
                return Err(e);
            }
            return Ok(state().terms.get(&x_id).map(term_out).unwrap_or(Value::Null));
        }
        fail!(404, "not found");
    };
    if state().terms.get(&tid).is_none_or(|x| x.todo_id != id) {
        fail!(404, "no such terminal");
    }
    match (action, method) {
        (Some("close"), "POST") => {
            drop_term(&mut state(), tid);
            emit(json!({ "type": "terminals", "id": id }));
            Ok(json!({}))
        }
        (Some("busy"), "GET") => Ok(json!({ "busy": state().terms.get(&tid).is_some_and(term_busy) })),
        (Some("restart"), "POST") => {
            kill_term(&mut state(), tid);
            spawn_term(tid).await?; // the pages' screens are reset before the new output
            Ok(state().terms.get(&tid).map(term_out).unwrap_or(Value::Null))
        }
        _ => fail!(404, "not found"),
    }
}

/// The page's terminal WebSocket (/api/todos/:id/terminals/:tid/ws), after the HTTP layer checked token and origin.
pub fn ws(upgrade: WebSocketUpgrade, todo_id: i64, term_id: i64) -> axum::response::Response {
    upgrade.on_upgrade(move |socket| serve_socket(socket, todo_id, term_id))
}

async fn serve_socket(socket: WebSocket, todo_id: i64, id: i64) {
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = unbounded_channel::<Message>();
    let sock = SOCKET.fetch_add(1, Ordering::Relaxed);
    if !attach(todo_id, id, sock, tx) {
        let close = axum::extract::ws::CloseFrame { code: 4404, reason: "no such terminal".into() };
        let _ = sink.send(Message::Close(Some(close))).await;
        return;
    }
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = sink.close().await;
    });
    while let Some(Ok(m)) = source.next().await {
        match m {
            Message::Binary(data) => input(id, &data),
            Message::Text(text) => resize(id, &text),
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(t) = state().terms.get_mut(&id) {
        t.sockets.remove(&sock);
    }
    writer.abort();
}

// Reset, then the recent output, then live output; the first resize makes full-screen programs redraw.
fn attach(todo_id: i64, id: i64, sock: u64, tx: UnboundedSender<Message>) -> bool {
    let mut s = state();
    let Some(t) = s.terms.get_mut(&id).filter(|t| t.todo_id == todo_id) else { return false };
    let _ = tx.send(Message::Binary(b"\x1bc".to_vec()));
    for b in &t.buf {
        let _ = tx.send(Message::Binary(b.clone()));
    }
    if t.exited {
        let _ = tx.send(Message::Text(json!({ "type": "exit", "code": t.code, "signal": t.signal }).to_string()));
    }
    t.fresh = true;
    t.sockets.insert(sock, tx);
    true
}

fn input(id: i64, data: &[u8]) {
    let (w, touch) = {
        let mut s = state();
        let Some(t) = s.terms.get_mut(&id) else { return };
        if t.exited {
            return;
        }
        // Typing in a TODO's terminal may run git there: its worktree gets a new look.
        let touch = t.last_input.is_none_or(|at| at.elapsed() >= INPUT_EVERY).then(|| {
            t.last_input = Some(Instant::now());
            t.todo_id
        });
        (t.writer.clone(), touch)
    };
    if let Some(todo) = touch {
        git_touched(todo);
    }
    if let Some(w) = w {
        let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.write_all(data).and_then(|_| w.flush());
    }
}

fn resize(id: i64, text: &str) {
    let c = util::parse(text);
    if c.s("type") != Some("resize") {
        return;
    }
    let cols = c.i("cols").unwrap_or(0).clamp(2, 500) as u16;
    let rows = c.i("rows").unwrap_or(0).clamp(2, 300) as u16;
    let mut s = state();
    let Some(t) = s.terms.get_mut(&id) else { return };
    let Some(m) = t.master.as_ref() else { return };
    let size = |rows| PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    // A page that attaches with the same size would not send SIGWINCH, so vim or Claude Code would not redraw.
    if t.fresh && cols == t.cols && rows == t.rows {
        let _ = m.resize(size(rows - 1));
    }
    let _ = m.resize(size(rows));
    t.fresh = false;
    t.cols = cols;
    t.rows = rows;
}

// ---------- process trees and signals ----------

#[cfg(unix)]
fn hangup_tree(pid: u32) -> Vec<u32> {
    let mut pids = vec![pid];
    if let Ok(out) = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]).output() {
        pids.extend(helpers::descendants(&String::from_utf8_lossy(&out.stdout), pid as i64).into_iter().map(|p| p as u32));
    }
    signal_all(&pids, libc::SIGHUP);
    pids
}
#[cfg(windows)]
fn hangup_tree(pid: u32) -> Vec<u32> {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x0800_0000).status();
    Vec::new()
}
#[cfg(unix)]
fn signal_all(pids: &[u32], sig: libc::c_int) {
    for &p in pids {
        unsafe { libc::kill(p as libc::pid_t, sig) };
    }
}

/// The last window title an OSC 0 or 2 sequence sets in this chunk (ESC ] 0 ; title BEL, or ST for BEL). Shells and
/// Claude Code set one. ponytail: a title split across two output chunks is missed; the next prompt sets it again.
fn osc_title(chunk: &str) -> Option<String> {
    let mut title = None;
    let mut rest = chunk;
    while let Some(i) = rest.find("\x1b]") {
        rest = &rest[i + 2..];
        let Some(body) = rest.strip_prefix("0;").or_else(|| rest.strip_prefix("2;")) else { continue };
        let Some(end) = body.find(['\x07', '\x1b']) else { break };
        let after = &body[end..];
        if after.starts_with('\x07') || after.starts_with("\x1b\\") {
            title = Some(body[..end].to_string());
        }
        rest = after;
    }
    title
}

/// portable-pty names a signal by its description ("Hangup"); the page shows the name (SIGHUP), as Bun reported it.
#[cfg(unix)]
fn signal_name(desc: &str) -> String {
    const SIGNALS: &[(&str, libc::c_int)] = &[
        ("SIGHUP", libc::SIGHUP), ("SIGINT", libc::SIGINT), ("SIGQUIT", libc::SIGQUIT), ("SIGILL", libc::SIGILL),
        ("SIGTRAP", libc::SIGTRAP), ("SIGABRT", libc::SIGABRT), ("SIGBUS", libc::SIGBUS), ("SIGFPE", libc::SIGFPE),
        ("SIGKILL", libc::SIGKILL), ("SIGUSR1", libc::SIGUSR1), ("SIGSEGV", libc::SIGSEGV), ("SIGUSR2", libc::SIGUSR2),
        ("SIGPIPE", libc::SIGPIPE), ("SIGALRM", libc::SIGALRM), ("SIGTERM", libc::SIGTERM), ("SIGXCPU", libc::SIGXCPU),
        ("SIGXFSZ", libc::SIGXFSZ),
    ];
    for &(name, sig) in SIGNALS {
        let p = unsafe { libc::strsignal(sig) };
        if !p.is_null() && unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy() == desc {
            return name.to_string();
        }
    }
    desc.to_string()
}
#[cfg(not(unix))]
fn signal_name(desc: &str) -> String {
    desc.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(osc_title("abc\x1b]0;zsh — feat-x\x07more\x1b]2;Claude Code\x1b\\").as_deref(), Some("Claude Code"));
        assert_eq!(osc_title("\x1b]1;icon\x07"), None);
        assert_eq!(osc_title("\x1b]0;cut"), None);
        assert_eq!(osc_title("plain output"), None);
    }

    #[cfg(unix)]
    #[test]
    fn signal_names() {
        let p = unsafe { libc::strsignal(libc::SIGHUP) };
        let desc = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().to_string();
        assert_eq!(signal_name(&desc), "SIGHUP");
    }
}

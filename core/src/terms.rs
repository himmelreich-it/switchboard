//! switchboard-core: the terminal host (Phase 2, step 1). server.ts starts it and keeps the TODOs, grants and routes;
//! this process owns the PTYs and serves the page's terminal WebSockets itself, so keystrokes and output never pass
//! through the Bun server's event loop.
//!
//! Control: JSON lines. stdin carries server.ts's requests, stdout this process's replies and events (see Op, emit).
//! stdin closing (the server exits or crashes) ends every terminal and this process.

use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::frame::{coding::CloseCode, CloseFrame};
use tokio_tungstenite::tungstenite::Message;

const TERM_BUF: usize = 256 * 1024; // scrollback replayed when a page attaches
const INPUT_EVERY: Duration = Duration::from_secs(1); // "input" events, for the worktree's git look

struct Term {
    todo: u64,
    gen: u64, // bumped by each spawn and kill, so a replaced process's output and exit are dropped
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    pid: Option<u32>,
    exited: Option<Value>, // the exit message, once the process ended
    title: Option<String>,
    buf: VecDeque<Vec<u8>>,
    buf_len: usize,
    sockets: HashMap<u64, UnboundedSender<Message>>,
    cols: u16,
    rows: u16,
    fresh: bool,
    last_input: Option<Instant>,
}

#[derive(Default)]
struct State {
    terms: HashMap<u64, Term>,
    token: Option<String>,
    origins: Vec<String>,
    port: u16,
    pending_kills: Vec<(Instant, Vec<u32>)>, // process trees sent SIGHUP; SIGKILL follows after 5 s or at shutdown
}

type Shared = Arc<Mutex<State>>;
static OUT: OnceLock<Mutex<std::io::Stdout>> = OnceLock::new();
static GEN: AtomicU64 = AtomicU64::new(0);
static SOCKET: AtomicU64 = AtomicU64::new(0);

fn emit(v: Value) {
    let mut out = OUT.get_or_init(|| Mutex::new(std::io::stdout())).lock().unwrap();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Op {
    Init { token: String, port: u16 },
    Spawn { req: u64, id: u64, todo: u64, cmd: Vec<String>, cwd: String, env: HashMap<String, String>, cols: u16, rows: u16 },
    Kill { id: u64 },
    Close { id: u64 },
    Busy { req: u64, id: u64 },
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 127.0.0.1");
    let port = listener.local_addr().unwrap().port();
    let state: Shared = Arc::new(Mutex::new(State { port, ..Default::default() }));
    emit(json!({ "ev": "ready", "port": port }));

    let st = state.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_socket(st.clone(), stream));
        }
    });

    let st = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            kill_pending(&st, Duration::from_secs(5));
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<Op>(&line) {
            Ok(op) => handle(&state, op),
            Err(e) => eprintln!("switchboard-core: bad request {line}: {e}"),
        }
    }
    shutdown(&state).await;
}

fn handle(state: &Shared, op: Op) {
    match op {
        Op::Init { token, port } => {
            let mut s = state.lock().unwrap();
            s.token = Some(token);
            s.origins = vec![format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
        }
        Op::Spawn { req, id, todo, cmd, cwd, env, cols, rows } => match spawn(state, id, todo, &cmd, &cwd, env, cols, rows) {
            Ok(()) => emit(json!({ "ev": "spawned", "req": req, "id": id })),
            Err(e) => emit(json!({ "ev": "error", "req": req, "message": format!("could not start {}: {e}", cmd.first().map(String::as_str).unwrap_or("")) })),
        },
        Op::Kill { id } => {
            let mut s = state.lock().unwrap();
            if let Some(t) = s.terms.get_mut(&id) {
                if let Some(tree) = stop(t) {
                    s.pending_kills.push((Instant::now(), tree));
                }
            }
        }
        Op::Close { id } => {
            let mut s = state.lock().unwrap();
            if let Some(mut t) = s.terms.remove(&id) {
                if let Some(tree) = stop(&mut t) {
                    s.pending_kills.push((Instant::now(), tree));
                }
                for tx in t.sockets.values() {
                    let _ = tx.send(Message::Close(None));
                }
            }
        }
        Op::Busy { req, id } => {
            let s = state.lock().unwrap();
            emit(json!({ "ev": "busy", "req": req, "busy": s.terms.get(&id).is_some_and(busy) }));
        }
    }
}

// Starts the terminal's process, or a new one in place of the old (Restart): the page sees a reset, then the new output.
#[allow(clippy::too_many_arguments)]
fn spawn(state: &Shared, id: u64, todo: u64, cmd: &[String], cwd: &str, env: HashMap<String, String>, cols: u16, rows: u16) -> Result<(), String> {
    let program = cmd.first().ok_or("no command")?;
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut c = CommandBuilder::new(program);
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
        let mut s = state.lock().unwrap();
        let old = s.terms.get_mut(&id).and_then(stop);
        if let Some(tree) = old {
            s.pending_kills.push((Instant::now(), tree));
        }
        let t = s.terms.entry(id).or_insert_with(|| Term {
            todo, gen, master: None, writer: None, pid: None, exited: None, title: None, buf: VecDeque::new(), buf_len: 0,
            sockets: HashMap::new(), cols, rows, fresh: false, last_input: None,
        });
        t.gen = gen;
        t.pid = child.process_id();
        t.master = Some(pair.master);
        t.writer = Some(Arc::new(Mutex::new(writer)));
        t.exited = None;
        t.title = None;
        t.buf.clear();
        t.buf_len = 0;
        t.cols = cols;
        t.rows = rows;
        for tx in t.sockets.values() {
            let _ = tx.send(Message::binary(b"\x1bc".to_vec()));
        }
    }
    let st = state.clone();
    std::thread::spawn(move || read_output(st, id, gen, reader));
    let st = state.clone();
    std::thread::spawn(move || wait_exit(st, id, gen, child));
    Ok(())
}

fn read_output(state: Shared, id: u64, gen: u64, mut reader: Box<dyn Read + Send>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let data = chunk[..n].to_vec();
        let mut s = state.lock().unwrap();
        let Some(t) = s.terms.get_mut(&id).filter(|t| t.gen == gen) else { return };
        t.buf_len += data.len();
        t.buf.push_back(data.clone());
        while t.buf_len > TERM_BUF && t.buf.len() > 1 {
            t.buf_len -= t.buf.pop_front().map_or(0, |b| b.len());
        }
        if let Some(title) = osc_title(&String::from_utf8_lossy(&data)) {
            if t.title.as_deref() != Some(title.as_str()) {
                emit(json!({ "ev": "title", "id": id, "title": title }));
                t.title = Some(title);
            }
        }
        for tx in t.sockets.values() {
            let _ = tx.send(Message::binary(data.clone()));
        }
    }
}

// The exit code comes from the process; the PTY's end only says the stream closed.
fn wait_exit(state: Shared, id: u64, gen: u64, mut child: Box<dyn Child + Send + Sync>) {
    let status = child.wait();
    let (code, signal) = match &status {
        Ok(st) => match st.signal() {
            Some(desc) => (Value::Null, json!(signal_name(desc))),
            None => (json!(st.exit_code()), Value::Null),
        },
        Err(_) => (Value::Null, Value::Null),
    };
    let mut s = state.lock().unwrap();
    let Some(t) = s.terms.get_mut(&id).filter(|t| t.gen == gen) else { return };
    let msg = json!({ "type": "exit", "code": code, "signal": signal });
    for tx in t.sockets.values() {
        let _ = tx.send(Message::text(msg.to_string()));
    }
    t.exited = Some(msg);
    t.writer = None;
    emit(json!({ "ev": "exit", "id": id, "code": code, "signal": signal }));
}

// Ends the terminal's process (SIGHUP to its whole tree, as when a terminal window closes) and drops its PTY. The
// tree is returned for the SIGKILL that follows. Its exit is not reported: the server already knows.
// The tree is collected while the PTY is still open: closing it first would hang up the shell, and its background
// jobs, now children of init, would be missed.
fn stop(t: &mut Term) -> Option<Vec<u32>> {
    t.gen = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let tree = match t.pid.take() {
        Some(pid) if t.exited.is_none() => Some(hangup_tree(pid)), // an ended process's pid may be reused
        _ => None,
    };
    t.writer = None;
    t.master = None;
    tree
}

// Item 156: a shell runs a foreground program when the terminal's foreground process group is not the shell's own.
// Background jobs (`cmd &`) do not count. Unknown (Windows): busy.
fn busy(t: &Term) -> bool {
    if t.exited.is_some() || t.pid.is_none() {
        return false;
    }
    #[cfg(unix)]
    {
        match t.master.as_ref().and_then(|m| m.process_group_leader()) {
            Some(fg) => t.pid != Some(fg as u32),
            None => true,
        }
    }
    #[cfg(not(unix))]
    true
}

#[cfg(unix)]
fn hangup_tree(pid: u32) -> Vec<u32> {
    let mut pids = vec![pid];
    if let Ok(out) = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]).output() {
        let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace().map(|x| x.parse::<u32>());
                Some((it.next()?.ok()?, it.next()?.ok()?))
            })
            .collect();
        let mut i = 0;
        while i < pids.len() {
            let p = pids[i];
            pids.extend(pairs.iter().filter(|(_, pp)| *pp == p).map(|(c, _)| *c));
            i += 1;
        }
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

// SIGKILL to the trees sent SIGHUP at least `after` ago (all of them at shutdown).
fn kill_pending(state: &Shared, after: Duration) {
    let due: Vec<Vec<u32>> = {
        let mut s = state.lock().unwrap();
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

async fn shutdown(state: &Shared) {
    {
        let mut s = state.lock().unwrap();
        let ids: Vec<u64> = s.terms.keys().copied().collect();
        for id in ids {
            let tree = s.terms.get_mut(&id).and_then(stop);
            if let Some(tree) = tree {
                s.pending_kills.push((Instant::now(), tree));
            }
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    kill_pending(state, Duration::ZERO);
    std::process::exit(0);
}

// ---------- the page's terminal WebSocket ----------

fn reject(code: StatusCode, why: &str) -> ErrorResponse {
    let mut r = ErrorResponse::new(Some(why.to_string()));
    *r.status_mut() = code;
    r
}

// /api/todos/<todo>/terminals/<id>/ws?token=…, as on the server's own port before.
fn parse_path(path: &str) -> Option<(u64, u64)> {
    let rest = path.strip_prefix("/api/todos/")?;
    let mut parts = rest.split('/');
    let todo = parts.next()?.parse().ok()?;
    (parts.next()? == "terminals").then_some(())?;
    let id = parts.next()?.parse().ok()?;
    (parts.next()? == "ws" && parts.next().is_none()).then_some((todo, id))
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn serve_socket(state: Shared, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let mut target = None;
    // The handshake callback's error type is tungstenite's own; its size is not ours to choose.
    #[allow(clippy::result_large_err)]
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let s = state.lock().unwrap();
        let header = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("");
        // Blocks DNS-rebinding pages: the Host header must name this loopback port.
        if ![format!("127.0.0.1:{}", s.port), format!("localhost:{}", s.port)].iter().any(|h| h == header("host")) {
            return Err(reject(StatusCode::FORBIDDEN, "bad host"));
        }
        // Browsers apply no CORS to WebSockets, so any website could open one; the Origin check stops websites, the token programs.
        if !s.origins.iter().any(|o| o == header("origin")) {
            return Err(reject(StatusCode::FORBIDDEN, "bad origin"));
        }
        let token = req.uri().query().unwrap_or("").split('&').find_map(|kv| kv.strip_prefix("token=")).unwrap_or("");
        if !s.token.as_deref().is_some_and(|t| same(t.as_bytes(), token.as_bytes())) {
            return Err(reject(StatusCode::UNAUTHORIZED, "open the link the server printed at start"));
        }
        let (todo, id) = parse_path(req.uri().path()).ok_or_else(|| reject(StatusCode::NOT_FOUND, "not found"))?;
        if s.terms.get(&id).map(|t| t.todo) != Some(todo) {
            return Err(reject(StatusCode::NOT_FOUND, "no such terminal"));
        }
        target = Some(id);
        Ok(resp)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else { return };
    let Some(id) = target else { return };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = unbounded_channel::<Message>();
    let sock = SOCKET.fetch_add(1, Ordering::Relaxed);
    let attached = {
        let mut s = state.lock().unwrap();
        attach(&mut s, id, sock, tx)
    };
    if !attached {
        // closed between the handshake and here
        let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(4404), reason: "no such terminal".into() }))).await;
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
            Message::Binary(data) => input(&state, id, &data),
            Message::Text(text) => resize(&state, id, &text),
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(t) = state.lock().unwrap().terms.get_mut(&id) {
        t.sockets.remove(&sock);
    }
    writer.abort();
}

// Reset, then the recent output, then live output; the first resize makes full-screen programs redraw.
fn attach(s: &mut State, id: u64, sock: u64, tx: UnboundedSender<Message>) -> bool {
    let Some(t) = s.terms.get_mut(&id) else { return false };
    let _ = tx.send(Message::binary(b"\x1bc".to_vec()));
    for b in &t.buf {
        let _ = tx.send(Message::binary(b.clone()));
    }
    if let Some(exit) = &t.exited {
        let _ = tx.send(Message::text(exit.to_string()));
    }
    t.fresh = true;
    t.sockets.insert(sock, tx);
    true
}

fn input(state: &Shared, id: u64, data: &[u8]) {
    let w = {
        let mut s = state.lock().unwrap();
        let Some(t) = s.terms.get_mut(&id) else { return };
        if t.exited.is_some() {
            return;
        }
        // Typing may run git in the worktree: the server takes a new look, at most once a second.
        if t.last_input.is_none_or(|at| at.elapsed() >= INPUT_EVERY) {
            t.last_input = Some(Instant::now());
            emit(json!({ "ev": "input", "id": id }));
        }
        t.writer.clone()
    };
    if let Some(w) = w {
        let mut w = w.lock().unwrap();
        let _ = w.write_all(data).and_then(|_| w.flush());
    }
}

fn resize(state: &Shared, id: u64, text: &str) {
    let Ok(c) = serde_json::from_str::<Value>(text) else { return };
    if c["type"] != "resize" {
        return;
    }
    let cols = c["cols"].as_i64().unwrap_or(0).clamp(2, 500) as u16;
    let rows = c["rows"].as_i64().unwrap_or(0).clamp(2, 300) as u16;
    let mut s = state.lock().unwrap();
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

// ---------- helpers ----------

// The last window title an OSC 0 or 2 sequence sets in this chunk (ESC ] 0 ; title BEL, or ST for BEL).
// ponytail: a title split across two output chunks is missed; the next prompt sets it again.
fn osc_title(chunk: &str) -> Option<String> {
    let mut title = None;
    let mut rest = chunk;
    while let Some(i) = rest.find("\x1b]") {
        rest = &rest[i + 2..];
        let Some(body) = rest.strip_prefix("0;").or_else(|| rest.strip_prefix("2;")) else { continue };
        let end = body.find(['\x07', '\x1b']);
        let Some(end) = end else { break };
        let after = &body[end..];
        if after.starts_with('\x07') || after.starts_with("\x1b\\") {
            title = Some(body[..end].to_string());
        }
        rest = &body[end..];
    }
    title
}

// portable-pty names a signal by its description ("Hangup"); the page shows the name (SIGHUP), as Bun reported it.
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
        assert_eq!(osc_title("a\x1b]0;one\x07b\x1b]2;two\x1b\\c").as_deref(), Some("two"));
        assert_eq!(osc_title("\x1b]1;icon\x07"), None);
        assert_eq!(osc_title("\x1b]0;cut"), None);
        assert_eq!(osc_title("plain"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(parse_path("/api/todos/3/terminals/7/ws"), Some((3, 7)));
        assert_eq!(parse_path("/api/todos/3/terminals/7/ws/x"), None);
        assert_eq!(parse_path("/api/todos/x/terminals/7/ws"), None);
    }

    #[cfg(unix)]
    #[test]
    fn signal_names() {
        let p = unsafe { libc::strsignal(libc::SIGHUP) };
        let desc = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().to_string();
        assert_eq!(signal_name(&desc), "SIGHUP");
    }
}

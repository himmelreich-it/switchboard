//! Port of acp.ts: the agent driver. Agent Client Protocol (ACP) over stdio, one agent process per live session.
//! Nothing about TODOs here; runner.rs decides what a session is for. See designs/acp/ (investigation, design, spike).
//!
//! PORT: acp.ts used the @agentclientprotocol/sdk client. This speaks the protocol itself: newline-delimited JSON-RPC
//! 2.0 over the agent's stdin and stdout, with the method names and PROTOCOL_VERSION of that SDK (schema/index.js).

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use crate::util;

/// The SDK's PROTOCOL_VERSION (ACP v1).
pub const PROTOCOL_VERSION: i64 = 1;

// The SDK's method names (AGENT_METHODS / CLIENT_METHODS).
const INITIALIZE: &str = "initialize";
const AUTHENTICATE: &str = "authenticate";
const SESSION_NEW: &str = "session/new";
const SESSION_LOAD: &str = "session/load";
const SESSION_RESUME: &str = "session/resume";
const SESSION_LIST: &str = "session/list";
const SESSION_PROMPT: &str = "session/prompt";
const SESSION_CANCEL: &str = "session/cancel";
const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
const SESSION_STEERING: &str = "_session/steering";
const SESSION_UPDATE: &str = "session/update";
const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
const CLAUDE_SDK_MESSAGE: &str = "_claude/sdkMessage";

/// How to start an agent: its command line and extra environment (agents.rs builds it from the registry).
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub command: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub hint: Option<String>,
}

/// session/new refused for want of a login (ACP error -32000): the agent's ways to log in, from its initialize answer.
#[derive(Debug)]
pub struct AuthRequired(pub Vec<AuthMethod>);
impl std::fmt::Display for AuthRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the agent needs a login")
    }
}
impl std::error::Error for AuthRequired {}

fn auth_methods_of(init: &Value) -> Vec<AuthMethod> {
    let methods = init.get("authMethods").and_then(Value::as_array).cloned().unwrap_or_default();
    methods
        .iter()
        .map(|m| {
            let id = js_string(m.get("id"));
            let name = match m.get("name") {
                None | Some(Value::Null) => id.clone(),
                n => js_string(n),
            };
            AuthMethod { id, name, description: m.get("description").and_then(Value::as_str).map(String::from) }
        })
        .collect()
}
/// JS `String(v)`.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.to_string(),
    }
}

/// An error answer from the agent (a JSON-RPC error object), like the SDK's RequestError.
#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Value,
}
impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RpcError {}

pub trait Handlers: Send + Sync {
    /// The `update` object of a session/update, with its `_meta`.
    fn on_update(&self, u: &Value);
    /// A raw SDK message the session asked for in `_meta.claudeCode.emitRawSDKMessages` (Claude only).
    fn on_sdk(&self, _m: &Value) {}
    /// session/request_permission's params → the `outcome` object.
    fn on_permission(&self, r: Value) -> BoxFuture<'static, Value>;
}

pub struct SessionOpts {
    pub cwd: String,
    pub additional_directories: Vec<String>,
    pub mcp_servers: Vec<Value>,
    /// the agent's own `_meta` for session/new and session/resume
    pub meta: Option<Value>,
    /// added to the agent process's environment
    pub env: HashMap<String, String>,
}

const INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// JS number formatting for the "timed out after N s" texts (`ms / 1000`).
fn secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s.fract() == 0.0 {
        format!("{}", s as i64)
    } else {
        format!("{s}")
    }
}
async fn with_timeout<T>(f: impl std::future::Future<Output = anyhow::Result<T>>, d: Duration, what: &str) -> anyhow::Result<T> {
    match tokio::time::timeout(d, f).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!("{what} timed out after {} s", secs(d))),
    }
}

// ---------- the JSON-RPC connection ----------

enum Fail {
    Rpc(RpcError),
    Closed(String),
}

#[derive(Default)]
struct State {
    config_options: Vec<Value>,
    commands: Vec<Command>,
}

struct Conn {
    /// Lines for the writer task; None once closed (dropping it ends the agent's stdin).
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, Fail>>>>,
    next_id: AtomicU64,
    /// Why the connection closed (the SDK's `signal.aborted` and its reason).
    closed: Mutex<Option<String>>,
    /// May be set later (session/load replays before the caller has a Live), and stays swappable.
    handlers: Mutex<Option<Arc<dyn Handlers>>>,
    state: Mutex<State>,
}

impl Conn {
    fn send(&self, msg: Value) -> bool {
        match &*self.out.lock().unwrap() {
            Some(tx) => tx.send(msg.to_string()).is_ok(),
            None => false,
        }
    }
    fn is_closed(&self) -> bool {
        self.closed.lock().unwrap().is_some()
    }
    /// Closes the connection and rejects pending requests with `reason`.
    fn close(&self, reason: Option<String>) {
        {
            let mut c = self.closed.lock().unwrap();
            if c.is_some() {
                return;
            }
            *c = Some(reason.unwrap_or_else(|| "ACP connection closed".into()));
        }
        self.out.lock().unwrap().take();
        let msg = self.closed.lock().unwrap().clone().unwrap_or_default();
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(Fail::Closed(msg.clone())));
        }
    }
    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            // The pending entry goes in under the lock close() drains with, so a close cannot miss it.
            let mut p = self.pending.lock().unwrap();
            if let Some(reason) = self.closed.lock().unwrap().clone() {
                anyhow::bail!(reason);
            }
            p.insert(id, tx);
        }
        if !self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })) {
            self.pending.lock().unwrap().remove(&id);
            anyhow::bail!(self.closed.lock().unwrap().clone().unwrap_or_else(|| "ACP connection closed".into()));
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(Fail::Rpc(e))) => Err(e.into()),
            Ok(Err(Fail::Closed(m))) => Err(anyhow::anyhow!(m)),
            Err(_) => Err(anyhow::anyhow!("ACP connection closed")),
        }
    }
    fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// One message from the agent: a response to one of ours, a request, or a notification.
    fn incoming(self: &Arc<Self>, msg: Value) {
        let method = msg.get("method").and_then(Value::as_str).map(String::from);
        let id = msg.get("id").cloned().filter(|i| !i.is_null());
        match (method, id) {
            (Some(method), Some(id)) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                if method == SESSION_REQUEST_PERMISSION {
                    let h = self.handlers.lock().unwrap().clone();
                    let me = self.clone();
                    tokio::spawn(async move {
                        let outcome = match h {
                            Some(h) => h.on_permission(params).await,
                            None => json!({ "outcome": "cancelled" }),
                        };
                        me.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }));
                    });
                } else {
                    self.send(json!({ "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("\"Method not found\": {method}"), "data": { "method": method } } }));
                }
            }
            (Some(method), None) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                if method == SESSION_UPDATE {
                    let u = params.get("update").cloned().unwrap_or(Value::Null);
                    self.track(&u);
                    let h = self.handlers.lock().unwrap().clone();
                    if let Some(h) = h {
                        h.on_update(&u);
                    }
                } else if method == CLAUDE_SDK_MESSAGE {
                    let h = self.handlers.lock().unwrap().clone();
                    if let Some(h) = h {
                        h.on_sdk(params.get("message").unwrap_or(&Value::Null));
                    }
                }
            }
            (None, Some(id)) => {
                let Some(n) = id.as_u64() else { return };
                let Some(tx) = self.pending.lock().unwrap().remove(&n) else { return };
                let r = match msg.get("error") {
                    Some(e) if !e.is_null() => Err(Fail::Rpc(RpcError {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                        data: e.get("data").cloned().unwrap_or(Value::Null),
                    })),
                    _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(r);
            }
            (None, None) => {}
        }
    }

    /// Config options, the mode and the commands, as the agent last reported them; before the handler sees the update.
    fn track(&self, u: &Value) {
        let mut s = self.state.lock().unwrap();
        match u.get("sessionUpdate").and_then(Value::as_str) {
            Some("config_option_update") => {
                if let Some(a) = u.get("configOptions").and_then(Value::as_array) {
                    s.config_options = a.clone();
                }
            }
            Some("current_mode_update") => {
                let mode = u.get("currentModeId").cloned().unwrap_or(Value::Null);
                for o in s.config_options.iter_mut() {
                    if o.get("id").and_then(Value::as_str) == Some("mode") {
                        if let Some(m) = o.as_object_mut() {
                            m.insert("currentValue".into(), mode.clone());
                        }
                    }
                }
            }
            Some("available_commands_update") => {
                if let Some(a) = u.get("availableCommands").and_then(Value::as_array) {
                    s.commands = a
                        .iter()
                        .map(|c| Command {
                            name: js_string(c.get("name")),
                            description: match c.get("description") {
                                None | Some(Value::Null) => String::new(),
                                d => js_string(d),
                            },
                            hint: c.pointer("/input/hint").and_then(Value::as_str).map(String::from),
                        })
                        .collect();
                }
            }
            _ => {}
        }
    }
}

// ---------- the agent process ----------

struct Agent {
    conn: Arc<Conn>,
    init: Value,
    log: Arc<Mutex<String>>,
    /// The exit code once the process has exited.
    exit: watch::Receiver<Option<i64>>,
    pid: Option<u32>,
    kill: mpsc::UnboundedSender<()>,
}

impl Agent {
    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }
    /// `proc.kill()`: SIGTERM to the agent process itself.
    fn kill(&self) {
        let _ = self.kill.send(());
    }
    fn exit_code(&self) -> Option<i64> {
        *self.exit.borrow()
    }
}

fn last_line(log: &str) -> String {
    log.trim().split('\n').next_back().unwrap_or("").chars().take(300).collect()
}

/// Keeps the last `n` chars of `s`.
fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

/// One agent process and its ACP connection, initialized.
async fn spawn_agent(spec: &Spec, h: Option<Arc<dyn Handlers>>, extra_env: &HashMap<String, String>) -> anyhow::Result<Agent> {
    let Some(program) = spec.command.first() else { anyhow::bail!("could not run : no command") };
    let mut c = tokio::process::Command::new(program);
    c.args(&spec.command[1..])
        .envs(&spec.env)
        .envs(extra_env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false);
    util::no_window(&mut c);
    let mut child = c.spawn().map_err(|e| anyhow::anyhow!("could not run {program}: {e}"))?;
    let pid = child.id();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    let log = Arc::new(Mutex::new(String::new()));
    {
        let log = log.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut l = log.lock().unwrap();
                *l = tail(&format!("{}{}", l, String::from_utf8_lossy(&buf[..n])), 8000);
            }
        });
    }

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
        // The sender is gone (the connection closed): dropping stdin ends the agent's input.
    });

    let conn = Arc::new(Conn {
        out: Mutex::new(Some(out_tx)),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(0),
        closed: Mutex::new(None),
        handlers: Mutex::new(h),
        state: Mutex::new(State::default()),
    });

    {
        let conn = conn.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                // PORT: the SDK closes the connection on a line that is not JSON; this skips it.
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    conn.incoming(msg);
                }
            }
            conn.close(None);
        });
    }

    let (exit_tx, exit_rx) = watch::channel(None);
    let (kill_tx, mut kill_rx) = mpsc::unbounded_channel::<()>();
    {
        let conn = conn.clone();
        let log = log.clone();
        tokio::spawn(async move {
            let status = loop {
                tokio::select! {
                    s = child.wait() => break s,
                    Some(()) = kill_rx.recv() => {
                        // The waiter owns the child, so the pid cannot have been reused yet.
                        #[cfg(unix)]
                        if let Some(p) = pid {
                            unsafe { libc::kill(p as i32, libc::SIGTERM) };
                        }
                        #[cfg(not(unix))]
                        let _ = child.start_kill();
                    }
                }
            };
            let code = status.ok().map(exit_code).unwrap_or(-1);
            let l = log.lock().unwrap().clone();
            conn.close(Some(format!("the agent process exited (code {code}){}", if l.is_empty() { String::new() } else { format!(": {}", last_line(&l)) })));
            let _ = exit_tx.send(Some(code));
        });
    }

    let init = with_timeout(
        conn.request(
            INITIALIZE,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                "clientInfo": { "name": "switchboard", "version": "1" },
            }),
        ),
        INIT_TIMEOUT,
        "starting the agent",
    )
    .await;
    let a = Agent { conn, init: Value::Null, log, exit: exit_rx, pid, kill: kill_tx };
    match init {
        Ok(init) => Ok(Agent { init, ..a }),
        Err(e) => {
            a.kill();
            Err(e)
        }
    }
}

/// PORT: Bun's `proc.exited` gives the exit code; a process ended by a signal has none here, so it is 128 + the signal,
/// as a shell reports it.
fn exit_code(s: std::process::ExitStatus) -> i64 {
    if let Some(c) = s.code() {
        return c as i64;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return 128 + sig as i64;
        }
    }
    -1
}

// ---------- ending process trees ----------

// PORT: copy of helpers::descendants until integration
/// The processes below `root` in `ps -A -o pid=,ppid=` output (lines "pid ppid"), children before grandchildren.
fn descendants(ps: &str, root: i64) -> Vec<i64> {
    let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
    for line in ps.split('\n') {
        let mut it = line.split_whitespace().map(|x| x.parse::<i64>().ok());
        let (pid, ppid) = (it.next().flatten(), it.next().flatten());
        if let (Some(pid), Some(ppid)) = (pid, ppid) {
            if pid != 0 {
                kids.entry(ppid).or_default().push(pid);
            }
        }
    }
    let mut out: Vec<i64> = Vec::new();
    let mut i: isize = -1;
    while i < out.len() as isize {
        let parent = if i < 0 { root } else { out[i as usize] };
        for &k in kids.get(&parent).map(Vec::as_slice).unwrap_or(&[]) {
            if k != root && !out.contains(&k) {
                out.push(k);
            }
        }
        i += 1;
    }
    out
}

/// The SIGKILLs kill_tree has not sent yet, by a key. The server's shutdown sends them at once: its exit would drop
/// the timers.
static PENDING_KILLS: LazyLock<Mutex<HashMap<u64, Vec<i32>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_KILL: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn send_signal(pids: &[i32], signal: i32) {
    for &p in pids {
        unsafe { libc::kill(p, signal) };
    }
}

/// Ends a process and every process below it: `signal` now, SIGKILL 5 s later to the ones still there. The tree is
/// read first, because a shell's jobs, nohup or disowned commands and Claude Code's background shells (item 136) each
/// run in their own process group, and move to pid 1 once their parent ends. Windows has no signals: taskkill /T /F.
/// ponytail: a daemon that already left the tree (parent pid 1) is missed; the SIGKILL goes to the pids read at the start.
pub fn kill_tree(pid: u32, signal: i32) {
    #[cfg(windows)]
    {
        let _ = signal;
        let p = pid.to_string();
        util::sh_sync(&["taskkill", "/PID", &p, "/T", "/F"]);
    }
    #[cfg(unix)]
    {
        let mut pids = vec![pid as i32];
        let ps = util::sh_sync(&["/bin/ps", "-A", "-o", "pid=,ppid="]);
        if ps.code.is_some() {
            pids.extend(descendants(&ps.out, pid as i64).into_iter().map(|p| p as i32));
        }
        send_signal(&pids, signal);
        let key = NEXT_KILL.fetch_add(1, Ordering::Relaxed);
        PENDING_KILLS.lock().unwrap().insert(key, pids);
        let kill = move || {
            if let Some(pids) = PENDING_KILLS.lock().unwrap().remove(&key) {
                send_signal(&pids, libc::SIGKILL);
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    kill();
                });
            }
            Err(_) => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(5));
                    kill();
                });
            }
        }
    }
}

/// Sends the pending SIGKILLs now.
pub fn kill_now() {
    let all: Vec<Vec<i32>> = PENDING_KILLS.lock().unwrap().drain().map(|(_, v)| v).collect();
    #[cfg(unix)]
    for pids in all {
        send_signal(&pids, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = all;
}

// ---------- a live session ----------

struct LiveInner {
    a: Agent,
    session_id: String,
}

#[derive(Clone)]
pub struct Live(Arc<LiveInner>);

impl Live {
    fn new(a: Agent, session_id: String, config_options: Option<&Value>) -> Live {
        if let Some(Value::Array(o)) = config_options {
            a.conn.state.lock().unwrap().config_options = o.clone();
        }
        Live(Arc::new(LiveInner { a, session_id }))
    }

    pub fn session_id(&self) -> String {
        self.0.session_id.clone()
    }
    /// The session's config options (mode, model, effort, …) as the agent last reported them: from session/new or
    /// session/resume, a set_config_option answer, or a config_option_update. A model switch rewrites the others.
    pub fn config_options(&self) -> Vec<Value> {
        self.0.a.conn.state.lock().unwrap().config_options.clone()
    }
    /// The agent's slash commands, from its last available_commands_update (empty until the first one).
    pub fn commands(&self) -> Vec<Command> {
        self.0.a.conn.state.lock().unwrap().commands.clone()
    }
    /// Swaps the handlers, so the same process serves the next turn's run.
    pub fn set_handlers(&self, h: Arc<dyn Handlers>) {
        *self.0.a.conn.handlers.lock().unwrap() = Some(h);
    }

    /// Resolves when the turn ends: the prompt response is the end of the turn (ACP v1). Fails on a protocol error or
    /// exit. Returns the stopReason.
    pub async fn prompt(&self, text: &str) -> anyhow::Result<String> {
        let a = &self.0.a;
        let r = a.conn.request(SESSION_PROMPT, json!({ "sessionId": self.0.session_id, "prompt": [{ "type": "text", "text": text }] })).await;
        match r {
            Ok(r) => Ok(r.get("stopReason").and_then(Value::as_str).unwrap_or("").to_string()),
            Err(e) => {
                // The connection can close on the agent's stdout ending before the exit is seen: name the exit, not
                // the closed connection.
                if !a.conn.is_closed() {
                    return Err(e);
                }
                let mut rx = a.exit.clone();
                let code = tokio::time::timeout(Duration::from_secs(1), rx.wait_for(|c| c.is_some())).await.ok().and_then(|r| r.ok().and_then(|c| *c));
                let log = a.log();
                anyhow::bail!(
                    "the agent process {}{}",
                    match code {
                        None => "stopped answering".to_string(),
                        Some(c) => format!("exited (code {c})"),
                    },
                    if log.is_empty() { String::new() } else { format!(": {}", last_line(&log)) }
                )
            }
        }
    }

    /// Hands a message to the running turn (`_session/steering`, item 20). True when the turn took it; false when no
    /// turn runs or the agent cannot steer, and the message should go out as the next prompt.
    pub async fn steer(&self, text: &str) -> anyhow::Result<bool> {
        if !util::truthy(self.0.a.init.pointer("/_meta/steering/supported").unwrap_or(&Value::Null)) {
            return Ok(false);
        }
        let r = self
            .0
            .a
            .conn
            .request(
                SESSION_STEERING,
                json!({ "sessionId": self.0.session_id, "prompt": [{ "type": "text", "text": text }], "_meta": { "steering": { "idleBehavior": "promptRequired" } } }),
            )
            .await?;
        Ok(r.get("outcome").and_then(Value::as_str) == Some("injected"))
    }

    pub fn cancel(&self) {
        self.0.a.conn.notify(SESSION_CANCEL, json!({ "sessionId": self.0.session_id }));
    }

    pub async fn set_config(&self, config_id: &str, value: &str) -> anyhow::Result<()> {
        let r = self.0.a.conn.request(SESSION_SET_CONFIG_OPTION, json!({ "sessionId": self.0.session_id, "configId": config_id, "value": value })).await?;
        if let Some(Value::Array(o)) = r.get("configOptions") {
            self.0.a.conn.state.lock().unwrap().config_options = o.clone();
        }
        Ok(())
    }

    /// SIGTERM, so the agent can stop the processes it started; SIGKILL if it is still there 5 s later.
    pub fn close(&self) {
        let a = &self.0.a;
        if a.exit_code().is_some() {
            return;
        }
        // Before the connection closes: the agent may exit on it, and its children would no longer be found below it.
        if let Some(pid) = a.pid {
            #[cfg(unix)]
            kill_tree(pid, libc::SIGTERM);
            #[cfg(not(unix))]
            kill_tree(pid, 15);
        }
        a.conn.close(None);
    }

    /// Dead as soon as either its process exited or its connection closed, whichever is seen first.
    pub fn alive(&self) -> bool {
        self.0.a.exit_code().is_none() && !self.0.a.conn.is_closed()
    }

    /// Resolves once the agent process has exited, for whatever reason.
    pub async fn exited(&self) {
        let mut rx = self.0.a.exit.clone();
        let _ = rx.wait_for(|c| c.is_some()).await;
    }

    /// The agent's last log lines, for error events.
    pub fn stderr(&self) -> String {
        self.0.a.log()
    }

    pub fn pid(&self) -> Option<u32> {
        self.0.a.pid
    }
}

fn session_params(session_id: Option<&str>, o: &SessionOpts) -> Value {
    let mut p = serde_json::Map::new();
    if let Some(id) = session_id {
        p.insert("sessionId".into(), json!(id));
    }
    p.insert("cwd".into(), json!(o.cwd));
    p.insert("mcpServers".into(), json!(o.mcp_servers));
    p.insert("additionalDirectories".into(), json!(o.additional_directories));
    // `_meta: undefined` is left out of the JSON, as JSON.stringify did.
    if let Some(m) = &o.meta {
        p.insert("_meta".into(), m.clone());
    }
    Value::Object(p)
}

fn auth_or(e: anyhow::Error, init: &Value) -> anyhow::Error {
    if e.downcast_ref::<RpcError>().is_some_and(|r| r.code == -32000) {
        return AuthRequired(auth_methods_of(init)).into();
    }
    e
}

/// session/new in a new agent process. The agent makes the session id; for Claude it is the Claude Code session id.
pub async fn open_session(spec: &Spec, o: SessionOpts, h: Arc<dyn Handlers>) -> anyhow::Result<Live> {
    let a = spawn_agent(spec, Some(h), &o.env).await?;
    match a.conn.request(SESSION_NEW, session_params(None, &o)).await {
        Ok(s) => {
            let id = s.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
            Ok(Live::new(a, id, s.get("configOptions")))
        }
        Err(e) => {
            a.kill();
            Err(auth_or(e, &a.init))
        }
    }
}

/// An existing session in a new agent process: session/resume, or session/load for agents without resume (its
/// replayed history is dropped: the app keeps its own record). MCP servers and `_meta` are given again, so they may
/// differ.
pub async fn reopen_session(spec: &Spec, session_id: &str, o: SessionOpts, h: Arc<dyn Handlers>) -> anyhow::Result<Live> {
    let a = spawn_agent(spec, None, &o.env).await?;
    let caps = a.init.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    let params = session_params(Some(session_id), &o);
    let r = if util::truthy(caps.pointer("/sessionCapabilities/resume").unwrap_or(&Value::Null)) {
        a.conn.request(SESSION_RESUME, params).await
    } else if util::truthy(caps.get("loadSession").unwrap_or(&Value::Null)) {
        a.conn.request(SESSION_LOAD, params).await
    } else {
        Err(anyhow::anyhow!("this agent cannot resume sessions"))
    };
    match r {
        Ok(s) => {
            *a.conn.handlers.lock().unwrap() = Some(h);
            Ok(Live::new(a, session_id.to_string(), s.get("configOptions")))
        }
        Err(e) => {
            a.kill();
            Err(auth_or(e, &a.init))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Described {
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(rename = "authMethods")]
    pub auth_methods: Vec<AuthMethod>,
}

/// What an agent says about itself in initialize (Settings › Agents): its name and version, and its ways to log in.
pub async fn describe(spec: &Spec) -> anyhow::Result<Described> {
    let a = spawn_agent(spec, None, &HashMap::new()).await?;
    a.kill();
    let info = a.init.get("agentInfo").cloned().unwrap_or(Value::Null);
    let s = |k: &str| info.get(k).and_then(Value::as_str).map(String::from);
    Ok(Described { name: s("title").or_else(|| s("name")), version: s("version"), auth_methods: auth_methods_of(&a.init) })
}

/// ACP authenticate in a short-lived agent process. The agent does the login itself (codex-acp opens the browser for
/// ChatGPT and stores the login in ~/.codex/, P5 probe); this resolves when it is done, false when the agent reports
/// a failure. TS's default timeout was 5 minutes.
pub async fn authenticate(spec: &Spec, method_id: &str, timeout: Duration) -> anyhow::Result<bool> {
    let a = spawn_agent(spec, None, &HashMap::new()).await?;
    let r = with_timeout(a.conn.request(AUTHENTICATE, json!({ "methodId": method_id })), timeout, "the login").await;
    a.kill();
    Ok(r? != Value::Bool(false))
}

/// Whether the agent still has the session (the restore check). An agent that cannot list sessions gets the benefit
/// of the doubt: the resume itself then reports the problem.
pub async fn session_exists(spec: &Spec, cwd: &str, session_id: &str) -> anyhow::Result<bool> {
    let a = spawn_agent(spec, None, &HashMap::new()).await?;
    let r = async {
        if !util::truthy(a.init.pointer("/agentCapabilities/sessionCapabilities/list").unwrap_or(&Value::Null)) {
            return Ok(true);
        }
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let mut p = json!({ "cwd": cwd });
            if let Some(c) = &cursor {
                p["cursor"] = json!(c);
            }
            let r = a.conn.request(SESSION_LIST, p).await?;
            if r.get("sessions").and_then(Value::as_array).is_some_and(|s| s.iter().any(|s| s.get("sessionId").and_then(Value::as_str) == Some(session_id))) {
                return Ok(true);
            }
            cursor = r.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(String::from);
            if cursor.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }
    .await;
    a.kill();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn mock(env: &[(&str, &str)]) -> Spec {
        let bun = util::which("bun").expect("bun on PATH");
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mock-acp-agent.js");
        Spec { command: vec![bun, script.into()], env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }
    fn opts() -> SessionOpts {
        SessionOpts { cwd: "/tmp".into(), additional_directories: vec!["/var".into()], mcp_servers: vec![], meta: Some(json!({ "x": 1 })), env: HashMap::new() }
    }

    #[derive(Default)]
    struct Rec {
        updates: Mutex<Vec<Value>>,
        sdk: Mutex<Vec<Value>>,
        perms: Mutex<Vec<Value>>,
        mode_seen_tracked: AtomicBool,
        live: Mutex<Option<Live>>,
    }
    struct H(Arc<Rec>);
    impl Handlers for H {
        fn on_update(&self, u: &Value) {
            if u["sessionUpdate"] == "current_mode_update" {
                // Tracked before the handler sees the update.
                if let Some(l) = &*self.0.live.lock().unwrap() {
                    let mode = l.config_options().into_iter().find(|o| o["id"] == "mode").unwrap();
                    self.0.mode_seen_tracked.store(mode["currentValue"] == "plan", Ordering::SeqCst);
                }
            }
            self.0.updates.lock().unwrap().push(u.clone());
        }
        fn on_sdk(&self, m: &Value) {
            self.0.sdk.lock().unwrap().push(m.clone());
        }
        fn on_permission(&self, r: Value) -> BoxFuture<'static, Value> {
            self.0.perms.lock().unwrap().push(r);
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                json!({ "outcome": "selected", "optionId": "allow" })
            })
        }
    }

    #[tokio::test]
    async fn session_round_trip() {
        let rec = Arc::new(Rec::default());
        let live = open_session(&mock(&[]), opts(), Arc::new(H(rec.clone()))).await.unwrap();
        *rec.live.lock().unwrap() = Some(live.clone());
        assert_eq!(live.session_id(), "s1");
        assert!(live.alive());
        assert!(live.pid().is_some());
        assert_eq!(live.config_options().len(), 2);

        assert_eq!(live.prompt("hello").await.unwrap(), "end_turn");
        let ups = rec.updates.lock().unwrap().clone();
        assert!(ups.iter().any(|u| u["sessionUpdate"] == "available_commands_update"));
        let chunk = ups.iter().find(|u| u["sessionUpdate"] == "agent_message_chunk").unwrap();
        assert_eq!(chunk["content"]["text"], "echo: hello");
        assert_eq!(chunk["_meta"]["claudeCode"]["parentToolUseId"], "t0");
        assert!(rec.mode_seen_tracked.load(Ordering::SeqCst));
        let tool = ups.iter().find(|u| u["sessionUpdate"] == "tool_call_update").unwrap();
        assert_eq!(tool["rawOutput"]["permission"], json!({ "outcome": { "outcome": "selected", "optionId": "allow" } }));
        assert_eq!(tool["rawOutput"]["unknown"]["code"], -32601);
        assert_eq!(rec.perms.lock().unwrap()[0]["toolCall"]["toolCallId"], "t1");
        assert_eq!(rec.sdk.lock().unwrap()[0], json!({ "type": "system", "subtype": "mock" }));

        let cmds = live.commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].hint.as_deref(), Some("what to review"));
        assert_eq!(cmds[1].description, "");

        // No turn runs: steering is refused. During a turn it goes in; cancel ends the turn.
        assert!(!live.steer("more").await.unwrap());
        let turn = tokio::spawn({
            let l = live.clone();
            async move { l.prompt("wait").await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(live.steer("more").await.unwrap());
        live.cancel();
        assert_eq!(turn.await.unwrap().unwrap(), "cancelled");

        live.set_config("mode", "default").await.unwrap();
        let mode = live.config_options().into_iter().find(|o| o["id"] == "mode").unwrap();
        assert_eq!(mode["currentValue"], "default");

        // A child of the agent is ended with it.
        let r = live.prompt("tree").await.unwrap();
        let child: i32 = r.trim_start_matches("pid ").parse().unwrap();
        live.close();
        tokio::time::timeout(Duration::from_secs(5), live.exited()).await.unwrap();
        live.exited().await;
        assert!(!live.alive());
        tokio::time::sleep(Duration::from_millis(200)).await;
        let stat = std::fs::read_to_string(format!("/proc/{child}/stat")).unwrap_or_default();
        assert!(stat.is_empty() || stat.contains(") Z "), "the agent's child still runs: {stat}");
        assert!(live.prompt("again").await.is_err());
    }

    #[tokio::test]
    async fn exit_mid_turn_names_the_exit() {
        let live = open_session(&mock(&[]), opts(), Arc::new(H(Arc::default()))).await.unwrap();
        let e = live.prompt("die").await.unwrap_err().to_string();
        assert_eq!(e, "the agent process exited (code 3): boom: the mock died");
        assert!(live.stderr().contains("first line"));
    }

    #[tokio::test]
    async fn auth_required() {
        let e = open_session(&mock(&[("MOCK_AUTH_FAIL", "1")]), opts(), Arc::new(H(Arc::default()))).await.err().unwrap();
        let a = e.downcast_ref::<AuthRequired>().expect("AuthRequired");
        assert_eq!(e.to_string(), "the agent needs a login");
        assert_eq!(a.0[0].id, "mock-login");
        assert_eq!(a.0[0].description.as_deref(), Some("pretend to log in"));
    }

    #[tokio::test]
    async fn reopen_and_list() {
        let spec = mock(&[]);
        let live = reopen_session(&spec, "old", opts(), Arc::new(H(Arc::default()))).await.unwrap();
        assert_eq!(live.session_id(), "old");
        assert_eq!(live.config_options()[0]["currentValue"], "plan");
        live.close();
        live.exited().await;

        assert!(session_exists(&spec, "/tmp", "s1").await.unwrap());
        assert!(!session_exists(&spec, "/tmp", "nope").await.unwrap());
        assert!(authenticate(&spec, "mock-login", Duration::from_secs(5)).await.unwrap());
        assert!(!authenticate(&spec, "other", Duration::from_secs(5)).await.unwrap());
    }

    #[tokio::test]
    async fn describe_mock() {
        let d = describe(&mock(&[])).await.unwrap();
        assert_eq!(d.name.as_deref(), Some("Mock Agent"));
        assert_eq!(d.version.as_deref(), Some("0.1.0"));
        assert_eq!(d.auth_methods.len(), 1);
        let e = describe(&Spec { command: vec!["/no/such/agent".into()], env: HashMap::new() }).await.unwrap_err();
        assert!(e.to_string().starts_with("could not run /no/such/agent: "));
    }

    #[test]
    fn descendants_order() {
        let ps = " 1 0\n 10 1\n 11 10\n 12 10\n 13 11\n 20 1\n\n";
        assert_eq!(descendants(ps, 10), vec![11, 12, 13]);
    }

    /// The real adapter (needs bun and node_modules at the repository root). `cargo test -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn describe_real_adapter() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let script = root.join("node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js");
        let spec = Spec { command: vec![util::which("bun").unwrap(), script.to_string_lossy().into()], env: HashMap::new() };
        let d = describe(&spec).await.unwrap();
        println!("{}", serde_json::to_string_pretty(&d).unwrap());
        assert!(d.name.is_some());
    }
}

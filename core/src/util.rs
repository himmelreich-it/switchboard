//! What every module uses: the clock, the data folder, running a command, HTTP errors and JSON access.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

// ---------- time ----------

/// `new Date().toISOString()`: UTC, milliseconds, "Z". Stored timestamps compare as strings, so the format must match.
pub fn now() -> String {
    iso(chrono::Utc::now())
}
pub fn iso(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}
/// `Date.now()`: milliseconds since the epoch.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
/// Parses an ISO timestamp as JS `new Date(s)` does for the app's own timestamps.
pub fn parse_iso(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

// ---------- the data folder and the server's address ----------

/// No login on this server: it must only ever listen on 127.0.0.1.
pub const HOST: &str = "127.0.0.1";
pub fn port() -> u16 {
    static P: OnceLock<u16> = OnceLock::new();
    *P.get_or_init(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(4477))
}
/// SWITCHBOARD_HOME or ~/.switchboard, created and resolved to a real path once (macOS /tmp is /private/tmp), so
/// attach_file can compare real paths against it.
pub fn home() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        let h = std::env::var("SWITCHBOARD_HOME").unwrap_or_else(|_| join(&home_dir(), ".switchboard"));
        let _ = std::fs::create_dir_all(&h);
        std::fs::canonicalize(&h).map(|p| p.to_string_lossy().into_owned()).unwrap_or(h)
    })
}
/// os.homedir()
pub fn home_dir() -> String {
    std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|_| ".".into())
}
/// The app's own folder (index.html, vendor/, node_modules/): SWITCHBOARD_APP, else the parent of core/ next to the
/// binary (core/target/release/switchboard → the repository), else the current directory.
pub fn app_dir() -> &'static str {
    static A: OnceLock<String> = OnceLock::new();
    A.get_or_init(|| {
        if let Ok(a) = std::env::var("SWITCHBOARD_APP") {
            return a;
        }
        let exe = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok());
        if let Some(root) = exe.as_deref().and_then(|e| e.ancestors().find(|a| a.join("index.html").exists() && a.join("core").exists())) {
            return root.to_string_lossy().into_owned();
        }
        std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".into())
    })
}

// ---------- paths (node:path, for the forms the app uses) ----------

pub fn join(a: &str, b: &str) -> String {
    Path::new(a).join(b).to_string_lossy().into_owned()
}
pub fn basename(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
pub fn dirname(p: &str) -> String {
    Path::new(p).parent().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into())
}
pub fn extname(p: &str) -> String {
    let b = basename(p);
    match b.rfind('.') {
        Some(i) if i > 0 => b[i..].to_string(),
        _ => String::new(),
    }
}
pub fn exists(p: &str) -> bool {
    Path::new(p).exists()
}
pub fn realpath(p: &str) -> std::io::Result<String> {
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned())
}
pub const SEP: &str = std::path::MAIN_SEPARATOR_STR;

/// `Bun.which`: the command's path on PATH (PATHEXT on Windows).
pub fn which(cmd: &str) -> Option<String> {
    if cmd.contains('/') || (cfg!(windows) && cmd.contains('\\')) {
        return Path::new(cmd).is_file().then(|| cmd.to_string());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD".into()).split(';').map(|e| e.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        for e in &exts {
            let p = dir.join(format!("{cmd}{e}"));
            if p.is_file() && is_executable(&p) {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    true
}

// ---------- commands ----------

pub struct ShOut {
    pub ok: bool,
    pub code: Option<i32>,
    /// stdout, trimmed
    pub out: String,
    /// stderr trimmed, or stdout when stderr is empty
    pub err: String,
}

#[derive(Default)]
pub struct ShOpts {
    pub env: Option<HashMap<String, String>>,
    pub timeout: Option<Duration>,
    pub cwd: Option<String>,
}

/// server.ts `sh()`: runs a command to its end, stdin closed. A command that cannot start is `ok: false` with the
/// reason in `err`; one past its timeout is killed.
pub async fn sh(cmd: &[&str], opts: ShOpts) -> ShOut {
    let mut c = tokio::process::Command::new(cmd[0]);
    c.args(&cmd[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    if let Some(env) = &opts.env {
        c.envs(env);
    }
    if let Some(cwd) = &opts.cwd {
        c.current_dir(cwd);
    }
    no_window(&mut c);
    let fut = c.output();
    let res = match opts.timeout {
        Some(t) => match tokio::time::timeout(t, fut).await {
            Ok(r) => r,
            Err(_) => return ShOut { ok: false, code: None, out: String::new(), err: format!("{} timed out", cmd[0]) },
        },
        None => fut.await,
    };
    match res {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ShOut { ok: o.status.success(), code: o.status.code(), err: if e.is_empty() { out.clone() } else { e }, out }
        }
        Err(e) => ShOut { ok: false, code: None, out: String::new(), err: e.to_string() },
    }
}
/// `sh(cmd)` with owned strings.
pub async fn sh_v(cmd: &[String], opts: ShOpts) -> ShOut {
    let v: Vec<&str> = cmd.iter().map(String::as_str).collect();
    sh(&v, opts).await
}
/// `Bun.spawnSync`, for the few short checks the TS code ran synchronously.
pub fn sh_sync(cmd: &[&str]) -> ShOut {
    let mut c = std::process::Command::new(cmd[0]);
    c.args(&cmd[1..]).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    match c.output() {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ShOut { ok: o.status.success(), code: o.status.code(), err: if e.is_empty() { out.clone() } else { e }, out }
        }
        Err(e) => ShOut { ok: false, code: None, out: String::new(), err: e.to_string() },
    }
}
/// A console program started from the GUI app on Windows opens its own window without this.
pub fn no_window(c: &mut tokio::process::Command) {
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    #[cfg(not(windows))]
    let _ = c;
}

/// The server's runtime, for spawning from threads outside it (a terminal's output reader).
static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();
pub fn set_runtime(h: tokio::runtime::Handle) {
    let _ = RT.set(h);
}
fn rt() -> tokio::runtime::Handle {
    tokio::runtime::Handle::try_current().unwrap_or_else(|_| RT.get().expect("runtime not set").clone())
}
/// Runs a future in the background (a promise nobody awaits).
pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    rt().spawn(f);
}
/// `setTimeout(f, ms)`.
pub fn after<F: std::future::Future<Output = ()> + Send + 'static>(ms: u64, f: F) -> tokio::task::JoinHandle<()> {
    rt().spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        f.await
    })
}

// ---------- HTTP errors ----------

/// `httpError(status, message)`: an error the HTTP layer answers with `{ error: message }` and this status. Anything
/// else that goes wrong is a 500 with its message, as in server.ts.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for HttpError {}
pub fn http_error(status: u16, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(HttpError { status, message: message.into() })
}
/// `throw httpError(status, msg)` / `return fail(msg, status)` inside a function returning anyhow::Result.
#[macro_export]
macro_rules! fail {
    ($status:expr, $($arg:tt)+) => { return Err($crate::util::http_error($status, format!($($arg)+))) };
}

// ---------- JSON access (rows and request bodies are serde_json::Value, like `any` in the TS code) ----------

pub trait J {
    /// a string field; None when missing, null or not a string
    fn s(&self, k: &str) -> Option<&str>;
    /// an integer field (a float is truncated, a numeric string parsed, as Number() would)
    fn i(&self, k: &str) -> Option<i64>;
    /// JS truthiness of a field
    fn truthy(&self, k: &str) -> bool;
    /// a field parsed from JSON text (a TEXT column holding JSON), or Null
    fn parsed(&self, k: &str) -> Value;
}
impl J for Value {
    fn s(&self, k: &str) -> Option<&str> {
        self.get(k).and_then(Value::as_str)
    }
    fn i(&self, k: &str) -> Option<i64> {
        match self.get(k)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.trim().parse().ok(),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }
    fn truthy(&self, k: &str) -> bool {
        truthy(self.get(k).unwrap_or(&Value::Null))
    }
    fn parsed(&self, k: &str) -> Value {
        self.s(k).and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null)
    }
}
/// JS truthiness.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
/// JS `String(v ?? "")` for request fields.
pub fn str_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}
/// `JSON.parse(text)`, Null on failure.
pub fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}
/// Merges `b`'s fields into a copy of `a` (`{ ...a, ...b }`).
pub fn merge(a: &Value, b: Value) -> Value {
    let mut out = a.clone();
    if let (Some(o), Value::Object(bm)) = (out.as_object_mut(), b) {
        for (k, v) in bm {
            o.insert(k, v);
        }
    }
    out
}

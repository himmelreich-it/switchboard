//! Port of server.ts HTTP: the routes of api(), static files, the event stream, the fetch handler. See PORTING.md.

use crate::agents::{self, DEFAULT_AGENT};
use crate::server::{self, artifact_dir, project_row, todo_row, Upload};
use crate::util::{self, basename, dirname, extname, http_error, join, now, J};
use crate::{acp, apptools, db, fail, helpers, params, runner, server_ops, terms};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Request};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use axum::Router;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

/// Bun's default maxRequestBodySize: uploads come base64-encoded inside the JSON body.
const MAX_BODY: usize = 128 * 1024 * 1024;

macro_rules! re {
    ($p:literal) => {{
        static R: LazyLock<Regex> = LazyLock::new(|| Regex::new($p).unwrap());
        &*R
    }};
}

// ---------- HTTP ----------

fn json_status(v: &Value, status: u16) -> Response {
    let mut r = Response::new(Body::from(v.to_string()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json;charset=utf-8"));
    r
}
fn ok(v: Value) -> anyhow::Result<Response> {
    Ok(json_status(&v, 200))
}
fn fail_resp(msg: &str, status: u16) -> Response {
    json_status(&json!({ "error": msg }), status)
}

// JS conversions of request fields, where the routes depend on them.

/// `String(v)`; a missing field is `undefined`.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => num_text(n.as_f64().unwrap_or(0.0)),
        Some(Value::Array(a)) => a.iter().map(|x| if x.is_null() { String::new() } else { js_string(Some(x)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Bool(b)) => b.to_string(),
    }
}
/// `String(v ?? "")`
fn str_of(v: Option<&Value>) -> String {
    if nullish(v) {
        String::new()
    } else {
        js_string(v)
    }
}
/// `String(n)` for a number: 2, not 2.0.
fn num_text(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e21 {
        format!("{}", f as i64)
    } else {
        f.to_string()
    }
}
/// A number as JSON the way JS writes it (2, not 2.0).
fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}
/// `Number(v)`
fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        Some(_) => f64::NAN,
    }
}
/// `v == null`
fn nullish(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null))
}
fn opt_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "linux"
    }
}

// A permission mode from a request body: Ok(None) for "not set" (empty or null), the mode when known, Err when not.
// Claude's modes are known here; another agent's are its own, so any name is taken and the agent reports a miss.
fn mode_in(v: Option<&Value>, agent: Option<&str>) -> Result<Option<String>, ()> {
    if nullish(v) || opt_str(v) == Some("") {
        return Ok(None);
    }
    if !agents::is_claude(agent) || opt_str(v).is_some_and(|s| helpers::MODES.contains(&s)) {
        Ok(Some(js_string(v)))
    } else {
        Err(())
    }
}
// A project's defaults from a request body, {agent: {config id: value, mode}}, stored as JSON (None when none is set);
// Err with the message when a value is not taken.
fn defaults_in(v: Option<&Value>) -> Result<Option<String>, String> {
    let mut out = Map::new();
    if let Some(Value::Object(m)) = v {
        for (agent, d) in m {
            if !agents::is_known(agent) {
                return Err(format!("no agent {agent}"));
            }
            let mut config = helpers::config_in(d);
            let mode = mode_in(d.get("mode"), Some(agent)).map_err(|_| format!("mode must be one of {}", helpers::MODES.join(", ")))?;
            if let Some(mode) = mode {
                // auto is Claude's default anyway
                if !(agents::is_claude(Some(agent)) && mode == "auto") {
                    config["mode"] = json!(mode);
                }
            }
            if config.as_object().is_some_and(|o| !o.is_empty()) {
                out.insert(agent.clone(), config);
            }
        }
    }
    Ok((!out.is_empty()).then(|| Value::Object(out).to_string()))
}
/// Ok(None) for no agent, Err for an unknown one.
fn agent_in(v: Option<&Value>) -> Result<Option<String>, ()> {
    if nullish(v) || opt_str(v) == Some("") {
        return Ok(None);
    }
    let s = js_string(v);
    if agents::is_known(&s) {
        Ok(Some(s))
    } else {
        Err(())
    }
}
// A custom agent's arguments (a list, or one string split on spaces) and environment ({name: value}, or NAME=value lines).
fn agent_args(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| js_string(Some(x))).collect(),
        _ => str_of(v).split_whitespace().map(String::from).collect(),
    }
}
fn agent_env(v: Option<&Value>) -> Value {
    let mut out = Map::new();
    match v {
        Some(Value::Object(m)) => {
            for (k, x) in m {
                out.insert(k.clone(), json!(js_string(Some(x))));
            }
        }
        _ => {
            for l in str_of(v).split('\n').map(str::trim).filter(|l| l.contains('=')) {
                let i = l.find('=').unwrap_or(0);
                out.insert(l[..i].trim().to_string(), json!(l[i + 1..]));
            }
        }
    }
    Value::Object(out)
}

fn todo_or_404(id: i64) -> anyhow::Result<Value> {
    match todo_row(id) {
        Some(t) => Ok(t),
        None => fail!(404, "no such TODO"),
    }
}
// A config or mode change reaches a live agent process at once (apply_config). The change is stored either way; what
// the agent did not take (a mode the model lacks) comes back as a warning, and the next turn tries again.
async fn apply_now(todo_id: i64, reset: Option<&str>) -> anyhow::Result<Response> {
    let Some(a) = runner::live_agent(todo_id) else { return ok(json!({})) };
    runner::apply_config(todo_id, &a, reset).await?;
    let warning = a.take_config_error();
    if !warning.is_empty() {
        server::add_event(Some(todo_id), "error", "app", Some(&warning), None);
    }
    ok(json!({ "warning": if warning.is_empty() { Value::Null } else { json!(warning) } }))
}
fn live_todo(id: i64) -> anyhow::Result<Value> {
    let t = todo_or_404(id)?;
    if t.truthy("removed_at") {
        fail!(409, "this TODO is removed");
    }
    Ok(t)
}
fn expand_home(p: &str) -> String {
    let p = p.trim();
    let p = if p == "~" || p.starts_with("~/") { format!("{}{}", util::home_dir(), &p[1..]) } else { p.to_string() };
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".into());
    resolve(&cwd, &p)
}
// A topic's folder: any existing folder, a git repository or not.
fn require_folder(path: &str) -> anyhow::Result<()> {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
        fail!(400, "{path} is not a folder");
    }
    Ok(())
}
fn uploads_in(v: Option<&Value>) -> anyhow::Result<Vec<Upload>> {
    if nullish(v) {
        return Ok(vec![]);
    }
    let list = v.and_then(Value::as_array).filter(|a| a.iter().all(|u| u.s("name").is_some() && u.s("data").is_some()));
    match list {
        Some(a) => Ok(a.iter().map(|u| Upload { name: u.s("name").unwrap_or("").into(), data: u.s("data").unwrap_or("").into() }).collect()),
        None => fail!(400, "attachments must be a list of {{name, data}}"),
    }
}

// node:path's resolve and relative, lexically (no file system access), as the TS code used them.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
fn resolve(base: &str, p: &str) -> String {
    normalize(&Path::new(base).join(p)).to_string_lossy().into_owned()
}
fn relative(from: &str, to: &str) -> String {
    let (f, t) = (normalize(Path::new(from)), normalize(Path::new(to)));
    let (fc, tc): (Vec<_>, Vec<_>) = (f.components().collect(), t.components().collect());
    let same = fc.iter().zip(&tc).take_while(|(a, b)| a == b).count();
    if same == 0 && !fc.is_empty() && !tc.is_empty() {
        return t.to_string_lossy().into_owned(); // another drive: node returns the absolute path
    }
    let mut out = PathBuf::new();
    for _ in same..fc.len() {
        out.push("..");
    }
    for c in &tc[same..] {
        out.push(c.as_os_str());
    }
    out.to_string_lossy().into_owned()
}
fn is_absolute(p: &str) -> bool {
    Path::new(p).is_absolute() || p.starts_with('/') || p.starts_with('\\')
}
/// `statSync(p).mtimeMs`
fn mtime_ms(m: &std::fs::Metadata) -> f64 {
    let d = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
    d.as_secs() as f64 * 1e3 + d.subsec_nanos() as f64 / 1e6
}
/// `mtime !== body.base_mtime`, where mtime is a number or null and a missing base_mtime is undefined.
// PORT: the page echoes the mtime it was sent; serde_json's float parse may land one ulp off, so numbers compare
// with a tolerance far below a file system's resolution.
fn mtime_differs(mtime: Option<f64>, base: Option<&Value>) -> bool {
    match (mtime, base) {
        (None, Some(Value::Null)) => false,
        (Some(a), Some(Value::Number(b))) => b.as_f64().is_none_or(|b| (a - b).abs() > 1e-6 * a.abs().max(1.0)),
        _ => true,
    }
}

// Shows a file in Finder / Explorer. Not awaited: Explorer exits with 1 even when it opened the window.
// Verbatim arguments only matter on Windows; see reveal_command.
fn reveal(file: &str) -> anyhow::Result<Response> {
    if !util::exists(file) {
        fail!(404, "{file} no longer exists on disk");
    }
    let cmd = helpers::reveal_command(platform(), file);
    let mut c = verbatim_command(&cmd);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Err(e) = c.spawn() {
        fail!(500, "could not open the folder: {e}");
    }
    ok(json!({ "ok": true }))
}
/// Bun.spawn with windowsVerbatimArguments: on Windows the arguments go to the command line as they are, unquoted.
fn verbatim_command(cmd: &[String]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&cmd[0]);
    #[cfg(windows)]
    for a in &cmd[1..] {
        c.raw_arg(a);
    }
    #[cfg(not(windows))]
    c.args(&cmd[1..]);
    util::no_window(&mut c);
    c
}
/// server.ts `sh(cmd, { timeout, verbatim: true })`.
async fn sh_verbatim(cmd: &[String], timeout: Duration) -> util::ShOut {
    if !cfg!(windows) {
        return util::sh_v(cmd, util::ShOpts { timeout: Some(timeout), ..Default::default() }).await;
    }
    let mut c = verbatim_command(cmd);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    match tokio::time::timeout(timeout, c.output()).await {
        Err(_) => util::ShOut { ok: false, code: None, out: String::new(), err: format!("{} timed out", cmd[0]) },
        Ok(Err(e)) => util::ShOut { ok: false, code: None, out: String::new(), err: e.to_string() },
        Ok(Ok(o)) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            util::ShOut { ok: o.status.success(), code: o.status.code(), err: if e.is_empty() { out.clone() } else { e }, out }
        }
    }
}

// ---------- worktree and project files (items 101-110) ----------

// A request names a root and a path relative to it. The real path must lie inside the topic's folder or its
// worktree root (item 110; a folder that is not a git repository has none), so the worktree skill's symlinks into the
// main folder open and nothing else does.
fn root_dir(t: &Value, root: &str) -> anyhow::Result<String> {
    let base = match root {
        "worktree" => t.s("worktree_path").map(String::from),
        "project" => project_row(t.i("project_id")).and_then(|p| p.s("local_path").map(String::from)),
        _ => None,
    };
    match base {
        Some(b) if util::exists(&b) => Ok(b),
        _ => fail!(
            404,
            "{}",
            match root {
                "worktree" => "this TODO has no worktree on disk",
                "project" => "this TODO has no topic",
                _ => "unknown root",
            }
        ),
    }
}
struct Target {
    base: String,
    real: String,
}
fn fs_target(t: &Value, root: &str, rel: &str, write: bool) -> anyhow::Result<Target> {
    let base = root_dir(t, root)?;
    // root_dir found a folder, so the TODO has a topic
    let p = project_row(t.i("project_id")).ok_or_else(|| http_error(404, "this TODO has no topic"))?;
    let local = p.s("local_path").unwrap_or("").to_string();
    if rel.is_empty() || is_absolute(rel) {
        fail!(400, "path must be relative to the root");
    }
    let lexical = resolve(&base, rel);
    if !helpers::is_inside(&lexical, &resolve(&base, ".")) {
        fail!(403, "{rel} is outside the {root}");
    }
    // A link whose target is missing would make a write create that target, wherever it points.
    if !util::exists(&lexical) && std::fs::symlink_metadata(&lexical).is_ok_and(|m| m.file_type().is_symlink()) {
        fail!(403, "{rel} is a link to a missing file");
    }
    let real = if util::exists(&lexical) { util::realpath(&lexical)? } else { join(&util::realpath(&dirname(&lexical))?, &basename(&lexical)) };
    let mut allowed = vec![util::realpath(&local)?];
    if let Some(wt) = runner::worktree_root(&local) {
        allowed.push(util::realpath(&wt)?);
    }
    if !allowed.iter().any(|d| helpers::is_inside(&real, d)) {
        fail!(403, "{rel} points outside the project");
    }
    if write && (rel.split('/').any(|s| s == ".git") || real.split('/').any(|s| s == ".git")) {
        fail!(403, "writes inside .git/ are refused");
    }
    Ok(Target { base, real })
}
// The Files tree of a folder git does not know: its files, without dot entries and node_modules.
// ponytail: stops after 5000 files, enough for notes and documents; a big checkout is a git repository anyway.
fn plain_files(base: &str) -> Vec<String> {
    fn walk(base: &str, rel: &str, found: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(join(base, rel)) else { return };
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in names {
            if found.len() >= 5000 || n.starts_with('.') || n == "node_modules" {
                continue;
            }
            let r = if rel.is_empty() { n } else { format!("{rel}/{n}") };
            let Ok(st) = std::fs::symlink_metadata(join(base, &r)) else { continue };
            if st.is_dir() {
                walk(base, &r, found);
            } else if st.is_file() || (st.file_type().is_symlink() && std::fs::metadata(join(base, &r)).is_ok_and(|m| m.is_file())) {
                found.push(r);
            }
        }
    }
    let mut found = vec![];
    walk(base, "", &mut found);
    found
}
// The base the marks and Diff compare with: the merge base of HEAD with origin/HEAD, else HEAD.
async fn diff_base(dir: &str) -> Option<Value> {
    let mb = util::sh(&["git", "-C", dir, "merge-base", "HEAD", "origin/HEAD"], Default::default()).await;
    if mb.ok {
        return Some(json!({ "sha": mb.out, "label": "merge-base" }));
    }
    let head = util::sh(&["git", "-C", dir, "rev-parse", "HEAD"], Default::default()).await;
    head.ok.then(|| json!({ "sha": head.out, "label": "HEAD" }))
}
/// Image files the page shows as pictures (no line count, no text).
pub static IMAGE_EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$").unwrap());
pub const MAX_TEXT: u64 = 2 * 1024 * 1024;
fn is_image(p: &str) -> bool {
    IMAGE_EXT.is_match(p) && !p.to_lowercase().ends_with(".svg")
}
// ponytail: a whole read per request; fine for source files, the 2 MB cut keeps big files out of the JSON.
fn file_facts(real: &str) -> anyhow::Result<Value> {
    let st = std::fs::metadata(real)?;
    if !st.is_file() {
        fail!(400, "not a file");
    }
    let image = is_image(real);
    let (size, mtime) = (st.len(), num(mtime_ms(&st)));
    if image || st.len() > MAX_TEXT {
        return Ok(json!({ "size": size, "mtime": mtime, "image": image, "large": !image, "binary": false, "content": null }));
    }
    let buf = std::fs::read(real)?;
    let binary = buf[..buf.len().min(8192)].contains(&0);
    let content = if binary { Value::Null } else { json!(String::from_utf8_lossy(&buf)) };
    Ok(json!({ "size": size, "mtime": mtime, "image": false, "large": false, "binary": binary, "content": content }))
}
fn file_mtime(p: &str) -> Option<f64> {
    std::fs::metadata(p).ok().map(|m| mtime_ms(&m))
}

// ---------- static files ----------

/// The content type Bun.file gives a file, by extension.
fn mime_of(path: &str) -> &'static str {
    match extname(path).to_lowercase().as_str() {
        ".html" | ".htm" => "text/html;charset=utf-8",
        ".js" | ".mjs" | ".cjs" => "text/javascript;charset=utf-8",
        ".css" => "text/css;charset=utf-8",
        ".json" | ".map" => "application/json;charset=utf-8",
        ".txt" | ".log" => "text/plain;charset=utf-8",
        ".md" => "text/markdown;charset=utf-8",
        ".csv" => "text/csv;charset=utf-8",
        ".xml" => "application/xml;charset=utf-8",
        ".svg" => "image/svg+xml",
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".avif" => "image/avif",
        ".bmp" => "image/bmp",
        ".ico" => "image/x-icon",
        ".woff" => "font/woff",
        ".woff2" => "font/woff2",
        ".ttf" => "font/ttf",
        ".otf" => "font/otf",
        ".wasm" => "application/wasm",
        ".pdf" => "application/pdf",
        ".zip" => "application/zip",
        ".mp4" => "video/mp4",
        ".webm" => "video/webm",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}
/// `new Response(Bun.file(path), { headers })`
async fn file_response(path: &str, headers: &[(header::HeaderName, &'static str)]) -> anyhow::Result<Response> {
    let bytes = tokio::fs::read(path).await?;
    let mut r = Response::new(Body::from(bytes));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_of(path)));
    for (k, v) in headers {
        r.headers_mut().insert(k.clone(), HeaderValue::from_static(v));
    }
    Ok(r)
}
const SANDBOX: (header::HeaderName, &str) = (header::CONTENT_SECURITY_POLICY, "sandbox");

// ---------- URLs (what `new URL(req.url)` gave the TS code) ----------

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}
/// `decodeURIComponent`; Err on a malformed sequence, where JS throws URIError.
fn decode_uri_component(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let (h, l) = (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex));
            let (Some(h), Some(l)) = (h, l) else { return Err(()) };
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}
/// application/x-www-form-urlencoded decoding, as URLSearchParams does it: `+` is a space, a bad `%` stays.
fn decode_form(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1]).unwrap_or(0) * 16 + hex(b[i + 2]).unwrap_or(0));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// `url.searchParams`
struct Query(Vec<(String, String)>);
impl Query {
    fn of(uri: &Uri) -> Query {
        Query(
            uri.query()
                .unwrap_or("")
                .split('&')
                .filter(|kv| !kv.is_empty())
                .map(|kv| match kv.split_once('=') {
                    Some((k, v)) => (decode_form(k), decode_form(v)),
                    None => (decode_form(kv), String::new()),
                })
                .collect(),
        )
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    fn has(&self, k: &str) -> bool {
        self.0.iter().any(|(n, _)| n == k)
    }
}
/// `url.pathname`: the WHATWG URL parser removes `.` and `..` segments (also written `%2e`), so does this.
fn pathname(uri: &Uri) -> String {
    let dot = |s: &str| s == "." || s.eq_ignore_ascii_case("%2e");
    let dotdot = |s: &str| matches!(s.to_ascii_lowercase().as_str(), ".." | ".%2e" | "%2e." | "%2e%2e");
    let raw = uri.path();
    let segs: Vec<&str> = raw.split('/').skip(1).collect();
    let mut out: Vec<&str> = vec![];
    for (i, s) in segs.iter().enumerate() {
        let last = i == segs.len() - 1;
        if dotdot(s) {
            out.pop();
            if last {
                out.push("");
            }
        } else if dot(s) {
            if last {
                out.push("");
            }
        } else {
            out.push(s);
        }
    }
    format!("/{}", out.join("/"))
}

// ---------- the event stream ----------

/// GET /api/events: one `data:` line per event, and a comment every 20 s so proxies and the page see the stream live.
fn events() -> Response {
    let rx = server::subscribe();
    let start = tokio::time::Instant::now() + Duration::from_secs(20);
    let ping = tokio::time::interval_at(start, Duration::from_secs(20));
    let stream = futures_util::stream::unfold((rx, ping), |(mut rx, mut ping)| async move {
        let chunk = tokio::select! {
            ev = rx.recv() => format!("data: {}\n\n", ev?),
            _ = ping.tick() => ": ping\n\n".to_string(),
        };
        Some((Ok::<_, std::convert::Infallible>(Bytes::from(chunk)), (rx, ping)))
    });
    let mut r = Response::new(Body::from_stream(stream));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

// ---------- api() ----------

const RESTORE_DETAIL: &str = "Nothing was started and the TODO stays removed. Claude Code deletes session transcripts after 30 days by default (cleanupPeriodDays in ~/.claude/settings.json).";

fn meta_num(key: &str, default: f64) -> f64 {
    let n = js_number(server::get_meta(key).map(Value::String).as_ref());
    if n.is_nan() || n == 0.0 {
        default
    } else {
        n
    }
}
fn run_settings() -> Value {
    json!({
        "background_wait_hours": num(meta_num("background_wait_hours", runner::DEFAULT_BACKGROUND_HOURS)),
        "started_todos_per_turn": num(meta_num("started_todos_per_turn", runner::DEFAULT_STARTED_PER_TURN as f64)),
    })
}
async fn summary_of(id: i64) -> Value {
    server::summaries("t.id = ?", params![id]).await.into_iter().next().unwrap_or(Value::Null)
}
fn cap_id(c: &regex::Captures, i: usize) -> i64 {
    c.get(i).and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

async fn api(req: Request, path: &str) -> anyhow::Result<Response> {
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    let body: Value = if method == "GET" {
        Value::Null
    } else {
        match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|_| json!({})),
            // PORT: Bun refused a body over its limit before the handler ran, with 413.
            Err(_) => fail!(413, "request body too large"),
        }
    };
    let b = |k: &str| body.get(k);
    let defined = |k: &str| body.get(k).is_some();
    let m = method.as_str();

    if path == "/api/events" {
        return Ok(events());
    }

    if path == "/api/meta" && m == "GET" {
        // The Waiting lane (laneOf): the status, or a running agent waiting for a permission answer, or for background
        // tasks only after it raised attention itself.
        let mut n = db::get("SELECT COUNT(*) AS n FROM todos WHERE (status = 'waiting' OR (status = 'running' AND permission IS NOT NULL)) AND removed_at IS NULL", params![])
            .and_then(|r| r.i("n"))
            .unwrap_or(0);
        n += db::all("SELECT id, status FROM todos WHERE status = 'running' AND permission IS NULL AND attention_by = 'agent' AND removed_at IS NULL", params![])
            .iter()
            .filter(|r| server::background_wait(r))
            .count() as i64;
        let shell = terms::shell();
        let shell_name = re!(r"(?i)\.exe$").replace(&basename(&shell), "").into_owned();
        // P4: the config options last seen from any session, for Settings and a TODO without a session yet.
        // P5: the agents, and per agent its last seen config options.
        let s = run_settings();
        return ok(json!({ "contract": runner::contract_for(DEFAULT_AGENT, "{artifact_dir}"), "waiting_count": n, "agents": agents::agent_list(), "agent_config": runner::agent_configs(),
            "shell": shell_name, "login_shell": terms::LOGIN_SHELL,
            "background_wait_hours": s["background_wait_hours"], "started_todos_per_turn": s["started_todos_per_turn"] }));
    }
    // Settings › Runs: how long an agent may wait for its background tasks between turns (item 136), and how many TODOs
    // one agent turn may start with create_todo (item 145). Each field is optional.
    if path == "/api/settings" && m == "PUT" {
        if defined("background_wait_hours") {
            let h = js_number(b("background_wait_hours"));
            if !(0.25..=48.0).contains(&h) {
                fail!(400, "background tasks: between 0.25 and 48 hours");
            }
            server::set_meta("background_wait_hours", Some(&num_text(h)));
        }
        if defined("started_todos_per_turn") {
            let n = js_number(b("started_todos_per_turn"));
            if !(n.is_finite() && n.fract() == 0.0) || !(1.0..=100.0).contains(&n) {
                fail!(400, "started TODOs per turn: a whole number from 1 to 100");
            }
            server::set_meta("started_todos_per_turn", Some(&num_text(n)));
        }
        return ok(run_settings());
    }

    // ----- projects -----

    if path == "/api/projects" && m == "GET" {
        let rows = db::all("SELECT * FROM projects ORDER BY name", params![]);
        let out = futures_util::future::join_all(rows.iter().map(server_ops::project_out)).await;
        return ok(json!(out));
    }
    if path == "/api/projects" && m == "POST" {
        let name = str_of(b("name")).trim().to_string();
        if name.is_empty() || str_of(b("local_path")).trim().is_empty() {
            fail!(400, "name and local folder are required");
        }
        let local_path = expand_home(&str_of(b("local_path")));
        require_folder(&local_path)?;
        let taken: HashSet<String> = db::all("SELECT slug FROM projects", params![]).iter().filter_map(|r| r.s("slug").map(String::from)).collect();
        let (defaults, agent) = (defaults_in(b("defaults")), agent_in(b("default_agent")));
        let defaults = match defaults {
            Ok(d) => d,
            Err(e) => fail!(400, "{e}"),
        };
        let Ok(agent) = agent else { fail!(400, "no agent {}", js_string(b("default_agent"))) };
        let id = db::run(
            "INSERT INTO projects (name, slug, local_path, agent_prompt, created_at, defaults, default_agent, independent) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                name,
                helpers::unique_slug(&helpers::slugify(&name), &taken),
                local_path,
                str_of(b("agent_prompt")),
                now(),
                defaults,
                agent.filter(|a| a != DEFAULT_AGENT),
                if util::truthy(b("independent").unwrap_or(&Value::Null)) { Some(1) } else { None }
            ],
        )
        .last_insert_rowid;
        util::spawn(runner::scan_repo(id)); // the profile routing reads (801)
        server::emit(json!({ "type": "projects" }));
        return ok(server_ops::project_out(&project_row(Some(id)).unwrap_or_default()).await);
    }
    if let (Some(c), "PUT") = (re!(r"^/api/projects/([0-9]+)$").captures(path), m) {
        let Some(p) = project_row(Some(cap_id(&c, 1))) else { fail!(404, "no such project") };
        let name = if defined("name") { js_string(b("name")).trim().to_string() } else { p.s("name").unwrap_or("").to_string() };
        let p_local = p.s("local_path").unwrap_or("").to_string();
        let local_path = if defined("local_path") { expand_home(&str_of(b("local_path"))) } else { p_local.clone() };
        if name.is_empty() {
            fail!(400, "name is required");
        }
        if local_path != p_local {
            // Claude Code keeps transcripts under the launch folder, so moving it would orphan every started session.
            if server_ops::project_out(&p).await.truthy("locked") {
                fail!(400, "the local folder is read-only once a TODO in this project has started");
            }
            require_folder(&local_path)?;
        }
        let defaults = if defined("defaults") {
            match defaults_in(b("defaults")) {
                Ok(d) => d,
                Err(e) => fail!(400, "{e}"),
            }
        } else {
            p.s("defaults").map(String::from)
        };
        let agent = if defined("default_agent") {
            match agent_in(b("default_agent")) {
                Ok(a) => a,
                Err(()) => fail!(400, "no agent {}", js_string(b("default_agent"))),
            }
        } else {
            p.s("default_agent").map(String::from)
        };
        let agent_prompt = if defined("agent_prompt") { json!(js_string(b("agent_prompt"))) } else { p["agent_prompt"].clone() };
        let independent = if defined("independent") {
            if util::truthy(b("independent").unwrap_or(&Value::Null)) {
                json!(1)
            } else {
                Value::Null
            }
        } else {
            p["independent"].clone()
        };
        db::run(
            "UPDATE projects SET name = ?, local_path = ?, agent_prompt = ?, defaults = ?, default_agent = ?, independent = ? WHERE id = ?",
            params![name, local_path, agent_prompt, defaults, agent.filter(|a| a != DEFAULT_AGENT), independent, p["id"]],
        );
        // 801: a profile the user edits is marked edited, so a Rescan asks before it overwrites it.
        if defined("profile") {
            let profile = js_string(b("profile")).trim().to_string();
            if profile != p.s("profile").unwrap_or("") {
                db::run("UPDATE projects SET profile = ?, profile_edited = 1 WHERE id = ?", params![(!profile.is_empty()).then_some(profile), p["id"]]);
            }
        }
        server::emit(json!({ "type": "projects" }));
        server::emit(json!({ "type": "todos" })); // rows show the project name
        return ok(server_ops::project_out(&project_row(p.i("id")).unwrap_or_default()).await);
    }

    // 801: Rescan writes the profile again; an edited one only with force. Scan all scans every topic without a profile.
    if let (Some(c), "POST") = (re!(r"^/api/projects/([0-9]+)/scan$").captures(path), m) {
        let Some(p) = project_row(Some(cap_id(&c, 1))) else { fail!(404, "no such topic") };
        if p.truthy("profile_edited") && b("force") != Some(&Value::Bool(true)) {
            return Ok(json_status(&json!({ "error": "edited" }), 409));
        }
        util::spawn(runner::scan_repo(p.i("id").unwrap_or(0)));
        return ok(json!({}));
    }
    if path == "/api/projects/scan-all" && m == "POST" {
        for p in db::all("SELECT id FROM projects WHERE profile IS NULL", params![]) {
            util::spawn(runner::scan_repo(p.i("id").unwrap_or(0)));
        }
        return ok(json!({}));
    }

    // ----- TODOs -----

    if path == "/api/todos" && m == "GET" {
        return ok(json!(server::summaries("1", params![]).await));
    }
    if path == "/api/ports" && m == "GET" {
        return ok(server_ops::scan_ports().await);
    }
    if path == "/api/ports/stop" && m == "POST" {
        return ok(server_ops::stop_port(&js_string(b("kind")), &js_string(b("key"))).await?);
    }
    if path == "/api/todos" && m == "POST" {
        let instruction = str_of(b("instruction")).trim().to_string();
        if instruction.is_empty() {
            fail!(400, "the first instruction is required");
        }
        let Ok(agent) = agent_in(b("agent")) else { fail!(400, "no agent {}", js_string(b("agent"))) };
        // item 144: Repeat in the New TODO dialog
        let schedule = if nullish(b("schedule")) {
            Value::Null
        } else {
            match helpers::schedule_in(&body["schedule"]) {
                Ok(s) => s,
                Err(e) => fail!(400, "{e}"),
            }
        };
        let uploads = uploads_in(b("attachments"))?;
        let id = server_ops::create_todo(&instruction, "user", &uploads, agent.as_deref(), &json!({ "schedule": schedule }));
        if b("start") == Some(&Value::Bool(true)) {
            runner::start(id); // Add & start (item 92): one request, so the TODO never sits in the backlog
        }
        return ok(summary_of(id).await);
    }

    // Item 144: set, change, pause or remove (null) a TODO's schedule.
    if let (Some(c), "PUT") = (re!(r"^/api/todos/([0-9]+)/schedule$").captures(path), m) {
        let t = live_todo(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        let s = if nullish(b("schedule")) {
            Value::Null
        } else {
            match helpers::schedule_in(&body["schedule"]) {
                Ok(s) => s,
                Err(e) => fail!(400, "{e}"),
            }
        };
        server_ops::set_schedule(id, &s, "user");
        return ok(summary_of(id).await);
    }
    // Item 144: Run now, with the skip rules of a firing; the refusal says why.
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/run-now$").captures(path), m) {
        let id = live_todo(cap_id(&c, 1))?.i("id").unwrap_or(0);
        if let Some(why) = runner::fire_schedule(id, "run now") {
            fail!(409, "Not started: {why}.");
        }
        return ok(json!({}));
    }

    // Items 94-97: the first instruction and the agent can change until the TODO starts. The topic has its own route below.
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/edit$").captures(path), m) {
        let t = live_todo(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        if t.s("status") != Some("not_started") {
            fail!(409, "this TODO has started; its first instruction is fixed now");
        }
        let first = db::get("SELECT id, body FROM messages WHERE todo_id = ? ORDER BY id LIMIT 1", params![id]).ok_or_else(|| anyhow::anyhow!("no first message"))?;
        let instruction = defined("instruction").then(|| js_string(b("instruction")).trim().to_string());
        if instruction.as_deref() == Some("") {
            fail!(400, "the first instruction is required");
        }
        let agent = if defined("agent") {
            match agent_in(b("agent")) {
                Ok(a) => a,
                Err(()) => fail!(400, "no agent {}", js_string(b("agent"))),
            }
        } else {
            None
        };
        db::transaction(|| {
            // P5: the agent of the first session, like the project, until the start.
            if let Some(agent) = agent.as_deref().filter(|a| *a != t.s("agent").filter(|x| !x.is_empty()).unwrap_or(DEFAULT_AGENT)) {
                server::set_todo(id, json!({ "agent": agent, "mode": null, "overrides": null, "config": null }), true);
                let detail = format!("agent {} → {}", agents::agent_name(t.s("agent")), agents::agent_name(Some(agent)));
                server::add_event(Some(id), "edited", "user", Some(&detail), None);
            }
            if let Some(instruction) = instruction.as_deref().filter(|i| Some(*i) != first.s("body")) {
                db::run("UPDATE messages SET body = ? WHERE id = ?", params![instruction, first["id"]]);
                // The title is the first instruction until an agent sets one (item 7).
                server::set_todo(id, if t.s("title") == first.s("body") { json!({ "title": instruction }) } else { json!({}) }, true);
                server::add_event(Some(id), "edited", "user", Some("first instruction"), None);
            }
            Ok(())
        })?;
        server::emit(json!({ "type": "todos" }));
        return ok(summary_of(id).await);
    }

    // 801: Change topic, and a click on one of the operator's options while routing asks. project_id null is "no topic".
    // Allowed while the agent and the TODO's terminals are stopped, as their folder is fixed at launch. A worktree is let go.
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/topic$").captures(path), m) {
        let t = live_todo(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        let to = if nullish(b("project_id")) { None } else { project_row(body.i("project_id")) };
        if !nullish(b("project_id")) && to.is_none() {
            fail!(400, "no such topic");
        }
        if runner::is_running(Some(id)) {
            fail!(409, "the agent is running; change the topic once it stops");
        }
        if terms::any_running(id) {
            fail!(409, "close this TODO's terminals first; they run in the old topic's folder");
        }
        let branch = match t.s("worktree_path") {
            Some(wt) => server::git_state(wt).await.and_then(|g| g.s("branch").map(String::from)),
            None => None,
        };
        runner::change_repo(id, to.and_then(|p| p.i("id")), branch.as_deref());
        return ok(summary_of(id).await);
    }

    // Opens the TODO's folder in VS Code: its worktree, else its topic folder, else its own folder (no topic).
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/vscode$").captures(path), m) {
        let t = todo_or_404(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        // v10: `place` opens the worktree a TODO keeps in another topic; only a folder listed in its places is taken.
        let place = util::truthy(b("place").unwrap_or(&Value::Null));
        let kept: Option<String> = if place {
            let places = t.parsed("places");
            places.as_object().and_then(|o| o.values().filter_map(|x| x.s("worktree_path")).find(|d| Some(&json!(d)) == b("place")).map(String::from))
        } else {
            None
        };
        if place && kept.is_none() {
            fail!(400, "not a worktree this TODO keeps");
        }
        let candidates: Vec<Option<String>> = match &kept {
            Some(k) => vec![Some(k.clone())],
            None => vec![
                t.s("worktree_path").map(String::from),
                project_row(t.i("project_id")).and_then(|p| p.s("local_path").map(String::from)),
                Some(artifact_dir(id)),
            ],
        };
        let Some(dir) = candidates.into_iter().flatten().find(|d| !d.is_empty() && util::exists(d)) else {
            fail!(404, "this TODO has no folder on disk yet")
        };
        let r = sh_verbatim(&helpers::vscode_command(platform(), &dir), Duration::from_secs(15)).await;
        let hint = if platform() == "darwin" { "Is VS Code installed?" } else { "Is VS Code's code command on PATH?" };
        if !r.ok {
            fail!(500, "{}", format!("could not open VS Code. {hint} {}", r.err).trim());
        }
        return ok(json!({ "ok": true, "dir": dir }));
    }

    // Item 147: New note. The user's own markdown file in the artifact folder, notes.md, then notes-2.md, notes-3.md.
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/files$").captures(path), m) {
        let t = todo_or_404(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        let dir = artifact_dir(id);
        std::fs::create_dir_all(&dir)?;
        let (mut n, mut p) = (1, join(&dir, "notes.md"));
        while util::exists(&p) {
            n += 1;
            p = join(&dir, &format!("notes-{n}.md"));
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
        let r = db::run("INSERT INTO files (todo_id, path, by_user) VALUES (?, ?, 1)", params![id, util::realpath(&p)?]);
        server::add_event(Some(id), "file created", "user", Some(&basename(&p)), None);
        server::changed(Some(id));
        return ok(json!({ "id": r.last_insert_rowid, "name": basename(&p) }));
    }

    if let Some(c) = re!(r"^/api/todos/([0-9]+)/files/([0-9]+)(?:/(reveal))?$").captures(path) {
        let id = cap_id(&c, 1);
        let t = todo_or_404(id)?;
        let Some(f) = db::get("SELECT id, path, by_user FROM files WHERE id = ? AND todo_id = ?", params![cap_id(&c, 2), id]) else { fail!(404, "no such file") };
        let (fid, fpath, by_user) = (f.i("id").unwrap_or(0), f.s("path").unwrap_or("").to_string(), f.truthy("by_user"));
        if c.get(3).is_some() {
            return if m == "POST" { reveal(&fpath) } else { fail!(404, "not found") };
        }
        // Item 147: the user deletes a note made with New note, with its comments. Any other file moves to archive/ in
        // the artifact folder, which the list skips; the user gets it back by moving it out in Finder.
        if m == "DELETE" {
            if by_user {
                if let Err(e) = std::fs::remove_file(&fpath) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Err(e.into());
                    }
                }
            } else if util::exists(&fpath) {
                if t.s("status") == Some("running") {
                    fail!(409, "running");
                }
                let dir = artifact_dir(id);
                let rel = relative(&dir, &fpath);
                if rel.starts_with("..") || is_absolute(&rel) {
                    fail!(403, "{fpath} is outside the artifact folder");
                }
                let ext = extname(&rel);
                let stem = join(&join(&dir, "archive"), &rel[..rel.len() - ext.len()]);
                let (mut to, mut n) = (format!("{stem}{ext}"), 1);
                while util::exists(&to) {
                    n += 1;
                    to = format!("{stem}-{n}{ext}");
                }
                std::fs::create_dir_all(dirname(&to))?;
                std::fs::rename(&fpath, &to)?;
            }
            server_ops::drop_file(id, fid);
            server::add_event(Some(id), if by_user { "file deleted" } else { "file archived" }, "user", Some(&basename(&fpath)), None);
            server::changed(Some(id));
            return ok(json!({ "ok": true }));
        }
        if m == "GET" {
            if !util::exists(&fpath) {
                fail!(404, "{fpath} no longer exists on disk");
            }
            if Query::of(&uri).has("raw") {
                return file_response(&fpath, &[SANDBOX]).await;
            }
            return ok(util::merge(&json!({ "id": fid, "name": basename(&fpath), "path": fpath, "by_user": by_user }), file_facts(&fpath)?));
        }
        if m == "PUT" {
            let Some(content) = body.s("content") else { fail!(400, "content is required") };
            // The user's own notes stay editable while the agent runs (item 147); the mtime check guards them, as for Repo files.
            if t.s("status") == Some("running") && !by_user {
                fail!(409, "running");
            }
            let mtime = if util::exists(&fpath) { file_mtime(&fpath) } else { None };
            if !body.truthy("force") && mtime_differs(mtime, b("base_mtime")) {
                return Ok(json_status(&json!({ "error": "changed", "mtime": mtime.map(num) }), 409));
            }
            std::fs::write(&fpath, content)?; // the agent is not told; the user hands work back with a chat message
            server::add_event(Some(id), "file saved", "user", Some(&format!("{} · agent not notified", basename(&fpath))), None);
            return ok(json!({ "mtime": file_mtime(&fpath).map(num) }));
        }
    }

    // Item 146: comments on selected text of a file the TODO shows. The agent reads them with list_comments.
    if let Some(c) = re!(r"^/api/todos/([0-9]+)/comments(?:/([0-9]+))?$").captures(path) {
        let t = todo_or_404(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        if c.get(2).is_none() && m == "POST" {
            let (quote, text) = (str_of(b("quote")), str_of(b("body")).trim().to_string());
            if quote.trim().is_empty() || text.is_empty() {
                fail!(400, "quote and body are required");
            }
            let (file, rel) = if b("root") == Some(&json!("todo")) {
                let Some(f) = db::get("SELECT path FROM files WHERE id = ? AND todo_id = ?", params![js_number(b("file_id")), id]) else { fail!(404, "no such file") };
                (f.s("path").unwrap_or("").to_string(), js_string(b("file_id")))
            } else {
                (fs_target(&t, &js_string(b("root")), &js_string(b("path")), false)?.real, js_string(b("path")))
            };
            let line = b("line").and_then(Value::as_f64).filter(|l| l.fract() == 0.0 && *l > 0.0).map(|l| l as i64);
            let r = db::run(
                "INSERT INTO comments (todo_id, root, rel, path, quote, line, body, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'open', ?)",
                params![id, js_string(b("root")), rel, file, quote, line, text, now()],
            );
            server::changed(Some(id));
            return ok(json!({ "id": r.last_insert_rowid }));
        }
        let cm = match c.get(2) {
            Some(cid) => db::get("SELECT * FROM comments WHERE id = ? AND todo_id = ?", params![cid.as_str().parse::<i64>().unwrap_or(0), id]),
            None => None,
        };
        let Some(cm) = cm else { fail!(404, "no such comment") };
        if m == "DELETE" {
            db::run("DELETE FROM comments WHERE id = ?", params![cm["id"]]);
            server::changed(Some(id));
            return ok(json!({ "ok": true }));
        }
        if m == "PATCH" {
            // An edited comment is open again: the agent's next list_comments returns the new text with its earlier reply.
            if let Some(text) = body.s("body").map(str::trim).filter(|s| !s.is_empty()) {
                db::run("UPDATE comments SET body = ?, state = 'open' WHERE id = ?", params![text, cm["id"]]);
            } else if let Some(state) = body.s("state").filter(|s| *s == "resolved" || *s == "open") {
                db::run("UPDATE comments SET state = ? WHERE id = ?", params![state, cm["id"]]);
            } else {
                fail!(400, "body or state (resolved, open) is required");
            }
            server::changed(Some(id));
            return ok(json!({ "ok": true }));
        }
    }

    if let Some(c) = re!(r"^/api/todos/([0-9]+)/(tree|fs|fs/raw|fs/reveal|diff)$").captures(path) {
        let t = todo_or_404(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        let what = c.get(2).map_or("", |x| x.as_str());
        let q = Query::of(&uri);
        let root = q.get("root").map(String::from).unwrap_or_else(|| js_string(body.get("root")));
        let rel = q.get("path").map(String::from).unwrap_or_else(|| js_string(body.get("path")));
        if what == "tree" && m == "GET" {
            let base = root_dir(&t, &root)?;
            // A topic folder that is not a git repository: every file, no marks and no base, so no Diff either.
            if !runner::is_git_folder(&base) {
                return ok(json!({ "paths": plain_files(&base), "marks": {}, "links": [], "base": null, "dir": base, "git": false }));
            }
            let bse = diff_base(&base).await;
            let diff = async {
                match bse.as_ref().and_then(|x| x.s("sha")) {
                    Some(sha) => util::sh(&["git", "-C", &base, "--no-optional-locks", "diff", "--relative", "--name-status", "-z", sha], Default::default()).await,
                    None => util::ShOut { ok: true, code: Some(0), out: String::new(), err: String::new() },
                }
            };
            let all_cmd = ["git", "-C", &base, "--no-optional-locks", "ls-files", "--cached", "--others", "--exclude-standard", "-z"];
            let others_cmd = ["git", "-C", &base, "--no-optional-locks", "ls-files", "--others", "--exclude-standard", "-z"];
            let (files, others, diff) = tokio::join!(util::sh(&all_cmd, Default::default()), util::sh(&others_cmd, Default::default()), diff);
            if !files.ok {
                fail!(500, "git could not list {base}");
            }
            let marks = helpers::parse_marks(if diff.ok { &diff.out } else { "" }, &others.out);
            let mut paths: Vec<String> = vec![];
            let mut seen = HashSet::new();
            for p in files.out.split('\0').filter(|p| !p.is_empty()) {
                if seen.insert(p.to_string()) {
                    paths.push(p.to_string());
                }
            }
            // The worktree skill links .env or CLAUDE.local.md into the main folder; git ignores them, the tree shows them.
            // ponytail: only symlinks at the top of the root are found this way.
            let mut links: Vec<String> = vec![];
            for e in std::fs::read_dir(&base)?.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if std::fs::symlink_metadata(join(&base, &n)).is_ok_and(|m| m.file_type().is_symlink()) {
                    links.push(n.clone());
                    if seen.insert(n.clone()) {
                        paths.push(n);
                    }
                }
            }
            let paths: Vec<String> = paths.into_iter().filter(|p| marks.s(p) != Some("D") || util::exists(&join(&base, p))).collect();
            let tree = json!({ "paths": paths, "marks": marks, "links": links, "base": bse, "dir": base, "git": true });
            // The page polls the tree; a tree like the one it has (its tag) is answered without the paths.
            // PORT: Bun.hash (wyhash) became std's SipHash with fixed keys; the tag is only compared with itself.
            let tag = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                tree.to_string().hash(&mut h);
                h.finish().to_string()
            };
            return ok(if q.get("tag") == Some(tag.as_str()) { json!({ "unchanged": true }) } else { util::merge(&tree, json!({ "tag": tag })) });
        }
        if what == "fs" && m == "GET" {
            let real = fs_target(&t, &root, &rel, false)?.real;
            if !util::exists(&real) {
                fail!(404, "{rel} no longer exists on disk");
            }
            return ok(util::merge(&json!({ "path": rel, "abs": real }), file_facts(&real)?));
        }
        if what == "fs/raw" && m == "GET" {
            let real = fs_target(&t, &root, &rel, false)?.real;
            if !util::exists(&real) {
                fail!(404, "no such file");
            }
            return file_response(&real, &[SANDBOX]).await;
        }
        if what == "fs/reveal" && m == "POST" {
            return reveal(&fs_target(&t, &root, &rel, false)?.real);
        }
        if what == "fs" && m == "PUT" {
            let real = fs_target(&t, &root, &rel, true)?.real;
            let Some(content) = body.s("content") else { fail!(400, "content is required") };
            // Item 108: worktree files lock while the agent runs; the project folder has no lock, the mtime check guards it.
            if root == "worktree" && t.s("status") == Some("running") {
                fail!(409, "running");
            }
            let mtime = if util::exists(&real) { file_mtime(&real) } else { None };
            if !body.truthy("force") && mtime_differs(mtime, b("base_mtime")) {
                return Ok(json_status(&json!({ "error": "changed", "mtime": mtime.map(num) }), 409));
            }
            std::fs::write(&real, content)?; // to the resolved file, so a symlink stays a symlink
            server::add_event(Some(id), "file saved", "user", Some(&format!("{root} · {rel} · agent not notified")), None);
            return ok(json!({ "mtime": file_mtime(&real).map(num) }));
        }
        if what == "diff" && m == "GET" {
            let Target { base, real } = fs_target(&t, &root, &rel, false)?;
            let bse = diff_base(&base).await;
            let top = util::sh(&["git", "-C", &base, "rev-parse", "--show-toplevel"], Default::default()).await;
            let from = if top.ok { util::realpath(&top.out)? } else { base.clone() };
            let in_repo = relative(&from, &join(&util::realpath(&base)?, &rel));
            // Not through sh(): it trims, and a diff needs the exact text. A file added since the base has no old version.
            let mut old = String::new();
            if let Some(sha) = bse.as_ref().and_then(|x| x.s("sha")) {
                let mut c = tokio::process::Command::new("git");
                c.args(["-C", &base, "show", &format!("{sha}:{in_repo}")]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
                util::no_window(&mut c);
                if let Ok(o) = c.output().await {
                    if o.status.success() {
                        old = String::from_utf8_lossy(&o.stdout).into_owned();
                    }
                }
            }
            let current = if util::exists(&real) { file_facts(&real)?.s("content").unwrap_or("").to_string() } else { String::new() };
            return ok(json!({
                "base": old,
                "base_sha": bse.as_ref().map_or(Value::Null, |x| x["sha"].clone()),
                "base_label": bse.as_ref().map_or(Value::Null, |x| x["label"].clone()),
                "current": current,
            }));
        }
    }

    if let Some(c) = re!(r"^/api/todos/([0-9]+)/terminals(?:/([0-9]+)/(restart|close|busy))?$").captures(path) {
        let tid = c.get(2).and_then(|x| x.as_str().parse().ok());
        return ok(terms::api(m, cap_id(&c, 1), tid, c.get(3).map(|x| x.as_str()), &body).await?);
    }

    if let (Some(c), "GET") = (re!(r"^/api/attachments/([0-9]+)$").captures(path), m) {
        let a = db::get("SELECT path FROM attachments WHERE id = ?", params![cap_id(&c, 1)]);
        let Some(p) = a.and_then(|a| a.s("path").map(String::from)).filter(|p| util::exists(p)) else { fail!(404, "no such attachment") };
        // sandbox: an uploaded SVG or HTML opened in a tab cannot run script against this API.
        return file_response(&p, &[SANDBOX]).await;
    }

    if let (Some(c), "GET") = (re!(r"^/api/todos/([0-9]+)$").captures(path), m) {
        let id = cap_id(&c, 1);
        let todo = summary_of(id).await;
        if todo.is_null() {
            fail!(404, "no such TODO");
        }
        if let Some(t) = todo_row(id) {
            server_ops::register_disk_files(&t);
        }
        let ids = db::all("SELECT id FROM messages WHERE todo_id = ? ORDER BY id", params![id]);
        // P5: each session's agent, and how to resume it outside the app where that agent has a command for it.
        let sessions: Vec<Value> = server::sessions_of(id)
            .into_iter()
            .map(|s| {
                let agent = s.s("agent").filter(|a| !a.is_empty()).unwrap_or(DEFAULT_AGENT).to_string();
                let extra = json!({ "agent": agent, "agent_name": agents::agent_name(s.s("agent")), "resume": agents::resume_hint(s.s("agent"), s.s("session_id").unwrap_or("")) });
                util::merge(&s, extra)
            })
            .collect();
        let artifact = artifact_dir(id) + "/";
        let seq = server::flush_seq();
        let messages: Vec<Value> = ids.iter().map(|r| server_ops::message_out(r.i("id").unwrap_or(0))).collect();
        let files: Vec<Value> = server::files_of(id).iter().map(server_ops::file_out).collect();
        let comments = server::comments_of(id, "1");
        let events: Vec<Value> = db::all("SELECT id, kind, actor, detail, at, session_no, meta FROM events WHERE todo_id = ? ORDER BY id", params![id]).into_iter().map(server::event_out).collect();
        return ok(json!({
            "todo": todo, "sessions": sessions, "artifact_dir": artifact, "seq": seq, "messages": messages, "files": files,
            "comments": comments, "events": events, "draft": server::get_draft(&format!("todo:{id}")),
        }));
    }

    if let (Some(c), "PUT") = (re!(r"^/api/todos/([0-9]+)/draft$").captures(path), m) {
        let id = todo_or_404(cap_id(&c, 1))?.i("id").unwrap_or(0);
        server::set_draft(&format!("todo:{id}"), &str_of(b("body")));
        server::changed(Some(id));
        return ok(json!({}));
    }

    // P3: the TODO's own permission mode (null: the project's). A live agent process gets it at once, also mid-turn;
    // otherwise the next run applies it.
    if let (Some(c), "PUT") = (re!(r"^/api/todos/([0-9]+)/mode$").captures(path), m) {
        let t = live_todo(cap_id(&c, 1))?;
        let id = t.i("id").unwrap_or(0);
        let Ok(mode) = mode_in(b("mode"), t.s("agent")) else {
            fail!(400, "mode must be one of {}, or empty for the project's", helpers::MODES.join(", "))
        };
        server::set_todo(id, json!({ "mode": mode }), false);
        let detail = match &mode {
            Some(m) => m.clone(),
            None => {
                let p = project_row(t.i("project_id")).unwrap_or_else(|| json!({ "defaults": null }));
                let of = runner::mode_of(&json!({ "mode": mode, "agent": t["agent"] }), &p);
                format!("topic's ({})", of.as_deref().unwrap_or("the agent's own"))
            }
        };
        server::add_event(Some(id), "mode", "user", Some(&detail), None);
        return apply_now(id, None).await;
    }

    // P4: one of the TODO's own config values (model, effort, …; empty: the project's). Like the mode, a live agent
    // process gets it at once, also mid-turn; otherwise the next run applies it. Only options and values the agent has
    // offered are taken.
    if let (Some(c), "PUT") = (re!(r"^/api/todos/([0-9]+)/config$").captures(path), m) {
        let t = live_todo(cap_id(&c, 1))?;
        let tid = t.i("id").unwrap_or(0);
        let id = str_of(b("id"));
        let value = if nullish(b("value")) { String::new() } else { js_string(b("value")) };
        let agent = t.s("agent").filter(|a| !a.is_empty()).unwrap_or(DEFAULT_AGENT);
        let raw = t.s("config").map(String::from).or_else(|| server::get_meta(&runner::meta_key("agent_config", agent))).unwrap_or_else(|| "[]".into());
        let opts = util::parse(&raw);
        let known = opts.as_array().and_then(|a| a.iter().find(|o| o.s("id") == Some(id.as_str())).cloned());
        let Some(known) = known.filter(|_| id != "mode") else {
            fail!(400, "no config option {}", if id.is_empty() { "(none)" } else { &id })
        };
        let options = known["options"].as_array().cloned().unwrap_or_default();
        let known_name = known.s("name").unwrap_or("").to_string();
        if !value.is_empty() && !options.iter().any(|o| o.s("value") == Some(value.as_str())) {
            fail!(400, "{known_name} has no value {value}");
        }
        let mut overrides = helpers::config_in(&t.parsed("overrides"));
        if let Some(o) = overrides.as_object_mut() {
            if value.is_empty() {
                o.shift_remove(&id);
            } else {
                o.insert(id.clone(), json!(value));
            }
        }
        let empty = overrides.as_object().is_none_or(|o| o.is_empty());
        server::set_todo(tid, json!({ "overrides": if empty { Value::Null } else { json!(overrides.to_string()) } }), false);
        let shown = if value.is_empty() {
            "not set".to_string()
        } else {
            options.iter().find(|o| o.s("value") == Some(value.as_str())).and_then(|o| o.s("name")).unwrap_or("").to_string()
        };
        server::add_event(Some(tid), "config", "user", Some(&format!("{known_name}: {shown}")), None);
        return apply_now(tid, if value.is_empty() { Some(&id) } else { None }).await;
    }

    // P4: the slash commands the composer completes.
    if let (Some(c), "GET") = (re!(r"^/api/todos/([0-9]+)/commands$").captures(path), m) {
        let id = todo_or_404(cap_id(&c, 1))?.i("id").unwrap_or(0);
        return ok(runner::commands_of(id));
    }

    // P3: the user's answer to the open permission request.
    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/permission$").captures(path), m) {
        let id = live_todo(cap_id(&c, 1))?.i("id").unwrap_or(0);
        if !runner::answer_permission(id, &str_of(b("optionId"))) {
            fail!(409, "no such open permission request; the agent may have moved on");
        }
        return ok(json!({}));
    }

    if let (Some(c), "POST") = (re!(r"^/api/todos/([0-9]+)/(start|messages|read|clear-attention|remove|restore|deliver|new-session|stop)$").captures(path), m) {
        let id = cap_id(&c, 1);
        let action = c.get(2).map_or("", |x| x.as_str());
        let suggested = b("suggested") == Some(&Value::Bool(true));

        // Item 72: ends the live session and starts the next with the hand-over header, the queued messages and this one.
        if action == "new-session" {
            let t = live_todo(id)?;
            if !t.truthy("session_id") {
                fail!(409, "start the TODO first; its first session starts with Start");
            }
            if runner::is_running(Some(id)) {
                fail!(409, "the agent is running; a new session can start once it stops");
            }
            let Ok(agent) = agent_in(b("agent")) else { fail!(400, "no agent {}", js_string(b("agent"))) };
            let text = str_of(b("body")).trim().to_string();
            let uploads = uploads_in(b("attachments"))?;
            if suggested && !server::pick_suggestion(id, &text) {
                fail!(409, "That suggestion is gone");
            }
            let mid = db::transaction(|| {
                let mid = server::add_message(Some(id), "user", if text.is_empty() { "continue" } else { &text }, "queued", None, None);
                server::save_attachments(id, mid, &uploads);
                Ok(mid)
            })?;
            if !suggested {
                server::set_draft(&format!("todo:{id}"), "");
            }
            runner::new_session(id, agent.as_deref(), None);
            return ok(server_ops::message_out(mid));
        }

        match action {
            "start" => {
                if live_todo(id)?.s("status") != Some("not_started") {
                    fail!(409, "this TODO has already started");
                }
                runner::start(id);
            }
            "messages" => {
                live_todo(id)?;
                let text = str_of(b("body")).trim().to_string();
                let uploads = uploads_in(b("attachments"))?;
                if text.is_empty() && uploads.is_empty() {
                    fail!(400, "empty message");
                }
                if suggested && !server::pick_suggestion(id, &text) {
                    fail!(409, "That suggestion is gone");
                }
                let mid = runner::commit_message(id, "user", &text, &uploads);
                // A pill click sends its text; the draft in the composer stays.
                if !suggested {
                    server::set_draft(&format!("todo:{id}"), "");
                }
                return ok(server_ops::message_out(mid));
            }
            "read" => {
                // Only an actual change is announced, so a page that marks read on every refetch cannot loop.
                let s = summary_of(todo_or_404(id)?.i("id").unwrap_or(0)).await;
                if s.truthy("unread") {
                    server::set_todo(id, json!({ "last_read_at": now() }), false);
                } else {
                    db::run("UPDATE todos SET last_read_at = ? WHERE id = ?", params![now(), id]);
                }
            }
            "clear-attention" => {
                let t = todo_or_404(id)?;
                if t.truthy("attention") {
                    // A TODO whose routing asks (801) has no session to be idle in: it stays waiting for the pick.
                    let status = if t.s("status") == Some("waiting") && !t.truthy("routing") { json!("idle") } else { t["status"].clone() };
                    server::set_todo(id, json!({ "attention": 0, "attention_by": null, "status": status }), true);
                    server::add_event(Some(id), "attention cleared", "user", None, None);
                }
            }
            "remove" => {
                live_todo(id)?;
                server_ops::remove_todo(id, "user");
            }
            "restore" => {
                let t = todo_or_404(id)?;
                if !t.truthy("removed_at") {
                    fail!(409, "this TODO is not removed");
                }
                // A session ended by Change topic is not resumed: the next message starts a new one.
                let session_id = t.s("session_id").filter(|s| !s.is_empty());
                if let Some(sid) = session_id.filter(|_| !server::sessions_of(id).last().is_some_and(|s| s.truthy("ended_at"))) {
                    let folder = project_row(t.i("project_id")).and_then(|p| p.s("local_path").map(String::from)).unwrap_or_else(|| artifact_dir(id));
                    let spec = agents::spec_of(t.s("agent").filter(|a| !a.is_empty()).unwrap_or(DEFAULT_AGENT));
                    if let Some(spec) = spec.filter(|_| util::exists(&folder)) {
                        if !acp::session_exists(&spec, &folder, sid).await.unwrap_or(true) {
                            return Ok(json_status(&json!({ "warning": format!("The {} session {sid} no longer exists.", agents::agent_name(t.s("agent"))), "detail": RESTORE_DETAIL }), 409));
                        }
                    }
                    if let Some(wt) = t.s("worktree_path").filter(|w| !util::exists(w)) {
                        return Ok(json_status(&json!({ "warning": format!("The worktree {wt} no longer exists."), "detail": RESTORE_DETAIL }), 409));
                    }
                    if !util::exists(&folder) {
                        return Ok(json_status(&json!({ "warning": format!("The topic's folder {folder} no longer exists."), "detail": RESTORE_DETAIL }), 409));
                    }
                }
                // A scheduled TODO gets its next slot from now (item 144); the slots missed while removed are skipped.
                let s = t.parsed("schedule");
                let mut fields = json!({ "removed_at": null, "removed_by": null });
                if util::truthy(&s) {
                    fields["next_run_at"] = if s.truthy("paused") {
                        Value::Null
                    } else {
                        json!(helpers::next_run(&s, chrono::Local::now()).map(|d| util::iso(d.with_timezone(&chrono::Utc))))
                    };
                }
                server::set_todo(id, fields, true);
                server::add_event(Some(id), "restored", "user", None, None);
                server::emit(json!({ "type": "todos" }));
                // A never-started TODO goes back to not started with its pending messages; nothing launches.
                if session_id.is_some() {
                    // Queued messages plus the draft go out as one prompt; with neither, the prompt is "continue".
                    // "continue" is queued like any message, so a restore while the interrupted run is still ending
                    // goes out when that run ends.
                    let draft = server::get_draft(&format!("todo:{id}")).trim().to_string();
                    let queued = db::get("SELECT COUNT(*) AS n FROM messages WHERE todo_id = ? AND state = 'queued'", params![id]).and_then(|r| r.i("n")).unwrap_or(0);
                    if !draft.is_empty() || queued == 0 {
                        server::add_message(Some(id), "user", if draft.is_empty() { "continue" } else { &draft }, "queued", None, None);
                    }
                    server::set_draft(&format!("todo:{id}"), "");
                    runner::deliver(id, "restore");
                }
            }
            "stop" => {
                // Like Esc in Claude Code. Messages queued during the turn are answered right after.
                // Also ends the background tasks the agent waits for between turns (item 136), and routing (801): a
                // routing run ends with its interrupt, a question waiting for the user ends here. The TODO is not
                // started again.
                if todo_row(id).is_some_and(|t| t.truthy("routing")) && !runner::is_running(Some(id)) {
                    runner::cancel_routing(id);
                    return ok(json!({}));
                }
                let lid = live_todo(id)?.i("id");
                if !runner::is_running(lid) && runner::background_count(id) == 0 {
                    fail!(409, "the agent is not running");
                }
                runner::interrupt(Some(id), "user");
            }
            "deliver" => {
                if runner::is_running(live_todo(id)?.i("id")) {
                    fail!(409, "the agent is running; queued messages go out when it stops");
                }
                runner::deliver(id, "deliver");
            }
            _ => {}
        }
        return ok(json!({}));
    }

    // ----- agents (P5) -----

    if path == "/api/worktrees" && m == "GET" {
        return ok(server_ops::worktree_scan().unwrap_or_else(|| json!({ "at": null, "rows": [], "errors": [] })));
    }
    if path == "/api/worktrees/scan" && m == "POST" {
        server_ops::scan_worktrees().await;
        return ok(server_ops::worktree_scan().unwrap_or(Value::Null));
    }
    if path == "/api/worktrees/delete" && m == "POST" {
        let items: Vec<Value> = body["items"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|i| i.s("key").is_some())
                    .map(|i| json!({ "key": i["key"], "force": i.get("force") == Some(&Value::Bool(true)), "seen": str_of(i.get("seen")) }))
                    .collect()
            })
            .unwrap_or_default();
        if items.is_empty() {
            fail!(400, "items required");
        }
        return ok(server_ops::delete_worktrees(&json!(items)).await);
    }
    if path == "/api/agents" && m == "GET" {
        return ok(json!(agents::agent_list()));
    }
    // A custom agent: an ACP agent started as `command args…` with `env` added (Settings › Agents).
    if path == "/api/agents" && m == "POST" {
        let (name, command) = (str_of(b("name")).trim().to_string(), str_of(b("command")).trim().to_string());
        if name.is_empty() || command.is_empty() {
            fail!(400, "name and command are required");
        }
        let id = helpers::slugify(&if nullish(b("id")) { name.clone() } else { js_string(b("id")) });
        if agents::is_known(&id) {
            fail!(400, "an agent {id} exists already");
        }
        let (args, env) = (agent_args(b("args")), agent_env(b("env")));
        db::run("INSERT INTO agents (id, name, command, args, env, created_at) VALUES (?, ?, ?, ?, ?, ?)", params![id, name, command, json!(args).to_string(), env.to_string(), now()]);
        server::emit(json!({ "type": "agents" }));
        return ok(agents::agent_list().into_iter().find(|x| x.s("id") == Some(id.as_str())).unwrap_or(Value::Null));
    }
    if let Some(c) = re!(r"^/api/agents/([A-Za-z0-9_-]+)$").captures(path).filter(|_| m == "PUT" || m == "DELETE") {
        let id = c.get(1).map_or("", |x| x.as_str());
        if agents::is_builtin(id) {
            fail!(409, "the built-in agents cannot be changed");
        }
        let Some(r) = db::get("SELECT * FROM agents WHERE id = ?", params![id]) else { fail!(404, "no such agent") };
        if m == "DELETE" {
            let n = db::get("SELECT COUNT(*) AS n FROM todos WHERE agent = ? AND removed_at IS NULL", params![id]).and_then(|r| r.i("n")).unwrap_or(0);
            if n > 0 {
                fail!(409, "{n} TODO{} on this agent", if n > 1 { "s run" } else { " runs" });
            }
            db::run("DELETE FROM agents WHERE id = ?", params![id]);
            db::run("UPDATE projects SET default_agent = NULL WHERE default_agent = ?", params![id]);
        } else {
            let keep = |k: &str| {
                let v = if nullish(b(k)) { js_string(r.get(k)) } else { js_string(b(k)) };
                let v = v.trim().to_string();
                if v.is_empty() {
                    r.s(k).unwrap_or("").to_string()
                } else {
                    v
                }
            };
            let args = if defined("args") { json!(json!(agent_args(b("args"))).to_string()) } else { r["args"].clone() };
            let env = if defined("env") { json!(agent_env(b("env")).to_string()) } else { r["env"].clone() };
            db::run("UPDATE agents SET name = ?, command = ?, args = ?, env = ? WHERE id = ?", params![keep("name"), keep("command"), args, env, id]);
        }
        server::emit(json!({ "type": "agents" }));
        return ok(json!({}));
    }
    // What the agent says about itself: its version and its ways to log in (Settings › Agents).
    if let (Some(c), "GET") = (re!(r"^/api/agents/([A-Za-z0-9_-]+)/describe$").captures(path), m) {
        let id = c.get(1).map_or("", |x| x.as_str());
        let Some(spec) = agents::spec_of(id) else { fail!(409, "{} is not installed", agents::agent_name(Some(id))) };
        return match acp::describe(&spec).await {
            Ok(d) => ok(serde_json::to_value(d)?),
            Err(e) => fail!(502, "{} did not start: {e}", agents::agent_name(Some(id))),
        };
    }
    // Log in with one of the agent's methods. The agent does it itself (Codex opens the browser); this waits until it is done.
    if let (Some(c), "POST") = (re!(r"^/api/agents/([A-Za-z0-9_-]+)/login$").captures(path), m) {
        let id = c.get(1).map_or("", |x| x.as_str());
        let Some(spec) = agents::spec_of(id) else { fail!(409, "{} is not installed", agents::agent_name(Some(id))) };
        match acp::authenticate(&spec, &str_of(b("methodId")), Duration::from_secs(5 * 60)).await {
            Ok(true) => {}
            Ok(false) => fail!(409, "the login did not complete"),
            Err(e) => fail!(502, "the login failed: {e}"),
        }
        return ok(json!({}));
    }

    if path == "/api/gh/retry" && m == "POST" {
        server_ops::forget_gh_logins(); // pick up a `gh auth login` done since the last try
        server_ops::refresh_all_prs().await;
        return ok(json!({}));
    }

    // ----- operator -----

    if path == "/api/operator" && m == "GET" {
        let since = server::get_meta("operator_since").unwrap_or_default();
        let mut messages = db::all("SELECT id, author, body, committed_at AS at, state FROM messages WHERE todo_id IS NULL AND committed_at >= ? ORDER BY id", params![since]);
        // Item 127: each reply lists the tools the operator used since the message before it.
        let used = db::all("SELECT kind, detail, at FROM events WHERE todo_id IS NULL AND actor = 'operator' AND at >= ? ORDER BY id", params![since]);
        let mut prev = since.clone();
        for msg in messages.iter_mut() {
            let at = msg.s("at").unwrap_or("").to_string();
            if msg.s("author") == Some("operator") {
                let tools: Vec<String> = used
                    .iter()
                    .filter(|e| e.s("at").is_some_and(|a| a > prev.as_str() && a <= at.as_str()))
                    .map(|e| format!("{}({})", e.s("kind").unwrap_or(""), e.s("detail").unwrap_or("")))
                    .collect();
                msg["tools"] = json!(tools);
            }
            prev = at;
        }
        let ctx = server::get_meta("operator_ctx").filter(|c| !c.is_empty()).map(|c| util::parse(&c)).unwrap_or(Value::Null);
        return ok(json!({
            "status": if runner::is_running(None) { "running" } else { "idle" },
            "session_id": server::get_meta("operator_session_id"),
            "ctx": ctx,
            "messages": messages,
            "draft": server::get_draft("operator"),
        }));
    }
    if path == "/api/operator/messages" && m == "POST" {
        let text = str_of(b("body")).trim().to_string();
        if text.is_empty() {
            fail!(400, "empty message");
        }
        let id = runner::commit_operator_message(&text);
        server::set_draft("operator", "");
        return ok(db::get("SELECT id, author, body, committed_at AS at, state FROM messages WHERE id = ?", params![id]).unwrap_or(Value::Null));
    }
    if path == "/api/operator/draft" && m == "PUT" {
        server::set_draft("operator", &str_of(b("body")));
        server::changed(None);
        return ok(json!({}));
    }
    if path == "/api/operator/new-session" && m == "POST" {
        if runner::is_running(None) {
            fail!(409, "the operator is running; wait for its reply");
        }
        // The next message starts a new session id. Older messages stay in the database but are hidden.
        server::set_meta("operator_session_id", None);
        server::set_meta("operator_ctx", None);
        server::set_meta("operator_since", Some(&now()));
        server::add_event(None, "new operator session", "user", None, None);
        return ok(json!({}));
    }

    fail!(404, "not found")
}

// ---------- the fetch handler ----------

/// Item 120: a random secret per start. Every /api/ request must carry it, so a program that can reach the port but
/// cannot read ~/.switchboard/token (another user, a container on the host network) gets no data and no shell.
pub fn token() -> &'static str {
    static T: OnceLock<String> = OnceLock::new();
    T.get_or_init(|| rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect())
}

/// One fallback handler doing what Bun.serve's fetch did, in the same order.
pub fn router() -> Router {
    Router::new().fallback(handle).layer(DefaultBodyLimit::max(MAX_BODY))
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// `fail(e.message ?? String(e), e.status ?? 500)`
fn error_response(e: &anyhow::Error) -> Response {
    match e.downcast_ref::<util::HttpError>() {
        Some(h) => fail_resp(&h.message, h.status),
        None => fail_resp(&e.to_string(), 500),
    }
}

async fn handle(req: Request) -> Response {
    let port = util::port();
    let host = format!("{}:{port}", util::HOST);
    // Blocks DNS-rebinding pages: the Host header must name this loopback server.
    let h = header_str(req.headers(), "host").unwrap_or("");
    if h != host && h != format!("localhost:{port}") {
        return fail_resp("bad host", 403);
    }
    // Cross-site forms can only send simple content types; requiring JSON forces a CORS preflight, which this server never answers.
    if req.method() != "GET" && !header_str(req.headers(), "content-type").is_some_and(|c| c.starts_with("application/json")) {
        return fail_resp("JSON body required", 415);
    }
    let path = pathname(req.uri());
    // The page, the icon and the bundled libraries hold no data, so they stay open.
    let app = util::app_dir();
    let static_file = |p: String, h: &'static [(header::HeaderName, &'static str)]| async move { file_response(&p, h).await.unwrap_or_else(|e| error_response(&e)) };
    if path == "/" {
        return static_file(join(app, "index.html"), &[]).await;
    }
    if path == "/favicon.svg" {
        return static_file(join(app, "images/icons/flat/switchboard-flat-small.svg"), &[]).await;
    }
    if let Some(rest) = path.strip_prefix("/vendor").filter(|_| path.starts_with("/vendor/")) {
        let vendor = join(app, "vendor");
        // PORT: a malformed %-sequence made decodeURIComponent throw outside the try, which Bun answered with a 500.
        let Ok(decoded) = decode_uri_component(rest) else { return fail_resp("URI malformed", 500) };
        let file = resolve(&vendor, &format!(".{decoded}"));
        if !helpers::is_inside(&file, &vendor) || file.starts_with(&join(&vendor, "src")) || !util::exists(&file) {
            return fail_resp("not found", 404);
        }
        return static_file(file, &[(header::CACHE_CONTROL, "no-cache")]).await;
    }
    // The agents' route to the app's MCP tools: each agent session carries its own secret instead of the launch token.
    if path == "/mcp/app" {
        return apptools::handle_mcp(req).await;
    }
    let q = Query::of(req.uri());
    if !helpers::token_ok(token(), header_str(req.headers(), "x-switchboard-token"), q.get("token")) {
        return fail_resp("open the link the server printed at start", 401);
    }
    // Browsers apply no CORS to WebSockets, so any website could open one; the Origin check stops websites, the token programs.
    if let Some(c) = re!(r"^/api/todos/([0-9]+)/terminals/([0-9]+)/ws$").captures(&path) {
        let origin = header_str(req.headers(), "origin").unwrap_or("");
        if origin != format!("http://{host}") && origin != format!("http://localhost:{port}") {
            return fail_resp("bad origin", 403);
        }
        let (todo_id, term_id) = (cap_id(&c, 1), cap_id(&c, 2));
        let (mut parts, _) = req.into_parts();
        return match axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(up) => terms::ws(up, todo_id, term_id),
            Err(_) => fail_resp("websocket upgrade failed", 400),
        };
    }
    // PORT: a panic (an SQL error, a bug) is answered like a thrown error in server.ts: a 500 with its message.
    let r = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(api(req, &path))).await;
    match r {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => error_response(&e),
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "internal error".into());
            fail_resp(&msg, 500)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn req(method: &str, uri: &str, host: &str, ctype: Option<&str>) -> Request {
        let mut b = axum::http::Request::builder().method(method).uri(uri).header("host", host);
        if let Some(c) = ctype {
            b = b.header("content-type", c);
        }
        b.body(Body::from("{}")).unwrap()
    }
    async fn send(r: Request) -> (StatusCode, HeaderMap, String) {
        let resp = router().oneshot(r).await.unwrap();
        let (s, h) = (resp.status(), resp.headers().clone());
        let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (s, h, String::from_utf8_lossy(&b).into_owned())
    }
    fn host() -> String {
        format!("127.0.0.1:{}", util::port())
    }

    #[tokio::test]
    async fn bad_host_is_403() {
        let (s, h, b) = send(req("GET", "/api/todos", "evil.example:4477", None)).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert_eq!(h["content-type"], "application/json;charset=utf-8");
        assert_eq!(b, r#"{"error":"bad host"}"#);
    }
    #[tokio::test]
    async fn non_json_post_is_415() {
        let (s, _, b) = send(req("POST", "/api/todos", &host(), Some("text/plain"))).await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(b, r#"{"error":"JSON body required"}"#);
        let (s, _, _) = send(req("POST", "/api/todos", &host(), None)).await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    #[tokio::test]
    async fn index_is_served_without_token() {
        let (s, h, b) = send(req("GET", "/", &format!("localhost:{}", util::port()), None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h["content-type"], "text/html;charset=utf-8");
        assert!(b.contains("<html") || b.contains("<!doctype") || b.contains("<!DOCTYPE"));
    }
    #[tokio::test]
    async fn missing_token_is_401() {
        let (s, _, b) = send(req("GET", "/api/todos", &host(), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(b, r#"{"error":"open the link the server printed at start"}"#);
    }
    #[test]
    fn token_is_64_hex() {
        assert_eq!(token().len(), 64);
        assert!(token().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(token(), token());
    }
    #[test]
    fn url_helpers() {
        assert_eq!(pathname(&"/vendor/../server.ts".parse().unwrap()), "/server.ts");
        assert_eq!(pathname(&"/vendor/%2e%2E/x".parse().unwrap()), "/x");
        assert_eq!(pathname(&"/a/b/..".parse().unwrap()), "/a/");
        assert_eq!(pathname(&"/api/todos/3".parse().unwrap()), "/api/todos/3");
        let q = Query::of(&"/x?token=a%2Bb+c&raw".parse().unwrap());
        assert_eq!(q.get("token"), Some("a+b c"));
        assert!(q.has("raw"));
        assert_eq!(decode_uri_component("/a%20b"), Ok("/a b".into()));
        assert!(decode_uri_component("/a%zz").is_err());
        assert_eq!(resolve("/v", "./x/../y"), "/v/y");
        assert_eq!(relative("/a/b", "/a/b/c/d.md"), "c/d.md");
        assert_eq!(relative("/a/b", "/a/x"), "../x");
    }
    #[test]
    fn js_conversions() {
        assert_eq!(js_string(Some(&json!(2.0))), "2");
        assert_eq!(js_string(None), "undefined");
        assert_eq!(str_of(None), "");
        assert_eq!(str_of(Some(&json!(5))), "5");
        assert!(js_number(Some(&json!("x"))).is_nan());
        assert_eq!(num(2.0), json!(2));
        assert!(!mtime_differs(None, Some(&Value::Null)));
        assert!(mtime_differs(None, None));
        assert!(!mtime_differs(Some(1730000000123.4567), Some(&json!(1730000000123.4567))));
        assert!(mtime_differs(Some(1.0), Some(&json!(2.0))));
    }
}

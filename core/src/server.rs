//! Port of server.ts, first part: storage helpers, live updates, the git state cache and the TODO summaries.
//! PRs, the worktree scan, ports and the shared writes are in server_ops.rs; the HTTP routes in http.rs.

use crate::util::{self, basename, home, join, now, J};
use crate::{db, helpers, params, runner, server_ops};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{mpsc, Notify};

// ---------- storage ----------

pub fn todo_row(id: i64) -> Option<Value> {
    db::get("SELECT * FROM todos WHERE id = ?", params![id])
}
/// A TODO's topic: None before routing and for a TODO without a topic (801).
pub fn project_row(id: Option<i64>) -> Option<Value> {
    db::get("SELECT * FROM projects WHERE id = ?", params![id?])
}
/// work: the file sits in work/, where agents keep output meant for themselves; the Files tab folds those.
pub fn files_of(todo_id: i64) -> Vec<Value> {
    let work = join(&artifact_dir(todo_id), "work") + util::SEP;
    db::all("SELECT id, path, session_no, by_user FROM files WHERE todo_id = ? ORDER BY id", params![todo_id])
        .into_iter()
        .map(|mut f| {
            let w = f.s("path").is_some_and(|p| p.starts_with(&work));
            f["work"] = json!(w);
            f
        })
        .collect()
}
pub fn comments_of(todo_id: i64, where_: &str) -> Vec<Value> {
    db::all(&format!("SELECT * FROM comments WHERE todo_id = ? AND {where_} ORDER BY path, line, id"), params![todo_id])
}
/// Since 801 the folder no longer depends on the topic, so it exists before routing and never moves.
pub fn artifact_dir(todo_id: i64) -> String {
    join(&join(home(), "data"), &todo_id.to_string())
}
pub fn attachments_of(message_id: i64) -> Vec<Value> {
    db::all("SELECT id, name, path FROM attachments WHERE message_id = ? ORDER BY id", params![message_id])
}

/// Files the user sends with a message, base64 inside the JSON body.
#[derive(Clone, Deserialize)]
pub struct Upload {
    pub name: String,
    pub data: String,
}
/// Written to <artifact folder>/uploads/<message id>-<n>-<name>, so two pasted "image.png" never collide.
pub fn save_attachments(todo_id: i64, message_id: i64, uploads: &[Upload]) {
    use base64::Engine;
    if uploads.is_empty() {
        return;
    }
    let dir = join(&artifact_dir(todo_id), "uploads");
    let _ = std::fs::create_dir_all(&dir);
    for (i, u) in uploads.iter().enumerate() {
        let name = helpers::safe_file_name(&u.name);
        let path = join(&dir, &format!("{message_id}-{}-{name}", i + 1));
        let bytes = base64::engine::general_purpose::STANDARD.decode(u.data.trim()).unwrap_or_default();
        let _ = std::fs::write(&path, bytes);
        db::run("INSERT INTO attachments (message_id, name, path) VALUES (?, ?, ?)", params![message_id, name, path]);
    }
}

pub fn get_meta(key: &str) -> Option<String> {
    db::get("SELECT value FROM meta WHERE key = ?", params![key]).and_then(|r| r.s("value").map(String::from))
}
pub fn set_meta(key: &str, value: Option<&str>) {
    match value {
        None => db::run("DELETE FROM meta WHERE key = ?", params![key]),
        Some(v) => db::run("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, v]),
    };
}
pub fn get_draft(key: &str) -> String {
    db::get("SELECT body FROM drafts WHERE key = ?", params![key]).and_then(|r| r.s("body").map(String::from)).unwrap_or_default()
}
pub fn set_draft(key: &str, body: &str) {
    if body.trim().is_empty() {
        db::run("DELETE FROM drafts WHERE key = ?", params![key]);
    } else {
        db::run("INSERT INTO drafts (key, body) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body", params![key, body]);
    }
}
/// A pill click: false when the agent no longer offers this text; else logs the pick, before the run's launch events.
pub fn pick_suggestion(id: i64, text: &str) -> bool {
    let offered = todo_row(id).map(|t| t.parsed("suggestions")).unwrap_or(Value::Null);
    if !offered.as_array().is_some_and(|a| a.iter().any(|s| s.as_str() == Some(text))) {
        return false;
    }
    add_event(Some(id), "suggestion picked", "user", Some(text), None);
    true
}

/// 801: artifact folders move once from data/<project slug>/<todo id>/ to data/<todo id>/, and the stored file and
/// attachment paths with them. HOME is a real path, so the stored paths start with these folders exactly.
/// ponytail: a project slug that is a number ("12") shares its name with TODO 12's new folder; that TODO's folder stays
/// and the log says so.
pub fn move_artifact_folders() {
    if get_meta("artifact_layout").as_deref() == Some("todo-id") {
        return;
    }
    let rows = db::all("SELECT t.id, p.slug FROM todos t JOIN projects p ON p.id = t.project_id ORDER BY t.id", params![]);
    let mut blocked = false;
    for r in &rows {
        let (id, slug) = (r.i("id").unwrap_or(0), r.s("slug").unwrap_or(""));
        let a = join(&join(&join(home(), "data"), slug), &id.to_string());
        let b = artifact_dir(id);
        if !util::exists(&a) {
            continue;
        }
        if util::exists(&b) {
            eprintln!("artifact folders: {b} exists already, so {a} stays where it is");
            continue;
        }
        // Windows refuses to rename a folder while a process holds a file in it open; the next start tries again.
        if let Err(e) = std::fs::rename(&a, &b) {
            blocked = true;
            eprintln!("artifact folders: could not move {a} ({e}), it stays until the next start");
            continue;
        }
        for tbl in ["attachments", "files"] {
            db::run(
                &format!("UPDATE {tbl} SET path = ? || substr(path, ?) WHERE substr(path, 1, ?) = ?"),
                params![b.clone() + util::SEP, a.len() + 2, a.len() + 1, a.clone() + util::SEP],
            );
        }
        db::run("INSERT INTO events (todo_id, kind, actor, detail, at) VALUES (?, 'moved', 'app', ?, ?)", params![id, format!("data/{slug}/{id}/ → data/{id}/"), now()]);
        println!("artifact folders: moved data/{slug}/{id}/ to data/{id}/");
    }
    let slugs: HashSet<&str> = rows.iter().filter_map(|r| r.s("slug")).collect();
    for slug in slugs {
        let _ = std::fs::remove_dir(join(&join(home(), "data"), slug)); // only when empty
    }
    if !blocked {
        set_meta("artifact_layout", Some("todo-id"));
    }
}

// ---------- live updates (one SSE stream) ----------
// A TODO event carries the TODO's summary and the transcript rows that changed, so the page patches them in place.
// `full` asks the page to refetch the TODO's whole transcript: anything but set_todo, add_event and set_event sets it,
// so a write the patches do not cover (messages, sessions, files, comments) is never missed.

static CLIENTS: LazyLock<Mutex<Vec<mpsc::UnboundedSender<String>>>> = LazyLock::new(Default::default);
/// The page's event stream (GET /api/events): each item is one event's JSON.
pub fn subscribe() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    CLIENTS.lock().unwrap().push(tx);
    rx
}
fn send(s: &str) {
    CLIENTS.lock().unwrap().retain(|c| c.send(s.to_string()).is_ok());
}

enum Pending {
    Plain(Value),
    Todo { todo: i64, full: bool, events: Vec<i64> },
}
/// One change often writes a status, a message and an event; they go out together, 50 ms after the first.
static OUTBOX: LazyLock<Mutex<(Vec<String>, HashMap<String, Pending>)>> = LazyLock::new(Default::default);
/// Flushes run one after another, so events leave in the order they were made.
static FLUSHING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);
/// seq numbers the flushes. The rows a flush sends are read before any await, and GET /api/todos/:id reports the
/// number current when it read the transcript: the page drops a patch that is not newer than the transcript it loaded.
static SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
pub fn flush_seq() -> i64 {
    SEQ.load(std::sync::atomic::Ordering::SeqCst)
}

fn queue(key: String, ev: Pending) {
    let mut o = OUTBOX.lock().unwrap();
    if !o.1.contains_key(&key) {
        o.0.push(key.clone());
    }
    o.1.insert(key, ev);
    if o.0.len() == 1 {
        util::after(50, async { flush().await });
    }
}
async fn flush() {
    let _turn = FLUSHING.lock().await;
    let batch: Vec<Pending> = {
        let mut o = OUTBOX.lock().unwrap();
        let keys = std::mem::take(&mut o.0);
        keys.into_iter().filter_map(|k| o.1.remove(&k)).collect()
    };
    if batch.is_empty() {
        return;
    }
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    // The rows first, all in this tick; the summaries may wait for git.
    let rows: Vec<Option<Vec<Value>>> = batch
        .iter()
        .map(|ev| match ev {
            Pending::Todo { full: false, events, .. } if !events.is_empty() => Some(
                db::all(
                    &format!("SELECT id, kind, actor, detail, at, session_no, meta FROM events WHERE id IN ({})", events.iter().map(i64::to_string).collect::<Vec<_>>().join(",")),
                    params![],
                )
                .into_iter()
                .map(event_out)
                .collect(),
            ),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for (ev, events) in batch.into_iter().zip(rows) {
        out.push(match ev {
            Pending::Plain(v) => v.to_string(),
            Pending::Todo { todo, full, .. } => {
                let s = summaries("t.id = ?", params![todo]).await.into_iter().next();
                // A TODO deleted meanwhile has no summary: the page refetches the list.
                match s {
                    Some(s) => {
                        let mut ev = json!({ "type": "todo", "id": todo, "seq": n, "todo": s });
                        if let Some(e) = events {
                            ev["events"] = Value::Array(e);
                        }
                        if full {
                            ev["full"] = json!(true);
                        }
                        ev.to_string()
                    }
                    None => json!({ "type": "todos" }).to_string(),
                }
            }
        });
    }
    for s in out {
        send(&s);
    }
}
/// An event row as the page reads it: meta parsed.
pub fn event_out(mut e: Value) -> Value {
    let meta = e.parsed("meta");
    e["meta"] = meta;
    e
}

pub fn emit(ev: Value) {
    if ev.s("type") == Some("todo") {
        if let Some(id) = ev.i("id") {
            return note_todo(id, Part::Summary, true);
        }
    }
    queue(ev.to_string(), Pending::Plain(ev));
}
/// What changed: `Full`, the transcript may have changed anywhere; `Event(id)`, that event row; `Summary`, only the todos row.
pub enum Part {
    Full,
    Summary,
    Event(i64),
}
/// Every note but a git refresh's own (`git: false`) marks the worktree's git state for a new look.
pub fn note_todo(id: i64, part: Part, git: bool) {
    {
        let mut o = OUTBOX.lock().unwrap();
        let key = format!("todo:{id}");
        let fresh = !o.1.contains_key(&key);
        if fresh {
            o.0.push(key.clone());
            o.1.insert(key.clone(), Pending::Todo { todo: id, full: false, events: vec![] });
            if o.0.len() == 1 {
                util::after(50, async { flush().await });
            }
        }
        if let Some(Pending::Todo { full, events, .. }) = o.1.get_mut(&key) {
            match part {
                Part::Full => *full = true,
                Part::Event(e) => {
                    if !events.contains(&e) {
                        events.push(e)
                    }
                }
                Part::Summary => {}
            }
        }
    }
    if git {
        git_touched(id);
    }
}
pub fn changed(todo_id: Option<i64>) {
    match todo_id {
        None => emit(json!({ "type": "operator" })),
        Some(id) => note_todo(id, Part::Full, true),
    }
}

/// `touch` = false for changes that must not move the TODO up its lane (last tool, read marker). `fields` is an object
/// of column → value.
pub fn set_todo(id: i64, fields: Value, touch: bool) {
    let mut f: Map<String, Value> = fields.as_object().cloned().unwrap_or_default();
    if touch {
        f.insert("updated_at".into(), json!(now()));
    }
    if !f.is_empty() {
        let sets: Vec<String> = f.keys().map(|k| format!("{k} = ?")).collect();
        let mut ps: Vec<Value> = f.values().cloned().collect();
        ps.push(json!(id));
        db::run(&format!("UPDATE todos SET {} WHERE id = ?", sets.join(", ")), &ps);
    }
    note_todo(id, Part::Summary, true);
}
/// The live session's number (items 68, 77): agent and app messages, events and files belong to the session they
/// happen in; a user message gets its session when it is delivered (take() in runner.rs).
pub fn live_session_no(todo_id: Option<i64>) -> Option<i64> {
    db::get("SELECT MAX(no) AS n FROM sessions WHERE todo_id = ?", params![todo_id?]).and_then(|r| r.i("n"))
}
pub fn sessions_of(todo_id: i64) -> Vec<Value> {
    db::all("SELECT no, session_id, started_at, ended_at, agent FROM sessions WHERE todo_id = ? ORDER BY no", params![todo_id])
}
pub fn add_message(todo_id: Option<i64>, author: &str, body: &str, state: &str, file_id: Option<i64>, via: Option<&str>) -> i64 {
    let at = now();
    let delivered = state == "delivered";
    let id = db::run(
        "INSERT INTO messages (todo_id, author, body, state, file_id, committed_at, delivered_at, session_no, via) VALUES (?,?,?,?,?,?,?,?,?)",
        params![todo_id, author, body, state, file_id, at, delivered.then(|| at.clone()), if delivered { live_session_no(todo_id) } else { None }, via],
    )
    .last_insert_rowid;
    if let Some(t) = todo_id {
        db::run("UPDATE todos SET updated_at = ? WHERE id = ?", params![at, t]);
    }
    changed(todo_id);
    id
}
pub fn add_event(todo_id: Option<i64>, kind: &str, actor: &str, detail: Option<&str>, meta: Option<Value>) -> i64 {
    let id = db::run(
        "INSERT INTO events (todo_id, kind, actor, detail, at, session_no, meta) VALUES (?,?,?,?,?,?,?)",
        params![todo_id, kind, actor, detail, now(), live_session_no(todo_id), meta.map(|m| m.to_string())],
    )
    .last_insert_rowid;
    match todo_id {
        None => changed(None),
        Some(t) => note_todo(t, Part::Event(id), true),
    }
    id
}
/// A transcript row that grows while the agent works (a tool call's status, input, diff and output; the plan).
pub fn set_event(id: i64, todo_id: i64, detail: Option<&str>, meta: Option<Value>) {
    db::run("UPDATE events SET detail = ?, meta = ? WHERE id = ?", params![detail, meta.map(|m| m.to_string()), id]);
    note_todo(todo_id, Part::Event(id), true);
}

// ---------- git state of a worktree (local, cached per path) ----------

/// Two git processes, ~0.03 s each. --no-optional-locks: a plain `git status` may rewrite the index and hold
/// .git/index.lock, which would fail a `git commit` the agent runs in the same worktree at that moment.
/// None when the worktree is gone or not a repository.
pub async fn git_state(wt: &str) -> Option<Value> {
    let st = util::sh(&["git", "-C", wt, "--no-optional-locks", "status", "--porcelain=v2", "--branch"], Default::default()).await;
    if !st.ok {
        return None;
    }
    let s = helpers::parse_git_status(&st.out);
    let branch = s.s("branch").map(String::from);
    let mut refs = vec!["refs/remotes/origin/HEAD".to_string()];
    if let Some(b) = &branch {
        refs.push(format!("refs/remotes/origin/{b}"));
    }
    let mut cmd: Vec<String> = ["git", "-C", wt, "for-each-ref", "--format=%(refname) %(symref:short) %(ahead-behind:HEAD)"].map(String::from).to_vec();
    cmd.extend(refs);
    let r = util::sh_v(&cmd, Default::default()).await;
    Some(util::merge(&s, helpers::parse_ref_counts(if r.ok { &r.out } else { "" }, branch.as_deref())))
}

// The list reads git state from this cache, so a list fetch starts no git process once a path was seen. A change to
// the TODO (an agent's tool call, a status) or terminal input marks its worktree for a new look, at most one per
// second per path; a read of an entry older than GIT_TTL_MS does too, for changes made outside the app. A new look
// that finds a different state sends the TODO's summary again.
struct GitEntry {
    value: Option<Value>,
    at: i64,
    running: bool,
    again: bool,
    timer: bool,
    done: Arc<Notify>,
}
static GIT_CACHE: LazyLock<Mutex<HashMap<String, GitEntry>>> = LazyLock::new(Default::default);
const GIT_MIN_MS: i64 = 1000;
const GIT_TTL_MS: i64 = 10_000;

/// Starts a look unless one runs (then it runs again after). Returns the notifier of its end.
fn git_look(wt: String) -> Arc<Notify> {
    let mut c = GIT_CACHE.lock().unwrap();
    let e = c.get_mut(&wt).expect("git entry");
    let done = e.done.clone();
    if e.running {
        e.again = true;
        return done;
    }
    e.running = true;
    let (before, first) = (e.value.clone(), e.at == 0);
    drop(c);
    util::spawn(async move {
        let v = git_state(&wt).await;
        let again = {
            let mut c = GIT_CACHE.lock().unwrap();
            let e = c.get_mut(&wt).expect("git entry");
            e.value = v.clone();
            e.at = util::now_ms();
            e.running = false;
            std::mem::take(&mut e.again)
        };
        if !first && v != before {
            for r in db::all("SELECT id FROM todos WHERE worktree_path = ?", params![wt]) {
                note_todo(r.i("id").unwrap_or(0), Part::Summary, false);
            }
        }
        GIT_CACHE.lock().unwrap().get(&wt).map(|e| e.done.clone()).expect("git entry").notify_waiters();
        if again {
            git_soon(&wt);
        }
    });
    done
}
fn git_soon(wt: &str) {
    let mut c = GIT_CACHE.lock().unwrap();
    let Some(e) = c.get_mut(wt) else { return }; // never read: the next read looks
    if e.running {
        e.again = true;
        return;
    }
    if e.timer {
        return;
    }
    e.timer = true;
    let wait = (e.at + GIT_MIN_MS - util::now_ms()).max(0) as u64;
    let wt = wt.to_string();
    util::after(wait, async move {
        if let Some(e) = GIT_CACHE.lock().unwrap().get_mut(&wt) {
            e.timer = false;
        }
        git_look(wt);
    });
}
pub async fn cached_git_state(wt: &str) -> Option<Value> {
    let done = {
        let mut c = GIT_CACHE.lock().unwrap();
        let e = c.entry(wt.to_string()).or_insert_with(|| GitEntry { value: None, at: 0, running: false, again: false, timer: false, done: Arc::new(Notify::new()) });
        if e.at != 0 {
            let stale = util::now_ms() - e.at > GIT_TTL_MS;
            let v = e.value.clone();
            drop(c);
            if stale {
                git_soon(wt);
            }
            return v;
        }
        e.done.clone()
    };
    // First read: wait for a look. The waiter is registered under the lock, so a look ending meanwhile is not missed.
    let notified = done.notified();
    tokio::pin!(notified);
    let running = {
        let c = GIT_CACHE.lock().unwrap();
        notified.as_mut().enable();
        match c.get(wt) {
            Some(e) if e.at != 0 => return e.value.clone(),
            Some(e) => e.running,
            None => false,
        }
    };
    if !running {
        git_look(wt.to_string());
    }
    notified.await;
    GIT_CACHE.lock().unwrap().get(wt).and_then(|e| e.value.clone())
}
fn worktree_of(id: i64) -> Option<String> {
    db::get("SELECT worktree_path FROM todos WHERE id = ?", params![id]).and_then(|r| r.s("worktree_path").map(String::from))
}
pub fn git_touched(todo_id: i64) {
    if let Some(wt) = worktree_of(todo_id) {
        git_soon(&wt);
    }
}

// ---------- read models ----------

const SUMMARY_SQL: &str = "
SELECT t.*, p.name AS project_name,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id) AS message_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'queued') AS queued_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'pending') AS pending_count,
  (SELECT COUNT(*) FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.todo_id = t.id AND m.state = 'pending') AS pending_attachments,
  (SELECT COUNT(*) FROM sessions s WHERE s.todo_id = t.id) AS session_count,
  (SELECT author FROM messages m WHERE m.todo_id = t.id ORDER BY m.id LIMIT 1) AS creator,
  EXISTS (SELECT 1 FROM messages m WHERE m.todo_id = t.id AND m.author = 'agent'
          AND (t.last_read_at IS NULL OR m.committed_at > t.last_read_at)) AS unread,
  EXISTS (SELECT 1 FROM drafts d WHERE d.key = 'todo:' || t.id) AS has_draft,
  (SELECT json_object('author', author, 'body', body, 'at', committed_at,
     'files', (SELECT group_concat(name, ', ') FROM (SELECT name FROM attachments a WHERE a.message_id = m.id ORDER BY a.id))) FROM messages m
   WHERE m.todo_id = t.id ORDER BY m.id DESC LIMIT 1) AS last_message,
  (SELECT json_object('body', body, 'at', committed_at) FROM messages m
   WHERE m.todo_id = t.id AND m.author = 'agent' ORDER BY m.id DESC LIMIT 1) AS last_agent_message,
  (SELECT json_group_array(path) FROM (SELECT path FROM files f WHERE f.todo_id = t.id ORDER BY f.id)) AS file_paths,
  (SELECT json_object('kind', kind, 'detail', detail, 'at', at) FROM events e
   WHERE e.todo_id = t.id AND e.kind IN ('scheduled run', 'schedule skipped') ORDER BY e.id DESC LIMIT 1) AS schedule_last
FROM todos t LEFT JOIN projects p ON p.id = t.project_id";

/// Item 136: the TODO shows running only for the agent's background tasks, between turns.
pub fn background_wait(r: &Value) -> bool {
    let id = r.i("id").unwrap_or(0);
    r.s("status") == Some("running") && !runner::is_running(Some(id)) && runner::background_count(id) > 0
}

/// Git state comes from cached_git_state, PRs from the PR store in memory: a list fetch runs no git or gh process.
pub async fn summaries(where_: &str, args: &[Value]) -> Vec<Value> {
    let rows = db::all(&format!("{SUMMARY_SQL} WHERE {where_} ORDER BY t.updated_at DESC"), args);
    let gits = futures_util::future::join_all(rows.iter().map(|r| async move {
        match r.s("worktree_path") {
            Some(wt) => cached_git_state(wt).await,
            None => None,
        }
    }))
    .await;
    rows.into_iter().zip(gits).map(|(r, git)| summary(r, git)).collect()
}

fn summary(r: Value, git: Option<Value>) -> Value {
    let Value::Object(mut o) = r else { return r };
    let take_json = |o: &mut Map<String, Value>, k: &str| -> Value {
        match o.shift_remove(k) {
            Some(Value::String(s)) => util::parse(&s),
            _ => Value::Null,
        }
    };
    let file_paths = take_json(&mut o, "file_paths");
    let ctx = take_json(&mut o, "ctx");
    let permission = take_json(&mut o, "permission");
    let config = take_json(&mut o, "config");
    let overrides = take_json(&mut o, "overrides");
    let auto_effort = take_json(&mut o, "auto_effort");
    let suggestions = take_json(&mut o, "suggestions");
    let routing = take_json(&mut o, "routing");
    let schedule = take_json(&mut o, "schedule");
    let schedule_last = take_json(&mut o, "schedule_last");
    let places = take_json(&mut o, "places");
    let r = Value::Object(o);
    let id = r.i("id").unwrap_or(0);
    let names: Vec<String> = file_paths.as_array().map(|a| a.iter().filter_map(Value::as_str).map(basename).collect()).unwrap_or_default();
    let branch = git.as_ref().and_then(|g| g.s("branch").map(String::from));
    let (pr, checked_at, gh_error) = server_ops::pr_for(r.i("project_id"), r.i("pr_number"), branch.as_deref());
    let mut s = r.clone();
    let parse_field = |v: &Value, k: &str| v.parsed(k);
    let fields = json!({
        "agent": r.s("agent").filter(|a| !a.is_empty()).unwrap_or(crate::agents::DEFAULT_AGENT),
        "attention": r.truthy("attention"), "unread": r.truthy("unread"), "has_draft": r.truthy("has_draft"),
        "last_message": parse_field(&r, "last_message"),
        "last_agent_message": parse_field(&r, "last_agent_message"),
        "file_names": names, "file_count": names.len(), "ctx": ctx, "plan": parse_field(&r, "plan"),
        "permission": permission,
        "config": config, "overrides": if overrides.is_null() { json!({}) } else { overrides }, "auto_effort": auto_effort,
        "suggestions": suggestions, "routing": routing,
        // Change topic: the worktree, branch and PR each other topic keeps for this TODO.
        "places": places.as_object().map(|m| m.iter().map(|(k, v)| {
            util::merge(&json!({ "project_id": if k == "none" { Value::Null } else { json!(k.parse::<i64>().ok()) } }), v.clone())
        }).collect::<Vec<_>>()).unwrap_or_default(),
        // Item 144: the schedule, its chip text, and its last firing (run or skipped) for the chip's amber state.
        "schedule": schedule.clone(),
        "schedule_text": if schedule.is_null() { Value::Null } else { json!(helpers::schedule_text(&schedule)) },
        "schedule_last": schedule_last,
        "branch": branch, "git": git, "pr": pr.map(|p| util::merge(&p, json!({ "via": if r.i("pr_number").is_some() { json!("set_pr") } else { Value::Null } }))),
        "pr_checked_at": checked_at, "gh_error": gh_error,
        // Item 136: the agent's background tasks still running, and whether the TODO shows running only for them.
        "background": runner::background_count(id), "background_wait": background_wait(&r), "background_tasks": runner::background_tasks(id),
    });
    if let (Some(o), Value::Object(f)) = (s.as_object_mut(), fields) {
        for (k, v) in f {
            o.insert(k, v);
        }
    }
    s
}

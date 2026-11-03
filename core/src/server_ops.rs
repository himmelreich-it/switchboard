//! Port of server.ts, second part: pull requests, the worktree scan, ports, the writes shared with runner.rs and
//! apptools.rs (createTodo, removeTodo, setSchedule), and the migration from auto-coder. See PORTING.md.

use crate::server::{self, add_event, add_message, artifact_dir, changed, emit, files_of, project_row, Upload};
use crate::util::{self, basename, home, home_dir, join, now, ShOpts, ShOut, J};
use crate::{acp, db, helpers, params, runner, terms};
use futures_util::future::{join_all, BoxFuture, FutureExt, Shared};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// util::sh with the command copied first, so the future borrows nothing (several run at once in tokio::join!).
fn sh(cmd: &[&str], opts: ShOpts) -> impl Future<Output = ShOut> + Send + 'static {
    let cmd: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    async move { util::sh_v(&cmd, opts).await }
}
fn timeout(ms: u64) -> ShOpts {
    ShOpts { timeout: Some(Duration::from_millis(ms)), ..Default::default() }
}
/// `[...new Set(v)]`: first occurrence wins, order kept.
fn uniq<T: Eq + Hash + Clone>(v: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(x.clone())).collect()
}
/// realpathSync, or the path as given when it cannot be resolved.
fn real(p: &str) -> String {
    util::realpath(p).unwrap_or_else(|_| p.to_string())
}

/// A promise that is already running and that several callers await (prInFlight, worktreeScanning).
type Flight = Shared<BoxFuture<'static, ()>>;
/// Starts `work` now, as a promise does, and returns a future that ends with it. `done` runs at its end, also after a
/// panic, which is logged as `<what>: <panic>` (the TS `.catch(console.error)`). The caller holds its in-flight
/// lock while calling this and storing the result, and `done` takes that lock, so `done` cannot run first.
fn start_flight(what: String, work: impl Future<Output = ()> + Send + 'static, done: impl FnOnce() + Send + 'static) -> Flight {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    util::spawn(async move {
        if let Err(e) = tokio::spawn(work).await {
            eprintln!("{what}: {e}");
        }
        done();
        let _ = tx.send(());
    });
    async move {
        let _ = rx.await;
    }
    .boxed()
    .shared()
}

// ---------- pull requests (GitHub, refreshed every 2 minutes and after each run, never on a list fetch) ----------

/// project id -> the last result; an error keeps the last good PRs. `error` is `{ kind, why }` (GhError).
#[derive(Clone, Default)]
struct PrStore {
    at: Option<String>,
    by_branch: HashMap<String, Value>,
    by_number: HashMap<i64, Value>,
    error: Option<Value>,
}
static PR_STORE: LazyLock<Mutex<HashMap<i64, PrStore>>> = LazyLock::new(Default::default);
static PR_IN_FLIGHT: LazyLock<Mutex<HashMap<i64, Flight>>> = LazyLock::new(Default::default);
/// project id -> the gh account that can read its repository
static GH_LOGIN_OF: LazyLock<Mutex<HashMap<i64, String>>> = LazyLock::new(Default::default);
/// every gh account on this machine; read once, again after an access failure
static GH_LOGINS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The PR the list shows for a TODO, from the PR store: set_pr's number wins over the worktree's branch.
/// Returns (the PR, when its project was last checked, the project's gh error) as in server.ts `summaries`.
pub fn pr_for(project_id: Option<i64>, pr_number: Option<i64>, branch: Option<&str>) -> (Option<Value>, Option<String>, Value) {
    let stores = PR_STORE.lock().unwrap();
    let Some(store) = project_id.and_then(|id| stores.get(&id)) else { return (None, None, Value::Null) };
    // set_pr wins over the branch: a review worktree's local branch need not match the PR's head branch.
    let pr = match (pr_number.filter(|n| *n != 0), branch.filter(|b| !b.is_empty())) {
        (Some(n), _) => store.by_number.get(&n).cloned(),
        (None, Some(b)) => store.by_branch.get(b).cloned(),
        (None, None) => None,
    };
    (pr, store.at.clone(), store.error.clone().unwrap_or(Value::Null))
}

/// One call per project for all its worktree TODOs. Each gh account sees only its own repositories, so the call runs
/// as each logged-in account in turn (GH_TOKEN for that call only; the active `gh` account is never switched).
/// The refresh starts at once; await the result to wait for it. A refresh already running for the project is shared.
pub fn refresh_prs(project_id: i64) -> impl Future<Output = ()> + Send + 'static {
    let mut m = PR_IN_FLIGHT.lock().unwrap();
    if let Some(p) = m.get(&project_id) {
        return p.clone();
    }
    let p = start_flight(format!("PR refresh for project {project_id}"), fetch_prs(project_id), move || {
        PR_IN_FLIGHT.lock().unwrap().remove(&project_id);
    });
    m.insert(project_id, p.clone());
    p
}
/// A TODO is looked up when it has a worktree (by its branch) or a PR registered with set_pr (by number, also without a worktree).
const PR_TODOS: &str = "removed_at IS NULL AND (worktree_path IS NOT NULL OR pr_number IS NOT NULL)";
pub async fn refresh_all_prs() {
    let ids = db::all(&format!("SELECT DISTINCT project_id AS id FROM todos WHERE project_id IS NOT NULL AND {PR_TODOS}"), params![]);
    join_all(ids.iter().filter_map(|r| r.i("id")).map(refresh_prs)).await;
}

/// `-f b<i>=<branch>` and `-F n<i>=<number>`: values go in as variables, never into the query text.
fn query_vars(branches: &[String], numbers: &[i64]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, b) in branches.iter().enumerate() {
        v.extend(["-f".to_string(), format!("b{i}={b}")]);
    }
    for (i, n) in numbers.iter().enumerate() {
        v.extend(["-F".to_string(), format!("n{i}={n}")]);
    }
    v
}

async fn fetch_prs(project_id: i64) {
    let p = project_row(Some(project_id));
    let todos = db::all(&format!("SELECT worktree_path, pr_number FROM todos WHERE project_id = ? AND {PR_TODOS}"), params![project_id]);
    let Some(p) = p else { return };
    if todos.is_empty() {
        return;
    }
    let remote = sh(&["git", "-C", p.s("local_path").unwrap_or(""), "remote", "get-url", "origin"], Default::default()).await;
    let repo = if remote.ok { helpers::parse_github_remote(&remote.out) } else { None };
    let Some(repo) = repo else { return }; // not on GitHub: no PR line
    let numbers = uniq(todos.iter().filter_map(|t| t.i("pr_number")).filter(|n| *n != 0));
    let gits = join_all(todos.iter().filter(|t| !t.truthy("pr_number")).filter_map(|t| t.s("worktree_path")).filter(|w| !w.is_empty()).map(server::cached_git_state)).await;
    let branches = uniq(gits.iter().flatten().filter_map(|g| g.s("branch")).filter(|b| !b.is_empty()).map(String::from));
    if numbers.is_empty() && branches.is_empty() {
        return;
    }
    let prev = PR_STORE.lock().unwrap().get(&project_id).cloned().unwrap_or_default();
    let query = helpers::pr_query(branches.len() as i64, numbers.len() as i64, helpers::PR_FIELDS);
    let o = match gh_repo_query(project_id, &repo, &query, query_vars(&branches, &numbers)).await {
        Ok(o) => o,
        Err(error) => {
            PR_STORE.lock().unwrap().insert(project_id, PrStore { error: Some(error), ..prev });
            emit(json!({ "type": "todos" }));
            return;
        }
    };
    let newest = |i: usize| &o[format!("b{i}").as_str()]["nodes"][0]; // pullRequests(...) returns a list
    let store = PrStore {
        at: Some(now()),
        error: None,
        by_branch: branches.iter().enumerate().filter(|(i, _)| util::truthy(newest(*i))).map(|(i, b)| (b.clone(), helpers::pr_from_node(newest(i)))).collect(),
        by_number: numbers
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let node = &o[format!("n{i}").as_str()];
                util::truthy(node).then(|| (*n, helpers::pr_from_node(node)))
            })
            .collect(),
    };
    PR_STORE.lock().unwrap().insert(project_id, store);
    emit(json!({ "type": "todos" }));
}

/// One `gh api graphql` call on the repository, for the PR refresh and the worktree scan. Each gh account sees only its
/// own repositories, so the call runs as each logged-in account in turn; the one that worked for the project goes first.
/// Ok: the `repository` object; Err: the GhError `{ kind, why }`.
async fn gh_repo_query(project_id: i64, repo: &Value, query: &str, vars: Vec<String>) -> Result<Value, Value> {
    let (owner, name) = (repo.s("owner").unwrap_or(""), repo.s("name").unwrap_or(""));
    let mut args: Vec<String> = ["gh", "api", "graphql", "-f"].map(String::from).to_vec();
    args.extend([format!("query={query}"), "-f".into(), format!("o={owner}"), "-f".into(), format!("n={name}")]);
    args.extend(vars);
    let cached = GH_LOGINS.lock().unwrap().clone();
    let logins = match cached {
        Some(l) => l,
        None => {
            // PORT: Bun.spawn throws when gh is not on PATH, which the TS caught here; util::sh reports it as a failed
            // run instead, so the same case is checked up front.
            if util::which("gh").is_none() {
                return Err(json!({ "kind": "gh access failed", "why": "gh is not installed or did not answer. Install it, run gh auth login, then retry." }));
            }
            let j = sh(&["gh", "auth", "status", "--json", "hosts"], timeout(15_000)).await;
            let t = if j.ok { None } else { Some(sh(&["gh", "auth", "status", "--hostname", "github.com"], timeout(15_000)).await) };
            let text = t.map(|t| format!("{}\n{}", t.out, t.err)).unwrap_or_default();
            let l = helpers::parse_gh_logins(j.ok.then_some(j.out.as_str()), &text);
            *GH_LOGINS.lock().unwrap() = Some(l.clone());
            l
        }
    };
    let known = GH_LOGIN_OF.lock().unwrap().get(&project_id).cloned();
    let order: Vec<String> = match &known {
        Some(k) => std::iter::once(k.clone()).chain(logins.iter().filter(|l| *l != k).cloned()).collect(),
        None => logins.clone(),
    };
    for login in order {
        let token = sh(&["gh", "auth", "token", "--user", &login], timeout(15_000)).await;
        if !token.ok {
            continue;
        }
        let env = HashMap::from([("GH_TOKEN".to_string(), token.out.clone())]);
        let r = util::sh_v(&args, ShOpts { env: Some(env), timeout: Some(Duration::from_millis(15_000)), ..Default::default() }).await;
        let o = helpers::gh_outcome(r.code.map(i64::from), &r.out, &r.err);
        match o.s("error") {
            Some("no access") => continue,
            Some("no network") => return Err(json!({ "kind": "no network", "why": "Can't reach github.com." })),
            Some(_) => {
                let detail = o.s("detail").unwrap_or("");
                return Err(json!({ "kind": "unclear", "why": format!("gh answered, but not in a form Switchboard can read ({detail}). Usually passes on the next try.") }));
            }
            None => {
                GH_LOGIN_OF.lock().unwrap().insert(project_id, login);
                return Ok(o["repo"].clone());
            }
        }
    }
    *GH_LOGINS.lock().unwrap() = None; // a `gh auth login` since then is seen on the next try
    GH_LOGIN_OF.lock().unwrap().remove(&project_id);
    let names = if logins.is_empty() { "none".to_string() } else { logins.join(", ") };
    Err(json!({ "kind": "gh access failed", "why": format!("gh could not open {owner}/{name} with any logged-in account ({names}). Run gh auth login, then retry.") }))
}

// ---------- worktree scan (Settings › Worktrees) ----------

// A row is a worktree (key: its path) or a local branch no worktree has checked out (key: "<repo folder>#<branch>"):
// the WorktreeFacts, the Verdict, then key, kind, path, kb, last_commit, project and repo.
/// the last scan, `{ at, rows, errors }`, kept until the next
static WORKTREE_SCAN: Mutex<Option<Value>> = Mutex::new(None);
static WORKTREE_SCANNING: Mutex<Option<Flight>> = Mutex::new(None);

/// The last scan (GET /api/worktrees answers `{ at: null, rows: [], errors: [] }` before the first).
pub fn worktree_scan() -> Option<Value> {
    WORKTREE_SCAN.lock().unwrap().clone()
}
/// `ghLogins = null` (POST /api/gh/retry): the next refresh reads the gh accounts again, to see a `gh auth login`.
pub fn forget_gh_logins() {
    *GH_LOGINS.lock().unwrap() = None;
}

/// Commits of `ref` on no remote branch, minus the PR's: a merged PR's branch is often deleted on GitHub, so its
/// commits are then on no remote, but GitHub keeps them in the PR. "<sha8> <subject>" each.
async fn local_only(dir: &str, git_ref: &str, pr: Option<&Value>) -> Vec<String> {
    let in_pr: HashSet<String> = pr
        .and_then(|p| p["commits"]["nodes"].as_array())
        .map(|a| a.iter().filter_map(|c| c["commit"]["oid"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    let log = sh(&["git", "-C", dir, "log", "--no-merges", "--format=%H %s", git_ref, "--not", "--remotes"], Default::default()).await;
    if !log.ok || log.out.is_empty() {
        return vec![];
    }
    log.out
        .split('\n')
        .filter(|l| !in_pr.contains(l.get(..40).unwrap_or(l)))
        .map(|l| format!("{}{}", l.get(..8).unwrap_or(l), l.get(40..).unwrap_or("")))
        .collect()
}
fn pr_of(n: Option<&Value>) -> Value {
    match n {
        Some(n) => json!({ "number": n["number"], "url": n["url"], "state": util::str_of(n.get("state")).to_lowercase() }),
        None => Value::Null,
    }
}
/// A branch named pr-<n> or pr-<n>-head (made by `git fetch origin pull/<n>/head:pr-<n>`) is PR <n>.
fn pr_no(b: &str) -> Option<i64> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^pr-(\d+)(?:-head)?$").unwrap());
    RE.captures(b).and_then(|c| c[1].parse::<i64>().ok()).filter(|n| *n != 0)
}

/// Every git worktree of every topic folder that is a git repository, except the main checkouts, and every local branch
/// that no worktree has checked out, except the default branch. Read-only: git and gh only read, and `du` measures.
/// One GraphQL call per repository finds each branch's newest PR with its commits; a branch named pr-<n> or pr-<n>-head
/// is looked up as PR <n>. A scan already running is shared; await the result, then read worktree_scan().
/// ponytail: `du -sk` takes ~1 s on a 550 MB worktree; runs on the button only, results kept in memory.
pub fn scan_worktrees() -> impl Future<Output = ()> + Send + 'static {
    let mut s = WORKTREE_SCANNING.lock().unwrap();
    if let Some(f) = &*s {
        return f.clone();
    }
    let f = start_flight("worktree scan".into(), scan_worktrees_now(), || {
        *WORKTREE_SCANNING.lock().unwrap() = None;
    });
    *s = Some(f.clone());
    f
}

async fn scan_worktrees_now() {
    let mut errors: Vec<String> = vec![];
    let mut rows: Vec<Value> = vec![];
    // git prints real paths (/private/tmp/…); a TODO's path is as the agent gave it (/tmp/…)
    let todos: Vec<(i64, String, bool)> = db::all("SELECT id, worktree_path, removed_at FROM todos WHERE worktree_path IS NOT NULL", params![])
        .iter()
        .map(|t| (t.i("id").unwrap_or(0), real(t.s("worktree_path").unwrap_or("")), t.truthy("removed_at")))
        .collect();
    let mut seen: HashSet<String> = HashSet::new(); // two topics on one folder list it once
    for p in db::all("SELECT id, name, local_path FROM projects ORDER BY name", params![]) {
        let (pid, pname, local) = (p.i("id").unwrap_or(0), p.s("name").unwrap_or("").to_string(), p.s("local_path").unwrap_or(""));
        let list = sh(&["git", "-C", local, "worktree", "list", "--porcelain"], Default::default()).await;
        if !list.ok {
            continue; // not a git repository
        }
        let all: Vec<Value> = helpers::parse_worktree_list(&list.out).as_array().cloned().unwrap_or_default();
        // PORT: the TS read all[0]!.path and failed the whole scan on an empty list; git always lists the main checkout.
        let Some(repo_dir) = all.first().and_then(|w| w.s("path")).map(String::from) else { continue };
        if !seen.insert(repo_dir.clone()) {
            continue;
        }
        let wts = &all[1..];
        let (heads, def) = tokio::join!(
            sh(&["git", "-C", &repo_dir, "for-each-ref", "--format=%(refname:short)", "refs/heads"], Default::default()),
            sh(&["git", "-C", &repo_dir, "symbolic-ref", "--short", "refs/remotes/origin/HEAD"], Default::default()),
        );
        let mut out: HashSet<String> = all.iter().filter_map(|w| w.s("branch").map(String::from)).collect();
        out.insert(if def.ok { def.out.strip_prefix("origin/").unwrap_or(&def.out).to_string() } else { "main".into() });
        out.extend(["main".to_string(), "master".to_string()]);
        let loose: Vec<String> =
            if heads.ok && !heads.out.is_empty() { heads.out.split('\n').filter(|b| !out.contains(*b)).map(String::from).collect() } else { vec![] };
        let remote = sh(&["git", "-C", &repo_dir, "remote", "get-url", "origin"], Default::default()).await;
        let repo = if remote.ok { helpers::parse_github_remote(&remote.out) } else { None };
        let branches = uniq(wts.iter().filter_map(|w| w.s("branch").map(String::from)).chain(loose.iter().cloned()).filter(|b| !b.is_empty() && pr_no(b).is_none()));
        let numbers = uniq(loose.iter().filter_map(|b| pr_no(b)));
        let mut prs: HashMap<String, Value> = HashMap::new();
        if let Some(repo) = repo.filter(|_| !branches.is_empty() || !numbers.is_empty()) {
            let query = helpers::pr_query(branches.len() as i64, numbers.len() as i64, helpers::SCAN_PR_FIELDS);
            match gh_repo_query(pid, &repo, &query, query_vars(&branches, &numbers)).await {
                Err(e) => errors.push(format!("{pname}: {}", e.s("why").unwrap_or(""))),
                Ok(o) => {
                    for (i, b) in branches.iter().enumerate() {
                        let n = &o[format!("b{i}").as_str()]["nodes"][0];
                        if util::truthy(n) {
                            prs.insert(b.clone(), n.clone());
                        }
                    }
                    for (i, n) in numbers.iter().enumerate() {
                        let node = &o[format!("n{i}").as_str()];
                        if util::truthy(node) {
                            prs.insert(format!("#{n}"), node.clone());
                        }
                    }
                }
            }
        }
        let pr_for = |b: Option<&str>| -> Option<Value> {
            let b = b?;
            prs.get(&match pr_no(b) {
                Some(n) => format!("#{n}"),
                None => b.to_string(),
            })
            .cloned()
        };
        let row = |f: Value, r: Value| -> Value {
            let v = helpers::worktree_verdict(&f);
            util::merge(&util::merge(&util::merge(&f, v), r), json!({ "project": pname, "repo": repo_dir }))
        };
        let wt_rows = join_all(wts.iter().map(|w| {
            let node = pr_for(w.s("branch"));
            let path = w.s("path").unwrap_or("").to_string();
            let prunable = w.truthy("prunable");
            let todos = &todos;
            async move {
                let (st, only, last, du): (Option<ShOut>, Vec<String>, Option<ShOut>, Option<ShOut>) = if prunable {
                    (None, vec![], None, None)
                } else {
                    let (st, only, last, du) = tokio::join!(
                        sh(&["git", "-C", &path, "--no-optional-locks", "status", "--porcelain"], Default::default()),
                        local_only(&path, "HEAD", node.as_ref()),
                        sh(&["git", "-C", &path, "log", "-1", "--format=%cI"], Default::default()),
                        sh(&["du", "-sk", &path], Default::default()),
                    );
                    (Some(st), only, Some(last), Some(du))
                };
                let changed: Vec<String> = match &st {
                    Some(st) if st.ok && !st.out.is_empty() => st.out.split('\n').map(String::from).collect(),
                    _ => vec![],
                };
                let wt_todos: Vec<Value> = todos.iter().filter(|t| t.1 == path).map(|t| json!({ "id": t.0, "removed": t.2 })).collect();
                let kb = du.filter(|d| d.ok).and_then(|d| d.out.split_whitespace().next().and_then(|k| k.parse::<i64>().ok()));
                row(
                    json!({ "prunable": prunable, "locked": w.truthy("locked"), "branch": w["branch"], "changed": changed, "localOnly": only,
                        "pr": pr_of(node.as_ref()), "todos": wt_todos }),
                    json!({ "key": path, "kind": "worktree", "path": path, "kb": kb, "last_commit": last.filter(|l| l.ok).map(|l| l.out) }),
                )
            }
        }))
        .await;
        rows.extend(wt_rows);
        let branch_rows = join_all(loose.iter().map(|b| {
            let node = pr_for(Some(b));
            let repo_dir = &repo_dir;
            let row = &row;
            async move {
                let head = format!("refs/heads/{b}");
                let (only, last) = tokio::join!(
                    local_only(repo_dir, &head, node.as_ref()),
                    sh(&["git", "-C", repo_dir, "log", "-1", "--format=%cI", &head], Default::default()),
                );
                row(
                    json!({ "prunable": false, "locked": false, "branch": b, "changed": [], "localOnly": only, "pr": pr_of(node.as_ref()), "todos": [], "branchOnly": true }),
                    json!({ "key": format!("{repo_dir}#{b}"), "kind": "branch", "path": null, "kb": null, "last_commit": if last.ok { Some(last.out) } else { None } }),
                )
            }
        }))
        .await;
        rows.extend(branch_rows);
    }
    *WORKTREE_SCAN.lock().unwrap() = Some(json!({ "at": now(), "rows": rows, "errors": errors }));
}

/// Deletes the rows the user ticked (`items`: `[{ key, force, seen }]`), after a fresh scan. A Can remove row goes; a
/// Check first row only with force, and only when its reasons still read as the user saw them (`seen`) when confirming
/// what is lost. A Keep row, a locked worktree or a key the scan no longer finds is refused.
/// A worktree goes with `git worktree remove` (--force when forced: it has changed files), then its branch with
/// `git branch -D`: -d refuses a squash-merged branch, and the verdict already checked that no commit exists only there.
/// Returns `{ done, failed, scan }`.
pub async fn delete_worktrees(items: &Value) -> Value {
    scan_worktrees().await;
    // PORT: a scan that panicked leaves no result; the TS threw then (worktreeScan!). Here every key is "not found".
    let rows: Vec<Value> = worktree_scan().and_then(|s| s["rows"].as_array().cloned()).unwrap_or_default();
    let mut done: Vec<String> = vec![];
    let mut failed: Vec<Value> = vec![];
    for it in items.as_array().map(Vec::as_slice).unwrap_or_default() {
        let key = util::str_of(it.get("key"));
        let force = it.truthy("force");
        let seen = util::str_of(it.get("seen"));
        let r = rows.iter().find(|x| x.s("key") == Some(key.as_str()));
        let why = match r {
            None => Some("not found in a fresh scan".to_string()),
            Some(r) => {
                let said = r["reasons"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" · ")).unwrap_or_default();
                let verdict = r.s("verdict").unwrap_or("");
                if verdict == "keep" {
                    Some(format!("kept: {said}"))
                } else if r.truthy("locked") {
                    Some("locked".into())
                } else if verdict == "check" && !force {
                    Some(format!("check first: {said}"))
                } else if verdict == "check" && said != seen {
                    Some(format!("changed since you confirmed: {said}"))
                } else {
                    None
                }
            }
        };
        let (Some(r), None) = (r, &why) else {
            failed.push(json!({ "key": key, "why": why }));
            continue;
        };
        let repo = r.s("repo").unwrap_or("");
        let git = |a: Vec<&str>| {
            let mut cmd = vec!["git", "-C", repo];
            cmd.extend(a);
            let cmd: Vec<String> = cmd.into_iter().map(String::from).collect();
            async move { util::sh_v(&cmd, timeout(120_000)).await }
        };
        let prunable = r.truthy("prunable");
        let (mut ok, mut err) = if prunable {
            let res = git(vec!["worktree", "prune"]).await;
            (res.ok, res.err)
        } else if r.s("kind") == Some("worktree") {
            let mut a = vec!["worktree", "remove"];
            if force {
                a.push("--force");
            }
            a.push(r.s("path").unwrap_or(""));
            let res = git(a).await;
            (res.ok, res.err)
        } else {
            (true, String::new())
        };
        if let Some(branch) = r.s("branch").filter(|b| ok && !b.is_empty() && !prunable) {
            let res = git(vec!["branch", "-D", branch]).await;
            (ok, err) = (res.ok, res.err);
        }
        if ok {
            done.push(key);
        } else {
            failed.push(json!({ "key": key, "why": err.split('\n').next().unwrap_or("") }));
        }
    }
    let mut scan = WORKTREE_SCAN.lock().unwrap();
    if let Some(Value::Array(rows)) = scan.as_mut().and_then(|s| s.get_mut("rows")) {
        rows.retain(|x| !x.s("key").is_some_and(|k| done.iter().any(|d| d == k)));
    }
    json!({ "done": done, "failed": failed, "scan": scan.clone() })
}

// ---------- ports (item 148): what listens on a TCP port, and which TODO or topic folder it runs in ----------

// A row: kind ("process" | "container"), key, ports, name, command, folder, since, age, todo_id, project_id, self, agent.
// `since`: a process's start; a container has Docker's own text ("17 hours ago") in `age` instead.
/// The last scan's rows: Stop only acts on a pid or container it listed, never on one the request names on its own.
static LAST_SCAN: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn parent_pid() -> i64 {
    #[cfg(unix)]
    return std::os::unix::process::parent_id() as i64;
    #[cfg(not(unix))]
    return -1;
}

/// GET /api/ports: `{ unsupported, rows, docker, at, home }`.
pub async fn scan_ports() -> Value {
    if cfg!(windows) {
        return json!({ "unsupported": true, "rows": [], "docker": "not checked", "at": now(), "home": home_dir() });
    }
    // lsof exits 1 when nothing listens; the output is still read.
    let listen = helpers::parse_listen(&sh(&["lsof", "-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"], timeout(15_000)).await.out);
    let listen: Vec<Value> = listen.as_array().cloned().unwrap_or_default();
    let pids: Vec<String> = listen.iter().filter_map(|l| l.i("pid")).map(|p| p.to_string()).collect();
    let my_pid = std::process::id() as i64;
    let has_docker = util::which("docker").is_some();
    let (cwd, ps, dk, tree) = tokio::join!(
        async {
            if pids.is_empty() {
                HashMap::new()
            } else {
                helpers::parse_cwd(&sh(&["lsof", "-a", "-p", &pids.join(","), "-d", "cwd", "-Fn"], timeout(15_000)).await.out)
            }
        },
        async { helpers::parse_ps(&sh(&["/bin/ps", "-A", "-o", "pid=,etime=,command="], Default::default()).await.out) },
        async {
            if has_docker {
                Some(sh(&["docker", "ps", "--format", helpers::DOCKER_FORMAT], timeout(10_000)).await)
            } else {
                None
            }
        },
        async { helpers::descendants(&sh(&["/bin/ps", "-A", "-o", "pid=,ppid="], Default::default()).await.out, my_pid).into_iter().collect::<HashSet<i64>>() },
    );
    // An agent's own process (Claude Code listens for its IDE and MCP links): a `claude` this server started. Stopping it
    // breaks the run, so the row offers no Stop; the user stops the run in the TODO.
    static CLAUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^claude\b").unwrap());
    let is_agent = |pid: i64, name: &str| tree.contains(&pid) && CLAUDE.is_match(name);
    // lsof reports real paths (macOS /var is /private/var), so the stored folders are resolved too.
    let todos: Vec<Value> = db::all("SELECT id, worktree_path FROM todos WHERE removed_at IS NULL AND worktree_path IS NOT NULL", params![])
        .into_iter()
        .map(|t| util::merge(&t, json!({ "worktree_path": real(t.s("worktree_path").unwrap_or("")) })))
        .collect();
    let projects: Vec<Value> = db::all("SELECT id, local_path FROM projects", params![])
        .into_iter()
        .map(|p| util::merge(&p, json!({ "local_path": real(p.s("local_path").unwrap_or("")) })))
        .collect();
    let (todos, projects) = (Value::Array(todos), Value::Array(projects));
    let owner = |folder: &str| helpers::owner_of(folder, &todos, &projects);
    let containers: Vec<Value> = match &dk {
        Some(d) if d.ok => helpers::parse_docker(&d.out).as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let ports_of = |v: &Value| -> Vec<i64> { v["ports"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
    // A published container port also listens in Docker's own process (OrbStack, Docker Desktop): the container's row shows it.
    let taken: HashSet<i64> = containers.iter().flat_map(ports_of).collect();
    let ppid = parent_pid();
    let mut rows: Vec<Value> = vec![];
    for l in &listen {
        let ports: Vec<i64> = ports_of(l).into_iter().filter(|p| !taken.contains(p)).collect();
        if ports.is_empty() {
            continue;
        }
        let pid = l.i("pid").unwrap_or(0);
        let name = l.s("name").unwrap_or("");
        let folder = cwd.get(&pid).cloned().unwrap_or_default();
        let p = ps.get(&pid);
        let since = p.map(|p| {
            let secs = p["secs"].as_f64().unwrap_or(0.0);
            util::iso(chrono::Utc::now() - chrono::Duration::milliseconds((secs * 1000.0) as i64))
        });
        let o = owner(&folder);
        rows.push(json!({ "kind": "process", "key": pid.to_string(), "ports": ports, "name": name,
            "command": p.and_then(|p| p.s("command")).unwrap_or(""), "folder": folder, "since": since, "age": "",
            "todo_id": o["todo_id"], "project_id": o["project_id"], "self": pid == my_pid || pid == ppid, "agent": is_agent(pid, name) }));
    }
    for c in containers.iter().filter(|c| !ports_of(c).is_empty()) {
        let folder = c.s("folder").unwrap_or("");
        let o = owner(folder);
        rows.push(json!({ "kind": "container", "key": c["name"], "ports": c["ports"], "name": c["name"], "command": c["image"],
            "folder": folder, "since": null, "age": c["age"], "todo_id": o["todo_id"], "project_id": o["project_id"], "self": false, "agent": false }));
    }
    rows.sort_by_key(|r| ports_of(r).first().copied().unwrap_or(0));
    *LAST_SCAN.lock().unwrap() = rows.clone();
    let docker = match &dk {
        None => "not installed",
        Some(d) if d.ok => "ok",
        Some(_) => "not running",
    };
    json!({ "unsupported": false, "rows": rows, "docker": docker, "at": now(), "home": home_dir() })
}

/// Stop: a process and the processes it started (SIGTERM, SIGKILL 5 s later), or `docker stop` for a container.
/// Answers `{ ok: true }`.
pub async fn stop_port(kind: &str, key: &str) -> anyhow::Result<Value> {
    let row = LAST_SCAN.lock().unwrap().iter().find(|r| r.s("kind") == Some(kind) && r.s("key") == Some(key)).cloned();
    let Some(row) = row else { crate::fail!(409, "not in the last scan: scan again") };
    if row.truthy("self") {
        crate::fail!(400, "that is Switchboard itself");
    }
    if row.truthy("agent") {
        crate::fail!(400, "that is an agent: stop its run in the TODO");
    }
    if kind == "container" {
        let r = sh(&["docker", "stop", key], timeout(30_000)).await;
        if !r.ok {
            crate::fail!(500, "docker stop failed: {}", r.err);
        }
    } else {
        // Number(key): the keys of process rows are pids, so this parses.
        acp::kill_tree(key.parse().unwrap_or(0), libc::SIGTERM);
    }
    // PORT: the TS dropped the row by identity; rows are values here, so the first equal row goes.
    let mut last = LAST_SCAN.lock().unwrap();
    if let Some(i) = last.iter().position(|r| *r == row) {
        last.remove(i);
    }
    Ok(json!({ "ok": true }))
}

/// A message as the transcript shows it: its file's name and its attachments with their sizes.
/// PORT: the TS threw on a message id that does not exist; this returns Null.
pub fn message_out(id: i64) -> Value {
    let Some(m) = db::get(
        "SELECT m.id, m.author, m.body, m.state, m.committed_at AS at, m.file_id, m.session_no, m.via, f.path AS file_path
    FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.id = ?",
        params![id],
    ) else {
        return Value::Null;
    };
    let Value::Object(mut rest) = m else { return Value::Null };
    let file_path = rest.shift_remove("file_path");
    let file_name = file_path.as_ref().and_then(Value::as_str).filter(|p| !p.is_empty()).map(basename);
    rest.insert("file_name".into(), json!(file_name));
    let attachments: Vec<Value> = server::attachments_of(id)
        .into_iter()
        .map(|a| {
            // Bun.file(path).size is 0 for a file that is gone.
            let size = std::fs::metadata(a.s("path").unwrap_or("")).map(|m| m.len()).unwrap_or(0);
            json!({ "id": a["id"], "name": a["name"], "size": size })
        })
        .collect();
    rest.insert("attachments".into(), json!(attachments));
    Value::Object(rest)
}

// The same constants as the HTTP part's fileFacts (server.ts IMAGE_EXT and MAX_TEXT).
static IMAGE_EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$").unwrap());
const MAX_TEXT: u64 = 2 * 1024 * 1024;

fn mtime_ms(m: &std::fs::Metadata) -> Value {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| json!(d.as_secs_f64() * 1000.0))
        .unwrap_or(Value::Null)
}

/// A TODO file (a files_of row) as the Files tab lists it.
pub fn file_out(f: &Value) -> Value {
    let path = f.s("path").unwrap_or("");
    let (id, name, session_no, by_user, work) = (&f["id"], basename(path), &f["session_no"], f.truthy("by_user"), &f["work"]);
    let facts = (|| -> std::io::Result<Value> {
        // Images, binaries and files over MAX_TEXT have no line count; the list shows their size instead (the same test as fileFacts).
        let st = std::fs::metadata(path)?;
        let lower = path.to_lowercase();
        let image = IMAGE_EXT.is_match(path) && !lower.ends_with(".svg");
        let buf = if image || st.len() > MAX_TEXT { None } else { Some(std::fs::read(path)?) };
        let lines = buf.filter(|b| !b[..b.len().min(8192)].contains(&0)).map(|b| String::from_utf8_lossy(&b).trim_end().split('\n').count());
        Ok(json!({ "id": id, "name": name, "path": path, "session_no": session_no, "by_user": by_user, "work": work, "lines": lines,
            "size": st.len(), "mtime": mtime_ms(&st) }))
    })();
    facts.unwrap_or_else(|_| {
        // deleted on disk
        json!({ "id": id, "name": name, "path": path, "session_no": session_no, "by_user": by_user, "work": work, "lines": null, "mtime": null })
    })
}

/// Item 85: every file in the artifact folder is a TODO file, also one written from a terminal. Files no session
/// registered get session_no NULL ("added outside a session"). uploads/ (attachments), terminal/ (context) and archive/
/// (files the user removed) are skipped. A file gone from disk leaves the list, with its comments.
/// Symlinks are skipped: they could point the editor at files outside the folder, or loop.
/// ponytail: stops after 500 files; an artifact folder holds plans and notes, not a checkout.
pub fn register_disk_files(t: &Value) {
    let id = t.i("id").unwrap_or(0);
    let dir = artifact_dir(id);
    if !util::exists(&dir) {
        return;
    }
    fn walk(d: &str, top: bool, found: &mut Vec<String>) {
        let Ok(names) = std::fs::read_dir(d) else { return };
        for n in names.flatten() {
            let n = n.file_name().to_string_lossy().into_owned();
            if found.len() >= 500 || n.starts_with('.') || (top && (n == "uploads" || n == "terminal" || n == "archive")) {
                continue;
            }
            let p = join(d, &n);
            if let Ok(st) = std::fs::symlink_metadata(&p) {
                if st.is_dir() {
                    walk(&p, false, found);
                } else if st.is_file() {
                    found.push(p);
                }
            }
        }
    }
    let mut found = vec![];
    walk(&real(&dir), true, &mut found);
    for p in &found {
        db::run("INSERT OR IGNORE INTO files (todo_id, path) VALUES (?, ?)", params![id, p]);
    }
    for f in files_of(id) {
        if !util::exists(f.s("path").unwrap_or("")) {
            drop_file(id, f.i("id").unwrap_or(0));
        }
    }
}

pub fn drop_file(todo_id: i64, file_id: i64) {
    db::run("DELETE FROM comments WHERE todo_id = ? AND root = 'todo' AND rel = ?", params![todo_id, file_id.to_string()]);
    db::run("DELETE FROM files WHERE id = ?", params![file_id]);
    changed(Some(todo_id));
}

/// A projects row as Settings shows it: the remote, whether the folder is a git repository, and whether it is locked.
pub async fn project_out(p: &Value) -> Value {
    let local = p.s("local_path").unwrap_or("");
    let (r, g) = tokio::join!(
        sh(&["git", "-C", local, "remote", "get-url", "origin"], Default::default()),
        sh(&["git", "-C", local, "rev-parse", "--git-dir"], Default::default()),
    );
    let id = p.i("id").unwrap_or(0);
    let c = db::get(
        "SELECT EXISTS (SELECT 1 FROM todos WHERE project_id = ? AND session_id IS NOT NULL) AS locked,
    (SELECT COUNT(*) FROM todos WHERE project_id = ? AND removed_at IS NULL) AS todo_count",
        params![id, id],
    )
    .unwrap_or(Value::Null);
    util::merge(
        p,
        json!({
            "default_agent": p.s("default_agent").unwrap_or(crate::agents::DEFAULT_AGENT), "defaults": helpers::defaults_of(p.s("defaults")),
            "repo": if r.ok { Some(r.out) } else { None }, "git": g.ok, "locked": c.truthy("locked"), "todo_count": c["todo_count"],
            "profile_edited": p.truthy("profile_edited"), "independent": p.truthy("independent"),
            "scanning": runner::is_scanning(id), "scan_error": runner::scan_error(id),
        }),
    )
}

// ---------- writes shared with runner.rs ----------

/// `nextRun(s, new Date())?.toISOString() ?? null`, or null for a paused schedule (and for none).
fn next_run_at(s: &Value) -> Option<String> {
    if s.is_null() || s.truthy("paused") {
        return None;
    }
    helpers::next_run(s, chrono::Local::now()).map(|d| util::iso(d.with_timezone(&chrono::Utc)))
}

/// One transaction, so a failed file write leaves no TODO behind. A new TODO has no topic: routing picks it at Start (801).
/// `agent` is the agent of the first session (P5); when not given, routing takes the topic's default.
/// Item 145: an agent's TODO (`actor` agent) records its creator in parent_id, and its first message says "From T-12"
/// (via); `project_id` given (null: no topic) decides the topic now, so Start skips routing. Item 144: `schedule`.
/// `title`, from an agent's create_todo, replaces the first instruction as the title; session 1 is told to keep it.
/// `o` holds the TS options object: parent_id, key, project_id, schedule, title. A `project_id` key that is present,
/// also as null, counts as given (TS `!== undefined`).
pub fn create_todo(instruction: &str, actor: &'static str, uploads: &[Upload], agent: Option<&str>, o: &Value) -> i64 {
    let routed = o.get("project_id").is_some();
    let p = project_row(o.i("project_id"));
    let parent_id = o.i("parent_id").filter(|n| *n != 0);
    let schedule = o.get("schedule").filter(|s| util::truthy(s)).cloned().unwrap_or(Value::Null);
    let title = o.s("title").filter(|t| !t.is_empty()).unwrap_or(instruction);
    let id = db::transaction(|| {
        let at = now();
        let agent = agent.map(String::from).or_else(|| if routed { p.as_ref().and_then(|p| p.s("default_agent").map(String::from)) } else { None });
        let id = db::run(
            "INSERT INTO todos (title, status, created_at, updated_at, agent, parent_id, key, project_id, routed_at, schedule, next_run_at)
       VALUES (?, 'not_started', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                title,
                at,
                at,
                agent,
                o.i("parent_id"),
                o.s("key"),
                p.as_ref().and_then(|p| p.i("id")),
                routed.then(|| at.clone()),
                if schedule.is_null() { None } else { Some(schedule.to_string()) },
                next_run_at(&schedule)
            ],
        )
        .last_insert_rowid;
        let via = parent_id.map(|n| format!("T-{n}"));
        let mid = add_message(Some(id), if actor == "user" { "user" } else { "user via operator" }, instruction, "pending", None, via.as_deref());
        server::save_attachments(id, mid, uploads);
        Ok(id)
    })
    .expect("the transaction body cannot fail");
    let mut detail: Vec<String> = vec![];
    if let Some(n) = parent_id {
        detail.push(format!("by T-{n}"));
    }
    if routed {
        detail.push(format!("topic {}", p.as_ref().and_then(|p| p.s("name")).unwrap_or("none")));
    }
    if !schedule.is_null() {
        detail.push(format!("scheduled · {}", helpers::schedule_text(&schedule)));
    }
    detail.push("not started yet".into());
    add_event(Some(id), "created", actor, Some(&detail.join(" · ")), None);
    emit(json!({ "type": "todos" }));
    id
}

/// Removes a live TODO, for the user ("user"), the operator's remove_todo ("operator") or a worker's remove_todo on a
/// TODO it created ("T-<id>"). Like Esc in Claude Code; queued messages stay queued.
/// The agent process ends with the run, or now when idle. Routing that asks ends here; routing that runs ends with its run.
/// Either way the TODO is not started again.
pub fn remove_todo(id: i64, by: &str) {
    server::set_todo(id, json!({ "removed_at": now(), "removed_by": by, "suggestions": null }), true);
    let agent = by.starts_with("T-");
    add_event(Some(id), "removed", if agent { "agent" } else { by }, agent.then(|| format!("by {by}")).as_deref(), None);
    if runner::is_running(Some(id)) {
        runner::interrupt(Some(id), "app");
    } else {
        runner::close_agent(Some(id));
        runner::cancel_routing(id);
    }
    terms::close_terms(Some(id)); // restoring does not bring them back (item 115)
    emit(json!({ "type": "todos" }));
}

/// Item 144: sets, changes, pauses or removes (Null) a TODO's schedule, for the user ("user") or a worker's
/// set_schedule ("T-<id>"). The next slot counts from now.
pub fn set_schedule(id: i64, s: &Value, by: &str) {
    let s = if util::truthy(s) { s } else { &Value::Null };
    server::set_todo(id, json!({ "schedule": if s.is_null() { None } else { Some(s.to_string()) }, "next_run_at": next_run_at(s) }), true);
    let mut detail = vec![if s.is_null() {
        "removed".to_string()
    } else if s.truthy("paused") {
        "paused".to_string()
    } else {
        helpers::schedule_text(s)
    }];
    if by != "user" {
        detail.push(format!("by {by}"));
    }
    add_event(Some(id), "schedule", if by == "user" { "user" } else { "agent" }, Some(&detail.join(" · ")), None);
    emit(json!({ "type": "todos" }));
}

// ---------- migration from auto-coder (only when this database is new) ----------

/// main calls this when the database file did not exist before this start.
pub fn migrate() {
    let old = join(&join(&home_dir(), ".auto-coder"), "db.sqlite");
    if !util::exists(&old) {
        return;
    }
    let r = (|| -> anyhow::Result<()> {
        use rusqlite::OptionalExtension;
        // never written: the old reviews and findings are just left behind
        let odb = rusqlite::Connection::open_with_flags(&old, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut taken: HashSet<String> = HashSet::new();
        let projects: Vec<(String, String)> =
            odb.prepare("SELECT name, path FROM projects ORDER BY id")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (name, path) in &projects {
            let slug = helpers::unique_slug(&helpers::slugify(name), &taken);
            taken.insert(slug.clone());
            db::run("INSERT INTO projects (name, slug, local_path, created_at) VALUES (?, ?, ?, ?)", params![name, slug, path, now()]);
        }
        let prompt: Option<String> =
            odb.query_row("SELECT value FROM settings WHERE key = 'prompt'", [], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().filter(|p| !p.is_empty());
        if let Some(p) = &prompt {
            std::fs::write(join(home(), "old-review-prompt.md"), p)?;
        }
        drop(odb);
        println!("imported {} projects{} from {old}", projects.len(), if prompt.is_some() { " and the old review prompt" } else { "" });
        Ok(())
    })();
    if let Err(e) = r {
        eprintln!("could not import from {old}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// A database in a temporary SWITCHBOARD_HOME with the columns this module touches. Shared by every test in the
    /// binary (the connection is global), so each test works on its own TODOs.
    /// db::transaction does not keep other threads off the connection between BEGIN and COMMIT, so tests that write
    /// take turns.
    fn test_db() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("switchboard-ops-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            if std::env::var("SWITCHBOARD_HOME").is_err() {
                std::env::set_var("SWITCHBOARD_HOME", &dir);
            }
            let _ = db::open(&join(home(), "db.sqlite")); // another module's tests may have opened it already
            db::exec(
                "
CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, local_path TEXT NOT NULL,
  agent_prompt TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, default_agent TEXT, defaults TEXT, profile_edited INTEGER, independent INTEGER);
CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY, project_id INTEGER, title TEXT NOT NULL, status TEXT NOT NULL, attention INTEGER NOT NULL DEFAULT 0,
  session_id TEXT, worktree_path TEXT, pr_number INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT,
  agent TEXT, parent_id INTEGER, key TEXT, routed_at TEXT, schedule TEXT, next_run_at TEXT, suggestions TEXT);
CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, todo_id INTEGER, author TEXT NOT NULL, body TEXT NOT NULL, state TEXT NOT NULL,
  file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT, session_no INTEGER, via TEXT);
CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE, session_no INTEGER, by_user INTEGER);
CREATE TABLE IF NOT EXISTS attachments (id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL, actor TEXT NOT NULL, detail TEXT, at TEXT NOT NULL,
  session_no INTEGER, meta TEXT);
CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, no INTEGER NOT NULL, session_id TEXT NOT NULL,
  started_at TEXT NOT NULL, ended_at TEXT, agent TEXT);
CREATE TABLE IF NOT EXISTS comments (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, root TEXT NOT NULL, rel TEXT NOT NULL, path TEXT NOT NULL,
  quote TEXT NOT NULL, line INTEGER, body TEXT NOT NULL, reply TEXT, state TEXT NOT NULL, created_at TEXT NOT NULL, replied_at TEXT);",
            );
        });
        turn
    }
    fn last_event(id: i64) -> Value {
        db::get("SELECT kind, actor, detail FROM events WHERE todo_id = ? ORDER BY id DESC LIMIT 1", params![id]).unwrap()
    }

    #[tokio::test]
    async fn create_todo_without_topic() {
        let _turn = test_db();
        let id = create_todo("Fix the build", "user", &[], None, &json!({}));
        let t = server::todo_row(id).unwrap();
        assert_eq!((t.s("title"), t.s("status")), (Some("Fix the build"), Some("not_started")));
        assert!(t["project_id"].is_null() && t["routed_at"].is_null() && t["agent"].is_null() && t["next_run_at"].is_null());
        let m = db::get("SELECT author, body, state, via FROM messages WHERE todo_id = ?", params![id]).unwrap();
        assert_eq!(m, json!({ "author": "user", "body": "Fix the build", "state": "pending", "via": null }));
        assert_eq!(last_event(id), json!({ "kind": "created", "actor": "user", "detail": "not started yet" }));
    }

    #[tokio::test]
    async fn create_todo_by_an_agent_with_topic() {
        let _turn = test_db();
        let pid = db::run(
            "INSERT INTO projects (name, slug, local_path, created_at, default_agent) VALUES ('Billing', ?, '/tmp/billing', ?, 'codex')",
            params![format!("billing-{}", util::now_ms()), now()],
        )
        .last_insert_rowid;
        let id = create_todo("long instruction", "agent", &[], None, &json!({ "parent_id": 12, "project_id": pid, "title": "Short title", "key": null }));
        let t = server::todo_row(id).unwrap();
        assert_eq!((t.s("title"), t.i("project_id"), t.s("agent"), t.i("parent_id")), (Some("Short title"), Some(pid), Some("codex"), Some(12)));
        assert!(t.s("routed_at").is_some());
        let m = db::get("SELECT author, via FROM messages WHERE todo_id = ?", params![id]).unwrap();
        assert_eq!(m, json!({ "author": "user via operator", "via": "T-12" }));
        assert_eq!(last_event(id)["detail"], "by T-12 · topic Billing · not started yet");

        // project_id given as null: routed, no topic
        let id = create_todo("x", "operator", &[], Some("claude"), &json!({ "project_id": null }));
        let t = server::todo_row(id).unwrap();
        assert!(t["project_id"].is_null() && t.s("routed_at").is_some());
        assert_eq!(t.s("agent"), Some("claude"));
        assert_eq!(last_event(id), json!({ "kind": "created", "actor": "operator", "detail": "topic none · not started yet" }));
    }

    #[tokio::test]
    async fn set_schedule_removed() {
        let _turn = test_db();
        let id = create_todo("x", "user", &[], None, &json!({}));
        set_schedule(id, &Value::Null, "T-4");
        assert_eq!(last_event(id), json!({ "kind": "schedule", "actor": "agent", "detail": "removed · by T-4" }));
        set_schedule(id, &Value::Null, "user");
        assert_eq!(last_event(id), json!({ "kind": "schedule", "actor": "user", "detail": "removed" }));
        assert!(server::todo_row(id).unwrap()["schedule"].is_null());
    }

    #[tokio::test]
    async fn disk_files_register_and_drop() {
        let _turn = test_db();
        let id = create_todo("files", "user", &[], None, &json!({}));
        let dir = artifact_dir(id);
        for sub in ["uploads", "terminal", "archive", "notes"] {
            std::fs::create_dir_all(join(&dir, sub)).unwrap();
        }
        for f in ["plan.md", ".hidden", "uploads/a.png", "terminal/t.txt", "archive/old.md", "notes/b.txt"] {
            std::fs::write(join(&dir, f), "one\ntwo\n").unwrap();
        }
        register_disk_files(&json!({ "id": id }));
        let names = |id| files_of(id).iter().map(|f| basename(f.s("path").unwrap())).collect::<Vec<_>>();
        let mut got = names(id);
        got.sort();
        assert_eq!(got, ["b.txt", "plan.md"]);

        let f = files_of(id).into_iter().find(|f| f.s("path").unwrap().ends_with("plan.md")).unwrap();
        let out = file_out(&f);
        assert_eq!((out["name"].as_str(), out["lines"].as_i64(), out["size"].as_i64(), out["by_user"].as_bool()), (Some("plan.md"), Some(2), Some(8), Some(false)));

        std::fs::remove_file(join(&dir, "notes/b.txt")).unwrap();
        let gone = files_of(id).into_iter().find(|f| f.s("path").unwrap().ends_with("b.txt")).unwrap();
        let out = file_out(&gone);
        assert!(out["lines"].is_null() && out["mtime"].is_null() && out.get("size").is_none());
        register_disk_files(&json!({ "id": id }));
        assert_eq!(names(id), ["plan.md"]);
    }

    #[tokio::test]
    async fn message_out_shape() {
        let _turn = test_db();
        let id = create_todo("hello", "user", &[], None, &json!({}));
        let mid = db::get("SELECT id FROM messages WHERE todo_id = ?", params![id]).unwrap().i("id").unwrap();
        let m = message_out(mid);
        let keys: Vec<&String> = m.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "author", "body", "state", "at", "file_id", "session_no", "via", "file_name", "attachments"]);
        assert_eq!((m["file_name"].clone(), m["attachments"].clone()), (Value::Null, json!([])));
        assert!(message_out(-1).is_null());
    }

    #[test]
    fn pr_for_prefers_the_set_pr_number() {
        let (a, b) = (json!({ "number": 7 }), json!({ "number": 9 }));
        PR_STORE.lock().unwrap().insert(
            -5,
            PrStore {
                at: Some("2026-01-01T00:00:00.000Z".into()),
                by_branch: HashMap::from([("fix".to_string(), a.clone())]),
                by_number: HashMap::from([(9, b.clone())]),
                error: Some(json!({ "kind": "no network", "why": "Can't reach github.com." })),
            },
        );
        assert_eq!(pr_for(Some(-5), Some(9), Some("fix")).0, Some(b));
        assert_eq!(pr_for(Some(-5), None, Some("fix")).0, Some(a));
        assert_eq!(pr_for(Some(-5), Some(8), Some("fix")).0, None);
        assert_eq!(pr_for(Some(-5), None, None).1.as_deref(), Some("2026-01-01T00:00:00.000Z"));
        assert_eq!(pr_for(Some(-5), None, None).2["kind"], "no network");
        assert_eq!(pr_for(Some(-6), Some(9), None), (None, None, Value::Null));
        assert_eq!(pr_for(None, Some(9), None), (None, None, Value::Null));
    }

    #[test]
    fn pr_branch_names() {
        assert_eq!(pr_no("pr-12"), Some(12));
        assert_eq!(pr_no("pr-12-head"), Some(12));
        assert_eq!(pr_no("pr-0"), None);
        assert_eq!(pr_no("pr-12-fix"), None);
        assert_eq!(query_vars(&["a".into()], &[5]), ["-f", "b0=a", "-F", "n0=5"]);
        assert_eq!(uniq([3, 1, 3, 2, 1]), [3, 1, 2]);
    }

    #[tokio::test]
    async fn flights_are_shared_and_cleared() {
        static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static SLOT: Mutex<Option<Flight>> = Mutex::new(None);
        let go = || {
            let mut s = SLOT.lock().unwrap();
            if let Some(f) = &*s {
                return f.clone();
            }
            let f = start_flight(
                "test".into(),
                async {
                    RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                },
                || *SLOT.lock().unwrap() = None,
            );
            *s = Some(f.clone());
            f
        };
        tokio::join!(go(), go());
        assert_eq!(RUNS.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(SLOT.lock().unwrap().is_none());
        go().await;
        assert_eq!(RUNS.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

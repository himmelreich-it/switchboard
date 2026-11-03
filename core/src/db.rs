//! SQLite, as server.ts used bun:sqlite: rows come back as JSON objects keyed by column name, parameters are JSON
//! values. One connection behind a mutex; every call is short and synchronous, never held across an await.

use rusqlite::types::{ToSqlOutput, Value as Sql, ValueRef};
use rusqlite::{Connection, ToSql};
use serde_json::{Map, Value};
use std::sync::{Mutex, MutexGuard, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn open(path: &str) -> anyhow::Result<()> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    DB.set(Mutex::new(c)).map_err(|_| anyhow::anyhow!("database opened twice"))?;
    Ok(())
}

pub fn conn() -> MutexGuard<'static, Connection> {
    DB.get().expect("database not open").lock().unwrap_or_else(|e| e.into_inner())
}

struct P<'a>(&'a Value);
impl ToSql for P<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(match self.0 {
            Value::Null => Sql::Null,
            Value::Bool(b) => Sql::Integer(*b as i64),
            Value::Number(n) => match n.as_i64() {
                Some(i) => Sql::Integer(i),
                None => Sql::Real(n.as_f64().unwrap_or(0.0)),
            },
            Value::String(s) => Sql::Text(s.clone()),
            other => Sql::Text(other.to_string()),
        }))
    }
}

fn row_json(stmt_cols: &[String], row: &rusqlite::Row) -> rusqlite::Result<Value> {
    let mut m = Map::new();
    for (i, name) in stmt_cols.iter().enumerate() {
        let v = match row.get_ref(i)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(n) => Value::from(n),
            ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
            ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        };
        m.insert(name.clone(), v);
    }
    Ok(Value::Object(m))
}

fn query(c: &Connection, sql: &str, params: &[Value]) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = c.prepare_cached(sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let ps: Vec<P> = params.iter().map(P).collect();
    let refs: Vec<&dyn ToSql> = ps.iter().map(|p| p as &dyn ToSql).collect();
    let rows = stmt.query_map(refs.as_slice(), |r| row_json(&cols, r))?;
    rows.collect()
}

/// Every row (bun:sqlite `.all()`). Panics on an SQL error, like a thrown error in the TS code: a bug, not a state.
pub fn all(sql: &str, params: &[Value]) -> Vec<Value> {
    query(&conn(), sql, params).unwrap_or_else(|e| panic!("SQL {sql}: {e}"))
}

/// The first row or None (`.get()`).
pub fn get(sql: &str, params: &[Value]) -> Option<Value> {
    all(sql, params).into_iter().next()
}

pub struct RunResult {
    pub changes: usize,
    pub last_insert_rowid: i64,
}

/// A write (`db.run`).
pub fn run(sql: &str, params: &[Value]) -> RunResult {
    let c = conn();
    let ps: Vec<P> = params.iter().map(P).collect();
    let refs: Vec<&dyn ToSql> = ps.iter().map(|p| p as &dyn ToSql).collect();
    let changes = c.prepare_cached(sql).and_then(|mut s| s.execute(refs.as_slice())).unwrap_or_else(|e| panic!("SQL {sql}: {e}"));
    RunResult { changes, last_insert_rowid: c.last_insert_rowid() }
}

/// Several statements, no parameters (`db.exec`).
pub fn exec(sql: &str) {
    conn().execute_batch(sql).unwrap_or_else(|e| panic!("SQL {sql}: {e}"));
}

/// `db.transaction(fn)()`: BEGIN, the closure, COMMIT (ROLLBACK when it returns Err or panics). The closure calls
/// all/get/run as usual; the connection is not held between them.
pub fn transaction<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    exec("BEGIN");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match r {
        Ok(Ok(v)) => {
            exec("COMMIT");
            Ok(v)
        }
        Ok(Err(e)) => {
            exec("ROLLBACK");
            Err(e)
        }
        Err(p) => {
            exec("ROLLBACK");
            std::panic::resume_unwind(p)
        }
    }
}

/// Parameters: `params![a, b]` turns anything serde_json can take into a `[Value]`.
#[macro_export]
macro_rules! params {
    () => { &[] as &[serde_json::Value] };
    ($($x:expr),+ $(,)?) => { &[$(serde_json::json!($x)),+] as &[serde_json::Value] };
}

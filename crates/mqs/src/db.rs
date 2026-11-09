//! Synchronous SQLite storage behind one mutex: single writer, WAL, short transactions.
//!
//! Every operation is short (a few statements at most) so holding the lock across
//! a call is fine; the async server never waits on I/O here.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, Transaction};
use serde_json::{Map, Value};

use crate::config::SCHEMA_VERSION;
use crate::error::MqsResult;

pub type Row = Map<String, Value>;

const SCHEMA: &str = include_str!("schema.sql");

/// A bound SQL value (JSON-friendly).
#[derive(Debug, Clone)]
pub enum V {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl From<i64> for V {
    fn from(v: i64) -> Self {
        V::Int(v)
    }
}
impl From<i32> for V {
    fn from(v: i32) -> Self {
        V::Int(v as i64)
    }
}
impl From<u32> for V {
    fn from(v: u32) -> Self {
        V::Int(v as i64)
    }
}
impl From<usize> for V {
    fn from(v: usize) -> Self {
        V::Int(v as i64)
    }
}
impl From<bool> for V {
    fn from(v: bool) -> Self {
        V::Int(if v { 1 } else { 0 })
    }
}
impl From<f64> for V {
    fn from(v: f64) -> Self {
        V::Real(v)
    }
}
impl From<&str> for V {
    fn from(v: &str) -> Self {
        V::Text(v.to_string())
    }
}
impl From<&String> for V {
    fn from(v: &String) -> Self {
        V::Text(v.clone())
    }
}
impl From<String> for V {
    fn from(v: String) -> Self {
        V::Text(v)
    }
}
impl From<Value> for V {
    fn from(v: Value) -> Self {
        V::from(&v)
    }
}
impl From<&Value> for V {
    fn from(v: &Value) -> Self {
        match v {
            Value::Null => V::Null,
            Value::Bool(b) => V::Int(if *b { 1 } else { 0 }),
            Value::Number(n) => n
                .as_i64()
                .map(V::Int)
                .or_else(|| n.as_f64().map(V::Real))
                .unwrap_or(V::Null),
            Value::String(s) => V::Text(s.clone()),
            other => V::Text(other.to_string()),
        }
    }
}
impl<T: Into<V>> From<Option<T>> for V {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(x) => x.into(),
            None => V::Null,
        }
    }
}

impl V {
    fn sql(&self) -> SqlValue {
        match self {
            V::Null => SqlValue::Null,
            V::Int(i) => SqlValue::Integer(*i),
            V::Real(f) => SqlValue::Real(*f),
            V::Text(t) => SqlValue::Text(t.clone()),
        }
    }
}

/// `p!["a", 1, true]` -> `Vec<V>`
#[macro_export]
macro_rules! p {
    () => { [] as [$crate::db::V; 0] };
    ($($v:expr),+ $(,)?) => { [$($crate::db::V::from($v)),+] };
}

pub fn default_settings() -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in [
        ("clock_mode", Value::String("real".into())),
        ("clock_now_ms", Value::from(0)),
        ("seed", Value::from(42)),
        ("rate_mode", Value::String("certified".into())),
        ("require_online", Value::Bool(true)),
        ("feature.whitelist", Value::Bool(true)),
        ("feature.expose", Value::String("all".into())),
        ("chat.auto_at_bot", Value::Bool(false)),
        ("active_quota_per_day", Value::from(1000)),
        ("passive_window_c2c_ms", Value::from(60 * 60 * 1000)),
        ("passive_reply_limit_c2c", Value::from(4)),
        ("passive_window_group_ms", Value::from(5 * 60 * 1000)),
        ("passive_reply_limit_group", Value::from(5)),
        ("active_c2c_require_friend", Value::Bool(false)),
        ("allow_remote_media", Value::Bool(false)),
        ("language", Value::String("zh-CN".into())),
        ("theme", Value::String("auto".into())),
        ("telemetry", Value::Bool(false)),
        ("autostart", Value::Bool(false)),
        ("group.ensure_operator_admin", Value::Bool(true)),
        ("timezone", Value::String("Asia/Shanghai".into())),
        ("heartbeat_interval_ms", Value::from(41250)),
        ("log_level", Value::String("info".into())),
    ] {
        map.insert(key.to_string(), value);
    }
    map
}

pub struct Database {
    pub path: PathBuf,
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> MqsResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            path: path.to_path_buf(),
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> MqsResult<()> {
        let conn = self.conn.lock().unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
        }
        for (key, value) in default_settings() {
            conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO NOTHING",
                rusqlite::params![key, value.to_string()],
            )?;
        }
        Ok(())
    }

    pub fn execute(&self, sql: &str, values: &[V]) -> MqsResult<u64> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        stmt.execute(params_from_iter(values.iter().map(|v| v.sql())))?;
        if stmt.column_count() > 0 {
            return Ok(0);
        }
        let id = conn.last_insert_rowid();
        Ok(if id > 0 { id as u64 } else { 0 })
    }

    pub fn all(&self, sql: &str, values: &[V]) -> MqsResult<Vec<Row>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
        let mut rows = stmt.query(params_from_iter(values.iter().map(|v| v.sql())))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_map(row, &names)?);
        }
        Ok(out)
    }

    pub fn one(&self, sql: &str, values: &[V]) -> MqsResult<Option<Row>> {
        Ok(self.all(sql, values)?.into_iter().next())
    }

    /// Write transaction. The closure must not call other `Database` methods.
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> MqsResult<T>) -> MqsResult<T> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn get_setting(&self, key: &str) -> Value {
        match self.one("SELECT value FROM settings WHERE key=?", &crate::p![key]) {
            Ok(Some(row)) => row
                .get("value")
                .and_then(|v| v.as_str())
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null),
            _ => default_settings().get(key).cloned().unwrap_or(Value::Null),
        }
    }

    pub fn set_setting(&self, key: &str, value: &Value) -> MqsResult<()> {
        self.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            &crate::p![key, value.to_string()],
        )?;
        Ok(())
    }

    pub fn all_settings(&self) -> MqsResult<Map<String, Value>> {
        let rows = self.all("SELECT key, value FROM settings", &[])?;
        let mut out = Map::new();
        for row in rows {
            let key = s(&row, "key");
            let raw = row.get("value").cloned().unwrap_or(Value::Null);
            let value = match raw.as_str() {
                Some(text) => {
                    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
                }
                None => raw,
            };
            out.insert(key, value);
        }
        Ok(out)
    }

    pub fn prune_frames(&self, keep: i64) -> MqsResult<()> {
        let max_id = self
            .one("SELECT COALESCE(MAX(id),0) AS m FROM frames", &[])?
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        if max_id > keep {
            self.execute(
                "DELETE FROM frames WHERE id <= ?",
                &crate::p![max_id - keep],
            )?;
        }
        Ok(())
    }

    pub fn size_bytes(&self) -> i64 {
        let mut total = std::fs::metadata(&self.path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        for suffix in ["-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{}", self.path.display(), suffix));
            total += std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
        }
        total
    }

    pub fn checkpoint_for_snapshot(&self, target: &Path) -> MqsResult<()> {
        let _ = std::fs::remove_file(target);
        let conn = self.conn.lock().unwrap();
        conn.execute("VACUUM INTO ?", rusqlite::params![target.to_string_lossy()])?;
        Ok(())
    }

    pub fn integrity_check(&self) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .map(|v| v == "ok")
            .unwrap_or(false)
    }
}

fn row_to_map(row: &rusqlite::Row<'_>, names: &[String]) -> MqsResult<Row> {
    let mut map = Map::new();
    for (index, name) in names.iter().enumerate() {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(v) => Value::from(v),
            ValueRef::Real(v) => Value::from(v),
            ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
            ValueRef::Blob(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        };
        map.insert(name.clone(), value);
    }
    Ok(map)
}

// ------------------------------------------------------------------ typed accessors

pub fn s(row: &Row, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

pub fn s_opt(row: &Row, key: &str) -> Option<String> {
    match row.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

pub fn i(row: &Row, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        Some(Value::Bool(v)) => i64::from(*v),
        _ => 0,
    }
}

pub fn f(row: &Row, key: &str) -> f64 {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn b(row: &Row, key: &str) -> bool {
    match row.get(key) {
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(Value::String(text)) => text == "1" || text == "true",
        _ => false,
    }
}

pub fn json(row: &Row, key: &str) -> Value {
    row.get(key).cloned().unwrap_or(Value::Null)
}

/// Parse a JSON column, falling back to the default on malformed data.
pub fn json_col(row: &Row, key: &str, default: Value) -> Value {
    match row.get(key) {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(default),
        Some(Value::Null) | None => default,
        Some(other) => other.clone(),
    }
}

// ------------------------------------------------------------------ transaction-scoped helpers

pub fn conn_all(conn: &Connection, sql: &str, values: &[V]) -> MqsResult<Vec<Row>> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
    let mut rows = stmt.query(params_from_iter(values.iter().map(|v| v.sql())))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_map(row, &names)?);
    }
    Ok(out)
}

pub fn conn_one(conn: &Connection, sql: &str, values: &[V]) -> MqsResult<Option<Row>> {
    Ok(conn_all(conn, sql, values)?.into_iter().next())
}

pub fn conn_exec(conn: &Connection, sql: &str, values: &[V]) -> MqsResult<usize> {
    let mut stmt = conn.prepare(sql)?;
    Ok(stmt.execute(params_from_iter(values.iter().map(|v| v.sql())))?)
}

/// Execute an INSERT and return its `last_insert_rowid`.
pub fn conn_insert(conn: &Connection, sql: &str, values: &[V]) -> MqsResult<i64> {
    conn_exec(conn, sql, values)?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent_and_seeds_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        for _ in 0..2 {
            let db = Database::open(&path).unwrap();
            let settings = db.all_settings().unwrap();
            assert_eq!(settings.get("seed"), Some(&Value::from(42)));
            assert_eq!(
                db.get_setting("rate_mode"),
                Value::String("certified".into())
            );
        }
        assert!(path.exists());
    }

    #[test]
    fn rows_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("db.sqlite")).unwrap();
        db.execute(
            "INSERT INTO users(id,nickname,avatar,gender,age,city,actor_type,seed,behavior,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
            &crate::p!["u_1", "小满", "", 0, 0, "上海", "human", 1, "normal", 1],
        )
        .unwrap();
        let row = db
            .one("SELECT * FROM users WHERE id=?", &crate::p!["u_1"])
            .unwrap()
            .unwrap();
        assert_eq!(s(&row, "nickname"), "小满");
        assert_eq!(i(&row, "seed"), 1);
    }
}

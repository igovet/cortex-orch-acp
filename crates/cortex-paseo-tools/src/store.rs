//! Read-only store access with Python `sqlite3.Row` value semantics.
//!
//! A store opens only through cortex-core's `OpenMode::LiveReadOnly`: never
//! initializing, migrating, installing hooks or writing. A format-11 store is
//! refused (`legacy_storage_migration_required`), exactly as a
//! non-initializing inspection open must be; a store opened by the Rust ACP
//! is migrated by its own initializing open first.
use crate::py::{self, Crash, Py};
use cortex_core::store::{OpenMode, ProjectKey, Store, StoreChannel};
use rusqlite::{types::ValueRef, Connection};
use std::rc::Rc;

/// Why a reader stopped where the Python reader would have raised.
#[derive(Debug)]
pub enum Fail {
    /// `sqlite3.Error` (or an unopenable store).
    Sql,
    /// Any other exception the Python reader does not catch.
    Crash,
}

impl From<Crash> for Fail {
    fn from(_: Crash) -> Self {
        Fail::Crash
    }
}

impl From<rusqlite::Error> for Fail {
    fn from(_: rusqlite::Error) -> Self {
        Fail::Sql
    }
}

/// One SQLite value as Python's `sqlite3` returns it.
#[derive(Debug, Clone)]
pub enum Sv {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Sv {
    pub fn text(value: impl Into<String>) -> Sv {
        Sv::Text(value.into())
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Sv::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Sv::Null)
    }
    /// Python truthiness of the returned value.
    pub fn truthy(&self) -> bool {
        match self {
            Sv::Null => false,
            Sv::Int(i) => *i != 0,
            Sv::Real(f) => *f != 0.0,
            Sv::Text(s) => !s.is_empty(),
            Sv::Blob(b) => !b.is_empty(),
        }
    }
    /// `value == "text"`.
    pub fn eq_str(&self, text: &str) -> bool {
        self.as_text() == Some(text)
    }
    /// `value == number` (`int`/`float` equality).
    pub fn eq_int(&self, number: i64) -> bool {
        match self {
            Sv::Int(i) => *i == number,
            Sv::Real(f) => *f == number as f64,
            _ => false,
        }
    }
    /// `value == other`, where `other` is optional text (`None` = Python `None`).
    pub fn eq_opt(&self, other: Option<&str>) -> bool {
        match other {
            None => self.is_null(),
            Some(text) => self.eq_str(text),
        }
    }
    /// The value as a JSON-serializable Python object; `bytes` are not.
    pub fn to_py(&self) -> Result<Py, Crash> {
        Ok(match self {
            Sv::Null => Py::Null,
            Sv::Int(i) => Py::int(*i),
            Sv::Real(f) => Py::Float(*f),
            Sv::Text(s) => Py::str(s.clone()),
            Sv::Blob(_) => return Err(Crash),
        })
    }
    /// Python `str(value)`.
    pub fn to_str(&self) -> String {
        match self {
            Sv::Null => "None".into(),
            Sv::Int(i) => i.to_string(),
            Sv::Real(f) => py::to_str(&Py::Float(*f)),
            Sv::Text(s) => s.clone(),
            Sv::Blob(b) => format!("{b:?}"),
        }
    }
    /// `json.loads(value)`: `Err` for `ValueError`/`TypeError`.
    pub fn loads(&self) -> Result<Py, ()> {
        match self {
            Sv::Text(s) => py::loads(s).ok_or(()),
            Sv::Blob(b) => std::str::from_utf8(b).ok().and_then(py::loads).ok_or(()),
            _ => Err(()),
        }
    }
    /// A Python object bound as a query parameter.
    pub fn from_py(value: &Py) -> Result<Sv, Fail> {
        Ok(match value {
            Py::Null => Sv::Null,
            Py::Bool(b) => Sv::Int(i64::from(*b)),
            Py::Int(text) => Sv::Int(text.parse().map_err(|_| Fail::Crash)?),
            Py::Float(f) => Sv::Real(*f),
            Py::Str(s) => Sv::Text(s.clone()),
            Py::List(_) | Py::Dict(_) => return Err(Fail::Sql),
        })
    }
}

impl rusqlite::ToSql for Sv {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(ToSqlOutput::Owned(match self {
            Sv::Null => Value::Null,
            Sv::Int(i) => Value::Integer(*i),
            Sv::Real(f) => Value::Real(*f),
            Sv::Text(s) => Value::Text(s.clone()),
            Sv::Blob(b) => Value::Blob(b.clone()),
        }))
    }
}

/// One result row, addressed by column name like `sqlite3.Row`.
#[derive(Debug, Clone)]
pub struct Row {
    names: Rc<Vec<String>>,
    values: Vec<Sv>,
}

impl Row {
    /// `row[name]`; a missing column is an `IndexError`.
    pub fn get(&self, name: &str) -> Result<&Sv, Crash> {
        self.names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|at| &self.values[at])
            .ok_or(Crash)
    }
    /// `name in row.keys()`.
    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
    /// `row[index]`.
    pub fn at(&self, index: usize) -> Result<&Sv, Crash> {
        self.values.get(index).ok_or(Crash)
    }
}

/// `db.execute(sql, params).fetchall()`.
pub fn query(db: &Connection, sql: &str, params: &[Sv]) -> Result<Vec<Row>, Fail> {
    let mut statement = db.prepare(sql)?;
    let names: Rc<Vec<String>> = Rc::new(
        statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    let count = names.len();
    let mut rows = statement.query(rusqlite::params_from_iter(params.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(match row.get_ref(index)? {
                ValueRef::Null => Sv::Null,
                ValueRef::Integer(i) => Sv::Int(i),
                ValueRef::Real(f) => Sv::Real(f),
                // Python's sqlite3 raises OperationalError on undecodable text.
                ValueRef::Text(bytes) => {
                    Sv::Text(String::from_utf8(bytes.to_vec()).map_err(|_| Fail::Sql)?)
                }
                ValueRef::Blob(bytes) => Sv::Blob(bytes.to_vec()),
            });
        }
        out.push(Row {
            names: Rc::clone(&names),
            values,
        });
    }
    Ok(out)
}

/// `.fetchone()`.
pub fn query_one(db: &Connection, sql: &str, params: &[Sv]) -> Result<Option<Row>, Fail> {
    Ok(query(db, sql, params)?.into_iter().next())
}

/// `{name for name in sqlite_master tables}`.
pub fn tables(db: &Connection) -> Result<Vec<String>, Fail> {
    query(db, "select name from sqlite_master where type='table'", &[])?
        .iter()
        .map(|row| Ok(row.at(0)?.to_str()))
        .collect()
}

/// `{row[1] for row in pragma table_info(table)}`.
pub fn columns(db: &Connection, table: &str) -> Result<Vec<String>, Fail> {
    query(db, &format!("pragma table_info({table})"), &[])?
        .iter()
        .map(|row| Ok(row.at(1)?.to_str()))
        .collect()
}

/// `','.join('?' for _ in values)`.
pub fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

/// Open `<root>/.cortex/cortex.sqlite3` read-only and run `body` in one
/// query-only snapshot. Any open refusal is reported as `Fail::Sql` (the
/// reader cannot read the store) with its code on stderr for diagnostics.
pub fn read<T>(root: &str, body: impl FnOnce(&Connection) -> Result<T, Fail>) -> Result<T, Fail> {
    let key = ProjectKey::new(root, StoreChannel::Release).map_err(refused)?;
    if key.root().to_str() != Some(root) {
        // A linked worktree's own directory names another store.
        eprintln!("cortex-paseo-tools: project_root_not_logical");
        return Err(Fail::Sql);
    }
    let store = Store::open(key, OpenMode::LiveReadOnly).map_err(refused)?;
    let mut outcome = None;
    store
        .read(|db| {
            outcome = Some(body(db));
            Ok(())
        })
        .map_err(refused)?;
    outcome.unwrap_or(Err(Fail::Sql))
}

fn refused(error: cortex_core::CoreError) -> Fail {
    eprintln!("cortex-paseo-tools: store_unavailable ({})", error.code());
    Fail::Sql
}

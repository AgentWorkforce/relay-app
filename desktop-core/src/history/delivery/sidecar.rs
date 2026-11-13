//! The upload journal lives beside evidence, not in the evidence database.
//!
//! Migration first fences every legacy table while holding SQLite's writer
//! lock. Cold helpers from an old build then fail before claiming work. A
//! copied destination is committed and checked before source rows are removed;
//! a restart never recopies over a destination that may already hold receipts.
use super::*;
use rusqlite::types::ValueRef;
use std::path::{Path, PathBuf};

mod migration;
#[cfg(test)]
mod tests;

const STATE: &str = "_relay_delivery_sidecar";
const EVIDENCE: &str = "delivery_evidence";
const MOVED: &str = "upload state moved to delivery-queue.db; update the helper";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Table {
    name: String,
    sql: String,
    columns: Vec<String>,
    order: Vec<String>,
    rowid: bool,
    rows: u64,
    digest: String,
    sequence: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    version: u32,
    identity: String,
    tables: Vec<Table>,
    objects: Vec<String>,
    #[serde(default)]
    capture_triggers: Vec<(String, String)>,
}

pub(super) fn path(history: &Path) -> PathBuf {
    if history.file_name().is_some_and(|name| name == "history.db") {
        history.with_file_name("delivery-queue.db")
    } else {
        let mut name = history.file_name().unwrap_or_default().to_os_string();
        name.push(".delivery-queue.db");
        history.with_file_name(name)
    }
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn exists(conn: &Connection, schema: &str, name: &str) -> Result<bool> {
    Ok(conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM {}.sqlite_master WHERE name=?)",
            ident(schema)
        ),
        [name],
        |r| r.get(0),
    )?)
}

fn state(conn: &Connection) -> Result<Option<(String, Manifest)>> {
    if !exists(conn, "main", STATE)? {
        return Ok(None);
    }
    let (phase, json): (String, String) = conn.query_row(
        &format!("SELECT phase,manifest FROM {STATE} WHERE singleton=1"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let manifest: Manifest = serde_json::from_str(&json)?;
    ensure!(
        manifest.version == crate::storage_format::UPLOAD_QUEUE_FORMAT,
        "unsupported upload queue migration version"
    );
    Ok(Some((phase, manifest)))
}

#[cfg(test)]
pub(super) fn started(conn: &Connection) -> Result<bool> {
    exists(conn, "main", STATE)
}

fn save(conn: &Connection, phase: &str, manifest: &Manifest) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {STATE} (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), phase TEXT NOT NULL,
        manifest TEXT NOT NULL)"
    ))?;
    conn.execute(
        &format!(
            "INSERT INTO {STATE} VALUES(1,?,?)
        ON CONFLICT(singleton) DO UPDATE SET phase=excluded.phase,manifest=excluded.manifest"
        ),
        params![phase, serde_json::to_string(manifest)?],
    )?;
    Ok(())
}

fn fingerprint(conn: &Connection, schema: &str, table: &Table) -> Result<(u64, String)> {
    let columns = table
        .columns
        .iter()
        .map(|c| ident(c))
        .collect::<Vec<_>>()
        .join(",");
    let order = table
        .order
        .iter()
        .map(|c| ident(c))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {}{columns} FROM {}.{} ORDER BY {order}",
        if table.rowid { "rowid," } else { "" },
        ident(schema),
        ident(&table.name)
    );
    let mut statement = conn.prepare(&sql)?;
    let width = statement.column_count();
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    let mut count = 0u64;
    while let Some(row) = rows.next()? {
        digest.update([0xff]);
        for column in 0..width {
            match row.get_ref(column)? {
                ValueRef::Null => digest.update([0]),
                ValueRef::Integer(value) => {
                    digest.update([1]);
                    digest.update(value.to_le_bytes());
                }
                ValueRef::Real(value) => {
                    digest.update([2]);
                    digest.update(value.to_bits().to_le_bytes());
                }
                ValueRef::Text(value) | ValueRef::Blob(value) => {
                    digest.update([if matches!(row.get_ref(column)?, ValueRef::Text(_)) {
                        3
                    } else {
                        4
                    }]);
                    digest.update((value.len() as u64).to_le_bytes());
                    digest.update(value);
                }
            }
        }
        count += 1;
    }
    Ok((count, format!("{:x}", digest.finalize())))
}

fn attach(conn: &Connection, history: &Path) -> Result<()> {
    let absolute = std::fs::canonicalize(history)?;
    let url = url::Url::from_file_path(&absolute)
        .map_err(|_| anyhow::anyhow!("history path cannot be opened read-only"))?;
    conn.execute(
        &format!("ATTACH DATABASE ? AS {EVIDENCE}"),
        [readonly_uri(&url)],
    )?;
    Ok(())
}

// SQLite rejects a non-local authority unless built with a special option.
// Express a Windows UNC name in the path instead, keeping mode=ro and the
// URL library's escaping of spaces, percent signs, '?' and '#'.
fn readonly_uri(url: &url::Url) -> String {
    match url.host_str() {
        Some(host) if !host.is_empty() && host != "localhost" => {
            format!("file:////{host}{}?mode=ro", url.path())
        }
        _ => format!("file:{}?mode=ro", url.path()),
    }
}

/// Only the collector owner calls this, before it starts delivery threads.
#[cfg(test)]
pub(super) fn migrate(history: &Path) -> Result<Connection> {
    migration::open(history)
}

pub(super) fn migrate_initialized(
    history: &Path,
    initialize: impl FnOnce(&Connection) -> Result<()>,
) -> Result<Connection> {
    migration::open_initialized(history, |_| Ok(()), initialize)
}

/// Cold control/read helpers never initiate a cutover or fall back to legacy.
pub(super) fn open(history: &Path, readonly: bool) -> Result<Connection> {
    let queue = path(history);
    ensure!(
        !queue.is_symlink(),
        "upload queue cannot be a symbolic link"
    );
    ensure!(queue.is_file(), UpgradePending);
    let queue_identity = DatabaseIdentity::read(&queue)?;
    let source_identity = DatabaseIdentity::read(history)?;
    let flags = if readonly {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(&queue, flags | rusqlite::OpenFlags::SQLITE_OPEN_URI)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let (phase, manifest) = state(&conn)?.context(UpgradePending)?;
    ensure!(phase == "ready", UpgradePending);
    let source = Connection::open_with_flags(history, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (source_phase, source_manifest) = state(&source)?.context(UpgradePending)?;
    ensure!(
        source_phase == "done" && source_manifest == manifest,
        "upload queue does not belong to this history database"
    );
    attach(&conn, history)?;
    ensure!(
        queue_identity == DatabaseIdentity::read(&queue)?
            && source_identity == DatabaseIdentity::read(history)?,
        "history or upload queue changed while opening delivery"
    );
    conn.pragma_update(None, "recursive_triggers", true)?;
    Ok(conn)
}

pub(super) fn history_path(conn: &Connection) -> Result<PathBuf> {
    let mut statement = conn.prepare("PRAGMA database_list")?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (name, path) = row?;
        if name == EVIDENCE {
            return Ok(PathBuf::from(path));
        }
    }
    conn.path()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .context("delivery needs a history database file")
}

pub(super) fn history_schema_version(conn: &Connection) -> Result<i64> {
    let history = history_path(conn)?;
    let attached = conn.path().is_some_and(|path| Path::new(path) != history);
    let sql = if attached {
        "PRAGMA delivery_evidence.schema_version"
    } else {
        "PRAGMA main.schema_version"
    };
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

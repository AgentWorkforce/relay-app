use super::*;

fn inventory(source: &Connection) -> Result<Manifest> {
    let identity = source.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
    let mut statement = source.prepare(&format!(
        "SELECT name,sql FROM sqlite_master WHERE type='table' AND ({}) ORDER BY name",
        table_filter("name"),
    ))?;
    let definitions = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut tables = Vec::new();
    for (name, sql) in definitions {
        tables.push(snapshot_table(source, name, sql)?);
    }
    let mut objects = source.prepare(&format!(
        "SELECT sql FROM sqlite_master WHERE type IN ('index','trigger')
        AND ({}) AND sql IS NOT NULL ORDER BY type,name",
        table_filter("tbl_name"),
    ))?;
    let objects = objects
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Manifest {
        version: crate::storage_format::UPLOAD_QUEUE_FORMAT,
        identity,
        tables,
        objects,
        capture_triggers: schema::capture_triggers(source)?,
    })
}

fn snapshot_table(source: &Connection, name: String, sql: String) -> Result<Table> {
    let mut columns = source.prepare(&format!("PRAGMA table_xinfo({})", ident(&name)))?;
    let columns = columns
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        columns.iter().all(|(name, _, hidden)| *hidden == 0
            && !["rowid", "_rowid_", "oid"].contains(&name.to_ascii_lowercase().as_str())),
        "unsupported upload table column shape"
    );
    let rowid = !sql.to_uppercase().contains("WITHOUT ROWID");
    let mut primary: Vec<_> = columns
        .iter()
        .filter(|(_, position, _)| *position > 0)
        .collect();
    primary.sort_by_key(|(_, position, _)| *position);
    let order = if rowid {
        vec!["rowid".into()]
    } else {
        primary
            .into_iter()
            .map(|(name, _, _)| name.clone())
            .collect()
    };
    ensure!(
        !order.is_empty(),
        "upload table has no stable migration order"
    );
    let sequence = if exists(source, "main", "sqlite_sequence")? {
        source
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name=?",
                [&name],
                |r| r.get(0),
            )
            .optional()?
    } else {
        None
    };
    let mut table = Table {
        name,
        sql,
        columns: columns.into_iter().map(|(name, _, _)| name).collect(),
        order,
        rowid,
        rows: 0,
        digest: String::new(),
        sequence,
    };
    (table.rows, table.digest) = fingerprint(source, "main", &table)?;
    Ok(table)
}

// Include the retired journal's auxiliary tables in the exact-state copy.
fn table_filter(column: &str) -> String {
    let retired = schema::JOURNAL_TABLES
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{column} GLOB 'delivery_*' OR {column} IN ({retired})")
}

pub(super) fn refuse_live_worker(source: &Connection, now: i64) -> Result<()> {
    if !schema::table_exists(source, "delivery_batches")? {
        return Ok(());
    }
    let leased: bool = source.query_row(
        "SELECT EXISTS(SELECT 1 FROM delivery_batches b JOIN delivery_jobs j ON j.id=b.job_id
         WHERE b.state='leased' AND j.worker_id IS NOT NULL AND j.lease_until_ms>?)",
        [now],
        |r| r.get(0),
    )?;
    ensure!(
        !leased,
        "an old upload worker still owns a batch; stop it before migration"
    );
    Ok(())
}

pub(super) fn freeze(source: &Connection, queue_path: &Path) -> Result<(String, Manifest)> {
    let tx = write_transaction(source)?;
    // The observation outside this transaction may be stale: another opener
    // may have frozen this source while we waited for SQLite's writer lock.
    if let Some(record) = state(&tx)? {
        return Ok(record);
    }
    ensure!(
        !queue_path.exists(),
        "upload queue has no matching source migration record"
    );
    refuse_live_worker(&tx, worker::system_clock())?;
    let manifest = inventory(&tx)?;
    for (index, table) in manifest.tables.iter().enumerate() {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            tx.execute_batch(&format!(
                "CREATE TRIGGER {} BEFORE {operation} ON {} BEGIN
                SELECT RAISE(ABORT, '{MOVED}'); END;",
                ident(&format!("_relay_sidecar_fence_{index}_{operation}")),
                ident(&table.name)
            ))?;
        }
    }
    save(&tx, "frozen", &manifest)?;
    tx.commit()?;
    Ok(("frozen".into(), manifest))
}

fn verify(conn: &Connection, schema: &str, manifest: &Manifest) -> Result<()> {
    let mut tables = conn.prepare(&format!(
        "SELECT name FROM {}.sqlite_master WHERE type='table' AND ({}) ORDER BY name",
        ident(schema),
        table_filter("name")
    ))?;
    let names = tables
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        names
            == manifest
                .tables
                .iter()
                .map(|table| table.name.clone())
                .collect::<Vec<_>>(),
        "upload table inventory changed during migration"
    );
    for table in &manifest.tables {
        let sql: String = conn.query_row(
            &format!(
                "SELECT sql FROM {}.sqlite_master WHERE type='table' AND name=?",
                ident(schema)
            ),
            [&table.name],
            |r| r.get(0),
        )?;
        ensure!(
            sql == table.sql,
            "upload table definition changed during migration"
        );
        if let Some(expected) = table.sequence {
            let actual: i64 = conn.query_row(
                &format!(
                    "SELECT seq FROM {}.sqlite_sequence WHERE name=?",
                    ident(schema)
                ),
                [&table.name],
                |r| r.get(0),
            )?;
            ensure!(
                actual == expected,
                "upload sequence changed during migration"
            );
        }
        let mut columns = conn.prepare(&format!(
            "PRAGMA {}.table_info({})",
            ident(schema),
            ident(&table.name)
        ))?;
        let columns = columns
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            columns == table.columns,
            "upload table schema changed during migration"
        );
        ensure!(
            fingerprint(conn, schema, table)? == (table.rows, table.digest.clone()),
            "upload queue copy differs; source was not removed"
        );
    }
    let mut objects = conn.prepare(&format!(
        "SELECT sql FROM {}.sqlite_master WHERE type IN ('index','trigger')
        AND ({}) AND sql IS NOT NULL
        AND name NOT GLOB '_relay_sidecar_fence_*' ORDER BY type,name",
        ident(schema),
        table_filter("tbl_name")
    ))?;
    let objects = objects
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        objects == manifest.objects,
        "upload indexes or triggers changed during migration"
    );
    Ok(())
}

fn copy(queue: &Connection, manifest: &Manifest) -> Result<()> {
    let tx = write_transaction(queue)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        count == 0,
        "unrecognized upload queue database; refusing to overwrite it"
    );
    for table in &manifest.tables {
        tx.execute_batch(&table.sql)?;
        let columns = table
            .columns
            .iter()
            .map(|c| ident(c))
            .collect::<Vec<_>>()
            .join(",");
        let columns = format!("{}{columns}", if table.rowid { "rowid," } else { "" });
        tx.execute_batch(&format!(
            "INSERT INTO main.{}({columns}) SELECT {columns} FROM {EVIDENCE}.{};",
            ident(&table.name),
            ident(&table.name)
        ))?;
        if let Some(sequence) = table.sequence {
            tx.execute("DELETE FROM sqlite_sequence WHERE name=?", [&table.name])?;
            tx.execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES(?,?)",
                params![table.name, sequence],
            )?;
        }
    }
    // Install derived-counter triggers only AFTER copying their source and
    // derived rows, so the move never increments a count or fence twice.
    for sql in &manifest.objects {
        tx.execute_batch(sql)?;
    }
    verify(&tx, "main", manifest)?;
    save(&tx, "copied", manifest)?;
    tx.commit()?;
    Ok(())
}

fn remove_source(source: &Connection, queue: &Connection, manifest: &Manifest) -> Result<()> {
    let tx = write_transaction(source)?;
    verify(&tx, "main", manifest)?;
    verify(queue, "main", manifest)?;
    ensure!(
        schema::capture_triggers(&tx)? == manifest.capture_triggers,
        "journal capture triggers changed during migration"
    );
    for (name, _) in &manifest.capture_triggers {
        tx.execute_batch(&format!("DROP TRIGGER {}", ident(name)))?;
    }
    schema::retire_capture_indexes(&tx)?;
    schema::retire_migration_keys(&tx)?;
    for table in &manifest.tables {
        tx.execute_batch(&format!(
            "DROP TABLE {}; CREATE VIEW {} AS
            SELECT RAISE(ABORT, '{MOVED}') AS queue_moved;",
            ident(&table.name),
            ident(&table.name)
        ))?;
    }
    save(&tx, "done", manifest)?;
    tx.commit()?;
    Ok(())
}

fn source_state(source: &Connection, queue_path: &Path) -> Result<(String, Manifest)> {
    match state(source)? {
        Some(record) => Ok(record),
        None => freeze(source, queue_path),
    }
}

// Separate from either SQLite inode: closing an unrelated fd for a SQLite
// file can release its POSIX locks. Cold/new-store callers contend here and
// retry rather than running a second cross-file cutover on stale phases.
fn migration_lock(queue: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(queue.with_extension("migration-lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| anyhow::Error::from(UpgradePending))?;
    Ok(lock)
}

#[cfg(test)]
pub(super) fn open(history: &Path) -> Result<Connection> {
    open_with(history, |_| Ok(()))
}

#[cfg(test)]
pub(super) fn open_with(
    history: &Path,
    checkpoint: impl FnMut(&str) -> Result<()>,
) -> Result<Connection> {
    open_initialized(history, checkpoint, |_| Ok(()))
}

pub(super) fn open_initialized(
    history: &Path,
    mut checkpoint: impl FnMut(&str) -> Result<()>,
    initialize: impl FnOnce(&Connection) -> Result<()>,
) -> Result<Connection> {
    let queue_path = path(history);
    ensure!(
        !queue_path.is_symlink(),
        "upload queue cannot be a symbolic link"
    );
    let _migration_lock = migration_lock(&queue_path)?;
    let source = super::super::open_connection(history)?;
    let (source_phase, manifest) = source_state(&source, &queue_path)?;
    ensure!(
        source_phase == "frozen" || source_phase == "done",
        "invalid upload migration phase"
    );
    checkpoint("frozen")?;
    super::super::DatabaseIdentity::open_or_create(&queue_path)?;
    let (queue, queue_phase) = open_queue(history, &queue_path, &source_phase, &manifest)?;
    checkpoint("copied")?;
    if source_phase == "frozen" {
        ensure!(queue_phase == "copied", "upload cutover phases disagree");
        remove_source(&source, &queue, &manifest)?;
    }
    checkpoint("source_removed")?;
    finish(&queue, &queue_phase, &manifest, initialize, &mut checkpoint)?;
    checkpoint("ready")?;
    Ok(queue)
}

fn open_queue(
    history: &Path,
    queue_path: &Path,
    source_phase: &str,
    manifest: &Manifest,
) -> Result<(Connection, String)> {
    let queue = super::super::open_connection(queue_path)?;
    queue.pragma_update(None, "journal_mode", "WAL")?;
    queue.pragma_update(None, "synchronous", "FULL")?;
    attach(&queue, history)?;
    let queue_phase = match state(&queue)? {
        None => {
            ensure!(
                source_phase == "frozen",
                "upload source was removed but its queue is missing"
            );
            copy(&queue, manifest)?;
            "copied".to_owned()
        }
        Some((phase, previous)) => {
            ensure!(
                &previous == manifest,
                "upload queue belongs to another source"
            );
            ensure!(
                phase == "copied" || phase == "upgrading" || phase == "ready",
                "invalid upload queue phase"
            );
            phase
        }
    };
    Ok((queue, queue_phase))
}

fn finish(
    queue: &Connection,
    queue_phase: &str,
    manifest: &Manifest,
    initialize: impl FnOnce(&Connection) -> Result<()>,
    checkpoint: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if queue_phase == "copied" {
        // On a crash after source cleanup, its immutable manifest is still
        // the witness. Never copy from the source again.
        verify(queue, "main", manifest)?;
        let tx = write_transaction(queue)?;
        save(&tx, "upgrading", manifest)?;
        tx.commit()?;
    }
    checkpoint("upgrading")?;
    // Only an owner may initialize/upgrade the copied schema. Cold helpers
    // refuse `upgrading`, including a crash partway through a legacy rebuild.
    // Each initializer transaction is idempotent and resumes on the next open.
    initialize(queue)?;
    if queue_phase != "ready" {
        let tx = write_transaction(queue)?;
        save(&tx, "ready", manifest)?;
        tx.commit()?;
    }
    Ok(())
}

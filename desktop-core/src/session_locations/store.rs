use super::*;
pub(super) fn database(root: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let db = Connection::open(root.join("session-locations.sqlite3")).map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version < 1 {
        db.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS locations(source TEXT NOT NULL, session_id TEXT NOT NULL, pid INTEGER NOT NULL, start TEXT NOT NULL, location TEXT NOT NULL, PRIMARY KEY(source,session_id,pid,start));
            CREATE TABLE IF NOT EXISTS origins(source TEXT NOT NULL, session_id TEXT NOT NULL, original TEXT NOT NULL, latest TEXT NOT NULL, PRIMARY KEY(source,session_id));
            INSERT OR IGNORE INTO origins SELECT source,session_id,(SELECT a.location FROM locations a WHERE a.source=l.source AND a.session_id=l.session_id ORDER BY json_extract(a.location,'$.observed_at_ms') ASC LIMIT 1),(SELECT b.location FROM locations b WHERE b.source=l.source AND b.session_id=l.session_id ORDER BY json_extract(b.location,'$.observed_at_ms') DESC LIMIT 1) FROM locations l GROUP BY source,session_id;
            PRAGMA user_version=1;
            COMMIT;").map_err(|e|e.to_string())?;
    }
    Ok(db)
}

pub(super) fn save(
    db: &Connection,
    source: &str,
    id: &str,
    locations: &[SessionLocation],
) -> Result<(), String> {
    // A savepoint is atomic both for report() and inside observe()'s batch.
    db.execute_batch("SAVEPOINT session_location_observation")
        .map_err(|e| e.to_string())?;
    let result = save_rows(db, source, id, locations);
    match result {
        Ok(()) => db
            .execute_batch("RELEASE session_location_observation")
            .map_err(|e| e.to_string()),
        Err(error) => {
            let _ = db.execute_batch(
                "ROLLBACK TO session_location_observation; RELEASE session_location_observation",
            );
            Err(error)
        }
    }
}
fn save_rows(
    db: &Connection,
    source: &str,
    id: &str,
    locations: &[SessionLocation],
) -> Result<(), String> {
    for location in locations {
        let json = serde_json::to_string(location).map_err(|e| e.to_string())?;
        // Independent observers can commit out of capture order. Equal-time
        // observations keep the committed value rather than guessing an order.
        db.execute("INSERT INTO origins VALUES (?1,?2,?3,?3) ON CONFLICT(source,session_id) DO UPDATE SET latest=excluded.latest WHERE json_extract(excluded.latest,'$.observed_at_ms') > json_extract(origins.latest,'$.observed_at_ms')",params![source,id,json]).map_err(|e| e.to_string())?;
        db.execute("INSERT INTO locations VALUES (?1,?2,?3,?4,?5) ON CONFLICT(source,session_id,pid,start) DO UPDATE SET location=excluded.location WHERE json_extract(excluded.location,'$.observed_at_ms') > json_extract(locations.location,'$.observed_at_ms')", params![source,id,location.pid,format!("{}:{}",location.boot_id.as_deref().unwrap_or("legacy"),location.process_start),json]).map_err(|e| e.to_string())?;
    }
    // Bound each session's navigation history without discarding other sessions.
    db.execute("DELETE FROM locations WHERE source=?1 AND session_id=?2 AND rowid NOT IN (SELECT rowid FROM locations WHERE source=?1 AND session_id=?2 ORDER BY json_extract(location,'$.observed_at_ms') DESC LIMIT 20)", params![source,id]).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn history(
    db: &Connection,
    source: &str,
    id: &str,
) -> Result<Vec<SessionLocation>, String> {
    let mut query = db.prepare("SELECT location FROM locations WHERE source=?1 AND session_id=?2 ORDER BY json_extract(location,'$.observed_at_ms') DESC").map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![source, id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|row| {
            row.map_err(|e| e.to_string())
                .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
        })
        .collect();
    rows
}

pub(super) fn origins(
    db: &Connection,
    source: &str,
    id: &str,
) -> Result<(Option<SessionLocation>, Option<SessionLocation>), String> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT original,latest FROM origins WHERE source=?1 AND session_id=?2",
            params![source, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    row.map(|(first, last)| {
        Ok((
            Some(serde_json::from_str(&first).map_err(|e| e.to_string())?),
            Some(serde_json::from_str(&last).map_err(|e| e.to_string())?),
        ))
    })
    .unwrap_or(Ok((None, None)))
}

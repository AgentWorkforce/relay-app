use super::*;

/// A persistent user deny, independent of transient folder consent.
pub fn is_explicitly_excluded(conn: &Connection, identity: &SessionIdentity) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM delivery_exclusions WHERE source=? AND session_id=?)",
        params![identity.source, identity.session_id],
        |row| row.get::<_, bool>(0),
    )?)
}

/// The combined explicit and folder consent rule for leaving the machine.
pub fn is_shareable(conn: &Connection, identity: &SessionIdentity) -> Result<bool> {
    Ok(!is_explicitly_excluded(conn, identity)?
        && !in_unshared_folder(conn, &identity.source, &identity.session_id)?)
}

/// Records `session` as excluded: its records waiting in an oops window
/// never send.
pub(super) fn exclude(conn: &Connection, session: &SessionIdentity) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO delivery_exclusions(source,session_id) VALUES (?,?)",
        params![session.source, session.session_id],
    )?;
    hold::drop_session(conn, None, session)
}

/// Exclusions are persistent and source-scoped. They are rechecked when a batch
/// is claimed/prepared. This cannot revoke a request already sent to a server.
/// Removing an existing exclusion requires cancelling affected jobs first and
/// creating new generations afterward, so previously skipped history receives
/// a fresh baseline. Jobs permanently excluding this session are unaffected.
pub fn set_session_excluded(
    conn: &Connection,
    session: &SessionIdentity,
    value: bool,
) -> Result<()> {
    let tx = write_transaction(conn)?;
    set_session_excluded_in_transaction(&tx, session, value, None)?;
    if value {
        // A person can turn off a session already withheld by the mode.
        // Persist that stronger choice even when effective consent is unchanged.
        tx.execute("UPDATE delivery_exclusions SET explicit_deny=1 WHERE source=? AND session_id=? AND explicit_deny=0", params![session.source, session.session_id])?;
    }
    tx.commit()?;
    Ok(())
}

/// Automatically withholding a session must not erase a person's saved denial.
/// Withdrawing a mode baseline removes only exclusions recorded as automatic.
pub fn set_policy_excluded(
    conn: &Connection,
    session: &SessionIdentity,
    value: bool,
) -> Result<()> {
    let tx = write_transaction(conn)?;
    let current = tx
        .query_row(
            "SELECT explicit_deny FROM delivery_exclusions WHERE source=? AND session_id=?",
            params![session.source, session.session_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    if current == Some(true) {
        return Ok(());
    }
    set_session_excluded_in_transaction(&tx, session, value, None)?;
    if value {
        tx.execute(
            "UPDATE delivery_exclusions SET explicit_deny=0 WHERE source=? AND session_id=?",
            params![session.source, session.session_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// User denials survive switching between selected, new and all uploads.
pub fn user_exclusions(conn: &Connection) -> Result<Vec<SessionIdentity>> {
    let mut statement = conn.prepare("SELECT source,session_id FROM delivery_exclusions WHERE explicit_deny=1 ORDER BY source,session_id")?;
    let rows = statement.query_map([], |row| {
        Ok(SessionIdentity {
            source: row.get(0)?,
            session_id: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

use super::*;

/// Test fixtures write capture evidence through its own writer, as the
/// production collector does. Never make the delivery attachment writable.
#[cfg(test)]
pub(crate) enum EvidenceWriter<'a> {
    Borrowed(&'a Connection),
    Owned(Connection),
}
#[cfg(test)]
impl std::ops::Deref for EvidenceWriter<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Self::Borrowed(conn) => conn,
            Self::Owned(conn) => conn,
        }
    }
}
#[cfg(test)]
pub(crate) fn test_evidence(conn: &Connection) -> EvidenceWriter<'_> {
    if conn.path().is_none_or(str::is_empty) {
        return EvidenceWriter::Borrowed(conn);
    }
    let path = sidecar::history_path(conn).expect("fixture evidence path");
    if conn
        .path()
        .is_some_and(|main| std::path::Path::new(main) == path)
    {
        EvidenceWriter::Borrowed(conn)
    } else {
        EvidenceWriter::Owned(open_connection(&path).expect("fixture capture writer"))
    }
}

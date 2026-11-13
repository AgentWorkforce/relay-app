use super::*;

#[test]
fn missing_state_rebuilds_orphaned_membership_and_counts_exclusions() {
    let (_directory, conn) = store();
    let job_id = job(&conn);
    capture(&conn, "codex:denied");
    capture(&conn, "codex:gone");
    set_session_excluded(&conn, &identity("codex:denied"), true).unwrap();
    set_session_excluded(&conn, &identity("codex:not-captured"), true).unwrap();
    refresh(&conn).unwrap();

    conn.execute("DELETE FROM delivery_session_census_state", [])
        .unwrap();
    crate::history::delivery::test_evidence(&conn)
        .execute("DELETE FROM sessions WHERE session_id='gone'", [])
        .unwrap();
    capture(&conn, "codex:added");
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 2 }
    );
    agrees(&conn, &job_id);
    assert_eq!(
        counts(&conn, &job_id).unwrap().unwrap(),
        SessionCounts {
            total: 2,
            shared: 1,
            excluded: 1,
        }
    );
    let members = conn
        .prepare("SELECT session_id FROM delivery_session_census ORDER BY session_id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(members, ["added", "denied"]);
    assert!(is_explicitly_excluded(&conn, &identity("codex:not-captured")).unwrap());
}

#[test]
fn full_take_repairs_drifted_counters_for_unchanged_membership() {
    let (_directory, conn) = store();
    let job_id = job(&conn);
    capture(&conn, "codex:denied");
    capture(&conn, "codex:shared");
    set_session_excluded(&conn, &identity("codex:denied"), true).unwrap();
    set_session_excluded(&conn, &identity("codex:not-captured"), true).unwrap();
    refresh(&conn).unwrap();

    conn.execute(
        "UPDATE delivery_session_census_state SET total=99,excluded=77,revision=revision+1000",
        [],
    )
    .unwrap();
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 2 }
    );
    agrees(&conn, &job_id);
    assert_eq!(
        counts(&conn, &job_id).unwrap().unwrap(),
        SessionCounts {
            total: 2,
            shared: 1,
            excluded: 1,
        }
    );
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

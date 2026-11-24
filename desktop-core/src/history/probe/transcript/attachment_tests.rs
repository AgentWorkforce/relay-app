//! The reader draws the images a relayed message brought, from the files
//! the core saved for it. Invented text and tiny invented files only.
use super::*;
use std::path::{Path, PathBuf};

struct Fixture {
    temp: tempfile::TempDir,
    conn: Connection,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app").join("attachments");
    std::fs::create_dir_all(&root).unwrap();
    let db = temp.path().join("history.db");
    super::super::history::open(&db).unwrap();
    let conn = Connection::open(db).unwrap();
    Fixture { temp, conn, root }
}

/// A tiny invented file saved where the core saves a message's file.
fn saved(dir: &Path, file_id: &str, name: &str) -> PathBuf {
    let path = dir.join(file_id).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"\x89PNG").unwrap();
    path
}

fn line(name: &str, kind: &str, path: &Path) -> String {
    format!("- {name} ({kind}, 4 B) saved to {}", path.display())
}

fn insert(conn: &Connection, ts: i64, uid: &str, role: &str, text: &str) {
    crate::history::delivery::test_evidence(conn)
        .execute(
            "INSERT INTO session_events(source,session_id,ts_ms,event_uid,role,kind,text) VALUES('claude','s',?1,?2,?3,'text',?4)",
            rusqlite::params![ts, uid, role, text],
        )
        .unwrap();
}

fn page(f: &Fixture) -> LocalTranscriptPage {
    read_page(&f.conn, &f.root, "claude", "s", None).unwrap()
}

fn path_text(path: &Path) -> Option<&str> {
    path.to_str()
}

/// The text a turn keeps, and the image it draws, when nothing is accepted.
fn untouched(f: &Fixture, text: &str) {
    insert(&f.conn, 1, "m", "user", text);
    let page = page(f);
    assert_eq!(page.entries.len(), 1, "{text}");
    assert_eq!(page.entries[0].text, text);
    assert_eq!(page.entries[0].image_path, None, "{text}");
}

#[test]
fn one_saved_image_draws_with_the_text_before_it() {
    let f = fixture();
    let file = saved(&f.root, "file_1", "shot.png");
    let text = format!(
        "Here is the screen\n\nAttachments:\n{}",
        line("shot.png", "image/png", &file)
    );
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].text, "Here is the screen");
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&file));
    assert_eq!(page.entries[0].id, "m");
    assert_eq!(page.entries[0].kind, "text");
    assert!(!page.entries[0].truncated);
}

#[test]
fn an_image_only_message_is_an_image_entry_not_a_filtered_row() {
    let f = fixture();
    let file = saved(&f.root, "file_1", "shot.jpg");
    let text = format!("Attachments:\n{}", line("shot.jpg", "image/jpeg", &file));
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].text, "");
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&file));
}

#[test]
fn a_framed_message_keeps_its_frame_and_draws_the_block_after_it() {
    let f = fixture();
    let file = saved(&f.root, "file_1", "chart.webp");
    let framed = "[via Agent Relay — from @alice to @worker · message m1 · ref 0a1b2c3d4e5f]\n\nsee this\n[end of Agent Relay message · ref 0a1b2c3d4e5f]";
    let text = format!(
        "{framed}\n\nAttachments:\n{}",
        line("chart.webp", "image/webp", &file)
    );
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(page.entries[0].text, framed);
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&file));
}

#[test]
fn a_block_written_inside_a_body_is_text_and_only_the_real_block_draws() {
    let f = fixture();
    let real = saved(&f.root, "file_1", "real.png");
    let fake = saved(&f.root, "file_2", "fake.png");
    let body = format!(
        "[via Agent Relay — from @alice to @worker · message m1 · ref 0a1b2c3d4e5f]\n\nlook\n\nAttachments:\n{}\n[end of Agent Relay message · ref 0a1b2c3d4e5f]",
        line("fake.png", "image/png", &fake)
    );
    let text = format!("{body}\n\nAttachments:\n{}", line("real.png", "image/png", &real));
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].text, body);
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&real));

    // Without the real block, the one in the body draws nothing.
    let f = fixture();
    let fake = saved(&f.root, "file_2", "fake.png");
    untouched(
        &f,
        &format!(
            "[via Agent Relay — from @alice to @worker · message m1 · ref 0a1b2c3d4e5f]\n\nlook\n\nAttachments:\n{}\n[end of Agent Relay message · ref 0a1b2c3d4e5f]",
            line("fake.png", "image/png", &fake)
        ),
    );
}

#[test]
fn several_messages_in_one_turn_draw_nothing_but_a_note_after_the_block_stays() {
    let f = fixture();
    let file = saved(&f.root, "file_1", "a.png");
    untouched(
        &f,
        &format!(
            "[via Agent Relay — from @alice to @worker · message m1 · ref 0a1b2c3d4e5f]\n\none\n[end of Agent Relay message · ref 0a1b2c3d4e5f]\n\nAttachments:\n{}\n\n[via Agent Relay — from @bob to @worker · message m2 · ref 6a7b8c9d0e1f]\n\ntwo\n[end of Agent Relay message · ref 6a7b8c9d0e1f]",
            line("a.png", "image/png", &file)
        ),
    );

    let f = fixture();
    let file = saved(&f.root, "file_1", "a.png");
    let text = format!(
        "please check\n\nAttachments:\n{}\n\nA note added after the message.",
        line("a.png", "image/png", &file)
    );
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(
        page.entries[0].text,
        "please check\n\nA note added after the message."
    );
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&file));
}

#[test]
fn two_and_three_images_make_one_group_with_stable_ids() {
    let f = fixture();
    let a = saved(&f.root, "file_1", "a.png");
    let b = saved(&f.root, "file_2", "b.gif");
    let c = saved(&f.root, "file_3", "c.png");
    insert(
        &f.conn,
        1,
        "two",
        "user",
        &format!(
            "two\n\nAttachments:\n{}\n{}",
            line("a.png", "image/png", &a),
            line("b.gif", "image/gif", &b)
        ),
    );
    insert(
        &f.conn,
        2,
        "three",
        "user",
        &format!(
            "Attachments:\n{}\n{}\n{}",
            line("a.png", "image/png", &a),
            line("b.gif", "image/gif", &b),
            line("c.png", "image/png", &c)
        ),
    );
    let page = page(&f);
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        ["two", "two:image:2", "three", "three:image:2", "three:image:3"]
    );
    let images: Vec<Option<&str>> = page
        .entries
        .iter()
        .map(|e| e.image_path.as_deref())
        .collect();
    assert_eq!(
        images,
        [path_text(&a), path_text(&b), path_text(&a), path_text(&b), path_text(&c)]
    );
    let texts: Vec<&str> = page.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["two", "", "", "", ""]);
    for entry in &page.entries {
        assert_eq!(entry.role, "user");
        assert_eq!(entry.kind, "text");
        assert!(!entry.truncated);
    }
    assert_eq!(page.entries[1].timestamp_ms, 1);
    assert_eq!(page.entries[4].timestamp_ms, 2);
}

#[test]
fn a_group_that_crosses_the_page_limit_moves_whole_to_the_next_page() {
    let f = fixture();
    let a = saved(&f.root, "file_1", "a.png");
    let b = saved(&f.root, "file_2", "b.png");
    let c = saved(&f.root, "file_3", "c.png");
    let mut lengths = [60_000usize; 8];
    let write = |lengths: &[usize; 8]| {
        crate::history::delivery::test_evidence(&f.conn)
            .execute("DELETE FROM session_events", [])
            .unwrap();
        for (i, length) in lengths.iter().enumerate() {
            let ts = i as i64 + 1;
            insert(&f.conn, ts, &format!("filler{ts}"), "assistant", &"a".repeat(*length));
        }
        insert(
            &f.conn,
            9,
            "group",
            "user",
            &format!(
                "three\n\nAttachments:\n{}\n{}\n{}",
                line("a.png", "image/png", &a),
                line("b.png", "image/png", &b),
                line("c.png", "image/png", &c)
            ),
        );
        insert(&f.conn, 10, "after", "assistant", "after");
    };
    write(&lengths);
    let all = page(&f);
    assert_eq!(all.entries.len(), 12);
    let size = |e: &LocalTranscriptEntry| serde_json::to_vec(e).unwrap().len();
    let fillers: usize = all.entries[..8].iter().map(size).sum();
    let first = size(&all.entries[8]);
    let group: usize = all.entries[8..11].iter().map(size).sum();
    // Grow the fillers until the group's first entry would still fit but
    // the whole group would not.
    let delta = 512 * 1024 - first - fillers;
    for length in lengths.iter_mut() {
        *length += delta / 8;
    }
    lengths[7] += delta % 8;
    assert!(lengths.iter().all(|l| *l <= 64 * 1024));
    write(&lengths);
    let first_page = page(&f);
    assert_eq!(first_page.entries.len(), 8);
    let fillers: usize = first_page.entries.iter().map(size).sum();
    assert!(fillers + first <= 512 * 1024 && fillers + group > 512 * 1024);
    let cursor = first_page.next_cursor.clone().expect("a next page");
    assert_eq!(cursor.ts_ms, 8);
    let next = read_page(&f.conn, &f.root, "claude", "s", Some(cursor)).unwrap();
    let ids: Vec<&str> = next.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["group", "group:image:2", "group:image:3", "after"]);
}

#[test]
fn a_mixed_block_keeps_the_lines_it_does_not_draw() {
    let f = fixture();
    let shot = saved(&f.root, "file_1", "shot.png");
    let report = saved(&f.root, "file_2", "report.pdf");
    let pdf = line("report.pdf", "application/pdf", &report);
    let missing = "- notes.txt file file_3 (not downloaded: too large); fetch with: a command";
    let text = format!(
        "Look\n\nAttachments:\n{pdf}\n{}\n{missing}",
        line("shot.png", "image/png", &shot)
    );
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(
        page.entries[0].text,
        format!("Look\n\nAttachments:\n{pdf}\n{missing}")
    );
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&shot));
}

#[test]
fn lines_that_fail_a_check_draw_nothing() {
    let cases: Vec<Box<dyn Fn(&Fixture) -> String>> = vec![
        // Not one of the four image types.
        Box::new(|f| line("a.svg", "image/svg+xml", &saved(&f.root, "f", "a.svg"))),
        Box::new(|f| line("a.png", "IMAGE/PNG", &saved(&f.root, "f", "a.png"))),
        // A relative path.
        Box::new(|_| "- a.png (image/png, 4 B) saved to attachments/f/a.png".into()),
        // A `..` component, even one that lands back inside the root.
        Box::new(|f| {
            saved(&f.root, "f", "a.png");
            line("a.png", "image/png", &f.root.join("f").join("..").join("f").join("a.png"))
        }),
        // Outside the root, and in the root but not under a file id.
        Box::new(|f| line("a.png", "image/png", &saved(&f.temp.path().join("other"), "f", "a.png"))),
        Box::new(|f| {
            let path = f.root.join("a.png");
            std::fs::write(&path, b"x").unwrap();
            line("a.png", "image/png", &path)
        }),
        // A missing file, and a directory in a file's place.
        Box::new(|f| line("a.png", "image/png", &f.root.join("f").join("a.png"))),
        Box::new(|f| {
            let path = f.root.join("f").join("a.png");
            std::fs::create_dir_all(&path).unwrap();
            line("a.png", "image/png", &path)
        }),
        // A file larger than the core ever saves.
        Box::new(|f| {
            let path = saved(&f.root, "f", "a.png");
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(agent_relay_node::relay::files::MAX_FILE_BYTES + 1)
                .unwrap();
            line("a.png", "image/png", &path)
        }),
        // No type at all, and a size the core never writes.
        Box::new(|f| format!("- a.png (4 B) saved to {}", saved(&f.root, "f", "a.png").display())),
        Box::new(|f| format!("- a.png (image/png, 4 bytes) saved to {}", saved(&f.root, "f", "a.png").display())),
    ];
    for case in cases {
        let f = fixture();
        let text = format!("hi\n\nAttachments:\n{}", case(&f));
        untouched(&f, &text);
    }
    // A heading that is not its own paragraph is not a block.
    let f = fixture();
    let file = saved(&f.root, "f", "a.png");
    untouched(&f, &format!("hi\nAttachments:\n{}", line("a.png", "image/png", &file)));
}

#[cfg(unix)]
#[test]
fn a_link_that_leaves_the_root_draws_nothing() {
    let f = fixture();
    let outside = saved(&f.temp.path().join("other"), "f", "a.png");
    std::fs::create_dir_all(f.root.join("f")).unwrap();
    let link = f.root.join("f").join("a.png");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    untouched(&f, &format!("hi\n\nAttachments:\n{}", line("a.png", "image/png", &link)));
}

#[test]
fn a_name_with_spaces_parentheses_and_saved_to_is_read_from_the_right() {
    let f = fixture();
    let name = "my (old) shot saved to desk.png";
    let file = saved(&f.root, "file_1", name);
    let text = format!("see\n\nAttachments:\n{}", line(name, "image/png", &file));
    insert(&f.conn, 1, "m", "user", &text);
    let page = page(&f);
    assert_eq!(page.entries[0].text, "see");
    assert_eq!(page.entries[0].image_path.as_deref(), path_text(&file));
}

#[test]
fn assistant_context_and_cut_rows_keep_their_text() {
    let f = fixture();
    let file = saved(&f.root, "file_1", "a.png");
    let block = format!("Attachments:\n{}", line("a.png", "image/png", &file));
    let text = format!("hi\n\n{block}");
    insert(&f.conn, 1, "assistant", "assistant", &text);
    crate::history::delivery::test_evidence(&f.conn)
        .execute(
            "INSERT INTO session_events(source,session_id,ts_ms,event_uid,role,kind,text,control_kind) VALUES('claude','s',2,'control','user','text',?1,'compaction')",
            [&text],
        )
        .unwrap();
    let context = format!(
        "# Environment\nYou have been invoked in the following environment: \n - Primary working directory: a folder\n\n{block}"
    );
    insert(&f.conn, 3, "environment", "user", &context);
    let long = format!("{}\n\n{block}", "x".repeat(70_000));
    insert(&f.conn, 4, "cut", "user", &long);
    let page = page(&f);
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["assistant", "control", "environment", "cut"]);
    for entry in &page.entries {
        assert_eq!(entry.image_path, None, "{}", entry.id);
    }
    assert_eq!(page.entries[0].text, text);
    assert_eq!(page.entries[1].text, text);
    assert_eq!(page.entries[1].kind, "context");
    assert_eq!(page.entries[2].text, context);
    assert_eq!(page.entries[2].kind, "context");
    assert_eq!(page.entries[3].text.len(), 64 * 1024);
    assert!(page.entries[3].truncated);
}

#[test]
fn an_embedded_image_row_is_drawn_as_before() {
    let f = fixture();
    insert(
        &f.conn,
        1,
        "m",
        "user",
        "![Attached image](data:image/png;base64,iVBORw0KGgo=)",
    );
    let page = page(&f);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].text, "");
    let image = page.entries[0].image_path.as_deref().unwrap();
    assert!(Path::new(image).starts_with(f.temp.path().join("images")));
}

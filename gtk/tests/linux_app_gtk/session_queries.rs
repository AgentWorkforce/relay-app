use super::pages::named;
use super::*;

fn empty_continuation_page_does_not_claim_history_is_exhausted(runtime: &tokio::runtime::Runtime) {
    let (release, held) = tokio::sync::watch::channel(false);
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query", Answer::with(move |_| {
            let mut held = held.clone();
            async move {
                held.wait_for(|released| *released).await.unwrap();
                Ok(json!({"sessions":[],"local_sessions":[],"revision":1,"unchanged":false,"verification_pending":false,"candidates_scanned":200,"next_cursor":"next"}))
            }
        }));
    });
    case.set_state(workspace_snapshot());
    case.app.show_window();
    case.pump(|case| {
        case.app.window().is_mapped()
            && case
                .app
                .with_state(|s| !s.local_sessions.is_empty() && !s.roster.is_empty())
            && !case.called("uploader.sessions.query").is_empty()
    });
    let shell = case.app.shell().unwrap();
    assert!(case.app.with_state(|s| s.refreshing_sessions));
    // Mapping and Relay's independent startup reads do not finish the query.
    // Release it only once they have arrived, then await the actual rendered page.
    release.send(true).unwrap();
    case.pump(|case| {
        case.app
            .with_state(|s| s.session_query.next_cursor.is_some() && !s.refreshing_sessions)
            && labels(&shell.pages.sessions.empty).contains(&"No matches on this page.".into())
    });
    let empty = labels(&shell.pages.sessions.empty);
    assert!(empty.contains(&"No matches on this page.".into()));
    assert!(empty.contains(&"Continue to search older local history.".into()));
    assert!(!empty.contains(&"No sessions found yet".into()));
    assert!(buttons(&shell.pages.sessions.root, "Next page")
        .iter()
        .any(|b| b.is_visible()));
    assert!(
        case.called("uploader.sessions.query")
            .iter()
            .all(|params| params["cursor"].is_null()),
        "An empty intermediate page must not automatically scan the next candidates"
    );
}

fn expired_facet_cursor_restarts_without_another_session_query(runtime: &tokio::runtime::Runtime) {
    let (release_facets, held_facets) = tokio::sync::watch::channel(false);
    let rounds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = rounds.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query", Answer::Value(json!({"archived_keys":["codex:old-archive"],"sessions":[upload_row("codex","old-archive","Old archive","/work",false,"not_shared")],"local_sessions":[],"revision":1,"unchanged":false,"verification_pending":false,"candidates_scanned":0,"next_cursor":null})));
        core.answer("uploader.sessions.facets", Answer::with(move |params| {
            let seen = seen.clone();
            let mut held = held_facets.clone();
            async move {
                held.wait_for(|released| *released).await.unwrap();
                if params["cursor"] == "expired" { return Err(RpcError::new(ErrorKind::Busy, "Facet membership changed")); }
                let round = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(if round == 0 { json!({"harnesses":["codex"],"projects":[],"revision":1,"next_cursor":"expired"}) }
                else { json!({"harnesses":["codex","claude"],"projects":["complete"],"revision":2,"next_cursor":null}) })
            }
        }));
    });
    case.set_state(workspace_snapshot());
    case.app.show_window();
    case.pump(|case| case.app.shell().is_some());
    let shell = case.app.shell().unwrap();
    // Initial Relay/roster replies can legitimately restart the session query.
    // Hold the facet read until those query inputs and the rendered page settle.
    case.pump(|case| {
        case.app.window().is_mapped()
            && case.app.with_state(|s| {
                !s.local_sessions.is_empty()
                    && !s.roster.is_empty()
                    && !s.refreshing_sessions
                    && !s.stale_sessions
                    && s.session_query.facets_loading
            })
            && case.called("uploader.sessions.facets").len() == 1
            && labels(&shell.pages.sessions.list).contains(&"Archived".into())
    });
    let queries_before_repair = case.called("uploader.sessions.query").len();
    release_facets.send(true).unwrap();
    case.pump(|case| {
        case.app.with_state(|s| {
            !s.session_query.facets_loading
                && s.session_query
                    .facets
                    .as_ref()
                    .is_some_and(|f| f.revision == 2)
        })
    });
    assert_eq!(
        case.app
            .with_state(|s| s.session_query.facets.as_ref().unwrap().projects.clone()),
        ["complete"]
    );
    assert!(
        labels(&case.app.shell().unwrap().pages.sessions.list).contains(&"Archived".into()),
        "Page archive classification must survive the bounded recent-agent list"
    );
    assert_eq!(rounds.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        case.called("uploader.sessions.facets")
            .iter()
            .map(|params| params["cursor"].clone())
            .collect::<Vec<_>>(),
        [Value::Null, json!("expired"), Value::Null]
    );
    assert_eq!(
        case.called("uploader.sessions.query").len(),
        queries_before_repair,
        "Repairing an expired facet cursor must not restart the session query"
    );
}

fn query_pages_replace_working_set_and_unchanged_replies_keep_rows(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| {
        core.answer("uploader.sessions.query", Answer::with(|params| async move {
            let unchanged = params["known_revision"] == 42;
            let first = params["cursor"].is_null();
            let rows = if unchanged { vec![] } else { vec![upload_row("codex", if first { "first" } else { "old-match" }, "CPU history", "/work/relay", true, "uploaded")] };
            Ok(json!({"sessions":rows,"local_sessions":[],"revision":42,"unchanged":unchanged,"candidates_scanned":if unchanged{0}else{200},"next_cursor":if first{json!("next")}else{Value::Null}}))
        }));
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app.with_state(|s| {
            s.sessions
                .first()
                .is_some_and(|row| row.session_id == "first")
        })
    });
    case.app.show_window();
    case.pump(|case| case.app.window().is_mapped());
    let shell = case.app.shell().unwrap();
    let next = buttons(&shell.pages.sessions.root, "Next page")
        .into_iter()
        .find(|button| button.label().as_deref() == Some("Next page"))
        .unwrap();
    case.pump(|_| next.is_visible());
    assert!(next.is_visible());
    case.app.update_state(|s| {
        s.session_selection.insert("codex:first".into());
    });
    next.emit_clicked();
    case.pump(|case| {
        case.app.with_state(|s| {
            s.sessions
                .first()
                .is_some_and(|row| row.session_id == "old-match")
        })
    });
    case.app.with_state(|s| {
        assert_eq!(s.sessions.len(), 1);
        assert!(
            s.session_selection.is_empty(),
            "Previous-page selection cannot survive page replacement"
        );
        assert_eq!(s.session_query.limit, 500);
        assert_eq!(s.session_query.local_sessions.as_ref().unwrap().len(), 0);
    });
    let before = case.called("uploader.sessions.query").len();
    case.app.refresh_uploader(true);
    case.pump(|case| {
        case.called("uploader.sessions.query").len() > before
            && case.app.with_state(|s| !s.refreshing_sessions)
    });
    assert_eq!(
        case.app.with_state(|s| s.sessions[0].session_id.clone()),
        "old-match"
    );
    assert!(case.called("uploader.sessions.list").is_empty());
    case.pump(|case| case.app.with_state(|s| !s.local_sessions.is_empty()));
    let local = case.app.with_state(|s| s.local_sessions[0].clone());
    let before = case.called("uploader.sessions.query").len();
    case.app.toggle_relay(&local, true);
    case.pump(|case| case.called("uploader.sessions.query").len() > before);
    assert!(
        case.called("uploader.sessions.query")[before..]
            .iter()
            .all(|p| p["cursor"].is_null()),
        "Own toggle intent resets retained cursor before requesting new global order"
    );

    assert!(case
        .called("uploader.sessions.query")
        .iter()
        .all(|p| p["limit"] == 200));
}

fn an_earlier_query_response_cannot_replace_the_new_search(runtime: &tokio::runtime::Runtime) {
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query", Answer::with(move |params| {
            let gate = gate.clone();
            async move {
                let query = params["query"].as_str().unwrap_or("");
                if query == "old" { gate.notified().await; }
                let id = if query.is_empty() { "initial" } else { query };
                Ok(json!({"sessions":[upload_row("codex",id,id,"/work/relay",false,"not_shared")],"local_sessions":[],"revision":1,"unchanged":false,"candidates_scanned":1,"next_cursor":null}))
            }
        }));
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| case.app.with_state(|s| !s.sessions.is_empty()));
    case.app.set_session_search("old");
    case.pump(|case| {
        case.called("uploader.sessions.query")
            .iter()
            .any(|p| p["query"] == "old")
    });
    let mut config = case.app.session_view_configuration();
    config.search = "new".into();
    config.sort.column = agent_relay_app::models::LocalSessionSortColumn::Reasoning;
    case.app
        .apply_session_view(&agent_relay_app::models::SavedSessionView {
            id: 1,
            name: "saved".into(),
            configuration: config,
        })
        .unwrap();
    assert_ne!(
        case.app.session_view_configuration().sort.column,
        agent_relay_app::models::LocalSessionSortColumn::Reasoning
    );
    release.notify_one();
    case.pump(|case| {
        case.app.with_state(|s| {
            assert!(s.sessions.iter().all(|row| row.session_id != "old"));
            !s.refreshing_sessions
                && s.sessions
                    .first()
                    .is_some_and(|row| row.session_id == "new")
        })
    });
    assert!(case
        .called("uploader.sessions.query")
        .iter()
        .any(|p| p["query"] == "new" && p["sort"] == "updated"));
}

fn hiding_an_offpage_key_stops_upload_once(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        core.answer("uploader.sessions.query",Answer::Value(json!({"sessions":[],"local_sessions":[],"revision":42,"unchanged":false,"verification_pending":false,"candidates_scanned":0,"next_cursor":null})));
        core.answer(
            "uploader.sessions.exclude",
            Answer::Value(json!({"updated":1})),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app
            .with_state(|s| !s.refreshing_sessions && s.sessions_refreshed.is_some())
    });
    case.app
        .set_sessions_hidden(vec!["codex:off-page".into()], true);
    case.pump(|case| {
        case.called("uploader.sessions.exclude").len() == 1
            && case.app.with_state(|s| s.hiding_sessions.is_empty())
    });
    assert!(case.app.hidden_sessions().contains("codex:off-page"));
    let before = case.called("uploader.sessions.query").len();
    case.app.refresh_uploader(true);
    case.pump(|case| {
        case.called("uploader.sessions.query").len() > before
            && case.app.with_state(|s| !s.refreshing_sessions)
    });
    assert_eq!(
        case.called("uploader.sessions.exclude").len(),
        1,
        "Confirmed offpage exclusions must not repeat on each poll"
    );
}

fn hidden_offpage_policy_change_stops_external_upload_without_idle_queries(
    runtime: &tokio::runtime::Runtime,
) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let included = Arc::new(AtomicBool::new(true));
    let policy = Arc::new(AtomicU64::new(10));
    let sharing = included.clone();
    let revision = policy.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query", Answer::Value(json!({"sessions":[],"local_sessions":[],"revision":42,"unchanged":false,"verification_pending":false,"candidates_scanned":0,"next_cursor":null})));
        let status_revision = revision.clone();
        core.answer(
            "uploader.status",
            Answer::with(move |_| {
                let mut status = uploader();
                status["upload_policy_revision"] = json!(status_revision.load(Ordering::SeqCst));
                async move { Ok(status) }
            }),
        );
        core.answer(
            "uploader.sessions.exclude",
            Answer::with(move |_| {
                if sharing.swap(false, Ordering::SeqCst) {
                    revision.fetch_add(1, Ordering::SeqCst);
                }
                async { Ok(json!({"updated":1})) }
            }),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app
            .with_state(|s| s.status.is_some() && !s.refreshing_sessions)
    });
    case.settle();
    case.app
        .set_sessions_hidden(vec!["codex:off-page".into()], true);
    case.pump(|case| {
        !included.load(Ordering::SeqCst) && case.app.with_state(|s| s.hiding_sessions.is_empty())
    });
    case.settle();
    case.app.refresh_uploader(false);
    case.settle();
    let exclusions = case.called("uploader.sessions.exclude").len();
    let queries = case.called("uploader.sessions.query").len();
    included.store(true, Ordering::SeqCst);
    policy.fetch_add(1, Ordering::SeqCst); // Counts, mode and filtered rows stay identical.
    case.app.refresh_uploader(false);
    case.pump(|case| {
        !included.load(Ordering::SeqCst) && case.app.with_state(|s| s.hiding_sessions.is_empty())
    });
    assert!(case.app.hidden_sessions().contains("codex:off-page"));
    assert!(case.app.with_state(|s| s.sessions.is_empty()));
    case.app.refresh_uploader(false);
    case.settle();
    let settled_exclusions = case.called("uploader.sessions.exclude").len();
    for _ in 0..3 {
        case.app.refresh_uploader(false);
        case.settle();
    }
    assert_eq!(
        case.called("uploader.sessions.exclude").len(),
        settled_exclusions,
        "Unchanged policy must not repeat exclusions"
    );
    assert!(
        settled_exclusions - exclusions <= 2,
        "Policy change and acknowledgment must converge"
    );
    assert_eq!(
        case.called("uploader.sessions.query").len(),
        queries,
        "Policy-only reads must not restart history queries"
    );
}

fn hidden_policy_change_during_held_exclusion_cannot_confirm_old_write(
    runtime: &tokio::runtime::Runtime,
) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let included = Arc::new(AtomicBool::new(true));
    let policy = Arc::new(AtomicU64::new(10));
    let held = Arc::new(Notify::new());
    let requests = Arc::new(AtomicU64::new(0));
    let sharing = included.clone();
    let revision = policy.clone();
    let release = held.clone();
    let calls = requests.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query", Answer::Value(json!({"sessions":[],"local_sessions":[],"revision":42,"unchanged":false,"verification_pending":false,"candidates_scanned":0,"next_cursor":null})));
        let status_revision = revision.clone();
        core.answer(
            "uploader.status",
            Answer::with(move |_| {
                let mut status = uploader();
                status["upload_policy_revision"] = json!(status_revision.load(Ordering::SeqCst));
                async move { Ok(status) }
            }),
        );
        core.answer(
            "uploader.sessions.exclude",
            Answer::with(move |_| {
                let request = calls.fetch_add(1, Ordering::SeqCst);
                if sharing.swap(false, Ordering::SeqCst) {
                    revision.fetch_add(1, Ordering::SeqCst);
                }
                let held = release.clone();
                async move {
                    if request == 0 {
                        held.notified().await;
                    }
                    Ok(json!({"updated":1}))
                }
            }),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app
            .with_state(|s| s.status.is_some() && !s.refreshing_sessions)
    });
    case.settle();
    case.app
        .set_sessions_hidden(vec!["codex:off-page".into()], true);
    case.pump(|_| requests.load(Ordering::SeqCst) == 1 && !included.load(Ordering::SeqCst));
    included.store(true, Ordering::SeqCst);
    let observed = policy.fetch_add(1, Ordering::SeqCst) + 1;
    case.app.refresh_uploader(false);
    case.pump(|case| {
        case.app
            .with_state(|s| s.session_query.upload_policy_revision == Some(observed as i64))
    });
    held.notify_one();
    case.pump(|case| {
        requests.load(Ordering::SeqCst) == 2
            && !included.load(Ordering::SeqCst)
            && case.app.with_state(|s| s.hiding_sessions.is_empty())
    });
    assert!(case.app.hidden_sessions().contains("codex:off-page"));
    assert!(case
        .app
        .with_state(|s| !s.upload_errors.contains_key("codex:off-page")));
}

fn hidden_intent_waits_for_attach_and_busy_history(runtime: &tokio::runtime::Runtime) {
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saved = ready.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query",Answer::Value(json!({"sessions":[],"local_sessions":[],"revision":42,"unchanged":false,"verification_pending":false,"candidates_scanned":0,"next_cursor":null})));
        core.answer(
            "uploader.sessions.exclude",
            Answer::with(move |_| {
                let ready = saved.clone();
                async move {
                    if ready.load(std::sync::atomic::Ordering::SeqCst) {
                        Ok(json!({"updated":1}))
                    } else {
                        Err(RpcError::new(
                            ErrorKind::Busy,
                            "Session history is preparing.",
                        ))
                    }
                }
            }),
        );
    });
    case.app
        .set_sessions_hidden(vec!["codex:future".into()], true);
    case.pump(|case| case.app.with_state(|s| s.hiding_sessions.is_empty()));
    assert!(case.app.hidden_sessions().contains("codex:future"));
    assert!(case.called("uploader.sessions.exclude").is_empty());
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        !case.called("uploader.sessions.exclude").is_empty()
            && case.app.with_state(|s| s.hiding_sessions.is_empty())
    });
    assert!(case.app.hidden_sessions().contains("codex:future"));
    assert!(case
        .app
        .with_state(|s| !s.session_query.hidden_confirmed.contains("codex:future")));
    case.settle();
    let queries = case.called("uploader.sessions.query").len();
    ready.store(true, std::sync::atomic::Ordering::SeqCst);
    case.pump(|case| {
        case.app
            .with_state(|s| s.session_query.hidden_confirmed.contains("codex:future"))
    });
    assert!(case.app.hidden_sessions().contains("codex:future"));
    assert_eq!(
        case.called("uploader.sessions.query").len(),
        queries,
        "Deferred exclusion retries must not spin listing queries"
    );
}

fn hidden_pending_toggle_survives_disconnect_and_reconciles_after_reconnect(
    runtime: &tokio::runtime::Runtime,
) {
    let (release_status, held_status) = tokio::sync::watch::channel(false);
    let held = Arc::new(Notify::new());
    let release = held.clone();
    let case = Case::with(runtime, move |core| {
        core.answer("uploader.sessions.query",Answer::Value(json!({"sessions":[upload_row("codex","held","Held", "/work/relay",false,"not_shared")],"local_sessions":[],"revision":42,"unchanged":false,"verification_pending":false,"candidates_scanned":1,"next_cursor":null})));
        core.answer(
            "uploader.status",
            Answer::with(move |_| {
                let mut held = held_status.clone();
                async move {
                    held.wait_for(|released| *released).await.unwrap();
                    Ok(uploader())
                }
            }),
        );
        core.answer(
            "uploader.sessions.include",
            Answer::with(move |_| {
                let gate = release.clone();
                async move {
                    gate.notified().await;
                    Ok(json!({"updated":1}))
                }
            }),
        );
        core.answer(
            "uploader.sessions.exclude",
            Answer::Value(json!({"updated":1})),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app
            .with_state(|s| s.sessions.iter().any(|r| r.session_id == "held"))
    });
    // Query rows arrive independently of upload policy. The row alone does not
    // establish that its Upload control can accept the mutation under test.
    assert!(case
        .app
        .with_state(|s| s.status.is_none() && s.sessions_read_only()));
    case.app.change_uploads(vec!["codex:held".into()], true);
    assert!(case.called("uploader.sessions.include").is_empty());
    assert!(case.app.with_state(|s| s.busy_uploads.is_empty()));
    release_status.send(true).unwrap();
    case.pump(|case| case.app.with_state(|s| !s.sessions_read_only()));
    case.app.change_uploads(vec!["codex:held".into()], true);
    case.pump(|case| case.called("uploader.sessions.include").len() == 1);
    assert!(case
        .app
        .with_state(|s| s.busy_uploads.contains("codex:held")));
    case.app
        .set_sessions_hidden(vec!["codex:held".into()], true);
    assert!(case
        .app
        .with_state(|s| s.hiding_sessions.contains("codex:held")));
    case.app.on_connection(false, None);
    assert!(
        case.app.with_state(|s| s.hiding_sessions.is_empty()),
        "Disconnect must clear aborted hide markers"
    );
    assert!(case.app.hidden_sessions().contains("codex:held"));
    held.notify_one();
    case.app.on_connection(true, None);
    case.app
        .on_status(reader::result::<method::StatusGet>(workspace_snapshot()).unwrap());
    case.app.refresh_uploader(true);
    case.pump(|case| {
        case.app.with_state(|s| {
            s.session_query.hidden_confirmed.contains("codex:held") && s.hiding_sessions.is_empty()
        })
    });
    assert!(case.app.hidden_sessions().contains("codex:held"));
    assert!(!case.called("uploader.sessions.exclude").is_empty());
    case.app
        .set_sessions_hidden(vec!["codex:held".into()], false);
    assert!(
        !case.app.hidden_sessions().contains("codex:held"),
        "Reconnect must not leave an unhide-blocking marker"
    );
}

fn unhiding_claude_web_session_removes_target_and_history_aliases(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    let target = "claude-cloud:session_019a7SvoqfvBg4Z6zbjanBat";
    let history = "claude:cse_019a7SvoqfvBg4Z6zbjanBat";
    case.app
        .set_sessions_hidden(vec![target.into(), history.into()], true);
    case.pump(|case| {
        case.app
            .with_state(|state| state.hiding_sessions.is_empty())
    });
    assert!(case.app.hidden_sessions().contains(target));
    assert!(case.app.hidden_sessions().contains(history));
    assert!(case
        .app
        .with_state(|state| state.session_query.hidden_confirmed.contains(target)));

    case.app.set_sessions_hidden(vec![history.into()], false);

    assert!(!case.app.hidden_sessions().contains(target));
    assert!(!case.app.hidden_sessions().contains(history));
    assert!(case
        .app
        .with_state(|state| state.session_query.hidden_confirmed.is_empty()));
}

fn agent_search_offers_roster_agents_and_shows_agent_chips(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.sign_in();
    case.pump(|case| case.app.with_state(|s| !s.roster.is_empty()));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions;
    let filters: gtk::MenuButton = named(&shell.header_search, "session-filters").remove(0);
    let popover = filters.popover().unwrap();
    filters.popup();
    // Offered: the choice and its section are shown (the popover itself
    // need not be mapped here).
    let shown = |title: &str| {
        buttons(&popover, title)
            .iter()
            .any(|b| b.get_visible() && b.parent().is_some_and(|p| p.get_visible()))
    };
    // The roster feeds an Agents section, filtered by the prefix after `@`.
    case.app.set_session_search("fix @b");
    case.settle();
    assert!(shown("@builder") && shown("@bos"));
    assert!(
        !shown("@remote"),
        "Only agents starting with the typed prefix"
    );
    assert!(!shown("Codex"), "An @ prefix is not a harness search");
    case.app.set_session_search("fix @");
    case.settle();
    assert!(shown("@remote") && shown("@napper"));
    case.app.set_session_search("fix @bu");
    case.settle();
    buttons(&popover, "@builder")[0].emit_clicked();
    assert_eq!(page.search.text(), "fix @builder ");
    filters.popdown();
    case.pump(|_| !popover.is_visible());

    // Agent tokens become removable chips; only the first one filters.
    let note: gtk::Label = named(&shell.header_search, "session-agent-note").remove(0);
    case.app.set_session_search("@Builder login");
    case.settle();
    assert!(labels(&shell.header_search).contains(&"@Builder".into()));
    assert!(!note.get_visible());
    case.app.set_session_search("@builder @bos @BUILDER");
    case.settle();
    assert!(note.get_visible());
    assert_eq!(note.text(), "One agent at a time");
    let chip: gtk::Button = named(&shell.header_search, "session-filter-chip-1").remove(0);
    chip.emit_clicked();
    assert_eq!(page.search.text(), "@builder @BUILDER");
    case.settle();
    assert!(!note.get_visible(), "The same agent twice is one agent");
    // A prefix needs three letters or digits before the star.
    case.app.set_session_search("@bu*");
    case.settle();
    assert!(labels(&shell.header_search).contains(&"@bu* (too short)".into()));
    case.app.set_session_search("@bui*");
    case.settle();
    assert!(labels(&shell.header_search).contains(&"@bui*".into()));
    // Addresses and a bare @ stay free text: no chip.
    case.app.set_session_search("someone@example.com @");
    case.settle();
    assert!(named::<gtk::Button>(&shell.header_search, "session-filter-chip-0").is_empty());
    // The read API takes agent values up to 200 characters; a longer @word is
    // free text, so it gets no chip.
    let longest = format!("@{}", "a".repeat(200));
    case.app.set_session_search(&longest);
    case.settle();
    assert!(labels(&shell.header_search).contains(&longest));
    case.app
        .set_session_search(&format!("@{}", "a".repeat(201)));
    case.settle();
    assert!(named::<gtk::Button>(&shell.header_search, "session-filter-chip-0").is_empty());
}

fn agent_prefix_refusals_show_unavailable_without_text_fallback(_: &tokio::runtime::Runtime) {
    use agent_relay_app::models::{LocalSessionQuery, AGENT_PREFIX_UNAVAILABLE};
    // The pipeline's v1 refusals of agentMatch = prefix: prefix search is not
    // available yet, never a silent text search.
    for reason in ["param_agentMatch", "agent_match_not_migrated"] {
        let only = LocalSessionQuery::new("@example*")
            .agent_refusal(501, "unsupported_by_backend", Some(reason), "")
            .unwrap();
        assert_eq!(only.notice, AGENT_PREFIX_UNAVAILABLE, "{reason}");
        assert_eq!(only.free_text, None, "{reason}: no results, no fallback");
        let with_text = LocalSessionQuery::new("@example* fix")
            .agent_refusal(501, "unsupported_by_backend", Some(reason), "")
            .unwrap();
        assert_eq!(with_text.notice, AGENT_PREFIX_UNAVAILABLE, "{reason}");
        assert_eq!(with_text.free_text.as_deref(), Some("fix"), "{reason}");
    }
    // A 400 invalid_request (the 200-character cap) shows the server's message.
    let invalid = LocalSessionQuery::new("@example-agent")
        .agent_refusal(400, "invalid_request", None, "agent is too long")
        .unwrap();
    assert_eq!(invalid.notice, "agent is too long");
    assert_eq!(invalid.free_text, None);
}

pub const TESTS: &[(&str, Test)] = &[
    (
        "agent_prefix_refusals_show_unavailable_without_text_fallback",
        agent_prefix_refusals_show_unavailable_without_text_fallback,
    ),
    (
        "agent_search_offers_roster_agents_and_shows_agent_chips",
        agent_search_offers_roster_agents_and_shows_agent_chips,
    ),
    (
        "unhiding_claude_web_session_removes_target_and_history_aliases",
        unhiding_claude_web_session_removes_target_and_history_aliases,
    ),
    (
        "hidden_policy_change_during_held_exclusion_cannot_confirm_old_write",
        hidden_policy_change_during_held_exclusion_cannot_confirm_old_write,
    ),
    (
        "hidden_offpage_policy_change_stops_external_upload_without_idle_queries",
        hidden_offpage_policy_change_stops_external_upload_without_idle_queries,
    ),
    (
        "empty_continuation_page_does_not_claim_history_is_exhausted",
        empty_continuation_page_does_not_claim_history_is_exhausted,
    ),
    (
        "expired_facet_cursor_restarts_without_another_session_query",
        expired_facet_cursor_restarts_without_another_session_query,
    ),
    (
        "hidden_pending_toggle_survives_disconnect_and_reconciles_after_reconnect",
        hidden_pending_toggle_survives_disconnect_and_reconciles_after_reconnect,
    ),
    (
        "hidden_intent_waits_for_attach_and_busy_history",
        hidden_intent_waits_for_attach_and_busy_history,
    ),
    (
        "hiding_an_offpage_key_stops_upload_once",
        hiding_an_offpage_key_stops_upload_once,
    ),
    (
        "query_pages_replace_working_set_and_unchanged_replies_keep_rows",
        query_pages_replace_working_set_and_unchanged_replies_keep_rows,
    ),
    (
        "an_earlier_query_response_cannot_replace_the_new_search",
        an_earlier_query_response_cannot_replace_the_new_search,
    ),
];

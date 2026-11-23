use super::*;
use gtk4::prelude::*;

impl App {
    // A slow status read cannot delay history invalidations or pagination.
    pub(super) fn refresh_upload_sessions(&self) {
        if self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderSessionsQuery::NAME) {
            self.refresh_session_query();
        } else {
            self.refresh_legacy_upload_sessions();
        }
    }
    fn refresh_legacy_upload_sessions(&self) {
        let request = self.update_state(|state| {
            if state.busy || state.refreshing_sessions {
                state.stale_sessions = true;
                return None;
            }
            state.stale_sessions = false;
            state.refreshing_sessions = true;
            Some((
                state.generation,
                state.session_query.limit,
                state.upload_selection_revision,
                state.title_write_revision,
            ))
        });
        if let Some((generation, limit, selection, title_revision)) = request {
            let current = move |app: &App| {
                app.with_state(|state| state.generation == generation && state.connected())
            };
            self.call::<agent_relay_desktop_core::contract::method::UploaderSessionsList>(
                LimitParams { limit: Some(limit) },
                move |app, result| {
                    if app.with_state(|state| state.generation) != generation {
                        return;
                    }
                    app.update_state(|state| {
                        state.refreshing_sessions = false;
                        if state.upload_selection_revision != selection
                            || state.title_write_revision != title_revision
                        {
                            state.uploader_stale = true;
                            state.stale_sessions = true;
                        }
                    });
                    if current(app)
                        && app.with_state(|state| {
                            state.upload_selection_revision == selection
                                && state.title_write_revision == title_revision
                        })
                    {
                        match result {
                            Err(error) => app.refresh_failed(&error),
                            Ok(listed) => {
                                app.update_state(|state| {
                                    let keys: std::collections::BTreeSet<String> =
                                        listed.sessions.iter().map(session_key).collect();
                                    state.session_selection.retain(|key| keys.contains(key));
                                    state.sessions = listed.sessions;
                                    state.sessions_refreshed = Some(crate::ui::now_ms());
                                });
                                app.reconcile_hidden_sessions();
                                app.update_uploader_views();
                            }
                        }
                    }
                    app.refresh_if_stale();
                },
            );
        }
    }

    pub(in crate::ui) fn session_query_changed(&self) {
        self.update_state(|state| {
            state.session_query.revision += 1;
            state.session_query.known_revision = None;
            state.session_query.verification_pending = false;
            state.session_query.cursor = None;
            state.session_query.next_cursor = None;
            state.stale_sessions = true;
        });
    }

    pub(in crate::ui) fn refresh_presentation_query(&self) {
        if !self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderSessionsQuery::NAME) {
            return;
        }
        let changed = self.update_state(|state| {
            use std::hash::{Hash, Hasher};
            let mut wanted = state.wanted.iter().collect::<Vec<_>>();
            wanted.sort_by_key(|(key, _)| *key);
            let mut busy = state.busy_targets.iter().collect::<Vec<_>>();
            busy.sort();
            let mut idle = state
                .roster
                .iter()
                .filter(|a| a.liveness == agent_relay_node::relay::people::Liveness::Idle)
                .map(|a| &a.name)
                .collect::<Vec<_>>();
            idle.sort();
            let mut archived = state
                .archived_agents
                .iter()
                .map(|a| a.target.key())
                .collect::<Vec<_>>();
            archived.sort();
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            // A Devin Cloud session's activity changes by time alone; the
            // core's page must be read again when one does.
            let now = chrono::Utc::now().timestamp_millis();
            crate::models::activity_revision(&state.local_sessions, now).hash(&mut hash);
            (
                wanted,
                busy,
                &state.requesting,
                &state.stopping,
                idle,
                archived,
                state
                    .local_sessions
                    .iter()
                    .map(|s| {
                        (
                            s.upload_key().unwrap_or_else(|| s.target.key()),
                            (
                                &s.title,
                                s.live,
                                s.running,
                                &s.directory,
                                &s.project,
                                &s.host,
                                s.registration
                                    .as_ref()
                                    .map(|r| (&r.name, &r.alias, &r.address)),
                                state.presence_of(s).is_on(),
                            ),
                        )
                    })
                    .collect::<std::collections::BTreeMap<_, _>>(),
            )
                .hash(&mut hash);
            let version = hash.finish();
            let changed = state
                .session_query
                .presentation_revision
                .is_some_and(|old| old != version);
            state.session_query.presentation_revision = Some(version);
            changed
        });
        if changed {
            // Preserve a first page in flight; refresh_session_query coalesces
            // one follow-up read. Completed pages restart from the beginning.
            if self.with_state(|state| state.session_query.local_sessions.is_some()) {
                self.session_query_changed();
            }
            self.refresh_upload_sessions();
        }
    }

    fn global_session_sort(&self) -> crate::models::LocalSessionSort {
        let Some(shell) = self.shell() else {
            return Default::default();
        };
        let page = &shell.pages.sessions;
        let sort = page.sort.get();
        if sort.column == crate::models::LocalSessionSortColumn::Reasoning {
            let supported = Default::default();
            page.sort.set(supported);
            supported
        } else {
            sort
        }
    }
    fn refresh_session_query(&self) {
        use agent_relay_desktop_core::contract::account::SessionQueryParams;
        let sort = self.global_session_sort();
        let hidden = if self.show_hidden_sessions() {
            vec![]
        } else {
            self.hidden_sessions().into_iter().collect()
        };
        let request = self.update_state(|state| {
            if state.busy || state.refreshing_sessions {
                state.stale_sessions = true;
                return None;
            }
            state.stale_sessions = false;
            state.refreshing_sessions = true;
            Some((
                state.generation,
                state.session_query.revision,
                state.upload_selection_revision,
                state.title_write_revision,
                SessionQueryParams {
                    presentation: Some(
                        agent_relay_desktop_core::contract::account::SessionQueryPresentation {
                            idle_names: state
                                .roster
                                .iter()
                                .filter(|a| {
                                    a.liveness == agent_relay_node::relay::people::Liveness::Idle
                                })
                                .map(|a| a.name.clone())
                                .collect(),
                            relay_states: state
                                .local_sessions
                                .iter()
                                .map(|s| {
                                    (
                                        s.upload_key().unwrap_or_else(|| s.target.key()),
                                        state.presence_of(s).is_on(),
                                    )
                                })
                                .collect(),
                            upload_states: state
                                .requesting
                                .union(&state.stopping)
                                .map(|key| (key.clone(), !state.stopping.contains(key)))
                                .collect(),
                            preparing: state.requesting.iter().cloned().collect(),
                        },
                    ),
                    query: state.session_search.clone(),
                    sort: if sort.column == crate::models::LocalSessionSortColumn::Reasoning {
                        "updated"
                    } else {
                        sort.column.query_name()
                    }
                    .into(),
                    ascending: sort.ascending,
                    cursor: state.session_query.cursor.clone(),
                    known_revision: state.session_query.known_revision,
                    limit: Some(200),
                    hidden,
                },
            ))
        });
        let Some((generation, query_revision, selection, title, request)) = request else {
            return;
        };
        self.call::<agent_relay_desktop_core::contract::method::UploaderSessionsQuery>(request, move |app, result| {
            if app.with_state(|state| state.generation) != generation {
                return;
            }
            let current = app.update_state(|state| {
                state.refreshing_sessions = false;
                let current = state.connected()
                    && state.session_query.revision == query_revision
                    && state.upload_selection_revision == selection
                    && state.title_write_revision == title;
                if !current {
                    state.stale_sessions = true;
                }
                current
            });
            if current {
                match result {
                    Ok(page) => {
                        app.accept_session_query_page(page);
                        app.reconcile_hidden_sessions();
                        app.refresh_session_facets();
                        app.update_uploader_views();
                    }
                    Err(error)
                        if error.kind() == Some(ErrorKind::Busy)
                            && app.with_state(|state| state.session_query.cursor.is_some()) =>
                    {
                        app.session_query_changed();
                    }
                    Err(error) => app.refresh_failed(&error),
                }
            }
            app.refresh_if_stale();
        });
    }

    fn accept_session_query_page(
        &self,
        page: agent_relay_desktop_core::contract::account::SessionQueryPage,
    ) {
        self.update_state(|state| {
            state.session_query.known_revision = if page.verification_pending {
                None
            } else {
                Some(page.revision)
            };
            state.session_query.verification_pending = page.verification_pending;
            if !page.unchanged {
                let keys: std::collections::BTreeSet<_> = page
                    .sessions
                    .iter()
                    .map(session_key)
                    .chain(
                        page.local_sessions
                            .iter()
                            .map(|s| s.upload_key().unwrap_or_else(|| s.target.key())),
                    )
                    .collect();
                state.session_selection.retain(|key| keys.contains(key));
                state.sessions = page.sessions;
                state.session_query.archived_keys =
                    page.archived_keys.map(|keys| keys.into_iter().collect());
                state.session_query.local_sessions = Some(page.local_sessions);
                state.session_query.next_cursor = page.next_cursor;
            }
            state.sessions_refreshed = Some(crate::ui::now_ms());
            state.session_error = None;
        });
    }

    pub(in crate::ui) fn finish_upload(&self, key: &str) {
        self.update_state(|state| {
            state.busy_uploads.remove(key);
            state.requesting.remove(key);
            state.stopping.remove(key);
        });
        self.refresh_presentation_query();
        self.render_sessions();
        self.refresh_uploader(true);
    }

    pub fn set_session_search(&self, value: &str) {
        let changed = self.update_state(|state| {
            let changed = state.session_search != value;
            state.session_search = value.to_string();
            changed
        });
        if changed {
            self.session_query_changed();
            if let Some(shell) = self.shell() {
                if let Some((search, Page::Sessions)) = shell.topbar_search.borrow().as_ref() {
                    if search.text().as_str() != value {
                        shell.pages.sessions.syncing_search.set(true);
                        search.set_text(value);
                        shell.pages.sessions.syncing_search.set(false);
                    }
                }
                // Commit query, chips and checks before doing any session-table work.
                self.render_session_controls(&shell.pages.sessions);
                shell.pages.sessions.schedule_search_results(self);
            }
        }
    }
}

use super::*;

impl App {
    pub(super) fn render_session_summary(&self, page: &SessionsPage, count: usize) {
        let (loaded, loading, query, error, more) = self.with_state(|state| {
            (
                page.search_data
                    .borrow_mut()
                    .get(
                        &state.sessions,
                        state
                            .session_query
                            .local_sessions
                            .as_deref()
                            .unwrap_or(&state.local_sessions),
                    )
                    .rows
                    .len(),
                state.refreshing_sessions,
                state.session_search.clone(),
                state.session_error.clone(),
                state.session_query.next_cursor.is_some(),
            )
        });
        page.count
            .set_text(&self.with_state(|state| state.local_session_count_text()));
        clear(&page.empty);
        if count == 0 {
            if loading {
                let spinner = gtk::Spinner::new();
                spinner.start();
                page.empty.append(&spinner);
            }
            page.empty.append(
                &text(
                    if more {
                        "No matches on this page."
                    } else if loaded == 0 {
                        "No sessions found yet"
                    } else {
                        "No sessions match these filters."
                    },
                    &["secondary"],
                )
                .centered(),
            );
            if more {
                page.empty.append(
                    &text("Continue to search older local history.", &["caption"]).centered(),
                );
            } else if loaded == 0 {
                page.empty.append(
                    &text(
                        "Local sessions appear here as they are discovered.",
                        &["caption"],
                    )
                    .centered(),
                );
            }
            if !query.is_empty() {
                page.empty.append(&button(
                    "Clear search and filters",
                    &[],
                    self.on(|app| app.set_session_search("")),
                ));
            }
        }
        page.empty.set_visible(count == 0);
        page.error.set_text(error.as_deref().unwrap_or_default());
        page.error.set_tooltip_text(error.as_deref());
        page.error.set_visible(error.is_some());
    }
}

use super::*;

impl App {
    /// The sessions the filters, source and search leave, in list order.
    pub fn filtered_sessions(&self) -> Vec<UploadSession> {
        let hidden_sessions = self.hidden_sessions();
        let show_hidden = self.show_hidden_sessions();
        self.with_state(|state| {
            let query = LocalSessionQuery::new(&state.session_search);
            merge_local_sessions(
                &state.sessions,
                state
                    .session_query
                    .local_sessions
                    .as_deref()
                    .unwrap_or(&state.local_sessions),
            )
            .into_iter()
            .map(|row| super::titles::with_optimistic_title(state, row))
            .filter(|row| {
                (show_hidden || !hidden_sessions.contains(&row.key()))
                    && query.matches(
                        row,
                        row.upload
                            .as_ref()
                            .map(|s| SessionUploadState::of(s, state.status.as_ref())),
                    )
            })
            .filter_map(|row| row.upload)
            .collect()
        })
    }
}

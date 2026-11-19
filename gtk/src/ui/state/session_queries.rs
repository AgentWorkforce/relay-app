use agent_relay_node::relay::contract::agents::LocalSession;

#[derive(Default)]
pub struct SessionQueryState {
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub revision: u64,
    pub presentation_revision: Option<u64>,
    pub known_revision: Option<u64>,
    pub verification_pending: bool,
    pub facets: Option<std::rc::Rc<agent_relay_desktop_core::contract::account::SessionFacetsPage>>,
    pub facets_loading: bool,
    pub hidden_confirmed: std::collections::BTreeSet<String>,
    pub upload_policy_revision: Option<i64>,
    pub hidden_retry_scheduled: bool,
    pub hidden_retry_attempt: u32,
    /// Retired hides whose exclusion failed: a reconcile skips them until the
    /// backoff timer fires, the connection is replaced or the account signs
    /// in.
    pub hidden_retry_waiting: std::collections::BTreeSet<String>,
    pub archived_keys: Option<std::collections::BTreeSet<String>>,
    pub local_sessions: Option<Vec<LocalSession>>,
}

impl super::super::State {
    /// A retired hide kept while signed out has no retry timer; signing in
    /// lets the reads that follow send it again.
    pub(crate) fn release_hidden_waits_on_sign_in(
        &mut self,
        before: agent_relay_node::relay::state::SignInState,
        after: agent_relay_node::relay::state::SignInState,
    ) {
        use agent_relay_node::relay::state::SignInState::SignedIn;
        if after == SignedIn && before != SignedIn {
            self.session_query.hidden_retry_waiting.clear();
        }
    }

    pub(super) fn interrupt_session_queries(&mut self) {
        self.hiding_sessions.clear();
        self.session_query.facets_loading = false;
        self.session_query.hidden_confirmed.clear();
        self.session_query.upload_policy_revision = None;
        self.session_query.hidden_retry_scheduled = false;
        self.session_query.hidden_retry_attempt = 0;
        self.session_query.hidden_retry_waiting.clear();
        self.session_query.presentation_revision = None;
        self.session_query.known_revision = None;
        self.session_query.cursor = None;
        self.session_query.next_cursor = None;
        self.session_query.verification_pending = false;
        self.session_query.revision = self.session_query.revision.wrapping_add(1);
    }
}

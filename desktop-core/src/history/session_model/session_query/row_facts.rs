//! One session row as a session search sees it.
use super::super::{session_is_active, LocalSessionRow, SessionUploadState};
use super::{AgentFilter, SessionSearchTerm};

/// What a query's terms are matched against for one session row.
pub(super) struct RowFacts<'a> {
    row: &'a LocalSessionRow,
    state: SessionUploadState,
    directory: &'a str,
    agent_names: Vec<&'a str>,
    text: String,
    active: bool,
}
impl<'a> RowFacts<'a> {
    pub(super) fn of(
        row: &'a LocalSessionRow,
        state: Option<SessionUploadState>,
        now_ms: i64,
    ) -> Self {
        // Discovery can lead the catalog; absence alone is not an upload failure.
        let state = state
            .or_else(|| row.upload.as_ref().map(|s| SessionUploadState::of(s, None)))
            .unwrap_or(SessionUploadState::Discovering);
        let directory = row
            .upload
            .as_ref()
            .and_then(|s| s.cwd.as_deref())
            .or_else(|| row.relay.as_ref().and_then(|s| s.directory.as_deref()))
            .unwrap_or_default();
        // An alias, the name it points at and the address all find the session.
        let agent = row.agent_names();
        let text = format!(
            "{} {} {} {} {} {}",
            row.title(),
            row.source(),
            row.project(),
            directory,
            row.key(),
            agent
        )
        .to_lowercase();
        let active = row
            .relay
            .as_ref()
            .is_some_and(|s| session_is_active(s, now_ms));
        RowFacts {
            row,
            state,
            directory,
            agent_names: row.agent_name_list(),
            text,
            active,
        }
    }
    /// Whether one term, ignoring its exclusion, holds for the row. `filter`
    /// is the query's agent filter.
    pub(super) fn matches(&self, term: &SessionSearchTerm, filter: Option<&AgentFilter>) -> bool {
        let row = self.row;
        let value = term.value.to_lowercase();
        match term.field.as_deref() {
            Some("title") => row.title().to_lowercase().contains(&value),
            Some("project") => row.project().to_lowercase().contains(&value),
            Some("harness" | "source") => row.source().to_lowercase() == value,
            Some("path") => self.directory.to_lowercase().contains(&value),
            Some("id") => row.key().to_lowercase().contains(&value),
            // Only the first agent filters; later ones are shown, not applied.
            Some("agent") if term.excluded => AgentFilter::of(&value).matches(&self.agent_names),
            Some("agent") => {
                filter.is_none_or(|f| f.value != value || f.matches(&self.agent_names))
            }
            Some("is") => self.is_state(&value),
            Some("upload") => match value.as_str() {
                "none" => row.upload.is_none(),
                "in-progress" => self.state.is_pending(),
                _ => format!("{:?}", self.state).to_lowercase() == value,
            },
            Some("updated" | "after" | "before") => row
                .updated()
                .zip(term.dates)
                .is_some_and(|(date, (start, end))| date >= start && date < end),
            _ => self.text.contains(&value),
        }
    }
    /// An `is:` value: active, inactive, on-relay or off-relay.
    fn is_state(&self, value: &str) -> bool {
        let on_relay = self
            .row
            .relay
            .as_ref()
            .is_some_and(|s| s.registration.is_some());
        match value {
            "active" => self.active,
            "inactive" => !self.active,
            "on-relay" => on_relay,
            "off-relay" => !on_relay,
            _ => false,
        }
    }
}

//! `sessions preview` rows: catalog metadata and the local title, before any
//! install exists.
use super::ranking::RankedSession;
use super::{catalog_title, conductor};
use serde_json::{json, Value};

/// The catalog fields a preview row carries: metadata and the local title,
/// never transcript text beyond it or the provider file's location.
pub(super) struct Preview {
    pub(super) avatar: Option<ai_hist::SessionAvatar>,
    pub(super) models: Vec<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) source: &'static str,
    pub(super) session_id: String,
    pub(super) title: Option<String>,
    pub(super) first_prompt: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) git_branch: Option<String>,
    pub(super) first_activity_ms: Option<i64>,
    pub(super) last_activity_ms: Option<i64>,
    pub(super) is_agent: bool,
    pub(super) originator: Option<String>,
}
impl From<RankedSession> for Preview {
    fn from(row: RankedSession) -> Self {
        let session = row.session;
        Self {
            avatar: row.profile.and_then(|profile| profile.avatar),
            models: row
                .metadata
                .model
                .map(|model| vec![model])
                .unwrap_or(session.models),
            reasoning_effort: row.metadata.reasoning_effort,
            source: session.source.as_str(),
            session_id: session.session_id,
            title: session.title,
            first_prompt: session.first_prompt,
            cwd: session.cwd,
            git_branch: session.git_branch,
            first_activity_ms: session.first_activity_ms,
            last_activity_ms: row.last_activity_ms,
            is_agent: row.is_agent,
            originator: session.originator,
        }
    }
}
pub(super) fn preview_response(rows: &[Preview]) -> Value {
    let sessions: Vec<_> = rows.iter().map(|row| json!({
        "source": row.source, "session_id": row.session_id,
        "avatar": row.avatar,
        "title": catalog_title(row.title.as_deref()).or_else(|| row.first_prompt.as_deref().map(conductor::without_preamble))
            .filter(|title| !title.is_empty()).unwrap_or(&row.session_id),
        "cwd": row.cwd, "git_branch": row.git_branch,
        "first_activity_ms": row.first_activity_ms, "last_activity_ms": row.last_activity_ms,
        "is_agent": row.is_agent, "host": conductor::host(row.cwd.as_deref()),
        "originator": row.originator,
        "included": false, "status": "unknown",
        "models": row.models, "reasoning_effort": row.reasoning_effort
    })).collect();
    json!({"bridge_version":1, "sessions":sessions})
}

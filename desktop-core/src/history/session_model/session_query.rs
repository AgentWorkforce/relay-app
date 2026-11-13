//! GitHub-like local session search; filter menus edit this same query.
use super::{LocalSessionRow, SessionUploadState};
use chrono::NaiveDate;

mod agent_terms;
mod query_errors;
mod row_facts;
pub use agent_terms::*;
use row_facts::RowFacts;

#[derive(Clone, Debug)]
pub struct SessionSearchTerm {
    pub raw: String,
    pub field: Option<String>,
    pub value: String,
    pub excluded: bool,
    dates: Option<(i64, i64)>,
}
impl SessionSearchTerm {
    pub fn display_title(&self) -> String {
        if self.field.as_deref() == Some("agent") && !self.excluded {
            if AgentFilter::of(&self.value).too_short() {
                return format!("{} (too short)", self.raw);
            }
            return self.raw.clone();
        }
        if self.field.as_deref() == Some("is")
            && ["on-relay", "off-relay"].contains(&self.value.to_lowercase().as_str())
        {
            if self.value.eq_ignore_ascii_case("on-relay") != self.excluded {
                "Receives messages"
            } else {
                "Does not receive messages"
            }
            .to_string()
        } else {
            self.raw.clone()
        }
    }
}
pub struct LocalSessionQuery {
    pub terms: Vec<SessionSearchTerm>,
    /// When `is:active` judges a Devin Cloud session's activity: when the
    /// query was made, or [`Self::at`].
    now_ms: i64,
}
impl LocalSessionQuery {
    pub fn new(query: &str) -> Self {
        let words = query_words(query);
        let mut terms = Vec::new();
        let mut index = 0;
        while index < words.len() {
            let mut raw = words[index].clone();
            if raw.ends_with(':')
                && index + 1 < words.len()
                && (!words[index + 1].contains(':') || words[index + 1].starts_with('"'))
            {
                index += 1;
                raw.push_str(&words[index]);
            }
            terms.push(SessionSearchTerm::parse(raw));
            index += 1;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        Self { terms, now_ms }
    }
    /// This query judged at `now_ms`.
    pub fn at(self, now_ms: i64) -> Self {
        Self { now_ms, ..self }
    }
    pub fn value(&self, field: &str) -> Option<&str> {
        self.terms
            .iter()
            .rev()
            .find(|t| t.field.as_deref() == Some(field) && !t.excluded)
            .map(|t| t.value.as_str())
    }
    pub fn removing_term(&self, index: usize) -> String {
        self.terms
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, term)| term.raw.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn setting(&self, field: &str, value: &str) -> String {
        let mut parts: Vec<_> = self
            .terms
            .iter()
            .filter(|t| t.field.as_deref() != Some(field))
            .map(|t| t.raw.clone())
            .collect();
        let agent = value.strip_prefix('@').unwrap_or(value).to_lowercase();
        if field == "agent" && agent_token(&format!("@{agent}")).is_some() {
            // Agents are written the way people type them: `@name`.
            parts.push(format!("@{agent}"));
        } else if !value.is_empty() {
            let escaped = value.replace('"', "\\\"");
            parts.push(format!(
                "{field}:{}",
                if value.chars().any(char::is_whitespace) {
                    format!("\"{escaped}\"")
                } else {
                    escaped
                }
            ));
        }
        parts.join(" ")
    }
    pub fn contains_selection(&self, field: &str, value: &str) -> bool {
        let canonical = |f: &str| {
            if f == "source" {
                "harness".to_string()
            } else {
                f.to_string()
            }
        };
        self.terms.iter().any(|term| {
            !term.excluded
                && canonical(term.field.as_deref().unwrap_or_default()) == canonical(field)
                && term.value.eq_ignore_ascii_case(value)
        })
    }
    fn without_selection_conflicts(&self, field: &str, value: &str) -> Vec<String> {
        self.terms
            .iter()
            .filter(|term| {
                let same = term.field.as_deref() == Some(field)
                    || (["source", "harness"].contains(&field)
                        && matches!(term.field.as_deref(), Some("source" | "harness")));
                !same
                    || !(if term.excluded {
                        term.value.eq_ignore_ascii_case(value)
                    } else {
                        Self::opposite(field, &term.value, value)
                    })
            })
            .map(|term| term.raw.clone())
            .collect()
    }

    pub fn completing_suggestion(text: &str, field: &str, value: &str, title: &str) -> String {
        let query = Self::new(text);
        let matches = |fragment: &str| {
            !fragment.is_empty()
                && (value.to_lowercase().contains(&fragment.to_lowercase())
                    || title.to_lowercase().contains(&fragment.to_lowercase()))
        };
        let mut removed = 0;
        if let Some(last) = query.terms.last().filter(|term| !term.excluded) {
            if let Some(last_field) = last.field.as_deref() {
                let same = last_field == field
                    || (["harness", "source"].contains(&field)
                        && ["harness", "source"].contains(&last_field));
                if same
                    && !last.value.eq_ignore_ascii_case(value)
                    && (last.value.is_empty() || matches(&last.value))
                {
                    removed = 1;
                }
            } else {
                let mut suffix = Vec::new();
                for term in query.terms.iter().rev() {
                    if term.field.is_some() || term.excluded {
                        break;
                    }
                    suffix.insert(0, term.value.as_str());
                    if matches(&suffix.join(" ")) {
                        removed = suffix.len();
                    }
                }
            }
        }
        let base = if removed == 0 {
            text.to_string()
        } else {
            query.terms[..query.terms.len() - removed]
                .iter()
                .map(|term| term.raw.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        Self::selecting_suggestion(&base, field, value, true)
    }

    pub fn selecting_suggestion(text: &str, field: &str, value: &str, selected: bool) -> String {
        let query = Self::new(text);
        if query.contains_selection(field, value) == selected {
            let kept = query.without_selection_conflicts(field, value);
            if selected && kept.len() != query.terms.len() {
                return format!("{} ", kept.join(" "));
            }
            return text.to_string();
        }
        Self::suggesting(text, field, value)
    }

    /// Toggle a choice, completing only an unfinished qualifier prefix.
    pub fn suggesting(text: &str, field: &str, value: &str) -> String {
        let query = Self::new(text);
        if value.is_empty() {
            return query.applying_filter(field, "");
        }
        let mut parts: Vec<_> = query.terms.iter().map(|term| term.raw.clone()).collect();
        let last = query
            .terms
            .last()
            .filter(|_| !text.ends_with(char::is_whitespace));
        if last.is_some_and(|last| last.completes_to(field, value)) {
            parts.pop();
        }
        let base = Self::new(&parts.join(" "));
        if base.contains_selection(field, value) {
            parts = base
                .terms
                .iter()
                .filter(|term| {
                    let same_field = term.field.as_deref() == Some(field)
                        || (["harness", "source"].contains(&field)
                            && matches!(term.field.as_deref(), Some("harness" | "source")));
                    term.excluded || !same_field || !term.value.eq_ignore_ascii_case(value)
                })
                .map(|term| term.raw.clone())
                .collect();
        } else {
            parts = base.without_selection_conflicts(field, value);
            parts.push(Self::new("").setting(field, value));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("{} ", parts.join(" "))
    }

    pub fn applying_filter(&self, field: &str, value: &str) -> String {
        let aliases: &[&str] = match field {
            "harness" => &["source"],
            "updated" => &["after", "before"],
            _ => &[],
        };
        let mut query = self
            .terms
            .iter()
            .map(|term| term.raw.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for alias in aliases {
            query = Self::new(&query).setting(alias, "");
        }
        Self::new(&query).setting(field, value)
    }
    pub fn matches(&self, row: &LocalSessionRow, state: Option<SessionUploadState>) -> bool {
        if self.opposite_selection_error().is_some() {
            return false;
        }
        let facts = RowFacts::of(row, state, self.now_ms);
        let filter = self.agent_filter();
        let mut alternatives = std::collections::HashMap::new();
        for term in &self.terms {
            let matched = facts.matches(term, filter.as_ref());
            if term.excluded {
                if matched {
                    return false;
                }
            } else if let Some(field @ ("harness" | "source" | "project" | "upload")) =
                term.field.as_deref()
            {
                let key = if field == "source" { "harness" } else { field };
                let entry = alternatives.entry(key).or_insert(false);
                *entry |= matched;
            } else if !matched {
                return false;
            }
        }
        alternatives.values().all(|matched| *matched)
    }
}
/// The query's words: split at whitespace outside double quotes (a `\"` does
/// not open or close a quote).
fn query_words(query: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for c in query.chars() {
        if c.is_whitespace() && !quoted {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
            if c == '"' && !escaped {
                quoted = !quoted;
            }
        }
        escaped = c == '\\' && !escaped;
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}
impl SessionSearchTerm {
    /// This unfinished, not excluded term is a prefix of the qualifier
    /// `field` or of the term choosing `value` for it, so choosing that
    /// value replaces it.
    fn completes_to(&self, field: &str, value: &str) -> bool {
        let token = self.raw.to_lowercase();
        let suggestion = if field == "agent" {
            LocalSessionQuery::new("")
                .setting(field, value)
                .to_lowercase()
        } else {
            format!("{}:{}", field.to_lowercase(), value.to_lowercase())
        };
        let qualifier_prefix =
            self.field.is_none() && !token.is_empty() && field.to_lowercase().starts_with(&token);
        let value_prefix = (self.field.as_deref() == Some(field)
            || (field == "agent" && token == "@"))
            && token != suggestion
            && suggestion.starts_with(&token);
        !self.excluded && (qualifier_prefix || value_prefix)
    }
    /// One term from its raw text (a `field:` already joined to its value).
    fn parse(raw: String) -> Self {
        if let Some(value) = agent_token(&raw) {
            return SessionSearchTerm {
                raw,
                field: Some("agent".into()),
                value,
                excluded: false,
                dates: None,
            };
        }
        let excluded = raw.starts_with('-') && raw.len() > 1;
        let body = if excluded { &raw[1..] } else { &raw };
        let (field, value) = Self::field_value(body);
        let dates = field
            .as_deref()
            .filter(|f| ["updated", "after", "before"].contains(f))
            .and_then(|f| date_bounds(&value, f));
        SessionSearchTerm {
            raw,
            field,
            value,
            excluded,
            dates,
        }
    }
    /// A term body's qualifier (lowercased, known ones only) and its value,
    /// unquoted. An agent value longer than [`AGENT_MAX_LEN`] is free text.
    fn field_value(body: &str) -> (Option<String>, String) {
        let (field, value) = match body.split_once(':') {
            Some((field, value))
                if [
                    "title", "project", "harness", "source", "path", "id", "agent", "is", "upload",
                    "updated", "after", "before",
                ]
                .contains(&field.to_lowercase().as_str()) =>
            {
                (Some(field.to_lowercase()), value)
            }
            _ => (None, body),
        };
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value)
            .replace("\\\"", "\"");
        if field.as_deref() != Some("agent") {
            return (field, value);
        }
        // `agent:@x` and `agent:x` name the same agent as `@x`.
        let value = agent_value(value.strip_prefix('@').unwrap_or(&value));
        if value.len() > AGENT_MAX_LEN {
            (None, body.to_string())
        } else {
            (field, value)
        }
    }
}
fn date_bounds(value: &str, field: &str) -> Option<(i64, i64)> {
    const DAY: i64 = 86_400_000;
    fn day(value: &str) -> Option<i64> {
        if value.len() != 10 {
            return None;
        }
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
        if date.format("%Y-%m-%d").to_string() != value {
            return None;
        }
        Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
    }
    if field == "after" {
        return Some((day(value)? + DAY, i64::MAX));
    }
    if field == "before" {
        return Some((i64::MIN, day(value)?));
    }
    if let Some((start, end)) = value.split_once("..") {
        let start = if start.is_empty() || start == "*" {
            i64::MIN
        } else {
            day(start)?
        };
        let end = if end.is_empty() || end == "*" {
            i64::MAX
        } else {
            day(end)? + DAY
        };
        return (start < end).then_some((start, end));
    }
    for op in [">=", "<=", ">", "<"] {
        if let Some(value) = value.strip_prefix(op) {
            let date = day(value)?;
            return Some(match op {
                ">=" => (date, i64::MAX),
                ">" => (date + DAY, i64::MAX),
                "<=" => (i64::MIN, date + DAY),
                _ => (i64::MIN, date),
            });
        }
    }
    let date = day(value)?;
    Some((date, date + DAY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::session_model::merge_local_sessions;
    use serde_json::{json, Value};
    fn row(at: &str, live: bool) -> LocalSessionRow {
        let ms = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .timestamp_millis();
        let upload = serde_json::from_value(json!({"source":"codex", "session_id":"one", "cwd":"/work/cloud", "title":"Fix login", "included":false, "is_agent":false, "status":"not_shared", "last_activity_ms":ms})).unwrap();
        let local = serde_json::from_value(json!({"target":{"kind":"codex", "id":"one"}, "title":"Local work", "live":live, "suggested_name":"session"})).unwrap();
        merge_local_sessions(&[upload], &[local]).remove(0)
    }
    #[test]
    fn the_session_list_shows_and_finds_the_agent_by_either_name() {
        let session = |id: &str,
                       registration: Value|
         -> agent_relay_node::relay::contract::agents::LocalSession {
            serde_json::from_value(
                json!({"target":{"kind":"codex", "id":id}, "title":id, "live":true,
                "suggested_name":"session", "registration":registration}),
            )
            .unwrap()
        };
        let rows = merge_local_sessions(
            &[],
            &[
                session("one", Value::Null),
                session(
                    "two",
                    json!({"name":"codex-api", "alias":"reviewer",
                    "address":"codex-api@desk", "added_by_session":false}),
                ),
            ],
        );
        let off = rows.iter().find(|r| r.title() == "one").unwrap();
        let renamed = rows.iter().find(|r| r.title() == "two").unwrap();
        assert_eq!(off.agent_name(), None);
        assert_eq!(renamed.agent_name(), Some("reviewer"));
        assert_eq!(
            renamed.agent_detail().unwrap(),
            "Renamed from @codex-api. Only @reviewer is shown and replies come from it; messages still sent to @codex-api arrive here."
        );
        // Words find any part of a name; `@name` and `agent:` find a whole
        // name, alias or address, ignoring case.
        let found = |query: &str| -> Vec<String> {
            let parsed = LocalSessionQuery::new(query);
            rows.iter()
                .filter(|row| parsed.matches(row, None))
                .map(|row| row.title())
                .collect()
        };
        for query in ["@codex", "agent:codex", "@co*", "@desk", "@reviewer@mac"] {
            assert!(found(query).is_empty(), "{query}");
        }
        for query in [
            "agent:reviewer",
            "agent:codex-api",
            "reviewer",
            "revi",
            "@reviewer",
            "@REVIEWER",
            "@codex-api",
            "@CODEX-API",
            "@codex*",
            "@codex-api@d*",
            "agent:@codex-api",
            "codex-api@desk",
            "@codex-api@desk",
            "@Codex-API@Desk",
        ] {
            let parsed = LocalSessionQuery::new(query);
            let found: Vec<String> = rows
                .iter()
                .filter(|row| parsed.matches(row, None))
                .map(|row| row.title())
                .collect();
            assert_eq!(found, ["two"], "{query}");
        }
    }
    /// The registered row the shared `agent_matches` vectors run against.
    fn agent_row() -> LocalSessionRow {
        let local = serde_json::from_value(json!({"target":{"kind":"codex", "id":"a1"},
            "title":"Agent work", "live":true, "suggested_name":"session",
            "registration":{"name":"example-agent", "alias":"example-reviewer",
                "address":"example-agent@desk", "added_by_session":false}}))
        .unwrap();
        merge_local_sessions(&[], &[local]).remove(0)
    }
    #[test]
    fn shared_search_contract() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/shared/session-search-cases.json"
        ))
        .unwrap();
        for case in cases {
            let text = case["query"].as_str().unwrap();
            let query = LocalSessionQuery::new(text);
            if let Some(agents) = case["agents"].as_array() {
                let agents: Vec<&str> = agents.iter().map(|a| a.as_str().unwrap()).collect();
                assert_eq!(query.agents(), agents, "{text}");
                assert_eq!(
                    query.agent_filter().map(|f| f.value),
                    agents.first().map(|a| a.to_string()),
                    "{text}"
                );
                assert_eq!(
                    query.extra_agents(),
                    agents.iter().skip(1).copied().collect::<Vec<_>>(),
                    "{text}"
                );
            }
            if let Some(free) = case["free_text"].as_str() {
                assert_eq!(query.free_text(), free, "{text}");
            }
            assert_eq!(query.agent_note(), case["agent_note"].as_str(), "{text}");
            assert_eq!(
                query.agent_filter().is_some_and(|f| f.too_short()),
                case["too_short"].as_bool().unwrap_or(false),
                "{text}"
            );
            if let Some(expected) = case["agent_matches"].as_bool() {
                assert_eq!(query.matches(&agent_row(), None), expected, "{text}");
            }
            if let Some(expected) = case["matches"].as_bool() {
                assert_eq!(
                    query.matches(&row("2026-10-11T23:59:59Z", true), None),
                    expected,
                    "{text}"
                );
            }
            assert_eq!(
                query.error().is_some(),
                case["invalid"].as_bool().unwrap_or(false),
                "{text}"
            );
            if let Some(refusal) = case.get("refusal") {
                let answer = query.agent_refusal(
                    refusal["status"].as_u64().unwrap() as u16,
                    refusal["code"].as_str().unwrap(),
                    refusal["reason"].as_str(),
                    refusal["message"].as_str().unwrap(),
                );
                assert_eq!(
                    answer.as_ref().map(|a| a.notice.as_str()),
                    case["refusal_notice"].as_str(),
                    "{text}"
                );
                assert_eq!(
                    answer.and_then(|a| a.free_text),
                    case["refusal_free_text"].as_str().map(String::from),
                    "{text}"
                );
            }
        }
    }
    #[test]
    fn shared_filter_menu_actions() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/shared/session-filter-actions.json"
        ))
        .unwrap();
        for case in cases {
            let query = LocalSessionQuery::new(case["query"].as_str().unwrap());
            assert_eq!(
                query.applying_filter(
                    case["field"].as_str().unwrap(),
                    case["value"].as_str().unwrap()
                ),
                case["expected"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn agent_tokens_follow_the_shared_grammar() {
        for word in [
            "@codex",
            "@Codex-API",
            "@a.b_c-d",
            "@x@y",
            "@layered-*",
            "@a@b*",
            "@a**",
        ] {
            assert!(agent_token(word).is_some(), "{word}");
        }
        for word in [
            "someone@example.com",
            "@",
            "@*",
            "@foo!",
            "@a@b@c",
            "@a*b",
            "\"@a\"",
            "@a@",
            "@@a",
            "-@a",
            "@**",
            "@é",
        ] {
            assert_eq!(agent_token(word), None, "{word}");
        }
        assert_eq!(
            agent_token("@Layered-Control@No*").unwrap(),
            "layered-control@no*"
        );
        // The whole trailing run of `*` and whitespace is one prefix star.
        assert_eq!(agent_token("@a**").as_deref(), Some("a*"));
        assert_eq!(agent_token("@Abc*\t ").as_deref(), Some("abc*"));
        assert_eq!(agent_value("abc* "), "abc*");
        assert_eq!(agent_value("abc* *"), "abc*");
        assert_eq!(agent_value("abc "), "abc");
        assert_eq!(AgentFilter::of("abc** "), AgentFilter::of("abc*"));
        assert!(!AgentFilter::of("abc ").prefix);
        let query = LocalSessionQuery::new("@B fix @a  @b \"@c\" agent:@A");
        assert_eq!(query.agents(), ["b", "a"]);
        assert_eq!(query.extra_agents(), ["a"]);
        assert_eq!(query.agent_note(), Some("One agent at a time"));
        assert_eq!(query.free_text(), "fix \"@c\"");
        assert!(!query.agent_only());
        assert!(LocalSessionQuery::new("@codex @b").agent_only());
        // Further agents stay in the query text.
        assert_eq!(query.removing_term(0), "fix @a @b \"@c\" agent:@A");
    }

    #[test]
    fn agent_values_are_capped_at_200_characters() {
        let longest = "a".repeat(AGENT_MAX_LEN);
        let over = "a".repeat(AGENT_MAX_LEN + 1);
        assert_eq!(agent_token(&format!("@{longest}")), Some(longest.clone()));
        assert_eq!(
            agent_token(&format!("@{}*", "a".repeat(AGENT_MAX_LEN - 1))).map(|v| v.len()),
            Some(AGENT_MAX_LEN)
        );
        assert_eq!(agent_token(&format!("@{longest}*")), None);
        assert_eq!(agent_token(&format!("@{over}")), None);
        let query = LocalSessionQuery::new(&format!("@{over}"));
        assert!(query.agent_filter().is_none());
        assert_eq!(query.free_text(), format!("@{over}"));
        let query = LocalSessionQuery::new(&format!("agent:{over}"));
        assert!(query.agent_filter().is_none());
        assert_eq!(query.terms[0].field, None);
    }

    #[test]
    fn read_api_refusals_show_a_notice_and_never_fall_back_for_an_agent_only_query() {
        let only = LocalSessionQuery::new("@example*");
        let with_text = LocalSessionQuery::new("@example* fix");
        for reason in [
            "param_agentMatch",
            "agent_match_not_migrated",
            "agent_prefix_not_supported",
        ] {
            let refusal = only
                .agent_refusal(501, "unsupported_by_backend", Some(reason), "")
                .unwrap();
            assert_eq!(refusal.notice, AGENT_PREFIX_UNAVAILABLE, "{reason}");
            assert_eq!(refusal.free_text, None, "{reason}");
            let refusal = with_text
                .agent_refusal(501, "unsupported_by_backend", Some(reason), "")
                .unwrap();
            assert_eq!(refusal.free_text.as_deref(), Some("fix"), "{reason}");
        }
        for reason in [
            "agent_with_text",
            "agent_filter_not_ready",
            "agent_filter_not_on_search",
        ] {
            let refusal = only
                .agent_refusal(501, "unsupported_by_backend", Some(reason), "")
                .unwrap();
            assert_eq!(refusal.notice, AGENT_FILTER_UNAVAILABLE, "{reason}");
            assert_eq!(refusal.free_text, None, "{reason}");
        }
        let invalid = only
            .agent_refusal(400, "invalid_request", None, "agent is too long")
            .unwrap();
        assert_eq!(
            invalid,
            AgentRefusal {
                notice: "agent is too long".into(),
                free_text: None
            }
        );
        assert_eq!(
            only.agent_refusal(400, "agent_prefix_too_short", None, "")
                .unwrap()
                .notice,
            AGENT_PREFIX_TOO_SHORT
        );
        assert_eq!(
            only.agent_refusal(400, "agent_prefix_too_broad", None, "")
                .unwrap()
                .notice,
            AGENT_PREFIX_TOO_BROAD
        );
        assert_eq!(only.agent_refusal(500, "internal", None, "boom"), None);
        assert_eq!(
            LocalSessionQuery::new("fix").agent_refusal(
                501,
                "unsupported_by_backend",
                Some("agent_with_text"),
                ""
            ),
            None
        );
    }
    #[test]
    fn agent_filters_are_exact_or_a_long_enough_prefix() {
        let filter = |q: &str| LocalSessionQuery::new(q).agent_filter().unwrap();
        assert_eq!(
            filter("@Codex-API@Desk"),
            AgentFilter {
                value: "codex-api@desk".into(),
                prefix: false
            }
        );
        let prefix = filter("@layered-*");
        assert!(prefix.prefix && !prefix.too_short());
        assert_eq!(prefix.stem(), "layered-");
        assert!(prefix.matches(&["Layered-Control"]));
        assert!(!prefix.matches(&["control-layered"]));
        assert!(filter("@co*").too_short());
        assert!(filter("@a.-b*").too_short());
        assert!(!filter("@abc*").too_short());
        assert!(!filter("@co*").matches(&["codex"]));
        assert!(!filter("@codex").matches(&["codex-api"]));
        assert!(filter("@codex-api").matches(&["reviewer", "CODEX-API"]));
        // The `@node` suffix is part of the value.
        assert!(!filter("@codex-api@desk").matches(&["codex-api"]));
        assert!(LocalSessionQuery::new("@x")
            .agent_filter()
            .unwrap()
            .matches(&["x"]));
        assert_eq!(LocalSessionQuery::new("fix").agent_filter(), None);
    }
    #[test]
    fn agent_chips_carry_the_agent_field() {
        let query = LocalSessionQuery::new("fix @Reviewer agent:@codex-api @co*");
        let chips: Vec<_> = query
            .terms
            .iter()
            .filter(|t| t.field.is_some())
            .map(|t| {
                (
                    t.field.as_deref().unwrap(),
                    t.value.as_str(),
                    t.display_title(),
                )
            })
            .collect();
        assert_eq!(
            chips,
            [
                ("agent", "reviewer", "@Reviewer".to_string()),
                ("agent", "codex-api", "agent:@codex-api".to_string()),
                ("agent", "co*", "@co* (too short)".to_string()),
            ]
        );
        assert_eq!(query.removing_term(1), "fix agent:@codex-api @co*");
        assert_eq!(query.error(), None);
        assert!(LocalSessionQuery::new("agent:").error().is_some());
    }
    #[test]
    fn roster_agents_are_suggested_by_the_prefix_after_at() {
        let roster: Vec<String> = ["reviewer", "Codex-API", "codex-api", "lead", "bad name"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            LocalSessionQuery::agent_suggestions("fix @co", &roster),
            ["Codex-API"]
        );
        assert_eq!(
            LocalSessionQuery::agent_suggestions("@", &roster),
            ["Codex-API", "lead", "reviewer"]
        );
        assert_eq!(
            LocalSessionQuery::agent_suggestions("fix ", &roster),
            ["Codex-API", "lead", "reviewer"]
        );
        assert!(LocalSessionQuery::agent_suggestions("fix co", &roster).is_empty());
        assert!(LocalSessionQuery::agent_suggestions("@zz", &roster).is_empty());
        // Choosing one writes `@name`, completing the typed fragment.
        assert_eq!(
            LocalSessionQuery::suggesting("fix @co", "agent", "Codex-API"),
            "fix @codex-api "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("fix @", "agent", "lead"),
            "fix @lead "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("fix ", "agent", "lead"),
            "fix @lead "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("fix @lead ", "agent", "lead"),
            "fix "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion("fix @rev", "agent", "reviewer", "@reviewer"),
            "fix @reviewer "
        );
        assert_eq!(
            LocalSessionQuery::new("fix").applying_filter("agent", "@lead"),
            "fix @lead"
        );
    }
    #[test]
    fn midnight_and_live_upload_state() {
        assert!(row("2026-10-11T00:00:00Z", true).matches("updated:2026-10-11"));
        assert!(!row("2026-10-12T00:00:00Z", true).matches("updated:2026-10-11"));
        assert!(
            LocalSessionQuery::new("is:active upload:paused project:cloud").matches(
                &row("2026-10-11T00:00:00Z", true),
                Some(SessionUploadState::Paused)
            )
        );
        assert!(row("2026-10-11T00:00:00Z", false).matches("is:inactive"));
    }
    #[test]
    fn menu_edits_preserve_other_qualifiers() {
        let query = LocalSessionQuery::new(
            "title: \"Fix login\" -harness:claude harness:codex updated:>=2026-10-01",
        );
        let changed = query.setting("harness", "cursor");
        assert_eq!(
            changed,
            "title:\"Fix login\" updated:>=2026-10-01 harness:cursor"
        );
        assert_eq!(
            LocalSessionQuery::new(&changed).setting("harness", ""),
            "title:\"Fix login\" updated:>=2026-10-01"
        );
        assert_eq!(
            LocalSessionQuery::new("title: \"Fix: login\"").value("title"),
            Some("Fix: login")
        );
        assert_eq!(
            LocalSessionQuery::new("title:\"Fix \\\"login\\\"\"").value("title"),
            Some("Fix \"login\"")
        );
    }
}

#[cfg(test)]
mod suggestion_tests {
    use super::LocalSessionQuery;
    #[test]
    fn completes_tokens_preserving_prior_filters_and_quoted_values() {
        assert_eq!(
            LocalSessionQuery::suggesting("fix is:active har", "harness", "codex"),
            "fix is:active harness:codex "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("title:\"Fix login\" ", "project", "my project"),
            "title:\"Fix login\" project:\"my project\" "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("source:claude ", "harness", "codex"),
            "source:claude harness:codex "
        );
    }
    #[test]
    fn suggestions_toggle_individual_values() {
        assert_eq!(
            LocalSessionQuery::suggesting("harness:codex", "harness", "claude"),
            "harness:codex harness:claude "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("harness:codex ", "harness", "codex"),
            ""
        );
        assert_eq!(
            LocalSessionQuery::suggesting("fix login", "is", "active"),
            "fix login is:active "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("harness:cl", "harness", "claude"),
            "harness:claude "
        );
    }
    #[test]
    fn deselection_preserves_other_values_exclusions_and_quotes() {
        assert_eq!(
            LocalSessionQuery::suggesting(
                "fix harness:codex harness:claude is:active",
                "harness",
                "codex"
            ),
            "fix harness:claude is:active "
        );
        assert_eq!(
            LocalSessionQuery::suggesting(
                "source:codex harness:codex -harness:codex harness:claude",
                "harness",
                "codex"
            ),
            "-harness:codex harness:claude "
        );
        assert_eq!(
            LocalSessionQuery::suggesting(
                "project:\"my project\" project:cloud title:\"Fix login\"",
                "project",
                "my project"
            ),
            "project:cloud title:\"Fix login\" "
        );
    }
    #[test]
    fn chip_removal_preserves_text_other_values_and_exclusions() {
        let query = LocalSessionQuery::new(
            "fix harness:codex harness:claude -is:inactive project:\"my project\"",
        );
        assert_eq!(
            query.removing_term(1),
            "fix harness:claude -is:inactive project:\"my project\""
        );
        assert_eq!(
            query.removing_term(3),
            "fix harness:codex harness:claude project:\"my project\""
        );
        assert_eq!(LocalSessionQuery::new("is:active").removing_term(0), "");
    }
    #[test]
    fn right_adds_and_left_removes_without_toggling_on_repeat() {
        let added = LocalSessionQuery::selecting_suggestion("is:active", "harness", "codex", true);
        assert_eq!(added, "is:active harness:codex ");
        assert_eq!(
            LocalSessionQuery::selecting_suggestion(&added, "harness", "codex", true),
            added
        );
        let removed = LocalSessionQuery::selecting_suggestion(&added, "harness", "codex", false);
        assert_eq!(removed, "is:active ");
        assert_eq!(
            LocalSessionQuery::selecting_suggestion(&removed, "harness", "codex", false),
            removed
        );
    }
    #[test]
    fn opposite_filters_replace_while_compatible_filters_remain() {
        assert_eq!(
            LocalSessionQuery::suggesting("harness:codex is:active is:on-relay", "is", "inactive"),
            "harness:codex is:on-relay is:inactive "
        );
        assert_eq!(
            LocalSessionQuery::selecting_suggestion("is:inactive", "is", "active", true),
            "is:active "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("upload:local project:cloud", "upload", "in-progress"),
            "project:cloud upload:in-progress "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("upload:uploaded", "upload", "in-progress"),
            "upload:uploaded upload:in-progress "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("upload:none", "upload", "uploaded"),
            "upload:uploaded "
        );
        assert_eq!(
            LocalSessionQuery::suggesting("is:inactive -is:active project:cloud", "is", "active"),
            "project:cloud is:active "
        );
        assert_eq!(
            LocalSessionQuery::selecting_suggestion("is:active is:inactive", "is", "active", true),
            "is:active "
        );
        assert!(LocalSessionQuery::new("is:active is:inactive")
            .error()
            .is_some());
    }
    #[test]
    fn enter_commits_the_completion_instead_of_the_typed_fragment() {
        assert_eq!(
            LocalSessionQuery::completing_suggestion("cloud ", "project", "cloud", "cloud"),
            "project:cloud "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion("cloud", "project", "cloud", "cloud"),
            "project:cloud "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion(
                "fix harness:codex clo",
                "project",
                "cloud",
                "cloud"
            ),
            "fix harness:codex project:cloud "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion(
                "fix In pro",
                "upload",
                "in-progress",
                "In progress"
            ),
            "fix upload:in-progress "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion(
                "project:\"clo\"",
                "project",
                "cloud",
                "cloud"
            ),
            "project:cloud "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion("project:cloud ", "project", "cloud", "cloud"),
            "project:cloud "
        );
        assert_eq!(
            LocalSessionQuery::completing_suggestion("title:cloud", "project", "cloud", "cloud"),
            "title:cloud project:cloud "
        );
    }
}

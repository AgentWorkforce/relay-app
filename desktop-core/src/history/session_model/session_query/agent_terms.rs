//! The `@name` / `@prefix*` agent terms of a session search: the token
//! grammar, the agent filter, the read API's refusals and roster suggestions.
use super::super::LocalSessionRow;
use super::LocalSessionQuery;

/// The note shown beside the chips when a query names more than one agent.
pub const ONE_AGENT_AT_A_TIME: &str = "One agent at a time";

/// The longest agent value (without the `@`, star included) the read API
/// accepts; it answers a longer one with 400 `invalid_request`. A longer
/// `@word` is free text.
pub const AGENT_MAX_LEN: usize = 200;

/// Shown when the read API refuses the agent filter (501).
pub const AGENT_FILTER_UNAVAILABLE: &str = "Agent filter unavailable";
/// Shown when the read API cannot match an agent prefix yet (501).
pub const AGENT_PREFIX_UNAVAILABLE: &str = "Prefix search is not available yet";
/// Shown for 400 `agent_prefix_too_short`.
pub const AGENT_PREFIX_TOO_SHORT: &str =
    "Agent prefix too short: type at least 3 letters or digits before *";
/// Shown for 400 `agent_prefix_too_broad`.
pub const AGENT_PREFIX_TOO_BROAD: &str =
    "Agent prefix matches too many agents: type more of the name";

/// What to show when the read API refuses a query's agent filter. See
/// [`LocalSessionQuery::agent_refusal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRefusal {
    /// The notice for the user.
    pub notice: String,
    /// The free text to run instead, without the agent filter; `None` means
    /// no results at all (an agent-only query never falls back to text,
    /// local or buffered rows).
    pub free_text: Option<String>,
}

/// The agent value of an `@name` word, lowercased, or `None` for free text.
///
/// The grammar is relay-control's (`src/sessions/agent-query.ts`), shared by
/// the dashboard, Desktop and the MCP tool: a whitespace-separated word is an
/// agent token iff it matches `^@([a-z0-9._-]+(?:@[a-z0-9._-]+)?\*?)$`,
/// case-insensitively. `someone@example.com`, `@`, `@*`, `@foo!`, `@a@b@c`,
/// `@a*b` and `"@a"` are free text. The whole trailing run of `*` and
/// whitespace is first reduced to one star (or none, without a star; see
/// [`agent_value`]), so `@a**` is `@a*`. The star stays on the value. A value
/// longer than [`AGENT_MAX_LEN`] is free text too.
/// Agent-search contract: sessions-pipeline revision `317fa2f7` (pipeline PR #420).
pub fn agent_token(word: &str) -> Option<String> {
    let rest = agent_value(word.strip_prefix('@')?);
    if rest.len() > AGENT_MAX_LEN {
        return None;
    }
    let body = rest.strip_suffix('*').unwrap_or(&rest);
    let part = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    let valid = match body.split_once('@') {
        Some((name, node)) => part(name) && part(node),
        None => part(body),
    };
    valid.then_some(rest)
}

/// An agent value, lowercased, with its whole trailing run of `*` and
/// whitespace replaced by one `*` when the run holds a star and removed
/// otherwise: `a**` and `abc* ` are prefixes `a*` and `abc*`, `abc ` is exact.
/// This decides exact or prefix before anything else looks at the value
/// (contract revision `317fa2f7`, delta 1).
pub fn agent_value(value: &str) -> String {
    let stem = value.trim_end_matches(|c: char| c == '*' || c.is_whitespace());
    let star = if value[stem.len()..].contains('*') {
        "*"
    } else {
        ""
    };
    format!("{stem}{star}").to_lowercase()
}

/// The agent a session search is narrowed to: the first `@name` (or
/// `agent:name`) in the query.
///
/// Read-API v1 mapping (relay-control's sessions read API; Desktop's search is
/// local and sends no request today):
/// - Route: `GET /cloud/api/v1/workspaces/{ws}/sessions?agent=<value>`. One
///   value only: the first agent token, lowercased, with no leading `@` and
///   the `@node` suffix kept, at most [`AGENT_MAX_LEN`] characters (a longer
///   one is answered with 400 `invalid_request`, shown as the server's
///   message). Matching is case-insensitive and exact against each of the
///   session's agent names.
/// - v1 of Desktop sends exact values only. A trailing `*` asks for a prefix
///   match; it is sent only after the server has answered with
///   `agentMatch = "prefix"`, and until then prefix filtering is local only.
///   The `X-Sessions-Read-Agent-Match` response header is not read.
/// - The response adds `agent` (the value applied), `ignoredAgents` (the later
///   `@` tokens, see [`LocalSessionQuery::extra_agents`], shown as "One agent
///   at a time") and `agentMatch` (the match header value, or null).
/// - An agent filter and free text are never combined on the wire: the server
///   refuses that. The parsed query keeps them apart
///   ([`LocalSessionQuery::agent_filter`], [`LocalSessionQuery::extra_agents`],
///   [`LocalSessionQuery::free_text`]), while local matching applies both.
/// - Refusals: 501 `unsupported_by_backend` with `reason` `agent_with_text`,
///   `agent_filter_not_ready`, `agent_prefix_not_supported`,
///   `agent_filter_not_on_search`, `param_agentMatch` or
///   `agent_match_not_migrated` (the last three: prefix search is not
///   available yet); 400 `agent_prefix_too_short` (fewer than 3 characters,
///   see [`AgentFilter::too_short`]), `agent_prefix_too_broad` or
///   `invalid_request`. [`LocalSessionQuery::agent_refusal`] maps each to
///   what the user sees.
/// - On a 501 refusal the agent filter is unavailable: the client drops the
///   agent filter, runs the free-text part as before and shows the notice.
///   When the query was only `@name`, it shows that notice with no results
///   and never falls back to text search, local or buffered rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFilter {
    /// The lowercased value, star included when `prefix` is set.
    pub value: String,
    /// The value ends in `*`: a prefix match on the same names.
    pub prefix: bool,
}

impl AgentFilter {
    /// A prefix needs at least this many letters or digits before the `*`.
    pub const MIN_PREFIX: usize = 3;

    pub fn of(value: &str) -> Self {
        let value = agent_value(value);
        AgentFilter {
            prefix: value.ends_with('*'),
            value,
        }
    }

    /// The value without the prefix star.
    pub fn stem(&self) -> &str {
        if self.prefix {
            &self.value[..self.value.len() - 1]
        } else {
            &self.value
        }
    }

    /// A prefix with fewer than [`Self::MIN_PREFIX`] letters or digits before
    /// the star matches nothing.
    pub fn too_short(&self) -> bool {
        self.prefix
            && self
                .stem()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .count()
                < Self::MIN_PREFIX
    }

    /// Whether any of a session's agent names (name, alias, address) is this
    /// agent: equal ignoring case, or starting with the stem for a prefix.
    pub fn matches(&self, names: &[&str]) -> bool {
        if self.too_short() {
            return false;
        }
        names.iter().any(|name| {
            let name = name.to_lowercase();
            if self.prefix {
                name.starts_with(self.stem())
            } else {
                name == self.value
            }
        })
    }
}

impl LocalSessionQuery {
    /// Each agent named (not excluded), lowercased, without repeats, in the
    /// order typed.
    pub fn agents(&self) -> Vec<String> {
        let mut agents: Vec<String> = Vec::new();
        for term in &self.terms {
            if term.field.as_deref() == Some("agent")
                && !term.excluded
                && !term.value.is_empty()
                && !agents.contains(&term.value)
            {
                agents.push(term.value.clone());
            }
        }
        agents
    }

    /// The agent filter: the first agent named. See [`AgentFilter`].
    pub fn agent_filter(&self) -> Option<AgentFilter> {
        self.agents().first().map(|value| AgentFilter::of(value))
    }

    /// Further agents named: the read API's `ignoredAgents`. They stay in the
    /// query and as chips but are not applied (v1 filters one agent at a time).
    pub fn extra_agents(&self) -> Vec<String> {
        self.agents().into_iter().skip(1).collect()
    }

    /// "One agent at a time" when the query names more than one agent.
    pub fn agent_note(&self) -> Option<&'static str> {
        (self.agents().len() > 1).then_some(ONE_AGENT_AT_A_TIME)
    }

    /// The query without its agent terms, words joined by single spaces. On the
    /// wire this is never sent together with an agent filter (the read API
    /// refuses `agent_with_text`); locally both apply.
    pub fn free_text(&self) -> String {
        self.terms
            .iter()
            .filter(|term| term.field.as_deref() != Some("agent"))
            .map(|term| term.raw.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The query is only an agent filter: it lists that agent's sessions in
    /// the chosen order, newest first by default.
    pub fn agent_only(&self) -> bool {
        !self.terms.is_empty()
            && self
                .terms
                .iter()
                .all(|term| term.field.as_deref() == Some("agent") && !term.excluded)
    }

    /// What to show when the read API refuses this query's agent filter, from
    /// the response status, its `code`, its `reason` (if any) and its message.
    /// `None` when the query has no agent filter or the refusal is not about
    /// one. 501s show a notice and drop the agent filter: the free text runs
    /// alone, and an agent-only query shows no results. A 400 about the agent
    /// value shows its notice (`invalid_request`: the server's message as-is)
    /// with no results.
    pub fn agent_refusal(
        &self,
        status: u16,
        code: &str,
        reason: Option<&str>,
        message: &str,
    ) -> Option<AgentRefusal> {
        self.agent_filter()?;
        let reason = reason.unwrap_or(code);
        let notice = match (status, reason) {
            (
                501,
                "agent_prefix_not_supported" | "param_agentMatch" | "agent_match_not_migrated",
            ) => AGENT_PREFIX_UNAVAILABLE.to_string(),
            (501, _) => AGENT_FILTER_UNAVAILABLE.to_string(),
            (400, "agent_prefix_too_short") => AGENT_PREFIX_TOO_SHORT.to_string(),
            (400, "agent_prefix_too_broad") => AGENT_PREFIX_TOO_BROAD.to_string(),
            (400, "invalid_request") => message.to_string(),
            _ => return None,
        };
        let free_text = self.free_text();
        Some(AgentRefusal {
            free_text: (status == 501 && !free_text.is_empty()).then_some(free_text),
            notice,
        })
    }

    /// Roster agents to offer for the word being typed: every name while no
    /// word is typed, names starting with what follows a typed `@`, none for
    /// other words. Distinct ignoring case, sorted.
    pub fn agent_suggestions(text: &str, roster: &[String]) -> Vec<String> {
        let token = if text.ends_with(char::is_whitespace) {
            ""
        } else {
            text.split_whitespace().last().unwrap_or("")
        };
        let prefix = match token.strip_prefix('@') {
            Some(prefix) => prefix.to_lowercase(),
            None if token.is_empty() => String::new(),
            None => return vec![],
        };
        let mut names: Vec<String> = roster
            .iter()
            .filter(|name| agent_token(&format!("@{name}")).is_some())
            .filter(|name| name.to_lowercase().starts_with(&prefix))
            .cloned()
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        names
    }
}

impl LocalSessionRow {
    /// Every name that reaches this session (name, alias) and its address:
    /// what an `@name` search compares against, each one whole.
    pub fn agent_name_list(&self) -> Vec<&str> {
        self.relay
            .as_ref()
            .and_then(|s| s.registration.as_ref())
            .map(|r| {
                [
                    Some(r.name.as_str()),
                    r.alias.as_deref(),
                    Some(r.address.as_str()),
                ]
                .into_iter()
                .flatten()
                .filter(|name| !name.is_empty())
                .collect()
            })
            .unwrap_or_default()
    }
}

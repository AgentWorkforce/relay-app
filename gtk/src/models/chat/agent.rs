//! What the Chat page says about one agent: its name, what it is doing,
//! where it runs and how a message reaches it.

use super::*;

impl ChatAgent {
    /// Its session on this computer, when the core still lists it.
    pub fn local(&self) -> Option<(&LocalSessionRow, &LocalSession)> {
        match &self.place {
            ChatPlace::Local { row, session } => Some((row, session)),
            ChatPlace::History(_) | ChatPlace::Elsewhere(_) => None,
        }
    }

    /// Its row in this computer's session history, running or not.
    pub fn row(&self) -> Option<&LocalSessionRow> {
        match &self.place {
            ChatPlace::Local { row, .. } | ChatPlace::History(row) => Some(row),
            ChatPlace::Elsewhere(_) => None,
        }
    }

    fn registration(&self) -> Option<&SessionRegistration> {
        self.local()
            .and_then(|(_, session)| session.registration.as_ref())
    }

    /// The name the core registered it with, which renaming takes.
    pub fn registered_name(&self) -> Option<String> {
        self.registration()
            .map(|registration| registration.name.clone())
    }

    /// The uploader's key for its session, once history has it.
    pub fn upload_key(&self) -> Option<String> {
        self.row()?.upload.as_ref().map(session_key)
    }

    pub fn key(&self) -> String {
        match &self.place {
            ChatPlace::Local { session, .. } => session.target.key(),
            ChatPlace::History(row) => format!("history:{}", row.key()),
            ChatPlace::Elsewhere(agent) => format!(
                "elsewhere:{}",
                agent.address.clone().unwrap_or_else(|| agent.name.clone())
            ),
        }
    }

    /// Its relay name, else the session's title.
    pub fn name(&self) -> String {
        match (&self.place, self.registration()) {
            (ChatPlace::Elsewhere(agent), _) => agent.name.clone(),
            (_, Some(registration)) => registration
                .alias
                .clone()
                .unwrap_or_else(|| registration.name.clone()),
            (ChatPlace::Local { row, .. } | ChatPlace::History(row), None) => row.title(),
        }
    }

    pub fn title(&self) -> String {
        match &self.place {
            ChatPlace::Local { row, .. } | ChatPlace::History(row) => row.title(),
            ChatPlace::Elsewhere(agent) => agent.description.clone(),
        }
    }

    /// The harness's source id its mark is drawn for.
    pub fn icon_source(&self) -> String {
        match &self.place {
            ChatPlace::Local { row, .. } | ChatPlace::History(row) => row.icon_source(),
            ChatPlace::Elsewhere(agent) => agent.provider.clone().unwrap_or_default(),
        }
    }

    pub fn provider(&self) -> String {
        let source = self.icon_source();
        if source.is_empty() {
            CHAT_AGENT.into()
        } else {
            source_name(&source)
        }
    }

    pub fn project(&self) -> String {
        match &self.place {
            ChatPlace::Local { row, .. } | ChatPlace::History(row) => row.project(),
            ChatPlace::Elsewhere(agent) => agent
                .activity
                .as_ref()
                .and_then(|activity| activity.repo.clone())
                .unwrap_or_default(),
        }
    }

    /// The exact address it is messaged at, once it is on the relay.
    /// Every name the agent goes by on the relay, for matching a direct
    /// conversation's peer: its name, its registered name and alias, and
    /// its address without the machine.
    pub fn relay_names(&self) -> Vec<String> {
        let mut names = vec![self.name()];
        if let Some(registration) = self.registration() {
            names.push(registration.name.clone());
            names.extend(registration.alias.clone());
        }
        if let Some(address) = self.address() {
            names.extend(address.split('@').next().map(str::to_string));
            names.push(address);
        }
        names.retain(|name| !name.is_empty());
        names.dedup();
        names
    }

    pub fn address(&self) -> Option<String> {
        match &self.place {
            ChatPlace::Elsewhere(agent) => agent.address.clone(),
            ChatPlace::Local { .. } | ChatPlace::History(_) => self
                .registration()
                .map(|registration| registration.address.clone()),
        }
    }

    /// What it is doing at a glance: its summary, else its title when that
    /// is not already its name.
    pub fn glance(&self) -> String {
        if !self.summary.trim().is_empty() {
            return self.summary.trim().to_string();
        }
        let title = self.title();
        if title != self.name() {
            title
        } else {
            String::new()
        }
    }

    /// The line under its name: `Claude Code · relay-app`, and where it
    /// runs when that is not this computer.
    pub fn meta(&self) -> String {
        let mut parts = vec![self.provider()];
        let project = self.project();
        if !project.is_empty() {
            parts.push(project);
        }
        if let ChatPlace::Elsewhere(agent) = &self.place {
            parts.push(agent.host.clone().unwrap_or_else(|| CHAT_ELSEWHERE.into()));
        }
        parts.join(" · ")
    }

    /// The composer's placeholder.
    pub fn placeholder(&self) -> String {
        if self.address().is_some() {
            format!("Message @{}", self.name())
        } else {
            format!("Message {}", self.title())
        }
    }

    /// The note under the composer before anything is sent.
    pub fn composer_note(&self) -> String {
        // The core's reason a message would wait (a question the session
        // is asking, a busy turn) comes first: it is why a queue stalls.
        if let Some(wait) = self
            .local()
            .and_then(|(_, session)| session.delivery_wait.as_deref())
            .filter(|wait| !wait.trim().is_empty())
        {
            return wait.to_string();
        }
        match self.activity {
            ChatActivity::Ended => return CHAT_ENDED.into(),
            ChatActivity::Offline => return CHAT_NOT_LIVE.into(),
            _ => {}
        }
        match (&self.place, self.address()) {
            (ChatPlace::Elsewhere(_), _) => CHAT_ELSEWHERE_NOTE.into(),
            (_, None) => CHAT_JOINS_ON_SEND.into(),
            (_, Some(_)) => CHAT_SEND_NOTE.into(),
        }
    }

    /// Whether every message the person sends it reaches it through the
    /// relay, framed or not: a headless agent's stdio input, and direct
    /// messages to an agent elsewhere.
    pub fn relay_only(&self) -> bool {
        match &self.place {
            ChatPlace::Elsewhere(_) => true,
            ChatPlace::History(_) => false,
            ChatPlace::Local { session, .. } => {
                SpawnHandover::joins(Some(session))
                    || session.holder == Some(SessionHolder::Background)
            }
        }
    }

    /// Whether the composer can send to it.
    pub fn can_send(&self) -> bool {
        self.activity != ChatActivity::Ended && (self.local().is_some() || self.address().is_some())
    }

    /// Whether the agent is yours: every agent on this computer is, and an
    /// agent elsewhere when the relay says so.
    pub fn is_yours(&self) -> bool {
        match &self.place {
            ChatPlace::Elsewhere(remote) => remote.is_self,
            _ => true,
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [self.name(), self.title(), self.meta(), self.summary.clone()]
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
    }
}

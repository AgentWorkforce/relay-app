//! Agents in words: initials, and whether a session can
//! receive relay messages right now.

/// A person's initials (at most two), else `?`.
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .take(2)
        .filter_map(|part| part.chars().next())
        .collect();
    if letters.is_empty() {
        "?".to_string()
    } else {
        letters.to_uppercase()
    }
}

/// The session’s registration and relay connection, as its row
/// shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Presence {
    /// Not on the relay: no name, nothing delivered.
    Off,
    /// On the relay with a connected Relaycast push channel. This does not
    /// verify the local provider inbox.
    Online,
    /// On the relay while the push channel (re)connects; messages still
    /// arrive through the queue check.
    Connecting,
    /// On the relay but the session is not running; messages wait.
    Offline,
    /// Asked to go on the relay; registering.
    Joining,
    /// Asked to leave the relay; releasing its name.
    Leaving,
}

impl Presence {
    /// `wanted` is the state last asked for while it differs from the
    /// relay's. `push_live` is `None` before the push channel reports a
    /// state.
    pub fn of(
        on_relay: bool,
        wanted: Option<bool>,
        live: bool,
        push_live: Option<bool>,
    ) -> Presence {
        if let Some(wanted) = wanted.filter(|wanted| *wanted != on_relay) {
            return if wanted {
                Presence::Joining
            } else {
                Presence::Leaving
            };
        }
        if !on_relay {
            Presence::Off
        } else if !live {
            Presence::Offline
        } else if push_live == Some(true) {
            Presence::Online
        } else {
            Presence::Connecting
        }
    }

    pub fn message_status(self) -> &'static str {
        match self {
            Self::Online => "Relay connected",
            Self::Joining | Self::Connecting => "Connecting…",
            Self::Off => "Off",
            Self::Offline => "Offline",
            Self::Leaving => "Turning off…",
        }
    }

    pub fn message_symbol(self) -> &'static str {
        match self {
            Self::Online => "checkmark.circle.fill",
            Self::Joining | Self::Connecting | Self::Leaving => "arrow.triangle.2.circlepath",
            Self::Offline => "pause.circle",
            Self::Off => "stop.circle",
        }
    }

    pub fn message_class(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Joining | Self::Connecting => "connecting",
            Self::Off | Self::Offline | Self::Leaving => "offline",
        }
    }

    pub fn message_help(self) -> &'static str {
        match self {
            Self::Online => "Connected to the relay. Check each message’s delivery status. Click to go offline.",
            Self::Joining | Self::Connecting => "Connecting to the relay. Click to go offline.",
            Self::Offline => "Offline — this session is not running. Messages wait until it starts. Click to turn off.",
            Self::Off => "Off. Click to receive messages.",
            Self::Leaving => "Turning off message delivery. Click to reconnect.",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Presence::Off => "Not on the relay",
            Presence::Online => "Relay connected",
            Presence::Connecting => "Connecting",
            Presence::Offline => "Offline",
            Presence::Joining => "Joining the relay…",
            Presence::Leaving => "Leaving the relay…",
        }
    }

    /// On, as the switch shows it: the state asked for wins.
    pub fn is_on(self) -> bool {
        !matches!(self, Presence::Off | Presence::Leaving)
    }

    pub fn is_changing(self) -> bool {
        matches!(self, Presence::Joining | Presence::Leaving)
    }

    /// The Mac's `RelayPresence.detail`.
    pub fn detail(self) -> &'static str {
        match self {
            Self::Off => "Turn on to make this session reachable on the relay.",
            Self::Online => "Connected to the relay. Check each message’s delivery status.",
            Self::Connecting => "Connecting to the relay. Messages wait for delivery.",
            Self::Offline => "The session isn't running. Messages wait until it is.",
            Self::Joining => "Registering a name on the relay.",
            Self::Leaving => "Releasing its name on the relay.",
        }
    }

    /// The badge on the session header's second line; `None` while the
    /// session is not on the relay. The Mac's `RelayPresence.headerBadge`.
    pub fn header_badge(self) -> Option<&'static str> {
        match self {
            Self::Online => Some("Online"),
            Self::Joining | Self::Connecting => Some("Connecting…"),
            Self::Offline => Some("Offline"),
            Self::Off | Self::Leaving => None,
        }
    }
}

/// The session header's second line: the provider, then `@name` on the relay
/// when the session has one. The presence badge follows it.
pub fn session_header_identity(provider: &str, agent_name: Option<&str>) -> String {
    match agent_name.filter(|name| !name.is_empty()) {
        Some(name) if provider.is_empty() => format!("@{name}"),
        Some(name) => format!("{provider} · @{name}"),
        None => provider.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_desktop_core::history::session_model::source_name;

    #[test]
    fn session_header_names_the_provider_agent_and_presence() {
        assert_eq!(
            session_header_identity("Claude Code", Some("api-fixer")),
            "Claude Code · @api-fixer"
        );
        assert_eq!(session_header_identity("Codex", None), "Codex");
        assert_eq!(session_header_identity("Codex", Some("")), "Codex");
        assert_eq!(Presence::Online.header_badge(), Some("Online"));
        assert_eq!(Presence::Connecting.header_badge(), Some("Connecting…"));
        assert_eq!(Presence::Joining.header_badge(), Some("Connecting…"));
        assert_eq!(Presence::Offline.header_badge(), Some("Offline"));
        assert_eq!(Presence::Off.header_badge(), None);
        assert_eq!(Presence::Leaving.header_badge(), None);
        // The tooltips match the Mac's `RelayPresence.detail` word for word.
        for (presence, detail) in [
            (Presence::Off, "Turn on to make this session reachable on the relay."),
            (Presence::Online, "Connected to the relay. Check each message’s delivery status."),
            (Presence::Connecting, "Connecting to the relay. Messages wait for delivery."),
            (Presence::Offline, "The session isn't running. Messages wait until it is."),
            (Presence::Joining, "Registering a name on the relay."),
            (Presence::Leaving, "Releasing its name on the relay."),
        ] {
            assert_eq!(presence.detail(), detail);
        }
    }

    #[test]
    fn message_orbs_only_go_green_after_push_is_live() {
        for (presence, label) in [
            (Presence::Off, "Off"),
            (Presence::Offline, "Offline"),
            (Presence::Leaving, "Turning off…"),
        ] {
            assert_eq!(presence.message_status(), label);
            assert_eq!(presence.message_class(), "offline");
        }
        for presence in [Presence::Joining, Presence::Connecting] {
            assert_eq!(presence.message_status(), "Connecting…");
            assert_eq!(presence.message_class(), "connecting");
        }
        assert_eq!(Presence::Online.message_status(), "Relay connected");
        assert_eq!(Presence::Online.message_class(), "online");
    }

    #[test]
    fn presence_follows_the_requested_state_then_the_push_channel() {
        assert_eq!(
            Presence::of(false, Some(true), true, None),
            Presence::Joining
        );
        assert_eq!(
            Presence::of(true, Some(false), true, Some(true)),
            Presence::Leaving
        );
        assert_eq!(Presence::of(false, None, true, None), Presence::Off);
        assert_eq!(
            Presence::of(true, None, false, Some(true)),
            Presence::Offline
        );
        assert_eq!(Presence::of(true, None, true, Some(true)), Presence::Online);
        assert_eq!(Presence::of(true, None, true, None), Presence::Connecting);
        assert_eq!(
            Presence::of(true, Some(true), true, Some(true)),
            Presence::Online
        );
        assert!(Presence::Joining.is_on() && Presence::Joining.is_changing());
        assert!(!Presence::Leaving.is_on());
    }

    #[test]
    fn names_and_initials() {
        assert_eq!(source_name("XAI"), "Grok");
        assert_eq!(source_name("Grok-Bot"), "Grok Bot");
        assert_eq!(source_name("claude-code"), "Claude Code");
        assert_eq!(source_name("cursor-cloud"), "Cursor Cloud");
        assert_eq!(source_name("gemini"), "Gemini");
        assert_eq!(source_name("openclaw"), "OpenClaw");
        assert_eq!(source_name("hermes"), "Hermes");
        assert_eq!(source_name("droid"), "Factory Droid");
        assert_eq!(source_name("Factory"), "Factory Droid");
        assert_eq!(source_name("pi"), "Pi");
        assert_eq!(source_name("omp"), "Oh My Pi");
        assert_eq!(initials("ada  lovelace byron"), "AL");
        assert_eq!(initials(" "), "?");
    }
}

//! The calls Cursor's `beforeMCPExecution` hook announced, each waiting for
//! the helper's matching `tools/call`. An intent is local and short-lived:
//! it only says which chat a call with exactly this tool and these
//! arguments belongs to, and is consumed by the first call that matches.
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long an announced call waits for its `tools/call`. Cursor sends the
/// call right after the hook allows it, unless the person has to approve it
/// first.
pub(crate) const LIFETIME: Duration = Duration::from_secs(120);
/// How many calls may wait at once, across every chat.
pub(crate) const CAPACITY: usize = 256;

/// One announced call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Intent {
    /// The chat: Cursor's conversation id, lowercased.
    pub sid: String,
    /// The hosted MCP tool, without Cursor's server prefix.
    pub tool: String,
    /// The call's arguments without the proof field, canonical
    /// (`proof::canonical`).
    pub args: String,
    /// Cursor's `tool_use_id`, the proof's `tuid`.
    pub tuid: String,
    /// The identity sync generation it was announced under.
    pub generation: u64,
}

/// The waiting calls, oldest first.
#[derive(Default)]
pub struct Intents(Mutex<VecDeque<(Instant, Intent)>>);

impl Intents {
    fn waiting(&self) -> std::sync::MutexGuard<'_, VecDeque<(Instant, Intent)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `intent` until its call comes or it expires. A second
    /// announcement of the same `tool_use_id` replaces the first.
    pub(crate) fn register(&self, intent: Intent, now: Instant) -> Result<(), &'static str> {
        let mut waiting = self.waiting();
        waiting.retain(|(at, waiting)| {
            now.saturating_duration_since(*at) < LIFETIME && waiting.tuid != intent.tuid
        });
        if waiting.len() >= CAPACITY {
            return Err("cursor_intent_capacity");
        }
        waiting.push_back((now, intent));
        Ok(())
    }

    /// Consumes the oldest call announced under `generation` with exactly
    /// `tool` and `args`. Calls of other generations are dropped. When chats
    /// other than one announced the same call, none of them can be told
    /// apart, so every match is dropped and the call is refused.
    pub(crate) fn take(
        &self,
        tool: &str,
        args: &str,
        generation: u64,
        now: Instant,
    ) -> Result<Option<Intent>, &'static str> {
        let mut waiting = self.waiting();
        waiting.retain(|(at, waiting)| {
            now.saturating_duration_since(*at) < LIFETIME && waiting.generation == generation
        });
        let matches = |intent: &Intent| intent.tool == tool && intent.args == args;
        let Some(first) = waiting.iter().position(|(_, intent)| matches(intent)) else {
            return Ok(None);
        };
        let sid = waiting[first].1.sid.clone();
        if waiting
            .iter()
            .any(|(_, intent)| matches(intent) && intent.sid != sid)
        {
            waiting.retain(|(_, intent)| !matches(intent));
            return Err("cursor_intent_ambiguous");
        }
        Ok(waiting.remove(first).map(|(_, intent)| intent))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.waiting().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(sid: &str, tuid: &str, args: &str) -> Intent {
        Intent {
            sid: sid.into(),
            tool: "send_relay_message".into(),
            args: args.into(),
            tuid: tuid.into(),
            generation: 3,
        }
    }

    #[test]
    fn a_call_takes_only_its_exact_tool_and_arguments_once() {
        let intents = Intents::default();
        let now = Instant::now();
        intents
            .register(intent("a", "t1", r#"{"to":"x"}"#), now)
            .unwrap();
        assert_eq!(
            intents.take("send_relay_message", r#"{"to":"y"}"#, 3, now),
            Ok(None)
        );
        assert_eq!(
            intents.take("post_relay_channel", r#"{"to":"x"}"#, 3, now),
            Ok(None)
        );
        let taken = intents
            .take("send_relay_message", r#"{"to":"x"}"#, 3, now)
            .unwrap()
            .unwrap();
        assert_eq!((taken.sid.as_str(), taken.tuid.as_str()), ("a", "t1"));
        assert_eq!(
            intents.take("send_relay_message", r#"{"to":"x"}"#, 3, now),
            Ok(None),
            "single use"
        );
    }

    #[test]
    fn identical_calls_of_one_chat_are_taken_oldest_first() {
        let intents = Intents::default();
        let now = Instant::now();
        intents.register(intent("a", "t1", "{}"), now).unwrap();
        intents.register(intent("a", "t2", "{}"), now).unwrap();
        let take = || {
            intents
                .take("send_relay_message", "{}", 3, now)
                .unwrap()
                .unwrap()
                .tuid
        };
        assert_eq!(take(), "t1");
        assert_eq!(take(), "t2");
    }

    #[test]
    fn the_same_call_from_two_chats_is_refused_and_dropped() {
        let intents = Intents::default();
        let now = Instant::now();
        intents.register(intent("a", "t1", "{}"), now).unwrap();
        intents.register(intent("b", "t2", "{}"), now).unwrap();
        intents
            .register(intent("b", "t3", r#"{"other":1}"#), now)
            .unwrap();
        assert_eq!(
            intents.take("send_relay_message", "{}", 3, now),
            Err("cursor_intent_ambiguous")
        );
        assert_eq!(intents.take("send_relay_message", "{}", 3, now), Ok(None));
        assert!(intents
            .take("send_relay_message", r#"{"other":1}"#, 3, now)
            .unwrap()
            .is_some());
    }

    #[test]
    fn calls_expire_and_do_not_cross_generations() {
        let intents = Intents::default();
        let now = Instant::now();
        intents.register(intent("a", "t1", "{}"), now).unwrap();
        assert_eq!(
            intents.take("send_relay_message", "{}", 3, now + LIFETIME),
            Ok(None)
        );
        intents.register(intent("a", "t2", "{}"), now).unwrap();
        assert_eq!(intents.take("send_relay_message", "{}", 4, now), Ok(None));
        assert_eq!(intents.len(), 0, "another generation's calls are dropped");
    }

    #[test]
    fn a_repeated_tool_use_id_replaces_its_call_and_capacity_is_bounded() {
        let intents = Intents::default();
        let now = Instant::now();
        intents.register(intent("a", "t1", "{}"), now).unwrap();
        intents
            .register(intent("a", "t1", r#"{"to":"x"}"#), now)
            .unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents.take("send_relay_message", "{}", 3, now), Ok(None));
        for n in 1..CAPACITY {
            intents
                .register(intent("a", &format!("c{n}"), "{}"), now)
                .unwrap();
        }
        assert_eq!(
            intents.register(intent("a", "late", "{}"), now),
            Err("cursor_intent_capacity")
        );
        assert!(intents
            .register(intent("a", "late", "{}"), now + LIFETIME)
            .is_ok());
    }
}

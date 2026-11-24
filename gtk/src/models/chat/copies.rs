//! Telling a copy of a message from the message itself. The session's
//! transcript and the core's own record of the same turn arrive under
//! different ids, seconds apart, and Cloud can hold the same line twice.

use std::collections::HashMap;

/// How far apart two lines of the same role and text can be and still be
/// one message recorded twice.
const SAME_TURN_MS: i64 = 10_000;
/// How far the core's own record of a turn (an id with a content hash) can
/// lag the transcript's line: most of a turn.
const RECORD_LAG_MS: i64 = 600_000;

/// One text drawn: when it was written, whether it was the core's record,
/// and whether another text was drawn after it.
#[derive(Clone, Copy, Debug)]
struct Drawn {
    at: i64,
    record: bool,
    interrupted: bool,
}

/// The texts drawn in a conversation, by role and text.
#[derive(Debug, Default)]
pub struct ChatCopies {
    drawn: HashMap<String, Drawn>,
}

impl ChatCopies {
    /// Whether the `role` text `text` written at `at_ms` (the core's record
    /// when `record`) is a copy of one already drawn. Two lines of the same
    /// kind are one message only with nothing else said between them; the
    /// core's record matches its transcript line across the agent's reply.
    pub fn copied(&mut self, role: &str, text: &str, at_ms: i64, record: bool) -> bool {
        let key = format!("{role}|{}", text.trim());
        let copy = self.drawn.get(&key).is_some_and(|drawn| {
            let window = if record != drawn.record {
                RECORD_LAG_MS
            } else if drawn.interrupted {
                return false;
            } else {
                SAME_TURN_MS
            };
            (at_ms - drawn.at).abs() <= window
        });
        if !copy {
            for (other, drawn) in &mut self.drawn {
                if *other != key {
                    drawn.interrupted = true;
                }
            }
        }
        self.drawn.insert(
            key,
            Drawn {
                at: at_ms,
                record,
                interrupted: false,
            },
        );
        copy
    }

    /// Forgets every text, as when a conversation is drawn afresh.
    pub fn clear(&mut self) {
        self.drawn.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_line_twice_is_one_message() {
        let mut copies = ChatCopies::default();
        assert!(!copies.copied("user", "ping", 0, false));
        assert!(copies.copied("user", "ping ", 81, false));
        // Long after, it is another message.
        assert!(!copies.copied("user", "ping", 60_000, false));
    }

    #[test]
    fn a_reply_between_makes_the_same_words_another_message() {
        let mut copies = ChatCopies::default();
        assert!(!copies.copied("user", "yes", 0, false));
        assert!(!copies.copied("assistant", "Done", 3_000, false));
        assert!(!copies.copied("user", "yes", 6_000, false));
    }

    #[test]
    fn the_cores_record_matches_its_line_across_the_reply() {
        let mut copies = ChatCopies::default();
        assert!(!copies.copied("user", "fix it", 0, false));
        assert!(!copies.copied("assistant", "Fixed.", 90_000, false));
        assert!(copies.copied("user", "fix it", 120_000, true));
    }
}

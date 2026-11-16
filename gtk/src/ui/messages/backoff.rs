//! The read backoff: after a read of a conversation failed (its history,
//! or the look-up of its recipient), nothing but the user's own asking
//! reads it until the wait ends, and then the retry reads it. A read that
//! succeeds goes back to the normal cadence, but only the read whose
//! failure started the wait: a history read says nothing about a look-up
//! of the recipient that failed. The Mac's counterpart is `RelayModel`'s
//! `settleRead`.
use super::*;
use crate::models::{read_failure_text, recipient_failure_text};

/// Which read of a conversation answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Read {
    History,
    Recipient,
}

impl ChatState {
    /// Whether a failed read's wait is still pending.
    pub(super) fn backing_off(&self) -> bool {
        self.backoff_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Whether the last failure, still unanswered, was the recipient's
    /// look-up: the line under the conversation says so until it reads.
    fn recipient_failed(&self) -> bool {
        self.backoff_by == Some(Read::Recipient)
    }

    /// `read` succeeded: ends the wait when its failure started it, and
    /// says whether it did.
    fn read_succeeded(&mut self, read: Read) -> bool {
        if self.backoff_by != Some(read) {
            return false;
        }
        self.backoff.succeeded();
        self.backoff_until = None;
        self.backoff_by = None;
        true
    }
}

impl Chat {
    /// Stops the conversation's reads without being asked.
    pub(super) fn stop_following(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        self.stop_retry();
    }

    /// Drops the read waiting for a failed read's wait to end.
    fn stop_retry(&self) {
        if let Some(retry) = self.retry.borrow_mut().take() {
            retry.remove();
        }
    }

    /// The user turned to the conversation, which is asking for it: a wait
    /// is dropped and the next failure starts afresh. Returns whether a
    /// failure was still unanswered, so the history is read at once.
    pub(super) fn drop_backoff(&self) -> bool {
        let waiting = {
            let mut state = self.state.borrow_mut();
            let waiting = state.backoff_by.is_some();
            state.backoff = ReadBackoff::default();
            state.backoff_until = None;
            state.backoff_by = None;
            waiting
        };
        self.stop_retry();
        waiting
    }
}

impl App {
    /// `read` of the conversation failed with `message`: it is not read
    /// again without being asked until the wait ends, and then it is.
    pub(super) fn back_off_chat(&self, chat: &Rc<Chat>, read: Read, message: &str) {
        let delay = {
            let mut state = chat.state.borrow_mut();
            let delay = state.backoff.failed(None, glib::random_double());
            let delay = match state.backoff_unit {
                Some(unit) => {
                    delay.mul_f64(unit.as_secs_f64() / ReadBackoff::CADENCE.as_secs_f64())
                }
                None => delay,
            };
            state.backoff_until = Some(Instant::now() + delay);
            state.backoff_by = Some(read);
            delay
        };
        chat.status.set_text(&match read {
            Read::History => read_failure_text(message, delay),
            Read::Recipient => recipient_failure_text(message, delay),
        });
        chat.stop_retry();
        let retry = glib::timeout_add_local_once(delay, self.retry_chat(chat));
        *chat.retry.borrow_mut() = Some(retry);
    }

    /// The read that ends a wait.
    fn retry_chat(&self, chat: &Rc<Chat>) -> impl FnOnce() + 'static {
        let weak = self.downgrade();
        let retried = Rc::downgrade(chat);
        move || {
            let (Some(app), Some(chat)) = (weak.upgrade(), retried.upgrade()) else {
                return;
            };
            // It ran: there is nothing to remove any more.
            chat.retry.borrow_mut().take();
            // The wait is over, even when the timer fired a moment early
            // (it counts whole milliseconds): left pending, it would turn
            // this read away. The failures stay counted.
            chat.state.borrow_mut().backoff_until = None;
            if app
                .chat(&chat.key)
                .is_some_and(|current| Rc::ptr_eq(&current, &chat))
            {
                app.follow_chat(&chat.key);
            }
        }
    }

    /// `read` of the conversation succeeded: a wait its failure started
    /// ends, and the line under the conversation says who you message as
    /// unless a failed look-up of the recipient is what it says.
    pub(super) fn chat_read(&self, chat: &Chat, read: Read) {
        let (ended, identity) = {
            let mut state = chat.state.borrow_mut();
            let ended = state.read_succeeded(read);
            if state.recipient_failed() {
                return;
            }
            (ended, state.identity.clone())
        };
        if ended {
            chat.stop_retry();
        }
        if ended || read == Read::History {
            chat.status.set_text(&message_as(&identity));
        }
    }
}

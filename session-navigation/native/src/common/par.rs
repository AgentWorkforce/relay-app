//! Two small tools for a fast look:
//!
//! - [`join`] / [`join3`]: run independent reads at once when the system
//!   allows it ([`System::concurrent`]), else one after another in the order
//!   given, so a `FakeSystem` records its calls in one stable order.
//! - [`Reuse`]: keep a slow-changing fact for a short window (2 s by default)
//!   inside one observer, so the second to fourth look of one
//!   `engine::open` do not read it again. Only facts that cannot change
//!   within one click may go in one (README, "What a look reuses").
use std::time::{Duration, Instant};

use crate::system::System;

/// How long a [`Reuse`] keeps its value unless told otherwise.
pub const REUSE: Duration = Duration::from_secs(2);

fn finish<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    match handle.join() {
        Ok(value) => value,
        // A panic in a read is a bug; let it surface where it would have
        // without the thread.
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// `(a(), b())`, at once when `sys` allows it.
pub fn join<S, A, B>(sys: &S, a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B)
where
    S: System + ?Sized,
    A: Send,
    B: Send,
{
    if !sys.concurrent() {
        let first = a();
        return (first, b());
    }
    std::thread::scope(|scope| {
        let second = scope.spawn(b);
        let first = a();
        (first, finish(second))
    })
}

/// `(a(), b(), c())`, at once when `sys` allows it.
pub fn join3<S, A, B, C>(
    sys: &S,
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B + Send,
    c: impl FnOnce() -> C + Send,
) -> (A, B, C)
where
    S: System + ?Sized,
    A: Send,
    B: Send,
    C: Send,
{
    if !sys.concurrent() {
        let first = a();
        let second = b();
        return (first, second, c());
    }
    std::thread::scope(|scope| {
        let second = scope.spawn(b);
        let third = scope.spawn(c);
        let first = a();
        (first, finish(second), finish(third))
    })
}

/// `f` over every item, at once when `sys` allows it; results in order.
pub fn map<S, T, R>(sys: &S, items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R>
where
    S: System + ?Sized,
    T: Send,
    R: Send,
{
    if !sys.concurrent() || items.len() < 2 {
        return items.into_iter().map(f).collect();
    }
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .into_iter()
            .map(|item| scope.spawn(move || f(item)))
            .collect();
        handles.into_iter().map(finish).collect()
    })
}

/// A value kept for a short window.
#[derive(Clone, Debug)]
pub struct Reuse<T> {
    window: Duration,
    entry: Option<(Instant, T)>,
}

impl<T> Default for Reuse<T> {
    fn default() -> Self {
        Self::new(REUSE)
    }
}

impl<T> Reuse<T> {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            entry: None,
        }
    }
}

impl<T: Clone> Reuse<T> {
    /// Change the window (zero turns reuse off); the kept value is dropped.
    pub fn set_window(&mut self, window: Duration) {
        self.window = window;
        self.entry = None;
    }

    /// The kept value when younger than the window.
    pub fn fresh(&self) -> Option<T> {
        self.entry
            .as_ref()
            .filter(|(at, _)| at.elapsed() < self.window)
            .map(|(_, value)| value.clone())
    }

    /// The kept value, or `read()`'s; a `None` read is not kept.
    pub fn get_or(&mut self, read: impl FnOnce() -> Option<T>) -> Option<T> {
        if let Some(value) = self.fresh() {
            return Some(value);
        }
        let value = read();
        self.entry = value.clone().map(|v| (Instant::now(), v));
        value
    }

    /// The kept value, or `read()`'s, kept only when `keep` says so.
    pub fn get_or_keep(&mut self, read: impl FnOnce() -> T, keep: impl FnOnce(&T) -> bool) -> T {
        if let Some(value) = self.fresh() {
            return value;
        }
        let value = read();
        self.entry = keep(&value).then(|| (Instant::now(), value.clone()));
        value
    }
}

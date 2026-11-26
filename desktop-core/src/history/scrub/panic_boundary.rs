//! A scrubber panic can include the sensitive input in its diagnostic. The
//! panic hook runs before catch_unwind, including in release builds.
use super::ScrubError;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

thread_local! {
    static SCRUBBING: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn run<T>(scrub: impl FnOnce() -> Result<T, ScrubError>) -> Result<T, ScrubError> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !SCRUBBING.try_with(Cell::get).unwrap_or(false) {
                previous(info);
            }
        }));
    });
    SCRUBBING.with(|active| {
        let outer = active.replace(true);
        let result = panic::catch_unwind(AssertUnwindSafe(scrub));
        active.set(outer);
        result.unwrap_or_else(|_| {
            Err(ScrubError {
                reason: "scrubber panicked".into(),
                panicked: true,
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubber_panic_does_not_print_input_or_silence_other_threads() {
        const CHILD: &str = "RELAY_TEST_PRIVATE_SCRUB_HOOK";
        if std::env::var_os(CHILD).is_some() {
            let error = run::<()>(|| {
                let thread = std::thread::spawn(|| panic!("unrelated-thread-diagnostic"));
                assert!(thread.join().is_err());
                panic!("synthetic-private-body")
            })
            .unwrap_err();
            assert!(error.panicked());
            assert!(!error.reason().contains("synthetic-private-body"));
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "scrubber_panic_does_not_print_input_or_silence_other_threads",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        let diagnostic = String::from_utf8(out.stderr).unwrap();
        assert!(diagnostic.contains("unrelated-thread-diagnostic"));
        assert!(!diagnostic.contains("synthetic-private-body"));
    }
}

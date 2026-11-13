//! Whether this build's `start` replaces a collector another build of its
//! release launched (`delivery_status::outdated`). Only the build whose core
//! is the default one does: a site profile built to coexist
//! (DESKTOP_COEXIST) shares the collector directory and never replaces it.
use super::*;

impl ProcessRunner {
    /// Sets whether `start` replaces, and status reports as outdated, a
    /// collector another build of this release launched: `replace` when
    /// this build's core is the default one.
    pub fn replacing_same_release(mut self, replace: bool) -> ProcessRunner {
        self.replace_same_release = replace;
        self
    }

    /// `--replace-same-release` for a `start` when this build replaces its
    /// own release; nothing for any other command.
    pub(super) fn same_release_flag(&self, args: &[String]) -> Option<&'static str> {
        let start = args.first().is_some_and(|name| name == "start");
        (self.replace_same_release && start).then_some("--replace-same-release")
    }
}

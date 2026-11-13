//! Migrating the old independent GUI/service startup registrations.
use super::app::App;

const UNIFIED_STARTUP: &str = "unifiedStartup.v1";

impl App {
    /// Preserve either existing opt-in on upgrade: older headless services
    /// deliberately removed the GUI entry. Never disable an enabled service
    /// while repairing those old registrations.
    pub(super) fn reconcile_startup(&self) {
        if self.inner.updating.get()
            || self.prefs().get(UNIFIED_STARTUP).and_then(|v| v.as_bool()) == Some(true)
        {
            return;
        }
        let mismatch = self.with_state(|state| {
            if state.login_item_wanted.is_pending() {
                return None;
            }
            let gui = state.login_item?;
            let service = state.service_state?;
            Some(service.installed && gui != service.enabled)
        });
        let Some(mismatch) = mismatch else {
            return;
        };
        // Attempt migration once. A failed attempt stays visible as a
        // mismatch with a repair action, instead of retrying on every read.
        self.startup_choice_made();
        if mismatch {
            self.change_autostart(true);
        }
    }

    pub(super) fn startup_choice_made(&self) {
        let _ = self
            .inner
            .prefs
            .borrow_mut()
            .set(UNIFIED_STARTUP, true.into());
    }
}

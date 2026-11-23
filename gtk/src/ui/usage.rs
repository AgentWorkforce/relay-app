//! Explicit desktop usage and debounced search events. Never records query text.
use super::app::App;
use super::state::{Page, Pane, State};
use agent_relay_desktop_core::telemetry::{
    TelemetryEvent, TelemetryFailure, TelemetryOperation, TelemetryOutcome, TelemetrySurface,
};
use gtk::glib;
use gtk4 as gtk;
use std::time::Duration;

impl App {
    /// Changes the window's state without redrawing.
    pub fn update_state<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let (result, page, session_search, connected) = {
            let mut state = self.inner.state.borrow_mut();
            let before = (state.connected(), state.page, state.session_search.clone());
            let result = change(&mut state);
            let page =
                (state.connected() && (!before.0 || before.1 != state.page)).then_some(state.page);
            let sessions = (before.2 != state.session_search).then(|| state.session_search.clone());
            (result, page, sessions, state.connected())
        };
        if let Some(page) = page {
            self.cancel_search_telemetry(Some(page.telemetry_surface()));
            self.record_surface(
                TelemetryOperation::PageView,
                TelemetryOutcome::Succeeded,
                page.telemetry_surface(),
            );
            if matches!(page, Page::Settings(_)) {
                self.record_surface(
                    TelemetryOperation::Settings,
                    TelemetryOutcome::Started,
                    page.telemetry_surface(),
                );
            }
        }
        if connected {
            if let Some(query) = session_search {
                self.schedule_search_telemetry(&query, TelemetrySurface::Sessions);
            }
        } else {
            self.cancel_search_telemetry(None);
        }
        result
    }

    fn cancel_search_telemetry(&self, keep: Option<TelemetrySurface>) {
        let mut timers = self.inner.search_telemetry.borrow_mut();
        // A query can be applied in a separate update just before navigation.
        let arriving = keep.and_then(|surface| timers.remove(&surface));
        for (_, timer) in timers.drain() {
            timer.remove();
        }
        if let (Some(surface), Some(timer)) = (keep, arriving) {
            timers.insert(surface, timer);
        }
    }

    /// `telemetry.record` for what only the UI sees.
    pub(super) fn record(
        &self,
        operation: TelemetryOperation,
        outcome: TelemetryOutcome,
        failure: Option<TelemetryFailure>,
    ) {
        if !self.inner.bridge.offers(agent_relay_desktop_core::contract::method::TelemetryRecord::NAME) {
            return;
        }
        let mut event = TelemetryEvent::new(operation, outcome);
        if let Some(failure) = failure {
            event = event.failure(failure);
        }
        self.call::<agent_relay_desktop_core::contract::method::TelemetryRecord>(event, |_, _| {});
    }

    pub(super) fn record_surface(
        &self,
        operation: TelemetryOperation,
        outcome: TelemetryOutcome,
        surface: TelemetrySurface,
    ) {
        if self.inner.bridge.offers(agent_relay_desktop_core::contract::method::TelemetryRecord::NAME) {
            self.call::<agent_relay_desktop_core::contract::method::TelemetryRecord>(
                TelemetryEvent::new(operation, outcome).surface(surface),
                |_, _| {},
            );
        }
    }

    fn schedule_search_telemetry(&self, query: &str, surface: TelemetrySurface) {
        if let Some(timer) = self.inner.search_telemetry.borrow_mut().remove(&surface) {
            timer.remove();
        }
        if query.trim().is_empty() {
            return;
        }
        let weak = self.downgrade();
        let timer = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if let Some(app) = weak.upgrade() {
                app.inner.search_telemetry.borrow_mut().remove(&surface);
                if !app.with_state(|state| {
                    state.connected() && state.page.telemetry_surface() == surface
                }) {
                    return;
                }
                app.record_surface(
                    TelemetryOperation::Search,
                    TelemetryOutcome::Succeeded,
                    surface,
                );
            }
        });
        self.inner
            .search_telemetry
            .borrow_mut()
            .insert(surface, timer);
    }

    pub(super) fn open_approval_url(&self, url: &str) {
        self.record(
            TelemetryOperation::BrowserApproval,
            TelemetryOutcome::Started,
            None,
        );
        let opened = self.inner.desktop.open_uri(url);
        self.record(
            TelemetryOperation::BrowserApproval,
            if opened {
                TelemetryOutcome::Succeeded
            } else {
                TelemetryOutcome::Failed
            },
            (!opened).then_some(TelemetryFailure::BrowserOpen),
        );
    }

    pub(super) fn open_message_composer(&self) {
        self.record_surface(
            TelemetryOperation::MessageComposer,
            TelemetryOutcome::Started,
            TelemetrySurface::Messages,
        );
        self.go(Page::Messages);
    }
}

impl Page {
    pub fn telemetry_surface(self) -> agent_relay_desktop_core::telemetry::TelemetrySurface {
        use agent_relay_desktop_core::telemetry::TelemetrySurface as Surface;
        match self {
            Self::Sessions => Surface::Sessions,
            Self::Messages => Surface::Messages,
            Self::Settings(pane) => match pane {
                Pane::General => Surface::SettingsGeneral,
                Pane::Tools => Surface::SettingsTools,
                Pane::Opening => Surface::SettingsOpening,
                Pane::Relay => Surface::SettingsRelay,
                Pane::Sharing => Surface::SettingsSharing,
                Pane::Account => Surface::SettingsAccount,
                Pane::About => Surface::SettingsAbout,
                Pane::Diagnostics => Surface::SettingsDiagnostics,
            },
        }
    }
}

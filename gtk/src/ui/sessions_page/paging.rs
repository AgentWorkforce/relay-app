use super::*;

impl App {
    pub(super) fn connect_query_paging(&self, page: &SessionsPage) {
        let weak = self.downgrade();
        page.next_page.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| {
                    state.session_query.cursor = state.session_query.next_cursor.clone();
                    state.session_query.known_revision = None;
                });
                app.refresh_uploader(true);
            }
        });
        let weak = self.downgrade();
        page.first_page.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.session_query_changed();
                app.refresh_uploader(true);
            }
        });
    }
    pub(super) fn load_more_sessions_if_needed(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.sessions;
        if self.page() != crate::ui::state::Page::Sessions
            || !page.table.is_visible()
            || !page.scroller.is_mapped()
            || page.drawing.get()
        {
            return;
        }
        if self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderSessionsQuery::NAME) {
            return;
        }
        let adjustment = page.scroller.vadjustment();
        if adjustment.page_size() <= 0.0
            || adjustment.value() + adjustment.page_size() + 200.0 < adjustment.upper()
        {
            return;
        }
        let load = self.update_state(|state| {
            if !state.connected()
                || state.busy
                || state.refreshing_sessions
                || state.sessions_refreshed.is_none()
                || state.account().phase.as_deref() == Some("preparing_local_sessions")
                || state.session_query.limit >= 10_000
                || state.sessions.len() < state.session_query.limit
            {
                return false;
            }
            state.session_query.limit = (state.session_query.limit + 500).min(10_000);
            true
        });
        if load {
            self.refresh_uploader(true);
        }
    }
}

pub(super) fn configure_sort(
    button: &gtk::Button,
    column: crate::models::LocalSessionSortColumn,
    ascending: bool,
    core_query: bool,
) {
    let unsupported = core_query && column == crate::models::LocalSessionSortColumn::Reasoning;
    button.set_sensitive(!unsupported);
    let tooltip = if unsupported {
        "Reasoning sort is unavailable because local history does not record session reasoning effort.".to_owned()
    } else {
        format!(
            "{}: click to sort {}",
            column.title(),
            if ascending { "ascending" } else { "descending" }
        )
    };
    button.set_tooltip_text(Some(&tooltip));
}

impl SessionsPage {
    pub(super) fn update_query_paging(&self, app: &App) {
        app.with_state(|state| {
            self.next_page
                .set_label(if state.session_query.verification_pending {
                    "Retry page"
                } else {
                    "Next page"
                });
            self.next_page.set_tooltip_text(
                state
                    .session_query
                    .verification_pending
                    .then_some("Checking upload status for this page. Retry to continue."),
            );
            self.next_page
                .set_visible(state.session_query.next_cursor.is_some());
            self.next_page.set_sensitive(!state.refreshing_sessions);
            self.first_page
                .set_visible(state.session_query.cursor.is_some());
            self.first_page.set_sensitive(!state.refreshing_sessions);
        });
    }
}

pub(super) fn build(table: &gtk::Box) -> (gtk::Button, gtk::Button) {
    let paging = hbox(8, &[]);
    let first_page = gtk::Button::with_label("First page");
    let next_page = gtk::Button::with_label("Next page");
    widgets::name(&first_page, "First page");
    widgets::name(&next_page, "Next page");
    paging.append(&first_page);
    paging.append(&next_page);
    table.append(&paging);
    first_page.set_visible(false);
    next_page.set_visible(false);
    (first_page, next_page)
}

impl App {
    pub(super) fn restore_default_query_view(&self, page: &SessionsPage) {
        if !self.with_state(|state| state.restored_default_session_view) {
            if let Some(view) = self.saved_session_views().default_view() {
                let config = &view.configuration;
                page.grouping.set(config.grouping.min(4));
                page.sort.set(
                    if self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderSessionsQuery::NAME)
                        && config.sort.column == crate::models::LocalSessionSortColumn::Reasoning
                    {
                        LocalSessionSort::default()
                    } else {
                        config.sort
                    },
                );
                self.update_state(|state| state.session_search = config.search.clone());
                let _ = self
                    .inner
                    .prefs
                    .borrow_mut()
                    .set(crate::models::SESSION_COLUMNS_KEY, config.layout().value());
                let _ = self
                    .inner
                    .prefs
                    .borrow_mut()
                    .set("showHiddenSessions.v1", config.show_hidden.into());
            }
            self.update_state(|state| state.restored_default_session_view = true);
            self.session_query_changed();
        }
    }
}

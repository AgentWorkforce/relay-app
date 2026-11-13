use super::*;
use agent_relay_desktop_core::contract::account::{SessionFacetParams, SessionFacetsPage};

impl App {
    pub(super) fn refresh_session_facets(&self) {
        if !self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderSessionsFacets::NAME) {
            return;
        }
        let generation = self.update_state(|state| {
            if state.session_query.facets_loading {
                return None;
            }
            state.session_query.facets_loading = true;
            Some(state.generation)
        });
        if let Some(generation) = generation {
            self.facet_page(generation, None, None, vec![], vec![], 0);
        }
    }
    fn facet_page(
        &self,
        generation: u64,
        cursor: Option<String>,
        revision: Option<u64>,
        mut harnesses: Vec<String>,
        mut projects: Vec<String>,
        restarts: u8,
    ) {
        let first = cursor.is_none();
        self.call::<agent_relay_desktop_core::contract::method::UploaderSessionsFacets>(
            SessionFacetParams {
                cursor,
                limit: Some(200),
            },
            move |app, result| {
                if app.with_state(|s| s.generation) != generation {
                    return;
                }
                match result {
                    Ok(page) => {
                        if (first
                            && app.with_state(|s| {
                                s.session_query
                                    .facets
                                    .as_ref()
                                    .is_some_and(|f| f.revision == page.revision)
                            }))
                            || revision.is_some_and(|r| r != page.revision)
                        {
                            app.update_state(|s| s.session_query.facets_loading = false);
                            return;
                        }
                        harnesses.extend(page.harnesses);
                        projects.extend(page.projects);
                        if let Some(cursor) = page.next_cursor {
                            app.facet_page(
                                generation,
                                Some(cursor),
                                Some(page.revision),
                                harnesses,
                                projects,
                                restarts,
                            );
                        } else {
                            app.update_state(|s| {
                                s.session_query.facets_loading = false;
                                s.session_query.facets =
                                    Some(std::rc::Rc::new(SessionFacetsPage {
                                        harnesses,
                                        projects,
                                        revision: page.revision,
                                        next_cursor: None,
                                    }));
                            });
                            if let Some(shell) = app.shell() {
                                app.render_session_controls(&shell.pages.sessions);
                            }
                        }
                    }
                    Err(error)
                        if !first
                            && restarts == 0
                            && error.kind()
                                == Some(agent_relay_node::relay::protocol::ErrorKind::Busy) =>
                    {
                        app.facet_page(generation, None, None, vec![], vec![], 1);
                    }
                    Err(_) => {
                        app.update_state(|s| s.session_query.facets_loading = false);
                    }
                }
            },
        );
    }
}

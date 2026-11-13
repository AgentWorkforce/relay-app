//! The organization's upload policy on the sessions page: the bulk Upload
//! switch it can withdraw, the "Uploads in N min" status, and keeping a
//! waiting session back.

use super::*;
use crate::models::upload_label;

/// The status a row reads out: deleting, preparing, or the upload's own
/// label, "Uploads in N min" while it waits in the upload window.
pub(super) fn row_upload_status(drawn: &DrawnRow) -> String {
    if drawn.stopping {
        "Deleting…".to_string()
    } else if drawn.requesting {
        SessionUploadState::Preparing.icon_title().to_string()
    } else {
        match (drawn.state, drawn.session.upload.as_ref()) {
            (Some(state), Some(upload)) => upload_label(state, upload, crate::ui::now_ms()),
            (Some(state), None) => state.icon_title().to_string(),
            (None, _) => SessionUploadState::Discovering.icon_title().to_string(),
        }
    }
}

impl App {
    /// The selection bar's Upload switch, or "Managed by your organization."
    /// where the operator withdrew deleting from Cloud.
    pub(super) fn append_bulk_upload(
        &self,
        bar: &gtk::Box,
        rows: &[UploadSession],
        selected_keys: &[String],
        included: Vec<String>,
        uploadable: Vec<String>,
        read_only: bool,
    ) {
        let upload_on = !selected_keys.is_empty()
            && selected_keys.iter().all(|key| {
                rows.iter().any(|session| {
                    session_key(session) == *key
                        && self.with_state(|state| {
                            !state.stopping.contains(key)
                                && (session.included || state.requesting.contains(key))
                        })
                })
            });
        let upload = gtk::Switch::new();
        upload.set_widget_name("bulk-upload");
        widgets::name(&upload, "Upload");
        upload.set_active(upload_on);
        upload.set_sensitive(
            !read_only
                && if upload_on {
                    !included.is_empty()
                } else {
                    !uploadable.is_empty()
                },
        );
        upload.set_valign(gtk::Align::Center);
        // Turning Upload off deletes the cloud copies; when the operator
        // withdraws that, a switch that could only delete is removed.
        let can_delete = self.with_state(|state| state.can_delete_from_cloud());
        let upload_shown = can_delete || (!upload_on && !uploadable.is_empty());
        let managed = !can_delete && !included.is_empty();
        let action = self.on(move |app| {
            if upload_on {
                app.request_upload_change(included.clone(), false);
            } else {
                app.change_uploads(uploadable.clone(), true);
            }
        });
        let resetting = Cell::new(false);
        upload.connect_state_set(move |switch, _| {
            if resetting.replace(true) {
                return glib::Propagation::Stop;
            }
            switch.set_active(upload_on);
            resetting.set(false);
            action();
            glib::Propagation::Stop
        });
        if upload_shown {
            bar.append(&text("Upload", &[]));
            bar.append(&upload);
        }
        if managed {
            bar.append(&crate::ui::settings_page::sharing::managed_line(0));
        }
    }

    /// Keeps a session waiting in the organization's upload window from
    /// uploading. Local only, through the exclusion store; the core refuses
    /// once the window has ended. As the Mac's `keepBack`.
    pub(in crate::ui) fn keep_back_session(&self, key: String) {
        self.update_state(|state| {
            state.upload_errors.remove(&key);
        });
        let params = SessionKeysParams {
            session_keys: vec![key.clone()],
        };
        self.view_call::<agent_relay_desktop_core::contract::method::UploaderSessionsExclude>(
            params,
            move |app, result| {
                app.update_state(|state| match result {
                    Ok(_) => {
                        state.upload_selection_revision += 1;
                        if let Some(row) = state.sessions.iter_mut().find(|s| session_key(s) == key) {
                            row.included = false;
                            row.status = agent_relay_desktop_core::contract::account::UploadShareStatus::NotShared;
                            row.upload_hold_until_ms = None;
                        }
                    }
                    Err(error) => {
                        state.upload_errors.insert(key.clone(), error.message);
                    }
                });
            },
        );
    }
}

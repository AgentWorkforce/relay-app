//! The open session's upload status in its sidebar: the operator's
//! "Managed by your organization." line, the status with its
//! "Uploads in N min" label, and "Don't upload" while it waits.
use super::*;
use crate::models::upload_label;
use crate::ui::state::State;
use agent_relay_desktop_core::contract::account::UploadSession;

impl App {
    /// Redraws the status rows under the Upload switch.
    pub(super) fn render_upload_status(
        &self,
        page: &TranscriptPage,
        state: &State,
        row: &LocalSessionRow,
        managed: bool,
        requesting: bool,
        stopping: bool,
    ) {
        clear(&page.status);
        if managed {
            page.status
                .append(&crate::ui::settings_page::sharing::managed_line(0));
        }
        let Some(upload) = &row.upload else {
            let status = SessionUploadState::Discovering;
            let line = hbox(6, &[]);
            line.append(&super::super::sessions_page::status_cell(
                status, false, false,
            ));
            line.append(&text(status.icon_title(), &["caption"]));
            page.status.append(&line);
            return;
        };
        let status = SessionUploadState::of(upload, state.status.as_ref());
        page.status
            .append(&upload_status_line(status, upload, requesting, stopping));
        // Only while the session waits in the organization's upload
        // window, as on the Mac.
        if status == SessionUploadState::Waiting && !stopping && !requesting {
            page.status.append(&self.keep_back_button(row.key()));
        }
    }

    /// "Don't upload": keeps a waiting session on this machine.
    fn keep_back_button(&self, key: String) -> gtk::Button {
        let weak = self.downgrade();
        let keep_back = widgets::button("Don't upload", &[], move || {
            if let Some(app) = weak.upgrade() {
                app.keep_back_session(key.clone());
            }
        });
        keep_back.set_halign(gtk::Align::Start);
        keep_back.set_tooltip_text(Some(
            "Keep this session from uploading. It stays on this machine.",
        ));
        keep_back
    }
}

/// The status mark and its label: deleting, preparing, or the upload's own
/// label.
fn upload_status_line(
    status: SessionUploadState,
    upload: &UploadSession,
    requesting: bool,
    stopping: bool,
) -> gtk::Box {
    let line = hbox(6, &[]);
    line.append(&super::super::sessions_page::status_cell(
        status, requesting, stopping,
    ));
    line.append(&text(
        &if stopping {
            "Deleting…".to_string()
        } else if requesting {
            "Preparing".to_string()
        } else {
            upload_label(status, upload, crate::ui::now_ms())
        },
        &["caption"],
    ));
    line
}

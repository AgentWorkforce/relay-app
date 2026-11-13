//! Settings scrolling column layout.

use super::*;

/// The settings pane's scrolling column.
pub struct SettingsPage {
    pub scroller: gtk::ScrolledWindow,
    pub column: gtk::Box,
    pub(super) holder: widgets::Column,
}

impl SettingsPage {
    pub fn build() -> SettingsPage {
        let (column, holder) = widgets::column(776);
        super::super::zoom::spacing(&column, 28);
        widgets::pad(&column, 28, None, None);
        column.add_css_class("settings-content");
        SettingsPage {
            scroller: widgets::scrolled(&holder),
            holder,
            column,
        }
    }
}

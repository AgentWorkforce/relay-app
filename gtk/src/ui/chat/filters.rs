//! The Chat list's filters: a search over names and summaries, then status,
//! harness and project. Each change redraws the list.

use super::super::widgets::{self, hbox, vbox};
use super::{filters_from, status_labels, ANY};
use crate::models::ChatFilters;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct FilterBar {
    pub root: gtk::Box,
    search: gtk::SearchEntry,
    status: gtk::DropDown,
    harness: gtk::DropDown,
    project: gtk::DropDown,
    /// The harnesses and projects offered now, after the "any" choice.
    offered: RefCell<(Vec<String>, Vec<String>)>,
    /// Set while the choices are replaced, so that is not a change.
    offering: Rc<Cell<bool>>,
}

fn dropdown(choices: &[&str], tooltip: &str) -> gtk::DropDown {
    let dropdown = gtk::DropDown::from_strings(choices);
    dropdown.set_tooltip_text(Some(tooltip));
    dropdown.add_css_class("chat-filter");
    dropdown.set_hexpand(true);
    widgets::name(&dropdown, tooltip);
    dropdown
}

impl FilterBar {
    pub fn build(placeholder: &str) -> FilterBar {
        let root = vbox(6, &[]);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(placeholder));
        widgets::name(&search, placeholder);
        root.append(&search);
        let status = dropdown(&status_labels(), "Status");
        let harness = dropdown(&[ANY[0]], "Harness");
        let project = dropdown(&[ANY[1]], "Project");
        let row = hbox(6, &[]);
        row.append(&status);
        row.append(&harness);
        root.append(&row);
        root.append(&project);
        FilterBar {
            root,
            search,
            status,
            harness,
            project,
            offered: RefCell::default(),
            offering: Rc::new(Cell::new(false)),
        }
    }

    pub fn connect_changed(&self, changed: impl Fn() + 'static) {
        let changed = Rc::new(changed);
        let run = changed.clone();
        self.search.connect_search_changed(move |_| run());
        for dropdown in [&self.status, &self.harness, &self.project] {
            let run = changed.clone();
            let offering = self.offering.clone();
            dropdown.connect_selected_notify(move |_| {
                if !offering.get() {
                    run();
                }
            });
        }
    }

    /// Offers `harnesses` and `projects`, keeping what is chosen when it is
    /// still offered.
    pub fn offer(&self, harnesses: &[String], projects: &[String]) {
        let offered = (harnesses.to_vec(), projects.to_vec());
        if *self.offered.borrow() == offered {
            return;
        }
        let chosen = (self.chosen(&self.harness, 0), self.chosen(&self.project, 1));
        self.offering.set(true);
        for (dropdown, any, choices, keep) in [
            (&self.harness, ANY[0], harnesses, chosen.0),
            (&self.project, ANY[1], projects, chosen.1),
        ] {
            let mut labels = vec![any];
            labels.extend(choices.iter().map(String::as_str));
            dropdown.set_model(Some(&gtk::StringList::new(&labels)));
            let at = keep
                .and_then(|keep| choices.iter().position(|choice| *choice == keep))
                .map_or(0, |at| at as u32 + 1);
            dropdown.set_selected(at);
        }
        *self.offered.borrow_mut() = offered;
        self.offering.set(false);
    }

    /// The choice in `dropdown` (0 harness, 1 project), `None` for "any".
    fn chosen(&self, dropdown: &gtk::DropDown, which: usize) -> Option<String> {
        let at = dropdown.selected() as usize;
        let offered = self.offered.borrow();
        let list = if which == 0 { &offered.0 } else { &offered.1 };
        at.checked_sub(1).and_then(|at| list.get(at).cloned())
    }

    pub fn read(&self) -> ChatFilters {
        filters_from(
            self.status.selected(),
            self.chosen(&self.harness, 0),
            self.chosen(&self.project, 1),
            self.search.text().to_string(),
        )
    }
}

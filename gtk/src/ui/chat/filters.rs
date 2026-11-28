//! The Chat list's filters: a search over names and summaries, then status,
//! harness, project and machine. Each change redraws the list.

use super::super::widgets::{self, hbox, vbox};
use crate::models::{
    ChatFilters, ChatStatusFilter, CHAT_ANY_HARNESS, CHAT_ANY_MACHINE, CHAT_ANY_PROJECT,
};
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
    machine: gtk::DropDown,
    /// The harnesses, projects and machines offered now, after the "any"
    /// choice.
    offered: RefCell<[Vec<String>; 3]>,
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
        let machine = dropdown(&[ANY[2]], "Machine");
        for pair in [[&status, &harness], [&project, &machine]] {
            let row = hbox(6, &[]);
            row.set_homogeneous(true);
            for dropdown in pair {
                row.append(dropdown);
            }
            root.append(&row);
        }
        FilterBar {
            root,
            search,
            status,
            harness,
            project,
            machine,
            offered: RefCell::default(),
            offering: Rc::new(Cell::new(false)),
        }
    }

    pub fn connect_changed(&self, changed: impl Fn() + 'static) {
        let changed = Rc::new(changed);
        let run = changed.clone();
        self.search.connect_search_changed(move |_| run());
        for dropdown in [&self.status, &self.harness, &self.project, &self.machine] {
            let run = changed.clone();
            let offering = self.offering.clone();
            dropdown.connect_selected_notify(move |_| {
                if !offering.get() {
                    run();
                }
            });
        }
    }

    /// Offers `harnesses`, `projects` and `machines`, keeping what is
    /// chosen when it is still offered.
    pub fn offer(&self, harnesses: &[String], projects: &[String], machines: &[String]) {
        let offered = [harnesses.to_vec(), projects.to_vec(), machines.to_vec()];
        if *self.offered.borrow() == offered {
            return;
        }
        let dropdowns = self.choices();
        let chosen = dropdowns.map(|(dropdown, which)| self.chosen(dropdown, which));
        self.offering.set(true);
        for ((dropdown, which), keep) in dropdowns.into_iter().zip(chosen) {
            let choices = &offered[which];
            let mut labels = vec![ANY[which]];
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

    /// The dropdowns with choices offered, each with its place in `offered`.
    fn choices(&self) -> [(&gtk::DropDown, usize); 3] {
        [(&self.harness, 0), (&self.project, 1), (&self.machine, 2)]
    }

    /// The choice in `dropdown` (`which` in `offered`), `None` for "any".
    fn chosen(&self, dropdown: &gtk::DropDown, which: usize) -> Option<String> {
        let at = dropdown.selected() as usize;
        at.checked_sub(1)
            .and_then(|at| self.offered.borrow()[which].get(at).cloned())
    }

    pub fn read(&self) -> ChatFilters {
        let [harness, project, machine] = self
            .choices()
            .map(|(dropdown, which)| self.chosen(dropdown, which));
        ChatFilters {
            status: ChatStatusFilter::ALL
                .get(self.status.selected() as usize)
                .copied()
                .unwrap_or(ChatStatusFilter::Running),
            harness,
            project,
            machine,
            query: self.search.text().to_string(),
        }
    }
}

/// The status filter's labels, in `ChatStatusFilter::ALL`'s order.
fn status_labels() -> Vec<&'static str> {
    ChatStatusFilter::ALL
        .iter()
        .map(|status| status.label())
        .collect()
}

const ANY: [&str; 3] = [CHAT_ANY_HARNESS, CHAT_ANY_PROJECT, CHAT_ANY_MACHINE];

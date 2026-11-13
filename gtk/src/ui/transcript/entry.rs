use super::{
    append_markdown, code_view, hbox, source_name, text, transcript_entry_context, vbox, widgets,
    LabelStyle, LocalTranscriptEntry,
};
use gtk::prelude::*;
use gtk4 as gtk;
use std::{cell::Cell, rc::Rc};

fn attachment_view(path: &str) -> gtk::Expander {
    let disclosure = gtk::Expander::new(Some("Attached image"));
    disclosure.set_widget_name("transcript-attached-image");
    disclosure.add_css_class("transcript-tool-group");
    widgets::name(&disclosure, "Attached image");
    let path = path.to_string();
    disclosure.connect_expanded_notify(move |disclosure| {
        if disclosure.is_expanded() {
            let picture = gtk::Picture::for_filename(&path);
            picture.set_can_shrink(true);
            picture.set_keep_aspect_ratio(true);
            picture.set_size_request(-1, 320);
            picture.set_alternative_text(Some("Attached image"));
            disclosure.set_child(Some(&picture));
        } else {
            disclosure.set_child(gtk::Widget::NONE);
        }
    });
    disclosure
}

pub(in crate::ui) fn entry_view(
    entry: &LocalTranscriptEntry,
    source: &str,
    person_name: &str,
    avatar_url: Option<&str>,
    table_cells_remaining: &mut usize,
) -> gtk::Widget {
    let context = transcript_entry_context(entry);
    if context || entry.kind != "text" || entry.role == "tool_result" {
        return activity_view(entry, context, table_cells_remaining);
    }
    if entry.text.is_empty() {
        if let Some(path) = &entry.image_path {
            return attachment_view(path).upcast();
        }
    }
    message_view(
        entry,
        source,
        person_name,
        avatar_url,
        table_cells_remaining,
    )
}

fn activity_view(
    entry: &LocalTranscriptEntry,
    context: bool,
    table_cells_remaining: &mut usize,
) -> gtk::Widget {
    let body = vbox(8, &[]);
    let title = if context {
        "System prompt"
    } else {
        match entry.kind.as_str() {
            "thinking" => "Thinking",
            "context" => "Session context",
            "tool_use" => "Tool call",
            _ => "Tool result",
        }
    };
    let disclosure = gtk::Expander::new(None);
    disclosure.add_css_class(if context {
        "transcript-tool-group"
    } else {
        "transcript-activity"
    });
    if context {
        disclosure.set_widget_name("transcript-system-prompt");
    }
    let heading = hbox(10, &[]);
    heading.append(&text(title, &["caption"]));
    let preview = text(&entry.text.replace('\n', " "), &["tertiary", "caption"]).ellipsized();
    preview.set_hexpand(true);
    if !context {
        heading.append(&preview);
    }
    disclosure.set_label_widget(Some(&heading));
    disclosure.set_tooltip_text(Some(&format!(
        "Expand or collapse {}",
        title.to_lowercase()
    )));
    widgets::name(&disclosure, title);
    let actions = hbox(0, &[]);
    actions.append(&widgets::spacer());
    actions.append(&copy_button(&entry.text));
    body.append(&actions);
    if entry.kind == "thinking" {
        append_markdown(&body, &entry.text, table_cells_remaining);
    } else {
        body.append(&code_view(&entry.text));
    }
    append_truncation(&body, entry);
    disclosure.set_child(Some(&body));
    disclosure.upcast()
}

fn message_view(
    entry: &LocalTranscriptEntry,
    source: &str,
    person_name: &str,
    avatar_url: Option<&str>,
    table_cells_remaining: &mut usize,
) -> gtk::Widget {
    let body = vbox(8, &[]);
    let user = entry.role == "user";
    if user {
        body.add_css_class("transcript-user");
        body.set_margin_start(48);
    }
    body.append(&message_header(
        &body,
        entry,
        source,
        person_name,
        avatar_url,
    ));
    if let Some(path) = &entry.image_path {
        body.append(&attachment_view(path));
    }
    if !entry.text.is_empty() || entry.image_path.is_none() {
        append_markdown(
            &body,
            if entry.text.is_empty() {
                "No text recorded."
            } else {
                &entry.text
            },
            table_cells_remaining,
        );
    }
    append_truncation(&body, entry);
    body.upcast()
}

fn message_header(
    body: &gtk::Box,
    entry: &LocalTranscriptEntry,
    source: &str,
    person_name: &str,
    avatar_url: Option<&str>,
) -> gtk::Box {
    let user = entry.role == "user";
    let header = hbox(8, &[]);
    if user {
        header.append(&widgets::avatar(person_name, avatar_url, 24));
    } else {
        header.append(&widgets::agent_badge(source, 16));
    }
    header.append(&text(
        &if user {
            person_name.into()
        } else if source == "claude" {
            "Claude Code".into()
        } else {
            source_name(source)
        },
        &["transcript-author"],
    ));
    if let Some(date) = chrono::DateTime::from_timestamp_millis(entry.timestamp_ms) {
        header.append(&text(
            &date
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string(),
            &["caption", "tertiary"],
        ));
    }
    header.append(&widgets::spacer());
    let copy = copy_button(&entry.text);
    copy.set_opacity(0.0);
    let hovering = Rc::new(Cell::new(false));
    let motion = gtk::EventControllerMotion::new();
    let hovered = hovering.clone();
    let action = copy.clone();
    motion.connect_enter(move |_, _, _| {
        hovered.set(true);
        action.set_opacity(1.0);
    });
    let hovered = hovering.clone();
    let action = copy.clone();
    motion.connect_leave(move |_| {
        hovered.set(false);
        action.set_opacity(if action.has_focus() { 1.0 } else { 0.0 });
    });
    copy.connect_has_focus_notify(move |action| {
        action.set_opacity(if action.has_focus() || hovering.get() {
            1.0
        } else {
            0.0
        })
    });
    body.add_controller(motion);
    header.append(&copy);
    header
}

fn append_truncation(body: &gtk::Box, entry: &LocalTranscriptEntry) {
    if entry.truncated {
        body.append(
            &text(
                "Long message shortened. Open the original session for the full output.",
                &["caption"],
            )
            .wrapped(),
        );
    }
}

pub(super) fn copy_button(value: &str) -> gtk::Button {
    let value = value.to_string();
    let copy = widgets::button("", &["flat"], move || {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&value);
        }
    });
    copy.set_icon_name("edit-copy-symbolic");
    copy.set_tooltip_text(Some("Copy message"));
    widgets::name(&copy, "Copy message");
    copy
}

use super::{code_view, copy_button, hbox, text, vbox, widgets, LabelStyle};
use gtk::prelude::*;
use gtk4 as gtk;

/// Native selectable Markdown; raw HTML is always escaped.
pub(super) fn append_markdown(body: &gtk::Box, markdown: &str, table_cells_remaining: &mut usize) {
    let mut lines = markdown.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(headers) = table_header(line, lines.peek().copied()) {
            append_table(body, line, headers, &mut lines, table_cells_remaining);
        } else if line.trim_start().starts_with("```") {
            append_code_block(body, line, &mut lines);
        } else if !line.trim().is_empty() {
            append_markdown_line(body, line);
        }
    }
}

fn table_header(line: &str, separator: Option<&str>) -> Option<Vec<String>> {
    if !line.contains('|') {
        return None;
    }
    let headers = table_cells(line);
    let cells = table_cells(separator?);
    (cells.len() == headers.len()
        && cells.iter().all(|cell| {
            let marker = cell.trim_matches(':');
            marker.len() >= 3 && marker.chars().all(|character| character == '-')
        }))
    .then_some(headers)
}

fn append_table<'a>(
    body: &gtk::Box,
    header: &'a str,
    headers: Vec<String>,
    lines: &mut std::iter::Peekable<std::str::Lines<'a>>,
    remaining: &mut usize,
) {
    let Some(separator) = lines.next() else {
        return;
    };
    let mut raw = vec![header, separator];
    while let Some(line) = lines.next_if(|line| line.contains('|') && !line.trim().is_empty()) {
        raw.push(line);
    }
    let columns = headers.len();
    let Some(rows) = bounded_table_rows(headers, &raw[2..], remaining) else {
        let label = text(&raw.join("\n"), &["transcript-text"]).wrapped();
        label.set_selectable(true);
        body.append(&label);
        return;
    };
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(8);
    for (row, cells) in rows.iter().enumerate() {
        for column in 0..columns {
            let label = text("", &["transcript-text"]).wrapped();
            label.set_selectable(true);
            label.set_max_width_chars(40);
            let value = inline_markup(cells.get(column).map(String::as_str).unwrap_or(""));
            label.set_markup(&if row == 0 {
                format!("<b>{value}</b>")
            } else {
                value
            });
            label.set_valign(gtk::Align::Start);
            grid.attach(&label, column as i32, row as i32, 1, 1);
        }
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroll.set_child(Some(&grid));
    body.append(&scroll);
}

fn append_code_block<'a>(
    body: &gtk::Box,
    fence: &str,
    lines: &mut std::iter::Peekable<std::str::Lines<'a>>,
) {
    let language = fence.trim_start().trim_start_matches('`').trim();
    let mut code = Vec::new();
    for line in lines.by_ref() {
        if line.trim_start().starts_with("```") {
            break;
        }
        code.push(line);
    }
    let code = code.join("\n");
    let block = vbox(6, &["transcript-code-block"]);
    let header = hbox(8, &[]);
    header.append(&text(
        if language.is_empty() {
            "Code"
        } else {
            language
        },
        &["caption"],
    ));
    header.append(&widgets::spacer());
    let copy = copy_button(&code);
    copy.set_tooltip_text(Some("Copy code"));
    widgets::name(&copy, "Copy code");
    header.append(&copy);
    block.append(&header);
    block.append(&code_view(&code));
    body.append(&block);
}

fn append_markdown_line(body: &gtk::Box, line: &str) {
    let trimmed = line.trim_start();
    let heading = trimmed.chars().take_while(|c| *c == '#').count();
    let (value, class) =
        if heading > 0 && heading <= 6 && trimmed.as_bytes().get(heading) == Some(&b' ') {
            (&trimmed[heading + 1..], "transcript-heading")
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            (quote, "transcript-quote")
        } else {
            (line, "transcript-text")
        };
    let label = text("", &[class]).wrapped();
    if class == "transcript-heading" {
        match heading {
            1 => label.add_css_class("transcript-heading-one"),
            2 => label.add_css_class("transcript-heading-two"),
            _ => {}
        }
    }
    label.set_selectable(true);
    label.set_markup(&inline_markup(value));
    body.append(&label);
}

// A caller-owned conversation budget bounds table widgets across entries.
// Oversized tables remain readable as a single text label.
fn bounded_table_rows(
    headers: Vec<String>,
    rows: &[&str],
    remaining: &mut usize,
) -> Option<Vec<Vec<String>>> {
    if headers.len() > 32 || rows.len() > 200 {
        return None;
    }
    let cells = headers.len() * (rows.len() + 1);
    if cells > *remaining {
        return None;
    }
    *remaining -= cells;
    Some(
        std::iter::once(headers)
            .chain(rows.iter().map(|line| table_cells(line)))
            .collect(),
    )
}

fn table_cells(line: &str) -> Vec<String> {
    let text = line.trim().strip_prefix('|').unwrap_or(line.trim());
    let text = if text.ends_with('|') && !text.ends_with("\\|") {
        &text[..text.len() - 1]
    } else {
        text
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            if character != '|' && character != '\\' {
                cell.push('\\');
            }
            cell.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(cell.trim().to_string());
            cell.clear();
        } else {
            cell.push(character);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().to_string());
    cells
}

fn inline_markup(value: &str) -> String {
    let mut result = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        if let Some(link) = rest.strip_prefix('[') {
            if let Some((label, tail)) = link.split_once("](") {
                if let Some(end) = tail.find(')') {
                    let href = &tail[..end];
                    if url::Url::parse(href)
                        .is_ok_and(|url| matches!(url.scheme(), "https" | "http" | "mailto"))
                    {
                        result.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            gtk::glib::markup_escape_text(href),
                            gtk::glib::markup_escape_text(label)
                        ));
                        rest = &tail[end + 1..];
                        continue;
                    }
                }
            }
        }
        let token = [("**", "b"), ("`", "tt"), ("*", "i")]
            .into_iter()
            .find_map(|(marker, tag)| {
                let tail = rest.strip_prefix(marker)?;
                let end = tail.find(marker)?;
                (end > 0).then_some((marker, tag, tail, end))
            });
        if let Some((marker, tag, tail, end)) = token {
            result.push_str(&format!(
                "<{tag}>{}</{tag}>",
                gtk::glib::markup_escape_text(&tail[..end])
            ));
            rest = &tail[end + marker.len()..];
        } else {
            let Some(character) = rest.chars().next() else {
                break;
            };
            let length = character.len_utf8();
            result.push_str(&gtk::glib::markup_escape_text(&rest[..length]));
            rest = &rest[length..];
        }
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn oversized_markdown_tables_never_expand_into_widget_matrices() {
        for (columns, count) in [(33, 1), (2, 201), (999, 30000), (32, 64)] {
            assert!(super::bounded_table_rows(
                vec!["h".into(); columns],
                &vec!["|"; count],
                &mut 2048
            )
            .is_none());
        }
        let rows =
            super::bounded_table_rows(vec!["h".into(); 32], &vec!["|"; 63], &mut 2048).unwrap();
        assert_eq!(rows.len(), 64);
        assert_eq!(rows[0].len(), 32);
    }

    #[test]
    fn repeated_tables_respect_the_supplied_cell_budget() {
        let mut remaining = 2048;
        let accepted = (0..4)
            .filter(|_| {
                super::bounded_table_rows(vec!["h".into(); 32], &vec!["|"; 16], &mut remaining)
                    .is_some()
            })
            .count();
        assert_eq!(accepted, 3);
        assert_eq!(2048 - remaining, 1632);
    }

    #[test]
    fn tables_keep_escaped_pipes_and_aligned_separators() {
        assert_eq!(
            super::table_cells(r"| `a\|b` | **Ready** |"),
            vec!["`a|b`", "**Ready**"]
        );
        assert_eq!(
            super::table_header("| key | note |", Some("| --- | :---: |")),
            Some(vec!["key".into(), "note".into()])
        );
    }

    #[test]
    fn markdown_is_readable_and_cannot_inject_markup() {
        assert_eq!(
            super::inline_markup("**Bold** and `a < b` & *fine*"),
            "<b>Bold</b> and <tt>a &lt; b</tt> &amp; <i>fine</i>"
        );
        assert_eq!(
            super::inline_markup("[Docs](https://example.com/?a=1&b=2)"),
            "<a href=\"https://example.com/?a=1&amp;b=2\">Docs</a>"
        );
        assert!(!super::inline_markup("[Run](javascript:alert(1))").contains("<a "));
        assert_eq!(
            super::inline_markup("<span>🦀</span>"),
            "&lt;span&gt;🦀&lt;/span&gt;"
        );
    }
}

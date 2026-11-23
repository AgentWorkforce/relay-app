//! Images pasted into a session: Claude Code records each as a text line,
//! `[Image: source: <path>]`, and marks where it sat in the message with
//! `[Image #N]`. Chat draws the image itself, not either line.

/// The paths of the images `text` records, and `text` without those lines
/// and without the `[Image #N]` marks.
pub fn chat_pasted_images(text: &str) -> (String, Vec<String>) {
    const SOURCE: &str = "[Image: source: ";
    let mut images = Vec::new();
    let mut rest = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(at) = remaining.find(SOURCE) {
        let after = &remaining[at + SOURCE.len()..];
        let Some(end) = after.find(']') else {
            break;
        };
        rest.push_str(&remaining[..at]);
        let path = after[..end].trim();
        if !path.is_empty() {
            images.push(path.to_string());
        }
        remaining = &after[end + 1..];
    }
    rest.push_str(remaining);
    let rest = without_notes(&rest);
    (without_marks(&rest).trim().to_string(), images)
}

/// `text` without Claude Code's notes on an image it read
/// (`[Image: original 3804x2085, displayed at …]`): they are for the
/// model, not something anyone wrote.
fn without_notes(text: &str) -> String {
    const NOTE: &str = "[Image: original ";
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(at) = remaining.find(NOTE) {
        out.push_str(&remaining[..at]);
        let after = &remaining[at..];
        match after.find(']') {
            Some(end) => remaining = &after[end + 1..],
            None => {
                remaining = after;
                break;
            }
        }
    }
    out.push_str(remaining);
    out
}

/// `text` without its `[Image #N]` marks.
fn without_marks(text: &str) -> String {
    const MARK: &str = "[Image #";
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(at) = remaining.find(MARK) {
        let after = &remaining[at + MARK.len()..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            out.push_str(&remaining[..at]);
            remaining = after[digits + 1..].trim_start_matches(' ');
        } else {
            out.push_str(&remaining[..at + MARK.len()]);
            remaining = after;
        }
    }
    out.push_str(remaining);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_image_is_its_path() {
        let (text, images) = chat_pasted_images("[Image: source: /tmp/shots/7.png]");
        assert_eq!(
            (text.as_str(), images),
            ("", vec!["/tmp/shots/7.png".to_string()])
        );
        let (text, images) = chat_pasted_images("[Image #7] this is too wide");
        assert_eq!((text.as_str(), images.len()), ("this is too wide", 0));
        let note = "[Image: original 3804x2085, displayed at 2000x1096. Multiply coordinates by 1.90 to map to original image.]";
        assert_eq!(chat_pasted_images(note), (String::new(), Vec::new()));
        let (text, _) = chat_pasted_images("see [Image #x] and a[1]");
        assert_eq!(text, "see [Image #x] and a[1]");
    }
}

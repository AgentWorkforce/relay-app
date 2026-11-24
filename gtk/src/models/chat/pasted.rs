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

/// The images a relayed message came with, and `text` without the lines
/// that name them. The core saves each file it receives into `saved` and
/// lists it under `Attachments:` as `- <name> (image/<type>, <size>) saved
/// to <path>`. The block is plain message text anyone can type, so only a
/// path inside `saved` is drawn; any other line stays text, and so does a
/// file that was not saved here.
pub fn chat_attached_images(text: &str, saved: &std::path::Path) -> (String, Vec<String>) {
    const HEADER: &str = "Attachments:";
    let Some(at) = text.find(HEADER) else {
        return (text.to_string(), Vec::new());
    };
    let mut images = Vec::new();
    let mut kept = Vec::new();
    for line in text[at + HEADER.len()..].lines() {
        match attached_image(line, saved) {
            Some(path) => images.push(path),
            None => kept.push(line),
        }
    }
    if images.is_empty() {
        return (text.to_string(), images);
    }
    let rest = kept.join("\n");
    let mut out = text[..at].trim_end().to_string();
    if !rest.trim().is_empty() {
        out.push_str("\n");
        out.push_str(HEADER);
        out.push_str(&rest);
    }
    (out.trim().to_string(), images)
}

/// The saved image `line` names, when it is one inside `saved`.
fn attached_image(line: &str, saved: &std::path::Path) -> Option<String> {
    const SAVED: &str = ") saved to ";
    let line = line.trim().strip_prefix("- ")?;
    let (described, path) = line.rsplit_once(SAVED)?;
    let kind = described.rsplit_once(" (")?.1;
    let image = ["image/png", "image/jpeg", "image/gif", "image/webp"]
        .iter()
        .any(|mime| kind.starts_with(&format!("{mime},")));
    let path = std::path::Path::new(path.trim());
    let inside = path.is_absolute()
        && path.starts_with(saved)
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir));
    (image && inside).then(|| path.display().to_string())
}

/// The lines that carry `files` in a message to an agent on this computer,
/// which can open them where they are: an image as Claude Code records a
/// pasted one (`[Image: source: <path>]`, drawn inline here), anything else
/// as `[File: <path>]`.
pub fn chat_attachment_lines(files: &[std::path::PathBuf]) -> String {
    files
        .iter()
        .map(|path| {
            let image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp"
                )
            });
            let kind = if image { "Image: source" } else { "File" };
            format!("[{kind}: {}]", path.display())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_go_as_lines_an_agent_here_can_open() {
        let lines = chat_attachment_lines(&["/tmp/a/shot.PNG".into(), "/tmp/a/notes.txt".into()]);
        assert_eq!(
            lines,
            "[Image: source: /tmp/a/shot.PNG]\n[File: /tmp/a/notes.txt]"
        );
        // The image line is drawn as the image here.
        assert_eq!(
            chat_pasted_images(&lines).1,
            vec!["/tmp/a/shot.PNG".to_string()]
        );
    }

    #[test]
    fn a_saved_attachment_is_drawn_and_its_line_goes() {
        let saved = std::path::Path::new("/data/attachments");
        let text = "look at this\n\nAttachments:\n- Screenshot 21.38.png (image/png, 352.8 KB) saved to /data/attachments/250858/Screenshot 21.38.png";
        let (body, images) = chat_attached_images(text, saved);
        assert_eq!(body, "look at this");
        assert_eq!(images, ["/data/attachments/250858/Screenshot 21.38.png"]);
        // What is not a saved image here stays text.
        for line in [
            "- notes.txt (text/plain, 1 KB) saved to /data/attachments/1/notes.txt",
            "- x.png (image/png, 1 KB) saved to /home/me/.ssh/x.png",
            "- x.png (image/png, 1 KB) saved to /data/attachments/../secret.png",
            "- x.png (image/png, 1 KB) file f1 (not downloaded: too big); fetch with: …",
        ] {
            let text = format!("hi\nAttachments:\n{line}");
            assert_eq!(
                chat_attached_images(&text, saved),
                (text.clone(), Vec::new())
            );
        }
        // An image among other files: the others stay listed.
        let text = "Attachments:\n- a.png (image/png, 1 KB) saved to /data/attachments/1/a.png\n- b.txt (text/plain, 1 KB) saved to /data/attachments/2/b.txt";
        let (body, images) = chat_attached_images(text, saved);
        assert_eq!(
            body,
            "Attachments:\n- b.txt (text/plain, 1 KB) saved to /data/attachments/2/b.txt"
        );
        assert_eq!(images.len(), 1);
    }

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

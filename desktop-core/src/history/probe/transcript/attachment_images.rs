//! Images a relayed message brought. The relay core saves a message's files
//! under its attachments directory and ends the delivered text with an
//! `Attachments:` block naming them. The reader draws the saved images of a
//! person's turn from that block; it only checks the files, it never reads
//! or copies them.
use agent_relay_node::relay::files::{ATTACHMENTS_DIR, MAX_FILE_BYTES};
use std::path::{Component, Path, PathBuf};

const HEADING: &str = "Attachments:";
const SAVED: &str = ") saved to ";
/// The line that closes a framed message. A block above one sits inside a
/// message body, or the turn carries several messages; neither is drawn.
const FRAME_END: &str = "[end of Agent Relay message · ref ";
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// The running core's attachments directory: the data directory the core
/// launched this reader with, else the default one.
pub(super) fn root() -> PathBuf {
    crate::history::retention::app_data_dir().join(ATTACHMENTS_DIR)
}

/// A turn's text without the image lines it drew, and the images in order.
#[derive(Debug, PartialEq)]
pub(super) struct Pictures {
    pub(super) text: String,
    pub(super) images: Vec<String>,
}

/// None when the turn has no block or none of its lines is a saved image.
pub(super) fn pictures(text: &str, root: &Path) -> Option<Pictures> {
    let lines: Vec<&str> = text.split('\n').collect();
    let heading = (0..lines.len())
        .rev()
        .find(|&i| lines[i] == HEADING && (i == 0 || lines[i - 1].is_empty()))?;
    let end = (heading + 1..lines.len())
        .find(|&i| !lines[i].starts_with("- "))
        .unwrap_or(lines.len());
    if end == heading + 1 || lines[end..].iter().any(|l| l.starts_with(FRAME_END)) {
        return None;
    }
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let mut images = Vec::new();
    let mut kept = Vec::new();
    for line in &lines[heading + 1..end] {
        match saved_image(line, root, &canonical_root) {
            Some(path) => images.push(path),
            None => kept.push(*line),
        }
    }
    if images.is_empty() {
        return None;
    }
    let mut out: Vec<&str> = lines[..heading].to_vec();
    if kept.is_empty() {
        if heading > 0 {
            out.pop();
        }
    } else {
        out.push(HEADING);
        out.extend(kept);
    }
    out.extend(&lines[end..]);
    Some(Pictures {
        text: out.join("\n").trim().to_string(),
        images,
    })
}

/// `- <name> (<type>[, <size>]) saved to <path>`, read from the right: a
/// name may hold spaces, parentheses and ` saved to `, a saved file's own
/// name holds no path separator, so the path is the last absolute remainder.
fn saved_image(line: &str, root: &Path, canonical_root: &Path) -> Option<String> {
    let line = line.strip_prefix("- ")?;
    let (head, path) = line
        .rmatch_indices(SAVED)
        .map(|(at, _)| (&line[..at], &line[at + SAVED.len()..]))
        .find(|(_, path)| Path::new(path).is_absolute())?;
    let (name, details) = head.rsplit_once(" (")?;
    if name.is_empty() {
        return None;
    }
    let (kind, size) = match details.split_once(", ") {
        Some((kind, size)) => (kind, Some(size)),
        None => (details, None),
    };
    if !IMAGE_TYPES.contains(&kind) || !size.map_or(true, size_shape) {
        return None;
    }
    let path = Path::new(path);
    if !inside(path, root) {
        return None;
    }
    let resolved = std::fs::canonicalize(path).ok()?;
    if !resolved.starts_with(canonical_root) {
        return None;
    }
    let metadata = std::fs::metadata(&resolved).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_FILE_BYTES)
        .then(|| path.to_string_lossy().into_owned())
}

/// The core's size wording: `12 B`, `153.1 KB` or `2.0 MB`.
fn size_shape(size: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(bytes) = size.strip_suffix(" B") {
        return digits(bytes);
    }
    let Some(number) = size.strip_suffix(" KB").or_else(|| size.strip_suffix(" MB")) else {
        return false;
    };
    number
        .split_once('.')
        .is_some_and(|(whole, tenth)| digits(whole) && tenth.len() == 1 && digits(tenth))
}

/// Written as `<root>/<file id>/<name>`, with no `..` anywhere. Windows
/// compares names without regard to letter case and either separator.
fn inside(path: &Path, root: &Path) -> bool {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    let mut parts = path.components();
    for expected in root.components() {
        match parts.next() {
            Some(part) if same(part.as_os_str(), expected.as_os_str()) => {}
            _ => return false,
        }
    }
    let rest: Vec<Component> = parts.collect();
    rest.len() == 2 && rest.iter().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(windows)]
fn same(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(not(windows))]
fn same(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_size_and_name_from_the_right() {
        assert!(size_shape("4 B") && size_shape("153.1 KB") && size_shape("2.0 MB"));
        assert!(!size_shape("4") && !size_shape("1.25 KB") && !size_shape("x B"));
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("attachments");
        let file = root.join("f1").join("a (b) saved to c.png");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        let line = format!(
            "- a (b) saved to c.png (image/png, 1 B) saved to {}",
            file.display()
        );
        assert_eq!(
            saved_image(&line, &root, &canonical).as_deref(),
            file.to_str()
        );
        let typeless = format!("- a.png (1 B) saved to {}", file.display());
        assert_eq!(saved_image(&typeless, &root, &canonical), None);
        let nameless = format!("- (image/png) saved to {}", file.display());
        assert_eq!(saved_image(&nameless, &root, &canonical), None);
    }

    #[test]
    fn a_path_is_inside_only_as_root_file_id_name() {
        let root = Path::new(if cfg!(windows) { r"C:\data\attachments" } else { "/data/attachments" });
        let ok = root.join("f1").join("a.png");
        assert!(inside(&ok, root));
        assert!(!inside(&root.join("a.png"), root));
        assert!(!inside(&root.join("f1").join("x").join("a.png"), root));
        assert!(!inside(&root.join("f1").join("..").join("a.png"), root));
        assert!(!inside(Path::new("attachments/f1/a.png"), root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case_and_take_either_separator() {
        let root = Path::new(r"C:\Users\x\AppData\attachments");
        assert!(inside(Path::new(r"c:\users\X\appdata\Attachments\f1\a.png"), root));
        assert!(inside(Path::new("C:/Users/x/AppData/attachments/f1/a.png"), root));
        assert!(!inside(Path::new(r"D:\Users\x\AppData\attachments\f1\a.png"), root));
    }
}

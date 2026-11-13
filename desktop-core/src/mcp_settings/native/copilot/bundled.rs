//! Installation lookup only. Delivery still requires the core's independent
//! signed-process and native held-session proof.
use super::*;

pub(super) fn candidates(vscode: &Path, snap: Option<&Path>) -> Vec<PathBuf> {
    let resolved = fs::canonicalize(vscode).unwrap_or_else(|_| vscode.to_owned());
    let mut paths = Vec::new();
    if vscode.ends_with("snap/bin/code") || resolved.file_name().is_some_and(|name| name == "snap")
    {
        paths.extend(snap.map(Path::to_owned));
    }
    for executable in [vscode, resolved.as_path()] {
        if let Some(app) = executable
            .ancestors()
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        {
            paths.push(app.join("Contents/Resources/app/extensions/copilot"));
        } else if let Some(parent) = executable.parent() {
            paths.push(parent.join("resources/app/extensions/copilot"));
            if let Some(base) = parent.parent() {
                paths.push(base.join("resources/app/extensions/copilot"));
                paths.push(base.join("extensions/copilot"));
            }
        }
    }
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}
pub(super) fn is_chat_extension(path: &Path) -> bool {
    let package = path.join("package.json");
    if !fs::metadata(&package).is_ok_and(|m| m.is_file() && m.len() <= 2 * 1024 * 1024) {
        return false;
    }
    fs::read(&package)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value["publisher"] == "GitHub" && value["name"] == "copilot-chat")
}

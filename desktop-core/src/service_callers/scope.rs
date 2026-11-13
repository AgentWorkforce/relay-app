//! Harness and folder scope (`docs/service-callers.md` section 7).

use super::registry::Root;
use super::Class;
use agent_relay_node::relay::harness::HarnessId;
use std::path::{Component, Path, PathBuf};

/// A non-empty list of known harness names, without repeats.
pub(crate) fn harnesses(given: &[String]) -> Result<Vec<String>, Class> {
    if given.is_empty() {
        return Err(Class::InvalidInput);
    }
    let mut kept: Vec<String> = Vec::new();
    for name in given {
        let harness = HarnessId::from_name(name).ok_or(Class::InvalidInput)?;
        let name = harness.name().to_string();
        if !kept.contains(&name) {
            kept.push(name);
        }
    }
    Ok(kept)
}

/// Absolute, existing folders, each resolved. None is the filesystem root,
/// the home folder or a folder that contains it, or the app data folder, a
/// folder inside it or one that contains it: a root never spans the
/// person's whole home or Desktop's own files.
pub(crate) fn roots(
    given: &[String],
    home: Option<&Path>,
    app_data: &Path,
) -> Result<Vec<Root>, Class> {
    if given.is_empty() {
        return Err(Class::InvalidInput);
    }
    let home = home.and_then(|home| std::fs::canonicalize(home).ok());
    let app_data = std::fs::canonicalize(app_data).unwrap_or_else(|_| app_data.to_path_buf());
    let mut kept: Vec<Root> = Vec::new();
    for root in given {
        let path = Path::new(root);
        if !path.is_absolute() || root.contains('\0') || has_dotdot(path) {
            return Err(Class::DirectoryOutside);
        }
        let resolved = std::fs::canonicalize(path).map_err(|_| Class::DirectoryOutside)?;
        if !resolved.is_dir()
            || resolved.parent().is_none()
            || home
                .as_deref()
                .is_some_and(|home| home.starts_with(&resolved))
            || resolved.starts_with(&app_data)
            || app_data.starts_with(&resolved)
        {
            return Err(Class::DirectoryOutside);
        }
        if !kept.iter().any(|kept| kept.resolved == resolved) {
            kept.push(Root {
                given: root.clone(),
                resolved,
            });
        }
    }
    Ok(kept)
}

fn has_dotdot(path: &Path) -> bool {
    path.components()
        .any(|part| matches!(part, Component::ParentDir))
}

/// The folder a spawn asks for, resolved, when it lies under a root that
/// still resolves and outside the app data folder. A `..` that climbs out
/// and a symbolic link that leads out both resolve outside every root; one
/// that leads into Desktop's own files is refused even under a root.
pub(crate) fn directory(
    requested: &str,
    roots: &[Root],
    app_data: &Path,
) -> Result<PathBuf, Class> {
    let path = Path::new(requested);
    if !path.is_absolute() || requested.contains('\0') {
        return Err(Class::DirectoryOutside);
    }
    let resolved = std::fs::canonicalize(path).map_err(|_| Class::DirectoryOutside)?;
    let app_data = std::fs::canonicalize(app_data).unwrap_or_else(|_| app_data.to_path_buf());
    if resolved.starts_with(&app_data) {
        return Err(Class::DirectoryOutside);
    }
    let inside = roots.iter().any(|root| {
        std::fs::canonicalize(&root.resolved)
            .is_ok_and(|now| now == root.resolved && resolved.starts_with(&now))
    });
    if inside {
        Ok(resolved)
    } else {
        Err(Class::DirectoryOutside)
    }
}

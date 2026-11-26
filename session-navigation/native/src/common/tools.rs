//! Tool resolution: the first executable file named for the tool on the
//! configured search path, and its version, cached per (path, mtime, size).
//!
//! - Most tools answer `<path> --version`.
//! - tmux answers `tmux -V` (`tmux 3.5a`).
//! - Grok installs itself as `grok-<X.Y.Z>-<os>-<arch>` behind a `grok`
//!   link; as relay does, the version comes from that canonical file name
//!   and Grok is not run. Only a name of another shape falls back to
//!   `<path> --version`.
use std::collections::HashMap;

use session_navigation::evidence::{Fact, ToolInfo};
use session_navigation::model::Tool;

use super::config::CommonConfig;
use super::parse;
use super::probe;
use crate::system::{join, Run, System};

type VersionKey = (String, u64, u64);

/// How tool files are named and joined to their directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStyle {
    /// `dir/name`, an executable regular file (macOS, Linux, WSL).
    Posix,
    /// `dir\name.exe`, then `dir\name.cmd` (an npm shim); Windows has no
    /// execute bit, so any regular file counts. `.ps1` shims are never
    /// chosen, and `.bat` is not looked for.
    Windows,
}

/// The paths tried for `tool` in `dir`, in order.
pub fn candidates(dir: &str, tool: Tool, style: PathStyle) -> Vec<String> {
    match style {
        PathStyle::Posix => vec![join(dir, tool.program())],
        PathStyle::Windows => {
            let dir = dir.trim_end_matches(['\\', '/']);
            ["exe", "cmd"]
                .iter()
                .map(|ext| format!("{dir}\\{}.{ext}", tool.program()))
                .collect()
        }
    }
}

/// Remembers successful version probes across observations.
#[derive(Clone, Debug, Default)]
pub struct ToolResolver {
    versions: HashMap<VersionKey, Option<String>>,
}

/// The argv that makes `tool` print its version.
pub fn version_args(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Tmux => &["-V"],
        _ => &["--version"],
    }
}

impl ToolResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every tool in [`Tool::ALL`] order.
    pub fn all<S: System>(&mut self, sys: &S, cfg: &CommonConfig) -> Vec<ToolInfo> {
        Tool::ALL
            .into_iter()
            .map(|tool| self.tool(sys, cfg, tool))
            .collect()
    }

    /// [`Self::all`], with the version probes not cached yet run at once
    /// when the system allows it (a first look runs several `--version`).
    /// One after another otherwise, exactly as [`Self::all`].
    pub fn all_at_once<S: System + Sync>(&mut self, sys: &S, cfg: &CommonConfig) -> Vec<ToolInfo> {
        if !sys.concurrent() {
            return self.all(sys, cfg);
        }
        let found: Vec<(Tool, Option<(String, VersionKey)>)> = Tool::ALL
            .into_iter()
            .map(|tool| (tool, Self::find(sys, cfg, tool, PathStyle::Posix)))
            .collect();
        let missing: Vec<(Tool, String, VersionKey)> = found
            .iter()
            .filter_map(|(tool, f)| {
                let (path, key) = f.as_ref()?;
                (!self.versions.contains_key(key)).then(|| (*tool, path.clone(), key.clone()))
            })
            .collect();
        let probed = super::par::map(sys, missing, |(tool, path, key)| {
            (key, Self::version(sys, cfg, tool, &path))
        });
        let mut failed = Vec::new();
        for (key, version) in probed {
            match version {
                Some(v) => {
                    self.versions.insert(key, Some(v));
                }
                None => failed.push(key),
            }
        }
        found
            .into_iter()
            .map(|(tool, f)| match f {
                None => ToolInfo {
                    tool,
                    version: Fact::Known(None),
                    path: None,
                },
                Some((path, key)) => {
                    let version = if failed.contains(&key) {
                        None
                    } else {
                        self.versions.get(&key).cloned().flatten()
                    };
                    ToolInfo {
                        tool,
                        version: version.map_or(Fact::Unknown, |v| Fact::Known(Some(v))),
                        path: Some(path),
                    }
                }
            })
            .collect()
    }

    /// The first executable file for `tool` on the search path, with its
    /// version-cache key.
    fn find<S: System>(
        sys: &S,
        cfg: &CommonConfig,
        tool: Tool,
        style: PathStyle,
    ) -> Option<(String, VersionKey)> {
        cfg.paths.tool_search_path.iter().find_map(|dir| {
            candidates(dir, tool, style).into_iter().find_map(|path| {
                sys.metadata(&path)
                    .ok()
                    .filter(|m| m.is_file && (m.executable || style == PathStyle::Windows))
                    .map(|m| {
                        let key = (path.clone(), m.mtime, m.len);
                        (path, key)
                    })
            })
        })
    }

    pub fn tool<S: System>(&mut self, sys: &S, cfg: &CommonConfig, tool: Tool) -> ToolInfo {
        self.tool_in(sys, cfg, tool, PathStyle::Posix)
    }

    /// Every tool in [`Tool::ALL`] order, with `style`'s file names.
    pub fn all_in<S: System>(
        &mut self,
        sys: &S,
        cfg: &CommonConfig,
        style: PathStyle,
    ) -> Vec<ToolInfo> {
        Tool::ALL
            .into_iter()
            .map(|tool| self.tool_in(sys, cfg, tool, style))
            .collect()
    }

    /// [`Self::tool`] with `style`'s file names and separator.
    pub fn tool_in<S: System>(
        &mut self,
        sys: &S,
        cfg: &CommonConfig,
        tool: Tool,
        style: PathStyle,
    ) -> ToolInfo {
        let found = cfg.paths.tool_search_path.iter().find_map(|dir| {
            candidates(dir, tool, style).into_iter().find_map(|path| {
                sys.metadata(&path)
                    .ok()
                    .filter(|m| m.is_file && (m.executable || style == PathStyle::Windows))
                    .map(|m| (path, m))
            })
        });
        let Some((path, meta)) = found else {
            return ToolInfo {
                tool,
                version: Fact::Known(None),
                path: None,
            };
        };
        let key = (path.clone(), meta.mtime, meta.len);
        let version = match self.versions.get(&key) {
            Some(cached) => cached.clone(),
            None => {
                let version = Self::version(sys, cfg, tool, &path);
                if version.is_some() {
                    self.versions.insert(key, version.clone());
                }
                version
            }
        };
        ToolInfo {
            tool,
            version: version.map_or(Fact::Unknown, |v| Fact::Known(Some(v))),
            path: Some(path),
        }
    }

    fn version<S: System>(sys: &S, cfg: &CommonConfig, tool: Tool, path: &str) -> Option<String> {
        if tool == Tool::GrokCli {
            let named = sys.canonicalize(path).ok().and_then(|canonical| {
                let name = canonical.rsplit(['/', '\\']).next().unwrap_or(&canonical);
                parse::grok_version_from_name(name)
            });
            if named.is_some() {
                return named;
            }
        }
        let run = Run::new(path, version_args(tool))
            .timeout(cfg.timeouts.version)
            .max_output(64 * 1024);
        probe(sys, &run).and_then(|o| parse::tool_version(&o.stdout))
    }
}

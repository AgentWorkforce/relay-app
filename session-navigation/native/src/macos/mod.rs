//! The macOS adapter: the process table from kernel reads ([`kernel`]) or
//! `ps`/`sysctl`/`lsof`, the JXA app registry (reused briefly, with a fresh
//! front app from `lsappinfo`: [`registry`]), AppleScript focus and launch, and [`MacObserver`],
//! [`MacExecutor`] and [`MacPorts`] on top of [`crate::common`]. Plain logic
//! over [`crate::system::System`], so it builds and tests on any OS; only a
//! Mac runs it for real.
pub mod config;
pub mod execute;
pub mod hosts;
pub mod kernel;
pub mod machine;
pub mod observe;
pub mod parse;
pub mod ports;
pub mod registry;
pub mod scripts;

pub use config::{programs, provider_paths, HostConfig, MacConfig};
pub use execute::MacExecutor;
pub use machine::{CURL, LSOF, SQLITE};
pub use observe::MacObserver;
pub use ports::MacPorts;

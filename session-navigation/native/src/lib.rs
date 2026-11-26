//! Native adapters for `session-navigation`.
//!
//! - [`system`]: the only impure seam ([`system::System`]), with
//!   [`system::RealSystem`] (portable std) and [`system::FakeSystem`].
//! - [`common`]: everything OS-independent (provider observers for Claude,
//!   Codex, OpenCode and Grok, reservations, quoting, tool resolution, the
//!   provider-paths config).
//! - `macos`, `linux`, `windows` (one Cargo feature each, all default):
//!   the per-platform adapters. Only macOS is a full adapter so far
//!   ([`macos::MacObserver`], [`macos::MacExecutor`], bundled with
//!   [`common::reservations::FileReservations`] as [`macos::MacPorts`]).
//!
//! Only the FFI files in [`system`] are gated on the target OS (and hold
//! the crate's only `unsafe`); everything on top of `System` is plain
//! assembly and parsing, tested deterministically against `FakeSystem` on
//! any machine.
// `unsafe` is allowed only in the FFI files of `system` (each opts in with
// `#[allow(unsafe_code)]` on its module); everything else stays safe.
#![deny(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

pub mod common;
#[cfg(feature = "linux")]
pub mod linux;
#[cfg(feature = "macos")]
pub mod macos;
pub mod system;
#[cfg(feature = "windows")]
pub mod windows;

pub use common::config::{CommonConfig, Programs, ProviderPaths, Timeouts};
pub use common::reservations::FileReservations;
pub use common::{HostHistory, NoHistory};
#[cfg(feature = "linux")]
pub use linux::{LinuxConfig, LinuxExecutor, LinuxObserver, LinuxPorts};
#[cfg(feature = "macos")]
pub use macos::{MacConfig, MacExecutor, MacObserver, MacPorts};
pub use system::{FakeSystem, RealSystem, System};
#[cfg(feature = "windows")]
pub use windows::{WindowsConfig, WindowsExecutor, WindowsObserver, WindowsPorts};

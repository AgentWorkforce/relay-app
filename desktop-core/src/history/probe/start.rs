//! The `start` command's arguments: the install, and whether to replace a
//! collector another build of this release launched.
use super::{bridge, Target};
use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub(super) struct Start {
    #[command(flatten)]
    target: Target,
    /// Also replace a running collector another build of this release
    /// launched. The desktop passes it when its core is the default one; a
    /// site profile built to coexist never replaces the shared collector.
    #[arg(long)]
    pub(super) replace_same_release: bool,
}

impl Start {
    pub(super) fn run(&self) -> Result<()> {
        bridge::start(&self.target.directory()?, self.replace_same_release)
    }
}

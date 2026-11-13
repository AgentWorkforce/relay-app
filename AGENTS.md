# Working in relay-desktop

Instructions for coding agents working in this repository. People contributing
by hand should read [CONTRIBUTING.md](CONTRIBUTING.md); the rules below apply
to them too.

**Which workflow you follow depends on who you work for, not on the files in
your checkout.** By default, and always when working in a fork, contribute
through a pull request as [CONTRIBUTING.md](CONTRIBUTING.md) describes, and
ignore `docs/maintainer-workflow.md`. Follow `docs/maintainer-workflow.md`
instead only when this repository's maintainers assigned you the work and your
instructions say to use the maintainer workflow.

**Tests.** Run `swift test` and the GTK suite before asking for a review; see
[docs/workspace-layout.md](docs/workspace-layout.md#validation).
`scripts/check-open-source.sh` must pass: it keeps internal hosts, private
repositories and developer paths out of the tree.

## Rules

- **macOS and GTK change together.** `macos/` is the reference. `gtk/src` must
  match it in features, layout and wording, because GTK serves Linux and
  Windows. Pairs are listed in [docs/workspace-layout.md](docs/workspace-layout.md#macos-and-gtk-counterparts).
  - In `gtk/src` and `desktop-core/`, code that touches the OS needs a
    `cfg(windows)` counterpart.
  - If a feature can't exist on one platform, the core reports it as
    unavailable; don't hide it.
- **The public core lives in `AgentWorkforce/relay`** (`agent-relay-node`, in
  `crates/relay-node`). It is messaging only.
  - Change it there first.
  - Then move the `agent-relay-node` pin in both Cargo.toml files.
  - Refresh both lockfiles and run `scripts/generate-contract.sh`.
- **Desktop-only code lives in `desktop-core/`.** That covers sign-in, hosted
  MCP, provider setup, attestation and session history
  (`desktop-core/src/history`), added through the core's plugin hooks. Never
  put it in relay's core.
- **Release notes:** add a bullet under `## Unreleased` in `CHANGELOG.md` for
  any change a user would notice. Name the platform where behaviour differs.
  Changes that only touch build, CI, tests or iOS don't need one.
- **Never stop, kill or disable** the running core. That is the
  `agent-relay.service` user unit on Linux, the
  `com.agentrelay.desktop*.core*` LaunchAgent that is loaded on macOS, and the
  `Agent Relay (<account>)` logon task on Windows. Restart it if it has to pick
  up a change.
- **Don't publish releases.** Don't dispatch the release workflows or push
  `desktop-v*` or `released-v*` tags; the maintainers publish.

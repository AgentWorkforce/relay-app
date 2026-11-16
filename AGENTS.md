# Working in relay-app

**Which workflow you follow depends on who you work for, not on the files in your checkout.**
This repository takes no pull requests. If the maintainers did not assign you the work, propose it as a GitHub issue as [CONTRIBUTING.md](CONTRIBUTING.md) describes, and ignore `docs/maintainer-workflow.md`. Follow `docs/maintainer-workflow.md` only when this repository’s maintainers assigned you the work and your instructions say to use the maintainer workflow.

- Before asking for review, `scripts/check-open-source.sh` must pass. Run `swift test --package-path macos` and the GTK suite as described in [validation](docs/workspace-layout.md#validation).
- **macOS and GTK change together.** macOS is the reference; GTK matches features, layout and wording. See [platform pairs](docs/workspace-layout.md#macos-and-gtk-counterparts). In `gtk/src` and `desktop-core/`, code that touches the OS needs a `cfg(windows)` counterpart. Report unavailable features explicitly.
- The public core is `AgentWorkforce/relay` (`agent-relay-node` in `crates/relay-node`), messaging only. Change it there first; update the `agent-relay-node` pin in both Cargo.toml files, refresh both lockfiles and run `scripts/generate-contract.sh`.
- Desktop-only concerns—sign-in, hosted MCP, provider setup, attestation and session history (`desktop-core/src/history`) through plugin hooks—belong in `desktop-core/`. Never put them in relay’s core.
- Add platform-specific Unreleased notes for user-visible changes, excluding build/CI/test/iOS-only changes.
- Never stop, kill or disable the running core: the `agent-relay.service` user unit on Linux, the loaded `com.agentrelay.desktop*.core*` LaunchAgent on macOS, or the `Agent Relay (<account>)` logon task on Windows. Restart it if needed.
- Maintainers publish releases; do not dispatch release workflows or push `desktop-v*`/`released-v*` tags.

## Agency and guidance

- Act autonomously on routine assigned work; ask forgiveness rather than permission. Preserve explicit production, release and credential gates.
- For guidance, ask the engineering leads, the agent that spawned you, or the layered owner on Agent Relay.
- Only a truly pressing or blocking question goes to `khaliq-gant` or `will-washburn` on Agent Relay.

## Review and landing

- Only for maintainer-assigned work explicitly instructed to use `docs/maintainer-workflow.md`: submit a review branch to `layered-relay-app`, resolve findings, and let the owner squash the commit that passes the required CI/GitHub gate onto `main`. Outside contributors follow `CONTRIBUTING.md`, propose GitHub issues, and ignore the maintainer workflow. Pull requests are disabled.

## Cross-repo context

- Before and during changes, understand how this repo fits the wider system; consult its repo map and related-repo docs where available.
- Check relevant agents’ summaries on Agent Relay and proactively contact owning agents about dependencies, adjacent changes and work in flight.

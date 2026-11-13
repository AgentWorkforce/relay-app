# Contributing to Agent Relay Desktop

Thanks for helping. Agent Relay Desktop is the macOS, Linux and Windows app
for [Agent Relay](https://agentrelay.com), licensed under FSL-1.1-ALv2 (see LICENSE and NOTICE).

## Layout

- `macos/`: the macOS app (SwiftUI, SwiftPM). It is the reference design.
- `gtk/`: the GTK 4 app for Linux and Windows. It matches `macos/` in
  features, layout and wording.
- `desktop-core/`: the desktop core in Rust: sign-in, hosted MCP, provider
  setup, attestation and session history, plugged into Relay's public
  messaging core.
- `linux/`, `windows/`: installers and platform notes.
- `docs/`: design notes and contracts. Start with
  [docs/workspace-layout.md](docs/workspace-layout.md).

The messaging core itself lives in
[AgentWorkforce/relay](https://github.com/AgentWorkforce/relay)
(`crates/relay-node`); changes to it go there first.

## Proposing a change

This repository does not take pull requests. Open a
[GitHub issue](https://github.com/AgentWorkforce/relay-app/issues) instead:

- **Bugs:** what you did, what you expected, what happened, your platform
  (macOS, Linux or Windows) and the app version.
- **Features:** the problem you want solved and how you would use it.
- **A fix you already have:** describe it in the issue, and include the diff
  if it is short. The maintainers make the change and credit you in the
  commit.

## Building and testing

The [Development](README.md#development) section of the README covers
building the apps and the core. The checks in
[docs/workspace-layout.md](docs/workspace-layout.md#validation) cover
`swift test`, the GTK suite and `bash scripts/check-open-source.sh`.

The rules in [AGENTS.md](AGENTS.md) apply to every change in this repository.

## Security

Please report vulnerabilities privately through GitHub's security advisories
for this repository rather than in a public issue.

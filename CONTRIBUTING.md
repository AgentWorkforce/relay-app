# Contributing to Agent Relay Desktop

Thanks for helping. Agent Relay Desktop is the macOS, Linux and Windows app
for [Agent Relay](https://agentrelay.com), licensed under Apache-2.0.

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

## Building and testing

The [Development](README.md#development) section of the README covers
building the apps and the core. Before opening a pull request, run the checks
in [docs/workspace-layout.md](docs/workspace-layout.md#validation), including
`swift test`, the GTK suite and `bash scripts/check-open-source.sh`.

## Pull requests

1. Fork the repository and branch from `main`.
2. Keep one change per pull request, with tests.
3. Change macOS and GTK together, or say in the pull request why a feature
   can't exist on one platform (the core then reports it as unavailable).
4. Add a bullet under `## Unreleased` in `CHANGELOG.md` for anything a user
   would notice, naming the platform where behaviour differs.
5. CI runs on pull requests from forks after a maintainer approves the run.

The rules in [AGENTS.md](AGENTS.md) apply to every change, whether a person
or an agent writes it.

## Security

Please report vulnerabilities privately through GitHub's security advisories
for this repository rather than in a public issue.

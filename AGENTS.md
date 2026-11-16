# Working in relay-app

```sh
swift test
scripts/check-open-source.sh
```

- This repo takes no PRs. External proposals use CONTRIBUTING.md. Only maintainer-assigned work explicitly instructed to use it follows docs/maintainer-workflow.md.
- Run swift test, the GTK suite and scripts/check-open-source.sh; exact platform commands are in docs/workspace-layout.md#validation.
- macOS is the reference; GTK matches feature/layout/wording and OS code has Windows counterparts. Report unavailable features explicitly.
- Messaging core changes land in AgentWorkforce/relay first; update both pins/lockfiles and run scripts/generate-contract.sh. Desktop integrations/history stay in desktop-core.
- Add platform-specific Unreleased notes for user-visible changes, excluding build/CI/test/iOS-only changes.
- Never stop, kill or disable the installed core; restart if needed. Maintainers publish; do not dispatch releases or push desktop-v*/released-v* tags.

## Agency and guidance

- Act autonomously on routine assigned work; ask forgiveness rather than permission. Preserve explicit production, release and credential gates.
- For guidance, ask the engineering leads, the agent that spawned you, or the layered owner on Agent Relay.
- Only a truly pressing or blocking question goes to `khaliq-gant` or `will-washburn` (CODEOWNERS) on Agent Relay.

## Review and landing

- Pull requests are disabled. Use the review-branch and `layered-relay-app` owner workflow in `docs/maintainer-workflow.md`; review findings and the required CI/GitHub gate must be resolved on the exact commit that lands. Changes reach `main` through the owner’s documented squash/verification process, not a pull request.

## Cross-repo context

- Before and during changes, understand how this repo fits the wider system; consult its repo map and related-repo docs where available.
- Check relevant agents’ summaries on Agent Relay and proactively contact owning agents about dependencies, adjacent changes and work in flight.

#!/usr/bin/env bash
# Fails when the tracked tree would leak something into the public source:
# an embedded PostHog project key, a developer's home path, a link into a
# private AgentWorkforce repository, a missing project license, CI reading the
# analytics secrets outside a release, or a file that was removed for being
# internal. It prints file names and line numbers only, never the matching
# text, so a hit cannot itself leak a value.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
status=0

fail() {
  echo "::error::$1" >&2
  status=1
}

hits() { # MESSAGE PATTERN [PATHSPEC...]
  local message="$1" pattern="$2"
  shift 2
  local found
  found="$(git grep -n -I -E "$pattern" -- "$@" | cut -d: -f1,2 || true)"
  if [ -n "$found" ]; then
    fail "$message"
    printf '  %s\n' "$found" >&2
  fi
}

if ! grep -qs "Apache License" LICENSE; then
  fail "LICENSE must hold the Apache License 2.0."
fi
grep -q 'license = "Apache-2.0"' desktop-core/Cargo.toml || fail "desktop-core/Cargo.toml must declare license = \"Apache-2.0\"."
if grep -q "All rights reserved" macos/Info.plist.template; then
  fail "macos/Info.plist.template must not claim all rights reserved."
fi

hits "Embedded PostHog project key; supply it from the release secrets." \
  'phc_[A-Za-z0-9]{20,}' . ':!scripts/check-open-source.sh'
hits "Link into a private AgentWorkforce repository." \
  'github\.com[/:]AgentWorkforce/(cloud|relaycast-cloud|relayhistory-cloud)([/.#]|$)' . ':!scripts/check-open-source.sh'
hits "Developer home path; use a neutral fixture such as /Users/me." \
  '/Users/(will|khaliq|khaliqgant|miya|barry)/' . ':!scripts/check-open-source.sh'
hits "Internal machine name or address; use a neutral host such as example.test." \
  '(sf-frame|linuyc|finn-mini|dogpatch|kjg-lap|ip-10-[0-9])' . ':!scripts/check-open-source.sh'
# relay-control (hyphenated) and relayhistory are public; relaycontrol is not.
internal=(':!scripts/check-open-source.sh')
hits "Reference to a private AgentWorkforce repository; point at a public one or describe it." \
  '(relaycontrol|hosted-docs|github\.com[/:]AgentWorkforce/relayhistory-cloud)' . "${internal[@]}" \
  ':!desktop-core/tests/fixtures/desktop-feed/specs'
# The agent host deploys through layered-agents' reusable workflow and state
# store; only the agent stack and its workflows may name it.
hits "Reference to the private layered-agents repository outside the agent stack." \
  'layered-agents' . "${internal[@]}" ':!agent' ':!.github/workflows/agent-ci.yml' \
  ':!.github/workflows/agent-deploy.yml'
# Release builds fetch a profile's branding archive from desktop-branding;
# only the code that fetches it, its test and its documentation may name it.
hits "Reference to the private branding repository outside the branding fetch." \
  'desktop-branding' . "${internal[@]}" ':!.github/actions/site-profile/action.yml' \
  ':!scripts/test_release_workflow.py' ':!.github/README.md' ':!docs/workspace-layout.md'

# Pushes and same-repository pull requests can read repository secrets, so a
# CI build embeds the official analytics configuration only behind the
# release-only input Desktop Release sets.
ungated="$(grep -n 'secrets\.POSTHOG_' .github/workflows/desktop-ci.yml \
  | grep -v 'inputs\.release-analytics && secrets\.POSTHOG_' | cut -d: -f1 || true)"
if [ -n "$ungated" ]; then
  fail "desktop-ci.yml reads the PostHog secrets without the release-analytics input."
  while read -r line; do echo "  .github/workflows/desktop-ci.yml:$line" >&2; done <<< "$ungated"
fi

for removed in .claude/launch.json config/posthog.json docs/contractor-access.md \
  docs/macos-core-startup-incident.md docs/spawn-restart-dogfood.md docs/quota.md \
  docs/cloud-receiver.md docs/spawn-validation.md docs/machine-address-verification.md \
  docs/hosted-mcp-desktop-identity-live-check.md docs/e2e-approval-plan.md \
  docs/mcp-agent-credentials.md docs/skip-chat.md docs/mobile-app-review-audit-20261017.md \
  docs/retention-verification.md \
  docs/service-callers-e2e.md docs/probe-ownership-plan.md \
  scripts/import-grok-avatar-art.mjs; do
  if git ls-files --error-unmatch "$removed" > /dev/null 2>&1; then
    fail "$removed was removed as internal and must not be tracked again."
  fi
done
# work/ held developer scratch files; nothing may be tracked there again.
if [ -n "$(git ls-files -- work/)" ]; then
  fail "work/ was removed as internal scratch space and must not be tracked again."
fi

exit "$status"

#!/usr/bin/env bash
#
# check-telemetry-key.sh -- guard the analytics key an official build embeds.
#
#   check-telemetry-key.sh source              the key in AGENT_RELAY_POSTHOG_API_KEY
#   check-telemetry-key.sh packaged BINARY...  each packaged helper
#
# The key comes from the environment only and is never printed. `source`
# fails unless the key is a PostHog project (capture) token, phc_..., which
# is meant to be public. A personal API key (phx_...) grants account access
# and must never ship; 2026.11.19's helpers did. `packaged` fails unless each
# binary contains exactly that key and no personal-key-shaped string.
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
die() { printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2; exit 1; }

KEY="${AGENT_RELAY_POSTHOG_API_KEY:-}"
PROJECT_KEY='^phc_[A-Za-z0-9_]{1,251}$'
# Personal API keys: phx_ followed by the token body.
PERSONAL_KEY='phx_[A-Za-z0-9_-]{16,}'

check_source() {
  [ -n "$KEY" ] || die "AGENT_RELAY_POSTHOG_API_KEY is empty; an official build needs the PostHog project token"
  case "$KEY" in
    phx_*) die "AGENT_RELAY_POSTHOG_API_KEY holds a PostHog personal API key (phx_); use the project token (phc_)" ;;
  esac
  [[ "$KEY" =~ $PROJECT_KEY ]] || die "AGENT_RELAY_POSTHOG_API_KEY is not a PostHog project token (phc_)"
}

# grep's own answer: 0 found, 1 not found, anything else an error, which
# fails closed: an unreadable or partly read helper is never accepted.
scan() {
  local status=0
  LC_ALL=C grep -a -q "$@" || status=$?
  case "$status" in
    0) return 0 ;;
    1) return 1 ;;
    *) die "could not read $(basename "${!#}") to check its analytics key" ;;
  esac
}

check_packaged() {
  [ "$#" -gt 0 ] || die "usage: $SCRIPT_NAME packaged BINARY..."
  check_source
  local binary
  for binary in "$@"; do
    if [[ ! -f "$binary" || ! -r "$binary" || ! -s "$binary" ]]; then
      die "no readable packaged helper at $binary"
    fi
    if scan -E -- "$PERSONAL_KEY" "$binary"; then
      die "$(basename "$binary") contains a PostHog personal API key (phx_); it must not ship"
    fi
    scan -F -- "$KEY" "$binary" \
      || die "$(basename "$binary") was not built with this release's PostHog project token"
  done
}

case "${1:-}" in
  source) shift; [ "$#" -eq 0 ] || die "usage: $SCRIPT_NAME source"; check_source ;;
  packaged) shift; check_packaged "$@" ;;
  *) die "usage: $SCRIPT_NAME source | packaged BINARY..." ;;
esac

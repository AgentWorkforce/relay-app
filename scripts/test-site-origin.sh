#!/usr/bin/env bash
# The normalization build-probe.sh records and the packaging scripts
# compare (site-origin.sh) keeps what the core's build.rs keeps, and the
# checks on a packaged core say why they refuse one.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=site-origin.sh
. "$(dirname "${BASH_SOURCE[0]}")/site-origin.sh"

failed=0
expect() {
  local got
  if got="$(site_origin "$1")"; then
    [ "$got" = "$2" ] || { printf 'site_origin %q: got %q, want %q\n' "$1" "$got" "$2"; failed=1; }
  else
    [ "$2" = "" ] || { printf 'site_origin %q: refused, want %q\n' "$1" "$2"; failed=1; }
  fi
}
expect "http://localhost:3100/" "http://localhost:3100"
expect "  http://localhost:3100  " "http://localhost:3100"
expect "HTTPS://AgentRelay.com" "https://agentrelay.com"
expect "https://agentrelay.com:443/" "https://agentrelay.com"
expect "https://staging.example" "https://staging.example"
expect "http://localhost:80" "http://localhost"
expect "http://127.0.0.1" "http://127.0.0.1"
expect "http://127.0.0.1:8443" "http://127.0.0.1:8443"
# A port is a number: leading zeros are read, not kept.
expect "http://localhost:0080" "http://localhost"
expect "http://127.0.0.1:03100" "http://127.0.0.1:3100"
expect "https://agentrelay.com:0443" "https://agentrelay.com"
# What the core's build refuses, this refuses: https off its default port
# or on loopback, http off loopback, another scheme, no origin at all.
for bad in "https://staging.example:8443" "https://localhost:3100" "http://example.com:3100" "ftp://localhost:21" \
  "localhost:3100" "http://localhost:3100/path" "http://localhost:3100/?x=1" "http://user:pw@localhost:3100" \
  "http://localhost:31a" "http://localhost:0" "http://localhost:65536" "http://localhost:" "http://local host" "" "http://"; do
  expect "$bad" ""
done
[ "$(url_scheme_name 'AgentRelay-Dev ')" = "agentrelay-dev" ] || { echo "url_scheme_name lowercases and trims"; failed=1; }
# The web's schemes, which the core's build refuses for the app's links.
for bad in "1relay" "agent relay" "" "https:" "http" "HTTPS" "ws" "wss" "ftp" "file"; do
  url_scheme_name "$bad" >/dev/null && { printf 'url_scheme_name %q: accepted\n' "$bad"; failed=1; }
done

# What a build is made for: the option, else the exported variable, else
# production's; each normalized, exported for cargo or unset, and recorded.
(
  export AGENT_RELAY_SITE_URL="http://localhost:4000/" AGENT_RELAY_URL_SCHEME="AgentRelay-Dev"
  built_in_from "" "" >/dev/null || exit 1
  [ "$BUILT_IN_SITE" = "http://localhost:4000" ] && [ "$BUILT_IN_SCHEME" = "agentrelay-dev" ] || { echo "built_in_from: exported values: $BUILT_IN_SITE $BUILT_IN_SCHEME"; exit 1; }
  [ "$AGENT_RELAY_SITE_URL" = "http://localhost:4000" ] && [ "$AGENT_RELAY_URL_SCHEME" = "agentrelay-dev" ] || { echo "built_in_from: exported values not normalized"; exit 1; }
  built_in_from "HTTPS://Staging.Example/" "relay" >/dev/null || exit 1
  [ "$BUILT_IN_SITE" = "https://staging.example" ] && [ "$BUILT_IN_SCHEME" = "relay" ] || { echo "built_in_from: options win: $BUILT_IN_SITE $BUILT_IN_SCHEME"; exit 1; }
  unset AGENT_RELAY_SITE_URL AGENT_RELAY_URL_SCHEME
  built_in_from "" "" >/dev/null || exit 1
  [ "$BUILT_IN_SITE" = "https://agentrelay.com" ] && [ "$BUILT_IN_SCHEME" = "agentrelay" ] && [ -z "${AGENT_RELAY_SITE_URL:-}" ] && [ -z "${AGENT_RELAY_URL_SCHEME:-}" ] || { echo "built_in_from: production when nothing is given"; exit 1; }
  built_in_from "https://staging.example:8443" "" >/dev/null 2>&1 && { echo "built_in_from: accepted a site the core refuses"; exit 1; }
  built_in_from "" "https" >/dev/null 2>&1 && { echo "built_in_from: accepted a web scheme"; exit 1; }
  dir="$(mktemp -d)"
  built_in_from "http://localhost:3100" "agentrelay-dev" >/dev/null
  record_built_in "$dir" probe
  [ "$(cat "$dir/probe-site.txt")" = "http://localhost:3100" ] && [ "$(cat "$dir/probe-url-scheme.txt")" = "agentrelay-dev" ] || { echo "record_built_in: wrong sidecars"; rm -rf "$dir"; exit 1; }
  rm -rf "$dir"
) || failed=1

# A packaged core is checked by its sidecars, normalized; without them, by
# what it prints, and the scheme only when the caller built it itself; a
# malformed sidecar is named as such, not treated as missing.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf 'not a program\n' > "$work/probe"
reason="$(check_probe "$work/probe" https://agentrelay.com agentrelay 2>&1 >/dev/null)" && { echo "check_probe: a core that neither runs nor carries sidecars passed"; failed=1; }
[[ "$reason" == *"does not run here to say its site"* ]] || { echo "check_probe: unrunnable reason: $reason"; failed=1; }
printf '#!/bin/sh\necho "      --site-url <SITE_URL>"\necho "          The Agent Relay site [default: http://localhost:3100]"\n' > "$work/probe"
chmod +x "$work/probe"
check_probe "$work/probe" https://agentrelay.com agentrelay 2>/dev/null && { echo "check_probe: a core printing another site passed"; failed=1; }
check_probe "$work/probe" http://localhost:3100 agentrelay 2>/dev/null && { echo "check_probe: an unknown scheme passed"; failed=1; }
check_probe "$work/probe" http://localhost:3100 agentrelay self-built 2>/dev/null || { echo "check_probe: a self-built core for its site failed"; failed=1; }
printf 'HTTP://Localhost:3100/\n' > "$work/probe-site.txt"
printf 'AgentRelay-Dev\n' > "$work/probe-url-scheme.txt"
check_probe "$work/probe" http://localhost:3100 agentrelay-dev 2>/dev/null || { echo "check_probe: matching sidecars failed"; failed=1; }
check_probe "$work/probe" https://agentrelay.com agentrelay 2>/dev/null && { echo "check_probe: a dev core passed as production"; failed=1; }
reason="$(check_probe "$work/probe" http://localhost:3100 agentrelay 2>&1 >/dev/null)" && { echo "check_probe: a dev scheme passed"; failed=1; }
[[ "$reason" == *"opens agentrelay-dev:// links, not agentrelay://"* ]] || { echo "check_probe: reason: $reason"; failed=1; }
printf 'agent relay\n' > "$work/probe-url-scheme.txt"
reason="$(check_probe "$work/probe" http://localhost:3100 agentrelay-dev 2>&1 >/dev/null)" && { echo "check_probe: a malformed sidecar passed"; failed=1; }
[[ "$reason" == *"malformed"* ]] || { echo "check_probe: malformed sidecar reason: $reason"; failed=1; }
# An app is checked by its own sidecars only (it prints nothing).
printf 'https://agentrelay.com\n' > "$work/agent-relay-site.txt"
printf 'agentrelay\n' > "$work/agent-relay-url-scheme.txt"
check_sidecars "$work/agent-relay" agent-relay https://agentrelay.com agentrelay 2>/dev/null || { echo "check_sidecars: a production app failed"; failed=1; }
rm "$work/agent-relay-site.txt"
if check_sidecars "$work/agent-relay" agent-relay https://agentrelay.com agentrelay 2>/dev/null; then status=0; else status=$?; fi
[ "$status" = 2 ] || { echo "check_sidecars: a missing sidecar is status 2, got $status"; failed=1; }

# The site profile: an opaque id names it (empty is the default), the
# artifact names start with the id and never a product name, and the values
# a profile sets are checked as the scripts take them.
[ "$(profile_id ' acme-7 ')" = "acme-7" ] || { echo "profile_id trims"; failed=1; }
[ "$(profile_id "$(printf 'a%.0s' {1..40})")" = "$(printf 'a%.0s' {1..40})" ] || { echo "profile_id takes 40 characters"; failed=1; }
[ "$(profile_id 'a-b-7')" = "a-b-7" ] || { echo "profile_id takes single hyphens"; failed=1; }
# An id ending in a hyphen or holding two together is refused, as build-app.sh
# --asset-prefix and the Mac updater refuse Desktop-<id> for it.
for bad in "Acme" "-acme" "acme-" "a--b" "-" "acme_7" "acme 7" "" "$(printf 'a%.0s' {1..41})"; do
  profile_id "$bad" >/dev/null 2>&1 && { printf 'profile_id %q: accepted\n' "$bad"; failed=1; }
done
[ "$(asset_prefix '')" = "AgentRelay" ] || { echo "asset_prefix: the default profile is AgentRelay"; failed=1; }
[ "$(asset_prefix 'acme-7')" = "Desktop-acme-7" ] || { echo "asset_prefix: a profile is Desktop-<id>"; failed=1; }
[ "$(releases_repository ' Owner/their-releases ')" = "Owner/their-releases" ] || { echo "releases_repository trims"; failed=1; }
for bad in "owner" "owner/" "/name" "owner/name/extra" "owner name/x" ""; do
  releases_repository "$bad" >/dev/null 2>&1 && { printf 'releases_repository %q: accepted\n' "$bad"; failed=1; }
done
[ "$(product_name ' Acme Desktop ')" = "Acme Desktop" ] || { echo "product_name trims"; failed=1; }
for bad in "" "a/b" 'a\b' 'a"b' "a'b" "$(printf 'a\tb')" "$(printf 'a%.0s' {1..65})" 'Acme: Relay' 'Acme Relay {Beta}' 'A,B' 'what?'; do
  product_name "$bad" >/dev/null 2>&1 && { printf 'product_name %q: accepted\n' "$bad"; failed=1; }
done

# The release feed: github:<owner>/<name> or cloud, and the default profile's
# is its releases repository on GitHub. A cloud feed's key is the base64 of
# 32 bytes.
[ "$DEFAULT_RELEASE_FEED" = "github:AgentWorkforce/relay-desktop-releases" ] || { echo "DEFAULT_RELEASE_FEED is the default releases repository on GitHub"; failed=1; }
[ "$(release_feed "$DEFAULT_RELEASE_FEED")" = "$DEFAULT_RELEASE_FEED" ] || { echo "release_feed takes the default feed"; failed=1; }
[ "$(release_feed ' github:Owner/their-releases ')" = "github:Owner/their-releases" ] || { echo "release_feed trims a github: feed"; failed=1; }
[ "$(release_feed ' cloud ')" = "cloud" ] || { echo "release_feed takes cloud"; failed=1; }
for bad in "" "github:" "github:owner" "github:owner/" "github:/name" "github:owner/name/extra" "github: owner/name" \
    "github:owner /name" "Cloud" "CLOUD" "cloud:" "cloud:acme" "gitlab:owner/name" "owner/name" "https://github.com/owner/name" "s3"; do
  release_feed "$bad" >/dev/null 2>&1 && { printf 'release_feed %q: accepted\n' "$bad"; failed=1; }
done
key="11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="
[ "$(feed_public_key " $key ")" = "$key" ] || { echo "feed_public_key trims"; failed=1; }
[ "$(feed_public_key "$(printf '%.0s\0' {1..32} | base64)")" = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" ] || { echo "feed_public_key takes 32 zero bytes"; failed=1; }
for bad in "" "${key%=}" "${key}=" "${key:1}" "A${key}" "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURp=" \
    "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo=" "11qYAYKx CrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHUR=" "$(printf '%.0s\0' {1..31} | base64)"; do
  feed_public_key "$bad" >/dev/null 2>&1 && { printf 'feed_public_key %q: accepted\n' "$bad"; failed=1; }
done

[ "$failed" = 0 ] && echo "site-origin.sh: ok"
exit "$failed"

#!/usr/bin/env bash
# build-app.sh's profile checks, which run before it needs macOS: a build
# for another site must carry its own identity, a loopback site is the
# development build, and the options must be well formed. Each case runs
# the real script and expects one refusal (or, for a good profile, only the
# macOS requirement).
set -euo pipefail
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failures=0
expect() { # EXPECTED-FRAGMENT ARGS...
  local want="$1"; shift
  local got
  got="$(bash "$SCRIPTS/build-app.sh" --arch arm64 --version 1.2.3 --probe /nonexistent --out /nonexistent "$@" 2>&1 || true)"
  if grep -qF -- "$want" <<<"$got"; then
    return
  fi
  printf 'build-app.sh %s\n  wanted: %s\n  got: %s\n' "$*" "$want" "$(grep -m1 'error:' <<<"$got")" >&2
  failures=$((failures + 1))
}
# A good profile gets past the profile checks: on Linux the script then
# refuses to run at all; on a Mac it goes on to its tools, the package and
# the probe directory, which does not exist here. Either way, no profile
# refusal is in the output.
accept() { # ARGS...
  local got
  got="$(bash "$SCRIPTS/build-app.sh" --arch arm64 --version 1.2.3 --probe /nonexistent --out /nonexistent "$@" 2>&1 || true)"
  if grep -qE -- "this script only runs on macOS|required tool '|no SwiftPM package|run scripts/build-probe.sh first" <<<"$got" \
    && ! grep -qE -- "needs its own|use --dev-site-url|cannot both be given|must be|must look like|needs a " <<<"$got"; then
    return
  fi
  printf 'build-app.sh %s\n  wanted: past the profile checks\n  got: %s\n' "$*" "$(grep -m1 'error:' <<<"$got")" >&2
  failures=$((failures + 1))
}
accept
accept --site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx --releases-repository Example/releases
accept --dev-site-url http://localhost:3100
accept --site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx --releases-repository Example/releases --asset-prefix Desktop-acme-7
expect "--asset-prefix must be letters, digits and hyphens" --asset-prefix "Desktop acme"
expect "--asset-prefix must be letters, digits and hyphens" --asset-prefix -Desktop
accept --dev-site-url http://127.0.0.1:3100 --bundle-id com.example.relay.dev --product-name "Relay X Dev"
expect "needs its own --bundle-id" --site-url https://staging.example
expect "needs its own --bundle-id" --site-url https://staging.example --bundle-id com.agentrelay.desktop --url-scheme relayx --releases-repository Example/releases
expect "needs its own --url-scheme" --site-url https://staging.example --bundle-id com.example.relay
expect "needs its own --url-scheme" --site-url https://staging.example --bundle-id com.example.relay --url-scheme agentrelay --releases-repository Example/releases
expect "needs its own --releases-repository" --site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx
expect "needs its own --releases-repository" --site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx --releases-repository AgentWorkforce/relay-desktop-releases
expect "use --dev-site-url http://localhost:<port>" --site-url http://localhost:3100
expect "use --dev-site-url http://localhost:<port>" --site-url http://localhost
expect "is Agent Relay Dev's" --site-url https://staging.example --bundle-id com.agentrelay.desktop.dev --url-scheme relayx --releases-repository Example/releases
expect "links are Agent Relay Dev's" --site-url https://staging.example --bundle-id com.example.relay --url-scheme agentrelay-dev --releases-repository Example/releases
expect "is the production app's" --dev-site-url http://localhost:3100 --bundle-id com.agentrelay.desktop
expect "links are the production app's" --dev-site-url http://localhost:3100 --url-scheme agentrelay
expect "--icon: no file at" --icon /nonexistent/icon.svg
expect "--product-name must be a file name" --product-name "Relay:X"
expect "use --dev-site-url" --site-url http://127.0.0.1:3100 --bundle-id com.example.relay --url-scheme relayx --releases-repository Example/releases
expect "cannot both be given" --dev-site-url http://localhost:3100 --site-url https://staging.example
expect "--dev-site-url must be an HTTP loopback origin with a port" --dev-site-url https://staging.example
expect "--dev-site-url port must be between" --dev-site-url http://localhost:80
expect "--site-url must be an origin" --site-url "not a site"
expect "--url-scheme must be" --site-url https://staging.example --bundle-id com.example.relay --url-scheme https --releases-repository Example/releases
expect "--bundle-id must be a reverse-DNS identifier" --site-url https://staging.example --bundle-id "com.example relay"
expect "--bundle-id must be at most 32 characters" --site-url https://staging.example --bundle-id com.example.a-long-reverse-dns.identifier
expect "--releases-repository must be OWNER/NAME" --site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx --releases-repository no-slash
expect "--product-name must be a file name" --product-name "Relay/X"
# The release feed: github: names the releases repository, and cloud is a
# site profile's own site, signed with its key.
KEY=11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=
STAGING=(--site-url https://staging.example --bundle-id com.example.relay --url-scheme relayx)
accept --release-feed github:AgentWorkforce/relay-desktop-releases
accept "${STAGING[@]}" --release-feed github:Example/releases
accept "${STAGING[@]}" --release-feed github:example/RELEASES --releases-repository Example/releases
accept "${STAGING[@]}" --asset-prefix Desktop-acme-7 --release-feed cloud --feed-public-key "$KEY"
expect "does not name the releases repository" "${STAGING[@]}" --release-feed github:Example/other --releases-repository Example/releases
expect "the release feed must be github:<owner>/<name> or cloud" "${STAGING[@]}" --release-feed https://staging.example
expect "only a cloud release feed takes a feed public key" "${STAGING[@]}" --releases-repository Example/releases --feed-public-key "$KEY"
expect "a cloud release feed takes no releases repository" "${STAGING[@]}" --asset-prefix Desktop-acme-7 --release-feed cloud --feed-public-key "$KEY" --releases-repository Example/releases
expect "a cloud release feed is a site profile's" "${STAGING[@]}" --release-feed cloud --feed-public-key "$KEY"
expect "a cloud release feed is on the profile's own site" --asset-prefix Desktop-acme-7 --release-feed cloud --feed-public-key "$KEY"
expect "a cloud release feed needs its feed public key" "${STAGING[@]}" --asset-prefix Desktop-acme-7 --release-feed cloud
expect "the feed public key must be an Ed25519 public key" "${STAGING[@]}" --asset-prefix Desktop-acme-7 --release-feed cloud --feed-public-key not-a-key
expect "--release-feed needs github:OWNER/NAME or cloud" --release-feed
expect "--feed-public-key needs a key" --feed-public-key
# --coexist (a site profile's DESKTOP_COEXIST) builds without the product
# marker, never under the production app's bundle id, in any case.
accept --product-name "Agent Relay Coexist" --bundle-id com.agentrelay.desktop.coexist --coexist
accept "${STAGING[@]}" --releases-repository Example/releases --coexist
expect "--coexist needs its own --bundle-id" --coexist
expect "--coexist needs its own --bundle-id" --bundle-id Com.AgentRelay.Desktop --coexist
if [ "$failures" -ne 0 ]; then
  printf 'build-app.sh options: %d failure(s)\n' "$failures" >&2
  exit 1
fi
python3 "$SCRIPTS/test_build_app_schemes.py"
echo "build-app.sh options: ok"

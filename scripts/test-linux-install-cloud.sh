# shellcheck shell=bash disable=SC2154
#
# test-linux-install-cloud.sh -- the cloud release feed cases of
# scripts/test-linux-install.sh, which sources this file after its own cases
# and shares its fake systemctl, HOME and helpers. A profile's copy of
# install.sh with RELEASE_FEED='cloud' reads a fixture feed (Cloud's proposed
# routes, served by a fake curl) signed at test time with a throwaway
# Ed25519 key from `openssl genpkey`: a first install with an install token,
# no token, each environment override, a tampered asset, a key of another
# profile, not_entitled, the 501 streaming fallback, and an update the
# installed core verifies with `feed verify`, download addresses off the
# manifest's hosts, and an update while the installed core is stopped.
#
# It deletes under HOME, so it refuses to run on its own or against a HOME
# the harness did not make (scripts/install-test-home.sh).
# shellcheck source=scripts/install-test-home.sh
. "$(dirname "${BASH_SOURCE[0]}")/install-test-home.sh"
install_test_fragment

if ! command -v openssl >/dev/null || ! command -v python3 >/dev/null; then
  fail "the cloud cases need openssl and python3"
fi
feed="$root/feed"
mkdir -p "$feed/objects"
token="sh_dfi_fixture-only-token"
openssl genpkey -algorithm ed25519 -out "$feed/key.pem" 2>/dev/null
public_key="$(openssl pkey -in "$feed/key.pem" -pubout -outform DER | tail -c 32 | base64 -w0)"

# A fake core for the cloud profile: like release()'s, plus a signed-in
# status, feed-headers and feed verify. feed verify records its arguments and
# answers with $root/state/verify-answer (verified by default);
# $root/state/signed-out makes status.get report no account.
cloud_release() { # version
  local version="$1" tree asset
  tree="$(mktemp -d)"
  mkdir -p "$tree/agent_relay/helpers"
  cat > "$tree/agent_relay/helpers/agent-relay-probe" <<PROBE
#!/usr/bin/env bash
# product: Desktop-x7k2q
if [ "\$1" = --version ]; then echo "agent-relay-probe $version"; exit 0; fi
[ "\$1" = relay ] && shift
case "\$1" in
  setup) echo 'Usage: agent-relay-probe setup [OPTIONS]'; exit 0 ;;
  feed-headers)
    printf '%s\n' "\$*" >> "$root/state/feed-headers-calls"
    echo '{"Authorization":"Bearer sh_dfa_core-fixture","X-Agent-Relay-Desktop":"1","X-Agent-Relay-Profile":"x7k2q"}'
    exit 0 ;;
  feed)
    [ "\$2" = --help ] && exit 0
    printf '%s\n' "\$*" >> "$root/state/verify-calls"
    cat "$root/state/verify-answer" 2>/dev/null || echo '{"ok":true}'
    exit 0 ;;
esac
if [ "\$1 \$2" = "call status.get" ]; then
  [ "\$(cat "$root/state/running" 2>/dev/null)" = "$version" ] || exit 1
  if [ -f "$root/state/signed-out" ]; then
    echo '{"daemon":{"version":"$version","pid":1},"account":{"state":"signed_out"}}'; exit 0
  fi
  echo '{"daemon":{"version":"$version","pid":1},"account":{"state":"signed_in","workspace_id":"ws_fixture","person":{"id":"user_fixture"}}}'
  exit 0
fi
exit 0
PROBE
  chmod 0755 "$tree/agent_relay/helpers/agent-relay-probe"
  asset="Desktop-x7k2q-$version-Linux-$arch-probe.tar.gz"
  tar -czf "$feed/objects/$asset" -C "$tree" agent_relay
  rm -rf "$tree"
}

# Publishes $1 as the feed's stable release: its probe asset plus another
# platform's, signed with key_id $2 (this profile's key by default).
publish() { # version [key_id]
  python3 -I - "$feed" "$1" "$arch" "${2:-fk_x7k2q_2026a}" <<'PY'
import hashlib, json, os, sys
feed, version, arch, key_id = sys.argv[1:]
name = f"Desktop-x7k2q-{version}-Linux-{arch}-probe.tar.gz"
data = open(os.path.join(feed, "objects", name), "rb").read()
manifest = {
    "schema": 1, "profile": "x7k2q", "channel": "stable", "version": version,
    "published_at": "2026-11-20T10:04:31Z", "notes": "- Fixture \"notes\" \\ and é.\n",
    "download_hosts": ["storage.test"],
    "assets": [
        {"id": "a_probe", "name": name, "platform": "Linux", "arch": arch, "kind": "probe",
         "size": len(data), "sha256": hashlib.sha256(data).hexdigest()},
        {"id": "a_other", "name": f"Desktop-x7k2q-{version}-Windows-x64.zip", "platform": "Windows",
         "arch": "x64", "kind": "zip", "size": 1, "sha256": "0" * 64},
    ],
}
open(os.path.join(feed, "objects", "a_probe"), "wb").write(data)
canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
open(os.path.join(feed, "manifest.jcs"), "wb").write(canonical.encode("utf-8"))
json.dump({"manifest": manifest, "key_id": key_id}, open(os.path.join(feed, "unsigned.json"), "w"))
PY
  openssl pkeyutl -sign -rawin -inkey "$feed/key.pem" -in "$feed/manifest.jcs" -out "$feed/manifest.sig"
  python3 -I - "$feed" <<'PY'
import base64, json, os, sys
feed = sys.argv[1]
envelope = json.load(open(os.path.join(feed, "unsigned.json")))
signature = open(os.path.join(feed, "manifest.sig"), "rb").read()
envelope["signature"] = base64.urlsafe_b64encode(signature).decode().rstrip("=")
json.dump(envelope, open(os.path.join(feed, "envelope.json"), "w"), indent=2)
PY
}

# The fake site and object store. Each request is logged as "METHOD URL" in
# curl.log and its headers, one per line, in headers.log. state/feed-status
# makes the manifest answer that status; state/no-storage makes download-url
# answer 501; state/download-url replaces the address download-url gives.
cat > "$root/bin/curl" <<CURL
#!/usr/bin/env bash
out=/dev/null; url=""; method=GET; headers=""; write=""
while [ \$# -gt 0 ]; do
  case "\$1" in
    -o) out="\$2"; shift 2 ;;
    -w) write="\$2"; shift 2 ;;
    -X) method="\$2"; shift 2 ;;
    -d) method=POST; shift 2 ;;
    -H) if [ "\$2" = @- ]; then headers="\$(cat)"; else headers="\$2"; fi; shift 2 ;;
    --proto|--retry|--max-redirs|--max-filesize) shift 2 ;;
    -*) shift ;;
    *) url="\$1"; shift ;;
  esac
done
echo "\$method \$url" >> "$root/state/curl.log"
printf '%s\n' "--- \$url" "\$headers" >> "$root/state/headers.log"
api=https://site.test/cloud/api/v1/desktop/feed
answer() { # status [file]
  if [ -n "\${2:-}" ]; then cp "\$2" "\$out"; fi
  [ -z "\$write" ] || printf '%s' "\$1"
}
case "\$method \$url" in
  "GET \$api/manifest?channel=stable")
    if [ -f "$root/state/feed-status" ]; then
      printf '{"error":"%s","message":"BODY-TEXT-NEVER-SHOWN"}' "\$(cat "$root/state/feed-code")" > "\$out"
      answer "\$(cat "$root/state/feed-status")"
    else
      answer 200 "$feed/envelope.json"
    fi ;;
  "POST \$api/assets/a_probe/download-url")
    if [ -f "$root/state/no-storage" ]; then
      echo '{"error":"no_object_storage"}' > "\$out"; answer 501
    elif [ -f "$root/state/download-url" ]; then
      printf '{"url":"%s","expires_at":"2026-11-20T11:00:00Z"}' "\$(cat "$root/state/download-url")" > "\$out"
      answer 200
    else
      echo '{"url":"https://storage.test/o/a_probe?sig=fixture","expires_at":"2026-11-20T11:00:00Z"}' > "\$out"
      answer 200
    fi ;;
  "GET https://storage.test/o/a_probe?sig=fixture"|"GET \$api/assets/a_probe") answer 200 "$feed/objects/a_probe" ;;
  *) exit 6 ;;
esac
CURL
chmod 0755 "$root/bin/curl"

cloud_install="$root/install-cloud.sh"
sed -e "s|^PRODUCT_NAME=.*|PRODUCT_NAME='Desktop'|" \
  -e "s|^RELEASES_REPOSITORY=.*|RELEASES_REPOSITORY=''|" \
  -e "s|^ASSET_PREFIX=.*|ASSET_PREFIX='Desktop-x7k2q'|" \
  -e "s|^RELEASE_FEED=.*|RELEASE_FEED='cloud'|" \
  -e "s|^SITE_URL=.*|SITE_URL='https://site.test'|" \
  -e "s|^PROFILE_ID=.*|PROFILE_ID='x7k2q'|" \
  -e "s|^FEED_PUBLIC_KEY=.*|FEED_PUBLIC_KEY='$public_key'|" "$INSTALL" > "$cloud_install"
[ "$(grep -c "^RELEASE_FEED='cloud'$" "$cloud_install")" = 1 ] || fail "the cloud copy was not made"
cloud() { env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE bash "$cloud_install"; }
reset_logs() { : > "$root/state/curl.log"; : > "$root/state/headers.log"; }
refused() { # what expected-message [env...]
  local what="$1" message="$2"; shift 2
  reset_logs
  if env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE "$@" bash "$cloud_install" \
    > "$root/state/out" 2>&1; then fail "$what was accepted"; fi
  grep -qF "$message" "$root/state/out" || { cat "$root/state/out"; fail "$what: no \"$message\""; }
  ! grep -qF "$token" "$root/state/out" || fail "$what printed the install token"
  ! grep -q BODY-TEXT-NEVER-SHOWN "$root/state/out" || fail "$what printed the error body"
  [ "$(current)" = "$expected_current" ] || fail "$what changed current"
}

# Start from no install at all.
require_disposable_home
rm -rf "$HOME/.local/lib/agent-relay" "$XDG_CONFIG_HOME/systemd/user/agent-relay.service"
rm -f "$root/state/running" "$root/state/enabled"
current() {
  local link
  link="$(readlink "$HOME/.local/lib/agent-relay/current" 2>/dev/null || true)"
  if [ -n "$link" ]; then basename "$link"; else echo none; fi
}
expected_current=none
cloud_release 2026.11.1; publish 2026.11.1

refused "a first install without a token" "needs AGENT_RELAY_INSTALL_TOKEN"
grep -q "downloads page" "$root/state/out" || fail "the missing token does not say where to get one"
[ ! -s "$root/state/curl.log" ] || fail "a first install without a token reached the feed"
pass "a first install without AGENT_RELAY_INSTALL_TOKEN is refused before any request"

refused "AGENT_RELAY_RELEASES_API" "AGENT_RELAY_RELEASES_API cannot be used" \
  AGENT_RELAY_INSTALL_TOKEN="$token" AGENT_RELAY_RELEASES_API=https://api.test/releases
refused "AGENT_RELAY_DOWNLOAD_BASE" "AGENT_RELAY_DOWNLOAD_BASE cannot be used" \
  AGENT_RELAY_INSTALL_TOKEN="$token" AGENT_RELAY_DOWNLOAD_BASE=https://releases.test
[ ! -s "$root/state/curl.log" ] || fail "an override reached the feed"
pass "each environment override is refused"

echo 403 > "$root/state/feed-status"; echo not_entitled > "$root/state/feed-code"
refused "a workspace without entitlement" "not entitled" AGENT_RELAY_INSTALL_TOKEN="$token"
grep -q "(not_entitled)" "$root/state/out" || fail "403 does not name its error code"
echo 401 > "$root/state/feed-status"; echo token_expired > "$root/state/feed-code"
refused "an expired token" "(token_expired)" AGENT_RELAY_INSTALL_TOKEN="$token"
rm -f "$root/state/feed-status"
pass "403 not_entitled and 401 token_expired name only the error code"

publish 2026.11.1 fk_other_2026a
refused "a manifest signed with another profile's key" "fk_other_2026a" AGENT_RELAY_INSTALL_TOKEN="$token"
pass "a key_id of another profile is refused"

publish 2026.11.1
python3 -I - "$feed/envelope.json" <<'PY'
import json, sys
envelope = json.load(open(sys.argv[1]))
envelope["manifest"]["assets"][0]["size"] += 1
json.dump(envelope, open(sys.argv[1], "w"))
PY
refused "a tampered manifest" "signature does not verify" AGENT_RELAY_INSTALL_TOKEN="$token"
pass "a manifest changed after signing is refused"

publish 2026.11.1
printf 'x' >> "$feed/objects/a_probe"
refused "a tampered asset" "not the size the manifest gives" AGENT_RELAY_INSTALL_TOKEN="$token"
cp "$feed/objects/Desktop-x7k2q-2026.11.1-Linux-$arch-probe.tar.gz" "$feed/objects/a_probe"
python3 -I -c 'import sys; p=sys.argv[1]; d=bytearray(open(p,"rb").read()); d[-1]^=1; open(p,"wb").write(d)' \
  "$feed/objects/a_probe"
refused "a tampered asset" "does not match the manifest's sha256" AGENT_RELAY_INSTALL_TOKEN="$token"
[ ! -d "$HOME/.local/lib/agent-relay/versions/2026.11.1" ] || fail "a tampered asset was unpacked"
pass "an asset whose size or sha256 differs from the manifest is never unpacked"

publish 2026.11.1
printf '#!/bin/sh\necho "LibreSSL 3.8.2"\n' > "$root/bin/openssl"; chmod 0755 "$root/bin/openssl"
refused "a first install without OpenSSL 3" "install OpenSSL 3" AGENT_RELAY_INSTALL_TOKEN="$token"
rm "$root/bin/openssl"
[ ! -d "$HOME/.local/lib/agent-relay/versions/2026.11.1" ] || fail "an unverified manifest's asset was unpacked"
pass "a first install without OpenSSL 3 fails closed and asks for it"

# Each address must fail on its own: another host, plain http, a port.
for address in https://evil.test/o/a_probe?sig=fixture http://storage.test/o/a_probe?sig=fixture \
  https://storage.test:8443/o/a_probe?sig=fixture; do
  printf '%s' "$address" > "$root/state/download-url"
  refused "the download address $address" "not on a host the manifest allows" AGENT_RELAY_INSTALL_TOKEN="$token"
  ! grep -qF "GET $address" "$root/state/curl.log" || fail "the refused address $address was fetched"
  [ "$(tail -n 1 "$root/state/curl.log")" = \
    "POST https://site.test/cloud/api/v1/desktop/feed/assets/a_probe/download-url" ] \
    || { cat "$root/state/curl.log"; fail "a request followed the refused address $address"; }
done
rm "$root/state/download-url"
[ ! -d "$HOME/.local/lib/agent-relay/versions/2026.11.1" ] || fail "an asset from a refused address was unpacked"
pass "a download address on another host, over http or with a port is refused and never fetched"

reset_logs
AGENT_RELAY_INSTALL_TOKEN="$token" cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the cloud first install failed"; }
[ "$(current)" = 2026.11.1 ] || fail "the cloud first install points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.1 ] || fail "2026.11.1 is not running"
[ "$(head -n 1 "$HOME/.local/lib/agent-relay/current/product")" = Desktop-x7k2q ] || fail "the product was not recorded"
expected_log="GET https://site.test/cloud/api/v1/desktop/feed/manifest?channel=stable
POST https://site.test/cloud/api/v1/desktop/feed/assets/a_probe/download-url
GET https://storage.test/o/a_probe?sig=fixture"
[ "$(cat "$root/state/curl.log")" = "$expected_log" ] || { cat "$root/state/curl.log"; fail "unexpected feed requests"; }
grep -A2 -F -- "--- https://site.test/cloud/api/v1/desktop/feed/manifest" "$root/state/headers.log" \
  | grep -qxF "Authorization: Bearer $token" || fail "the manifest request lacked the install token"
grep -A2 -F -- "--- https://site.test/cloud/api/v1/desktop/feed/manifest" "$root/state/headers.log" \
  | grep -qxF "X-Agent-Relay-Profile: x7k2q" || fail "the manifest request lacked the profile header"
[ -z "$(grep -A1 -F -- "--- https://storage.test/" "$root/state/headers.log" | sed 1d)" ] \
  || fail "the signed download address was sent headers"
grep -q "verified by OpenSSL" "$root/state/out" || fail "the first install did not verify with OpenSSL"
! grep -qF "$token" "$root/state/out" || fail "the install printed the token"
! grep -q '\.sha256' "$root/state/curl.log" || fail "the .sha256 sidecar was fetched"
pass "a first install with a token: manifest, download-url, then the signed address without credentials"

expected_current=2026.11.1
cloud_release 2026.11.2; publish 2026.11.2
touch "$root/state/no-storage"; reset_logs; : > "$root/state/verify-calls"
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the cloud update failed"; }
[ "$(current)" = 2026.11.2 ] || fail "the cloud update points at $(current)"
grep -qxF "GET https://site.test/cloud/api/v1/desktop/feed/assets/a_probe" "$root/state/curl.log" \
  || fail "a 501 did not fall back to streaming the asset"
grep -q -- "--profile x7k2q --public-key $public_key --installed-version 2026.11.1" "$root/state/verify-calls" \
  || { cat "$root/state/verify-calls"; fail "the installed core did not verify the manifest"; }
grep -q "verified by the installed core" "$root/state/out" || fail "the update did not say who verified it"
grep -q -- "--site https://site.test --workspace ws_fixture --account user_fixture --profile x7k2q" \
  "$root/state/feed-headers-calls" || fail "the update did not ask the installed core for headers"
grep -A3 -F -- "--- https://site.test/cloud/api/v1/desktop/feed/assets/a_probe" "$root/state/headers.log" \
  | grep -qxF "Authorization: Bearer sh_dfa_core-fixture" || fail "the stream lacked the core's headers"
! grep -q "sh_dfa_core-fixture" "$root/state/out" || fail "the update printed the core's credential"
rm -f "$root/state/no-storage"
pass "an update without a token uses the core's headers, its feed verify, and the 501 stream fallback"

expected_current=2026.11.2
echo '{"ok":false,"code":"signature_invalid","message":"x"}' > "$root/state/verify-answer"
refused "a manifest the installed core refuses" "(signature_invalid)"
echo '{"ok":false,"code":"not_newer","message":"x"}' > "$root/state/verify-answer"
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a not-newer feed failed"; }
grep -q "already up to date" "$root/state/out" || fail "a manifest that is not newer was not up to date"
rm -f "$root/state/verify-answer"
pass "the installed core's refusal stops the update; not_newer is up to date"

# A stopped core is started before it is asked who is signed in, and a
# signed-out one is still told apart from one that is not running.
rm -f "$root/state/running"; touch "$root/state/signed-out"
refused "a stopped, signed-out core" "is not signed in"
grep -q "not running; starting it" "$root/state/out" || fail "the stopped core was not reported as not running"
[ "$(cat "$root/state/running" 2>/dev/null)" = 2026.11.2 ] || fail "the stopped core was not started"
rm -f "$root/state/signed-out" "$root/state/running"; touch "$root/state/restart-fails"
refused "a stopped core that does not start" "not running and did not start"
! grep -q "not signed in" "$root/state/out" || fail "a core that did not start was called signed out"
rm -f "$root/state/restart-fails" "$root/state/running"
cloud_release 2026.11.3; publish 2026.11.3
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the update of a stopped core failed"; }
grep -q "not running; starting it" "$root/state/out" || fail "the update did not start the stopped core"
[ "$(current)" = 2026.11.3 ] || fail "the update of a stopped core points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.3 ] || fail "2026.11.3 is not running"
pass "an update without a token starts a stopped core first; not running and not signed in are told apart"

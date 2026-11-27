# shellcheck shell=bash disable=SC2154
#
# test-macos-install-cloud.sh -- the cloud release feed cases of
# scripts/test-macos-install.sh, which sources this file after its own cases
# and shares its fake uname, shasum, codesign, launchctl and perl, HOME and
# helpers. A profile's copy of macos/install.sh with RELEASE_FEED='cloud'
# reads a fixture feed (Cloud's proposed routes, served by a fake curl) signed
# at test time with a throwaway Ed25519 key from OpenSSL 3: a first install
# with an install token (piped into bash, reading nothing else from standard
# input), no token, each environment override, not_entitled, no_release, an
# expired token, a key of another profile, a signed manifest of another
# profile or channel, a manifest changed after signing, an asset whose size
# or sha256 differs, LibreSSL or no OpenSSL 3, download addresses off the
# manifest's hosts, --dry-run, an update the installed core verifies with
# `feed verify` through its own feed-headers (and the 501 streaming
# fallback), the installed core's refusal, not newer (up to date), --version,
# --status, --rollback, an update while the core is stopped, an update whose
# installed core has no feed verify (OpenSSL 3 verifies it, LibreSSL is
# refused), and --uninstall.
#
# It deletes under HOME, so it refuses to run on its own or against a HOME
# the harness did not make (scripts/install-test-home.sh).
# shellcheck source=scripts/install-test-home.sh
. "$(dirname "${BASH_SOURCE[0]}")/install-test-home.sh"
install_test_fragment

# An OpenSSL 3 signs the fixture feed, and is the one the installer finds on
# PATH: the system's on Linux, Homebrew's openssl@3 on a Mac.
signer=""
for candidate in "$(command -v openssl || true)" /opt/homebrew/opt/openssl@3/bin/openssl \
  /usr/local/opt/openssl@3/bin/openssl; do
  if [ -n "$candidate" ] && [ -x "$candidate" ] \
    && "$candidate" version 2>/dev/null | grep -q '^OpenSSL 3\.'; then signer="$candidate"; break; fi
done
[ -n "$signer" ] || fail "the cloud cases need OpenSSL 3 (on a Mac: brew install openssl@3)"
use_openssl() { # real | a version line for a fake
  if [ "$1" = real ]; then
    printf '#!/usr/bin/env bash\n%s\nexec "%s" "$@"\n' "$stdin_check" "$signer" > "$root/bin/openssl"
  else
    printf '#!/usr/bin/env bash\necho "%s"\n' "$1" > "$root/bin/openssl"
  fi
  chmod 0755 "$root/bin/openssl"
}
use_openssl real

feed="$root/feed"
mkdir -p "$feed/objects"
token="sh_dfi_fixture-only-token"
"$signer" genpkey -algorithm ed25519 -out "$feed/key.pem" 2>/dev/null
public_key="$("$signer" pkey -in "$feed/key.pem" -pubout -outform DER | tail -c 32 \
  | python3 -I -c 'import base64, sys; print(base64.b64encode(sys.stdin.buffer.read()).decode())')"

# A fake core for the cloud profile: like release()'s, plus a signed-in
# status, feed-headers and feed verify. feed verify records its arguments and
# answers with $root/state/verify-answer (verified by default);
# $root/state/no-feed-verify makes `feed --help` fail, as a core without feed
# verify does; $root/state/signed-out makes status.get report no account.
cloud_release() { # version
  local version="$1" tree asset
  tree="$(mktemp -d)"
  mkdir -p "$tree/agent_relay/helpers"
  cat > "$tree/agent_relay/helpers/agent-relay-probe" <<PROBE
#!/usr/bin/env bash
# product: Desktop-x7k2q
$stdin_check
if [ "\$1" = --version ]; then echo "agent-relay-probe $version"; exit 0; fi
if [ "\$1" = --local-storage-capabilities ]; then echo '{"protocol":1,"upload_queue_formats":[0,1]}'; exit 0; fi
[ "\$1" = relay ] && shift
case "\$1" in
  setup) echo 'Usage: agent-relay-probe setup [OPTIONS]'; exit 0 ;;
  serve) exit 0 ;;
  feed-headers)
    printf '%s\n' "\$*" >> "$root/state/feed-headers-calls"
    echo '{"Authorization":"Bearer sh_dfa_core-fixture","X-Agent-Relay-Desktop":"1","X-Agent-Relay-Profile":"x7k2q"}'
    exit 0 ;;
  feed)
    if [ "\$2" = --help ]; then [ ! -f "$root/state/no-feed-verify" ] || exit 2; exit 0; fi
    printf '%s\n' "\$*" >> "$root/state/verify-calls"
    cat "$root/state/verify-answer" 2>/dev/null || echo '{"ok":true}'
    exit 0 ;;
esac
if [ "\$1 \$2" = "call status.get" ]; then
  [ "\$(cat "$root/state/running" 2>/dev/null)" = "$version" ] || exit 1
  if [ -f "$root/state/signed-out" ]; then
    echo '{"result":{"daemon":{"version":"$version","pid":1},"account":{"state":"signed_out"}}}'; exit 0
  fi
  echo '{"result":{"daemon":{"version":"$version","pid":1},"account":{"state":"signed_in","workspace_id":"ws_fixture","person":{"id":"user_fixture"}}}}'
  exit 0
fi
exit 2
PROBE
  chmod 0755 "$tree/agent_relay/helpers/agent-relay-probe"
  echo "License" > "$tree/agent_relay/LICENSE"
  asset="Desktop-x7k2q-$version-macOS-arm64-probe.tar.gz"
  tar -czf "$feed/objects/$asset" -C "$tree" agent_relay
  rm -rf "$tree"
}

# Publishes $1 as the feed's stable release: its macOS probe asset plus
# another platform's and install-macos.sh, signed with key_id $2 (this
# profile's key by default). $3 and $4 replace the profile and channel the
# signed manifest declares (x7k2q and stable by default).
publish() { # version [key_id [profile [channel]]]
  python3 -I - "$feed" "$1" "${2:-fk_x7k2q_2026a}" "${3:-x7k2q}" "${4:-stable}" <<'PY'
import hashlib, json, os, sys
feed, version, key_id, profile, channel = sys.argv[1:]
name = f"Desktop-x7k2q-{version}-macOS-arm64-probe.tar.gz"
data = open(os.path.join(feed, "objects", name), "rb").read()
manifest = {
    "schema": 1, "profile": profile, "channel": channel, "version": version,
    "published_at": "2026-11-20T10:04:31Z", "notes": "- Fixture \"notes\" \\ and é.\n",
    "download_hosts": ["storage.test"],
    "assets": [
        {"id": "a_probe", "name": name, "platform": "macOS", "arch": "arm64", "kind": "probe",
         "size": len(data), "sha256": hashlib.sha256(data).hexdigest()},
        {"id": "a_linux", "name": f"Desktop-x7k2q-{version}-Linux-arm64-probe.tar.gz", "platform": "Linux",
         "arch": "arm64", "kind": "probe", "size": 1, "sha256": "0" * 64},
        {"id": "a_script", "name": "install-macos.sh", "platform": "macOS", "arch": "universal",
         "kind": "install-script", "size": 1, "sha256": "1" * 64},
    ],
}
open(os.path.join(feed, "objects", "a_probe"), "wb").write(data)
canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
open(os.path.join(feed, "manifest.jcs"), "wb").write(canonical.encode("utf-8"))
json.dump({"manifest": manifest, "key_id": key_id}, open(os.path.join(feed, "unsigned.json"), "w"))
PY
  "$signer" pkeyutl -sign -rawin -inkey "$feed/key.pem" -in "$feed/manifest.jcs" -out "$feed/manifest.sig"
  python3 -I - "$feed" <<'PY'
import base64, json, os, sys
feed = sys.argv[1]
envelope = json.load(open(os.path.join(feed, "unsigned.json")))
signature = open(os.path.join(feed, "manifest.sig"), "rb").read()
envelope["signature"] = base64.urlsafe_b64encode(signature).decode().rstrip("=")
json.dump(envelope, open(os.path.join(feed, "envelope.json"), "w"), indent=2)
PY
}

# The fake site and object store, replacing the releases server's curl. Each
# request is logged as "METHOD URL" in curl.log and its headers, one per line,
# in headers.log. state/feed-status (with state/feed-code) makes the manifest
# answer that status; state/no-storage makes download-url answer 501;
# state/download-url replaces the address download-url gives. Headers come on
# standard input (-H @-); a call without them gets STDIN_CHECK's check.
cat > "$root/bin/curl" <<CURL
#!/usr/bin/env bash
out=/dev/null; url=""; method=GET; headers=""; write=""; piped_headers=0
while [ \$# -gt 0 ]; do
  case "\$1" in
    -o) out="\$2"; shift 2 ;;
    -w) write="\$2"; shift 2 ;;
    -X) method="\$2"; shift 2 ;;
    -d) method=POST; shift 2 ;;
    -H) if [ "\$2" = @- ]; then headers="\$(cat)"; piped_headers=1; else headers="\$2"; fi; shift 2 ;;
    --proto|--retry|--max-redirs|--max-filesize) shift 2 ;;
    -*) shift ;;
    *) url="\$1"; shift ;;
  esac
done
if [ "\$piped_headers" = 0 ]; then
$stdin_check
fi
echo "\$method \$url" >> "$root/state/curl.log"
printf '%s\n' "--- \$url" "\$headers" >> "$root/state/headers.log"
api=https://site.test/cloud/api/v1/desktop/feed
answer() { # status [file]
  if [ -n "\${2:-}" ]; then cp "\$2" "\$out"; fi
  case "\$write" in
    '\n%{http_code}') printf '\n%s' "\$1" ;;
    '%{http_code}') printf '%s' "\$1" ;;
  esac
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

# A cloud profile's copy: the Stage step sets the seven assignments.
cloud_install="$root/install-cloud.sh"
sed -e "s|^PRODUCT_NAME=.*|PRODUCT_NAME='Desktop'|" \
  -e "s|^RELEASES_REPOSITORY=.*|RELEASES_REPOSITORY=''|" \
  -e "s|^ASSET_PREFIX=.*|ASSET_PREFIX='Desktop-x7k2q'|" \
  -e "s|^RELEASE_FEED=.*|RELEASE_FEED='cloud'|" \
  -e "s|^SITE_URL=.*|SITE_URL='https://site.test'|" \
  -e "s|^PROFILE_ID=.*|PROFILE_ID='x7k2q'|" \
  -e "s|^FEED_PUBLIC_KEY=.*|FEED_PUBLIC_KEY='$public_key'|" "$INSTALL" > "$cloud_install"
[ "$(grep -c "^RELEASE_FEED='cloud'$" "$cloud_install")" = 1 ] || fail "the cloud copy was not made"
[ "$(grep -c "^FEED_PUBLIC_KEY='$public_key'$" "$cloud_install")" = 1 ] || fail "the cloud copy has no feed key"
cloud() { env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE bash "$cloud_install" "$@"; }
reset_logs() { : > "$root/state/curl.log"; : > "$root/state/headers.log"; }
current() {
  local link
  link="$(readlink "$prefix/current" 2>/dev/null || true)"
  if [ -n "$link" ]; then basename "$link"; else echo none; fi
}
no_credential() { # what
  ! grep -qF "$token" "$root/state/out" || fail "$1 printed the install token"
  ! grep -q "sh_dfa_core-fixture" "$root/state/out" || fail "$1 printed the core's credential"
  # The fake core's own copy holds the core credential it hands out.
  ! grep -rqF --exclude=agent-relay-probe -e "$token" -e sh_dfa_core-fixture "$HOME" \
    || fail "$1 wrote a credential to a file"
  ! grep -rqF "$token" "$HOME" || fail "$1 wrote the install token to a file"
}
refused() { # what expected-message [env...]
  local what="$1" message="$2"; shift 2
  reset_logs
  if env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE "$@" bash "$cloud_install" \
    > "$root/state/out" 2>&1; then fail "$what was accepted"; fi
  grep -qF "$message" "$root/state/out" || { cat "$root/state/out"; fail "$what: no \"$message\""; }
  no_credential "$what"
  ! grep -q BODY-TEXT-NEVER-SHOWN "$root/state/out" || fail "$what printed the error body"
  [ "$(current)" = "$expected_current" ] || fail "$what changed current"
}

# Start from no install at all: the cases above end with --uninstall.
require_disposable_home
[ ! -e "$prefix" ] || fail "the cloud cases did not start from an uninstalled Mac"
rm -f "$root/state/running" "$root/state/loaded-$label" "$root/state/stdin-violations"
expected_current=none
cloud_release 2026.11.1; publish 2026.11.1

refused "a first install without a token" "needs AGENT_RELAY_INSTALL_TOKEN"
grep -q "downloads page" "$root/state/out" || fail "the missing token does not say where to get one"
[ ! -s "$root/state/curl.log" ] || fail "a first install without a token reached the feed"
cloud --help > "$root/state/out" 2>&1 || fail "the cloud copy's --help failed"
grep -q "A first install needs AGENT_RELAY_INSTALL_TOKEN" "$root/state/out" || fail "the cloud copy's usage does not name the token"
pass "a first install without AGENT_RELAY_INSTALL_TOKEN is refused before any request"

refused "AGENT_RELAY_RELEASES_API" "AGENT_RELAY_RELEASES_API cannot be used" \
  AGENT_RELAY_INSTALL_TOKEN="$token" AGENT_RELAY_RELEASES_API=https://api.test/releases
refused "AGENT_RELAY_DOWNLOAD_BASE" "AGENT_RELAY_DOWNLOAD_BASE cannot be used" \
  AGENT_RELAY_INSTALL_TOKEN="$token" AGENT_RELAY_DOWNLOAD_BASE=https://releases.test
refused "a token that is not printable ASCII" "is not an install token" \
  AGENT_RELAY_INSTALL_TOKEN="$token
X-Injected: 1"
[ ! -s "$root/state/curl.log" ] || fail "an override or a bad token reached the feed"
pass "each environment override, and a token that is not printable ASCII, is refused"

echo 403 > "$root/state/feed-status"; echo not_entitled > "$root/state/feed-code"
refused "a workspace without entitlement" "not entitled" AGENT_RELAY_INSTALL_TOKEN="$token"
grep -q "(not_entitled)" "$root/state/out" || fail "403 does not name its error code"
echo 404 > "$root/state/feed-status"; echo no_release > "$root/state/feed-code"
refused "a feed with no release" "no Desktop release is published on the stable channel (no_release)" \
  AGENT_RELAY_INSTALL_TOKEN="$token"
echo 401 > "$root/state/feed-status"; echo token_expired > "$root/state/feed-code"
refused "an expired token" "(token_expired)" AGENT_RELAY_INSTALL_TOKEN="$token"
rm -f "$root/state/feed-status" "$root/state/feed-code"
pass "403 not_entitled, 404 no_release and 401 token_expired name only the error code"

publish 2026.11.1 fk_other_2026a
refused "a manifest signed with another profile's key" "fk_other_2026a" AGENT_RELAY_INSTALL_TOKEN="$token"
pass "a key_id of another profile is refused"

# Signed with this profile's key, so only the manifest's own fields differ.
for field in profile channel; do
  if [ "$field" = profile ]; then publish 2026.11.1 fk_x7k2q_2026a other; else publish 2026.11.1 "" "" beta; fi
  refused "a signed manifest of another $field" "the manifest is for another $field" \
    AGENT_RELAY_INSTALL_TOKEN="$token"
  ! grep -q a_probe/download-url "$root/state/curl.log" || fail "a manifest of another $field had its asset requested"
  { [ ! -e "$prefix/versions" ] || [ -z "$(ls -A "$prefix/versions")" ]; } \
    || fail "a manifest of another $field installed something"
done
pass "a signed manifest of another profile or channel is refused and installs nothing"

publish 2026.11.1
python3 -I - "$feed/envelope.json" <<'PY'
import json, sys
envelope = json.load(open(sys.argv[1]))
envelope["manifest"]["assets"][0]["size"] += 1
json.dump(envelope, open(sys.argv[1], "w"))
PY
refused "a tampered manifest" "signature does not verify (signature_invalid)" AGENT_RELAY_INSTALL_TOKEN="$token"
pass "a manifest changed after signing is refused"

publish 2026.11.1
printf 'x' >> "$feed/objects/a_probe"
refused "a tampered asset" "not the size the manifest gives" AGENT_RELAY_INSTALL_TOKEN="$token"
cp "$feed/objects/Desktop-x7k2q-2026.11.1-macOS-arm64-probe.tar.gz" "$feed/objects/a_probe"
python3 -I -c 'import sys; p=sys.argv[1]; d=bytearray(open(p,"rb").read()); d[-1]^=1; open(p,"wb").write(d)' \
  "$feed/objects/a_probe"
: > "$root/state/codesign.log"
refused "a tampered asset" "does not match the manifest's sha256" AGENT_RELAY_INSTALL_TOKEN="$token"
[ ! -s "$root/state/codesign.log" ] || fail "a tampered asset was unpacked and checked"
[ ! -d "$prefix/versions/2026.11.1" ] || fail "a tampered asset was installed"
pass "an asset whose size or sha256 differs from the manifest is never unpacked"

publish 2026.11.1
use_openssl "LibreSSL 3.3.6"
refused "a first install with LibreSSL" "the openssl on PATH is LibreSSL" AGENT_RELAY_INSTALL_TOKEN="$token"
grep -q "brew install openssl@3" "$root/state/out" || fail "the LibreSSL refusal does not name Homebrew's openssl@3"
use_openssl "OpenSSL 1.1.1w  11 Sep 2023"
refused "a first install without OpenSSL 3" "no OpenSSL 3" AGENT_RELAY_INSTALL_TOKEN="$token"
grep -q "brew install openssl@3" "$root/state/out" || fail "the OpenSSL 3 refusal does not name Homebrew's openssl@3"
use_openssl real
! grep -q a_probe/download-url "$root/state/curl.log" || fail "an unverified manifest's asset was requested"
pass "a first install with LibreSSL or without OpenSSL 3 fails closed and names Homebrew's openssl@3"

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
[ ! -d "$prefix/versions/2026.11.1" ] || fail "an asset from a refused address was installed"
pass "a download address on another host, over http or with a port is refused and never fetched"

# The release asset's one-line install, with the token in the environment.
reset_logs; : > "$root/state/codesign.log"
piped "$cloud_install" | env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE \
  AGENT_RELAY_INSTALL_TOKEN="$token" STDIN_CHECK=1 bash > "$root/state/out" 2>&1 \
  || { cat "$root/state/out"; fail "the cloud first install failed"; }
[ "$(current)" = 2026.11.1 ] || fail "the cloud first install points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.1 ] || fail "2026.11.1 is not running"
[ -f "$root/state/loaded-$label" ] || fail "the cloud first install loaded no LaunchAgent"
[ "$(head -n 1 "$prefix/current/product")" = Desktop-x7k2q ] || fail "the product was not recorded"
grep -q "agent-relay-probe$" "$root/state/codesign.log" || fail "the cloud core's signature was not checked"
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
grep -q "size and sha256 verified" "$root/state/out" || fail "the first install did not check the asset"
no_credential "the cloud first install"
! grep -q '\.sha256' "$root/state/curl.log" || fail "the .sha256 sidecar was fetched"
[ ! -s "$root/state/stdin-violations" ] || { cat "$root/state/stdin-violations" >&2; fail "a piped cloud install let a command read the script"; }
pass "a first install with a token: manifest, download-url, the signed address without credentials, then the Developer ID check"

expected_current=2026.11.1
cloud_release 2026.11.2; publish 2026.11.2
reset_logs; : > "$root/state/verify-calls"
cloud --dry-run > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a cloud --dry-run failed"; }
grep -q "would install $prefix/versions/2026.11.2" "$root/state/out" || { cat "$root/state/out"; fail "a cloud --dry-run did not plan the update"; }
[ "$(current)" = 2026.11.1 ] || fail "a cloud --dry-run changed current"
[ ! -d "$prefix/versions/2026.11.2" ] || fail "a cloud --dry-run installed"
pass "--dry-run reads the feed and changes nothing"

touch "$root/state/no-storage"; reset_logs; : > "$root/state/verify-calls"; : > "$root/state/feed-headers-calls"
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the cloud update failed"; }
[ "$(current)" = 2026.11.2 ] || fail "the cloud update points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.2 ] || fail "2026.11.2 is not running"
grep -qxF "GET https://site.test/cloud/api/v1/desktop/feed/assets/a_probe" "$root/state/curl.log" \
  || fail "a 501 did not fall back to streaming the asset"
grep -q -- "--profile x7k2q --public-key $public_key --installed-version 2026.11.1" "$root/state/verify-calls" \
  || { cat "$root/state/verify-calls"; fail "the installed core did not verify the manifest"; }
grep -q "verified by the installed core" "$root/state/out" || fail "the update did not say who verified it"
grep -q -- "--site https://site.test --workspace ws_fixture --account user_fixture --profile x7k2q" \
  "$root/state/feed-headers-calls" || fail "the update did not ask the installed core for headers"
grep -A3 -F -- "--- https://site.test/cloud/api/v1/desktop/feed/assets/a_probe" "$root/state/headers.log" \
  | grep -qxF "Authorization: Bearer sh_dfa_core-fixture" || fail "the stream lacked the core's headers"
no_credential "the cloud update"
rm -f "$root/state/no-storage"
pass "an update without a token uses the core's feed-headers, its feed verify, and the 501 stream fallback"

# A scheduled Cloud check uses the existing signed-in core and gives up if
# it cannot answer. Use an unchanged release to isolate authentication.
printf '%s\n' arm64 > "$root/state/native-arch"
cloud --schedule-enable > "$root/state/out"
cloud_job="$(find "$HOME/Library/LaunchAgents" -name "$label.update.*.plist")"
reset_logs
schedule_tick "$cloud_job" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "scheduled Cloud check failed"; }
grep -qxF 'Authorization: Bearer sh_dfa_core-fixture' "$root/state/headers.log" || fail "scheduled check did not use the core credential"
no_credential "the scheduled Cloud check"
rm "$root/state/running"
reset_logs
if schedule_tick "$cloud_job" > "$root/state/out" 2>&1; then fail "scheduled Cloud check accepted a stopped core"; fi
grep -q 'require a running, signed-in core' "$root/state/out" || fail "scheduled Cloud stopped-core guard was not reached"
[ ! -s "$root/state/curl.log" ] && [ ! -e "$root/state/running" ] || fail "scheduled check contacted Cloud or started the core"
printf '%s\n' 2026.11.2 > "$root/state/running"
cloud --schedule-disable > "$root/state/out"
pass "scheduled Cloud checks use core-held credentials and skip a stopped core"

expected_current=2026.11.2
echo '{"ok":false,"code":"signature_invalid","message":"x"}' > "$root/state/verify-answer"
refused "a manifest the installed core refuses" "(signature_invalid)"
echo '{"ok":false,"code":"not_newer","message":"x"}' > "$root/state/verify-answer"
reset_logs
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a not-newer feed failed"; }
grep -q "already up to date" "$root/state/out" || fail "a manifest that is not newer was not up to date"
! grep -q download-url "$root/state/curl.log" || fail "a manifest that is not newer downloaded an asset"
rm -f "$root/state/verify-answer"
pass "the installed core's refusal stops the update; not_newer is already up to date"

reset_logs
if cloud --version 2026.11.9 > "$root/state/out" 2>&1; then fail "a --version the feed does not offer was accepted"; fi
grep -qF "the release feed offers 2026.11.2 only; --version 2026.11.9 is not available" "$root/state/out" \
  || { cat "$root/state/out"; fail "--version did not name the feed's release"; }
cloud --version 2026.11.2 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--version of the feed's release failed"; }
grep -q "already up to date" "$root/state/out" || fail "--version of the installed release was not up to date"
pass "--version installs only the release the feed offers"

cloud --status > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a cloud --status failed"; }
grep -qF "installed: 2026.11.2 ($prefix/current/agent-relay-probe)" "$root/state/out" || fail "--status does not name the version"
grep -q "product: Desktop-x7k2q" "$root/state/out" || fail "--status does not name the product"
grep -q "core: answering, version 2026.11.2" "$root/state/out" || fail "--status does not say the core answers"
reset_logs
cloud --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a cloud --rollback failed"; }
{ [ "$(current)" = 2026.11.1 ] && [ "$(cat "$root/state/running")" = 2026.11.1 ]; } || fail "a cloud --rollback did not run 2026.11.1"
[ ! -s "$root/state/curl.log" ] || fail "a cloud --rollback reached the feed"
pass "--status and --rollback work for a cloud install, and --rollback needs no feed"

# A stopped core is started before it is asked who is signed in, and a
# signed-out one is still told apart from one that is not running.
expected_current=2026.11.1
rm -f "$root/state/running"; touch "$root/state/signed-out"
refused "a stopped, signed-out core" "is not signed in"
grep -q "not running; starting it" "$root/state/out" || fail "the stopped core was not reported as not running"
[ "$(cat "$root/state/running" 2>/dev/null)" = 2026.11.1 ] || fail "the stopped core was not started"
rm -f "$root/state/signed-out" "$root/state/running"
cloud_release 2026.11.3; publish 2026.11.3
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the update of a stopped core failed"; }
grep -q "not running; starting it" "$root/state/out" || fail "the update did not start the stopped core"
[ "$(current)" = 2026.11.3 ] || fail "the update of a stopped core points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.3 ] || fail "2026.11.3 is not running"
no_credential "the update of a stopped core"
pass "an update without a token starts a stopped core first; not running and not signed in are told apart"

# An installed core without feed verify: the update checks the signature
# with OpenSSL 3, as a first install does, and refuses LibreSSL.
expected_current=2026.11.3
cloud_release 2026.11.4; publish 2026.11.4
touch "$root/state/no-feed-verify"; : > "$root/state/verify-calls"
use_openssl "LibreSSL 3.3.6"
refused "an update with LibreSSL and no feed verify" \
  "the openssl on PATH is LibreSSL, which cannot verify it, and there is no installed core with feed verify"
! grep -q a_probe/download-url "$root/state/curl.log" || fail "the LibreSSL update requested an asset"
[ ! -d "$prefix/versions/2026.11.4" ] || fail "the LibreSSL update installed 2026.11.4"
use_openssl real
reset_logs
cloud > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the OpenSSL 3 update failed"; }
grep -q "verified by OpenSSL" "$root/state/out" || { cat "$root/state/out"; fail "the update did not verify with OpenSSL"; }
[ ! -s "$root/state/verify-calls" ] || fail "a core without feed verify was asked to verify"
[ "$(current)" = 2026.11.4 ] || fail "the OpenSSL 3 update points at $(current)"
[ "$(cat "$root/state/running")" = 2026.11.4 ] || fail "2026.11.4 is not running"
no_credential "the OpenSSL 3 update"
rm -f "$root/state/no-feed-verify"
pass "an update whose installed core has no feed verify verifies with OpenSSL 3 and refuses LibreSSL"

cloud --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a cloud --uninstall failed"; }
{ [ ! -e "$plist" ] && [ ! -e "$prefix" ] && [ ! -e "$root/state/loaded-$label" ]; } || fail "a cloud --uninstall left the core behind"
[ "$(sha256sum "$data/injectable-agents.json")" = "$data_sum" ] || fail "the core's data changed"
pass "a cloud install's --uninstall removes it and keeps the core's data"

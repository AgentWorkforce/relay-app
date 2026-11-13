#!/usr/bin/env bash
# Tests scripts/agent-relay-desktop-update.sh against a fake publisher feed
# (python http.server over TLS on 127.0.0.1, trusted through
# CURL_CA_BUNDLE) and a fake gh on PATH. Nothing leaves the machine.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/agent-relay-desktop-update.sh"
for tool in python3 openssl jq curl sha256sum; do
  command -v "$tool" > /dev/null || { echo "test-agent-relay-desktop-update: needs $tool" >&2; exit 1; }
done

root="$(mktemp -d)"
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; fi
  rm -rf -- "$root"
}
trap cleanup EXIT

failures=0
pass() { echo "ok - $1"; }
bad() { echo "not ok - $1" >&2; failures=$((failures + 1)); }
check() { local name="$1"; shift; if "$@"; then pass "$name"; else bad "$name"; fi; }
not_grep() { ! grep -q "$@"; }

TOKEN="sh_dfp_$(od -An -N20 -tx1 /dev/urandom | tr -d ' \n')"
PROFILE=x7k2q
VERSION=2026.11.4
BETA=2026.11.4-beta.2

# A self-signed certificate for 127.0.0.1.
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=127.0.0.1 \
  -addext subjectAltName=IP:127.0.0.1 -keyout "$root/key.pem" -out "$root/cert.pem" > /dev/null 2>&1

# The fake feed: a mode file picks the answers; every request is logged as
# a JSON line (method, path, the headers that matter, body sha256).
cat > "$root/feed.py" <<'PY'
import hashlib, json, os, ssl, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

root = sys.argv[1]
token = os.environ["FAKE_FEED_TOKEN"]
base = "/cloud/api/v1/desktop/feed"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def answer(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def serve(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        with open(os.path.join(root, "mode")) as f:
            mode = f.read().strip()
        names = ("Authorization", "X-Agent-Relay-Profile", "Content-Type", "X-Content-SHA256", "Content-Length")
        entry = {"method": self.command, "path": self.path, "body_sha256": hashlib.sha256(body).hexdigest(),
                 "body": body.decode() if self.command == "POST" else None,
                 "headers": {n: self.headers.get(n) for n in names if self.headers.get(n) is not None}}
        with open(os.path.join(root, "requests.jsonl"), "a") as f:
            f.write(json.dumps(entry) + "\n")
        if self.headers.get("Authorization") != f"Bearer {token}":
            return self.answer(401, {"error": "unauthenticated", "message": "refused"})
        if mode == "forbidden":
            return self.answer(403, {"error": "not_entitled", "message": f"refused {token}"})
        if self.headers.get("X-Agent-Relay-Profile") != "x7k2q":
            return self.answer(400, {"error": "profile_header_required", "message": "refused"})
        if not self.path.startswith(base):
            return self.answer(404, {"error": "not_found", "message": "refused"})
        parts = self.path[len(base):].split("/")[1:]
        if self.command == "POST" and parts == ["releases"]:
            if mode == "exists":
                return self.answer(409, {"error": "version_exists", "message": "a different release holds it"})
            request = json.loads(body)
            return self.answer(200 if mode == "resume" else 201,
                               {"version": request["version"], "state": "draft"})
        if self.command == "PUT" and len(parts) == 4 and parts[2] == "assets":
            if mode == "mismatch":
                return self.answer(422, {"error": "asset_mismatch", "message": "the bytes differ"})
            if hashlib.sha256(body).hexdigest() != self.headers.get("X-Content-SHA256"):
                return self.answer(422, {"error": "asset_mismatch", "message": "the bytes differ"})
            return self.answer(201, {"id": "a_00000000", "name": parts[3]})
        if self.command == "POST" and parts[2:] == ["publish"]:
            return self.answer(200, {"manifest": {"version": parts[1]}, "signature": "c2ln", "key_id": "k1"})
        return self.answer(404, {"error": "not_found", "message": "refused"})

    do_POST = do_PUT = do_GET = serve


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(os.path.join(root, "cert.pem"), os.path.join(root, "key.pem"))
server.socket = context.wrap_socket(server.socket, server_side=True)
with open(os.path.join(root, "port.tmp"), "w") as f:
    f.write(str(server.server_address[1]))
os.rename(os.path.join(root, "port.tmp"), os.path.join(root, "port"))
server.serve_forever()
PY
echo created > "$root/mode"
FAKE_FEED_TOKEN="$TOKEN" python3 -I "$root/feed.py" "$root" &
server_pid=$!
for _ in $(seq 1 100); do [ -f "$root/port" ] && break; sleep 0.1; done
[ -f "$root/port" ] || { echo "test-agent-relay-desktop-update: the fake feed did not start" >&2; exit 1; }
SITE="https://127.0.0.1:$(cat "$root/port")"

# The fake gh: release download copies the fixture bundle; attestation
# verify succeeds unless FAKE_ATTEST=fail. Every call is logged.
mkdir "$root/bin"
cat > "$root/bin/gh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
case "$1 $2" in
  "release download")
    dir=""
    while [ "$#" -gt 0 ]; do [ "$1" = --dir ] && dir="$2"; shift; done
    cp -- "$FAKE_BUNDLE"/* "$dir"/ ;;
  "attestation verify")
    [ "${FAKE_ATTEST:-ok}" = ok ] ;;
  *) echo "fake gh: unexpected $*" >&2; exit 9 ;;
esac
SH
chmod +x "$root/bin/gh"
# The fake curl: logs its arguments (never the token, which reaches curl in
# a header file), then exits FAKE_CURL_EXIT or runs the real curl.
real_curl="$(command -v curl)"
cat > "$root/bin/curl" <<SH
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "\$FAKE_CURL_LOG"
if [ -n "\${FAKE_CURL_EXIT:-}" ]; then echo "curl: (\$FAKE_CURL_EXIT) fake failure" >&2; exit "\$FAKE_CURL_EXIT"; fi
exec "$real_curl" "\$@"
SH
chmod +x "$root/bin/curl"

# A bundle as Desktop Promote attaches it. BUNDLE_PROFILE is the profile
# pull-bundle.json names, ASSET_PROFILE the one the package is named for;
# both default to $PROFILE.
make_bundle() {
  local bundle="$1" version="$2" channel="$3" declared="${4:-$2}" name
  local for_profile="${BUNDLE_PROFILE:-$PROFILE}" package="Desktop-${ASSET_PROFILE:-$PROFILE}-$2-Linux-x64.deb"
  rm -rf -- "$bundle"
  mkdir -p "$bundle"
  printf 'deb %s\n' "$version" > "$bundle/$package"
  printf '#!/bin/sh\necho install\n' > "$bundle/install.sh"
  printf "What's new\n" > "$bundle/release-notes.md"
  cp "$script" "$bundle/agent-relay-desktop-update.sh"
  local assets='[]'
  for name in "$package" install.sh; do
    assets="$(jq -c --arg name "$name" --arg sha "$(sha256sum < "$bundle/$name" | cut -d' ' -f1)" \
      --argjson size "$(wc -c < "$bundle/$name")" '. + [{name: $name, platform: "Linux", arch: "x64", sha256: $sha, size: $size}]' <<< "$assets")"
  done
  jq -cn --arg version "$declared" --arg channel "$channel" --argjson assets "$assets" \
    '{version: $version, channel: $channel, notes: "What'"'"'s new\n", expected_assets: $assets}' | tr -d '\n' > "$bundle/publish-request.json"
  jq -cn --arg profile "$for_profile" --arg version "$version" --arg channel "$channel" \
    '{profile: $profile, version: $version, channel: $channel}' > "$bundle/pull-bundle.json"
  (cd "$bundle" && sha256sum -- pull-bundle.json publish-request.json release-notes.md agent-relay-desktop-update.sh \
    "$package" install.sh > SHA256SUMS)
  echo '{"mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json"}' > "$bundle/SHA256SUMS.sigstore.json"
}

# Runs the script; leaves status and the files out and err.
run() {
  local mode="$1"; shift
  echo "$mode" > "$root/mode"
  : > "$root/requests.jsonl"
  : > "$root/gh.log"
  : > "$root/curl.log"
  set +e
  env PATH="$root/bin:$PATH" CURL_CA_BUNDLE="$root/cert.pem" FAKE_GH_LOG="$root/gh.log" FAKE_CURL_LOG="$root/curl.log" \
    FAKE_BUNDLE="$BUNDLE" AGENT_RELAY_PULL_REPOSITORY=example-org/desktop-pull AGENT_RELAY_SITE="$SITE" \
    "$@" > "$root/out" 2> "$root/err"
  status=$?
  set -e
  if grep -qF -- "$TOKEN" "$root/out" "$root/err"; then bad "the token never appears in output ($mode)"; fi
}
dl() { echo "$root/dl-$RANDOM-$RANDOM"; }
kinds() { jq -r '"\(.method) \(.path)"' "$root/requests.jsonl"; }

BUNDLE="$root/bundle"
make_bundle "$BUNDLE" "$VERSION" stable

# A missing tool is refused before anything runs.
mkdir "$root/nojq"
for tool in gh curl sha256sum; do ln -s "$(PATH="$root/bin:$PATH" command -v "$tool")" "$root/nojq/$tool"; done
set +e
env PATH="$root/nojq" "$BASH" "$script" --profile "$PROFILE" "$VERSION" > "$root/out" 2> "$root/err"
status=$?
set -e
check "a missing tool exits 1" [ "$status" -eq 1 ]
check "a missing tool is named" grep -q "jq is required" "$root/err"

# The token is never taken from the command line.
run argv bash "$script" --profile "$PROFILE" --token "$TOKEN" "$VERSION"
check "a token on the command line is refused" [ "$status" -eq 1 ]

# --profile is required; the environment does not stand in for it.
run created env AGENT_RELAY_PROFILE="$PROFILE" bash "$script" --dir "$(dl)" "$VERSION"
check "no --profile exits 1" [ "$status" -eq 1 ]
check "no --profile is said" grep -q -- "--profile <id> is required" "$root/err"
check "no --profile downloads nothing" [ ! -s "$root/gh.log" ]
run created bash "$script" --profile Not_An_Id --dir "$(dl)" "$VERSION"
check "a malformed --profile exits 1" [ "$status" -eq 1 ]

# The plan: verified, printed, nothing sent.
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "the plan exits 0" [ "$status" -eq 0 ]
check "the plan sends nothing" [ ! -s "$root/requests.jsonl" ]
check "the plan names each request" grep -qx "PUT $SITE/cloud/api/v1/desktop/feed/releases/$VERSION/assets/install.sh" "$root/out"
check "the plan says nothing was sent" grep -q "Nothing was sent" "$root/out"
check "the attestation verifies SHA256SUMS with its bundle" grep -qE \
  "^attestation verify [^ ]*/SHA256SUMS --bundle [^ ]*/SHA256SUMS.sigstore.json " "$root/gh.log"
check "the signer is Desktop Promote at main, from a run on main" grep -qF \
  -- "--owner AgentWorkforce --cert-identity https://github.com/AgentWorkforce/relay-app/.github/workflows/desktop-promote.yml@refs/heads/main --source-ref refs/heads/main" "$root/gh.log"
check "the signer is never any ref of the workflow" not_grep -- "--signer-workflow" "$root/gh.log"
check "the plan checks the bundle's profile" grep -qx "Checked the bundle is profile $PROFILE's v$VERSION." "$root/out"
run created env AGENT_RELAY_SIGNER_WORKFLOW=example-org/relay-desktop-fork/.github/workflows/desktop-promote.yml \
  bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a fork's signer workflow is pinned to its main too" grep -qF \
  -- "--owner example-org --cert-identity https://github.com/example-org/relay-desktop-fork/.github/workflows/desktop-promote.yml@refs/heads/main --source-ref refs/heads/main" "$root/gh.log"

# The happy path, with the token on standard input.
target="$(dl)"
run created bash "$script" --profile "$PROFILE" --dir "$target" --apply --token-stdin "$VERSION" <<< "$TOKEN"
check "the happy path exits 0" [ "$status" -eq 0 ]
expected="POST /cloud/api/v1/desktop/feed/releases
PUT /cloud/api/v1/desktop/feed/releases/$VERSION/assets/Desktop-$PROFILE-$VERSION-Linux-x64.deb
PUT /cloud/api/v1/desktop/feed/releases/$VERSION/assets/install.sh
POST /cloud/api/v1/desktop/feed/releases/$VERSION/publish"
check "create, each upload, then publish, in order" [ "$(kinds)" = "$expected" ]
check "the create body is publish-request.json byte for byte" [ \
  "$(jq -r 'select(.path | endswith("/releases")) | .body_sha256' "$root/requests.jsonl")" = \
  "$(sha256sum < "$BUNDLE/publish-request.json" | cut -d' ' -f1)" ]
check "every request carries the bearer token and the profile" [ \
  "$(jq -c --arg t "Bearer $TOKEN" 'select(.headers.Authorization != $t or .headers["X-Agent-Relay-Profile"] != "x7k2q")' "$root/requests.jsonl")" = "" ]
check "each upload declares its sha256 and size" [ \
  "$(jq -c 'select(.method == "PUT") | select(.headers["X-Content-SHA256"] != .body_sha256)' "$root/requests.jsonl")" = "" ]
check "the upload Content-Length is the declared size" [ \
  "$(jq -r 'select(.method == "PUT" and (.path | endswith("/install.sh"))) | .headers["Content-Length"]' "$root/requests.jsonl")" = \
  "$(wc -c < "$BUNDLE/install.sh" | tr -d ' ')" ]
check "a stable publish is latest" [ \
  "$(jq -r 'select(.path | endswith("/publish")) | .body' "$root/requests.jsonl")" = '{"latest":true}' ]
check "the create is JSON" [ \
  "$(jq -r 'select(.path | endswith("/releases")) | .headers["Content-Type"]' "$root/requests.jsonl")" = application/json ]
check "the published version and key are printed" grep -qx "Published v$VERSION (stable), signed with key k1." "$root/out"
check "every feed request is bounded after connecting" [ \
  "$(grep -cF -- "--connect-timeout 30 --speed-limit 1024 --speed-time 120 --max-time 3600 " "$root/curl.log")" -eq 4 ]
check "no feed request carries the token in its arguments" not_grep -F -- "$TOKEN" "$root/curl.log"

# A stalled or endless request times out: exit 2, said, nothing further sent.
run created env FAKE_CURL_EXIT=28 DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a timed-out request exits 2" [ "$status" -eq 2 ]
check "a timed-out request is said, with how to resume" grep -q \
  "the request to the feed timed out (.*over 3600 s in all); run the same command again to resume the draft" "$root/err"
check "a timed-out create sends nothing further" [ "$(wc -l < "$root/curl.log")" -eq 1 ]
check "a timed-out request never publishes" not_grep -q "Published" "$root/out"
run created env FAKE_CURL_EXIT=7 DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a failed connection exits 2 with curl's exit" grep -q "the request to the feed failed (curl exit 7)" "$root/err"

# The token from the environment; a resumed draft continues to the uploads.
run resume env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a resumed draft (200) continues" [ "$status" -eq 0 ]
check "a resumed draft is said" grep -q "Resumed the draft v$VERSION." "$root/out"
check "a resumed draft still uploads and publishes" [ "$(kinds | wc -l)" -eq 4 ]

# A beta publishes without latest.
BUNDLE="$root/beta"
make_bundle "$BUNDLE" "$BETA" beta
run created env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$BETA"
check "a beta publishes" [ "$status" -eq 0 ]
check "a beta publish is not latest" [ \
  "$(jq -r 'select(.path | endswith("/publish")) | .body' "$root/requests.jsonl")" = '{}' ]
BUNDLE="$root/bundle"

# --apply without a token.
run created env -u DESKTOP_FEED_PUBLISHER_TOKEN bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "--apply without a token exits 1" [ "$status" -eq 1 ]

# 409 version_exists at create stops before any upload.
run exists env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "409 at create exits 2" [ "$status" -eq 2 ]
check "409 at create shows the feed's error" grep -q "version_exists" "$root/err"
check "409 at create sends no upload" [ "$(kinds)" = "POST /cloud/api/v1/desktop/feed/releases" ]

# 422 at an upload is fatal and named.
run mismatch env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "422 asset_mismatch exits 2" [ "$status" -eq 2 ]
check "422 asset_mismatch is shown" grep -q "asset_mismatch" "$root/err"
check "422 asset_mismatch never publishes" [ "$(kinds | grep -c '/publish$' || true)" -eq 0 ]

# A refused credential, with the feed echoing the token: redacted.
run forbidden env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "403 exits 2" [ "$status" -eq 2 ]
check "403 is a credential failure, the token redacted" grep -q "refused the publisher credential (HTTP 403, not_entitled: refused \*\*\*)" "$root/err"

# A wrong token: 401.
run created env DESKTOP_FEED_PUBLISHER_TOKEN="sh_dfp_wrong" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "401 exits 2" [ "$status" -eq 2 ]

# A failed attestation stops before any check or request.
run created env FAKE_ATTEST=fail DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a failed attestation exits 2" [ "$status" -eq 2 ]
check "a failed attestation sends nothing" [ ! -s "$root/requests.jsonl" ]

# A file that does not match SHA256SUMS.
BUNDLE="$root/tampered"
make_bundle "$BUNDLE" "$VERSION" stable
printf 'other bytes\n' > "$BUNDLE/install.sh"
run created env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a bad sha256 exits 2" [ "$status" -eq 2 ]
check "a bad sha256 is named" grep -q "does not match SHA256SUMS" "$root/err"
check "a bad sha256 sends nothing" [ ! -s "$root/requests.jsonl" ]

# A listed file that was not downloaded.
BUNDLE="$root/missing"
make_bundle "$BUNDLE" "$VERSION" stable
rm "$BUNDLE/install.sh"
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a listed file missing exits 2" [ "$status" -eq 2 ]

# A declaration for another version (SHA256SUMS consistent with it).
BUNDLE="$root/other"
make_bundle "$BUNDLE" "$VERSION" stable 2026.11.5
run created env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a declaration of another version exits 2" [ "$status" -eq 2 ]
check "a declaration of another version is named" grep -q "does not declare v$VERSION" "$root/err"
check "a declaration of another version sends nothing" [ ! -s "$root/requests.jsonl" ]

# A beta declaration asked for as stable: the channel follows the version.
BUNDLE="$root/channel"
make_bundle "$BUNDLE" "$VERSION" beta
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a channel that does not follow the version exits 2" [ "$status" -eq 2 ]

# A declared sha256 that is not the file's (SHA256SUMS consistent).
BUNDLE="$root/declared"
make_bundle "$BUNDLE" "$VERSION" stable
jq -c '.expected_assets[1].sha256 = ("0" * 64)' "$BUNDLE/publish-request.json" | tr -d '\n' > "$BUNDLE/request.tmp"
mv "$BUNDLE/request.tmp" "$BUNDLE/publish-request.json"
(cd "$BUNDLE" && sed -i "s/^[0-9a-f]*  publish-request.json$/$(sha256sum publish-request.json | sed 's/ .*//')  publish-request.json/" SHA256SUMS)
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a declared sha256 that differs exits 2" [ "$status" -eq 2 ]
check "a declared sha256 that differs is named" grep -q "install.sh does not have the declared sha256" "$root/err"

# An intact bundle attested for another profile at the same version is
# refused before any publisher call.
BUNDLE="$root/another"
BUNDLE_PROFILE=m3p9z ASSET_PROFILE=m3p9z make_bundle "$BUNDLE" "$VERSION" stable
run created env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "a bundle for another profile exits 2" [ "$status" -eq 2 ]
check "a bundle for another profile is named" grep -q "was not promoted for profile $PROFILE" "$root/err"
check "a bundle for another profile sends nothing" [ ! -s "$root/requests.jsonl" ]
check "a bundle for another profile calls no publisher" [ ! -s "$root/curl.log" ]

# The profile's pull-bundle.json with another profile's package.
BUNDLE="$root/foreign"
ASSET_PROFILE=m3p9z make_bundle "$BUNDLE" "$VERSION" stable
run created env DESKTOP_FEED_PUBLISHER_TOKEN="$TOKEN" bash "$script" --profile "$PROFILE" --dir "$(dl)" --apply "$VERSION"
check "another profile's package exits 2" [ "$status" -eq 2 ]
check "another profile's package is named" grep -q "Desktop-m3p9z-$VERSION-Linux-x64.deb is not one of profile $PROFILE's v$VERSION packages" "$root/err"
check "another profile's package calls no publisher" [ ! -s "$root/curl.log" ]

# A package name outside the cloud set, even with the profile's prefix.
BUNDLE="$root/oddname"
make_bundle "$BUNDLE" "$VERSION" stable
mv "$BUNDLE/Desktop-$PROFILE-$VERSION-Linux-x64.deb" "$BUNDLE/Desktop-$PROFILE-$VERSION-Linux-x64.run"
sed -i "s/Linux-x64\.deb/Linux-x64.run/g" "$BUNDLE/publish-request.json" "$BUNDLE/SHA256SUMS"
(cd "$BUNDLE" && sed -i "s/^[0-9a-f]*  publish-request.json$/$(sha256sum publish-request.json | sed 's/ .*//')  publish-request.json/" SHA256SUMS)
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a package outside the cloud set exits 2" [ "$status" -eq 2 ]
check "a package outside the cloud set is named" grep -q "Linux-x64.run is not one of profile $PROFILE's v$VERSION packages" "$root/err"

# A bundle without pull-bundle.json (an older or hand-made one).
BUNDLE="$root/unbound"
make_bundle "$BUNDLE" "$VERSION" stable
rm "$BUNDLE/pull-bundle.json"
sed -i '/  pull-bundle.json$/d' "$BUNDLE/SHA256SUMS"
run created bash "$script" --profile "$PROFILE" --dir "$(dl)" "$VERSION"
check "a bundle without pull-bundle.json exits 2" [ "$status" -eq 2 ]
check "a bundle without pull-bundle.json is named" grep -q "does not list pull-bundle.json" "$root/err"

# A download directory whose name holds a backslash: the digests still match.
BUNDLE="$root/bundle"
run created bash "$script" --profile "$PROFILE" --dir "$root/dl-back\\slash" "$VERSION"
check "a --dir with a backslash verifies" [ "$status" -eq 0 ]
check "a --dir with a backslash checks the declaration" grep -q "Checked the declaration: v$VERSION (stable), 2 assets." "$root/out"

# Bad configuration.
BUNDLE="$root/bundle"
run created env AGENT_RELAY_SITE="http://127.0.0.1:1" bash "$script" --profile "$PROFILE" "$VERSION"
check "a site that is not https exits 1" [ "$status" -eq 1 ]
run created bash "$script" --profile "$PROFILE" 2026.13.1
check "a malformed version exits 1" [ "$status" -eq 1 ]
used="$(dl)"; mkdir "$used"; touch "$used/left-over"
run created bash "$script" --profile "$PROFILE" --dir "$used" "$VERSION"
check "a non-empty --dir exits 1" [ "$status" -eq 1 ]

if [ "$failures" -gt 0 ]; then
  echo "$failures failed" >&2
  exit 1
fi
echo "all passed"

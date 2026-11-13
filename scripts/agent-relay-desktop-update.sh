#!/usr/bin/env bash
# Publish one Agent Relay Desktop release to the customer deployment's own
# cloud feed from the pull model's attested bundle
# (docs/enterprise-distribution.md, Pull model).
#
#   agent-relay-desktop-update.sh --profile <id> [--dir <path>] [--token-stdin] [--apply] <version>
#
# Desktop Promote attaches the bundle to the release v<version> of the
# profile's pull repository: publish-request.json (the exact POST /releases
# body, notes already fitted), pull-bundle.json (the profile, version and
# channel the bundle was promoted for), the feed's assets, release-notes.md,
# this script, SHA256SUMS over all of them, and SHA256SUMS.sigstore.json, the
# provenance attestation of SHA256SUMS by the promote workflow. This script
# downloads it, then, in this order:
#   1. verifies the attestation (gh attestation verify): signed by the
#      promote workflow at refs/heads/main, from a run on refs/heads/main;
#   2. checks every file against SHA256SUMS (sha256sum -c);
#   3. checks pull-bundle.json names --profile, <version> and its channel;
#      that publish-request.json declares <version>, its channel, and each
#      asset with the sha256 and size of the downloaded file; and that each
#      asset is install.sh, install.ps1 or one of the profile's packages,
#      Desktop-<profile>-<version>-<platform>...;
#   4. prints the plan: each request's method, URL and header names;
#   5. with --apply only, sends it to the feed's publisher API: POST
#      /releases with publish-request.json byte for byte (201 created, 200
#      resumed), one PUT per declared asset, then POST .../publish. The
#      customer deployment's Control signs the release with its own key.
#      Each request gives up after 30 s without a connection, 120 s below
#      1 KiB/s, or 1 hour in all; the draft stays unpublished and running
#      the same command again resumes it.
# The publisher contract is relay-control (docs/desktop-feed.md,
# openapi.yaml).
#
# Environment:
#   AGENT_RELAY_PULL_REPOSITORY   owner/name of the pull repository.
#   AGENT_RELAY_SITE              the deployment's https origin; the API is
#                                 {site}/cloud/api/v1/desktop/feed.
#   DESKTOP_FEED_PUBLISHER_TOKEN  the publisher token (--apply only), unless
#                                 --token-stdin reads it from standard input.
#   AGENT_RELAY_SIGNER_WORKFLOW   optional, for forks: owner/repo/path of the
#                                 workflow that must have signed SHA256SUMS
#                                 (default AgentWorkforce/relay-app/
#                                 .github/workflows/desktop-promote.yml); its
#                                 owner is the attestation owner. It must
#                                 have signed at refs/heads/main.
# The token is never taken from the command line, never printed, and is
# redacted from any error text the feed returns.
#
# Exit status: 0 published, or the plan printed; 1 usage, configuration or a
# missing tool; 2 verification failed, or the feed refused or failed a request.
set -euo pipefail
umask 077

API_PATH=/cloud/api/v1/desktop/feed
DEFAULT_SIGNER=AgentWorkforce/relay-app/.github/workflows/desktop-promote.yml
# The only ref Desktop Promote is dispatched on (by the release tooling) and the
# one its production environment trusts: an attestation signed by the
# workflow from any other branch is refused.
TRUSTED_REF=refs/heads/main
# Feed requests: no connection in 30 s, under 1 KiB/s for 120 s, or longer
# than an hour in all (the largest package is a few hundred MB) gives up.
CONNECT_TIMEOUT=30
SPEED_LIMIT=1024
SPEED_TIME=120
MAX_TIME=3600
# The packages a cloud feed publishes after Desktop-<profile>-<version>-
# (scripts/release-candidate.py CLOUD_PACKAGE).
CLOUD_PACKAGE='^(macOS[1-9][0-9]*-(arm64|x64)\.dmg|Linux-(arm64|x64)\.(tar\.gz|deb)|Linux-(arm64|x64)-probe\.tar\.gz|Windows-(arm64|x64)(\.zip|-Setup\.exe|-probe\.zip))$'

token=""
work=""
cleanup() {
  token=""
  if [ -n "$work" ]; then rm -rf -- "$work"; fi
}
trap cleanup EXIT

usage() {
  echo "usage: agent-relay-desktop-update.sh --profile <id> [--dir <path>] [--token-stdin] [--apply] <version>" >&2
  exit 1
}
fail() {
  echo "agent-relay-desktop-update: $1" >&2
  exit 1
}
refuse() {
  echo "agent-relay-desktop-update: $1" >&2
  exit 2
}
redact() {
  local text="$1"
  if [ -n "$token" ]; then text="${text//"$token"/***}"; fi
  printf '%s' "$text" | tr -d '\000-\010\013-\037\177'
}

version="" dir="" apply="" token_stdin="" profile=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --apply) apply=yes ;;
    --token-stdin) token_stdin=yes ;;
    --dir) [ "$#" -ge 2 ] || usage; dir="$2"; shift ;;
    --dir=*) dir="${1#--dir=}" ;;
    --profile) [ "$#" -ge 2 ] || usage; profile="$2"; shift ;;
    --profile=*) profile="${1#--profile=}" ;;
    -h|--help) usage ;;
    --token|--token=*) fail "the token is never taken from the command line: set DESKTOP_FEED_PUBLISHER_TOKEN or use --token-stdin" ;;
    -*) usage ;;
    *) [ -z "$version" ] || usage; version="$1" ;;
  esac
  shift
done
[ -n "$version" ] || usage
[ -n "$profile" ] || fail "--profile <id> is required: the profile the release is published for (X-Agent-Relay-Profile)"

for tool in gh curl jq sha256sum; do
  command -v "$tool" > /dev/null 2>&1 || fail "$tool is required and was not found on PATH"
done

# The cloud feed's version grammar (scripts/desktop_feed.py VERSION).
[[ "$version" =~ ^[0-9]{4}\.([1-9]|1[0-2])\.(0|[1-9][0-9]*)(-beta\.(0|[1-9][0-9]*))?$ ]] \
  || fail "the version must be YYYY.M.N or YYYY.M.N-beta.N"
if [[ "$version" == *-beta.* ]]; then channel=beta; else channel=stable; fi

repository="${AGENT_RELAY_PULL_REPOSITORY:-}"
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail "AGENT_RELAY_PULL_REPOSITORY must be owner/name"
site="${AGENT_RELAY_SITE:-}"
site="${site%/}"
[[ "$site" =~ ^https://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?$ ]] \
  || fail "AGENT_RELAY_SITE must be an https origin such as https://relay.example"
if ! [[ "$profile" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] || [ "${#profile}" -gt 40 ]; then
  fail "--profile must be the profile's id: lowercase letters and digits, single hyphens between them"
fi
signer="${AGENT_RELAY_SIGNER_WORKFLOW:-$DEFAULT_SIGNER}"
[[ "$signer" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/.+$ ]] || fail "AGENT_RELAY_SIGNER_WORKFLOW must be owner/repo/path"
owner="${signer%%/*}"
base="$site$API_PATH"

# The token: standard input with --token-stdin, else the environment. Only
# --apply needs it; it never reaches a child process's environment or argv.
if [ -n "$token_stdin" ]; then
  IFS= read -r token || true
  token="${token%$'\r'}"
  [ -n "$token" ] || fail "--token-stdin read no token"
else
  token="${DESKTOP_FEED_PUBLISHER_TOKEN:-}"
fi
unset DESKTOP_FEED_PUBLISHER_TOKEN
if [ -n "$apply" ] && [ -z "$token" ]; then
  fail "--apply needs the publisher token: set DESKTOP_FEED_PUBLISHER_TOKEN or use --token-stdin"
fi
case "$token" in *[[:space:]]*) fail "the publisher token must not contain whitespace" ;; esac

work="$(mktemp -d)"
if [ -z "$dir" ]; then
  dir="$(mktemp -d)"
  echo "Downloading into $dir"
elif [ -e "$dir" ]; then
  if [ ! -d "$dir" ] || [ -n "$(ls -A -- "$dir")" ]; then fail "--dir must be an empty or new directory"; fi
else
  mkdir -p -- "$dir"
fi

# 1. The bundle, and its attestation.
gh release download "v$version" --repo "$repository" --dir "$dir" \
  || refuse "downloading release v$version from the pull repository failed"
for name in SHA256SUMS SHA256SUMS.sigstore.json publish-request.json; do
  [ -f "$dir/$name" ] || refuse "the release has no $name"
done
# The certificate identity is the signing workflow at its ref, exactly; the
# source ref is the ref the run was dispatched on. Both must be main.
gh attestation verify "$dir/SHA256SUMS" --bundle "$dir/SHA256SUMS.sigstore.json" \
  --owner "$owner" --cert-identity "https://github.com/$signer@$TRUSTED_REF" --source-ref "$TRUSTED_REF" \
  || refuse "SHA256SUMS is not attested by $signer at $TRUSTED_REF"
echo "Verified the attestation of SHA256SUMS ($signer at $TRUSTED_REF)."

# 2. Every listed file, present and unchanged.
listed=()
while IFS= read -r line; do
  [[ "$line" =~ ^[0-9a-f]{64}\ \ ([A-Za-z0-9][A-Za-z0-9._-]*)$ ]] || refuse "SHA256SUMS has a line that is not <sha256>  <bare name>"
  listed+=("${BASH_REMATCH[1]}")
done < "$dir/SHA256SUMS"
[ "${#listed[@]}" -gt 0 ] || refuse "SHA256SUMS lists no files"
for name in "${listed[@]}"; do
  if [ ! -f "$dir/$name" ] || [ -L "$dir/$name" ]; then refuse "$name is listed in SHA256SUMS but was not downloaded"; fi
done
(cd "$dir" && sha256sum --check --strict --quiet SHA256SUMS) || refuse "a file does not match SHA256SUMS"
echo "Checked ${#listed[@]} files against SHA256SUMS."
is_listed() {
  local want="$1" name
  for name in "${listed[@]}"; do [ "$name" = "$want" ] && return 0; done
  return 1
}
is_listed publish-request.json || refuse "SHA256SUMS does not list publish-request.json"
is_listed pull-bundle.json || refuse "SHA256SUMS does not list pull-bundle.json, the profile the bundle is for"

# 3. The bundle's profile, then the declaration: this version and channel,
# and each asset as downloaded and named for this profile.
jq -e --arg profile "$profile" --arg version "$version" --arg channel "$channel" '
  type == "object" and .profile == $profile and .version == $version and .channel == $channel
' "$dir/pull-bundle.json" > /dev/null \
  || refuse "the bundle was not promoted for profile $profile, v$version on the $channel channel (pull-bundle.json)"
echo "Checked the bundle is profile $profile's v$version."
request="$dir/publish-request.json"
jq -e --arg version "$version" --arg channel "$channel" '
  type == "object" and .version == $version and .channel == $channel and
  (.expected_assets | type == "array" and length > 0) and
  all(.expected_assets[]; type == "object" and (.name | type == "string" and test("^[A-Za-z0-9][A-Za-z0-9._-]*$"))
      and (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))
      and (.size | type == "number" and . >= 1 and . == floor))
' "$request" > /dev/null || refuse "publish-request.json does not declare v$version on the $channel channel with well-formed assets"
names=() sums=() sizes=()
while IFS=$'\t' read -r name sum size; do
  names+=("$name") sums+=("$sum") sizes+=("$size")
done < <(jq -r '.expected_assets[] | [.name, .sha256, (.size | tostring)] | @tsv' "$request")
for i in "${!names[@]}"; do
  name="${names[$i]}"
  is_listed "$name" || refuse "$name is declared but not listed in SHA256SUMS"
  case "$name" in
    install.sh|install.ps1) ;;
    "Desktop-$profile-$version-"*)
      [[ "${name#"Desktop-$profile-$version-"}" =~ $CLOUD_PACKAGE ]] \
        || refuse "$name is not one of profile $profile's v$version packages" ;;
    *) refuse "$name is not one of profile $profile's v$version packages" ;;
  esac
  # From standard input: sha256sum escapes a file name holding a backslash
  # or newline, which would corrupt the digest read from its output.
  actual_sum="$(sha256sum < "$dir/$name")"
  actual_sum="${actual_sum%% *}"
  actual_size="$(wc -c < "$dir/$name" | tr -d '[:space:]')"
  [ "$actual_sum" = "${sums[$i]}" ] || refuse "$name does not have the declared sha256"
  [ "$actual_size" = "${sizes[$i]}" ] || refuse "$name does not have the declared size"
done
echo "Checked the declaration: v$version ($channel), ${#names[@]} assets."

# 4. The plan.
if [ "$channel" = stable ]; then publish_body='{"latest":true}'; else publish_body='{}'; fi
echo "Plan for $base (profile $profile):"
echo "POST $base/releases"
echo "  headers: Authorization, X-Agent-Relay-Profile, Content-Type"
echo "  body: publish-request.json"
for i in "${!names[@]}"; do
  echo "PUT $base/releases/$version/assets/${names[$i]}"
  echo "  headers: Authorization, X-Agent-Relay-Profile, Content-Type, X-Content-SHA256, Content-Length"
done
echo "POST $base/releases/$version/publish"
echo "  headers: Authorization, X-Agent-Relay-Profile, Content-Type"
echo "  body: $publish_body"
if [ -z "$apply" ]; then
  echo "Nothing was sent. Run again with --apply to publish."
  exit 0
fi

# 5. Publish. The token reaches curl through a private header file, never
# its argv.
headers="$work/headers"
printf 'Authorization: Bearer %s\nX-Agent-Relay-Profile: %s\n' "$token" "$profile" > "$headers"
response="$work/response"
# Sends one request; prints the HTTP status. Never follows a redirect, and
# gives up on a stalled or endless transfer (CONNECT_TIMEOUT, SPEED_LIMIT,
# SPEED_TIME, MAX_TIME).
call() {
  local status code=0
  status="$(curl --silent --show-error --proto =https --connect-timeout "$CONNECT_TIMEOUT" \
    --speed-limit "$SPEED_LIMIT" --speed-time "$SPEED_TIME" --max-time "$MAX_TIME" --output "$response" \
    --write-out '%{http_code}' -H @"$headers" -H 'Expect:' "$@")" || code=$?
  case "$code" in
    0) ;;
    28) refuse "the request to the feed timed out (no connection in ${CONNECT_TIMEOUT} s, under $SPEED_LIMIT bytes/s for ${SPEED_TIME} s, or over ${MAX_TIME} s in all); run the same command again to resume the draft" ;;
    *) refuse "the request to the feed failed (curl exit $code); run the same command again to resume the draft" ;;
  esac
  printf '%s' "$status"
}
# The feed's error code and message, redacted.
error_text() {
  local code message
  code="$(jq -r 'if type == "object" then (.error // "") else "" end' "$response" 2>/dev/null || true)"
  message="$(jq -r 'if type == "object" then (.message // "") else "" end' "$response" 2>/dev/null || true)"
  redact "${code:-unknown}${message:+: $message}"
}

status="$(call -X POST -H 'Content-Type: application/json' --data-binary @"$request" "$base/releases")"
case "$status" in
  201) echo "Created the draft v$version." ;;
  200) echo "Resumed the draft v$version." ;;
  401|403) refuse "the feed refused the publisher credential (HTTP $status, $(error_text))" ;;
  409) refuse "creating v$version was refused (HTTP 409, $(error_text)); a different v$version already exists" ;;
  *) refuse "creating v$version failed (HTTP $status, $(error_text))" ;;
esac

for i in "${!names[@]}"; do
  name="${names[$i]}"
  status="$(call -X PUT -H 'Content-Type: application/octet-stream' -H "X-Content-SHA256: ${sums[$i]}" \
    -H "Content-Length: ${sizes[$i]}" --upload-file "$dir/$name" "$base/releases/$version/assets/$name")"
  case "$status" in
    200) echo "Already uploaded $name." ;;
    201) echo "Uploaded $name." ;;
    401|403) refuse "the feed refused the publisher credential (HTTP $status, $(error_text))" ;;
    *) refuse "uploading $name was refused (HTTP $status, $(error_text)); the draft stays unpublished" ;;
  esac
done

status="$(call -X POST -H 'Content-Type: application/json' --data-binary "$publish_body" \
  "$base/releases/$version/publish")"
case "$status" in
  200) ;;
  401|403) refuse "the feed refused the publisher credential (HTTP $status, $(error_text))" ;;
  *) refuse "publishing v$version was refused (HTTP $status, $(error_text)); the draft stays unpublished" ;;
esac
published="$(jq -r '.manifest.version // empty' "$response" 2>/dev/null || true)"
key_id="$(jq -r '.key_id // empty' "$response" 2>/dev/null || true)"
[ "$published" = "$version" ] || refuse "the feed answered the publish without a manifest for v$version"
echo "Published v$(redact "$published") ($channel), signed with key $(redact "${key_id:-unknown}")."

#!/bin/bash
# Submission bridge only: trusted main's code, never review code or host keys.
set -euo pipefail
unset GH_TOKEN GITHUB_TOKEN DESKTOP_RELEASE_GH_TOKEN FLOWS_CLOUD_TOKEN FLOWS_CLOUD_URL
secret() { sed -n -E "s/^(export +)?$1=//p" "${REVIEW_SECRETS_FILE:-/run/agent-host/secrets.env}" 2>/dev/null | tail -1 | sed -E "s/^(['\"])(.*)\1\$/\2/"; }
FLOWS_CLOUD_TOKEN=$(secret FLOWS_CLOUD_TOKEN)
[ -n "$FLOWS_CLOUD_TOKEN" ] || { echo 'Cloud release refused: no FLOWS_CLOUD_TOKEN; no local fallback' >&2; exit 3; }
export FLOWS_CLOUD_TOKEN
cloud_url=$(secret FLOWS_CLOUD_URL)
if [ -n "$cloud_url" ]; then export FLOWS_CLOUD_URL="$cloud_url"; fi
# Resolve a receipt relative to the caller before changing to the upload clone.
args=()
while [ $# -gt 0 ]; do
  if [ "$1" = --readiness ]; then
    [ -n "${2:-}" ] || { echo '--readiness needs a file' >&2; exit 2; }
    case "$2" in /*) receipt=$2 ;; *) receipt=$PWD/$2 ;; esac
    args+=(--readiness "$receipt"); shift 2
  else args+=("$1"); shift
  fi
done
repo=$HOME/build/relay-app
state=$HOME/state/cloud-releases
mkdir -p "$state"
(
  flock -w 600 9
  git -C "$repo" fetch -q origin main
) 9> "$HOME/state/.tick.lock"
# A temporary checkout per submission; retain pointers and diagnostics only.
cleanup() {
  local status=$? pointer
  trap - EXIT
  if [ -d "$work/source/release-state" ]; then
    if ! { mkdir -p "$work/release-state" && cp -R "$work/source/release-state/." "$work/release-state/"; }; then
      echo "could not retain release diagnostics in $work/release-state" >&2
      [ "$status" -ne 0 ] || status=1
    fi
    for pointer in run.json request.json; do
      if [ -f "$work/source/release-state/$pointer" ]; then
        cp "$work/source/release-state/$pointer" "$work/$pointer" || { [ "$status" -ne 0 ] || status=1; }
      fi
    done
  fi
  rm -rf -- "$work/source" || { [ "$status" -ne 0 ] || status=1; }
  exit "$status"
}
work=$(mktemp -d "$state/request.XXXXXX")
trap cleanup EXIT
git clone -q --shared --no-checkout "$repo" "$work/source"
git -C "$work/source" checkout -q --detach "$(git -C "$repo" rev-parse origin/main)"
cd "$work/source"
npm --prefix flows ci --no-audit --no-fund --ignore-scripts >&2
if node flows/lib/release-launcher.mjs "${args[@]}"; then
  cp release-state/run.json "$work/run.json"
  cp release-state/request.json "$work/request.json"
else
  echo "submission failed; inspect $work/release-state before retrying" >&2
  exit 1
fi

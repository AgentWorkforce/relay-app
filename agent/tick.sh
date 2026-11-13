#!/bin/bash
# Every 30 minutes (agent-host-tick.timer): keep a signed release candidate of
# main ready, so publishing a beta or stable is a ~10-minute promote of bytes
# already built instead of a full build.
#
#  1. Refresh the status (and version) of candidates already dispatched.
#  2. If a Desktop Release run is already queued or running, stop: they share
#     one concurrency group and a third trigger cancels a pending one.
#  3. Diff origin/main against the newest candidate for each channel. If main
#     has not moved, or moved only in paths that select no CI lane (docs,
#     changelog), there is nothing to build. Otherwise dispatch ONE
#     prepare-only Desktop Release for the first channel that is behind.
#     Stable candidates are prepared by the workflow itself (workflow_run after
#     Desktop CI passes on main): the tick adopts
#     those runs instead of dispatching stable. Only beta is dispatched here.
#  A sha whose candidate failed is not retried for that channel until main moves.
#
# The source sha of a candidate is the "Source <sha>" line of its Resolve
# version job, never the run's head_sha: a workflow_run's head_sha can be a
# newer main than the commit that was tested and built.
#
# State: ~/state/candidates.json, a list of
#   {channel, sha, run_id, dispatched_at, status, version, auto}
set -euo pipefail
# gh reads the current token from /run/agent-host/gh-token via /usr/local/bin/gh;
# an inherited GH_TOKEN (possibly stale) must not shadow it.
unset GH_TOKEN GITHUB_TOKEN
# Activate after provisioning the Cloud-owned credential and a prepare smoke run.
# A Cloud refusal never falls back to host orchestration.
if [ "${DESKTOP_RELEASE_ORCHESTRATOR:-legacy}" = cloud ]; then
  if [ "${1:-}" = --refresh ]; then exec /opt/agent-host/cloud-release.sh inventory beta; fi
  exec /opt/agent-host/cloud-release.sh tick beta
fi
REPO=AgentWorkforce/relay-app
WF=desktop-release.yml
state=$HOME/state/candidates.json
mkdir -p "$HOME/state"
[ -f "$state" ] || echo '[]' > "$state"
exec 9> "$HOME/state/.tick.lock"; flock -n 9 || { echo "tick already running"; exit 0; }
cd "$HOME/build/relay-app"
git fetch -q origin main
# Tags separately: a local tag that differs from origin's ("would clobber
# existing tag") must not stop the run, but it is worth surfacing.
git fetch -q --tags origin 2>/dev/null || echo "warning: tag fetch failed (a local tag differs from origin?)" >&2
head=$(git rev-parse origin/main)

resolve_log() {  # the "Resolve version" job log of a Desktop Release run
  local job
  job=$(gh api "repos/$REPO/actions/runs/$1/jobs" --jq '.jobs[] | select(.name=="Resolve version") | .id' | head -1)
  [ -n "$job" ] && gh api --allow-escape-sequences "repos/$REPO/actions/jobs/$job/logs" 2>/dev/null
}
run_version() {  # the version a run resolved: "Releasing v<version>"
  resolve_log "$1" | grep -oE 'Releasing v[0-9][0-9A-Za-z.-]*' | head -1 | cut -c12-
}
run_source() {  # the commit a run tested and built: "Source <sha>"
  resolve_log "$1" | grep -oE 'Source [0-9a-f]{40}' | head -1 | cut -c8-
}

# 1a. Adopt automatic stable candidates: workflow_run-triggered Desktop Release runs the tick did not dispatch.
tmp=$(mktemp)
# A run that skipped (cancelled CI, or no Unreleased bullet) is no candidate: don't adopt it.
# Oldest first, so the file's order is time order and "last" is the newest.
gh run list -R "$REPO" --workflow "$WF" --event workflow_run --limit 10 --json databaseId,createdAt,conclusion \
  --jq 'sort_by(.createdAt) | .[] | select(.conclusion != "skipped")' | while read -r r; do
  id=$(jq -r .databaseId <<<"$r")
  if ! jq -e --argjson id "$id" 'any(.[]; .run_id == $id)' "$state" >/dev/null; then
    jq --argjson id "$id" --arg at "$(jq -r .createdAt <<<"$r")" \
      '. + [{channel: "stable", sha: null, run_id: $id, dispatched_at: $at, status: "pending", version: null, auto: true}] | .[-40:]' \
      "$state" > "$tmp" && mv "$tmp" "$state"
    echo "adopted automatic stable candidate run $id"
  fi
done

# 1b. Refresh pending candidates. A successful run's version and source come from its log; an automatic
#     run that skipped (no release note bullet) succeeds with no version and is recorded as skipped.
jq -c '.[]' "$state" | while read -r c; do
  if [ "$(jq -r .status <<<"$c")" = pending ] && [ "$(jq -r .run_id <<<"$c")" != null ]; then
    id=$(jq -r .run_id <<<"$c")
    # A gh or log failure leaves the candidate pending for the next tick;
    # only a log that was read and names no version means the run skipped.
    if read -r st concl < <(gh run view "$id" -R "$REPO" --json status,conclusion --jq '"\(.status) \(.conclusion)"' 2>/dev/null) && [ "$st" = completed ]; then
      v=""; src=""; log_ok=1
      if [ "$concl" = success ]; then
        if log=$(resolve_log "$id") && [ -n "$log" ]; then
          v=$(grep -oE 'Releasing v[0-9][0-9A-Za-z.-]*' <<<"$log" | head -1 | cut -c12- || true)
          src=$(grep -oE 'Source [0-9a-f]{40}' <<<"$log" | head -1 | cut -c8- || true)
        else
          log_ok=0
        fi
      fi
      if [ "$log_ok" = 1 ]; then
        c=$(jq -c --arg s "$concl" --arg v "$v" --arg src "$src" \
          '.status = (if $s == "skipped" then "skipped" elif $s != "success" then "failed:" + $s elif $v == "" then "skipped" else "ready" end)
           | .version = ($v | select(. != "") // null)
           | .sha = (if $src != "" then $src else .sha end)' <<<"$c")
      fi
    fi
  fi
  echo "$c"
done | jq -s '.' > "$tmp" && mv "$tmp" "$state"

# 1c. A version that already exists as a release tag in the releases repository
#     is taken (a hotfix publishes from its own branch, never from a candidate):
#     a pending or ready candidate carrying it must never be promotable. Marks
#     only, never un-marks; a failed or empty tag listing marks nothing.
#     A beta candidate (2026.11.3-beta.4) whose base version already has a
#     stable tag (v2026.11.3) is taken too: the updater ranks that beta below
#     the stable. Same tag list, no extra API call.
if tags=$(gh api "repos/AgentWorkforce/relay-desktop-releases/releases?per_page=100" --paginate --jq '.[].tag_name') && [ -n "$tags" ]; then
  jq --arg tags "$tags" \
    '($tags | split("\n")) as $taken
     | map(if (.status == "pending" or .status == "ready") and .version != null
              and ((("v" + .version) as $t | $taken | index($t)) != null
                   or ((.version | contains("-beta."))
                       and (("v" + (.version | split("-beta.")[0])) as $b | $taken | index($b)) != null))
           then .status = "stale:version-taken" else . end)' \
    "$state" > "$tmp" && mv "$tmp" "$state"
else
  echo "warning: could not list the releases repository's tags; version-taken check skipped" >&2
fi
[ "${1:-}" = --refresh ] && exit 0

# 2. One release run at a time.
busy=$(gh run list -R "$REPO" --workflow "$WF" --json status --jq '[.[] | select(.status != "completed")] | length')
if [ "$busy" != 0 ]; then echo "a Desktop Release run is in flight; skipping"; exit 0; fi

# 3. Which channel is behind main with buildable changes? Stable is the workflow's own job now.
# shellcheck disable=SC2043  # stable is no longer dispatched here
for channel in beta; do
  last=$(jq -r --arg ch "$channel" '[.[] | select(.channel == $ch)] | last | .sha // empty' "$state")
  [ "$last" = "$head" ] && continue
  if [ -n "$last" ] && git cat-file -e "$last" 2>/dev/null; then
    # main's own copy of the classifier: this clone's working tree is never
    # advanced, so the file on disk would keep the rules of the first clone.
    lanes=$(git show "$head:scripts/ci-changes.py" | python3 - --base "$last" --head "$head")
    if ! grep -q '=true' <<<"$lanes"; then
      echo "$channel: main moved since $last only in docs/changelog paths; no build"
      continue
    fi
  fi
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  gh workflow run "$WF" -R "$REPO" --ref main -f channel="$channel" -f prepare-only=true
  run_id=""
  for _ in $(seq 1 12); do
    sleep 5
    run_id=$(gh run list -R "$REPO" --workflow "$WF" --event workflow_dispatch --limit 5 \
      --json databaseId,headSha,createdAt --jq "[.[] | select(.headSha == \"$head\" and .createdAt >= \"$since\")][0].databaseId // empty")
    [ -n "$run_id" ] && break
  done
  jq --arg ch "$channel" --arg sha "$head" --arg id "${run_id:-}" --arg at "$since" \
    '. + [{channel: $ch, sha: $sha, run_id: (if $id == "" then null else ($id | tonumber) end), dispatched_at: $at, status: "pending", version: null, auto: false}] | .[-40:]' \
    "$state" > "$tmp" && mv "$tmp" "$state"
  echo "dispatched $channel candidate for $head (run ${run_id:-unknown})"
  exit 0
done
stable=$(jq -r '[.[] | select(.channel == "stable" and .status == "ready")] | last | .sha // empty' "$state")
echo "beta candidates current with main ($head); newest ready stable candidate: ${stable:-none} (prepared by the workflow after Desktop CI on main)"

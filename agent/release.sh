#!/bin/bash
# shellcheck disable=SC2016  # jq filters quote $vars on purpose
# Publish a relay-desktop release from a candidate the tick already built, then
# tag the source commit so a later hotfix can start from exactly what shipped.
#
#   release.sh beta|stable [--profile <id>]              newest ready candidate for that channel
#   release.sh beta|stable [--run <id>] [--profile <id>] a specific prepare-only run (e.g. a hotfix candidate)
#   release.sh prepare beta|stable <ref> [--profile <id>]   build a candidate from <ref> now (hotfix branch)
#
# --profile <id> is the opaque id of a site profile (its desktop-<id> GitHub
# Environment); no --profile means the default public profile. It is passed to
# the workflow as -f profile=<id>, and the promote must name the same profile
# the candidate was prepared with. A candidate row carries .profile (null =
# default); without --run only rows of the requested profile are picked, and
# with --run a row's own profile is used when --profile is not given
# (a conflicting one is refused). A row without one accepts --profile only
# while it is ready or pending: a promoting or tagging row is a default
# release killed mid-promote and is resumed as one. prepare does not create a
# row (the tick adopts it, or the promote records it). A command-line profile
# is recorded on the row only with the promoting write, marked
# profile_source "cli", and dropped again if that promote fails, so a wrong
# --profile leaves the row as it was.
# Profile releases get no released-v tag (version strings are shared across
# profiles, so the tag would be ambiguous) and land no promoted changelog.
#
# Publishing = Desktop Promote (verifies and publishes the exact prepared bytes,
# ~10 minutes). Then: annotated tag released-v<version> on the candidate's
# source sha in AgentWorkforce/relay-desktop. The source sha is the "Source
# <sha>" line of the run's Resolve version job; a run's head_sha is used only
# for a workflow_dispatch run that predates that line, never for a
# workflow_run-triggered (automatic) candidate, whose head_sha can be newer
# than the commit that was tested and built. (Not desktop-v*: that pattern
# triggers a fresh Desktop Release build.) A stable promote uploads the
# promoted CHANGELOG.md as an artifact; it is saved to ~/state/promoted/ for
# the lead to land on main.
set -euo pipefail
# gh reads the current token from /run/agent-host/gh-token via /usr/local/bin/gh;
# an inherited GH_TOKEN (possibly stale) must not shadow it.
unset GH_TOKEN GITHUB_TOKEN
# Activate after provisioning the Cloud-owned credential and a prepare smoke run.
# A Cloud refusal never falls back to host orchestration.
if [ "${DESKTOP_RELEASE_ORCHESTRATOR:-legacy}" = cloud ]; then
  exec /opt/agent-host/cloud-release.sh "$@"
fi
REPO=AgentWorkforce/relay-desktop
state=$HOME/state/candidates.json
# Every rewrite of the candidates file takes the tick's lock, so a tick
# refresh running at the same time cannot read the file before a rewrite
# and put the old state back after it.
write_state() {
  (
    flock -w 600 8 || { echo "could not lock $state" >&2; exit 1; }
    tmp=$(mktemp) && jq "$@" "$state" > "$tmp" && mv "$tmp" "$state"
  ) 8> "$HOME/state/.tick.lock"
}
cd "$HOME/build/relay-desktop"
git fetch -q origin main
# Tags separately: a local tag that differs from origin's ("would clobber
# existing tag") must not stop the run, but it is worth surfacing.
git fetch -q --tags origin 2>/dev/null || echo "warning: tag fetch failed (a local tag differs from origin?)" >&2

PREPARE_USAGE="usage: release.sh prepare beta|stable <ref> [--profile <id>]"
USAGE="usage: release.sh beta|stable [--run <id>] [--profile <id>]"
if [ "${1:-}" = prepare ]; then
  channel=${2:?channel}; ref=${3:?ref}
  profile=""
  shift 3
  while [ $# -gt 0 ]; do
    case "$1" in
      --profile) [ -n "${2:-}" ] || { echo "--profile needs an id; $PREPARE_USAGE" >&2; exit 2; }; profile=$2; shift 2 ;;
      *) echo "unknown argument $1; $PREPARE_USAGE" >&2; exit 2 ;;
    esac
  done
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  pflag=(); [ -z "$profile" ] || pflag=(-f "profile=$profile")
  gh workflow run desktop-release.yml -R "$REPO" --ref "$ref" -f channel="$channel" -f prepare-only=true ${pflag[@]+"${pflag[@]}"}
  sleep 10
  id=$(gh run list -R "$REPO" --workflow desktop-release.yml --event workflow_dispatch --limit 5 \
    --json databaseId,createdAt --jq "[.[] | select(.createdAt >= \"$since\")][0].databaseId // empty")
  echo "candidate run ${id:-unknown} for $ref ($channel${profile:+, profile $profile}); when it succeeds: release.sh $channel --run $id${profile:+ --profile $profile}"
  exit 0
fi

channel=${1:?$USAGE}
case "$channel" in beta|stable) ;; *) echo "channel must be beta or stable" >&2; exit 2 ;; esac
run_id=""; profile=""
shift
while [ $# -gt 0 ]; do
  case "$1" in
    --run) [ -n "${2:-}" ] || { echo "--run needs a run id; $USAGE" >&2; exit 2; }; run_id=$2; shift 2 ;;
    --profile) [ -n "${2:-}" ] || { echo "--profile needs an id; $USAGE" >&2; exit 2; }; profile=$2; shift 2 ;;
    *) echo "unknown argument $1; $USAGE" >&2; exit 2 ;;
  esac
done

if [ -z "$run_id" ]; then
  /opt/agent-host/tick.sh --refresh >/dev/null 2>&1 || true   # refresh candidate statuses (never dispatches)
  # A promote this command was killed while watching is resumed first;
  # otherwise the newest ready candidate by dispatch time (the tick adopts
  # automatic runs in batches, so array order is not time order). Only rows
  # of the requested profile count (a row without .profile is the default).
  run_id=$(jq -r --arg ch "$channel" --arg pr "$profile" '[.[] | select(.channel == $ch and ((.profile // "") == $pr) and (.status == "promoting" or .status == "tagging"))] | sort_by(.promote_since) | last | .run_id // empty' "$state")
  [ -z "$run_id" ] || echo "resuming the promote of run $run_id${profile:+ (profile $profile)}"
  [ -n "$run_id" ] || run_id=$(jq -r --arg ch "$channel" --arg pr "$profile" '[.[] | select(.channel == $ch and ((.profile // "") == $pr) and .status == "ready")] | sort_by(.dispatched_at) | last | .run_id // empty' "$state")
  [ -n "$run_id" ] || { echo "no ready $channel candidate${profile:+ for profile $profile} yet (see ~/state/candidates.json); the tick builds one when main changes" >&2; exit 1; }
else
  # --run: the row's own profile is used when none was given; a row with no recorded
  # profile accepts --profile only while ready or pending; a different
  # non-empty one is refused.
  rowprofile=$(jq -r --argjson id "$run_id" '[.[] | select(.run_id == $id)] | last | .profile // empty' "$state")
  rowstatus=$(jq -r --argjson id "$run_id" '[.[] | select(.run_id == $id)] | last | .status // empty' "$state")
  if jq -e --argjson id "$run_id" 'any(.[]; .run_id == $id)' "$state" >/dev/null; then
    if [ -z "$profile" ] && [ -n "$rowprofile" ]; then
      profile=$rowprofile
      echo "run $run_id is a profile $profile candidate; promoting with --profile $profile"
    elif [ -n "$rowprofile" ] && [ "$profile" != "$rowprofile" ]; then
      echo "run $run_id was prepared for profile $rowprofile, not profile $profile; refusing" >&2
      exit 1
    elif [ -z "$rowprofile" ] && [ -n "$profile" ]; then
      # tick.sh adopts workflow_dispatch runs without recording a profile.
      # Only a row that has not started a promote can take one: a promoting
      # or tagging row is a default release to resume, not a profile's.
      case "$rowstatus" in
        ready|pending) ;;
        *) echo "run $run_id has no recorded profile and is $rowstatus (a default release in progress or done); refusing --profile $profile. Rerun without --profile to resume it." >&2; exit 1 ;;
      esac
      # Recorded only with the promoting write below, so nothing is stamped
      # before the run is validated.
      echo "run $run_id has no recorded profile; promoting as profile $profile (the promote workflow verifies the candidate identity)"
    fi
  fi
fi
run=$(gh api "repos/$REPO/actions/runs/$run_id")
[ "$(jq -r .conclusion <<<"$run")" = success ] || { echo "run $run_id did not succeed" >&2; exit 1; }
job=$(gh api "repos/$REPO/actions/runs/$run_id/jobs" --jq '.jobs[] | select(.name=="Resolve version") | .id' | head -1)
log=$(gh api --allow-escape-sequences "repos/$REPO/actions/jobs/$job/logs")
# `|| true`: under set -e a grep that matches nothing would end the script
# before the diagnostics below.
version=$(grep -oE 'Releasing v[0-9][0-9A-Za-z.-]*' <<<"$log" | head -1 | cut -c12- || true)
[ -n "$version" ] || { echo "cannot read the prepared version from run $run_id (an automatic run that skipped has none)" >&2; exit 1; }
sha=$(grep -oE 'Source [0-9a-f]{40}' <<<"$log" | head -1 | cut -c8- || true)
if [ -z "$sha" ]; then
  [ "$(jq -r .event <<<"$run")" = workflow_dispatch ] || { echo "run $run_id has no Source line and was not dispatched by hand: cannot tell what it built" >&2; exit 1; }
  sha=$(jq -r .head_sha <<<"$run")
fi
git cat-file -e "$sha^{commit}" 2>/dev/null || { echo "source $sha of run $run_id is not a commit here" >&2; exit 1; }
# A tag for this version refuses the candidate, unless the tag is this very
# candidate's own, left by a run killed between the promote and the push:
# then the tag step below finishes it.
resuming=$(jq -r --argjson id "$run_id" '[.[] | select(.run_id == $id and (.status == "promoting" or .status == "tagging"))] | length' "$state")
if [ -z "$profile" ] && git rev-parse -q --verify "refs/tags/released-v$version" >/dev/null; then
  if [ "$resuming" = 0 ] || [ "$(git rev-parse "refs/tags/released-v$version^{commit}")" != "$sha" ]; then
    echo "v$version is already released (tag released-v$version exists); this candidate resolved a version that has since shipped. Prepare a new candidate." >&2
    exit 1
  fi
fi
behind=$(git rev-list --count "$sha..origin/main")
echo "promoting v$version ($channel${profile:+, profile $profile}) from run $run_id, source $sha ($behind commits behind main)"

# The intent is recorded before the promote is dispatched, so a release
# killed while it watches the run can be rerun: the same command finds the
# promote run started since and goes on to the tag instead of promoting twice.
promote=""
resume=$(jq -r --argjson id "$run_id" '[.[] | select(.run_id == $id and (.status == "promoting" or .status == "tagging"))] | last | .promote_since // empty' "$state")
if [ -n "$resume" ]; then
  # The candidate's own promote run, recorded once it appeared. Only a run
  # killed before that is found by time, and then only when exactly one
  # promote started since: the API does not expose a run's inputs, so two
  # candidates would be indistinguishable and the lead must sort them out.
  promote=$(jq -r --argjson id "$run_id" '[.[] | select(.run_id == $id)] | last | .promote_run_id // empty' "$state")
  if [ -z "$promote" ]; then
    found=$(gh run list -R "$REPO" --workflow desktop-promote.yml --limit 10 --json databaseId,createdAt \
      --jq "[.[] | select(.createdAt >= \"$resume\") | .databaseId] | join(\" \")")
    case "$found" in
      "") ;;
      *" "*) echo "several promote runs started since $resume ($found); which one is candidate $run_id's cannot be told from the API. Check them, set promote_run_id on the candidate in $state by hand, and rerun." >&2; exit 1 ;;
      *) promote=$found ;;
    esac
  fi
  [ -z "$promote" ] || echo "resuming promote run $promote (dispatched at $resume)"
fi
if [ -z "$promote" ]; then
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  # A candidate release.sh prepare built (a hotfix) is not in the file yet;
  # it is recorded too, so its promote can be resumed like any other.
  if ! jq -e --argjson id "$run_id" 'any(.[]; .run_id == $id)' "$state" >/dev/null; then
    write_state --arg ch "$channel" --argjson id "$run_id" --arg at "$since" \
      '. + [{channel: $ch, sha: null, run_id: $id, dispatched_at: $at, status: "ready", version: null, auto: false, profile: null}] | .[-40:]'
  fi
  # The profile goes on the row with the promote intent. One the row did not
  # already carry came from the command line: profile_source "cli" lets a
  # failed promote drop it again.
  write_state --argjson id "$run_id" --arg at "$since" --arg v "$version" --arg sha "$sha" --arg pr "$profile" \
    '(.[] | select(.run_id == $id)) |= (.status = "promoting" | .promote_since = $at | .version = $v | .sha = $sha
      | if $pr == "" or .profile == $pr then . else .profile = $pr | .profile_source = "cli" end)'
  pflag=(); [ -z "$profile" ] || pflag=(-f "profile=$profile")
  gh workflow run desktop-promote.yml -R "$REPO" --ref main -f candidate-run-id="$run_id" -f version="$version" -f channel="$channel" ${pflag[@]+"${pflag[@]}"}
  for _ in $(seq 1 12); do
    sleep 5
    promote=$(gh run list -R "$REPO" --workflow desktop-promote.yml --limit 5 --json databaseId,createdAt \
      --jq "[.[] | select(.createdAt >= \"$since\")][0].databaseId // empty")
    [ -n "$promote" ] && break
  done
  [ -n "$promote" ] || { echo "promote run did not appear; rerun this command to pick it up once it does (recorded as promoting)" >&2; exit 1; }
fi
write_state --argjson id "$run_id" --argjson p "$promote" '(.[] | select(.run_id == $id)).promote_run_id = $p'
# gh run watch also exits non-zero on an API error or a rotated token while
# the promote keeps running, so the run's own conclusion decides: only a run
# that failed goes back to ready (the next run of this command dispatches a
# fresh promote; the released-v tag check still refuses a version that
# shipped meanwhile). A run still going is left as promoting for a rerun;
# one that succeeded goes on to the tag.
if ! gh run watch "$promote" -R "$REPO" --exit-status --interval 20 >/dev/null; then
  read -r pstatus pconcl < <(gh run view "$promote" -R "$REPO" --json status,conclusion --jq '"\(.status) \(.conclusion)"' 2>/dev/null || echo "unknown unknown")
  case "$pstatus/$pconcl" in
    completed/success) ;;
    completed/*)
      # A profile the command line supplied is dropped with the attempt, so a
      # wrong --profile does not stick to the candidate.
      write_state --argjson id "$run_id" '(.[] | select(.run_id == $id)) |= (.status = "ready" | del(.promote_since) | del(.promote_run_id)
        | if .profile_source == "cli" then del(.profile) | del(.profile_source) else . end)'
      echo "promote run $promote $pconcl: https://github.com/$REPO/actions/runs/$promote (candidate back to ready; rerun to promote again)" >&2
      exit 1 ;;
    *)
      echo "lost track of promote run $promote ($pstatus/$pconcl): https://github.com/$REPO/actions/runs/$promote; the candidate stays promoting, rerun this command to watch it again" >&2
      exit 1 ;;
  esac
fi

if [ -n "$profile" ]; then
  # Version strings are shared across profiles: a released-v tag would be ambiguous.
  write_state --argjson id "$run_id" '(.[] | select(.run_id == $id)).status = "published"'
  echo "profile $profile v$version published from run $run_id, source $sha (no released-v tag for profile releases)"
  exit 0
fi
# Tagging is recorded before it starts and is idempotent, so a run killed
# or failing here is finished by a rerun instead of wedging the channel.
tag=released-v$version
write_state --argjson id "$run_id" '(.[] | select(.run_id == $id)).status = "tagging"'
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  [ "$(git rev-parse "refs/tags/$tag^{commit}")" = "$sha" ] || { echo "local tag $tag points elsewhere than $sha; fix it by hand" >&2; exit 1; }
else
  git tag -a "$tag" "$sha" -m "relay-desktop v$version ($channel), published from Desktop Release run $run_id"
fi
git push -q origin "refs/tags/$tag" || { echo "pushing $tag failed; rerun this command to push it again (the release is published)" >&2; exit 1; }
write_state --argjson id "$run_id" '(.[] | select(.run_id == $id)).status = "published"'
if [ "$channel" = stable ]; then
  mkdir -p "$HOME/state/promoted/v$version"
  gh run download "$promote" -R "$REPO" -n "promoted-changelog-v$version" -D "$HOME/state/promoted/v$version" \
    && echo "promoted changelog: $HOME/state/promoted/v$version/CHANGELOG.md (land it on main)"
fi
echo "published v$version ($channel); tagged $tag at $sha"
echo "release: https://github.com/AgentWorkforce/relay-desktop-releases/releases/tag/v$version"

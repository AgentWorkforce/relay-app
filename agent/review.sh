#!/bin/bash
# One review round for a pushed relay-desktop change. The reviewers run only
# as an Agent Relay Cloud flow run (flows/cloud-review.flow.ts), never on this
# host; local CI (flows/ci.flow.ts) runs here, beside the Cloud run.
#
#   review.sh <ref> [--intent "<what was asked>"] [--depth small|standard|deep] [--final] [--trust-ref-flows]
#                   [--lead codex|claude] [--lenses claude,codex,grok]
#   review.sh <ref> --ci-only [--final] [--trust-ref-flows]
#
# <ref> is a pushed branch or sha on AgentWorkforce/relay-desktop, prepared in
# its own clone (~/reviews/<slug>) with flow state in its git-ignored
# .relayflow/. A round:
#  1. refuses at once, exit 3, when this host has no FLOWS_CLOUD_TOKEN
#     (secrets.env): there is no local fallback;
#  2. sizes the change here with flows/plan-review.flow.ts (Jev over
#     TYPESAFE_API_KEY, or --depth): the depth picks adversarial-review and/or
#     context-review and the Claude model (sonnet for small, opus for standard
#     and deep);
#  3. writes the change (patch, file list, meta, the uploaded files'
#     manifest, and for context-review the intent) to cloud-review/input/ and
#     submits flows/cloud-review.flow.ts with `flows run --cloud --sync-code`,
#     30 budget minutes for small, 60 for standard and 90 for deep: the flow
#     picks per depth the lenses, cut-offs and second opinions that fit,
#     every reviewer and verifier running its full time one after another
#     (cloud-review.flow.ts DEPTH_PLANS); the submission is in
#     .relayflow/cloud/run.json.
#     A submission is never repeated: one that failed transiently (a transport
#     timeout, an unknown admission, HTTP 408, 429 or 5xx) may have started a
#     run anyway, so it ends the round "round refused: cloud_transient
#     (<message>); run again", exit 3, and any other refusal ends it
#     "round refused: <code> <message>", exit 3;
#  4. runs local CI in the background while it polls the run every two
#     minutes until it ends, whatever the end; a run that does not end within
#     its budget plus ten minutes, or cannot be read for lack of auth, ends the
#     round refused, exit 3;
#  5. pulls the results back with `flows sync`, never into the clone: the
#     run's change set is listed first (`--dry-run`), refused if any path in
#     it lies outside cloud-review/, then applied in a scratch directory and
#     checked there again; only then are the files under cloud-review/ copied
#     into the clone (nothing else ever is, and a refused sync changes nothing
#     in it). Each flow's report goes to .relayflow/<flow>/ and the summary to
#     .relayflow/cloud/; context-review's state is saved for the next round's
#     plan; .relayflow/sized-review/report.md gets the depth, the reviewers'
#     CLI and lenses, the lanes, the Cloud run and each flow's verdict, CI's
#     included. It is printed when the round ends: exit 0 green, 1 red.
#     A run that ended failed (or cancelled, or over budget) is reported the
#     same way when it wrote its summary, its failed flows red; it is refused,
#     exit 3, when no summary came back or the one that did is not in the
#     shape the flow writes.
# Every lens and verifier runs on the lead CLI (the report's "Reviewers on"):
# --lead, else REVIEW_LEAD in secrets.env, else claude. --lenses adds
# second-opinion reviewers on other CLIs to adversarial-review (one on the
# lead's CLI is ignored); a depth runs as many as fit its cap (none for small,
# one for standard, two for deep) and reports the rest as unavailable. Claude runs on the plan's model, Sonnet or Opus only;
# codex and grok get no model override.
# --final adds the GitHub gate (flows/ci.flow.ts github:true). It dispatches
# Desktop CI with the merge base, so lanes follow the changed paths: a change
# that selects no windows lane never starts the Windows runners.
# --ci-only runs the CI flow alone, for a fix push or a rebase after a review
# round, per the 2026-11-05 review policy: one review per change, then CI
# decides. It prepares the checkout as above, then runs flows/ci.flow.ts with
# the input a review round gives it (so --ci-only --final is the local
# lanes plus the GitHub gate, no reviewers) and prints .relayflow/ci/report.md.
# It needs no Cloud token; --depth, --lead and --lenses are errors with it,
# and --intent is not used.
# Fix pushes and rebases after the review round use --ci-only.
set -uo pipefail
# A ref whose base predates main's flows/ (a hotfix from an old release)
# carries reviewer tooling that may lack flows/sized-review.flow.ts or
# guards main's lanes rely on. The tooling is main's, not the change's, so
# such a round runs origin/main's flows/ against the ref's tree: the
# overlay is hidden from the flows' own diff and untracked checks
# (assume-unchanged on tracked paths, a marked block in .git/info/exclude
# for new ones), and clear_main_flows_overlay undoes it before the next
# round's reset.
overlay_begin="# >>> review.sh: main's flows/ overlay (regenerated each round)"
overlay_end="# <<< review.sh: main's flows/ overlay"
overlay_exclude() { git rev-parse --git-path info/exclude; }
# Drop the marked block from .git/info/exclude, keeping everything else.
drop_overlay_block() {
  local exclude
  exclude=$(overlay_exclude) || return 1
  [ -f "$exclude" ] || return 0
  awk -v b="$overlay_begin" -v e="$overlay_end" '$0 == b { skip = 1; next } $0 == e { skip = 0; next } !skip' "$exclude" > "$exclude.tmp" &&
    mv -f "$exclude.tmp" "$exclude"
}
# Clear every assume-unchanged bit (lowercase tags in `ls-files -v`) and the
# exclude block, so the reset --hard and clean -x that follow restore the
# ref's own flows/ and remove the overlay's new files.
clear_main_flows_overlay() {
  git ls-files -v -z | sed -z -n 's/^[a-z] //p' | xargs -0 -r git update-index --no-assume-unchanged -- &&
    drop_overlay_block
}
# Check out FETCH_HEAD with nothing left of the last round. Clearing the
# overlay turns it into visible local edits and untracked files, which would
# abort the checkout ("would be overwritten"), so the tree goes back to the
# old ref's own files first; the reset/clean after the checkout is kept.
fresh_checkout() {
  clear_main_flows_overlay && git reset -q --hard && git clean -qfdx -e flows/node_modules -e .relayflow &&
    git checkout -q --detach FETCH_HEAD && git reset -q --hard FETCH_HEAD && git clean -qfdx -e flows/node_modules -e .relayflow
}
overlay_main_flows() {
  local main exclude
  main=$(git rev-parse --verify -q 'origin/main^{commit}') || { echo "cannot resolve origin/main" >&2; return 1; }
  git checkout -q origin/main -- flows && git reset -q -- flows || return 1
  # Files main's flows/ dropped would otherwise linger beside its own.
  git diff -z --name-only --no-renames --diff-filter=D HEAD origin/main -- flows | xargs -0 -r rm -f -- || return 1
  git diff -z --name-only -- flows | xargs -0 -r git update-index --assume-unchanged -- || return 1
  exclude=$(overlay_exclude) || return 1
  mkdir -p "$(dirname "$exclude")" && drop_overlay_block || return 1
  {
    echo "$overlay_begin"
    # A base whose .gitignore predates main's flows entries leaves the
    # flows' own state and packages untracked, which the flows refuse; the
    # block (checked after it was dropped, so against the ref's own rules)
    # ignores those for this round only.
    for ignored in .relayflow/ .relayflowd/ flows/node_modules/; do
      git check-ignore -q "${ignored}x" || echo "/$ignored"
    done
    # Anchored, with gitignore's special characters escaped: each line
    # matches exactly one overlaid file.
    git ls-files -z --others --exclude-standard -- flows | sed -z -e 's/[][*?\\!# ]/\\&/g' -e 's|^|/|' | tr '\0' '\n'
    echo "$overlay_end"
  } >> "$exclude" || return 1
  echo "note: this ref's flows/ predates main's; the round runs main's flows/ ($main) against the ref's tree"
}
# A content hash of every file under flows/ but its installed packages, read
# from the files themselves rather than through git: the overlay hides
# main's flows/ from git (assume-unchanged, excluded), so the flows' own
# TREE_FINGERPRINT (git diff HEAD, ls-files --others, status) is blind to an
# edit a reviewer makes to the overlaid files during the round.
flows_fingerprint() {
  find flows -path flows/node_modules -prune -o -type f -print0 | sort -z | xargs -0 -r sha256sum | sha256sum
}
# The files the round uploads, as cloud-review.flow.ts checks them in the
# sandbox: `git ls-files -s -z` (mode, blob id, stage, path) of a scratch
# index that holds exactly the files `--sync-code` packs, with their content
# as it is on disk. upload_paths repeats the uploader's choice
# (@relayflows/sdk cloud-sync.ts listTreeFiles and packWorkingTree): `git
# ls-files --cached --others --exclude-standard`, less any path with a .git
# or node_modules segment, a path missing from disk (a file main's flows/
# dropped, which the overlay deletes but leaves in the index), anything but a
# regular file or a symlink that stays inside the tree; cloud-review/ is the
# round's own input and the flow skips it. A round that overlaid main's
# flows/ so lists main's changed files, and not the ones its exclude block
# hides: the uploader does not send those either. Blob ids are content
# hashes: they survive the commit Cloud makes of the upload, whose commit and
# tree ids never match HEAD's. The clone's own index is untouched.
upload_paths() {
  local path target
  while IFS= read -r -d '' path; do
    case "/$path/" in */.git/* | */node_modules/* | *//* | */../* | /cloud-review/*) continue ;; esac
    if [ -L "$path" ]; then
      target=$(readlink -- "$path") || continue
      case "$target" in /*) continue ;; esac
      target=$(realpath -m -s --relative-to=. -- "$(dirname -- "$path")/$target") || continue
      case "$target" in .. | ../*) continue ;; esac
    elif [ ! -f "$path" ]; then
      continue
    fi
    printf '%s\0' "$path"
  done
}
upload_manifest() {
  local scratch rc
  scratch=$(mktemp -d) || return 1
  # The listing reads the clone's index; only the scratch one is written.
  # Modes come from the files, never the clone's config: core.filemode and
  # core.symlinks are forced on, and since git takes the owner's execute bit
  # where the uploader packs 0755 for any execute bit, a regular file with
  # any execute bit is then marked +x.
  git ls-files -z --cached --others --exclude-standard | sort -zu | upload_paths > "$scratch/paths" &&
    GIT_INDEX_FILE=$scratch/index git -c core.filemode=true -c core.symlinks=true update-index --add -z --stdin < "$scratch/paths" &&
    sed -z 's|^|./|' "$scratch/paths" | xargs -0 -r sh -c 'find -P "$@" -maxdepth 0 -type f -perm /111 -print0' find | sed -z 's|^\./||' |
    GIT_INDEX_FILE=$scratch/index git -c core.filemode=true update-index -z --chmod=+x --stdin &&
    GIT_INDEX_FILE=$scratch/index git ls-files -s -z
  rc=$?
  rm -rf "$scratch"
  return "$rc"
}
# What a round saves for the next one, and the next round trusts: each flow's
# report dir, <flow>.state.json (the reviewed snapshot and open findings, keyed
# by branch and merge base), the refs/relayflow/<flow>/reviewed snapshots, and
# the .relayflowd run journals. A round whose result is refused (flows/ was
# tampered with mid-round) must leave none of it: an identical rerun would find
# the saved state, review nothing and report the tampered round green. Kept:
# flows.lockstamp (the npm install stamp), logs/ and the .lock file.
discard_round_state() {
  local root=${RELAYFLOW_STATE_DIR:?RELAYFLOW_STATE_DIR is not set} flow
  for flow in adversarial-review context-review ci sized-review plan cloud; do
    rm -rf -- "${root:?}/$flow" "${root:?}/$flow.state.json"
    git update-ref -d "refs/relayflow/$flow/reviewed" 2>/dev/null || true
  done
  rm -rf -- .relayflowd
  echo "discarded this round's saved state (${root}: adversarial-review, context-review, ci, sized-review, plan, cloud, *.state.json; refs/relayflow/*/reviewed; .relayflowd); the next round starts from nothing" >&2
}
# Secrets come from the host's secrets.env (REVIEW_SECRETS_FILE in the
# tests), read one name at a time, never sourced into this script. Each value
# reaches only the process that needs it, through its environment: the Cloud
# token the flows CLI that talks to Cloud, TYPESAFE_API_KEY the local plan.
# The host also loads secrets.env into this script's own environment, so every
# local flow or helper runs with FLOWS_CLOUD_TOKEN and FLOWS_CLOUD_URL removed
# (and the review keys), and only cloud_flows sets them again.
secret() { sed -n -E "s/^(export +)?$1=//p" "${REVIEW_SECRETS_FILE:-/run/agent-host/secrets.env}" 2>/dev/null | tail -1 | sed -E "s/^(['\"])(.*)\1\$/\2/"; }

parse_args() {
  ref=${1:?usage: review.sh <ref> [--intent text] [--depth small|standard|deep] [--final] [--trust-ref-flows] [--lead codex|claude] [--lenses claude,codex,grok] [--ci-only]}
  shift
  # The ref comes from a peer: it must be a ref name, never something git
  # would read as an option (`--upload-pack=...` would run a command here).
  case "$ref" in -*) echo "refusing ref $ref: a ref must not start with -" >&2; exit 2 ;; esac
  git check-ref-format --allow-onelevel "$ref" || { echo "refusing ref $ref: not a valid ref name" >&2; exit 2; }
  intent="" depth="" final=false trust_ref_flows=false ci_only=false flows_before="" lead="" lenses=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --intent) intent=${2:?--intent needs the text of what was asked}; shift 2 ;;
      --depth) depth=${2:?--depth needs small, standard or deep}; shift 2 ;;
      --lead) lead=${2:?--lead needs codex or claude}; shift 2 ;;
      --lenses) lenses=${2:?--lenses needs a comma-separated list of claude, codex and grok}; shift 2 ;;
      --final) final=true; shift ;;
      --trust-ref-flows) trust_ref_flows=true; shift ;;
      --ci-only) ci_only=true; shift ;;
      *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
  done
  case "$depth" in ""|small|standard|deep) ;; *) echo "bad depth $depth" >&2; exit 2 ;; esac
  [ "$ci_only" = false ] || [ -z "$depth" ] || { echo "--depth does not apply with --ci-only: no reviewers run" >&2; exit 2; }
  [ "$ci_only" = false ] || [ -z "$lead$lenses" ] || { echo "--lead and --lenses do not apply with --ci-only: no reviewers run" >&2; exit 2; }
  local lens
  for lens in ${lenses//,/ }; do
    case "$lens" in claude|codex|grok) ;; *) echo "bad lens $lens: --lenses takes claude, codex and grok" >&2; exit 2 ;; esac
  done
  [ -n "$lead" ] || [ "$ci_only" = true ] || lead=$(secret REVIEW_LEAD)
  [ "$ci_only" = true ] || lead=${lead:-claude}
  case "$lead" in ""|claude|codex) ;; *) echo "bad lead $lead: the lead is codex or claude (--lead, or REVIEW_LEAD in secrets.env)" >&2; exit 2 ;; esac
}

# Fail closed: a review round runs only as a Cloud flow run.
cloud_credentials() {
  cloud_token=$(secret FLOWS_CLOUD_TOKEN)
  cloud_url=$(secret FLOWS_CLOUD_URL)
  if [ -z "$cloud_token" ]; then
    echo "round refused: cloud_auth_missing (no FLOWS_CLOUD_TOKEN on this host; reviews run only as Agent Relay Cloud flow runs, never locally)" >&2
    exit 3
  fi
}

# The flows CLI for Cloud calls. The token (and FLOWS_CLOUD_URL when set) is
# exported in a subshell that execs the CLI, so it is in no argv, no log, and
# no other process's environment; the review keys are dropped.
cloud_flows() {
  (
    unset ANTHROPIC_API_KEY TYPESAFE_API_KEY
    export FLOWS_CLOUD_TOKEN="$cloud_token"
    if [ -n "${cloud_url:-}" ]; then export FLOWS_CLOUD_URL="$cloud_url"; fi
    exec flows/node_modules/.bin/flows "$@"
  )
}

# A refusal ends the round with exit 3. Local CI, if it was started, is
# waited for after the refusal is printed: it holds the round's lock and the
# tree until it ends.
cloud_refuse() {
  echo "round refused: $*" >&2
  if [ -n "${ci_pid:-}" ]; then
    echo "waiting for the local CI run to finish (see $RELAYFLOW_STATE_DIR/logs/ci.log)" >&2
    wait "$ci_pid"
    ci_pid=""
  fi
  rm -rf cloud-review
  return 3
}

# Submit the Cloud run, once. A failed submission is never repeated: one that
# failed transiently (transient_error, admission_unknown, HTTP 408/429/5xx)
# may have started a run, and the CLI's run listing carries nothing that
# would tell this round's run from another, so the round is refused and run
# again by hand. Anything else is refused with its own code.
cloud_submit() {
  local input=$1 budget=$2 out
  out=$(cloud_flows run --cloud --json --sync-code --no-connect --budget-minutes "$budget" flows/cloud-review.flow.ts --input "$input" 2>> "$RELAYFLOW_STATE_DIR/logs/cloud.log")
  printf '%s\n' "$out" > "$RELAYFLOW_STATE_DIR/cloud/run.json"
  printf 'run: %s\n' "$out" >> "$RELAYFLOW_STATE_DIR/logs/cloud.log"
  if jq -e '.ok == true and (.runId | type) == "string"' <<< "$out" > /dev/null 2>&1; then
    cloud_run_id=$(jq -r '.runId' <<< "$out")
    cloud_api_url=$(jq -r '.apiUrl // ""' <<< "$out")
    return 0
  fi
  if jq -e '.ok == false and (.code == "transient_error" or .code == "admission_unknown" or (.code == "http_error" and ((.message // "") | test("HTTP (408|429|5[0-9][0-9])"))))' <<< "$out" > /dev/null 2>&1; then
    cloud_refuse "cloud_transient ($(jq -r '(.message // .code) + (if .runId then " run \(.runId)" else "" end)' <<< "$out")); run again"
  elif jq -e '.ok == false' <<< "$out" > /dev/null 2>&1; then
    cloud_refuse "$(jq -r '"\(.code // "cloud_submit_failed") \(.message // "")" + (if .runId then " run \(.runId)" else "" end)' <<< "$out")"
  else
    cloud_refuse "cloud_submit_unreadable the flows CLI printed no result (see $RELAYFLOW_STATE_DIR/logs/cloud.log)"
  fi
}

# Poll the run every CLOUD_POLL_SECONDS (120) until it ends, within its budget
# plus ten minutes for queueing (CLOUD_POLL_DEADLINE_SECONDS replaces that
# limit in the tests). Any end returns, with cloud_end_status and
# cloud_end_reason set (a failed run's budget_exceeded reads as the status
# budget_exceeded): the round then syncs whatever the run left. Transient read
# failures are polled through; an auth or any other read refusal, and no end
# in time, refuse.
cloud_poll() {
  local budget=$1 limit deadline out status reason code message
  limit=${CLOUD_POLL_DEADLINE_SECONDS:-$(((budget + 10) * 60))}
  deadline=$((SECONDS + limit))
  cloud_end_status="" cloud_end_reason=""
  while :; do
    sleep "${CLOUD_POLL_SECONDS:-120}"
    out=$(cloud_flows status --cloud --json "$cloud_run_id" 2>> "$RELAYFLOW_STATE_DIR/logs/cloud.log")
    if jq -e '.ok == true and (.run | type) == "object"' <<< "$out" > /dev/null 2>&1; then
      printf '%s\n' "$out" > "$RELAYFLOW_STATE_DIR/cloud/status.json"
      status=$(jq -r '.run.status // ""' <<< "$out")
      reason=$(jq -r '.run.completion_reason // ""' <<< "$out")
      case "$status" in
        ""|queued|pending|launching|running) ;;
        *)
          [ "$status:$reason" != failed:budget_exceeded ] || status=budget_exceeded
          cloud_end_status=$status cloud_end_reason=$reason
          return 0
          ;;
      esac
    else
      code=$(jq -r '.code // "cloud_status_unreadable"' <<< "$out" 2>/dev/null) || code=cloud_status_unreadable
      message=$(jq -r '.message // ""' <<< "$out" 2>/dev/null) || message="the flows CLI printed no result"
      case "$code" in
        cloud_unreachable|cloud_transport_failed) ;;
        cloud_http_error) [[ "$message" =~ HTTP\ (408|429|5[0-9][0-9]) ]] || { cloud_refuse "$code $message run $cloud_run_id"; return; } ;;
        *) cloud_refuse "$code ${message:-the flows CLI printed no result} run $cloud_run_id"; return ;;
      esac
      echo "note: reading run $cloud_run_id failed transiently ($code); polling on" >&2
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      cloud_refuse "cloud_poll_timeout no end within $((limit / 60)) minute(s) run $cloud_run_id"
      return
    fi
  done
}

# Whether every path a patch names lies under cloud-review/: the `diff --git`
# headers, the ---/+++ lines and any rename or copy lines. A quoted header
# (an unusual path), a path with a .. segment, or a header in no known shape
# counts as outside. Prints the first offending lines.
patch_outside_paths() {
  awk '
    function bad(path) { return path !~ /^cloud-review\/./ || path ~ /(^|\/)\.\.(\/|$)/ }
    /^diff --git / {
      rest = substr($0, 12)
      if (rest !~ /^a\/[^ "].* b\/[^ "].*$/) { print; next }
      n = index(rest, " b/"); a = substr(rest, 3, n - 3); b = substr(rest, n + 3)
      if (bad(a) || bad(b)) print
      next
    }
    /^(---|\+\+\+) / {
      path = substr($0, 5)
      if (path == "/dev/null") next
      if (path !~ /^[ab]\//) { print; next }
      if (bad(substr(path, 3))) print
      next
    }
    /^(rename|copy) (from|to) / { path = $0; sub(/^(rename|copy) (from|to) /, "", path); if (bad(path)) print }
  ' | head -3
}

# Bring the run's results back into cloud-review/, and nothing else. The
# run's change set is listed first (`flows sync --dry-run`, which applies
# nothing) and refused unless every path in it lies under cloud-review/; it
# is then applied in a scratch repository holding only the input the run was
# submitted with (its changes are a patch against that), and the scratch tree
# is checked again, since the patch is fetched anew for the apply. Only then
# are the regular files under its cloud-review/ (but input/) copied into the
# clone. On failure it returns 1 with cloud_sync_problem set and the clone
# untouched; cloud_sync_unsafe is true when the run reached outside
# cloud-review/.
cloud_sync() {
  local out outside tmp
  cloud_sync_problem="" cloud_sync_unsafe=false
  rm -rf cloud-review
  out=$(cloud_flows sync --json --dry-run "$cloud_run_id" 2>> "$RELAYFLOW_STATE_DIR/logs/cloud.log")
  jq 'del(.patch)' <<< "$out" > "$RELAYFLOW_STATE_DIR/cloud/sync-dry-run.json" 2>/dev/null || printf '%s\n' "$out" > "$RELAYFLOW_STATE_DIR/cloud/sync-dry-run.json"
  if ! jq -e '.ok == true' <<< "$out" > /dev/null 2>&1; then
    cloud_sync_problem="$(jq -r '"\(.code // "unreadable") \(.message // "")"' <<< "$out" 2>/dev/null || echo "the flows CLI printed no result") run $cloud_run_id"
    return 1
  fi
  if ! jq -e '.hasChanges == true' <<< "$out" > /dev/null; then
    cloud_sync_problem="no changes came back from run $cloud_run_id"
    return 1
  fi
  if jq -e '.multiPath == true or (.patch | type) != "string"' <<< "$out" > /dev/null; then
    cloud_sync_problem="the run's changes are not one patch over the uploaded tree run $cloud_run_id" cloud_sync_unsafe=true
    return 1
  fi
  outside=$({ jq -r '(.files // [])[] | select(type != "string" or (startswith("cloud-review/") | not) or test("(^|/)\\.\\.(/|$)"))' <<< "$out"
    jq -r '.patch' <<< "$out" | patch_outside_paths; } | head -3)
  if [ -n "$outside" ]; then
    cloud_sync_problem="the run changed files outside cloud-review/ ($(tr '\n' ' ' <<< "$outside")) run $cloud_run_id" cloud_sync_unsafe=true
    return 1
  fi
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/cloud-sync.XXXXXX") || { cloud_sync_problem="no scratch directory run $cloud_run_id"; return 1; }
  cloud_sync_scratch "$tmp"
  local rc=$?
  rm -rf -- "$tmp"
  return "$rc"
}

# What cloud-review.flow.ts writes to summary.json: a result per flow with
# numeric blocking and total, a string report and an optional string failed,
# and the lists of extra lenses and unavailable ones.
summary_shape='type == "object"
  and (.depth | type) == "string" and (.lead | type) == "string"
  and (.lenses | type) == "array" and all(.lenses[]; type == "string")
  and (.lensesUnavailable | type) == "array" and all(.lensesUnavailable[]; type == "string")
  and (.flows | type) == "object"
  and all(.flows[]; type == "object"
    and (.blocking | type) == "number" and (.total | type) == "number"
    and (.report | type) == "string"
    and ((has("failed") | not) or (.failed | type) == "string"))'

# The apply half of cloud_sync, in the scratch directory $1.
cloud_sync_scratch() {
  local tmp=$1 out outside
  if ! { git init -q "$tmp" && mkdir -p "$tmp/cloud-review" && cp -R "$RELAYFLOW_STATE_DIR/cloud/input" "$tmp/cloud-review/input"; }; then
    cloud_sync_problem="cannot prepare the scratch directory run $cloud_run_id"
    return 1
  fi
  out=$(cloud_flows sync --json --dir "$tmp" "$cloud_run_id" 2>> "$RELAYFLOW_STATE_DIR/logs/cloud.log")
  printf '%s\n' "$out" > "$RELAYFLOW_STATE_DIR/cloud/sync.json"
  if ! jq -e '.ok == true' <<< "$out" > /dev/null 2>&1; then
    cloud_sync_problem="$(jq -r '"\(.code // "unreadable") \(.message // "")"' <<< "$out" 2>/dev/null || echo "the flows CLI printed no result") run $cloud_run_id"
    return 1
  fi
  outside=$({ jq -r '(.files // [])[] | select(type != "string" or (startswith("cloud-review/") | not))' <<< "$out"
    (cd "$tmp" && find . -mindepth 1 -path ./.git -prune -o -print | sed 's|^\./||' | grep -v -e '^cloud-review$' -e '^cloud-review/'); } | head -3)
  if [ -n "$outside" ]; then
    cloud_sync_problem="the run changed files outside cloud-review/ ($(tr '\n' ' ' <<< "$outside")) run $cloud_run_id" cloud_sync_unsafe=true
    return 1
  fi
  outside=$(cd "$tmp" && find cloud-review ! -type d ! -type f | head -3)
  if [ -n "$outside" ]; then
    cloud_sync_problem="the run left something other than files in cloud-review/ ($(tr '\n' ' ' <<< "$outside")) run $cloud_run_id" cloud_sync_unsafe=true
    return 1
  fi
  if [ ! -s "$tmp/cloud-review/summary.json" ]; then
    cloud_sync_problem="no cloud-review/summary.json came back run $cloud_run_id"
    return 1
  fi
  # The report is read from the summary unchecked, so a truncated or
  # wrong-shaped one would read as green: it must be what the flow writes.
  local why
  if ! why=$(jq -e "$summary_shape" < "$tmp/cloud-review/summary.json" 2>&1 > /dev/null); then
    why=${why%%$'\n'*}
    cloud_sync_problem="unreadable cloud-review/summary.json (${why:-wrong shape}) run $cloud_run_id"
    return 1
  fi
  rm -rf -- "$tmp/cloud-review/input"
  cp -R "$tmp/cloud-review" cloud-review || { rm -rf cloud-review; cloud_sync_problem="cannot copy the results into the clone run $cloud_run_id"; return 1; }
}

# The round's report, in the shape the sized review gave it.
cloud_report() {
  local ci_status=$1 state=$RELAYFLOW_STATE_DIR plan=$RELAYFLOW_STATE_DIR/plan/plan.json summary=$RELAYFLOW_STATE_DIR/cloud/summary.json
  local depth flow_lines ci_line red head gate lead_line
  depth=$(jq -r '.depth' "$plan")
  flow_lines=$(jq -r --arg state "$state" --argjson planned "$(jq -c '.flows' "$plan")" '
    . as $s | $planned[] | . as $f | ($s.flows[$f] // {failed: "no result came back from the run"}) as $r
    | (($r.failed // "") != "" or ($r.blocking // 0) > 0) as $red
    | "- \(if $red then "FAIL" else "PASS" end) \($f): \($state)/\($f)/report.md (\($r.blocking // 0) blocker/major of \($r.total // 0) finding(s)\(if ($r.failed // "") != "" then "; \($r.failed)" else "" end))"' "$summary")
  if [ -f "$state/ci/report.md" ]; then ci_line="$state/ci/report.md"; else ci_line="(no report; see $state/logs/ci.log)"; fi
  if [ "$ci_status" = 0 ]; then ci_line="- PASS ci: $ci_line"; else ci_line="- FAIL ci: $ci_line"; fi
  red=$(printf '%s\n%s\n' "$flow_lines" "$ci_line" | grep -c '^- FAIL ')
  if [ "$red" = 0 ]; then head="# Review: $depth, green"; else head="# Review: $depth, $red flow(s) red"; fi
  if [ "$final" = true ]; then gate="GitHub gate: requested; the CI flow's report says whether any lane needed it"; else gate="GitHub gate: not run; before landing, run again with --ci-only --final"; fi
  lead_line=$(jq -r --arg lead "$lead" --arg model "${cloud_model:-}" --argjson extra "$cloud_lenses" '
    def on($cli): if $cli == "claude" and $model != "" then "\($cli) on \($model)" else $cli end;
    "Reviewers on: \(on($lead)); extra lenses: \(($extra | keys | map(on(.)) | join(", ")) as $e | if $e == "" then "none" else $e end)"
    + (if (.lensesUnavailable // []) | length > 0 then "; unavailable: \(.lensesUnavailable | join("; "))" else "" end)' "$summary")
  {
    echo "$head"
    echo
    jq -r '"Depth: \(.depth) (\(.origin)); the whole change is reviewed in every Cloud round"' "$plan"
    echo "$lead_line"
    jq -r '.change | "Change: \(.fileCount) file(s), \(.lines) line(s), \(.commitCount) commit(s); merge base \(.mergeBase[0:12])"
      + (if (.redacted // 0) > 0 then "; \(.redacted) secret-shaped line(s) redacted before sizing" else "" end)
      + (if (.redactedSubjects // 0) > 0 then "; \(.redactedSubjects) subject(s) redacted" else "" end)' "$plan"
    jq -r '"Lanes selected: \(.lanes | to_entries | map(select(.value) | .key) | join(", ") | if . == "" then "none (always-on checks only)" else . end); Windows runners: \(if .windows then "yes" else "no" end)"' "$plan"
    echo "$gate"
    echo "Cloud run: $cloud_run_id $cloud_api_url$(if [ "${cloud_end_status:-completed}" != completed ]; then echo " (ended $cloud_end_status${cloud_end_reason:+: $cloud_end_reason}; reported from its summary)"; fi)"
    echo
    echo "$flow_lines"
    echo "$ci_line"
  } > "$state/sized-review/report.md"
  if [ "$red" = 0 ]; then
    # A trusted lead receipt for Cloud release readiness. CI evidence is
    # independently fetched by the release flow/worker; this records only
    # the reported review and local CI for the exact reviewed tree. A fix
    # that changes the tree needs the lead to reconcile the findings first.
    jq -n --arg tree "$(git rev-parse 'HEAD^{tree}')" --arg run "$cloud_run_id" \
      '{sourceTree: $tree, reviewRunId: $run, reviewReported: true, openBlockers: 0,
        blockerFixesReviewed: true, localCiPassed: true}' > "$state/cloud/release-readiness.json"
  fi
  [ "$red" = 0 ]
}

# One review round from a prepared checkout (the main body below, or the
# tests): plan here, review in Cloud, CI here, report.
cloud_round() {
  local state=$RELAYFLOW_STATE_DIR plan plan_input input flows_json pdepth pmodel budget ci_input ci_status flow
  cloud_run_id="" cloud_api_url="" ci_pid="" cloud_model="" cloud_end_status="" cloud_end_reason=""
  rm -rf -- "${state:?}/plan" "${state:?}/cloud" "${state:?}/sized-review" "${state:?}/adversarial-review" "${state:?}/context-review" cloud-review
  mkdir -p "$state/logs" "$state/cloud" "$state/sized-review"
  plan_input=$(jq -nc --arg intent "$intent" --arg depth "$depth" \
    '{base: "origin/main"} + (if $intent == "" then {} else {intent: $intent} end) + (if $depth == "" then {} else {depth: $depth} end)')
  (
    unset ANTHROPIC_API_KEY FLOWS_CLOUD_TOKEN FLOWS_CLOUD_URL
    TYPESAFE_API_KEY=$(secret TYPESAFE_API_KEY)
    export TYPESAFE_API_KEY
    exec flows/node_modules/.bin/flows run --local-agent flows/plan-review.flow.ts --input "$plan_input"
  ) > "$state/logs/plan-review.log" 2>&1
  plan=$state/plan/plan.json
  if ! jq -e 'type == "object"' "$plan" > /dev/null 2>&1; then
    echo "no plan: see $state/logs/plan-review.log" >&2
    tail -20 "$state/logs/plan-review.log" >&2
    return 1
  fi
  if ! jq -e '.depth' "$plan" > /dev/null; then
    # A round that cannot start (bad base, untracked files, nothing to review, no intent) says why.
    jq -r '"# Review: \(.title)\n\n\(.detail)"' "$plan" > "$state/sized-review/report.md"
    return 1
  fi
  pdepth=$(jq -r '.depth' "$plan")
  flows_json=$(jq -c '.flows' "$plan")
  pmodel=$(jq -r '.model' "$plan")
  case "$pmodel" in
    claude-sonnet-*|claude-opus-*) ;;
    *) echo "refusing the plan's model $pmodel: Claude reviewers run on Sonnet or Opus only" >&2; return 1 ;;
  esac
  # Extra lenses on the lead's own CLI are dropped; only claude takes a model.
  cloud_lenses=$(jq -nc --arg l "$lenses" --arg lead "$lead" '[$l | split(",")[] | select(. != "" and . != $lead)] | unique | map({(.): true}) | add // {}')
  if [ "$lead" = claude ] || jq -e '.claude' <<< "$cloud_lenses" > /dev/null; then cloud_model=$pmodel; fi
  input=$(jq -nc --arg depth "$pdepth" --argjson flows "$flows_json" --arg lead "$lead" --argjson lenses "$cloud_lenses" --arg model "$cloud_model" \
    '{depth: $depth, flows: $flows, lead: $lead, dir: "cloud-review"} + (if $lenses == {} then {} else {lenses: $lenses} end) + (if $model == "" then {} else {model: $model} end)')
  # The round's cap by depth. cloud-review.flow.ts DEPTH_PLANS fits its
  # reviewers and verifiers, one after another, inside it: 24 minutes at
  # worst for small, 54 for standard, 80 for deep, second opinions included.
  case "$pdepth" in
    small) budget=30 ;;
    standard) budget=60 ;;
    *) budget=90 ;;
  esac

  # The run's input, uploaded with the tree by --sync-code; a copy stays in
  # the state dir for the sync.
  mkdir -p cloud-review/input
  if ! { git diff --binary --no-ext-diff "$base" HEAD > cloud-review/input/change.patch &&
    git diff --name-only --no-renames "$base" HEAD > cloud-review/input/files.txt &&
    upload_manifest > cloud-review/input/manifest &&
    jq -n --arg ref "$ref" --arg head "$(git rev-parse HEAD)" --arg mb "$base" --arg depth "$pdepth" --arg model "$pmodel" \
      '{ref: $ref, head: $head, mergeBase: $mb, depth: $depth, model: $model}' > cloud-review/input/meta.json; }; then
    rm -rf cloud-review
    echo "cannot write the run's input" >&2
    return 1
  fi
  if jq -e 'index("context-review")' <<< "$flows_json" > /dev/null; then
    {
      printf '# Intent\n\n%s\n\n' "${intent:-(none given: the commits below are the intent)}"
      printf '## Branch\n\n%s\n\n## Commits\n\n' "$ref"
      git log --no-merges --format='### %s%n%n%b' "$base..HEAD"
    } > cloud-review/input/intent.md
  fi
  cp -R cloud-review/input "$state/cloud/input"

  cloud_submit "$input" "$budget" || return 3
  echo "Cloud run $cloud_run_id submitted ($pdepth, $budget budget minutes): $cloud_api_url"
  # CI refuses untracked files: cloud-review/ goes before it starts.
  rm -rf cloud-review
  ci_input=$(jq -nc --argjson final "$final" '{base: "origin/main", github: $final}')
  env -u ANTHROPIC_API_KEY -u TYPESAFE_API_KEY -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL flows/node_modules/.bin/flows run --local-agent flows/ci.flow.ts --input "$ci_input" > "$state/logs/ci.log" 2>&1 &
  ci_pid=$!

  cloud_poll "$budget" || return 3
  wait "$ci_pid"
  ci_status=$?
  ci_pid=""
  # Whatever the end, the run's summary is the round's verdict when it came
  # back: a failed run that wrote one is reported red from it. A run that
  # left none is refused, and so is any sync that reached outside
  # cloud-review/ or failed after a completed run.
  if ! cloud_sync; then
    if [ "$cloud_end_status" = completed ] || [ "$cloud_sync_unsafe" = true ]; then
      cloud_refuse "cloud_sync_failed $cloud_sync_problem"
      return
    fi
    echo "note: no summary came back from run $cloud_run_id: $cloud_sync_problem" >&2
    if [ -n "$cloud_end_reason" ] && [ "$cloud_end_reason" != "$cloud_end_status" ]; then
      cloud_refuse "cloud_$cloud_end_status $cloud_end_reason run $cloud_run_id"
    else
      cloud_refuse "cloud_$cloud_end_status run $cloud_run_id"
    fi
    return
  fi
  [ "$cloud_end_status" = completed ] ||
    echo "note: run $cloud_run_id ended $cloud_end_status${cloud_end_reason:+ ($cloud_end_reason)} after writing its summary; the round is reported from it" >&2
  for flow in $(jq -r '.flows | keys[]' cloud-review/summary.json); do
    case "$flow" in adversarial-review|context-review) ;; *) continue ;; esac
    rm -rf -- "${state:?}/$flow"
    [ ! -d cloud-review/"$flow" ] || mv cloud-review/"$flow" "$state/$flow"
  done
  # context-review's open findings carry to the next round's plan (its `open`
  # read), as a local context-review saves them: only for a review that ran
  # to its end, keyed by the plan's snapshot and merge base.
  if [ -f "$state/context-review/report.json" ] && jq -e '.flows["context-review"] | type == "object" and ((.failed // "") == "")' cloud-review/summary.json > /dev/null; then
    env -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL node flows/lib/review-state.mjs save context-review "$(jq -r '.change.snapshot' "$plan")" "$(jq -r '.change.mergeBase' "$plan")" ||
      echo "note: context-review's state was not saved; the next round's plan will not count its open findings" >&2
  fi
  mv cloud-review/summary.json "$state/cloud/summary.json"
  rm -rf cloud-review
  cloud_report "$ci_status"
}

# Sourced (the tests): stop after the definitions above.
if (return 0 2>/dev/null); then return 0; fi

parse_args "$@"
[ "$ci_only" = true ] || cloud_credentials

slug=$(printf '%s' "$ref" | tr -c 'A-Za-z0-9._-' '-' | cut -c1-60)
dir=$HOME/reviews/$slug
# Flow state lives inside the clone (git-ignored .relayflow/): the flows
# runtime refuses agent cwds outside the run root.
export RELAYFLOW_STATE_DIR=$dir/.relayflow

if [ ! -d "$dir/.git" ]; then
  git clone -q --reference "$HOME/build/relay-desktop" https://github.com/AgentWorkforce/relay-desktop.git "$dir" || exit 1
fi
cd "$dir" || exit 1
mkdir -p "$RELAYFLOW_STATE_DIR/logs"
# One round per ref at a time: a second request for the same ref while a
# round runs (a repeated message, or a rerun right after a push) would
# otherwise reset the tree under the first round's lanes and reviewers.
exec 9> "$RELAYFLOW_STATE_DIR/.lock"
flock -n 9 || { echo "a round for $ref is already running in $dir; wait for it or stop it first" >&2; exit 2; }
# Fetch the ref on its own: with two refspecs FETCH_HEAD marks only the first (main).
if ! { git fetch -q origin -- main && git fetch -q origin -- "$ref"; }; then echo "cannot fetch $ref from origin" >&2; exit 1; fi
# The last round's overlay (if any) comes off first: an assume-unchanged
# bit would survive the reset and keep main's flows/ in this tree.
if ! fresh_checkout; then
  echo "cannot check out $ref in $dir; not reviewing the previous round's tree" >&2; exit 1
fi
# Flow code runs with this host's keys, so only flow code from main's
# history runs here: a ref whose flows/ differs from its merge base with
# main is refused until the lead has read that diff and reruns with
# --trust-ref-flows. A ref may not track review state or installed packages
# either (.relayflow, .relayflowd, flows/node_modules), which the clone
# keeps across rounds. The reviewers run in Cloud, not here, and the Cloud
# token reaches only the flows CLI that submits, polls and syncs; the local
# plan (with TYPESAFE_API_KEY) and the local lanes still run here, and the
# lanes build and test the ref's Rust and scripts as this user, with no key.
tracked=$(git ls-tree -r --name-only HEAD -- .relayflow .relayflowd flows/node_modules)
[ -z "$tracked" ] || { echo "refusing $ref: it tracks review state or installed packages ($(echo "$tracked" | head -3 | tr '\n' ' '))" >&2; exit 2; }
base=$(git merge-base origin/main HEAD) || { echo "cannot find the merge base of $ref and main" >&2; exit 1; }
if ! git diff --quiet "$base" HEAD -- flows; then
  if [ "$trust_ref_flows" = true ]; then
    echo "note: this change edits flows/; its flow code runs this round because --trust-ref-flows was given"
  else
    echo "refusing $ref: it edits flows/, the code a round runs with this host's keys. Read 'git -C $dir diff $base HEAD -- flows' and rerun with --trust-ref-flows only if that is safe." >&2
    exit 2
  fi
elif ! git diff --quiet "$base" origin/main -- flows; then
  # The ref does not edit flows/, but its base's flows/ is older than main's.
  overlay_main_flows || { echo "cannot overlay main's flows/ onto $ref" >&2; exit 1; }
  # The flows' tamper check cannot see the overlaid files (see
  # flows_fingerprint), so this round checks flows/ itself: hashed here and
  # again after the round, and a round that changed it is not reported.
  # Without the overlay the flows' own check covers flows/.
  flows_before=$(flows_fingerprint) || { echo "cannot fingerprint the overlaid flows/" >&2; exit 1; }
fi
# Hashes main's lockfile when the overlay applied, so main's packages install.
lock=$(sha256sum flows/package-lock.json | cut -c1-16)
stamp=$RELAYFLOW_STATE_DIR/flows.lockstamp
if [ "$(cat "$stamp" 2>/dev/null)" != "$lock" ]; then
  # No lifecycle scripts: nothing from the ref runs at install time.
  npm --prefix flows ci --no-audit --no-fund --ignore-scripts >/dev/null && echo "$lock" > "$stamp"
fi

# A previous local round's detached review fleets outlive its driver and
# hold no flock; stop them as sized-review.flow.ts does, so nothing reviews
# beside this round.
for f in sized-review adversarial-review context-review ci; do
  env -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL node flows/lib/fleet.mjs stop "$RELAYFLOW_STATE_DIR/$f/fleet" || true
done
again='--ci-only --final'
if [ "$ci_only" = true ]; then
  # CI needs no keys (ci.flow.ts reads none): this script passes none and
  # strips the ones it knows, the Cloud token and URL among them.
  flow=ci
  input=$(jq -nc --argjson final "$final" '{base: "origin/main", github: $final}')
  env -u ANTHROPIC_API_KEY -u TYPESAFE_API_KEY -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL flows/node_modules/.bin/flows run --local-agent flows/ci.flow.ts --input "$input" > "$RELAYFLOW_STATE_DIR/logs/ci-only.log" 2>&1
  status=$?
  log=$RELAYFLOW_STATE_DIR/logs/ci-only.log
else
  # The review round: planned here, reviewed in Cloud, CI here (cloud_round).
  # Its report keeps the sized review's place, .relayflow/sized-review/report.md.
  flow=sized-review
  cloud_round 2>&1 | tee "$RELAYFLOW_STATE_DIR/logs/sized-review.log"
  status=${PIPESTATUS[0]}
  log=$RELAYFLOW_STATE_DIR/logs/sized-review.log
fi
if [ -n "${flows_before:-}" ] && [ "$(flows_fingerprint)" != "$flows_before" ]; then
  echo "refusing this round's result: flows/ changed while the round ran (the overlaid reviewer code was edited); see $log" >&2
  discard_round_state
  exit 1
fi
# A refused round has no report: the refusal above is its outcome.
[ "$status" != 3 ] || exit 3
report=$RELAYFLOW_STATE_DIR/$flow/report.md
if [ -f "$report" ]; then cat "$report"; else echo "no report: see $log" >&2; tail -20 "$log" >&2; fi
echo
echo "head: $(git rev-parse HEAD)  clone: $dir"
[ "$final" = true ] || [ "$status" != 0 ] || echo "round green: run again with $again for the GitHub gate before landing"
exit "$status"

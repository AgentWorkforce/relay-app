#!/bin/bash
# Hourly (agent-host-disk-cleanup.timer), as the agent user: free the root
# volume of finished review checkouts (~/reviews/<slug>) and work trees
# (~/work/<slug>). The volume filled twice on 2026-11-04 from cargo targets.
#
#   disk-cleanup.sh [--dry-run]      --dry-run decides and logs, deletes nothing
#
# Policy:
#  1. A whole checkout idle for DISK_CLEANUP_MAX_AGE_HOURS (24) is deleted
#     only when it is safe to lose: `git status --porcelain` is empty (ignoring
#     .relayflow/, .relayflowd/ and its cargo targets, which the repo's
#     .gitignore does not cover below the root) and every local branch and HEAD is
#     contained in an origin/* ref (nothing unpushed). An unsafe one is kept,
#     logged as `uncommitted changes` and/or `unpushed commits`, until it has
#     been idle DISK_CLEANUP_ABANDON_HOURS (168, a week): then it is deleted
#     anyway, logged as `abandoned (idle Nh, <what was unsafe>)`. A younger
#     one is `fresh (idle Nh < 24h)`. Idle is the newest mtime of the
#     directory, its .git, the FETCH_HEAD, HEAD, logs/HEAD and index of its
#     git dir (resolved, so a linked worktree's are read from the main repo),
#     and .relayflow/ or .relayflowd/ (with their entries).
#  2. Never one that is still owned:
#     - an open ledger entry's branch (~/state/ledger.json, or LEDGER.md)
#       matches its HEAD branch or its slug (review.sh's slug of the branch);
#     - a live round: its .relayflow/.lock is held (review.sh holds it for the
#       whole round), or a process (a `flows run`, cargo, a session) has its
#       cwd inside it. A detached flows daemon (comm relayflowd, parent pid
#       1: a round's daemon that outlived the round) is not work: the round
#       lock and the ledger are the liveness signals. When a checkout, stub or
#       trash goes, the detached daemons working in it are stopped after the
#       rename and before the rm (SIGTERM, up to 5 s, then SIGKILL), noted
#       `stopped N orphaned relayflowd`; no other process is ever signalled;
#     - another clone borrows its objects (alternates) or git dir (worktree).
#     The round lock is held by this script while it deletes, and the
#     checkout is renamed away first. The lock alone does not keep out a
#     review.sh that cds in between the cwd scan and the rename (it then
#     recreates the lock file by path and takes that), so after the rename
#     the cwd scan is repeated against the trash path: a process found there
#     entered before the rename, the checkout is moved back and logged
#     `now in use`. Nothing can enter by the old path after the rename.
#  3. If the root volume is still above DISK_CLEANUP_CAP percent (70), the
#     cargo targets (target/ dirs with a CACHEDIR.TAG, up to three levels
#     deep) of the remaining unowned checkouts are pruned oldest first until
#     usage is under the cap. Targets go at any age, since a target is always
#     rebuildable. The cap never makes a whole checkout deletable: if it
#     cannot be reached the line says `still over cap: N kept (reasons)`.
#  4. One line per run in ~/state/disk-cleanup.log.
# First of all, detached relayflowd daemons whose cwd under ~/reviews or
# ~/work was deleted (they hold the deleted tree's space) are stopped, noted
# `orphan daemons: N stopped`.
# Only direct children of ~/reviews and ~/work that are git checkouts and not
# symlinks are ever candidates: ~/build, ~/src, ~/state and ~/hotfix (release
# work) are never touched. One that holds only .relayflow/ and/or .relayflowd/
# and no .git is the stub an interrupted round leaves, which would make
# review.sh's clone fail for that ref forever: it is removed as
# `leftover stub` unless its round lock is held or a process works in it.
# An unreadable ledger deletes nothing; no ledger file at all (a fresh host:
# ledger.py writes one only on the first entry) counts as an empty ledger,
# noted on the line.
#
# Test-only: DISK_CLEANUP_FAKE_FS=<dir>:<capacity_bytes> replaces the root
# volume with a simulated one: used = the bytes under <dir> (du), total =
# <capacity_bytes>, measured again after every deletion like df would be, so
# tests never depend on the host's real root usage and see the cap's effect.
# Never set it in production.
#
# Test-only: DISK_CLEANUP_BEFORE_REMOVE=<command> runs (bash -c, with
# DISK_CLEANUP_CHECKOUT set to the checkout) inside remove_checkout, after the
# round lock is taken and right before the ownership re-check, so tests can
# make a checkout owned or in use in that window. Empty by default; never set
# it in production.
#
# Test-only: DISK_CLEANUP_AFTER_RENAME=<command> runs (bash -c, with
# DISK_CLEANUP_CHECKOUT set) in phase 3 right after a checkout's round lock is
# released and before its renamed targets are measured and removed.
#
# Test-only: DISK_CLEANUP_AFTER_RENAME_SCAN=<command> runs (bash -c, with
# DISK_CLEANUP_CHECKOUT and DISK_CLEANUP_TRASH set) in remove_checkout right
# after the checkout is renamed to its trash path and before the cwd scan of
# that path, so tests can put a process inside it in that window.
#
# Test-only: DISK_CLEANUP_DAEMON_COMM=<comm> (default relayflowd) is the comm
# of the detached flows daemon, so tests can stand one in with a renamed
# sleep. Never set it in production.
set -euo pipefail

dry_run=false
case "${1:-}" in
  "") ;;
  --dry-run) dry_run=true ;;
  *) echo "usage: disk-cleanup.sh [--dry-run]" >&2; exit 2 ;;
esac
[ "$(id -u)" != 0 ] || { echo "disk-cleanup.sh runs as the agent user, not root" >&2; exit 2; }
cap=${DISK_CLEANUP_CAP:-70}
max_age_h=${DISK_CLEANUP_MAX_AGE_HOURS:-24}
abandon_h=${DISK_CLEANUP_ABANDON_HOURS:-168}
case "$cap$max_age_h$abandon_h" in *[!0-9]*) echo "DISK_CLEANUP_CAP, DISK_CLEANUP_MAX_AGE_HOURS and DISK_CLEANUP_ABANDON_HOURS must be integers" >&2; exit 2 ;; esac
daemon_comm=${DISK_CLEANUP_DAEMON_COMM:-relayflowd}
fake_fs=${DISK_CLEANUP_FAKE_FS:-}
if [ -n "$fake_fs" ]; then
  fake_dir=${fake_fs%:*} fake_cap=${fake_fs##*:}
  case "$fake_cap" in "" | *[!0-9]* | 0) echo "DISK_CLEANUP_FAKE_FS is <dir>:<capacity_bytes>" >&2; exit 2 ;; esac
  [ -d "$fake_dir" ] || { echo "DISK_CLEANUP_FAKE_FS: $fake_dir is not a directory" >&2; exit 2; }
fi

state=$HOME/state
log=$state/disk-cleanup.log
mkdir -p "$state"
exec 8>> "$state/.disk-cleanup.lock"
flock -n 8 || { echo "disk cleanup already running"; exit 0; }

now=$(date +%s)
freed=0
deleted=() pruned=() skipped=() failed=()
declare -A pruned_bytes=() kept_why=()

human() { numfmt --to=iec-i --suffix=B "$1" 2>/dev/null || echo "${1}B"; }
size_of() { local s; s=$(du -sxB1 -- "$1" 2>/dev/null | cut -f1) || true; echo "${s:-0}"; }
# The root volume's used and total bytes (or the test-only simulated
# volume's); a dry run subtracts what it would free instead of measuring again.
usage() {
  if [ -n "$fake_fs" ]; then used=$(size_of "$fake_dir") total=$fake_cap; return; fi
  read -r used total < <(df -P -B1 / | awk 'NR == 2 { print $3, $3 + $4 }')
}
usage
pct() { echo $(( (used * 100 + total - 1) / total )); }
measure() {
  if [ "$dry_run" = true ]; then used=$(( used - $1 )); else usage; fi
}
before=$(pct)

finish() {
  local mode=run
  [ "$dry_run" = true ] && mode=dry-run
  local IFS=,
  printf '%s %s freed=%s(%s) usage=%s%%->%s%% cap=%s%% deleted=[%s] targets=[%s] skipped=[%s] failed=[%s]%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$mode" "$freed" "$(human "$freed")" "$before" "$(pct)" "$cap" \
    "${deleted[*]}" "${pruned[*]}" "${skipped[*]}" "${failed[*]}" "${1:+ $1}" | tee -a "$log"
}

slug() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '-' | cut -c1-60; }

# Open ledger branches, one per line. ledger.json is the ledger's state;
# LEDGER.md (its rendered copy) is read from the right, since a task may
# contain a "|": the branch is the third field from the end.
ledger_branches() {
  if [ -f "$state/ledger.json" ]; then
    jq -r 'if type == "array" then .[] | select(.status != "done") | .branch // empty else error("not a list") end' "$state/ledger.json"
  elif [ -f "$state/LEDGER.md" ]; then
    awk -F'|' '/^\|/ && NR > 4 { s = $3; b = $(NF - 3); gsub(/^ +| +$/, "", s); gsub(/^ +| +$/, "", b); if (s != "done" && b != "") print b }' "$state/LEDGER.md"
  else
    return 1
  fi
}
declare -A open_branch=() open_slug=()
ledger_note=""
# (Re)read the ledger into open_branch/open_slug; fails when unreadable. No
# ledger file at all is an empty ledger, as ledger.py's load() reads it.
load_ledger() {
  local branches="" b
  if [ ! -e "$state/ledger.json" ] && [ ! -e "$state/LEDGER.md" ]; then
    ledger_note="warning: no ledger in $state yet, read as empty"
  else
    branches=$(ledger_branches) || return 1
    ledger_note=""
  fi
  open_branch=() open_slug=()
  while IFS= read -r b; do
    [ -n "$b" ] || continue
    open_branch[$b]=1
    open_slug[$(slug "$b")]=1
  done <<<"$branches"
}
if ! load_ledger; then
  finish "aborted: no readable ledger in $state; nothing deleted"
  exit 1
fi

# Paths other clones depend on: their alternates and their gitfile targets.
borrowed=()
load_borrowed() {
  local g a
  borrowed=()
  for g in "$HOME"/{reviews,work,hotfix,build,src}/*/.git; do
    if [ -f "$g/objects/info/alternates" ]; then
      while IFS= read -r a; do borrowed+=("$a"$'\t'"${g%/.git}"); done < "$g/objects/info/alternates"
    elif [ -f "$g" ]; then
      borrowed+=("$(sed -n 's/^gitdir: //p' "$g")"$'\t'"${g%/.git}")
    fi
  done
}
load_borrowed

activity() {  # newest mtime (epoch seconds) of the checkout's activity markers
  local d=$1 newest=0 t p g
  # A linked worktree's .git is a file naming its git dir elsewhere.
  g=$(git -C "$d" rev-parse --absolute-git-dir 2>/dev/null) || g=$d/.git
  for p in "$d" "$d/.git" "$g/FETCH_HEAD" "$g/HEAD" "$g/logs/HEAD" "$g/index"; do
    if [ -e "$p" ]; then t=$(stat -c %Y -- "$p"); if [ "$t" -gt "$newest" ]; then newest=$t; fi; fi
  done
  # .relayflowd/ (daemon-written state) is left out: a finished round's
  # detached daemons touch it every ~30 s, which would keep it fresh forever.
  p=$d/.relayflow
  if [ -d "$p" ]; then
    t=$(find "$p" -maxdepth 3 -printf '%T@\n' 2>/dev/null | sort -n | tail -1)
    t=${t%.*}
    if [ -n "$t" ] && [ "$t" -gt "$newest" ]; then newest=$t; fi
  fi
  echo "$newest"
}

# Why a checkout is owned, or nothing when it is not.
owner() {
  local d=$1 name=${1##*/} head entry p cwd
  head=$(git --git-dir="$d/.git" rev-parse --abbrev-ref HEAD 2>/dev/null || true)
  if [ -n "$head" ] && [ "$head" != HEAD ] && [ -n "${open_branch[$head]:-}" ]; then echo "open ledger entry $head"; return; fi
  if [ -n "${open_slug[$name]:-}" ]; then echo "open ledger entry for slug $name"; return; fi
  for entry in "${borrowed[@]}"; do
    case "${entry%%$'\t'*}/" in "$d"/*) [ "${entry#*$'\t'}" = "$d" ] || { echo "used by ${entry#*$'\t'}"; return; } ;; esac
  done
  cwd_in "$d"
}
# A process whose cwd is inside the directory, or nothing. A detached flows
# daemon does not count (see the policy).
cwd_in() {
  local d=$1 p cwd
  for p in /proc/[0-9]*; do
    cwd=$(readlink "$p/cwd" 2>/dev/null) || continue
    case "$cwd/" in
      "$d"/*)
        if detached_daemon "${p#/proc/}"; then continue; fi
        echo "pid ${p#/proc/} ($(cat "$p/comm" 2>/dev/null)) works in it"; return ;;
    esac
  done
}

# Is pid $1 a live detached flows daemon (comm $daemon_comm, parent pid 1)? If so
# its start time is left in $daemon_start: with the pid it names the process,
# so a reused pid is never signalled. Builtins only (it runs per process).
daemon_start=""
detached_daemon() {
  local s f
  daemon_start=""
  { read -r s < "/proc/$1/stat"; } 2>/dev/null || return 1
  [ "${s#*(}" != "$s" ] || return 1
  f=${s#*(}; [ "${f%)*}" = "$daemon_comm" ] || return 1
  read -r -a f <<<"${s##*) }"
  # A zombie (stopped, not yet reaped) is gone already.
  [ "${f[1]:-}" = 1 ] && [ "${f[0]}" != Z ] || return 1
  daemon_start=${f[19]:-}
}
# Is $1 (pid:starttime) still that detached daemon?
same_daemon() { detached_daemon "${1%:*}" && [ "$daemon_start" = "${1#*:}" ]; }
# The detached flows daemons (pid:starttime) whose cwd is inside $1.
daemons_in() {
  local p id cwd
  for p in /proc/[0-9]*; do
    detached_daemon "${p#/proc/}" || continue
    id=$daemon_start
    cwd=$(readlink "$p/cwd" 2>/dev/null) || continue
    case "$cwd/" in "$1"/*) echo "${p#/proc/}:$id" ;; esac
  done
}
# SIGTERM the given daemons (pid:starttime) that are still those processes,
# wait up to 5 s, SIGKILL stragglers; prints how many were signalled.
stop_daemons() {
  local e n=0 i alive
  for e in "$@"; do
    if same_daemon "$e" && kill -TERM "${e%:*}" 2>/dev/null; then n=$(( n + 1 )); fi
  done
  for (( i = 0; i < 50; i++ )); do
    alive=false
    for e in "$@"; do if same_daemon "$e"; then alive=true; fi; done
    [ "$alive" = true ] || break
    sleep 0.1
  done
  for e in "$@"; do
    if same_daemon "$e"; then kill -KILL "${e%:*}" 2>/dev/null || true; fi
  done
  echo "$n"
}
# Stop the detached daemons working in a renamed-away path; prints the note
# for its log entry ("; stopped N orphaned relayflowd") or nothing.
stop_daemons_in() {
  local ids n
  mapfile -t ids < <(daemons_in "$1")
  [ "${#ids[@]}" -gt 0 ] || return 0
  n=$(stop_daemons "${ids[@]}")
  [ "$n" -eq 0 ] || echo "; stopped $n orphaned $daemon_comm"
}

# Take the round lock (fd 7) for the length of an action; fails when a
# round holds it. A checkout without one has no round to wait for.
claim() {
  exec 7<&-
  [ -f "$1/.relayflow/.lock" ] || return 0
  exec 7< "$1/.relayflow/.lock"
  flock -n -x 7
}

targets_of() {
  find "$1" -mindepth 1 -maxdepth 3 \( -name .git -o -name node_modules \) -prune -o -type d -name target -print -prune 2>/dev/null |
    while IFS= read -r t; do [ -f "$t/CACHEDIR.TAG" ] && echo "$t"; done || true
}

# What would be lost by deleting a checkout: "uncommitted changes", "stashed
# work" and/or "unpushed commits" (joined by "/"), or nothing. A git failure counts as
# unsafe. --no-optional-locks: do not touch .git/index (it is idle evidence).
# The cargo targets are left out of the status: the repo ignores only the
# top-level /target/, so a built gtk/target or desktop-core/target is
# untracked, and a target is always rebuildable.
unsafe_of() {
  local d=$1 out r="" t
  local spec=(. ':(exclude).relayflow' ':(exclude).relayflowd')
  while IFS= read -r t; do
    [ -n "$t" ] && spec+=(":(exclude,literal)${t#"$d"/}")
  done < <(targets_of "$d")
  if ! out=$(git -C "$d" --no-optional-locks status --porcelain -- "${spec[@]}" 2>/dev/null) || [ -n "$out" ]; then
    r="uncommitted changes"
  fi
  if ! out=$(git -C "$d" --no-optional-locks stash list 2>/dev/null) || [ -n "$out" ]; then
    r="${r:+$r/}stashed work"
  fi
  # A sha ref is fetched into FETCH_HEAD only (review.sh): what it holds is
  # pushed too.
  local not=(--remotes=origin)
  if git -C "$d" rev-parse -q --verify FETCH_HEAD >/dev/null 2>&1; then not+=(FETCH_HEAD); fi
  if ! out=$(git -C "$d" rev-list --count --branches HEAD --not "${not[@]}" 2>/dev/null) || [ "$out" != 0 ]; then
    r="${r:+$r/}unpushed commits"
  fi
  echo "$r"
}

# May a whole checkout go? Returns 0 with the log note in $suffix, or 1 with
# the reason it stays in $reason.
judge() {
  local d=$1 idle=$2 unsafe
  if [ "$idle" -lt "$max_age_h" ]; then reason="fresh (idle ${idle}h < ${max_age_h}h)"; return 1; fi
  unsafe=$(unsafe_of "$d")
  if [ -z "$unsafe" ]; then suffix="idle ${idle}h"; return 0; fi
  if [ "$idle" -ge "$abandon_h" ]; then suffix="abandoned (idle ${idle}h, $unsafe)"; return 0; fi
  reason=$unsafe
  return 1
}

# Delete a whole checkout: claimed, re-checked, renamed out of the way, removed.
remove_checkout() {
  local d=$1 label=$2 why size trash stopped=""
  if ! claim "$d"; then skipped+=("$label:round lock held"); return 1; fi
  # The scan's checks are stale by now: redo them against fresh state, with
  # the round lock held, right before the rename.
  if [ -n "${DISK_CLEANUP_BEFORE_REMOVE:-}" ]; then
    DISK_CLEANUP_CHECKOUT=$d bash -c "$DISK_CLEANUP_BEFORE_REMOVE" >/dev/null 2>&1 || true
  fi
  if ! load_ledger; then skipped+=("$label:ledger unreadable"); exec 7<&-; return 1; fi
  load_borrowed
  why=$(owner "$d")
  if [ -n "$why" ]; then
    case "$why" in
      "open ledger"*) why="became owned ($why)" ;;
      "used by"*) why="borrowed ($why)" ;;
      *) why="now in use ($why)" ;;
    esac
    skipped+=("$label:$why"); exec 7<&-; return 1
  fi
  if [ "$dry_run" = true ]; then
    # A dry run's targets were only counted, not removed: count them once.
    size=$(( $(size_of "$d") - ${pruned_bytes[$d]:-0} ))
  else
    trash=${d%/*}/.disk-cleanup-trash.${d##*/}
    # Measured on the trash path, outside the check-to-rename window.
    if ! mv -T -- "$d" "$trash"; then failed+=("$label"); exec 7<&-; return 1; fi
    if [ -n "${DISK_CLEANUP_AFTER_RENAME_SCAN:-}" ]; then
      DISK_CLEANUP_CHECKOUT=$d DISK_CLEANUP_TRASH=$trash bash -c "$DISK_CLEANUP_AFTER_RENAME_SCAN" >/dev/null 2>&1 || true
    fi
    # A process that entered between the scan and the rename is in the trash
    # now: put the checkout back. If the old path was recreated meanwhile
    # (review.sh's mkdir of .relayflow/logs), the trash stays where it is and
    # the leftover sweep takes it once nothing works in it.
    why=$(cwd_in "$trash")
    if [ -n "$why" ]; then
      if mv -T -- "$trash" "$d"; then skipped+=("$label:now in use ($why)")
      else failed+=("$label(now in use ($why), left at ${trash#"$HOME"/})"); fi
      exec 7<&-; return 1
    fi
    stopped=$(stop_daemons_in "$trash")
    size=$(size_of "$trash")
    if ! rm -rf -- "$trash"; then failed+=("$label(rm failed$stopped)"); exec 7<&-; return 1; fi
  fi
  exec 7<&-
  freed=$(( freed + size )); measure "$size"
  deleted+=("$label($(human "$size") $3$stopped)")
}

# A directory holding only .relayflow/ and/or .relayflowd/ (at least one), no
# .git: a detached daemon recreates .relayflowd/ by path after a delete.
is_stub() {
  local e
  [ -d "$1/.relayflow" ] || [ -d "$1/.relayflowd" ] || return 1
  for e in "$1"/* "$1"/.[!.]* "$1"/..?*; do
    [ -e "$e" ] || [ -L "$e" ] || continue
    case "${e##*/}" in .relayflow | .relayflowd) ;; *) return 1 ;; esac
  done
}
# Remove a stub, under its round lock, unless a process works in it. It has
# no work to lose, so the ledger and the idle time do not keep it.
remove_stub() {
  local d=$1 label=$2 why size trash stopped=""
  if ! claim "$d"; then skipped+=("$label:round lock held"); return 1; fi
  why=$(cwd_in "$d")
  if [ -n "$why" ]; then skipped+=("$label:$why"); exec 7<&-; return 1; fi
  size=$(size_of "$d")
  if [ "$dry_run" = false ]; then
    trash=${d%/*}/.disk-cleanup-trash.${d##*/}
    if ! mv -T -- "$d" "$trash"; then failed+=("$label"); exec 7<&-; return 1; fi
    stopped=$(stop_daemons_in "$trash")
    if ! rm -rf -- "$trash"; then failed+=("$label(rm failed$stopped)"); exec 7<&-; return 1; fi
  fi
  exec 7<&-
  freed=$(( freed + size )); measure "$size"
  deleted+=("$label($(human "$size") leftover stub$stopped)")
}

# Detached daemons of finished rounds whose checkout is already gone: their
# deleted cwd keeps the tree's space allocated. Only ones that worked under
# ~/reviews or ~/work.
orphan_note=""
orphans=()
for p in /proc/[0-9]*; do
  detached_daemon "${p#/proc/}" || continue
  id=$daemon_start
  cwd=$(readlink "$p/cwd" 2>/dev/null) || continue
  case "$cwd" in "$HOME"/reviews/*" (deleted)" | "$HOME"/work/*" (deleted)") orphans+=("${p#/proc/}:$id") ;; esac
done
if [ "${#orphans[@]}" -gt 0 ]; then
  if [ "$dry_run" = true ]; then orphan_note="orphan daemons: ${#orphans[@]} to stop"
  else orphan_note="orphan daemons: $(stop_daemons "${orphans[@]}") stopped"; fi
fi

# Leftovers of an interrupted run.
for t in "$HOME"/reviews/.disk-cleanup-trash.* "$HOME"/work/.disk-cleanup-trash.*; do
  [ -e "$t" ] || continue
  why=$(cwd_in "$t")
  if [ -n "$why" ]; then skipped+=("${t#"$HOME"/}:$why"); continue; fi
  stopped=""
  if [ "$dry_run" = false ]; then stopped=$(stop_daemons_in "$t"); fi
  size=$(size_of "$t")
  if [ "$dry_run" = true ] || rm -rf -- "$t"; then freed=$(( freed + size )); measure "$size"; deleted+=("${t#"$HOME"/}($(human "$size") leftover$stopped)"); fi
done

# Target trash of an interrupted run, inside the checkouts.
while IFS= read -r t; do
  [ -n "$t" ] || continue
  stopped=""
  if [ "$dry_run" = false ]; then stopped=$(stop_daemons_in "$t"); fi
  size=$(size_of "$t")
  if [ "$dry_run" = true ] || rm -rf -- "$t"; then freed=$(( freed + size )); measure "$size"; deleted+=("${t#"$HOME"/}($(human "$size") leftover$stopped)"); fi
done < <(find "$HOME"/reviews/* "$HOME"/work/* -maxdepth 3 \( -name .git -o -name node_modules \) -prune -o -type d -name 'target.disk-cleanup-trash-*' -print -prune 2>/dev/null || true)

# Candidates, with their activity, for phase 3 (oldest first).
remaining=()
for root in "$HOME/reviews" "$HOME/work"; do
  if [ ! -d "$root" ] || [ -L "$root" ]; then continue; fi
  for d in "$root"/*; do
    label=${d#"$HOME"/}
    [ -d "$d" ] || continue
    if [ -L "$d" ]; then skipped+=("$label:symlink"); continue; fi
    if [ ! -e "$d/.git" ]; then
      if is_stub "$d"; then remove_stub "$d" "$label" || true
      else skipped+=("$label:not a checkout"); fi
      continue
    fi
    why=$(owner "$d")
    if [ -n "$why" ]; then skipped+=("$label:$why"); continue; fi
    last=$(activity "$d")
    idle_h=$(( (now - last) / 3600 ))
    if judge "$d" "$idle_h"; then
      remove_checkout "$d" "$label" "$suffix" || true
    else
      kept_why[$d]=$reason
      remaining+=("$last $d")
    fi
  done
done

mapfile -t remaining < <(printf '%s\n' "${remaining[@]}" | sed '/^$/d' | sort -n)
over() { [ "$(pct)" -gt "$cap" ]; }
if over; then
  # Targets first, oldest checkout first.
  for entry in "${remaining[@]}"; do
    over || break
    d=${entry#* }
    if ! claim "$d"; then continue; fi
    if [ -n "$(owner "$d")" ]; then exec 7<&-; continue; fi
    # Under the lock only the cheap part: rename each target to a trash name
    # beside it. The du and rm -rf (minutes for a big target) run after the
    # lock is released, so a review for this checkout is not turned away.
    any=false moved=()
    while IFS= read -r t; do
      [ -n "$t" ] || continue
      if [ "$dry_run" = true ]; then moved+=("$t"); continue; fi
      trash=$t.disk-cleanup-trash-$$
      if ! mv -T -- "$t" "$trash"; then failed+=("${t#"$HOME"/}"); continue; fi
      moved+=("$t")
    done < <(targets_of "$d")
    exec 7<&-
    # Test-only: runs right after the lock is released, before the removal.
    if [ -n "${DISK_CLEANUP_AFTER_RENAME:-}" ]; then
      DISK_CLEANUP_CHECKOUT=$d bash -c "$DISK_CLEANUP_AFTER_RENAME" >/dev/null 2>&1 || true
    fi
    for t in "${moved[@]+"${moved[@]}"}"; do
      trash=$t.disk-cleanup-trash-$$ stopped=""
      if [ "$dry_run" = true ]; then size=$(size_of "$t"); else
        stopped=$(stop_daemons_in "$trash")
        size=$(size_of "$trash")
        if ! rm -rf -- "$trash"; then failed+=("${t#"$HOME"/}"); continue; fi
      fi
      freed=$(( freed + size )); measure "$size"; any=true
      pruned_bytes[$d]=$(( ${pruned_bytes[$d]:-0} + size ))
      pruned+=("${t#"$HOME"/}($(human "$size")$stopped)")
    done
    # Removing target/ touched the checkout: keep its idle time for phase 1.
    if [ "$any" = true ] && [ "$dry_run" = false ]; then touch -m -d "@${entry%% *}" -- "$d" || true; fi
  done
fi
kept=0 kinds=()
for entry in "${remaining[@]}"; do
  d=${entry#* }
  [ -n "${kept_why[$d]:-}" ] || continue
  skipped+=("${d#"$HOME"/}:${kept_why[$d]}")
  kept=$(( kept + 1 ))
  case "${kept_why[$d]}" in fresh*) k=fresh ;; *) k=${kept_why[$d]} ;; esac
  case " ${kinds[*]:-} " in *" $k "*) ;; *) kinds+=("$k") ;; esac
done

note=""
if over; then
  if [ "$kept" -eq 0 ]; then note="still over cap: nothing left that is safe to delete"
  else note="still over cap: $kept kept ($(IFS=';'; echo "${kinds[*]}"))"; fi
fi
notes=""
for n in "$orphan_note" "$ledger_note" "$note"; do [ -z "$n" ] || notes="${notes:+$notes; }$n"; done
finish "$notes"

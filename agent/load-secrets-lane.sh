#!/bin/bash
# relay-desktop's lane step after the kit loads secrets and clones the repo:
# install the review and CI flows once in the lead's checkout.
set -euo pipefail
[ -d /home/agent/src/relay-desktop/flows ] || exit 0
sudo -H -u agent bash -lc '[ -x ~/src/relay-desktop/flows/node_modules/.bin/flows ] || npm --prefix ~/src/relay-desktop/flows ci --no-audit --no-fund --ignore-scripts'
# ~/build/relay-desktop: the reference clone tick.sh, release.sh, review.sh
# and hotfix.sh work from (fetches, tags, --reference clones). A fresh host
# has none; it is cloned from the lead's checkout and pointed at GitHub, so
# later fetches use the same credentials the lead's checkout does.
sudo -H -u agent bash -lc '[ -d ~/build/relay-desktop/.git ] || { mkdir -p ~/build && git clone -q ~/src/relay-desktop ~/build/relay-desktop && git -C ~/build/relay-desktop remote set-url origin https://github.com/AgentWorkforce/relay-desktop.git; }'
# tick.sh, review.sh, release.sh, hotfix.sh and disk-cleanup.sh are not in
# the user data (EC2's 16 KiB raw cap): they come from main's agent/ in the
# reference clone, fetched first, and from nowhere else (the lead's checkout
# can be on an unreviewed branch, and these run from timers). The clone is
# agent-owned, so what it serves is trusted only when the six files hash
# to lane-scripts.sha, which the user data carries (the sha256 of their
# sha256sum listing, as lane-scripts.ts computes it). A mismatch (main moved
# past the last deploy, or a tampered clone) keeps the installed copies,
# records origin/main's hash in lane-scripts.origin and says so; with no
# installed copies at all the lane fails, so a setup that would enable the
# tick timer over a missing script is a red deploy, not a quiet one.
# Installed by atomic rename, so a tick already running keeps its file.
install_lane_scripts() {
  local f tmp sha want
  if [ ! -d /home/agent/build/relay-desktop/.git ]; then
    echo "lane scripts not installed yet: no reference clone on this host" >&2
    lane_scripts_present
    return
  fi
  want=$(cut -c1-64 /opt/agent-host/lane-scripts.sha 2>/dev/null || true)
  if [ -z "$want" ]; then
    echo "warning: /opt/agent-host/lane-scripts.sha is missing; lane scripts not installed" >&2
    lane_scripts_present
    return
  fi
  # tick.sh fetches the same clone under this lock; two fetches at once
  # lose a ref lock. The lock is taken as the agent user, so the file it
  # creates is the agent's (tick.sh and release.sh open it for writing),
  # and ~/state is made as the agent too: root must never chown or chmod a
  # path the agent controls (a planted symlink would redirect it). A tick
  # can hold the lock for minutes (dispatch polling, gh calls), so wait
  # longer than one.
  sudo -H -u agent mkdir -p /home/agent/state
  if ! sudo -H -u agent flock -w 600 /home/agent/state/.tick.lock git -C /home/agent/build/relay-desktop fetch -q origin main; then
    echo "warning: could not fetch main into the reference clone; lane scripts not installed this time" >&2
    lane_scripts_present
    return
  fi
  tmp=$(mktemp -d /opt/agent-host/.lane-scripts.XXXXXX)
  for f in tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh; do
    # This script runs as root: only git runs as agent, the redirect is root's.
    # shellcheck disable=SC2024
    if ! sudo -H -u agent git -C /home/agent/build/relay-desktop --no-replace-objects show "origin/main:agent/$f" > "$tmp/$f" 2>/dev/null; then
      echo "warning: agent/$f is not on origin/main; lane scripts not installed this time" >&2
      rm -rf "$tmp"
      lane_scripts_present
      return
    fi
  done
  sha=$(cd "$tmp" && sha256sum tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh | sha256sum | cut -c1-64)
  if [ "$sha" != "$want" ]; then
    echo "$sha" > /opt/agent-host/lane-scripts.origin
    echo "warning: origin/main's lane scripts hash $sha, the deployed lane-scripts.sha says $want; keeping the installed copies (a landing that changes them deploys the new hash first)" >&2
    rm -rf "$tmp"
    lane_scripts_present
    return
  fi
  for f in tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh; do
    chmod 755 "$tmp/$f" && mv -f "$tmp/$f" "/opt/agent-host/$f"
  done
  rm -rf "$tmp" /opt/agent-host/lane-scripts.origin
}
# After a skipped install: a failure when any script is missing. With
# LANE_SCRIPTS_STRICT set (the deploy's setup lane) also a failure when the
# installed copies are not the ones the deploy's hash expects, so a deploy
# that could not put the scripts it shipped in place is red and rerun,
# never green with old scripts; the boot and refresh lane only warns, so one
# red deploy cannot fail every secrets refresh until the next deploy.
lane_scripts_present() {
  local f have want
  for f in tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh; do
    [ -x "/opt/agent-host/$f" ] || { echo "error: /opt/agent-host/$f is missing and could not be installed" >&2; return 1; }
  done
  want=$(cut -c1-64 /opt/agent-host/lane-scripts.sha 2>/dev/null || true)
  [ -n "$want" ] || return 0
  have=$(cd /opt/agent-host && sha256sum tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh | sha256sum | cut -c1-64)
  [ "$have" = "$want" ] && return 0
  if [ -n "${LANE_SCRIPTS_STRICT:-}" ]; then
    echo "error: the installed lane scripts hash $have, this deploy expects $want, and the install was skipped; rerun the deploy" >&2
    return 1
  fi
  echo "warning: the installed lane scripts hash $have, the deployed lane-scripts.sha says $want; the next deploy installs them" >&2
  return 0
}
install_lane_scripts

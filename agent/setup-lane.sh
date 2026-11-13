#!/bin/bash
# relay-app's lane setup, run as root at the end of the kit's setup.sh
# (first boot, and again by every hot update). Idempotent.
#  - Rust for the agent user (the CI flow's cargo lanes). Apt packages come
#    from aptPackages in agent.config.ts.
#  - ~/reviews (review.sh clones each change there), trusted by Claude Code.
#  - The candidate tick: every 30 minutes, prepare a release candidate when
#    main changed (tick.sh).
#  - The lead's session kept shared to Cloud every 5 minutes
#    (share-lead-session.sh), since the host shares selected sessions only.
#  - Hourly disk cleanup of idle review checkouts and work trees, with a 70%
#    cap on the root volume (disk-cleanup.sh, as the agent user).
#  - The lane scripts the user data does not carry (tick.sh, review.sh,
#    release.sh, hotfix.sh, disk-cleanup.sh), installed from main's agent/.
set -euo pipefail
# shellcheck source=/dev/null
. /opt/agent-host/env

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
  if [ ! -d /home/agent/build/relay-app/.git ]; then
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
  if ! sudo -H -u agent flock -w 600 /home/agent/state/.tick.lock git -C /home/agent/build/relay-app fetch -q origin main; then
    echo "warning: could not fetch main into the reference clone; lane scripts not installed this time" >&2
    lane_scripts_present
    return
  fi
  tmp=$(mktemp -d /opt/agent-host/.lane-scripts.XXXXXX)
  for f in tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh; do
    # This script runs as root: only git runs as agent, the redirect is root's.
    # shellcheck disable=SC2024
    if ! sudo -H -u agent git -C /home/agent/build/relay-app --no-replace-objects show "origin/main:agent/$f" > "$tmp/$f" 2>/dev/null; then
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
install -d -o agent -g agent /home/agent/reviews
sudo -H -u agent bash -lc 'command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable'
sudo -H -u agent python3 - <<'PY'
import json, os
p = os.path.expanduser("~/.claude.json")
c = json.load(open(p)) if os.path.exists(p) else {}
c.setdefault("projects", {}).setdefault("/home/agent/reviews", {}).update(hasTrustDialogAccepted=True, hasCompletedProjectOnboarding=True)
fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f: json.dump(c, f, indent=2)
PY

cat > /etc/systemd/system/agent-host-tick.service <<'UNIT'
[Unit]
Description=relay-app candidate tick (prepare a release candidate when main changed)
After=agent-host-secrets.service
Requires=agent-host-secrets.service
[Service]
Type=oneshot
User=agent
Environment=HOME=/home/agent
EnvironmentFile=/opt/agent-host/env
ExecStart=/opt/agent-host/tick.sh
UNIT
cat > /etc/systemd/system/agent-host-tick.timer <<'UNIT'
[Unit]
Description=relay-app candidate tick every 30 minutes
[Timer]
OnCalendar=*:00/30
RandomizedDelaySec=60
Persistent=true
[Install]
WantedBy=timers.target
UNIT
cat > /etc/systemd/system/agent-host-share-lead.service <<'UNIT'
[Unit]
Description=relay-app lead session kept shared to Agent Relay Cloud
After=agent-relay.service
[Service]
Type=oneshot
User=agent
Environment=HOME=/home/agent
ExecStart=/opt/agent-host/share-lead-session.sh
UNIT
cat > /etc/systemd/system/agent-host-share-lead.timer <<'UNIT'
[Unit]
Description=relay-app lead session share check every 5 minutes
[Timer]
OnCalendar=*:00/5
RandomizedDelaySec=30
Persistent=true
[Install]
WantedBy=timers.target
UNIT
cat > /etc/systemd/system/agent-host-disk-cleanup.service <<'UNIT'
[Unit]
Description=relay-app disk cleanup (idle review checkouts and work trees, 70% cap)
[Service]
Type=oneshot
User=agent
Environment=HOME=/home/agent
Nice=19
IOSchedulingClass=idle
ExecStart=/opt/agent-host/disk-cleanup.sh
UNIT
cat > /etc/systemd/system/agent-host-disk-cleanup.timer <<'UNIT'
[Unit]
Description=relay-app disk cleanup every hour
[Timer]
OnCalendar=hourly
RandomizedDelaySec=300
Persistent=true
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now agent-host-tick.timer
systemctl enable --now agent-host-share-lead.timer
systemctl enable --now agent-host-disk-cleanup.timer
# Last, so a failed install still leaves the units above in place for the
# rerun that follows a red deploy; strict, so stale scripts make it red.
LANE_SCRIPTS_STRICT=1
install_lane_scripts

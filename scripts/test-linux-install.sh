#!/usr/bin/env bash
#
# test-linux-install.sh -- exercise linux/install.sh end to end on Linux with
# a fake release server and fake systemctl, curl and dpkg-query on PATH: a
# first install, an idempotent rerun, an update, a bad checksum, a new version
# that never answers or that systemd refuses to start (rolled back),
# --rollback (also back to an in-app updater's core it moved from), a .deb
# install it leaves to apt, and a site profile's copy of the script, which
# reads only its own releases repository and replaces another product's core
# even at the same version, and a cloud profile's copy against a fixture
# release feed (scripts/test-linux-install-cloud.sh). The core's data
# directory must come through untouched.
#
#   scripts/test-linux-install.sh            (Linux, or in a container:)
#   docker run --rm -v "$PWD":/src -w /src debian:bookworm-slim scripts/test-linux-install.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL="$REPO/linux/install.sh"
[ "$(uname -s)" = Linux ] || { echo "run on Linux (see the header)"; exit 2; }
# The HOME and XDG directories this was started with, kept across the
# re-exec below: the guard refuses them, and anything under them, as the
# test's HOME, XDG_CONFIG_HOME or XDG_DATA_HOME.
export INSTALL_TEST_ORIGINAL_HOME="${INSTALL_TEST_ORIGINAL_HOME:-${HOME:-}}"
export INSTALL_TEST_ORIGINAL_XDG_CONFIG_HOME="${INSTALL_TEST_ORIGINAL_XDG_CONFIG_HOME:-${XDG_CONFIG_HOME:-}}"
export INSTALL_TEST_ORIGINAL_XDG_DATA_HOME="${INSTALL_TEST_ORIGINAL_XDG_DATA_HOME:-${XDG_DATA_HOME:-}}"
[ "$(id -u)" != 0 ] || exec setpriv --reuid=65534 --regid=65534 --clear-groups env HOME=/tmp/install-test-home "$0" "$@"

# A throwaway HOME, marked as the harness's own before anything is removed;
# require_disposable_home (scripts/install-test-home.sh) refuses any other.
# shellcheck source=scripts/install-test-home.sh
. "$REPO/scripts/install-test-home.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
export HOME="$root/home" XDG_DATA_HOME="$root/home/.local/share" XDG_CONFIG_HOME="$root/home/.config"
mkdir -p "$HOME"
install_test_home_init
require_disposable_home
mkdir -p "$root/bin" "$root/releases" "$root/state"
case "$(uname -m)" in x86_64|amd64) arch=x64 ;; *) arch=arm64 ;; esac

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

# A fake core: reports its version, and answers status.get with it only
# while the fake service manager says it is the one running.
# A shipped archive also holds agent_relay/LICENSE; older ones hold the core
# alone. ARCHIVE_EXTRA picks what else goes in: license (the default, as
# shipped), none (an older release), license-link (a LICENSE that is a link)
# or stray (a file that is neither). RELEASE_PREFIX and RELEASE_ROOT pick the
# product's asset prefix and where its releases are served from (Agent
# Relay's by default).
release() { # version [broken]
  local prefix="${RELEASE_PREFIX:-AgentRelay}" releases="${RELEASE_ROOT:-$root/releases}"
  local version="$1" dir="$releases/v$1" tree extra="${ARCHIVE_EXTRA:-license}"
  local queue_formats="${QUEUE_FORMATS:-}" migrate="${MIGRATE_ON_START:-0}"
  local capability_reply raw_capabilities=0
  printf -v capability_reply '%q' "${CAPABILITY_RAW:-}"
  [ -z "${CAPABILITY_RAW+x}" ] || raw_capabilities=1
  tree="$(mktemp -d)"
  mkdir -p "$dir" "$tree/agent_relay/helpers"
  cat > "$tree/agent_relay/helpers/agent-relay-probe" <<EOF
#!/usr/bin/env bash
# product: $prefix
if [ "\$1" = --version ]; then echo "agent-relay-probe $version"; exit 0; fi
if [ "\$1" = --local-storage-capabilities ]; then
  if [ "$raw_capabilities" = 1 ]; then
    printf '%s\\n' $capability_reply
    exit 0
  fi
  [ -n "$queue_formats" ] || exit 2
  echo '{"protocol":1,"upload_queue_formats":[$queue_formats]}'
  exit 0
fi
# Like the real core, it drops the \`relay\` word earlier releases needed.
[ "\$1" = relay ] && shift
if [ "\$1 \$2" = 'serve --headless' ] && [ "$migrate" = 1 ]; then
  store="\$HOME/.agentworkforce/probe/store"
  mkdir -p "\$store"
  exec {migration_fd}>>"\$store/delivery-queue.migration-lock"
  flock -n "\$migration_fd" || exit 73
  echo acquired >> "$root/state/owner-migration-lock"
  python3 - "\$HOME/.agentworkforce/probe/store" <<'PY'
import json, pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1]); root.mkdir(parents=True, exist_ok=True)
for name, phase in [('history.db', 'done'), ('delivery-queue.db', 'ready')]:
    c = sqlite3.connect(root / name)
    c.execute('CREATE TABLE IF NOT EXISTS _relay_delivery_sidecar(singleton PRIMARY KEY, phase, manifest)')
    c.execute('INSERT OR REPLACE INTO _relay_delivery_sidecar VALUES(1,?,?)', (phase, json.dumps({'version':1})))
    c.commit(); c.close()
PY
  flock -u "\$migration_fd"
  exec {migration_fd}>&-
fi
if [ "\$1" = "setup" ]; then
  printf '%s\n' "\$*" >> "$root/state/setup-calls"
  [ "\$2" = --help ] || { echo "installer must not start onboarding" >&2; exit 99; }
  echo 'Usage: agent-relay-probe setup [OPTIONS]'
  exit 0
fi
if [ "\$1 \$2" = "call status.get" ]; then
  [ "${2:-}" = broken ] && exit 1
  [ "\$(cat "$root/state/running" 2>/dev/null)" = "$version" ] || exit 1
  echo '{"bridge_version":1,"result":{"daemon":{"version":"$version","pid":1},"spawn":null}}'
  exit 0
fi
exit 0
EOF
  chmod 0755 "$tree/agent_relay/helpers/agent-relay-probe"
  case "$extra" in
    license) echo "Apache License" > "$tree/agent_relay/LICENSE" ;;
    license-link) ln -s /etc/hostname "$tree/agent_relay/LICENSE" ;;
    stray) echo "Apache License" > "$tree/agent_relay/LICENSE"; echo x > "$tree/agent_relay/README" ;;
    none) ;;
  esac
  local asset="$prefix-$version-Linux-$arch-probe.tar.gz"
  tar -czf "$dir/$asset" -C "$tree" agent_relay
  (cd "$dir" && sha256sum "$asset" > "$asset.sha256")
  rm -rf "$tree"
}

cat > "$root/bin/curl" <<EOF
#!/usr/bin/env bash
# Serves https://releases.test/... from $root/releases, and the acme
# profile's GitHub releases repository from $root/releases-acme.
out=""; url=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) out="\$2"; shift 2 ;; --proto|--retry) shift 2 ;; -*) shift ;; *) url="\$1"; shift ;; esac
done
echo "\$url" >> "$root/state/curl.log"
if [ "\${ASSERT_STORAGE_UNLOCKED_DURING_DOWNLOAD:-0}" = 1 ]; then
  for lock in "\$HOME"/.agentworkforce/probe/*/delivery-queue.migration-lock; do
    [ ! -e "\$lock" ] || flock -n "\$lock" true || exit 98
  done
fi
acme_api=https://api.github.com/repos/AgentWorkforce/relay-desktop-releases-acme/releases
acme_download=https://github.com/AgentWorkforce/relay-desktop-releases-acme/releases/download
case "\$url" in
  https://api.test/releases/latest) printf '{"tag_name": "v%s"}' "\$(cat "$root/state/latest")" > "\$out" ;;
  https://releases.test/*) cp "$root/releases/\${url#https://releases.test/}" "\$out" 2>/dev/null || exit 22 ;;
  "\$acme_api/latest") printf '{"tag_name": "v%s"}' "\$(cat "$root/state/acme-latest")" > "\$out" ;;
  "\$acme_download"/*) cp "$root/releases-acme/\${url#"\$acme_download"/}" "\$out" 2>/dev/null || exit 22 ;;
  *) exit 6 ;;
esac
EOF
cat > "$root/bin/systemctl" <<EOF
#!/usr/bin/env bash
# Records each call; a (re)start runs what the unit's ExecStart names: the
# version current points at, or an in-app updater install by its path.
echo "\$*" >> "$root/state/systemctl.log"
start() {
  local line exe
  line="\$(sed -n 's/^ExecStart=//p' "$XDG_CONFIG_HOME/systemd/user/agent-relay.service" | head -n 1)"
  exe="\${line%% *}"
  # A core that rejects the unit's command line never runs.
  \$line || { rm -f "$root/state/running"; return 0; }
  case "\$exe" in
    */current/*) basename "\$(readlink "$HOME/.local/lib/agent-relay/current")" ;;
    *) "\$exe" --version | awk '{print \$NF}' ;;
  esac > "$root/state/running"
}
case "\$*" in
  "--user daemon-reload") ;;
  "--user is-enabled --quiet agent-relay.service") [ -f "$root/state/enabled" ] ;;
  "--user reset-failed agent-relay.service") ;;
  "--user restart agent-relay.service")
    if [ -f "$root/state/restart-fails" ]; then rm "$root/state/restart-fails"; exit 1; fi
    start ;;
  "--user enable --now agent-relay.service") touch "$root/state/enabled"; start ;;
esac
EOF
cat > "$root/bin/dpkg-query" <<EOF
#!/usr/bin/env bash
[ -f "$root/state/deb" ] && { case "\$*" in *Status*) echo "install ok installed" ;; *) echo "2026.1.1" ;; esac; exit 0; }
exit 1
EOF
chmod 0755 "$root/bin/"*
export PATH="$root/bin:$PATH"
export AGENT_RELAY_RELEASES_API=https://api.test/releases
export AGENT_RELAY_DOWNLOAD_BASE=https://releases.test
export AGENT_RELAY_HEALTH_TIMEOUT=4

current() { basename "$(readlink "$HOME/.local/lib/agent-relay/current")"; }
restarts() { grep -cE 'restart|enable --now' "$root/state/systemctl.log" 2>/dev/null || true; }

# The core's data: settings, sign-in, agents. Never touched.
data="$XDG_DATA_HOME/com.agentrelay.desktop"
mkdir -p "$data" && echo '{"cloudSession":"kept"}' > "$data/injectable-agents.json"
data_sum="$(sha256sum "$data/injectable-agents.json")"

ARCHIVE_EXTRA=none release 1.0.0; release 1.1.0; release 1.2.0; release 1.3.0 broken; release 0.9.0 broken; release 1.4.0
echo 1.0.0 > "$root/state/latest"
bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install failed"; }
[ "$(current)" = 1.0.0 ] || fail "first install points at $(current)"
grep -q "ExecStart=$HOME/.local/lib/agent-relay/current/agent-relay-probe relay serve --headless" \
  "$XDG_CONFIG_HOME/systemd/user/agent-relay.service" || fail "unit not written"
grep -q 'enable --now' "$root/state/systemctl.log" || fail "first install did not enable the service"
grep -Fq "$HOME/.local/lib/agent-relay/current/agent-relay-probe setup" "$root/state/out" || fail "wizard command missing from install instructions"
grep -q 'browser on any computer' "$root/state/out" || fail "remote approval instructions missing"
[ "$(cat "$root/state/setup-calls")" = "setup --help" ] || fail "installer ran onboarding instead of only showing instructions"
pass "first install enables and starts 1.0.0"

before="$(restarts)"
bash "$INSTALL" > "$root/state/out" 2>&1 || fail "rerun failed"
grep -q "already up to date" "$root/state/out" || fail "rerun was not a no-op"
[ "$(restarts)" = "$before" ] || fail "rerun restarted the service"
pass "rerun leaves a healthy install alone"

echo 1.1.0 > "$root/state/latest"
bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install failed"; }
[ "$(current)" = 1.1.0 ] || fail "update points at $(current)"
[ -d "$HOME/.local/lib/agent-relay/versions/1.0.0" ] || fail "previous version removed"
grep -q 'restart agent-relay.service' "$root/state/systemctl.log" || fail "update did not restart"
pass "update to 1.1.0 restarts and keeps 1.0.0"
[ ! -e "$HOME/.local/lib/agent-relay/versions/1.0.0/LICENSE" ] || fail "an archive without a license grew one"
[ "$(cat "$HOME/.local/lib/agent-relay/versions/1.1.0/LICENSE")" = "Apache License" ] \
  || fail "the archive's LICENSE was not installed beside the core"
[ ! -x "$HOME/.local/lib/agent-relay/versions/1.1.0/LICENSE" ] || fail "the LICENSE was installed executable"
pass "the license ships beside the core, and an archive without one still installs"

ARCHIVE_EXTRA=stray release 1.5.0; ARCHIVE_EXTRA=license-link release 1.6.0
for bad in 1.5.0 1.6.0; do
  if bash "$INSTALL" --version "$bad" >/dev/null 2>&1; then fail "archive $bad was accepted"; fi
  [ "$(current)" = 1.1.0 ] || fail "archive $bad changed current"
  [ ! -d "$HOME/.local/lib/agent-relay/versions/$bad" ] || fail "archive $bad was unpacked"
done
pass "an archive with a stray file or a linked LICENSE changes nothing"

echo "0000000000000000000000000000000000000000000000000000000000000000  x" \
  > "$root/releases/v1.2.0/AgentRelay-1.2.0-Linux-$arch-probe.tar.gz.sha256"
if bash "$INSTALL" --version 1.2.0 >/dev/null 2>&1; then fail "a bad checksum was accepted"; fi
[ "$(current)" = 1.1.0 ] || fail "a bad checksum changed current"
[ ! -d "$HOME/.local/lib/agent-relay/versions/1.2.0" ] || fail "a bad download was unpacked"
pass "a checksum mismatch changes nothing"

if bash "$INSTALL" --version 1.3.0 >/dev/null 2>&1; then fail "a core that never answers was accepted"; fi
[ "$(current)" = 1.1.0 ] || fail "the failed update was not rolled back ($(current))"
[ ! -d "$HOME/.local/lib/agent-relay/versions/1.3.0" ] || fail "the failed version was kept"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after rollback"
pass "a new core that never answers is rolled back"

touch "$root/state/restart-fails"
if bash "$INSTALL" --version 1.4.0 >/dev/null 2>&1; then fail "a refused restart was accepted"; fi
[ "$(current)" = 1.1.0 ] || fail "a refused restart was not rolled back ($(current))"
[ ! -d "$HOME/.local/lib/agent-relay/versions/1.4.0" ] || fail "the version systemd refused was kept"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after a refused restart"
pass "a restart systemd refuses is rolled back"

bash "$INSTALL" --rollback >/dev/null
[ "$(current)" = 1.0.0 ] || fail "rollback picked $(current)"
pass "--rollback switches to an earlier version ($(current))"

# With 1.0.0, 1.1.0 and 1.2.0 installed, rollbacks only go back: 1.2.0 ->
# 1.1.0 -> 1.0.0, then there is nothing earlier and nothing changes.
(cd "$root/releases/v1.2.0" && sha256sum "AgentRelay-1.2.0-Linux-$arch-probe.tar.gz" > "AgentRelay-1.2.0-Linux-$arch-probe.tar.gz.sha256")
bash "$INSTALL" --version 1.2.0 >/dev/null 2>&1 || fail "installing 1.2.0 failed"
[ "$(current)" = 1.2.0 ] || fail "1.2.0 install points at $(current)"
bash "$INSTALL" --rollback >/dev/null 2>&1 || fail "first of two rollbacks failed"
[ "$(current)" = 1.1.0 ] || fail "first rollback picked $(current)"
bash "$INSTALL" --rollback >/dev/null 2>&1 || fail "second of two rollbacks failed"
[ "$(current)" = 1.0.0 ] || fail "second rollback picked $(current), not 1.0.0"
before="$(restarts)"
set +e; bash "$INSTALL" --rollback > "$root/state/out" 2>&1; status=$?; set -e
[ "$status" != 0 ] || fail "a rollback with no earlier version succeeded"
grep -q "no earlier version" "$root/state/out" || fail "no earlier version was not reported"
[ "$(current)" = 1.0.0 ] || fail "a rollback with no earlier version moved current to $(current)"
[ "$(cat "$root/state/running")" = 1.0.0 ] || fail "the running core changed on a rollback with no earlier version"
[ "$(restarts)" = "$before" ] || fail "a rollback with no earlier version restarted the service"
pass "--rollback only goes to older versions and stops at the oldest"

# Prereleases go by SemVer precedence, which sort -V gets wrong: 1.2.0-rc.1 is
# older than 1.2.0, so a rollback from 1.2.0-rc.1 skips the newer 1.2.0 for
# 1.1.0 and one from 1.2.0 reaches 1.2.0-rc.1; 1.3.0-beta.2 is older than
# 1.3.0-beta.11.
release 1.2.0-rc.1; release 1.3.0-beta.2; release 1.3.0-beta.11
bash "$INSTALL" --version 1.2.0-rc.1 >/dev/null 2>&1 || fail "installing 1.2.0-rc.1 failed"
for kept in 1.1.0 1.2.0; do
  [ -d "$HOME/.local/lib/agent-relay/versions/$kept" ] || fail "$kept is not installed beside 1.2.0-rc.1"
done
bash "$INSTALL" --rollback >/dev/null 2>&1 || fail "rollback from 1.2.0-rc.1 failed"
[ "$(current)" = 1.1.0 ] || fail "rollback from 1.2.0-rc.1 picked $(current), not 1.1.0"
bash "$INSTALL" --version 1.2.0 >/dev/null 2>&1 || fail "reinstalling 1.2.0 failed"
bash "$INSTALL" --rollback >/dev/null 2>&1 || fail "rollback from 1.2.0 failed"
[ "$(current)" = 1.2.0-rc.1 ] || fail "rollback from 1.2.0 picked $(current), not 1.2.0-rc.1"
bash "$INSTALL" --version 1.3.0-beta.2 >/dev/null 2>&1 || fail "installing 1.3.0-beta.2 failed"
bash "$INSTALL" --version 1.3.0-beta.11 >/dev/null 2>&1 || fail "installing 1.3.0-beta.11 failed"
bash "$INSTALL" --rollback >/dev/null 2>&1 || fail "rollback from 1.3.0-beta.11 failed"
[ "$(current)" = 1.3.0-beta.2 ] || fail "rollback from 1.3.0-beta.11 picked $(current), not 1.3.0-beta.2"
[ "$(cat "$root/state/running")" = 1.3.0-beta.2 ] || fail "1.3.0-beta.2 is not running after rollback"
pass "--rollback orders prereleases by SemVer precedence"

bash "$INSTALL" --version 1.1.0 >/dev/null
before="$(restarts)"
bash "$INSTALL" --dry-run --version 1.0.0 >/dev/null
if [ "$(current)" != 1.1.0 ] || [ "$(restarts)" != "$before" ]; then
  fail "--dry-run changed something"
fi
pass "--dry-run changes nothing"

touch "$root/state/deb"
set +e; bash "$INSTALL" >/dev/null 2>&1; status=$?; set -e
[ "$status" = 3 ] || fail "a .deb install was not left to apt (exit $status)"
rm "$root/state/deb"
pass "a .deb install is left to apt"

# Moving over from the in-app updater's layout, whose core is not under
# versions/ and runs from the unit's own ExecStart.
require_disposable_home
rm -rf "$HOME/.local/lib/agent-relay/current" "$HOME/.local/lib/agent-relay/versions"
legacy="$HOME/.local/lib/agent-relay/0.9.0/AgentRelay-linux-x64/usr/lib/agent-relay/agent-relay-probe"
mkdir -p "$(dirname "$legacy")"
cat > "$legacy" <<EOF
#!/usr/bin/env bash
if [ "\$1" = --version ]; then echo "agent-relay-probe 0.9.0"; exit 0; fi
# An earlier release: it knows its commands only after the \`relay\` word.
if [ "\$1 \$2 \$3" = "relay call status.get" ]; then
  [ "\$(cat "$root/state/running" 2>/dev/null)" = 0.9.0 ] || exit 1
  echo '{"bridge_version":1,"result":{"daemon":{"version":"0.9.0","pid":1},"spawn":null}}'
  exit 0
fi
[ "\$1 \$2" = "relay serve" ] && exit 0
exit 2
EOF
chmod 0755 "$legacy"
legacy_unit="$(printf '[Service]\nExecStart=%s relay serve --headless\nRestart=always\n' "$legacy")"
require_disposable_home
printf '%s\n' "$legacy_unit" > "$XDG_CONFIG_HOME/systemd/user/agent-relay.service"
echo 0.9.0 > "$root/state/running"

if bash "$INSTALL" --version 1.3.0 >/dev/null 2>&1; then fail "a core that never answers was accepted"; fi
[ "$(cat "$XDG_CONFIG_HOME/systemd/user/agent-relay.service")" = "$legacy_unit" ] \
  || fail "the in-app updater's unit was not restored"
[ ! -e "$HOME/.local/lib/agent-relay/current" ] || fail "current was left pointing at the failed core"
[ ! -d "$HOME/.local/lib/agent-relay/versions/1.3.0" ] || fail "the failed version was kept"
[ "$(cat "$root/state/running")" = 0.9.0 ] || fail "the in-app updater's core is not running again"
pass "a failed move from the in-app updater's install goes back to it"

# The same version as the in-app updater's: versions/0.9.0 is the release
# just unpacked, never a rollback target.
if bash "$INSTALL" --version 0.9.0 >/dev/null 2>&1; then fail "a core that never answers was accepted"; fi
[ "$(cat "$XDG_CONFIG_HOME/systemd/user/agent-relay.service")" = "$legacy_unit" ] \
  || fail "a failed same-version move did not restore the in-app updater's unit"
[ ! -e "$HOME/.local/lib/agent-relay/current" ] || fail "a failed same-version move left current behind"
[ ! -d "$HOME/.local/lib/agent-relay/versions/0.9.0" ] || fail "the failed same-version core was kept"
if [ "$(cat "$root/state/running")" != 0.9.0 ] || [ ! -x "$legacy" ]; then
  fail "the in-app updater's core is not running again"
fi
pass "a failed same-version move from the in-app updater's install goes back to it"

bash "$INSTALL" --version 1.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "move from the in-app updater failed"; }
[ "$(current)" = 1.1.0 ] || fail "the move points at $(current)"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after the move"
pass "a move from the in-app updater's install switches to the managed layout"

bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "rollback after the move failed"; }
[ "$(current)" = 0.9.0 ] || fail "rollback after the move picked $(current)"
[ "$(cat "$root/state/running")" = 0.9.0 ] || fail "the in-app updater's core is not running after rollback"
pass "--rollback after a move goes back to the in-app updater's core"

# A site profile's copy of install.sh, made the way Stage the release assets
# makes it (desktop-release.yml): its product name, releases repository and
# asset prefix replace Agent Relay's. Its release holds only its own assets,
# and it is run without the test's URL overrides, so it reads the defaults
# its RELEASES_REPOSITORY gives.
acme_install="$root/install-acme.sh"
sed -e "s|^PRODUCT_NAME=.*|PRODUCT_NAME='Acme Relay'|" \
  -e "s|^RELEASES_REPOSITORY=.*|RELEASES_REPOSITORY='AgentWorkforce/relay-desktop-releases-acme'|" \
  -e "s|^ASSET_PREFIX=.*|ASSET_PREFIX='Desktop-acme-7'|" "$INSTALL" > "$acme_install"
[ "$(grep -c "^ASSET_PREFIX='Desktop-acme-7'$" "$acme_install")" = 1 ] || fail "the acme copy was not made"
acme() { env -u AGENT_RELAY_RELEASES_API -u AGENT_RELAY_DOWNLOAD_BASE bash "$acme_install"; }
product_of() { head -n 1 "$HOME/.local/lib/agent-relay/current/product" 2>/dev/null || echo none; }
core_product() { sed -n 's/^# product: //p' "$HOME/.local/lib/agent-relay/current/agent-relay-probe"; }
mkdir -p "$root/releases-acme"
release 2.0.0; release 2.1.0
RELEASE_PREFIX=Desktop-acme-7 RELEASE_ROOT="$root/releases-acme" release 2.0.0
RELEASE_PREFIX=Desktop-acme-7 RELEASE_ROOT="$root/releases-acme" release 2.1.0 broken
echo 2.0.0 > "$root/state/latest"; echo 2.0.0 > "$root/state/acme-latest"

bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of 2.0.0 failed"; }
[ "$(product_of)" = AgentRelay ] || fail "Agent Relay's install recorded $(product_of)"
pass "an install records its product"

# An install from before the record counts as Agent Relay's.
require_disposable_home
rm "$HOME/.local/lib/agent-relay/current/product"
before="$(restarts)"
bash "$INSTALL" > "$root/state/out" 2>&1 || fail "rerun without a record failed"
grep -q "already up to date" "$root/state/out" || fail "Agent Relay did not count an unrecorded install as its own"
[ "$(restarts)" = "$before" ] || fail "a rerun over an unrecorded install restarted the service"
pass "an install without a product record is Agent Relay's"

: > "$root/state/curl.log"
acme > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the acme install over Agent Relay 2.0.0 failed"; }
! grep -q "already up to date" "$root/state/out" || fail "the acme install took Agent Relay's 2.0.0 as its own"
[ "$(current)" = 2.0.0 ] || fail "the acme install points at $(current)"
[ "$(product_of)" = Desktop-acme-7 ] || fail "the acme install recorded $(product_of)"
[ "$(core_product)" = Desktop-acme-7 ] || fail "the core is still $(core_product)'s"
[ "$(cat "$root/state/running")" = 2.0.0 ] || fail "2.0.0 is not running after the acme install"
[ ! -e "$HOME/.local/lib/agent-relay/versions/2.0.0.replaced" ] || fail "Agent Relay's 2.0.0 was left aside"
grep -qxF "https://api.github.com/repos/AgentWorkforce/relay-desktop-releases-acme/releases/latest" "$root/state/curl.log" \
  || fail "the acme install did not read its own releases API"
grep -qxF "https://github.com/AgentWorkforce/relay-desktop-releases-acme/releases/download/v2.0.0/Desktop-acme-7-2.0.0-Linux-$arch-probe.tar.gz" "$root/state/curl.log" \
  || fail "the acme install did not download its own asset"
! grep -v relay-desktop-releases-acme "$root/state/curl.log" | grep -q . || fail "the acme install read another repository"
pass "a profile's install.sh reads its own repository and replaces Agent Relay's core at the same version"

before="$(restarts)"
acme > "$root/state/out" 2>&1 || fail "the acme rerun failed"
grep -q "already up to date" "$root/state/out" || fail "the acme rerun was not a no-op"
[ "$(restarts)" = "$before" ] || fail "the acme rerun restarted the service"
pass "the same product at the same version is already up to date"

bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "Agent Relay over acme 2.0.0 failed"; }
[ "$(product_of)" = AgentRelay ] || fail "Agent Relay over acme recorded $(product_of)"
[ "$(core_product)" = AgentRelay ] || fail "the core is still $(core_product)'s"
pass "Agent Relay replaces a profile's core at the same version"

bash "$INSTALL" --version 2.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of 2.1.0 failed"; }
echo 2.1.0 > "$root/state/acme-latest"
if acme > "$root/state/out" 2>&1; then fail "an acme core that never answers was accepted"; fi
grep -q "replacing the installed 2.1.0 failed and it is running again" "$root/state/out" \
  || { cat "$root/state/out"; fail "the failed replacement did not go back to Agent Relay's 2.1.0"; }
[ "$(current)" = 2.1.0 ] || fail "the failed replacement points at $(current)"
[ "$(product_of)" = AgentRelay ] || fail "the failed replacement left $(product_of) recorded"
[ "$(core_product)" = AgentRelay ] || fail "Agent Relay's 2.1.0 was not put back"
[ ! -e "$HOME/.local/lib/agent-relay/versions/2.1.0.replaced" ] || fail "the set-aside copy was left behind"
[ "$(cat "$root/state/running")" = 2.1.0 ] || fail "2.1.0 is not running after the failed replacement"
pass "a failed replacement at the same version puts the other product's core back"

# Storage transitions exercise the real installer with only the core/network
# boundaries faked. The fixtures and their databases live under this test HOME.
# shellcheck source=scripts/test-linux-install-storage.sh
. "$REPO/scripts/test-linux-install-storage.sh"

# A cloud profile's copy, against a fixture release feed.
# shellcheck source=scripts/test-linux-install-cloud.sh
. "$REPO/scripts/test-linux-install-cloud.sh"

[ "$(sha256sum "$data/injectable-agents.json")" = "$data_sum" ] || fail "the core's data changed"
pass "settings and sign-in data untouched"
echo "all install tests passed"

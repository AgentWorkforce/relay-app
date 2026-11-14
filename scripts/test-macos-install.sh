#!/usr/bin/env bash
#
# test-macos-install.sh -- exercise macos/install.sh end to end on Linux (or a
# Mac) with a fake release server and fake uname, sysctl, curl, shasum,
# codesign, launchctl and sqlite3 on PATH, in a throwaway HOME: refusals (not
# macOS, the app installed or loaded, no login session, --update with nothing
# installed), a first install and the LaunchAgent it writes, an idempotent
# rerun, an update, a bad checksum, an unsigned core (never run), a stray
# archive, a new version that never answers or that launchd refuses to
# restart (rolled back), --rollback, --dry-run, --status, pruning to three
# versions in numeric order, the local history checks, and --uninstall, which
# keeps the core's data directory. Nothing here talks to a real launchd.
#
#   scripts/test-macos-install.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL="$REPO/macos/install.sh"
export INSTALL_TEST_ORIGINAL_HOME="${INSTALL_TEST_ORIGINAL_HOME:-${HOME:-}}"
export INSTALL_TEST_ORIGINAL_XDG_CONFIG_HOME="${INSTALL_TEST_ORIGINAL_XDG_CONFIG_HOME:-${XDG_CONFIG_HOME:-}}"
export INSTALL_TEST_ORIGINAL_XDG_DATA_HOME="${INSTALL_TEST_ORIGINAL_XDG_DATA_HOME:-${XDG_DATA_HOME:-}}"
[ "$(id -u)" != 0 ] || exec setpriv --reuid=65534 --regid=65534 --clear-groups env HOME=/tmp/install-test-home "$0" "$@"
command -v python3 >/dev/null || { echo "python3 is required (for the fake sqlite3 and plist checks)"; exit 2; }

# shellcheck source=scripts/install-test-home.sh
. "$REPO/scripts/install-test-home.sh"
real_uname="$(command -v uname)"
real_sha256="$(command -v sha256sum || true)"
root="$(mktemp -d)"
holder=""
trap '[ -z "$holder" ] || kill "$holder" 2>/dev/null || true; rm -rf "$root"' EXIT
export HOME="$root/home" XDG_DATA_HOME="$root/home/.local/share" XDG_CONFIG_HOME="$root/home/.config"
mkdir -p "$HOME"
install_test_home_init
require_disposable_home
mkdir -p "$root/bin" "$root/releases" "$root/state"
uid="$(id -u)"
label=com.agentrelay.desktop.core.headless
prefix="$HOME/.local/lib/agent-relay"
plist="$HOME/Library/LaunchAgents/$label.plist"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

# A fake core: reports its version, and answers status.get with it (and an
# account secret the installer must never print) only while the fake launchd
# says it is the one running. KIND: ok, broken (never answers) or unsigned
# (fails codesign, and records being run at all).
release() { # version [kind]
  local version="$1" kind="${2:-ok}" dir="$root/releases/v$1" tree extra="${ARCHIVE_EXTRA:-license}"
  local formats="${QUEUE_FORMATS:-0,1}"
  tree="$(mktemp -d)"
  mkdir -p "$dir" "$tree/agent_relay/helpers"
  cat > "$tree/agent_relay/helpers/agent-relay-probe" <<EOF
#!/usr/bin/env bash
# kind: $kind
[ "$kind" != unsigned ] || touch "$root/state/ran-unsigned"
if [ "\$1" = --version ]; then echo "agent-relay-probe $version"; exit 0; fi
if [ "\$1" = --local-storage-capabilities ]; then
  echo '{"protocol":1,"upload_queue_formats":[$formats]}'
  exit 0
fi
# Like the real core, it drops the \`relay\` word earlier releases needed.
[ "\$1" = relay ] && shift
if [ "\$1" = setup ]; then
  [ "\$2" = --help ] || { echo "installer must not start onboarding" >&2; exit 99; }
  echo 'Usage: agent-relay-probe setup [OPTIONS]'
  exit 0
fi
if [ "\$1 \$2" = "call status.get" ]; then
  [ "$kind" = broken ] && exit 1
  [ "\$(cat "$root/state/running" 2>/dev/null)" = "$version" ] || exit 1
  echo '{"bridge_version":1,"result":{"daemon":{"version":"$version","pid":1},"account":{"token":"secret-token-do-not-print"}}}'
  exit 0
fi
[ "\$1 \$2" = "serve --headless" ] && exit 0
exit 2
EOF
  chmod 0755 "$tree/agent_relay/helpers/agent-relay-probe"
  case "$extra" in
    license) echo "License" > "$tree/agent_relay/LICENSE" ;;
    stray) echo "License" > "$tree/agent_relay/LICENSE"; echo x > "$tree/agent_relay/README" ;;
  esac
  local asset="AgentRelay-$version-macOS-arm64-probe.tar.gz"
  tar -czf "$dir/$asset" -C "$tree" agent_relay
  (cd "$dir" && sha256sum "$asset" > "$asset.sha256")
  rm -rf "$tree"
}

cat > "$root/bin/uname" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
  -s) echo "\${FAKE_UNAME_S:-Darwin}" ;;
  -m) echo "\${FAKE_UNAME_M:-arm64}" ;;
  *) exec "$real_uname" "\$@" ;;
esac
EOF
cat > "$root/bin/sysctl" <<'EOF'
#!/usr/bin/env bash
[ "$*" = "-n sysctl.proc_translated" ] && [ "${FAKE_TRANSLATED:-0}" = 1 ] && { echo 1; exit 0; }
exit 1
EOF
cat > "$root/bin/shasum" <<EOF
#!/usr/bin/env bash
[ "\$1 \$2" = "-a 256" ] || exit 2
exec "$real_sha256" "\$3"
EOF
cat > "$root/bin/codesign" <<EOF
#!/usr/bin/env bash
# Passes a core unless it is the unsigned kind; the requirement must name the team.
echo "\$*" >> "$root/state/codesign.log"
[ "\$1 \$2" = "--verify --strict" ] || exit 2
case "\$3" in -R=*'leaf[subject.OU] = "QUJ7SA6X8X"'*) ;; *) exit 3 ;; esac
grep -q '^# kind: unsigned' "\$4" && { echo "\$4: invalid signature" >&2; exit 1; }
exit 0
EOF
cat > "$root/bin/curl" <<EOF
#!/usr/bin/env bash
out=""; url=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) out="\$2"; shift 2 ;; --proto|--retry) shift 2 ;; -*) shift ;; *) url="\$1"; shift ;; esac
done
echo "\$url" >> "$root/state/curl.log"
case "\$url" in
  https://api.test/releases/latest) printf '{"tag_name": "v%s"}' "\$(cat "$root/state/latest")" > "\$out" ;;
  https://releases.test/*) cp "$root/releases/\${url#https://releases.test/}" "\$out" 2>/dev/null || exit 22 ;;
  *) exit 6 ;;
esac
EOF
# A fake launchd for gui/<uid>: loaded jobs are files in state/, a job's start
# runs the ProgramArguments its plist names (parsed as a real property list)
# and records the version `current` points at as the running core.
cat > "$root/bin/launchctl" <<EOF
#!/usr/bin/env bash
echo "\$*" >> "$root/state/launchctl.log"
state="$root/state"
start() {
  local args=()
  while IFS= read -r line; do args+=("\$line"); done < "\$state/job-args"
  "\${args[@]}" || { rm -f "\$state/running"; return 0; }
  basename "\$(readlink "$prefix/current")" > "\$state/running"
}
case "\$1" in
  print)
    case "\$2" in
      gui/$uid) [ ! -f "\$state/no-gui" ] ;;
      gui/$uid/*) [ -f "\$state/loaded-\${2##*/}" ] || exit 113; printf '\tstate = running\n\tpid = 4242\n' ;;
      *) exit 113 ;;
    esac ;;
  enable) [ "\$2" = "gui/$uid/$label" ] ;;
  bootstrap)
    [ "\$2" = "gui/$uid" ] || exit 125
    job="\$(python3 -I -c '
import plistlib, sys
with open(sys.argv[1], "rb") as f: p = plistlib.load(f)
print(p["Label"]); print("\n".join(p["ProgramArguments"]))' "\$3")" || exit 5
    name="\${job%%\$'\n'*}"
    [ ! -f "\$state/loaded-\$name" ] || exit 5
    touch "\$state/loaded-\$name"
    printf '%s\n' "\${job#*\$'\n'}" > "\$state/job-args"
    start ;;
  bootout)
    [ -f "\$state/loaded-\${2##*/}" ] || exit 3
    rm -f "\$state/loaded-\${2##*/}" "\$state/running" ;;
  kickstart)
    [ "\$2" = -k ] || exit 2
    [ -f "\$state/loaded-\${3##*/}" ] || exit 113
    if [ -f "\$state/kickstart-fails" ]; then rm "\$state/kickstart-fails"; exit 1; fi
    start ;;
  *) exit 64 ;;
esac
EOF
# The sqlite3 command line every Mac ships, as far as the installer uses it.
cat > "$root/bin/sqlite3" <<'EOF'
#!/usr/bin/env python3
import sqlite3, sys
args = sys.argv[1:]
rest = []
while args:
    arg = args.pop(0)
    if arg == "-cmd":
        args.pop(0)
    elif arg.startswith("-"):
        continue
    else:
        rest.append(arg)
db, sql = rest
connection = sqlite3.connect("file:" + db + "?mode=ro", uri=True)
for row in connection.execute(sql):
    print("|".join("" if value is None else str(value) for value in row))
EOF
chmod 0755 "$root/bin/"*
export PATH="$root/bin:$PATH"
export AGENT_RELAY_RELEASES_API=https://api.test/releases
export AGENT_RELAY_DOWNLOAD_BASE=https://releases.test
export AGENT_RELAY_HEALTH_TIMEOUT=4

current() { basename "$(readlink "$prefix/current")"; }
starts() { grep -cE '^(bootstrap|kickstart)' "$root/state/launchctl.log" 2>/dev/null || true; }
kept() { (find "$prefix/versions" -mindepth 1 -maxdepth 1 -printf "%f\n" | sort -t. -k1,1n -k2,2n -k3,3n | tr '\n' ' '); }
no_secret() { ! grep -q secret-token "$root/state/out" || fail "$1 printed the account secret"; }

# The core's data: settings, sign-in, agents. Never touched, not even by --uninstall.
data="$HOME/Library/Application Support/com.agentrelay.desktop"
mkdir -p "$data" && echo '{"cloudSession":"kept"}' > "$data/injectable-agents.json"
data_sum="$(sha256sum "$data/injectable-agents.json")"

release 1.0.0; ARCHIVE_EXTRA=none release 1.1.0; release 1.2.0; release 1.3.0 broken
release 1.4.0; release 1.5.0 unsigned; ARCHIVE_EXTRA=stray release 1.6.0
release 2.0.0; release 9.0.0; release 10.0.0; release 11.0.0
QUEUE_FORMATS=0,1 release 12.0.0; QUEUE_FORMATS=0,1,2 release 12.1.0
echo 1.0.0 > "$root/state/latest"

# --- refusals, before anything is installed -----------------------------------

if FAKE_UNAME_S=Linux bash "$INSTALL" > "$root/state/out" 2>&1; then fail "installed on Linux"; fi
grep -q "installs the macOS core" "$root/state/out" || fail "the non-Darwin refusal does not say why"
pass "refuses to run anywhere but macOS"

app="$HOME/Applications/Agent Relay.app/Contents/Library/LaunchAgents"
mkdir -p "$app" && touch "$app/com.agentrelay.desktop.core.plist"
if bash "$INSTALL" > "$root/state/out" 2>&1; then fail "installed beside the app"; fi
grep -q "Agent Relay.app is installed" "$root/state/out" || { cat "$root/state/out"; fail "the app refusal does not say why"; }
rm -rf "$HOME/Applications"
touch "$root/state/loaded-com.agentrelay.desktop.core.local"
if bash "$INSTALL" > "$root/state/out" 2>&1; then fail "installed beside the app's loaded core"; fi
grep -q "com.agentrelay.desktop.core.local) is loaded" "$root/state/out" || fail "the loaded-app refusal does not say why"
rm "$root/state/loaded-com.agentrelay.desktop.core.local"
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "a refused install left files behind"
pass "refuses while the app is installed or its core is loaded"

touch "$root/state/no-gui"
if bash "$INSTALL" > "$root/state/out" 2>&1; then fail "installed without a login session"; fi
grep -q "no login session (gui/$uid)" "$root/state/out" || fail "the login session refusal does not say why"
rm "$root/state/no-gui"
if bash "$INSTALL" --update > "$root/state/out" 2>&1; then fail "--update installed from nothing"; fi
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "a refused install left files behind"
pass "refuses without a login session, and --update with nothing installed"

FAKE_UNAME_M=x86_64 bash "$INSTALL" --dry-run > "$root/state/out" 2>&1 || true
grep -q "release: 1.0.0 (x64)" "$root/state/out" || fail "an Intel Mac did not pick x64"
FAKE_UNAME_M=x86_64 FAKE_TRANSLATED=1 bash "$INSTALL" --dry-run > "$root/state/out" 2>&1 || true
grep -q "release: 1.0.0 (arm64)" "$root/state/out" || fail "a shell under Rosetta did not pick arm64"
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "--dry-run left files behind"
pass "picks x64 on Intel and arm64 under Rosetta"

# --- install, rerun, update ----------------------------------------------------------

bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install failed"; }
[ "$(current)" = 1.0.0 ] || fail "first install points at $(current)"
[ -f "$root/state/loaded-$label" ] || fail "the LaunchAgent was not loaded"
grep -qx "bootstrap gui/$uid $plist" "$root/state/launchctl.log" || fail "the LaunchAgent was not bootstrapped into gui/$uid"
[ "$(cat "$root/state/running")" = 1.0.0 ] || fail "1.0.0 is not running"
python3 -I - "$plist" "$prefix" "$HOME" <<'PY' || fail "the LaunchAgent is not the one expected"
import plistlib, sys
path, prefix, home = sys.argv[1:]
with open(path, "rb") as f:
    p = plistlib.load(f)
expected = {
    "Label": "com.agentrelay.desktop.core.headless",
    "ProgramArguments": [prefix + "/current/agent-relay-probe", "relay", "serve", "--headless"],
    "RunAtLoad": True,
    "KeepAlive": {"SuccessfulExit": False},
    "ThrottleInterval": 10,
    "ProcessType": "Background",
    "StandardOutPath": home + "/Library/Logs/Agent Relay/headless.stdout.log",
    "StandardErrorPath": home + "/Library/Logs/Agent Relay/headless.stderr.log",
}
if p != expected:
    sys.exit("got %r" % p)
PY
[ -d "$HOME/Library/Logs/Agent Relay" ] || fail "the log directory was not made"
grep -q 'leaf\[subject.OU\] = "QUJ7SA6X8X"' "$root/state/codesign.log" || fail "the signature was not checked"
grep -q "signature verified" "$root/state/out" || fail "the install does not say the signature was checked"
grep -Fq "$prefix/current/agent-relay-probe setup" "$root/state/out" || fail "the sign-in step is missing"
grep -q -- "--status" "$root/state/out" || fail "the status step is missing"
grep -q "automatic login" "$root/state/out" || fail "the automatic login note is missing"
grep -q -- "--uninstall" "$root/state/out" || fail "the uninstall-before-the-app note is missing"
no_secret "the first install"
pass "first install loads com.agentrelay.desktop.core.headless running 1.0.0"

before="$(starts)"
bash "$INSTALL" > "$root/state/out" 2>&1 || fail "rerun failed"
grep -q "already up to date" "$root/state/out" || fail "rerun was not a no-op"
[ "$(starts)" = "$before" ] || fail "rerun restarted the core"
pass "rerun leaves a healthy install alone"

echo 1.1.0 > "$root/state/latest"
: > "$root/state/launchctl.log"
bash "$INSTALL" --update > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "update failed"; }
[ "$(current)" = 1.1.0 ] || fail "update points at $(current)"
[ -d "$prefix/versions/1.0.0" ] || fail "previous version removed"
grep -qx "kickstart -k gui/$uid/$label" "$root/state/launchctl.log" || fail "update did not kickstart the core"
! grep -q '^bootstrap' "$root/state/launchctl.log" || fail "an unchanged LaunchAgent was loaded again"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running"
[ ! -e "$prefix/versions/1.1.0/LICENSE" ] || fail "an archive without a license grew one"
[ "$(cat "$prefix/versions/1.0.0/LICENSE")" = License ] || fail "the LICENSE was not installed beside the core"
pass "--update to 1.1.0 restarts with kickstart -k and keeps 1.0.0"

# A LaunchAgent that differs (an older script's) is unloaded and loaded again.
echo '<plist version="1.0"><dict/></plist>' > "$plist"
: > "$root/state/launchctl.log"
bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "rewriting the LaunchAgent failed"; }
grep -qx "bootout gui/$uid/$label" "$root/state/launchctl.log" || fail "a changed LaunchAgent was not unloaded"
grep -qx "bootstrap gui/$uid $plist" "$root/state/launchctl.log" || fail "a changed LaunchAgent was not loaded again"
grep -q '<string>com.agentrelay.desktop.core.headless</string>' "$plist" || fail "the LaunchAgent was not rewritten"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after the LaunchAgent was rewritten"
pass "a changed LaunchAgent is rewritten and loaded again"

# --- downloads that are refused -------------------------------------------------

echo "0000000000000000000000000000000000000000000000000000000000000000  x" \
  > "$root/releases/v1.2.0/AgentRelay-1.2.0-macOS-arm64-probe.tar.gz.sha256"
if bash "$INSTALL" --version 1.2.0 >/dev/null 2>&1; then fail "a bad checksum was accepted"; fi
[ "$(current)" = 1.1.0 ] || fail "a bad checksum changed current"
[ ! -d "$prefix/versions/1.2.0" ] || fail "a bad download was unpacked"
pass "a checksum mismatch changes nothing"

if bash "$INSTALL" --version 1.5.0 > "$root/state/out" 2>&1; then fail "an unsigned core was accepted"; fi
grep -q "not signed with Agent Relay's Developer ID" "$root/state/out" || fail "the signature refusal does not say why"
[ ! -e "$root/state/ran-unsigned" ] || fail "the unsigned core was run"
[ "$(current)" = 1.1.0 ] || fail "an unsigned core changed current"
[ ! -d "$prefix/versions/1.5.0" ] || fail "an unsigned core was installed"
pass "a core without the Developer ID signature is refused before it runs"

if bash "$INSTALL" --version 1.6.0 >/dev/null 2>&1; then fail "an archive with a stray file was accepted"; fi
{ [ "$(current)" = 1.1.0 ] && [ ! -d "$prefix/versions/1.6.0" ]; } || fail "a stray archive changed something"
pass "an archive with a stray file changes nothing"

# --- rollbacks ------------------------------------------------------------------

if bash "$INSTALL" --version 1.3.0 > "$root/state/out" 2>&1; then fail "a core that never answers was accepted"; fi
grep -q "update to 1.3.0 failed and was rolled back to 1.1.0" "$root/state/out" || { cat "$root/state/out"; fail "no rollback message"; }
[ "$(current)" = 1.1.0 ] || fail "the failed update was not rolled back ($(current))"
[ ! -d "$prefix/versions/1.3.0" ] || fail "the failed version was kept"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after rollback"
pass "a new core that never answers is rolled back"

touch "$root/state/kickstart-fails"
if bash "$INSTALL" --version 1.4.0 >/dev/null 2>&1; then fail "a refused restart was accepted"; fi
[ "$(current)" = 1.1.0 ] || fail "a refused restart was not rolled back ($(current))"
[ ! -d "$prefix/versions/1.4.0" ] || fail "the version launchd refused was kept"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running after a refused restart"
pass "a restart launchd refuses is rolled back"

bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback failed"; }
[ "$(current)" = 1.0.0 ] || fail "rollback picked $(current)"
[ "$(cat "$root/state/running")" = 1.0.0 ] || fail "1.0.0 is not running after --rollback"
pass "--rollback switches to the earlier version"

before="$(starts)"
bash "$INSTALL" --dry-run --version 1.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--dry-run failed"; }
if [ "$(current)" != 1.0.0 ] || [ "$(starts)" != "$before" ]; then fail "--dry-run changed something"; fi
grep -q "would run: launchctl kickstart -k gui/$uid/$label" "$root/state/out" || fail "--dry-run does not say it would restart"
pass "--dry-run changes nothing"

# --- status ---------------------------------------------------------------------

bash "$INSTALL" --status > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--status of a running core failed"; }
grep -q "installed: 1.0.0" "$root/state/out" || fail "--status does not name the version"
grep -q "is loaded in gui/$uid, pid 4242" "$root/state/out" || fail "--status does not say the job is loaded"
grep -q "core: answering, version 1.0.0" "$root/state/out" || fail "--status does not say the core answers"
no_secret "--status"
pass "--status reports the version, the job and the answering core"

# --- pruning, in numeric version order ------------------------------------------

for version in 2.0.0 9.0.0 10.0.0 11.0.0; do
  bash "$INSTALL" --version "$version" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of $version failed"; }
done
[ "$(kept)" = "9.0.0 10.0.0 11.0.0 " ] || fail "kept $(kept) rather than the newest three"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || fail "--rollback from 11.0.0 failed"
[ "$(current)" = 10.0.0 ] || fail "rollback from 11.0.0 picked $(current)"
pass "keeps the newest three versions, ordered as numbers"

# --- local session history ------------------------------------------------------

store="$HOME/.agentworkforce/probe/store"
mkdir -p "$store"
python3 -I - "$store/delivery-queue.db" "$store/history.db" <<'PY'
import json, sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute('CREATE TABLE _relay_delivery_sidecar(singleton PRIMARY KEY, phase, manifest)')
c.execute('INSERT INTO _relay_delivery_sidecar VALUES(1, ?, ?)', ('ready', json.dumps({'version': 2})))
c.commit()
h = sqlite3.connect(sys.argv[2])
h.execute('CREATE TABLE sessions(id)')
h.commit()
PY
if bash "$INSTALL" --version 12.0.0 > "$root/state/out" 2>&1; then fail "a core that cannot read the history was installed"; fi
grep -q "cannot read the upgraded local history" "$root/state/out" || { cat "$root/state/out"; fail "the history refusal does not say why"; }
{ [ "$(current)" = 10.0.0 ] && [ ! -d "$prefix/versions/12.0.0" ]; } || fail "the refused core changed the install"
pass "a core that cannot read the upgraded history is refused"

perl -MFcntl=:flock -e 'open(my $h, ">>", $ARGV[0]) or die; flock($h, LOCK_EX) or die; open(my $r, ">", $ARGV[1]); close($r); sleep 60' \
  "$store/delivery-queue.migration-lock" "$root/state/held" &
holder=$!
for _ in $(seq 50); do [ -e "$root/state/held" ] && break; sleep 0.1; done
if bash "$INSTALL" --version 12.1.0 > "$root/state/out" 2>&1; then fail "installed while the history was upgrading"; fi
grep -q "local session history is upgrading" "$root/state/out" || { cat "$root/state/out"; fail "the lock refusal does not say why"; }
kill "$holder"; wait "$holder" 2>/dev/null || true; holder=""
[ "$(current)" = 10.0.0 ] || fail "a held migration lock changed the install"
pass "an update waits for a history upgrade in progress"

bash "$INSTALL" --version 12.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a compatible core was refused"; }
[ "$(current)" = 12.1.0 ] || fail "the compatible core points at $(current)"
if bash "$INSTALL" --rollback > "$root/state/out" 2>&1; then fail "rolled back to a core that cannot read the history"; fi
grep -q "rollback refused before changing the installation" "$root/state/out" || fail "the unsafe rollback refusal does not say why"
[ "$(current)" = 12.1.0 ] || fail "the refused rollback changed current"
require_disposable_home
rm -rf "$HOME/.agentworkforce"
pass "a compatible core is installed, and an unsafe rollback refused"

# --- uninstall ------------------------------------------------------------------

bash "$INSTALL" --uninstall --dry-run > "$root/state/out" 2>&1 || fail "--uninstall --dry-run failed"
{ [ -f "$plist" ] && [ -e "$prefix/current" ] && [ -f "$root/state/loaded-$label" ]; } || fail "--uninstall --dry-run changed something"
bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--uninstall failed"; }
grep -qx "bootout gui/$uid/$label" "$root/state/launchctl.log" || fail "--uninstall did not unload the job"
[ ! -e "$root/state/loaded-$label" ] || fail "the job is still loaded"
[ ! -e "$plist" ] || fail "the LaunchAgent was left behind"
[ ! -e "$prefix" ] || fail "the install tree was left behind"
grep -Fq "kept settings, sign-in and agent registrations in: $data" "$root/state/out" || fail "--uninstall does not say the data is kept"
[ "$(sha256sum "$data/injectable-agents.json")" = "$data_sum" ] || fail "the core's data changed"
if bash "$INSTALL" --status > "$root/state/out" 2>&1; then fail "--status after --uninstall reported success"; fi
grep -q "not installed" "$root/state/out" || fail "--status after --uninstall does not say it is not installed"
pass "--uninstall unloads and removes the core and keeps its data"
echo "all macOS install tests passed"

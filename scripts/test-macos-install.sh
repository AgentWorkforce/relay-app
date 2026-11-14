#!/usr/bin/env bash
#
# test-macos-install.sh -- exercise macos/install.sh end to end on Linux (or a
# Mac) with a fake release server and fake uname, sysctl, curl, shasum,
# codesign, launchctl and sqlite3 on PATH, in a throwaway HOME: refusals (not
# macOS, the app installed or loaded, no login session, --update with nothing
# installed), a first install and the LaunchAgent it writes, an idempotent
# rerun, an update, a bad checksum, an unsigned core (never run), a stray
# archive, a new version that never answers or that launchd refuses to
# restart (rolled back), --rollback (only ever to an older version), a
# LaunchAgent that cannot be written (nothing switched), --dry-run, the
# installer lock (held through a whole install, and taken again when the lock
# file is removed while it is being locked), --status, another product's copy
# at the same version (replaced, and put back when it never answers), pruning
# to three versions in numeric order, the local history checks, --uninstall, which
# keeps the core's data directory and waits for the core and its session
# collector to exit (bounded by one timeout in all, so a collector that never
# stops does not hold it up, and never for another install's collector), so
# an install right after it works, the script piped into bash as the release
# asset's one-line install is (no command it runs may read the script from
# standard input), --help and an unknown option, and a cloud profile's copy,
# which refuses to install. Nothing here talks to a real launchd.
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
real_perl="$(command -v perl)"
root="$(mktemp -d)"
holder=""
# holder: the pids of background processes a test leaves running.
trap 'for pid in $holder; do kill "$pid" 2>/dev/null || true; done; rm -rf "$root"' EXIT
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

# Piped into bash (curl ... | bash), the installer is bash's standard input.
# With STDIN_CHECK set, the fake core, codesign, curl and launchctl each read
# a byte of theirs and record it if there was one: a command that reads the
# script eats the rest of the installer.
stdin_check="if [ -n \"\${STDIN_CHECK:-}\" ] && [ -n \"\$(head -c 1)\" ]; then echo \"\$0 read standard input\" >> \"$root/state/stdin-violations\"; fi"
# The script as curl hands it to bash: on a pipe, not a seekable file
# (`bash -s < file` would let a stray read skip the script, not starve it).
piped() { cat "$1"; }

# A fake core: reports its version, and answers status.get with it (and an
# account secret the installer must never print) only while the fake launchd
# says it is the one running. KIND: ok, broken (never answers) or unsigned
# (fails codesign, and records being run at all). RELEASE_ASSET_PREFIX names
# the product whose asset it is (AgentRelay by default).
release() { # version [kind]
  local version="$1" kind="${2:-ok}" dir="$root/releases/v$1" tree extra="${ARCHIVE_EXTRA:-license}"
  local formats="${QUEUE_FORMATS:-0,1}" product="${RELEASE_ASSET_PREFIX:-AgentRelay}"
  tree="$(mktemp -d)"
  mkdir -p "$dir" "$tree/agent_relay/helpers"
  cat > "$tree/agent_relay/helpers/agent-relay-probe" <<EOF
#!/usr/bin/env bash
# kind: $kind
# asset: $product
$stdin_check
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
  local asset="$product-$version-macOS-arm64-probe.tar.gz"
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
# The designated requirement, written out here rather than read from the
# installer: dropping any constraint (the Apple anchor, the Developer ID
# intermediate, the Developer ID Application leaf or the team) fails the suite.
requirement='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "QUJ7SA6X8X"'
printf '%s\n' "-R=$requirement" > "$root/state/requirement"
cat > "$root/bin/codesign" <<EOF
#!/usr/bin/env bash
$stdin_check
# Passes a core unless it is the unsigned kind; the requirement must be the
# full Developer ID requirement, exactly.
echo "\$*" >> "$root/state/codesign.log"
[ "\$1 \$2" = "--verify --strict" ] || exit 2
[ "\$3" = "\$(cat "$root/state/requirement")" ] || { echo "codesign: unexpected requirement: \$3" >&2; exit 3; }
grep -q '^# kind: unsigned' "\$4" && { echo "\$4: invalid signature" >&2; exit 1; }
exit 0
EOF
cat > "$root/bin/curl" <<EOF
#!/usr/bin/env bash
$stdin_check
out=""; url=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) out="\$2"; shift 2 ;; --proto|--retry) shift 2 ;; -*) shift ;; *) url="\$1"; shift ;; esac
done
echo "\$url" >> "$root/state/curl.log"
# While state/curl-pause exists, a core download waits (up to 30 s), so a
# test can hold a real installer in the middle of an install.
case "\$url" in
  *.tar.gz)
    if [ -e "$root/state/curl-pause" ]; then
      touch "$root/state/curl-paused"
      for _ in \$(seq 300); do [ -e "$root/state/curl-pause" ] || break; sleep 0.1; done
    fi ;;
esac
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
$stdin_check
echo "\$*" >> "$root/state/launchctl.log"
state="$root/state"
if [ -n "\${CURRENT_GUARD:-}" ] && [ ! -L "\$CURRENT_GUARD" ]; then
  echo "current absent at: launchctl \$*" >> "\$state/current-violations"
fi
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
      gui/$uid/*) [ -f "\$state/loaded-\${2##*/}" ] || exit 113; printf '\tstate = running\n'; [ ! -f "\$state/core-pid" ] || printf '\tpid = %s\n' "\$(cat "\$state/core-pid")" ;;
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
    [ ! -f "\$state/bootout-fails" ] || exit 5
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
# plutil -lint, failing while state/plutil-fails exists.
cat > "$root/bin/plutil" <<EOF
#!/usr/bin/env bash
[ "\$1" = -lint ] || exit 2
[ ! -e "$root/state/plutil-fails" ] || { echo "\$2: invalid" >&2; exit 1; }
echo "\$2: OK"
EOF
# perl, but while state/swap-lock exists, a perl given the installer lock's
# path first removes that file, as --uninstall does while holding it: the
# installer has opened it and has not yet locked it. "once" in swap-lock
# removes it once; "always" on every attempt.
cat > "$root/bin/perl" <<EOF
#!/usr/bin/env bash
if [ -e "$root/state/swap-lock" ]; then
  for arg in "\$@"; do
    case "\$arg" in
      */.install-lock)
        "$(command -v rm)" -f "\$arg"
        [ "\$(cat "$root/state/swap-lock")" = always ] || "$(command -v rm)" -f "$root/state/swap-lock"
        echo swapped >> "$root/state/swaps" ;;
    esac
  done
fi
exec "$real_perl" "\$@"
EOF
# While CURRENT_GUARD names the \`current\` link, rm, ln, mv and unlink record
# any call that names it, and any call made while it is absent: the installer
# may change \`current\` only by rename(2) of current.new over it, so the link
# exists at every step of an update. Unlinking it and linking it again fails.
for tool in rm ln mv unlink; do
  cat > "$root/bin/$tool" <<EOF
#!/usr/bin/env bash
if [ -n "\${CURRENT_GUARD:-}" ]; then
  for arg in "\$@"; do
    case "\$arg" in "\$CURRENT_GUARD"|"\$CURRENT_GUARD/") echo "$tool names current: $tool \$*" >> "$root/state/current-violations" ;; esac
  done
  [ -L "\$CURRENT_GUARD" ] || echo "current absent at: $tool \$*" >> "$root/state/current-violations"
fi
exec "$(command -v "$tool")" "\$@"
EOF
done
chmod 0755 "$root/bin/"*
export PATH="$root/bin:$PATH"
export AGENT_RELAY_RELEASES_API=https://api.test/releases
export AGENT_RELAY_DOWNLOAD_BASE=https://releases.test
export AGENT_RELAY_HEALTH_TIMEOUT=4

current() { basename "$(readlink "$prefix/current")"; }
no_current_gap() { # what was checked
  [ ! -s "$root/state/current-violations" ] || { cat "$root/state/current-violations" >&2; fail "$1 removed or replaced current other than by rename"; }
}
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

bash "$INSTALL" --help > "$root/state/out" 2>&1 || fail "--help failed"
grep -q "^Usage: install-macos.sh" "$root/state/out" || fail "--help prints no usage"
piped "$INSTALL" | bash -s -- -h > "$root/state/out" 2>&1 || fail "-h through a pipe failed"
grep -q "^Usage: install-macos.sh" "$root/state/out" || fail "-h through a pipe prints no usage"
rc=0; bash "$INSTALL" --frobnicate > "$root/state/out" 2> "$root/state/err" || rc=$?
[ "$rc" = 2 ] || fail "an unknown option exited $rc, not 2"
grep -q "^Usage: install-macos.sh" "$root/state/err" || fail "an unknown option prints no usage on stderr"
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "--help or an unknown option left files behind"
pass "--help and -h print usage and exit 0; an unknown option exits 2 with usage"

# A cloud profile's copy: the Stage step sets the seven assignments.
cloud_copy="$root/install-cloud.sh"
sed -e "s/^RELEASE_FEED=.*/RELEASE_FEED=cloud/" -e "s|^SITE_URL=.*|SITE_URL=https://relay.acme.test|" \
  -e "s/^PROFILE_ID=.*/PROFILE_ID=x7k2q/" "$INSTALL" > "$cloud_copy"
[ "$(grep -c '^RELEASE_FEED=cloud$' "$cloud_copy")" = 1 ] || fail "the cloud copy does not set RELEASE_FEED once"
: > "$root/state/curl.log"
if piped "$cloud_copy" | bash -s > "$root/state/out" 2>&1; then fail "a cloud profile's copy installed"; fi
grep -q "the macOS installer does not support the cloud feed yet" "$root/state/out" || { cat "$root/state/out"; fail "the cloud refusal does not say why"; }
[ ! -s "$root/state/curl.log" ] || fail "a cloud profile's copy downloaded something"
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "a cloud profile's copy left files behind"
bash "$cloud_copy" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a cloud profile's copy could not --uninstall"; }
pass "a cloud profile's copy refuses to install, and can still --uninstall"

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

piped "$INSTALL" | STDIN_CHECK=1 bash -s -- --dry-run > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a piped --dry-run failed"; }
grep -q "release: 1.0.0 (arm64)" "$root/state/out" || { cat "$root/state/out"; fail "a piped --dry-run did not resolve the release"; }
grep -q "would run: launchctl bootstrap gui/$uid $plist" "$root/state/out" || { cat "$root/state/out"; fail "a piped --dry-run did not reach the end"; }
[ ! -s "$root/state/stdin-violations" ] || { cat "$root/state/stdin-violations" >&2; fail "a piped --dry-run let a command read the script"; }
{ [ ! -e "$prefix" ] && [ ! -e "$plist" ]; } || fail "a piped --dry-run left files behind"
pass "bash -s -- --dry-run through a pipe runs to the end and changes nothing"

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
CURRENT_GUARD="$prefix/current" bash "$INSTALL" --update > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "update failed"; }
no_current_gap "the update"
[ "$(current)" = 1.1.0 ] || fail "update points at $(current)"
[ -d "$prefix/versions/1.0.0" ] || fail "previous version removed"
grep -qx "kickstart -k gui/$uid/$label" "$root/state/launchctl.log" || fail "update did not kickstart the core"
! grep -q '^bootstrap' "$root/state/launchctl.log" || fail "an unchanged LaunchAgent was loaded again"
[ "$(cat "$root/state/running")" = 1.1.0 ] || fail "1.1.0 is not running"
[ ! -e "$prefix/versions/1.1.0/LICENSE" ] || fail "an archive without a license grew one"
[ "$(cat "$prefix/versions/1.0.0/LICENSE")" = License ] || fail "the LICENSE was not installed beside the core"
pass "--update to 1.1.0 restarts with kickstart -k and keeps 1.0.0, never without current"

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

CURRENT_GUARD="$prefix/current" bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback failed"; }
no_current_gap "--rollback"
[ "$(current)" = 1.0.0 ] || fail "rollback picked $(current)"
[ "$(cat "$root/state/running")" = 1.0.0 ] || fail "1.0.0 is not running after --rollback"
pass "--rollback switches to the earlier version"

# --rollback only ever goes back: never to a version it rolled back from.
release 1.2.0 # a good .sha256 again; the checksum test above broke it
for version in 1.1.0 1.2.0; do
  bash "$INSTALL" --version "$version" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of $version failed"; }
done
[ "$(kept)" = "1.0.0 1.1.0 1.2.0 " ] || fail "kept $(kept) rather than 1.0.0 1.1.0 1.2.0"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback from 1.2.0 failed"; }
{ [ "$(current)" = 1.1.0 ] && [ "$(cat "$root/state/running")" = 1.1.0 ]; } || fail "rollback from 1.2.0 picked $(current)"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback from 1.1.0 failed"; }
{ [ "$(current)" = 1.0.0 ] && [ "$(cat "$root/state/running")" = 1.0.0 ]; } || fail "rollback from 1.1.0 picked $(current), not 1.0.0"
before="$(starts)"
if CURRENT_GUARD="$prefix/current" bash "$INSTALL" --rollback > "$root/state/out" 2>&1; then fail "--rollback from the oldest version succeeded ($(current))"; fi
grep -q "no earlier version to roll back to" "$root/state/out" || { cat "$root/state/out"; fail "the refused rollback does not say why"; }
no_current_gap "a refused --rollback"
{ [ "$(current)" = 1.0.0 ] && [ "$(starts)" = "$before" ] && [ "$(cat "$root/state/running")" = 1.0.0 ]; } \
  || fail "a refused --rollback changed something"
pass "--rollback goes from 1.2.0 to 1.1.0 to 1.0.0, then refuses rather than going forward"

# Prereleases of one release go by SemVer precedence, not as equals: the
# comparator itself (beta.2 is older than beta.11, not lexically; build
# metadata is ignored), then --rollback from a later beta and from the release.
eval "$(grep '^valid_version() ' "$INSTALL")"
for fn in installed_versions version_older rollback_target; do
  eval "$(sed -n "/^$fn() {/,/^}/p" "$INSTALL")"
done
# shellcheck disable=SC2034 # read by the functions above
PREFIX="$prefix"
older() { version_older "$1" "$2"; }
older 4.0.0-beta.1 4.0.0-beta.2 || fail "4.0.0-beta.1 is not older than 4.0.0-beta.2"
! older 4.0.0-beta.2 4.0.0-beta.1 || fail "4.0.0-beta.2 is older than 4.0.0-beta.1"
older 4.0.0-beta.2 4.0.0-beta.11 || fail "4.0.0-beta.2 is not older than 4.0.0-beta.11"
! older 4.0.0-beta.11 4.0.0-beta.2 || fail "4.0.0-beta.11 is older than 4.0.0-beta.2"
older 4.0.0-beta.2 4.0.0 || fail "4.0.0-beta.2 is not older than 4.0.0"
! older 4.0.0 4.0.0-beta.2 || fail "4.0.0 is older than 4.0.0-beta.2"
! older 4.0.0-beta.1+build.5 4.0.0-beta.1+build.9 || fail "build metadata made a version older"
! older 4.0.0+a 4.0.0+b || fail "build metadata made a release older"
older 4.0.0-beta.1+z 4.0.0-beta.2+a || fail "build metadata changed the order of two betas"
older 3.9.9 4.0.0-beta.1 || fail "3.9.9 is not older than 4.0.0-beta.1"
pass "version_older orders prereleases of one release by SemVer precedence"

release 4.0.0-beta.1; release 4.0.0-beta.2; release 4.0.0
for version in 4.0.0-beta.1 4.0.0-beta.2; do
  bash "$INSTALL" --version "$version" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of $version failed"; }
done
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback from 4.0.0-beta.2 failed"; }
{ [ "$(current)" = 4.0.0-beta.1 ] && [ "$(cat "$root/state/running")" = 4.0.0-beta.1 ]; } \
  || { cat "$root/state/out"; fail "rollback from 4.0.0-beta.2 picked $(current), not 4.0.0-beta.1"; }
bash "$INSTALL" --version 4.0.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "install of 4.0.0 failed"; }
for version in 4.0.0-beta.1 4.0.0-beta.2; do
  [ -d "$prefix/versions/$version" ] || fail "$version is not installed beside 4.0.0"
done
# shellcheck disable=SC2034 # read by rollback_target
current_version=4.0.0
[ "$(rollback_target)" = 4.0.0-beta.2 ] || fail "rollback from 4.0.0 would pick $(rollback_target), not 4.0.0-beta.2"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback from 4.0.0 failed"; }
{ [ "$(current)" = 4.0.0-beta.2 ] && [ "$(cat "$root/state/running")" = 4.0.0-beta.2 ]; } \
  || fail "rollback from 4.0.0 picked $(current), not 4.0.0-beta.2"
pass "--rollback orders betas of one release: beta.2 to beta.1, and the release to beta.2"
# Back to the state the tests below start from: 1.0.0 current, 1.1.0 and 1.2.0 kept.
bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--uninstall after the beta tests failed"; }
for version in 1.0.0 1.1.0 1.2.0; do
  release "$version"
  bash "$INSTALL" --version "$version" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "reinstall of $version failed"; }
done
for _ in 1 2; do
  bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback back to 1.0.0 failed"; }
done
[ "$(kept)" = "1.0.0 1.1.0 1.2.0 " ] || fail "kept $(kept) rather than 1.0.0 1.1.0 1.2.0"

# A LaunchAgent that cannot be written: nothing is switched or restarted.
echo '<plist version="1.0"><dict/></plist>' > "$plist"
touch "$root/state/plutil-fails"
before="$(starts)"
if bash "$INSTALL" --version 1.2.0 > "$root/state/out" 2>&1; then fail "an update succeeded without its LaunchAgent"; fi
grep -q "the LaunchAgent written is not a valid property list" "$root/state/out" || { cat "$root/state/out"; fail "the plist failure does not say why"; }
{ [ "$(current)" = 1.0.0 ] && [ "$(cat "$root/state/running")" = 1.0.0 ] && [ "$(starts)" = "$before" ]; } \
  || fail "a failed LaunchAgent write during an update switched to $(current) or restarted the core"
{ [ ! -e "$plist.new" ] && grep -q '<dict/>' "$plist" && [ ! -e "$prefix/versions/1.2.0.replaced" ]; } \
  || fail "a failed LaunchAgent write changed the LaunchAgent or the versions"
rm "$root/state/plutil-fails"
bash "$INSTALL" --version 1.2.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the update after the plist failure failed"; }
[ "$(current)" = 1.2.0 ] || fail "the update after the plist failure points at $(current)"
echo '<plist version="1.0"><dict/></plist>' > "$plist"
touch "$root/state/plutil-fails"
before="$(starts)"
if bash "$INSTALL" --rollback > "$root/state/out" 2>&1; then fail "a rollback succeeded without its LaunchAgent"; fi
grep -q "the LaunchAgent written is not a valid property list" "$root/state/out" || { cat "$root/state/out"; fail "the plist failure on rollback does not say why"; }
{ [ "$(current)" = 1.2.0 ] && [ "$(cat "$root/state/running")" = 1.2.0 ] && [ "$(starts)" = "$before" ]; } \
  || fail "a failed LaunchAgent write during a rollback switched to $(current) or restarted the core"
rm "$root/state/plutil-fails"
for _ in 1 2; do
  bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback after the plist failure failed"; }
done
{ [ "$(current)" = 1.0.0 ] && grep -q '<string>com.agentrelay.desktop.core.headless</string>' "$plist"; } \
  || fail "the rollbacks after the plist failure left $(current)"
pass "a LaunchAgent that cannot be written leaves current and the running core alone, on update and rollback"

before="$(starts)"
bash "$INSTALL" --dry-run --version 1.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--dry-run failed"; }
if [ "$(current)" != 1.0.0 ] || [ "$(starts)" != "$before" ]; then fail "--dry-run changed something"; fi
grep -q "would run: launchctl kickstart -k gui/$uid/$label" "$root/state/out" || fail "--dry-run does not say it would restart"
pass "--dry-run changes nothing"

# --- one installer at a time ----------------------------------------------------

perl -MFcntl=:flock -e 'open(my $h, ">>", $ARGV[0]) or die; flock($h, LOCK_EX) or die; open(my $r, ">", $ARGV[1]); close($r); sleep 60' \
  "$prefix/.install-lock" "$root/state/held" &
holder=$!
for _ in $(seq 50); do [ -e "$root/state/held" ] && break; sleep 0.1; done
before="$(starts)"
for args in "--version 1.1.0" "--rollback" "--uninstall"; do
  # shellcheck disable=SC2086
  if bash "$INSTALL" $args > "$root/state/out" 2>&1; then fail "$args ran beside another installer"; fi
  grep -q "another install, update, rollback or uninstall is running" "$root/state/out" || { cat "$root/state/out"; fail "$args does not say another installer is running"; }
done
kill "$holder"; wait "$holder" 2>/dev/null || true; holder=""; rm -f "$root/state/held"
{ [ "$(current)" = 1.0.0 ] && [ "$(starts)" = "$before" ] && [ -f "$plist" ] && [ -d "$prefix/versions/1.1.0" ]; } \
  || fail "a refused concurrent installer changed something"
pass "install, rollback and uninstall refuse while another installer holds the lock"

# A real installer holds the lock for the whole install: paused in the
# middle of its download, it keeps a second one out.
rm -f "$root/state/curl-paused"
touch "$root/state/curl-pause"
bash "$INSTALL" --version 1.2.0 > "$root/state/out-first" 2>&1 &
holder=$!
for _ in $(seq 100); do [ -e "$root/state/curl-paused" ] && break; sleep 0.1; done
[ -e "$root/state/curl-paused" ] || { rm -f "$root/state/curl-pause"; cat "$root/state/out-first"; fail "the first installer never reached its download"; }
if bash "$INSTALL" --version 1.1.0 > "$root/state/out" 2>&1; then fail "a second installer ran beside a paused install"; fi
grep -q "another install, update, rollback or uninstall is running" "$root/state/out" || { cat "$root/state/out"; fail "the second installer does not say another is running"; }
rm -f "$root/state/curl-pause" "$root/state/curl-paused"
wait "$holder" || { holder=""; cat "$root/state/out-first"; fail "the paused install failed"; }
holder=""
[ "$(current)" = 1.2.0 ] || fail "the paused install points at $(current)"
for _ in 1 2; do
  bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--rollback after the paused install failed"; }
done
[ "$(current)" = 1.0.0 ] || fail "the rollbacks after the paused install left $(current)"
pass "a running install holds the lock until it finishes"

# The lock file removed (as --uninstall does) after the installer opened it
# and before it locked it: the lock it took is on a file nobody else can open,
# so it opens the path again and locks that.
echo once > "$root/state/swap-lock"
: > "$root/state/swaps"
bash "$INSTALL" --version 1.0.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "an installer whose lock file was removed while locking did not retry"; }
grep -q "already up to date" "$root/state/out" || { cat "$root/state/out"; fail "the retried installer did not finish"; }
{ [ ! -e "$root/state/swap-lock" ] && [ "$(wc -l < "$root/state/swaps")" = 1 ]; } || fail "the lock file was not removed exactly once"
[ -f "$prefix/.install-lock" ] || fail "the retried installer did not lock a new lock file"
echo always > "$root/state/swap-lock"
: > "$root/state/swaps"
if bash "$INSTALL" --version 1.0.0 > "$root/state/out" 2>&1; then fail "an installer ran on a lock file that kept being removed"; fi
grep -q "kept being removed or replaced while it was being locked" "$root/state/out" || { cat "$root/state/out"; fail "the lock retries do not say why they gave up"; }
[ "$(wc -l < "$root/state/swaps")" = 5 ] || fail "the installer tried $(wc -l < "$root/state/swaps") times, not 5"
rm -f "$root/state/swap-lock" "$root/state/swaps"
[ "$(current)" = 1.0.0 ] || fail "the lock retries changed current"
pass "a lock on a removed lock file does not count: the installer locks the new file, and gives up after 5 tries"

# --- status ---------------------------------------------------------------------

echo 4242 > "$root/state/core-pid"
bash "$INSTALL" --status > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--status of a running core failed"; }
rm "$root/state/core-pid"
grep -q "installed: 1.0.0" "$root/state/out" || fail "--status does not name the version"
grep -q "is loaded in gui/$uid, pid 4242" "$root/state/out" || fail "--status does not say the job is loaded"
grep -q "core: answering, version 1.0.0" "$root/state/out" || fail "--status does not say the core answers"
no_secret "--status"
pass "--status reports the version, the job and the answering core"

# --- another product's copy at the same version ---------------------------------

# A site profile's copy: the Stage step sets its own product.
site_copy="$root/install-site.sh"
sed -e "s/^ASSET_PREFIX=.*/ASSET_PREFIX='Desktop-x7k2q'/" -e "s/^PRODUCT_NAME=.*/PRODUCT_NAME='Acme Relay'/" "$INSTALL" > "$site_copy"
[ "$(grep -c "^ASSET_PREFIX='Desktop-x7k2q'$" "$site_copy")" = 1 ] || fail "the site copy does not set ASSET_PREFIX once"
grep -qx "DEFAULT_ASSET_PREFIX='AgentRelay'" "$site_copy" || fail "the site copy changed the default product"
probe_asset() { sed -n 's/^# asset: //p' "$prefix/versions/1.0.0/agent-relay-probe"; }
# An install from before the product record is Agent Relay's.
[ "$(cat "$prefix/versions/1.0.0/product")" = AgentRelay ] || fail "the install did not record its product"
rm "$prefix/versions/1.0.0/product"
bash "$INSTALL" --version 1.0.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the default copy over an unrecorded install failed"; }
grep -q "already up to date" "$root/state/out" || { cat "$root/state/out"; fail "an unrecorded install is not the default product's"; }
bash "$INSTALL" --status > "$root/state/out" 2>&1 || fail "--status failed"
grep -q "product: AgentRelay" "$root/state/out" || fail "--status does not name the product"
# Its new core never answers: Agent Relay's copy of 1.0.0 comes back.
RELEASE_ASSET_PREFIX=Desktop-x7k2q release 1.0.0 broken
if bash "$site_copy" --version 1.0.0 > "$root/state/out" 2>&1; then fail "another product's core that never answers was accepted"; fi
grep -q "replacing the installed 1.0.0 failed and it is running again" "$root/state/out" || { cat "$root/state/out"; fail "the failed replacement does not say so"; }
{ [ "$(probe_asset)" = AgentRelay ] && [ ! -e "$prefix/versions/1.0.0/product" ] && [ ! -e "$prefix/versions/1.0.0.replaced" ]; } \
  || fail "the failed replacement did not put Agent Relay's 1.0.0 back"
{ [ "$(current)" = 1.0.0 ] && [ "$(cat "$root/state/running")" = 1.0.0 ]; } || fail "Agent Relay's 1.0.0 is not running after the failed replacement"
RELEASE_ASSET_PREFIX=Desktop-x7k2q release 1.0.0
: > "$root/state/curl.log"
bash "$site_copy" --version 1.0.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the site copy at the same version failed"; }
! grep -q "already up to date" "$root/state/out" || fail "the site copy took Agent Relay's 1.0.0 as its own"
grep -q "the installed 1.0.0 is another product's; replacing it with Acme Relay's" "$root/state/out" || { cat "$root/state/out"; fail "the site copy does not say it replaces another product"; }
grep -q "/v1.0.0/Desktop-x7k2q-1.0.0-macOS-arm64-probe.tar.gz$" "$root/state/curl.log" || fail "the site copy did not download its own asset"
{ [ "$(cat "$prefix/versions/1.0.0/product")" = Desktop-x7k2q ] && [ "$(probe_asset)" = Desktop-x7k2q ]; } || fail "the site copy did not install and record its product"
{ [ "$(current)" = 1.0.0 ] && [ "$(cat "$root/state/running")" = 1.0.0 ] && [ ! -e "$prefix/versions/1.0.0.replaced" ]; } \
  || fail "the site copy's 1.0.0 is not running cleanly"
bash "$site_copy" --version 1.0.0 > "$root/state/out" 2>&1 || fail "the site copy's rerun failed"
grep -q "already up to date" "$root/state/out" || { cat "$root/state/out"; fail "the site copy's rerun was not a no-op"; }
bash "$INSTALL" --version 1.0.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "the default copy over the site copy failed"; }
grep -q "replacing it with Agent Relay's" "$root/state/out" || { cat "$root/state/out"; fail "the default copy did not replace the site copy"; }
{ [ "$(cat "$prefix/versions/1.0.0/product")" = AgentRelay ] && [ "$(probe_asset)" = AgentRelay ]; } || fail "the default copy did not put Agent Relay's 1.0.0 back"
pass "another product's copy at the same version replaces the installed core, and is put back when it never answers"

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
touch "$root/state/bootout-fails"
if bash "$INSTALL" --uninstall > "$root/state/out" 2>&1; then fail "--uninstall succeeded with the job still loaded"; fi
grep -q "could not unload $label; nothing was removed. Run: launchctl bootout gui/$uid/$label" "$root/state/out" \
  || { cat "$root/state/out"; fail "the failed unload does not say what to run"; }
{ [ -f "$plist" ] && [ -x "$prefix/current/agent-relay-probe" ] && [ -f "$root/state/loaded-$label" ]; } \
  || fail "--uninstall removed files while the job was still loaded"
rm "$root/state/bootout-fails"
# launchd has unloaded the job while its core is still exiting, and the core's
# detached session collector, outside the job, still holds its store's lock:
# it stops a second after its executable is gone. Until both have, an install
# can find the history mid-close, so --uninstall returns only after them.
# A collector runs as the core that started it: the fake one runs perl under
# the core's name.
own_collector() { # store marker perl-code [args...]
  local store="$1" marker="$2" code="$3"; shift 3
  mkdir -p "$store"
  # shellcheck disable=SC2016 # perl code
  ( exec -a "$prefix/current/agent-relay-probe" "$real_perl" -MFcntl=:flock -e '
      my ($lock_path, $held, $code, @rest) = @ARGV;
      open(my $lock, ">>", $lock_path) or die; flock($lock, LOCK_EX) or die;
      open(my $h, ">", $held); close($h);
      eval $code; die $@ if $@;' "$store/collector.lock" "$marker" "$code" "$@" ) &
  holder="$holder $!"
  for _ in $(seq 50); do [ -e "$marker" ] && break; sleep 0.1; done
}
( { sleep 3; touch "$root/state/core-exited"; } > /dev/null 2>&1 & echo $! > "$root/state/core-pid" )
store="$HOME/.agentworkforce/probe/store"
# shellcheck disable=SC2016 # perl code
own_collector "$store" "$root/state/held" '
  my ($current, $stopped) = @rest;
  for (1 .. 300) { last unless -e $current; select(undef, undef, undef, 0.1) }
  sleep 1;
  open(my $s, ">", $stopped); close($s);' "$prefix/current" "$root/state/collector-stopped"
AGENT_RELAY_STOP_TIMEOUT=10 bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--uninstall failed"; }
[ -e "$root/state/core-exited" ] || fail "--uninstall returned while the core was still exiting"
[ -e "$root/state/collector-stopped" ] || fail "--uninstall returned while the session collector still held the history"
! grep -q "still closing its local session history" "$root/state/out" || fail "--uninstall timed out on a collector that stopped"
for pid in $holder; do wait "$pid" 2>/dev/null || true; done; holder=""
rm -f "$root/state/core-pid" "$root/state/core-exited" "$root/state/collector-stopped" "$root/state/held"
grep -qx "bootout gui/$uid/$label" "$root/state/launchctl.log" || fail "--uninstall did not unload the job"
[ ! -e "$root/state/loaded-$label" ] || fail "the job is still loaded"
[ ! -e "$plist" ] || fail "the LaunchAgent was left behind"
[ ! -e "$prefix" ] || fail "the install tree was left behind"
grep -Fq "kept settings, sign-in and agent registrations in: $data" "$root/state/out" || fail "--uninstall does not say the data is kept"
[ "$(sha256sum "$data/injectable-agents.json")" = "$data_sum" ] || fail "the core's data changed"
if bash "$INSTALL" --status > "$root/state/out" 2>&1; then fail "--status after --uninstall reported success"; fi
grep -q "not installed" "$root/state/out" || fail "--status after --uninstall does not say it is not installed"
pass "--uninstall keeps everything while the job will not unload, then removes the core and keeps its data"
pass "--uninstall waits for the core and its session collector to exit"

# A core and a collector of this install that never stop hold --uninstall up
# only for AGENT_RELAY_STOP_TIMEOUT in all, not each; it then says so and
# finishes.
bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "an install right after --uninstall failed"; }
sleep 300 & holder="$holder $!"; echo $! > "$root/state/core-pid"
own_collector "$store" "$root/state/held" 'sleep 300'
start=$SECONDS
AGENT_RELAY_STOP_TIMEOUT=4 bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--uninstall failed beside a collector that never stops"; }
elapsed=$((SECONDS - start))
[ "$elapsed" -lt 8 ] || { cat "$root/state/out"; fail "--uninstall took ${elapsed}s with AGENT_RELAY_STOP_TIMEOUT=4: the core and collector waits are not one timeout"; }
grep -q "the core (pid [0-9]*) has not exited yet" "$root/state/out" || { cat "$root/state/out"; fail "--uninstall does not say the core is still running"; }
grep -q "a session collector of this install (pid [0-9]*) is still closing its local session history" "$root/state/out" \
  || { cat "$root/state/out"; fail "--uninstall does not say a collector is still running"; }
{ [ ! -e "$plist" ] && [ ! -e "$prefix" ]; } || fail "--uninstall beside a collector that never stops left the core behind"
for pid in $holder; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done; holder=""
rm -f "$root/state/held" "$root/state/core-pid"
pass "a core and a collector that never stop hold --uninstall up only for AGENT_RELAY_STOP_TIMEOUT in all"

# The stores are keyed by workspace, not by install: the app's collector, or
# another install's, holds one under the same root. --uninstall does not wait
# for it, and says nothing about it.
bash "$INSTALL" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "an install after --uninstall failed"; }
mkdir -p "$HOME/.agentworkforce/probe/app-store"
perl -MFcntl=:flock -e 'open(my $h, ">>", $ARGV[0]) or die; flock($h, LOCK_EX) or die; open(my $r, ">", $ARGV[1]); close($r); sleep 300' \
  "$HOME/.agentworkforce/probe/app-store/collector.lock" "$root/state/held" &
holder="$holder $!"
for _ in $(seq 50); do [ -e "$root/state/held" ] && break; sleep 0.1; done
start=$SECONDS
AGENT_RELAY_STOP_TIMEOUT=20 bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "--uninstall failed beside another install's collector"; }
elapsed=$((SECONDS - start))
[ "$elapsed" -lt 10 ] || fail "--uninstall waited ${elapsed}s for a collector that is not this install's"
! grep -q "session collector" "$root/state/out" || { cat "$root/state/out"; fail "--uninstall reported a collector that is not this install's"; }
{ [ ! -e "$plist" ] && [ ! -e "$prefix" ]; } || fail "--uninstall beside another install's collector left the core behind"
for pid in $holder; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done; holder=""
rm -f "$root/state/held"
require_disposable_home
rm -rf "$HOME/.agentworkforce"
pass "--uninstall does not wait for a collector that is not this install's"

# The release asset's one-line install and uninstall: curl ... | bash, and
# curl ... | bash -s -- --uninstall.
piped "$INSTALL" | STDIN_CHECK=1 bash > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a piped install failed"; }
latest="$(cat "$root/state/latest")"
[ "$(current)" = "$latest" ] || fail "a piped install points at $(current), not $latest"
[ "$(cat "$root/state/running")" = "$latest" ] || fail "$latest is not running after a piped install"
grep -q "before installing the Agent Relay app on this Mac" "$root/state/out" || { cat "$root/state/out"; fail "a piped install did not reach the end"; }
piped "$INSTALL" | STDIN_CHECK=1 bash -s -- --uninstall > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "a piped --uninstall failed"; }
{ [ ! -e "$plist" ] && [ ! -e "$prefix" ] && [ ! -e "$root/state/loaded-$label" ]; } || fail "a piped --uninstall left the core behind"
grep -Fq "kept settings, sign-in and agent registrations in: $data" "$root/state/out" || fail "a piped --uninstall does not say the data is kept"
[ ! -s "$root/state/stdin-violations" ] || { cat "$root/state/stdin-violations" >&2; fail "a piped run let a command read the script"; }
[ "$(sha256sum "$data/injectable-agents.json")" = "$data_sum" ] || fail "the core's data changed"
pass "curl | bash installs and curl | bash -s -- --uninstall removes it, and no command reads the script"
echo "all macOS install tests passed"

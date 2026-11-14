#!/usr/bin/env bash
#
# install.sh -- install or update the Agent Relay core on a Mac without the app.
#
# One idempotent command for a Mac that runs agents but not the desktop app
# (a Mac mini in a rack, a build Mac): it installs the core
# (`agent-relay-probe`) for this user, runs it as the LaunchAgent
# com.agentrelay.desktop.core.headless in the user's login session, and on
# later runs updates it in place. Run it again at any time; an up-to-date
# healthy install is left alone.
#
#   curl -fsSL https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/install-macos.sh | bash
#   curl -fsSL <same URL> | bash -s -- --uninstall      (any option, the same way)
#   bash install.sh [--version X.Y.Z] [--update] [--rollback] [--dry-run]
#   bash install.sh --status
#   bash install.sh --uninstall
#
# Every release publishes this script as the asset install-macos.sh (Stage
# the release assets, .github/workflows/desktop-release.yml). Piped into
# bash, the script itself is bash's standard input: no command here may read
# it, so every one that could is given /dev/null.
#
# What it does:
#   1. Refuses when the Agent Relay app is installed or its core is loaded:
#      the app runs the same core on the same socket, and the headless core
#      hands over to it (daemon.rs), so the two must not both be set to start.
#   2. Resolves the release (latest, or --version), downloads
#      <ASSET_PREFIX>-<version>-macOS-<arch>-probe.tar.gz, checks it against
#      the release's .sha256, and checks the core's Developer ID signature
#      (Apple's anchor, team QUJ7SA6X8X) before running it.
#   3. Unpacks into versions/<version>, checks the binary reports that
#      version, records the product beside it (versions/<version>/product),
#      and points `current` at it atomically. The previous version stays for
#      rollback; older ones beyond KEEP are removed.
#   4. Writes the LaunchAgent (only when it is missing or differs; before
#      anything is replaced, so a failure there leaves the installation as it
#      was) and loads it into gui/<uid>, or restarts the loaded job with
#      kickstart -k.
#   5. Waits for the new core to answer with the new version; if it does not,
#      points `current` back at the previous version and restarts again.
#
# Never touched: settings, sign-in and agent registrations, which live in the
# core's data directory (~/Library/Application Support/com.agentrelay.desktop),
# not here, and are kept by --uninstall too. Never done: anything needing
# root, or changing credentials; it says what to run instead.
#
# Every product installs into the same prefix and LaunchAgent, as in
# linux/install.sh: a site profile's copy of this script replaces whichever
# product's headless core is installed, as an update would, even at the same
# version: a release counts as installed only when the recorded product is
# this one (an install without a record is Agent Relay's).
#
# Written for the bash 3.2 and BSD tools macOS ships: no `sort -V`,
# `find -printf`, `mv -T`, flock(1), associative arrays or {fd} redirections.
set -euo pipefail

# The product whose releases this installs. A site profile's release
# publishes a copy with these lines set to its own (Stage the release
# assets, .github/workflows/desktop-release.yml); keep each one assignment.
# RELEASE_FEED is 'github' (the releases repository) or 'cloud' (the signed
# release feed of SITE_URL for PROFILE_ID, checked with FEED_PUBLIC_KEY), as
# in linux/install.sh. This script reads the releases repository only: a
# cloud feed carries no macOS core bundle, so a cloud profile's copy refuses
# to install.
PRODUCT_NAME='Agent Relay'
RELEASES_REPOSITORY='AgentWorkforce/relay-desktop-releases'
ASSET_PREFIX='AgentRelay'
RELEASE_FEED='github'
# The cloud feed's three values are substituted with the rest but not read
# until the macOS installer has a cloud path.
# shellcheck disable=SC2034
SITE_URL=''
# shellcheck disable=SC2034
PROFILE_ID=''
# shellcheck disable=SC2034
FEED_PUBLIC_KEY=''
# The product an install without a product record belongs to: every earlier
# release of this script was Agent Relay's.
DEFAULT_ASSET_PREFIX='AgentRelay'

SCRIPT_NAME="agent-relay install"
RELEASES_API="${AGENT_RELAY_RELEASES_API:-https://api.github.com/repos/$RELEASES_REPOSITORY/releases}"
DOWNLOAD_BASE="${AGENT_RELAY_DOWNLOAD_BASE:-https://github.com/$RELEASES_REPOSITORY/releases/download}"
KEEP=3
HEALTH_TIMEOUT="${AGENT_RELAY_HEALTH_TIMEOUT:-60}"
# The label is the app's core label plus `.headless`: the app's own jobs
# (com.agentrelay.desktop.core, .core.local, .core.session) never match it,
# and its ProductTakeover only looks at plists carrying AgentRelayProduct,
# which this one does not.
LABEL='com.agentrelay.desktop.core.headless'
APP_LABELS='com.agentrelay.desktop.core com.agentrelay.desktop.core.local com.agentrelay.desktop.core.session'
APP_PLIST='Agent Relay.app/Contents/Library/LaunchAgents/com.agentrelay.desktop.core.plist'
# The signature package-macos-probe.sh checks before a probe is published:
# a Developer ID Application certificate from Apple, for this team.
REQUIREMENT='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "QUJ7SA6X8X"'

VERSION=""
DRY_RUN=0
ROLLBACK=0
UPDATE=0
ACTION=install

usage() {
  cat <<EOF
Usage: install-macos.sh [--version X.Y.Z] [--update] [--rollback] [--dry-run]
       install-macos.sh --status
       install-macos.sh --uninstall [--dry-run]

  --version X.Y.Z  Install this release instead of the latest.
  --update         Update an existing install (fails when there is none).
  --rollback       Switch back to the newest installed version older than
                   the current one.
  --status         Say what is installed and whether the core answers.
  --uninstall      Stop the core and remove it and its LaunchAgent; settings,
                   sign-in and agents stay in the core's data directory.
  --dry-run        Say what would change and change nothing.
EOF
}

note() { printf '%s: %s\n' "$SCRIPT_NAME" "$*"; }
die() { printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2; exit 1; }
run() {
  if [ "$DRY_RUN" = 1 ]; then
    printf '%s: would run: %s\n' "$SCRIPT_NAME" "$*"
  else
    "$@" < /dev/null
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="${2#v}"; shift 2 ;;
    --update) UPDATE=1; shift ;;
    --rollback) ROLLBACK=1; shift ;;
    --status) ACTION=status; shift ;;
    --uninstall) ACTION=uninstall; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = Darwin ] || die "this installs the macOS core; on Linux use linux/install.sh"
[ -n "${HOME:-}" ] || die "HOME is not set"
[ "$(id -u)" != 0 ] || die "run this as the user the agents belong to, not root"
if [ "$ACTION" != install ] && { [ -n "$VERSION" ] || [ "$UPDATE" = 1 ] || [ "$ROLLBACK" = 1 ]; }; then
  die "--$ACTION takes no --version, --update or --rollback"
fi
[ "$ROLLBACK" = 0 ] || [ -z "$VERSION" ] || die "--rollback takes no --version"

PREFIX="${AGENT_RELAY_PREFIX:-$HOME/.local/lib/agent-relay}"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs/Agent Relay"
DATA_DIR="$HOME/Library/Application Support/com.agentrelay.desktop"
DOMAIN="gui/$(id -u)"
PROBE="$PREFIX/current/agent-relay-probe"

case "$(uname -m)" in
  arm64|aarch64) ARCH=arm64 ;;
  x86_64)
    # A shell under Rosetta on Apple silicon still wants the native core.
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then ARCH=arm64; else ARCH=x64; fi ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

valid_version() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; }
[ -z "$VERSION" ] || valid_version "$VERSION" || die "not a release version: $VERSION"

have() { command -v "$1" >/dev/null 2>&1; }

fetch() { # url dest
  curl -fsSL --proto '=https' --retry 3 -o "$2" "$1" < /dev/null
}

probe_version() { # binary -> version it reports
  "$1" --version < /dev/null 2>/dev/null | awk '{print $NF}' | head -n 1
}

# The unit and the health check say `relay serve` and `relay call`, as
# linux/install.sh does: earlier releases' cores need that word and later ones
# drop it, so a rollback to an earlier core still starts and answers.
core_version() { # -> what the running core reports, or nothing
  "$PROBE" relay call status.get --json < /dev/null 2>/dev/null \
    | grep -o '"daemon":{[^}]*}' | grep -o '"version":"[^"]*"' | head -n 1 | cut -d'"' -f4
}

healthy() { # version
  [ "$DRY_RUN" = 1 ] && return 0
  local waited=0
  while [ "$waited" -lt "$HEALTH_TIMEOUT" ]; do
    [ "$(core_version || true)" = "$1" ] && return 0
    sleep 2
    waited=$((waited + 2))
  done
  return 1
}

installed_versions() { # oldest first
  local entry name
  [ -d "$PREFIX/versions" ] || return 0
  for entry in "$PREFIX/versions"/*; do
    if [ -d "$entry" ] && [ ! -L "$entry" ]; then
      name="${entry##*/}"
      if valid_version "$name"; then printf "%s\n" "$name"; fi
    fi
  done | sort -t. -k1,1n -k2,2n -k3,3n
}

# Whether release a is older than release b by SemVer precedence: the dotted
# numeric core compares numerically (a missing component is 0), a prerelease
# (1.2.0-rc.1) is older than its release (1.2.0), and prerelease identifiers
# compare one by one, numbers numerically and before words, words in ASCII
# order, with fewer identifiers older (beta.2 < beta.11, beta < beta.1).
# Build metadata (+...) is ignored. sort -V puts 1.2.0-rc.1 after 1.2.0.
version_older() { # a b
  LC_ALL=C awk -v a="$1" -v b="$2" '
    function cmp(x, y,   n, m, i, p, q, pa, pb, xp, yp, xn, yn) {
      sub(/[+].*/, "", x); sub(/[+].*/, "", y)
      pa = index(x, "-"); pb = index(y, "-")
      xp = pa ? substr(x, pa + 1) : ""; if (pa) x = substr(x, 1, pa - 1)
      yp = pb ? substr(y, pb + 1) : ""; if (pb) y = substr(y, 1, pb - 1)
      n = split(x, p, "."); m = split(y, q, ".")
      for (i = 1; i <= (n > m ? n : m); i++) {
        if (p[i] + 0 < q[i] + 0) return -1
        if (p[i] + 0 > q[i] + 0) return 1
      }
      if (!pa || !pb) return (pa ? -1 : 0) + (pb ? 1 : 0)
      n = split(xp, p, "."); m = split(yp, q, ".")
      for (i = 1; i <= n && i <= m; i++) {
        xn = p[i] ~ /^[0-9]+$/; yn = q[i] ~ /^[0-9]+$/
        if (xn && yn) {
          if (p[i] + 0 != q[i] + 0) return p[i] + 0 < q[i] + 0 ? -1 : 1
        } else if (xn != yn) {
          return xn ? -1 : 1
        } else if (p[i] "" != q[i] "") {
          return p[i] "" < q[i] "" ? -1 : 1
        }
      }
      return n < m ? -1 : n > m ? 1 : 0
    }
    BEGIN { exit !(cmp(a, b) < 0) }'
}

# The newest installed version older than the current one: --rollback never
# goes forward, even to a version it rolled back from. Like the rest of the
# installed versions, it may be another product's.
rollback_target() {
  local candidate target=""
  [ -n "$current_version" ] || return 0
  for candidate in $(installed_versions); do
    version_older "$candidate" "$current_version" || continue
    if [ -z "$target" ] || version_older "$target" "$candidate"; then target="$candidate"; fi
  done
  printf '%s' "$target"
}

# The product of the installed version: its record, written on install, or
# Agent Relay's for an install made before there was one.
read_current_product() {
  current_product=""
  [ -n "$current_version" ] || return 0
  current_product="$(head -n 1 "$PREFIX/current/product" 2>/dev/null || true)"
  current_product="${current_product:-$DEFAULT_ASSET_PREFIX}"
}

loaded() { launchctl print "$DOMAIN/$LABEL" < /dev/null >/dev/null 2>&1; }

# One installer at a time per prefix: install, update, rollback and uninstall
# all change versions/ and `current`, and two at once can leave `current`
# naming a version the other removed. The lock is held, on a descriptor bash
# keeps open (macOS has no flock(1); perl flock(2)s it, as for the history
# locks below), until this script exits: through replacement, switching, the
# health check, any rollback and pruning. launchd starts the core itself, so
# the core never inherits it. A dry run changes nothing and takes no lock.
#
# --uninstall unlinks the lock file while holding it. An installer that had
# opened the file but not yet locked it would then lock a file no other
# installer can open, beside one that locks a new file at the same path; so
# the lock counts only while the path still names the file locked (same
# device and inode), and otherwise is opened and taken again.
INSTALL_LOCK_FD=19
lock_installer() {
  [ "$DRY_RUN" = 0 ] || return 0
  local lock="$PREFIX/.install-lock" old_umask attempt=1 status
  while :; do
    mkdir -p "$PREFIX" || die "could not create $PREFIX"
    [ ! -L "$lock" ] || die "$lock is a link; remove it, then run this again"
    old_umask="$(umask)"
    umask 077
    eval "exec $INSTALL_LOCK_FD>>\"\$lock\"" || die "could not open $lock"
    umask "$old_umask"
    status=0
    perl -MFcntl=:flock -e '
        open(my $lock, ">>&=", $ARGV[0]) or exit 2;
        flock($lock, LOCK_EX | LOCK_NB) or exit 1;
        my @held = stat($lock);
        my @named = lstat($ARGV[1]);
        exit 3 unless @held && @named && $held[0] == $named[0] && $held[1] == $named[1];
        exit 0' "$INSTALL_LOCK_FD" "$lock" < /dev/null || status=$?
    case "$status" in
      0) return 0 ;;
      1) die "another install, update, rollback or uninstall is running in $PREFIX; wait for it to finish, then run this again" ;;
      3) eval "exec $INSTALL_LOCK_FD>&-"
         [ "$attempt" -lt 5 ] || die "$lock kept being removed or replaced while it was being locked; run this again"
         attempt=$((attempt + 1))
         sleep 1 ;;
      *) die "could not lock $lock" ;;
    esac
  done
}

current_version=""
if [ -x "$PROBE" ]; then
  current_version="$(probe_version "$PROBE" || true)"
fi
read_current_product

# --- status ---------------------------------------------------------------------

if [ "$ACTION" = status ]; then
  if [ -n "$current_version" ]; then
    note "installed: $current_version ($PROBE)"
    note "product: $current_product"
  else
    note "not installed (no $PROBE)"
  fi
  versions="$(installed_versions | tr '\n' ' ')"
  [ -z "$versions" ] || note "versions kept: $versions"
  if [ -f "$PLIST" ]; then note "LaunchAgent: $PLIST"; else note "LaunchAgent: none"; fi
  if loaded; then
    pid="$(launchctl print "$DOMAIN/$LABEL" < /dev/null 2>/dev/null | awk '$1 == "pid" && $2 == "=" {print $3; exit}')"
    note "launchd: $LABEL is loaded in $DOMAIN${pid:+, pid $pid}"
  else
    note "launchd: $LABEL is not loaded in $DOMAIN"
  fi
  answering=""
  [ ! -x "$PROBE" ] || answering="$(core_version || true)"
  note "logs: $LOG_DIR"
  note "data (never removed by this script): $DATA_DIR"
  if [ -n "$answering" ]; then
    note "core: answering, version $answering"
    exit 0
  fi
  note "core: not answering"
  exit 1
fi

# --- uninstall ------------------------------------------------------------------

if [ "$ACTION" = uninstall ]; then
  case "$PREFIX" in
    ""|/|"$HOME"|"$HOME"/) die "refusing to remove the install prefix $PREFIX" ;;
  esac
  if [ -d "$PREFIX" ]; then lock_installer; fi
  if loaded; then
    note "stopping $LABEL"
    run launchctl bootout "$DOMAIN/$LABEL" || true
    # Nothing is deleted while the job is still loaded: launchd would keep
    # (or restart) a core whose executable is gone.
    waited=0
    while [ "$DRY_RUN" = 0 ] && loaded && [ "$waited" -lt 10 ]; do sleep 1; waited=$((waited + 1)); done
    if [ "$DRY_RUN" = 0 ] && loaded; then
      die "launchctl could not unload $LABEL; nothing was removed. Run: launchctl bootout $DOMAIN/$LABEL, then run this with --uninstall again"
    fi
  fi
  if [ -f "$PLIST" ] || [ -L "$PLIST" ]; then run rm -f "$PLIST"; fi
  if [ -e "$PREFIX/current" ] || [ -L "$PREFIX/current" ] || [ -d "$PREFIX/versions" ]; then
    run rm -rf "${PREFIX:?}/versions" "${PREFIX:?}/current" "${PREFIX:?}/current.new"
  fi
  if [ "$DRY_RUN" = 0 ] && [ -d "$PREFIX" ]; then
    # Safe while held: every other installer takes the lock without waiting,
    # so none is queued on this file.
    rm -f "${PREFIX:?}/.install-lock"
    rmdir "$PREFIX" 2>/dev/null || true
  fi
  note "removed the headless core${current_version:+ ($current_version)} and its LaunchAgent"
  note "kept settings, sign-in and agent registrations in: $DATA_DIR"
  note "kept logs in: $LOG_DIR"
  note "delete those yourself if you do not mean to install $PRODUCT_NAME again"
  exit 0
fi

# --- install, update, rollback: preflight -------------------------------------

# --status and --uninstall above need no feed; installing does.
case "$RELEASE_FEED" in
  github) ;;
  cloud) die "the macOS installer does not support the cloud feed yet; install the $PRODUCT_NAME app from your workspace's downloads page" ;;
  *) die "this copy of install-macos.sh names an unknown release feed" ;;
esac

[ "$UPDATE" = 0 ] || [ -n "$current_version" ] || die "--update: nothing is installed in $PREFIX; run without --update to install"

# The app's core and this one serve the same socket: the second to start
# exits ("already running"), and a headless core that sees the app's job
# exits cleanly to hand over. Installing both leaves one of them failing.
app_reason=""
for apps in /Applications "$HOME/Applications"; do
  if [ -f "$apps/$APP_PLIST" ]; then app_reason="$apps/${APP_PLIST%%/*} is installed"; break; fi
done
if [ -z "$app_reason" ]; then
  for app_label in $APP_LABELS; do
    if launchctl print "$DOMAIN/$app_label" < /dev/null >/dev/null 2>&1; then app_reason="the app's core ($app_label) is loaded"; break; fi
  done
fi
if [ -n "$app_reason" ]; then
  die "$app_reason. The $PRODUCT_NAME app runs this core itself; use the app, or quit and delete it (then log out and in) before installing the headless core"
fi

# gui/<uid> exists only while the user is logged in at the Mac (or by
# automatic login); an SSH session alone has no such domain.
launchctl print "$DOMAIN" < /dev/null >/dev/null 2>&1 \
  || die "there is no login session ($DOMAIN) for $(id -un): log in at the Mac's screen once (or turn on automatic login), then run this again; SSH alone cannot start a LaunchAgent"

# Read the installation again under the lock: another installer may have
# changed it since the checks above.
lock_installer
current_version=""
if [ -x "$PROBE" ]; then
  current_version="$(probe_version "$PROBE" || true)"
fi
read_current_product
[ "$UPDATE" = 0 ] || [ -n "$current_version" ] || die "--update: nothing is installed in $PREFIX; run without --update to install"

if [ -n "$current_version" ]; then
  note "installed: $current_version"
else
  note "no per-user install found"
fi

# --- local session history ------------------------------------------------------
#
# A version answering status.get says nothing about its ability to open local
# history. Hold the SAME locks as the queue migration across each check and
# binary switch, then release them before starting the core that resumes it.
# macOS has no flock(1): bash opens each lock file on a fixed descriptor and
# perl flock(2)s it; the lock belongs to the open file, so it holds until bash
# closes the descriptor.
storage_lock_fds=""
next_lock_fd=20
lock_storage_migrations() {
  [ "$DRY_RUN" = 0 ] || return 0
  local wait=0 store lock fd old_umask
  [ "${1:-}" != "--wait" ] || wait=1
  for store in "$HOME"/.agentworkforce/probe/*; do
    [ -d "$store" ] || continue
    [ -e "$store/history.db" ] || [ -e "$store/delivery-queue.db" ] || continue
    lock="$store/delivery-queue.migration-lock"
    if [ "$wait" = 1 ]; then
      [ ! -L "$lock" ] || die "rollback not completed: local history migration lock is a link; the new core ($VERSION) is still installed. Remove the link, then run the installer again"
    else
      [ ! -L "$lock" ] || die "local history migration lock is a link; installation unchanged"
    fi
    fd="$next_lock_fd"
    next_lock_fd=$((next_lock_fd + 1))
    old_umask="$(umask)"
    umask 077
    if ! eval "exec $fd>>\"\$lock\""; then
      [ "$wait" = 1 ] && die "rollback not completed: could not lock local session history; the new core ($VERSION) is still installed. Run the installer again"
      die "could not lock local session history; installation unchanged"
    fi
    umask "$old_umask"
    storage_lock_fds="$storage_lock_fds $fd"
    if ! perl -MFcntl=:flock -e '
        open(my $lock, ">>&=", $ARGV[0]) or exit 2;
        my $wait = $ARGV[1];
        until (flock($lock, LOCK_EX | LOCK_NB)) { exit 1 if $wait-- <= 0; sleep 1 }
        exit 0' "$fd" "$([ "$wait" = 1 ] && echo 60 || echo 0)"; then
      [ "$wait" = 1 ] && die "rollback not completed: local session history is still upgrading and the new core ($VERSION) did not answer. Wait for the upgrade to finish, then run the installer again"
      die "local session history is upgrading; try the update again when it finishes"
    fi
  done
}

unlock_storage_migrations() {
  local fd
  for fd in $storage_lock_fds; do
    eval "exec $fd>&-"
  done
  storage_lock_fds=""
}

# A check, never a schema repair: the macOS port of linux/install.sh's, in the
# perl and sqlite3 every Mac ships rather than python3.
storage_compatible() { # candidate binary
  [ -d "$HOME/.agentworkforce/probe" ] || return 0
  have sqlite3 || { note "sqlite3 is required to check local history compatibility"; return 1; }
  local allow_immutable=0
  # A clean WAL database has no -wal/-shm files. macOS sqlite3 cannot open
  # that state with -readonly because it would have to create -shm. Only a
  # stopped service may use immutable=1 instead: it ignores locks and WAL, so
  # the collector lock and the absence of both sidecars are mandatory too.
  loaded || allow_immutable=1
  perl -MJSON::PP -MCwd=abs_path -MB -e '
use strict; use warnings;
use Encode qw(encode);
use Fcntl qw(:DEFAULT :flock);
my ($core, $root, $allow_immutable, $service) = @ARGV;
my %collector_locks;
sub refuse { print STDERR "agent-relay install: $_[0]\n"; exit 1 }
sub sqlite_query {
    my ($db, $sql) = @_;
    my $pid = open(my $out, "-|");
    defined $pid or die "sqlite3\n";
    if (!$pid) {
        open(STDIN, "<", "/dev/null"); open(STDERR, ">", "/dev/null");
        exec("sqlite3", "-readonly", "-batch", "-noheader", "-cmd", ".timeout 1000", $db, $sql);
        exit 127;
    }
    local $/;
    my $text = <$out>;
    my $ok = close($out);
    $text = "" unless defined $text;
    $text =~ s/\n\z//;
    return ($ok, $text);
}
sub collector_stopped {
    my ($store) = @_;
    return 1 if exists $collector_locks{$store};
    my $path = "$store/collector.lock";
    return 0 if -l $path;
    my $old = umask(077);
    my $opened = sysopen(my $lock, $path, O_RDWR | O_CREAT | O_NOFOLLOW, 0600);
    umask($old);
    return 0 unless $opened && flock($lock, LOCK_EX | LOCK_NB);
    $collector_locks{$store} = $lock;
    return 1;
}
sub immutable_uri {
    my ($db) = @_;
    my $bytes = encode("UTF-8", $db);
    $bytes =~ s/([^A-Za-z0-9._~\/-])/sprintf("%%%02X", ord($1))/ge;
    return "file:$bytes?mode=ro&immutable=1";
}
sub service_stopped {
    my $pid = fork();
    defined $pid or return 0;
    if (!$pid) {
        open(STDIN, "<", "/dev/null"); open(STDOUT, ">", "/dev/null"); open(STDERR, ">", "/dev/null");
        exec("launchctl", "print", $service);
        exit 127;
    }
    waitpid($pid, 0);
    return $? != 0;
}
sub query {
    my ($db, $sql, $store) = @_;
    my ($ok, $text) = sqlite_query($db, $sql);
    return $text if $ok;
    die "sqlite3\n" unless $allow_immutable eq "1"
        && !-e "$db-wal" && !-e "$db-shm"
        && collector_stopped($store)
        && service_stopped()
        && !-e "$db-wal" && !-e "$db-shm";
    ($ok, $text) = sqlite_query(immutable_uri($db), $sql);
    die "sqlite3\n" unless $ok;
    return $text;
}
sub is_int {
    my ($value) = @_;
    return 0 if !defined $value || ref $value;
    my $flags = B::svref_2object(\$value)->FLAGS;
    return ($flags & B::SVf_IOK()) && !($flags & B::SVf_POK()) && !($flags & B::SVf_NOK());
}
my %required;
my $verified = eval {
    opendir(my $dir, $root) or die "opendir\n";
    for my $entry (sort readdir($dir)) {
        next if $entry eq "." || $entry eq "..";
        my $store = "$root/$entry";
        next unless -d $store;
        for my $name ("history.db", "delivery-queue.db") {
            my $path = "$store/$name";
            next unless -e $path;
            if (($name eq "delivery-queue.db" && -l $path) || !-f $path) {
                refuse("local history format cannot be verified; saved uploads were not changed");
            }
            my $db = abs_path($path);
            if (query($db, "SELECT 1 FROM sqlite_master WHERE type=\x27table\x27 AND name=\x27_relay_delivery_sidecar\x27", $store) eq "1") {
                if (query($db, "SELECT count(*) FROM _relay_delivery_sidecar WHERE singleton=1", $store) ne "1") {
                    refuse("local history migration metadata is incomplete; saved uploads were not changed");
                }
                my $hex = query($db, "SELECT hex(manifest) FROM _relay_delivery_sidecar WHERE singleton=1", $store);
                my $manifest = eval { JSON::PP->new->utf8->decode(pack("H*", $hex)) };
                refuse("local history migration metadata is invalid; saved uploads were not changed")
                    unless ref $manifest eq "HASH";
                my $version = $manifest->{version};
                refuse("local history migration format is unknown; saved uploads were not changed")
                    unless is_int($version) && $version >= 1;
                $required{$version} = 1;
            } elsif ($name eq "delivery-queue.db") {
                # An empty queue file can precede the copy'"'"'s first commit; the
                # owner resumes that. Only a populated foreign queue is unverifiable.
                if (query($db, "SELECT 1 FROM sqlite_master WHERE name NOT GLOB \x27sqlite_*\x27 LIMIT 1", $store) eq "1") {
                    refuse("local upload queue has no migration metadata; saved uploads were not changed");
                }
            } else {
                if (query($db, "SELECT 1 FROM sqlite_master WHERE type=\x27view\x27 AND name=\x27delivery_jobs\x27", $store) eq "1") {
                    refuse("local history has an unverified upload migration; saved uploads were not changed");
                }
                $required{0} = 1;
            }
        }
    }
    my $output = "";
    my $pid = open(my $answer, "-|") // die "fork\n";
    if (!$pid) {
        open(STDIN, "<", "/dev/null"); open(STDERR, ">", "/dev/null");
        exec($core, "--local-storage-capabilities") or exit 127;
    }
    local $SIG{ALRM} = sub { kill("KILL", $pid); die "timeout\n" };
    alarm 5;
    { local $/; $output = <$answer>; }
    close($answer);
    my $status = $?;
    alarm 0;
    $output = "" unless defined $output;
    my @supported;
    # Older releases have no capability command. They support legacy state only.
    if ($status != 0 || $output !~ /\S/) {
        @supported = (0);
    } else {
        refuse("the target core returned invalid local history capabilities") if length($output) > 4096;
        my $capabilities = eval { JSON::PP->new->utf8->decode($output) };
        refuse("the target core returned invalid local history capabilities")
            unless ref $capabilities eq "HASH" && is_int($capabilities->{protocol}) && $capabilities->{protocol} == 1
            && ref $capabilities->{upload_queue_formats} eq "ARRAY" && @{$capabilities->{upload_queue_formats}}
            && !grep { !is_int($_) || $_ < 0 } @{$capabilities->{upload_queue_formats}};
        @supported = @{$capabilities->{upload_queue_formats}};
    }
    my %supported = map { $_ => 1 } @supported;
    %required = (0 => 1) unless %required;
    for my $format (keys %required) {
        refuse("this core cannot read the upgraded local history; keeping saved uploads and the compatible installation. Install a newer compatible release instead of downgrading")
            unless $supported{$format};
    }
    1;
};
refuse("could not verify local history compatibility; saved uploads were not changed") unless $verified;
' "$1" "$HOME/.agentworkforce/probe" "$allow_immutable" "$DOMAIN/$LABEL" < /dev/null
}

# --- the LaunchAgent ------------------------------------------------------------

xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

# KeepAlive restarts the core when it fails but not when it exits cleanly:
# a headless core exits 0 to hand over when the app's core job appears, and
# must then stay down. ThrottleInterval matches the app's own fallback job.
plist_text() {
  local probe out err
  probe="$(xml_escape "$PROBE")"
  out="$(xml_escape "$LOG_DIR/headless.stdout.log")"
  err="$(xml_escape "$LOG_DIR/headless.stderr.log")"
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$probe</string>
    <string>relay</string>
    <string>serve</string>
    <string>--headless</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>$out</string>
  <key>StandardErrorPath</key>
  <string>$err</string>
</dict>
</plist>
EOF
}

plist_changed=0
ensure_plist() {
  local wanted
  wanted="$(plist_text)"
  if [ -f "$PLIST" ] && [ "$(cat "$PLIST")" = "$wanted" ]; then
    return 0
  fi
  plist_changed=1
  run mkdir -p "$HOME/Library/LaunchAgents" "$LOG_DIR"
  if [ "$DRY_RUN" = 1 ]; then
    note "would write $PLIST"
    return 0
  fi
  printf '%s\n' "$wanted" > "$PLIST.new"
  chmod 0644 "$PLIST.new"
  if have plutil; then
    plutil -lint "$PLIST.new" < /dev/null >/dev/null || { rm -f "$PLIST.new"; die "the LaunchAgent written is not a valid property list"; }
  fi
  mv "$PLIST.new" "$PLIST"
}

# Loads the job, or restarts it when it is already loaded. The plist names
# `current`, so an update needs only kickstart -k; a changed plist needs the
# job unloaded and loaded again for launchd to read it.
restart_core() {
  run mkdir -p "$LOG_DIR"
  if loaded; then
    if [ "$plist_changed" = 0 ]; then
      run launchctl kickstart -k "$DOMAIN/$LABEL"
      return
    fi
    run launchctl bootout "$DOMAIN/$LABEL" || true
    local waited=0
    while [ "$DRY_RUN" = 0 ] && loaded && [ "$waited" -lt 10 ]; do sleep 1; waited=$((waited + 1)); done
  fi
  # A job someone disabled refuses to load until it is enabled again.
  run launchctl enable "$DOMAIN/$LABEL" || true
  run launchctl bootstrap "$DOMAIN" "$PLIST" || return 1
  plist_changed=0
}

switch_to() { # version
  local link="$PREFIX/current"
  run rm -f "$link.new"
  run ln -s "versions/$1" "$link.new"
  # rename(2) replaces the old link in one step; BSD mv has no -T.
  run perl -e 'rename($ARGV[0], $ARGV[1]) or die "rename: $!\n"' "$link.new" "$link"
}

# Finish setup as a separate command: curl | bash does not leave a terminal
# on stdin, and installation must never silently change sign-in or preferences.
next_steps() {
  [ "$DRY_RUN" = 0 ] || return 0
  [ -x "$PROBE" ] || return 0
  if "$PROBE" setup --help < /dev/null >/dev/null 2>&1; then
    note "finish signing in and choosing providers with:"
    printf '  %q setup\n' "$PROBE"
    note "approve the displayed device code in a browser on any computer."
  else
    note "finish setup with the agent-driven setup guide:"
    note "  https://github.com/AgentWorkforce/relay-app/blob/main/docs/agent-driven-setup.md"
  fi
  note "check it with this installer's --status, or: $PROBE relay call status.get"
  note "logs: $LOG_DIR"
  note "the core runs in your login session ($DOMAIN): after a restart it starts only once you log in."
  note "on an unattended Mac, turn on automatic login (System Settings > Users & Groups; FileVault must be off)."
  note "before installing the $PRODUCT_NAME app on this Mac, run this installer with --uninstall."
}

# Puts back the copy of $VERSION this install set aside (another product's,
# or one left over), if there was one.
destination=""
restore_replaced() {
  [ -n "$destination" ] && [ -d "$destination.replaced" ] || return 0
  run rm -rf "$destination"
  run mv "$destination.replaced" "$destination"
}

# Between switching `current` and handing the result to the restart and its
# health check, an exit for any reason puts the previous installation back:
# launchd must never start a release nothing checked.
undo_to=""
undo_switch() {
  [ -n "$undo_to" ] || return 0
  printf '%s: %s\n' "$SCRIPT_NAME" "putting the previous installation ($undo_to) back" >&2
  [ "$undo_to" = "$VERSION" ] || switch_to "$undo_to" || true
  restore_replaced || true
  undo_to=""
}
work=""
trap 'status=$?; [ "$status" = 0 ] || undo_switch; [ -z "$work" ] || rm -rf "$work"' EXIT

# --- rollback -------------------------------------------------------------------

if [ "$ROLLBACK" = 1 ]; then
  target="$(rollback_target)"
  [ -n "$target" ] || die "no earlier version to roll back to"
  lock_storage_migrations
  storage_compatible "$PREFIX/versions/$target/agent-relay-probe" || die "rollback refused before changing the installation"
  # The LaunchAgent first: if it cannot be written, `current` is unchanged.
  ensure_plist
  note "rolling back ${current_version:-?} -> $target"
  undo_to="$current_version"
  switch_to "$target"
  unlock_storage_migrations
  undo_to=""
  restart_core || true
  healthy "$target" || die "the core did not come back on $target; see: $LOG_DIR"
  note "now running $target"
  next_steps
  exit 0
fi

# --- 2. which release -----------------------------------------------------------

work="$(mktemp -d)"
if [ -z "$VERSION" ]; then
  fetch "$RELEASES_API/latest" "$work/latest.json" || die "could not read the latest release"
  VERSION="$(grep -o '"tag_name": *"[^"]*"' "$work/latest.json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
  VERSION="${VERSION#v}"
  valid_version "$VERSION" || die "the latest release has no usable version"
fi
note "release: $VERSION ($ARCH)"

if [ "$current_version" = "$VERSION" ] && [ "$current_product" != "$ASSET_PREFIX" ]; then
  note "the installed $VERSION is another product's; replacing it with $PRODUCT_NAME's"
fi
if [ "$current_version" = "$VERSION" ] && [ "$current_product" = "$ASSET_PREFIX" ]; then
  lock_storage_migrations
  storage_compatible "$PROBE" || die "the installed core needs a compatible update"
  ensure_plist
  if [ "$plist_changed" = 0 ] && loaded && { [ "$DRY_RUN" = 1 ] || [ "$(core_version || true)" = "$VERSION" ]; }; then
    unlock_storage_migrations
    note "already up to date"
    next_steps
    exit 0
  fi
  note "installed but not answering; restarting it"
  unlock_storage_migrations
  restart_core || true
  healthy "$VERSION" || die "the core did not start; see: $LOG_DIR"
  note "running $VERSION"
  next_steps
  exit 0
fi

# --- 3. download, verify, unpack ------------------------------------------------

asset="$ASSET_PREFIX-$VERSION-macOS-$ARCH-probe.tar.gz"
note "downloading $asset"
fetch "$DOWNLOAD_BASE/v$VERSION/$asset" "$work/$asset" || die "could not download $asset"
fetch "$DOWNLOAD_BASE/v$VERSION/$asset.sha256" "$work/$asset.sha256" || die "could not download $asset.sha256"
expected="$(awk '{print $1}' "$work/$asset.sha256" | head -n 1)"
printf '%s' "$expected" | grep -Eq '^[0-9a-f]{64}$' || die "the published checksum is unreadable"
[ "$(shasum -a 256 "$work/$asset" | cut -d' ' -f1)" = "$expected" ] || die "$asset does not match its published checksum"
note "checksum verified"

# Only the published paths are accepted out of the archive: the core, and the
# project license shipped beside it.
listing="$(tar -tzf "$work/$asset")"
grep -qxF agent_relay/helpers/agent-relay-probe <<< "$listing" \
  || die "the archive holds no core"
if grep -vxE 'agent_relay/|agent_relay/helpers/|agent_relay/helpers/agent-relay-probe|agent_relay/LICENSE' \
  <<< "$listing" > /dev/null || [ -n "$(sort <<< "$listing" | uniq -d)" ]; then
  die "the archive holds something other than the core"
fi
has_license=0
if grep -qxF agent_relay/LICENSE <<< "$listing"; then has_license=1; fi
tar -xzf "$work/$asset" -C "$work" agent_relay/helpers/agent-relay-probe
candidate="$work/agent_relay/helpers/agent-relay-probe"
{ [ -f "$candidate" ] && [ ! -L "$candidate" ]; } || die "the archive's core is not a plain file"
if [ "$has_license" = 1 ]; then
  tar -xzf "$work/$asset" -C "$work" agent_relay/LICENSE
  # A link would copy whatever it points at into the install tree.
  { [ -f "$work/agent_relay/LICENSE" ] && [ ! -L "$work/agent_relay/LICENSE" ]; } \
    || die "the archive's LICENSE is not a plain file"
fi
# The signature is checked before the core is run at all.
codesign --verify --strict "-R=$REQUIREMENT" "$candidate" < /dev/null 2>/dev/null \
  || die "the downloaded core is not signed with $PRODUCT_NAME's Developer ID (team QUJ7SA6X8X)"
note "signature verified"
reported="$(probe_version "$candidate" || true)"
[ "$reported" = "$VERSION" ] || die "the downloaded core reports ${reported:-nothing}, not $VERSION"
lock_storage_migrations
storage_compatible "$candidate" || die "installation refused before replacing the core"

# The LaunchAgent names `current`, not a version, so it is written before
# anything is replaced: if it cannot be, the installation stays as it was.
ensure_plist

destination="$PREFIX/versions/$VERSION"
was="${current_version:-}"
if [ "$DRY_RUN" = 1 ]; then
  note "would install $destination"
else
  mkdir -p "$PREFIX/versions"
  rm -rf "$destination.partial"
  mkdir -p "$destination.partial"
  install -m 0755 "$candidate" "$destination.partial/agent-relay-probe"
  if [ "$has_license" = 1 ]; then
    install -m 0644 "$work/agent_relay/LICENSE" "$destination.partial/LICENSE"
  fi
  printf '%s\n' "$ASSET_PREFIX" > "$destination.partial/product"
  # An existing copy of this version (another product's, or one left over)
  # is set aside until the new core answers, and comes back if it does not.
  rm -rf "$destination.replaced"
  # Replacing the version `current` names changes the installation now.
  [ "$was" != "$VERSION" ] || undo_to="$was"
  if [ -d "$destination" ]; then mv "$destination" "$destination.replaced"; fi
  mv "$destination.partial" "$destination"
fi

# --- 4. switch and restart ------------------------------------------------------

[ -z "$was" ] || undo_to="$was"
switch_to "$VERSION"
unlock_storage_migrations
undo_to=""
# A job launchd refuses to load rolls back like a core that never answers.
restarted=1
restart_core || { restarted=0; note "launchctl could not start $LABEL"; }

# --- 5. health, or roll back ----------------------------------------------------

if [ "$restarted" = 0 ] || ! healthy "$VERSION"; then
  if [ -n "$was" ] && [ "$was" != "$VERSION" ] && [ -d "$PREFIX/versions/$was" ]; then
    lock_storage_migrations --wait
    storage_compatible "$PREFIX/versions/$was/agent-relay-probe" || die "the new core did not answer; unsafe rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; rolling back to $was"
    switch_to "$was"
    # A version that failed is never a rollback target.
    run rm -rf "${PREFIX:?}/versions/$VERSION"
    restore_replaced
    unlock_storage_migrations
    restart_core || true
    healthy "$was" || die "rollback to $was did not answer either; see: $LOG_DIR"
    die "update to $VERSION failed and was rolled back to $was; see: $LOG_DIR"
  fi
  # The same version of another product: put its copy back and restart it.
  if [ -n "$was" ] && [ "$was" = "$VERSION" ] && [ -d "$destination.replaced" ]; then
    lock_storage_migrations --wait
    storage_compatible "$destination.replaced/agent-relay-probe" || die "the new core did not answer; unsafe same-version rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; going back to the installed $was"
    restore_replaced
    unlock_storage_migrations
    restart_core || true
    healthy "$was" || die "going back to $was did not answer either; see: $LOG_DIR"
    die "replacing the installed $was failed and it is running again; see: $LOG_DIR"
  fi
  die "the core did not start; see: $LOG_DIR (remove it with --uninstall)"
fi
note "running $VERSION"
[ "$DRY_RUN" = 1 ] || rm -rf "$destination.replaced"

# Keep the newest KEEP versions (always the current one).
if [ "$DRY_RUN" = 0 ]; then
  installed_versions | { grep -vxF "$VERSION" || true; } \
    | awk -v keep=$((KEEP - 1)) '{ line[NR] = $0 } END { for (i = 1; i <= NR - keep; i++) print line[i] }' \
    | while read -r old; do rm -rf "${PREFIX:?}/versions/$old"; done
fi
next_steps

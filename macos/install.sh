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
#   curl -fsSL <this script> | bash
#   bash install.sh [--version X.Y.Z] [--update] [--rollback] [--dry-run]
#   bash install.sh --status
#   bash install.sh --uninstall
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
#      version, and points `current` at it atomically. The previous version
#      stays for rollback; older ones beyond KEEP are removed.
#   4. Writes the LaunchAgent (only when it is missing or differs) and loads
#      it into gui/<uid>, or restarts the loaded job with kickstart -k.
#   5. Waits for the new core to answer with the new version; if it does not,
#      points `current` back at the previous version and restarts again.
#
# Never touched: settings, sign-in and agent registrations, which live in the
# core's data directory (~/Library/Application Support/com.agentrelay.desktop),
# not here, and are kept by --uninstall too. Never done: anything needing
# root, or changing credentials; it says what to run instead.
#
# Written for the bash 3.2 and BSD tools macOS ships: no `sort -V`,
# `find -printf`, `mv -T`, flock(1), associative arrays or {fd} redirections.
set -euo pipefail

PRODUCT_NAME='Agent Relay'
RELEASES_REPOSITORY='AgentWorkforce/relay-desktop-releases'
ASSET_PREFIX='AgentRelay'

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
Usage: install.sh [--version X.Y.Z] [--update] [--rollback] [--dry-run]
       install.sh --status
       install.sh --uninstall [--dry-run]

  --version X.Y.Z  Install this release instead of the latest.
  --update         Update an existing install (fails when there is none).
  --rollback       Switch back to the previously installed version.
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
    "$@"
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
  curl -fsSL --proto '=https' --retry 3 -o "$2" "$1"
}

probe_version() { # binary -> version it reports
  "$1" --version 2>/dev/null | awk '{print $NF}' | head -n 1
}

# The unit and the health check say `relay serve` and `relay call`, as
# linux/install.sh does: earlier releases' cores need that word and later ones
# drop it, so a rollback to an earlier core still starts and answers.
core_version() { # -> what the running core reports, or nothing
  "$PROBE" relay call status.get --json 2>/dev/null \
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

loaded() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }

# One installer at a time per prefix: install, update, rollback and uninstall
# all change versions/ and `current`, and two at once can leave `current`
# naming a version the other removed. The lock is held, on a descriptor bash
# keeps open (macOS has no flock(1); perl flock(2)s it, as for the history
# locks below), until this script exits: through replacement, switching, the
# health check, any rollback and pruning. launchd starts the core itself, so
# the core never inherits it. A dry run changes nothing and takes no lock.
INSTALL_LOCK_FD=19
lock_installer() {
  [ "$DRY_RUN" = 0 ] || return 0
  local lock="$PREFIX/.install-lock" old_umask
  mkdir -p "$PREFIX" || die "could not create $PREFIX"
  [ ! -L "$lock" ] || die "$lock is a link; remove it, then run this again"
  old_umask="$(umask)"
  umask 077
  eval "exec $INSTALL_LOCK_FD>>\"\$lock\"" || die "could not open $lock"
  umask "$old_umask"
  perl -MFcntl=:flock -e 'open(my $lock, ">>&=", $ARGV[0]) or exit 2; flock($lock, LOCK_EX | LOCK_NB) or exit 1' "$INSTALL_LOCK_FD" \
    || die "another install, update, rollback or uninstall is running in $PREFIX; wait for it to finish, then run this again"
}

current_version=""
if [ -x "$PROBE" ]; then
  current_version="$(probe_version "$PROBE" || true)"
fi

# --- status ---------------------------------------------------------------------

if [ "$ACTION" = status ]; then
  if [ -n "$current_version" ]; then
    note "installed: $current_version ($PROBE)"
  else
    note "not installed (no $PROBE)"
  fi
  versions="$(installed_versions | tr '\n' ' ')"
  [ -z "$versions" ] || note "versions kept: $versions"
  if [ -f "$PLIST" ]; then note "LaunchAgent: $PLIST"; else note "LaunchAgent: none"; fi
  if loaded; then
    pid="$(launchctl print "$DOMAIN/$LABEL" 2>/dev/null | awk '$1 == "pid" && $2 == "=" {print $3; exit}')"
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
    if launchctl print "$DOMAIN/$app_label" >/dev/null 2>&1; then app_reason="the app's core ($app_label) is loaded"; break; fi
  done
fi
if [ -n "$app_reason" ]; then
  die "$app_reason. The $PRODUCT_NAME app runs this core itself; use the app, or quit and delete it (then log out and in) before installing the headless core"
fi

# gui/<uid> exists only while the user is logged in at the Mac (or by
# automatic login); an SSH session alone has no such domain.
launchctl print "$DOMAIN" >/dev/null 2>&1 \
  || die "there is no login session ($DOMAIN) for $(id -un): log in at the Mac's screen once (or turn on automatic login), then run this again; SSH alone cannot start a LaunchAgent"

# Read the installation again under the lock: another installer may have
# changed it since the checks above.
lock_installer
current_version=""
if [ -x "$PROBE" ]; then
  current_version="$(probe_version "$PROBE" || true)"
fi
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
  perl -MJSON::PP -MCwd=abs_path -MB -e '
use strict; use warnings;
my ($core, $root) = @ARGV;
sub refuse { print STDERR "agent-relay install: $_[0]\n"; exit 1 }
sub query {
    my ($db, $sql) = @_;
    open(my $out, "-|", "sqlite3", "-readonly", "-batch", "-noheader", "-cmd", ".timeout 1000", $db, $sql)
        or die "sqlite3\n";
    local $/;
    my $text = <$out>;
    close($out) or die "sqlite3\n";
    $text = "" unless defined $text;
    $text =~ s/\n\z//;
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
            if (query($db, "SELECT 1 FROM sqlite_master WHERE type=\x27table\x27 AND name=\x27_relay_delivery_sidecar\x27") eq "1") {
                if (query($db, "SELECT count(*) FROM _relay_delivery_sidecar WHERE singleton=1") ne "1") {
                    refuse("local history migration metadata is incomplete; saved uploads were not changed");
                }
                my $hex = query($db, "SELECT hex(manifest) FROM _relay_delivery_sidecar WHERE singleton=1");
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
                if (query($db, "SELECT 1 FROM sqlite_master WHERE name NOT GLOB \x27sqlite_*\x27 LIMIT 1") eq "1") {
                    refuse("local upload queue has no migration metadata; saved uploads were not changed");
                }
            } else {
                if (query($db, "SELECT 1 FROM sqlite_master WHERE type=\x27view\x27 AND name=\x27delivery_jobs\x27") eq "1") {
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
' "$1" "$HOME/.agentworkforce/probe"
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
    plutil -lint "$PLIST.new" >/dev/null || { rm -f "$PLIST.new"; die "the LaunchAgent written is not a valid property list"; }
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
  if "$PROBE" setup --help >/dev/null 2>&1; then
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

# --- rollback -------------------------------------------------------------------

if [ "$ROLLBACK" = 1 ]; then
  target="$(installed_versions | { grep -vxF "${current_version:-}" || true; } | tail -n 1)"
  [ -n "$target" ] || die "no earlier version to roll back to"
  lock_storage_migrations
  storage_compatible "$PREFIX/versions/$target/agent-relay-probe" || die "rollback refused before changing the installation"
  note "rolling back ${current_version:-?} -> $target"
  switch_to "$target"
  ensure_plist
  unlock_storage_migrations
  restart_core || true
  healthy "$target" || die "the core did not come back on $target; see: $LOG_DIR"
  note "now running $target"
  next_steps
  exit 0
fi

# --- 2. which release -----------------------------------------------------------

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -z "$VERSION" ]; then
  fetch "$RELEASES_API/latest" "$work/latest.json" || die "could not read the latest release"
  VERSION="$(grep -o '"tag_name": *"[^"]*"' "$work/latest.json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
  VERSION="${VERSION#v}"
  valid_version "$VERSION" || die "the latest release has no usable version"
fi
note "release: $VERSION ($ARCH)"

if [ "$current_version" = "$VERSION" ]; then
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
codesign --verify --strict "-R=$REQUIREMENT" "$candidate" 2>/dev/null \
  || die "the downloaded core is not signed with $PRODUCT_NAME's Developer ID (team QUJ7SA6X8X)"
note "signature verified"
reported="$(probe_version "$candidate" || true)"
[ "$reported" = "$VERSION" ] || die "the downloaded core reports ${reported:-nothing}, not $VERSION"
lock_storage_migrations
storage_compatible "$candidate" || die "installation refused before replacing the core"

destination="$PREFIX/versions/$VERSION"
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
  # A copy of this version left by an earlier, failed install is replaced.
  rm -rf "$destination"
  mv "$destination.partial" "$destination"
fi

# --- 4. switch and restart ------------------------------------------------------

was="${current_version:-}"
switch_to "$VERSION"
ensure_plist
unlock_storage_migrations
# A job launchd refuses to load rolls back like a core that never answers.
restarted=1
restart_core || { restarted=0; note "launchctl could not start $LABEL"; }

# --- 5. health, or roll back ----------------------------------------------------

if [ "$restarted" = 0 ] || ! healthy "$VERSION"; then
  if [ -n "$was" ] && [ -d "$PREFIX/versions/$was" ]; then
    lock_storage_migrations --wait
    storage_compatible "$PREFIX/versions/$was/agent-relay-probe" || die "the new core did not answer; unsafe rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; rolling back to $was"
    switch_to "$was"
    # A version that failed is never a rollback target.
    run rm -rf "${PREFIX:?}/versions/$VERSION"
    unlock_storage_migrations
    restart_core || true
    healthy "$was" || die "rollback to $was did not answer either; see: $LOG_DIR"
    die "update to $VERSION failed and was rolled back to $was; see: $LOG_DIR"
  fi
  die "the core did not start; see: $LOG_DIR (remove it with --uninstall)"
fi
note "running $VERSION"

# Keep the newest KEEP versions (always the current one).
if [ "$DRY_RUN" = 0 ]; then
  installed_versions | { grep -vxF "$VERSION" || true; } \
    | awk -v keep=$((KEEP - 1)) '{ line[NR] = $0 } END { for (i = 1; i <= NR - keep; i++) print line[i] }' \
    | while read -r old; do rm -rf "${PREFIX:?}/versions/$old"; done
fi
next_steps

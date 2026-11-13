# shellcheck shell=bash
#
# install-test-home.sh -- the disposable-HOME guard for the installer tests.
# Sourced, not run. The installer tests delete under $HOME (the installed
# cores, the unit, the core's data); run against a real HOME they destroy a
# live Agent Relay install, its session history and its delivery queue.
#
# scripts/test-linux-install.sh makes its HOME with mktemp, calls
# install_test_home_init right after it exports it, and calls
# require_disposable_home before it deletes anything under it. A fragment it
# sources starts with:
#
#   # shellcheck source=scripts/install-test-home.sh
#   . "$(dirname "${BASH_SOURCE[0]}")/install-test-home.sh"
#   install_test_fragment
#
# which refuses to run the fragment on its own, then checks HOME the same way.

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  echo "install-test-home.sh is sourced by the installer tests, not run" >&2
  exit 2
fi

INSTALL_TEST_HOME_MARKER=.agent-relay-install-test-home
INSTALL_TEST_HOME_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

# Writes the marker that tells the guard this HOME is the harness's own
# throwaway directory. Call it right after HOME is made and exported.
install_test_home_init() {
  printf 'pid %s nonce %s\n' "$$" "$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')" \
    > "$HOME/$INSTALL_TEST_HOME_MARKER"
}

# Echoes why the XDG directory named by $1 is unsafe for a test whose HOME
# resolves to $2, or nothing: it must be set, lie under that HOME, and not be
# or lie under the passwd home or the HOME and XDG directories the harness
# started with (INSTALL_TEST_ORIGINAL_*), except that a protected directory
# which contains the test HOME itself (TMPDIR under the starting HOME, or
# HOME=/tmp) is skipped: the harness's own mktemp HOME may legitimately lie
# there, and the XDG directories must still lie under that HOME. Resolved
# with realpath -m, so it need not exist yet.
install_test_xdg_reason() { # name resolved-home
  local name="$1" home="$2" value resolved protected real
  value="${!name:-}"
  [ -n "$value" ] || { echo "$name is not set"; return; }
  resolved="$(realpath -m -- "$value")"
  case "$resolved/" in
    "$home"/?*) ;;
    *) echo "$name ($value) is not under the test's HOME"; return ;;
  esac
  real="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f6 || true)"
  for protected in "$real" "${INSTALL_TEST_ORIGINAL_HOME:-}" \
    "${INSTALL_TEST_ORIGINAL_XDG_CONFIG_HOME:-}" "${INSTALL_TEST_ORIGINAL_XDG_DATA_HOME:-}"; do
    [ -n "$protected" ] || continue
    protected="$(realpath -m -- "$protected")"
    [ "$protected" != / ] || continue
    case "$home/" in
      "$protected"/*) continue ;;
    esac
    case "$resolved/" in
      "$protected"/*) echo "$name ($value) is inside $protected, a real home or config directory"; return ;;
    esac
  done
}

# Exits 2 with an explanation unless HOME is a throwaway directory the
# harness made: set, marked, not the user's real home (the passwd entry's, or
# the HOME the harness started with, INSTALL_TEST_ORIGINAL_HOME), not / and
# not the checkout or a directory above it; and unless XDG_CONFIG_HOME and
# XDG_DATA_HOME are set inside it (install_test_xdg_reason).
require_disposable_home() {
  local reason="" home="${HOME:-}" resolved real
  real="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f6 || true)"
  if [ -z "$home" ]; then
    reason="HOME is not set"
  elif ! resolved="$(cd "$home" 2>/dev/null && pwd -P)"; then
    reason="HOME ($home) is not a directory"
  elif [ "$resolved" = / ]; then
    reason="HOME is /"
  elif [ -n "$real" ] && [ "$resolved" = "$(cd "$real" 2>/dev/null && pwd -P || echo "$real")" ]; then
    reason="HOME ($home) is your real home directory"
  elif [ -n "${INSTALL_TEST_ORIGINAL_HOME:-}" ] \
    && [ "$resolved" = "$(cd "$INSTALL_TEST_ORIGINAL_HOME" 2>/dev/null && pwd -P || echo "$INSTALL_TEST_ORIGINAL_HOME")" ]; then
    reason="HOME ($home) is the HOME the test started with"
  else
    case "$INSTALL_TEST_HOME_REPO/" in
      "$resolved"/*) reason="HOME ($home) holds this checkout" ;;
    esac
  fi
  if [ -z "$reason" ] && { [ ! -f "$home/$INSTALL_TEST_HOME_MARKER" ] || [ -L "$home/$INSTALL_TEST_HOME_MARKER" ]; }; then
    reason="HOME ($home) has no $INSTALL_TEST_HOME_MARKER marker"
  fi
  [ -n "$reason" ] || reason="$(install_test_xdg_reason XDG_CONFIG_HOME "$(realpath -m -- "$resolved")")"
  [ -n "$reason" ] || reason="$(install_test_xdg_reason XDG_DATA_HOME "$(realpath -m -- "$resolved")")"
  [ -z "$reason" ] && return 0
  {
    echo "refusing to run the installer test: $reason."
    echo "The installer tests delete the installed cores and the core's data under"
    echo "HOME, and write and delete the systemd user unit under XDG_CONFIG_HOME; run"
    echo "against a real HOME or XDG_CONFIG_HOME they destroy a live Agent Relay"
    echo "install, its service unit, its session history and its delivery queue. Run"
    echo "scripts/test-linux-install.sh, which makes a throwaway HOME with mktemp,"
    echo "marks it with $INSTALL_TEST_HOME_MARKER and sets XDG_CONFIG_HOME and"
    echo "XDG_DATA_HOME inside it; never run or source a test fragment against your"
    echo "own HOME."
  } >&2
  exit 2
}

# The top of every fragment the harness sources: refuses to run the fragment
# on its own, then checks HOME.
install_test_fragment() {
  if [ "${BASH_SOURCE[1]}" = "$0" ]; then
    echo "this file is a fragment sourced by scripts/test-linux-install.sh; run that instead" >&2
    exit 2
  fi
  require_disposable_home
}

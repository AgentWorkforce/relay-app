#!/usr/bin/env bash
# Reproduces how a Claude Code token refresh withdraws Agent Relay's Keychain
# access, on a disposable item.
#
# Usage (macOS only):
#   scripts/dev/keychain-partition-repro.sh --password <disposable> [--trusted-app <path>]
#   scripts/dev/keychain-partition-repro.sh --login-keychain [--trusted-app <path>]
#
#   --password <p>       required without --login-keychain: password for the
#                        temporary keychain this script creates; make one up,
#                        it is thrown away. Not accepted with --login-keychain
#                        (usage error): the login keychain password never goes
#                        on a command line, so the security tool asks for it
#                        itself, at most once, when setting the partition list.
#                        Never prompted for by this script.
#   --login-keychain     opt in to the run that can reproduce: a disposable
#                        item in the default (login) keychain, under the
#                        service name "Claude Code-credentials-repro-<pid>",
#                        never "Claude Code-credentials". Without it the run
#                        uses a temporary keychain.
#   --trusted-app <path> optional app trusted (-T) when the item is created,
#                        standing in for Agent Relay's Always Allow (for
#                        example the agent-relay-probe helper inside
#                        Agent Relay.app).
#
# Which run to use: on a keychain made with `security create-keychain` the
# partition list never materializes (set-generic-password-partition-list
# exits 0 with empty stderr and the dump keeps three access entries), so on
# current macOS the default run stops at SETUP FAILED (exit 3) by design.
# The --login-keychain run is the one that can reproduce.
#
# Guarantees, default run: every `security` call names the temporary
# keychain by explicit path; the login keychain is never read or written;
# the keychain search list and the default keychain are never changed (no
# `list-keychains -s`, no `default-keychain -s`); the temporary keychain is
# deleted on exit, also on failure. Note that `security create-keychain`
# itself appends the new keychain to the user search list until
# `delete-keychain` removes it at exit; this script never sets the list.
#
# Guarantees, --login-keychain run: only the disposable item is written,
# found by its own service name, which the script checks is not Claude
# Code's; the real "Claude Code-credentials" item is never read or written;
# the disposable item is deleted on exit (`delete-generic-password -s
# <service> -a <account>`), also on failure; the search list and the default
# keychain are never changed.
#
# Both runs: the stored secrets are fixed synthetic placeholders, never a
# real credential, and are never printed; no step changes an existing
# access list, so nothing raises an access dialog.
#
# Steps:
#   1. add the item the way Claude Code does (`add-generic-password -U -s
#      <service> -a "$USER"`), with its access list set at creation:
#      -T /usr/bin/security, plus -T <trusted app> if given;
#   2. read the item once through the security tool (`find-generic-password
#      -w`, output discarded), since the partition list may only materialize
#      after a first decrypt;
#   3. simulate Always Allow for Agent Relay: set the partition list, once, to
#      apple-tool:,apple:,teamid:QUJ7SA6X8X;
#   4. dump the item's access list and partition list (BEFORE) and check that
#      the partition list holds teamid:QUJ7SA6X8X; stop with SETUP FAILED if
#      it does not;
#   5. rewrite the item the way a token refresh does (`add-generic-password -U`
#      with a different placeholder and no -T);
#   6. dump again (AFTER) and print the BEFORE/AFTER difference.
#
# The partition list is printed by `dump-keychain -a` as the
# ACLAuthorizationPartitionID entry's description, a hex-encoded plist; hex
# descriptions are decoded before they are read (others are used as is). On
# the login keychain `dump-keychain -a` may print no access lists at all, so
# the dump also includes the item's own `find-generic-password` output.
#
# A confirming result: BEFORE's partition list contains teamid:QUJ7SA6X8X and
# AFTER's does not (only apple-tool:), while the trusted applications stay the
# same. Agent Relay's Security framework read then fails with -25293
# (errSecAuthFailed) although its Always Allow is still in the access list,
# and /usr/bin/security (apple-tool:) can still read the item.
#
# Exit status:
#   0  the run completed: the output ends with CONFIRMED or NOT REPRODUCED
#   1  not macOS, the trusted app does not exist, or xxd is missing
#   2  usage
#   3  SETUP FAILED: the simulated grant did not materialize a partition list
#      holding teamid:QUJ7SA6X8X, so BEFORE is not a valid starting state
#      (expected for the default run on current macOS, see above)
set -euo pipefail

REAL_SERVICE="Claude Code-credentials"
TEAM_PARTITION="teamid:QUJ7SA6X8X"
PLACEHOLDER="synthetic-placeholder-not-a-credential"
PLACEHOLDER_AFTER="synthetic-placeholder-after-refresh"

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//' >&2
  exit 2
}

password=""
password_given=0
trusted_app=""
login_keychain=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --password) [ "$#" -ge 2 ] || usage; password="$2"; password_given=1; shift 2 ;;
    --trusted-app) [ "$#" -ge 2 ] || usage; trusted_app="$2"; shift 2 ;;
    --login-keychain) login_keychain=1; shift ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
if [ "$login_keychain" = 1 ]; then
  # The login keychain password would show in this process's and the tool's
  # arguments; the tool asks for it instead.
  [ "$password_given" = 0 ] || usage
else
  [ -n "$password" ] || usage
fi
[ "$(uname -s)" = "Darwin" ] || { echo "error: macOS only" >&2; exit 1; }
if [ -n "$trusted_app" ] && [ ! -e "$trusted_app" ]; then
  echo "error: --trusted-app $trusted_app does not exist" >&2
  exit 1
fi
command -v xxd >/dev/null || { echo "error: xxd is needed to decode the partition list" >&2; exit 1; }
account="${USER:-$(id -un)}"

# keychain_args names the keychain every item call works on: the temporary
# keychain by path, or nothing (the default keychain) with --login-keychain.
keychain_args=()
if [ "$login_keychain" = 1 ]; then
  SERVICE="$REAL_SERVICE-repro-$$"
  if [ "$SERVICE" = "$REAL_SERVICE" ]; then
    echo "error: refusing to touch the real $REAL_SERVICE item" >&2
    exit 1
  fi
  # Read only (no -s): the path the dump reads, never changed.
  keychain="$(security default-keychain -d user | sed -e 's/^[[:space:]]*"//' -e 's/"[[:space:]]*$//')"
  [ -f "$keychain" ] || { echo "error: no default keychain found" >&2; exit 1; }
  cleanup() {
    security delete-generic-password -s "$SERVICE" -a "$account" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT INT TERM HUP
else
  SERVICE="$REAL_SERVICE"
  workdir="$(mktemp -d "${TMPDIR:-/tmp}/keychain-partition-repro.XXXXXX")"
  keychain="$workdir/repro.keychain-db"
  keychain_args=("$keychain")
  cleanup() {
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf "$workdir"
  }
  trap cleanup EXIT INT TERM HUP

  security create-keychain -p "$password" "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  security set-keychain-settings "$keychain" # no auto-lock during the run
fi

# 1. The item Claude Code writes, through the security tool. Its access list
# is set here, at creation, and never changed afterwards: changing an existing
# item's access list (a second add-generic-password -U with -T) is a ChangeACL
# that makes securityd show a dialog.
trusted=(-T /usr/bin/security)
if [ -n "$trusted_app" ]; then
  trusted+=(-T "$trusted_app")
fi
security add-generic-password -U -s "$SERVICE" -a "$account" -w "$PLACEHOLDER" \
  "${trusted[@]}" ${keychain_args[@]+"${keychain_args[@]}"}

# 2. One decrypt through the security tool, which the access list trusts. The
# output is the placeholder and is discarded. That the partition list only
# materializes after a first decrypt is a hypothesis, not a known fact; the
# check after step 4 is what tells whether this ordering works.
security find-generic-password -s "$SERVICE" -a "$account" -w ${keychain_args[@]+"${keychain_args[@]}"} >/dev/null

# 3. Always Allow, as macOS records it for a Developer ID app of team
# QUJ7SA6X8X: the partition list, set exactly once. The temporary keychain's
# made-up password is passed with -k. With --login-keychain no password is
# passed (--password is refused there), so the tool may ask for the login
# keychain password itself.
key_args=()
if [ -n "$password" ]; then
  key_args=(-k "$password")
fi
security set-generic-password-partition-list -S "apple-tool:,apple:,$TEAM_PARTITION" \
  -s "$SERVICE" -a "$account" ${key_args[@]+"${key_args[@]}"} ${keychain_args[@]+"${keychain_args[@]}"} >/dev/null

# `dump-keychain -a` of the keychain. The login keychain holds the person's
# other items, so with --login-keychain only the disposable item's record is
# kept (records start at a `keychain:` line).
dump_keychain() {
  if [ "$login_keychain" = 0 ]; then
    security dump-keychain -a "$keychain" 2>&1
    return
  fi
  security dump-keychain -a "$keychain" 2>&1 | awk -v svce="\"svce\"<blob>=\"$SERVICE\"" '
    /^keychain: / { if (keep) printf "%s", record; record = ""; keep = 0 }
    { record = record $0 "\n"; line = $0; sub(/^[ \t]+/, "", line); if (line == svce) keep = 1 }
    END { if (keep) printf "%s", record }'
}

# The item's attributes and access list, never its secret (-a without -d/-w
# dumps access lists; secrets are not printed). Hex descriptions (the
# partition entry's plist) are decoded onto one line; others are kept.
dump() {
  local line value
  {
    dump_keychain
    security find-generic-password -s "$SERVICE" -a "$account" ${keychain_args[@]+"${keychain_args[@]}"} 2>&1
  } | while IFS= read -r line; do
    value="${line#*description: }"
    if [ "$value" != "$line" ] && [ -n "$value" ] && [ $((${#value} % 2)) -eq 0 ] \
      && [[ "$value" =~ ^[0-9A-Fa-f]+$ ]]; then
      printf '%sdescription: %s\n' "${line%%description: *}" \
        "$(printf '%s' "$value" | xxd -r -p | tr '\n\t' '  ')"
    else
      printf '%s\n' "$line"
    fi
  done
}

# The partition list from a decoded dump: the IDs in the descriptions (the
# ACLAuthorizationPartitionID entry's plist holds the Partitions array).
partitions() {
  grep -E '^[[:space:]]*description: ' <<<"$1" \
    | grep -Eo '(apple-tool:|apple:|teamid:[A-Z0-9]+|cdhash:[0-9a-f]+|codesign:)' | sort -u || true
}
trusted_apps() {
  grep -E '^[[:space:]]+[0-9]+: (/|group://|<)' <<<"$1" | sed -e 's/^[[:space:]]*[0-9]*: //' | sort -u || true
}

# 4. BEFORE, and the check that the grant took hold.
before="$(dump)"
echo "=== BEFORE (after simulated Always Allow) ==="
printf '%s\n' "$before"
echo
if ! partitions "$before" | grep -qx "$TEAM_PARTITION"; then
  echo "SETUP FAILED: the simulated grant did not materialize a partition list"
  exit 3
fi

# 5. A token refresh: Claude Code writes the item again through the security
# tool, with a new secret and no -T.
security add-generic-password -U -s "$SERVICE" -a "$account" -w "$PLACEHOLDER_AFTER" \
  ${keychain_args[@]+"${keychain_args[@]}"}

# 6. AFTER and the difference.
after="$(dump)"
echo "=== AFTER (after simulated token refresh) ==="
printf '%s\n' "$after"
echo
echo "=== Partition list: BEFORE vs AFTER ==="
if diff <(partitions "$before") <(partitions "$after"); then echo "(no change)"; fi
echo
echo "=== Trusted applications: BEFORE vs AFTER ==="
if diff <(trusted_apps "$before") <(trusted_apps "$after"); then echo "(no change)"; fi
echo
if ! partitions "$after" | grep -qx "$TEAM_PARTITION"; then
  echo "CONFIRMED: the rewrite dropped $TEAM_PARTITION from the partition list."
else
  echo "NOT REPRODUCED: $TEAM_PARTITION is still in the partition list after the rewrite."
fi

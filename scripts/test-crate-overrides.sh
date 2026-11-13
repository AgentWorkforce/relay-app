#!/usr/bin/env bash
# apply-crate-overrides.sh on a copy of the four Cargo files, with a
# stand-in cargo that edits the lockfile the way cargo update would: no
# override changes nothing and runs no cargo; a rev is written into both
# manifests and both lockfiles; a version or tag cargo cannot resolve, in
# either workspace, and a lockfile the record cannot read fail with a plain
# message and leave every file as it was; a second run is a no-op. A
# relayscrubber version is refused in an explicit fixture whose manifests do
# not depend on it, and applied to both workspaces once they do.
set -euo pipefail
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPTS/.." && pwd)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tools="$work/tools"
mkdir -p "$tools"
failed=0
fail() { echo "FAIL: $1" >&2; failed=1; }

# cargo update --manifest-path M -p CRATE [--precise V]: logs its arguments,
# then rewrites CRATE's entry in M's lockfile. A tag resolves to a fixed
# commit and the tag no-such-tag does not exist; relaycast 99.0.0 does not
# exist, 98.0.0 resolves in desktop-core but not in gtk, and 97.0.0 resolves
# but leaves agent-relay-node without a git source (as a path [patch] would).
cat > "$tools/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$CARGO_LOG"
manifest=""; crate=""; precise=""
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path) manifest="$2"; shift 2 ;;
    -p) crate="$2"; shift 2 ;;
    --precise) precise="$2"; shift 2 ;;
    *) shift ;;
  esac
done
lock="$(dirname "$manifest")/Cargo.lock"
workspace="$(basename "$(dirname "$manifest")")"
registry_only=false
case "$crate" in
  registry+https://github.com/rust-lang/crates.io-index#relaycast@*)
    crate=relaycast; registry_only=true ;;
  registry+https://github.com/rust-lang/crates.io-index#relayscrubber@*)
    crate=relayscrubber ;;
esac
case "$crate" in
  agent-relay-node)
    line="$(grep '^agent-relay-node = ' "$manifest")"
    if [[ "$line" =~ (rev|tag)\ =\ \"([^\"]+)\" ]]; then
      key="${BASH_REMATCH[1]}"; value="${BASH_REMATCH[2]}"
    else
      echo "error: no ref" >&2; exit 101
    fi
    if [ "$value" = no-such-tag ]; then
      echo "error: failed to find tag \`no-such-tag\`" >&2
      exit 101
    fi
    commit="$value"
    [ "$key" = rev ] || commit=1111111111111111111111111111111111111111
    source="git+https://github.com/AgentWorkforce/relay?$key=$value#$commit"
    awk -v source="$source" '
      /^name = / { name = $3 }
      name == "\"agent-relay-node\"" && /^source = / { $0 = "source = \"" source "\"" }
      { print }' "$lock" > "$lock.new" ;;
  relaycast)
    if [ "$precise" = 99.0.0 ] || { [ "$precise" = 98.0.0 ] && [ "$workspace" = gtk ]; }; then
      echo "error: failed to select a version for the requirement \`relaycast = \"=$precise\"\`" >&2
      exit 101
    fi
    no_source=false
    [ "$precise" != 97.0.0 ] || no_source=true
    awk -v version="$precise" -v no_source="$no_source" -v registry_only="$registry_only" '
      function emit( i, selected, line) {
        selected = name == "\"relaycast\"" && (registry_only != "true" || source == "\"registry+https://github.com/rust-lang/crates.io-index\"")
        for (i = 1; i <= count; i++) {
          line = lines[i]
          if (selected && line ~ /^version = /) line = "version = \"" version "\""
          if (name == "\"agent-relay-node\"" && line ~ /^source = / && no_source == "true") continue
          print line
        }
      }
      /^\[\[package\]\]/ { emit(); count = 0; name = ""; source = "" }
      /^name = / { name = $3 }
      /^source = / { source = $3 }
      { lines[++count] = $0 }
      END { emit() }' "$lock" > "$lock.new" ;;
  relayscrubber)
    awk -v version="$precise" '
      /^name = / { name = $3 }
      name == "\"relayscrubber\"" && /^version = / { $0 = "version = \"" version "\"" }
      { print }' "$lock" > "$lock.new" ;;
  *) echo "error: unexpected package $crate" >&2; exit 101 ;;
esac
mv "$lock.new" "$lock"
CARGO
chmod +x "$tools/cargo"
export PATH="$tools:$PATH"
export CARGO_LOG="$work/cargo.log"

files=(desktop-core/Cargo.toml desktop-core/Cargo.lock gtk/Cargo.toml gtk/Cargo.lock)
fresh() {
  rm -rf "$work/tree" "$CARGO_LOG"
  : > "$CARGO_LOG"
  local file
  for file in "${files[@]}"; do
    mkdir -p "$work/tree/$(dirname "$file")"
    cp "$REPO/$file" "$work/tree/$file"
  done
}
unchanged() {
  local file
  for file in "${files[@]}"; do
    cmp -s "$REPO/$file" "$work/tree/$file" || { fail "$1: $file changed"; return; }
  done
}
apply() { bash "$SCRIPTS/apply-crate-overrides.sh" --repo-root "$work/tree" "$@"; }
pin_rev="$(awk '/^name = "agent-relay-node"/ { found = 1 } found && /^source = / { sub(/.*#/, ""); sub(/"$/, ""); print; exit }' "$REPO/desktop-core/Cargo.lock")"
pin_version="$(awk '/^name = "relaycast"/ { found = 1 } found && /^version = / { gsub(/"/, "", $3); print $3; exit }' "$REPO/desktop-core/Cargo.lock")"
pin_scrubber_version="$(awk '/^name = "relayscrubber"/ { found = 1 } found && /^version = / { gsub(/"/, "", $3); print $3; exit }' "$REPO/desktop-core/Cargo.lock")"
pinned="{\"agent-relay-node\":{\"rev\":\"$pin_rev\"},\"relaycast\":{\"version\":\"$pin_version\"},\"relayscrubber\":{\"version\":\"$pin_scrubber_version\"}}"

# No input: nothing changes, cargo is not run, and the record is the pins.
fresh
apply --json "$work/none.json" 2>/dev/null || fail "no input failed"
unchanged "no input"
[ ! -s "$CARGO_LOG" ] || fail "no input ran cargo: $(cat "$CARGO_LOG")"
[ "$(cat "$work/none.json")" = "$pinned" ] || fail "no input recorded $(cat "$work/none.json"), not $pinned"

# The pinned version is no override either.
fresh
apply --relaycast-version "$pin_version" --json "$work/pinned.json" 2>/dev/null || fail "the pinned version failed"
unchanged "the pinned version"
[ ! -s "$CARGO_LOG" ] || fail "the pinned version ran cargo: $(cat "$CARGO_LOG")"
[ "$(cat "$work/pinned.json")" = "$pinned" ] || fail "the pinned version recorded $(cat "$work/pinned.json")"

# A rev is written into both manifests and resolved in both lockfiles.
rev=0123456789abcdef0123456789abcdef01234567
fresh
apply --agent-relay-node-rev "$rev" --json "$work/rev.json" 2>/dev/null || fail "the rev override failed"
for manifest in desktop-core/Cargo.toml gtk/Cargo.toml; do
  grep -q "^agent-relay-node = { git = \"https://github.com/AgentWorkforce/relay\", rev = \"$rev\", default-features = false }$" "$work/tree/$manifest" \
    || fail "the rev is not in $manifest: $(grep '^agent-relay-node' "$work/tree/$manifest")"
done
for lock in desktop-core/Cargo.lock gtk/Cargo.lock; do
  grep -qF "source = \"git+https://github.com/AgentWorkforce/relay?rev=$rev#$rev\"" "$work/tree/$lock" || fail "the rev is not in $lock"
done
[ "$(grep -c -- '-p agent-relay-node' "$CARGO_LOG")" = 2 ] || fail "the rev override ran: $(cat "$CARGO_LOG")"
! grep -q -- '-p relaycast' "$CARGO_LOG" || fail "the rev override updated relaycast"
cmp -s "$REPO/desktop-core/Cargo.lock" "$work/tree/desktop-core/Cargo.lock" && fail "desktop-core/Cargo.lock did not change"
[ "$(cat "$work/rev.json")" = "{\"agent-relay-node\":{\"rev\":\"$rev\"},\"relaycast\":{\"version\":\"$pin_version\"},\"relayscrubber\":{\"version\":\"$pin_scrubber_version\"}}" ] \
  || fail "the rev override recorded $(cat "$work/rev.json")"
# The same rev again is a no-op.
cp -R "$work/tree" "$work/after-rev"
: > "$CARGO_LOG"
apply --agent-relay-node-rev "$rev" 2>/dev/null || fail "the rev override's second run failed"
[ ! -s "$CARGO_LOG" ] || fail "the second run ran cargo: $(cat "$CARGO_LOG")"
diff -r "$work/after-rev" "$work/tree" >/dev/null || fail "the second run changed the tree"

# A tag is written as a tag; the record holds the commit it resolved to.
fresh
apply --agent-relay-node-rev v4.2.0 --json "$work/tag.json" 2>/dev/null || fail "the tag override failed"
grep -q '^agent-relay-node = .* tag = "v4.2.0", default-features = false }$' "$work/tree/gtk/Cargo.toml" || fail "the tag is not in gtk/Cargo.toml"
grep -q '"rev":"1111111111111111111111111111111111111111"' "$work/tag.json" || fail "the tag override recorded $(cat "$work/tag.json")"

# A version that resolves: an exact requirement, both lockfiles.
fresh
apply --relaycast-version 10.0.1 --json "$work/version.json" 2>/dev/null || fail "the version override failed"
grep -qx 'relaycast = "=10.0.1"' "$work/tree/desktop-core/Cargo.toml" || fail "desktop-core/Cargo.toml does not require relaycast =10.0.1"
cmp -s "$REPO/gtk/Cargo.toml" "$work/tree/gtk/Cargo.toml" || fail "the version override changed gtk/Cargo.toml"
[ "$(grep -c -- '-p registry+https://github.com/rust-lang/crates.io-index#relaycast@.* --precise 10.0.1' "$CARGO_LOG")" = 2 ] || fail "the version override ran: $(cat "$CARGO_LOG")"
grep -q '"relaycast":{"version":"10.0.1"}' "$work/version.json" || fail "the version override recorded $(cat "$work/version.json")"

# A core can pin its own Git SDK at the same version as the app's registry
# dependency. Updating the app must keep that distinct package byte-for-byte.
git_sdk() {
  awk '
    function emit( i) { if (name == "\"relaycast\"" && source ~ /^"git\+/) for (i=1; i<=count; i++) print lines[i] }
    /^\[\[package\]\]/ { emit(); count=0; name=""; source="" }
    /^name = / { name=$3 }
    /^source = / { source=$3 }
    { lines[++count]=$0 }
    END { emit() }
  ' "$1"
}
for lock in desktop-core/Cargo.lock gtk/Cargo.lock; do
  [ "$(git_sdk "$REPO/$lock")" = "$(git_sdk "$work/tree/$lock")" ] || fail "the registry override changed the core Git SDK in $lock"
done

# A version cargo cannot resolve fails plainly and restores every file.
fresh
if apply --relaycast-version 99.0.0 --json "$work/bad.json" 2>"$work/bad.err"; then
  fail "an unresolvable version succeeded"
fi
grep -q 'cargo could not resolve desktop-core/Cargo.lock with relaycast 99.0.0; the Cargo.toml and Cargo.lock files were left as they were' "$work/bad.err" \
  || fail "an unresolvable version said: $(cat "$work/bad.err")"
unchanged "an unresolvable version"
[ ! -e "$work/bad.json" ] || fail "an unresolvable version wrote a record"

# A failure in gtk, after desktop-core's lockfile, both manifests and gtk's
# own lockfile were written, restores every file.
fresh
if apply --agent-relay-node-rev "$rev" --relaycast-version 98.0.0 --json "$work/gtk.json" 2>"$work/gtk.err"; then
  fail "a version gtk cannot resolve succeeded"
fi
grep -q 'cargo could not resolve gtk/Cargo.lock with relaycast 98.0.0; the Cargo.toml and Cargo.lock files were left as they were' "$work/gtk.err" \
  || fail "a version gtk cannot resolve said: $(cat "$work/gtk.err")"
[ "$(grep -c -- '--manifest-path' "$CARGO_LOG")" = 4 ] || fail "the gtk failure ran: $(cat "$CARGO_LOG")"
unchanged "a version gtk cannot resolve"
[ ! -e "$work/gtk.json" ] || fail "a version gtk cannot resolve wrote a record"

# A tag cargo cannot find fails plainly and restores every file.
fresh
if apply --agent-relay-node-rev no-such-tag 2>"$work/tag.err"; then
  fail "an unknown tag succeeded"
fi
grep -q 'cargo could not resolve desktop-core/Cargo.lock with agent-relay-node tag no-such-tag; the Cargo.toml and Cargo.lock files were left as they were' "$work/tag.err" \
  || fail "an unknown tag said: $(cat "$work/tag.err")"
unchanged "an unknown tag"

# A lockfile the record cannot read, after cargo resolved it, restores every
# file too.
fresh
if apply --relaycast-version 97.0.0 --json "$work/record.json" 2>"$work/record.err"; then
  fail "an unreadable record succeeded"
fi
grep -q 'holds 0 entries for agent-relay-node' "$work/record.err" || fail "an unreadable record said: $(cat "$work/record.err")"
grep -q 'the Cargo.toml and Cargo.lock files were left as they were' "$work/record.err" \
  || fail "an unreadable record did not say the files were restored: $(cat "$work/record.err")"
unchanged "an unreadable record"
[ ! -e "$work/record.json" ] || fail "an unreadable record wrote a record"

# relayscrubber while neither manifest depends on it: refused, never ignored.
fresh
for workspace in desktop-core gtk; do
  sed -i '/^relayscrubber = /d' "$work/tree/$workspace/Cargo.toml"
done
cp -R "$work/tree" "$work/scrubber-no-dependency"
if apply --relayscrubber-version 1.0.0 --json "$work/scrubber.json" 2>"$work/scrubber.err"; then
  fail "a relayscrubber version without the dependency succeeded"
fi
grep -q "relayscrubber version '1.0.0' was given but desktop-core/Cargo.toml and gtk/Cargo.toml do not both depend on relayscrubber" "$work/scrubber.err" \
  || fail "a relayscrubber version without the dependency said: $(cat "$work/scrubber.err")"
[ ! -s "$CARGO_LOG" ] || fail "a relayscrubber version without the dependency ran cargo"
diff -r "$work/scrubber-no-dependency" "$work/tree" >/dev/null \
  || fail "a relayscrubber version without the dependency changed the fixture"
[ ! -e "$work/scrubber.json" ] || fail "a relayscrubber version without the dependency wrote a record"

# Once both workspaces depend on it, the version is required exactly in both
# manifests, both lockfiles move to it, and the record names it.
pin_scrubber() {
  local workspace
  for workspace in desktop-core gtk; do
    sed -i 's/^relayscrubber = "=[^"]*"$/relayscrubber = "=1.0.0"/' "$work/tree/$workspace/Cargo.toml"
    awk '
      /^name = / { name = $3 }
      name == "\"relayscrubber\"" && /^version = / { $0 = "version = \"1.0.0\"" }
      { print }' "$work/tree/$workspace/Cargo.lock" > "$work/tree/$workspace/Cargo.lock.new"
    mv "$work/tree/$workspace/Cargo.lock.new" "$work/tree/$workspace/Cargo.lock"
  done
}
fresh
pin_scrubber
apply --json "$work/scrubber-pinned.json" 2>/dev/null || fail "a pinned relayscrubber without an override failed"
[ ! -s "$CARGO_LOG" ] || fail "a pinned relayscrubber without an override ran cargo"
[ "$(cat "$work/scrubber-pinned.json")" = "{\"agent-relay-node\":{\"rev\":\"$pin_rev\"},\"relaycast\":{\"version\":\"$pin_version\"},\"relayscrubber\":{\"version\":\"1.0.0\"}}" ] \
  || fail "a pinned relayscrubber recorded $(cat "$work/scrubber-pinned.json")"
apply --relayscrubber-version 1.2.3 --json "$work/scrubber-version.json" 2>/dev/null || fail "the relayscrubber override failed"
for workspace in desktop-core gtk; do
  grep -qx 'relayscrubber = "=1.2.3"' "$work/tree/$workspace/Cargo.toml" || fail "$workspace/Cargo.toml does not require relayscrubber =1.2.3"
  grep -qx 'version = "1.2.3"' "$work/tree/$workspace/Cargo.lock" || fail "$workspace/Cargo.lock does not resolve relayscrubber 1.2.3"
done
[ "$(grep -c -- '-p registry+https://github.com/rust-lang/crates.io-index#relayscrubber@1.0.0 --precise 1.2.3' "$CARGO_LOG")" = 2 ] \
  || fail "the relayscrubber override ran: $(cat "$CARGO_LOG")"
grep -q '"relayscrubber":{"version":"1.2.3"}' "$work/scrubber-version.json" || fail "the relayscrubber override recorded $(cat "$work/scrubber-version.json")"

# Malformed input is refused before cargo runs.
for input in "--relaycast-version 10.0" "--relaycast-version ^10.0.0" "--relayscrubber-version 1.0" "--agent-relay-node-rev 54a2a06" \
  "--agent-relay-node-rev ../main" "--agent-relay-node-rev a&b"; do
  fresh
  # shellcheck disable=SC2086 # each input is an option and its value
  if apply $input 2>"$work/malformed.err"; then fail "$input was accepted"; fi
  grep -q '^apply-crate-overrides: ' "$work/malformed.err" || fail "$input said: $(cat "$work/malformed.err")"
  [ ! -s "$CARGO_LOG" ] || fail "$input ran cargo"
  unchanged "$input"
done

[ "$failed" = 0 ] && echo "apply-crate-overrides.sh: ok"
exit "$failed"

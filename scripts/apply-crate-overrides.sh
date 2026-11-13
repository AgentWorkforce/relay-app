#!/usr/bin/env bash
# Override the versions of Desktop's core crates for an internal build, and
# record the versions the build resolves.
#
#   scripts/apply-crate-overrides.sh [--agent-relay-node-rev REV]
#     [--relaycast-version VERSION] [--relayscrubber-version VERSION]
#     [--json FILE] [--repo-root DIR]
#
# --agent-relay-node-rev  a full 40-character commit sha or a tag in
#                         AgentWorkforce/relay. It replaces the rev (or tag)
#                         of agent-relay-node in desktop-core/Cargo.toml and
#                         gtk/Cargo.toml, and both lockfiles are updated for
#                         that crate alone (cargo update -p agent-relay-node).
# --relaycast-version     an exact crates.io version. desktop-core/Cargo.toml
#                         requires exactly it and both lockfiles move to it
#                         (cargo update -p relaycast --precise VERSION).
# --relayscrubber-version an exact crates.io version of relayscrubber, the
#                         session-body scrub engine. Both desktop-core/Cargo.toml
#                         and gtk/Cargo.toml must already depend on it; until
#                         the crate is pinned the script refuses the input
#                         rather than ignore it. Both manifests require exactly
#                         it and both lockfiles move to it.
# --json FILE             writes the versions both lockfiles resolve, as
#                         {"agent-relay-node":{"rev":SHA},"relaycast":{"version":V}},
#                         plus "relayscrubber":{"version":V} once the crate is
#                         pinned. Without an override these are the pinned values.
#
# With no override the tree is left as it is. A value the tree already
# resolves changes nothing and does not run cargo, so a second run with the
# same values is a no-op. When cargo cannot resolve a lockfile the script
# fails, says which, and puts all four files back as they were.
#
# The Desktop Release workflow runs this before the builds of an internal
# prepare-only dispatch (docs/workspace-layout.md, "Crate overrides"); it
# runs scripts/generate-contract.sh itself afterwards when the rev changed.
set -euo pipefail

usage() {
  sed -n '4,7p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

die() {
  printf 'apply-crate-overrides: %s\n' "$1" >&2
  exit 1
}

node_rev=""
relaycast_version=""
relayscrubber_version=""
json_out=""
repo_root="$(cd "$(dirname "$0")/.." && pwd)"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --agent-relay-node-rev) [ "$#" -ge 2 ] || usage; node_rev="$2"; shift 2 ;;
    --relaycast-version) [ "$#" -ge 2 ] || usage; relaycast_version="$2"; shift 2 ;;
    --relayscrubber-version) [ "$#" -ge 2 ] || usage; relayscrubber_version="$2"; shift 2 ;;
    --json) [ "$#" -ge 2 ] || usage; json_out="$2"; shift 2 ;;
    --repo-root) [ "$#" -ge 2 ] || usage; repo_root="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) printf 'apply-crate-overrides: unknown option %s\n' "$1" >&2; usage ;;
  esac
done

cargo_bin="${CARGO:-cargo}"
core_manifest="$repo_root/desktop-core/Cargo.toml"
gtk_manifest="$repo_root/gtk/Cargo.toml"
core_lock="$repo_root/desktop-core/Cargo.lock"
gtk_lock="$repo_root/gtk/Cargo.lock"
for file in "$core_manifest" "$gtk_manifest" "$core_lock" "$gtk_lock"; do
  [ -f "$file" ] || die "missing $file"
done

# --- the requested values -----------------------------------------------------
node_key=""
if [ -n "$node_rev" ]; then
  sha_pattern='^[0-9a-f]{40}$'
  short_sha_pattern='^[0-9a-fA-F]{7,39}$'
  tag_pattern='^[A-Za-z0-9][A-Za-z0-9._/-]*$'
  if [[ "$node_rev" =~ $sha_pattern ]]; then
    node_key="rev"
  elif [[ "$node_rev" =~ $short_sha_pattern ]]; then
    die "agent-relay-node rev '$node_rev' looks like a short commit sha; give the full 40-character sha"
  elif [[ "$node_rev" =~ $tag_pattern ]] && [[ "$node_rev" != *..* ]] && [[ "$node_rev" != */ ]]; then
    node_key="tag"
  else
    die "agent-relay-node rev '$node_rev' is neither a 40-character commit sha nor a tag name"
  fi
fi
version_pattern='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
if [ -n "$relaycast_version" ]; then
  [[ "$relaycast_version" =~ $version_pattern ]] \
    || die "relaycast version '$relaycast_version' is not an exact version such as 10.0.0"
fi
if [ -n "$relayscrubber_version" ]; then
  [[ "$relayscrubber_version" =~ $version_pattern ]] \
    || die "relayscrubber version '$relayscrubber_version' is not an exact version such as 1.0.0"
fi

# True when MANIFEST declares a relayscrubber dependency.
declares_relayscrubber() {
  grep -Eq '^relayscrubber[[:space:]]*=' "$1"
}
relayscrubber_pinned=false
if declares_relayscrubber "$core_manifest" && declares_relayscrubber "$gtk_manifest"; then
  relayscrubber_pinned=true
fi
if [ -n "$relayscrubber_version" ] && [ "$relayscrubber_pinned" = false ]; then
  die "relayscrubber version '$relayscrubber_version' was given but desktop-core/Cargo.toml and gtk/Cargo.toml do not both depend on relayscrubber; pin the crate in both first (nothing was changed)"
fi

# --- what the tree holds --------------------------------------------------------
# The value of FIELD in each [[package]] of a lockfile named CRATE.
lock_field() {
  awk -v crate="$2" -v field="$3" -v required_source="${4:-}" '
    function emit() {
      if (name == crate && value != "" && (required_source == "" || source == required_source)) print value
    }
    /^\[\[package\]\]/ { emit(); name = ""; source = ""; value = "" }
    /^name = / { name = $3; gsub(/"/, "", name) }
    /^source = / { source = $3; gsub(/"/, "", source) }
    name == crate && $1 == field && $2 == "=" { value = $3; gsub(/"/, "", value) }
    END { emit() }
  ' "$1"
}

# The single entry of CRATE's FIELD in a lockfile; anything else is an error.
lock_single() {
  local values count
  values="$(lock_field "$1" "$2" "$3" "${4:-}")"
  count="$(printf '%s' "$values" | grep -c . || true)"
  [ "$count" -eq 1 ] || die "$1 holds $count entries for $2 (expected one)"
  printf '%s\n' "$values"
}

node_source() { lock_single "$1" agent-relay-node source; }
# The app overrides its direct crates.io SDK, independently of a Git SDK the
# pinned core may use. Equal version strings do not make their Rust types or
# Cargo package IDs interchangeable.
relaycast_lock_version() { lock_single "$1" relaycast version registry+https://github.com/rust-lang/crates.io-index; }
relaycast_package_spec() { printf 'registry+https://github.com/rust-lang/crates.io-index#relaycast@%s\n' "$(relaycast_lock_version "$1")"; }
relayscrubber_lock_version() { lock_single "$1" relayscrubber version registry+https://github.com/rust-lang/crates.io-index; }
relayscrubber_package_spec() { printf 'registry+https://github.com/rust-lang/crates.io-index#relayscrubber@%s\n' "$(relayscrubber_lock_version "$1")"; }

# The commit a lockfile resolves agent-relay-node to.
node_commit() {
  local source
  source="$(node_source "$1")"
  case "$source" in
    git+*'#'*) printf '%s\n' "${source##*#}" ;;
    *) die "$1 does not resolve agent-relay-node from git ($source)" ;;
  esac
}

node_current() {
  # True when both manifests name the requested ref and both locks resolve it.
  local manifest lock
  for manifest in "$core_manifest" "$gtk_manifest"; do
    awk -v wanted="$node_key = \"$node_rev\"" '
      /^agent-relay-node = \{/ && index($0, wanted) { found = 1 }
      END { exit found ? 0 : 1 }
    ' "$manifest" || return 1
  done
  for lock in "$core_lock" "$gtk_lock"; do
    case "$(node_source "$lock")" in
      *"?$node_key=$node_rev#"*) ;;
      *) return 1 ;;
    esac
  done
}

relaycast_current() {
  [ "$(relaycast_lock_version "$core_lock")" = "$relaycast_version" ] \
    && [ "$(relaycast_lock_version "$gtk_lock")" = "$relaycast_version" ]
}

relayscrubber_current() {
  [ "$(relayscrubber_lock_version "$core_lock")" = "$relayscrubber_version" ] \
    && [ "$(relayscrubber_lock_version "$gtk_lock")" = "$relayscrubber_version" ]
}

change_node=false
change_relaycast=false
change_relayscrubber=false
if [ -n "$node_rev" ] && ! node_current; then change_node=true; fi
if [ -n "$relaycast_version" ] && ! relaycast_current; then change_relaycast=true; fi
if [ -n "$relayscrubber_version" ] && ! relayscrubber_current; then change_relayscrubber=true; fi

# --- the change -----------------------------------------------------------------
backup_dir=""
restore() {
  [ -n "$backup_dir" ] || return 0
  cp "$backup_dir/core.toml" "$core_manifest"
  cp "$backup_dir/gtk.toml" "$gtk_manifest"
  cp "$backup_dir/core.lock" "$core_lock"
  cp "$backup_dir/gtk.lock" "$gtk_lock"
}
cleanup() {
  [ -z "$backup_dir" ] || rm -rf "$backup_dir"
  backup_dir=""
}
fail_restored() {
  restore
  cleanup
  die "$1; the Cargo.toml and Cargo.lock files were left as they were"
}
# Any other failure after the backup (a lockfile read that dies under set -e)
# puts the four files back too.
on_exit() {
  local status=$?
  if [ "$status" -ne 0 ] && [ -n "$backup_dir" ]; then
    restore
    printf 'apply-crate-overrides: the Cargo.toml and Cargo.lock files were left as they were\n' >&2
  fi
  cleanup
}

# Rewrite one line of a manifest through a temporary file (no sed -i: BSD and
# GNU differ). AWK_PROGRAM edits $0 and sets `changed` on the lines it edits.
rewrite_manifest() {
  local manifest="$1" program="$2" temporary
  temporary="$(mktemp "$manifest.XXXXXX")"
  if ! awk -v node_key="$node_key" -v node_rev="$node_rev" -v version="$relaycast_version" \
      -v scrubber_version="$relayscrubber_version" \
      "$program"' { print } END { exit changed ? 0 : 3 }' "$manifest" > "$temporary"; then
    rm -f "$temporary"
    return 1
  fi
  cat "$temporary" > "$manifest"
  rm -f "$temporary"
}

node_program='/^agent-relay-node = \{/ {
  if (gsub(/(rev|tag|branch) = "[^"]*"/, node_key " = \"" node_rev "\"") == 1) changed = 1
}'
# shellcheck disable=SC2016 # an awk program: $0 and $/ are awk's, not the shell's
relaycast_program='/^relaycast = "[^"]*"[[:space:]]*$/ {
  $0 = "relaycast = \"=" version "\""
  changed = 1
}'
# relayscrubber as a plain version string or as a table with a version key.
# shellcheck disable=SC2016 # an awk program: $0 is awk's, not the shell's
relayscrubber_program='/^relayscrubber[[:space:]]*=[[:space:]]*"[^"]*"[[:space:]]*$/ {
  $0 = "relayscrubber = \"=" scrubber_version "\""
  changed = 1
}
/^relayscrubber[[:space:]]*=[[:space:]]*\{/ {
  if (gsub(/version[[:space:]]*=[[:space:]]*"[^"]*"/, "version = \"=" scrubber_version "\"") == 1) changed = 1
}'

if [ "$change_node" = true ] || [ "$change_relaycast" = true ] || [ "$change_relayscrubber" = true ]; then
  backup_dir="$(mktemp -d "${TMPDIR:-/tmp}/crate-overrides.XXXXXX")"
  trap on_exit EXIT
  cp "$core_manifest" "$backup_dir/core.toml"
  cp "$gtk_manifest" "$backup_dir/gtk.toml"
  cp "$core_lock" "$backup_dir/core.lock"
  cp "$gtk_lock" "$backup_dir/gtk.lock"

  if [ "$change_node" = true ]; then
    for manifest in "$core_manifest" "$gtk_manifest"; do
      rewrite_manifest "$manifest" "$node_program" \
        || fail_restored "$manifest has no single agent-relay-node git dependency with a rev or tag to replace"
    done
  fi
  if [ "$change_relaycast" = true ]; then
    rewrite_manifest "$core_manifest" "$relaycast_program" \
      || fail_restored "$core_manifest has no plain relaycast = \"VERSION\" line to replace"
  fi
  if [ "$change_relayscrubber" = true ]; then
    for manifest in "$core_manifest" "$gtk_manifest"; do
      rewrite_manifest "$manifest" "$relayscrubber_program" \
        || fail_restored "$manifest has no relayscrubber version to replace"
    done
  fi

  for workspace in desktop-core gtk; do
    manifest="$repo_root/$workspace/Cargo.toml"
    if [ "$change_node" = true ]; then
      printf 'apply-crate-overrides: %s: agent-relay-node %s %s\n' "$workspace" "$node_key" "$node_rev" >&2
      "$cargo_bin" update --manifest-path "$manifest" -p agent-relay-node \
        || fail_restored "cargo could not resolve $workspace/Cargo.lock with agent-relay-node $node_key $node_rev"
    fi
    if [ "$change_relaycast" = true ]; then
      printf 'apply-crate-overrides: %s: relaycast %s\n' "$workspace" "$relaycast_version" >&2
      "$cargo_bin" update --manifest-path "$manifest" -p "$(relaycast_package_spec "$repo_root/$workspace/Cargo.lock")" --precise "$relaycast_version" \
        || fail_restored "cargo could not resolve $workspace/Cargo.lock with relaycast $relaycast_version"
    fi
    if [ "$change_relayscrubber" = true ]; then
      printf 'apply-crate-overrides: %s: relayscrubber %s\n' "$workspace" "$relayscrubber_version" >&2
      "$cargo_bin" update --manifest-path "$manifest" -p "$(relayscrubber_package_spec "$repo_root/$workspace/Cargo.lock")" --precise "$relayscrubber_version" \
        || fail_restored "cargo could not resolve $workspace/Cargo.lock with relayscrubber $relayscrubber_version"
    fi
  done

  if [ "$change_node" = true ] && ! node_current; then
    fail_restored "the lockfiles do not resolve agent-relay-node $node_key $node_rev after the update"
  fi
  if [ "$change_relaycast" = true ] && ! relaycast_current; then
    fail_restored "the lockfiles do not resolve relaycast $relaycast_version after the update"
  fi
  if [ "$change_relayscrubber" = true ] && ! relayscrubber_current; then
    fail_restored "the lockfiles do not resolve relayscrubber $relayscrubber_version after the update"
  fi
fi

# --- the record -----------------------------------------------------------------
core_commit="$(node_commit "$core_lock")"
gtk_commit="$(node_commit "$gtk_lock")"
core_relaycast="$(relaycast_lock_version "$core_lock")"
gtk_relaycast="$(relaycast_lock_version "$gtk_lock")"
[ "$core_commit" = "$gtk_commit" ] \
  || fail_restored "desktop-core/Cargo.lock resolves agent-relay-node $core_commit but gtk/Cargo.lock $gtk_commit"
[ "$core_relaycast" = "$gtk_relaycast" ] \
  || fail_restored "desktop-core/Cargo.lock resolves relaycast $core_relaycast but gtk/Cargo.lock $gtk_relaycast"
[[ "$core_commit" =~ ^[0-9a-f]{40}$ ]] || fail_restored "agent-relay-node resolves to '$core_commit', not a commit sha"
core_relayscrubber=""
scrubber_note=""
if [ "$relayscrubber_pinned" = true ]; then
  core_relayscrubber="$(relayscrubber_lock_version "$core_lock")"
  gtk_relayscrubber="$(relayscrubber_lock_version "$gtk_lock")"
  [ "$core_relayscrubber" = "$gtk_relayscrubber" ] \
    || fail_restored "desktop-core/Cargo.lock resolves relayscrubber $core_relayscrubber but gtk/Cargo.lock $gtk_relayscrubber"
  scrubber_note=", relayscrubber $core_relayscrubber"
fi

if [ "$change_node" = false ] && [ "$change_relaycast" = false ] && [ "$change_relayscrubber" = false ]; then
  printf 'apply-crate-overrides: no change (agent-relay-node %s, relaycast %s%s)\n' "$core_commit" "$core_relaycast" "$scrubber_note" >&2
else
  printf 'apply-crate-overrides: resolved agent-relay-node %s, relaycast %s%s\n' "$core_commit" "$core_relaycast" "$scrubber_note" >&2
fi

if [ -n "$json_out" ]; then
  mkdir -p "$(dirname "$json_out")"
  scrubber_json=""
  if [ "$relayscrubber_pinned" = true ]; then
    scrubber_json="$(printf ',"relayscrubber":{"version":"%s"}' "$core_relayscrubber")"
  fi
  printf '{"agent-relay-node":{"rev":"%s"},"relaycast":{"version":"%s"}%s}\n' \
    "$core_commit" "$core_relaycast" "$scrubber_json" > "$json_out"
fi
cleanup

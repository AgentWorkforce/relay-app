#!/usr/bin/env bash
# The `service-callers-dev-attest` preview feature (docs/service-callers.md)
# takes every same-uid local peer for the app. It is never enabled by a
# workflow, build or packaging script, flow, or a Cargo.toml: it is named
# only by its definition in desktop-core/Cargo.toml (and comments there),
# desktop-core's source, docs/, and this guard. Usage: test-dev-attest-guard.sh [ROOT]
set -euo pipefail
root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
feature="service-callers-dev-attest"

# Every line naming the feature, as path:line:text.
search() {
  if git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$root" grep --untracked -n -F -e "$feature" -- .
  else
    (cd "$root" && grep -r -n -F --exclude-dir=.git --exclude-dir=target -e "$feature" .) | sed 's|^\./||'
  fi
}
hits="$(search || true)"

failed=0
while IFS= read -r hit; do
  [ -n "$hit" ] || continue
  path="${hit%%:*}"
  rest="${hit#*:}"
  text="${rest#*:}"
  case "$path" in
    docs/* | desktop-core/src/* | scripts/test-dev-attest-guard.sh) continue ;;
    desktop-core/Cargo.toml)
      # The definition, or a comment; never a default or another feature's list.
      trimmed="${text#"${text%%[![:space:]]*}"}"
      if [ "$trimmed" = "$feature = []" ] || [ "${trimmed:0:1}" = "#" ]; then
        continue
      fi
      ;;
  esac
  printf 'enables %s outside a debug preview: %s\n' "$feature" "$hit"
  failed=1
done <<< "$hits"

if [ "$failed" = 0 ]; then
  echo "$feature: enabled nowhere"
fi
exit "$failed"

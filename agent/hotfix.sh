#!/bin/bash
# Start a hotfix from exactly what shipped, when main has moved on with changes
# that must not go out yet.
#
#   hotfix.sh <version> <slug>     e.g. hotfix.sh 2026.11.2 upload-crash
#
# Creates hotfix/<version>-<slug> from tag released-v<version>, checked out in
# ~/hotfix/<slug> (its own clone; the repo forbids worktrees), and pushes it.
# Then: cherry-pick or write the fix there, push, and run one review round,
# review.sh hotfix/<version>-<slug>; after a fix push or a rebase run
# review.sh hotfix/<version>-<slug> --ci-only --final (CI and the GitHub gate,
# no reviewers) before releasing. Then
#   release.sh prepare <beta|stable> hotfix/<version>-<slug>
#   release.sh <beta|stable> --run <candidate run id>
# Land the same fix on main too, so the next regular release keeps it.
set -euo pipefail
version=${1:?usage: hotfix.sh <version> <slug>}; slug=${2:?slug}
tag=released-v$version
branch=hotfix/$version-$slug
dir=$HOME/hotfix/$slug
# A local tag that differs from origin's makes the whole-tags fetch exit 1
# after every other ref was fetched (tick.sh and release.sh tolerate the
# same). The one tag that matters is then fetched by itself, forced, so a
# stale local copy can never stand in for origin's: that fetch failing, or
# the tag not existing on origin, stops the hotfix.
git -C "$HOME/build/relay-desktop" fetch -q origin --tags 2>/dev/null || echo "warning: tag fetch failed (a local tag differs from origin?)" >&2
git -C "$HOME/build/relay-desktop" fetch -q origin "+refs/tags/$tag:refs/tags/$tag" || { echo "no tag $tag on origin (or the fetch failed)" >&2; exit 1; }
mkdir -p "$HOME/hotfix"
[ -d "$dir/.git" ] || git clone -q --reference "$HOME/build/relay-desktop" https://github.com/AgentWorkforce/relay-desktop.git "$dir"
git -C "$dir" fetch -q origin "+refs/tags/$tag:refs/tags/$tag" || { echo "cannot fetch $tag into $dir" >&2; exit 1; }
git -C "$dir" checkout -q -B "$branch" "$tag"
git -C "$dir" push -q -u origin "$branch"
echo "hotfix branch $branch at $(git -C "$dir" rev-parse --short HEAD) in $dir"

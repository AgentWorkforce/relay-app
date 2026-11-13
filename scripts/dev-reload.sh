#!/usr/bin/env bash
#
# dev-reload.sh -- rebuild the Swift app into an existing development bundle.
#
# Takes a bundle from `build-app.sh --debug`, runs the same incremental debug
# build, rebuilds its Rust helper, and replaces both executables and SwiftPM
# resource bundles. Info.plist, icon and site URL stay as built. The bundle is re-signed
# with the identity it already carries, so Keychain access survives reloads.
#
# Cloud's `npm run dev:desktop` selects --swift-only for UI edits and relaunches the
# app; it also works by hand while the app is stopped.
#
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MACOS_DIR="$REPO_ROOT/macos"
EXECUTABLE_NAME="RelayDesktop"

usage() {
  cat <<EOF
Usage:
  ${SCRIPT_NAME} --app BUNDLE --arch arm64|x64 [--swift-only]

Options:
  --app BUNDLE       An .app produced by build-app.sh --debug. Required.
  --arch arm64|x64   Architecture the bundle was built for. Required.
  --swift-only       Keep the bundled helper. Only use when its source, build
                     inputs and generated Swift contract are unchanged.
  -h, --help         Show this help.
EOF
}

die() {
  printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2
  exit 1
}

note() {
  printf '%s: %s\n' "$SCRIPT_NAME" "$*"
}

APP_BUNDLE=""
ARCH=""
SWIFT_ONLY=false
while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      [ $# -ge 2 ] || die "--app needs a bundle path"
      APP_BUNDLE="$2"
      shift 2
      ;;
    --arch)
      [ $# -ge 2 ] || die "--arch needs a value (arm64 or x64)"
      ARCH="$2"
      shift 2
      ;;
    --swift-only)
      SWIFT_ONLY=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown argument: $1"
      ;;
  esac
done

[ -n "$APP_BUNDLE" ] || { usage >&2; die "--app is required"; }
case "$ARCH" in
  arm64) SWIFT_ARCH="arm64" ;;
  x64)   SWIFT_ARCH="x86_64" ;;
  *)     usage >&2; die "--arch must be arm64 or x64" ;;
esac
TARGET="$APP_BUNDLE/Contents/MacOS/$EXECUTABLE_NAME"
[ -f "$TARGET" ] || die "no $EXECUTABLE_NAME in $APP_BUNDLE; build it with build-app.sh --debug first"

SWIFT_FLAGS=(-c debug --arch "$SWIFT_ARCH" --package-path "$MACOS_DIR")
swift build "${SWIFT_FLAGS[@]}"
BIN_PATH="$(swift build "${SWIFT_FLAGS[@]}" --show-bin-path | tail -n 1)"
[ -f "$BIN_PATH/$EXECUTABLE_NAME" ] || die "swift build succeeded but no $EXECUTABLE_NAME is in $BIN_PATH"

APP_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$APP_BUNDLE/Contents/Info.plist")"
HELPER="$APP_BUNDLE/Contents/Helpers/$APP_NAME Helper"
[ -f "$HELPER" ] || die "no bundled helper at $HELPER"

if [ "$SWIFT_ONLY" = false ]; then
  # Rebuild the helper from this checkout as well: a freshly generated Swift
  # contract must never be paired with the helper from the initial stack launch.
  # Native reloads reuse Cargo's normal debug cache; cross builds name the target.
  #
  # The helper signs in to the site it was built for and opens links of the
  # scheme it was built with (nothing at run time changes either), so it is
  # built for the bundle's, which build-app.sh fixed when it made the bundle:
  # the site in Resources/site-url.txt and the scheme in its Info.plist
  # (production's when a bundle predates the record). The site is checked on
  # the built helper afterwards (`cloud install --help` prints it; the scheme
  # it does not).
  # shellcheck source-path=SCRIPTDIR
  # shellcheck source=site-origin.sh
  . "$SCRIPT_DIR/site-origin.sh"
  BUNDLE_SITE="https://agentrelay.com"
  if [ -f "$APP_BUNDLE/Contents/Resources/site-url.txt" ]; then
    BUNDLE_SITE="$(site_origin "$(tr -d '\n\r' < "$APP_BUNDLE/Contents/Resources/site-url.txt")")" || die "site-url.txt in the bundle is not an origin"
  fi
  BUNDLE_SCHEME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleURLTypes:0:CFBundleURLSchemes:0' "$APP_BUNDLE/Contents/Info.plist")"
  BUNDLE_SCHEME="$(url_scheme_name "$BUNDLE_SCHEME")" || die "the bundle's URL scheme is not a scheme"
  if [ "$BUNDLE_SITE" = "https://agentrelay.com" ]; then
    unset AGENT_RELAY_SITE_URL
  else
    export AGENT_RELAY_SITE_URL="$BUNDLE_SITE"
    note "building the helper for the bundle's site $BUNDLE_SITE"
  fi
  if [ "$BUNDLE_SCHEME" = "agentrelay" ]; then
    unset AGENT_RELAY_URL_SCHEME
  else
    export AGENT_RELAY_URL_SCHEME="$BUNDLE_SCHEME"
    note "building the helper with the bundle's link scheme $BUNDLE_SCHEME://"
  fi
  PROBE_TARGET_DIR="$REPO_ROOT/target"
  PROBE_FLAGS=(--locked --manifest-path "$REPO_ROOT/desktop-core/Cargo.toml" --target-dir "$PROBE_TARGET_DIR" --bin agent-relay-desktop-probe)
  PROBE_BINARY="$PROBE_TARGET_DIR/debug/agent-relay-desktop-probe"
  if [ "$(uname -m)" != "$SWIFT_ARCH" ]; then
    if [ "$ARCH" = arm64 ]; then RUST_TARGET=aarch64-apple-darwin; else RUST_TARGET=x86_64-apple-darwin; fi
    PROBE_FLAGS+=(--target "$RUST_TARGET")
    PROBE_BINARY="$PROBE_TARGET_DIR/$RUST_TARGET/debug/agent-relay-desktop-probe"
  fi
  env -u CARGO_BUILD_TARGET cargo build "${PROBE_FLAGS[@]}"
  bash "$SCRIPT_DIR/check-probe-bridge.sh" "$PROBE_BINARY" "$ARCH"
  if [ "$(uname -m)" = "$SWIFT_ARCH" ]; then
    BUILT_SITE="$(probe_help_site "$PROBE_BINARY")"
    [ -n "$BUILT_SITE" ] || die "the rebuilt helper prints no site (cloud install --help); not installing it"
    [ "$BUILT_SITE" = "$BUNDLE_SITE" ] || die "the rebuilt helper is built for $BUILT_SITE, not the bundle's $BUNDLE_SITE; not installing it"
  else
    note "the rebuilt helper is for $ARCH and does not run on this $(uname -m) host: its site is not checked here (it was built for $BUNDLE_SITE)"
  fi
  PROBE_VERSION="$("$PROBE_BINARY" --version)"
  PROBE_VERSION="${PROBE_VERSION#agent-relay-probe }"
else
  note "Swift-only reload: keeping the signed helper"
fi

# Read the identity before the executable changes and invalidates the seal.
IDENTITY="-"
AUTHORITY="$(codesign -d --verbose=2 "$APP_BUNDLE" 2>&1 | sed -n 's/^Authority=//p' | head -n 1)"
if [ -n "$AUTHORITY" ]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null |
    awk -v name="\"$AUTHORITY\"" 'index($0, name) {print $2; exit}')"
  [ -n "$IDENTITY" ] || die "the keychain no longer has '$AUTHORITY'; rebuild with build-app.sh --debug"
fi

install -m 0755 "$BIN_PATH/$EXECUTABLE_NAME" "$TARGET"
for resource_bundle in "$BIN_PATH"/*.bundle; do
  [ -e "$resource_bundle" ] || continue
  rm -rf "$APP_BUNDLE/Contents/Resources/$(basename "$resource_bundle")"
  cp -R "$resource_bundle" "$APP_BUNDLE/Contents/Resources/"
done

# Both release diagnostics and local builds identify the desktop source revision.
BUILD_REVISION="$(git -C "$MACOS_DIR" rev-parse --short=12 HEAD 2>/dev/null || printf unknown)"
printf '%s\n' "$BUILD_REVISION" > "$APP_BUNDLE/Contents/Resources/build-revision.txt"

if [ "$SWIFT_ONLY" = false ]; then
  # Refresh only the exact managed CLI copy of the old bundle. Do not replace
  # a symlink, another user's file, or a separately installed/newer CLI. Release
  # upgrades use version ordering; a dev reload also needs equal-version builds.
  CLI_COPY="$HOME/.local/bin/agent-relay-probe"
  REFRESH_CLI=false
  if [ -f "$CLI_COPY" ] && [ ! -L "$CLI_COPY" ] && [ -O "$CLI_COPY" ] && cmp -s "$CLI_COPY" "$HELPER"; then
    REFRESH_CLI=true
  fi

  # Replace atomically so a running helper keeps its old inode until it retires.
  HELPER_STAGED="$HELPER.reload"
  SESSIONS_HELPER="$HELPER (Sessions)"
  SESSIONS_STAGED="$SESSIONS_HELPER.reload"
  CLI_STAGED=""
  trap 'rm -f "$HELPER_STAGED" "$SESSIONS_STAGED" "${CLI_STAGED:-}"' EXIT
  install -m 0755 "$PROBE_BINARY" "$HELPER_STAGED"
  codesign --force --identifier agent-relay-probe --sign "$IDENTITY" "$HELPER_STAGED"
  mv -f "$HELPER_STAGED" "$HELPER"
  # Keep the collector on the same signed executable as the core, as build-app.sh
  # does. Replacing only the core leaves the old collector inode behind, so a
  # Keychain grant to the new core cannot cover its background session reads.
  # Link after signing, which can replace the file, then publish atomically so an
  # already-running collector can finish using its old executable.
  ln -f "$HELPER" "$SESSIONS_STAGED"
  mv -f "$SESSIONS_STAGED" "$SESSIONS_HELPER"
  if [ "$REFRESH_CLI" = true ]; then
    CLI_STAGED="$(mktemp "$CLI_COPY.reload.XXXXXX")"
    install -m 0755 "$HELPER" "$CLI_STAGED"
    codesign --verify --strict "$CLI_STAGED"
    mv -f "$CLI_STAGED" "$CLI_COPY"
    CLI_STAGED=""
  fi
  printf '%s\n' "$PROBE_VERSION" > "$APP_BUNDLE/Contents/Resources/probe-version.txt"
  shasum -a 256 "$PROBE_BINARY" | awk '{print $1 "  agent-relay-probe"}' > "$APP_BUNDLE/Contents/Resources/agent-relay-probe.sha256"
fi

# Not --deep: sign the helper first, then seal the containing app.
codesign --force --sign "$IDENTITY" "$APP_BUNDLE"
note "reloaded $APP_BUNDLE"

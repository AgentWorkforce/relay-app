#!/usr/bin/env bash
#
# build-app.sh -- build the SwiftPM executable and assemble "Agent Relay.app".
#
# A build's Agent Relay site, link scheme and branding are fixed here, by the
# options (--site-url, --url-scheme, --product-name, --bundle-id,
# --releases-repository, --release-feed, --feed-public-key, --icon;
# production's when none is given), and
# nothing at run time changes them. The core in --probe must be built for the
# same site and scheme (scripts/build-probe.sh records both beside it). Only
# a loopback site (--dev-site-url, the shorthand for Agent Relay Dev) makes a
# development build; a build for any other site is a release build like
# production's. --coexist builds one without the AgentRelayProduct marker, so
# it sits beside Agent Relay and other products instead of replacing them.
#
# Produces the bundle layout from docs/plan.md section 4:
#
#   Agent Relay.app/Contents/
#     Info.plist                        from macos/Info.plist.template, with
#                                       the bundle id, name and link scheme,
#                                       and the AgentRelayProduct marker
#                                       (none with --dev-site-url or --coexist)
#     PkgInfo                           APPL????
#     MacOS/RelayDesktop                swift build product
#     Helpers/Agent Relay Helper        agent-relay-probe: the relay core, 0755,
#                                       named for Activity Monitor
#     Helpers/Agent Relay Helper (Sessions)
#                                       a hard link to it the core runs its
#                                       session collector as
#     Library/LaunchAgents/<label>.plist the core's LaunchAgent (SMAppService.agent)
#     Resources/AppIcon.icns            rendered by scripts/render-icon.swift
#     Resources/agent-relay-probe.sha256
#     Resources/probe-version.txt
#     Resources/crates.json             the core crates the core was built with,
#                                       when the lockfiles record them
#     Resources/site-url.txt            the site this build signs in to
#     Resources/url-scheme.txt          the scheme of the links it opens
#     Resources/releases-repository.txt the GitHub repository it checks for updates
#     Resources/asset-prefix.txt        the first part of the asset names it looks for there
#     Resources/release-feed.txt        where its releases come from, when not the default:
#                                       github:OWNER/NAME, or cloud (its own site)
#     Resources/feed-public-key.txt     the key a cloud feed's records are signed with
#     Resources/mark.txt                "AppIcon" with --icon: the app icon is the mark the app draws
#
# Signing is picked from the environment, never from flags:
#   Developer ID  CSC_LINK (base64 .p12) + CSC_KEY_PASSWORD
#   notarization  additionally APPLE_API_KEY_BASE64 (base64 .p8),
#                 APPLE_API_KEY_ID and APPLE_API_ISSUER
#   otherwise     ad-hoc, with a printed Gatekeeper warning
#
# Secret values are never echoed and the decoded .p12 lives in a 0600 file
# inside the temporary directory, which the exit trap removes.
#
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

EXECUTABLE_NAME="RelayDesktop"
PROBE_NAME="agent-relay-probe"
# Production's profile: what a build is made for when no option says otherwise.
DEFAULT_SITE_URL="https://agentrelay.com"
DEFAULT_URL_SCHEME="agentrelay"
DEFAULT_PRODUCT_NAME="Agent Relay"
DEFAULT_BUNDLE_ID="com.agentrelay.desktop"
DEFAULT_RELEASES_REPOSITORY="AgentWorkforce/relay-desktop-releases"
DEFAULT_ASSET_PREFIX="AgentRelay"
# Agent Relay Dev's, which --dev-site-url selects.
DEV_URL_SCHEME="agentrelay-dev"
DEV_PRODUCT_NAME="Agent Relay Dev"
DEV_BUNDLE_ID="com.agentrelay.desktop.dev"

usage() {
  cat <<EOF
Usage:
  ${SCRIPT_NAME} --arch arm64|x64 --version X.Y.Z --probe DIR --out DIR [--build N] [--debug]
                 [--site-url URL] [--url-scheme NAME] [--product-name NAME] [--bundle-id ID]
                 [--releases-repository OWNER/NAME] [--asset-prefix NAME] [--icon FILE] [--dev-site-url URL]
                 [--release-feed github:OWNER/NAME|cloud] [--feed-public-key KEY] [--coexist]

Options:
  --arch arm64|x64   Architecture to build for. Maps to 'swift build --arch
                     arm64' / '--arch x86_64'. Required.
  --version X.Y.Z    Marketing version; substituted for __VERSION__ in
                     macos/Info.plist.template. Required.
  --probe DIR        Directory produced by scripts/build-probe.sh, holding
                     ${PROBE_NAME}, ${PROBE_NAME}.sha256, probe-version.txt,
                     probe-site.txt and probe-url-scheme.txt. The core must be
                     built for this build's site and link scheme. Required.
  --out DIR          Directory the "<product name>.app" bundle is written into.
                     Created when missing. Required.
  --site-url URL     The Agent Relay site this build signs in to, fixed when
                     it is built: an https origin, or http://localhost[:port]
                     or http://127.0.0.1[:port]. Default ${DEFAULT_SITE_URL}.
  --url-scheme NAME  The scheme of the links this build opens, one per build.
                     Default ${DEFAULT_URL_SCHEME} (${DEV_URL_SCHEME} with --dev-site-url).
  --product-name NAME
                     The app's name: the bundle's file name, CFBundleName and
                     CFBundleDisplayName, and the helper's name. Default
                     "${DEFAULT_PRODUCT_NAME}" ("${DEV_PRODUCT_NAME}" with --dev-site-url).
  --bundle-id ID     CFBundleIdentifier. The core's LaunchAgent label
                     (<id>.core), socket and data directory follow it, so two
                     builds with different ids run side by side. Default
                     ${DEFAULT_BUNDLE_ID} (${DEV_BUNDLE_ID} with --dev-site-url).
  --releases-repository OWNER/NAME
                     The GitHub repository whose releases the app checks for
                     updates. Default ${DEFAULT_RELEASES_REPOSITORY}.
  --asset-prefix NAME
                     The first part of the release asset names the app looks
                     for there: ${DEFAULT_ASSET_PREFIX}, or a site profile's
                     Desktop-<id>. Default ${DEFAULT_ASSET_PREFIX}.
  --release-feed github:OWNER/NAME|cloud
                     Where the app's releases come from: a GitHub
                     repository's releases (which must be the one
                     --releases-repository names), or cloud, the site's own
                     feed, for a site profile's build (--asset-prefix
                     Desktop-<id>) for a site other than production's, with
                     no --releases-repository. Default
                     github:${DEFAULT_RELEASES_REPOSITORY}.
  --feed-public-key KEY
                     The Ed25519 public key (the base64 of 32 bytes) a cloud
                     feed's release records are signed with. Required with
                     --release-feed cloud, refused otherwise.
  --icon FILE        An SVG, or a 1024x1024 PNG, rendered into AppIcon.icns
                     in place of the repository's own mark.
  --dev-site-url URL Build Agent Relay Dev: --site-url URL, where URL is an
                     HTTP localhost/127.0.0.1 origin with a port, with the Dev
                     scheme, name and bundle id unless an option names another.
                     Not combined with --site-url. The probe in --probe must
                     be built for URL:
                     scripts/build-probe.sh --site-url URL --url-scheme ${DEV_URL_SCHEME}
  --coexist          Build an app that coexists with other Agent Relay apps (a
                     site profile's DESKTOP_COEXIST): no AgentRelayProduct
                     marker in its Info.plist or its core's LaunchAgent, so it
                     neither replaces an installed Agent Relay product nor is
                     replaced by one; its core keeps the socket, data
                     directory and LaunchAgent label of its own bundle id.
                     Refused for ${DEFAULT_BUNDLE_ID}, which always takes
                     over. Implied by --dev-site-url.
  --build N          Build number; substituted for __BUILD__ in the Info.plist
                     template. Defaults to \$GITHUB_RUN_NUMBER, else 1.
  --debug            Local development build: a debug Swift build that
                     scripts/dev-reload.sh rebuilds incrementally, signed with
                     the keychain's Apple Development identity when there is
                     one so Keychain access survives rebuilds, else ad-hoc.
                     Developer ID credentials are not used.
  -h, --help         Show this help.

Environment (all optional, read but never printed):
  CSC_LINK, CSC_KEY_PASSWORD
  APPLE_API_KEY_BASE64, APPLE_API_KEY_ID, APPLE_API_ISSUER
EOF
}

die() {
  printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2
  exit 1
}

note() {
  printf '%s: %s\n' "$SCRIPT_NAME" "$*"
}

warn() {
  printf '%s: warning: %s\n' "$SCRIPT_NAME" "$*" >&2
}

ARCH=""
VERSION=""
PROBE_DIR=""
OUT_DIR=""
BUILD_NUMBER=""
DEV_SITE_URL=""
SITE_URL=""
URL_SCHEME=""
PRODUCT_NAME=""
BUNDLE_ID=""
RELEASES_REPOSITORY=""
ASSET_PREFIX=""
RELEASE_FEED=""
FEED_PUBLIC_KEY=""
ICON_FILE=""
COEXIST=""
CONFIGURATION="release"

while [ $# -gt 0 ]; do
  case "$1" in
    --arch)
      [ $# -ge 2 ] || die "--arch needs a value (arm64 or x64)"
      ARCH="$2"
      shift 2
      ;;
    --version)
      [ $# -ge 2 ] || die "--version needs a value (for example 1.2.3)"
      VERSION="$2"
      shift 2
      ;;
    --probe)
      [ $# -ge 2 ] || die "--probe needs a directory"
      PROBE_DIR="$2"
      shift 2
      ;;
    --out)
      [ $# -ge 2 ] || die "--out needs a directory"
      OUT_DIR="$2"
      shift 2
      ;;
    --site-url)
      [ $# -ge 2 ] || die "--site-url needs an origin"
      SITE_URL="$2"
      shift 2
      ;;
    --url-scheme)
      [ $# -ge 2 ] || die "--url-scheme needs a name"
      URL_SCHEME="$2"
      shift 2
      ;;
    --product-name)
      [ $# -ge 2 ] || die "--product-name needs a name"
      PRODUCT_NAME="$2"
      shift 2
      ;;
    --bundle-id)
      [ $# -ge 2 ] || die "--bundle-id needs an identifier"
      BUNDLE_ID="$2"
      shift 2
      ;;
    --releases-repository)
      [ $# -ge 2 ] || die "--releases-repository needs OWNER/NAME"
      RELEASES_REPOSITORY="$2"
      shift 2
      ;;
    --release-feed)
      [ $# -ge 2 ] || die "--release-feed needs github:OWNER/NAME or cloud"
      RELEASE_FEED="$2"
      shift 2
      ;;
    --feed-public-key)
      [ $# -ge 2 ] || die "--feed-public-key needs a key"
      FEED_PUBLIC_KEY="$2"
      shift 2
      ;;
    --asset-prefix)
      [ $# -ge 2 ] || die "--asset-prefix needs a name"
      ASSET_PREFIX="$2"
      shift 2
      ;;
    --icon)
      [ $# -ge 2 ] || die "--icon needs a file"
      ICON_FILE="$2"
      shift 2
      ;;
    --dev-site-url)
      [ $# -ge 2 ] || die "--dev-site-url needs a loopback origin"
      DEV_SITE_URL="$2"
      shift 2
      ;;
    --coexist)
      COEXIST=1
      shift
      ;;
    --build)
      [ $# -ge 2 ] || die "--build needs a number"
      BUILD_NUMBER="$2"
      shift 2
      ;;
    --debug)
      CONFIGURATION="debug"
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

[ -n "$ARCH" ] || { usage >&2; die "--arch is required"; }
[ -n "$VERSION" ] || { usage >&2; die "--version is required"; }
[ -n "$PROBE_DIR" ] || { usage >&2; die "--probe is required"; }
[ -n "$OUT_DIR" ] || { usage >&2; die "--out is required"; }

case "$ARCH" in
  arm64) SWIFT_ARCH="arm64" ;;
  x64)   SWIFT_ARCH="x86_64" ;;
  *)     die "--arch must be arm64 or x64, got: $ARCH" ;;
esac

# CFBundleShortVersionString accepts up to three dot-separated integers; allow a
# prerelease suffix so 1.2.3-rc.1 tags do not need a second code path.
case "$VERSION" in
  [0-9]*.[0-9]*.[0-9]*) : ;;
  *) die "--version must look like X.Y.Z, got: $VERSION" ;;
esac

# shellcheck source-path=SCRIPTDIR
# shellcheck source=site-origin.sh
. "$SCRIPT_DIR/site-origin.sh"

# --- the build's profile ------------------------------------------------------
#
# The site, link scheme, name, bundle id and update repository are fixed here
# and recorded in the bundle; nothing at run time changes them. --dev-site-url
# is --site-url for a loopback site with Agent Relay Dev's scheme, name and
# bundle id filled in; only such a loopback site makes a development build.
if [ -n "$DEV_SITE_URL" ]; then
  [ -z "$SITE_URL" ] || die "--dev-site-url and --site-url cannot both be given; --dev-site-url is --site-url for a loopback site"
  [[ "$DEV_SITE_URL" =~ ^http://(localhost|127\.0\.0\.1):([0-9]+)$ ]] || die "--dev-site-url must be an HTTP loopback origin with a port"
  DEV_PORT="${BASH_REMATCH[2]}"
  (( 10#$DEV_PORT >= 1024 && 10#$DEV_PORT <= 65535 )) || die "--dev-site-url port must be between 1024 and 65535"
  SITE_URL="$DEV_SITE_URL"
  URL_SCHEME="${URL_SCHEME:-$DEV_URL_SCHEME}"
  PRODUCT_NAME="${PRODUCT_NAME:-$DEV_PRODUCT_NAME}"
  BUNDLE_ID="${BUNDLE_ID:-$DEV_BUNDLE_ID}"
fi
# Normalized as the core normalizes them (site-origin.sh), so a site typed
# with a trailing slash or capitals is the same site.
WANT_SITE="$(site_origin "${SITE_URL:-$DEFAULT_SITE_URL}")" || die "--site-url must be an origin such as https://relay.example or http://localhost:3100, got: $SITE_URL"
WANT_SCHEME="$(url_scheme_name "${URL_SCHEME:-$DEFAULT_URL_SCHEME}")" || die "--url-scheme must be a letter followed by letters, digits, +, - or ., and not a web scheme, got: $URL_SCHEME"
APP_NAME="${PRODUCT_NAME:-$DEFAULT_PRODUCT_NAME}"
case "$APP_NAME" in
  *[/:]*|"") die "--product-name must be a file name without / or :, got: $APP_NAME" ;;
esac
BUNDLE_ID="${BUNDLE_ID:-$DEFAULT_BUNDLE_ID}"
[[ "$BUNDLE_ID" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)+$ ]] || die "--bundle-id must be a reverse-DNS identifier such as com.example.relay, got: $BUNDLE_ID"
# Production is the one app every other Agent Relay product makes way for; a
# build under its bundle id always carries the marker and takes over. macOS
# compares bundle ids without case, as the site-profile action does.
[ -z "$COEXIST" ] || [ "$(printf '%s' "$BUNDLE_ID" | tr '[:upper:]' '[:lower:]')" != "$DEFAULT_BUNDLE_ID" ] || die "--coexist needs its own --bundle-id; $DEFAULT_BUNDLE_ID is the production app's, which always takes over"
# The id names the core's socket, ~/.agentworkforce/desktop/core.<id>.sock;
# a Unix socket path is limited to 104 bytes on macOS, and the home
# directory takes its share (/Users/<name>, up to a few dozen bytes).
[ "${#BUNDLE_ID}" -le 32 ] || die "--bundle-id must be at most 32 characters, got ${#BUNDLE_ID}: $BUNDLE_ID"
[ -z "$RELEASES_REPOSITORY" ] || [[ "$RELEASES_REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die "--releases-repository must be OWNER/NAME, got: $RELEASES_REPOSITORY"
# The first part of the release asset names the updater looks for in that
# repository: AgentRelay, or the profile's Desktop-<id> (the workflow passes
# it); recorded in the bundle as Resources/asset-prefix.txt.
ASSET_PREFIX="${ASSET_PREFIX:-$DEFAULT_ASSET_PREFIX}"
[[ "$ASSET_PREFIX" =~ ^[A-Za-z0-9]+(-[A-Za-z0-9]+)*$ ]] || die "--asset-prefix must be letters, digits and hyphens, got: $ASSET_PREFIX"
# The feed and the repository settle together (site-origin.sh):
# github:OWNER/NAME is that repository's releases, and cloud, the site's own
# feed, has no repository and needs the profile (its Desktop-<id>) and the
# key the feed's records are signed with.
PROFILE_ID=""
case "$ASSET_PREFIX" in Desktop-?*) PROFILE_ID="${ASSET_PREFIX#Desktop-}" ;; esac
settle_release_feed "$RELEASE_FEED" "$RELEASES_REPOSITORY" "$FEED_PUBLIC_KEY" "$WANT_SITE" "$PROFILE_ID" \
  || die "--release-feed, --releases-repository or --feed-public-key refused"
RELEASE_FEED="$SETTLED_FEED"
RELEASES_REPOSITORY="$SETTLED_REPOSITORY"
FEED_PUBLIC_KEY="$SETTLED_KEY"
# A build for another site is another app: beside the production app it
# must not share its bundle id (LaunchAgent label, socket, data directory,
# the legacy migration), its link scheme, or its update repository, from
# which it would otherwise install production over itself. A loopback site
# is the development build, which --dev-site-url names with its own
# identity and port rule.
if [ "$WANT_SITE" != "$DEFAULT_SITE_URL" ]; then
  if [ -z "$DEV_SITE_URL" ] && [[ "$WANT_SITE" =~ ^http://(localhost|127\.0\.0\.1)(:[0-9]+)?$ ]]; then
    die "a loopback site is the development build: use --dev-site-url http://localhost:<port> (a port between 1024 and 65535) instead of --site-url"
  fi
  [ "$BUNDLE_ID" != "$DEFAULT_BUNDLE_ID" ] || die "a build for $WANT_SITE needs its own --bundle-id; $DEFAULT_BUNDLE_ID is the production app's (its LaunchAgent, socket and data directory would be shared)"
  [ "$WANT_SCHEME" != "$DEFAULT_URL_SCHEME" ] || die "a build for $WANT_SITE needs its own --url-scheme; $DEFAULT_URL_SCHEME:// links are the production app's"
  # Agent Relay Dev's identity is the loopback build's alone, for the same
  # reasons; a release build for another site may not borrow it either.
  if [ -z "$DEV_SITE_URL" ]; then
    [ "$BUNDLE_ID" != "$DEV_BUNDLE_ID" ] || die "a build for $WANT_SITE needs its own --bundle-id; $DEV_BUNDLE_ID is Agent Relay Dev's"
    [ "$WANT_SCHEME" != "$DEV_URL_SCHEME" ] || die "a build for $WANT_SITE needs its own --url-scheme; $DEV_URL_SCHEME:// links are Agent Relay Dev's"
  fi
  # The development build never checks for or installs updates (AppUpdater), so it keeps the default.
  # A cloud feed is the site's own, so it is never production's.
  [ -n "$DEV_SITE_URL" ] || [ "$RELEASE_FEED" = cloud ] || [ "$RELEASES_REPOSITORY" != "$DEFAULT_RELEASES_REPOSITORY" ] || die "a build for $WANT_SITE needs its own --releases-repository; updates from $DEFAULT_RELEASES_REPOSITORY would install the production app over it"
fi
if [ -n "$ICON_FILE" ]; then
  [ -f "$ICON_FILE" ] || die "--icon: no file at $ICON_FILE"
  case "$ICON_FILE" in
    *.svg|*.SVG|*.png|*.PNG) ;;
    *) die "--icon must be an SVG or a 1024x1024 PNG, got: $ICON_FILE" ;;
  esac
  ICON_FILE="$(cd "$(dirname "$ICON_FILE")" && pwd)/$(basename "$ICON_FILE")"
fi
# The bundled core's file name, which Activity Monitor shows for it and the
# uploader it starts. Its code signature keeps the identifier $PROBE_NAME, the
# designated requirement the command-line copy in ~/.local/bin is checked
# against.
HELPER_NAME="$APP_NAME Helper"

BUILD_NUMBER="${BUILD_NUMBER:-${GITHUB_RUN_NUMBER:-1}}"
case "$BUILD_NUMBER" in
  ''|*[!0-9]*) die "--build must be a positive integer, got: $BUILD_NUMBER" ;;
esac

[ "$(uname -s)" = "Darwin" ] || die "this script only runs on macOS (swift build --arch, codesign, iconutil)"
for tool in swift codesign iconutil plutil ditto; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool '$tool' not found; install the Xcode command line tools"
done

MACOS_DIR="$REPO_ROOT/macos"
[ -f "$MACOS_DIR/Package.swift" ] || die "no SwiftPM package at $MACOS_DIR/Package.swift"
INFO_PLIST_TEMPLATE="$MACOS_DIR/Info.plist.template"
[ -f "$INFO_PLIST_TEMPLATE" ] || die "no Info.plist template at $INFO_PLIST_TEMPLATE"

PROBE_BINARY="$PROBE_DIR/$PROBE_NAME"
PROBE_CHECKSUM="$PROBE_DIR/$PROBE_NAME.sha256"
[ -f "$PROBE_BINARY" ] || die "no $PROBE_NAME in $PROBE_DIR; run scripts/build-probe.sh first"
[ -f "$PROBE_CHECKSUM" ] || die "no $PROBE_NAME.sha256 in $PROBE_DIR; run scripts/build-probe.sh first"

bash "$SCRIPT_DIR/check-probe-bridge.sh" "$PROBE_BINARY" "$ARCH"

# The core signs in to the site it was built for (AGENT_RELAY_SITE_URL when it
# was built) and opens links of the scheme it was built with; nothing at run
# time changes either. The core prints its site (`cloud install --help`) but
# not its scheme, and may be for another architecture than this host, so
# build-probe.sh records both beside the binary (site-origin.sh reads and
# normalizes them), and they must be exactly this build's site and scheme.
case "$ARCH" in
  arm64) RUST_TARGET="aarch64-apple-darwin" ;;
  *)     RUST_TARGET="x86_64-apple-darwin" ;;
esac
REBUILD="scripts/build-probe.sh --target $RUST_TARGET --version '$VERSION' --out '$PROBE_DIR' --site-url '$WANT_SITE' --url-scheme '$WANT_SCHEME'"
check_sidecars "$PROBE_BINARY" probe "$WANT_SITE" "$WANT_SCHEME" || die "the probe in $PROBE_DIR is not built for this bundle's site $WANT_SITE and link scheme $WANT_SCHEME://; build it with: $REBUILD"

PROBE_VERSION="unknown"
if [ -f "$PROBE_DIR/probe-version.txt" ]; then
  PROBE_VERSION="$(tr -d '\n\r' < "$PROBE_DIR/probe-version.txt")"
else
  warn "no probe-version.txt in $PROBE_DIR; recording the probe version as 'unknown'"
fi

mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
APP_BUNDLE="$OUT_DIR/$APP_NAME.app"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/build-app.XXXXXX")"
KEYCHAIN_PATH=""
cleanup() {
  if [ -n "$KEYCHAIN_PATH" ]; then
    # Also drops the keychain from the user search list.
    security delete-keychain "$KEYCHAIN_PATH" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# --- 1. build the executable -------------------------------------------------

SWIFT_FLAGS=(-c "$CONFIGURATION" --arch "$SWIFT_ARCH" --package-path "$MACOS_DIR")

note "swift build -c $CONFIGURATION --arch $SWIFT_ARCH"
swift build "${SWIFT_FLAGS[@]}"

BIN_PATH="$(swift build "${SWIFT_FLAGS[@]}" --show-bin-path | tail -n 1)"
BUILT_EXECUTABLE="$BIN_PATH/$EXECUTABLE_NAME"
if [ ! -f "$BUILT_EXECUTABLE" ]; then
  # --show-bin-path is the reliable answer, but fall back to the conventional
  # layouts so a SwiftPM change does not silently break packaging.
  for candidate in \
    "$MACOS_DIR/.build/$SWIFT_ARCH-apple-macosx/$CONFIGURATION/$EXECUTABLE_NAME" \
    "$MACOS_DIR/.build/$CONFIGURATION/$EXECUTABLE_NAME"
  do
    if [ -f "$candidate" ]; then
      BIN_PATH="$(dirname "$candidate")"
      BUILT_EXECUTABLE="$candidate"
      break
    fi
  done
fi
[ -f "$BUILT_EXECUTABLE" ] || die "swift build succeeded but no '$EXECUTABLE_NAME' product was found under $BIN_PATH"

# --- 2. assemble the bundle --------------------------------------------------

note "assembling $APP_BUNDLE"
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Helpers" "$APP_BUNDLE/Contents/Resources"

install -m 0755 "$BUILT_EXECUTABLE" "$APP_BUNDLE/Contents/MacOS/$EXECUTABLE_NAME"
install -m 0755 "$PROBE_BINARY" "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME"
install -m 0644 "$PROBE_CHECKSUM" "$APP_BUNDLE/Contents/Resources/$PROBE_NAME.sha256"
# The project license and the bundled agent icons' license ship with the app.
install -m 0644 "$REPO_ROOT/LICENSE" "$APP_BUNDLE/Contents/Resources/LICENSE"
install -m 0644 "$REPO_ROOT/assets/agent-icons-LICENSE.txt" "$APP_BUNDLE/Contents/Resources/agent-icons-LICENSE.txt"

# Both release diagnostics and local builds identify the desktop source revision.
BUILD_REVISION="$(git -C "$MACOS_DIR" rev-parse --short=12 HEAD 2>/dev/null || printf unknown)"
printf '%s\n' "$BUILD_REVISION" > "$APP_BUNDLE/Contents/Resources/build-revision.txt"
# The core crates the bundled core resolves, from the lockfiles (About >
# Diagnostics): the pins, or an internal build's overrides. A release build
# (Developer ID signed) must record them. A local build of a core from a path
# or a [patch] has no commit to record: it warns and goes without, and About
# shows the crates as Unavailable, as GTK's does.
CRATES_JSON="$APP_BUNDLE/Contents/Resources/crates.json"
if ! bash "$REPO_ROOT/scripts/apply-crate-overrides.sh" --json "$CRATES_JSON"; then
  if [ "$CONFIGURATION" = release ] && [ -n "${CSC_LINK:-}" ] && [ -n "${CSC_KEY_PASSWORD:-}" ]; then
    die "could not record the core crates (Resources/crates.json) of a release build"
  fi
  warn "could not record the core crates: About > Diagnostics shows them as Unavailable"
  rm -f "$CRATES_JSON"
fi

printf '%s\n' "$PROBE_VERSION" > "$APP_BUNDLE/Contents/Resources/probe-version.txt"
# Classic four-character type/creator pair; no trailing newline.
printf 'APPL????' > "$APP_BUNDLE/Contents/PkgInfo"

# SwiftPM emits resource bundles next to the executable; carry any along.
for resource_bundle in "$BIN_PATH"/*.bundle; do
  [ -e "$resource_bundle" ] || continue
  cp -R "$resource_bundle" "$APP_BUNDLE/Contents/Resources/"
done

# --- 3. Info.plist -----------------------------------------------------------

sed -e "s|__VERSION__|$VERSION|g" -e "s|__BUILD__|$BUILD_NUMBER|g" \
  "$INFO_PLIST_TEMPLATE" > "$APP_BUNDLE/Contents/Info.plist"

if grep -q -e '__VERSION__' -e '__BUILD__' "$APP_BUNDLE/Contents/Info.plist"; then
  die "__VERSION__/__BUILD__ survived substitution in the generated Info.plist"
fi
if grep -q '__[A-Z0-9_]\{2,\}__' "$APP_BUNDLE/Contents/Info.plist"; then
  warn "the generated Info.plist still contains __PLACEHOLDER__ style tokens"
fi
# The profile the build is made for, in the Info.plist and beside it in
# Resources, where the app reads what it was built for (AppInfo.swift) instead
# of inferring it. The template carries production's values; every build
# writes its own, so a profile is never half applied.
plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$APP_BUNDLE/Contents/Info.plist"
plutil -replace CFBundleName -string "$APP_NAME" "$APP_BUNDLE/Contents/Info.plist"
plutil -replace CFBundleDisplayName -string "$APP_NAME" "$APP_BUNDLE/Contents/Info.plist"
plutil -replace CFBundleURLTypes.0.CFBundleURLName -string "$APP_NAME connection" "$APP_BUNDLE/Contents/Info.plist"
# Replace the whole array so a site build cannot retain a production scheme.
plutil -replace CFBundleURLTypes.0.CFBundleURLSchemes -json "[\"$WANT_SCHEME\"]" "$APP_BUNDLE/Contents/Info.plist"
# The Agent Relay family marker: the bundle id under AgentRelayProduct, here
# and in the core's LaunchAgent below. One Agent Relay app runs per Mac: a
# release build launched where another marked product is installed replaces
# it (ProductTakeover.swift), and recognises it by this key alone, never by
# its name. The development build carries none, and nor does a --coexist
# build (an internal profile's), so they sit beside the others.
if [ -z "$DEV_SITE_URL" ] && [ -z "$COEXIST" ]; then
  PRODUCT_MARKER="$BUNDLE_ID"
  plutil -replace AgentRelayProduct -string "$PRODUCT_MARKER" "$APP_BUNDLE/Contents/Info.plist"
else
  PRODUCT_MARKER=""
fi
printf '%s\n' "$WANT_SITE" > "$APP_BUNDLE/Contents/Resources/site-url.txt"
printf '%s\n' "$WANT_SCHEME" > "$APP_BUNDLE/Contents/Resources/url-scheme.txt"
printf '%s\n' "$RELEASES_REPOSITORY" > "$APP_BUNDLE/Contents/Resources/releases-repository.txt"
printf '%s\n' "$ASSET_PREFIX" > "$APP_BUNDLE/Contents/Resources/asset-prefix.txt"
# The feed and its key are recorded only when they are not the default's,
# so a default build's records are the same as before there was a feed.
rm -f "$APP_BUNDLE/Contents/Resources/release-feed.txt" "$APP_BUNDLE/Contents/Resources/feed-public-key.txt"
if [ "$RELEASE_FEED" != "$DEFAULT_RELEASE_FEED" ]; then
  printf '%s\n' "$RELEASE_FEED" > "$APP_BUNDLE/Contents/Resources/release-feed.txt"
fi
if [ -n "$FEED_PUBLIC_KEY" ]; then
  printf '%s\n' "$FEED_PUBLIC_KEY" > "$APP_BUNDLE/Contents/Resources/feed-public-key.txt"
fi
plutil -lint "$APP_BUNDLE/Contents/Info.plist" >/dev/null || \
  die "the generated Info.plist is not a valid property list"

# --- 4. the core's LaunchAgent ----------------------------------------------
#
# launchd runs Helpers/$HELPER_NAME `serve` at login, keeps it
# alive, and restarts it from this bundle after an update. The label, socket
# and data directory follow the bundle id, so builds with different ids on
# one Mac never share a core: production's are the core's defaults, Agent
# Relay Dev's are core.dev.sock and com.agentrelay.desktop.dev, and any other
# build's are core.<bundle id>.sock and <bundle id> (relative names resolve
# beside the defaults; a property list cannot expand ~). AppInfo.swift derives
# the same names from the bundle id. The site and link scheme are built into
# the probe, not arguments.

CORE_ARGS=("$HELPER_NAME" serve)
case "$BUNDLE_ID" in
  "$DEFAULT_BUNDLE_ID") ;;
  "$DEV_BUNDLE_ID") CORE_ARGS+=(--socket core.dev.sock --app-data-dir "$DEV_BUNDLE_ID") ;;
  *) CORE_ARGS+=(--socket "core.$BUNDLE_ID.sock" --app-data-dir "$BUNDLE_ID") ;;
esac
CORE_LABEL="$BUNDLE_ID.core"
AGENT_PLIST="$APP_BUNDLE/Contents/Library/LaunchAgents/$CORE_LABEL.plist"
mkdir -p "$(dirname "$AGENT_PLIST")"
{
  printf '<?xml version="1.0" encoding="UTF-8"?>\n'
  printf '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
  printf '<plist version="1.0">\n<dict>\n'
  printf '  <key>Label</key>\n  <string>%s</string>\n' "$CORE_LABEL"
  printf '  <key>BundleProgram</key>\n  <string>Contents/Helpers/%s</string>\n' "$HELPER_NAME"
  printf '  <key>ProgramArguments</key>\n  <array>\n'
  for arg in "${CORE_ARGS[@]}"; do printf '    <string>%s</string>\n' "$arg"; done
  printf '  </array>\n'
  printf '  <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>%s</string>\n  </array>\n' "$BUNDLE_ID"
  if [ -n "$PRODUCT_MARKER" ]; then
    printf '  <key>AgentRelayProduct</key>\n  <string>%s</string>\n' "$PRODUCT_MARKER"
  fi
  printf '  <key>RunAtLoad</key>\n  <true/>\n'
  printf '  <key>KeepAlive</key>\n  <true/>\n'
  printf '</dict>\n</plist>\n'
} > "$AGENT_PLIST"
plutil -lint "$AGENT_PLIST" >/dev/null || die "the generated LaunchAgent is not a valid property list"

# --- 5. icon -----------------------------------------------------------------

ICON_WORK_DIR="$TMP_DIR/icon"
mkdir -p "$ICON_WORK_DIR"
# The repository's own mark unless --icon supplies the artwork; either way
# render-icon.swift writes the iconset and checks what it was given.
RENDER_ICON=("$SCRIPT_DIR/render-icon.swift" "$ICON_WORK_DIR")
if [ -n "$ICON_FILE" ]; then
  note "rendering AppIcon.icns from $ICON_FILE"
  RENDER_ICON+=(--from "$ICON_FILE")
else
  note "rendering AppIcon.icns"
fi
swift "${RENDER_ICON[@]}"
[ -d "$ICON_WORK_DIR/AppIcon.iconset" ] || die "render-icon.swift did not produce $ICON_WORK_DIR/AppIcon.iconset"
iconutil -c icns "$ICON_WORK_DIR/AppIcon.iconset" -o "$APP_BUNDLE/Contents/Resources/AppIcon.icns"
[ -f "$APP_BUNDLE/Contents/Resources/AppIcon.icns" ] || die "iconutil did not write AppIcon.icns"
# A profile's artwork is its mark too: the app draws its icon where it would
# draw Agent Relay's (AppInfo.marksWithAppIcon), as Linux and Windows draw it.
if [ -n "$ICON_FILE" ]; then
  printf 'AppIcon\n' > "$APP_BUNDLE/Contents/Resources/mark.txt"
fi

# --- 6. signing --------------------------------------------------------------

ENTITLEMENTS="$MACOS_DIR/$EXECUTABLE_NAME.entitlements"
if [ ! -f "$ENTITLEMENTS" ]; then
  warn "no $ENTITLEMENTS; signing with an empty entitlements dictionary"
  ENTITLEMENTS="$TMP_DIR/$EXECUTABLE_NAME.entitlements"
  cat > "$ENTITLEMENTS" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict/>
</plist>
PLIST
fi

CSC_LINK="${CSC_LINK:-}"
CSC_KEY_PASSWORD="${CSC_KEY_PASSWORD:-}"
APPLE_API_KEY_BASE64="${APPLE_API_KEY_BASE64:-}"
APPLE_API_KEY_ID="${APPLE_API_KEY_ID:-}"
APPLE_API_ISSUER="${APPLE_API_ISSUER:-}"
MACOS_SIGNING_IDENTITY=""

import_signing_certificate() {
  local p12_file="$TMP_DIR/signing-certificate.p12"
  local certificate_data="$CSC_LINK"
  local keychain_password
  local original_keychains

  # Create the file first, then lock it down, then write through the existing
  # (0600) inode -- so the decoded certificate is never world readable.
  : > "$p12_file"
  chmod 600 "$p12_file"
  # electron-builder accepts either raw base64 or a base64 data URL in CSC_LINK.
  if [[ "$certificate_data" == data:*';base64,'* ]]; then
    certificate_data="${certificate_data#*,}"
  fi
  if ! printf '%s' "$certificate_data" | base64 --decode > "$p12_file" 2>/dev/null; then
    # BSD base64 before macOS 13 spells the flag -D.
    printf '%s' "$certificate_data" | base64 -D > "$p12_file"
  fi
  [ -s "$p12_file" ] || die "CSC_LINK did not base64-decode to anything"

  keychain_password="$(uuidgen)"
  KEYCHAIN_PATH="$TMP_DIR/relay-desktop-signing.keychain-db"

  security create-keychain -p "$keychain_password" "$KEYCHAIN_PATH" >/dev/null
  security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH" >/dev/null
  security unlock-keychain -p "$keychain_password" "$KEYCHAIN_PATH" >/dev/null
  if ! security import "$p12_file" -k "$KEYCHAIN_PATH" -P "$CSC_KEY_PASSWORD" \
      -T /usr/bin/codesign -T /usr/bin/security -f pkcs12 >/dev/null; then
    die "could not import the signing certificate; check CSC_LINK and CSC_KEY_PASSWORD"
  fi
  # Without this codesign prompts for keychain access and hangs on a runner.
  # It is chatty and occasionally returns nonzero while still having done its
  # job, so warn rather than abort -- a genuine failure surfaces as a codesign
  # hang or error a few lines later.
  if ! security set-key-partition-list -S apple-tool:,apple:,codesign: \
      -s -k "$keychain_password" "$KEYCHAIN_PATH" >/dev/null 2>&1; then
    warn "security set-key-partition-list returned nonzero; codesign may prompt for keychain access"
  fi

  # Keep the login keychain reachable; the temporary one is deleted by the trap.
  # shellcheck disable=SC2207
  original_keychains=($(security list-keychains -d user | sed -e 's/^[[:space:]]*"//' -e 's/"$//'))
  if [ "${#original_keychains[@]}" -gt 0 ]; then
    security list-keychains -d user -s "$KEYCHAIN_PATH" "${original_keychains[@]}" >/dev/null
  else
    security list-keychains -d user -s "$KEYCHAIN_PATH" >/dev/null
  fi

  rm -f "$p12_file"
  MACOS_SIGNING_IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN_PATH" | awk '/"Developer ID Application:/{print $2; exit}')"
  [ -n "$MACOS_SIGNING_IDENTITY" ] || die "CSC_LINK contains no valid Developer ID Application signing identity"
}

# A hard link, made once the helper is signed (re-signing replaces the file
# and would leave the link on the old bytes): the session collector the core
# starts runs as it, so Activity Monitor lists it apart from the core. Not a
# symlink, which the process would be named after the target of; not a copy,
# which would add the whole core to the download.
link_sessions_helper() {
  ln "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME" "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME (Sessions)"
}

codesign_with_identity() {
  # usage: codesign_with_identity TARGET [EXTRA CODESIGN ARGS...]
  # Apple's timestamp server sometimes does not answer ("A timestamp was
  # expected but was not found."). --force re-signing is safe, so that one
  # failure is retried with backoff; any other failure stops at once.
  local target="$1"
  shift
  local attempt=1 delay=5 output status
  while :; do
    status=0
    output="$(codesign --force --options runtime --timestamp \
      --entitlements "$ENTITLEMENTS" \
      --keychain "$KEYCHAIN_PATH" \
      --sign "$MACOS_SIGNING_IDENTITY" \
      "$@" "$target" 2>&1)" || status=$?
    [ -z "$output" ] || printf '%s\n' "$output" >&2
    [ "$status" -eq 0 ] && return 0
    if [ "$attempt" -ge 5 ] || ! is_timestamp_failure "$output"; then
      return "$status"
    fi
    warn "the timestamp server did not answer for $target (attempt $attempt of 5); retrying in ${delay}s"
    sleep "$delay"
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
}

is_timestamp_failure() {
  case "$1" in
    *"A timestamp was expected but was not found"* | *"The timestamp service is not available"*) return 0 ;;
    *) return 1 ;;
  esac
}

stapler_with_retry() {
  # usage: stapler_with_retry staple TARGET
  # The ticket lookup (api.apple-cloudkit.com) sometimes drops the connection
  # (NSURLErrorDomain, "The staple and validate action failed! Error 68.").
  # Stapling again is safe, so a network failure is retried with backoff;
  # any other failure, such as a missing ticket, stops at once.
  local action="$1" target="$2"
  local attempt=1 delay=10 output status
  while :; do
    status=0
    output="$(xcrun stapler "$action" "$target" 2>&1)" || status=$?
    [ -z "$output" ] || printf '%s\n' "$output" >&2
    [ "$status" -eq 0 ] && return 0
    if [ "$attempt" -ge 5 ] || ! is_stapler_network_failure "$output"; then
      return "$status"
    fi
    warn "stapler $action could not reach Apple for $target (attempt $attempt of 5); retrying in ${delay}s"
    sleep "$delay"
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
}

is_stapler_network_failure() {
  case "$1" in
    *NSURLErrorDomain* | *"network connection was lost"* | *"Error 68"*) return 0 ;;
    *) return 1 ;;
  esac
}

notarize_bundle() {
  local zip_path="$TMP_DIR/$EXECUTABLE_NAME-notarize.zip"
  local api_key_path="$TMP_DIR/AuthKey.p8"
  : > "$api_key_path"
  chmod 600 "$api_key_path"
  if ! printf '%s' "$APPLE_API_KEY_BASE64" | base64 --decode > "$api_key_path" 2>/dev/null; then
    printf '%s' "$APPLE_API_KEY_BASE64" | base64 -D > "$api_key_path"
  fi
  [ -s "$api_key_path" ] || die "APPLE_API_KEY_BASE64 did not base64-decode to anything"
  note "submitting the bundle to the notary service (this waits for the verdict)"
  ditto -c -k --keepParent "$APP_BUNDLE" "$zip_path"
  if ! xcrun notarytool submit "$zip_path" \
      --key "$api_key_path" \
      --key-id "$APPLE_API_KEY_ID" \
      --issuer "$APPLE_API_ISSUER" \
      --wait; then
    die "notarization failed; run 'xcrun notarytool log <submission-id>' with the same API key for the reason"
  fi
  # Stapling is the real gate: it fails unless the ticket exists, and it
  # validates the ticket it attaches.
  stapler_with_retry staple "$APP_BUNDLE"
  rm -f "$zip_path"
  note "notarized and stapled"
}

SIGNING_MODE="adhoc"
DEVELOPMENT_IDENTITY=""
if [ "$CONFIGURATION" = "debug" ]; then
  # Debug bundles are re-signed by dev-reload.sh, so they need an identity
  # that outlives this script: never the Developer ID from the temporary
  # keychain. An ad-hoc signature changes with every build, and Keychain asks
  # again for each one; a development certificate keeps the identity stable.
  DEVELOPMENT_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | awk '/"Apple Development: /{print $2; exit}')"
  if [ -n "$DEVELOPMENT_IDENTITY" ]; then
    SIGNING_MODE="development"
  fi
elif [ -n "$CSC_LINK" ] && [ -n "$CSC_KEY_PASSWORD" ]; then
  SIGNING_MODE="developer-id"
elif [ -n "$CSC_LINK" ] || [ -n "$CSC_KEY_PASSWORD" ]; then
  warn "partial Developer ID configuration: CSC_LINK and CSC_KEY_PASSWORD are both required; falling back to ad-hoc signing"
fi

if [ "$SIGNING_MODE" = "developer-id" ]; then
  import_signing_certificate
  note "signing with the Developer ID identity from CSC_LINK"
  # Nested code first: the outer signature seals whatever is inside it.
  codesign_with_identity "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME" --identifier "$PROBE_NAME"
  link_sessions_helper
  codesign_with_identity "$APP_BUNDLE"
  codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

  if [ -n "$APPLE_API_KEY_BASE64" ] && [ -n "$APPLE_API_KEY_ID" ] && [ -n "$APPLE_API_ISSUER" ]; then
    notarize_bundle
  else
    warn "APPLE_API_KEY_BASE64 / APPLE_API_KEY_ID / APPLE_API_ISSUER are not all set: the bundle is signed but NOT notarized, so first launch still shows a Gatekeeper warning"
  fi
elif [ "$SIGNING_MODE" = "development" ]; then
  note "signing with the keychain's Apple Development identity"
  # Not --deep, which would name the helper's signature after its file.
  codesign --force --identifier "$PROBE_NAME" --sign "$DEVELOPMENT_IDENTITY" "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME"
  link_sessions_helper
  codesign --force --sign "$DEVELOPMENT_IDENTITY" "$APP_BUNDLE"
  codesign --verify --deep --strict "$APP_BUNDLE"
else
  note "ad-hoc signing (no Developer ID credentials in the environment)"
  codesign --force --identifier "$PROBE_NAME" --sign - "$APP_BUNDLE/Contents/Helpers/$HELPER_NAME"
  link_sessions_helper
  codesign --force --sign - "$APP_BUNDLE"
  codesign --verify --deep --strict "$APP_BUNDLE"
  cat >&2 <<EOF

  ****************************************************************************
  NOTICE: "$APP_NAME.app" is ad-hoc signed and not notarized.
  macOS will refuse to open it with "cannot be opened because it is from an
  unidentified developer". Users have to right-click the app and choose Open
  once (README.md documents this). Set CSC_LINK and CSC_KEY_PASSWORD (plus
  APPLE_API_KEY_BASE64, APPLE_API_KEY_ID and APPLE_API_ISSUER) to produce a
  distributable build.
  ****************************************************************************

EOF
fi

note "built $APP_BUNDLE"
note "  version      $VERSION (build $BUILD_NUMBER)"
note "  architecture $ARCH"
note "  probe        $PROBE_VERSION"
note "  site         $WANT_SITE (${WANT_SCHEME}:// links)"
note "  bundle id    $BUNDLE_ID"
if [ "$RELEASE_FEED" = cloud ]; then
  note "  updates      the cloud feed on $WANT_SITE"
else
  note "  updates      github.com/$RELEASES_REPOSITORY"
fi
note "  icon         ${ICON_FILE:-the repository mark (scripts/render-icon.swift)}"
note "  signing      $SIGNING_MODE"

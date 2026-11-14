#!/usr/bin/env bash
#
# build-gtk.sh -- build the GTK app, `agent-relay`, from gtk/ for Linux (the
# Windows app is built by build-windows.sh in its MSYS2 shell).
#
# Writes into --out:
#   agent-relay                         the binary, mode 0755
#   agent-relay.sha256                  `sha256sum` format line naming `agent-relay`
#   agent-relay-site.txt                the Agent Relay site built in (--site-url)
#   agent-relay-url-scheme.txt          the link scheme built in (--url-scheme)
#   agent-relay-product-name.txt        the product name built in (--product-name)
#   agent-relay-releases-repository.txt the releases repository built in
#                                       (--releases-repository; empty for a
#                                       cloud feed)
#   agent-relay-release-feed.txt        the release feed built in
#                                       (--release-feed), written only when
#                                       it is not the default's
#   agent-relay-feed-public-key.txt     a cloud feed's public key
#                                       (--feed-public-key), likewise
#   agent-relay-cobrand-name.txt        the customer a co-branded build says
#                                       it is "for" (--cobrand-name),
#                                       written only for a co-branded build
#
# The app links the same core crate as the probe, whose build bakes the site
# (AGENT_RELAY_SITE_URL) and link scheme (AGENT_RELAY_URL_SCHEME) in and
# which the app reads for its sign-in screen and links. The rest of the
# build's site profile (docs/workspace-layout.md, "Site profiles") is the
# app's own: its product name (DESKTOP_PRODUCT_NAME), the GitHub repository
# its updater reads releases from (RELEASES_REPOSITORY), its release feed
# (RELEASE_FEED: github:<owner>/<name> or cloud) with a cloud feed's public
# key (DESKTOP_FEED_PUBLIC_KEY), what those releases' artifact names start
# with (DESKTOP_ASSET_PREFIX and DESKTOP_PROFILE_ID, from --profile) and the
# mark it draws (DESKTOP_ICON, an SVG) and, for a co-branded build, the
# customer it says it is "for" under the lockup (DESKTOP_COBRAND_MARK, an
# SVG or a PNG, and DESKTOP_COBRAND_NAME), each read by gtk/build.rs.
# The app prints none of them, so they are recorded beside it, and
# build-linux.sh packages an app only with the core's site and scheme.
#
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$REPO_ROOT/gtk/Cargo.toml"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=site-origin.sh
. "$SCRIPT_DIR/site-origin.sh"

usage() {
  cat <<EOF
Usage:
  ${SCRIPT_NAME} --version VERSION --out DIR [--target TRIPLE]
                 [--site-url URL] [--url-scheme NAME] [--profile ID]
                 [--product-name NAME] [--releases-repository OWNER/NAME]
                 [--release-feed FEED] [--feed-public-key KEY] [--icon FILE]
                 [--cobrand-mark FILE --cobrand-name NAME]

Options:
  --version VERSION  The desktop release version the app reports. Required.
  --out DIR          Directory the app, its .sha256 and the site and scheme
                     records are written to. Created when missing. Required.
  --target TRIPLE    Rust target, for example aarch64-unknown-linux-gnu; the
                     host's when omitted.
  --site-url URL     The Agent Relay site the app is built for:
                     AGENT_RELAY_SITE_URL, which an exported value also
                     sets; https://agentrelay.com when neither is given.
  --url-scheme NAME  The scheme of the links the app opens:
                     AGENT_RELAY_URL_SCHEME, which an exported value also
                     sets; agentrelay when neither is given.
  --profile ID       The site profile's opaque id: the release artifacts the
                     updater looks for are named Desktop-ID-<version>-...;
                     AgentRelay-<version>-... for the default profile (no id).
  --product-name NAME
                     The product name the app shows: DESKTOP_PRODUCT_NAME,
                     which an exported value also sets; "Agent Relay" when
                     neither is given.
  --releases-repository OWNER/NAME
                     The GitHub repository the updater reads releases from:
                     RELEASES_REPOSITORY, which an exported value also
                     sets; AgentWorkforce/relay-desktop-releases when
                     neither is given. None for a cloud feed.
  --release-feed FEED
                     Where the updater reads releases: RELEASE_FEED, which an
                     exported value also sets. github:OWNER/NAME names the
                     releases repository (and sets it when that is not
                     given); cloud is the profile's own site, and needs
                     --profile, a --site-url of its own and
                     --feed-public-key. github: with the releases repository
                     when neither is given.
  --feed-public-key KEY
                     A cloud feed's Ed25519 public key, the base64 of 32
                     bytes: DESKTOP_FEED_PUBLIC_KEY, which an exported value
                     also sets. Only a cloud feed takes one.
  --icon FILE        The app's mark, an SVG, drawn in the window and shipped
                     as its launcher icon; assets/agent-relay-mark.svg when
                     omitted.
  --cobrand-mark FILE
                     A co-branded build's customer mark, an SVG or a PNG,
                     drawn in the "for <customer>" line under the lockup of
                     the sign-in window. Needs --cobrand-name and --profile.
  --cobrand-name NAME
                     The customer a co-branded build says it is "for": one
                     line of at most 80 characters, trimmed. Needs
                     --cobrand-mark and --profile; never printed.
EOF
}

die() { printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2; exit 1; }
note() { printf '%s: %s\n' "$SCRIPT_NAME" "$*"; }

TARGET=""
VERSION=""
OUT_DIR=""
SITE_URL=""
URL_SCHEME=""
PROFILE=""
PRODUCT_NAME=""
RELEASES_REPO=""
FEED=""
FEED_KEY=""
ICON=""
COBRAND_MARK=""
COBRAND_NAME=""
COBRAND_NAME_SET=""
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --out) OUT_DIR="${2:-}"; shift 2 ;;
    --site-url) SITE_URL="${2:-}"; shift 2 ;;
    --url-scheme) URL_SCHEME="${2:-}"; shift 2 ;;
    --profile) PROFILE="${2:-}"; shift 2 ;;
    --product-name) PRODUCT_NAME="${2:-}"; shift 2 ;;
    --releases-repository) RELEASES_REPO="${2:-}"; shift 2 ;;
    --release-feed) FEED="${2:-}"; shift 2 ;;
    --feed-public-key) FEED_KEY="${2:-}"; shift 2 ;;
    --icon) ICON="${2:-}"; shift 2 ;;
    --cobrand-mark) COBRAND_MARK="${2:-}"; shift 2 ;;
    --cobrand-name) COBRAND_NAME="${2:-}"; COBRAND_NAME_SET=1; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done
[ -n "$VERSION" ] || die "--version is required"
[ -n "$OUT_DIR" ] || die "--out is required"
command -v cargo >/dev/null 2>&1 || die "cargo is required; install a Rust toolchain"

built_in_from "$SITE_URL" "$URL_SCHEME" || die "--site-url or --url-scheme refused"

# The profile's other values follow the same rule as the site: the option,
# else the exported variable (said aloud), else the default; each checked,
# then exported for gtk/build.rs, or unset so the default applies.
if [ -n "$PROFILE" ]; then
  PROFILE="$(profile_id "$PROFILE")" || die "--profile must be an opaque id of lowercase letters, digits and hyphens"
fi
ASSET_PREFIX="$(asset_prefix "$PROFILE")"
if [ -z "$PRODUCT_NAME" ] && [ -n "${DESKTOP_PRODUCT_NAME:-}" ]; then
  PRODUCT_NAME="$DESKTOP_PRODUCT_NAME"
  note "DESKTOP_PRODUCT_NAME is set: building with the product name $PRODUCT_NAME"
fi
if [ -n "$PRODUCT_NAME" ]; then
  PRODUCT_NAME="$(product_name "$PRODUCT_NAME")" || die "--product-name must be a name with no control characters or slashes, got: $PRODUCT_NAME"
else
  PRODUCT_NAME="$DEFAULT_PRODUCT_NAME"
fi
if [ -z "$RELEASES_REPO" ] && [ -n "${RELEASES_REPOSITORY:-}" ]; then
  RELEASES_REPO="$RELEASES_REPOSITORY"
  note "RELEASES_REPOSITORY is set: building with the releases repository $RELEASES_REPO"
fi
if [ -z "$FEED" ] && [ -n "${RELEASE_FEED:-}" ]; then
  FEED="$RELEASE_FEED"
  note "RELEASE_FEED is set: building with the release feed $FEED"
fi
if [ -z "$FEED_KEY" ] && [ -n "${DESKTOP_FEED_PUBLIC_KEY:-}" ]; then
  FEED_KEY="$DESKTOP_FEED_PUBLIC_KEY"
  note "DESKTOP_FEED_PUBLIC_KEY is set: building with its feed public key"
fi
# The feed and the repository settle together: github:OWNER/NAME is that
# repository's releases, and cloud has none (site-origin.sh).
settle_release_feed "$FEED" "$RELEASES_REPO" "$FEED_KEY" "$BUILT_IN_SITE" "$PROFILE" \
  || die "--release-feed, --releases-repository or --feed-public-key refused"
FEED="$SETTLED_FEED"
RELEASES_REPO="$SETTLED_REPOSITORY"
FEED_KEY="$SETTLED_KEY"
if [ -n "$ICON" ]; then
  [ -f "$ICON" ] || die "--icon does not exist: $ICON"
  grep -q '<svg' "$ICON" || die "--icon must be an SVG: $ICON"
  ICON="$(cd "$(dirname "$ICON")" && pwd)/$(basename "$ICON")"
fi
# A co-branded build takes the customer's mark and name together, only for a
# site profile, and checks the name as the app does (AppInfo.coBrandName and
# build_profile.rs): trimmed, not blank, one line, at most 80 characters. A
# refusal gives the name's length, never the name.
if [ -n "$COBRAND_NAME_SET" ] && [ -z "$COBRAND_MARK" ]; then die "--cobrand-name needs --cobrand-mark"; fi
if [ -n "$COBRAND_MARK" ]; then
  [ -n "$COBRAND_NAME_SET" ] || die "--cobrand-mark needs --cobrand-name"
  [ -n "$PROFILE" ] || die "--cobrand-mark and --cobrand-name need --profile; the default profile is Agent Relay's own"
  [ -f "$COBRAND_MARK" ] || die "--cobrand-mark: no file at $COBRAND_MARK"
  case "$COBRAND_MARK" in
    *.svg|*.SVG) grep -q '<svg' "$COBRAND_MARK" || die "--cobrand-mark must be an SVG or a PNG; $COBRAND_MARK is not an SVG" ;;
    *.png|*.PNG) [ "$(head -c 4 "$COBRAND_MARK" | od -An -tx1 | tr -d ' \n')" = 89504e47 ] \
                   || die "--cobrand-mark must be an SVG or a PNG; $COBRAND_MARK is not a PNG" ;;
    *) die "--cobrand-mark must be an SVG or a PNG, got: $COBRAND_MARK" ;;
  esac
  COBRAND_NAME="${COBRAND_NAME#"${COBRAND_NAME%%[![:space:]]*}"}"
  COBRAND_NAME="${COBRAND_NAME%"${COBRAND_NAME##*[![:space:]]}"}"
  [ -n "$COBRAND_NAME" ] || die "--cobrand-name must not be empty or blank"
  [[ "$COBRAND_NAME" != *[[:cntrl:]]* ]] || die "--cobrand-name must be one line with no control characters"
  [ "${#COBRAND_NAME}" -le 80 ] || die "--cobrand-name must be at most 80 characters; it has ${#COBRAND_NAME}"
  COBRAND_MARK="$(cd "$(dirname "$COBRAND_MARK")" && pwd)/$(basename "$COBRAND_MARK")"
fi
# What gtk/build.rs reads. The default of each is applied there, so an
# unset variable and the default value build the same binary.
if [ "$PRODUCT_NAME" != "$DEFAULT_PRODUCT_NAME" ]; then export DESKTOP_PRODUCT_NAME="$PRODUCT_NAME"; else unset DESKTOP_PRODUCT_NAME; fi
if [ "$RELEASES_REPO" != "$DEFAULT_RELEASES_REPOSITORY" ]; then export RELEASES_REPOSITORY="$RELEASES_REPO"; else unset RELEASES_REPOSITORY; fi
if [ "$FEED" != "$DEFAULT_RELEASE_FEED" ]; then export RELEASE_FEED="$FEED"; else unset RELEASE_FEED; fi
if [ -n "$FEED_KEY" ]; then export DESKTOP_FEED_PUBLIC_KEY="$FEED_KEY"; else unset DESKTOP_FEED_PUBLIC_KEY; fi
if [ "$ASSET_PREFIX" != "$DEFAULT_ASSET_PREFIX" ]; then export DESKTOP_ASSET_PREFIX="$ASSET_PREFIX"; else unset DESKTOP_ASSET_PREFIX; fi
if [ -n "$PROFILE" ]; then export DESKTOP_PROFILE_ID="$PROFILE"; else unset DESKTOP_PROFILE_ID; fi
if [ -n "$ICON" ]; then export DESKTOP_ICON="$ICON"; else unset DESKTOP_ICON; fi
if [ -n "$COBRAND_MARK" ]; then
  export DESKTOP_COBRAND_MARK="$COBRAND_MARK" DESKTOP_COBRAND_NAME="$COBRAND_NAME"
else
  unset DESKTOP_COBRAND_MARK DESKTOP_COBRAND_NAME
fi
[ -z "$PROFILE" ] || note "building the site profile $PROFILE (artifacts named $ASSET_PREFIX-...)"

BUILD=(build --manifest-path "$MANIFEST" --release --locked --features gtk --bin agent-relay)
TARGET_ROOT="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
BUILT="$TARGET_ROOT/release/agent-relay"
if [ -n "$TARGET" ]; then
  BUILD+=(--target "$TARGET")
  BUILT="$TARGET_ROOT/$TARGET/release/agent-relay"
fi
note "building agent-relay $VERSION${TARGET:+ for $TARGET}"
AGENT_RELAY_PROBE_VERSION="$VERSION" cargo "${BUILD[@]}"
[ -f "$BUILT" ] || die "cargo reported success but no binary at $BUILT"

mkdir -p "$OUT_DIR"
install -m 0755 "$BUILT" "$OUT_DIR/agent-relay"
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$OUT_DIR" && sha256sum agent-relay > agent-relay.sha256 )
else
  ( cd "$OUT_DIR" && shasum -a 256 agent-relay > agent-relay.sha256 )
fi
chmod 0644 "$OUT_DIR/agent-relay.sha256"
record_built_in "$OUT_DIR" agent-relay
printf '%s\n' "$PRODUCT_NAME" > "$OUT_DIR/agent-relay-product-name.txt"
printf '%s\n' "$RELEASES_REPO" > "$OUT_DIR/agent-relay-releases-repository.txt"
# The feed's records exist only for a feed other than the default's, so a
# default build's records are what they were before feeds.
rm -f "$OUT_DIR/agent-relay-release-feed.txt" "$OUT_DIR/agent-relay-feed-public-key.txt"
if [ "$FEED" != "$DEFAULT_RELEASE_FEED" ]; then
  printf '%s\n' "$FEED" > "$OUT_DIR/agent-relay-release-feed.txt"
fi
if [ -n "$FEED_KEY" ]; then
  printf '%s\n' "$FEED_KEY" > "$OUT_DIR/agent-relay-feed-public-key.txt"
fi
# The co-brand's record exists only for a co-branded build, so a build
# without one has the records it had before.
rm -f "$OUT_DIR/agent-relay-cobrand-name.txt"
if [ -n "$COBRAND_MARK" ]; then
  printf '%s\n' "$COBRAND_NAME" > "$OUT_DIR/agent-relay-cobrand-name.txt"
fi
note "agent-relay $VERSION ready in $OUT_DIR"

#!/usr/bin/env bash
# Build the portable Windows tree and zips of Agent Relay under MSYS2.
#
# The Windows app is the Linux GTK 4 app (`agent-relay`, the probe crate's
# `gtk` feature) built natively with MSYS2's CLANG64 (x64) or CLANGARM64
# (arm64) toolchain. GTK links dynamically, so the tree carries the GTK
# runtime beside the two executables, laid out as an MSYS2 prefix
# (agent-relay-probe's src/install/windows.rs describes it to the app):
#
#   AgentRelay-Windows-<arch>/
#     bin/agent-relay.exe              the app
#     bin/agent-relay-probe.exe        the relay core
#     bin/*.dll                        every DLL either needs from the MSYS2 prefix
#     bin/gdbus.exe                    the session bus GApplication uses to stay one instance
#     bin/gspawn-win64-helper*.exe     GLib's process-spawn helpers
#     lib/gdk-pixbuf-2.0/2.10.0/       image loaders (PNG, JPEG, GIF, BMP, ICO, SVG) and loaders.cache
#     share/glib-2.0/schemas/gschemas.compiled
#     share/gtk-4.0/                   GTK's data (emoji chooser translations)
#     share/icons/hicolor/             index.theme and the Agent Relay icon
#     share/icons/Adwaita/             index.theme and the symbolic icons
#     share/icons/agent-relay.ico      the Agent Relay icon for Windows
#     NOTICES.txt                      the bundled packages, their licenses and license texts
#
# Then, in --out (AgentRelay- is the default site profile's prefix; a
# profile's artifacts are Desktop-<id>-..., see --profile):
#   AgentRelay-<version>-Windows-<arch>.zip (+ .sha256)        the tree above
#   AgentRelay-<version>-Windows-<arch>-probe.zip (+ .sha256)  the bare core:
#     agent_relay/helpers/agent-relay-probe.exe and the DLLs it needs
#   AgentRelay-<version>-Windows-<arch>-Setup.exe (+ .sha256)  the per-user
#     installer (windows/installer/AgentRelay.iss), when Inno Setup 6 is found
#
# The .ico is rendered from the SVG mark (--icon, else the app's own) with
# rsvg-convert; no binary asset is checked in. When this script builds the
# executables it embeds the icon, the version and the product name in them
# (AGENT_RELAY_WINDOWS_ICON and DESKTOP_PRODUCT_NAME, read by
# desktop-core/build.rs and gtk/build.rs), and a co-branded app the
# customer it says it is "for" under the lockup (DESKTOP_COBRAND_MARK and
# DESKTOP_COBRAND_NAME, read by gtk/build.rs), whose name it records beside
# the app it stages (agent-relay-cobrand-name.txt).
set -euo pipefail
shopt -s nullglob

SCRIPT_NAME="$(basename "$0")"

usage() {
  cat <<'EOF'
Usage: build-windows.sh --version VERSION [--arch x64|arm64] [--out DIR]
                        [--app FILE] [--probe FILE] [--installer|--no-installer]
                        [--build-only core|app [--runtime-out DIR]]
                        [--site-url URL] [--url-scheme NAME] [--profile ID]
                        [--product-name NAME] [--releases-repository OWNER/NAME]
                        [--release-feed FEED] [--feed-public-key KEY]
                        [--support-url URL] [--icon FILE]
                        [--cobrand-mark FILE --cobrand-name NAME]

Builds the portable Windows tree and zips from a built app and relay core.
Run it from an MSYS2 shell matching --arch: CLANG64 for x64, CLANGARM64 for
arm64, with the toolchain, rust, gtk4, pkgconf, librsvg and ntldd packages
plus zip.

  --version VERSION  The release version (AGENT_RELAY_PROBE_VERSION). Required.
  --arch x64|arm64   Default x64.
  --out DIR          Default dist/windows.
  --app FILE         `agent-relay.exe`, built with the probe crate's `gtk`
                     feature, with agent-relay-site.txt and
                     agent-relay-url-scheme.txt beside it saying which site
                     and link scheme it is built for: `--build-only app`
                     stages it so, and nothing else on Windows writes them.
                     Built here (release, icon embedded) when omitted.
  --probe FILE       `agent-relay-probe.exe` with probe-site.txt and
                     probe-url-scheme.txt beside it, which `--build-only
                     core` and scripts/build-probe.sh write. Built here when
                     omitted.
  --installer        Require the Setup .exe: fail when Inno Setup 6's
                     ISCC.exe is not found. Without it the installer is built
                     when ISCC.exe is found ($ISCC, PATH, or Inno Setup 6's
                     machine or per-user install folder) and skipped otherwise.
  --no-installer     Do not build the Setup .exe.
  --build-only PART  Build only core or app, stage it with the records of its
                     site and link scheme beside it in --out/built/PART/, print
                     that path (pass it back with --app or --probe) and exit.
                     Renders the same icon as a complete package build.
  --runtime-out DIR  With --build-only: stage that component and all its
                     runtime files with a hash manifest for package-windows.py
                     in DIR instead.

The site profile the package is for (docs/workspace-layout.md, "Site
profiles"), each the default's when omitted. The options alone count: a
value exported in the shell never reaches a build here.
  --site-url URL     The site the core signs in to: https://agentrelay.com.
  --url-scheme NAME  The scheme of the links both executables open, the
                     installer's handler: agentrelay.
  --profile ID       The profile's opaque id: the zips and the Setup .exe are
                     named Desktop-ID-<version>-...; AgentRelay-... for the
                     default profile (no id).
  --product-name NAME
                     The product name the app, the executables' properties
                     and the installer show: "Agent Relay".
  --releases-repository OWNER/NAME
                     The GitHub repository the updater reads releases from:
                     AgentWorkforce/relay-desktop-releases. None for a cloud
                     feed.
  --release-feed FEED
                     Where the updater reads releases: github:OWNER/NAME
                     names the releases repository (and sets it when that is
                     not given); cloud is the profile's own site, and needs
                     --profile, a --site-url of its own and
                     --feed-public-key. github: with the releases repository
                     when omitted.
  --feed-public-key KEY
                     A cloud feed's Ed25519 public key, the base64 of 32
                     bytes. Only a cloud feed takes one.
  --support-url URL  The installer's support link: the releases repository's
                     page; for a cloud feed the site's downloads page,
                     SITE/cloud/dashboard/downloads.
  --icon FILE        The app's mark, an SVG, rendered into the .ico and shipped
                     as the tree's icon: gtk/resources/linux/agent-relay-mark.svg.
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

version=""
app=""
probe=""
arch="x64"
out="dist/windows"
installer="auto"
build_only=""
runtime_out=""
site_url=""
url_scheme=""
profile=""
product=""
releases_repo=""
release_feed=""
feed_key=""
support_url=""
icon_svg=""
cobrand_mark=""
cobrand_name=""
cobrand_name_set=""
while [ $# -gt 0 ]; do
  case "$1" in
    --runtime-out) runtime_out="${2:?}"; shift 2 ;;
    --build-only) build_only="${2:?}"; shift 2 ;;
    --site-url) site_url="${2:?}"; shift 2 ;;
    --url-scheme) url_scheme="${2:?}"; shift 2 ;;
    --profile) profile="${2:?}"; shift 2 ;;
    --product-name) product="${2:?}"; shift 2 ;;
    --releases-repository) releases_repo="${2:?}"; shift 2 ;;
    --release-feed) release_feed="${2:?}"; shift 2 ;;
    --feed-public-key) feed_key="${2:?}"; shift 2 ;;
    --support-url) support_url="${2:?}"; shift 2 ;;
    --icon) icon_svg="${2:?}"; shift 2 ;;
    --cobrand-mark) cobrand_mark="${2:?}"; shift 2 ;;
    --cobrand-name) cobrand_name="${2-}"; cobrand_name_set=1; shift 2 ;;
    --version) version="${2:?}"; shift 2 ;;
    --app) app="${2:?}"; shift 2 ;;
    --probe) probe="${2:?}"; shift 2 ;;
    --arch) arch="${2:?}"; shift 2 ;;
    --out) out="${2:?}"; shift 2 ;;
    --installer) installer="yes"; shift ;;
    --no-installer) installer="no"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done
[ -n "$version" ] || { usage >&2; exit 1; }
case "$build_only" in ""|core|app) ;; *) die "--build-only must be core or app" ;; esac
[ -z "$runtime_out" ] || [ -n "$build_only" ] || die "--runtime-out requires --build-only"
case "$arch" in
  x64) msystem=CLANG64; prefix=/clang64; triple=x86_64-pc-windows-gnullvm ;;
  arm64) msystem=CLANGARM64; prefix=/clangarm64; triple=aarch64-pc-windows-gnullvm ;;
  *) die "unsupported arch: $arch" ;;
esac
[ "${MSYSTEM:-}" = "$msystem" ] || die "run from an MSYS2 $msystem shell for $arch (MSYSTEM is '${MSYSTEM:-unset}')"
[ -d "$prefix/bin" ] || die "$prefix is missing; install the mingw-w64-clang packages for $msystem"
for tool in cygpath ntldd zip sha256sum glib-compile-schemas; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done


# Inno Setup 6's compiler: $ISCC, PATH, then where its installer (or winget,
# per user, under %LOCALAPPDATA%: CSIDL 28) puts it.
iscc=""
if [ "$installer" != "no" ] && [ -z "$build_only" ]; then
  for candidate in "${ISCC:-}" "$(command -v ISCC.exe 2>/dev/null || command -v iscc 2>/dev/null || true)" \
    "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "/c/Program Files/Inno Setup 6/ISCC.exe" \
    "$(cygpath -u -F 28 2>/dev/null)/Programs/Inno Setup 6/ISCC.exe"; do
    if [ -n "$candidate" ] && [ -f "$candidate" ]; then iscc="$candidate"; break; fi
  done
  if [ -z "$iscc" ] && [ "$installer" = "yes" ]; then
    die "Inno Setup 6 (ISCC.exe) is required for --installer; install it (winget install JRSoftware.InnoSetup) or set ISCC"
  fi
fi

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=site-origin.sh
. "$repo/scripts/site-origin.sh"

# --- the site profile --------------------------------------------------------

# The package is for one site profile: the options, else the default's. The
# core signs in to the site it was built for (AGENT_RELAY_SITE_URL) and both
# executables open links of the scheme they were built with
# (AGENT_RELAY_URL_SCHEME; the app links the same core crate), so each value
# is given to cargo below from the option alone, and a value exported in the
# shell never leaks into either build.
site="https://agentrelay.com"
scheme="agentrelay"
if [ -n "$site_url" ]; then site="$(site_origin "$site_url")" || die "--site-url must be an origin such as https://relay.example, got: $site_url"; fi
if [ -n "$url_scheme" ]; then scheme="$(url_scheme_name "$url_scheme")" || die "--url-scheme must be a name such as agentrelay, got: $url_scheme"; fi
if [ -n "$profile" ]; then profile="$(profile_id "$profile")" || die "--profile must be an opaque id of lowercase letters, digits and hyphens, got: $profile"; fi
asset_prefix="$(asset_prefix "$profile")"
if [ -n "$product" ]; then product="$(product_name "$product")" || die "--product-name must have no control characters or slashes, got: $product"; else product="$DEFAULT_PRODUCT_NAME"; fi
# The feed and the repository settle together: github:OWNER/NAME is that
# repository's releases, and cloud has none (site-origin.sh).
settle_release_feed "$release_feed" "$releases_repo" "$feed_key" "$site" "$profile" \
  || die "--release-feed, --releases-repository or --feed-public-key refused"
release_feed="$SETTLED_FEED"
releases_repo="$SETTLED_REPOSITORY"
feed_key="$SETTLED_KEY"
if [ -n "$support_url" ]; then
  [[ "$support_url" =~ ^https://[^[:space:]\"]+$ ]] || die "--support-url must be an https URL, got: $support_url"
elif [ "$release_feed" = cloud ]; then
  support_url="$site/cloud/dashboard/downloads"
else
  support_url="https://github.com/$releases_repo"
fi
if [ -n "$icon_svg" ]; then
  [ -f "$icon_svg" ] || die "--icon does not exist: $icon_svg"
  grep -q '<svg' "$icon_svg" || die "--icon must be an SVG: $icon_svg"
  # Absolute before cargo sees it: gtk/build.rs runs with gtk/ as its
  # working directory, and `cygpath -m` keeps a relative path relative.
  icon_svg="$(cd "$(dirname "$icon_svg")" && pwd)/$(basename "$icon_svg")"
fi
# A co-branded build takes the customer's mark and name together, only for a
# site profile, and checks the name as the app does (build_profile.rs, as
# scripts/build-gtk.sh): trimmed, not blank, one line, at most 80
# characters. A refusal gives the name's length, never the name.
if [ -n "$cobrand_name_set" ] && [ -z "$cobrand_mark" ]; then die "--cobrand-name needs --cobrand-mark"; fi
if [ -n "$cobrand_mark" ]; then
  [ -n "$cobrand_name_set" ] || die "--cobrand-mark needs --cobrand-name"
  [ -n "$profile" ] || die "--cobrand-mark and --cobrand-name need --profile; the default profile is Agent Relay's own"
  [ -f "$cobrand_mark" ] || die "--cobrand-mark: no file at $cobrand_mark"
  case "$cobrand_mark" in
    *.svg|*.SVG) grep -q '<svg' "$cobrand_mark" || die "--cobrand-mark must be an SVG or a PNG; $cobrand_mark is not an SVG" ;;
    *.png|*.PNG) [ "$(head -c 4 "$cobrand_mark" | od -An -tx1 | tr -d ' \n')" = 89504e47 ] \
                   || die "--cobrand-mark must be an SVG or a PNG; $cobrand_mark is not a PNG" ;;
    *) die "--cobrand-mark must be an SVG or a PNG, got: $cobrand_mark" ;;
  esac
  cobrand_name="${cobrand_name#"${cobrand_name%%[![:space:]]*}"}"
  cobrand_name="${cobrand_name%"${cobrand_name##*[![:space:]]}"}"
  [ -n "$cobrand_name" ] || die "--cobrand-name must not be empty or blank"
  [[ "$cobrand_name" != *[[:cntrl:]]* ]] || die "--cobrand-name must be one line with no control characters"
  # Its length in characters, as gtk/build.rs counts them, whatever the
  # locale: its bytes but UTF-8's continuation bytes (bash's ${#} counts
  # bytes under LC_ALL=C).
  cobrand_name_length="$(printf '%s' "$cobrand_name" | LC_ALL=C tr -d '\200-\277' | wc -c | tr -d ' ')"
  [ "$cobrand_name_length" -le 80 ] || die "--cobrand-name must be at most 80 characters; it has $cobrand_name_length"
  # Absolute before cargo sees it, as the icon.
  cobrand_mark="$(cd "$(dirname "$cobrand_mark")" && pwd)/$(basename "$cobrand_mark")"
fi
[ -z "$profile" ] || note "building the site profile $profile (artifacts named $asset_prefix-...)"
# What each cargo build sees of the profile: nothing from the shell, then
# the values that differ from the default (each build script applies the
# default to an unset variable, so the default profile builds as before).
cargo_env=(env -u AGENT_RELAY_SITE_URL -u AGENT_RELAY_URL_SCHEME -u DESKTOP_PRODUCT_NAME
  -u RELEASES_REPOSITORY -u DESKTOP_ASSET_PREFIX -u DESKTOP_ICON
  -u RELEASE_FEED -u DESKTOP_FEED_PUBLIC_KEY -u DESKTOP_PROFILE_ID
  -u DESKTOP_COBRAND_MARK -u DESKTOP_COBRAND_NAME)
[ "$site" = "https://agentrelay.com" ] || cargo_env+=("AGENT_RELAY_SITE_URL=$site")
[ "$scheme" = "agentrelay" ] || cargo_env+=("AGENT_RELAY_URL_SCHEME=$scheme")
[ "$product" = "$DEFAULT_PRODUCT_NAME" ] || cargo_env+=("DESKTOP_PRODUCT_NAME=$product")
[ "$releases_repo" = "$DEFAULT_RELEASES_REPOSITORY" ] || cargo_env+=("RELEASES_REPOSITORY=$releases_repo")
[ "$asset_prefix" = "$DEFAULT_ASSET_PREFIX" ] || cargo_env+=("DESKTOP_ASSET_PREFIX=$asset_prefix")
[ "$release_feed" = "$DEFAULT_RELEASE_FEED" ] || cargo_env+=("RELEASE_FEED=$release_feed")
[ -z "$feed_key" ] || cargo_env+=("DESKTOP_FEED_PUBLIC_KEY=$feed_key")
[ -z "$profile" ] || cargo_env+=("DESKTOP_PROFILE_ID=$profile")
[ -z "$icon_svg" ] || cargo_env+=("DESKTOP_ICON=$(cygpath -m "$icon_svg")")
[ -z "$cobrand_mark" ] || cargo_env+=("DESKTOP_COBRAND_MARK=$(cygpath -m "$cobrand_mark")" "DESKTOP_COBRAND_NAME=$cobrand_name")

source_revision=""
if [ -n "$runtime_out" ]; then
  if ! source_revision="$(git -C "$repo" rev-parse HEAD)"; then
    die "could not resolve the runtime source revision"
  fi
  [[ "$source_revision" =~ ^[0-9a-f]{40}$ ]] || die "runtime source revision must be a full Git SHA"
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `sha256sum` format, two spaces and no binary marker, as on Linux.
checksum() { # DIR NAME
  ( cd "$1" && printf '%s  %s\n' "$(sha256sum < "$2" | cut -d' ' -f1)" "$2" > "$2.sha256" )
}

# --- the icon ----------------------------------------------------------------

# PNG images at the sizes Windows asks for, packed into an ICO container
# (PNG entries, Windows Vista and later).
ico=""
ico_sizes=(16 20 24 32 40 48 64 256)
# shellcheck disable=SC2059 # the format is the byte being written
byte() { printf "\\$(printf '%03o' "$1")"; }
le16() { byte $(($1 & 255)); byte $((($1 >> 8) & 255)); }
le32() { le16 $(($1 & 65535)); le16 $((($1 >> 16) & 65535)); }
if command -v rsvg-convert >/dev/null 2>&1; then
  mkdir -p "$work/ico"
  if [ -n "$icon_svg" ]; then
    # A profile's mark is square (docs/workspace-layout.md, "Site profiles").
    cp "$icon_svg" "$work/ico/mark.svg"
  else
    # A square canvas: the mark is wider than tall, so it is centered.
    sed 's/width="112" height="91" viewBox="0 0 112 91"/width="112" height="112" viewBox="0 -10.5 112 112"/' \
      "$repo/gtk/resources/linux/agent-relay-mark.svg" > "$work/ico/mark.svg"
    grep -q 'viewBox="0 -10.5 112 112"' "$work/ico/mark.svg" || die "agent-relay-mark.svg changed size; update the square canvas here"
  fi
  for size in "${ico_sizes[@]}"; do
    rsvg-convert -w "$size" -h "$size" "$work/ico/mark.svg" -o "$work/ico/$size.png"
  done
  ico="$work/agent-relay.ico"
  {
    le16 0; le16 1; le16 "${#ico_sizes[@]}"
    offset=$((6 + 16 * ${#ico_sizes[@]}))
    for size in "${ico_sizes[@]}"; do
      bytes="$(wc -c < "$work/ico/$size.png")"
      dim=$((size == 256 ? 0 : size))
      byte "$dim"; byte "$dim"; byte 0; byte 0
      le16 1; le16 32; le32 "$bytes"; le32 "$offset"
      offset=$((offset + bytes))
    done
    for size in "${ico_sizes[@]}"; do cat "$work/ico/$size.png"; done
  } > "$ico"
else
  note "rsvg-convert is not installed (librsvg for $msystem): no agent-relay.ico, and the executables have no icon"
fi

# --- the executables ---------------------------------------------------------

if [ -z "$app" ] || [ -z "$probe" ]; then
  command -v cargo >/dev/null 2>&1 || die "cargo is required to build the app and core; or pass --app and --probe"
  host="$(rustc -vV | sed -n 's/^host: //p')"
  [ "$host" = "$triple" ] || die "the $msystem Rust toolchain builds for $triple, but rustc reports $host"
  target_dir="$(cygpath -u "${CARGO_TARGET_DIR:-$repo/target}")"
  icon_env=""
  if [ -n "$ico" ]; then icon_env="$(cygpath -m "$ico")"; fi
  # Each executable built here is for the profile resolved above
  # (`cargo_env`), and is staged at once, with the site and scheme it was
  # built for recorded beside it as scripts/build-probe.sh records them, in
  # this script's own work directory: a record beside cargo's shared target
  # would outlive the build it described. With --build-only and no
  # --runtime-out the staged copy is the result, so it goes under --out,
  # where it outlives this script, at the path printed below.
  built_root="$work/built"
  if [ -n "$build_only" ] && [ -z "$runtime_out" ]; then
    mkdir -p "$out"
    built_root="$(cd "$out" && pwd)/built"
  fi
  stage_built() { # EXE NAME -> prints the staged copy
    local staged="$built_root/$2"
    rm -rf "$staged"
    mkdir -p "$staged"
    cp "$1" "$staged/$(basename "$1")"
    printf '%s\n' "$site" > "$staged/$2-site.txt"
    printf '%s\n' "$scheme" > "$staged/$2-url-scheme.txt"
    # A co-branded app's customer, recorded as scripts/build-gtk.sh does.
    if [ "$2" = agent-relay ] && [ -n "$cobrand_mark" ]; then
      printf '%s\n' "$cobrand_name" > "$staged/$2-cobrand-name.txt"
    fi
    printf '%s\n' "$staged/$(basename "$1")"
  }
  # The core builds without `gtk` so it never links the GTK stack, as on Linux.
  if [ -z "$probe" ] && [ "$build_only" != "app" ]; then
    note "building agent-relay-probe $version for $triple"
    AGENT_RELAY_PROBE_VERSION="$version" AGENT_RELAY_WINDOWS_ICON="$icon_env" \
      "${cargo_env[@]}" \
      cargo build --manifest-path "$repo/desktop-core/Cargo.toml" --release --locked --target-dir "$target_dir" --bin agent-relay-desktop-probe
    probe="$(stage_built "$target_dir/release/agent-relay-desktop-probe.exe" probe)"
  fi
  if [ -z "$app" ] && [ "$build_only" != "core" ]; then
    note "building agent-relay $version for $triple"
    AGENT_RELAY_PROBE_VERSION="$version" AGENT_RELAY_WINDOWS_ICON="$icon_env" \
      "${cargo_env[@]}" \
      cargo build --manifest-path "$repo/gtk/Cargo.toml" --release --locked --target-dir "$target_dir" --features gtk --bin agent-relay
    app="$(stage_built "$target_dir/release/agent-relay.exe" agent-relay)"
  fi
fi
[ "$build_only" = core ] || [ -f "$app" ] || die "app does not exist: $app"
[ "$build_only" = app ] || [ -f "$probe" ] || die "probe does not exist: $probe"
# The core and the app must be built for the profile's site and scheme, by
# the records beside each (scripts/build-probe.sh's beside a core, this
# script's beside what it staged just above). An executable built
# elsewhere carries none and is not packaged; nothing else on Windows
# writes them, so build it here: --build-only core or app stages it with
# its records and prints the path to pass back.
if [ "$build_only" != app ]; then
  check_probe "$probe" "$site" "$scheme" \
    || die "not packaging that core: build it with this script (--build-only core) or scripts/build-probe.sh, which record its site and link scheme beside it, and pass that path"
fi
if [ "$build_only" != core ]; then
  check_sidecars "$app" agent-relay "$site" "$scheme" \
    || die "not packaging that app: build it with this script (--build-only app, or omit --app), which records its site and link scheme beside it"
fi
if [ -n "$build_only" ]; then
  if [ "$build_only" = core ]; then built="$probe"; else built="$app"; fi
  note "$build_only executable ready: $built"
  if [ -z "$runtime_out" ]; then exit 0; fi
fi

# --- DLL closure -------------------------------------------------------------

# Print the prefix DLLs FILE loads, directly or through other DLLs, one
# Windows path per line. System DLLs (Windows, API sets) are left to Windows;
# a missing lib*.dll, which only MSYS2 provides, is reported.
prefix_bin_win="$(cygpath -m "$prefix/bin" | tr '[:upper:]' '[:lower:]')"
dll_closure() {
  local listing
  # Keep stage context off stdout: copy_closure consumes only DLL paths.
  printf '%s: DLL closure ntldd capture start: %q\n' "$SCRIPT_NAME" "$1" >&2
  listing="$(ntldd -R "$1")" || die "ntldd could not read $1"
  printf '%s: DLL closure ntldd capture complete: %q\n' "$SCRIPT_NAME" "$1" >&2
  printf '%s\n' "$listing" | awk -v bin="$prefix_bin_win/" -v me="$SCRIPT_NAME" -v file="$1" '
    BEGIN { printf "%s: DLL closure awk filter start: %s\n", me, file > "/dev/stderr" }
    / => not found/ {
      if (tolower($1) ~ /^lib/)
        printf "%s: warning: %s needs %s, which was not found\n", me, file, $1 > "/dev/stderr"
      next
    }
    / => / {
      sub(/^.* => /, ""); sub(/ \(0x[0-9a-fA-F]*\)[[:space:]]*$/, ""); gsub(/[\\]/, "/")
      if (index(tolower($0), bin) == 1) print
    }
    END { printf "%s: DLL closure awk filter end: %s\n", me, file > "/dev/stderr" }'
}

copy_closure() { # DEST FILE...
  local dest="$1"; shift
  local file
  for file in "$@"; do dll_closure "$file"; done | sort -fu | cygpath -u -f - | xargs -r -d '\n' cp -t "$dest"
}

# --- the tree ----------------------------------------------------------------

name="AgentRelay-Windows-$arch"
root="$out/$name"
if [ -n "$runtime_out" ]; then root="$runtime_out/files"; fi
rm -rf "$root"
mkdir -p "$root/bin" "$root/lib" "$root/share/glib-2.0/schemas" "$root/share/icons"

if [ "$build_only" != core ]; then cp "$app" "$root/bin/agent-relay.exe"; fi
if [ "$build_only" != app ]; then cp "$probe" "$root/bin/agent-relay-probe.exe"; fi
cp "$repo/LICENSE" "$root/LICENSE.txt"
cp "$repo/assets/agent-icons-LICENSE.txt" "$root/agent-icons-LICENSE.txt"
if [ "$build_only" != core ]; then
# gdbus is the session bus GApplication keeps one instance on: GLib starts
# `gdbus.exe _win32_run_session_bus` from beside libgio the first time the
# app registers, and a later launch hands its command line (`--background`,
# agentrelay:// links) to the running app over it and exits. GLib runs the
# gspawn helpers to spawn child processes with redirected handles.
for helper in gdbus.exe gspawn-win64-helper.exe gspawn-win64-helper-console.exe; do
  [ -f "$prefix/bin/$helper" ] || die "$prefix/bin/$helper is missing; install glib2 for $msystem"
  cp "$prefix/bin/$helper" "$root/bin/"
done

# Image loaders: the formats the app and GTK draw, SVG for the icons. The
# loaders.cache paths are relative to the tree's root, so it copies as is,
# keeping only the loaders shipped.
pixbuf="lib/gdk-pixbuf-2.0/2.10.0"
loaders=(libpixbufloader-png.dll libpixbufloader-jpeg.dll libpixbufloader-gif.dll
  libpixbufloader-bmp.dll libpixbufloader-ico.dll pixbufloader_svg.dll libpixbufloader-svg.dll)
mkdir -p "$root/$pixbuf/loaders"
shipped=()
for loader in "${loaders[@]}"; do
  if [ -f "$prefix/$pixbuf/loaders/$loader" ]; then
    cp "$prefix/$pixbuf/loaders/$loader" "$root/$pixbuf/loaders/"
    shipped+=("$loader")
  fi
done
case " ${shipped[*]} " in *svg*) ;; *) die "the SVG pixbuf loader is missing; install librsvg for $msystem" ;; esac
awk -v keep="${shipped[*]}" '
  BEGIN { n = split(keep, names, " ") }
  /^#/ { print; next }
  /^"[^"]*[.]dll"$/ {
    on = 0
    for (i = 1; i <= n; i++) if (index($0, "\\\\" names[i] "\"") || index($0, "/" names[i] "\"")) on = 1
  }
  on { print }
' "$prefix/$pixbuf/loaders.cache" > "$root/$pixbuf/loaders.cache"
[ "$(grep -c '^"[^"]*[.]dll"$' "$root/$pixbuf/loaders.cache")" -eq "${#shipped[@]}" ] \
  || die "loaders.cache does not list every shipped loader"

copy_closure "$root/bin" "$root"/bin/*.exe "$root/$pixbuf/loaders"/*.dll

# GTK's settings schemas only, compiled for the tree.
mkdir -p "$work/schemas"
cp "$prefix"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$work/schemas/"
glib-compile-schemas --targetdir="$root/share/glib-2.0/schemas" "$work/schemas"

# GTK's data: the emoji chooser's translations (English is built in).
mkdir -p "$root/share/gtk-4.0"
if [ -d "$prefix/share/gtk-4.0/emoji" ]; then cp -r "$prefix/share/gtk-4.0/emoji" "$root/share/gtk-4.0/"; fi

# hicolor: the app's icon.
hicolor="$root/share/icons/hicolor"
mkdir -p "$hicolor/scalable/apps" "$hicolor/symbolic/apps"
if [ -f "$prefix/share/icons/hicolor/index.theme" ]; then cp "$prefix/share/icons/hicolor/index.theme" "$hicolor/"; fi
cp "${icon_svg:-$repo/gtk/resources/linux/agent-relay-mark.svg}" "$hicolor/scalable/apps/agent-relay.svg"
cp "$repo/gtk/resources/linux/agent-relay-symbolic.svg" "$hicolor/symbolic/apps/agent-relay-symbolic.svg"
# Symbolic icons the app names that Adwaita 47+ no longer ships.
# They go in symbolic/apps: the only symbolic directory hicolor's index.theme
# lists, and GTK looks only in listed directories.
cp "$repo"/gtk/resources/windows/icons/*-symbolic.svg "$hicolor/symbolic/apps/"
if [ -n "$ico" ]; then cp "$ico" "$root/share/icons/agent-relay.ico"; fi

# Adwaita, trimmed to the symbolic icons GTK and the app draw: no cursors
# (Windows draws its own), no full-color icons, and no icon-theme.cache, which
# would go stale and hide icons.
adwaita_src="$prefix/share/icons/Adwaita"
[ -d "$adwaita_src/symbolic" ] || die "$adwaita_src/symbolic is missing; install adwaita-icon-theme for $msystem"
mkdir -p "$root/share/icons/Adwaita"
cp "$adwaita_src/index.theme" "$root/share/icons/Adwaita/"
cp -r "$adwaita_src/symbolic" "$root/share/icons/Adwaita/"

fi
# The core component needs only its executable's DLL closure. The app branch
# above captured GTK helpers and pixbuf loaders as well.
if [ "$build_only" = core ]; then
  copy_closure "$root/bin" "$root/bin/agent-relay-probe.exe"
fi

# NOTICES.txt: which MSYS2 package each bundled file came from, its license,
# and the license texts the package installs.
notices="$root/NOTICES.txt"
{
  printf '%s %s for Windows (%s)\n\n' "$product" "$version" "$arch"
  printf 'agent-relay.exe and agent-relay-probe.exe are %s.' "$product"
} > "$notices"
bundled="$(cd "$root" && find bin lib share -type f ! -name 'agent-relay*' ! -name 'gschemas.compiled' ! -name 'loaders.cache' | sort)"
if [ -n "$bundled" ]; then
  {
    printf ' The other\n'
    printf 'files in this folder are unmodified builds from the MSYS2 project\n'
    printf '(https://www.msys2.org), redistributed under their own licenses below.\n'
    printf 'GTK, GLib and the other LGPL libraries are linked dynamically; you may\n'
    printf 'replace any DLL in bin with your own build of the same library.\n'
    printf 'Sources: https://packages.msys2.org and https://github.com/msys2/MINGW-packages\n\n'
  } >> "$notices"
fi
if [ -z "$bundled" ]; then
  # The gnullvm targets link libunwind statically (Rust 1.100), so a core
  # without GTK can need no MSYS2 file at all.
  note "no MSYS2 files are bundled with this component"
  printf '\n\nNo MSYS2 files are bundled with this component.\n' >> "$notices"
elif command -v pacman >/dev/null 2>&1; then
  # pacman is slow to start under MSYS2: one call for every file, then one for
  # every package. -Qo prints "<path> is owned by <package> <version>".
  owners="$(cd "$prefix" || exit 1; printf '%s\n' "$bundled" | xargs pacman -Qo 2>/dev/null || true)"
  packages="$(printf '%s\n' "$owners" | sed -n 's/^.* is owned by \([^ ]*\) .*$/\1/p' | sort -u)"
  [ -n "$packages" ] || die "pacman could not name the packages of the bundled files"
  {
    printf 'Bundled packages\n================\n\n'
    # shellcheck disable=SC2086 # one word per package
    pacman -Qi $packages | awk '
      /^Name +: /     { name = $3 }
      /^Version +: /  { version = $3 }
      /^URL +: /      { sub(/^URL +: /, ""); url = $0 }
      /^Licenses +: / { sub(/^Licenses +: /, ""); license = $0 }
      /^$/ && name    { printf "%s %s\n  License: %s\n  Upstream: %s\n", name, version, license, url; name = "" }
      END { if (name) printf "%s %s\n  License: %s\n  Upstream: %s\n", name, version, license, url }'
    # Libraries, loaders and helpers by name; the icons and data under share/
    # come from the icon-theme and GTK packages above.
    printf '\nLibraries\n=========\n\n'
    printf '%s\n' "$owners" | sed -n "s|^$prefix/\(bin/[^ ]*\|lib/[^ ]*\) is owned by \([^ ]*\) .*\$|\1 (\2)|p"
    printf '\nLicense texts\n=============\n'
    # shellcheck disable=SC2086 # one word per package
    pacman -Ql $packages | { grep '/share/licenses/.*[^/]$' || true; } | while read -r package text; do
      printf '\n----- %s: %s -----\n\n' "$package" "${text#"$prefix"/share/licenses/}"
      cat "$text"
    done
  } >> "$notices"
else
  note "pacman is not available: NOTICES.txt lists the bundled files without their packages and licenses"
  printf 'Files\n=====\n\n%s\n' "$bundled" >> "$notices"
fi

if [ -n "$runtime_out" ]; then
  python_command=python3
  if ! command -v "$python_command" >/dev/null 2>&1; then python_command=python; fi
  "$python_command" "$repo/scripts/package-windows.py" --record "$runtime_out" \
    --component "$build_only" --version "$version" --arch "$arch" \
    --source-sha "$source_revision" \
    --site-url "$site" --url-scheme "$scheme" --product-name "$product" \
    --asset-prefix "$asset_prefix" --releases-repository "$releases_repo" \
    --support-url "$support_url" --release-feed "$release_feed" --feed-public-key "$feed_key"
  note "$build_only runtime ready: $runtime_out"
  exit 0
fi

# --- archives ----------------------------------------------------------------

mkdir -p "$out"
out_abs="$(cd "$out" && pwd)"
zip_name="$asset_prefix-$version-Windows-$arch.zip"
rm -f "$out_abs/$zip_name"
( cd "$out_abs" && zip -qr -9 "$zip_name" "$name" )
checksum "$out_abs" "$zip_name"

# The bare core, as the Linux and macOS probe archives lay it out
# (agent_relay/helpers/agent-relay-probe, here with .exe), with the DLLs it
# needs beside it.
probe_name="$asset_prefix-$version-Windows-$arch-probe.zip"
helpers="$work/probe/agent_relay/helpers"
mkdir -p "$helpers"
cp "$probe" "$helpers/agent-relay-probe.exe"
cp "$repo/LICENSE" "$work/probe/agent_relay/LICENSE.txt"
copy_closure "$helpers" "$helpers/agent-relay-probe.exe"
rm -f "$out_abs/$probe_name"
( cd "$work/probe" && zip -qr -9 "$out_abs/$probe_name" agent_relay )
checksum "$out_abs" "$probe_name"

# --- the installer -----------------------------------------------------------

# A per-user Setup program that installs the tree where the zip goes,
# %LOCALAPPDATA%\Programs\AgentRelay\<version>\<tree>, so in-app updates keep
# working after it.
setup_name="$asset_prefix-$version-Windows-$arch-Setup.exe"
packages="$zip_name, $probe_name"
if [ -n "$iscc" ]; then
  note "building $setup_name with $(cygpath -w "$iscc")"
  rm -f "$out_abs/$setup_name"
  # ISCC's options start with /, which MSYS2 would otherwise rewrite as paths.
  MSYS2_ARG_CONV_EXCL='*' "$iscc" /Q \
    "/DVersion=$version" "/DArch=$arch" \
    "/DSourceDir=$(cygpath -w "$(cd "$root" && pwd)")" \
    "/DOutputDir=$(cygpath -w "$out_abs")" \
    "/DProductName=$product" "/DUrlScheme=$scheme" "/DAssetPrefix=$asset_prefix" \
    "/DSiteUrl=$site" "/DSupportUrl=$support_url" "/DReleaseFeed=$release_feed" \
    "$(cygpath -w "$repo/windows/installer/AgentRelay.iss")"
  [ -f "$out_abs/$setup_name" ] || die "ISCC did not write $setup_name"
  checksum "$out_abs" "$setup_name"
  packages="$packages, $setup_name"
elif [ "$installer" = "auto" ]; then
  note "Inno Setup 6 (ISCC.exe) was not found: no $setup_name"
fi

note "$(find "$root/bin" -name '*.dll' | wc -l) DLLs bundled; $(du -sh "$root" | cut -f1) unpacked, $(du -h "$out_abs/$zip_name" | cut -f1) zipped"
note "Windows package ready in $out: $packages"

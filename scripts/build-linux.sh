#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: build-linux.sh --version VERSION --app FILE --probe FILE [--arch x64|arm64] [--out DIR]
                      [--site-url URL] [--url-scheme NAME] [--profile ID]
                      [--product-name NAME] [--icon FILE]

Builds a portable Linux tree, tarball, and (when dpkg-deb or ar is installed)
.deb from a built app and relay core:

  --app FILE    `agent-relay` from scripts/build-gtk.sh (the probe crate's
                `gtk` feature), with its site and scheme records beside it;
                installed as `agent-relay-desktop`
  --probe FILE  `agent-relay-probe` (scripts/build-probe.sh)

Both are built for --arch with AGENT_RELAY_PROBE_VERSION=VERSION, and for
the site profile the package is for (docs/workspace-layout.md, "Site
profiles"), which the package's launcher entry and names follow:

  --site-url URL        The site both must be built for; https://agentrelay.com
                        when omitted.
  --url-scheme NAME     The link scheme both must open, the launcher entry's
                        handler; agentrelay when omitted.
  --profile ID          The profile's opaque id: the packages are named
                        Desktop-ID-<version>-Linux-<arch>.*; AgentRelay-...
                        for the default profile (no id).
  --product-name NAME   The launcher entry's name; "Agent Relay" when omitted.
  --icon FILE           The launcher icon, an SVG; the app's own mark
                        (gtk/resources/linux/agent-relay-mark.svg) when omitted.
EOF
}

version=""
app=""
probe=""
arch="x64"
out="dist/linux"
site_url=""
url_scheme=""
profile=""
product=""
icon=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?}"; shift 2 ;;
    --app) app="${2:?}"; shift 2 ;;
    --probe) probe="${2:?}"; shift 2 ;;
    --arch) arch="${2:?}"; shift 2 ;;
    --out) out="${2:?}"; shift 2 ;;
    --site-url) site_url="${2:?}"; shift 2 ;;
    --url-scheme) url_scheme="${2:?}"; shift 2 ;;
    --profile) profile="${2:?}"; shift 2 ;;
    --product-name) product="${2:?}"; shift 2 ;;
    --icon) icon="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; printf 'Unknown argument: %s\n' "$1" >&2; exit 1 ;;
  esac
done
[ -n "$version" ] || { usage >&2; exit 1; }
[ -f "$app" ] || { printf 'App does not exist: %s\n' "$app" >&2; exit 1; }
[ -f "$probe" ] || { printf 'Probe does not exist: %s\n' "$probe" >&2; exit 1; }
case "$arch" in x64) deb_arch=amd64 ;; arm64) deb_arch=arm64 ;; *) printf 'Unsupported arch: %s\n' "$arch" >&2; exit 1 ;; esac

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The core signs in to the site it was built for, and it and the app open
# links of the scheme they were built with (the app links the same core
# crate); nothing at run time changes either, and this package is for one
# site profile, so both must be built for its site and scheme (production's
# unless --site-url and --url-scheme say otherwise): the core by the records
# scripts/build-probe.sh writes beside it (else by what it prints), the app
# by the records scripts/build-gtk.sh writes beside it (it prints neither).
# shellcheck source-path=SCRIPTDIR
# shellcheck source=site-origin.sh
. "$repo/scripts/site-origin.sh"
site="https://agentrelay.com"
scheme="agentrelay"
if [ -n "$site_url" ]; then site="$(site_origin "$site_url")" || { printf 'The site must be an origin such as https://relay.example, got: %s\n' "$site_url" >&2; exit 1; }; fi
if [ -n "$url_scheme" ]; then scheme="$(url_scheme_name "$url_scheme")" || { printf 'The link scheme must be a name such as agentrelay, got: %s\n' "$url_scheme" >&2; exit 1; }; fi
if [ -n "$profile" ]; then profile="$(profile_id "$profile")" || { printf 'The profile must be an opaque id of lowercase letters, digits and hyphens, got: %s\n' "$profile" >&2; exit 1; }; fi
prefix="$(asset_prefix "$profile")"
if [ -n "$product" ]; then product="$(product_name "$product")" || { printf 'The product name must have no control characters or slashes, got: %s\n' "$product" >&2; exit 1; }; else product="$DEFAULT_PRODUCT_NAME"; fi
if [ -n "$icon" ]; then
  [ -f "$icon" ] || { printf 'Icon does not exist: %s\n' "$icon" >&2; exit 1; }
  grep -q '<svg' "$icon" || { printf 'The icon must be an SVG: %s\n' "$icon" >&2; exit 1; }
else
  icon="$repo/gtk/resources/linux/agent-relay-mark.svg"
fi
refuse() { printf 'Not packaging it: this package is built for %s and %s:// links.\n' "$site" "$scheme" >&2; exit 1; }
check_probe "$probe" "$site" "$scheme" || refuse
check_sidecars "$app" agent-relay "$site" "$scheme" || { printf 'Build the app with scripts/build-gtk.sh, which records its site and link scheme beside it.\n' >&2; refuse; }

# The launcher entry: the desktop lists the app by the product name and
# hands it links of the build's own scheme, which the running app also
# registers (`updates::install_launcher` rewrites the Exec line to the
# install's path and keeps the rest). A file in the repository could only
# name the default profile's.
write_launcher_entry() {
  cat > "$1" <<ENTRY
[Desktop Entry]
Type=Application
Name=$product
Comment=Connect your coding sessions to $product
Exec=agent-relay-desktop %u
Icon=agent-relay
Terminal=false
Categories=Development;
MimeType=x-scheme-handler/$scheme;
StartupNotify=true
ENTRY
}

# The tree agent-relay-probe's src/install.rs describes.
root="$out/AgentRelay-linux-$arch"
rm -rf "$root"
mkdir -p "$root/usr/bin" "$root/usr/lib/agent-relay" \
  "$root/usr/share/applications" "$root/usr/share/icons/hicolor/scalable/apps" \
  "$root/usr/lib/systemd/user"
# `agent-relay` is the npm CLI's name; the app never takes it on PATH.
install -m 0755 "$app" "$root/usr/bin/agent-relay-desktop"
install -m 0755 "$probe" "$root/usr/lib/agent-relay/agent-relay-probe"
write_launcher_entry "$root/usr/share/applications/agent-relay.desktop"
chmod 0644 "$root/usr/share/applications/agent-relay.desktop"
install -m 0644 "$icon" "$root/usr/share/icons/hicolor/scalable/apps/agent-relay.svg"
install -m 0644 "$repo/linux/agent-relay-headless.service" "$root/usr/lib/systemd/user/agent-relay.service"
install -D -m 0644 "$repo/LICENSE" "$root/usr/share/doc/agent-relay/LICENSE"
install -m 0644 "$repo/assets/agent-icons-LICENSE.txt" "$root/usr/share/doc/agent-relay/agent-icons-LICENSE.txt"

mkdir -p "$out"
# Updaters in releases up to v2026.10.57 find and relaunch this name in the
# tarball they download. Only the tarball has it; the .deb is built after it
# is removed, so the system package never installs /usr/bin/agent-relay.
ln -s agent-relay-desktop "$root/usr/bin/agent-relay"
tar -C "$out" -czf "$out/$prefix-$version-Linux-$arch.tar.gz" "$(basename "$root")"
sha256sum "$out/$prefix-$version-Linux-$arch.tar.gz" > "$out/$prefix-$version-Linux-$arch.tar.gz.sha256"
rm "$root/usr/bin/agent-relay"

# One Agent Relay app per machine: the package's file paths, binary,
# launcher entry and user unit are the same for every profile, and so is its
# name. Every profile's package is agent-relay, so dpkg -i or apt installs a
# profile's package over Agent Relay's (or Agent Relay's over a profile's, or
# one profile's over another's) as an ordinary upgrade, replacing it in place.
# Only the file name ($prefix, by the profile's opaque id) and the control
# file's Homepage and Description name the profile. Versions compare across
# profiles as within one: the same release scheme, a beta sorting before its
# release (~beta.).
write_control() {
  cat > "$1" <<EOF
Package: agent-relay
Version: ${version/-beta./~beta.}
Architecture: $deb_arch
Maintainer: Agent Relay <support@agentrelay.com>
Depends: libc6 (>= 2.34), libgtk-4-1 (>= 4.6), librsvg2-common
Section: devel
Priority: optional
Homepage: $site
Description: Native $product companion for Linux
 Connect coding sessions to $product with explicit upload controls.
EOF
}

deb="$out/$prefix-$version-Linux-$arch.deb"
deb="$(cd "$(dirname "$deb")" && pwd)/$(basename "$deb")"
if command -v dpkg-deb >/dev/null 2>&1; then
  mkdir -p "$root/DEBIAN"
  write_control "$root/DEBIAN/control"
  install -m 0755 "$repo/linux/agent-relay.postinst" "$root/DEBIAN/postinst"
  dpkg-deb --build --root-owner-group "$root" "$deb" >/dev/null
else
  command -v ar >/dev/null 2>&1 || { printf 'Neither dpkg-deb nor ar is available; cannot build .deb\n' >&2; exit 1; }
  deb_tmp="$out/.deb-$arch-$$"
  mkdir -p "$deb_tmp/control"
  write_control "$deb_tmp/control/control"
  install -m 0755 "$repo/linux/agent-relay.postinst" "$deb_tmp/control/postinst"
  printf '2.0\n' > "$deb_tmp/debian-binary"
  tar --numeric-owner --owner=0 --group=0 -C "$deb_tmp/control" -czf "$deb_tmp/control.tar.gz" .
  tar --numeric-owner --owner=0 --group=0 -C "$root" -czf "$deb_tmp/data.tar.gz" --exclude=./DEBIAN .
  ( cd "$deb_tmp" && ar r "$deb" debian-binary control.tar.gz data.tar.gz >/dev/null )
  rm -rf "$deb_tmp"
fi
sha256sum "$deb" > "$deb.sha256"
printf 'Linux package ready in %s\n' "$out"

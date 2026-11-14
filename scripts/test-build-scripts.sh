#!/usr/bin/env bash
# build-probe.sh and build-gtk.sh with a stand-in cargo: the site and link
# scheme a build is made for come from the options, else the exported
# variables, else production's; a value the core would refuse is refused
# before cargo runs; and the records beside each binary say what was built.
# The rest of the site profile reaches build-gtk.sh the same way, and
# build-linux.sh (on stand-in binaries) packages for it.
set -euo pipefail
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tools="$work/tools"
mkdir -p "$tools"
# cargo: logs its arguments and the two variables as it saw them, then
# writes the binary where the script expects it. rustc: a host no probe
# build targets here, so the bridge check (a real probe's) is skipped.
cat > "$tools/cargo" <<'CARGO'
#!/usr/bin/env bash
printf '%s site=%s scheme=%s product=%s repository=%s prefix=%s icon=%s\n' "$*" "${AGENT_RELAY_SITE_URL-unset}" "${AGENT_RELAY_URL_SCHEME-unset}" \
  "${DESKTOP_PRODUCT_NAME-unset}" "${RELEASES_REPOSITORY-unset}" "${DESKTOP_ASSET_PREFIX-unset}" "${DESKTOP_ICON-unset}" >> "$CARGO_LOG"
# The release feed's variables, in a log of their own.
printf 'feed=%s key=%s id=%s repository=%s\n' "${RELEASE_FEED-unset}" "${DESKTOP_FEED_PUBLIC_KEY-unset}" "${DESKTOP_PROFILE_ID-unset}" \
  "${RELEASES_REPOSITORY-unset}" >> "$CARGO_LOG.feed"
target=""; bin=""
while [ $# -gt 0 ]; do
  case "$1" in --target) target="$2"; shift 2 ;; --bin) bin="$2"; shift 2 ;; *) shift ;; esac
done
dir="$CARGO_TARGET_DIR${target:+/$target}/release"
mkdir -p "$dir"
printf '#!/bin/sh\n' > "$dir/$bin"
chmod +x "$dir/$bin"
CARGO
printf '#!/bin/sh\necho "host: x86_64-unknown-linux-gnu"\n' > "$tools/rustc"
chmod +x "$tools/cargo" "$tools/rustc"
export PATH="$tools:/usr/bin:/bin"
export CARGO_TARGET_DIR="$work/target"
export CARGO_LOG="$work/cargo.log"

unset AGENT_RELAY_SITE_URL AGENT_RELAY_URL_SCHEME DESKTOP_PRODUCT_NAME RELEASES_REPOSITORY DESKTOP_ASSET_PREFIX DESKTOP_ICON \
  RELEASE_FEED DESKTOP_FEED_PUBLIC_KEY DESKTOP_PROFILE_ID

failed=0
fail() { printf '%s\n' "$*"; failed=1; }
# `probe ENV... -- ARGS...` and `gtk ENV... -- ARGS...`: ENV as `env` takes it.
probe() { local e=(); while [ "$1" != -- ]; do e+=("$1"); shift; done; shift; env "${e[@]}" bash "$SCRIPTS/build-probe.sh" --target aarch64-unknown-linux-gnu --version 0.0.0 "$@"; }
gtk() { local e=(); while [ "$1" != -- ]; do e+=("$1"); shift; done; shift; env "${e[@]}" bash "$SCRIPTS/build-gtk.sh" --version 0.0.0 "$@"; }
last_call() { tail -n 1 "$CARGO_LOG"; }
records() { printf '%s %s' "$(cat "$1/$2-site.txt")" "$(cat "$1/$2-url-scheme.txt")"; }

# Options, normalized, reach cargo and the records.
probe -- --out "$work/p1" --site-url "HTTP://Localhost:3100/" --url-scheme AgentRelay-Dev >/dev/null || fail "build-probe.sh with options failed"
[[ "$(last_call)" == *"--bin agent-relay-desktop-probe"*"site=http://localhost:3100 scheme=agentrelay-dev product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-probe.sh: cargo saw: $(last_call)"
[ "$(records "$work/p1" probe)" = "http://localhost:3100 agentrelay-dev" ] || fail "build-probe.sh: records: $(records "$work/p1" probe)"
{ [ -f "$work/p1/agent-relay-probe" ] && [ -f "$work/p1/agent-relay-probe.sha256" ] && [ "$(cat "$work/p1/probe-version.txt")" = 0.0.0 ]; } || fail "build-probe.sh: outputs"
gtk -- --out "$work/g1" --site-url "http://127.0.0.1:03100" --url-scheme relay >/dev/null || fail "build-gtk.sh with options failed"
[[ "$(last_call)" == *"--features gtk --bin agent-relay"*"site=http://127.0.0.1:3100 scheme=relay product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-gtk.sh: cargo saw: $(last_call)"
[ "$(records "$work/g1" agent-relay)" = "http://127.0.0.1:3100 relay" ] || fail "build-gtk.sh: records: $(records "$work/g1" agent-relay)"
{ [ -f "$work/g1/agent-relay" ] && [ -f "$work/g1/agent-relay.sha256" ]; } || fail "build-gtk.sh: outputs"

# Exported variables count when the options are absent, and are said.
output="$(probe AGENT_RELAY_SITE_URL="https://Staging.Example" AGENT_RELAY_URL_SCHEME="relay-staging" -- --out "$work/p2")" || fail "build-probe.sh with exported variables failed"
[[ "$output" == *"AGENT_RELAY_SITE_URL is set"* ]] || fail "build-probe.sh: the exported site is not said"
[[ "$(last_call)" == *"site=https://staging.example scheme=relay-staging product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-probe.sh: cargo saw: $(last_call)"
[ "$(records "$work/p2" probe)" = "https://staging.example relay-staging" ] || fail "build-probe.sh: exported records: $(records "$work/p2" probe)"
gtk AGENT_RELAY_SITE_URL="http://localhost:4000" -- --out "$work/g2" >/dev/null || fail "build-gtk.sh with an exported site failed"
[[ "$(last_call)" == *"site=http://localhost:4000 scheme=unset product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-gtk.sh: cargo saw: $(last_call)"
[ "$(records "$work/g2" agent-relay)" = "http://localhost:4000 agentrelay" ] || fail "build-gtk.sh: exported records: $(records "$work/g2" agent-relay)"

# Nothing given: production, and neither variable reaches cargo.
{ probe -- --out "$work/p3" >/dev/null && gtk -- --out "$work/g3" >/dev/null; } || fail "production builds failed"
[[ "$(last_call)" == *"site=unset scheme=unset product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-gtk.sh: cargo saw: $(last_call)"
[ "$(records "$work/p3" probe)" = "https://agentrelay.com agentrelay" ] || fail "build-probe.sh: production records: $(records "$work/p3" probe)"
[ "$(records "$work/g3" agent-relay)" = "https://agentrelay.com agentrelay" ] || fail "build-gtk.sh: production records: $(records "$work/g3" agent-relay)"

# A value the core's build would refuse is refused before cargo runs.
calls="$(wc -l < "$CARGO_LOG")"
for refused in "--site-url https://staging.example:8443" "--site-url http://example.com:3100" "--url-scheme https" "--url-scheme 1relay"; do
  # shellcheck disable=SC2086
  probe -- --out "$work/p4" $refused >/dev/null 2>&1 && fail "build-probe.sh accepted $refused"
  # shellcheck disable=SC2086
  gtk -- --out "$work/g4" $refused >/dev/null 2>&1 && fail "build-gtk.sh accepted $refused"
done
reason="$(probe -- --out "$work/p4" --site-url https://staging.example:8443 2>&1 >/dev/null || true)"
[[ "$reason" == *"must be an origin"*"https://staging.example:8443"* ]] || fail "build-probe.sh: refusal reason: $reason"
[ "$(wc -l < "$CARGO_LOG")" = "$calls" ] || fail "cargo ran for a refused value"
{ [ ! -e "$work/p4" ] && [ ! -e "$work/g4" ]; } || fail "a refused build wrote outputs"

# The rest of the site profile reaches the app's build the same way: the
# option, else the exported variable (said), else the default, which cargo
# never sees as a variable; each is recorded beside the app.
gtk_log() { tail -n 1 "$CARGO_LOG"; }
printf '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"/>' > "$work/icon.svg"
gtk -- --out "$work/g5" --profile acme-7 --product-name " Acme Desktop " --releases-repository acme/desktop-releases --icon "$work/icon.svg" >/dev/null || fail "build-gtk.sh with a profile failed"
[[ "$(gtk_log)" == *"product=Acme Desktop repository=acme/desktop-releases prefix=Desktop-acme-7 icon=$work/icon.svg" ]] || fail "build-gtk.sh: cargo saw: $(gtk_log)"
# A workspace-relative --icon (what the site-profile action passes) reaches
# cargo absolute: gtk/build.rs runs with gtk/ as its working directory.
( cd "$work" && gtk -- --out "$work/g5r" --profile acme-7 --icon icon.svg >/dev/null ) || fail "build-gtk.sh with a relative --icon failed"
[[ "$(gtk_log)" == *"icon=$work/icon.svg" ]] || fail "build-gtk.sh: a relative --icon reached cargo as: $(gtk_log)"
[ "$(cat "$work/g5/agent-relay-product-name.txt")" = "Acme Desktop" ] || fail "build-gtk.sh: product name record"
[ "$(cat "$work/g5/agent-relay-releases-repository.txt")" = "acme/desktop-releases" ] || fail "build-gtk.sh: repository record"
output="$(gtk DESKTOP_PRODUCT_NAME="Acme Desktop" RELEASES_REPOSITORY="acme/desktop-releases" -- --out "$work/g6")" || fail "build-gtk.sh with exported profile variables failed"
[[ "$output" == *"DESKTOP_PRODUCT_NAME is set"*"RELEASES_REPOSITORY is set"* ]] || fail "build-gtk.sh: the exported profile variables are not said"
[[ "$(gtk_log)" == *"site=unset scheme=unset product=Acme Desktop repository=acme/desktop-releases prefix=unset icon=unset" ]] || fail "build-gtk.sh: cargo saw: $(gtk_log)"
gtk -- --out "$work/g7" >/dev/null || fail "build-gtk.sh default failed"
[[ "$(gtk_log)" == *"product=unset repository=unset prefix=unset icon=unset" ]] || fail "build-gtk.sh: the default profile reached cargo as a variable: $(gtk_log)"
{ [ "$(cat "$work/g7/agent-relay-product-name.txt")" = "Agent Relay" ] && [ "$(cat "$work/g7/agent-relay-releases-repository.txt")" = "AgentWorkforce/relay-desktop-releases" ]; } || fail "build-gtk.sh: default records"
calls="$(wc -l < "$CARGO_LOG")"
for refused in "--profile Acme" "--product-name Acme/Desktop" "--releases-repository acme" "--icon $work/missing.svg"; do
  # shellcheck disable=SC2086
  gtk -- --out "$work/g8" $refused >/dev/null 2>&1 && fail "build-gtk.sh accepted $refused"
done
printf 'not an svg\n' > "$work/icon.png"
gtk -- --out "$work/g8" --icon "$work/icon.png" >/dev/null 2>&1 && fail "build-gtk.sh accepted an icon that is no SVG"
[ "$(wc -l < "$CARGO_LOG")" = "$calls" ] || fail "cargo ran for a refused profile value"
[ ! -e "$work/g8" ] || fail "a refused profile build wrote outputs"

# The release feed: the default's reaches cargo as no variable and is not
# recorded; github:OWNER/NAME is the releases repository's; cloud, a
# profile's own site, has no repository and carries its key.
feed_log() { tail -n 1 "$CARGO_LOG.feed"; }
KEY=11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=
gtk -- --out "$work/f1" >/dev/null || fail "build-gtk.sh default feed failed"
[ "$(feed_log)" = "feed=unset key=unset id=unset repository=unset" ] || fail "build-gtk.sh: the default feed reached cargo as: $(feed_log)"
{ [ ! -e "$work/f1/agent-relay-release-feed.txt" ] && [ ! -e "$work/f1/agent-relay-feed-public-key.txt" ]; } || fail "build-gtk.sh: the default build recorded a feed"
gtk -- --out "$work/f1" --release-feed github:AgentWorkforce/relay-desktop-releases >/dev/null || fail "build-gtk.sh with the default feed named failed"
[ "$(feed_log)" = "feed=unset key=unset id=unset repository=unset" ] || fail "build-gtk.sh: the default feed, named, reached cargo as: $(feed_log)"
[ ! -e "$work/f1/agent-relay-release-feed.txt" ] || fail "build-gtk.sh: the default feed, named, was recorded"
gtk -- --out "$work/f2" --profile acme-7 --release-feed github:acme/desktop-releases >/dev/null || fail "build-gtk.sh with a github: feed failed"
[ "$(feed_log)" = "feed=github:acme/desktop-releases key=unset id=acme-7 repository=acme/desktop-releases" ] || fail "build-gtk.sh: a github: feed reached cargo as: $(feed_log)"
{ [ "$(cat "$work/f2/agent-relay-release-feed.txt")" = github:acme/desktop-releases ] && [ "$(cat "$work/f2/agent-relay-releases-repository.txt")" = acme/desktop-releases ]; } || fail "build-gtk.sh: github: feed records"
gtk -- --out "$work/f3" --site-url https://relay.example --url-scheme acme-relay --profile acme-7 --release-feed cloud --feed-public-key "$KEY" >/dev/null \
  || fail "build-gtk.sh with a cloud feed failed"
[ "$(feed_log)" = "feed=cloud key=$KEY id=acme-7 repository=" ] || fail "build-gtk.sh: a cloud feed reached cargo as: $(feed_log)"
{ [ "$(cat "$work/f3/agent-relay-release-feed.txt")" = cloud ] && [ "$(cat "$work/f3/agent-relay-feed-public-key.txt")" = "$KEY" ] \
    && [ -z "$(cat "$work/f3/agent-relay-releases-repository.txt")" ]; } || fail "build-gtk.sh: cloud feed records"
output="$(gtk RELEASE_FEED=cloud DESKTOP_FEED_PUBLIC_KEY="$KEY" AGENT_RELAY_SITE_URL=https://relay.example -- --out "$work/f4" --profile acme-7)" \
  || fail "build-gtk.sh with an exported cloud feed failed"
[[ "$output" == *"RELEASE_FEED is set"*"DESKTOP_FEED_PUBLIC_KEY is set"* ]] || fail "build-gtk.sh: the exported feed variables are not said"
[[ "$output" != *"$KEY"* ]] || fail "build-gtk.sh: the feed public key was printed"
[ "$(feed_log)" = "feed=cloud key=$KEY id=acme-7 repository=" ] || fail "build-gtk.sh: an exported cloud feed reached cargo as: $(feed_log)"
calls="$(wc -l < "$CARGO_LOG")"
SITE=(--site-url https://relay.example --url-scheme acme-relay)
refuse_feed() { # WHY ARGS...
  local why="$1"; shift
  if gtk -- --out "$work/f9" "$@" >/dev/null 2>&1; then fail "build-gtk.sh accepted $why"; fi
}
refuse_feed "a github: feed not naming the repository" --profile acme-7 --release-feed github:acme/other --releases-repository acme/desktop-releases
refuse_feed "a feed that is neither" --profile acme-7 --release-feed https://relay.example
refuse_feed "a key for a github: feed" --profile acme-7 --release-feed github:acme/desktop-releases --feed-public-key "$KEY"
refuse_feed "a cloud feed with a repository" "${SITE[@]}" --profile acme-7 --release-feed cloud --feed-public-key "$KEY" --releases-repository acme/desktop-releases
refuse_feed "a cloud feed without a key" "${SITE[@]}" --profile acme-7 --release-feed cloud
refuse_feed "a cloud feed with a bad key" "${SITE[@]}" --profile acme-7 --release-feed cloud --feed-public-key not-a-key
refuse_feed "a cloud feed without a profile" "${SITE[@]}" --release-feed cloud --feed-public-key "$KEY"
refuse_feed "a cloud feed on production's site" --profile acme-7 --release-feed cloud --feed-public-key "$KEY"
reason="$(gtk -- --out "$work/f9" "${SITE[@]}" --profile acme-7 --release-feed cloud 2>&1 >/dev/null || true)"
[[ "$reason" == *"needs its feed public key"* ]] || fail "build-gtk.sh: cloud refusal reason: $reason"
[ "$(wc -l < "$CARGO_LOG")" = "$calls" ] || fail "cargo ran for a refused feed"
[ ! -e "$work/f9" ] || fail "a refused feed build wrote outputs"

# build-linux.sh packages the app and core for the profile's site and
# scheme, and generates the launcher entry naming the product and the
# build's scheme as its handler; the packages are named by the profile's
# id, never the product.
# The control file of a .deb, whichever way build-linux.sh built it.
deb_control() {
  if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb -f "$1"
  else
    ar p "$1" control.tar.gz | tar -xzOf - ./control
  fi
}
linux_fixture() { # DIR SITE SCHEME
  mkdir -p "$1"
  printf '#!/bin/sh\n' > "$1/agent-relay"; printf '#!/bin/sh\n' > "$1/agent-relay-probe"
  chmod +x "$1/agent-relay" "$1/agent-relay-probe"
  for name in agent-relay probe; do printf '%s\n' "$2" > "$1/$name-site.txt"; printf '%s\n' "$3" > "$1/$name-url-scheme.txt"; done
}
linux_fixture "$work/l1" https://relay.example acme-relay
( cd "$work" && bash "$SCRIPTS/build-linux.sh" --version 0.0.0 --app l1/agent-relay --probe l1/agent-relay-probe --out l1/out \
    --site-url HTTPS://Relay.Example --url-scheme Acme-Relay --profile acme-7 --product-name "Acme Desktop" --icon icon.svg >/dev/null ) || fail "build-linux.sh with a profile failed"
entry="$work/l1/out/AgentRelay-linux-x64/usr/share/applications/agent-relay.desktop"
grep -qx 'MimeType=x-scheme-handler/acme-relay;' "$entry" || fail "build-linux.sh: the launcher entry does not name the scheme: $(cat "$entry")"
grep -qx 'Name=Acme Desktop' "$entry" || fail "build-linux.sh: the launcher entry does not name the product"
{ grep -qx 'Exec=agent-relay-desktop %u' "$entry" && grep -qx 'Icon=agent-relay' "$entry"; } || fail "build-linux.sh: the launcher entry's Exec or Icon changed"
cmp -s "$work/icon.svg" "$work/l1/out/AgentRelay-linux-x64/usr/share/icons/hicolor/scalable/apps/agent-relay.svg" || fail "build-linux.sh: the icon is not the given mark"
{ [ -f "$work/l1/out/Desktop-acme-7-0.0.0-Linux-x64.tar.gz" ] && [ -f "$work/l1/out/Desktop-acme-7-0.0.0-Linux-x64.deb" ] && [ -f "$work/l1/out/Desktop-acme-7-0.0.0-Linux-x64.deb.sha256" ]; } || fail "build-linux.sh: profile package names: $(ls "$work/l1/out")"
[ -z "$(find "$work/l1/out" -maxdepth 1 -name 'AgentRelay-0.0.0*')" ] || fail "build-linux.sh: a profile package carries the default name"
# One app per machine: every profile's package is agent-relay, with no
# Conflicts or Replaces, so dpkg upgrades over whichever product is installed;
# the control file names the product, never in the package name.
control="$(deb_control "$work/l1/out/Desktop-acme-7-0.0.0-Linux-x64.deb")"
{ grep -qx 'Package: agent-relay' <<< "$control" && ! grep -q '^Conflicts:\|^Replaces:' <<< "$control" \
    && grep -qx 'Homepage: https://relay.example' <<< "$control" && grep -q '^Description: Native Acme Desktop companion' <<< "$control" \
    && grep -qx 'Section: devel' <<< "$control"; } \
  || fail "build-linux.sh: a profile package's control: $control"
# The package's modes do not depend on the caller's umask: dpkg-deb refuses
# a 700 DEBIAN dir, and the installed tree must be readable by other users.
linux_fixture "$work/l4" https://agentrelay.com agentrelay
( umask 077; cd "$work" && bash "$SCRIPTS/build-linux.sh" --version 0.0.0 --app l4/agent-relay --probe l4/agent-relay-probe --out l4/out >/dev/null ) || fail "build-linux.sh failed under umask 077"
tree="$work/l4/out/AgentRelay-linux-x64"
for dir in "$tree" "$tree/usr/lib/agent-relay"; do
  [ "$(stat -c %a "$dir")" = 755 ] || fail "build-linux.sh under umask 077: $dir is mode $(stat -c %a "$dir"), not 755"
done
entry="$tree/usr/share/applications/agent-relay.desktop"
[ "$(stat -c %a "$entry")" = 644 ] || fail "build-linux.sh under umask 077: the launcher entry is mode $(stat -c %a "$entry"), not 644"
[ -f "$work/l4/out/AgentRelay-0.0.0-Linux-x64.deb" ] || fail "build-linux.sh under umask 077: no .deb: $(ls "$work/l4/out")"
# The default profile: the entry and names of every production release.
linux_fixture "$work/l2" https://agentrelay.com agentrelay
( cd "$work" && bash "$SCRIPTS/build-linux.sh" --version 0.0.0 --app l2/agent-relay --probe l2/agent-relay-probe --out l2/out >/dev/null ) || fail "build-linux.sh default failed"
entry="$work/l2/out/AgentRelay-linux-x64/usr/share/applications/agent-relay.desktop"
{ grep -qx 'MimeType=x-scheme-handler/agentrelay;' "$entry" && grep -qx 'Name=Agent Relay' "$entry"; } || fail "build-linux.sh: the default launcher entry: $(cat "$entry")"
cmp -s "$SCRIPTS/../gtk/resources/linux/agent-relay-mark.svg" "$work/l2/out/AgentRelay-linux-x64/usr/share/icons/hicolor/scalable/apps/agent-relay.svg" || fail "build-linux.sh: the default icon is not the app's mark"
[ -f "$work/l2/out/AgentRelay-0.0.0-Linux-x64.tar.gz" ] || fail "build-linux.sh: default package names: $(ls "$work/l2/out")"
control="$(deb_control "$work/l2/out/AgentRelay-0.0.0-Linux-x64.deb")"
{ grep -qx 'Package: agent-relay' <<< "$control" && ! grep -q '^Conflicts:\|^Replaces:' <<< "$control" \
    && grep -qx 'Section: devel' <<< "$control"; } || fail "build-linux.sh: the default package's control: $control"
# A core or app built for another site than the package's is refused.
( cd "$work" && bash "$SCRIPTS/build-linux.sh" --version 0.0.0 --app l1/agent-relay --probe l1/agent-relay-probe --out l3/out >/dev/null 2>&1 ) && fail "build-linux.sh packaged a profile build as the default"
( cd "$work" && bash "$SCRIPTS/build-linux.sh" --version 0.0.0 --app l2/agent-relay --probe l2/agent-relay-probe --out l3/out --site-url https://relay.example --url-scheme acme-relay >/dev/null 2>&1 ) && fail "build-linux.sh packaged a default build as a profile's"
[ ! -e "$work/l3" ] || fail "a refused Linux package wrote outputs"

# package-dmg.sh with stand-in macOS tools: the background renderer gets the
# co-brand options only when a co-brand is given, so an image without one is
# rendered exactly as before, and the customer's name is never printed.
dmg_tools="$work/dmg-tools"
mkdir -p "$dmg_tools" "$work/dmg/Acme.app/Contents"
printf '#!/bin/sh\necho Darwin\n' > "$dmg_tools/uname"
printf '#!/bin/sh\nexit 0\n' > "$dmg_tools/hdiutil"
cat > "$dmg_tools/ditto" <<'DITTO'
#!/bin/sh
cp -R "$1" "$2"
DITTO
cat > "$dmg_tools/swift" <<'SWIFT'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$SWIFT_LOG"
touch "$2/background.png" "$2/background@2x.png"
SWIFT
cat > "$dmg_tools/tiffutil" <<'TIFFUTIL'
#!/usr/bin/env bash
while [ $# -gt 0 ]; do if [ "$1" = -out ]; then touch "$2"; fi; shift; done
TIFFUTIL
cat > "$dmg_tools/dmgbuild" <<'DMGBUILD'
#!/usr/bin/env bash
touch "${@: -1}"
DMGBUILD
chmod +x "$dmg_tools"/*
printf '<svg xmlns="http://www.w3.org/2000/svg"/>\n' > "$work/dmg/cobrand.svg"
package_dmg() { # OUT ARGS...
  local out="$1"; shift
  ( cd "$work/dmg" && PATH="$dmg_tools:/usr/bin:/bin" SWIFT_LOG="$work/dmg/$out.swift" \
      bash "$SCRIPTS/package-dmg.sh" --app Acme.app --out "out/$out.dmg" "$@" 2>&1 )
}
got="$(package_dmg plain)" || fail "package-dmg.sh without a co-brand failed: $got"
[ "$(wc -l < "$work/dmg/plain.swift")" = 4 ] \
  || fail "package-dmg.sh without a co-brand: the renderer got $(tr '\n' ' ' < "$work/dmg/plain.swift")"
[ "$(head -n 1 "$work/dmg/plain.swift")" = "$SCRIPTS/render-dmg-background.swift" ] \
  || fail "package-dmg.sh without a co-brand: the renderer got $(tr '\n' ' ' < "$work/dmg/plain.swift")"
! grep -q -- '--cobrand' "$work/dmg/plain.swift" || fail "package-dmg.sh without a co-brand passed co-brand options"
got="$(package_dmg cobranded --product-name "Acme Desktop" --cobrand-mark cobrand.svg --cobrand-name "Acme Corp")" \
  || fail "package-dmg.sh with a co-brand failed: $got"
[ "$(tail -n 4 "$work/dmg/cobranded.swift")" = "--cobrand-mark"$'\n'"$work/dmg/cobrand.svg"$'\n'"--cobrand-name"$'\n'"Acme Corp" ] \
  || fail "package-dmg.sh with a co-brand: the renderer got $(tr '\n' ' ' < "$work/dmg/cobranded.swift")"
! grep -q "Acme Corp" <<< "$got" || fail "package-dmg.sh printed the co-brand name"
got="$(package_dmg refused --cobrand-mark cobrand.svg)" && fail "package-dmg.sh took a co-brand mark without a name"
grep -q -- "--cobrand-mark needs --cobrand-name" <<< "$got" || fail "package-dmg.sh: mark without a name: $got"
got="$(package_dmg refused --cobrand-name "Acme Corp")" && fail "package-dmg.sh took a co-brand name without a mark"
grep -q -- "--cobrand-name needs --cobrand-mark" <<< "$got" || fail "package-dmg.sh: name without a mark: $got"
got="$(package_dmg refused --cobrand-mark Acme.app --cobrand-name "Acme Corp")" && fail "package-dmg.sh took a co-brand mark that is no image"
# The name is trimmed and checked as the app checks it: a blank, control-
# character or over-80-character name is refused before any output, and the
# refusal gives its length, never the name.
got="$(package_dmg trimmed --cobrand-mark cobrand.svg --cobrand-name "  Acme Corp  ")" \
  || fail "package-dmg.sh with a padded co-brand name failed: $got"
[ "$(tail -n 1 "$work/dmg/trimmed.swift")" = "Acme Corp" ] \
  || fail "package-dmg.sh did not trim the co-brand name: $(tail -n 1 "$work/dmg/trimmed.swift")"
got="$(package_dmg refused --cobrand-mark cobrand.svg --cobrand-name $' \t ')" && fail "package-dmg.sh took a blank co-brand name"
grep -q -- "--cobrand-name must not be empty or blank" <<< "$got" || fail "package-dmg.sh: blank name: $got"
got="$(package_dmg refused --cobrand-mark cobrand.svg --cobrand-name $'Acme\aCorp')" && fail "package-dmg.sh took a co-brand name with a control character"
grep -q -- "--cobrand-name must be one line" <<< "$got" || fail "package-dmg.sh: control character: $got"
long="Acme Corp $(printf 'x%.0s' {1..71})"
got="$(package_dmg refused --cobrand-mark cobrand.svg --cobrand-name "$long")" && fail "package-dmg.sh took an 81-character co-brand name"
grep -q -- "--cobrand-name must be at most 80 characters; it has 81" <<< "$got" || fail "package-dmg.sh: long name: $got"
! grep -q "Acme Corp" <<< "$got" || fail "package-dmg.sh printed a refused co-brand name"
[ ! -e "$work/dmg/refused.swift" ] || fail "package-dmg.sh rendered a background for a refused co-brand"
[ ! -e "$work/dmg/out/refused.dmg" ] || fail "a refused disk image was written"

# The dev-attest preview feature is enabled by no workflow, script, flow,
# packaging or Cargo.toml, and the guard catches one that is. Its name is
# read from the guard, so this script does not name it.
bash "$SCRIPTS/test-dev-attest-guard.sh" || fail "test-dev-attest-guard.sh: the preview feature is enabled somewhere"
feature="$(sed -n 's/^feature="\(.*\)"$/\1/p' "$SCRIPTS/test-dev-attest-guard.sh")"
mkdir -p "$work/guard/.github/workflows"
printf 'run: cargo build --features %s\n' "$feature" > "$work/guard/.github/workflows/build.yml"
{ [ -n "$feature" ] && ! bash "$SCRIPTS/test-dev-attest-guard.sh" "$work/guard" >/dev/null; } \
  || fail "test-dev-attest-guard.sh passed a workflow that enables the preview feature"

[ "$failed" = 0 ] && echo "build-probe.sh, build-gtk.sh, build-linux.sh, package-dmg.sh and the dev-attest guard: ok"
exit "$failed"

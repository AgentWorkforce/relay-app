#!/usr/bin/env bash
#
# install.sh -- install or update the Agent Relay headless core on Linux.
#
# One idempotent command for a server or a desktop without the app: it
# installs the GTK-free core (`agent-relay-probe`) for this user, runs it as
# the systemd user service `agent-relay.service`, and on later runs updates it
# in place. Run it again at any time; an up-to-date healthy install is left
# alone.
#
#   curl -fsSL <this script> | bash
#   bash install.sh [--version X.Y.Z] [--rollback] [--dry-run]
#
# What it does:
#   1. Finds what is installed: a per-user install from this script
#      (~/.local/lib/agent-relay/current), the in-app updater's per-user
#      layout, or the .deb package (which apt owns; this script then stops).
#   2. Resolves the release (latest, or --version) from the published
#      releases, downloads <ASSET_PREFIX>-<version>-Linux-<arch>-probe.tar.gz and
#      verifies it against the release's .sha256. The checksum proves the
#      download is the published file, not who published it: Linux releases
#      carry no signature yet. A site profile with RELEASE_FEED='cloud' reads
#      its site's signed release feed instead (see "the cloud release feed"
#      below): the manifest's signature, then the asset's size and sha256.
#   3. Unpacks into versions/<version>, checks the binary reports that
#      version, records the product beside it (versions/<version>/product),
#      and points `current` at it atomically. The previous version stays for
#      rollback (--rollback goes to the newest installed version older than
#      the current one); older ones beyond KEEP are removed.
#   4. Writes the user unit (only when it is missing or names another
#      binary), reloads systemd, and restarts the service (or enables and
#      starts it on a first install). Agents the core spawned run in scopes of
#      their own and keep running through the restart.
#   5. Waits for the new core to answer with the new version; if it does not,
#      points `current` back at the previous version and restarts again.
#
# Never touched: settings, sign-in and agent registrations, which live in the
# core's data directory ($XDG_DATA_HOME/com.agentrelay.desktop), not here.
# Never done: anything needing root (the .deb), enabling lingering, or
# changing credentials; it says what to run instead.
#
# Every product installs into the same prefix and unit: one Agent Relay app
# per machine, so a site profile's copy of this script replaces whichever
# product's headless core is installed, as an update would, even at the same
# version: a release counts as installed only when the recorded product is
# this one (an install without a record is Agent Relay's).
set -euo pipefail

# The product whose releases this installs. A site profile's release
# publishes a copy with these lines set to its own (Stage the release
# assets, .github/workflows/desktop-release.yml); keep each one assignment.
# RELEASE_FEED is 'github' (the releases repository) or 'cloud' (the signed
# release feed of SITE_URL for PROFILE_ID, checked with FEED_PUBLIC_KEY, the
# standard base64 of the feed's Ed25519 public key).
PRODUCT_NAME='Agent Relay'
RELEASES_REPOSITORY='AgentWorkforce/relay-desktop-releases'
ASSET_PREFIX='AgentRelay'
RELEASE_FEED='github'
SITE_URL=''
PROFILE_ID=''
FEED_PUBLIC_KEY=''
# The product an install without a product record belongs to: every earlier
# release of this script was Agent Relay's.
DEFAULT_ASSET_PREFIX='AgentRelay'

SCRIPT_NAME="agent-relay install"
RELEASES_API="${AGENT_RELAY_RELEASES_API:-https://api.github.com/repos/$RELEASES_REPOSITORY/releases}"
DOWNLOAD_BASE="${AGENT_RELAY_DOWNLOAD_BASE:-https://github.com/$RELEASES_REPOSITORY/releases/download}"
PREFIX="${AGENT_RELAY_PREFIX:-$HOME/.local/lib/agent-relay}"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="agent-relay.service"
KEEP=3
HEALTH_TIMEOUT="${AGENT_RELAY_HEALTH_TIMEOUT:-60}"

VERSION=""
DRY_RUN=0
ROLLBACK=0

usage() {
  cat <<EOF
Usage: install.sh [--version X.Y.Z] [--rollback] [--dry-run]

  --version X.Y.Z  Install this release instead of the latest.
  --rollback       Switch to the newest installed version older than the current one.
  --dry-run        Say what would change and change nothing.
EOF
  if [ "$RELEASE_FEED" = cloud ]; then
    cat <<EOF

A first install needs AGENT_RELAY_INSTALL_TOKEN, the install token from your
workspace's downloads page; an update uses the installed core's sign-in.
EOF
  fi
}

note() { printf '%s: %s\n' "$SCRIPT_NAME" "$*"; }
die() { printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2; exit 1; }
run() {
  if [ "$DRY_RUN" = 1 ]; then
    printf '%s: would run: %s\n' "$SCRIPT_NAME" "$*"
  else
    "$@"
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="${2#v}"; shift 2 ;;
    --rollback) ROLLBACK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = Linux ] || die "this installs the Linux core; on macOS use the $PRODUCT_NAME app"
[ -n "${HOME:-}" ] || die "HOME is not set"
[ "$(id -u)" != 0 ] || die "run this as the user the agents belong to, not root"

case "$(uname -m)" in
  x86_64|amd64) ARCH=x64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

valid_version() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; }
[ -z "$VERSION" ] || valid_version "$VERSION" || die "not a release version: $VERSION"

have() { command -v "$1" >/dev/null 2>&1; }
have systemctl || die "systemctl is required: the core runs as a systemd user service"

sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

fetch() { # url dest
  if have curl; then curl -fsSL --proto '=https' --retry 3 -o "$2" "$1"
  elif have wget; then wget -q -O "$2" "$1"
  else die "curl or wget is required"
  fi
}

# --- the cloud release feed (RELEASE_FEED='cloud') ----------------------------
#
# The routes, headers and error codes below are Cloud's proposed release feed
# contract (docs/enterprise-distribution.md), not a stable API. The manifest
# is a signed envelope {manifest, signature, key_id}; this script refuses one
# whose key_id is not fk_<PROFILE_ID>_<label>, whose profile or channel is
# not this build's, or whose signature does not verify, then checks the
# asset's size and sha256 before anything is unpacked.
#
# The credential is never printed, logged or written to a file: it is held
# in a shell variable and handed to curl on its standard input (-H @-), so it
# is in no command line either.

site="${SITE_URL%/}"
FEED_LIMIT=1048576
cloud_headers=""
verified_by=""

cloud_preflight() {
  [ -z "${AGENT_RELAY_RELEASES_API+set}" ] \
    || die "AGENT_RELAY_RELEASES_API cannot be used: $PRODUCT_NAME installs from its site's release feed"
  [ -z "${AGENT_RELAY_DOWNLOAD_BASE+set}" ] \
    || die "AGENT_RELAY_DOWNLOAD_BASE cannot be used: $PRODUCT_NAME installs from its site's release feed"
  { [ -n "$SITE_URL" ] && [ -n "$PROFILE_ID" ] && [ -n "$FEED_PUBLIC_KEY" ]; } \
    || die "this copy of install.sh has no site, profile or feed key; download it again from your workspace's downloads page"
  printf '%s' "$site" | grep -Eq '^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?$' \
    || die "the built-in site is not an https:// address"
  printf '%s' "$PROFILE_ID" | grep -Eq '^[a-z0-9]{1,32}$' || die "the built-in profile id is malformed"
  have curl || die "curl is required to read the release feed"
  have python3 || die "python3 is required to read the release feed"
}

# The feed's JSON work, in one place: python3 -I, reading only what it is
# given. Commands: ids, headers, error, envelope, pem, select, url, verified.
feed_py() {
  python3 -I -c '
import base64, json, re, sys
from urllib.parse import urlsplit

LIMIT = 1048576
def fail(message):
    sys.stderr.write(message + "\n")
    sys.exit(1)
def refuse_float(text):
    raise ValueError("a non-integer number")
def unique(pairs):
    keys = [key for key, _ in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError("a repeated key")
    return dict(pairs)
def walk(value):
    if isinstance(value, dict):
        for item in value.values():
            walk(item)
    elif isinstance(value, list):
        for item in value:
            walk(item)
    elif isinstance(value, int) and not isinstance(value, bool) and abs(value) > 2 ** 53 - 1:
        raise ValueError("an integer JCS cannot write")
def load(path):
    with (sys.stdin.buffer if path == "-" else open(path, "rb")) as source:
        data = source.read(LIMIT + 1)
    if len(data) > LIMIT:
        fail("the answer is larger than 1 MiB")
    try:
        value = json.loads(data.decode("utf-8"), parse_float=refuse_float,
                           parse_constant=refuse_float, object_pairs_hook=unique)
        walk(value)
        return value
    except (ValueError, UnicodeDecodeError, RecursionError):
        fail("the answer is not valid JSON")
def jcs(value):
    # RFC 8785 for integers, strings, booleans and null: keys in UTF-16 order.
    if isinstance(value, dict):
        items = sorted(value.items(), key=lambda item: item[0].encode("utf-16-be"))
        return "{" + ",".join(json.dumps(key, ensure_ascii=False) + ":" + jcs(item)
                              for key, item in items) + "}"
    if isinstance(value, list):
        return "[" + ",".join(jcs(item) for item in value) + "]"
    return json.dumps(value, ensure_ascii=False)
def text(value, pattern):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None
HOST = r"(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]([a-z0-9-]{0,61}[a-z0-9])?"

command, args = sys.argv[1], sys.argv[2:]
if command == "ids":
    status = load("-")
    status = status.get("result", status) if isinstance(status, dict) else {}
    account = (status.get("account") if isinstance(status, dict) else None) or {}
    person = (account.get("person") if isinstance(account, dict) else None) or {}
    workspace = account.get("workspace_id") if isinstance(account, dict) else None
    who = person.get("id") if isinstance(person, dict) else None
    if not (text(workspace, r"[A-Za-z0-9_.-]{1,128}") and text(who, r"[A-Za-z0-9_.-]{1,128}")):
        fail("not signed in")
    print(workspace, who)
elif command == "headers":
    headers = load("-")
    bearer = headers.get("Authorization") if isinstance(headers, dict) else None
    if not (isinstance(headers, dict) and len(headers) == 3 and text(bearer, r"Bearer [!-~]+")
            and headers.get("X-Agent-Relay-Profile") == args[0]
            and headers.get("X-Agent-Relay-Desktop") == "1"):
        fail("unusable headers")
    for name in ("Authorization", "X-Agent-Relay-Profile", "X-Agent-Relay-Desktop"):
        print(name + ": " + headers[name])
elif command == "error":
    try:
        body = load(args[0])
    except SystemExit:
        sys.exit(0)
    code = body.get("error") if isinstance(body, dict) else None
    if text(code, r"[a-z_]{1,64}"):
        print(code)
elif command == "envelope":
    path, profile, signature_out, jcs_out = args
    envelope = load(path)
    if not (isinstance(envelope, dict) and sorted(envelope) == ["key_id", "manifest", "signature"]
            and isinstance(envelope["manifest"], dict)
            and text(envelope["signature"], r"[A-Za-z0-9_-]{85}[AQgw]")
            and text(envelope["key_id"], r"fk_[a-z0-9]{1,32}_[a-z0-9]{1,16}")):
        fail("the release feed answered something that is not a signed manifest")
    if envelope["key_id"].split("_")[1] != profile:
        fail("the manifest is signed with " + envelope["key_id"] + ", not a key of profile " + profile)
    with open(signature_out, "wb") as out:
        out.write(base64.urlsafe_b64decode(envelope["signature"] + "=="))
    try:
        canonical = jcs(envelope["manifest"]).encode("utf-8")
    except UnicodeEncodeError:
        fail("the manifest is not valid JSON")
    with open(jcs_out, "wb") as out:
        out.write(canonical)
elif command == "pem":
    try:
        key = base64.b64decode(args[0], validate=True)
    except ValueError:
        key = b""
    if len(key) != 32:
        fail("the built-in feed key is not a 32-byte Ed25519 key")
    der = bytes.fromhex("302a300506032b6570032100") + key
    with open(args[1], "w") as out:
        out.write("-----BEGIN PUBLIC KEY-----\n" + base64.b64encode(der).decode() + "\n-----END PUBLIC KEY-----\n")
elif command == "select":
    path, profile, channel, name, platform, arch, kind = args
    manifest = load(path)["manifest"]
    if manifest.get("schema") != 1:
        fail("the manifest has a schema this script does not know")
    if manifest.get("profile") != profile:
        fail("the manifest is for another profile")
    if manifest.get("channel") != channel:
        fail("the manifest is for another channel")
    version = manifest.get("version")
    if not text(version, r"[0-9]{4}\.(1[0-2]|[1-9])\.(0|[1-9][0-9]*)"):
        fail("the manifest has no usable stable version")
    hosts = manifest.get("download_hosts")
    assets = manifest.get("assets")
    if not (isinstance(hosts, list) and all(text(host, HOST) for host in hosts)
            and isinstance(assets, list) and all(isinstance(asset, dict) for asset in assets)):
        fail("the manifest is malformed")
    wanted = name.replace("@VERSION@", version)
    found = [asset for asset in assets if asset.get("name") == wanted]
    if len(found) != 1:
        fail("the release has no " + wanted)
    asset = found[0]
    size = asset.get("size")
    if not (asset.get("platform") == platform and asset.get("arch") == arch
            and asset.get("kind") == kind and text(asset.get("id"), r"[A-Za-z0-9_-]{1,64}")
            and isinstance(size, int) and not isinstance(size, bool) and 0 < size <= 2 ** 40
            and text(asset.get("sha256"), r"[0-9a-f]{64}")):
        fail("the manifest describes " + wanted + " wrongly")
    print(version, asset["id"], size, asset["sha256"], wanted)
    print(" ".join(hosts))
elif command == "url":
    path, site_host, hosts = args[0], args[1], args[2].split()
    body = load(path)
    url = body.get("url") if isinstance(body, dict) else None
    parts = urlsplit(url) if isinstance(url, str) else None
    allowed = (parts is not None and parts.scheme == "https" and url.startswith("https://")
               and parts.username is None and parts.password is None and parts.port is None
               and text(parts.hostname or "", HOST) and parts.netloc == parts.hostname
               and (parts.hostname in hosts or parts.hostname == site_host)
               and not any(ord(c) <= 32 or ord(c) >= 127 for c in url))
    if not allowed:
        fail("the download address is not on a host the manifest allows")
    print(url)
elif command == "verified":
    result = load(args[0])
    if not isinstance(result, dict):
        fail("unreadable")
    if result.get("ok") is True:
        print("ok")
    elif text(result.get("code"), r"[a-z_]{1,64}"):
        print(result["code"])
    else:
        fail("unreadable")
else:
    fail("unknown command")
' "$@"
}

# The installed core, if any: it supplies headers and verifies updates.
installed_probe() {
  if [ -n "$legacy" ] && [ -x "$legacy" ]; then printf '%s' "$legacy"
  elif [ -x "$PREFIX/current/agent-relay-probe" ]; then printf '%s' "$PREFIX/current/agent-relay-probe"
  fi
}

# Sets cloud_headers: the install token when there is one, else the installed
# core's own (feed-headers), never both: Cloud refuses mixed credentials.
cloud_auth() {
  local probe ids workspace account answer
  if [ -n "${AGENT_RELAY_INSTALL_TOKEN:-}" ]; then
    case "$AGENT_RELAY_INSTALL_TOKEN" in
      *[!!-~]*) die "AGENT_RELAY_INSTALL_TOKEN is not an install token; copy it again from your workspace's downloads page" ;;
    esac
    cloud_headers="Authorization: Bearer $AGENT_RELAY_INSTALL_TOKEN
X-Agent-Relay-Profile: $PROFILE_ID"
    return 0
  fi
  probe="$(installed_probe)"
  if [ -z "$current_version" ] || [ -z "$probe" ]; then
    die "a first install needs AGENT_RELAY_INSTALL_TOKEN: copy the install token from your workspace's downloads page and run this again with it set"
  fi
  # A stopped core cannot say who is signed in: start it first, so a stopped
  # core is not mistaken for a signed-out one.
  if [ -z "$(core_version "$probe" || true)" ]; then
    [ "$DRY_RUN" = 0 ] \
      || die "the installed core is not running; start it with: systemctl --user restart $UNIT, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
    note "the installed core is not running; starting it"
    # An in-app updater's unit keeps its own ExecStart until the update.
    [ -n "$legacy" ] || ensure_unit
    if ! restart_service || ! healthy "$current_version" "$probe"; then
      die "the installed core is not running and did not start; see: journalctl --user -u $UNIT, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
    fi
  fi
  ids="$("$probe" relay call status.get --json 2>/dev/null | feed_py ids 2>/dev/null)" \
    || die "the installed core is not signed in to a workspace; sign in, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
  read -r workspace account <<< "$ids"
  answer="$("$probe" feed-headers --site "$site" --workspace "$workspace" --account "$account" --profile "$PROFILE_ID")" \
    || die "the installed core could not supply release feed headers; or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
  cloud_headers="$(printf '%s' "$answer" | feed_py headers "$PROFILE_ID" 2>/dev/null)" \
    || die "the installed core supplied unusable release feed headers"
}

# cloud_call dest limit url [curl options]: a feed request with the
# credential; sets status to the HTTP status, or "failed".
cloud_call() {
  local dest="$1" limit="$2" url="$3"
  shift 3
  status="$(printf '%s\n' "$cloud_headers" | curl -sS --proto '=https' --max-redirs 0 --retry 3 \
    --max-filesize "$limit" -H @- -o "$dest" -w '%{http_code}' "$@" "$url")" || status=failed
  if [ -f "$dest" ] && [ "$(wc -c < "$dest")" -gt "$limit" ]; then status=failed; fi
}

# Says why the feed refused, naming only its error code, never the body.
cloud_refused() { # what body
  local code
  code="$(feed_py error "$2" 2>/dev/null || true)"
  case "$status" in
    401) if [ -z "${AGENT_RELAY_INSTALL_TOKEN:-}" ]; then
           die "the release feed refused the installed core's sign-in (${code:-unauthenticated}); sign in again, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
         fi
         die "the release feed refused the install token (${code:-unauthenticated}): it has expired or was revoked; get a new one from your workspace's downloads page" ;;
    403) if [ "$code" = not_entitled ]; then
           die "this workspace is not entitled to $PRODUCT_NAME (not_entitled); ask a workspace admin"
         fi
         die "the release feed refused this request (${code:-forbidden})" ;;
    404) die "no $PRODUCT_NAME release is published on the stable channel (${code:-no_release})" ;;
    429) die "the release feed is busy (${code:-rate_limited}); try again later" ;;
    failed) die "could not read $1 from $site" ;;
    *) die "the release feed answered HTTP $status for $1 (${code:-no error code})" ;;
  esac
}

# Verifies the envelope's signature: with the installed core's feed verify
# for an update, else OpenSSL 3, else refuses (OpenSSL 3 ships on every
# supported distribution; install.ps1 refuses too unless told to skip it).
cloud_verify() { # envelope
  local probe result code
  probe="$(installed_probe)"
  if [ -n "$current_version" ] && [ -n "$probe" ] && "$probe" feed --help >/dev/null 2>&1; then
    set -- "$1" --profile "$PROFILE_ID" --public-key "$FEED_PUBLIC_KEY"
    if [ "$current_product" = "$ASSET_PREFIX" ] && valid_version "$current_version"; then
      set -- "$@" --installed-version "$current_version"
    fi
    "$probe" feed verify "${@:2}" "$1" > "$work/verified.json" 2>/dev/null || true
    result="$(feed_py verified "$work/verified.json" 2>/dev/null)" \
      || die "the installed core could not check the manifest"
    case "$result" in
      ok) verified_by="the installed core" ;;
      not_newer) verified_by="the installed core"; cloud_current=1 ;;
      *) die "the release feed's manifest was refused ($result)" ;;
    esac
    return 0
  fi
  if have openssl && openssl version 2>/dev/null | grep -q '^OpenSSL 3\.'; then
    feed_py pem "$FEED_PUBLIC_KEY" "$work/feed.pem" || die "the built-in feed key is unusable"
    openssl pkeyutl -verify -rawin -pubin -inkey "$work/feed.pem" -sigfile "$work/manifest.sig" \
      -in "$work/manifest.jcs" > /dev/null 2>&1 \
      || die "the release feed's manifest signature does not verify (signature_invalid)"
    verified_by="OpenSSL"
    return 0
  fi
  die "cannot check the release feed's signature: no installed core with feed verify and no OpenSSL 3; install OpenSSL 3 (openssl) and run this again"
}

# Resolves VERSION and the asset from the signed manifest.
cloud_resolve() {
  local selected
  cloud_auth
  cloud_call "$work/envelope.json" "$FEED_LIMIT" "$site/cloud/api/v1/desktop/feed/manifest?channel=stable"
  [ "$status" = 200 ] || cloud_refused "the release manifest" "$work/envelope.json"
  feed_py envelope "$work/envelope.json" "$PROFILE_ID" "$work/manifest.sig" "$work/manifest.jcs" \
    || die "the release feed's manifest was refused (key_id_refused or schema_invalid)"
  cloud_current=0
  cloud_verify "$work/envelope.json"
  selected="$(feed_py select "$work/envelope.json" "$PROFILE_ID" stable \
    "$ASSET_PREFIX-@VERSION@-Linux-$ARCH-probe.tar.gz" Linux "$ARCH" probe)" \
    || die "the release feed's manifest was refused"
  read -r cloud_version cloud_asset_id cloud_size cloud_sha256 cloud_asset <<< "$(head -n 1 <<< "$selected")"
  cloud_hosts="$(sed -n 2p <<< "$selected")"
  if [ -n "$verified_by" ]; then note "release manifest verified by $verified_by"; fi
  if [ -n "$VERSION" ] && [ "$VERSION" != "$cloud_version" ]; then
    die "the release feed offers $cloud_version only; --version $VERSION is not available"
  fi
  if [ "$cloud_current" = 1 ]; then
    VERSION="$current_version"
  else
    VERSION="$cloud_version"
  fi
}

# Downloads the resolved asset to $work/$1 and checks its size and sha256.
cloud_download() { # asset
  local url_json url host
  [ "$1" = "$cloud_asset" ] || die "the release feed's asset is $cloud_asset, not $1"
  host="${site#https://}"
  note "downloading $1"
  cloud_call "$work/download-url.json" "$FEED_LIMIT" \
    "$site/cloud/api/v1/desktop/feed/assets/$cloud_asset_id/download-url" -X POST -d ''
  url_json="$work/download-url.json"
  if [ "$status" = 200 ]; then
    url="$(feed_py url "$url_json" "$host" "$cloud_hosts")" || die "the release feed's download address was refused"
    # A signed address carries its own authorisation: no credential goes to it.
    status="$(curl -sS --proto '=https' --max-redirs 0 --retry 3 --max-filesize "$cloud_size" \
      -o "$work/$1" -w '%{http_code}' "$url")" || status=failed
  elif [ "$status" = 501 ]; then
    # No object storage behind this site: the feed streams the asset itself.
    cloud_call "$work/$1" "$cloud_size" "$site/cloud/api/v1/desktop/feed/assets/$cloud_asset_id"
  else
    cloud_refused "the download address" "$url_json"
  fi
  [ "$status" = 200 ] || die "could not download $1 (HTTP $status)"
  [ "$(wc -c < "$work/$1")" -eq "$cloud_size" ] || die "$1 is not the size the manifest gives"
  [ "$(sha256_of "$work/$1")" = "$cloud_sha256" ] || die "$1 does not match the manifest's sha256"
  note "size and sha256 verified"
}

case "$RELEASE_FEED" in
  github) ;;
  cloud) cloud_preflight ;;
  *) die "this copy of install.sh names an unknown release feed" ;;
esac

# --- 1. what is installed ---------------------------------------------------

if have dpkg-query && dpkg-query -W -f='${Status}' agent-relay 2>/dev/null | grep -q 'install ok installed'; then
  installed="$(dpkg-query -W -f='${Version}' agent-relay 2>/dev/null || true)"
  note "the agent-relay package ($installed) is installed; apt owns it."
  note "update it with: sudo apt-get install --only-upgrade agent-relay (or install the new .deb)"
  note "then: systemctl --user restart $UNIT"
  exit 3
fi

probe_version() { # binary -> version it reports
  "$1" --version 2>/dev/null | awk '{print $NF}' | head -n 1
}

current_version=""
if [ -x "$PREFIX/current/agent-relay-probe" ]; then
  current_version="$(probe_version "$PREFIX/current/agent-relay-probe" || true)"
fi
# The in-app updater's layout: ExecStart in the user unit names
# ~/.local/lib/agent-relay/<version>/<tree>/usr/lib/agent-relay/agent-relay-probe.
legacy=""
if [ -z "$current_version" ] && [ -f "$UNIT_DIR/$UNIT" ]; then
  legacy="$(sed -n 's/^ExecStart=\([^ ]*agent-relay-probe\).*/\1/p' "$UNIT_DIR/$UNIT" | head -n 1)"
  if [ -n "$legacy" ] && [ -x "$legacy" ]; then
    current_version="$(probe_version "$legacy" || true)"
  fi
fi
# The product of the installed version: its record, written on install, or
# Agent Relay's for an install made before there was one.
current_product=""
if [ -n "$current_version" ] && [ -z "$legacy" ]; then
  current_product="$(head -n 1 "$PREFIX/current/product" 2>/dev/null || true)"
  current_product="${current_product:-$DEFAULT_ASSET_PREFIX}"
fi
if [ -n "$current_version" ]; then
  note "installed: $current_version"
else
  note "no per-user install found"
fi

installed_versions() { # oldest first
  [ -d "$PREFIX/versions" ] || return 0
  find "$PREFIX/versions" -mindepth 1 -maxdepth 1 -type d ! -name '*.partial' ! -name '*.replaced' -printf '%f\n' | sort -V
}

# Whether release a is older than release b by SemVer precedence: the dotted
# numeric core compares numerically (a missing component is 0), a prerelease
# (1.2.0-rc.1) is older than its release (1.2.0), and prerelease identifiers
# compare one by one, numbers numerically and before words, words in ASCII
# order, with fewer identifiers older (beta.2 < beta.11, beta < beta.1).
# Build metadata (+...) is ignored. sort -V puts 1.2.0-rc.1 after 1.2.0.
version_older() { # a b
  LC_ALL=C awk -v a="$1" -v b="$2" '
    function cmp(x, y,   n, m, i, p, q, pa, pb, xp, yp, xn, yn) {
      sub(/[+].*/, "", x); sub(/[+].*/, "", y)
      pa = index(x, "-"); pb = index(y, "-")
      xp = pa ? substr(x, pa + 1) : ""; if (pa) x = substr(x, 1, pa - 1)
      yp = pb ? substr(y, pb + 1) : ""; if (pb) y = substr(y, 1, pb - 1)
      n = split(x, p, "."); m = split(y, q, ".")
      for (i = 1; i <= (n > m ? n : m); i++) {
        if (p[i] + 0 < q[i] + 0) return -1
        if (p[i] + 0 > q[i] + 0) return 1
      }
      if (!pa || !pb) return (pa ? -1 : 0) + (pb ? 1 : 0)
      n = split(xp, p, "."); m = split(yp, q, ".")
      for (i = 1; i <= n && i <= m; i++) {
        xn = p[i] ~ /^[0-9]+$/; yn = q[i] ~ /^[0-9]+$/
        if (xn && yn) {
          if (p[i] + 0 != q[i] + 0) return p[i] + 0 < q[i] + 0 ? -1 : 1
        } else if (xn != yn) {
          return xn ? -1 : 1
        } else if (p[i] "" != q[i] "") {
          return p[i] "" < q[i] "" ? -1 : 1
        }
      }
      return n < m ? -1 : n > m ? 1 : 0
    }
    BEGIN { exit !(cmp(a, b) < 0) }'
}

previous_version() {
  [ -d "$PREFIX/versions" ] || return 0
  # Newest installed version older than the current one, so a second rollback
  # keeps going back rather than returning to the version just left. Nothing
  # when the current version is unknown or none is older.
  [ -n "${current_version:-}" ] || return 0
  local candidate prev=""
  while read -r candidate; do
    version_older "$candidate" "$current_version" || continue
    if [ -z "$prev" ] || version_older "$prev" "$candidate"; then prev="$candidate"; fi
  done < <(installed_versions)
  [ -z "$prev" ] || printf '%s\n' "$prev"
}

# A version answering status.get says nothing about its ability to open local
# history. Hold the SAME locks as the queue migration across each check and
# binary switch, then release them before starting the core that resumes it.
storage_lock_fds=()
# --wait is for the rollback path: wait up to 60 s for a held lock, and say the
# rollback was not completed rather than that an update should be retried.
lock_storage_migrations() {
  [ "$DRY_RUN" = 0 ] || return 0
  local wait=0 store lock fd old_umask
  [ "${1:-}" != "--wait" ] || wait=1
  for store in "$HOME"/.agentworkforce/probe/*; do
    [ -d "$store" ] || continue
    [ -e "$store/history.db" ] || [ -e "$store/delivery-queue.db" ] || continue
    have flock || die "flock is required to protect local session history during an update"
    lock="$store/delivery-queue.migration-lock"
    if [ "$wait" = 1 ]; then
      [ ! -L "$lock" ] || die "rollback not completed: local history migration lock is a link; the new core ($VERSION) is still installed. Remove the link, then run the installer again"
    else
      [ ! -L "$lock" ] || die "local history migration lock is a link; installation unchanged"
    fi
    old_umask="$(umask)"
    umask 077
    if [ "$wait" = 1 ]; then
      exec {fd}>>"$lock" || die "rollback not completed: could not lock local session history; the new core ($VERSION) is still installed. Run the installer again"
    else
      exec {fd}>>"$lock" || die "could not lock local session history; installation unchanged"
    fi
    umask "$old_umask"
    if [ "$wait" = 1 ]; then
      flock -w 60 "$fd" || die "rollback not completed: local session history is still upgrading and the new core ($VERSION) did not answer. Wait for the upgrade to finish, then run the installer again"
    else
      flock -n "$fd" || die "local session history is upgrading; try the update again when it finishes"
    fi
    storage_lock_fds+=("$fd")
  done
}

unlock_storage_migrations() {
  local fd
  [ "${#storage_lock_fds[@]}" -gt 0 ] || return 0
  for fd in "${storage_lock_fds[@]}"; do
    flock -u "$fd"
    exec {fd}>&-
  done
  storage_lock_fds=()
}

storage_compatible() { # candidate binary; a check, never a schema repair
  [ -d "$HOME/.agentworkforce/probe" ] || return 0
  have python3 || { note "python3 is required to check local history compatibility"; return 1; }
  python3 -I - "$1" "$HOME/.agentworkforce/probe" <<'PY'
import json, pathlib, sqlite3, subprocess, sys

def refuse(message):
    print('agent-relay install: ' + message, file=sys.stderr)
    raise SystemExit(1)

required = set()
try:
    for store in pathlib.Path(sys.argv[2]).iterdir():
        if not store.is_dir():
            continue
        for name in ('history.db', 'delivery-queue.db'):
            path = store / name
            if not path.exists():
                continue
            if (name == 'delivery-queue.db' and path.is_symlink()) or not path.is_file():
                refuse('local history format cannot be verified; saved uploads were not changed')
            connection = sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=1)
            try:
                marker = connection.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='_relay_delivery_sidecar'").fetchone()
                if marker:
                    rows = connection.execute('SELECT manifest FROM _relay_delivery_sidecar WHERE singleton=1').fetchall()
                    if len(rows) != 1:
                        refuse('local history migration metadata is incomplete; saved uploads were not changed')
                    manifest = json.loads(rows[0][0])
                    if not isinstance(manifest, dict):
                        refuse('local history migration metadata is invalid; saved uploads were not changed')
                    version = manifest.get('version')
                    if type(version) is not int or version < 1:
                        refuse('local history migration format is unknown; saved uploads were not changed')
                    required.add(version)
                elif name == 'delivery-queue.db':
                    # open_or_create can publish an empty file before copy's
                    # first transaction commits. The owner can resume that
                    # interrupted migration; only a populated foreign queue
                    # is unverifiable. Internal SQLite objects are not data.
                    if connection.execute("SELECT 1 FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' LIMIT 1").fetchone():
                        refuse('local upload queue has no migration metadata; saved uploads were not changed')
                else:
                    moved = connection.execute("SELECT 1 FROM sqlite_master WHERE type='view' AND name='delivery_jobs'").fetchone()
                    if moved:
                        refuse('local history has an unverified upload migration; saved uploads were not changed')
                    required.add(0)
            finally:
                connection.close()
    result = subprocess.run([sys.argv[1], '--local-storage-capabilities'], stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=False)
    # Older releases have no capability command. They support legacy state only.
    if result.returncode or not result.stdout.strip():
        supported = [0]
    else:
        if len(result.stdout) > 4096:
            refuse('the target core returned invalid local history capabilities')
        try:
            capabilities = json.loads(result.stdout)
        except ValueError:
            refuse('the target core returned invalid local history capabilities')
        if not isinstance(capabilities, dict):
            refuse('the target core returned invalid local history capabilities')
        supported = capabilities.get('upload_queue_formats')
        if type(capabilities.get('protocol')) is not int or capabilities['protocol'] != 1 or not isinstance(supported, list) or not supported or any(type(n) is not int or n < 0 for n in supported):
            refuse('the target core returned invalid local history capabilities')
    if not (required or {0}).issubset(set(supported)):
        refuse('this core cannot read the upgraded local history; keeping saved uploads and the compatible installation. Install a newer compatible release instead of downgrading')
except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError):
    refuse('could not verify local history compatibility; saved uploads were not changed')
PY
}

# The unit and the health check say `relay serve` and `relay call`: every
# earlier release's core needs that word, and later ones drop it, so a
# rollback or a failed update going back to an earlier core still starts and
# answers. Like its name, the unit is the same for every product.
unit_text() {
  cat <<EOF
[Unit]
Description=Agent Relay headless service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
UMask=0077
ExecStart=$PREFIX/current/agent-relay-probe relay serve --headless
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
EOF
}

switch_to() { # version
  local link="$PREFIX/current"
  run ln -sfn "versions/$1" "$link.new"
  run mv -T "$link.new" "$link"
}

core_version() { # [binary] -> what the running core reports, or nothing
  "${1:-$PREFIX/current/agent-relay-probe}" relay call status.get --json 2>/dev/null \
    | grep -o '"daemon":{[^}]*}' | grep -o '"version":"[^"]*"' | head -n 1 | cut -d'"' -f4
}

healthy() { # version [binary to ask]
  [ "$DRY_RUN" = 1 ] && return 0
  local waited=0
  while [ "$waited" -lt "$HEALTH_TIMEOUT" ]; do
    [ "$(core_version "${2:-}" || true)" = "$1" ] && return 0
    sleep 2
    waited=$((waited + 2))
  done
  return 1
}

restart_service() { # fails when systemd could not (re)start the unit
  run systemctl --user daemon-reload || return 1
  # A unit that hit its start limit refuses a restart until it is reset.
  run systemctl --user reset-failed "$UNIT" 2>/dev/null || true
  if systemctl --user is-enabled --quiet "$UNIT" 2>/dev/null; then
    run systemctl --user restart "$UNIT"
  else
    run systemctl --user enable --now "$UNIT"
  fi
}

ensure_unit() {
  local wanted
  wanted="$(unit_text)"
  if [ -f "$UNIT_DIR/$UNIT" ] && [ "$(cat "$UNIT_DIR/$UNIT")" = "$wanted" ]; then
    return 0
  fi
  if [ -f "$UNIT_DIR/$UNIT" ]; then
    run cp -p "$UNIT_DIR/$UNIT" "$UNIT_DIR/$UNIT.before-install"
  fi
  run mkdir -p "$UNIT_DIR"
  if [ "$DRY_RUN" = 1 ]; then
    note "would write $UNIT_DIR/$UNIT"
  else
    printf '%s\n' "$wanted" > "$UNIT_DIR/$UNIT.new"
    mv "$UNIT_DIR/$UNIT.new" "$UNIT_DIR/$UNIT"
  fi
}

linger_note() {
  if have loginctl && [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || echo yes)" = no ]; then
    note "the core stops when you log out. To keep it running on a server, run (once):"
    note "  sudo loginctl enable-linger $(id -un)"
  fi
}

# Finish setup as a separate command: curl | bash does not leave a terminal
# on stdin, and installation must never silently change sign-in or preferences.
setup_note() {
  [ "$DRY_RUN" = 0 ] || return 0
  local binary="$PREFIX/current/agent-relay-probe"
  [ -x "$binary" ] || return 0
  if "$binary" setup --help >/dev/null 2>&1; then
    note "finish signing in and choosing providers with:"
    printf '  %q setup\n' "$binary"
    note "approve the displayed device code in a browser on any computer; this server needs no display."
    note "for automation, see: agent-relay-probe setup --help"
  else
    note "finish setup with the agent-driven setup guide:"
    note "  https://github.com/AgentWorkforce/relay-app/blob/main/docs/agent-driven-setup.md"
  fi
}

# --- rollback -----------------------------------------------------------------

if [ "$ROLLBACK" = 1 ]; then
  target="$(previous_version)"
  [ -n "$target" ] || die "no earlier version to roll back to"
  lock_storage_migrations
  storage_compatible "$PREFIX/versions/$target/agent-relay-probe" || die "rollback refused before changing the installation"
  note "rolling back ${current_version:-?} -> $target"
  switch_to "$target"
  ensure_unit
  unlock_storage_migrations
  restart_service || true
  healthy "$target" || die "the core did not come back on $target; see: journalctl --user -u $UNIT"
  note "now running $target"
  setup_note
  exit 0
fi

# --- 2. which release -------------------------------------------------------

if [ "$RELEASE_FEED" = cloud ]; then
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  cloud_resolve
elif [ -z "$VERSION" ]; then
  tmp_json="$(mktemp)"
  fetch "$RELEASES_API/latest" "$tmp_json" || die "could not read the latest release"
  VERSION="$(grep -o '"tag_name": *"[^"]*"' "$tmp_json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
  VERSION="${VERSION#v}"
  rm -f "$tmp_json"
  valid_version "$VERSION" || die "the latest release has no usable version"
fi
note "release: $VERSION ($ARCH)"

if [ "$current_version" = "$VERSION" ] && [ -z "$legacy" ] && [ "$current_product" != "$ASSET_PREFIX" ]; then
  note "the installed $VERSION is another product's; replacing it with $PRODUCT_NAME's"
fi
if [ "$current_version" = "$VERSION" ] && [ -z "$legacy" ] && [ "$current_product" = "$ASSET_PREFIX" ]; then
  lock_storage_migrations
  storage_compatible "$PREFIX/current/agent-relay-probe" || die "the installed core needs a compatible update"
  if [ "$DRY_RUN" = 1 ] || [ "$(core_version || true)" = "$VERSION" ]; then
    unlock_storage_migrations
    note "already up to date"
    linger_note
    setup_note
    exit 0
  fi
  note "installed but not answering; restarting it"
  ensure_unit
  unlock_storage_migrations
  restart_service || true
  healthy "$VERSION" || die "the core did not start; see: journalctl --user -u $UNIT"
  setup_note
  exit 0
fi

# --- 3. download, verify, unpack -------------------------------------------

asset="$ASSET_PREFIX-$VERSION-Linux-$ARCH-probe.tar.gz"
if [ "$RELEASE_FEED" = cloud ]; then
  # The manifest's size and sha256 stand in for the .sha256 sidecar.
  cloud_download "$asset"
else
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  note "downloading $asset"
  fetch "$DOWNLOAD_BASE/v$VERSION/$asset" "$work/$asset" || die "could not download $asset"
  fetch "$DOWNLOAD_BASE/v$VERSION/$asset.sha256" "$work/$asset.sha256" || die "could not download $asset.sha256"
  expected="$(awk '{print $1}' "$work/$asset.sha256" | head -n 1)"
  printf '%s' "$expected" | grep -Eq '^[0-9a-f]{64}$' || die "the published checksum is unreadable"
  [ "$(sha256_of "$work/$asset")" = "$expected" ] || die "$asset does not match its published checksum"
  note "checksum verified"
fi

# Only the published paths are accepted out of the archive: the core, and the
# project license shipped beside it (older releases hold the core alone).
listing="$(tar -tzf "$work/$asset")"
grep -qxF agent_relay/helpers/agent-relay-probe <<< "$listing" \
  || die "the archive holds no core"
if grep -vxE 'agent_relay/|agent_relay/helpers/|agent_relay/helpers/agent-relay-probe|agent_relay/LICENSE' \
  <<< "$listing" > /dev/null || [ -n "$(sort <<< "$listing" | uniq -d)" ]; then
  die "the archive holds something other than the core"
fi
has_license=0
if grep -qxF agent_relay/LICENSE <<< "$listing"; then has_license=1; fi
tar -xzf "$work/$asset" -C "$work" agent_relay/helpers/agent-relay-probe
if [ "$has_license" = 1 ]; then
  tar -xzf "$work/$asset" -C "$work" agent_relay/LICENSE
  # A link would copy whatever it points at into the install tree.
  { [ -f "$work/agent_relay/LICENSE" ] && [ ! -L "$work/agent_relay/LICENSE" ]; } \
    || die "the archive's LICENSE is not a plain file"
fi
reported="$(probe_version "$work/agent_relay/helpers/agent-relay-probe" || true)"
[ "$reported" = "$VERSION" ] || die "the downloaded core reports ${reported:-nothing}, not $VERSION"
lock_storage_migrations
storage_compatible "$work/agent_relay/helpers/agent-relay-probe" || die "installation refused before replacing the core"

destination="$PREFIX/versions/$VERSION"
if [ "$DRY_RUN" = 1 ]; then
  note "would install $destination"
else
  mkdir -p "$PREFIX/versions"
  rm -rf "$destination.partial"
  mkdir -p "$destination.partial"
  install -m 0755 "$work/agent_relay/helpers/agent-relay-probe" "$destination.partial/agent-relay-probe"
  if [ "$has_license" = 1 ]; then
    install -m 0644 "$work/agent_relay/LICENSE" "$destination.partial/LICENSE"
  fi
  printf '%s\n' "$ASSET_PREFIX" > "$destination.partial/product"
  # An existing copy of this version (another product's, or one left over)
  # is set aside until the new core answers, and comes back if it does not.
  rm -rf "$destination.replaced"
  if [ -d "$destination" ]; then mv "$destination" "$destination.replaced"; fi
  mv "$destination.partial" "$destination"
fi

# Puts back the copy of $VERSION this install set aside, if there was one.
restore_replaced() {
  [ -d "$destination.replaced" ] || return 0
  run rm -rf "$destination"
  run mv "$destination.replaced" "$destination"
}

# --- 4. switch and restart ---------------------------------------------------

was="${current_version:-}"
switch_to "$VERSION"
ensure_unit
unlock_storage_migrations
# A restart systemd refuses rolls back like a core that never answers.
restarted=1
restart_service || { restarted=0; note "systemd could not restart $UNIT"; }

# --- 5. health, or roll back -------------------------------------------------

if [ "$restarted" = 0 ] || ! healthy "$VERSION"; then
  lock_storage_migrations --wait
  # Moving over from the in-app updater's layout: its core is not under
  # versions/ (a versions/ entry of the same version is the release just
  # unpacked), so go back by restoring the unit that ran it.
  if [ -n "$legacy" ] && [ -f "$UNIT_DIR/$UNIT.before-install" ]; then
    storage_compatible "$legacy" || die "the new core did not answer; unsafe rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; going back to the installed $was"
    run mv "$UNIT_DIR/$UNIT.before-install" "$UNIT_DIR/$UNIT"
    run rm -f "$PREFIX/current"
    run rm -rf "${PREFIX:?}/versions/$VERSION"
    restore_replaced
    unlock_storage_migrations
    restart_service || true
    healthy "$was" "$legacy" || die "going back to $was did not answer either; see: journalctl --user -u $UNIT"
    die "update to $VERSION failed and the installed $was is running again; see: journalctl --user -u $UNIT"
  fi
  if [ -n "$was" ] && [ "$was" != "$VERSION" ] && [ -d "$PREFIX/versions/$was" ]; then
    storage_compatible "$PREFIX/versions/$was/agent-relay-probe" || die "the new core did not answer; unsafe rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; rolling back to $was"
    switch_to "$was"
    # A version that failed is never a rollback target.
    run rm -rf "${PREFIX:?}/versions/$VERSION"
    restore_replaced
    unlock_storage_migrations
    restart_service || true
    healthy "$was" || die "rollback to $was did not answer either; see: journalctl --user -u $UNIT"
    die "update to $VERSION failed and was rolled back to $was; see: journalctl --user -u $UNIT"
  fi
  # The same version of another product: put its copy back and restart it.
  if [ -n "$was" ] && [ "$was" = "$VERSION" ] && [ -d "$destination.replaced" ]; then
    storage_compatible "$destination.replaced/agent-relay-probe" || die "the new core did not answer; unsafe same-version rollback refused. Keep both releases and install a newer compatible release"
    note "the new core did not answer; going back to the installed $was"
    restore_replaced
    unlock_storage_migrations
    restart_service || true
    healthy "$was" || die "going back to $was did not answer either; see: journalctl --user -u $UNIT"
    die "replacing the installed $was failed and it is running again; see: journalctl --user -u $UNIT"
  fi
  die "the core did not start; see: journalctl --user -u $UNIT"
fi
note "running $VERSION"
[ "$DRY_RUN" = 1 ] || rm -rf "$destination.replaced"

# Moved over from the in-app updater's layout: its core is kept as a version
# of its own, so --rollback can go back to it.
if [ -n "$legacy" ] && [ -n "$was" ] && [ "$was" != "$VERSION" ] && valid_version "$was" \
  && [ ! -e "$PREFIX/versions/$was" ] && [ "$DRY_RUN" = 0 ]; then
  rm -rf "$PREFIX/versions/$was.partial"
  mkdir -p "$PREFIX/versions/$was.partial"
  install -m 0755 "$legacy" "$PREFIX/versions/$was.partial/agent-relay-probe"
  mv "$PREFIX/versions/$was.partial" "$PREFIX/versions/$was"
  note "kept the in-app updater's $was for --rollback"
fi

# Keep the newest KEEP versions (always the current one).
if [ "$DRY_RUN" = 0 ]; then
  installed_versions | { grep -vxF "$VERSION" || true; } | head -n -$((KEEP - 1)) \
    | while read -r old; do rm -rf "${PREFIX:?}/versions/$old"; done
fi
linger_note
setup_note

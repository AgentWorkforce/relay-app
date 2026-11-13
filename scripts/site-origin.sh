#!/usr/bin/env bash
#
# site-origin.sh -- the Agent Relay site and link scheme as the core builds
# them in, for the scripts that build a core and bundle it. Sourced, not run.
#
# `site_origin VALUE` prints VALUE as the origin the core's build.rs keeps,
# by the same rules (crates/relay-node/build_site.rs in AgentWorkforce/relay):
# trimmed, one trailing slash dropped, scheme and host lowercased; https on
# any host but localhost or 127.0.0.1, on its default port only (443, which
# is dropped); http on localhost or 127.0.0.1 only, on any port (80
# dropped; a port is read as a number, so :0080 is :80). Fails on anything
# else: a path, query, fragment or user info, no scheme, another scheme, a
# host that is no host name, a port that is empty or no number from 1 to
# 65535.
#
# `url_scheme_name VALUE` prints VALUE as the scheme the core keeps:
# trimmed and lowercased; fails on anything but a letter followed by
# letters, digits, `+`, `-` and `.`, and on the web's schemes (http, https,
# ws, wss, ftp, file), which a link of the app's cannot use.
#
# `built_in_from SITE SCHEME` settles what a build is made for from the
# option values given (either may be empty), else the exported
# AGENT_RELAY_SITE_URL / AGENT_RELAY_URL_SCHEME (said aloud), else
# production's: it exports each variable, normalized, for the cargo build
# to come, or unsets it, and leaves the normalized values in BUILT_IN_SITE
# and BUILT_IN_SCHEME. Fails naming a value it refuses.
#
# `record_built_in DIR NAME` writes NAME-site.txt and NAME-url-scheme.txt
# in DIR from those two, beside the binary just built, for the packaging
# scripts to check (`check_sidecars`, `check_probe`).
#
# A build's site profile (docs/workspace-layout.md, "Site profiles") is the
# rest of what is fixed when it is built: `profile_id VALUE` prints the
# opaque id of a profile (lowercase letters, digits and hyphens, at most 40;
# empty is the default profile, agentrelay) and fails on anything else;
# `asset_prefix PROFILE` prints what a release's artifact names start with,
# `AgentRelay` for the default profile and `Desktop-<id>` for any other, so
# nothing named outside an artifact carries a product name;
# `releases_repository VALUE` and `product_name VALUE` print those values
# checked (owner/name; a name with no control characters, slashes or
# quotes, which installer scripts and defines could not carry).
#
# `policy_defaults VALUE` prints a profile's DESKTOP_POLICY_DEFAULTS
# checked, as compact JSON with its keys sorted (what desktop-core's
# build.rs passes on), and nothing for an empty VALUE: a JSON object setting
# only sharing_mode (all, new or selected), upload_control (operator or
# user), oops_window_seconds (a whole number from 0 to 86400) and
# user_retroactive_delete (true or false), the checks build.rs and a fetched
# policy pass (desktop-core/src/history/operator_policy/settings.rs). Fails
# on anything else, printing nothing.
#
# `upload_storage_origins VALUE` prints a profile's
# DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS checked, as the compact sorted JSON
# array of canonical origins desktop-core's build.rs passes on, and nothing
# for an empty VALUE or list (so no `[]` is masked): a JSON array of https
# origins on DNS names (no IP literal, user info, path, query or fragment;
# the port 1 to 65535, 443 dropped), the checks build.rs makes
# (desktop-core/src/history/destination/direct_s3/storage_origins.rs). Fails
# on anything else, printing nothing.
#
# Where a build's updater reads releases is its release feed:
# `release_feed VALUE` prints it checked, either `github:<owner>/<name>` (the
# GitHub Releases of that repository, as every build before feeds) or
# `cloud` (the profile's own site; phase 2 of
# docs/enterprise-distribution.md), and fails on anything else, empty
# included. The default profile's is DEFAULT_RELEASE_FEED, its releases
# repository on GitHub. `feed_public_key VALUE` prints a cloud feed's
# Ed25519 public key checked: the base64 of 32 bytes (44 characters, one
# `=`), and fails on anything else.
#
# `settle_release_feed FEED REPOSITORY KEY SITE PROFILE` settles a build's
# feed from the values given (any may be empty but SITE, the normalized
# site built in; PROFILE is the opaque id, empty for the default profile):
# no FEED is github: with REPOSITORY (the default's when that is empty too);
# github:<owner>/<name> must agree with a REPOSITORY given; cloud takes no
# REPOSITORY, needs KEY, a profile and a site other than the default's. Only
# cloud takes a KEY. Leaves the feed, the repository (empty for cloud) and
# the key in SETTLED_FEED, SETTLED_REPOSITORY and SETTLED_KEY; fails saying
# why on stderr.

site_origin() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  [[ "$value" == *"://"* ]] || return 1
  local scheme="${value%%://*}"
  local rest="${value#*://}"
  rest="${rest%/}"
  [ -n "$scheme" ] && [ -n "$rest" ] || return 1
  case "$rest" in */*|*\?*|*\#*|*@*) return 1 ;; esac
  local host="${rest%%:*}"
  local port=""
  if [ "$host" != "$rest" ]; then
    port="${rest#*:}"
    [[ "$port" =~ ^[0-9]+$ ]] || return 1
    port=$((10#$port))
    [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || return 1
  fi
  [[ "$host" =~ ^[A-Za-z0-9.-]+$ ]] || return 1
  scheme="$(printf '%s' "$scheme" | tr '[:upper:]' '[:lower:]')"
  host="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
  local local_host=""
  case "$host" in localhost|127.0.0.1) local_host=1 ;; esac
  case "$scheme" in
    https)
      [ -z "$local_host" ] || return 1
      [ -z "$port" ] || [ "$port" = 443 ] || return 1
      printf 'https://%s\n' "$host"
      ;;
    http)
      [ -n "$local_host" ] || return 1
      [ "$port" != 80 ] || port=""
      printf 'http://%s%s\n' "$host" "${port:+:$port}"
      ;;
    *) return 1 ;;
  esac
}

url_scheme_name() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  value="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')"
  [[ "$value" =~ ^[a-z][a-z0-9+.-]*$ ]] || return 1
  case "$value" in http|https|ws|wss|ftp|file) return 1 ;; esac
  printf '%s\n' "$value"
}

# The default profile's values, for the scripts that source this file.
# shellcheck disable=SC2034
DEFAULT_PRODUCT_NAME="Agent Relay"
# shellcheck disable=SC2034
DEFAULT_RELEASES_REPOSITORY="AgentWorkforce/relay-desktop-releases"
# shellcheck disable=SC2034
DEFAULT_RELEASE_FEED="github:$DEFAULT_RELEASES_REPOSITORY"
DEFAULT_ASSET_PREFIX="AgentRelay"
DEFAULT_SITE_URL="https://agentrelay.com"
DEFAULT_URL_SCHEME="agentrelay"
# shellcheck disable=SC2034
DEFAULT_BUNDLE_ID="com.agentrelay.desktop"

BUILT_IN_SITE=""
BUILT_IN_SCHEME=""
built_in_from() {
  local site="$1" scheme="$2"
  if [ -z "$site" ] && [ -n "${AGENT_RELAY_SITE_URL:-}" ]; then
    site="$AGENT_RELAY_SITE_URL"
    printf 'AGENT_RELAY_SITE_URL is set: building for %s\n' "$site"
  fi
  if [ -z "$scheme" ] && [ -n "${AGENT_RELAY_URL_SCHEME:-}" ]; then
    scheme="$AGENT_RELAY_URL_SCHEME"
    printf 'AGENT_RELAY_URL_SCHEME is set: building with the link scheme %s://\n' "$scheme"
  fi
  if [ -n "$site" ]; then
    BUILT_IN_SITE="$(site_origin "$site")" || { printf 'the site must be an origin such as https://relay.example or http://localhost:3100, got: %s\n' "$site" >&2; return 1; }
    export AGENT_RELAY_SITE_URL="$BUILT_IN_SITE"
    printf 'building for the Agent Relay site %s\n' "$BUILT_IN_SITE"
  else
    BUILT_IN_SITE="$DEFAULT_SITE_URL"
    unset AGENT_RELAY_SITE_URL
  fi
  if [ -n "$scheme" ]; then
    BUILT_IN_SCHEME="$(url_scheme_name "$scheme")" || { printf 'the link scheme must be a letter followed by letters, digits, +, - or ., and not a web scheme, got: %s\n' "$scheme" >&2; return 1; }
    export AGENT_RELAY_URL_SCHEME="$BUILT_IN_SCHEME"
    printf 'building with the link scheme %s://\n' "$BUILT_IN_SCHEME"
  else
    BUILT_IN_SCHEME="$DEFAULT_URL_SCHEME"
    unset AGENT_RELAY_URL_SCHEME
  fi
}

record_built_in() {
  printf '%s\n' "$BUILT_IN_SITE" > "$1/$2-site.txt"
  printf '%s\n' "$BUILT_IN_SCHEME" > "$1/$2-url-scheme.txt"
}

profile_id() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  # Hyphen-separated runs of lowercase letters and digits, at most 40: no
  # leading or trailing hyphen and no two together, the grammar build-app.sh
  # --asset-prefix and the Mac updater (AppInfo.assetPrefix) take.
  [[ "$value" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] && [ "${#value}" -le 40 ] || return 1
  printf '%s\n' "$value"
}

asset_prefix() {
  if [ -z "$1" ]; then
    printf '%s\n' "$DEFAULT_ASSET_PREFIX"
  else
    printf 'Desktop-%s\n' "$1"
  fi
}

releases_repository() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  [[ "$value" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || return 1
  printf '%s\n' "$value"
}

release_feed() {
  local value="$1" repository
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  case "$value" in
    cloud) printf 'cloud\n' ;;
    github:*)
      # The repository as releases_repository takes it, with no space
      # inside the value (it is trimmed as a whole).
      [[ "${value#github:}" =~ ^[^[:space:]]+$ ]] || return 1
      repository="$(releases_repository "${value#github:}")" || return 1
      printf 'github:%s\n' "$repository"
      ;;
    *) return 1 ;;
  esac
}

feed_public_key() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  # 32 bytes in base64: 42 characters, a 43rd whose low two bits are zero
  # (the last byte's padding), and one `=`.
  [[ "$value" =~ ^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$ ]] || return 1
  printf '%s\n' "$value"
}

SETTLED_FEED=""
SETTLED_REPOSITORY=""
SETTLED_KEY=""
settle_release_feed() {
  local feed="$1" repository="$2" key="$3" site="$4" profile="$5"
  if [ -n "$repository" ]; then
    repository="$(releases_repository "$repository")" || { printf 'the releases repository must be owner/name, got: %s\n' "$repository" >&2; return 1; }
  fi
  if [ -n "$feed" ]; then
    feed="$(release_feed "$feed")" || { printf 'the release feed must be github:<owner>/<name> or cloud, got: %s\n' "$feed" >&2; return 1; }
  else
    feed="github:${repository:-$DEFAULT_RELEASES_REPOSITORY}"
  fi
  if [ "$feed" = cloud ]; then
    [ -z "$repository" ] || { printf 'a cloud release feed takes no releases repository; its releases are on its own site\n' >&2; return 1; }
    [ -n "$profile" ] || { printf 'a cloud release feed is a site profile'"'"'s: give the profile'"'"'s id\n' >&2; return 1; }
    [ "$site" != "$DEFAULT_SITE_URL" ] || { printf 'a cloud release feed is on the profile'"'"'s own site, not %s\n' "$DEFAULT_SITE_URL" >&2; return 1; }
    [ -n "$key" ] || { printf 'a cloud release feed needs its feed public key\n' >&2; return 1; }
    key="$(feed_public_key "$key")" || { printf 'the feed public key must be an Ed25519 public key, the base64 of 32 bytes\n' >&2; return 1; }
  else
    [ -z "$key" ] || { printf 'only a cloud release feed takes a feed public key\n' >&2; return 1; }
    if [ -n "$repository" ] && [ "$(printf '%s' "${feed#github:}" | tr '[:upper:]' '[:lower:]')" != "$(printf '%s' "$repository" | tr '[:upper:]' '[:lower:]')" ]; then
      printf 'the release feed %s does not name the releases repository %s\n' "$feed" "$repository" >&2; return 1
    fi
    repository="${repository:-${feed#github:}}"
    feed="github:$repository"
  fi
  # Read by the scripts that source this file.
  # shellcheck disable=SC2034
  SETTLED_FEED="$feed"
  # shellcheck disable=SC2034
  SETTLED_REPOSITORY="$repository"
  # shellcheck disable=SC2034
  SETTLED_KEY="$key"
}

product_name() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  [ -n "$value" ] && [ "${#value}" -le 64 ] || return 1
  local pattern='^[^[:cntrl:]/\\"'"'"':*?<>|{},]+$'
  [[ "$value" =~ $pattern ]] || return 1
  printf '%s\n' "$value"
}

policy_defaults() {
  # The value goes to Python through the environment, not its arguments, and
  # is never printed but as the checked JSON on success.
  local python_command=python3
  command -v python3 >/dev/null 2>&1 || python_command=python
  POLICY_DEFAULTS_VALUE="$1" "$python_command" -I -c '
import json, os, sys
text = os.environ["POLICY_DEFAULTS_VALUE"]
if not text.strip():
    sys.exit(0)
def refuse(_):
    raise ValueError
try:
    value = json.loads(text, parse_constant=refuse)
except ValueError:
    sys.exit(1)
if not isinstance(value, dict):
    sys.exit(1)
checks = {
    "sharing_mode": lambda v: v in ("all", "new", "selected"),
    "upload_control": lambda v: v in ("operator", "user"),
    "oops_window_seconds": lambda v: type(v) is int and 0 <= v <= 86400,
    "user_retroactive_delete": lambda v: type(v) is bool,
}
for key, setting in value.items():
    check = checks.get(key)
    if check is None or not check(setting):
        sys.exit(1)
print(json.dumps(value, sort_keys=True, separators=(",", ":")))
'
}

upload_storage_origins() {
  # The value goes to Python through the environment, not its arguments, and
  # is never printed but as the checked JSON on success.
  local python_command=python3
  command -v python3 >/dev/null 2>&1 || python_command=python
  STORAGE_ORIGINS_VALUE="$1" "$python_command" -I -c '
import json, os, re, sys
text = os.environ["STORAGE_ORIGINS_VALUE"]
if not text.strip():
    sys.exit(0)
def refuse(_):
    raise ValueError
try:
    value = json.loads(text, parse_constant=refuse)
except ValueError:
    sys.exit(1)
if not isinstance(value, list):
    sys.exit(1)
label = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\Z")
number = re.compile(r"([0-9]+|0x[0-9a-f]*)\Z")
def canonical(raw):
    if not isinstance(raw, str):
        sys.exit(1)
    scheme, sep, rest = raw.strip().partition("://")
    if not sep or scheme.lower() != "https" or "?" in rest or "#" in rest:
        sys.exit(1)
    authority, _, path = rest.partition("/")
    if path or "@" in authority or authority.startswith("["):
        sys.exit(1)
    host, colon, port = authority.rpartition(":")
    if not colon:
        host, port = authority, "443"
    if not re.fullmatch(r"[0-9]+", port) or not 1 <= int(port) <= 65535:
        sys.exit(1)
    host = host.lower()
    labels = host.split(".")
    if not host.isascii() or len(host) > 253 or not all(label.match(part) for part in labels) or number.match(labels[-1]):
        sys.exit(1)
    return "https://" + host + ("" if int(port) == 443 else ":" + str(int(port)))
origins = sorted({canonical(entry) for entry in value})
if origins:
    print(json.dumps(origins, separators=(",", ":")))
'
}

# `sidecars_beside EXE NAME` prints "SITE SCHEME" from NAME-site.txt and
# NAME-url-scheme.txt beside EXE (scripts/build-probe.sh writes
# probe-site.txt and probe-url-scheme.txt beside a core), normalized. Fails
# with status 2 when either file is missing, 1 when one is malformed.
sidecars_beside() {
  local dir site scheme
  dir="$(dirname "$1")"
  [ -f "$dir/$2-site.txt" ] && [ -f "$dir/$2-url-scheme.txt" ] || return 2
  site="$(site_origin "$(tr -d '\n\r' < "$dir/$2-site.txt")")" || return 1
  scheme="$(url_scheme_name "$(tr -d '\n\r' < "$dir/$2-url-scheme.txt")")" || return 1
  printf '%s %s\n' "$site" "$scheme"
}

# `check_sidecars EXE NAME SITE SCHEME` succeeds when the sidecars beside
# EXE say it is built for SITE and opens SCHEME:// links; says why on
# stderr otherwise, with status 2 when they are missing.
check_sidecars() {
  local exe="$1" name="$2" want_site="$3" want_scheme="$4" pair site scheme status
  pair="$(sidecars_beside "$exe" "$name")"
  status=$?
  case "$status" in
    2) printf '%s has no %s-site.txt and %s-url-scheme.txt beside it to say which site and link scheme it is built for\n' "$exe" "$name" "$name" >&2; return 2 ;;
    0) ;;
    *) printf '%s-site.txt or %s-url-scheme.txt beside %s is malformed: a site is an origin such as https://agentrelay.com or http://localhost:3100, a scheme a name such as agentrelay\n' "$name" "$name" "$exe" >&2; return 1 ;;
  esac
  site="${pair%% *}"
  scheme="${pair##* }"
  [ "$site" = "$want_site" ] || { printf '%s is built for %s, not %s\n' "$exe" "$site" "$want_site" >&2; return 1; }
  [ "$scheme" = "$want_scheme" ] || { printf '%s opens %s:// links, not %s://\n' "$exe" "$scheme" "$want_scheme" >&2; return 1; }
}

# `probe_help_site PROBE` prints the site PROBE itself reports, the default
# of its `cloud install --site-url`, when PROBE runs on this host; the core
# has no print for its link scheme.
probe_help_site() {
  "$1" cloud install --help 2>/dev/null |
    awk '/--site-url/ {site = 1} site && match($0, /\[default: [^]]*\]/) {print substr($0, RSTART + 10, RLENGTH - 11); exit}'
}

# `check_probe PROBE SITE SCHEME [self-built]` succeeds when the core PROBE
# is built for SITE and opens SCHEME:// links: by the sidecars beside it
# (scripts/build-probe.sh writes them; a malformed one fails here), else by
# the site it prints and, with `self-built` (the caller made it with neither
# variable set), the default scheme. Says why on stderr otherwise.
check_probe() {
  local probe="$1" want_site="$2" want_scheme="$3" self_built="${4:-}" site
  check_sidecars "$probe" probe "$want_site" "$want_scheme" 2>/dev/null
  case $? in
    0) return 0 ;;
    1) check_sidecars "$probe" probe "$want_site" "$want_scheme"; return 1 ;;
  esac
  site="$(probe_help_site "$probe")"
  [ -n "$site" ] || { printf 'the core %s has no probe-site.txt and probe-url-scheme.txt beside it (scripts/build-probe.sh writes them) and does not run here to say its site\n' "$probe" >&2; return 1; }
  [ "$site" = "$want_site" ] || { printf 'the core %s is built for %s, not %s\n' "$probe" "$site" "$want_site" >&2; return 1; }
  [ -n "$self_built" ] || { printf 'the core %s has no probe-url-scheme.txt beside it to say its link scheme; build it with scripts/build-probe.sh\n' "$probe" >&2; return 1; }
}

"""Publisher client for a site profile's cloud release feed.

This follows the proposed Cloud contract (relay-control docs/desktop-feed.md).
Its paths, bodies and error codes are proposals, not a stable API:
when Cloud settles them, this module and its tests change with them.

The publisher API lives under {site}/cloud/api/v1/desktop/feed:

- GET  /releases?channel=&include_drafts=true lists every version, drafts
  included.
- POST /releases creates a draft that declares its complete asset set
  (expected_assets). Repeating the same body is 200, so a rerun resumes.
- PUT  /releases/{version}/assets/{name} uploads one declared asset, with
  X-Content-SHA256 and Content-Length. The same bytes again are 200.
- POST /releases/{version}/publish signs the release once every declared
  asset is uploaded, with {latest: true} for a stable.

The listing also carries limits.max_canonical_bytes: the most RFC 8785
bytes the profile's signer signs (4096 for an AWS KMS Ed25519 key, which
signs a raw message). The contract reads a null limit as "the signer
declares none"; the publisher deliberately does NOT treat null or absent as
uncapped. It assumes the KMS default of 4096 (DEFAULT_MAX_CANONICAL_BYTES)
instead, the safer real-world value, since every hosted signer today is a
KMS key, and says so in its pre-flight line. The feed refuses a larger
manifest at POST /releases, after its version_exists check, and again at
publish, with 422 manifest_too_large and the canonical_bytes and
max_canonical_bytes it measured. So a publish first reads the limit and
measures the manifest it will produce: it measures the manifest with empty
notes, cuts the release notes on whole lines to what fits (at most
NOTES_MAX_BYTES, less when the assets leave less room under the limit; cut
notes end with a link to the full notes, full_notes_line), measures again,
and stops before creating a draft when even empty notes do not fit. A 422
manifest_too_large that gets through anyway is a refusal (exit 2), never
retried and never read as a replay; at publish it names the draft it leaves.

Every request carries X-Agent-Relay-Profile and the publisher token as a
bearer token. The token is read from DESKTOP_FEED_PUBLISHER_TOKEN only. It
is never an argument, never printed, and is redacted from every message
this module writes, urllib's errors included.

Everything is checked before the first request, and the client fails
closed. It refuses a missing site, profile or token, a feed other than
cloud, a site that is not an https origin, a malformed name or version,
and a package not named <asset_prefix>-<version>-. Redirects are refused
too, so the token never follows one to another host.
"""

import hashlib
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request

API_PATH = "/cloud/api/v1/desktop/feed"
TOKEN_ENV = "DESKTOP_FEED_PUBLISHER_TOKEN"
MASK = "***"
# The proposed contract's patterns. Unlike the GitHub feed's YYYY.MM.N, a
# version's month has no leading zero.
VERSION = re.compile(r"[0-9]{4}\.(?:[1-9]|1[0-2])\.(?:0|[1-9][0-9]*)(?:-beta\.(?:0|[1-9][0-9]*))?\Z")
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_+-]*(?:\.[A-Za-z0-9_+-]+)*\Z")
PREFIX = re.compile(r"[A-Za-z0-9][A-Za-z0-9_+-]*\Z")
PROFILE = re.compile(r"[a-z0-9]{1,32}\Z")
SITE = re.compile(r"https://[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]{1,5})?\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
ASSET_ID = re.compile(r"a_[0-9A-Za-z]{1,64}\Z")
MAX_ASSETS = 64
MAX_NAME = 255
MAX_NOTES = 65536
MAX_PAGES = 100
RETRIES = 3
TIMEOUT = 300
# What follows <asset_prefix>-<version>- in each package's name, and what it
# declares. A macOS image is floored (macOS15-arm64.dmg) or not.
KINDS = (
    (re.compile(r"macOS([0-9]{2})?-(arm64|x64)\.dmg"), "macOS", "image"),
    (re.compile(r"macOS-(arm64|x64)-probe\.tar\.gz"), "macOS", "probe"),
    (re.compile(r"Linux-(arm64|x64)\.tar\.gz"), "Linux", "tarball"),
    (re.compile(r"Linux-(arm64|x64)\.deb"), "Linux", "deb"),
    (re.compile(r"Linux-(arm64|x64)-probe\.tar\.gz"), "Linux", "probe"),
    (re.compile(r"Windows-(arm64|x64)\.zip"), "Windows", "zip"),
    (re.compile(r"Windows-(arm64|x64)-probe\.zip"), "Windows", "probe"),
    (re.compile(r"Windows-(arm64|x64)-Setup\.exe"), "Windows", "setup"),
)
# The fields the signing Cloud fills in at publish, counted at their longest
# so the measured size is an upper bound:
# - an asset id is always "a_" and 8 base62 characters, 10 in all;
# - published_at is always 20 (YYYY-MM-DDTHH:MM:SSZ);
# - download_hosts is budgeted as ONE host of 100 characters. The schema
#   allows up to 8 hosts, but the feed owner confirmed a deployment fills in
#   exactly one (an S3 deployment's bucket host: a bucket name of at most 63
#   characters without dots, plus ".s3.<region>.amazonaws.com") or none (a
#   streaming-only site, whose empty list is smaller). So one 100-character
#   host is the most it can add.
BOUND_ID = "a_" + "0" * 8
BOUND_PUBLISHED_AT = "0000-00-00T00:00:00Z"
BOUND_DOWNLOAD_HOSTS = ["h" * 100]
MAX_SAFE_INTEGER = 2 ** 53 - 1
# The signer's cap when the listing reports none: an AWS KMS Ed25519 key
# signs at most 4096 raw bytes.
DEFAULT_MAX_CANONICAL_BYTES = 4096
# Release notes stay inside the signed manifest, cut on whole lines to at
# most this many canonical bytes for every cloud feed, and to less when the
# assets leave less room under the signer's limit (notes_budget).
NOTES_MAX_BYTES = 300
# Cut notes end with a link to the full notes. The contract defines no
# per-release notes URL; the one page a cloud site serves for its releases is
# the downloads page (docs/enterprise-distribution.md, the Windows
# installer's SupportUrl), so the link is that page with the version as a
# fragment, which never changes what the site serves.
DOWNLOADS_PATH = "/cloud/dashboard/downloads"
# The headless installers carry no version in their names; each declares
# x64, the convention install.sh set before install.ps1 joined it.
INSTALL_SCRIPTS = {"install.sh": "Linux", "install.ps1": "Windows"}


class FeedError(Exception):
    """A failed request or a reply the client cannot accept; the message is
    safe to print."""


class Refusal(FeedError):
    """Exit 2. Refused before any request was sent (missing or malformed
    input), or a manifest too large for the signer: the pre-flight's measure,
    before any draft exists, or the feed's 422 manifest_too_large, which at
    create leaves nothing behind and at publish leaves the uploaded draft
    (its message says which). A rerun with the same input is refused again."""


def redact(text, token):
    text = str(text)
    return text.replace(token, MASK) if token else text


def check_site(site):
    if not site:
        raise Refusal("the site URL is missing")
    if not SITE.fullmatch(site):
        raise Refusal("the site must be an https origin (https://host[:port], no path)")
    return site


def check_profile(profile):
    if not profile:
        raise Refusal("the profile id is missing")
    if not PROFILE.fullmatch(profile):
        raise Refusal("the profile id must be 1 to 32 lowercase letters and digits")
    return profile


def check_token(token):
    if not token:
        raise Refusal(f"{TOKEN_ENV} is not set; the publisher token is read from it only")
    if any(ord(c) < 0x21 or ord(c) > 0x7e for c in token):
        raise Refusal(f"{TOKEN_ENV} must be printable ASCII without spaces")
    return token


def check_feed(feed):
    if feed != "cloud":
        raise Refusal("the profile's RELEASE_FEED is not cloud; a github: feed publishes to its repository")


def check_release(version, channel):
    if not VERSION.match(version):
        raise Refusal(f"{version!r} is not a cloud feed version: YYYY.M.N[-beta.N], month without a leading zero")
    if channel not in ("stable", "beta"):
        raise Refusal("the channel must be stable or beta")
    if ("-beta." in version) != (channel == "beta"):
        raise Refusal("a beta version needs channel beta, and a stable version channel stable")


def describe(name, prefix, version):
    """What an asset name declares: platform, arch, kind and os_floor."""
    if len(name) > MAX_NAME or not NAME.match(name):
        raise Refusal(f"{name!r} is not a single-segment asset name")
    if name in INSTALL_SCRIPTS:
        # One script installs either arch, but the proposed contract's arch
        # is arm64 or x64 only: x64 is a placeholder the apps and the
        # installers ignore for an install-script (docs/workspace-layout.md).
        return {"platform": INSTALL_SCRIPTS[name], "arch": "x64", "kind": "install-script"}
    bound = f"{prefix}-{version}-"
    if not name.startswith(bound):
        raise Refusal(f"{name} is not named {bound}...; only install.sh and install.ps1 may be named otherwise")
    rest = name[len(bound):]
    for pattern, platform, kind in KINDS:
        match = pattern.fullmatch(rest)
        if not match:
            continue
        entry = {"platform": platform, "arch": match.groups()[-1], "kind": kind}
        if kind == "image" and match[1]:
            floor = int(match[1])
            if not 10 <= floor <= 99:
                raise Refusal(f"{name} has a macOS floor outside 10 to 99")
            entry["os_floor"] = floor
        return entry
    raise Refusal(f"{name} is not a package the release pipeline publishes")


def canonical(value):
    """The RFC 8785 (JCS) UTF-8 bytes of a manifest-shaped value, as the
    signing Cloud and desktop-core/src/feed/jcs.rs write them: members sorted
    by the UTF-16 code units of their names, no whitespace, strings escaped
    as section 3.2.2.2 requires (json.dumps without ensure_ascii does exactly
    that), and only safe integers as numbers."""
    if value is None or isinstance(value, bool):
        return {None: b"null", True: b"true", False: b"false"}[value]
    if isinstance(value, int):
        if abs(value) > MAX_SAFE_INTEGER:
            raise ValueError("a number is not a safe integer")
        return str(value).encode()
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False).encode()
    if isinstance(value, list):
        return b"[" + b",".join(canonical(item) for item in value) + b"]"
    if isinstance(value, dict):
        names = sorted(value, key=lambda name: name.encode("utf-16-be"))
        return b"{" + b",".join(canonical(name) + b":" + canonical(value[name]) for name in names) + b"}"
    raise ValueError(f"{type(value).__name__} has no canonical form here")


def manifest_digest(manifest):
    """The SHA-256 (hex) of a manifest's canonical bytes: exactly what the
    Control signs and the installed app verifies."""
    return hashlib.sha256(canonical(manifest)).hexdigest()


def manifest_bound(profile, version, channel, notes, assets):
    """The manifest Cloud would sign for this publish, with the fields it
    fills in at their longest (BOUND_*)."""
    return {"schema": 1, "profile": profile, "channel": channel, "version": version,
            "published_at": BOUND_PUBLISHED_AT, "notes": notes, "download_hosts": BOUND_DOWNLOAD_HOSTS,
            "assets": [dict(asset, id=BOUND_ID) for asset in assets]}


def canonical_size(profile, version, channel, notes, assets):
    """At most how many canonical bytes the signed manifest has."""
    return len(canonical(manifest_bound(profile, version, channel, notes, assets)))


def notes_size(text):
    """The canonical (escaped, UTF-8) bytes of the notes, without quotes."""
    return len(canonical(text)) - 2


def notes_budget(profile, version, channel, assets, limit, cap=NOTES_MAX_BYTES):
    """How many canonical bytes of notes fit: min(cap, limit less the bound
    of the manifest with empty notes), never below 0. Notes of n canonical
    bytes add exactly n to the bound, so notes within the budget fit unless
    the assets alone do not. Returns (budget, the empty-notes bound, why)."""
    empty = canonical_size(profile, version, channel, "", assets)
    room = limit - empty
    if cap <= room:
        return cap, empty, f"the fixed cap of {cap}"
    why = (f"the room {len(assets)} assets leave under the signer limit of {limit}"
           f" ({empty} bytes with empty notes)")
    return max(room, 0), empty, why


def full_notes_line(site, version):
    """The line cut notes end with: the site's downloads page, the only
    release page the contract gives a cloud site, with the version as a
    fragment (DOWNLOADS_PATH)."""
    site = check_site(site)
    if site.endswith(":443"):  # https's default port: both spellings cut the same notes
        site = site[:-4]
    return f"Full notes: {site}{DOWNLOADS_PATH}#v{version}\n"


def cap_notes(notes, say, cap=NOTES_MAX_BYTES, why=None, link=""):
    """The notes cut to their leading whole lines whose canonical bytes fit
    in `cap`; how many bytes were kept, and why (`why`, else the fixed cap),
    is always logged. Notes that fit are kept whole and get no link. Cut
    notes, given a `link` line, keep room for it inside `cap`: their trailing
    blank and heading lines are dropped and a blank line and the link follow
    (when even the link does not fit, the cut has no link, and says so). The
    cut depends on the notes, the cap and the link alone, and notes_budget's
    cap on the profile, version, channel, assets and limit alone, so a rerun
    with the same inputs declares the same body and resumes its draft
    instead of a 409; capped notes cap to themselves."""
    size = notes_size
    why = why or f"the fixed cap of {cap}"
    lines = notes.splitlines(keepends=True)
    if size(notes) <= cap:
        say(f"Release notes: kept all {size(notes)} canonical bytes, within a cap of {cap} ({why}).")
        return notes
    tail = "\n" + link if link else ""
    if tail and size(tail) > cap:
        say(f"Release notes: the link to the full notes ({size(tail)} canonical bytes) does not fit in"
            f" {cap}; the cut notes carry no link.")
        tail = ""
    room = cap - size(tail)
    kept = []
    for line in lines:
        if size("".join(kept) + line) > room:
            break
        kept.append(line)
    if tail:
        while kept and (not kept[-1].strip() or re.match(r"\s{0,3}#{1,6}(\s|$)", kept[-1])):
            kept.pop()
        capped = "".join(kept) + (tail if kept else link)
    else:
        capped = "".join(kept)
    linked = " and a link to the full notes" if tail else ""
    say(f"Release notes: kept {size(capped)} of {size(notes)} canonical bytes ({len(kept)} of"
        f" {len(lines)} whole lines{linked}), capped at {cap} ({why}).")
    return capped


def fit_notes(say, profile, version, channel, notes, assets, limit, link=""):
    """The pre-flight's order: measure the manifest with empty notes, derive
    the notes budget, cap the notes to it (cut notes end with `link`, inside
    the budget), and measure the final manifest. Returns (capped notes,
    final bound); the final bound exceeds `limit` only when the assets alone
    do not fit."""
    budget, _, why = notes_budget(profile, version, channel, assets, limit)
    capped = cap_notes(notes, say, budget, why, link)
    return capped, canonical_size(profile, version, channel, capped, assets)


def file_digest(path):
    checksum = hashlib.sha256()
    size = 0
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
            size += len(chunk)
    return checksum.hexdigest(), size


def expected_assets(directory, names, prefix, version):
    """Declare exactly the named files, in the order given. The set is never
    inferred: every name must be listed, exist, and appear once."""
    if not PREFIX.match(prefix):
        raise Refusal("the asset prefix must be a single name segment without dots")
    if not names:
        raise Refusal("no assets are listed")
    if len(names) > MAX_ASSETS:
        raise Refusal(f"at most {MAX_ASSETS} assets may be declared")
    entries, seen, kinds = [], set(), set()
    for name in names:
        entry = {"name": name, **describe(name, prefix, version)}
        if name in seen:
            raise Refusal(f"{name} is listed twice")
        key = (entry["platform"], entry["arch"], entry["kind"], entry.get("os_floor"))
        if key in kinds:
            raise Refusal(f"{name} repeats another asset's platform, arch, kind and floor")
        seen.add(name)
        kinds.add(key)
        path = directory / name
        if path.is_symlink() or not path.is_file():
            raise Refusal(f"{name} is not a regular file in the asset directory")
        entry["sha256"], entry["size"] = file_digest(path)
        if entry["size"] < 1:
            raise Refusal(f"{name} is empty")
        entries.append(entry)
    return entries


def request_bytes(body):
    """The exact bytes a JSON request body is sent as."""
    return json.dumps(body, separators=(",", ":")).encode()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise FeedError(f"the feed answered {code} with a redirect, which is never followed")


def default_open(request, timeout):
    return urllib.request.build_opener(NoRedirect).open(request, timeout=timeout)


class Response:
    def __init__(self, status, body):
        self.status = status
        self.body = body

    @property
    def code(self):
        return self.body.get("error") if isinstance(self.body, dict) else None


class Client:
    """One profile's publisher calls. `open_url` is the transport, which
    tests replace; it never changes what is sent."""

    def __init__(self, site, profile, token, open_url=default_open, sleep=time.sleep, out=print):
        self.site = check_site(site)
        self.base = self.site + API_PATH
        self.profile = check_profile(profile)
        self.token = check_token(token)
        self.open_url = open_url
        self.sleep = sleep
        self.out = out

    def headers(self, masked=False):
        return {"Authorization": "Bearer " + (MASK if masked else self.token),
                "X-Agent-Relay-Profile": self.profile}

    def say(self, text):
        self.out(redact(text, self.token))

    def call(self, method, path, body=None, headers=None, upload=None):
        """Send one request. A JSON body is serialised; an upload is a path
        streamed with its declared length. Returns a Response for any HTTP
        status; a transport failure is a FeedError."""
        request_headers = dict(self.headers(), **(headers or {}))
        for attempt in range(RETRIES + 1):
            data = None
            if body is not None:
                data = request_bytes(body)
                request_headers["Content-Type"] = "application/json"
            stream = open(upload, "rb") if upload else None
            if stream:
                data = stream
            request = urllib.request.Request(self.base + path, data=data, method=method,
                                             headers=request_headers)
            try:
                try:
                    with self.open_url(request, TIMEOUT) as reply:
                        status, raw, retry = reply.status, reply.read(), None
                except urllib.error.HTTPError as error:
                    status, raw, retry = error.code, error.read(), error.headers.get("Retry-After")
                    error.close()
            except FeedError:
                raise
            except (OSError, ValueError, urllib.error.URLError) as error:
                raise FeedError(f"{method} {API_PATH}{path} failed: {redact(error, self.token)}") from None
            finally:
                if stream:
                    stream.close()
            try:
                parsed = json.loads(raw) if raw else None
            except ValueError:
                parsed = None
            if status == 429 and attempt < RETRIES:
                wait = int(retry) if retry and retry.isdigit() else 30
                self.say(f"Rate limited on {method} {API_PATH}{path}; retrying in {min(wait, 60)}s.")
                self.sleep(min(wait, 60))
                continue
            return Response(status, parsed)
        return Response(status, parsed)

    def fail(self, step, response, extra=""):
        code = response.code or "no error code"
        message = response.body.get("message") if isinstance(response.body, dict) else None
        detail = f" ({message})" if isinstance(message, str) else ""
        meaning = {
            401: "the publisher token was refused: unauthenticated, expired or revoked",
            403: "the token may not publish this profile's feed",
        }.get(response.status, "")
        if response.code == "not_entitled":
            meaning = "the profile is not entitled to a cloud feed"
        text = f"{step} failed: HTTP {response.status} {code}{detail}"
        if meaning:
            text += f": {meaning}"
        raise FeedError(redact(text + extra, self.token))

    def listing(self, channel="", missing_feed_empty=False):
        """Every version of the feed, drafts included, across pages.

        With missing_feed_empty, a first page answered HTTP 404 not_found
        (the site does not serve the feed route) returns None, which a
        preparation reads as an empty feed. Every other status, and every
        transport failure, still fails."""
        versions, page = [], None
        for _ in range(MAX_PAGES):
            query = {"include_drafts": "true"}
            if channel:
                query["channel"] = channel
            if page:
                query["page"] = str(page)
            response = self.call("GET", "/releases?" + urllib.parse.urlencode(query))
            if missing_feed_empty and page is None and response.status == 404 and response.code == "not_found":
                return None
            if response.status != 200 or not isinstance(response.body, dict):
                self.fail("Listing the feed", response)
            batch = response.body.get("versions")
            if not isinstance(batch, list) or not all(
                    isinstance(entry, dict) and isinstance(entry.get("version"), str) for entry in batch):
                raise FeedError("the feed listing has no versions list")
            versions += batch
            page = response.body.get("next_page")
            if page is None:
                return {"versions": versions}
            if not isinstance(page, int) or isinstance(page, bool) or page < 2:
                raise FeedError("the feed listing has an invalid next_page")
        raise FeedError(f"the feed listing has more than {MAX_PAGES} pages")

    def max_canonical_bytes(self, channel=""):
        """The listing's limits.max_canonical_bytes: a positive integer, or
        None (null or absent) when the listing reports none."""
        query = {"include_drafts": "true", **({"channel": channel} if channel else {})}
        response = self.call("GET", "/releases?" + urllib.parse.urlencode(query))
        if response.status != 200 or not isinstance(response.body, dict):
            self.fail("Reading the feed's limits", response)
        limits = response.body.get("limits")
        limit = limits.get("max_canonical_bytes") if isinstance(limits, dict) else None
        if (limits is not None and not isinstance(limits, dict)) or (limit is not None and (
                not isinstance(limit, int) or isinstance(limit, bool) or limit < 1)):
            raise FeedError("the feed listing's limits.max_canonical_bytes is neither a positive integer nor null")
        return limit


def preflight(client, version, channel, notes, assets):
    """Read the signer's limit, cut the notes to what fits under it
    (fit_notes, with the link to the full notes), and refuse, before any
    draft or upload, a manifest whose canonical form would still exceed it:
    that is, assets that do not fit even with empty notes. A null or absent
    limit is the KMS default of 4096, never uncapped. Prints both numbers;
    returns the capped notes and a sentence on how they were cut, which a
    409 on a draft quotes (the cut follows the listed limit)."""
    listed = client.max_canonical_bytes(channel)
    limit = DEFAULT_MAX_CANONICAL_BYTES if listed is None else listed
    source = ("" if listed is not None else f" (the KMS default of {DEFAULT_MAX_CANONICAL_BYTES}: the listing"
              " gives none, and null or absent is not read as uncapped)")
    link = full_notes_line(client.site, version)
    notes, size = fit_notes(client.say, client.profile, version, channel, notes, assets, limit, link)
    budget = notes_budget(client.profile, version, channel, assets, limit)[0]
    client.say(f"Manifest canonical size: at most {size} bytes for {len(assets)} assets;"
               f" signer limit: {limit}{source}.")
    if size > limit:
        raise Refusal(f"the signed manifest would be up to {size} canonical bytes, over the signer's"
                      f" max_canonical_bytes of {limit}; nothing was created or uploaded. The"
                      f" {len(assets)} assets do not fit even with empty release notes: publish fewer assets.")
    cut = (f"this attempt cut the release notes to a budget of {budget} canonical bytes under a signer limit"
           f" of {limit}{' (the default; the listing gives none)' if listed is None else ''}")
    return notes, cut


def refuse_too_large(client, step, response, draft=None):
    """A 422 manifest_too_large: the feed measured the manifest over its
    signer's cap. Never retried (the same body is refused again) and never a
    replay; a refusal (exit 2) quoting both numbers. At create nothing was
    made. At publish (`draft` is (version, asset count)) the draft stays
    with every asset uploaded: the contract has no draft deletion, so the
    message names it and what a rerun does."""
    body = response.body if isinstance(response.body, dict) else {}
    measured = lambda key: body.get(key) if isinstance(body.get(key), int) else "unknown"
    text = (f"{step} failed: HTTP 422 manifest_too_large: the feed measured the signed manifest at"
            f" canonical_bytes {measured('canonical_bytes')}, over its max_canonical_bytes of"
            f" {measured('max_canonical_bytes')}; nothing was published, and it is not retried.")
    if draft:
        version, count = draft
        text += (f" The draft of v{version} stays on the feed with all {count} declared assets uploaded."
                 " A rerun with the same notes and packages resumes it (the create and the uploads replay)"
                 " and publishes once the signer's limit takes the manifest. Other notes or assets need a new"
                 " version, since the publisher cannot delete a draft (an admin can).")
    else:
        text += " Nothing was created. Publish fewer assets or shorter notes."
    raise Refusal(redact(text, client.token))


def plan(version, channel, notes, assets):
    """The requests a publish sends, in order: (method, path, body or None,
    extra headers, asset name or None)."""
    quoted = urllib.parse.quote(version, safe="")
    steps = [("POST", "/releases", {"version": version, "channel": channel, "notes": notes,
                                    "expected_assets": assets}, {}, None)]
    for asset in assets:
        steps.append(("PUT", f"/releases/{quoted}/assets/{urllib.parse.quote(asset['name'], safe='')}",
                      None, {"Content-Type": "application/octet-stream", "X-Content-SHA256": asset["sha256"],
                             "Content-Length": str(asset["size"])}, asset["name"]))
    steps.append(("POST", f"/releases/{quoted}/publish", {"latest": True} if channel == "stable" else {},
                  {}, None))
    return steps


def show_plan(client, steps):
    for method, path, body, headers, _ in steps:
        client.say(f"{method} {API_PATH}{path}")
        shown = dict(client.headers(masked=True), **headers)
        if body is not None:
            shown["Content-Type"] = "application/json"
        for name in sorted(shown):
            client.say(f"  {name}: {shown[name]}")
        if body is not None:
            client.say("  " + json.dumps(body, indent=2, sort_keys=True).replace("\n", "\n  "))


def same_declaration(draft, assets):
    declared = draft.get("expected_assets") if isinstance(draft, dict) else None
    if not isinstance(declared, list):
        return False
    strip = [{key: entry.get(key) for key in ("name", "platform", "arch", "os_floor", "kind", "size", "sha256")}
             for entry in declared if isinstance(entry, dict)]
    ours = [{key: entry.get(key) for key in ("name", "platform", "arch", "os_floor", "kind", "size", "sha256")}
            for entry in assets]
    key = lambda entry: entry["name"] or ""
    return sorted(strip, key=key) == sorted(ours, key=key)


def publish(client, directory, version, channel, notes, assets, unlisted=False, on_request=None):
    """Create (or resume) the draft, upload what it lacks, and publish.
    Returns the signed envelope, or None when an earlier attempt already
    published this version. Any failure leaves the draft unpublished; a
    manifest over the signer's limit fails before any draft exists, unless
    the feed's own measure refuses it at publish, which leaves the uploaded
    draft. The notes are cut to what fits first (preflight), the same on
    every rerun under the same listed limit.
    An unlisted candidate's version was taken as given and never checked
    against the feed: it is refused if the feed holds it other than as a
    draft, and a version_exists is never taken for a replay. That refusal
    comes first; the signer-limit pre-flight follows it.
    `on_request`, when given, is called with the exact bytes of the create
    (POST /releases) body before it is sent, on a rerun too: the pull model
    hands those bytes to the customer's operator (publish-request.json)."""
    if unlisted:
        unheld(client, version, channel)
    notes, cut = preflight(client, version, channel, notes, assets)
    steps = plan(version, channel, notes, assets)
    method, path, body, _, _ = steps[0]
    if on_request:
        on_request(request_bytes(body))
    response = client.call(method, path, body)
    if response.status == 422 and response.code == "manifest_too_large":
        refuse_too_large(client, "Creating the draft", response)
    if response.status == 409 and response.code == "version_exists":
        if unlisted:
            client.fail("Creating the draft", response, f": v{version} was taken as given when the candidate"
                        " was prepared and the feed already holds it; nothing is published")
        return replayed(client, response, version, channel, cut)
    if response.status not in (200, 201) or not isinstance(response.body, dict):
        client.fail("Creating the draft", response)
    draft = response.body
    if (draft.get("version") != version or draft.get("state") != "draft"
            or draft.get("origin") != "publish" or not same_declaration(draft, assets)):
        raise FeedError("the feed's draft does not declare exactly this release's assets")
    client.say(("Resumed" if response.status == 200 else "Created") + f" the draft of v{version}.")
    uploaded = {entry["name"] for entry in draft["expected_assets"] if entry.get("uploaded") is True}
    for method, path, _, headers, name in steps[1:-1]:
        if name in uploaded:
            client.say(f"{name}: already uploaded.")
            continue
        response = client.call(method, path, headers=headers, upload=directory / name)
        if response.status not in (200, 201) or not isinstance(response.body, dict):
            hint = {"asset_conflict": ": the draft holds other bytes under this name",
                    "asset_mismatch": ": the bytes or headers differ from the declared size and sha256",
                    "asset_not_declared": ": the draft does not declare this name",
                    "draft_not_found": ": the draft is gone",
                    "not_a_draft": ": the release is already published"}.get(response.code, "")
            client.fail(f"Uploading {name}", response, hint + "; the draft stays unpublished")
        entry = next(asset for asset in assets if asset["name"] == name)
        if (response.body.get("name") != name or response.body.get("sha256") != entry["sha256"]
                or response.body.get("size") != entry["size"]
                or not ASSET_ID.match(str(response.body.get("id")))):
            raise FeedError(f"the feed's receipt for {name} does not match it; the draft stays unpublished")
        client.say(f"{name}: uploaded ({'unchanged' if response.status == 200 else 'new'}).")
    method, path, body, _, _ = steps[-1]
    response = client.call(method, path, body)
    if response.status == 422 and response.code == "manifest_too_large":
        refuse_too_large(client, "Publishing", response, (version, len(assets)))
    if response.status == 422 and response.code == "assets_incomplete":
        missing = response.body.get("missing") or []
        mismatched = response.body.get("mismatched") or []
        client.fail("Publishing", response, f"; missing: {', '.join(map(str, missing)) or 'none'}"
                    f"; mismatched: {', '.join(map(str, mismatched)) or 'none'}; the draft stays unpublished")
    if response.status == 503 and response.code == "signer_unavailable":
        client.fail("Publishing", response, ": the signer is unavailable; the draft stays, rerun to publish it")
    if response.status != 200 or not isinstance(response.body, dict):
        client.fail("Publishing", response)
    return check_envelope(response.body, client.profile, version, channel, assets)


def replayed(client, response, version, channel, cut=""):
    """A 409 version_exists on create: a rerun after a publish whose reply
    was lost, or another release under this version. Done (None) only when
    the listing shows this version published by a publish, on this channel;
    a draft here declares other notes or assets, since an identical create
    resumes it (and cap_notes declares the same notes on every rerun). Only
    a 409 version_exists comes here: a 422, manifest_too_large included, is
    never a replay. The proposed contract lets the publisher token read versions
    and states only, never a published release's assets or a draft's notes,
    so they cannot be compared; `cut` (from preflight) says how this
    attempt's notes were cut, since a listed limit that changed since the
    draft was made changes the cut too."""
    entry = next((entry for entry in client.listing()["versions"] if entry.get("version") == version), None)
    if entry and (entry.get("state"), entry.get("channel"), entry.get("origin")) == ("published", channel, "publish"):
        client.say(f"v{version} is already published on the {channel} channel (at {entry.get('published_at')});"
                   " an earlier attempt's publish reply was lost. Its assets cannot be read with the"
                   " publisher token, so they are not compared; nothing is sent.")
        return None
    if entry and entry.get("state") == "draft":
        why = (f": a draft of v{version} declares other notes or assets; an identical rerun resumes a draft,"
               " so this one was made from other release notes or packages")
        if cut:
            why += (f". {cut[0].upper()}{cut[1:]}; if the listing's max_canonical_bytes changed since the draft"
                    " was made, the cut changed with it; so does a different spelling of the site (the link"
                    " names it) or a draft left by an older publisher, which cut the notes differently:"
                    " rerun once the listing reports the earlier limit, with the earlier site spelling,"
                    " or publish under a new version")
    elif entry:
        why = f": v{version} is already published as {entry.get('channel')} by {entry.get('origin')}"
    else:
        why = f": the feed listing does not show v{version}"
    client.fail("Creating the draft", response, why)


def unheld(client, version, channel):
    """Refuse an unlisted candidate whose version the feed already holds as
    anything but a draft. The listing shows no published release's assets or
    notes, so a published version cannot be told apart from this candidate's;
    a draft goes on to the create, which resumes only an identical one."""
    entry = next((entry for entry in client.listing()["versions"] if entry.get("version") == version), None)
    if entry and entry.get("state") != "draft":
        raise FeedError(f"profile {client.profile}: v{version} ({channel}) was taken as given when the candidate"
                        f" was prepared, and the feed already holds it ({entry.get('state')} on the"
                        f" {entry.get('channel')} channel by {entry.get('origin')}); nothing is published."
                        " Prepare a new candidate with another version.")


def check_envelope(envelope, profile, version, channel, assets):
    manifest = envelope.get("manifest")
    if not isinstance(manifest, dict) or not isinstance(envelope.get("signature"), str):
        raise FeedError("the published envelope has no manifest and signature")
    if (manifest.get("profile"), manifest.get("version"), manifest.get("channel")) != (profile, version, channel):
        raise FeedError("the published manifest names another profile, version or channel")
    published = manifest.get("assets")
    if not isinstance(published, list) or not same_declaration({"expected_assets": published}, assets):
        raise FeedError("the published manifest's assets differ from the declared ones")
    if not all(isinstance(asset, dict) and ASSET_ID.match(str(asset.get("id"))) for asset in published):
        raise FeedError("the published manifest has an asset without a valid id")
    return envelope

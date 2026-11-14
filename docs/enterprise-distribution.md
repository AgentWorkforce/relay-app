# Enterprise distribution: a private, workspace-authenticated release feed

A site profile's build today updates from a public GitHub releases
repository named by its `RELEASES_REPOSITORY` secret, and Desktop Release
refuses to publish a profile to a repository that is not public. This
document designs a second kind of release feed that removes that
requirement. A profile's feed becomes a build-time property, like its site:
either `github:<owner>/<name>` (today's behaviour) or `cloud`, a feed served
by the profile's own Cloud site to the people of the workspaces bound to it.
A `cloud` feed serves an Ed25519-signed manifest, and the installers and
updates it lists are downloaded from private storage through short-lived
signed URLs or an authorized download. The apps and `install.sh` verify the
manifest against a public key compiled into the build, then the sha256 of
every download, then the platform checks that exist today. What stays: the
default profile, the public beta and stable feeds, every existing `github:`
profile, the asset names, the macOS signing and notarisation, the
updater-bridge and floor rules, and the GitHub Environment `desktop-<id>` as
the container of a profile's secrets and approvals. Phases 2 to 4 of the
plan (publishing, the updaters and the headless installers) are implemented
against Cloud's proposed contract, which is not stable yet; the rest is not
implemented.

## Scope and constraints

- No customer releases repository is created, for this work or for any
  profile that moves to a `cloud` feed.
- This document changes nothing by itself: no merge, publish, deploy or
  policy change follows from it. Each phase below is its own change, with
  its own review round.
- No customer is named. A profile is named only by its opaque id;
  examples use `desktop-x7k2q` or `desktop-<id>`.
- Site and branding are fixed at build time (decided 2026-11-03). The feed
  is too: nothing at run time changes a build's site, feed, feed key or
  branding, and a build for another site or feed is another build.
- macOS and GTK change together (AGENTS.md, Rules). A feature one platform
  cannot have is reported as unavailable there, never hidden.
- Linux and Windows artifacts stay unsigned by their platforms. Windows
  Authenticode and Linux package signing are follow-ups, not part of this
  design.
- Where Cloud behaviour is required rather than observed, this document says
  "Cloud must"; those items are listed under
  [Cloud-side ownership](#cloud-side-ownership).

## Today

A profile is the GitHub Environment `desktop-<id>` on this repository. Its
secrets are listed in [workspace-layout.md](workspace-layout.md#site-profiles)
and [.github/README.md](../.github/README.md#site-profiles). The release feed
hangs off one of them, `RELEASES_REPOSITORY`.

| Concern | Today | Where |
|---|---|---|
| Profile validation | Requires the site, scheme, product name, bundle id and releases repository; refuses the default's repository, site, scheme and bundle id | `.github/actions/site-profile/action.yml:90-91`, `:114-117` |
| Defaults | `AgentWorkforce/relay-desktop-releases`, `AgentRelay`, `https://agentrelay.com`, `agentrelay`, `com.agentrelay.desktop` | `scripts/site-origin.sh:93-100` |
| Feed compiled into GTK | `RELEASES_REPOSITORY` and `DESKTOP_ASSET_PREFIX` as `env!` constants; GitHub API URL builders | `gtk/build.rs:29-66`, `gtk/src/build_profile.rs:11-20`, `:50-74` |
| Feed compiled into macOS | `Resources/releases-repository.txt`, `asset-prefix.txt`; a non-default site needs its own repository | `scripts/build-app.sh:315`, `:473-474`; `macos/Sources/RelayDesktop/AppInfo.swift:86-108` |
| Windows installer | `SupportUrl` defaults to the default releases repository's page | `windows/installer/AgentRelay.iss:61-62`, `:107` |
| Version allocation | The version job lists the releases repository's releases with `GITHUB_TOKEN`, so even a prepare-only run needs it public | `.github/workflows/desktop-release.yml` `version` job (from line 73) |
| Publish access | Mints a releases App token (or uses `RELEASES_TOKEN`); fails unless the repository is public and writable | `desktop-release.yml:1237-1268` (check at `:1261`); `desktop-promote.yml:173` |
| Publish | `softprops/action-gh-release` with an explicit file list, draft until every upload succeeds; promote uses `gh release create --draft` then `edit --draft=false` | `desktop-release.yml:1436-1521`; `desktop-promote.yml:191-201` |
| Candidates | `scripts/release-candidate.py` artifact, kept 7 days | `desktop-release.yml:1407-1432` |

Every updater reads the unauthenticated public GitHub Releases API:

- **macOS.** `AppUpdate.swift` builds `/releases?per_page=100&page=N` (beta)
  or `/releases/latest` (stable) (`macos/Sources/RelayDesktopCore/AppUpdate.swift:60`, `:64`),
  picks the highest-floored image the Mac can open (`:148-169`), and pins the
  image and `.sha256` URLs to `https://github.com` (`:140-143`). `AppUpdater`
  checks daily, retries after 15 minutes, and sends only `Accept` and
  `User-Agent` (`macos/Sources/RelayDesktop/AppUpdater.swift:54`, `:56`, `:191-194`).
  `AppUpdateInstaller` mounts the image, checks the bundle, verifies the code
  signature against team `QUJ7SA6X8X` and runs `spctl --assess`
  (`macos/Sources/RelayDesktop/AppUpdateInstaller.swift:54-67`, `:77`).
- **GTK (Linux and Windows).** `gtk/src/updates.rs` uses `ureq` with a
  30-second timeout and a 512 MiB cap (`:47`, `:49`), recognises GitHub
  hosts (`on_github`, `:398`) and checks only the sha256
  (`verified_download`, `:664`). Linux installs a tarball and rewrites the
  user unit (`install_tarball`, `:806`) or hands a `.deb` to apt
  (`download_deb`, `:891`; `gtk/src/ui/update_actions.rs:168`). Windows
  unpacks a zip and repoints the logon task (`gtk/src/updates/windows.rs:207`).
  Asset names are parsed in `gtk/src/updates/channels.rs`.
- **Headless.** `linux/install.sh` carries `PRODUCT_NAME`,
  `RELEASES_REPOSITORY` and `ASSET_PREFIX` once (`:49-51`), substituted per
  profile by the publish job (`desktop-release.yml:1349-1364`). The API and
  download base can be overridden by environment (`:57-58`). It fetches with
  `curl --proto '=https'` and no credentials (`:122`) and checks the sha256
  (`:329-332`). `windows/install.ps1` hardcodes the default repository
  (`:70-71`) and is not substituted per profile. `macos/install.sh`,
  published as `install-macos.sh`, carries the same seven values as
  `linux/install.sh` and is substituted per profile the same way; it reads
  a releases repository only and refuses a `cloud` feed.

What is unsigned: Linux and Windows artifacts (`linux/install.sh:20-22`,
`docs/windows-app.md:207-208`). No release carries a signed checksum list or
a detached signature. A `.sha256` proves the download is the published file,
not who published it.

The Cloud side the design builds on already exists in `desktop-core`: the
account's site is the built-in one (`desktop-core/src/account.rs:140-143`,
`:224`); sign-in is browser PKCE S256 (`desktop-core/src/browser_login.rs:136-143`);
the session is refreshed single-flight; `account.mcp_headers` hands the apps
an `Authorization: Bearer` and `X-Agent-Relay-Desktop: 1`
(`desktop-core/src/mcp_headers.rs:19`, `:39`). A build's own site is trusted
even on private addresses (`docs/relay-core.md:18-28`).

## Design

### The feed is a build-time property of the profile

A new profile secret, `RELEASE_FEED`, takes one of two forms:

| Value | Feed | Needs |
|---|---|---|
| `github:<owner>/<name>` | Today's GitHub Releases feed | `RELEASES_REPOSITORY` equal to `<owner>/<name>`, public, as today |
| `cloud` | `{site}/cloud/api/v1/desktop/feed/...` on the profile's own site | `DESKTOP_FEED_PUBLIC_KEY`, `DESKTOP_FEED_PUBLISHER_TOKEN`, a non-default site; no `RELEASES_REPOSITORY` |

Unset means `github:` with `RELEASES_REPOSITORY`, so every existing profile
keeps building as it does. The default profile is always
`github:AgentWorkforce/relay-desktop-releases`.

The feed is compiled in next to the site, with the feed public key:

| Where | Today | Adds |
|---|---|---|
| `scripts/site-origin.sh` | `DEFAULT_*` values | `DEFAULT_RELEASE_FEED`, a `release_feed` validator |
| `gtk/build.rs`, `gtk/src/build_profile.rs` | `RELEASES_REPOSITORY`, `DESKTOP_ASSET_PREFIX` | `RELEASE_FEED`, `DESKTOP_FEED_PUBLIC_KEY`, `DESKTOP_PROFILE_ID`; `RELEASES_REPOSITORY` empty for `cloud` |
| `scripts/build-app.sh` | `Resources/releases-repository.txt` | `Resources/release-feed.txt`, `Resources/feed-public-key.txt`; `--release-feed`, `--feed-public-key` |
| `scripts/build-gtk.sh`, `scripts/build-windows.sh` | `--releases-repository` | `--release-feed`, `--feed-public-key` |
| `windows/installer/AgentRelay.iss` | `SupportUrl` defaults to the releases page | for `cloud`, `SupportUrl` defaults to `{site}/cloud/dashboard/downloads` |
| `linux/install.sh`, `windows/install.ps1` | substituted repository (Linux only) | substituted `RELEASE_FEED`, `SITE_URL`, `PROFILE_ID`, `FEED_PUBLIC_KEY` on both |
| `macos/install.sh` (published as `install-macos.sh`) | substituted, all seven values | a `cloud` feed path; until then a `cloud` copy refuses to install |

No app code reads a feed from the environment. The `AGENT_RELAY_RELEASES_API`
and `AGENT_RELAY_DOWNLOAD_BASE` overrides of the install scripts stay for a
`github:` feed and are refused for `cloud`.

### Signed manifest

Feed authenticity does not depend on transport or storage. Cloud signs each
release's manifest at publish time with the profile's Ed25519 feed signing
key, held in Cloud's KMS. The public half is the profile secret
`DESKTOP_FEED_PUBLIC_KEY`, compiled into the build. An updater trusts no
name, size, sha256 or URL until the signature verifies.

The signed document:

```json
{
  "schema": 1,
  "profile": "x7k2q",
  "channel": "stable",
  "version": "2026.11.9",
  "published_at": "2026-11-20T10:04:31Z",
  "notes": "- Fixes the tray menu on Windows arm64.\n",
  "download_hosts": ["storage.googleapis.com"],
  "assets": [
    {
      "id": "a_01JD3Q4W6Y",
      "name": "Desktop-x7k2q-2026.11.9-macOS15-arm64.dmg",
      "platform": "macOS",
      "arch": "arm64",
      "os_floor": 15,
      "kind": "image",
      "size": 187236352,
      "sha256": "9f2c...64 hex..."
    }
  ]
}
```

The envelope served by Cloud:

```json
{
  "manifest": { "...": "the signed document above" },
  "signature": "<base64url, 64 bytes>",
  "key_id": "fk_x7k2q_2026a"
}
```

| Field | Type | Meaning |
|---|---|---|
| `schema` | integer | `1`. A client refuses a schema it does not know |
| `profile` | string | The opaque id. Must equal the build's compiled id |
| `channel` | `stable` \| `beta` | Must equal the channel asked for |
| `version` | string | `YYYY.M.N` or `YYYY.M.N-beta.N`; a beta channel version must carry `beta.N`, a stable one must not (the rule in `AppUpdate.swift:195-202`) |
| `published_at` | RFC 3339 UTC | Set by Cloud at publish |
| `notes` | string | Markdown release notes from `release-notes.py` |
| `download_hosts` | array of DNS names | The only hosts a signed URL may point at. Empty means streamed downloads only |
| `assets[].id` | string | Opaque; used by the download endpoints |
| `assets[].name` | string | Today's asset name, `<prefix>-<version>-<platform>[floor]-<arch>.<ext>` |
| `assets[].platform` | `macOS` \| `Linux` \| `Windows` | As in today's names |
| `assets[].arch` | `arm64` \| `x64` | As in today's names |
| `assets[].os_floor` | integer, optional | macOS major floor; absent on the unfloored updater-bridge name |
| `assets[].kind` | string | `image`, `tarball`, `zip`, `deb`, `setup`, `probe`, `install-script` |
| `assets[].size` | integer | Bytes; a download larger than this is cut off |
| `assets[].sha256` | hex string | Lower-case, 64 characters |

The signature is Ed25519 (RFC 8032) over the UTF-8 bytes of the `manifest`
object canonicalised with the JSON Canonicalization Scheme (RFC 8785). The
client canonicalises what it received, not a re-serialisation of parsed
types. `key_id` selects the public key; a build carries one key and refuses
any other `key_id`. A rotation is a new build: the last release signed with
the old key carries a build that knows the new one
([open question 3](#open-questions)).

Client order of checks, every platform:

1. Signature over the canonical `manifest` with the compiled key.
2. `profile`, `channel`, `schema`; `version` newer than the installed one
   (no downgrade; `isNewer` in `AppUpdate.swift`).
3. Select the asset by `platform`, `arch`, `kind`, `os_floor`; its `name`
   must match the expected pattern for the build's prefix and the version.
4. Download through the API below; the URL host must be in `download_hosts`
   or be the site itself (streamed).
5. `size` and `sha256` of the bytes.
6. The platform checks that exist today (macOS bundle name and id, codesign
   team `QUJ7SA6X8X`, `spctl`; GTK archive validation).

### Cloud feed API

All paths are under the profile's site API base, `{site}/cloud/api/v1`, the
same base `desktop-core` uses (`api_base`, imported at
`desktop-core/src/cloud_api.rs:5-7`). Cloud must implement them; the
contract is an OpenAPI document in relay-control, and this repository keeps a
copy of the manifest schema under `docs/specs/` once agreed.

**Read (apps, install scripts, downloads page).** Every request carries
`X-Agent-Relay-Profile: <id>`, the compiled id.

| Path | Method | Auth | Request | Response | Errors |
|---|---|---|---|---|---|
| `/desktop/feed/manifest` | GET | workspace bearer, or install token | `?channel=stable\|beta` | `200` envelope of the current release | `401` bearer missing or expired; `403` workspace not bound to the profile, or profile unknown; `404` channel has no release yet; `429` |
| `/desktop/feed/releases` | GET | as above | `?channel=beta&page=N` | `200` `{releases: [envelope], next_page}` newest first | as above |
| `/desktop/feed/assets/{asset_id}/download-url` | POST | as above | none | `200` `{url, expires_at}`; `url` is a V4 signed GET for one object, valid 5 minutes | `401`; `403` as above or asset not in the profile; `501` no object storage (use the stream) |
| `/desktop/feed/assets/{asset_id}` | GET | as above | `Range` allowed | `200`/`206` the bytes | as above |

**403, never 404.** A profile that exists but is not bound to the caller's
workspace, and a profile id Cloud does not know, both answer `403` with
`{"error": "not_entitled"}`. The app then says "This workspace is not
entitled to updates for <product>" instead of reporting a missing feed, and
a caller cannot probe which profile ids exist. `404` is kept for a channel
of an entitled profile that has no release yet.

A signed URL is checked only when the request starts, so a long download
finishes after it expires. A resumed download asks for a new URL.

### Downloads page and install token

`{site}/cloud/dashboard/downloads`, behind the browser session, lists the
installers of the current release of the workspace's profile per platform
and channel. A click issues the same signed URL. A first install therefore
needs a browser sign-in and no app. The page shows each file's sha256.

The page also mints an **install token** for headless installs: scope feed
read for that profile only, lifetime 1 hour, revocable from the same page.
It is accepted by the read endpoints only. `install.sh` (and `install.ps1`)
is an asset of kind `install-script` of the current release, listed on the
page like an installer. It is the one file fetched before a signature can be
checked, so its trust is the browser session's TLS to the built-in site and
the sha256 the page shows. The page shows the command:

```sh
AGENT_RELAY_INSTALL_TOKEN=<token> bash install.sh
```

### Publisher API and credential

| Path | Method | Request | Response | Errors |
|---|---|---|---|---|
| `/desktop/feed/releases` | POST | `{version, channel, notes}` | `201` draft | `409` version exists; `422` version or channel invalid |
| `/desktop/feed/releases/{version}/assets/{name}` | PUT | bytes; headers `X-Content-SHA256`, `Content-Length` | `201` `{id}` | `409` name exists with other bytes; `422` sha256 or size mismatch |
| `/desktop/feed/releases/{version}/publish` | POST | `{latest: true}` for stable | `200` envelope (Cloud signs here) | `409` not a draft; `422` an expected asset is missing |
| `/desktop/feed/releases` | GET | `?channel=&include_drafts=true` | `200` list, drafts included | `401`, `403` |

Auth: `Authorization: Bearer <DESKTOP_FEED_PUBLISHER_TOKEN>` and
`X-Agent-Relay-Profile`. The publisher token is per profile, write-only on
that profile's feed plus the version listing, can read nothing else, is
rotatable, and is never a workspace user's token. It is a `desktop-<id>`
environment secret. It replaces the releases App token, `RELEASES_TOKEN` and
the `.private == false` check for `cloud` profiles. A draft is invisible to
the read API; publishing is the only step that signs, and it fails unless
every asset the draft declared has been uploaded.

### Workspace isolation

Cloud must keep a profile registry:

| Field | Meaning |
|---|---|
| `profile_id` | The opaque id, e.g. `x7k2q` |
| `workspace_ids` | Workspaces whose members may read the feed |
| `feed_key_id` | The current KMS key id; the signer for this profile only |
| `publisher_token_hash` | Hash of the current publisher token(s) |
| `storage_prefix` | The object prefix, below |

Storage layout, in a private bucket with no public access and no listing
for any credential the feed hands out:

```text
gs://<feed-bucket>/profiles/<profile_id>/releases/<version>/<asset name>
gs://<feed-bucket>/profiles/<profile_id>/releases/<version>/manifest.json
```

A signed URL names one object, allows `GET` only and expires after
5 minutes. Cloud must write an audit record for every manifest read, list
read, URL issue and stream (workspace, user or install token, profile,
asset, time, client address). A build only ever asks for its compiled
profile, and Cloud only answers members of bound workspaces, so one
profile's build never sees another's release, and the public GitHub feed
never serves a profile's build.

### Updater auth and the signed-out state

The bearer is the `desktop-core` account session, refreshed single-flight as
today. A new RPC, `account.feed_headers`, mirrors `account.mcp_headers`
(`desktop-core/src/mcp_headers.rs`) and returns:

```json
{"Authorization": "Bearer <access token>", "X-Agent-Relay-Profile": "x7k2q", "X-Agent-Relay-Desktop": "1"}
```

Both apps call it before each feed request. A `401` triggers one refresh
and one retry. `CLOUD_API_ACCESS_TOKEN` keeps winning in `bearer()` as for
every Cloud call.

| State | macOS and GTK update UI |
|---|---|
| Signed out, or refresh failed | "Sign in to <product> to receive updates." The check is unavailable, not hidden; no other feed is tried |
| `403 not_entitled` | "This workspace is not entitled to updates for <product>." |
| Signature or schema invalid | "The update could not be verified." Nothing is downloaded; the next scheduled check tries again |
| Network error | As today (retry after 15 minutes on macOS) |

### macOS client

- `macos/Sources/RelayDesktopCore/AppUpdate.swift`: a `ReleaseFeed`
  protocol with `GitHubReleasesFeed` (today's code, unchanged) and
  `CloudFeed` (manifest and list endpoints, Ed25519 with CryptoKit
  `Curve25519.Signing.PublicKey`, RFC 8785 canonicaliser, asset selection by
  fields, `download_hosts` replacing the `github.com` pin at `:140-143`).
- `macos/Sources/RelayDesktop/AppInfo.swift`: reads `release-feed.txt` and
  `feed-public-key.txt`; a `cloud` build with no key fails closed.
- `macos/Sources/RelayDesktop/AppUpdater.swift`: picks the feed from
  `AppInfo`; adds the headers from `account.feed_headers`; the download
  checks size and the manifest sha256 instead of a `.sha256` asset; the
  signed-out and not-entitled states.
- `AppUpdateInstaller.swift`: unchanged.

### GTK client (Linux and Windows)

- `gtk/src/updates.rs` and `gtk/src/updates/channels.rs`: a `ReleaseFeed`
  enum with `GitHub` (today) and `Cloud`; Ed25519 with `ed25519-dalek`, a
  JCS canonicaliser; `verified_download` takes the expected size and sha256
  from the manifest; the host rule from `download_hosts` replaces
  `on_github` for `Cloud`. The 512 MiB cap and 30-second timeout stay.
- `gtk/src/build_profile.rs`: `RELEASE_FEED`, the key and the profile id.
  `releases_page()` becomes `{site}/cloud/dashboard/downloads` for `cloud`,
  so `InstallKind::Source` opens the downloads page.
- `gtk/src/ui/update_actions.rs`: the `.deb` is downloaded through the feed
  and the copied command installs that local file, as today; the
  signed-out and not-entitled states, worded as on macOS.
- `gtk/src/updates/windows.rs`: unchanged apart from the download source.
  There is no Authenticode check today, and none is added here.

### Headless

Phase 4 implements this against the proposed Cloud contract; the routes
and result codes below are the draft's and may change.

- `linux/install.sh` and `windows/install.ps1` read seven built-in values
  (`PRODUCT_NAME`, `RELEASES_REPOSITORY`, `ASSET_PREFIX`, `RELEASE_FEED`,
  `SITE_URL`, `PROFILE_ID`, `FEED_PUBLIC_KEY`; in PowerShell `$ProductName`
  and so on). The repository's copies are the default product's
  (`RELEASE_FEED='github'`); Desktop Release stages a profile's copies with
  its own values (see [Publish pipeline](#publish-pipeline)).
- `macos/install.sh`, published as `install-macos.sh`, takes the same seven
  values but has no `cloud` path yet: a `cloud` copy refuses to install
  ("the macOS installer does not support the cloud feed yet"; `--status`
  and `--uninstall` still work), and the feed does not carry it, since the
  feed has no macOS probe bundle for it to install.
- For a `cloud` profile, either script refuses `AGENT_RELAY_RELEASES_API`
  and `AGENT_RELAY_DOWNLOAD_BASE`, then reads the proposed
  `GET {site}/cloud/api/v1/desktop/feed/manifest?channel=stable` with
  `Authorization: Bearer` and `X-Agent-Relay-Profile`. The credential is
  `AGENT_RELAY_INSTALL_TOKEN` when it is set (required on a first install),
  or else the installed core's `agent-relay-probe feed-headers` for the
  signed-in account (an update). It is never printed, logged or written to
  a file, and never sent to the signed download URL.
- The envelope is read with a strict JSON reader (a repeated key and a body
  over 1 MiB are refused; `install.ps1` also refuses nesting deeper than 32), its `key_id` must name the
  built-in profile, and its manifest's JCS bytes are checked against
  `FEED_PUBLIC_KEY`. The asset is the version's `Linux-<arch>-probe.tar.gz`
  or `Windows-<arch>-probe.zip`. It is fetched through the proposed
  `POST .../assets/<id>/download-url`, from a host the manifest's
  `download_hosts` lists (or the site's own), with the proposed
  `GET .../assets/<id>` stream as the fallback on 501. Its size and sha256
  must match the manifest before anything is unpacked; the unpack, health
  check, rollback and three kept versions are today's.
- Refusals name the draft's codes: 401 (sign in again or a new token), 403
  `not_entitled`, 404 `no_release`, 429, `key_id_refused`,
  `signature_invalid`, and a manifest that is not newer than the installed
  version ends the run as up to date.
- Signature check, in this order: an installed core that knows `feed verify`
  (`agent-relay-probe feed verify`, which shares the GTK verifier) checks
  every update. A first install has no trusted verifier yet (the probe
  comes from the archive the manifest vouches for). Its trust is TLS to the
  built-in site, the install token and the sha256 shown on the downloads
  page, and the script verifies the signature with OpenSSL 3
  (`openssl pkeyutl -verify -rawin`, the one on `PATH`). Without it,
  both scripts refuse and ask for OpenSSL 3. `install.ps1` skips the check
  only when told to (`-AllowUnverifiedFeed`, or
  `AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1`), and then says so loudly (see
  Open question 1). LibreSSL and OpenSSL 1.x are not used.
- `linux/install.sh` needs `curl`, `python3` and, on a first install,
  OpenSSL 3 for a `cloud` profile; the
  `github` path needs neither beyond what it needed before.
- Updating a server stays "run the command again", with a fresh install
  token or a signed-in core. An automatic update timer is a follow-up.

### Publish pipeline

`desktop-release.yml` and `desktop-promote.yml` switch on the feed. For a
`github:` feed nothing changes.

| Job or file | `cloud` behaviour |
|---|---|
| `version` job | Lists versions from `GET /desktop/feed/releases?include_drafts=true` with the publisher token instead of the public repository; prepare-only needs no public repository |
| `publish` job | Skips the App token, `RELEASES_TOKEN` and the public-repository check (`:1237-1268`); stages the same asset set (`macos-release-assets.py`, probe parity, the substituted install scripts); creates the draft, uploads every versioned package, `install.sh` and `install.ps1` (not the unversioned aliases or `.sha256` files, which the feed's name binding and signed manifest replace), publishes; a failed upload leaves the draft unpublished |
| `desktop-promote.yml` | Same producer and candidate verification (`:110-156`); replaces `gh release create`/`edit` (`:185-201`) with the publisher calls; beta publishes without `latest` |
| `scripts/release-candidate.py` | A `cloud` publisher next to the GitHub one; the manifest records `"feed": "cloud"` |
| `scripts/release-notes.py` | `--feed cloud` allocates against the feed listing, as `--repository` does today |
| `changelog` job | Unchanged: default profile only |

The signer caps the canonical manifest at `limits.max_canonical_bytes` (4096
for an AWS KMS Ed25519 key, and assumed when the listing gives null or
nothing; null is never read as uncapped). Every
cloud profile therefore publishes one fixed set of 16 assets (the floored
macOS dmg, the Linux tarball and deb and the Windows Setup.exe and zip, each
for arm64 and x64, the four Linux and Windows probe bundles the headless
installers select, and both install scripts; no macOS probes and no unfloored
image). The release notes are cut on whole lines to up to 300 canonical
bytes, less when the asset list leaves less room under 4096, and cut notes end
with a link to `<site>/cloud/dashboard/downloads#v<version>`, inside the
budget. The publisher measures the manifest
before it creates a draft, at the longest fields: for a 10-character profile
the 16 assets with empty notes measure 3854 bytes at 2026.11.1-beta.1 and
3884 at 2026.12.99-beta.99, leaving the notes 242 and 212 bytes. It exits 2,
leaving nothing behind, when even empty notes would exceed the cap. A 422
`manifest_too_large` from the feed also exits 2 and is never retried: at
create nothing is left, and at publish the draft stays with its assets
uploaded and the message names it (a rerun with the same inputs resumes it). See
[Publishing to a cloud feed](workspace-layout.md#the-signers-manifest-limit).

The site-profile action validates:

| Secret | `github:` feed | `cloud` feed |
|---|---|---|
| `AGENT_RELAY_SITE_URL`, `AGENT_RELAY_URL_SCHEME`, `DESKTOP_PRODUCT_NAME`, `DESKTOP_BUNDLE_ID` | required; not the default's | required; not the default's |
| `RELEASE_FEED` | optional; must name `RELEASES_REPOSITORY` | `cloud` |
| `RELEASES_REPOSITORY` | required; not the default's | must be unset |
| `DESKTOP_RELEASES_APP_ID`/`_PRIVATE_KEY` or `RELEASES_TOKEN` | to publish | must be unset |
| `DESKTOP_RELEASES_USE_ORG_PUSHER` | optional; `true` (compared case-insensitively, as GitHub expressions do) lets Desktop Promote use the organisation's pusher App (`GH_APP_PUSHER_ID`/`_PRIVATE_KEY`, installed on the releases repository) when the profile's own pair is unset | optional; the same, for `PULL_RELEASES_REPOSITORY` |
| `DESKTOP_FEED_PUBLIC_KEY` | must be unset | required; a 32-byte Ed25519 public key, base64 |
| `DESKTOP_FEED_PUBLISHER_TOKEN` | must be unset | required to publish or prepare |
| `DESKTOP_SUPPORT_URL`, branding | as today | as today |
| `DESKTOP_POSTHOG_API_KEY` | not read: a profile build reports only to its site's analytics gateway, and the core's build refuses a PostHog host or key | not read |
| `DESKTOP_POLICY_DEFAULTS` | optional; a JSON object with any of `sharing_mode`, `upload_control`, `oops_window_seconds`, `user_retroactive_delete`, applied from first launch and offline until the site's `access/me` policy is read, always overridden by the fetched policy; unset keeps uploads withheld until the policy is read | same |
| `DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS` | optional; a JSON array of `https` origins of the storage hosts direct uploads may PUT to even when they resolve to private addresses; site builds only; matched exactly by scheme, host and port; storage PUTs only | same |

A `desktop-<id>-bridge` environment is the one exception: it holds only
`RELEASES_REPOSITORY` (never the default product's) and
`DESKTOP_BRIDGE_APP_ID`/`_PRIVATE_KEY` (no `RELEASES_TOKEN` fallback), is
read only by Desktop Promote's `bridge-environment`
path (migration, below), and never builds, so the site-profile action never
sees it.

The action exports `RELEASE_FEED` and `FEED_PUBLIC_KEY`, masks the token and
never prints it.

### Customer-hosted and on-prem

A customer-hosted site is its own relay-control deployment, so the feed lives
there, backed by that deployment's object store (GCS, S3 or MinIO) or the
streamed download. The signing key never leaves the hosted KMS: an on-prem
Cloud and any mirror only store and serve signed manifests verbatim.

| Path | How a release gets there | Who holds what |
|---|---|---|
| (a) Direct | Our publish job publishes to the hosted feed (which signs), then uploads the same assets and the signed envelope verbatim to the customer's site with a publisher token the customer issues; that site's publish stores, never signs | Customer: publisher token, storage. Us: signing key |
| (b) Mirror | We publish to the hosted feed under the profile; the on-prem Cloud runs `relayctl desktop-feed sync` with a mirror token | Customer: mirror token, storage. Us: signing key, publisher token |
| (c) Air-gapped | `relayctl desktop-feed export` writes a bundle (envelope and assets); it is carried in; `relayctl desktop-feed import` loads it | Customer: the bundle, storage. Us: signing key |

Only path (b) needs the customer's Cloud to reach ours (outbound, to
sync); (a) and (c) need no connection from it. In every path
the compiled public key verifies the manifest, the built-in site may resolve
to private addresses as today, and download hosts come only from the signed
`download_hosts`.

### Pull model

The pull model is an operator-run publish of an attested bundle: the
customer deployment's operator takes a release from a GitHub repository and
publishes it to the deployment's own feed with its publisher token, and that
deployment's Control signs the manifest with its own KMS key at publish.
Nothing reaches the customer deployment from ours. The contract is
relay-control main b503eaad (docs/desktop-feed.md, openapi.yaml).

**The bundle.** When the profile's environment sets
`PULL_RELEASES_REPOSITORY`, Desktop Promote creates the release `v<version>`
in that repository after the cloud-feed publish (a prerelease for a beta):

| Asset | What it is |
|---|---|
| `pull-bundle.json` | `{"profile", "version", "channel"}`: the profile, version and channel the bundle was promoted for |
| `publish-request.json` | The exact body of `POST /releases`, written by `desktop-feed-publish.py --request-out` (notes already fitted, `expected_assets` with each name, sha256 and size); written on a rerun too |
| every asset in `cloud-assets.txt` | The profile's 16 packages and install scripts |
| `release-notes.md` | The candidate's release notes |
| `agent-relay-desktop-update.sh` | The operator script |
| `SHA256SUMS` | sha256 of every file above, bare names |
| `SHA256SUMS.sigstore.json` | The `actions/attest-build-provenance` bundle for `SHA256SUMS`, signed by the promote job's workflow identity |

**Verification order.** `scripts/agent-relay-desktop-update.sh --profile
<id> <version>` (needs `gh`, `curl`, `jq`, `sha256sum`;
`AGENT_RELAY_PULL_REPOSITORY`, `AGENT_RELAY_SITE`) refuses to run without
`--profile` (exit 1; no environment variable stands in for it) and refuses at
the first failure after that, exit 2:

1. `gh release download v<version>` into a fresh directory (`--dir` to choose
   an empty one).
2. `gh attestation verify SHA256SUMS --bundle SHA256SUMS.sigstore.json
   --owner AgentWorkforce --cert-identity
   https://github.com/AgentWorkforce/relay-app/.github/workflows/desktop-promote.yml@refs/heads/main
   --source-ref refs/heads/main`: signed by Desktop Promote as it is on
   `main`, in a run of `main`, never by the workflow file of another branch
   or tag (`AGENT_RELAY_SIGNER_WORKFLOW` overrides the workflow; the ref stays
   `refs/heads/main`).
3. `sha256sum --check --strict` of `SHA256SUMS`, every line a bare name, every
   listed file present, `pull-bundle.json` and `publish-request.json` listed.
   Each file is hashed from standard input, so the download directory's name
   never reaches `sha256sum`'s output.
4. The profile: `pull-bundle.json` names the `--profile` given, the version
   asked for and its channel. A bundle attested for another profile stops
   here, before any publisher call.
5. The declaration: `publish-request.json` declares the version asked for and
   the channel that version implies, every declared asset is `install.sh`,
   `install.ps1` or one of the profile's packages
   (`Desktop-<profile>-<version>-` and a cloud package suffix), and each is
   listed in `SHA256SUMS` with the declared sha256 and size.
6. The plan: each request it would send. Without `--apply` it stops here and
   sends nothing.

**The three publisher calls**, with `--apply` only, to
`<site>/cloud/api/v1/desktop/feed` (`https` only), each with
`Authorization: Bearer` (`DESKTOP_FEED_PUBLISHER_TOKEN` or `--token-stdin`,
never an argument, never printed) and `X-Agent-Relay-Profile`:

| Call | Body | Outcome |
|---|---|---|
| `POST /releases` | `publish-request.json`, byte for byte | 201 created or 200 resumed (an identical body) continue; 409 `version_exists`, 401, 403, 422 and anything else stop with the feed's `error` |
| `PUT /releases/<version>/assets/<name>`, each declared asset | the file, with `X-Content-SHA256` | 200 or 201 continue; 409 `asset_conflict`, 409 `not_a_draft`, 422 `asset_mismatch` stop |
| `POST /releases/<version>/publish` | `{"latest":true}` for a stable, `{}` for a beta | prints the version and the `key_id` the deployment signed with |

Every request is bounded: 30 s to connect, then stopped when it moves under
1 KiB/s for 120 s or runs over an hour in all. A timed-out or failed request
exits 2 with that said and nothing further sent. A run stopped after the
create leaves the draft; running the same command again with the same bundle
resumes it.

**Rerunning Desktop Promote.** The cloud-feed publish of an unlisted
candidate's version happens once: the feed refuses a second create of that
version. So that a failure after it (the attestation, the pull release) can
be retried, the promote job keeps the workflow artifact
`pull-receipt-<profile>-v<version>` right after the cloud publish (90 days):
the create body sent and a receipt binding it to the profile, version,
channel, candidate run id, source commit and promote run. A later Desktop
Promote run of the same version first looks for that artifact, trusting only
one kept by Desktop Promote dispatched on `main` of this repository:

- none: a first publication, and the feed's collision check applies;
- the same candidate's: the run checks the kept body against the candidate's
  assets, skips the cloud publish and goes on to the pull bundle with that
  body;
- another candidate's: the run stops before the feed with "v<version> was
  already published to the cloud feed by Desktop Promote run <run> from
  candidate run <run>"; promote that candidate under a new version.

**What was signed.** A cloud publish prints the envelope's `key_id`, the
SHA-256 of the signed manifest's canonical (RFC 8785) bytes, which are what
the Control signed and the installed app verifies, and the base64 signature;
the job also keeps the signed envelope as the workflow artifact
`feed-envelope-<profile>-v<version>` (90 days; not on a replay of a lost
publish reply or a run that resumed from a pull receipt). For a cloud feed
the site-profile action prints a notice, `Feed public key SHA-256
fingerprint: <hex>`: the SHA-256 of the decoded 32-byte Ed25519 key the
build or promote was configured with, never the key itself. An observer compares these with what the installed app verifies.

### Compatibility and migration

- The default profile and every `github:` profile are unchanged: same
  feed, same names, same checks, same jobs.
- Changing a profile's feed is a new build. An installed `github:`-fed
  build of a profile moves by one final GitHub release whose assets are the
  `cloud`-fed build, signed and notarised as usual. Because a `cloud`
  profile may not hold GitHub secrets, that release is published by
  Desktop Promote's `bridge-environment` input from a `desktop-<id>-bridge`
  environment that holds only `RELEASES_REPOSITORY` and the bridge's own
  App secrets (table above); the promote job refuses a bridge environment that
  holds anything else, and nothing is ever built from it. After the
  bridge release the repository can be archived. New installs come from
  the downloads page.
- The updater-bridge name, macOS floors, the `beta.N` rule and the stable
  and beta channels are manifest fields, so release notes, candidates and
  promote keep their rules.

### Threat model

| Threat | Control |
|---|---|
| Stolen signed URL | One object, `GET` only, 5 minutes; bytes still checked against the signed sha256; every issue audited |
| Compromised storage or on-prem mirror | Holds no signing key; a changed asset fails the sha256, a changed manifest fails the signature |
| Cross-workspace access | Build sends its compiled profile; Cloud answers bound workspaces only; `403` for everything else; per-profile prefixes and keys |
| Stolen publisher token | Write-only on one profile's drafts; cannot read; publish is audited and alerts; revocable; the macOS signature check still applies |
| Downgrade or rollback | Client refuses a version not newer than installed; old envelopes cannot be relabelled (`version`, `channel`, `profile` are signed) |
| Stale manifest replayed (freeze) | Not prevented by publish-time signing; bounded by TLS to the built-in site and the audit log ([open question 4](#open-questions)) |
| Signed-out device | No feed access; updates shown as unavailable; no fallback feed |
| Revoked user | Refresh fails, so the device is signed out; outstanding signed URLs expire in 5 minutes; install tokens revoked with the user |
| Compromised feed signing key | KMS only, per profile; on macOS codesign and `spctl` still hold; on Linux and Windows the manifest is the only floor, so rotation is a new build pushed with the old key |

**Security floor for Linux and Windows.** Without platform signatures, the
floor is the Ed25519 manifest signature plus the sha256 and size of each
download, checked before anything is unpacked or run. This is stronger than
today's `.sha256` from the same host, but a stolen signing key is enough to
ship code there. Authenticode and Linux package signing remain follow-ups.

## Implementation plan

Each relay-desktop phase is one reviewable change on its own
`review/<name>-<slug>` branch with its own review round. No phase publishes a release.

| Phase | Repo / owner | Changes | Tests and proof | Depends on |
|---|---|---|---|---|
| 0 | relay-control / Cloud team | Profile registry and binding; private bucket and prefixes; KMS Ed25519 keys; publisher API; manifest, list, download-url and stream endpoints; downloads page; install tokens; audit; rate limits; revocation; OpenAPI contract | self-hosted end-to-end fixture serving a feed for a test profile; manifest schema copied to `docs/specs/` here | none |
| 1 | relay-desktop | `RELEASE_FEED` and `DESKTOP_FEED_PUBLIC_KEY` in `site-origin.sh`, the site-profile action, `build-app.sh`, `build-gtk.sh`, `build-windows.sh`, `gtk/build.rs`, `build_profile.rs`, `AppInfo.swift`, `AgentRelay.iss`; docs tables | Probe-script lanes, `build-app.sh` option tests, `test_release_workflow.py`; default build byte-identical in its feed records | none (parallel with 0) |
| 2 | relay-desktop | `cloud` backend in `desktop-release.yml` version and publish jobs, `desktop-promote.yml` with the `bridge-environment` input, `release-candidate.py`, `release-notes.py --feed` | `test_release_candidate.py`, `test_release_workflow.py`, `test_release_notes.py`; a prepare-only run against the staging feed | 0 (publisher API), 1 |
| 3 | relay-desktop | macOS and GTK together: `ReleaseFeed`, `CloudFeed`, verifier, `account.feed_headers` in `desktop-core`, signed-out and not-entitled states, `download_hosts` rule | `swift test`, GTK suite, `desktop-core` tests against a fixture feed with good, tampered, wrong-profile, wrong-key and downgrade manifests | 0 (read API), 1 |
| 4 | relay-desktop | `install.sh` and `install.ps1` `cloud` path, install token, `agent-relay-probe feed verify`, per-profile `install.ps1` substitution and publication (implemented against the proposed contract) | `scripts/test-linux-install-cloud.sh`, `scripts/test-windows-install-cloud.ps1` (fixture feed; refused overrides; tampered asset; wrong key id; signature refused; not newer), `desktop_binary` `feed verify` test, `test_release_workflow_cloud.py` staging | 0, 2, 3 (verifier) |
| 5 | relay-control, then relay-desktop docs | `relayctl desktop-feed sync`, `export`, `import`; docs for the three publish paths | Round trip of a bundle into an isolated deployment; signature still verifies | 0 |

**Rollout order.** Phases 0 and 1 run in parallel. Phase 2 follows the
publisher API, phase 3 the read API. Phase 4 follows phase 3, which brings
the verifier. A first customer profile moves after phase 3, and its headless
servers after phase 4. Promotion to stable follows the
project owner's own yes, as today.

**First-customer cutover**, for a profile `desktop-x7k2q`:

1. Cloud registers `x7k2q`, binds its workspaces, creates the KMS key and a
   publisher token.
2. Move `RELEASES_REPOSITORY` out of `desktop-x7k2q` into a new GitHub
   Environment `desktop-x7k2q-bridge`, with the releases App's id and key
   under the bridge's own names `DESKTOP_BRIDGE_APP_ID` and
   `DESKTOP_BRIDGE_APP_PRIVATE_KEY`, and delete the releases App secrets
   from `desktop-x7k2q`. The bridge environment holds those secrets and
   nothing else and never builds. Then set
   `RELEASE_FEED=cloud`, `DESKTOP_FEED_PUBLIC_KEY` and
   `DESKTOP_FEED_PUBLISHER_TOKEN` in `desktop-x7k2q`. The profile now
   passes the `cloud` column of the validation table above; with the
   GitHub secrets still present, step 3 would be refused.
3. A prepare-only Desktop Release run for the profile produces a `cloud`
   candidate, whose `install.sh` and `install.ps1` install from the feed
   (phase 4); a beta is published to the feed on request.
4. A member of a bound workspace installs from the downloads page and
   updates from the next beta; a member of another workspace gets `403`.
5. If the profile has `github:`-fed installs, Desktop Promote is dispatched
   once with `bridge-environment: desktop-x7k2q-bridge` and the step 3
   candidate: it publishes that `cloud` build's assets, already signed and
   notarised, to the bridge environment's repository as one last GitHub
   release. Installed builds update into it and use the feed from then on.
   Then the bridge environment is deleted and the repository archived.
6. Stable is promoted on the project owner's own yes.

## Cloud-side ownership

What relay-control must own and run:

- [ ] Profile registry and workspace binding: registry table, admin tool.
- [ ] Feed signing keys: one KMS Ed25519 key per profile, `key_id` in every
      manifest, a rotation procedure document.
- [ ] Private artifact storage: bucket, per-profile prefixes, retention
      (every stable, the last 10 betas per channel), a 20 GB per-profile
      quota that blocks publication instead of deleting.
- [ ] Manifest and list endpoints: `GET /desktop/feed/manifest`,
      `GET /desktop/feed/releases`.
- [ ] Downloads: `POST .../download-url` (5 minutes, one asset) and the
      streamed `GET /desktop/feed/assets/{id}`.
- [ ] Downloads page: `/cloud/dashboard/downloads`.
- [ ] Install tokens: minting, 1-hour expiry, revocation on the page and with
      the user.
- [ ] Publisher tokens: issue, scope, rotate, revoke; publisher endpoints.
- [ ] Audit log and alerting: a record per read, URL and publish; an alert on
      publish and on bursts of `403`.
- [ ] Rate limits on every feed endpoint.
- [ ] On-prem tooling: `relayctl desktop-feed sync`, `export`, `import`, and
      the `POST /desktop/feed/releases/import` publisher call that stores a
      pre-signed envelope after verifying it (path (a)).
- [ ] Contract: the OpenAPI document and the manifest schema.
- [ ] A staging feed for the self-hosted end-to-end test.

## Out of scope and follow-ups

- Windows Authenticode for Setup, executables and DLLs.
- Linux package signing (`.deb` and tarballs).
- A signed `SHA256SUMS` for the public GitHub feed; it could reuse the
  manifest signer.
- An automatic update timer for headless installs.
- Hosted MCP recognition of a profile's hosts
  (`desktop-core/src/mcp_settings/recognized.rs`).
- Per-profile substitution of `install.ps1`: closed in phase 4. Desktop
  Release stages and publishes `install.ps1` with the profile's seven
  values, as it does `install.sh`.

## Open questions

1. **First-install verifier for the install scripts.** Decided (lead):
   the first install trusts TLS to the built-in site, the install token and
   the page's sha256 and verifies with OpenSSL 3; every update is verified
   by the installed probe.
   Phase 4 fails closed on both platforms: with neither an installed core
   that knows `feed verify` nor OpenSSL 3, `install.sh` and `install.ps1`
   refuse and ask for OpenSSL 3. Linux already needs `python3`, and
   OpenSSL 3 ships on every supported distribution. A stock Windows machine
   has no Ed25519 verifier, so `install.ps1` also accepts an explicit
   opt-in, the `-AllowUnverifiedFeed` switch or
   `AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1`, which skips the check with loud
   warnings; a small standalone verifier shipped as its own pinned asset
   stays the alternative there. Confirms: lead, with the project owner.
2. **Which workspace counts.** A user in several workspaces signs in to one;
   is the feed answered when that workspace is bound, or when any of the
   user's workspaces is? Decides: Cloud team.
3. **Key rotation without a gap.** Should `DESKTOP_FEED_PUBLIC_KEY` hold a
   current and a next key, so a planned rotation needs no forced build?
   Decides: lead and Cloud team.
4. **Freeze protection.** Decided (2026-11-07): for testing, the
   client's checks are the manifest's timestamp (not older than the
   installed release's) and the downgrade refusal in step 2 above; before
   any production profile, Cloud adds stronger stale-feed protection, such
   as a countersigned short-lived freshness statement over the current
   manifest (it needs an online key). Open: the form of that statement.
   Decides: Cloud team.
5. **Path (a) publish semantics.** Decided (lead): the hosted publish
   endpoint is the only signer; a customer's Cloud takes a signed release
   through a separate publisher call, `POST /desktop/feed/releases/import`
   (envelope plus assets, verified against the profile's public key before
   it is stored). Open: whether the Cloud team prefers to ship (b) first and
   add the import call later. Decides: Cloud team.
6. **Retention and quota.** Decided (2026-11-07): every stable
   release is kept, and the last 10 betas per channel; the per-profile limit
   is 20 GB, and reaching it blocks a new publication (the publisher call
   fails with a clear error) rather than deleting a retained release.

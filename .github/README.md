# GitHub Actions: CI, candidate preparation and releases

Linux CI and release builds use Blacksmith through repository Actions variables.
Read-only jobs use
`CI_LINUX_*`; release publishing and changelog promotion use
`DEPLOY_LINUX_RUNNER`. Desktop CI and Desktop Release also pick their macOS and
Windows runners from `CI_MACOS_*` and `CI_WINDOWS_*`, which default to the
GitHub-hosted labels, so moving a lane is a settings change. TestFlight keeps
its GitHub-hosted runner.

| Repository variable | Blacksmith value | Fallback when unset |
| --- | --- | --- |
| `CI_LINUX_RUNNER` | `blacksmith-4vcpu-ubuntu-2404` | `ubuntu-latest` |
| `CI_LINUX_ARM64_RUNNER` | `blacksmith-4vcpu-ubuntu-2404-arm` | `ubuntu-24.04-arm` |
| `CI_LINUX_2204_RUNNER` | `blacksmith-4vcpu-ubuntu-2204` | `ubuntu-22.04` |
| `CI_LINUX_2204_ARM64_RUNNER` | `blacksmith-4vcpu-ubuntu-2204-arm` | `ubuntu-22.04-arm` |
| `DEPLOY_LINUX_RUNNER` | `blacksmith-4vcpu-ubuntu-2404` | `ubuntu-latest` |
| `CI_MACOS_ARM64_RUNNER` | `blacksmith-6vcpu-macos-15` (Apple Silicon) | `macos-15` |
| `CI_MACOS_X64_RUNNER` | none: Blacksmith has no Intel macOS | `macos-15-intel` |
| `CI_WINDOWS_RUNNER` | `blacksmith-4vcpu-windows-2025` (public beta) | `windows-latest` |
| `CI_WINDOWS_ARM64_RUNNER` | none: Blacksmith has no Windows on Arm | `windows-11-arm` |

The macOS variables also choose the runners that sign and notarize release
disk images; after changing one, publish a beta release and check its images
before the next stable one. Blacksmith's Windows image ships the Visual Studio
Build Tools rather than full Visual Studio. The Windows jobs build with MSYS2's
clang toolchain, which `msys2/setup-msys2` installs on any image, and install
Inno Setup with Chocolatey, or winget where Chocolatey is missing, when the
image lacks it.

Keep GTK app builds on Ubuntu 22.04: its GTK 4.6 is the supported minimum.
Each architecture builds and tests natively. Standalone probe CI uses Ubuntu
24.04 with Zig targeting glibc 2.34. Do not point an ARM variable at an x64
runner or use the general Ubuntu 24.04 variable for GTK builds.

The [Blacksmith GitHub App](https://docs.blacksmith.sh/introduction/quickstart)
must have access to this repository. Configure the variables under repository
Settings → Secrets and variables → Actions → Variables, or with `gh variable
set NAME --body VALUE`. Values use the
[published Blacksmith runner labels](https://docs.blacksmith.sh/blacksmith-runners/overview).
Delete a variable to send its jobs back to the GitHub-hosted fallback on the
next run. These fallbacks apply when a variable is empty, not when a configured
runner is unavailable.

Existing Rust cache actions work on Blacksmith without replacement.

## Parallel CI lanes and required checks

For same-repository pull requests, CI first selects changed components, then
checks representative native architectures: macOS arm64, Linux x64 and
Windows x64. Pushes to `main` select components the same way and test both
architectures on every platform, comparing each push with the last commit
whose Desktop CI run on `main` passed (not merely the previous push), so a
component whose push run failed keeps being tested until `main` is green
again. macOS packaging, Swift tests and core
integration tests run independently. Linux independently selects GTK/library
and real-core integration/startup steps within its native test job, alongside
the Zig core build and packaging. Windows also
runs native tests independently of packaging. Swift source/test changes select
Swift checks without Rust/npm core or release packaging work. Ordinary Rust
source/test changes retain native tests and core integration while deferring
release packaging on all platforms. Reviewed build inputs select the relevant
package lanes; unknown inputs conservatively select everything. Fork pull
requests retain the workflow's existing skip policy.

The existing `macOS arm64`, `macOS x64`, `Linux arm64`, `Linux x64`,
`Windows arm64` and `Windows x64` check names are retained as compatibility
completion gates over the selected architecture matrix. Their names preserve
existing branch protection; they do not imply that a deferred architecture
was tested in that run. Job summaries list the actual architectures covered.
Each gate requires successful change detection and successful completion of
every applicable platform lane. A failed, cancelled or unexpectedly skipped
applicable lane fails the gate. An intentionally excluded component passes its
lightweight gate after successful detection and validation of the skipped
native dependencies. Gate runners remain active to preserve the expanded
architecture check names used by branch protection; missing/invalid flags,
failed detection and unexpected dependency results fail validation.
`Shell scripts` and `Probe` retain their names.
Both `Linux probe glibc 2.34 (arm64)` and `Linux probe glibc 2.34 (x64)` are
also preserved as inexpensive compatibility gates over the selected Linux
core builds in both representative and full-matrix modes. The actual producer
jobs are named `Linux core (arch)`. Their
gate summaries identify which architectures were checked and which are
deferred; producer failure or an unexpected skip fails both gates.

Changelog/prose-only diffs skip all tests and builds; only change detection and
lightweight compatibility status jobs run. CI uses [job conditions](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-jobs-with-conditions)
to skip build/test jobs rather than the entire workflow, while retaining the
compatibility runners so expanded required-check names still report.

Release first looks for a passing Desktop CI push run of its exact commit on
`main` (waiting for one still in progress). With one, it skips the test matrix,
which `main` already ran on both architectures, builds only the versioned
Linux cores its packages need, and starts the macOS and Windows builds at
once. Without one (a tag on a commit `main` never passed, or a failed run), or
when dispatched with `full-checks`, it runs reusable CI tests for every
platform and both arm64 and x64 before release packaging begins, regardless of
changed-path selection, as before. Either way the release's own build jobs then
package both architectures, and duplicate unsigned CI packages are omitted.
There is no nightly workflow. The tradeoff is deliberate: macOS x64, Linux
arm64 and Windows arm64 regressions can pass pull request checks and are caught
on `main` or by the release's full matrix. The existing completion gate names remain usable by
branch protection under this representative-architecture policy. Release-only
linking, app bundle/runtime collection and installer issues introduced by
ordinary source edits may likewise be caught during release packaging. Every
release still packages both architectures after its full test prerequisite
succeeds.

Each `Linux core (arch)` job uses Zig to build the core once, validates
the glibc floor and uploads that binary. The matching Linux package job and
selected integration steps download the same core artifact, so packaging and real-core
tests exercise the validated binary without compiling it again. A GTK-only
test change skips the core download, real app build, real-core tests and startup
smoke test. The artifact
carries a checksum and source, target and version metadata. Prerelease CI
builds it with the release's resolved version; the Linux release package job
reuses that validated artifact too. Core artifacts are retained for seven
days. `probe-version` also flows to the Linux unsigned CI packages so the
package version matches its downloaded core.

Every `Linux tests (arch)` run that tests the GTK app also checks its glibc
floor (2.34) on the lib test binary it built, so a libm or glibc symbol newer
than the floor in GTK code fails there even when no package job is selected.
The Linux package job still checks the real release binary.

Reusable CI accepts `full-matrix` (both architectures), `force-all` (every
component), `skip-packages` (omit unsigned CI packaging), `probe-version`
(the version embedded in the core, default `0.0.0`) and `core-only` (build
only the checked Linux cores). Release supplies the first four to request full
tests without duplicate packages, or `core-only` when `main` already passed
its commit. A manual CI run can request
the full matrix with unsigned packaging enabled when diagnosing an
architecture-specific issue, and `base-sha` selects lanes from the changes
since that commit (empty runs every lane).

Swift build caches separate architecture and package/test lane, and include
Swift, Xcode, SDK and manifest identities. Windows caches separate downloaded
Cargo sources from compiled dependencies; compiled cache keys include
architecture, toolchain, dependency manifests and debug/release profile, stay
stable across ordinary source commits, and exclude Cargo incremental state.
Dependency-focused caching avoids storing a new copy of large app/test binaries
for each source revision: workspace products are pruned before the first save,
and the key includes profile settings rather than the source SHA. Windows tests
disable dev/test debug symbols to limit linker work and cache size while
retaining debug assertions and overflow checks. Release builds use the same
Windows cache setup with their own release profile; test settings do not
change release builds.

Windows platform, wording/state, GTK and hosting tests use targeted
`cargo test --no-default-features --features gtk` commands with `--lib` or
`--test`. The default `production-binaries` feature gates the app, core and
contract-codegen executables, so these tests avoid Cargo's automatic linking
of unrelated production binaries. The fake core remains available for the
integration harness. When packaging is skipped, a separate
`cargo check --features gtk --bins` keeps all Windows app/core/codegen entry
points checked with default features enabled without linking their executables.
Linux's GTK/library-only mode also opts out of default production binaries.
When Linux integration is selected, it shares one default-feature debug build
with GTK checks in the same native test job. Ordinary production builds,
real-core integration and release packaging retain default features.
`scripts/test_cargo_target_selection.py` guards that target selection.

macOS and Linux Rust caches also separate package and test lanes. Linux's
GTK/library and real-core steps share the `tests` cache and setup, avoiding
duplicate GTK builds when both are selected. Parallel package/test jobs save
their own dependency builds. A new lane can seed itself from the
previous shared cache, using the complete historical key with read-only saves.
A marker restored with the lane cache prevents an older shared seed from
replacing a newer lane cache, including a partial lockfile match. Package
lanes prune restored debug profiles; macOS core tests and Linux native tests
prune release profiles. Cache markers live under the runner temporary directory,
outside the repository.
New Windows and Swift scopes start cold; restores are an optimization and
every selected lane still executes its builds and checks.

## CI change selection

`scripts/ci-changes.py` independently selects shell checks, Probe tests, Swift
tests (`macos`), macOS Rust/npm tests (`macos_core`), macOS packages
(`macos_package`), Linux GTK/library tests (`linux`), the Zig core build
(`linux_core`), real-core integration/startup (`linux_integration`), Linux
packages (`linux_package`), Windows native tests (`windows`) and Windows
packages (`windows_package`). It uses only Python's standard library and emits
boolean `key=value` lines suitable for `$GITHUB_OUTPUT`:

```sh
python3 scripts/ci-changes.py --base "$BASE" --head "$HEAD" >> "$GITHUB_OUTPUT"
python3 scripts/test_ci_changes.py
```

The comparison uses `git diff --name-only --no-renames -z`: deleted paths and
both sides of a rename participate, and spaces or newlines in a filename stay
intact. Callers using stdin must supply that same NUL-delimited format. A
successful `--base`/`--head` Git comparison with no changed paths selects no
lanes, including distinct commits with identical trees. Empty/malformed stdin
or unavailable history selects every job, since an empty upstream transport
cannot prove that nothing changed. `--all` always selects every group. No
changed-path service or third-party filter action is required.

Pull request selection compares the event base with the tested merge commit,
covering the whole PR diff rather than only the latest commit. A doc-only
follow-up on a PR that also changes code still selects the lanes required by
that code.

| Changed input | Selected job groups |
| --- | --- |
| `CHANGELOG.md`, reviewed documentation allowlist, documentation PNG screenshots, or `ipad/` only | None |
| Swift source/tests in known macOS or shared package target directories | Swift checks only |
| Exact Swift package manifests, macOS plist/entitlements, or macOS app resources | Swift checks and macOS packaging |
| Generated `macos/Sources/RelayDesktopCore/CoreContract.swift` | Probe contract check and Swift checks |
| Linux packaging under `linux/` | Shell, Linux GTK/library tests, core, integration and packaging |
| Windows packaging under `windows/` | Shell, Windows native tests and Windows packaging |
| `gtk/tests/linux_app_gtk.rs` or Rust modules under `gtk/tests/linux_app_gtk/` | Linux GTK/library and Windows native tests; no core build or integration |
| Rust modules under `gtk/src/ui/` | Linux GTK/library tests, core, integration and Windows native tests |
| Other Rust modules under `desktop-core/src/` or `gtk/src/`, or Rust tests under `desktop-core/tests/` or `gtk/tests/` | All native test/core groups, without packaging |
| Cargo/build inputs, resources, scripts, CI configuration, shared fixtures, assets, global configuration, or unknown paths | All |

The reviewed prose allowlist lives in the script as explicit paths; `.md`
alone never exempts a file. It includes the reviewed top-level `docs/*.md`
files, probe and integration READMEs, Windows icon READMEs and the DMG artwork
prompt. New documents remain unknown until reviewed. `docs/relay-core.md` is
excluded from that allowlist because contract tests read it, and keeps every
component selected. Routine `CHANGELOG.md` edits add no tests or builds; in a
mixed change, the remaining paths determine all selected lanes. `desktop-core/build.rs`
still reads the release heading and derives development version metadata from it, so
changelog-only edits defer validation of that input until the next selected
build or release. Release forces every component and packages the actual
resolved version, even when only the changelog changed. Shared GTK bridge,
host and model source retains all native test/core groups because those
modules also build without GTK on macOS. Changes to this selector
or its tests run the shell job and every build group. Extend the allowlist only
after checking consumers and adding a regression test.

The audited GTK harness and its Rust modules select Linux GTK/library and
Windows native tests without the Zig core or real-core integration steps. Rust
UI modules under `gtk/src/ui/` are guarded by the GTK feature and
also select the Linux core and integration lanes, without scheduling Probe or
macOS jobs. Shared bridge/host/models remain broader native inputs.
The audited Pi live-session test and runner script select Probe and macOS core
tests, with shell checks added for the script. These exceptions apply before
the broader source/script defaults. Other test helpers retain their existing
coverage, and combining an exception with core, build or unknown inputs takes
the union rather than suppressing those inputs.

`macos`, `macos_core` and `macos_package` control Swift tests, Rust/npm tests
and release packaging independently. Swift tests compile the macOS app entry
point through the package's test-target dependency. The Swift lane also runs
`swift test --package-path shared/RelaySessionOverview` so the shared package's
own test target executes, rather than only building its library as a dependency.
Known source/test targets select Swift checks alone; exact package manifests,
plist/entitlements and the app's existing resource scope also select packaging.
Unreviewed future package targets/build paths default to all groups.
The macOS completion gate accepts Pi-only changes with app lanes skipped and
core tests required. The generated
Swift contract is an output of the Rust generator rather than a Rust/npm input,
so changing only that file runs Probe's consistency check and Swift validation. A matching Rust
registry change selects native core tests, including macOS core tests. Unknown paths,
shared core/dependency/toolchain inputs and every fallback also select
`macos_core`, so a mixed Swift/core change cannot hide core regressions.

`linux` selects GTK/library steps without requiring a core artifact;
`linux_core` controls the Zig build; `linux_integration` requires that build and
selects the real-core and app-startup steps in the same native test job.
`linux_package` also requires the core artifact. The completion gate validates
the selected work independently. `windows` and `windows_package` separately
control native tests and the release build, portable zip and installer smoke
checks. Ordinary Rust source changes skip all three package flags, while Cargo
build files, resource/build inputs and unknown paths retain packaging coverage.
Mixed changes take the union of their selections. The minimal prerequisites
are `macos_package` → `macos`, `linux_integration`/`linux_package` → `linux_core`,
and `windows_package` → `windows`. Reviewed docs and validated unchanged Git
trees select no lanes; unavailable history, empty/malformed stdin and explicit
`--all` select every group.
Release builds packages alongside CI verification. When main has not passed
the exact commit, fallback checks cover every component and reuse the already
built Linux core. For each Windows architecture, core and GTK compile in
separate native jobs and capture their runtimes with source/version identities
and file hashes. A subsequent native Python packaging job verifies and merges
those captured files, creates the app/core ZIPs and Inno Setup installer, then
runs packaged-core and install/uninstall checks. Packaging does not reinstall
MSYS2 or compile Rust again. Final `windows-x64` and `windows-arm64` artifacts
retain their existing names for release staging.

## Pinned actions

Workflows and the composite actions under `.github/actions` pin every
third-party action by full commit SHA, with the release as a trailing comment:
`uses: actions/checkout@<sha> # v4.4.0`. A moved or compromised tag cannot
change what runs, which matters most in `desktop-release.yml`, where signing and
notarization secrets are in scope. Resolve a new pin with
`gh api repos/OWNER/REPO/commits/TAG --jq .sha` (this dereferences annotated
tags to the commit). Dependabot (`.github/dependabot.yml`) proposes weekly,
grouped bumps that keep the SHA and comment in step. `dtolnay/rust-toolchain`
has no releases, so it is pinned to its `stable` branch head and updated by
hand.

## Prepare once, promote the same release artifacts

Each successful **Desktop CI** push run on `main` automatically starts a
**Desktop Release** preparation for that exact tested source commit and the
next unpublished stable version. Failed CI runs, pull requests and manually
started CI runs do not trigger preparation. Every checkout, the Linux core
artifact and Windows runtime provenance use that tested SHA even when `main`
advances while runners are queued. Automatic preparations use a separate
concurrency group for each tested source SHA and never cancel one another.
A late completion or rerun of older CI cannot cancel a newer candidate. They
cannot cancel or hold the ordinary release/promotion lock.
They never publish a release or promote the changelog.

Preparation builds release-version binaries for every native architecture,
signs and notarizes the macOS images, smoke-tests Windows installers, and
stages final assets and notes. It reuses the successful exact-commit CI test
result; CI's development/test executables cannot replace the release binaries
with their final version, analytics configuration and signing. Preparation
uploads the immutable `release-candidate` artifact for seven days. The entire
commit-to-candidate interval still includes CI, native compilation,
notarization and runner queues. An automatic run with an empty release notes
section succeeds with a skip notice identifying the tested source, without
starting native builds or requiring analytics credentials. Malformed or missing
changelog sections still fail. Manual preparation and publishing still require
release note bullets; preparation otherwise needs the existing release credentials.

To prepare a specific beta, stable version or other reviewed source ref,
dispatch **Desktop Release** with `prepare-only: true` and the intended final
version/channel. This manual path uses the existing release concurrency group
and exact-commit test prerequisites. This path shares `desktop-release` with
normal releases and promotion: avoid dispatching multiple manual preparations
ahead of an urgent publish, since GitHub retains at most one pending run per
concurrency group.

Once a candidate run succeeds, dispatch **Desktop Promote** with that numeric
Desktop Release run ID and its identical version/channel. Promotion downloads
and verifies that run's source, provenance, notes, all six architecture package
sets, updater aliases, checksums and every asset's SHA-256 and size. It runs on
the deployment Linux runner without native compilation, signing or
notarization. The promotion job has a ten-minute execution timeout; runner
queues and artifact transfer speeds still determine elapsed time. Measure
ready-candidate promotion separately from commit-to-candidate preparation
before claiming sub-ten-minute releases. Actual timing validation is pending. Candidates older than seven days,
incomplete packages, changed bytes and mismatched identity fail before
publishing.

Candidate creation does not reserve a release version. Several commits may
prepare the same next unpublished stable version; choose the desired source
explicitly by its successful run ID. Promotion rechecks that both the release
version and distribution tag are unused under the shared `desktop-release`
lock. Once one candidate is published, prepare a new candidate/version for any
other commit. A beta cannot be relabeled stable because the final version is
embedded in the signed binaries. Native outputs are compiled once during
preparation and those same bytes are uploaded during promotion.

Promotion uploads into a draft and makes it public only after every upload
succeeds. If an upload fails, inspect/remove the incomplete distribution draft
and its tag before retrying; promotion refuses to overwrite existing versions.
No workflow creates a tag in the source repository. Downloaded candidate files
are never executed; the verifier comes from the current `main` branch.

The standard-library Python manifest's `checks` field alone is not proof.
Promotion fetches independent GitHub API metadata proving the completed,
successful same-repository Desktop Release producer. Manual producers must
have the manifest's source SHA. Automatic producers additionally record the
triggering CI run ID; promotion fetches that successful Desktop CI push/main
run and matches its source SHA. This matters because a `workflow_run`
producer's own `head_sha` can refer to a newer default-branch workflow context.
The Resolve version log records `Releasing v<version>` followed by
`Source <tested source SHA>` and, for automatic preparations, `CI run <run ID>`.
Release coordinators use that logged source to identify the prepared commit;
the values match the candidate manifest rather than the producer context SHA.
The manifest binds these identities to the prepared bytes; it does not replace
platform signing verification during preparation.

```sh
python3 scripts/test_release_notes.py
python3 scripts/test_release_candidate.py
python3 scripts/test_release_workflow.py
python3 scripts/test_package_windows.py
python3 scripts/test_build_windows.py
```

## Site profiles

**Desktop Release** and **Desktop Promote** take a `profile` input: the
opaque id of a site profile, whose values live in a GitHub Environment on
this repository named `desktop-<id>` ([the contract](../docs/workspace-layout.md#site-profiles)
lists its secrets). With it, every job that builds or
publishes (`version`, `release-core`, `build`, `build-linux`, the Windows
compile and package jobs, `publish`; `promote`) runs with
`environment: desktop-<id>`, so that environment's secrets apply, and the environment's protection rules gate each of them. Empty, the
default, leaves `environment` empty: no environment, the repository's
secrets, the default profile (`agentrelay`), the release as before. Automatic
candidates (`workflow_run`) and `desktop-v*` tags carry no inputs and are
always the default profile's. The `changelog` job and the promote's changelog
steps run for the default profile only: a profile's versions are allocated
against its own releases repository.

Each such job starts with the `site-profile` composite action
(`.github/actions/site-profile`), after the checkout: it validates the
environment's values, masks the values it derives from them, exports them as
`PROFILE`, `ASSET_PREFIX`, `SITE_URL`, `URL_SCHEME`, `PRODUCT_NAME`,
`BUNDLE_ID`, `RELEASES_REPOSITORY`, `RELEASE_FEED`, `FEED_PUBLIC_KEY`,
`SUPPORT_URL` and `BRANDING_ICON` for
the steps, and, when `DESKTOP_BRANDING_URL` is set, fetches the branding
archive into `branding/`. A URL in the private `AgentWorkforce/desktop-branding`
repository is fetched with a short-lived installation token the action mints
from the organization's GitHub App (`GH_APP_PUSHER_ID`,
`GH_APP_PUSHER_PRIVATE_KEY`; Contents: read on that repository only), unless
the environment sets `DESKTOP_BRANDING_TOKEN`, which overrides it. The
default profile and profiles without branding never mint. The steps pass the values to the scripts as
options from those variables, never inline, so the resolved script in the
log shows the variable names. The Resolve version log adds `Profile <id>`
after `Source <sha>` (the lines before it are unchanged), the candidate
manifest records `"profile": "<id>"` (empty for the default), and every
artifact and release asset is `Desktop-<id>-...` instead of `AgentRelay-...`;
the release notes name the profile by that prefix and the release title is
the version alone. Promotion verifies that the candidate was prepared for
the requested profile.

The profile's values are environment **secrets**, not variables: the runner
prints a composite action's inputs and a step's `env` in the log before the
step runs, and masks only secrets from the start of a job. A job output that
holds a masked value is dropped, so only the opaque id goes through job
outputs; every job reads the values from its environment.

A profile's publish needs the environment's `RELEASES_REPOSITORY` to exist,
be public, and be writable with its releases App (`DESKTOP_RELEASES_APP_ID` and
`DESKTOP_RELEASES_APP_PRIVATE_KEY` in the environment, the App installed on
that repository) or its `RELEASES_TOKEN`. The default profile uses the
repository secrets `RELEASES_APP_ID` and `RELEASES_APP_PRIVATE_KEY` instead; a
profile never reads them (an environment secret shadows a repository secret of
the same name but cannot hide one, so sharing the name would hand the profile
the default's App). A profile without its own pair may set
`DESKTOP_RELEASES_USE_ORG_PUSHER` to `true` (compared case-insensitively, as
GitHub expressions do) in its environment: Desktop Promote then mints the token
from the organisation's pusher App (`GH_APP_PUSHER_ID` and
`GH_APP_PUSHER_PRIVATE_KEY`), which must be installed on that repository.

The profile's release feed is fixed in the apps when they are built
(`docs/workspace-layout.md`, "Site profiles"):

| Secret | `github:` feed | `cloud` feed |
|---|---|---|
| `RELEASE_FEED` | optional; must name `RELEASES_REPOSITORY` | `cloud` |
| `RELEASES_REPOSITORY` | required, not the default's | must be unset |
| `DESKTOP_FEED_PUBLIC_KEY` | must be unset | required; a 32-byte Ed25519 key, base64 |
| `DESKTOP_FEED_PUBLISHER_TOKEN` | must be unset | required; only whether it is set reaches the action |
| `PULL_RELEASES_REPOSITORY` | must be unset | optional; `owner/name`, not the default's releases repository |
| `AGENT_RELAY_SITE_URL` | not the default's | not the default's |

`PULL_RELEASES_REPOSITORY`, when a `cloud` profile's environment sets it,
names a GitHub repository for the profile's pull bundle
([Pull model](../docs/enterprise-distribution.md#pull-model)). After the
cloud-feed publish, Desktop Promote creates the release `v<version>` there (a
draft first, then published; a prerelease for a beta) holding
`pull-bundle.json` (the profile, version and channel it is for),
`publish-request.json` (the exact body of the feed's create call),
every asset in `cloud-assets.txt`, `release-notes.md`,
`agent-relay-desktop-update.sh`, `SHA256SUMS` (bare names) and
`SHA256SUMS.sigstore.json`, the build-provenance attestation of `SHA256SUMS`
made by the promote job (`id-token: write`, `attestations: write`). It writes
with the profile's releases App when the App is installed on that repository,
else with `RELEASES_TOKEN`, and checks access with `gh api repos/<repo>`
before the cloud publish, failing when neither can write. A rerun after the
release exists passes when its `SHA256SUMS` is the same and fails otherwise.
Right after the cloud publish the job keeps the artifact
`pull-receipt-<profile>-v<version>` (the create body and the candidate it
came from, found with the job's own token and `actions: read`), so a rerun of
the same candidate skips the cloud publish it already made and one of another
candidate is refused.
Unset, the promote prints a notice and skips the bundle. The action exports
and masks it.

The `site-profile` action refuses any other combination. A profile with an
unset `RELEASE_FEED` builds and publishes as before. The version, publish and
promote jobs refuse a `cloud` feed: publishing to one is not available yet.

A prepare-only run (and an automatic preparation) neither mints a releases
token nor checks publishing access: it needs `RELEASES_REPOSITORY` to name an
existing public repository, because the version job lists its releases with the
workflow's own `GITHUB_TOKEN` to allocate the version, and nothing more. The
releases App pair and `RELEASES_TOKEN` are needed by Desktop Promote, which
checks publishing access before it publishes, and by a publishing Desktop
Release run.

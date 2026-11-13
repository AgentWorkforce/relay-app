# Cloud release orchestration

`flows/cloud-release.flow.ts` owns a release request's durable state: source
resolution, candidate discovery, gate selection, overlapping native preparation,
readiness, bounded retries, stable confirmation, promotion and the shipped tag.
The lead submits it through the existing host entry points. Actions execute
native tests/builds/signing/packaging and independently verify CI and artifact
identity before handling signing/publishing credentials.

This is the first release-orchestration migration. Review/landing policy is
unchanged. Landing still belongs to the lead; the shadow land config is not
made live. Profile-specific version allocation stays in the native worker,
where the profile's private feed and environment are available. The existing
main-CI automatic stable preparation remains compatible during rollout.
Source refs must carry the reviewed worker inputs added by this migration.
For hotfixes based on an older release, backport those build-tool changes
through review or keep that release on the explicit legacy path; never
substitute main's source for the requested older release.

## Rollout

The switch is **off by default**. Land and deploy the reviewed code first;
do not switch an in-flight incident release to a new orchestrator.

1. The Cloud secret owner provisions `DESKTOP_RELEASE_GH_TOKEN` for the
   release flow's command steps. Use a dedicated GitHub App/service identity
   restricted to `AgentWorkforce/relay-desktop`, Actions read/write and
   Contents read/write. It does not need access to releases repositories or
   any signing/publishing key. Ensure the credential remains usable throughout
   a run (up to 180 minutes); short-lived installation tokens need the secret
   owner's refresh mechanism. Do not copy the host's `gh-token`, pusher App
   private key or Cloud submit token into the sandbox or flow input.
2. Run a **prepare-only** smoke request through `/opt/agent-host/cloud-release.sh`
   with a known source and gate. Check Cloud acceptance, native worker dispatch,
   exact-tree reuse, candidate output and a deliberately invalid evidence
   refusal. A local `flows check` does not establish Cloud execution support.
3. Set `DESKTOP_RELEASE_ORCHESTRATOR=cloud` in the lead host's managed
   environment and deploy it through Agent Deploy. `release.sh` and `tick.sh`
   then submit Cloud runs; they never silently fall back after a Cloud refusal.
   Reverting the switch to `legacy` preserves the old entry points during
   rollout, without stopping any already-submitted Cloud run.

## Commands

With the switch enabled:

```sh
release.sh prepare stable main --gate 123456
release.sh stable --run 234567 --readiness /absolute/path/to/release-readiness.json
release.sh beta --run 234568 --readiness /absolute/path/to/release-readiness.json
```

`prepare` starts compilation beside the supplied gate. Without `--gate`, the
flow looks for a successful main CI run of the source, or dispatches a full
native gate itself. Workers independently check the source ref has not moved.
The command prints a **Cloud run ID**, not a guessed Actions run ID. The
submission directory under `~/state/cloud-releases/request.*/` contains
`run.json` as a convenience pointer. The authoritative state is
the Cloud journal, not `~/state/candidates.json`. An EXIT trap removes the
source checkout on success and on clone, dependency-install or submit failure;
request/run pointers and diagnostic `release-state/` files remain beside it.

A successful clean review writes `.relayflow/cloud/release-readiness.json` (or
under `RELAYFLOW_STATE_DIR`) with the exact source tree, reported Cloud review,
zero open blockers and green local CI. This is a **trusted lead attestation**,
not independent verification of author-supplied JSON. If fixes or a rebase
change the tree, the lead must reconcile the reported findings, any required
blocker re-review and local CI before issuing a receipt for the new tree.
Never reuse an old receipt against changed contents. The release flow checks
this receipt's tree and independently fetches the GitHub gate evidence.

Stable publication parks at `f.human`, addressed to `khaliq-gant`. Its `to`
field records who is asked; it is not automatic DM delivery. The lead sends
the pending request to the owner, who answers through Cloud's UI or
`flows answer --cloud <run-id> yes|no`. A relayed quote or an input boolean
cannot bypass this wait. Beta publication is requested explicitly, without
adding a stable approval wait.

Inspect with `flows status --cloud --json <run-id>` using the host's existing
Cloud submit credentials. Do not blindly submit another request after an
ambiguous transport failure. Match the acknowledged run ID and inspect its
journal/workers first.

## CI reuse and retries

`ci-run-id` is evidence, not a skip switch. Both preparation and promotion
fetch the actual GitHub run from the source repository. A passing main push
keeps the existing conservative last-green-baseline contract. A dispatched
review snapshot may have a different commit SHA after a squash, but must have
an identical Git tree and positive successful job/step evidence for **both
architectures of every native test suite**. Skipped or missing test steps,
forks, unrelated workflows, failed/pending runs and evidence older than seven
days are refused. When a path-selected gate did not run all release-required
suites, run one full gate and supply its ID. Crate overrides and explicit full
checks cannot reuse this evidence. Native package checks still run on the
newly built bytes. For Windows, a same-architecture successful package build
may supply the entrypoint compilation that the test job deliberately skips;
every Windows runtime test step is still required. Discovered main CI older
than seven days is ignored so the flow starts a fresh gate instead.

Every worker is correlated by an explicit request ID plus a digest binding
its source, gate, candidate, channel and profile. The flow never adopts the
newest run by timestamp. Before dispatch it reserves a `release-requests/`
branch atomically. A lost POST response can therefore be discovered without
issuing a second POST. A reservation with no visible worker is an ambiguous
submission: the flow stops after bounded discovery for the lead to inspect.
It must not delete that reservation and retry blindly. These refs are request
receipts, not shipped tags. `layered-relay-desktop` owns their manual cleanup:
retain each reservation for at least seven days and until its Cloud run is
terminal and audited. Archive the request ID, payload identity, source SHA and
worker run IDs under the lead's release records before deleting that exact
reservation ref. Never reuse an archived request ID. Do not delete a ref while
its run is active, a worker is pending, or a dispatch response remains
ambiguous; resolve those conditions first. There is no automatic GC.

Only the two exact tests in `RETRY_TESTS` get one automatic failed-job rerun,
and only if every reported failed test is allowlisted. Compilation failures,
unknown failures, cancelled runs and a second failed attempt stop the flow.
Actions rerun only failed jobs. Successful lanes remain accepted. Cloud prepare
workers use a source/channel lock separate from publication, so an unrelated
build waiting on its gate cannot block promotion of an already-ready candidate.
The prepare worker's independent `ci-status` verifier can occupy its runner
for up to 75 minutes while waiting for the selected gate. A first failed
attempt gets a three-minute handoff for Cloud's allowed rerun; the worker
itself never retries a job. This bounded runner wait is retained as an
independent publishing-boundary check.

## Outputs and completion

`release-state/result.json` records the candidate/run IDs, version, source SHA
and tree. Publication creates `released-v<version>` once, checks an existing
tag resolves to the same commit, and never moves a tag or creates a
`desktop-v*` tag. Profiles retain the existing no-shared-shipped-tag rule.
Default stable publication also downloads the promoted changelog into
`release-state/promoted/v<version>/CHANGELOG.md` in Cloud. The lead can fetch
the same `promoted-changelog-v<version>` artifact from the recorded Promote
run into `~/state/promoted/v<version>/` and land it through the normal single
review process.

The lead notifies the requester after publication with version, shipped SHA,
Source line and release URL. Publication alone never proves runtime health:
the result explicitly leaves hosted upload recovery pending until the
assigned incident owner verifies it against the installed release.

## Validation

```sh
npm --prefix flows ci --ignore-scripts
npm --prefix flows run check
python3 scripts/test_release_ci_evidence.py
python3 scripts/test_release_workflow.py
python3 scripts/test_release_workflow_cloud.py
node --test agent/test/review-cloud.test.ts
shellcheck agent/*.sh
```

The existing host-script tests require GNU tools; run them on Linux or put GNU
sed/coreutils on PATH on macOS. Use Python 3.10+ with PyYAML for workflow tests.
The single review round and GitHub gate remain required before landing.

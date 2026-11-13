/**
 * The relay-desktop host this folder defines must match the one
 * relaycast-cloud/agent-host built: the fixture is desktopComputeTemplate()
 * from relaycast-cloud main 0d51607 (stage dev). Everything but UserData, the
 * AmiId parameter and AgentHost's Metadata.LayeredAgentAmiPin marker is
 * byte-identical, so adopting it is a stop/start (new user data), never a
 * replacement. AmiId is pinned to the AMI the host runs where the fixture
 * follows Canonical's SSM pointer, and kit 0.1.15 marks AgentHost with that
 * pin (resource metadata, no EC2 property) so CloudFormation persists the
 * switch; the kit adopts the pin without touching the instance.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import { EC2_USER_DATA_ENCODED_LIMIT, RELAY_SESSIONS_KIT_FILES, WORKER_KIT_FILES, fileReader, hostTemplate, networkParams, userDataBase64, withoutUserData } from "@agentworkforce/layered-agent";
import agent from "../agent.config.ts";
import { LANE_SCRIPTS, LANE_SCRIPTS_SHA_FILE, laneScriptsSha } from "../lane-scripts.ts";

const fixture = JSON.parse(readFileSync(new URL("./fixtures/relaycast-cloud-main-desktop-template.json", import.meta.url), "utf8"));
const read = fileReader(fileURLToPath(new URL("..", import.meta.url)));

test("the compute template is byte-identical to relaycast-cloud main's except UserData, AmiId and the pin marker", () => {
  const ours = hostTemplate(agent, userDataBase64(agent, read));
  assert.deepEqual(ours.Parameters.AmiId, { Type: "AWS::EC2::Image::Id", Default: "ami-08cf17e3856f70154" });
  assert.deepEqual(fixture.Parameters.AmiId, { Type: "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>", Default: "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id" });
  // The marker is the pin in ours, and the only Metadata key; the fixture has none.
  assert.deepEqual((ours.Resources.AgentHost as { Metadata?: unknown }).Metadata, { LayeredAgentAmiPin: "ami-08cf17e3856f70154" });
  assert.ok(!("Metadata" in fixture.Resources.AgentHost), "the fixture's AgentHost has no Metadata");
  // Everything else, AmiId's place among the parameters included, is the fixture's.
  const fixtureAmi = JSON.parse(JSON.stringify({ ...ours, Parameters: { ...ours.Parameters, AmiId: fixture.Parameters.AmiId } }));
  delete fixtureAmi.Resources.AgentHost.Metadata.LayeredAgentAmiPin;
  if (Object.keys(fixtureAmi.Resources.AgentHost.Metadata).length === 0) delete fixtureAmi.Resources.AgentHost.Metadata;
  assert.equal(JSON.stringify(withoutUserData(fixtureAmi), null, 2), JSON.stringify(withoutUserData(fixture), null, 2));
});

test("user data: under EC2's limit, every kit and lane file, no secret values", () => {
  const encoded = userDataBase64(agent, read);
  assert.ok(encoded.length < EC2_USER_DATA_ENCODED_LIMIT, `user data is ${encoded.length} bytes encoded`);
  const script = gunzipSync(Buffer.from(encoded, "base64")).toString("utf8");
  // Since 0.1.10 the optional kit files travel over SSM after first boot, not in
  // user data; skip exactly the ones the kit's OPTIONAL_KIT_FILES (src/spec.ts) lists.
  // 0.1.13 adds headless.py and delegation.md to it but does not export the constant.
  const optional: readonly string[] = ["headless.py", "delegation.md", ...WORKER_KIT_FILES, ...RELAY_SESSIONS_KIT_FILES];
  for (const f of agent.files) {
    const shipped = script.includes(`cat > /opt/agent-host/${f.name} <<'`);
    if (f.source === "kit" && optional.includes(f.name)) assert.ok(!shipped, `${f.name} is not in user data`);
    else assert.ok(shipped, f.name);
  }
  assert.doesNotMatch(script, /sk-ant-|ghp_|ghs_|gho_|BEGIN [A-Z ]*PRIVATE KEY/);
});

// The folder's files by name: the kit's reader takes its own descriptors,
// and the lane scripts are not among them any more.
const source = (name: string) => readFileSync(new URL(`../${name}`, import.meta.url));

test("the lane scripts the user data does not carry are installed from the clone against the embedded hash", () => {
  const embedded = agent.files.map((f) => f.name);
  for (const name of LANE_SCRIPTS) assert.ok(!embedded.includes(name), name);
  const list = LANE_SCRIPTS.map((name) => name.replace(".", "\\.")).join(" ");
  for (const lane of ["setup-lane.sh", "load-secrets-lane.sh"]) {
    assert.ok(embedded.includes(lane), lane);
    const text = String(source(lane));
    assert.match(text, new RegExp(`for f in ${list}; do`), lane);
    // Only the reference clone's fetched origin/main, never a checkout's HEAD,
    // and only when the fetched set matches the hash the user data carries.
    assert.match(text, /git -C \/home\/agent\/build\/relay-desktop --no-replace-objects show "origin\/main:agent\/\$f"/, lane);
    assert.doesNotMatch(text, /show "HEAD:agent\//, `${lane} must not install from a checkout's HEAD`);
    // The hash gate itself: compare, and only then move the files into place.
    assert.match(text, /if \[ "\$sha" != "\$want" \]; then/, lane);
    assert.match(text, /mv -f "\$tmp\/\$f" "\/opt\/agent-host\/\$f"/, lane);
    assert.match(text, /sha256sum tick\.sh review\.sh release\.sh hotfix\.sh disk-cleanup\.sh cloud-release\.sh \| sha256sum/, lane);
    // The fetch under the tick's lock, taken as the agent user so the lock
    // file a first boot creates is one tick.sh and release.sh can open.
    assert.match(text, /sudo -H -u agent flock -w 600 \/home\/agent\/state\/\.tick\.lock git -C \/home\/agent\/build\/relay-desktop fetch -q origin main/, lane);
    // ~/state is made as the agent: root never chowns a path the agent controls.
    assert.match(text, /sudo -H -u agent mkdir -p \/home\/agent\/state/, lane);
    assert.doesNotMatch(text, /install -d -o agent -g agent \/home\/agent\/state/, lane);
    // A skipped install with stale copies fails only the deploy's lane.
    assert.match(text, /if \[ -n "\$\{LANE_SCRIPTS_STRICT:-\}" \]; then/, lane);
  }
  assert.match(String(source("setup-lane.sh")), /^LANE_SCRIPTS_STRICT=1\ninstall_lane_scripts$/m);
  assert.doesNotMatch(String(source("load-secrets-lane.sh")), /LANE_SCRIPTS_STRICT=1/);
  // One installer, carried verbatim by both lanes: the text between the
  // marker comment and the call must not drift.
  const installer = (lane: string) => {
    const text = String(source(lane));
    const start = text.indexOf("# tick.sh, review.sh, release.sh, hotfix.sh and disk-cleanup.sh are not in\n# the user data");
    const helper = text.indexOf("\nlane_scripts_present() {", start);
    const end = text.indexOf("\n}\n", helper);
    assert.ok(start >= 0 && helper > start && end > helper, lane);
    return text.slice(start, end + 3);
  };
  assert.equal(installer("setup-lane.sh"), installer("load-secrets-lane.sh"));
  // The deploy compares only embedded files, so the scripts' hash is one:
  // a stale hash means `npm run lane-scripts-sha` was not run after editing them.
  assert.ok(embedded.includes(LANE_SCRIPTS_SHA_FILE));
  assert.equal(String(source(LANE_SCRIPTS_SHA_FILE)).trim(), laneScriptsSha(source), "run `npm run lane-scripts-sha` in agent/");
});

test("disk cleanup: an hourly timer runs the hashed disk-cleanup.sh as the agent user", () => {
  assert.ok(LANE_SCRIPTS.includes("disk-cleanup.sh"));
  const setup = String(source("setup-lane.sh"));
  const unit = (name: string) => setup.match(new RegExp(`^cat > /etc/systemd/system/${name.replace(".", "\\.")} <<'UNIT'\n((?:.*\n)*?)UNIT$`, "m"))?.[1] ?? "";
  const service = unit("agent-host-disk-cleanup.service");
  assert.match(service, /^Type=oneshot$/m);
  assert.match(service, /^User=agent$/m);
  assert.match(service, /^Environment=HOME=\/home\/agent$/m);
  assert.match(service, /^ExecStart=\/opt\/agent-host\/disk-cleanup\.sh$/m);
  assert.doesNotMatch(service, /--dry-run/);
  const timer = unit("agent-host-disk-cleanup.timer");
  assert.match(timer, /^OnCalendar=hourly$/m);
  assert.match(timer, /^Persistent=true$/m);
  assert.match(timer, /^WantedBy=timers\.target$/m);
  assert.match(setup, /^systemctl enable --now agent-host-disk-cleanup\.timer$/m);
  // The policy's fixed points; its behaviour is test/disk-cleanup.test.ts.
  const script = String(source("disk-cleanup.sh"));
  assert.match(script, /^set -euo pipefail$/m);
  assert.match(script, /^cap=\$\{DISK_CLEANUP_CAP:-70\}$/m);
  assert.match(script, /^max_age_h=\$\{DISK_CLEANUP_MAX_AGE_HOURS:-24\}$/m);
  assert.match(script, /^log=\$state\/disk-cleanup\.log$/m);
  assert.match(script, /^for root in "\$HOME\/reviews" "\$HOME\/work"; do$/m);
  assert.match(script, /--dry-run\) dry_run=true/);
  assert.match(script, /flock -n -x 7/);
  assert.match(script, /readlink "\$p\/cwd"/);
  // It deletes only what it found under ~/reviews and ~/work: a renamed
  // checkout, a leftover of its own, or a cargo target inside one.
  const removals = [...script.matchAll(/rm -rf -- "([^"]+)"/g)].map((m) => m[1]);
  assert.ok(removals.length > 0);
  for (const r of removals) assert.match(r ?? "", /^\$(trash|t)$/, r);
});

// Workflow text with CRLF line endings normalised, so the anchored patterns
// below hold on any checkout.
const workflow = (name: string) => readFileSync(new URL(`../../.github/workflows/${name}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

test("an automatic deploy follows only a green Agent CI run on main for main's current head", () => {
  // The kit checks out github.sha; a stale CI completion must not deploy a
  // newer, untested commit, and nothing but main may reach the host.
  const deploy = workflow("agent-deploy.yml");
  assert.match(deploy, /^on:\n  workflow_run:\n    workflows: \[Agent CI\]\n    types: \[completed\]\n    branches: \[main\]\n  workflow_dispatch:/m);
  assert.match(deploy, /^    concurrency:\n      group: agent-deploy-dev\n      cancel-in-progress: false$/m);
  assert.match(deploy, /allow-restart: \$\{\{ github\.event_name == 'workflow_dispatch' && inputs\.allow_restart \}\}/);
  assert.match(deploy, /adopt: \$\{\{ github\.event_name == 'workflow_dispatch' && inputs\.adopt \}\}/);
  const jobIf = (job: string) => deploy.match(new RegExp(`^  ${job}:\\n(?:    .*\\n)*?    if: >-\\n((?:      .*\\n)+)`, "m"))?.[1]?.replace(/\s+/g, " ").trim();
  const automatic = "(github.event_name == 'workflow_dispatch' || (github.event.workflow_run.conclusion == 'success' && github.event.workflow_run.head_branch == 'main' && github.event.workflow_run.head_sha == github.sha))";
  // The change-set preview runs under the deploy's conditions on every path
  // that can reach it, allow_restart dispatches included, and the deploy
  // needs it to pass: a skipped preview never deploys.
  assert.equal(jobIf("preview"),
    `github.ref == 'refs/heads/main' && needs.changed.outputs.deploy == 'true' && ${automatic}`);
  assert.equal(jobIf("deploy"),
    `github.ref == 'refs/heads/main' && !cancelled() && needs.changed.result == 'success' && needs.changed.outputs.deploy == 'true' && needs.preview.result == 'success' && ${automatic}`);
  assert.doesNotMatch(deploy, /needs\.preview\.result == 'skipped'/);
  // A replacement is allowed only by an allow_restart dispatch, and only
  // through this env on the run step.
  assert.match(deploy, /- name: Preview the compute stack change set\n        env:\n          PREVIEW_ALLOW_REPLACEMENT: \$\{\{ github\.event_name == 'workflow_dispatch' && inputs\.allow_restart && 'true' \|\| 'false' \}\}\n        run: agent\/preview-change-set\.sh run /);
  assert.equal(deploy.match(/PREVIEW_ALLOW_REPLACEMENT:/g)?.length, 1);
  assert.doesNotMatch(deploy, /^on:\n(?:.*\n)*?  push:/m);
  // Agent CI must run on every push to main, or an unrelated main commit
  // would never get its own run and so never deploy.
  const ci = workflow("agent-ci.yml");
  assert.match(ci, /^on:\n  push:\n    branches: \[main\]\n  workflow_dispatch:/m);
  assert.doesNotMatch(ci, /^    paths:/m);
});

test("an automatic deploy runs only when agent/ or its workflows changed, over AGENT_CI_PREFIXES", () => {
  // Landings elsewhere must neither assume the deploy role nor displace a
  // queued allow_restart dispatch from the concurrency group.
  const deploy = workflow("agent-deploy.yml");
  assert.match(deploy, /^jobs:\n(?:  #.*\n)*  changed:\n/m);
  assert.match(deploy, /^  preview:\n    needs: changed\n/m);
  assert.match(deploy, /^  deploy:\n    needs: \[changed, preview\]\n/m);
  // The concurrency group belongs to the deploy job alone.
  const changedJob = deploy.slice(deploy.indexOf("\n  changed:\n"), deploy.indexOf("\n  preview:\n"));
  const previewJob = deploy.slice(deploy.indexOf("\n  preview:\n"), deploy.indexOf("\n  deploy:\n"));
  assert.doesNotMatch(changedJob, /concurrency:/);
  assert.doesNotMatch(previewJob, /concurrency:|layered-agent-deploy decide/);
  // The deploy's own role, environment and region (the reusable workflow's
  // defaults), and the change set is deleted whatever happened.
  assert.match(previewJob, /^    environment: agent-deploy$/m);
  assert.match(previewJob, /^          role-to-assume: \$\{\{ vars\.LAYERED_AGENT_DEPLOY_ROLE_ARN \}\}$/m);
  assert.match(previewJob, /^          aws-region: us-west-2$/m);
  assert.match(previewJob, /run: agent\/preview-change-set\.sh run "preview-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}"/);
  assert.match(previewJob, /- name: Delete the change set\n        if: always\(\)\n        run: agent\/preview-change-set\.sh delete /);
  // The baseline is one lookup of agent-deployed artifacts, newest picked by
  // created_at (the API does not list newest first), not a search over runs
  // and their jobs.
  assert.ok(changedJob.includes("gh api --paginate "));
  assert.ok(changedJob.includes("actions/artifacts?name=agent-deployed&per_page=100"));
  assert.doesNotMatch(changedJob, /per_page=20\b/);
  assert.doesNotMatch(changedJob, /per_page=1\b/);
  assert.ok(changedJob.includes('"\\(.created_at) \\(.workflow_run.head_sha)"'));
  assert.ok(changedJob.includes("| sort | tail -1 | cut -d' ' -f2"));
  assert.doesNotMatch(changedJob, /--slurp/);
  assert.ok(!changedJob.includes("/jobs"));
  assert.doesNotMatch(changedJob, /[?&]page=|\$page|for page/);
  // The record job leaves that artifact after each successful deploy, outside
  // the concurrency group.
  const recordStart = deploy.indexOf("\n  record:\n");
  assert.ok(recordStart > 0);
  const recordJob = deploy.slice(recordStart);
  assert.match(recordJob, /^    needs: deploy$/m);
  assert.match(recordJob, /^    if: needs\.deploy\.result == 'success'$/m);
  assert.match(recordJob, /uses: actions\/upload-artifact@/);
  assert.match(recordJob, /^          name: agent-deployed$/m);
  assert.doesNotMatch(recordJob, /concurrency:/);
  // The path list mirrors flows/lib/remote-ci.mjs. Read as text: that module
  // imports yaml, which this package does not install.
  const remoteCi = readFileSync(new URL("../../flows/lib/remote-ci.mjs", import.meta.url), "utf8");
  const prefixes = JSON.parse(remoteCi.match(/^export const AGENT_CI_PREFIXES = (\[.*\]);$/m)?.[1] ?? "null") as string[];
  assert.ok(Array.isArray(prefixes) && prefixes.length > 0);
  const paths = changedJob.match(/^ +paths=\((.*)\)$/m)?.[1]?.split(" ");
  assert.deepEqual([...(paths ?? [])].sort(), [...prefixes].sort());
});

test("the host keeps main's parameter names and the Jev key stays optional", () => {
  assert.match(agent.envFile, /^ANTHROPIC_KEY_PARAM=\/relaycast-agent-host\/dev\/anthropic-api-key$/m);
  assert.match(agent.envFile, /^TYPESAFE_KEY_PARAM=\/relaycast-agent-host\/dev\/typesafe-api-key$/m);
  // A value with spaces is single-quoted by layered-agent; load-secrets.sh sources the file in bash and splits on the spaces.
  assert.match(agent.envFile, /^EXPORT_SECRETS='TYPESAFE_API_KEY:TYPESAFE_KEY_PARAM FLOWS_CLOUD_TOKEN:FLOWS_CLOUD_TOKEN_PARAM FLOWS_CLOUD_URL:FLOWS_CLOUD_URL_PARAM'$/m);
  assert.match(agent.envFile, /^FLOWS_CLOUD_TOKEN_PARAM=\/relaycast-agent-host\/dev\/flows-cloud-token$/m);
  assert.match(agent.envFile, /^FLOWS_CLOUD_URL_PARAM=\/relaycast-agent-host\/dev\/flows-cloud-url$/m);
  assert.match(agent.envFile, /^RELAY_NAME=layered-relay-desktop$/m);
});

test("the sessions MCP token: read by parameter name only, never exported, never in user data", () => {
  // The value reaches ~/.claude.json through kit/relay-sessions.sh, not secrets.env.
  // The shared relaycast-agent-host-dev role (relaycast-cloud) grants the read.
  assert.match(agent.envFile, /^RELAY_SESSIONS_PARAM=\/relaycast-agent-host\/dev\/relay-sessions-token-relay-desktop$/m);
  assert.deepEqual(agent.secrets.find((s) => s.envVar === "RELAY_SESSIONS_PARAM"), { envVar: "RELAY_SESSIONS_PARAM", param: "/relaycast-agent-host/dev/relay-sessions-token-relay-desktop" });
  assert.ok(agent.files.some((f) => f.name === "relay-sessions.sh" && f.source === "kit"));
  const script = gunzipSync(Buffer.from(userDataBase64(agent, read), "base64")).toString("utf8");
  assert.doesNotMatch(script, /cld_|Bearer [A-Za-z0-9]/);
});

test("the env file sources cleanly in bash", () => {
  const out = execFileSync("bash", ["-c", `set -eu; ${agent.envFile}\nprintf '%s|%s' "$LANE_APT_PACKAGES" "$LEDGER_COUNTERPARTS"`]).toString();
  assert.equal(out, "pkg-config libgtk-4-dev libssl-dev libdbus-1-dev xvfb shellcheck|macos=app-ui gtk=app-ui");
});

test("network ids are read from the params relaycast-cloud's agent-host stack publishes", () => {
  assert.deepEqual(networkParams(agent.names.platformPrefix), {
    subnetId: "/relaycast-agent-host/dev/network/private-subnet-id",
    securityGroupId: "/relaycast-agent-host/dev/network/host-security-group-id",
  });
});

test("tick.sh's refresh marks a candidate whose version, or a beta's base version, is already published as stale:version-taken", () => {
  const tick = readFileSync(new URL("../tick.sh", import.meta.url), "utf8");
  const refresh = tick.slice(0, tick.indexOf('[ "${1:-}" = --refresh ] && exit 0'));
  assert.match(refresh, /repos\/AgentWorkforce\/relay-desktop-releases\/releases\?per_page=100" --paginate --jq '\.\[\]\.tag_name'/);
  assert.match(refresh, /\("v" \+ \.version\) as \$t/);
  assert.match(refresh, /\.status == "pending" or \.status == "ready"/);
  assert.match(refresh, /\.version \| contains\("-beta\."\)/);
  assert.match(refresh, /split\("-beta\."\)\[0\]/);
  assert.match(refresh, /\("v" \+ \(\.version \| split\("-beta\."\)\[0\]\)\) as \$b/);
  assert.match(refresh, /\.status = "stale:version-taken"/);
});

test("release.sh promotes only candidates whose status is exactly ready", () => {
  const release = readFileSync(new URL("../release.sh", import.meta.url), "utf8");
  assert.match(release, /select\(\.channel == \$ch and \(\(\.profile \/\/ ""\) == \$pr\) and \.status == "ready"\)\] \| sort_by\(\.dispatched_at\)/);
  assert.doesNotMatch(release, /\.status\s*\|\s*(test|startswith|contains)/);
});

test("release.sh passes --profile as -f profile= to both the prepare and the promote dispatch", () => {
  const release = readFileSync(new URL("../release.sh", import.meta.url), "utf8");
  const pflag = 'pflag=(); [ -z "$profile" ] || pflag=(-f "profile=$profile")';
  assert.equal(release.split(pflag).length - 1, 2, "pflag is built for each dispatch");
  assert.match(release, /^  gh workflow run desktop-release\.yml .*-f prepare-only=true \$\{pflag\[@\]\+"\$\{pflag\[@\]\}"\}$/m);
  assert.match(release, /^  gh workflow run desktop-promote\.yml .*-f channel="\$channel" \$\{pflag\[@\]\+"\$\{pflag\[@\]\}"\}$/m);
  // Without --run, both the resume and the ready selection filter by profile.
  assert.equal(release.split('((.profile // "") == $pr)').length - 1, 2);
});

test("release.sh ends a profile release before the released-v tag", () => {
  const release = readFileSync(new URL("../release.sh", import.meta.url), "utf8");
  const exit = release.indexOf('if [ -n "$profile" ]; then\n  # Version strings are shared across profiles');
  assert.ok(exit > 0, "the profile branch exists");
  const branch = release.slice(exit, release.indexOf("\nfi\n", exit));
  assert.match(branch, /\.status = "published"/);
  assert.match(branch, /\n  exit 0$/);
  assert.ok(exit < release.indexOf("tag=released-v$version"), "the profile exit comes before the tag");
  assert.ok(exit < release.indexOf("promoted-changelog-v$version"), "and before the changelog download");
});

test("release.sh refuses --run --profile on a row without a profile unless it is ready or pending", () => {
  const release = readFileSync(new URL("../release.sh", import.meta.url), "utf8");
  assert.match(release, /rowstatus=\$\(jq -r --argjson id "\$run_id" '\[\.\[\] \| select\(\.run_id == \$id\)\] \| last \| \.status \/\/ empty'/);
  const branch = release.slice(release.indexOf('elif [ -z "$rowprofile" ] && [ -n "$profile" ]; then'), release.indexOf("\n  fi\nfi\n"));
  assert.match(branch, /case "\$rowstatus" in\n\s+ready\|pending\) ;;\n\s+\*\) echo "[^"]*refusing --profile[^"]*" >&2; exit 1 ;;\n\s+esac/);
  // Nothing is written to the row before validation.
  assert.doesNotMatch(branch, /write_state/);
});

test("release.sh records a command-line profile only with the promote and drops it when the promote fails", () => {
  const release = readFileSync(new URL("../release.sh", import.meta.url), "utf8");
  // The only writes of .profile: the promoting write (marked cli) and the null of a new row.
  assert.match(release, /\.status = "promoting" \| [^\n]*\n\s+\| if \$pr == "" or \.profile == \$pr then \. else \.profile = \$pr \| \.profile_source = "cli" end\)'/);
  assert.equal(release.match(/\.profile = \$pr/g)?.length, 1);
  const reset = release.slice(release.indexOf("    completed/*)"), release.indexOf("exit 1 ;;", release.indexOf("    completed/*)")));
  assert.match(reset, /\.status = "ready"/);
  assert.match(reset, /if \.profile_source == "cli" then del\(\.profile\) \| del\(\.profile_source\) else \. end/);
});

test("review.sh runs main's flows/ against a ref whose flows/ predates them, and undoes it before the next round", () => {
  const review = String(source("review.sh"));
  // The overlay: main's flows/ in the tree, the index left at HEAD, tracked
  // differences assume-unchanged and new files in a marked exclude block.
  const overlay = review.slice(review.indexOf("\noverlay_main_flows() {"), review.indexOf("\n}\n", review.indexOf("\noverlay_main_flows() {")));
  assert.ok(overlay.length > 0, "overlay_main_flows is defined");
  assert.ok(overlay.includes("git checkout -q origin/main -- flows && git reset -q -- flows"));
  assert.ok(overlay.includes("git diff -z --name-only -- flows | xargs -0 -r git update-index --assume-unchanged --"));
  assert.ok(overlay.includes("git ls-files -z --others --exclude-standard -- flows"));
  assert.ok(overlay.includes("drop_overlay_block"), "the exclude block is regenerated, not appended to");
  assert.ok(overlay.includes(`echo "note: this ref's flows/ predates main's; the round runs main's flows/ ($main) against the ref's tree"`));
  // Sourcing the script (the tests) stops after the definitions.
  assert.match(review, /^if \(return 0 2>\/dev\/null\); then return 0; fi$/m);
  // Only for a ref that does not edit flows/: the --trust-ref-flows refusal
  // keeps its branch and comes first; the overlay is its elif.
  assert.match(review,
    /^if ! git diff --quiet "\$base" HEAD -- flows; then\n(?:  .*\n)*?    exit 2\n  fi\nelif ! git diff --quiet "\$base" origin\/main -- flows; then\n(?:  #.*\n)*  overlay_main_flows \|\|/m);
  assert.ok(review.indexOf("\n  overlay_main_flows ||") < review.indexOf("\nlock=$(sha256sum flows/package-lock.json"), "the lockstamp hashes the overlaid lockfile");
  // The next round starts clean: assume-unchanged bits survive reset --hard,
  // so they are cleared before it.
  assert.match(review, /^clear_main_flows_overlay\(\) \{\n  git ls-files -v -z \| sed -z -n 's\/\^\[a-z\] \/\/p' \| xargs -0 -r git update-index --no-assume-unchanged -- &&\n    drop_overlay_block\n\}$/m);
  assert.match(review, /^fresh_checkout\(\) \{\n  clear_main_flows_overlay && git reset -q --hard && git clean -qfdx -e flows\/node_modules -e \.relayflow &&\n    git checkout -q --detach FETCH_HEAD && git reset -q --hard FETCH_HEAD && git clean -qfdx -e flows\/node_modules -e \.relayflow\n\}$/m);
  assert.match(review, /^if ! fresh_checkout; then$/m);
  assert.ok(overlay.includes("--no-renames --diff-filter=D"), "renamed files are listed as deletions");
  assert.ok(overlay.includes("for ignored in .relayflow/ .relayflowd/ flows/node_modules/") && overlay.includes(`git check-ignore -q "\${ignored}x" || echo "/\$ignored"`), "flows state and packages the ref does not ignore go in the block");
  // The flows' tamper check is blind to the overlay, so review.sh hashes
  // flows/ itself around the round and withholds a report when it changed.
  const hashed = review.indexOf("\n  flows_before=$(flows_fingerprint)");
  const run = review.indexOf("\n  cloud_round 2>&1 | tee");
  const refused = review.indexOf(`[ "$(flows_fingerprint)" != "$flows_before" ]`);
  assert.ok(review.indexOf("\n  overlay_main_flows ||") < hashed && hashed < run && run < refused, "hashed after the overlay, checked after the run");
  const ciRun = review.lastIndexOf("env -u ANTHROPIC_API_KEY -u TYPESAFE_API_KEY -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL flows/node_modules/.bin/flows run --local-agent flows/ci.flow.ts");
  assert.ok(ciRun > hashed && ciRun < refused, "the ci-only run sits between the hash and the check");
  // A refused round leaves no saved state, or an identical rerun reports it green unreviewed.
  const discard = review.slice(review.indexOf("\ndiscard_round_state() {"), review.indexOf("\n}\n", review.indexOf("\ndiscard_round_state() {")));
  assert.ok(discard.length > 0, "discard_round_state is defined");
  for (const flow of ["adversarial-review", "context-review", "ci", "sized-review", "plan", "cloud"]) assert.ok(discard.includes(flow), `${flow} state is discarded`);
  assert.ok(discard.includes('rm -rf -- "${root:?}/$flow" "${root:?}/$flow.state.json"'));
  assert.ok(discard.includes('git update-ref -d "refs/relayflow/$flow/reviewed"'));
  assert.ok(discard.includes("rm -rf -- .relayflowd"));
  assert.ok(!/lockstamp|logs|\.lock/.test(discard.replace(/^#.*$/gm, "")), "the lockstamp, logs and lock file are kept");
  assert.ok(review.includes("\n  discard_round_state\n  exit 1\nfi\n# A refused round has no report: the refusal above is its outcome.\n[ \"$status\" != 3 ] || exit 3\nreport="), "the refusal discards the state before exit 1; a refused Cloud round exits 3 with no report");
  assert.ok(refused < review.indexOf("\n  discard_round_state\n  exit 1"));
  assert.ok(refused < review.indexOf("\nreport=$RELAYFLOW_STATE_DIR/$flow/report.md"), "checked before the report is read");
  assert.ok(review.slice(refused).includes("refusing this round's result: flows/ changed while the round ran"));
});

test("review.sh --ci-only runs the CI flow alone, without keys, after stopping leftover fleets", () => {
  const review = readFileSync(fileURLToPath(new URL("../review.sh", import.meta.url)), "utf8");
  assert.ok(review.includes("--ci-only) ci_only=true; shift ;;"), "--ci-only is parsed");
  assert.ok(review.includes("--depth does not apply with --ci-only"), "--depth is refused with it");
  assert.ok(review.includes("--lead and --lenses do not apply with --ci-only"), "--lead and --lenses are refused with it");
  // It needs no Cloud token: only a review round checks for one.
  assert.ok(review.includes('\nparse_args "$@"\n[ "$ci_only" = true ] || cloud_credentials\n'));
  // The last ci.flow.ts line is the --ci-only branch's; cloud_round's background CI is stripped the same way.
  const ciLines = review.split("\n").filter((l) => l.includes("flows/ci.flow.ts --input"));
  assert.equal(ciLines.length, 2);
  assert.ok(ciLines.every((l) => l.trimStart().startsWith("env -u ANTHROPIC_API_KEY -u TYPESAFE_API_KEY -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL ")), "no ci run inherits a key or the Cloud token");
  const ciLine = ciLines[1];
  assert.ok(ciLine, "the ci run line exists");
  assert.ok(ciLine.trimStart().startsWith("env -u ANTHROPIC_API_KEY -u TYPESAFE_API_KEY -u FLOWS_CLOUD_TOKEN -u FLOWS_CLOUD_URL "), "inherited keys and the Cloud token are stripped");
  assert.ok(!ciLine.includes("secret("), "no key is passed to ci");
  assert.ok(review.includes(`'{base: "origin/main", github: $final}'`), "ci gets base and github");
  assert.ok(review.includes("flow=ci") && review.includes("flow=sized-review"));
  assert.equal(review.match(/^  log=/gm)?.length, 2, "log is set in both branches");
  const stop = review.indexOf("for f in sized-review adversarial-review context-review ci; do");
  assert.ok(stop > 0 && review.includes('node flows/lib/fleet.mjs stop "$RELAYFLOW_STATE_DIR/$f/fleet" || true'));
  assert.ok(stop < review.lastIndexOf(ciLine) && stop < review.indexOf("\n  cloud_round 2>&1 | tee"), "fleets are stopped before the ci run and the review round");
  // The round's own exit status (3 refused, 1 red), not tee's.
  assert.ok(review.includes('\n  cloud_round 2>&1 | tee "$RELAYFLOW_STATE_DIR/logs/sized-review.log"\n  status=${PIPESTATUS[0]}\n'), "status is cloud_round's, through the pipe");
  // A review round is followed by --ci-only --final, never a second review.
  assert.ok(review.includes("\nagain='--ci-only --final'\nif [ \"$ci_only\" = true ]; then") && !review.includes("again='--final'"));
  assert.ok(review.includes('echo "round green: run again with $again for the GitHub gate before landing"'));
});

test("review.sh's overlay leaves no tracked or untracked trace, and the next round starts clean", () => {
  const script = fileURLToPath(new URL("../review.sh", import.meta.url));
  const out = execFileSync("bash", ["-c", `set -eu
cd "$(mktemp -d)"
g() { git -c user.email=t@t -c user.name=t "$@"; }
git init -q -b main origin && cd origin
mkdir -p flows/lib && echo old > flows/lib/remote-ci.mjs && echo gone > flows/lib/dropped.mjs && echo same-content-to-be-renamed-by-main-so-detection-fires > flows/lib/renamed-old.mjs && echo app > app.txt
git add -A && g commit -qm base && git branch oldbase
echo new > flows/lib/remote-ci.mjs && echo flow > flows/sized-review.flow.ts && git rm -q flows/lib/dropped.mjs
printf '.relayflow/\n.relayflowd/\nflows/node_modules/\n' > .gitignore
git mv flows/lib/renamed-old.mjs flows/lib/renamed-new.mjs
git add -A && g commit -qm newer
cd .. && git clone -q origin review && cd review
git checkout -q -b hotfix origin/oldbase && echo fix >> app.txt && g commit -qam fix
. "$1"
# Under set -e a && chain fails the script only on its last term: each
# condition goes through check, which fails on any.
check() { "$@" || { echo "FAILED: $*" >&2; exit 1; }; }
for round in 1 2; do
  check clear_main_flows_overlay
  git reset -q --hard HEAD
  git clean -qfdx
  check test ! -e flows/sized-review.flow.ts
  check [ "$(cat flows/lib/remote-ci.mjs)" = old ]
  check [ -z "$(git ls-files -v | grep '^[a-z]' || true)" ]
  overlay_main_flows >/dev/null
  check [ "$(cat flows/sized-review.flow.ts)" = flow ]
  check [ "$(cat flows/lib/remote-ci.mjs)" = new ]
  check test ! -e flows/lib/dropped.mjs
  check test ! -e flows/lib/renamed-old.mjs
  check [ -e flows/lib/renamed-new.mjs ]
  # The old base's .gitignore predates the flows entries: the flows' own
  # state and packages must still not count as untracked files.
  check test ! -e .gitignore
  mkdir -p .relayflow .relayflowd flows/node_modules && touch .relayflow/s .relayflowd/s flows/node_modules/p
  check [ -z "$(git ls-files --others --exclude-standard)" ]
  check [ -z "$(git status --porcelain)" ]
  rm -rf .relayflow .relayflowd flows/node_modules
  printf '%s|%s|%s|%s;' "$(git diff --name-only)" "$(git ls-files --others --exclude-standard)" "$(git status --porcelain)" "$(grep -c "flows/ overlay" .git/info/exclude)"
done
# git cannot see an edit to an overlaid file; flows_fingerprint must.
before=$(flows_fingerprint)
echo tampered >> flows/lib/remote-ci.mjs
check [ "$(flows_fingerprint)" != "$before" ]
check [ -z "$(git status --porcelain)" ]
# A refused round discards what the flows saved, and only that.
export RELAYFLOW_STATE_DIR="$PWD/.git/state"
mkdir -p "$RELAYFLOW_STATE_DIR"/{logs,adversarial-review,context-review,ci,sized-review} .relayflowd/runs
cd "$RELAYFLOW_STATE_DIR"
touch .lock flows.lockstamp logs/sized-review.log adversarial-review.state.json context-review.state.json sized-review/report.md
cd - >/dev/null
touch .relayflowd/relayflowd.sqlite3
git update-ref refs/relayflow/context-review/reviewed HEAD
discard_round_state 2>/dev/null
for gone in adversarial-review context-review ci sized-review adversarial-review.state.json context-review.state.json; do check test ! -e "$RELAYFLOW_STATE_DIR/$gone"; done
check test ! -e .relayflowd
check [ -z "$(git for-each-ref refs/relayflow)" ]
for kept in .lock flows.lockstamp logs/sized-review.log; do check test -e "$RELAYFLOW_STATE_DIR/$kept"; done
unset RELAYFLOW_STATE_DIR
# Round 3: another ref, main-based with a non-flows change, into the same clone.
(cd ../origin && git checkout -q -b other main && echo more >> app.txt && g commit -qam other && git checkout -q main)
git fetch -q origin -- other
fresh_checkout
check [ "$(git rev-parse HEAD)" = "$(git rev-parse FETCH_HEAD)" ]
check [ "$(cat flows/sized-review.flow.ts)" = flow ]
check [ "$(cat flows/lib/remote-ci.mjs)" = new ]
check [ -e flows/lib/renamed-new.mjs ]
check test ! -e flows/lib/renamed-old.mjs
# This ref's .gitignore covers the flows entries itself: no block lists them.
check grep -q '^.relayflow/' .gitignore
check [ -z "$(sed -n '/>>> review.sh/,/<<< review.sh/p' .git/info/exclude | grep -v 'review.sh' || true)" ]
mkdir -p .relayflow flows/node_modules && touch .relayflow/s flows/node_modules/p
check [ -z "$(git status --porcelain)" ]
rm -rf .relayflow flows/node_modules
printf '%s|%s|%s|%s;' "$(git ls-files -v | grep -c '^[a-z]' || true)" "$(grep -c "flows/ overlay" .git/info/exclude || true)" "$(git status --porcelain)" "$(git diff --name-only HEAD origin/main -- flows)"`, "bash", script]).toString();
  assert.equal(out, "|||2;|||2;0|0||;");
});

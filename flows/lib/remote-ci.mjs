// GitHub Actions half of flows/ci.flow.ts.
//
//   node flows/lib/remote-ci.mjs mobile <base-sha>                     print 1 when Mobile CI's paths changed
//   node flows/lib/remote-ci.mjs agent <base-sha>                      print 1 when Agent CI's paths changed (main: changed, but the workflow is not on main yet)
//   node flows/lib/remote-ci.mjs watch <base-sha> <desktop 0|1> <mobile 0|1> [agent 0|1]   (Desktop CI on every architecture)
//
// Work lands on main as one squash commit and this repository never opens
// pull requests (AGENTS.md), so there is no branch to run CI on. Instead:
//
// 1. Snapshot the working tree as a commit without touching HEAD, the index
//    or the files: tracked edits plus whatever is staged, so stage a new file
//    for GitHub to see it. Untracked files are never pushed.
// 2. Push the snapshot to a throwaway ci/<sha> branch and dispatch Desktop CI
//    there with the merge base as `base-sha`, so it picks the lanes a pull
//    request would, and with `full-matrix`, so every architecture runs and a
//    green gate leaves evidence main's push can reuse. Mobile CI runs too when
//    its own path filter matches.
// 3. Wait (up to two hours) for exactly the dispatched runs, print the failed
//    jobs' log tails and then one verdict line per run, delete the branch, and
//    exit 1 unless every run passed. A Desktop CI run whose only red jobs are
//    the macOS x64 lanes passes (see DEFERRED_LANES): their results are
//    reported but do not block landing, and once only they are still queued
//    or running the gate stops waiting for them. Main's push CI and the release
//    candidate still require them, so a red x64 lane is fixed forward before
//    the next release. A SIGTERM or SIGINT (the fleet stopping this step)
//    deletes the branch too, and each watch first sweeps snapshot branches an
//    earlier, harder kill left behind.
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { MACHINE_LOCAL } from "./changes.ts";

// Failed-job logs run to megabytes; the default 1 MB buffer would throw them away.
const sh = (cmd, args, env) =>
  execFileSync(cmd, args, {
    encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, ...env }, maxBuffer: 256 * 1024 * 1024,
  }).trim();
const git = (args, env) => sh("git", args, env);
const gh = (...args) => sh("gh", args);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
// The listing is polled for up to two hours; a few failures in a row are a
// passing API hiccup, not a verdict.
const LISTING_RETRIES = 10;

/**
 * A commit of HEAD plus the working tree's tracked and staged changes, and
 * any untracked, non-ignored files under `include`; HEAD, the index and the
 * files are untouched.
 */
export function snapshot(include = []) {
  const scratch = mkdtempSync(join(tmpdir(), "relay-ci-index-"));
  try {
    const index = join(scratch, "index");
    const real = git(["rev-parse", "--git-path", "index"]);
    if (existsSync(real)) {
      copyFileSync(real, index);
      // Keep the index's own timestamp: git trusts an entry's stat data only
      // when the file is older than the index, so a fresh copy would hide an
      // edit made in the same instant the real index was written.
      const { atime, mtime } = statSync(real);
      utimesSync(index, atime, mtime);
    }
    const env = { GIT_INDEX_FILE: index };
    git(["add", "--update", "--", "."], env);
    const extra = include.length
      ? git(["ls-files", "--others", "--exclude-standard", "-z", "--", ...include], env).split("\0").filter(Boolean)
      : [];
    if (extra.length) git(["add", "--", ...extra], env);
    const tree = git(["write-tree"], env);
    return git(["commit-tree", tree, "-p", "HEAD", "-m", "CI snapshot (flows/ci.flow.ts)"]);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/**
 * A workflow's own push and pull_request `paths:` filters (Mobile CI's, or
 * Agent CI's), read from the workflow so the two cannot drift. Only plain
 * prefixes (`dir/**`) and exact files are understood; any other glob is
 * refused rather than misread.
 */
export function mobilePrefixes(workflowText, name = "mobile-ci.yml") {
  const on = parse(workflowText).on ?? {};
  const globs = [...(on.push?.paths ?? []), ...(on.pull_request?.paths ?? [])];
  if (!globs.length) throw new Error(`${name} has no push or pull_request paths`);
  return [...new Set(globs.map((glob) => {
    const prefix = glob.replace(/\*\*$/, "");
    if (/[*?[\]!]/.test(prefix)) throw new Error(`${name} path ${glob} is not a plain prefix; teach remote-ci.mjs to read it`);
    return prefix;
  }))];
}

export const touches = (paths, prefixes) => paths.some((path) => prefixes.some((prefix) => path.startsWith(prefix)));

/**
 * Why a snapshot must not leave this machine, or "" when it may. The remote
 * is public: machine-local state never goes, and the tree must pass the same
 * open-source check the `always` job runs, before the push rather than beside it.
 */
export function refuseToPublish(sha, root = ".") {
  const files = git(["ls-tree", "-r", "--name-only", sha]).split("\n");
  const local = files.filter((file) => MACHINE_LOCAL.some((dir) => file === dir || file.startsWith(`${dir}/`) || file.includes(`/${dir}/`)));
  if (local.length) return `the snapshot holds machine-local state (${local.slice(0, 5).join(", ")}); unstage it`;
  try {
    sh("bash", [join(root, "scripts/check-open-source.sh")]);
  } catch (error) {
    return `scripts/check-open-source.sh failed, so nothing was pushed:\n${String(error.stderr ?? "").trim()}`;
  }
  return "";
}

const STALE_SNAPSHOT_SECONDS = 3 * 60 * 60;

/**
 * Snapshot branches this checkout left behind: named `ci/<first 12 of their
 * own sha>`, the commit made here (so present locally), and older than any
 * watch can last. Another agent's in-flight snapshot, or a teammate's
 * ci/<topic> branch, is never among them.
 */
export function staleSnapshots(lsRemote, now = Date.now() / 1000) {
  return lsRemote
    .split("\n")
    .map((line) => line.split(/\s+/))
    .filter(([sha, ref]) => {
      if (!sha || ref !== `refs/heads/ci/${sha.slice(0, 12)}`) return false;
      try {
        return now - Number(git(["show", "-s", "--format=%ct", sha])) > STALE_SNAPSHOT_SECONDS;
      } catch {
        return false; // not made here
      }
    })
    .map(([, ref]) => ref.slice("refs/heads/".length));
}

const deleteBranch = (branch) => {
  try {
    git(["push", "--quiet", "origin", "--delete", branch]);
  } catch (error) {
    console.log(`could not delete ${branch}: ${error.message}`);
  }
};

/** The newest run per dispatched workflow, among runs created after `since`. */
export function latestRuns(runs, since, workflows) {
  const byWorkflow = new Map();
  for (const run of runs) {
    if (run.createdAt < since || !workflows.includes(run.workflowName)) continue;
    const seen = byWorkflow.get(run.workflowName);
    if (!seen || run.databaseId > seen.databaseId) byWorkflow.set(run.workflowName, run);
  }
  return [...byWorkflow.values()];
}

// The macOS x64 lanes, as desktop-ci.yml names them: macos-package,
// macos-swift-tests and macos-core-tests on the x64 row of the macOS matrix.
// They are a release-time check, not a landing gate.
export const DEFERRED_LANES = ["macOS package (x64)", "macOS Swift tests (x64)", "macOS core tests (x64)"];
// The same lanes' arm64 rows, which do count.
const ARM64_LANES = DEFERRED_LANES.map((name) => name.replace("(x64)", "(arm64)"));
// The "macOS <arch>" completion gates read each lane's result over the whole
// matrix, so one red x64 lane turns both of them red too.
export const MACOS_COMPLETION = ["macOS arm64", "macOS x64"];
// Jobs that need the macOS lanes, x64 rows included, so they cannot finish
// before the deferred lanes do.
const AFTER_DEFERRED = [...MACOS_COMPLETION, "Record successful snapshot gate"];
// The Windows and Linux completion gates need every other platform lane, so
// once they are listed and finished, every lane that counts has finished.
export const OTHER_COMPLETION = ["Windows arm64", "Windows x64", "Linux arm64", "Linux x64"];

const DEFERRED_NOTE = " (macOS x64 lanes deferred)";
// A job listed without a status (as the tests write them) has finished.
const finished = (job) => (job.status ?? "completed") === "completed";
const green = (job) => job.conclusion === "success" || job.conclusion === "skipped";

/**
 * True while a Desktop CI run is in progress but every job that counts has
 * finished: the Windows and Linux completion gates are listed, every listed
 * job other than the deferred lanes and the jobs after them has finished, and
 * a deferred lane is still queued or running. The gate stops waiting then.
 */
export function settled(jobs) {
  const names = new Set(jobs.map((job) => job.name));
  return OTHER_COMPLETION.every((name) => names.has(name))
    && jobs.every((job) => finished(job) || DEFERRED_LANES.includes(job.name) || AFTER_DEFERRED.includes(job.name))
    && jobs.some((job) => DEFERRED_LANES.includes(job.name) && !finished(job));
}

/**
 * True when the macOS completion gates' own checks hold on the arm64 rows, so
 * a red gate can only be the x64 rows' doing: each arm64 lane row succeeded,
 * or was skipped along with its x64 row (the lane was not selected), and
 * packaging ran only beside the Swift tests, as the gates require.
 */
function arm64Clean(jobs) {
  const conclusion = (name) => jobs.find((job) => job.name === name)?.conclusion ?? "skipped";
  const ran = (i) => conclusion(ARM64_LANES[i]) !== "skipped" || conclusion(DEFERRED_LANES[i]) !== "skipped";
  return ARM64_LANES.every((name, i) => conclusion(name) === "success" || (conclusion(name) === "skipped" && !ran(i)))
    && (!ran(DEFERRED_LANES.indexOf("macOS package (x64)")) || ran(DEFERRED_LANES.indexOf("macOS Swift tests (x64)")));
}

/**
 * The gate's verdict on one run, given the latest attempt's jobs ({name,
 * status, conclusion}) for a failed or still running Desktop CI run. Such a
 * run passes only when a macOS x64 lane failed or is still running and every
 * other job succeeded or was skipped, except that:
 * - a macOS completion gate may be red, but only when its checks hold on the
 *   arm64 rows (see arm64Clean), so the x64 rows alone turned it red;
 * - in a running run, the jobs after the deferred lanes may still be waiting.
 * Anything else red, cancelled or timed out, and any other cancelled or
 * failed run, still fails the gate.
 */
export function verdict(run, jobs = []) {
  if (run.conclusion === "success") return { pass: true, note: "" };
  const running = (run.status ?? "completed") !== "completed";
  if (run.workflowName !== "Desktop CI" || (!running && run.conclusion !== "failure")) return { pass: false, note: "" };
  const deferred = jobs.filter((job) => DEFERRED_LANES.includes(job.name));
  const x64Only = jobs
    .filter((job) => MACOS_COMPLETION.includes(job.name))
    .every((job) => !finished(job) || green(job) || arm64Clean(jobs));
  const pass = deferred.some((job) => running ? !finished(job) || !green(job) : !green(job))
    && x64Only
    && jobs.every((job) => DEFERRED_LANES.includes(job.name) || MACOS_COMPLETION.includes(job.name)
      || (running && AFTER_DEFERRED.includes(job.name) && !finished(job))
      || (finished(job) && green(job)));
  return pass ? { pass: true, note: DEFERRED_NOTE } : { pass: false, note: "" };
}

/**
 * One verdict per run. Jobs are listed (by `listJobs(runId)`) only for a
 * Desktop CI run that failed or is still running; when the listing fails the
 * verdict falls back to the run alone, which fails such a run.
 */
export function gateVerdicts(runs, listJobs, log = console.log) {
  return runs.map((run) => {
    const running = (run.status ?? "completed") !== "completed";
    if (run.workflowName !== "Desktop CI" || (!running && run.conclusion !== "failure")) return verdict(run);
    try {
      return verdict(run, listJobs(run.databaseId));
    } catch (error) {
      log(`could not list the jobs of ${run.url}: ${error.message}`);
      return verdict(run);
    }
  });
}

// Every job of the run's latest attempt; a Desktop CI run has more than one
// page of them.
const runJobs = (id) => gh(
  "api", "--paginate", `repos/{owner}/{repo}/actions/runs/${id}/jobs?filter=latest&per_page=100`,
  "--jq", ".jobs[] | {id, name, status, conclusion}",
).split("\n").filter(Boolean).map((line) => JSON.parse(line));

const changedSince = (base) => git(["diff", "--no-renames", "--name-only", base]).split("\n").filter(Boolean);
const mobileChanged = (base) =>
  touches(changedSince(base), mobilePrefixes(readFileSync(".github/workflows/mobile-ci.yml", "utf8")));
// The paths whose change asks the gate for an Agent CI run. Agent CI itself
// runs on every push to main (Agent Deploy follows a green run of main's
// current head), so its workflow file carries no path filter to read.
export const AGENT_CI_PREFIXES = ["agent/", ".github/workflows/agent-ci.yml", ".github/workflows/agent-deploy.yml"];
// The layered agent's stack tests (agent/) run only on GitHub, where its
// package is installed from GitHub Packages; the gate runs them before a
// landing, as main's own push does after.
// "1" to dispatch, "0" when its paths did not change, "main" when they did
// but GitHub cannot dispatch the workflow yet: it dispatches by file name
// only once the file exists on the default branch, so for the change that
// adds it main's push runs it after landing, and the report says so.
const agentChanged = (base) => {
  if (!touches(changedSince(base), AGENT_CI_PREFIXES)) return "0";
  try {
    git(["cat-file", "-e", "origin/main:.github/workflows/agent-ci.yml"]);
    return "1";
  } catch {
    return "main";
  }
};

async function watch(base, desktop, mobile, agent) {
  if (!/^[0-9a-f]{40}$/.test(base ?? "")) throw new Error(`watch needs the merge base as a full sha, got ${base}`);
  const workflows = [...(desktop ? ["Desktop CI"] : []), ...(mobile ? ["Mobile CI"] : []), ...(agent ? ["Agent CI"] : [])];
  if (!workflows.length) throw new Error("nothing to dispatch");
  // Snapshots a SIGKILL stopped before they could clean up. Best effort: a
  // failed sweep must not stop this run.
  try {
    for (const stale of staleSnapshots(git(["ls-remote", "--heads", "origin", "ci/*"]))) deleteBranch(stale);
  } catch (error) {
    console.log(`could not sweep old snapshot branches: ${error.message}`);
  }
  const sha = snapshot();
  const refused = refuseToPublish(sha);
  if (refused) throw new Error(refused);
  const branch = `ci/${sha.slice(0, 12)}`;
  git(["push", "--quiet", "origin", `${sha}:refs/heads/${branch}`]);
  const stop = (signal) => {
    deleteBranch(branch);
    process.exit(signal === "SIGINT" ? 130 : 143);
  };
  process.once("SIGTERM", stop);
  process.once("SIGINT", stop);
  // The jobs of a Desktop CI run the gate stopped waiting for: only its
  // deferred lanes (and the jobs after them) were still queued or running.
  let early = null;
  // Every other exit below, including a thrown error, goes through this finally.
  try {
    // Generous: runs are matched by the snapshot's own sha, and a local clock
    // running ahead of GitHub's must not hide them.
    const since = new Date(Date.now() - 10 * 60_000).toISOString();
    if (desktop) gh("workflow", "run", "desktop-ci.yml", "--ref", branch, "-f", `base-sha=${base}`, "-f", "full-matrix=true");
    if (mobile) gh("workflow", "run", "mobile-ci.yml", "--ref", branch);
    if (agent) gh("workflow", "run", "agent-ci.yml", "--ref", branch);
    console.log(`Testing snapshot ${sha} on ${branch}: ${workflows.join(", ")}`);

    const deadline = Date.now() + 120 * 60_000;
    let runs = [];
    let misses = 0;
    for (;;) {
      let listed;
      try {
        listed = JSON.parse(gh(
          "run", "list", "--commit", sha, "--event", "workflow_dispatch", "--limit", "50",
          "--json", "databaseId,workflowName,status,conclusion,url,createdAt",
        ));
        misses = 0;
      } catch (error) {
        if (++misses > LISTING_RETRIES) throw new Error(`could not list runs ${misses} times in a row: ${error.message}`);
      }
      if (listed) {
        runs = latestRuns(listed, since, workflows);
        const open = runs.filter((run) => run.status !== "completed");
        if (runs.length === workflows.length && !open.length) break;
        // Once Desktop CI is all that is left, its jobs are polled at the same
        // cadence, so a queued or hung x64 lane does not hold the gate.
        if (runs.length === workflows.length && open.length === 1 && open[0].workflowName === "Desktop CI") {
          try {
            const jobs = runJobs(open[0].databaseId);
            if (settled(jobs)) {
              early = { id: open[0].databaseId, jobs };
              break;
            }
          } catch (error) {
            console.log(`could not list the jobs of ${open[0].url}: ${error.message}`);
          }
        }
      }
      if (Date.now() > deadline) throw new Error(`GitHub Actions still running after two hours on ${sha}`);
      await sleep(30_000);
    }
    for (const run of runs.filter((r) => r.conclusion !== "success")) {
      if (early && run.databaseId === early.id) {
        // A run in progress has no run log yet; its finished jobs have theirs.
        for (const job of early.jobs.filter((j) => j.status === "completed" && j.conclusion !== "success" && j.conclusion !== "skipped")) {
          try {
            console.log(gh("api", `repos/{owner}/{repo}/actions/jobs/${job.id}/logs`).split("\n").slice(-80).join("\n"));
          } catch (error) {
            console.log(`could not read the log of ${job.name}: ${error.message}`);
          }
        }
        continue;
      }
      try {
        console.log(gh("run", "view", String(run.databaseId), "--log-failed").split("\n").slice(-80).join("\n"));
      } catch (error) {
        const partial = String(error.stdout ?? "").trim();
        console.log(partial ? partial.split("\n").slice(-80).join("\n") : `could not read the failed log: ${error.message}`);
      }
    }
    const verdicts = gateVerdicts(runs, (id) => (early && id === early.id ? early.jobs : runJobs(id)));
    // Last, so a size-capped tail of this output always keeps them.
    runs.forEach((run, i) => {
      console.log(`${verdicts[i].pass ? "PASS" : "FAIL"} ${run.workflowName}: ${run.conclusion || run.status}${verdicts[i].note} ${run.url}`);
    });
    return verdicts.every((v) => v.pass);
  } finally {
    // Runs and their logs outlive the branch; only the ref is removed. A run
    // the gate stopped waiting for keeps its branch, so its x64 lanes can
    // still check the snapshot out; the next watch's sweep removes it once it
    // is an hour old.
    if (!early) deleteBranch(branch);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, base, desktop, mobile, agent] = process.argv.slice(2);
  try {
    if (command === "mobile") console.log(mobileChanged(base) ? 1 : 0);
    else if (command === "agent") console.log(agentChanged(base));
    else if (command === "watch") process.exitCode = (await watch(base, desktop === "1", mobile === "1", agent === "1")) ? 0 : 1;
    else throw new Error("usage: remote-ci.mjs mobile <base-sha> | agent <base-sha> | watch <base-sha> <desktop 0|1> <mobile 0|1> [agent 0|1]");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

#!/usr/bin/env python3
"""Reuse a completed snapshot gate on main only with exact-tree, full-lane proof.

Evidence is an artifact of the successful same-repository Desktop CI run, not
a ref (the ci/ snapshot branch is deleted). Missing proof always reruns CI.
Only the Python standard library and the runner's authenticated gh CLI are used.
"""

import argparse
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import zipfile


spec = importlib.util.spec_from_file_location("ci_changes", Path(__file__).with_name("ci-changes.py"))
changes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(changes)
JOBS = changes.JOBS
MATRICES = ("macos-matrix", "linux-matrix", "linux-probe-matrix", "windows-matrix")
REQUIRED_GATES = {"changes", "open-source", "macos", "windows", "linux", "linux-probe-glibc"}
OPTIONAL_GATES = {
    "shellcheck": "shell", "probe": "probe", "linux-core": "linux_core",
    "macos-package": "macos_package", "macos-swift-tests": "macos",
    "macos-core-tests": "macos_core", "windows-package": "windows_package",
    "windows-tests": "windows", "linux-package": "linux_package", "linux-tests": "linux",
}
SHA = re.compile(r"[0-9a-f]{40}\Z")
ARTIFACT = "desktop-ci-evidence-"


def command(*args):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20).stdout


def selection(raw):
    result = {}
    for key in JOBS:
        value = raw[key]
        if value not in ("true", "false"):
            raise ValueError(f"invalid selection: {key}")
        result[key] = value == "true"
    return result


def matrices(raw):
    result = {key: json.loads(raw[key]) for key in MATRICES}
    for rows in result.values():
        if not isinstance(rows, list) or {row["arch"] for row in rows} != {"arm64", "x64"} or len(rows) != 2:
            raise ValueError("evidence needs both architectures on every platform")
    return result


def record(env):
    inputs = json.loads(env["INPUTS_JSON"])
    if (env["GITHUB_EVENT_NAME"] != "workflow_dispatch" or env["FULL_MATRIX"] != "true"
            or inputs.get("full-matrix") is not True
            or any(inputs.get(key, False) is not False for key in
                   ("skip-packages", "core-only", "reuse-linux-core", "release-analytics"))
            or inputs.get("probe-version", "") not in ("", "0.0.0")):
        raise ValueError("only default-input, full-matrix snapshot gates can publish evidence")
    sha = env["GITHUB_SHA"]
    if not SHA.fullmatch(sha) or env["GITHUB_REF"] != f"refs/heads/ci/{sha[:12]}":
        raise ValueError("not a CI snapshot")
    raw = json.loads(env["SELECTION_JSON"])
    selected = selection(raw)
    gates = json.loads(env["NEEDS_JSON"])
    if set(gates) != REQUIRED_GATES | set(OPTIONAL_GATES):
        raise ValueError("incomplete gate results")
    for job, info in gates.items():
        expected = "success" if job in REQUIRED_GATES or selected[OPTIONAL_GATES[job]] else "skipped"
        if info["result"] != expected:
            raise ValueError(f"{job}: expected {expected}, got {info['result']}")
    if command("git", "rev-parse", "HEAD").decode().strip() != sha:
        raise ValueError("checkout does not match snapshot")
    return {
        "schema": 1, "repository": env["GITHUB_REPOSITORY"],
        "run_id": int(env["GITHUB_RUN_ID"]), "run_attempt": int(env["GITHUB_RUN_ATTEMPT"]),
        "sha": sha, "tree": command("git", "rev-parse", "HEAD^{tree}").decode().strip(),
        "selected": selected, "matrices": matrices(raw),
    }


def eligible_run(run, repo, tree):
    sha = run.get("head_sha", "")
    return (
        bool(SHA.fullmatch(sha)) and run.get("event") == "workflow_dispatch"
        and run.get("status") == "completed" and run.get("conclusion") == "success"
        and run.get("head_branch") == f"ci/{sha[:12]}"
        and run.get("path") == ".github/workflows/desktop-ci.yml"
        and (run.get("head_repository") or {}).get("full_name") == repo
        and (run.get("head_commit") or {}).get("tree_id") == tree
    )


def covers(proof, run, repo, tree, wanted, runners):
    if not isinstance(proof, dict):
        return False
    return (
        type(proof.get("schema")) is int and proof["schema"] == 1
        and proof.get("repository") == repo and proof.get("sha") == run["head_sha"]
        and proof.get("tree") == tree
        and type(proof.get("run_id")) is int and proof["run_id"] == run["id"]
        and type(proof.get("run_attempt")) is int and proof["run_attempt"] == run["run_attempt"]
        and proof.get("matrices") == runners
        and isinstance(proof.get("selected"), dict)
        and set(proof["selected"]) == set(JOBS)
        and all(type(proof["selected"][key]) is bool and (not needed or proof["selected"][key])
                for key, needed in wanted.items())
    )


def find(repo, tree, raw, api):
    wanted, runners = selection(raw), matrices(raw)
    listing = json.loads(api(f"repos/{repo}/actions/workflows/desktop-ci.yml/runs?event=workflow_dispatch&status=success&per_page=50"))
    # Bound API time below the three-minute workflow step budget. An older
    # match outside this window simply runs CI again.
    candidates = [run for run in listing["workflow_runs"] if eligible_run(run, repo, tree)][:3]
    for run in candidates:
        try:
            artifacts = json.loads(api(f"repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100"))["artifacts"]
            matches = [a for a in artifacts if a["name"] == f"{ARTIFACT}{run['run_attempt']}" and not a["expired"]]
            if len(matches) != 1:
                continue
            archive = api(f"repos/{repo}/actions/artifacts/{int(matches[0]['id'])}/zip")
            with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
                if zipped.namelist() != ["evidence.json"] or zipped.getinfo("evidence.json").file_size > 65536:
                    continue
                proof = json.loads(zipped.read("evidence.json"))
            if covers(proof, run, repo, tree, wanted, runners):
                return int(run["id"])
        except (KeyError, TypeError, ValueError, OSError, zipfile.BadZipFile, subprocess.SubprocessError):
            # Deleted/expired/malformed evidence or a temporary API failure:
            # try another eligible run; otherwise run the existing CI lanes.
            continue
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("record", "find"))
    parser.add_argument("--path", type=Path)
    args = parser.parse_args(argv)
    if args.mode == "record":
        if args.path is None:
            parser.error("record needs --path")
        args.path.write_text(json.dumps(record(os.environ), sort_keys=True) + "\n")
        return 0
    try:
        # Never turn an ordinary dispatch, reusable release, or PR into a
        # no-op. Manual force-all and all release input variants run as before.
        if os.environ["GITHUB_EVENT_NAME"] != "push" or os.environ["GITHUB_REF"] != "refs/heads/main":
            return 0
        repo = os.environ["GITHUB_REPOSITORY"]
        tree = command("git", "rev-parse", "HEAD^{tree}").decode().strip()
        raw = json.loads(os.environ["SELECTION_JSON"])
        runner_outputs = json.loads(os.environ["MATRICES_JSON"])
        raw.update({key: runner_outputs[name] for key, name in zip(MATRICES, ("macos", "linux", "linux_probe", "windows"))})
        run_id = find(repo, tree, raw, lambda path: command("gh", "api", path))
        if run_id is not None:
            url = f"https://github.com/{repo}/actions/runs/{run_id}"
            message = f"Reused full-matrix Desktop CI [{run_id}]({url}) for identical tree `{tree}`. All required lanes and runner matrices match.\n"
            # Write outputs only after every check; any failure leaves the
            # original classifier outputs and full native work intact.
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
                summary.write(message)
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                output.write("".join(f"{job}=false\n" for job in JOBS) + f"reused-run={run_id}\n")
            print(message)
        else:
            print("No matching snapshot evidence; running the selected CI lanes.")
    except (KeyError, TypeError, ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"Snapshot evidence unavailable; running the selected CI lanes: {error}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())

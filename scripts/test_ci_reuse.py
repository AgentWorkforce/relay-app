#!/usr/bin/env python3
"""Exercise proof production, API provenance and conservative reuse fallbacks."""

import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile


SCRIPT = Path(__file__).with_name("ci-reuse.py")
spec = importlib.util.spec_from_file_location("ci_reuse", SCRIPT)
reuse = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reuse)
REPO = "AgentWorkforce/relay-app"
SHA, TREE = "a" * 40, "b" * 40


def fixture():
    raw = dict.fromkeys(reuse.JOBS, "true")
    for key in reuse.MATRICES:
        raw[key] = json.dumps([{"arch": arch, "runner": f"{key}-{arch}"} for arch in ("arm64", "x64")])
    run = {
        "id": 42, "run_attempt": 2, "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
        "head_sha": SHA, "head_branch": f"ci/{SHA[:12]}", "path": ".github/workflows/desktop-ci.yml",
        "head_repository": {"full_name": REPO}, "head_commit": {"tree_id": TREE},
    }
    proof = {
        "schema": 1, "repository": REPO, "run_id": 42, "run_attempt": 2, "sha": SHA, "tree": TREE,
        "selected": reuse.selection(raw), "matrices": reuse.matrices(raw),
    }
    return raw, run, proof


def zipped(proof):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as output:
        output.writestr("evidence.json", json.dumps(proof))
    return buffer.getvalue()


class ReuseTests(unittest.TestCase):
    def setUp(self):
        self.raw, self.run, self.proof = fixture()
        self.artifacts = [{"id": 99, "name": "desktop-ci-evidence-2", "expired": False}]
        self.archive = zipped(self.proof)
        self.calls = []

    def api(self, path):
        self.calls.append(path)
        if "/workflows/desktop-ci.yml/runs?" in path:
            return json.dumps({"workflow_runs": [self.run]}).encode()
        if "/runs/42/artifacts?" in path:
            return json.dumps({"artifacts": self.artifacts}).encode()
        if path.endswith("/artifacts/99/zip"):
            return self.archive
        self.fail(f"unexpected API path: {path}")

    def find(self):
        return reuse.find(REPO, TREE, self.raw, self.api)

    def test_identical_tree_reuses_snapshot_without_a_branch_lookup(self):
        self.assertEqual(self.find(), 42)
        self.assertEqual(len(self.calls), 3)
        self.assertFalse(any("/branches/" in path for path in self.calls))

    def test_only_successful_same_repo_snapshot_workflow_is_eligible(self):
        cases = {
            "event": "pull_request", "status": "in_progress", "conclusion": "failure",
            "head_sha": "invalid", "head_branch": "main", "path": ".github/workflows/other.yml",
            "head_repository": {"full_name": "attacker/fork"}, "head_commit": {"tree_id": "c" * 40},
        }
        for key, value in cases.items():
            with self.subTest(key=key):
                run = dict(self.run, **{key: value})
                self.assertFalse(reuse.eligible_run(run, REPO, TREE))

    def test_proof_must_bind_repo_tree_sha_run_and_latest_attempt(self):
        for key, value in {"schema": True, "repository": "attacker/fork", "run_id": 43,
                           "run_attempt": 1, "sha": "c" * 40, "tree": "d" * 40}.items():
            with self.subTest(key=key):
                proof = dict(self.proof, **{key: value})
                self.archive = zipped(proof)
                self.assertIsNone(self.find())

    def test_every_required_lane_including_linux_integration_must_be_covered(self):
        for key in reuse.JOBS:
            with self.subTest(lane=key):
                proof = copy.deepcopy(self.proof)
                proof["selected"][key] = False
                self.archive = zipped(proof)
                self.assertIsNone(self.find())

    def test_snapshot_can_cover_a_superset_of_mains_required_lanes(self):
        self.raw["windows_package"] = "false"
        self.raw["linux_integration"] = "false"
        self.assertEqual(self.find(), 42)

    def test_truthy_strings_and_missing_flags_are_not_proof(self):
        for value in ("false", "true", 1, None):
            proof = copy.deepcopy(self.proof)
            proof["selected"]["windows"] = value
            self.archive = zipped(proof)
            self.assertIsNone(self.find())
        proof["selected"].pop("windows")
        self.archive = zipped(proof)
        self.assertIsNone(self.find())

    def test_runner_change_or_single_architecture_never_reuses(self):
        for key in reuse.MATRICES:
            with self.subTest(matrix=key):
                proof = copy.deepcopy(self.proof)
                proof["matrices"][key][0]["runner"] = "another-runner"
                self.archive = zipped(proof)
                self.assertIsNone(self.find())
        self.raw["windows-matrix"] = json.dumps([{"arch": "x64", "runner": "windows"}])
        with self.assertRaises(ValueError):
            self.find()

    def test_missing_expired_duplicate_or_stale_attempt_artifacts_rerun(self):
        valid = copy.deepcopy(self.artifacts)
        for artifacts in ([], [dict(valid[0], expired=True)], valid + valid,
                          [dict(valid[0], name="desktop-ci-evidence-1")]):
            self.artifacts = artifacts
            self.assertIsNone(self.find())

    def test_invalid_archive_or_json_reruns(self):
        for archive in (b"not a zip", zipped(None), zipped({})):
            self.archive = archive
            self.assertIsNone(self.find())

    def test_api_failure_reaches_cli_fallback(self):
        with patch.object(reuse, "command", side_effect=OSError("offline")), patch.dict(os.environ, {
            "GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main", "GITHUB_REPOSITORY": REPO,
        }, clear=True):
            self.assertEqual(reuse.main(["find"]), 0)


class RecordingTests(unittest.TestCase):
    def setUp(self):
        self.raw, _, _ = fixture()
        gates = {key: {"result": "success"} for key in reuse.REQUIRED_GATES | set(reuse.OPTIONAL_GATES)}
        self.env = {
            "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": f"refs/heads/ci/{SHA[:12]}",
            "FULL_MATRIX": "true", "INPUTS_JSON": '{"full-matrix":true}', "GITHUB_SHA": SHA,
            "GITHUB_RUN_ID": "42", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_REPOSITORY": REPO,
            "SELECTION_JSON": json.dumps(self.raw), "NEEDS_JSON": json.dumps(gates),
        }

    def record(self):
        with patch.object(reuse, "command", side_effect=[f"{SHA}\n".encode(), f"{TREE}\n".encode()]):
            return reuse.record(self.env)

    def test_record_binds_successful_gate_to_tree(self):
        proof = self.record()
        self.assertEqual(proof, fixture()[2])

    def test_failed_cancelled_missing_or_skipped_required_gate_cannot_record(self):
        gates = json.loads(self.env["NEEDS_JSON"])
        for key in gates:
            for result in ("failure", "cancelled", "skipped"):
                with self.subTest(job=key, result=result):
                    altered = copy.deepcopy(gates)
                    altered[key]["result"] = result
                    self.env["NEEDS_JSON"] = json.dumps(altered)
                    with self.assertRaises(ValueError):
                        self.record()
        self.env["NEEDS_JSON"] = "{}"
        with self.assertRaises(ValueError):
            self.record()

    def test_unselected_lanes_must_be_skipped_and_record_as_false(self):
        self.raw["windows_package"] = "false"
        self.env["SELECTION_JSON"] = json.dumps(self.raw)
        gates = json.loads(self.env["NEEDS_JSON"])
        gates["windows-package"]["result"] = "skipped"
        self.env["NEEDS_JSON"] = json.dumps(gates)
        self.assertFalse(self.record()["selected"]["windows_package"])

    def test_partial_or_nonstandard_dispatch_cannot_record(self):
        original = dict(self.env)
        for key, value in {"FULL_MATRIX": "false", "INPUTS_JSON": '{"full-matrix":false}', "GITHUB_REF": "refs/heads/main",
                           "GITHUB_EVENT_NAME": "pull_request"}.items():
            with self.subTest(key=key):
                self.env = dict(original, **{key: value})
                with self.assertRaises(ValueError):
                    self.record()

    def test_release_input_variants_never_publish_reusable_proof(self):
        for key, value in {"skip-packages": True, "core-only": True, "reuse-linux-core": True,
                           "release-analytics": True, "probe-version": "2026.11.1"}.items():
            with self.subTest(key=key):
                self.env["INPUTS_JSON"] = json.dumps({"full-matrix": True, key: value})
                with self.assertRaises(ValueError):
                    self.record()


class CliTests(unittest.TestCase):
    def test_squashed_commit_reuses_tree_and_api_failure_writes_no_skip_outputs(self):
        raw, run, proof = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.DEVNULL).decode().strip()
            git("init", "-q")
            git("config", "user.email", "ci@example.invalid")
            git("config", "user.name", "CI test")
            (root / "source").write_text("tested source\n")
            git("add", ".")
            git("commit", "-qm", "snapshot")
            snapshot = git("rev-parse", "HEAD")
            git("commit", "--allow-empty", "-qm", "squash has a different commit sha")
            self.assertNotEqual(snapshot, git("rev-parse", "HEAD"))
            tree = git("rev-parse", "HEAD^{tree}")
            run.update(head_sha=snapshot, head_branch=f"ci/{snapshot[:12]}", head_commit={"tree_id": tree})
            proof.update(sha=snapshot, tree=tree)
            (root / "proof.zip").write_bytes(zipped(proof))
            (root / "run.json").write_text(json.dumps({"workflow_runs": [run]}))
            fake = root / "gh"
            fake.write_text("#!/usr/bin/env python3\nimport pathlib,sys\np=sys.argv[2]\n"
                            "if '/workflows/' in p: print(pathlib.Path('run.json').read_text())\n"
                            "elif '/runs/' in p: print('{\"artifacts\":[{\"id\":99,\"name\":\"desktop-ci-evidence-2\",\"expired\":false}]}')\n"
                            "else: sys.stdout.buffer.write(pathlib.Path('proof.zip').read_bytes())\n")
            fake.chmod(0o755)
            outputs, summary = root / "outputs", root / "summary"
            env = dict(os.environ, PATH=f"{root}{os.pathsep}{os.environ['PATH']}", GITHUB_EVENT_NAME="push",
                       GITHUB_REF="refs/heads/main", GITHUB_REPOSITORY=REPO, GITHUB_OUTPUT=str(outputs),
                       GITHUB_STEP_SUMMARY=str(summary), SELECTION_JSON=json.dumps(raw),
                       MATRICES_JSON=json.dumps({name: raw[key] for key, name in
                                                 zip(reuse.MATRICES, ("macos", "linux", "linux_probe", "windows"))}))
            def execute():
                return subprocess.run([sys.executable, str(SCRIPT), "find"], cwd=root, env=env, capture_output=True)
            result = execute()
            self.assertEqual(result.returncode, 0, result.stderr)
            expected = {**dict.fromkeys(reuse.JOBS, "false"), "reused-run": "42"}
            self.assertEqual(dict(line.split("=", 1) for line in outputs.read_text().splitlines()), expected)
            self.assertIn(tree, summary.read_text())
            outputs.unlink()
            fake.write_text("#!/bin/sh\nexit 1\n")
            self.assertEqual(execute().returncode, 0)
            self.assertFalse(outputs.exists())
            for event, ref in (("workflow_dispatch", "refs/heads/main"), ("pull_request", "refs/pull/1/merge")):
                env.update(GITHUB_EVENT_NAME=event, GITHUB_REF=ref)
                self.assertEqual(execute().returncode, 0)
                self.assertFalse(outputs.exists())


if __name__ == "__main__":
    unittest.main()

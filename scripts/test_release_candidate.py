#!/usr/bin/env python3
"""Reject changed, incomplete or wrongly attributed release candidates."""

import argparse
import contextlib
import io
from datetime import datetime, timedelta, timezone
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("candidate", Path(__file__).with_name("release-candidate.py"))
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)


class CandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        directory = root / "candidate"
        (directory / "assets").mkdir(parents=True)
        self.args = argparse.Namespace(directory=directory, source_sha="a" * 40,
                                       version="2026.11.1", channel="stable",
                                       repository="AgentWorkforce/relay-app", run_id="123",
                                       tests_passed=True, max_age_days=7, ci_run_id="", ci_run_metadata=None,
                                       run_metadata=root / "producer.json", profile="")
        self.write_assets()
        self.run = dict(id=123, repository={"full_name": self.args.repository},
                        head_repository={"full_name": self.args.repository},
                        head_sha=self.args.source_sha, path=candidate.WORKFLOW,
                        event="workflow_dispatch", status="completed", conclusion="success",
                        created_at=datetime.now(timezone.utc).isoformat())
        self.write_run()
        candidate.create(self.args)

    def write_assets(self, prefix="AgentRelay"):
        directory = self.args.directory
        if (directory / "assets").exists():
            shutil.rmtree(directory / "assets")
        (directory / "assets").mkdir()
        for name in sorted(candidate.required_assets(self.args.version, self.args.profile)) + list(candidate.OPTIONAL_INSTALL_SCRIPTS):
            (directory / "assets" / name).write_bytes(b"signed package bytes")
        (directory / "assets" / f"{prefix}-macOS-arm64.dmg").write_bytes(b"signed package bytes")
        (directory / "assets" / f"{prefix}-2026.11.1-macOS15-arm64.dmg").write_bytes(b"signed package bytes")
        (directory / "release-notes.md").write_text("Prepared notes\n")
        for path in (directory / "assets").iterdir():
            path.with_name(path.name + ".sha256").write_text(f"{candidate.digest(path)}  {path.name}\n")
        # Both architecture images carry the same deployment floor.
        image = directory / f"assets/{prefix}-2026.11.1-macOS15-x64.dmg"
        image.write_bytes(b"signed package bytes")
        image.with_name(image.name + ".sha256").write_text(f"{candidate.digest(image)}  {image.name}\n")

    def write_run(self):
        self.args.run_metadata.write_text(json.dumps(self.run))

    def test_a_profile_candidate_names_its_assets_by_the_opaque_id(self):
        # A profile's assets start with Desktop-<id>; the manifest records
        # the id and nothing else of the profile.
        self.assertEqual(candidate.asset_prefix(""), "AgentRelay")
        self.assertEqual(candidate.asset_prefix("acme-7"), "Desktop-acme-7")
        self.assertEqual(candidate.asset_prefix("a" * 40), "Desktop-" + "a" * 40)
        for bad in ("Acme", "-acme", "acme-", "a--b", "acme_7", "a" * 41, "acme\n"):
            with self.subTest(profile=bad), self.assertRaisesRegex(ValueError, "opaque id"):
                candidate.asset_prefix(bad)
        self.args.profile = "acme-7"
        self.write_assets("Desktop-acme-7")
        names = {path.name for path in (self.args.directory / "assets").iterdir()}
        self.assertTrue(all(name.startswith(("Desktop-acme-7-", "install.sh", "install-macos.sh", "install.ps1")) for name in names), names)
        candidate.create(self.args)
        manifest = json.loads((self.args.directory / candidate.MANIFEST).read_text())
        self.assertEqual(manifest["profile"], "acme-7")
        self.assertNotIn("Acme", json.dumps(manifest))
        candidate.verify(self.args)
        # Promoting it as the default profile, or as another, is refused.
        for profile in ("", "acme-8"):
            self.args.profile = profile
            with self.subTest(profile=profile), self.assertRaisesRegex(ValueError, "identity"):
                candidate.verify(self.args)

    def test_a_candidate_prepared_before_profiles_is_the_default_profiles(self):
        path = self.args.directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        del manifest["profile"]
        path.write_text(json.dumps(manifest))
        candidate.verify(self.args)
        self.args.profile = "acme-7"
        with self.assertRaisesRegex(ValueError, "identity"):
            candidate.verify(self.args)

    def automatic_candidate(self):
        self.args.ci_run_id = "456"
        candidate.create(self.args)
        self.run["event"] = "workflow_run"
        self.run["head_sha"] = "b" * 40  # Main advanced while the tested commit was queued.
        self.write_run()
        self.ci = dict(self.run, id=456, event="push", head_branch="main",
                       head_sha=self.args.source_sha, path=".github/workflows/desktop-ci.yml")
        self.args.ci_run_metadata = Path(self.temp.name) / "ci.json"
        self.args.ci_run_metadata.write_text(json.dumps(self.ci))

    def test_automatic_candidate_uses_tested_source_not_workflow_context(self):
        self.automatic_candidate()
        candidate.verify(self.args)

    def test_automatic_candidate_requires_matching_trusted_successful_ci(self):
        self.automatic_candidate()
        for key, value in [("id", 789), ("event", "pull_request"),
                           ("head_sha", "b" * 40), ("head_branch", "topic"),
                           ("conclusion", "failure"), ("status", "in_progress"),
                           ("path", ".github/workflows/other.yml"),
                           ("head_repository", {"full_name": "fork/repo"})]:
            original = self.ci[key]
            self.ci[key] = value
            self.args.ci_run_metadata.write_text(json.dumps(self.ci))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "CI metadata"):
                candidate.verify(self.args)
            self.ci[key] = original

    def test_automatic_candidate_cannot_omit_ci_proof(self):
        self.automatic_candidate()
        self.args.ci_run_metadata = None
        with self.assertRaisesRegex(ValueError, "independent CI"):
            candidate.verify(self.args)

    def test_invalid_creation_ci_run_id_is_rejected(self):
        self.args.ci_run_id = 'not-an-id'
        with self.assertRaisesRegex(ValueError, 'CI run ID'):
            candidate.create(self.args)

    def test_automatic_candidate_requires_manifest_ci_id(self):
        self.automatic_candidate()
        path = self.args.directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        del manifest['ci_run_id']
        path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, 'independent CI'):
            candidate.verify(self.args)

    def test_actual_promotion_verification_shell_uses_ci_source(self):
        from test_release_workflow import block
        checkout = Path(__file__).resolve().parents[1]
        workflow = (checkout / '.github/workflows/desktop-promote.yml').read_text()
        step = block(workflow, '- name: Verify identity and every prepared byte', 6)
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        root = Path(self.temp.name)
        (root / 'scripts').mkdir()
        shutil.copyfile(checkout / 'scripts/release-candidate.py', root / 'scripts/release-candidate.py')
        gh = root / 'gh'
        gh.write_text('#!/bin/sh\nprintf \'%s\n\' "$*" >> "$GH_CALLS"\ncat "$CI_FIXTURE"\n')
        gh.chmod(0o755)
        env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'],
                   VERSION=self.args.version, CHANNEL=self.args.channel,
                   GITHUB_REPOSITORY=self.args.repository, CANDIDATE_RUN_ID='123',
                   GITHUB_OUTPUT=str(root / 'output'), CI_FIXTURE=str(root / 'ci.json'),
                   GH_CALLS=str(root / 'calls'), PROFILE='')
        for automatic in (False, True):
            if automatic:
                self.automatic_candidate()
            shutil.copyfile(self.args.run_metadata, root / 'producer-run.json')
            (root / 'output').write_text('')
            result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((root / 'output').read_text().strip(), 'source-sha=' + self.args.source_sha)
        self.assertIn('actions/runs/456', (root / 'calls').read_text())
        self.ci['head_sha'] = 'b' * 40
        (root / 'ci.json').write_text(json.dumps(self.ci))
        result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        # Promoting a default candidate as a profile's is refused.
        self.ci['head_sha'] = self.args.source_sha
        (root / 'ci.json').write_text(json.dumps(self.ci))
        result = subprocess.run(['bash', '-c', script], cwd=root, env={**env, 'PROFILE': 'acme-7'}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('profile', result.stderr)

    def test_valid_candidate_preserves_signed_bytes(self):
        before = (self.args.directory / "assets" / "AgentRelay-2026.11.1-macOS-arm64.dmg").read_bytes()
        candidate.verify(self.args)
        self.assertEqual(before, (self.args.directory / "assets" / "AgentRelay-2026.11.1-macOS-arm64.dmg").read_bytes())

    def test_changed_notes_alias_or_signed_asset_rejected(self):
        for relative in ["release-notes.md", "assets/AgentRelay-macOS-arm64.dmg",
                         "assets/AgentRelay-2026.11.1-Windows-x64-Setup.exe"]:
            path = self.args.directory / relative
            original = path.read_bytes()
            path.write_bytes(b"changed")
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                candidate.verify(self.args)
            path.write_bytes(original)

    def test_missing_architecture_or_added_asset_rejected(self):
        path = self.args.directory / "assets/AgentRelay-2026.11.1-Linux-arm64.deb"
        path.unlink()
        with self.assertRaisesRegex(ValueError, "inventory differs"):
            candidate.verify(self.args)
        path.write_bytes(b"signed package bytes")
        (self.args.directory / "assets/unexpected.txt").write_text("extra")
        with self.assertRaisesRegex(ValueError, "inventory differs"):
            candidate.verify(self.args)

    def test_identity_mismatch_rejected(self):
        for key, value in [("source_sha", "b" * 40), ("version", "2026.11.2"),
                           ("repository", "other/repo"), ("run_id", "456")]:
            original = getattr(self.args, key)
            setattr(self.args, key, value)
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "identity"):
                candidate.verify(self.args)
            setattr(self.args, key, original)

    def test_untrusted_failed_or_incomplete_producer_rejected(self):
        for key, value in [("path", ".github/workflows/evil.yml"), ("id", 456),
                           ("event", "pull_request"), ("event", "push"),
                           ("head_sha", "b" * 40), ("conclusion", "failure"),
                           ("status", "in_progress"), ("head_repository", {"full_name": "fork/repo"})]:
            original = self.run[key]
            self.run[key] = value
            self.write_run()
            with self.subTest(key=key, value=value), self.assertRaisesRegex(ValueError, "producer"):
                candidate.verify(self.args)
            self.run[key] = original
        self.write_run()

    def test_stale_producer_rejected(self):
        self.run["created_at"] = (datetime.now(timezone.utc) - timedelta(days=8)).isoformat()
        self.write_run()
        with self.assertRaisesRegex(ValueError, "stale"):
            candidate.verify(self.args)

    def test_symlink_rejected(self):
        (self.args.directory / "assets/link").symlink_to(self.args.run_metadata)
        with self.assertRaisesRegex(ValueError, "symlink"):
            candidate.verify(self.args)

    def test_duplicate_packages_rejected(self):
        extra = self.args.directory / "direct children"
        extra.mkdir()
        (extra / "AgentRelay-2026.11.1-Linux-arm64.deb").write_bytes(b"direct children")
        with self.assertRaisesRegex(ValueError, "direct children"):
            candidate.verify(self.args)

    def test_creation_requires_test_gate(self):
        self.args.tests_passed = False
        with self.assertRaisesRegex(ValueError, "test prerequisites"):
            candidate.create(self.args)

    def test_channel_cannot_relabel_beta_as_stable(self):
        self.args.channel = "beta"
        with self.assertRaisesRegex(ValueError, "channel disagree"):
            candidate.verify(self.args)

    def test_checksum_missing_or_changed_rejected(self):
        path = self.args.directory / "assets/install.sh.sha256"
        path.write_text("0" * 64 + "  install.sh\n")
        with self.assertRaisesRegex(ValueError, "checksum"):
            candidate.verify(self.args)
        path.unlink()
        with self.assertRaisesRegex(ValueError, "inventory differs"):
            candidate.verify(self.args)

    def test_current_floored_image_required_even_when_bridge_exists(self):
        (self.args.directory / "assets/AgentRelay-2026.11.1-macOS15-arm64.dmg").unlink()
        with self.assertRaisesRegex(ValueError, "floored macOS image"):
            candidate.verify(self.args)

    def test_disagreeing_mac_floors_rejected(self):
        path = self.args.directory / "assets/AgentRelay-2026.11.1-macOS15-arm64.dmg"
        path.rename(path.with_name("AgentRelay-2026.11.1-macOS16-arm64.dmg"))
        with self.assertRaisesRegex(ValueError, "deployment floor"):
            candidate.verify(self.args)

    def test_missing_stable_alias_rejected(self):
        (self.args.directory / "assets/AgentRelay-macOS-arm64.dmg").unlink()
        with self.assertRaisesRegex(ValueError, "inventory differs"):
            candidate.verify(self.args)

    def test_notes_required(self):
        (self.args.directory / "release-notes.md").unlink()
        with self.assertRaisesRegex(ValueError, "release-notes"):
            candidate.verify(self.args)

    def cloud_candidate(self):
        # A cloud profile (proposed Cloud contract, relay-control
        # docs/desktop-feed.md) records its feed.
        self.args.profile = "x7k2q"
        self.args.feed = "cloud"
        self.write_assets("Desktop-x7k2q")
        candidate.create(self.args)
        return json.loads((self.args.directory / candidate.MANIFEST).read_text())

    def test_a_cloud_candidate_records_its_feed_and_a_github_one_none(self):
        self.assertNotIn("feed", json.loads((self.args.directory / candidate.MANIFEST).read_text()))
        self.assertEqual(self.cloud_candidate()["feed"], "cloud")
        candidate.verify(self.args)
        # Promoting it as a GitHub candidate, or a GitHub one as cloud, is refused.
        self.args.feed = "github"
        with self.assertRaisesRegex(ValueError, "feed"):
            candidate.verify(self.args)
        self.args.feed = "cloud"
        path = self.args.directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        del manifest["feed"]
        path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "feed"):
            candidate.verify(self.args)

    def test_an_unlisted_cloud_candidate_records_it_for_promote(self):
        self.assertNotIn("unlisted", self.cloud_candidate())
        self.args.unlisted = True
        self.assertIs(self.cloud_candidate()["unlisted"], True)
        candidate.verify(self.args)
        self.args.output = Path(self.temp.name) / "cloud-assets.txt"
        candidate.assets(self.args)
        path = self.args.directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        # Only true, and only on a cloud candidate.
        for value in (False, "true", 1, None):
            with self.subTest(value=value):
                path.write_text(json.dumps(dict(manifest, unlisted=value)))
                with self.assertRaisesRegex(ValueError, "unlisted"):
                    candidate.verify(self.args)
        github = dict(manifest)
        del github["feed"]
        path.write_text(json.dumps(github))
        self.args.feed = "github"
        with self.assertRaisesRegex(ValueError, "unlisted"):
            candidate.verify(self.args)
        self.args.profile = ""
        self.write_assets()
        with self.assertRaisesRegex(ValueError, "only a cloud candidate is prepared unlisted"):
            candidate.create(self.args)

    def test_a_candidate_records_the_core_crates_it_was_built_with(self):
        path = self.args.directory / candidate.MANIFEST
        # Without --crates (a candidate prepared before crates were recorded): none.
        self.assertNotIn("crates", json.loads(path.read_text()))
        crates = {"agent-relay-node": {"rev": "0123456789abcdef0123456789abcdef01234567"},
                  "relaycast": {"version": "10.0.1"},
                  "relayscrubber": {"version": "0.5.0-alpha.2"}}
        self.args.crates = Path(self.temp.name) / "crates.json"
        self.args.crates.write_text(json.dumps(crates) + "\n")
        candidate.create(self.args)
        manifest = json.loads(path.read_text())
        self.assertEqual(manifest["crates"], crates)
        candidate.verify(self.args)
        # Anything but that exact shape is refused, at creation and at promotion.
        bad = [{}, {"agent-relay-node": {"rev": "54a2a06"}, "relaycast": {"version": "10.0.1"}},
               {"agent-relay-node": {"rev": "a" * 40}, "relaycast": {"version": "^10"}},
               dict(crates, relayhistory={"version": "1.0.0"}),
               {"agent-relay-node": {"rev": "a" * 40, "tag": "v1"}, "relaycast": {"version": "10.0.1"}},
               {"agent-relay-node": "a" * 40, "relaycast": {"version": "10.0.1"}}]
        for value in bad:
            with self.subTest(value=value):
                self.args.crates.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, "crates must be"):
                    candidate.create(self.args)
                path.write_text(json.dumps(dict(manifest, crates=value)))
                with self.assertRaisesRegex(ValueError, "crates must be"):
                    candidate.verify(self.args)

    def test_a_candidate_requires_relayscrubber_now_that_both_workspaces_pin_it(self):
        path = self.args.directory / candidate.MANIFEST
        self.args.crates = Path(self.temp.name) / "crates.json"
        pins = {"agent-relay-node": {"rev": "0123456789abcdef0123456789abcdef01234567"},
                "relaycast": {"version": "10.0.1"}}
        value = dict(pins, relayscrubber={"version": "0.5.0-alpha.2"})
        self.args.crates.write_text(json.dumps(value) + "\n")
        candidate.create(self.args)
        manifest = json.loads(path.read_text())
        self.assertEqual(manifest["crates"], value)
        candidate.verify(self.args)
        # Missing relayscrubber, an unknown key, or a non-exact version is
        # refused at creation and at promotion.
        bad = [dict(pins, relayscrubber={"version": "0.3.0"}, relayhistory={"version": "1.0.0"}),
               pins,
               dict(pins, relayscrubber={}),
               dict(pins, relayscrubber={"rev": "a" * 40}),
               dict(pins, relayscrubber={"version": "0.3.0", "rev": "a" * 40}),
               dict(pins, relayscrubber={"version": "^0.3"}),
               dict(pins, relayscrubber={"version": 3}),
               dict(pins, relayscrubber="0.3.0"),
               {"agent-relay-node": pins["agent-relay-node"], "relayscrubber": {"version": "0.3.0"}}]
        for value in bad:
            with self.subTest(value=value):
                self.args.crates.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, "crates must be"):
                    candidate.create(self.args)
                path.write_text(json.dumps(dict(manifest, crates=value)))
                with self.assertRaisesRegex(ValueError, "crates must be"):
                    candidate.verify(self.args)

    def test_cloud_versions_and_profiles_follow_the_feed(self):
        self.args.profile = "x7k2q"
        self.args.feed = "cloud"
        for version, channel in (("2026.3.1", "stable"), ("2026.11.1", "stable"), ("2026.3.2-beta.1", "beta")):
            self.args.version, self.args.channel = version, channel
            candidate.identity(self.args)
        for version in ("2026.03.1", "2026.13.1", "2026.0.1", "2026.3.01"):
            self.args.version = version
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "version"):
                candidate.identity(self.args)
        self.args.version = "2026.11.1"
        for profile in ("", "acme-7", "a" * 33):
            self.args.profile = profile
            with self.subTest(profile=profile), self.assertRaisesRegex(ValueError, "profile"):
                candidate.identity(self.args)
        # A GitHub version keeps its two-digit month.
        self.args.feed, self.args.profile, self.args.version = "github", "", "2026.3.1"
        with self.assertRaisesRegex(ValueError, "version"):
            candidate.identity(self.args)

    def test_cloud_assets_are_the_versioned_packages_and_installers(self):
        self.cloud_candidate()
        output = Path(self.temp.name) / "cloud-assets.txt"
        self.args.output = output
        log = io.StringIO()
        with contextlib.redirect_stderr(log):
            candidate.assets(self.args)
        names = output.read_text().splitlines()
        self.assertEqual(names, sorted(names))
        # Every cloud profile publishes the fixed 16-asset cloud set.
        self.assertIn("Cloud assets of profile x7k2q: the fixed cloud set selects 16 of 21.", log.getvalue())
        self.assertIn("  selected: install.ps1\n", log.getvalue())
        # The feed carries no macOS probe for install-macos.sh to install.
        self.assertIn("  omitted: install-macos.sh\n", log.getvalue())
        self.assertNotIn("install-macos.sh", names)
        self.assertIn("  omitted: Desktop-x7k2q-2026.11.1-macOS-arm64.dmg\n", log.getvalue())
        self.assertIn("install.sh", names)
        self.assertIn("install.ps1", names)
        self.assertIn("Desktop-x7k2q-2026.11.1-macOS15-arm64.dmg", names)
        self.assertNotIn("Desktop-x7k2q-2026.11.1-macOS-arm64.dmg", names)
        self.assertEqual(len(names), 16)
        for name in names:
            self.assertTrue(name in ("install.sh", "install.ps1") or name.startswith("Desktop-x7k2q-2026.11.1-"), name)
            self.assertFalse(name.endswith(".sha256"), name)
        self.assertNotIn("Desktop-x7k2q-macOS-arm64.dmg", names)
        # Both installers are required: one missing lists nothing.
        for kept in ("install.sh", "install.ps1"):
            with self.subTest(kept=kept), self.assertRaisesRegex(ValueError, "exactly 16 assets: install.sh"):
                candidate.cloud_assets([f"assets/{kept}", "assets/Desktop-x7k2q-2026.11.1-Linux-x64.deb"],
                                       "2026.11.1", "x7k2q")
        # Changed bytes, or a GitHub candidate, list nothing.
        output.unlink()
        path = self.args.directory / "assets" / "Desktop-x7k2q-2026.11.1-Linux-arm64.deb"
        path.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "checksum"):
            candidate.assets(self.args)
        self.assertFalse(output.exists())
        self.args.feed = "github"
        with self.assertRaisesRegex(ValueError, "cloud"):
            candidate.assets(self.args)

    def test_every_cloud_profile_publishes_the_fixed_16_asset_set(self):
        # The signer caps the manifest at 4096 canonical bytes: the floored
        # macOS images, the Linux tarballs and debs, the Windows zips and
        # Setup.exes and the Linux and Windows probe bundles of both
        # architectures, and the two install scripts.
        version = "2026.11.1-beta.1"
        for profile in ("m5lxqm2osk", "x7k2q"):
            with self.subTest(profile=profile):
                files = []
                for name in sorted(candidate.required_assets(version, profile)) + ["install-macos.sh",
                        f"Desktop-{profile}-{version}-macOS15-arm64.dmg",
                        f"Desktop-{profile}-{version}-macOS15-x64.dmg"]:
                    files += [f"assets/{name}", f"assets/{name}.sha256"]
                lines = []
                names = candidate.cloud_assets(files, version, profile, log=lines.append)
                prefix = f"Desktop-{profile}-{version}-"
                self.assertEqual(names, sorted(["install.ps1", "install.sh"] + [
                    prefix + name for arch in ("arm64", "x64") for name in (
                        f"macOS15-{arch}.dmg", f"Linux-{arch}.tar.gz", f"Linux-{arch}.deb",
                        f"Windows-{arch}.zip", f"Windows-{arch}-Setup.exe", f"Linux-{arch}-probe.tar.gz",
                        f"Windows-{arch}-probe.zip")]))
                self.assertEqual(lines[0], f"Cloud assets of profile {profile}: the fixed cloud set selects 16 of 21.")
                self.assertEqual([line for line in lines if line.startswith("  selected: ")],
                                 [f"  selected: {name}" for name in names])
                omitted = [line.removeprefix("  omitted: ") for line in lines if line.startswith("  omitted: ")]
                self.assertEqual(sorted(omitted), sorted(["install-macos.sh"] + [
                    prefix + name for arch in ("arm64", "x64") for name in (
                        f"macOS-{arch}.dmg", f"macOS-{arch}-probe.tar.gz")]))
                # A candidate missing one of the 16 lists nothing.
                short = [f for f in files if not f.startswith(f"assets/{prefix}Windows-x64-Setup.exe")]
                with self.assertRaisesRegex(ValueError, "exactly 16 assets.*found 15"):
                    candidate.cloud_assets(short, version, profile, log=lambda _: None)

    def test_a_github_candidate_keeps_every_package(self):
        # The fixed cloud set changes nothing for a GitHub release.
        self.assertEqual(len(candidate.required_assets("2026.11.1", "x7k2q")), 34)
        self.assertNotIn("install-macos.sh", candidate.required_assets("2026.11.1", "x7k2q"))
        self.assertIn("Desktop-x7k2q-2026.11.1-macOS-arm64-probe.tar.gz",
                      candidate.required_assets("2026.11.1", "x7k2q"))
        candidate.verify(self.args)

    def test_install_macos_is_optional_but_checked_when_present(self):
        assets = self.args.directory / "assets"
        script, checksum = assets / "install-macos.sh", assets / "install-macos.sh.sha256"
        self.assertTrue(script.is_file())
        candidate.verify(self.args)
        # A candidate built before it, or a hotfix from an older base, has neither.
        saved = script.read_bytes(), checksum.read_bytes()
        script.unlink()
        checksum.unlink()
        candidate.verify(self.args)
        # A checksum without its script is unexpected.
        checksum.write_bytes(saved[1])
        with self.assertRaisesRegex(ValueError, "unexpected=install-macos.sh.sha256"):
            candidate.verify(self.args)
        checksum.unlink()
        # The script without its checksum is missing it.
        script.write_bytes(saved[0])
        with self.assertRaisesRegex(ValueError, "missing=install-macos.sh.sha256"):
            candidate.verify(self.args)
        # A wrong checksum is rejected.
        checksum.write_text(f"{'0' * 64}  install-macos.sh\n")
        with self.assertRaisesRegex(ValueError, "invalid package checksum: install-macos.sh"):
            candidate.verify(self.args)

    def test_cloud_assets_command_writes_only_names(self):
        self.cloud_candidate()
        script = Path(__file__).with_name("release-candidate.py")
        result = subprocess.run([sys.executable, str(script), "cloud-assets", "--feed", "cloud",
                                 "--directory", str(self.args.directory), "--source-sha", self.args.source_sha,
                                 "--version", self.args.version, "--channel", self.args.channel,
                                 "--repository", self.args.repository, "--run-id", self.args.run_id,
                                 "--profile", "x7k2q"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(result.stdout.splitlines()), 16)
        self.assertNotIn("Candidate", result.stdout)
        # The default profile is never a cloud profile.
        result = subprocess.run([sys.executable, str(script), "cloud-assets", "--feed", "cloud",
                                 "--directory", str(self.args.directory), "--source-sha", self.args.source_sha,
                                 "--version", self.args.version, "--channel", self.args.channel,
                                 "--repository", self.args.repository, "--run-id", self.args.run_id],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("profile", result.stderr)


if __name__ == "__main__":
    unittest.main()

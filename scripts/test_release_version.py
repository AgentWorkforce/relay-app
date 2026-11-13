#!/usr/bin/env python3
"""Run with python3 scripts/test_release_version.py."""
import importlib.util
import json
from pathlib import Path
import unittest
from contextlib import redirect_stdout
from io import StringIO
import tempfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release_version", Path(__file__).with_name("release-version.py"))
release_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_version)


class ReleaseVersionTests(unittest.TestCase):
    def test_push_tags_infer_channel_and_release_title(self):
        with tempfile.TemporaryDirectory() as directory:
            releases = Path(directory) / "releases.json"
            releases.write_text("[]")
            for version, channel, title in [("2026.10.1", "stable", "Stable"), ("2026.10.1-beta.2", "beta", "Beta")]:
                output = StringIO()
                with patch("sys.argv", ["release-version.py", "--releases", str(releases), "--tag", f"v{version}"]), redirect_stdout(output):
                    release_version.main()
                self.assertEqual(output.getvalue(), f"version={version}\ntag=v{version}\nchannel={channel}\ntitle={title} v{version}\n")

    def resolve(self, tags=(), requested="", channel="stable"):
        return release_version.resolve([{"tag_name": f"v{tag}"} for tag in tags], requested, channel, "2026.10")

    def test_default_stable(self):
        self.assertEqual(self.resolve(), "2026.10.1")
        self.assertEqual(self.resolve(["2026.10.9", "2026.09.99"]), "2026.10.10")

    def test_multiple_betas_then_stable_then_next_beta(self):
        tags = ["2026.10.1"]
        self.assertEqual(self.resolve(tags, channel="beta"), "2026.10.2-beta.1")
        tags += ["2026.10.2-beta.1", "2026.10.2-beta.9"]
        self.assertEqual(self.resolve(tags, channel="beta"), "2026.10.2-beta.10")
        self.assertEqual(self.resolve(tags), "2026.10.2")
        tags += ["2026.10.2"]
        self.assertEqual(self.resolve(tags, channel="beta"), "2026.10.3-beta.1")

    def test_explicit_versions(self):
        self.assertEqual(self.resolve(requested="v2026.10.4"), "2026.10.4")
        self.assertEqual(self.resolve(requested="2026.10.4", channel="beta"), "2026.10.4-beta.1")
        self.assertEqual(self.resolve(requested="2026.10.4-beta.3", channel="beta"), "2026.10.4-beta.3")

    def test_reject_mismatch_invalid_versions_and_overwrites(self):
        for requested, channel in [("2026.10.1-beta.1", "stable"), ("2026.10.1-rc.1", "beta"), ("2026.10.1-beta.0", "beta"), ("latest", "stable")]:
            with self.subTest(requested=requested), self.assertRaises(ValueError):
                self.resolve(requested=requested, channel=channel)
        for requested, channel in [("2026.10.1", "stable"), ("2026.10.1", "beta"), ("2026.10.1-beta.2", "beta")]:
            with self.subTest(requested=requested), self.assertRaises(ValueError):
                self.resolve(["2026.10.1"], requested, channel)
        with self.assertRaises(ValueError):
            self.resolve(["2026.10.2-beta.1"], "2026.10.2-beta.1", "beta")

    def test_updater_order_matches_the_gtk_and_macos_updaters(self):
        newer = release_version.is_newer
        self.assertFalse(newer("2026.3.1", "v2026.03.2"))
        self.assertTrue(newer("2026.3.3", "v2026.03.2"))
        self.assertFalse(newer("2026.03.2", "v2026.3.2"))
        self.assertTrue(newer("2026.10.2", "v2026.10.2-beta.9"))
        self.assertFalse(newer("2026.10.2-beta.9", "v2026.10.2"))
        self.assertTrue(newer("2026.10.2-beta.10", "v2026.10.2-beta.9"))
        self.assertTrue(newer("2026.10.2-beta.10", "v2026.10.2-beta.09"))
        self.assertTrue(newer("2026.10.3-beta.1", "v2026.10.2"))
        self.assertFalse(newer("2026.10.1", "latest"))

    def test_newer_than_all_refuses_a_version_the_updaters_rank_lower(self):
        with tempfile.TemporaryDirectory() as directory:
            releases = Path(directory) / "releases.json"
            for tags, version, channel, ok in [
                (["2026.03.2"], "2026.3.1", "stable", False),
                (["2026.03.2"], "2026.3.2-beta.1", "beta", False),
                (["2026.4.1-beta.2"], "2026.4.1-beta.1", "beta", False),
                (["2026.03.2", "nightly"], "2026.3.3", "stable", True),
                (["2026.4.1-beta.2"], "2026.4.1", "stable", True),
                ([], "2026.3.1", "stable", True),
            ]:
                releases.write_text(json.dumps([{"tag_name": f"v{tag}"} for tag in tags]))
                argv = ["release-version.py", "--releases", str(releases), "--version", version,
                        "--channel", channel, "--newer-than-all"]
                with self.subTest(tags=tags, version=version), patch("sys.argv", argv), redirect_stdout(StringIO()):
                    if ok:
                        release_version.main()
                    else:
                        with self.assertRaisesRegex(ValueError, "does not sort after"):
                            release_version.main()


if __name__ == "__main__":
    unittest.main()

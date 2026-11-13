#!/usr/bin/env python3
"""Tests for scripts/release-notes.py (run: python3 scripts/test_release_notes.py)."""

import contextlib
import hashlib
import importlib.util
import io
import json
import tempfile
import subprocess
import sys
import unittest
from unittest.mock import patch
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "release_notes", Path(__file__).with_name("release-notes.py")
)
release_notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_notes)

class ReleaseNotesExitTests(unittest.TestCase):
    def test_empty_bullets_have_distinct_cli_status(self):
        script = Path(__file__).with_name('release-notes.py')
        with tempfile.TemporaryDirectory() as temporary:
            changelog = Path(temporary) / 'CHANGELOG.md'
            for text, expected in [('## Unreleased\n\n', 3),
                                   ('## Unreleased\n\n## Unreleased\n\n', 1),
                                   ('## Unreleased\n\n- A change.\n', 0)]:
                changelog.write_text(text)
                result = subprocess.run([sys.executable, str(script), '2026.11.1', '--check',
                                         '--changelog', str(changelog)], capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stderr)
            changelog.unlink()
            result = subprocess.run([sys.executable, str(script), '2026.11.1', '--check',
                                     '--changelog', str(changelog)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotEqual(result.returncode, 3)


BASE = """# Changelog

Header.

## Unreleased

- **First change.** Does a thing.
- **Second change.** Does another.

## v2026.01.1

- **Old change.** Shipped.
"""


class ReleaseNotesTests(unittest.TestCase):
    def test_generated_notes_label_both_tracks_and_link_filters(self):
        path = self.write()
        output = self.write("")
        for version, label in [("2026.10.1", "Stable"), ("2026.10.1-beta.2", "Beta (opt-in)")]:
            with patch("sys.argv", ["release-notes.py", version, "--changelog", str(path),
                                     "--probe-version", version, "--output", str(output)]):
                release_notes.main()
            notes = output.read_text()
            self.assertIn(f"**Channel: {label}**", notes)
            self.assertIn("releases?q=prerelease%3Afalse", notes)
            self.assertIn("releases?q=prerelease%3Atrue", notes)
            self.assertIn(f"/releases/download/v{version}/", notes)
            floor = release_notes.macos_release_assets.deployment_floor()
            self.assertIn(f"AgentRelay-macOS-arm64.dmg) (macOS {floor} or newer)", notes)
            bridge = (f"[`AgentRelay-{version}-macOS-x64.dmg`](https://github.com/AgentWorkforce/"
                      f"relay-desktop-releases/releases/download/v{version}/AgentRelay-{version}-macOS-x64.dmg): "
                      f"for updaters older than 2026.10.65; installs 2026.10.66, which then updates itself\n")
            if floor > release_notes.macos_release_assets.LEGACY_FLOOR:
                self.assertIn(bridge, notes)
            else:
                self.assertNotIn("for updaters older than", notes)

    def test_a_profiles_notes_name_its_artifacts_by_the_opaque_id_and_no_product(self):
        path = self.write()
        output = self.write("")
        with patch("sys.argv", ["release-notes.py", "2026.10.1", "--changelog", str(path),
                                 "--probe-version", "2026.10.1", "--output", str(output),
                                 "--repository", "acme/desktop-releases", "--asset-prefix", "Desktop-acme-7"]):
            release_notes.main()
        notes = output.read_text()
        self.assertTrue(notes.startswith("Desktop-acme-7 v2026.10.1 for macOS, Linux and Windows.\n"), notes)
        self.assertNotIn("Agent Relay", notes)
        self.assertNotIn("AgentRelay", notes)
        self.assertNotIn("for updaters older than", notes)
        self.assertIn("[`Desktop-acme-7-Linux-x64.deb`](https://github.com/acme/desktop-releases/releases/download/v2026.10.1/Desktop-acme-7-Linux-x64.deb)", notes)
        self.assertIn("[`Desktop-acme-7-Windows-arm64-Setup.exe`](https://github.com/acme/desktop-releases/releases/download/v2026.10.1/Desktop-acme-7-Windows-arm64-Setup.exe)", notes)
        self.assertIn("- **First change.**", notes)
        with patch("sys.argv", ["release-notes.py", "2026.10.1", "--changelog", str(path),
                                 "--probe-version", "2026.10.1", "--output", str(output),
                                 "--asset-prefix", "Acme Desktop"]), self.assertRaises(ValueError):
            release_notes.main()

    def test_a_profiles_notes_never_fall_back_to_a_versioned_section(self):
        # A profile's version numbers are its own repository's, so a
        # `## v<version>` section here is another release's notes: with
        # --unreleased an empty Unreleased is "no notes" (exit 3) even when
        # that section has bullets, which --check alone would accept.
        script = Path(__file__).with_name("release-notes.py")
        changelog = self.write("## Unreleased\n\n## v2026.10.1\n\n- Shipped already.\n")
        check = [sys.executable, str(script), "2026.10.1", "--check", "--changelog", str(changelog)]
        result = subprocess.run(check, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run(check + ["--unreleased"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 3, result.stderr)

    def test_a_profiles_notes_are_the_unreleased_section_only(self):
        path = self.write("## Unreleased\n\n- **Profile change.** Not shipped yet.\n\n"
                          "## v2026.10.1\n\n- **Default change.** Shipped already.\n")
        output = self.write("")
        with patch("sys.argv", ["release-notes.py", "2026.10.1", "--changelog", str(path), "--unreleased",
                                 "--probe-version", "2026.10.1", "--output", str(output),
                                 "--repository", "acme/desktop-releases", "--asset-prefix", "Desktop-acme-7"]):
            release_notes.main()
        notes = output.read_text()
        self.assertIn("- **Profile change.** Not shipped yet.", notes)
        self.assertNotIn("Default change", notes)
        # Without --unreleased the same version reads its own section.
        with patch("sys.argv", ["release-notes.py", "2026.10.1", "--changelog", str(path),
                                 "--probe-version", "2026.10.1", "--output", str(output)]):
            release_notes.main()
        notes = output.read_text()
        self.assertIn("- **Default change.** Shipped already.", notes)
        self.assertNotIn("Profile change", notes)

    def test_betas_reuse_upcoming_stable_notes_without_consuming_unreleased(self):
        path = self.write(BASE + "\n## v2026.10.1\n\n- Upcoming stable changes.\n")
        before = path.read_text()
        for version in ["2026.10.1-beta.1", "2026.10.1-beta.2"]:
            self.assertEqual(release_notes.changes_for_version(path, version), "- Upcoming stable changes.")
        self.assertEqual(path.read_text(), before)
        self.assertIn("First change", release_notes.changes_for_version(path, "2026.10.2-beta.1"))

    def write(self, text=BASE):
        tmp = tempfile.NamedTemporaryFile(
            "w", suffix=".md", delete=False, encoding="utf-8"
        )
        tmp.write(text)
        tmp.close()
        self.addCleanup(Path(tmp.name).unlink)
        return Path(tmp.name)

    def test_promote_renames_unreleased_and_adds_fresh_one(self):
        path = self.write()
        self.assertTrue(release_notes.promote_changelog(path, "2026.02.1"))
        text = path.read_text(encoding="utf-8")
        self.assertIn("## Unreleased\n\n## v2026.02.1\n\n- **First change.**", text)
        self.assertIn("## v2026.01.1\n\n- **Old change.** Shipped.", text)
        self.assertEqual(
            release_notes.changes_for_version(path, "2026.02.1"),
            "- **First change.** Does a thing.\n- **Second change.** Does another.",
        )

    def test_promote_is_noop_when_version_already_present(self):
        path = self.write()
        release_notes.promote_changelog(path, "2026.02.1")
        before = path.read_text(encoding="utf-8")
        self.assertFalse(release_notes.promote_changelog(path, "2026.02.1"))
        self.assertEqual(path.read_text(encoding="utf-8"), before)

    def test_promote_rejects_empty_unreleased(self):
        path = self.write(BASE.replace("- **First change.** Does a thing.\n- **Second change.** Does another.\n", ""))
        with self.assertRaises(ValueError):
            release_notes.promote_changelog(path, "2026.02.1")

    def test_promote_rejects_missing_unreleased(self):
        path = self.write(BASE.replace("## Unreleased", "## Pending"))
        with self.assertRaises(ValueError):
            release_notes.promote_changelog(path, "2026.02.1")

    def test_changes_fall_back_to_unreleased_before_promotion(self):
        path = self.write()
        self.assertEqual(
            release_notes.changes_for_version(path, "2026.02.1"),
            "- **First change.** Does a thing.\n- **Second change.** Does another.",
        )

    def test_merge_keeps_concurrent_bullet_in_unreleased(self):
        path = self.write()
        base = path.read_text(encoding="utf-8")
        release_notes.promote_changelog(path, "2026.02.1")
        promoted = path.read_text(encoding="utf-8")
        latest = base.replace(
            "## Unreleased\n", "## Unreleased\n\n- **Concurrent change.** Landed during the release.\n", 1
        ).replace("- **Old change.** Shipped.", "- **Old change.** Shipped and documented.")
        merged, changed = release_notes.merge_promotion(base, promoted, latest, "2026.02.1")
        self.assertTrue(changed)
        unreleased = merged.split("## v2026.02.1")[0]
        self.assertIn("- **Concurrent change.** Landed during the release.", unreleased)
        self.assertNotIn("- **First change.**", unreleased)
        self.assertIn("## v2026.02.1\n\n- **First change.** Does a thing.", merged)
        self.assertIn("- **Old change.** Shipped and documented.", merged)

    def test_merge_is_noop_when_latest_already_promoted(self):
        path = self.write()
        base = path.read_text(encoding="utf-8")
        release_notes.promote_changelog(path, "2026.02.1")
        promoted = path.read_text(encoding="utf-8")
        merged, changed = release_notes.merge_promotion(base, promoted, promoted, "2026.02.1")
        self.assertFalse(changed)
        self.assertEqual(merged, promoted)

    def test_merge_rejects_a_different_existing_version_section(self):
        path = self.write()
        base = path.read_text(encoding="utf-8")
        release_notes.promote_changelog(path, "2026.02.1")
        promoted = path.read_text(encoding="utf-8")
        latest = promoted.replace("- **First change.** Does a thing.", "- **Other change.** No.")
        with self.assertRaises(ValueError):
            release_notes.merge_promotion(base, promoted, latest, "2026.02.1")

    def test_changes_reject_duplicate_version_sections(self):
        path = self.write(BASE + "\n## v2026.01.1\n\n- **Dup.** No.\n")
        with self.assertRaises(ValueError):
            release_notes.changes_for_version(path, "2026.01.1")


assets = release_notes.macos_release_assets


class MacOSReleaseAssetsTests(unittest.TestCase):
    def package(self, platform):
        tmp = tempfile.NamedTemporaryFile("w", suffix=".swift", delete=False, encoding="utf-8")
        tmp.write(f"let package = Package(\n    platforms: [\n        {platform}\n    ]\n)\n")
        tmp.close()
        self.addCleanup(Path(tmp.name).unlink)
        return Path(tmp.name)

    def test_floor_reads_the_deployment_target(self):
        self.assertEqual(assets.deployment_floor(self.package(".macOS(.v13)")), 13)
        self.assertEqual(assets.deployment_floor(self.package('.macOS("15.0")')), 15)
        self.assertEqual(assets.deployment_floor(self.package('.macOS("14.0.0")')), 14)
        self.assertIn(assets.deployment_floor(), range(13, 100))
        for platform in (".iOS(.v17)", ".macOS(.v12)", ".macOS(.v14_4)", '.macOS("14.4")', '.macOS("15.0.1")'):
            with self.assertRaises(ValueError):
                assets.deployment_floor(self.package(platform))

    def test_legacy_unfloored_name_exists_only_at_the_legacy_floor(self):
        self.assertEqual(assets.pinned_names("2026.11.1", "arm64", 13), [
            "AgentRelay-2026.11.1-macOS13-arm64.dmg",
            "AgentRelay-2026.11.1-macOS-arm64.dmg",
        ])
        self.assertEqual(assets.pinned_names("2026.11.1-beta.2", "x64", 15), [
            "AgentRelay-2026.11.1-beta.2-macOS15-x64.dmg",
        ])
        with self.assertRaises(ValueError):
            assets.pinned_names("2026.11.1", "ppc", 13)

    def bridge(self, image=b"bridge image", checksum=None, fail=None):
        """BridgeImages over a fake release, recording every URL it fetches."""
        fetched = []

        def download(url):
            fetched.append(url)
            if fail and url.endswith(fail):
                raise ValueError(f"could not download {url}")
            name = url.rsplit("/", 1)[1]
            if name.endswith(".sha256"):
                digest = checksum or hashlib.sha256(image).hexdigest()
                return f"{digest}  {name.removesuffix('.sha256')}\n".encode()
            return image

        return assets.BridgeImages(download), fetched

    def stage_cli(self, platform, arch="x64", bridge=None):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        source = Path(tmp.name, "built.dmg")
        source.write_bytes(b"disk image")
        dest = Path(tmp.name, "assets")
        with contextlib.redirect_stdout(io.StringIO()) as printed:
            assets.main(["2026.11.1", arch, str(source), str(dest),
                         "--package-swift", str(self.package(platform))], bridge)
        return printed.getvalue().split(), dest

    def test_floor_13_stages_the_built_image_under_floored_legacy_and_stable_names(self):
        bridge, fetched = self.bridge()
        printed, dest = self.stage_cli(".macOS(.v13)", bridge=bridge)
        expected = ["AgentRelay-2026.11.1-macOS13-x64.dmg",
                    "AgentRelay-2026.11.1-macOS-x64.dmg", "AgentRelay-macOS-x64.dmg"]
        self.assertEqual(printed, expected)
        self.assertEqual(sorted(p.name for p in dest.iterdir()), sorted(expected))
        self.assertTrue(all(p.read_bytes() == b"disk image" for p in dest.iterdir()))
        self.assertEqual(fetched, [])

    def test_floor_15_stages_the_bridge_image_under_the_legacy_name(self):
        bridge, fetched = self.bridge()
        printed, dest = self.stage_cli('.macOS("15.0")', bridge=bridge)
        self.assertEqual(printed, ["AgentRelay-2026.11.1-macOS15-x64.dmg",
                                   "AgentRelay-2026.11.1-macOS-x64.dmg", "AgentRelay-macOS-x64.dmg"])
        self.assertEqual(sorted(p.name for p in dest.iterdir()), sorted([
            *printed, "AgentRelay-2026.11.1-macOS-x64.dmg.sha256"]))
        for name in ("AgentRelay-2026.11.1-macOS15-x64.dmg", "AgentRelay-macOS-x64.dmg"):
            self.assertEqual(Path(dest, name).read_bytes(), b"disk image")
        self.assertEqual(Path(dest, "AgentRelay-2026.11.1-macOS-x64.dmg").read_bytes(), b"bridge image")
        self.assertEqual(
            Path(dest, "AgentRelay-2026.11.1-macOS-x64.dmg.sha256").read_text(),
            f"{hashlib.sha256(b'bridge image').hexdigest()}  AgentRelay-2026.11.1-macOS-x64.dmg\n")
        release = f"https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/v{assets.BRIDGE_VERSION}"
        self.assertEqual(fetched, [
            f"{release}/AgentRelay-{assets.BRIDGE_VERSION}-macOS-x64.dmg.sha256",
            f"{release}/AgentRelay-{assets.BRIDGE_VERSION}-macOS-x64.dmg",
        ])

    def test_a_profile_stages_the_built_image_under_every_name_and_never_bridges(self):
        # A profile has no updaters older than the floor-aware ones, so its
        # legacy name carries the built image and nothing is downloaded.
        bridge, fetched = self.bridge()
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        source = Path(tmp.name, "built.dmg")
        source.write_bytes(b"disk image")
        dest = Path(tmp.name, "assets")
        with contextlib.redirect_stdout(io.StringIO()) as printed:
            assets.main(["2026.11.1", "x64", str(source), str(dest), "--asset-prefix", "Desktop-acme-7",
                         "--package-swift", str(self.package('.macOS("15.0")'))], bridge)
        expected = ["Desktop-acme-7-2026.11.1-macOS15-x64.dmg",
                    "Desktop-acme-7-2026.11.1-macOS-x64.dmg", "Desktop-acme-7-macOS-x64.dmg"]
        self.assertEqual(printed.getvalue().split(), expected)
        self.assertEqual(sorted(p.name for p in dest.iterdir()), sorted(expected))
        self.assertTrue(all(p.read_bytes() == b"disk image" for p in dest.iterdir()))
        self.assertEqual(fetched, [])
        self.assertFalse(assets.bridges(15, "Desktop-acme-7"))
        self.assertTrue(assets.bridges(15))
        for bad in ("Acme Desktop", "Desktop-Acme", "AgentRelay-x"):
            with self.subTest(prefix=bad), self.assertRaises(ValueError):
                assets.pinned_names("2026.11.1", "x64", 15, bad)

    def test_bridge_images_download_once_per_architecture(self):
        bridge, fetched = self.bridge()
        for arch in ("arm64", "arm64", "x64", "x64"):
            self.stage_cli('.macOS("15.0")', arch=arch, bridge=bridge)
        self.assertEqual(len(fetched), 4)
        self.assertEqual([url.rsplit("/", 1)[1] for url in fetched][1::2], [
            f"AgentRelay-{assets.BRIDGE_VERSION}-macOS-arm64.dmg",
            f"AgentRelay-{assets.BRIDGE_VERSION}-macOS-x64.dmg",
        ])

    def test_a_bridge_that_fails_to_download_or_verify_stages_nothing(self):
        for bridge, _ in [
            self.bridge(checksum="0" * 64),
            self.bridge(checksum="not-a-digest"),
            self.bridge(fail=".dmg"),
            self.bridge(fail=".sha256"),
        ]:
            with tempfile.TemporaryDirectory() as tmp:
                source = Path(tmp, "built.dmg")
                source.write_bytes(b"disk image")
                dest = Path(tmp, "assets")
                with self.assertRaises(ValueError):
                    assets.stage("2026.11.1", "arm64", source, dest, 15, bridge)
                self.assertFalse(dest.exists())

    def test_fetch_reports_unreachable_urls_as_staging_errors(self):
        with self.assertRaises(ValueError):
            assets.fetch("http://127.0.0.1:9/AgentRelay.dmg")

    def test_stage_requires_the_build_artifact(self):
        with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
            assets.stage("2026.11.1", "arm64", Path(tmp, "missing.dmg"), Path(tmp), 13)


class CloudFeedNotesTests(unittest.TestCase):
    """A cloud feed (proposed Cloud contract, relay-control docs/desktop-feed.md
   ): versions are allocated against its listing, with no
    leading zero in the month, and its notes link nowhere."""
    script = Path(__file__).with_name("release-notes.py")

    def listing(self, directory, *entries):
        path = Path(directory) / "feed-listing.json"
        versions = [{"version": version, "channel": "beta" if "-beta." in version else "stable",
                     "state": state, "origin": "publish", "published_at": None, "current": False}
                    for version, state in entries]
        path.write_text(json.dumps({"versions": versions, "next_page": None}))
        return path

    def test_allocation_counts_drafts_and_never_pads_the_month(self):
        with tempfile.TemporaryDirectory() as temporary:
            listing = self.listing(temporary, ("2026.3.1", "published"), ("2026.3.2", "draft"),
                                   ("2026.3.3-beta.1", "published"))
            allocate = release_notes.allocate_cloud
            self.assertEqual(allocate(listing, "", "stable", "2026.3"), "2026.3.3")
            self.assertEqual(allocate(listing, "", "beta", "2026.3"), "2026.3.3-beta.2")
            self.assertEqual(allocate(listing, "", "stable", "2026.4"), "2026.4.1")
            self.assertEqual(allocate(listing, "2026.3.4", "stable"), "2026.3.4")
            for requested in ("2026.3.2", "2026.03.4"):
                with self.subTest(requested=requested), self.assertRaises(ValueError):
                    allocate(listing, requested, "stable")
            current = release_notes.allocate_cloud(listing, "", "stable")
            self.assertRegex(current, r"^[0-9]{4}\.(?:[1-9]|1[0-2])\.[0-9]+$")
            Path(listing).write_text(json.dumps({"releases": []}))
            with self.assertRaisesRegex(ValueError, "not a feed listing"):
                allocate(listing, "", "stable", "2026.3")

    def test_allocate_cli_prints_the_resolved_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            listing = self.listing(temporary, ("2026.3.1", "draft"))
            result = subprocess.run([sys.executable, str(self.script), "2026.3.2", "--allocate", "--feed", "cloud",
                                     "--feed-listing", str(listing), "--channel", "beta"],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.splitlines(), ["version=2026.3.2-beta.1", "tag=v2026.3.2-beta.1",
                                                          "channel=beta", "title=Beta v2026.3.2-beta.1"])
            result = subprocess.run([sys.executable, str(self.script), "--allocate", "--feed-listing", str(listing)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)

    def test_as_given_uses_the_explicit_version_without_a_listing(self):
        given = release_notes.given_cloud
        self.assertEqual(given("2026.3.2", "stable"), "2026.3.2")
        self.assertEqual(given("v2026.3.2-beta.4", "beta"), "2026.3.2-beta.4")
        # The version and channel rules still hold; nothing is allocated.
        for requested, channel in (("2026.3.2", "beta"), ("2026.3.2-beta.1", "stable"), ("2026.03.2", "stable"),
                                   ("2026.3", "stable"), ("2026.3.2-beta.0", "beta"), ("", "stable")):
            with self.subTest(requested=requested, channel=channel), self.assertRaises(ValueError):
                given(requested, channel)
        command = [sys.executable, str(self.script), "--allocate", "--feed", "cloud", "--as-given"]
        result = subprocess.run(command + ["2026.3.2-beta.1", "--channel", "beta"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["version=2026.3.2-beta.1", "tag=v2026.3.2-beta.1",
                                                      "channel=beta", "title=Beta v2026.3.2-beta.1"])
        with tempfile.TemporaryDirectory() as temporary:
            listing = self.listing(temporary)
            # Without a version, or with a listing as well, it is refused.
            for extra in ([], ["2026.3.2", "--feed-listing", str(listing)]):
                result = subprocess.run(command + extra, capture_output=True, text=True)
                self.assertEqual(result.returncode, 2, extra)
                self.assertEqual(result.stdout, "", extra)
        result = subprocess.run(command + ["2026.3.2", "--channel", "beta"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    def test_cloud_notes_link_nowhere_and_refuse_a_taken_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            changelog = Path(temporary) / "CHANGELOG.md"
            changelog.write_text("# Changelog\n\n## Unreleased\n\n- A change.\n")
            listing = self.listing(temporary, ("2026.3.1", "draft"))
            output = Path(temporary) / "notes.md"
            base = [sys.executable, str(self.script), "--changelog", str(changelog), "--unreleased",
                    "--feed", "cloud", "--feed-listing", str(listing)]
            result = subprocess.run(base[:2] + ["2026.3.2"] + base[2:] + [
                "--asset-prefix", "Desktop-x7k2q", "--probe-version", "1.2.3", "--output", str(output),
                "--repository", ""], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            notes = output.read_text()
            self.assertIn("- A change.", notes)
            self.assertIn("**Channel: Stable**", notes)
            self.assertNotIn("github.com", notes)
            self.assertNotIn("https://", notes)
            for version in ("2026.3.1", "2026.03.2"):
                result = subprocess.run(base[:2] + [version] + base[2:] + ["--check"], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0, version)
                self.assertNotEqual(result.returncode, 3, version)


if __name__ == "__main__":
    unittest.main()

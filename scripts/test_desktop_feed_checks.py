#!/usr/bin/env python3
"""The publisher bridge's fail-closed checks, against the fake feed of
test_desktop_feed_publish.py (proposed Cloud contract, relay-control
docs/desktop-feed.md). Nothing here reaches a network.
"""

import os
import secrets
import unittest

import test_desktop_feed_publish as harness

feed = harness.feed
PREFIX, STABLE, BETA = harness.PREFIX, harness.STABLE, harness.BETA
LINUX_DEB = f"{PREFIX}-{STABLE}-Linux-x64.deb"


def first(key, value):
    """Set `key` of the first expected asset (or manifest asset) to value."""
    def change(payload):
        entries = payload.get("expected_assets") or payload["manifest"]["assets"]
        entries[0][key] = value
    return change


def setter(key, value, inside=None):
    def change(payload):
        (payload[inside] if inside else payload)[key] = value
    return change


class ReplyCheckTests(harness.FeedHarness):
    def assert_fails(self, kind, change, message, state="draft"):
        self.feed.tamper = {kind: change}
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1, err)
        self.assertIn(message, err)
        self.assertNotIn("Traceback", err)
        self.assertEqual(self.feed.releases[STABLE]["state"], state)

    def test_a_draft_that_differs_from_the_declaration_fails(self):
        cases = {
            "another version": setter("version", "2026.11.9"),
            "not a draft": setter("state", "published"),
            "an import draft": setter("origin", "import"),
            "another sha256": first("sha256", "0" * 64),
            "another size": first("size", 1),
            "another arch": first("arch", "arm64"),
            "an asset missing": lambda payload: payload["expected_assets"].pop(),
        }
        for label, change in cases.items():
            with self.subTest(label):
                self.feed.releases.clear()
                self.assert_fails("draft", change, "does not declare exactly this release's assets")
                self.assertFalse(any(method == "PUT" for method, _, _ in self.feed.requests))

    def test_an_upload_receipt_that_differs_fails_before_publishing(self):
        cases = {"another name": setter("name", LINUX_DEB), "another sha256": setter("sha256", "0" * 64),
                 "another size": setter("size", 1), "an invalid id": setter("id", "../a")}
        for label, change in cases.items():
            with self.subTest(label):
                self.feed.releases.clear()
                self.feed.requests.clear()
                self.assert_fails("receipt", change, "does not match it; the draft stays unpublished")
                self.assertFalse(any(path.endswith("/publish") for _, path, _ in self.feed.requests))

    def test_an_envelope_that_differs_fails(self):
        cases = {
            "no signature": (lambda payload: payload.pop("signature"), "no manifest and signature"),
            "no manifest": (setter("manifest", None), "no manifest and signature"),
            "another profile": (setter("profile", "other1", "manifest"), "another profile, version or channel"),
            "another version": (setter("version", "2026.11.9", "manifest"), "another profile, version or channel"),
            "another channel": (setter("channel", "beta", "manifest"), "another profile, version or channel"),
            "another sha256": (first("sha256", "0" * 64), "assets differ from the declared ones"),
            "an asset missing": (lambda payload: payload["manifest"]["assets"].pop(),
                                 "assets differ from the declared ones"),
            "an invalid id": (first("id", "b_1"), "an asset without a valid id"),
        }
        for label, (change, message) in cases.items():
            with self.subTest(label):
                self.feed.releases.clear()
                self.assert_fails("envelope", change, message, state="published")


class ReplayTests(harness.FeedHarness):
    def test_a_rerun_after_a_lost_publish_reply_is_done_without_sending_more(self):
        self.feed.lose_reply = True
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("HTTP 504", err)
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")
        self.feed.lose_reply = False
        self.feed.requests.clear()
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"v{STABLE} is already published on the stable channel (at 2026-11-07T12:00:00Z)", out)
        # The first listing reads the signer's limit before the create.
        self.assertEqual([method for method, _, _ in self.feed.requests], ["GET", "POST", "GET"])
        self.assertIn("include_drafts=true", self.feed.requests[2][1])

    def test_a_version_published_by_an_import_is_refused(self):
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        self.feed.releases[STABLE]["origin"] = "import"
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn(f"v{STABLE} is already published as stable by import", err)

    def test_a_draft_with_another_declaration_is_refused(self):
        self.feed.signer = False
        self.assertEqual(self.publish(STABLE, "stable")[0], 1)
        (self.temp / STABLE / LINUX_DEB).write_bytes(b"rebuilt\n")
        self.feed.signer = True
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("HTTP 409 version_exists", err)
        self.assertIn(f"a draft of v{STABLE} declares other notes or assets", err)
        self.assertEqual(self.feed.releases[STABLE]["state"], "draft")


class RefusalTests(harness.FeedHarness):
    def assert_refused(self, version, channel, names=None, message="", **extra):
        if not (self.temp / version).exists():
            self.stage(version)
        if names is not None:
            (self.temp / f"{version}.txt").write_text("\n".join(names) + "\n")
        status, _, err = self.publish(version, channel, **extra)
        self.assertEqual(status, 2, err)
        self.assertIn(message, err)
        self.assertEqual(self.feed.requests, [])

    def test_a_version_on_the_other_channel_is_refused_before_any_request(self):
        for version, channel in ((BETA, "stable"), (STABLE, "beta")):
            with self.subTest(version=version, channel=channel):
                self.assert_refused(version, channel, message="a beta version needs channel beta")

    def test_duplicate_empty_symlinked_and_floored_assets_are_refused(self):
        directory = self.stage(STABLE)
        names = harness.package_names(STABLE)
        (directory / "install.sh").write_bytes(b"")
        target = directory / names[4]
        os.replace(target, self.temp / "outside")
        os.symlink(self.temp / "outside", target)
        floored = f"{PREFIX}-{STABLE}-macOS09-arm64.dmg"
        (directory / floored).write_bytes(b"image\n")
        for label, listed, message in (
                ("duplicate", [names[1], names[2], names[1]], "is listed twice"),
                ("empty", names[:2], "install.sh is empty"),
                ("symlink", [names[1], names[4]], "is not a regular file"),
                ("floor", [names[1], floored], "a macOS floor outside 10 to 99")):
            with self.subTest(label):
                self.assert_refused(STABLE, "stable", listed, message)

    def test_a_token_with_spaces_or_non_ascii_is_refused(self):
        for bad in ("sh" + "_dfp_ " + secrets.token_hex(20), "sh" + "_dfp_é" + secrets.token_hex(20),
                    "sh" + "_dfp_\n" + secrets.token_hex(20)):
            with self.subTest(repr(bad[:6])):
                self.assert_refused(STABLE, "stable", message="must be printable ASCII without spaces",
                                    token=bad)

    def test_rate_limit_waits_are_capped_at_60_seconds(self):
        waits = []
        responses = iter([(429, "3600"), (429, "soon"), (429, "7"), (200, None)])

        class Reply:
            def __init__(self, status, retry):
                self.status, self.retry = status, retry

            def __enter__(self):
                if self.status == 429:
                    raise feed.urllib.error.HTTPError("u", 429, "limited", {"Retry-After": self.retry},
                                                      None)
                return self

            def __exit__(self, *args):
                return False

            def read(self):
                return b'{"versions": [], "next_page": null}'

        client = feed.Client(harness.SITE, harness.PROFILE, self.token, sleep=waits.append, out=lambda _: None,
                             open_url=lambda request, timeout: Reply(*next(responses)))
        self.assertEqual(client.listing(), {"versions": []})
        self.assertEqual(waits, [60, 30, 7])


if __name__ == "__main__":
    harness.unittest.main()

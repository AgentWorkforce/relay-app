#!/usr/bin/env python3
"""Tests for desktop-feed-publish.py against an in-process fake feed.

The fake follows the draft rules of the proposed Cloud contract (relay-control
docs/desktop-feed.md) and listens on 127.0.0.1 only. Nothing here
reaches a network.
"""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import secrets
import string
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
SCRIPT = SCRIPTS / "desktop-feed-publish.py"
SITE = "https://feed.invalid"
PROFILE = "x7k2q"
PREFIX = "Desktop-x7k2q"
BASE = "/cloud/api/v1/desktop/feed"
STABLE = "2026.11.5"
BETA = "2026.11.6-beta.1"
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_+-]*(?:\.[A-Za-z0-9_+-]+)*\Z")


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


cli = load("desktop_feed_publish", SCRIPT)
feed = cli.feed


def package_names(version, prefix=PREFIX):
    """The fixed cloud set release-candidate.py cloud-assets lists: 16."""
    names = ["install.sh", "install.ps1"]
    for arch in ("arm64", "x64"):
        names += [f"{prefix}-{version}-{rest}" for rest in (
            f"macOS15-{arch}.dmg", f"Linux-{arch}.tar.gz", f"Linux-{arch}.deb",
            f"Windows-{arch}.zip", f"Windows-{arch}-Setup.exe", f"Linux-{arch}-probe.tar.gz",
            f"Windows-{arch}-probe.zip")]
    return names


def every_package_name(version):
    """Every package the publisher can describe (the macOS probes and unfloored
    images too): 20, more than the signer's 4096 bytes hold."""
    names = package_names(version)
    for arch in ("arm64", "x64"):
        names += [f"{PREFIX}-{version}-{rest}" for rest in (
            f"macOS-{arch}.dmg", f"macOS-{arch}-probe.tar.gz")]
    return names


class Feed:
    """A fake profile feed: drafts, uploads and publishing, by the rules."""

    def __init__(self, token):
        self.token = token
        self.profile = PROFILE
        # The listing's limits; max_canonical_bytes is the signer's cap.
        self.limits = {}
        self.entitled = True
        self.signer = True
        self.releases = {}
        self.requests = []
        # The raw bytes of every POST /releases body, as received.
        self.created = []
        self.fail_upload = set()
        self.forget = set()
        self.leak = False
        self.page_size = 50
        self.redirect = False
        self.limited = 0
        # (status, code) answered to every request, as a site without the
        # feed route answers (404, "not_found").
        self.answer = None
        # Knobs for the client's checks: "draft", "receipt" or "envelope" maps
        # to a function that alters that reply; lose_reply drops a publish's.
        self.tamper = {}
        self.lose_reply = False
        # (canonical_bytes, max_canonical_bytes) to answer the next create
        # with 422 manifest_too_large, after the version_exists check.
        self.too_large = None
        # The same, answered to POST /releases/{v}/publish after the asset
        # set check: the draft and its uploads stay.
        self.too_large_at_publish = None

    def tampered(self, kind, payload):
        if kind in self.tamper:
            payload = json.loads(json.dumps(payload))
            self.tamper[kind](payload)
        return payload

    def error(self, status, code, **extra):
        message = f"refused {self.token}" if self.leak else "refused"
        return status, {"error": code, "message": message, **extra}, {}

    def handle(self, method, path, headers, body):
        self.requests.append((method, path, dict(headers)))
        if self.redirect:
            return 302, {}, {"Location": "http://127.0.0.2/elsewhere"}
        if self.answer:
            return self.error(*self.answer)
        if self.limited:
            self.limited -= 1
            return 429, {"error": "rate_limited", "retry_after_seconds": 1}, {"Retry-After": "1"}
        if headers.get("Authorization") != f"Bearer {self.token}":
            return self.error(401, "unauthenticated")
        if headers.get("X-Agent-Relay-Profile") != self.profile:
            return self.error(400, "profile_header_required")
        if not self.entitled:
            return self.error(403, "not_entitled")
        url = urllib.parse.urlsplit(path)
        parts = url.path[len(BASE):].split("/")[1:]
        if method == "GET" and parts == ["releases"]:
            return self.listing(urllib.parse.parse_qs(url.query))
        if method == "POST" and parts == ["releases"]:
            self.created.append(bytes(body))
            return self.create(json.loads(body))
        version = urllib.parse.unquote(parts[1]) if len(parts) > 1 else ""
        release = self.releases.get(version)
        if release is None:
            return self.error(404, "draft_not_found")
        if release["state"] != "draft":
            return self.error(409, "not_a_draft")
        if method == "PUT" and len(parts) == 4 and parts[2] == "assets":
            return self.upload(release, urllib.parse.unquote(parts[3]), headers, body)
        if method == "POST" and parts[2:] == ["publish"]:
            return self.publish(release, json.loads(body or b"{}"))
        return self.error(404, "not_found")

    def listing(self, query):
        if query.get("include_drafts") != ["true"]:
            return self.error(403, "wrong_credential")
        channel = query.get("channel", [""])[0]
        rows = [{"version": v, "channel": r["channel"], "state": r["state"], "origin": r.get("origin", "publish"),
                 "published_at": r.get("published_at"), "current": r.get("current", False)}
                for v, r in sorted(self.releases.items()) if not channel or r["channel"] == channel]
        page = int(query.get("page", ["1"])[0])
        chunk = rows[(page - 1) * self.page_size:page * self.page_size]
        more = len(rows) > page * self.page_size
        reply = {"versions": chunk, "next_page": page + 1 if more else None}
        if self.limits is not None:
            reply["limits"] = self.limits
        return 200, reply, {}

    def create(self, body):
        assets = body.get("expected_assets", [])
        for asset in assets:
            if "id" in asset or not NAME.match(asset["name"]):
                return self.error(422, "invalid_asset")
            if asset["name"] not in ("install.sh", "install.ps1") and not asset["name"].startswith(
                    f"Desktop-{self.profile}-{body['version']}-"):
                return self.error(422, "invalid_asset")
        existing = self.releases.get(body["version"])
        if existing:
            if existing["state"] == "draft" and existing["body"] == body:
                return 200, self.tampered("draft", self.draft(existing)), {}
            return self.error(409, "version_exists")
        if self.too_large:
            measured, cap = self.too_large
            return 422, {"error": "manifest_too_large", "message": "the manifest is too large to sign",
                         "canonical_bytes": measured, "max_canonical_bytes": cap}, {}
        self.releases[body["version"]] = {"body": body, "channel": body["channel"], "state": "draft",
                                          "stored": {}}
        return 201, self.tampered("draft", self.draft(self.releases[body["version"]])), {}

    def draft(self, release):
        body = release["body"]
        return {"version": body["version"], "channel": body["channel"], "state": "draft", "origin": "publish",
                "notes": body["notes"], "created_at": "2026-11-07T00:00:00Z",
                "expected_assets": [dict(a, uploaded=a["name"] in release["stored"],
                                         id=release["stored"].get(a["name"], {}).get("id"))
                                    for a in body["expected_assets"]]}

    def upload(self, release, name, headers, data):
        declared = {a["name"]: a for a in release["body"]["expected_assets"]}
        if name not in declared:
            return self.error(422, "asset_not_declared")
        digest = hashlib.sha256(data).hexdigest()
        stored = release["stored"].get(name)
        if stored:
            if stored["sha256"] != digest:
                return self.error(409, "asset_conflict")
            return 200, stored, {}
        if name in self.fail_upload:
            self.fail_upload.discard(name)
            return self.error(500, "internal")
        asset = declared[name]
        if (headers.get("X-Content-SHA256") != asset["sha256"]
                or headers.get("Content-Length") != str(asset["size"])):
            return self.error(422, "asset_mismatch")
        if digest != asset["sha256"] or len(data) != asset["size"]:
            return self.error(422, "asset_mismatch")
        # Cloud's asset ids: "a_" and 8 base62 characters.
        base62 = string.digits + string.ascii_letters
        receipt = {"id": "a_" + "".join(secrets.choice(base62) for _ in range(8)), "name": name, "size": len(data), "sha256": digest}
        if name not in self.forget:
            release["stored"][name] = receipt
        return 201, self.tampered("receipt", receipt), {}

    def publish(self, release, body):
        assets = release["body"]["expected_assets"]
        missing = [a["name"] for a in assets if a["name"] not in release["stored"]]
        if missing:
            return self.error(422, "assets_incomplete", missing=missing, mismatched=[])
        if self.too_large_at_publish:
            measured, cap = self.too_large_at_publish
            return 422, {"error": "manifest_too_large", "message": "the manifest is too large to sign",
                         "canonical_bytes": measured, "max_canonical_bytes": cap}, {}
        if not self.signer:
            return self.error(503, "signer_unavailable")
        release.update(state="published", published_at="2026-11-07T12:00:00Z", latest=body.get("latest"),
                       publish_body=body)
        manifest = {"schema": 1, "profile": self.profile, "channel": release["channel"],
                    "version": release["body"]["version"], "published_at": release["published_at"],
                    "notes": release["body"]["notes"], "download_hosts": [],
                    "assets": [dict(a, id=release["stored"][a["name"]]["id"]) for a in assets]}
        if self.lose_reply:
            return self.error(504, "gateway_timeout")
        return 200, self.tampered("envelope", {"manifest": manifest, "signature": "c2ln", "key_id": "k1"}), {}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def serve(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        status, payload, extra = self.server.feed.handle(self.command, self.path, self.headers, body)
        data = json.dumps(payload).encode()
        self.send_response(status)
        for key, value in extra.items():
            self.send_header(key, value)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = serve


class FeedHarness(unittest.TestCase):
    """The fake feed and the helpers, without tests (shared with
    test_desktop_feed_checks.py)."""

    def setUp(self):
        self.token = "sh" + "_dfp_" + secrets.token_hex(20)
        self.feed = Feed(self.token)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.feed = self.feed
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.local = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.temp)], check=True))

    def open_url(self, request, timeout):
        self.assertTrue(request.full_url.startswith(SITE + BASE))
        request.full_url = self.local + request.full_url[len(SITE):]
        return feed.default_open(request, timeout)

    def stage(self, version, names=None):
        directory = self.temp / version
        directory.mkdir()
        names = names or package_names(version)
        for name in names:
            (directory / name).write_bytes(f"{name} {secrets.token_hex(4)}\n".encode())
        (self.temp / f"{version}.txt").write_text("\n".join(names) + "\n")
        (self.temp / "notes.md").write_text("What's new\n")
        return directory

    def run_cli(self, *argv, token=None):
        stdout, stderr = io.StringIO(), io.StringIO()
        environ = {feed.TOKEN_ENV: self.token if token is None else token}
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = cli.main(list(argv), environ, self.open_url)
        output = stdout.getvalue() + stderr.getvalue()
        self.assertNotIn(self.token, output)
        return status, stdout.getvalue(), stderr.getvalue()

    def publish(self, version, channel, *extra, token=None):
        if not (self.temp / version).exists():
            self.stage(version)
        return self.run_cli("publish", "--site", SITE, "--profile", PROFILE, "--feed", "cloud",
                            "--asset-prefix", PREFIX, "--version", version, "--channel", channel,
                            "--notes", str(self.temp / "notes.md"), "--directory", str(self.temp / version),
                            "--assets", str(self.temp / f"{version}.txt"), *extra, token=token)


class FeedTest(FeedHarness):
    def test_dry_run_prints_the_plan_and_sends_nothing(self):
        status, out, _ = self.publish(STABLE, "stable", "--dry-run")
        self.assertEqual(status, 0)
        self.assertEqual(self.feed.requests, [])
        self.assertIn(f"POST {BASE}/releases\n", out)
        self.assertIn(f"PUT {BASE}/releases/{STABLE}/assets/{PREFIX}-{STABLE}-macOS15-arm64.dmg", out)
        self.assertIn(f"POST {BASE}/releases/{STABLE}/publish", out)
        self.assertIn("Authorization: Bearer ***", out)
        self.assertIn(f"X-Agent-Relay-Profile: {PROFILE}", out)
        self.assertIn('"latest": true', out)
        self.assertIn('"os_floor": 15', out)
        self.assertIn('"kind": "install-script"', out)
        self.assertEqual(out.count("X-Content-SHA256: "), 16)

    def test_both_installers_declare_their_platform_as_install_scripts(self):
        # Neither carries a version; each declares x64, as install.sh did.
        self.assertEqual(feed.describe("install.sh", PREFIX, STABLE),
                         {"platform": "Linux", "arch": "x64", "kind": "install-script"})
        self.assertEqual(feed.describe("install.ps1", PREFIX, STABLE),
                         {"platform": "Windows", "arch": "x64", "kind": "install-script"})
        for bad in ("Install.ps1", "install.ps1.sha256", "install.bat"):
            with self.subTest(bad=bad), self.assertRaises(feed.Refusal):
                feed.describe(bad, PREFIX, STABLE)

    def test_stable_publishes_as_latest_with_every_declared_asset(self):
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        release = self.feed.releases[STABLE]
        self.assertEqual((release["state"], release["publish_body"]), ("published", {"latest": True}))
        declared = {a["name"]: a for a in release["body"]["expected_assets"]}
        self.assertEqual(sorted(declared), sorted(package_names(STABLE)))
        self.assertEqual(declared[f"{PREFIX}-{STABLE}-macOS15-x64.dmg"]["os_floor"], 15)
        self.assertNotIn(f"{PREFIX}-{STABLE}-macOS-x64.dmg", declared)
        self.assertEqual(declared[f"{PREFIX}-{STABLE}-Windows-arm64-Setup.exe"]["kind"], "setup")
        self.assertIn(f"Published v{STABLE} at 2026-11-07T12:00:00Z.", out)
        for receipt in release["stored"].values():
            self.assertIn(f"{receipt['id']}  {receipt['name']}", out)
        for _, _, headers in self.feed.requests:
            self.assertEqual(headers["X-Agent-Relay-Profile"], PROFILE)

    def test_beta_publishes_without_latest(self):
        status, _, _ = self.publish(BETA, "beta")
        self.assertEqual(status, 0)
        self.assertEqual(self.feed.releases[BETA]["publish_body"], {})

    def test_a_rerun_after_a_failed_upload_resumes_the_draft(self):
        broken = f"{PREFIX}-{STABLE}-Linux-x64.deb"
        self.feed.fail_upload.add(broken)
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn(f"Uploading {broken} failed: HTTP 500", err)
        self.assertIn("the draft stays unpublished", err)
        self.assertEqual(self.feed.releases[STABLE]["state"], "draft")
        done = set(self.feed.releases[STABLE]["stored"])
        self.feed.requests.clear()
        status, out, _ = self.publish(STABLE, "stable")
        self.assertEqual(status, 0)
        self.assertIn("Resumed the draft", out)
        uploads = [urllib.parse.unquote(path.rsplit("/", 1)[1]) for method, path, _ in self.feed.requests
                   if method == "PUT"]
        self.assertEqual(sorted(uploads), sorted(set(package_names(STABLE)) - done))
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")

    def test_request_out_holds_the_exact_create_body_sent(self):
        request = self.temp / "publish-request.json"
        status, out, _ = self.publish(STABLE, "stable", "--request-out", str(request))
        self.assertEqual(status, 0)
        self.assertIn(f"Wrote the create request body to {request}.", out)
        self.assertEqual(len(self.feed.created), 1)
        self.assertEqual(request.read_bytes(), self.feed.created[0])
        body = json.loads(request.read_bytes())
        self.assertEqual((body["version"], body["channel"]), (STABLE, "stable"))
        self.assertEqual(body, self.feed.releases[STABLE]["body"])

    def test_request_out_is_written_on_a_rerun_of_a_published_version(self):
        first = self.temp / "first.json"
        status, _, _ = self.publish(STABLE, "stable", "--request-out", str(first))
        self.assertEqual(status, 0)
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")
        again = self.temp / "again.json"
        status, out, _ = self.publish(STABLE, "stable", "--request-out", str(again))
        self.assertEqual(status, 0)
        self.assertIn("already published", out)
        self.assertEqual(again.read_bytes(), first.read_bytes())
        self.assertEqual(again.read_bytes(), self.feed.created[-1])

    def test_a_publish_prints_the_key_id_the_canonical_digest_and_the_signature(self):
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        lines = out.splitlines()
        published = lines.index(f"Published v{STABLE} at 2026-11-07T12:00:00Z.")
        release = self.feed.releases[STABLE]
        manifest = {"schema": 1, "profile": PROFILE, "channel": "stable", "version": STABLE,
                    "published_at": "2026-11-07T12:00:00Z", "notes": release["body"]["notes"],
                    "download_hosts": [], "assets": [dict(a, id=release["stored"][a["name"]]["id"])
                                                     for a in release["body"]["expected_assets"]]}
        digest = hashlib.sha256(feed.canonical(manifest)).hexdigest()
        self.assertEqual(feed.manifest_digest(manifest), digest)
        self.assertEqual(lines[published + 1:published + 4], [
            "Signed by key_id k1.",
            f"Signed manifest canonical SHA-256: {digest}",
            "Signature: c2ln",
        ])
        self.assertNotIn("Wrote the published envelope", out)

    def test_envelope_out_holds_the_signed_envelope_with_sorted_keys(self):
        envelope = self.temp / "feed-envelope.json"
        status, out, _ = self.publish(STABLE, "stable", "--envelope-out", str(envelope))
        self.assertEqual(status, 0)
        self.assertIn(f"Wrote the published envelope to {envelope}.", out)
        written = json.loads(envelope.read_text(encoding="utf-8"))
        self.assertEqual((written["key_id"], written["signature"]), ("k1", "c2ln"))
        self.assertEqual(written["manifest"]["version"], STABLE)
        self.assertEqual(envelope.read_text(encoding="utf-8"),
                         json.dumps(written, indent=2, sort_keys=True) + "\n")
        self.assertIn(f"Signed manifest canonical SHA-256: {feed.manifest_digest(written['manifest'])}", out)

    def test_an_unwritable_envelope_file_is_a_warning_not_a_failure(self):
        envelope = self.temp / "missing-directory" / "feed-envelope.json"
        status, out, err = self.publish(STABLE, "stable", "--envelope-out", str(envelope))
        self.assertEqual(status, 0)
        self.assertIn(f"Published v{STABLE}", out)
        self.assertIn("Signed by key_id k1.", out)
        self.assertIn("Signed manifest canonical SHA-256:", out)
        self.assertIn("Signature: c2ln", out)
        self.assertNotIn("Wrote the published envelope", out)
        self.assertIn(f"Warning: could not write the published envelope to {envelope}:", err)
        self.assertFalse(envelope.exists())

    def test_a_replay_prints_and_writes_no_envelope(self):
        status, _, _ = self.publish(STABLE, "stable")
        self.assertEqual(status, 0)
        envelope = self.temp / "feed-envelope.json"
        status, out, _ = self.publish(STABLE, "stable", "--envelope-out", str(envelope))
        self.assertEqual(status, 0)
        self.assertIn("already published", out)
        self.assertFalse(envelope.exists())
        for line in ("Wrote the published envelope", "Signed by key_id", "Signed manifest canonical SHA-256",
                     "Signature:"):
            self.assertNotIn(line, out)

    def test_changed_bytes_after_planning_are_a_mismatch(self):
        directory = self.stage(STABLE)
        client = feed.Client(SITE, PROFILE, self.token, open_url=self.open_url, out=lambda _: None)
        names = package_names(STABLE)
        assets = feed.expected_assets(directory, names, PREFIX, STABLE)
        target = directory / names[3]
        target.write_bytes(bytes(len(target.read_bytes())))
        with self.assertRaisesRegex(feed.FeedError, "asset_mismatch"):
            feed.publish(client, directory, STABLE, "stable", "notes", assets)
        self.assertEqual(self.feed.releases[STABLE]["state"], "draft")

    def test_the_feed_enforces_declaration_and_conflict(self):
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        client = feed.Client(SITE, PROFILE, self.token, open_url=self.open_url, out=lambda _: None)
        self.feed.releases[STABLE]["state"] = "draft"
        path = self.temp / "other"
        path.write_bytes(b"other bytes")
        sha, size = feed.file_digest(path)
        headers = {"X-Content-SHA256": sha, "Content-Length": str(size)}
        undeclared = client.call("PUT", f"/releases/{STABLE}/assets/{PREFIX}-{STABLE}-extra.zip",
                                 headers=headers, upload=path)
        self.assertEqual((undeclared.status, undeclared.code), (422, "asset_not_declared"))
        conflict = client.call("PUT", f"/releases/{STABLE}/assets/install.sh", headers=headers, upload=path)
        self.assertEqual((conflict.status, conflict.code), (409, "asset_conflict"))
        same = client.call("PUT", f"/releases/{STABLE}/assets/install.sh", upload=self.temp / STABLE / "install.sh",
                           headers={"Content-Length": str((self.temp / STABLE / "install.sh").stat().st_size)})
        self.assertEqual(same.status, 200)
        self.feed.releases[STABLE]["state"] = "published"
        again = client.call("PUT", f"/releases/{STABLE}/assets/install.sh", headers=headers, upload=path)
        self.assertEqual((again.status, again.code), (409, "not_a_draft"))
        gone = client.call("POST", "/releases/2026.1.1/publish", {})
        self.assertEqual((gone.status, gone.code), (404, "draft_not_found"))

    def test_a_version_published_under_another_channel_is_refused_as_version_exists(self):
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        self.feed.releases[STABLE]["channel"] = "beta"
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("HTTP 409 version_exists", err)
        self.assertIn(f"v{STABLE} is already published as beta by publish", err)

    def test_an_unlisted_candidate_whose_version_the_feed_holds_is_refused_before_any_create(self):
        # Its version was taken as given at prepare time: a version already
        # published (any channel, any origin) is never answered as a replay.
        for channel, origin in (("stable", "publish"), ("beta", "publish"), ("stable", "import")):
            with self.subTest(channel=channel, origin=origin):
                self.feed.releases.clear()
                self.assertEqual(self.publish(STABLE, "stable")[0], 0)
                self.feed.releases[STABLE].update(channel=channel, origin=origin)
                self.feed.requests.clear()
                status, out, err = self.publish(STABLE, "stable", "--unlisted")
                self.assertEqual(status, 1, out + err)
                self.assertIn(f"profile {PROFILE}: v{STABLE} (stable) was taken as given", err)
                self.assertIn(f"(published on the {channel} channel by {origin})", err)
                self.assertNotIn("already published on the stable channel", out)
                self.assertEqual({method for method, _, _ in self.feed.requests}, {"GET"})

    def test_an_unlisted_candidate_whose_version_is_absent_publishes(self):
        status, out, err = self.publish(STABLE, "stable", "--unlisted")
        self.assertEqual(status, 0, err)
        self.assertIn(f"Published v{STABLE} at 2026-11-07T12:00:00Z.", out)
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")
        self.assertEqual(self.feed.requests[0][0], "GET")
        self.assertIn("include_drafts=true", self.feed.requests[0][1])
        # The signer-limit pre-flight reads the listing's limits after the
        # unlisted check, still before the create.
        self.assertEqual(self.feed.requests[1][0], "GET")
        self.assertIn("include_drafts=true&channel=stable", self.feed.requests[1][1])
        self.assertEqual(self.feed.requests[2][:2], ("POST", f"{BASE}/releases"))
        # Its own earlier draft resumes; a dry run shows the listing first.
        self.feed.releases.clear()
        self.feed.signer = False
        self.assertEqual(self.publish(BETA, "beta", "--unlisted")[0], 1)
        self.feed.signer = True
        status, out, err = self.publish(BETA, "beta", "--unlisted")
        self.assertEqual(status, 0, err)
        self.assertIn(f"Resumed the draft of v{BETA}.", out)
        self.feed.requests.clear()
        status, out, _ = self.publish(STABLE, "stable", "--unlisted", "--dry-run")
        self.assertEqual((status, self.feed.requests), (0, []))
        self.assertLess(out.index(f"GET {BASE}/releases?include_drafts=true\n"), out.index(f"POST {BASE}/releases\n"))

    def test_an_unlisted_candidate_never_takes_version_exists_for_a_replay(self):
        # Published between the listing and the create, or another draft:
        # the 409 fails the publish instead of reporting it done.
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        directory = self.temp / STABLE
        client = feed.Client(SITE, PROFILE, self.token, open_url=self.open_url, out=lambda _: None)
        client.listing = lambda channel="": {"versions": []}
        assets = feed.expected_assets(directory, package_names(STABLE), PREFIX, STABLE)
        self.feed.requests.clear()
        with self.assertRaisesRegex(feed.FeedError, "HTTP 409 version_exists.*taken as given"):
            feed.publish(client, directory, STABLE, "stable", "What's new\n", assets, unlisted=True)
        # The only GET is the signer-limit pre-flight (the listing is stubbed).
        self.assertEqual([method for method, _, _ in self.feed.requests], ["GET", "POST"])
        # The same rerun of a listed candidate is still a lost reply's replay.
        self.assertIsNone(feed.publish(feed.Client(SITE, PROFILE, self.token, open_url=self.open_url,
                                                   out=lambda _: None),
                                       directory, STABLE, "stable", "What's new\n", assets))

    def test_incomplete_assets_list_what_is_missing(self):
        lost = f"{PREFIX}-{STABLE}-Windows-x64.zip"
        self.feed.forget.add(lost)
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn(f"assets_incomplete", err)
        self.assertIn(f"missing: {lost}; mismatched: none", err)

    def test_an_unavailable_signer_keeps_the_draft_and_fails(self):
        self.feed.signer = False
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("HTTP 503 signer_unavailable", err)
        self.assertEqual(self.feed.releases[STABLE]["state"], "draft")
        self.feed.signer = True
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)

    def test_not_entitled_and_unauthenticated_are_explained(self):
        self.feed.entitled = False
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("HTTP 403 not_entitled", err)
        self.assertIn("not entitled to a cloud feed", err)
        self.feed.entitled = True
        status, _, err = self.publish(STABLE, "stable", token="sh" + "_dfp_" + secrets.token_hex(20))
        self.assertEqual(status, 1)
        self.assertIn("HTTP 401", err)

    def test_the_token_never_appears_even_when_the_feed_echoes_it(self):
        self.feed.leak = True
        self.feed.entitled = False
        status, _, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 1)
        self.assertIn("refused ***", err)

        def broken(request, timeout):
            raise OSError(f"connection reset while sending {request.get_header('Authorization')}")
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = cli.main(["list", "--site", SITE, "--profile", PROFILE, "--feed", "cloud"],
                              {feed.TOKEN_ENV: self.token}, broken)
        self.assertEqual(status, 1)
        self.assertNotIn(self.token, stdout.getvalue() + stderr.getvalue())
        self.assertIn("Bearer ***", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_redirects_are_never_followed(self):
        self.feed.redirect = True
        status, _, err = self.run_cli("list", "--site", SITE, "--profile", PROFILE, "--feed", "cloud")
        self.assertEqual(status, 1)
        self.assertIn("redirect, which is never followed", err)
        self.assertEqual(len(self.feed.requests), 1)

    def test_rate_limits_are_retried_after_the_given_delay(self):
        self.feed.limited = 2
        waits = []
        client = feed.Client(SITE, PROFILE, self.token, open_url=self.open_url, sleep=waits.append,
                             out=lambda _: None)
        self.assertEqual(client.listing(), {"versions": []})
        self.assertEqual(waits, [1, 1])

    def test_the_listing_follows_pages_and_includes_drafts(self):
        self.feed.page_size = 1
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        self.feed.signer = False
        self.assertEqual(self.publish(BETA, "beta")[0], 1)
        output = self.temp / "listing.json"
        status, _, _ = self.run_cli("list", "--site", SITE, "--profile", PROFILE, "--feed", "cloud",
                                    "--output", str(output))
        self.assertEqual(status, 0)
        listing = json.loads(output.read_text())
        self.assertEqual([(v["version"], v["state"]) for v in listing["versions"]],
                         [(STABLE, "published"), (BETA, "draft")])
        self.assertIn("include_drafts=true&page=2", self.feed.requests[-1][1])

    def list_missing(self, *extra, answer=(404, "not_found")):
        self.feed.answer = answer
        output = self.temp / "listing.json"
        return (*self.run_cli("list", "--site", SITE, "--profile", PROFILE, "--feed", "cloud",
                              "--output", str(output), *extra), output)

    def test_preparation_lists_a_site_without_the_feed_route_as_empty(self):
        status, out, err, output = self.list_missing("--missing-feed-empty")
        self.assertEqual((status, err), (0, ""))
        self.assertEqual(json.loads(output.read_text()), {"versions": []})
        self.assertIn(f"::warning title=Desktop feed not enabled::{SITE} does not serve the desktop feed route",
                      out)
        self.assertIn("resolves as for a first release", out)
        self.assertEqual(len(self.feed.requests), 1)
        # The resolver reads it as a feed holding nothing.
        notes = load("release_notes", SCRIPTS / "release-notes.py")
        self.assertEqual(notes.feed_versions(output), set())

    def test_a_site_without_the_feed_route_still_fails_without_the_flag(self):
        status, out, err, output = self.list_missing()
        self.assertEqual(status, 1)
        self.assertIn("Listing the feed failed: HTTP 404 not_found", err)
        self.assertNotIn("::warning", out)
        self.assertFalse(output.exists())

    def test_only_a_404_not_found_is_listed_as_empty(self):
        for answer in ((404, "draft_not_found"), (404, "route_missing"), (500, "internal"), (503, "not_found"),
                       (401, "unauthenticated")):
            with self.subTest(answer=answer):
                status, out, err, output = self.list_missing("--missing-feed-empty", answer=answer)
                self.assertEqual(status, 1, err)
                self.assertIn(f"HTTP {answer[0]} {answer[1]}", err)
                self.assertNotIn("::warning", out)
                self.assertFalse(output.exists())

    def test_a_transport_failure_still_fails_with_the_flag(self):
        def unresolvable(request, timeout):
            raise feed.urllib.error.URLError("[Errno -2] Name or service not known")
        output = self.temp / "listing.json"
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = cli.main(["list", "--site", SITE, "--profile", PROFILE, "--feed", "cloud",
                               "--output", str(output), "--missing-feed-empty"],
                              {feed.TOKEN_ENV: self.token}, unresolvable)
        self.assertEqual(status, 1)
        self.assertIn("Name or service not known", stderr.getvalue())
        self.assertNotIn("::warning", stdout.getvalue())
        self.assertFalse(output.exists())

    def test_a_404_on_a_later_page_still_fails_with_the_flag(self):
        replies = iter([(200, b'{"versions": [{"version": "2026.11.1"}], "next_page": 2}'),
                        (404, b'{"error": "not_found"}')])

        class Reply:
            def __init__(self, status, body):
                self.status, self.body = status, body

            def __enter__(self):
                if self.status != 200:
                    raise feed.urllib.error.HTTPError("u", self.status, "x", {}, io.BytesIO(self.body))
                return self

            def __exit__(self, *args):
                return False

            def read(self):
                return self.body

        client = feed.Client(SITE, PROFILE, self.token, out=lambda _: None,
                             open_url=lambda request, timeout: Reply(*next(replies)))
        with self.assertRaises(feed.FeedError):
            client.listing(missing_feed_empty=True)

    def test_names_outside_the_binding_are_refused_before_any_request(self):
        for bad in (f"{PREFIX}-2026.11.4-Linux-x64.deb", f"Other-{STABLE}-Linux-x64.deb",
                    f"{PREFIX}-{STABLE}-Linux-x64.rpm", "../install.sh", f"{PREFIX}-macOS-arm64.dmg"):
            with self.subTest(bad=bad):
                version = STABLE
                if not (self.temp / version).exists():
                    self.stage(version)
                names = package_names(version)[:3] + [bad]
                (self.temp / f"{version}.txt").write_text("\n".join(names) + "\n")
                status, _, err = self.publish(version, "stable")
                self.assertEqual(status, 2, err)
                self.assertEqual(self.feed.requests, [])

    def test_missing_or_unsafe_settings_are_refused(self):
        notes = self.temp / "notes.md"
        notes.write_text("notes")
        base = ["--site", SITE, "--profile", PROFILE, "--feed", "cloud"]
        cases = {
            "no token": (["list", *base], {}),
            "no profile": (["list", "--site", SITE, "--feed", "cloud"], {feed.TOKEN_ENV: self.token}),
            "no site": (["list", "--profile", PROFILE, "--feed", "cloud"], {feed.TOKEN_ENV: self.token}),
            "http site": (["list", "--site", "http://feed.invalid", "--profile", PROFILE, "--feed", "cloud"],
                          {feed.TOKEN_ENV: self.token}),
            "site with a path": (["list", "--site", SITE + "/x", "--profile", PROFILE, "--feed", "cloud"],
                                 {feed.TOKEN_ENV: self.token}),
            "github feed": (["list", "--site", SITE, "--profile", PROFILE, "--feed", "github:o/r"],
                            {feed.TOKEN_ENV: self.token}),
            "padded month": (["publish", *base, "--asset-prefix", PREFIX, "--version", "2026.01.1",
                              "--channel", "stable", "--notes", str(notes), "--directory", str(self.temp),
                              "--assets", str(notes)], {feed.TOKEN_ENV: self.token}),
        }
        for label, (argv, environ) in cases.items():
            with self.subTest(label):
                env = {key: value for key, value in os.environ.items() if key != feed.TOKEN_ENV}
                env.update(environ)
                result = subprocess.run([sys.executable, "-I", str(SCRIPT), *argv], env=env,
                                        capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertNotIn(self.token, result.stdout + result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_the_token_is_not_an_argument(self):
        result = subprocess.run([sys.executable, "-I", str(SCRIPT), "list", "--token", "x"],
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 2)
        self.assertIn("unrecognized arguments", result.stderr)



FIXTURES = SCRIPTS.parent / "desktop-core" / "tests" / "fixtures" / "desktop-feed"
HOSTED = "m5lxqm2osk"
LIMIT = 4096
# The rehearsal's version shape, and the longest one plausible this decade.
REHEARSAL = "2026.11.1-beta.1"
LONGEST = "2026.12.99-beta.99"
# Ten digits per size, the most a package of up to 9.3 GB declares.
SIZE = 9_999_999_999


def candidate_inventory(version, profile):
    """A complete candidate's file list, named as release-candidate.py
    requires (with the floored macOS images), checksums included."""
    names = set(release.required_assets(version, profile))
    names |= {f"Desktop-{profile}-{version}-macOS15-{arch}.dmg" for arch in ("arm64", "x64")}
    return sorted(f"assets/{name}" for name in names | {name + ".sha256" for name in names})


def declared(names, profile, version):
    return [{"name": name, **feed.describe(name, f"Desktop-{profile}", version),
             "sha256": hashlib.sha256(name.encode()).hexdigest(), "size": SIZE} for name in names]


def notes_of(size):
    """Release-note lines of exactly `size` canonical bytes, with quotes and
    a non-ASCII character so escapes and UTF-8 count."""
    line = '- Fixed "the thing" on Linux and Windows — again.\n'
    text = line * ((size - 2) // feed.notes_size(line))
    # A newline is two canonical bytes ("\\n").
    return text + "x" * (size - feed.notes_size(text) - 2) + "\n"


def unreleased_notes():
    text = (SCRIPTS.parent / "CHANGELOG.md").read_text(encoding="utf-8")
    return text.split("## Unreleased", 1)[1].split("\n## ", 1)[0].strip() + "\n"


def measure(version, profile=HOSTED, channel="beta", names=None):
    """The pre-flight of profile's fixed 16-asset cloud manifest with
    ten-digit sizes and real sha256 values: the bound with empty notes, the
    notes budget derived from it under 4096 (and why), and the final bound
    with long notes cut to that budget; and, for comparison with the feed
    owner's 2713, the manifest without notes with 10-character ids and one
    46-character host."""
    if names is None:
        names = release.cloud_assets(candidate_inventory(version, profile), version, profile, log=lambda _: None)
    assets = declared(names, profile, version)
    budget, empty, why = feed.notes_budget(profile, version, channel, assets, LIMIT)
    notes, final = feed.fit_notes(lambda _: None, profile, version, channel, unreleased_notes() * 8, assets, LIMIT)
    likely = len(feed.canonical(dict(feed.manifest_bound(profile, version, channel, "", assets),
                                     download_hosts=["h" * 46])))
    return {"count": len(assets), "empty": empty, "budget": budget, "why": why, "notes": feed.notes_size(notes),
            "final": final, "likely": likely, "assets": assets}


release = load("release_candidate", SCRIPTS / "release-candidate.py")


class ManifestCapTests(FeedHarness):
    """The signer caps a manifest's RFC 8785 bytes (limits.max_canonical_bytes,
    4096 for an AWS KMS Ed25519 key, assumed when the listing gives none);
    the publisher measures before it creates a draft."""

    def publish_hosted(self, notes_text, version=REHEARSAL):
        self.feed.profile = HOSTED
        names = package_names(version, f"Desktop-{HOSTED}")
        if not (self.temp / version).exists():
            self.stage(version, names)
        notes = self.temp / "hosted-notes.md"
        notes.write_text(notes_text, encoding="utf-8")
        return self.run_cli(
            "publish", "--site", SITE, "--profile", HOSTED, "--feed", "cloud", "--asset-prefix", f"Desktop-{HOSTED}",
            "--version", version, "--channel", "beta", "--notes", str(notes), "--directory", str(self.temp / version),
            "--assets", str(self.temp / f"{version}.txt"))

    def bound_of(self, version, names, notes="What's new\n"):
        return feed.canonical_size(PROFILE, version, "stable" if "-beta." not in version else "beta", notes,
                                   feed.expected_assets(self.temp / version, names, PREFIX, version))

    def test_canonical_bytes_are_rfc_8785_as_the_verifier_writes_them(self):
        # desktop-core/src/feed/jcs.rs's vectors.
        self.assertEqual(feed.canonical({"ﬁ": 1, "\U0001f600": 2, "b": 3, "a": 4, "\xe9": 5, "A": 6}),
                         b'{"A":6,"a":4,"b":3,"\xc3\xa9":5,"\xf0\x9f\x98\x80":2,"\xef\xac\x81":1}')
        self.assertEqual(feed.canonical(["\b\t\n\f\r\"\\", "\x00\x1f\x01", "/\x7f\xe9€"]),
                         b'["\\b\\t\\n\\f\\r\\"\\\\","\\u0000\\u001f\\u0001","/\x7f\xc3\xa9\xe2\x82\xac"]')
        self.assertEqual(feed.canonical([0, -1, 2 ** 53 - 1, True, False, None]),
                         b"[0,-1,9007199254740991,true,false,null]")
        for bad in (2 ** 53, 1.5):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                feed.canonical(bad)
        # Whitespace and member order do not change the bytes the signature covers.
        pretty = json.loads((FIXTURES / "raw" / "valid-pretty.json").read_text().replace("$SIGNATURE", "x"))
        reordered = json.loads((FIXTURES / "raw" / "valid-reordered-whitespace.json").read_text()
                               .replace("$SIGNATURE", "x"))
        stable = json.loads((FIXTURES / "manifest" / "valid-stable.json").read_text())
        self.assertEqual(feed.canonical(pretty["manifest"]), feed.canonical(stable))
        self.assertEqual(feed.canonical(reordered["manifest"]), feed.canonical(stable))

    def test_the_bound_counts_clouds_fields_as_the_feed_owner_gave_them(self):
        # Ids are "a_" and 8 base62 characters; one host of at most 100.
        self.assertEqual(len(feed.BOUND_ID), 10)
        self.assertRegex(feed.BOUND_ID, feed.ASSET_ID)
        self.assertEqual(len(feed.BOUND_PUBLISHED_AT), 20)
        self.assertEqual([len(host) for host in feed.BOUND_DOWNLOAD_HOSTS], [100])
        self.assertEqual(feed.NOTES_MAX_BYTES, 300)
        self.assertEqual(feed.DEFAULT_MAX_CANONICAL_BYTES, 4096)

    def test_the_bound_is_the_signed_manifest_with_clouds_fields_at_their_longest(self):
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)
        release_body = self.feed.releases[STABLE]["body"]
        signed = {"schema": 1, "profile": PROFILE, "channel": "stable", "version": STABLE,
                  "published_at": "2026-11-07T12:00:00Z", "notes": release_body["notes"], "download_hosts": [],
                  "assets": [dict(asset, id=self.feed.releases[STABLE]["stored"][asset["name"]]["id"])
                             for asset in release_body["expected_assets"]]}
        bound = feed.canonical_size(PROFILE, STABLE, "stable", release_body["notes"], release_body["expected_assets"])
        # A streaming-only site signs no host: smaller than the one-host bound.
        self.assertEqual(len(feed.canonical(signed)), bound - len(feed.canonical(feed.BOUND_DOWNLOAD_HOSTS)) + 2)
        signed.update(published_at=feed.BOUND_PUBLISHED_AT, download_hosts=feed.BOUND_DOWNLOAD_HOSTS,
                      assets=[dict(asset, id=feed.BOUND_ID) for asset in signed["assets"]])
        self.assertEqual(len(feed.canonical(signed)), bound)

    def test_under_the_limit_publishes_and_prints_the_size_and_the_limit(self):
        self.feed.limits = {"max_canonical_bytes": LIMIT}
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        size = self.bound_of(STABLE, package_names(STABLE))
        self.assertLess(size, LIMIT)
        self.assertIn(f"Manifest canonical size: at most {size} bytes for 16 assets; signer limit: {LIMIT}.", out)
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")
        # Notes that fit whole are declared as written: no link is added.
        self.assertEqual(self.feed.releases[STABLE]["body"]["notes"], "What's new\n")
        self.assertNotIn("Full notes:", out)

    def test_a_bound_at_exactly_the_limit_passes_and_one_byte_over_refuses(self):
        self.stage(STABLE)
        empty = self.bound_of(STABLE, package_names(STABLE), notes="")
        size = self.bound_of(STABLE, package_names(STABLE))
        # Assets that do not fit even with empty notes are refused, with both numbers.
        self.feed.limits = {"max_canonical_bytes": empty - 1}
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 2, err)
        self.assertIn(f"up to {empty} canonical bytes, over the signer's max_canonical_bytes of {empty - 1};"
                      " nothing was created or uploaded. The 16 assets do not fit even with empty release notes",
                      err)
        self.assertIn("Release notes: kept 0 of 12 canonical bytes (0 of 1 whole lines), capped at 0 (the room"
                      f" 16 assets leave under the signer limit of {empty - 1} ({empty} bytes with empty notes)).",
                      out)
        self.assertEqual([method for method, _, _ in self.feed.requests], ["GET"])
        self.assertEqual(self.feed.releases, {})
        # At exactly the limit the notes are kept whole.
        self.feed.limits = {"max_canonical_bytes": size}
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        self.assertIn("Release notes: kept all 12 canonical bytes, within a cap of 12 (the room 16 assets leave"
                      f" under the signer limit of {size} ({empty} bytes with empty notes)).", out)
        self.assertIn(f"at most {size} bytes for 16 assets; signer limit: {size}.", out)
        self.assertEqual(self.feed.releases[STABLE]["body"]["notes"], "What's new\n")
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")
        # One byte less drops the last line that no longer fits, and still publishes.
        self.feed.releases.clear()
        self.feed.limits = {"max_canonical_bytes": size - 1}
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"at most {empty} bytes for 16 assets; signer limit: {size - 1}.", out)
        self.assertEqual(self.feed.releases[STABLE]["body"]["notes"], "")

    def test_the_listings_limit_is_used_when_it_gives_one(self):
        names = every_package_name(STABLE)
        self.stage(STABLE, names)
        size = self.bound_of(STABLE, names)
        self.assertGreater(size, LIMIT)
        # A larger cap from the listing wins over the KMS default...
        self.feed.limits = {"max_canonical_bytes": 100_000}
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"at most {size} bytes for 20 assets; signer limit: 100000.", out)
        # ...and so does a smaller one.
        self.feed.releases.clear()
        self.feed.limits = {"max_canonical_bytes": 1000}
        self.assertEqual(self.publish(BETA, "beta")[0], 2)

    def test_a_null_or_absent_limit_is_the_kms_default_of_4096(self):
        names = every_package_name(STABLE)
        self.stage(STABLE, names)
        for limits in ({"max_canonical_bytes": None}, {}, None):
            with self.subTest(limits=limits):
                self.feed.limits = limits
                self.feed.requests.clear()
                status, out, err = self.publish(STABLE, "stable")
                self.assertEqual(status, 2, err)
                self.assertIn("for 20 assets; signer limit: 4096 (the KMS default of 4096: the listing gives none, and null or absent is not read as uncapped).", out)
                self.assertIn("over the signer's max_canonical_bytes of 4096", err)
                self.assertEqual([method for method, _, _ in self.feed.requests], ["GET"])
                self.assertEqual(self.feed.releases, {})
        self.feed.limits = None
        status, out, err = self.publish(BETA, "beta")
        self.assertEqual((status, err), (0, ""))
        self.assertIn("for 16 assets; signer limit: 4096 (the KMS default of 4096: the listing gives none, and null or absent is not read as uncapped).", out)

    def test_a_malformed_limit_fails_closed_before_any_draft(self):
        for limits in ({"max_canonical_bytes": "4096"}, {"max_canonical_bytes": 0},
                       {"max_canonical_bytes": True}, {"max_canonical_bytes": 40.96}, []):
            with self.subTest(limits=limits):
                self.feed.limits = limits
                self.feed.requests.clear()
                status, _, err = self.publish(STABLE, "stable")
                self.assertEqual(status, 1)
                self.assertIn("max_canonical_bytes is neither a positive integer nor null", err)
                self.assertEqual([method for method, _, _ in self.feed.requests], ["GET"])
                self.assertEqual(self.feed.releases, {})

    def test_a_feed_422_manifest_too_large_is_a_refusal_never_retried_or_replayed(self):
        self.feed.too_large = (4321, 4096)
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 2, err)
        self.assertIn("Creating the draft failed: HTTP 422 manifest_too_large: the feed measured the signed"
                      " manifest at canonical_bytes 4321, over its max_canonical_bytes of 4096;"
                      " nothing was published, and it is not retried.", err)
        self.assertNotIn("already published", out)
        # One limits read and one create: no retry, no replay listing.
        self.assertEqual([method for method, _, _ in self.feed.requests], ["GET", "POST"])
        self.assertEqual(self.feed.releases, {})
        # The client raises a Refusal, and without numbers says so.
        self.feed.too_large = ("many", None)
        client = feed.Client(SITE, PROFILE, self.token, open_url=self.open_url, out=lambda _: None)
        assets = feed.expected_assets(self.temp / STABLE, package_names(STABLE), PREFIX, STABLE)
        with self.assertRaisesRegex(feed.Refusal, "canonical_bytes unknown, over its max_canonical_bytes of unknown"):
            feed.publish(client, self.temp / STABLE, STABLE, "stable", "notes", assets)
        # Once the manifest fits, the same version publishes: nothing was held.
        self.feed.too_large = None
        self.assertEqual(self.publish(STABLE, "stable")[0], 0)

    def test_a_dry_run_measures_without_reading_the_limit(self):
        status, out, _ = self.publish(STABLE, "stable", "--dry-run")
        self.assertEqual(status, 0)
        self.assertRegex(out, r"Manifest canonical size: at most [0-9]{4} bytes; signer limit: not read in a dry run"
                              r" \(notes fitted under the default of 4096\)\.")
        self.assertEqual(self.feed.requests, [])

    def test_notes_are_capped_at_300_bytes_on_whole_lines_and_logged(self):
        said = []
        notes = "- one\n- \"two\"\n- three\n"
        # Escapes count: "- \"two\"\n" is 11 canonical bytes, not 8.
        self.assertEqual(feed.cap_notes(notes, said.append, 18), "- one\n- \"two\"\n")
        self.assertEqual(said[-1], "Release notes: kept 18 of 27 canonical bytes (2 of 3 whole lines),"
                                   " capped at 18 (the fixed cap of 18).")
        self.assertEqual(feed.cap_notes(notes, said.append, 17), "- one\n")
        self.assertEqual(feed.cap_notes(notes, said.append, 0), "")
        self.assertEqual(feed.cap_notes(notes, said.append), notes)
        self.assertEqual(said[-1], "Release notes: kept all 27 canonical bytes, within a cap of 300"
                                   " (the fixed cap of 300).")
        feed.cap_notes(notes, said.append, 18, "why")
        self.assertEqual(said[-1], "Release notes: kept 18 of 27 canonical bytes (2 of 3 whole lines),"
                                   " capped at 18 (why).")
        exact = notes_of(300)
        self.assertEqual(feed.notes_size(exact), 300)
        self.assertEqual(feed.cap_notes(exact, said.append), exact)
        longer = exact + "- one more\n"
        capped = feed.cap_notes(longer, said.append)
        self.assertEqual(capped, exact)
        # Capping is a function of the notes and the cap alone, and capped notes cap to themselves.
        self.assertEqual(feed.cap_notes(capped, said.append), capped)
        self.assertEqual(feed.cap_notes(longer, said.append), capped)

    def test_a_capped_notes_rerun_resumes_its_draft_instead_of_a_409(self):
        long_notes = notes_of(300) + "- dropped line\n" * 40
        names = package_names(REHEARSAL, f"Desktop-{HOSTED}")
        self.stage(REHEARSAL, names)
        assets = feed.expected_assets(self.temp / REHEARSAL, names, f"Desktop-{HOSTED}", REHEARSAL)
        empty = feed.canonical_size(HOSTED, REHEARSAL, "beta", "", assets)
        # A limit that leaves the notes 200 bytes: the budget, not the fixed cap, cuts them.
        self.feed.limits = {"max_canonical_bytes": empty + 200}
        self.feed.fail_upload.add(names[5])
        status, out, err = self.publish_hosted(long_notes)
        self.assertEqual(status, 1, err)
        self.assertRegex(out, r"Release notes: kept 1[0-9]{2} of [0-9]+ canonical bytes \([0-9]+ of 46 whole lines"
                              r" and a link to the full notes\),"
                              rf" capped at 200 \(the room 16 assets leave under the signer limit of {empty + 200}"
                              rf" \({empty} bytes with empty notes\)\)\.")
        self.assertEqual(self.feed.releases[REHEARSAL]["state"], "draft")
        declared_notes = self.feed.releases[REHEARSAL]["body"]["notes"]
        self.assertLessEqual(feed.notes_size(declared_notes), 200)
        self.assertTrue(declared_notes.endswith("\n\n" + feed.full_notes_line(SITE, REHEARSAL)))
        # The rerun declares the same capped notes: 200, resumed, published.
        status, out, err = self.publish_hosted(long_notes)
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"Resumed the draft of v{REHEARSAL}.", out)
        self.assertEqual(self.feed.releases[REHEARSAL]["body"]["notes"], declared_notes)
        self.assertEqual(self.feed.releases[REHEARSAL]["state"], "published")
        # A replay after the publish is done, not a refusal.
        status, out, err = self.publish_hosted(long_notes)
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"v{REHEARSAL} is already published on the beta channel", out)

    def test_cut_notes_end_with_a_link_to_the_full_notes_inside_the_budget(self):
        said = []
        link = feed.full_notes_line(SITE, STABLE)
        self.assertEqual(link, f"Full notes: {SITE}/cloud/dashboard/downloads#v{STABLE}\n")
        tail = feed.notes_size("\n" + link)
        notes = "Headline.\n\n## What's new in this release\n\n- one\n" + "- a longer line of release notes\n" * 9
        # Notes that fit are kept whole, with no link.
        self.assertEqual(feed.cap_notes(notes, said.append, feed.notes_size(notes), link=link), notes)
        self.assertNotIn("link", said[-1])
        # Cut notes keep room for the link: the cut, with the link, is within the cap.
        cap = tail + feed.notes_size("Headline.\n\n## What's new in this release\n\n- one\n")
        capped = feed.cap_notes(notes, said.append, cap, link=link)
        self.assertEqual(capped, "Headline.\n\n## What's new in this release\n\n- one\n\n" + link)
        self.assertLessEqual(feed.notes_size(capped), cap)
        self.assertEqual(said[-1], f"Release notes: kept {feed.notes_size(capped)} of {feed.notes_size(notes)}"
                                   f" canonical bytes (5 of 14 whole lines and a link to the full notes), capped at"
                                   f" {cap} (the fixed cap of {cap}).")
        # A cut that ends on a heading or blank line drops them before the link.
        cap = tail + feed.notes_size("Headline.\n\n## What's new in this release\n\n")
        self.assertEqual(feed.cap_notes(notes, said.append, cap, link=link), "Headline.\n\n" + link)
        # Room for the link alone: the link, nothing else.
        self.assertEqual(feed.cap_notes(notes, said.append, tail, link=link), link)
        # No room for the link: the cut carries none, and says so.
        unlinked = feed.cap_notes(notes, said.append, tail - 1, link=link)
        self.assertNotIn("Full notes:", unlinked)
        self.assertTrue(notes.startswith(unlinked))
        self.assertLessEqual(feed.notes_size(unlinked), tail - 1)
        self.assertIn("the link to the full notes", said[-2])
        self.assertIn("carry no link", said[-2])
        # Capped notes cap to themselves under the same budget and link.
        self.assertEqual(feed.cap_notes(capped, said.append, feed.notes_size(capped), link=link), capped)

    def test_release_notes_cloud_body_cut_to_the_budget_keeps_the_link(self):
        # The body release-notes.py --feed cloud writes, at the tightest
        # budget (212 at LONGEST), ends with the link and fits.
        notes_module = load("release_notes", SCRIPTS / "release-notes.py")
        body = notes_module.cloud_body(f"Desktop-{HOSTED} v{LONGEST} for macOS, Linux and Windows.", LONGEST,
                                       unreleased_notes(), "1.2.3")
        assets = measure(LONGEST)["assets"]
        budget = feed.notes_budget(HOSTED, LONGEST, "beta", assets, LIMIT)[0]
        self.assertEqual(budget, 212)
        link = feed.full_notes_line("https://relay.example.com", LONGEST)
        notes, size = feed.fit_notes(lambda _: None, HOSTED, LONGEST, "beta", body, assets, LIMIT, link)
        self.assertTrue(notes.startswith(f"Desktop-{HOSTED} v{LONGEST} for macOS, Linux and Windows.\n"))
        self.assertTrue(notes.endswith("\n\n" + link))
        self.assertLessEqual(feed.notes_size(notes), budget)
        self.assertLessEqual(size, LIMIT)

    def test_a_null_limit_cuts_the_notes_as_the_4096_default_does(self):
        names = package_names(REHEARSAL, f"Desktop-{HOSTED}")
        self.stage(REHEARSAL, names)
        assets = [dict(asset, size=SIZE) for asset in
                  feed.expected_assets(self.temp / REHEARSAL, names, f"Desktop-{HOSTED}", REHEARSAL)]
        notes = unreleased_notes() * 8
        link = feed.full_notes_line(SITE, REHEARSAL)
        self.feed.profile = HOSTED
        client = feed.Client(SITE, HOSTED, self.token, open_url=self.open_url, out=lambda _: None)
        expected = feed.fit_notes(lambda _: None, HOSTED, REHEARSAL, "beta", notes, assets, 4096, link)[0]
        for limits in ({"max_canonical_bytes": None}, {}, None):
            with self.subTest(limits=limits):
                self.feed.limits = limits
                self.assertIsNone(client.max_canonical_bytes("beta"))
                cut, why = feed.preflight(client, REHEARSAL, "beta", notes, assets)
                self.assertEqual(cut, expected)
                self.assertIn("under a signer limit of 4096 (the default; the listing gives none)", why)
        # A listed 4096 cuts them the same way.
        self.feed.limits = {"max_canonical_bytes": 4096}
        self.assertEqual(feed.preflight(client, REHEARSAL, "beta", notes, assets)[0], expected)

    def test_a_publish_time_422_manifest_too_large_names_the_stranded_draft(self):
        self.feed.too_large_at_publish = (4200, 4096)
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual(status, 2, err)
        self.assertIn("Publishing failed: HTTP 422 manifest_too_large: the feed measured the signed manifest at"
                      " canonical_bytes 4200, over its max_canonical_bytes of 4096; nothing was published, and it"
                      " is not retried.", err)
        self.assertIn(f"The draft of v{STABLE} stays on the feed with all 16 declared assets uploaded.", err)
        self.assertIn("A rerun with the same notes and packages resumes it", err)
        self.assertNotIn("Nothing was created", err)
        # One publish call: no retry, no replay listing.
        self.assertEqual([path for method, path, _ in self.feed.requests if method == "POST"],
                         [f"{BASE}/releases", f"{BASE}/releases/{STABLE}/publish"])
        self.assertEqual(self.feed.requests[-1][0:2], ("POST", f"{BASE}/releases/{STABLE}/publish"))
        release = self.feed.releases[STABLE]
        self.assertEqual(release["state"], "draft")
        self.assertEqual(sorted(release["stored"]), sorted(package_names(STABLE)))
        # The rerun the message promises: it resumes the draft, uploads nothing, and publishes.
        self.feed.too_large_at_publish = None
        self.feed.requests.clear()
        status, out, err = self.publish(STABLE, "stable")
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"Resumed the draft of v{STABLE}.", out)
        self.assertEqual([method for method, _, _ in self.feed.requests if method == "PUT"], [])
        self.assertEqual(self.feed.releases[STABLE]["state"], "published")

    def test_a_dry_run_plans_the_cut_notes_not_the_raw_notes(self):
        self.stage(STABLE)
        raw = notes_of(300) + "- one line past the cap\n"
        (self.temp / "notes.md").write_text(raw, encoding="utf-8")
        status, out, _ = self.publish(STABLE, "stable", "--dry-run")
        self.assertEqual(status, 0)
        self.assertEqual(self.feed.requests, [])
        self.assertRegex(out, r"Release notes: kept [0-9]+ of 3[0-9]{2} canonical bytes \([0-9]+ of [0-9]+ whole lines"
                              r" and a link to the full notes\), capped at 300 \(the fixed cap of 300\)\.")
        assets = feed.expected_assets(self.temp / STABLE, package_names(STABLE), PREFIX, STABLE)
        cut = feed.fit_notes(lambda _: None, PROFILE, STABLE, "stable", raw, assets, LIMIT,
                             feed.full_notes_line(SITE, STABLE))[0]
        self.assertNotEqual(cut, raw)
        planned = json.dumps(cut)
        self.assertIn(f'"notes": {planned}', out)
        self.assertNotIn("one line past the cap", out)

    def test_a_limit_changed_between_attempts_fails_with_a_clear_message(self):
        long_notes = notes_of(300) + "- dropped line\n" * 40
        names = package_names(REHEARSAL, f"Desktop-{HOSTED}")
        self.stage(REHEARSAL, names)
        assets = feed.expected_assets(self.temp / REHEARSAL, names, f"Desktop-{HOSTED}", REHEARSAL)
        empty = feed.canonical_size(HOSTED, REHEARSAL, "beta", "", assets)
        # Attempt 1: the listing gives no limit (4096, the fixed 300 applies); an upload fails.
        self.feed.limits = {"max_canonical_bytes": None}
        self.feed.fail_upload.add(names[5])
        self.assertEqual(self.publish_hosted(long_notes)[0], 1)
        self.assertEqual(self.feed.releases[REHEARSAL]["state"], "draft")
        # Attempt 2: the listing now reports a smaller limit, so the notes are cut shorter.
        self.feed.limits = {"max_canonical_bytes": empty + 150}
        status, _, err = self.publish_hosted(long_notes)
        self.assertEqual(status, 1, err)
        self.assertIn(f"a draft of v{REHEARSAL} declares other notes or assets", err)
        self.assertIn(f"This attempt cut the release notes to a budget of 150 canonical bytes under a signer"
                      f" limit of {empty + 150}; if the listing's max_canonical_bytes changed since the draft was"
                      " made, the cut changed with it; so does a different spelling of the site (the link"
                      " names it) or a draft left by an older publisher, which cut the notes differently:"
                      " rerun once the listing reports the earlier limit, with the earlier site spelling,"
                      " or publish under a new version", err)
        self.assertRegex(err, r"older publisher")
        self.assertRegex(err, r"spelling of the site")
        # Back at the earlier limit, the rerun resumes the draft cleanly.
        self.feed.limits = {"max_canonical_bytes": None}
        status, out, err = self.publish_hosted(long_notes)
        self.assertEqual((status, err), (0, ""))
        self.assertIn(f"Resumed the draft of v{REHEARSAL}.", out)
        self.assertEqual(self.feed.releases[REHEARSAL]["state"], "published")

    def test_cut_notes_pop_only_markdown_headings_before_the_link(self):
        said = []
        link = feed.full_notes_line(SITE, STABLE)
        keep = "Headline.\n\n- one\n#1234 crash on launch fixed\n"
        notes = keep + "## Heading\n" + "- a longer line of release notes\n" * 9
        cap = feed.notes_size("\n" + link) + feed.notes_size(keep + "## Heading\n")
        self.assertEqual(feed.cap_notes(notes, said.append, cap, link=link), keep + "\n" + link)
        trailing = "Headline.\n\n- one\n## Heading\n" + "- a longer line of release notes\n" * 9
        cap = feed.notes_size("\n" + link) + feed.notes_size("Headline.\n\n- one\n## Heading\n")
        self.assertEqual(feed.cap_notes(trailing, said.append, cap, link=link),
                         "Headline.\n\n- one\n\n" + link)

    def test_a_default_port_in_the_site_does_not_change_the_cut_notes(self):
        said = []
        notes = "Headline.\n\n" + "- a longer line of release notes\n" * 40
        plain = feed.full_notes_line("https://h", STABLE)
        ported = feed.full_notes_line("https://h:443", STABLE)
        self.assertEqual(plain, ported)
        self.assertNotEqual(plain, feed.full_notes_line("https://h:8443", STABLE))
        names = package_names(REHEARSAL, f"Desktop-{HOSTED}")
        self.stage(REHEARSAL, names)
        assets = feed.expected_assets(self.temp / REHEARSAL, names, f"Desktop-{HOSTED}", REHEARSAL)
        results = [feed.fit_notes(said.append, HOSTED, REHEARSAL, "beta", notes, assets, 2048,
                                  feed.full_notes_line(site, REHEARSAL))
                   for site in ("https://h", "https://h:443")]
        self.assertEqual(results[0], results[1])

    def test_the_same_inputs_cap_the_notes_identically_on_a_rerun(self):
        # The budget depends on the profile, version, channel, assets and
        # limit, never on the notes, so a rerun declares the same notes, and
        # capped notes cap to themselves under it.
        notes = unreleased_notes() * 8
        for version in (REHEARSAL, LONGEST):
            with self.subTest(version=version):
                assets = measure(version)["assets"]
                first, size = feed.fit_notes(lambda _: None, HOSTED, version, "beta", notes, assets, LIMIT)
                again, size_again = feed.fit_notes(lambda _: None, HOSTED, version, "beta", notes, assets, LIMIT)
                self.assertEqual((first, size), (again, size_again))
                self.assertEqual(feed.fit_notes(lambda _: None, HOSTED, version, "beta", first, assets, LIMIT),
                                 (first, size))
                self.assertTrue(first and notes.startswith(first) and first.endswith("\n"))
                self.assertLessEqual(size, LIMIT)

    def test_the_enterprise_profiles_publish_declares_the_16_assets_and_capped_notes(self):
        self.feed.limits = {"max_canonical_bytes": LIMIT}
        names = release.cloud_assets(candidate_inventory(REHEARSAL, HOSTED), REHEARSAL, HOSTED, log=lambda _: None)
        self.assertEqual(sorted(names), sorted(package_names(REHEARSAL, f"Desktop-{HOSTED}")))
        status, out, err = self.publish_hosted(unreleased_notes() * 8)
        self.assertEqual((status, err), (0, ""))
        # The staged packages are small, so the fixed cap applies here.
        self.assertRegex(out, r"Release notes: kept [0-9]+ of [0-9]+ canonical bytes \([0-9]+ of [0-9]+ whole lines"
                              r" and a link to the full notes\), capped at 300 \(the fixed cap of 300\)\.")
        self.assertRegex(out, r"for 16 assets; signer limit: 4096\.")
        published = self.feed.releases[REHEARSAL]
        self.assertLessEqual(feed.notes_size(published["body"]["notes"]), 300)
        # Cut notes end with the link to the full notes, inside the 300.
        self.assertTrue(published["body"]["notes"].endswith(
            f"\n\nFull notes: {SITE}/cloud/dashboard/downloads#v{REHEARSAL}\n"))
        self.assertEqual(sorted(a["name"] for a in published["body"]["expected_assets"]), sorted(names))

    def test_the_16_asset_bound_with_budgeted_notes_fits_under_4096(self):
        # Measured with ten-digit sizes: with empty notes, the bound and the
        # notes budget it leaves under 4096 (300 at most).
        expected = {REHEARSAL: (3854, 242), LONGEST: (3884, 212)}
        for version in (REHEARSAL, LONGEST):
            with self.subTest(version=version):
                m = measure(version)
                print(f"\n{HOSTED} v{version} (prefix Desktop-{HOSTED}): {m['count']} assets, 10-digit sizes:"
                      f" empty-notes bound {m['empty']}, notes budget {m['budget']} ({m['why']}), final bound"
                      f" {m['final']} with {m['notes']} bytes of notes (margin {LIMIT - m['final']} under {LIMIT});"
                      f" without notes, with one 46-character host: {m['likely']}", file=sys.stderr)
                self.assertEqual(m["count"], 16)
                self.assertEqual((m["empty"], m["budget"]), expected[version])
                self.assertEqual(m["budget"], min(feed.NOTES_MAX_BYTES, LIMIT - m["empty"]))
                self.assertLessEqual(m["notes"], m["budget"])
                self.assertEqual(m["final"], m["empty"] + m["notes"])
                self.assertLessEqual(m["final"], LIMIT)
                self.assertLess(m["empty"] - m["likely"], 100)
                # A full 300 bytes of notes would not fit.
                self.assertGreater(m["empty"] + feed.NOTES_MAX_BYTES, LIMIT)

    def test_a_shorter_prefix_gets_the_full_300_bytes_of_notes(self):
        names = package_names(REHEARSAL)
        m = measure(REHEARSAL, PROFILE, names=names)
        print(f"\n{PROFILE} v{REHEARSAL} (prefix {PREFIX}): empty-notes bound {m['empty']}, notes budget"
              f" {m['budget']} ({m['why']}), final bound {m['final']}", file=sys.stderr)
        self.assertEqual((m["count"], m["budget"], m["why"]), (16, 300, "the fixed cap of 300"))
        self.assertLessEqual(m["empty"] + 300, LIMIT)
        exact = notes_of(300)
        notes, size = feed.fit_notes(lambda _: None, PROFILE, REHEARSAL, "beta", exact, m["assets"], LIMIT)
        self.assertEqual((notes, size), (exact, m["empty"] + 300))


if __name__ == "__main__":
    unittest.main()

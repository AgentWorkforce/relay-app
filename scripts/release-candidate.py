#!/usr/bin/env python3
"""Bind complete, already signed release packages to their producing workflow.

Creation runs only after release tests and all native packaging jobs pass.
Verification consumes run metadata fetched independently from GitHub's API;
manifest claims alone never establish the producer's identity or success.
"""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import re
import sys

MANIFEST = "release-candidate.json"
WORKFLOW = ".github/workflows/desktop-release.yml"
ARCHITECTURES = ["macOS-arm64", "macOS-x64", "Linux-arm64", "Linux-x64", "Windows-arm64", "Windows-x64"]
# The headless installers every release carries, set to its own releases.
INSTALL_SCRIPTS = ("install.sh", "install.ps1")
# A site profile's opaque id (docs/workspace-layout.md, "Site profiles");
# empty is the default profile. The manifest records the id, never a product
# name, and the assets are named by it (scripts/site-origin.sh asset_prefix).
PROFILE = re.compile(r"(?=[a-z0-9-]{1,40}\Z)[a-z0-9]+(?:-[a-z0-9]+)*")
# Where a candidate is published: the profile's GitHub releases repository
# (the default; its manifest records no feed, as before feeds) or its site's
# cloud release feed, whose manifest records "feed": "cloud". A cloud version
# follows the proposed Cloud contract (relay-control docs/desktop-feed.md): its
# month has no leading zero.
FEEDS = ("github", "cloud")
VERSIONS = {
    "github": re.compile(r"[0-9]{4}\.[0-9]{2}\.[0-9]+(?:-beta\.[0-9]+)?"),
    "cloud": re.compile(r"[0-9]{4}\.(?:[1-9]|1[0-2])\.(?:0|[1-9][0-9]*)(?:-beta\.(?:0|[1-9][0-9]*))?"),
}
# The cloud profile's id is also its feed's X-Agent-Relay-Profile header.
CLOUD_PROFILE = re.compile(r"[a-z0-9]{1,32}")
# The core crates a candidate was built with (scripts/apply-crate-overrides.sh
# --json): the pins, or an internal build's overrides. All three are required
# because both workspaces depend on them.
CRATE_REV = re.compile(r"[0-9a-f]{40}")
CRATE_VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?")
# What a cloud feed publishes after <prefix>-<version>-, the same for every
# cloud profile: the floored macOS disk image, the Linux tarball and deb and
# the Windows zip and Setup.exe, for both architectures (10 packages), the
# four Linux and Windows probe bundles the headless installers select, plus
# install.sh and install.ps1: 16 assets. The macOS probe bundles and the
# unfloored macOS images stay out, because the feed's signer caps a manifest's
# RFC 8785 canonical bytes (limits.max_canonical_bytes; 4096 for an AWS KMS
# Ed25519 key) and the full 20-asset set does not fit. Floor-aware updaters ignore an
# unfloored image when floored ones exist.
CLOUD_PACKAGE = re.compile(r"macOS[1-9][0-9]*-(arm64|x64)\.dmg|Linux-(arm64|x64)\.(?:tar\.gz|deb)"
                           r"|Linux-(arm64|x64)-probe\.tar\.gz"
                           r"|Windows-(arm64|x64)(?:\.zip|-Setup\.exe|-probe\.zip)")
CLOUD_ASSET_COUNT = 16


def asset_prefix(profile):
    if not profile:
        return "AgentRelay"
    if not PROFILE.fullmatch(profile):
        raise ValueError("profile must be an opaque id of lowercase letters, digits and hyphens")
    return f"Desktop-{profile}"


def required_assets(version, profile=""):
    # Every profile's release has install.sh and install.ps1, set to its own
    # releases.
    prefix = asset_prefix(profile)
    return {
        f"{prefix}-{pinned}{platform}-{arch}{suffix}"
        for platform, suffixes in {
            "macOS": [".dmg", "-probe.tar.gz"],
            "Linux": [".tar.gz", ".deb", "-probe.tar.gz"],
            "Windows": [".zip", "-probe.zip", "-Setup.exe"],
        }.items()
        for pinned in (f"{version}-", "")
        for arch in ("arm64", "x64")
        for suffix in suffixes
    } | set(INSTALL_SCRIPTS)


def inventory(directory, version, profile=""):
    prefix = asset_prefix(profile)
    assets = directory / "assets"
    for path in directory.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"symlink in candidate: {path}")
        if path.is_file() and path.parent != assets and path not in (
                directory / MANIFEST, directory / "release-notes.md"):
            raise ValueError("candidate payloads must be direct children of assets/")
    if not (directory / "release-notes.md").is_file():
        raise ValueError("missing prepared release-notes.md")
    required = required_assets(version, profile)
    floors = []
    for arch in ("arm64", "x64"):
        images = list(assets.glob(f"{prefix}-{version}-macOS[0-9]*-{arch}.dmg"))
        if len(images) != 1:
            raise ValueError(f"expected exactly one current floored macOS image for {arch}")
        image = images[0]
        match = re.fullmatch(rf"{re.escape(prefix)}-{re.escape(version)}-macOS([1-9][0-9]*)-{arch}\.dmg", image.name)
        if not match or int(match[1]) < 13:
            raise ValueError("invalid macOS deployment floor")
        floors.append(match[1])
        required.add(image.name)
    if len(set(floors)) != 1:
        raise ValueError("macOS architectures disagree on deployment floor")
    expected = required | {name + ".sha256" for name in required}
    actual = {path.name for path in assets.iterdir() if path.is_file()}
    if expected != actual:
        raise ValueError("candidate package inventory differs: missing=" +
                         ",".join(sorted(expected - actual)) + " unexpected=" +
                         ",".join(sorted(actual - expected)))
    for arch in ("arm64", "x64"):
        image = assets / f"{prefix}-{version}-macOS{floors[0]}-{arch}.dmg"
        if digest(image) != digest(assets / f"{prefix}-macOS-{arch}.dmg"):
            raise ValueError("stable macOS alias differs from current floored image")
    for name in sorted(required):
        expected_checksum = f"{digest(assets / name)}  {name}"
        if (assets / (name + ".sha256")).read_text().strip() != expected_checksum:
            raise ValueError(f"invalid package checksum: {name}")
        if name.startswith(f"{prefix}-{version}-") and not name.endswith(".dmg"):
            alias = name.replace(f"{prefix}-{version}-", f"{prefix}-", 1)
            if digest(assets / name) != digest(assets / alias):
                raise ValueError(f"stable alias differs from versioned package: {alias}")
    return {
        path.relative_to(directory).as_posix(): {"sha256": digest(path), "size": path.stat().st_size}
        for path in sorted(directory.rglob("*"))
        if path.is_file() and path != directory / MANIFEST
    }


def cloud_assets(files, version, profile, log=None):
    """The names a cloud feed publishes, from a candidate's inventory: the
    fixed cloud set (CLOUD_PACKAGE) of version-pinned packages, install.sh
    and install.ps1, exactly 16. The unversioned aliases, the .sha256 files,
    the macOS probes and the unfloored disk images stay out: the feed binds every
    name but the installers to <asset_prefix>-<version>-, and its signed
    manifest carries each sha256. Every selected and omitted name is logged
    (to stderr by default), so nothing is dropped silently."""
    log = log or (lambda text: print(text, file=sys.stderr))
    prefix = asset_prefix(profile)
    bound = f"{prefix}-{version}-"
    names = sorted(
        name for name in (path.removeprefix("assets/") for path in files if path.startswith("assets/"))
        if not name.endswith(".sha256") and (name in INSTALL_SCRIPTS or name.startswith(bound))
    )
    selected = [name for name in names
                if name in INSTALL_SCRIPTS or CLOUD_PACKAGE.fullmatch(name[len(bound):])]
    if not set(INSTALL_SCRIPTS) <= set(selected) or len(selected) != CLOUD_ASSET_COUNT:
        raise ValueError(f"a cloud release needs exactly {CLOUD_ASSET_COUNT} assets: install.sh, install.ps1 and"
                         " the floored macOS images, Linux tarballs, debs and probe bundles and Windows zips,"
                         f" Setup.exes and probe bundles of both architectures; found {len(selected)}")
    omitted = [name for name in names if name not in selected]
    log(f"Cloud assets of profile {profile}: the fixed cloud set selects {len(selected)} of {len(names)}.")
    for name in selected:
        log(f"  selected: {name}")
    for name in omitted:
        log(f"  omitted: {name}")
    if not omitted:
        log("  omitted: none")
    return selected


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def crate_entry(value, key, pattern):
    """Whether VALUE is exactly {KEY: STRING} with STRING matching PATTERN."""
    return (isinstance(value, dict) and set(value) == {key}
            and isinstance(value[key], str) and pattern.fullmatch(value[key]) is not None)


def crates(value):
    """The recorded core crates, exactly {"agent-relay-node": {"rev": SHA},
    "relaycast": {"version": VERSION}, "relayscrubber": {"version":
    VERSION}}."""
    if (not isinstance(value, dict)
            or set(value) != {"agent-relay-node", "relaycast", "relayscrubber"}
            or not crate_entry(value["agent-relay-node"], "rev", CRATE_REV)
            or not crate_entry(value["relaycast"], "version", CRATE_VERSION)
            or not crate_entry(value["relayscrubber"], "version", CRATE_VERSION)):
        raise ValueError('crates must be {"agent-relay-node": {"rev": SHA}, '
                         '"relaycast": {"version": VERSION}, '
                         '"relayscrubber": {"version": VERSION}}')
    return value


def identity(args):
    if not re.fullmatch(r"[0-9a-f]{40}", args.source_sha):
        raise ValueError("source SHA must be a full lowercase Git commit SHA")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", args.repository):
        raise ValueError("repository must be owner/name")
    if not re.fullmatch(r"[1-9][0-9]*", args.run_id):
        raise ValueError("run ID must be a positive integer")
    feed = getattr(args, "feed", "github")
    if feed not in FEEDS:
        raise ValueError("feed must be github or cloud")
    if not VERSIONS[feed].fullmatch(args.version):
        raise ValueError("invalid release version")
    if feed == "cloud" and not CLOUD_PROFILE.fullmatch(args.profile):
        raise ValueError("a cloud feed's profile is 1 to 32 lowercase letters and digits")
    if ("-beta." in args.version) != (args.channel == "beta"):
        raise ValueError("version and channel disagree")
    asset_prefix(args.profile)
    return {"source_sha": args.source_sha, "version": args.version,
            "channel": args.channel, "repository": args.repository,
            "run_id": args.run_id, "workflow": WORKFLOW, "profile": args.profile}


def create(args):
    manifest = {"schema": 1, **identity(args),
                "architectures": ARCHITECTURES,
                "checks": "passed", "files": inventory(args.directory, args.version, args.profile)}
    if getattr(args, "feed", "github") == "cloud":
        manifest["feed"] = "cloud"
    # A version taken as given, never checked against the cloud feed:
    # Desktop Promote checks the feed before it publishes this candidate.
    if getattr(args, "unlisted", False):
        if manifest.get("feed") != "cloud":
            raise ValueError("only a cloud candidate is prepared unlisted")
        manifest["unlisted"] = True
    if not args.tests_passed:
        raise ValueError("candidate creation requires successful release test prerequisites")
    if args.ci_run_id:
        if not re.fullmatch(r"[1-9][0-9]*", args.ci_run_id):
            raise ValueError("CI run ID must be a positive integer")
        manifest["ci_run_id"] = args.ci_run_id
    if getattr(args, "crates", None):
        manifest["crates"] = crates(json.loads(args.crates.read_text()))
    (args.directory / MANIFEST).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")


def recorded(args):
    """The manifest, if it names the requested identity and feed."""
    expected = identity(args)
    manifest = json.loads((args.directory / MANIFEST).read_text())
    # A candidate prepared before profiles were recorded is the default's.
    if manifest.get("schema") != 1 or any(manifest.get(key, "" if key == "profile" else None) != value
                                          for key, value in expected.items()):
        raise ValueError("candidate identity does not match requested source/version/channel/producer/profile")
    # A candidate prepared before feeds were recorded is a GitHub one.
    if manifest.get("feed", "github") != getattr(args, "feed", "github"):
        raise ValueError("candidate feed does not match the requested feed")
    if manifest.get("unlisted", True) is not True or ("unlisted" in manifest and manifest.get("feed") != "cloud"):
        raise ValueError("only a cloud candidate records unlisted, and only as true")
    # A candidate prepared before crates were recorded records none.
    if "crates" in manifest:
        crates(manifest["crates"])
    return manifest


def assets(args):
    """Print the cloud feed's asset names of a prepared cloud candidate,
    whose recorded files are still exactly its bytes."""
    if getattr(args, "feed", "github") != "cloud":
        raise ValueError("only a cloud candidate lists cloud assets")
    manifest = recorded(args)
    files = inventory(args.directory, args.version, args.profile)
    if manifest.get("files") != files:
        raise ValueError("candidate file inventory, sizes or SHA-256 hashes differ")
    names = cloud_assets(files, args.version, args.profile)
    text = "".join(name + "\n" for name in names)
    if args.output:
        args.output.write_text(text)
    else:
        sys.stdout.write(text)


def verify(args, now=None):
    manifest = recorded(args)
    if manifest.get("architectures") != ARCHITECTURES or manifest.get("checks") != "passed":
        raise ValueError("candidate does not record all architecture checks")
    run = json.loads(args.run_metadata.read_text())
    if (str(run.get("id")) != args.run_id
            or run.get("repository", {}).get("full_name") != args.repository
            or run.get("head_repository", {}).get("full_name") != args.repository
            or run.get("path") != WORKFLOW
            or run.get("event") not in ("workflow_dispatch", "workflow_run")
            or run.get("status") != "completed" or run.get("conclusion") != "success"):
        raise ValueError("GitHub run metadata does not prove a successful trusted candidate producer")
    if run["event"] == "workflow_dispatch":
        if run.get("head_sha") != args.source_sha:
            raise ValueError("candidate producer source revision differs")
    else:
        # A workflow_run producer's head_sha belongs to its default-branch
        # workflow context. The source must instead match the successful main
        # CI run recorded by the trusted preparation workflow.
        if not args.ci_run_metadata or not manifest.get("ci_run_id"):
            raise ValueError("automatic candidate requires independent CI run metadata")
        ci = json.loads(args.ci_run_metadata.read_text())
        if (str(ci.get("id")) != manifest["ci_run_id"]
                or ci.get("repository", {}).get("full_name") != args.repository
                or ci.get("head_repository", {}).get("full_name") != args.repository
                or ci.get("head_sha") != args.source_sha
                or ci.get("head_branch") != "main"
                or ci.get("path") != ".github/workflows/desktop-ci.yml"
                or ci.get("event") != "push"
                or ci.get("status") != "completed" or ci.get("conclusion") != "success"):
            raise ValueError("CI metadata does not prove this source passed Desktop CI on main")
    created = datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))
    now = now or datetime.now(timezone.utc)
    if created.tzinfo is None or created > now + timedelta(minutes=5) or now - created > timedelta(days=args.max_age_days):
        raise ValueError("candidate producer is stale or has an invalid creation time")
    if manifest.get("files") != inventory(args.directory, args.version, args.profile):
        raise ValueError("candidate file inventory, sizes or SHA-256 hashes differ")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["create", "verify", "cloud-assets"])
    parser.add_argument("--directory", required=True, type=Path)
    for name in ("source-sha", "version", "repository", "run-id"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--channel", choices=["stable", "beta"], required=True)
    parser.add_argument("--profile", default="")
    parser.add_argument("--tests-passed", action="store_true")
    parser.add_argument("--ci-run-id", default="")
    parser.add_argument("--ci-run-metadata", type=Path)
    parser.add_argument("--run-metadata", type=Path)
    parser.add_argument("--max-age-days", type=int, default=7)
    parser.add_argument("--feed", choices=FEEDS, default="github")
    parser.add_argument("--unlisted", action="store_true",
                        help="create: the version was taken as given, never checked against the cloud feed")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--crates", type=Path,
                        help="create: the core crates the build resolved (scripts/apply-crate-overrides.sh --json)")
    args = parser.parse_args()
    if args.command == "verify" and (not args.run_metadata or args.max_age_days <= 0):
        parser.error("verify requires --run-metadata and a positive --max-age-days")
    try:
        {"create": create, "verify": verify, "cloud-assets": assets}[args.command](args)
    except (ValueError, KeyError, OSError, TypeError) as exc:
        parser.exit(1, f"release candidate: {exc}\n")
    # The cloud asset list without --output is this command's whole stdout.
    if args.command != "cloud-assets" or args.output:
        print(f"Candidate {args.command} passed for {args.version} at {args.source_sha}")


if __name__ == "__main__":
    main()

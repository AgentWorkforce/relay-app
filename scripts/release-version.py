#!/usr/bin/env python3
"""Resolve stable and numbered beta CalVer releases without reusing a tag."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re

VERSION = re.compile(r"([0-9]+\.[0-9]+\.[0-9]+)(?:-beta\.([1-9][0-9]*))?\Z")


def resolve(releases, requested="", channel="stable", month=None):
    if channel not in ("stable", "beta"):
        raise ValueError(f"Unknown release channel: {channel}")
    tags = {release["tag_name"].removeprefix("v") for release in releases}
    published = [VERSION.fullmatch(tag) for tag in tags]
    published = [match for match in published if match]
    requested = requested.removeprefix("v")
    if not requested:
        month = month or datetime.now(timezone.utc).strftime("%Y.%m")
        stable_n = 0
        beta_n = 0
        for match in published:
            base, beta = match.groups()
            if base.rsplit(".", 1)[0] == month:
                n = int(base.rsplit(".", 1)[1])
                if beta:
                    beta_n = max(beta_n, n)
                else:
                    stable_n = max(stable_n, n)
        requested = f"{month}.{max(stable_n + 1, beta_n)}"
    match = VERSION.fullmatch(requested)
    if not match:
        raise ValueError("Use YYYY.MM.N for stable or YYYY.MM.N-beta.NUMBER for beta")
    base, beta = match.groups()
    if channel == "stable" and beta:
        raise ValueError("A beta version requires channel=beta")
    if channel == "beta" and not beta:
        if base in tags:
            raise ValueError(f"{base} is already stable; choose the next version for beta")
        numbers = [int(m[2]) for m in published if m[1] == base and m[2]]
        requested = f"{base}-beta.{max(numbers, default=0) + 1}"
    if requested in tags:
        raise ValueError(f"v{requested} already exists; published releases cannot be overwritten")
    if channel == "beta" and base in tags:
        raise ValueError(f"{base} is already stable; choose the next version for beta")
    return requested


def updater_version(value):
    """Parse a tag as the GTK and macOS updaters do (gtk/src/updates.rs):
    three numeric parts, compared by value, and an optional prerelease."""
    value = value.strip().removeprefix("v").split("+", 1)[0]
    numbers, _, prerelease = value.partition("-")
    if "-" in value and not re.fullmatch(r"[A-Za-z0-9.-]+", prerelease):
        return None
    parts = numbers.split(".")
    if len(parts) != 3 or not all(re.fullmatch(r"[0-9]+", part) for part in parts):
        return None
    return tuple(int(part) for part in parts), (prerelease if "-" in value else None)


def prerelease_order(left, right):
    """-1, 0 or 1 as the updaters order prerelease labels: digit runs by
    value, everything else as case-insensitive text."""
    left, right = (re.findall(r"[0-9]+|[^0-9]+", label) for label in (left, right))
    for a, b in zip(left, right):
        if a.isdigit() and b.isdigit():
            a, b = a.lstrip("0"), b.lstrip("0")
            a, b = (len(a), a), (len(b), b)
        else:
            a, b = a.lower(), b.lower()
        if a != b:
            return 1 if a > b else -1
    return (len(left) > len(right)) - (len(left) < len(right))


def is_newer(candidate, installed):
    """Whether the updaters rank `candidate` after `installed`; a release
    outranks its own prereleases and an unparseable version is never newer."""
    new, old = updater_version(candidate), updater_version(installed)
    if not new or not old:
        return False
    if new[0] != old[0]:
        return new[0] > old[0]
    if new[1] is not None and old[1] is not None:
        return prerelease_order(new[1], old[1]) > 0
    return new[1] is None and old[1] is not None


def check_newer_than_all(releases, version):
    """Refuse a version the updaters would not offer over every existing
    release, e.g. a cloud-allocated 2026.3.1 against a bridge 2026.03.2."""
    if not updater_version(version):
        raise ValueError(f"{version} is not a version the updaters can read")
    for release in releases:
        tag = release["tag_name"]
        if updater_version(tag) and not is_newer(version, tag):
            raise ValueError(
                f"v{version.removeprefix('v')} does not sort after {tag} for the updaters; "
                "publish a later version"
            )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--releases", type=Path, required=True)
    parser.add_argument("--version", default="")
    parser.add_argument("--channel", choices=["stable", "beta"], default="stable")
    parser.add_argument("--tag", help="A push tag; its suffix determines the channel")
    parser.add_argument(
        "--newer-than-all", action="store_true",
        help="Also require the version to sort after every release, as the updaters compare",
    )
    args = parser.parse_args()
    requested = args.tag.removeprefix("v") if args.tag else args.version
    channel = ("beta" if "-beta." in requested else "stable") if args.tag else args.channel
    releases = json.loads(args.releases.read_text())
    version = resolve(releases, requested, channel)
    if args.newer_than_all:
        check_newer_than_all(releases, version)
    print(f"version={version}")
    print(f"tag=v{version}")
    print(f"channel={channel}")
    print(f"title={'Beta' if channel == 'beta' else 'Stable'} v{version}")


if __name__ == "__main__":
    try:
        main()
    except ValueError as exc:
        raise SystemExit(f"release-version: {exc}") from exc

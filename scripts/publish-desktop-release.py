#!/usr/bin/env python3
"""Publish the default desktop release with GitHub asset display labels."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time

_candidate_spec = importlib.util.spec_from_file_location(
    "release_candidate", Path(__file__).with_name("release-candidate.py")
)
release_candidate = importlib.util.module_from_spec(_candidate_spec)
_candidate_spec.loader.exec_module(release_candidate)

REPOSITORY = "AgentWorkforce/relay-desktop-releases"
ARCHITECTURES = {"arm64": "Apple silicon", "x64": "Intel"}


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True, env=dict(os.environ, GH_HOST="github.com"))


def find_release(tag):
    pages = json.loads(gh("api", f"repos/{REPOSITORY}/releases?per_page=100",
                          "--paginate", "--slurp"))
    matches = [item for page in pages for item in page if item["tag_name"] == tag]
    if len(matches) > 1:
        raise ValueError(f"Multiple releases match {tag}")
    return matches[0] if matches else None


def upload_arguments(directory, version):
    paths = sorted(directory.iterdir())
    if any(path.is_symlink() or not path.is_file() or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", path.name)
           for path in paths):
        raise ValueError("Invalid release asset path")
    names = {path.name for path in paths}
    required = release_candidate.required_assets(version)
    required.update(names.intersection(release_candidate.OPTIONAL_INSTALL_SCRIPTS))
    labels = {}
    floors = set()
    for arch, processor in ARCHITECTURES.items():
        stem = f"AgentRelay-{version}-macOS"
        floored = [(name, match[1]) for name in names
                   if (match := re.fullmatch(rf"{re.escape(stem)}([1-9][0-9]*)-{arch}\.dmg", name))]
        if len(floored) != 1:
            raise ValueError(f"Expected one versioned macOS installer for {arch}")
        (current, floor), = floored
        legacy = f"{stem}-{arch}.dmg"
        alias = f"AgentRelay-macOS-{arch}.dmg"
        floors.add(int(floor))
        required.add(current)
        label = f"Agent Relay {version} installer ({processor}, macOS {floor}+)"
        labels[current] = label
        labels[alias] = f"Current installer ({processor}, macOS {floor}+)"
        labels[legacy] = (f"Compatibility bridge for older updaters ({processor}, macOS 13+)"
                          if int(floor) > 13 else label)
    if len(floors) != 1 or min(floors) < 13:
        raise ValueError("Invalid or inconsistent macOS deployment floor")
    expected = required | {f"{name}.sha256" for name in required}
    if names != expected:
        raise ValueError(f"Release inventory differs: missing={sorted(expected - names)}, "
                         f"unexpected={sorted(names - expected)}")
    return [f"{path}#{labels[path.name]}" if path.name in labels else str(path) for path in paths]


def publish(args):
    if os.environ["PROFILE"] or os.environ["RELEASES_REPOSITORY"].lower() != REPOSITORY.lower():
        raise ValueError("The labeled publisher requires the default product repository")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-beta\.[1-9][0-9]*)?", args.version):
        raise ValueError("Invalid desktop release version")
    tag = f"v{args.version}"
    # Match the ownership marker used by both version reservation steps.
    source = os.environ["GITHUB_REPOSITORY"]
    run_id = os.environ["GITHUB_RUN_ID"]
    draft_title = f"Desktop release draft from {source} run {run_id}"
    uploads = upload_arguments(args.assets, args.version)
    release = find_release(tag)
    if release is None:
        flags = ["--prerelease"] if args.channel == "beta" else []
        gh("release", "create", tag, "--repo", REPOSITORY, "--target", "main", "--draft",
           "--title", draft_title, "--notes-file", str(args.notes), *flags)
        release = find_release(tag)

    for attempt in range(3):
        if not release or not release["draft"] or release["name"] != draft_title:
            raise ValueError(f"{tag} is not a draft owned by this workflow run")
        try:
            # gh splits the display label from the path. --clobber replaces
            # partially uploaded assets on a retry while the release is private.
            gh("release", "upload", tag, "--repo", REPOSITORY, "--clobber", *uploads)
            break
        except subprocess.CalledProcessError:
            if attempt == 2:
                raise
            time.sleep(2 ** attempt)
            release = find_release(tag)

    ready = find_release(tag)
    if not ready or not ready["draft"] or ready["name"] != draft_title or ready["id"] != release["id"]:
        raise ValueError(f"The draft for {tag} changed during upload")
    gh("api", "--method", "PATCH", f"repos/{REPOSITORY}/releases/{int(ready['id'])}",
       "-f", f"name={args.title}", "-F", "draft=false",
       "-f", f"make_latest={'true' if args.channel == 'stable' else 'false'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("channel", choices=("stable", "beta"))
    parser.add_argument("title")
    parser.add_argument("notes", type=Path)
    parser.add_argument("assets", type=Path)
    try:
        publish(parser.parse_args())
    except (ValueError, subprocess.CalledProcessError) as exc:
        raise SystemExit(f"publish-desktop-release: {exc}") from exc


if __name__ == "__main__":
    main()

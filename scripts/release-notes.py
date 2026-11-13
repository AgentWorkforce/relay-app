#!/usr/bin/env python3
"""Build release notes from CHANGELOG.md and promote its Unreleased section."""

import argparse
from datetime import datetime, timezone
import importlib.util
import json
import re
import sys
from pathlib import Path

_assets_spec = importlib.util.spec_from_file_location(
    "macos_release_assets", Path(__file__).with_name("macos-release-assets.py")
)
macos_release_assets = importlib.util.module_from_spec(_assets_spec)
_assets_spec.loader.exec_module(macos_release_assets)
_version_spec = importlib.util.spec_from_file_location(
    "release_version", Path(__file__).with_name("release-version.py")
)
release_version = importlib.util.module_from_spec(_version_spec)
_version_spec.loader.exec_module(release_version)

# A cloud feed's versions follow the proposed Cloud contract (relay-control
# docs/desktop-feed.md): the month has no leading zero.
CLOUD_VERSION = re.compile(r"[0-9]{4}\.(?:[1-9]|1[0-2])\.(?:0|[1-9][0-9]*)(?:-beta\.(?:0|[1-9][0-9]*))?")

UNRELEASED_HEADING = "## Unreleased"


class NoReleaseNotes(ValueError):
    """A structurally valid selected section has no release note bullets."""



def _sections(text: str) -> list[str]:
    return re.split(r"(?=^## )", text, flags=re.MULTILINE)


def _section_body(section: str) -> str:
    return "\n".join(section.splitlines()[1:]).strip()


def _require_bullets(heading: str, body: str) -> str:
    if not re.search(r"^[-*] \S", body, flags=re.MULTILINE):
        raise NoReleaseNotes(f"{heading} must contain at least one release note bullet")
    return body


def unreleased_changes(changelog: Path) -> str:
    """The Unreleased section's bullets, whatever version is being built.

    A site profile's releases are numbered by its own repository, so a
    `## v<version>` section of this changelog is another release's notes; a
    profile ships what main has not released yet, named as such.
    """
    sections = _sections(changelog.read_text(encoding="utf-8"))
    unreleased = [s for s in sections if s.startswith(f"{UNRELEASED_HEADING}\n")]
    if len(unreleased) != 1:
        raise ValueError(f"Expected one {UNRELEASED_HEADING} section in {changelog}")
    return _require_bullets(UNRELEASED_HEADING, _section_body(unreleased[0]))


def changes_for_version(changelog: Path, version: str) -> str:
    sections = _sections(changelog.read_text(encoding="utf-8"))
    heading = f"## v{version}"
    matches = [s for s in sections if s.startswith(f"{heading}\n")]
    if len(matches) > 1:
        raise ValueError(f"Expected exactly one {heading} section in {changelog}")
    if matches:
        return _require_bullets(heading, _section_body(matches[0]))

    # Beta builds can share the upcoming stable version's cumulative notes.
    if "-beta." in version:
        base_heading = f"## v{version.split('-beta.', 1)[0]}"
        matches = [s for s in sections if s.startswith(f"{base_heading}\n")]
        if len(matches) > 1:
            raise ValueError(f"Expected exactly one {base_heading} section in {changelog}")
        if matches:
            return _require_bullets(base_heading, _section_body(matches[0]))

    unreleased = [s for s in sections if s.startswith(f"{UNRELEASED_HEADING}\n")]
    if len(unreleased) != 1:
        raise ValueError(
            f"Expected one {heading} or {UNRELEASED_HEADING} section in {changelog}"
        )
    return _require_bullets(UNRELEASED_HEADING, _section_body(unreleased[0]))


def promote_changelog(changelog: Path, version: str) -> bool:
    """Rename Unreleased to v<version> above a fresh Unreleased. True if written."""
    text = changelog.read_text(encoding="utf-8")
    sections = _sections(text)
    heading = f"## v{version}"
    if any(s.startswith(f"{heading}\n") for s in sections):
        changes_for_version(changelog, version)
        return False

    indexes = [i for i, s in enumerate(sections) if s.startswith(f"{UNRELEASED_HEADING}\n")]
    if len(indexes) != 1:
        raise ValueError(f"Expected exactly one {UNRELEASED_HEADING} section in {changelog}")
    body = _require_bullets(UNRELEASED_HEADING, _section_body(sections[indexes[0]]))
    sections[indexes[0]] = f"{UNRELEASED_HEADING}\n\n{heading}\n\n{body}\n\n"
    changelog.write_text("".join(sections), encoding="utf-8")
    return True


def _section_index(sections: list[str], heading: str) -> int | None:
    indexes = [i for i, s in enumerate(sections) if s.startswith(f"{heading}\n")]
    if len(indexes) > 1:
        raise ValueError(f"Expected at most one {heading} section")
    return indexes[0] if indexes else None


def merge_promotion(base_text: str, promoted_text: str, latest_text: str, version: str) -> tuple[str, bool]:
    """Apply a snapshot's promotion onto the newest changelog.

    The v<version> section takes the snapshot's Unreleased bullets. Lines the
    newest changelog added to Unreleased after the snapshot stay in Unreleased.
    """
    heading = f"## v{version}"
    promoted = _sections(promoted_text)
    p_idx = _section_index(promoted, heading)
    if p_idx is None:
        raise ValueError(f"Promoted snapshot has no {heading} section")
    version_body = _require_bullets(heading, _section_body(promoted[p_idx]))

    latest = _sections(latest_text)
    v_idx = _section_index(latest, heading)
    if v_idx is not None:
        if _section_body(latest[v_idx]) != version_body:
            raise ValueError(f"Latest changelog already has a different {heading} section")
        return latest_text, False

    base = _sections(base_text)
    b_idx = _section_index(base, UNRELEASED_HEADING)
    base_lines = _section_body(base[b_idx]).splitlines() if b_idx is not None else []

    l_idx = _section_index(latest, UNRELEASED_HEADING)
    if l_idx is None:
        raise ValueError(f"Latest changelog has no {UNRELEASED_HEADING} section")
    remaining = _section_body(latest[l_idx]).splitlines()
    for line in base_lines:
        if line in remaining:
            remaining.remove(line)
    additions = "\n".join(remaining).strip()
    if additions:
        latest[l_idx] = f"{UNRELEASED_HEADING}\n\n{additions}\n\n{heading}\n\n{version_body}\n\n"
    else:
        latest[l_idx] = f"{UNRELEASED_HEADING}\n\n{heading}\n\n{version_body}\n\n"
    return "".join(latest), True


def feed_versions(listing: Path) -> set[str]:
    """Every version a cloud feed holds, drafts included: a draft's version
    is taken, since the feed refuses a second draft of it."""
    data = json.loads(listing.read_text(encoding="utf-8"))
    versions = data.get("versions") if isinstance(data, dict) else None
    if not isinstance(versions, list) or not all(
            isinstance(entry, dict) and isinstance(entry.get("version"), str) for entry in versions):
        raise ValueError(f"{listing} is not a feed listing")
    return {entry["version"] for entry in versions}


def allocate_cloud(listing: Path, requested: str, channel: str, month: str | None = None) -> str:
    """The next free cloud feed version, as release-version.py allocates a
    GitHub one, with the month written without a leading zero."""
    if month is None:
        now = datetime.now(timezone.utc)
        month = f"{now.year}.{now.month}"
    taken = feed_versions(listing)
    version = release_version.resolve([{"tag_name": f"v{v}"} for v in taken], requested, channel, month)
    if not CLOUD_VERSION.fullmatch(version):
        raise ValueError("Use YYYY.M.N (no leading zero in the month) for a cloud feed version")
    return version


def given_cloud(requested: str, channel: str) -> str:
    """An explicit cloud feed version used as given, with no listing: the
    version and channel rules still apply, and nothing is allocated (a plain
    version on the beta channel is refused, not turned into a beta)."""
    version = release_version.resolve([], requested, channel)
    if version != requested.removeprefix("v"):
        raise ValueError("A plain version requires channel=stable")
    if not CLOUD_VERSION.fullmatch(version):
        raise ValueError("Use YYYY.M.N (no leading zero in the month) for a cloud feed version")
    return version


def cloud_body(headline: str, version: str, changes: str, probe_version: str) -> str:
    # A cloud feed's downloads live on the site, which the notes never name.
    return f"""{headline}

**Channel: {'Beta (opt-in)' if '-beta.' in version else 'Stable'}**

Install it from your site's downloads page, or update from the app.

## What's new in this release

{changes}

**Bundled probe**

Bundled `agent-relay-probe`: `{probe_version}`.
"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", default="")
    parser.add_argument("--changelog", type=Path, default=Path("CHANGELOG.md"))
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--promote", action="store_true")
    parser.add_argument("--base", type=Path)
    parser.add_argument("--merge", type=Path)
    parser.add_argument("--repository", default="AgentWorkforce/relay-desktop-releases")
    # The site profile's artifact prefix: AgentRelay, or Desktop-<id>, whose
    # notes name the profile by its id and never a product.
    parser.add_argument("--asset-prefix", default=macos_release_assets.DEFAULT_PREFIX)
    # A profile's notes: the Unreleased section, never a `## v<version>`
    # section, whose number belongs to the default profile's releases.
    parser.add_argument("--unreleased", action="store_true")
    parser.add_argument("--probe-version")
    parser.add_argument("--output", type=Path)
    # A cloud feed has no releases repository: these notes link to no
    # release page (desktop-feed-publish.py adds the downloads-page link to
    # notes it cuts), and
    # --allocate resolves the version against the feed's listing (written
    # by desktop-feed-publish.py list) instead of GitHub releases.
    parser.add_argument("--feed", choices=["github", "cloud"], default="github")
    parser.add_argument("--feed-listing", type=Path)
    parser.add_argument("--allocate", action="store_true")
    # --allocate --as-given: a prepare-only candidate whose feed was not
    # listed takes its explicit version as given; the feed checks it on publish.
    parser.add_argument("--as-given", action="store_true")
    parser.add_argument("--channel", choices=["stable", "beta"], default="stable")
    args = parser.parse_args()

    if args.allocate:
        if args.feed != "cloud" or bool(args.feed_listing) == args.as_given:
            parser.error("--allocate needs --feed cloud and either --feed-listing or --as-given")
        if args.as_given and not args.version:
            parser.error("--as-given needs the version")
        version = (given_cloud(args.version, args.channel) if args.as_given
                   else allocate_cloud(args.feed_listing, args.version, args.channel))
        print(f"version={version}")
        print(f"tag=v{version}")
        print(f"channel={args.channel}")
        print(f"title={'Beta' if args.channel == 'beta' else 'Stable'} v{version}")
        return
    if not args.version:
        parser.error("the version is required")
    if args.feed == "cloud":
        if not CLOUD_VERSION.fullmatch(args.version):
            raise ValueError("Use YYYY.M.N (no leading zero in the month) for a cloud feed version")
        if args.feed_listing and args.version in feed_versions(args.feed_listing):
            raise ValueError(f"v{args.version} is already in the cloud feed")

    if args.merge:
        if not args.base:
            parser.error("--base is required with --merge")
        merged, changed = merge_promotion(
            args.base.read_text(encoding="utf-8"),
            args.changelog.read_text(encoding="utf-8"),
            args.merge.read_text(encoding="utf-8"),
            args.version,
        )
        if changed:
            args.merge.write_text(merged, encoding="utf-8")
            print(f"Merged v{args.version} promotion into {args.merge}")
        else:
            print(f"v{args.version} already promoted in {args.merge}")
        return

    if args.promote:
        changed = promote_changelog(args.changelog, args.version)
        print(f"Promoted Unreleased to v{args.version}" if changed else f"v{args.version} already in CHANGELOG.md")
        return

    changes = unreleased_changes(args.changelog) if args.unreleased else changes_for_version(args.changelog, args.version)
    if args.check:
        return
    if not args.probe_version or not args.output:
        parser.error("--probe-version and --output are required unless --check or --promote is set")

    tag = f"v{args.version}"
    if args.feed == "cloud":
        prefix = macos_release_assets.check_prefix(args.asset_prefix)
        headline = (f"Agent Relay for macOS, Linux and Windows {tag}." if prefix == macos_release_assets.DEFAULT_PREFIX
                    else f"{prefix} {tag} for macOS, Linux and Windows.")
        args.output.write_text(cloud_body(headline, args.version, changes, args.probe_version), encoding="utf-8")
        return
    base = f"https://github.com/{args.repository}/releases/download/{tag}"
    prefix = macos_release_assets.check_prefix(args.asset_prefix)
    floor = macos_release_assets.deployment_floor()
    macos = f"macOS {floor} or newer"
    bridge = ""
    if macos_release_assets.bridges(floor, prefix):
        bridge_version = macos_release_assets.BRIDGE_VERSION
        floor_aware = macos_release_assets.FLOOR_AWARE_VERSION
        arm64, x64 = (macos_release_assets.legacy_name(args.version, arch, prefix) for arch in ("arm64", "x64"))
        bridge = (
            f"- Updater bridge: [`{arm64}`]({base}/{arm64}) and [`{x64}`]({base}/{x64}): for updaters older than "
            f"{floor_aware}; installs {bridge_version}, which then updates itself\n"
        )
    # The default profile's notes name the product; another profile's name
    # it by its artifact prefix, the opaque id, never a product name.
    headline = (f"Agent Relay for macOS, Linux and Windows {tag}." if prefix == macos_release_assets.DEFAULT_PREFIX
                else f"{prefix} {tag} for macOS, Linux and Windows.")

    def link(name):
        return f"[`{prefix}-{name}`]({base}/{prefix}-{name})"

    body = f"""{headline}

**Channel: {'Beta (opt-in)' if '-beta.' in args.version else 'Stable'}** · [Stable releases](https://github.com/{args.repository}/releases?q=prerelease%3Afalse) · [Beta releases](https://github.com/{args.repository}/releases?q=prerelease%3Atrue)

**Downloads**

- Apple silicon: {link("macOS-arm64.dmg")} ({macos})
- Intel: {link("macOS-x64.dmg")} ({macos})
{bridge}- Linux x64: {link("Linux-x64.deb")} or {link("Linux-x64.tar.gz")}
- Linux arm64: {link("Linux-arm64.deb")} or {link("Linux-arm64.tar.gz")}
- Windows x64: {link("Windows-x64-Setup.exe")} or {link("Windows-x64.zip")}
- Windows arm64: {link("Windows-arm64-Setup.exe")} or {link("Windows-arm64.zip")}

## What's new in this release

{changes}

**Bundled probe**

Bundled `agent-relay-probe`: `{args.probe_version}`.
"""
    args.output.write_text(body, encoding="utf-8")


if __name__ == "__main__":
    try:
        main()
    except NoReleaseNotes as exc:
        print(f"release-notes: {exc}", file=sys.stderr)
        raise SystemExit(3) from exc
    except ValueError as exc:
        raise SystemExit(f"release-notes: {exc}") from exc

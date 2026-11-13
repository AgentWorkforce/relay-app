#!/usr/bin/env python3
"""Name and stage the macOS disk images a release publishes.

The version-pinned image carries the oldest macOS it runs on, taken from the
deployment target in macos/Package.swift:

    AgentRelay-<version>-macOS<floor>-<arch>.dmg

The in-app updater reads that floor and skips images this Mac cannot open.
Updaters older than FLOOR_AWARE_VERSION look only for the unfloored legacy name
`AgentRelay-<version>-macOS-<arch>.dmg` in the newest release. While the floor
is LEGACY_FLOOR that name carries the built image. Above it, the legacy name
carries the BRIDGE_VERSION image instead: a macOS 13 build every legacy Mac
runs, whose floor-aware updater then takes the newest release this Mac can
open, or reports which macOS it needs. Floor-aware updaters ignore the
unfloored name whenever floored names exist. The stable
`AgentRelay-macOS-<arch>.dmg` name, used by agentrelay.com and the README, is
always published.

`AgentRelay` is the default site profile's prefix (docs/workspace-layout.md,
"Site profiles"); another profile's artifacts start with `Desktop-<id>`
(--asset-prefix), and as none of its releases predate floor-aware updaters,
its legacy name carries the built image, never a bridge.

Usage:
    macos-release-assets.py VERSION ARCH SOURCE.dmg DEST_DIR [--asset-prefix PREFIX]

Copies the built image to every published name in DEST_DIR, stages the
bridge image with its `.sha256` when the floor needs one, and prints the
image names.
"""

import argparse
import hashlib
import re
import shutil
import sys
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path

PACKAGE_SWIFT = Path(__file__).resolve().parent.parent / "macos" / "Package.swift"
DEFAULT_PREFIX = "AgentRelay"
PREFIX = re.compile(r"AgentRelay|Desktop-(?=[a-z0-9-]{1,40}\Z)[a-z0-9]+(?:-[a-z0-9]+)*")
LEGACY_FLOOR = 13
ARCHITECTURES = ("arm64", "x64")
# The first release whose updater reads floored names.
FLOOR_AWARE_VERSION = "2026.10.65"
# The last macOS 13 release: floor-aware, and it discards update metadata
# cached by an older app, so it looks the newest release up again at launch.
BRIDGE_VERSION = "2026.10.66"
BRIDGE_RELEASE_URL = (
    "https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/v" + BRIDGE_VERSION
)

Download = Callable[[str], bytes]

_PLATFORM = re.compile(r"\.macOS\(\s*(?:\.v(\d+(?:_\d+)*)|\"(\d+(?:\.\d+)*)\")\s*\)")


def deployment_floor(package_swift: Path = PACKAGE_SWIFT) -> int:
    """The macOS version in Package.swift's `.macOS(...)` platform, a whole major."""
    matches = _PLATFORM.findall(package_swift.read_text(encoding="utf-8"))
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one .macOS(...) platform in {package_swift}")
    target = matches[0][0] or matches[0][1]
    major, *rest = (int(part) for part in re.split(r"[._]", target))
    if any(rest):
        # The asset name and the updater compare major versions only.
        raise ValueError(
            f"macOS deployment target {target.replace('_', '.')} in {package_swift} is not a major "
            "version; release asset names carry only the major, so use a whole version such as "
            f'.macOS("{major + 1}.0")'
        )
    if major < LEGACY_FLOOR:
        raise ValueError(f"macOS deployment target {major} is below {LEGACY_FLOOR}")
    return major


def check_prefix(prefix: str) -> str:
    if not PREFIX.fullmatch(prefix):
        raise ValueError(f"The asset prefix must be AgentRelay or Desktop-<id>, got {prefix!r}")
    return prefix


def bridges(floor: int, prefix: str = DEFAULT_PREFIX) -> bool:
    """Whether the legacy name carries the bridge image: above the legacy
    floor, for the default profile only."""
    return floor > LEGACY_FLOOR and check_prefix(prefix) == DEFAULT_PREFIX


def pinned_names(version: str, arch: str, floor: int, prefix: str = DEFAULT_PREFIX) -> list[str]:
    """Version-pinned image names for one architecture, floored name first."""
    if arch not in ARCHITECTURES:
        raise ValueError(f"Unknown architecture {arch!r}")
    names = [f"{check_prefix(prefix)}-{version}-macOS{floor}-{arch}.dmg"]
    if not bridges(floor, prefix):
        names.append(legacy_name(version, arch, prefix))
    return names


def legacy_name(version: str, arch: str, prefix: str = DEFAULT_PREFIX) -> str:
    """The unfloored name updaters older than FLOOR_AWARE_VERSION install."""
    return f"{check_prefix(prefix)}-{version}-macOS-{arch}.dmg"


def stable_name(arch: str, prefix: str = DEFAULT_PREFIX) -> str:
    return f"{check_prefix(prefix)}-macOS-{arch}.dmg"


def fetch(url: str) -> bytes:
    try:
        with urllib.request.urlopen(url, timeout=300) as response:
            return response.read()
    except (urllib.error.URLError, TimeoutError) as exc:
        raise ValueError(f"could not download {url}: {exc}") from exc


class BridgeImages:
    """The BRIDGE_VERSION images, downloaded and digest-checked once per arch."""

    def __init__(self, download: Download = fetch):
        self._download = download
        self._images: dict[str, bytes] = {}

    def image(self, arch: str) -> bytes:
        if arch not in self._images:
            name = legacy_name(BRIDGE_VERSION, arch)
            checksum = self._download(f"{BRIDGE_RELEASE_URL}/{name}.sha256").decode("utf-8", "replace")
            fields = checksum.split()
            if len(fields) != 2 or fields[1] != name or not re.fullmatch(r"[0-9a-f]{64}", fields[0]):
                raise ValueError(f"{name}.sha256 is not '<sha256>  {name}': {checksum.strip()!r}")
            image = self._download(f"{BRIDGE_RELEASE_URL}/{name}")
            actual = hashlib.sha256(image).hexdigest()
            if actual != fields[0]:
                raise ValueError(f"{name} has sha256 {actual}, its .sha256 says {fields[0]}")
            self._images[arch] = image
        return self._images[arch]


def stage(
    version: str, arch: str, source: Path, dest: Path, floor: int, bridge: BridgeImages | None = None,
    prefix: str = DEFAULT_PREFIX,
) -> list[str]:
    """Copy one built image to every published name, plus the bridge image under
    the legacy name above LEGACY_FLOOR (the default profile only). Returns the
    image names written."""
    if not source.is_file():
        raise ValueError(f"missing build artifact {source}")
    pinned = pinned_names(version, arch, floor, prefix)
    # Fetch before writing anything: a release never goes out half-bridged.
    bridge_image = (bridge or BridgeImages()).image(arch) if bridges(floor, prefix) else None
    dest.mkdir(parents=True, exist_ok=True)
    names = [*pinned, stable_name(arch, prefix)]
    for name in names:
        shutil.copyfile(source, dest / name)
    if bridge_image is not None:
        name = legacy_name(version, arch, prefix)
        (dest / name).write_bytes(bridge_image)
        (dest / f"{name}.sha256").write_text(
            f"{hashlib.sha256(bridge_image).hexdigest()}  {name}\n", encoding="utf-8"
        )
        names.insert(len(pinned), name)
    return names


def main(argv: list[str] | None = None, bridge: BridgeImages | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version")
    parser.add_argument("arch")
    parser.add_argument("source", type=Path)
    parser.add_argument("dest", type=Path)
    parser.add_argument("--package-swift", type=Path, default=PACKAGE_SWIFT)
    parser.add_argument("--asset-prefix", default=DEFAULT_PREFIX)
    args = parser.parse_args(argv)
    floor = deployment_floor(args.package_swift)
    for name in stage(args.version, args.arch, args.source, args.dest, floor, bridge, args.asset_prefix):
        print(name)


if __name__ == "__main__":
    try:
        main()
    except ValueError as exc:
        sys.exit(f"macos-release-assets: {exc}")

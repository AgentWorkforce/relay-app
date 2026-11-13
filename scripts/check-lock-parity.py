#!/usr/bin/env python3
"""Fails when the desktop core and app lockfiles resolve a shared crate differently.

The app links the desktop core and relay's core, and each project keeps its own
Cargo.lock. Without this check, a dependency update in one lock would leave the
other behind. A crate is its version and source together, so a moved git pin of
relay's core counts.
"""

from collections import defaultdict
from pathlib import Path
import re
import sys

REPO = Path(__file__).resolve().parents[1]


def packages(text):
    found = defaultdict(set)
    for block in text.split("[[package]]")[1:]:
        fields = dict(re.findall(r'^(name|version|source) = "(.*)"$', block, re.M))
        found[fields["name"]].add((fields["version"], fields.get("source", "")))
    return found


def drifted(core, app):
    """Crates whose core resolution the app does not also have."""
    return sorted(name for name in core.keys() & app.keys() if core[name] - app[name])


def main():
    core = packages((REPO / "desktop-core/Cargo.lock").read_text())
    failed = False
    for lock in ("gtk/Cargo.lock",):
        other = packages((REPO / lock).read_text())
        for name in drifted(core, other):
            print(f"{name}: core {sorted(core[name])}, {lock} {sorted(other[name])}")
            failed = True
    if failed:
        sys.exit("Update the app and desktop core lockfiles together.")
    print("lockfiles agree")


if __name__ == "__main__":
    main()

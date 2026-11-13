#!/usr/bin/env python3
"""Check the lockfile parity gate on small locks."""

import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "parity", Path(__file__).with_name("check-lock-parity.py")
)
parity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(parity)


def lock(*entries):
    return "".join(f'[[package]]\nname = "{n}"\nversion = "{v}"\n{s}' for n, v, s in entries)


class Parity(unittest.TestCase):
    def test_equal_locks_agree(self):
        core = parity.packages(lock(("tar", "1", "")))
        self.assertEqual(parity.drifted(core, core), [])

    def test_a_different_version_drifts(self):
        core = parity.packages(lock(("tar", "2", "")))
        app = parity.packages(lock(("tar", "1", "")))
        self.assertEqual(parity.drifted(core, app), ["tar"])

    def test_a_moved_git_pin_drifts(self):
        core = parity.packages(lock(("hist", "1", 'source = "git+x#a"\n')))
        app = parity.packages(lock(("hist", "1", 'source = "git+x#b"\n')))
        self.assertEqual(parity.drifted(core, app), ["hist"])

    def test_a_crate_only_the_app_needs_is_ignored(self):
        core = parity.packages(lock(("tar", "1", "")))
        app = parity.packages(lock(("tar", "1", ""), ("gtk4", "1", "")))
        self.assertEqual(parity.drifted(core, app), [])


if __name__ == "__main__":
    unittest.main()

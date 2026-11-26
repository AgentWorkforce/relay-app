#!/usr/bin/env python3
"""check-telemetry-key.sh accepts only a PostHog project token. Synthetic keys only."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / 'check-telemetry-key.sh'
WORKFLOW = Path(__file__).resolve().parents[1] / '.github/workflows/desktop-release.yml'
# Synthetic boundary cases for the capture-token grammar.
ACCEPTED = ('phc_A', 'phc_' + 'A' * 251, 'phc_a_b', 'phc_' + 'T' * 39)
REFUSED = (None, '', 'phc_', 'phc_' + 'A' * 252, 'phx_' + 'S' * 39, 'phx_short', 'sk_' + 'A' * 40,
           'phc_' + 'T' * 39 + ' ', 'phc_' + 'T' * 39 + '-x', 'phc_a-b', 'PHC_abc', ' phc_abc')
PROJECT = 'phc_' + 'T' * 39          # synthetic, never a real key
OTHER_PROJECT = 'phc_' + 'U' * 39
PERSONAL = 'phx_' + 'S' * 39


def run(key, *args):
    env = {k: v for k, v in os.environ.items() if k != 'AGENT_RELAY_POSTHOG_API_KEY'}
    if key is not None:
        env['AGENT_RELAY_POSTHOG_API_KEY'] = key
    return subprocess.run(['bash', str(SCRIPT), *args], env=env, capture_output=True, text=True)


class SourceKey(unittest.TestCase):
    def test_accepts_a_project_token(self):
        self.assertEqual(run(PROJECT, 'source').returncode, 0)

    def test_rejects_missing_personal_and_malformed_keys(self):
        for key in REFUSED:
            with self.subTest(key=key and key[:5], length=key and len(key)):
                self.assertNotEqual(run(key, 'source').returncode, 0)

    def test_accepts_the_shortest_and_longest_bodies(self):
        for key in ACCEPTED:
            with self.subTest(length=len(key)):
                self.assertEqual(run(key, 'source').returncode, 0)

    def test_never_prints_the_key(self):
        for key in (PROJECT, PERSONAL):
            result = run(key, 'source')
            self.assertNotIn(key, result.stdout + result.stderr)
            self.assertNotIn(key[4:], result.stdout + result.stderr)


def preflight_script():
    """The release preflight step's shell, read from the workflow text."""
    lines = WORKFLOW.read_text().splitlines()
    start = lines.index('      - name: Require analytics release configuration')
    run_at = next(i for i in range(start, len(lines)) if lines[i].strip() == 'run: |')
    body = []
    for line in lines[run_at + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) < 10:
            break
        body.append(line[10:])
    return '\n'.join(body) + '\n'


class ReleasePreflight(unittest.TestCase):
    """The preflight agrees with the shared rule, with and without it checked out."""

    def preflight(self, key, with_script):
        with tempfile.TemporaryDirectory() as checkout:
            if with_script:
                Path(checkout, 'scripts').mkdir()
                Path(checkout, 'scripts', SCRIPT.name).write_text(SCRIPT.read_text())
            env = {'PATH': os.environ['PATH'], 'POSTHOG_API_KEY': key or ''}
            return subprocess.run(['bash', '-c', preflight_script()], cwd=checkout, env=env,
                                  capture_output=True, text=True)

    def test_matches_the_shared_rule_on_every_boundary(self):
        for key in ACCEPTED + REFUSED:
            expected = run(key, 'source').returncode == 0
            for with_script in (True, False):
                with self.subTest(key=key and key[:5], length=key and len(key), with_script=with_script):
                    result = self.preflight(key, with_script)
                    self.assertEqual(result.returncode == 0, expected)
                    if key and len(key) > 4:
                        self.assertNotIn(key[4:], result.stdout + result.stderr)


class PackagedHelper(unittest.TestCase):
    def helper(self, content):
        directory = tempfile.mkdtemp()
        path = Path(directory, 'Agent Relay Helper')
        path.write_bytes(b'\x00\xcf\xfa\xed' + content + b'\x00tail')
        return str(path)

    def test_accepts_a_helper_with_exactly_the_project_token(self):
        self.assertEqual(run(PROJECT, 'packaged', self.helper(PROJECT.encode())).returncode, 0)

    def test_rejects_a_helper_with_a_personal_key_even_beside_the_project_token(self):
        binary = self.helper(PROJECT.encode() + b'\x00' + PERSONAL.encode())
        result = run(PROJECT, 'packaged', binary)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('personal API key', result.stderr)
        self.assertNotIn(PERSONAL, result.stderr)

    def test_rejects_a_helper_built_with_another_project(self):
        result = run(PROJECT, 'packaged', self.helper(OTHER_PROJECT.encode()))
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(PROJECT, result.stderr)

    def test_rejects_a_helper_without_any_key_and_a_missing_helper(self):
        self.assertNotEqual(run(PROJECT, 'packaged', self.helper(b'nothing')).returncode, 0)
        self.assertNotEqual(run(PROJECT, 'packaged', '/nonexistent/helper').returncode, 0)

    def test_an_unreadable_helper_fails_closed(self):
        binary = self.helper(PROJECT.encode())
        os.chmod(binary, 0)
        try:
            if os.access(binary, os.R_OK):
                self.skipTest('running as a user who can read mode-0 files')
            result = run(PROJECT, 'packaged', binary)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(PROJECT, result.stderr)
        finally:
            os.chmod(binary, 0o644)

    def test_a_read_error_during_the_scan_fails_closed(self):
        # grep exits 2 on an I/O error; the scan must treat it as a failure,
        # never as "no personal key found". A fake grep stands in for a
        # partial read.
        binary = self.helper(PROJECT.encode())
        with tempfile.TemporaryDirectory() as bin_dir:
            fake = Path(bin_dir, 'grep')
            fake.write_text('#!/bin/sh\necho "grep: read error" >&2\nexit 2\n')
            fake.chmod(0o755)
            env = {'PATH': f'{bin_dir}:{os.environ["PATH"]}', 'AGENT_RELAY_POSTHOG_API_KEY': PROJECT}
            result = subprocess.run(['bash', str(SCRIPT), 'packaged', binary], env=env,
                                    capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('could not read', result.stderr)
        self.assertNotIn(PROJECT, result.stderr)

    def test_an_empty_helper_fails_closed(self):
        binary = self.helper(b'')
        Path(binary).write_bytes(b'')
        self.assertNotEqual(run(PROJECT, 'packaged', binary).returncode, 0)

    def test_checks_every_architecture(self):
        good, bad = self.helper(PROJECT.encode()), self.helper(OTHER_PROJECT.encode())
        self.assertEqual(run(PROJECT, 'packaged', good, good).returncode, 0)
        self.assertNotEqual(run(PROJECT, 'packaged', good, bad).returncode, 0)


if __name__ == '__main__':
    unittest.main()

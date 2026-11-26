#!/usr/bin/env python3
"""check-telemetry-key.sh accepts only a PostHog project token. Synthetic keys only."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / 'check-telemetry-key.sh'
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
        for key in (None, '', PERSONAL, 'phc_short', 'sk_' + 'A' * 40, PROJECT + ' '):
            with self.subTest(key=key and key[:4]):
                result = run(key, 'source')
                self.assertNotEqual(result.returncode, 0)

    def test_never_prints_the_key(self):
        for key in (PROJECT, PERSONAL):
            result = run(key, 'source')
            self.assertNotIn(key, result.stdout + result.stderr)
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

    def test_checks_every_architecture(self):
        good, bad = self.helper(PROJECT.encode()), self.helper(OTHER_PROJECT.encode())
        self.assertEqual(run(PROJECT, 'packaged', good, good).returncode, 0)
        self.assertNotEqual(run(PROJECT, 'packaged', good, bad).returncode, 0)


if __name__ == '__main__':
    unittest.main()

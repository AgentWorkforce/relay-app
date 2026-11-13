#!/usr/bin/env python3
"""Exercise reloads with fake build/sign tools; never touches a running app."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class ReloadTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scripts = self.root / 'scripts'
        scripts.mkdir()
        source = Path(__file__).resolve().parent
        shutil.copy(source / 'site-origin.sh', scripts)
        self.app = self.root / 'Agent Relay Dev.app'
        for directory in ['MacOS', 'Helpers', 'Resources']:
            (self.app / 'Contents' / directory).mkdir(parents=True)
        self.helper = self.app / 'Contents/Helpers/Agent Relay Dev Helper'
        self.helper.write_text('original signed helper')
        self.collector = self.helper.with_name(self.helper.name + ' (Sessions)')
        os.link(self.helper, self.collector)
        self.executable = self.app / 'Contents/MacOS/RelayDesktop'
        self.executable.write_text('original UI')
        self.resources = self.app / 'Contents/Resources'
        (self.resources / 'site-url.txt').write_text('http://localhost:3100\n')
        for name in ['probe-version.txt', 'agent-relay-probe.sha256']:
            (self.resources / name).write_text('original metadata\n')
        binary = self.root / 'swift-build'
        binary.mkdir()
        (binary / 'RelayDesktop').write_text('new UI')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.tool('swift', '''echo swift >> "$TEST_ROOT/calls"
[ "${FAIL_SWIFT:-0}" = 0 ] || exit 1
case "$*" in *--show-bin-path*) echo "$TEST_ROOT/swift-build";; esac''')
        self.tool('cargo', '''echo cargo >> "$TEST_ROOT/calls"
mkdir -p "$TEST_ROOT/target/debug"
cp "$TEST_ROOT/bin/probe" "$TEST_ROOT/target/debug/agent-relay-desktop-probe"''')
        self.tool('probe', '''case "$*" in
  --version) echo 'agent-relay-probe 0.0.0';;
  *) echo '--site-url [default: http://localhost:3100]';;
esac''')
        self.tool('codesign', '''echo "codesign $*" >> "$TEST_ROOT/calls"''')
        self.tool('PlistBuddy', '''case "$*" in
  *CFBundleName*) echo 'Agent Relay Dev';;
  *) echo 'agentrelay-dev';;
esac''')
        self.tool('uname', 'echo arm64')
        (scripts / 'check-probe-bridge.sh').write_text('exit 0\n')
        text = (source / 'dev-reload.sh').read_text().replace('/usr/libexec/PlistBuddy', str(self.bin / 'PlistBuddy'))
        (scripts / 'dev-reload.sh').write_text(text)
        self.env = {**os.environ, 'PATH': str(self.bin) + ':' + os.environ['PATH'], 'TEST_ROOT': str(self.root), 'HOME': str(self.root / 'home')}
        for key in ['AGENT_RELAY_SITE_URL', 'AGENT_RELAY_URL_SCHEME']:
            self.env.pop(key, None)

    def tool(self, name, contents):
        file = self.bin / name
        file.write_text('#!/bin/bash\nset -e\n' + contents + '\n')
        file.chmod(0o755)

    def run_reload(self, *args):
        return subprocess.run(['bash', str(self.root / 'scripts/dev-reload.sh'), '--app', str(self.app), '--arch', 'arm64', *args], env=self.env, text=True, capture_output=True)

    def test_swift_only_preserves_signed_helper_and_metadata(self):
        before = {file: (file.read_bytes(), file.stat().st_ino) for file in [self.helper, self.collector, self.resources / 'probe-version.txt', self.resources / 'agent-relay-probe.sha256']}
        result = self.run_reload('--swift-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.executable.read_text(), 'new UI')
        for file, snapshot in before.items():
            self.assertEqual((file.read_bytes(), file.stat().st_ino), snapshot)
        calls = (self.root / 'calls').read_text()
        self.assertNotIn('cargo', calls)
        self.assertNotIn('--identifier agent-relay-probe', calls)
        self.assertIn('codesign --force --sign - ' + str(self.app), calls)

    def test_default_reload_still_rebuilds_helper(self):
        result = self.run_reload()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = (self.root / 'calls').read_text()
        self.assertIn('cargo', calls)
        self.assertIn('codesign --force --identifier agent-relay-probe --sign - ', calls)
        self.assertEqual(self.helper.read_bytes(), (self.bin / 'probe').read_bytes())
        self.assertEqual(self.helper.stat().st_ino, self.collector.stat().st_ino)
        self.assertEqual((self.resources / 'probe-version.txt').read_text(), '0.0.0\n')
        digest = hashlib.sha256((self.bin / 'probe').read_bytes()).hexdigest()
        self.assertEqual((self.resources / 'agent-relay-probe.sha256').read_text(), f'{digest}  agent-relay-probe\n')

    def test_failed_swift_build_leaves_bundle_unchanged(self):
        self.env['FAIL_SWIFT'] = '1'
        for arguments in [(), ('--swift-only',)]:
            with self.subTest(arguments=arguments):
                (self.root / 'calls').write_text('')
                result = self.run_reload(*arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('cargo', (self.root / 'calls').read_text())
                self.assertEqual(self.executable.read_text(), 'original UI')
                self.assertEqual(self.helper.read_text(), 'original signed helper')

    def test_swift_only_requires_bundled_helper(self):
        self.helper.unlink()
        result = self.run_reload('--swift-only')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('no bundled helper', result.stderr)


if __name__ == '__main__':
    unittest.main()

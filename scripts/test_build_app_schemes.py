"""Exercise the real builder's URL-scheme command with the host's real plutil."""
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent

class AppSchemeSourceTests(unittest.TestCase):
    def test_builder_replaces_the_whole_scheme_array_on_every_platform(self):
        commands = [line for line in (SCRIPTS / "build-app.sh").read_text().splitlines()
                    if line.startswith("plutil -replace CFBundleURLTypes.0.CFBundleURLSchemes")]
        self.assertEqual(len(commands), 1)
        self.assertIn(r'CFBundleURLSchemes -json "[\"$WANT_SCHEME\"]"', commands[0])
        self.assertNotIn('CFBundleURLSchemes.0', commands[0])

@unittest.skipUnless(sys.platform == "darwin", "requires the real macOS plutil")
class AppSchemeTests(unittest.TestCase):
    def test_builder_replaces_the_whole_scheme_array(self):
        commands = [line for line in (SCRIPTS / "build-app.sh").read_text().splitlines()
                    if line.startswith("plutil -replace CFBundleURLTypes.0.CFBundleURLSchemes")]
        self.assertEqual(len(commands), 1)
        for scheme in ("agentrelay", "agentrelay-dev", "relay-site-test"):
            with self.subTest(scheme=scheme), tempfile.TemporaryDirectory() as tmp:
                app = Path(tmp) / "Candidate.app"
                info = app / "Contents/Info.plist"
                info.parent.mkdir(parents=True)
                original = {"CFBundleIdentifier": "com.example.test", "CFBundleURLTypes": [{
                    "CFBundleURLName": "A connection", "CFBundleURLSchemes": ["agentrelay", "obsolete-scheme"]}]}
                info.write_bytes(plistlib.dumps(original))
                for _ in range(2):
                    subprocess.run(["/bin/bash", "-c", commands[0]], check=True, capture_output=True,
                                   env=dict(os.environ, APP_BUNDLE=str(app), WANT_SCHEME=scheme))
                    got = plistlib.loads(info.read_bytes())
                    self.assertEqual(got["CFBundleURLTypes"][0]["CFBundleURLSchemes"], [scheme])
                    self.assertEqual(got["CFBundleIdentifier"], original["CFBundleIdentifier"])
                    self.assertEqual(got["CFBundleURLTypes"][0]["CFBundleURLName"], "A connection")

if __name__ == "__main__":
    unittest.main()

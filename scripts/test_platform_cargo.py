"""Exercise the real test wrapper's trust boundary with observable child processes."""
from pathlib import Path
import json
import os
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("test-platform-cargo.sh")
RUNNER_NAME = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER"
STRIPPING_RUNNER = "env -u SSL_CERT_FILE -u SSL_CERT_DIR"
UNSET = {"SSL_CERT_FILE": False, "SSL_CERT_DIR": False}


class TestPlatformCargo(unittest.TestCase):
    def run_case(self, *, inherited=None, linux=True, child_override=False,
                 host="x86_64-unknown-linux-gnu"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            python = os.sys.executable
            programs = {
                "uname": "#!/bin/sh\nprintf '%s\\n' " + ("Linux" if linux else "Darwin") + "\n",
                "rustc": f"#!/bin/sh\nprintf '%s\\n' 'host: {host}'\n",
                "cargo": f"#!{python}\nLINUX = {linux!r}\nRUNNER_NAME = {RUNNER_NAME!r}\n" + """
import json, os, pathlib, shlex, subprocess, sys
runner = os.environ.get(RUNNER_NAME)
pathlib.Path(os.environ['OBSERVED']).write_text(json.dumps({'args': sys.argv[1:], 'runner': runner}))
if LINUX:
    # Model Cargo's libgit2 additions, made whether or not a runner is set.
    os.environ['SSL_CERT_FILE'] = 'synthetic-cargo-probe'
    os.environ['SSL_CERT_DIR'] = 'synthetic-cargo-probe'
subprocess.run((shlex.split(runner) if runner else []) + [os.environ['CHILD']], check=True)
""",
                "child": f"#!{python}\n" + """
import json, os
before = {k: bool(os.environ.get(k)) for k in ['SSL_CERT_FILE', 'SSL_CERT_DIR']}
if os.environ.get('CHILD_OVERRIDE') == '1':
    os.environ['SSL_CERT_FILE'] = 'synthetic-refusal-test'
print(json.dumps({'before': before, 'after': bool(os.environ.get('SSL_CERT_FILE'))}))
""",
            }
            for name, text in programs.items():
                path = root / name
                path.write_text(text)
                path.chmod(0o700)
            env = dict(os.environ)
            for key in ("SSL_CERT_FILE", "SSL_CERT_DIR", RUNNER_NAME):
                env.pop(key, None)
            env.update(PATH=str(root) + ":" + os.environ["PATH"], CHILD=str(root / "child"),
                       OBSERVED=str(root / "observed"), CHILD_OVERRIDE="1" if child_override else "0")
            env.update(inherited or {})
            result = subprocess.run(["bash", str(SCRIPT), "--locked", "--lib"], env=env,
                                    capture_output=True, text=True)
            observed_path = root / "observed"
            observed = json.loads(observed_path.read_text()) if observed_path.exists() else None
            return result, observed

    def test_only_cargo_added_file_roots_are_removed(self):
        result, observed = self.run_case()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNotNone(observed)
        self.assertEqual(observed["args"], ["test", "--locked", "--lib"])
        self.assertEqual(observed["runner"], STRIPPING_RUNNER)
        self.assertEqual(json.loads(result.stdout)["before"], UNSET)

    def test_inherited_overrides_refuse_before_cargo_without_disclosing_values(self):
        for key in ("SSL_CERT_FILE", "SSL_CERT_DIR"):
            with self.subTest(key=key):
                result, observed = self.run_case(inherited={key: "never-disclose-this-value"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIsNone(observed)
                self.assertNotIn("never-disclose-this-value", result.stdout + result.stderr)

    def test_macos_also_refuses_inherited_overrides_without_disclosing_values(self):
        for key in ("SSL_CERT_FILE", "SSL_CERT_DIR"):
            with self.subTest(key=key):
                result, observed = self.run_case(linux=False, inherited={key: "never-disclose-this-value"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIsNone(observed)
                self.assertIn(f"inherited {key}", result.stderr)
                self.assertNotIn("never-disclose-this-value", result.stdout + result.stderr)

    def test_deliberate_negative_child_override_is_preserved(self):
        result, observed = self.run_case(child_override=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(observed["runner"], STRIPPING_RUNNER)
        output = json.loads(result.stdout)
        # The runner removed Cargo's additions, and the child's own override survives.
        self.assertEqual(output["before"], UNSET)
        self.assertTrue(output["after"])

    def test_an_existing_runner_is_not_overwritten(self):
        result, observed = self.run_case(inherited={RUNNER_NAME: "existing-runner"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIsNone(observed)

    def test_unsupported_linux_host_refuses_before_cargo(self):
        result, observed = self.run_case(host="riscv64gc-unknown-linux-gnu")
        self.assertNotEqual(result.returncode, 0)
        self.assertIsNone(observed)
        self.assertIn("Unsupported Linux test host", result.stderr)

    def test_macos_keeps_the_ordinary_cargo_path(self):
        result, observed = self.run_case(linux=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNotNone(observed)
        self.assertEqual(observed["args"], ["test", "--locked", "--lib"])
        self.assertIsNone(observed["runner"])
        output = json.loads(result.stdout)
        self.assertEqual(output["before"], UNSET)
        self.assertFalse(output["after"])


if __name__ == "__main__":
    unittest.main()
